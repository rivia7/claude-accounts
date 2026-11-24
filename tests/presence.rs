//! 连接设备（presence）：Team 的同步服务器的验签、名单、状态；客户端的上报、cca devices、共享记下的 API key、折合金额的同步。
//! 真的 ssh-keygen、真的 TLS 服务器（127.0.0.1，随机端口）；客户端是 cca 的辅助程序（cca-core，各台设备各一个临时 HOME），不联网。
//! 要 ssh-keygen、openssl、curl（和原来一样：没有就是失败，不跳过）。
//!
//! 原来是 tests/presence.py。服务器用哪一份跑照 CCA_TEST_SERVER（kit::server）：这里只经网络和它打交道。夹具（服务器、Team、
//! 设备、假命令）在 kit::fleet、kit::pki，别的几份测试也用。
//!
//! 服务器进程里面的事不在这里，归服务器自己的单元测试：Store 的各个上限、邀请和成员的规矩、以前的版本留下的折合金额启动时
//! 清掉（cli/src/server/store/tests.rs），验签和 ssh-keygen 对（cli/src/server/sshsig.rs），请求里各项的形状（parse_enroll、
//! parse_team_docs、parse_blob：cli/src/server/wire.rs）；各处留了一行说明。
//! 服务器里的几个上限在这里写成了数（旁边注明是它的哪个常量）：改了服务器的要一起改。

use kit::fleet::{self, CliSession, Device, Server, ServerOpts, ServerProc, SignedCalls, Team, DROPPED, SESSION_PID, WAIT};
use kit::fsx::{self, wait_for, TempDir};
use kit::harness::{Ctx, Module};
use kit::http::Request;
use kit::json::{self, Obj, Style, Value};
use kit::proc::Cmd;
use kit::sandbox::truthy;
use kit::{case, crypto, list, obj, pki, spec};
use std::collections::BTreeMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "presence",
    src: include_str!("presence.rs"),
    default: true,
    tests: &[
        case!(health_and_self_test),
        case!(old_ops),
        case!(states),
        case!(auth),
        case!(rate_limit_after_verify),
        case!(connection_caps),
        case!(revoke),
        case!(persist),
        case!(keys),
        case!(cost),
        case!(flow),
        case!(records_lost),
        case!(cli),
        case!(behind_proxy),
        case!(identity),
        case!(scan_cli_idle_from_tty),
        case!(post_and_devices),
        case!(agent_backoff),
        case!(keyshare_crypto),
        case!(keyshare),
        case!(keyshare_before_presence),
        case!(keyshare_bad_files),
        case!(keyshare_force_waits),
        case!(cost_many_accounts),
        case!(cost_sync),
    ],
};

// 服务器（server/cca-presence.py、辅助程序的 team-server）里的几个上限：测试照着它们发请求，两边要一起改
/// MAX_CONN_IP：同一个 IP 同时几个连接
const MAX_CONN_IP: usize = 8;
/// MAX_BODY：一般的请求最多这么大
const MAX_BODY: usize = 64 * 1024;
/// MAX_SYNC_BODY：交换密文、管理员存配置的请求一次最多这么大
const MAX_SYNC_BODY: usize = 1 << 20;
/// TEAM_OPS：只有 Team 的服务器有的 op（管理员的五个、设备的三个）
const TEAM_OPS: [&str; 8] =
    ["team_enroll", "team_invite", "team_put", "team_members", "team_revoke", "team_join", "team_get", "team_leave"];

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 几个用例共用的东西：头一个用到的用例建，模块完了收掉。用例拿着它的时候别的用例拿不到
/// （模块里的用例本来就是一个接一个跑的）
struct Held<T: 'static>(MutexGuard<'static, Option<T>>);

impl<T> Deref for Held<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_ref().expect("建好了才交出来")
    }
}

impl<T> DerefMut for Held<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().expect("建好了才交出来")
    }
}

/// 拿共用的那一份；还没有就建（make），并登记模块完了收掉（服务器停掉、临时目录删掉）。收的时候它还被卡住的用例拿着
/// （用例超时）就不等：起过的进程运行器会停掉
fn held<T: Send + 'static>(t: &mut Ctx, slot: &'static Mutex<Option<T>>, make: impl FnOnce() -> T) -> Held<T> {
    let mut guard = locked(slot);
    if guard.is_none() {
        *guard = Some(make());
        t.defer_module(move || {
            if let Ok(mut slot) = slot.try_lock() {
                drop(slot.take());
            }
        });
    }
    Held(guard)
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn p(path: &Path) -> &str {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display()))
}

/// haystack 里有 needle（assertIn）
#[track_caller]
fn assert_in(needle: &str, haystack: &str) {
    assert!(haystack.contains(needle), "没有 {needle:?}：\n{haystack}");
}

/// `json.dumps(v)` 默认的样子
fn dumps(v: &Value) -> String {
    json::dumps(v, &Style::default())
}

/// 16 个随机字节的十六进制：账号摘要的样子（`secrets.token_hex(16)`）
fn hex16() -> String {
    crypto::token_hex(16)
}

/// 一份密文的样子（服务器不懂里面是什么，只查形状和长度）：nonce 16 字节、密文 n 字节、认证码 32 字节，都是 base64
fn blob(n: usize) -> Value {
    obj! { "n" => crypto::b64(&crypto::random(16)), "c" => crypto::b64(&crypto::random(n)), "t" => crypto::b64(&crypto::random(32)) }
}

/// `dict(base, key=value)`：抄一份、换掉（或者添上）一项
fn with(base: &Value, key: &str, value: impl Into<Value>) -> Value {
    let mut o = base.obj().clone();
    o.insert(key, value);
    Value::Obj(o)
}

/// 上报里一个账号的那一项：cli 是各个命令行会话的 [开了多久, 闲了多久]，app 是桌面 App 的（null：没开）
fn unit(cli: &[[i64; 2]], app: Value) -> Value {
    let cli: Vec<Value> = cli.iter().map(|s| list![s[0], s[1]]).collect();
    obj! { "cli" => cli, "app" => app }
}

/// 对象的键，照它里面的先后（`list(d)`）
fn key_list(v: &Value) -> Vec<&str> {
    v.obj().keys().collect()
}

/// 对象的键，排好（`sorted(d)`）
fn sorted_keys(v: &Value) -> Vec<&str> {
    let mut out = key_list(v);
    out.sort_unstable();
    out
}

/// 数组里的各行按名字（`{r['name']: r for r in rows}`）
fn by_name(rows: &Value) -> BTreeMap<String, Value> {
    rows.list().iter().map(|r| (r["name"].str().to_string(), r.clone())).collect()
}

/// 各行的密文按名字，没带密文的是 None（`{x['name']: x.get('blob') for x in rows}`）
fn blobs_by_name(rows: &Value) -> BTreeMap<String, Option<Value>> {
    rows.list().iter().map(|r| (r["name"].str().to_string(), r.get("blob").cloned())).collect()
}

/// 数组里正好一项（`[r] = rows`）
#[track_caller]
fn only(rows: &Value) -> &Value {
    let rows = rows.list();
    assert_eq!(rows.len(), 1, "该正好一项：{rows:?}");
    &rows[0]
}

// ==== 服务器：设备的请求（上报、查看、记下的 API key、折合金额）。名单由管理员拿根密钥登记、移除（team_enroll、
// team_revoke），Team 的那几个 op 本身在下面「Team 的同步服务器」的那几个用例里（flow、records_lost、cli、behind_proxy） ====

/// 两台设备在 Team 里的编号
const DEV: &str = "0a0a0a0a0a0a0a0a";
const DEV2: &str = "0b0b0b0b0b0b0b0b";

/// 这一组用例共用的：一个目录（证书、根密钥、三把设备的密钥 dev、dev2、evil）和照它起的服务器
struct ServerFx {
    srv: Server,
    rootpub: String,
    devpub: String,
    dev2pub: String,
    evilpub: String,
    d: PathBuf,
    /// 放在最后：服务器先停，目录再删
    _tmp: TempDir,
}

static SERVER_FX: Mutex<Option<ServerFx>> = Mutex::new(None);

/// 一个请求是谁签的、怎么签的（原来 call 的那几个带默认值的参数）
#[derive(Clone, Copy)]
struct By<'a> {
    /// 拿哪把私钥签（d 下的文件名：dev、dev2、evil、root）
    key: &'a str,
    /// 签名的命名空间
    ns: &'a str,
    /// 请求里写谁的指纹（None：就是 key 的）
    pubkey: Option<&'a str>,
    /// 签完再改 body
    tamper: bool,
}

/// 平常的：dev 签的
const BY_DEV: By = By { key: "dev", ns: "cca-presence", pubkey: None, tamper: false };
const BY_DEV2: By = By { key: "dev2", ..BY_DEV };
const BY_EVIL: By = By { key: "evil", ..BY_DEV };
const BY_ROOT: By = By { key: "root", ..BY_DEV };

fn server_fx(t: &mut Ctx) -> Held<ServerFx> {
    held(t, &SERVER_FX, || {
        let tmp = TempDir::new("cca-presence-test-");
        let d = tmp.path().to_path_buf();
        pki::make_cert(&d);
        let rootpub = pki::keygen_as(d.join("root"), "test-root");
        let (devpub, dev2pub, evilpub) = (pki::keygen(d.join("dev")), pki::keygen(d.join("dev2")), pki::keygen(d.join("evil")));
        let srv = Server::start(&d, &rootpub);
        ServerFx { srv, rootpub, devpub, dev2pub, evilpub, d, _tmp: tmp }
    })
}

/// 公钥的指纹（服务器的 key_fp；这里照文档自己算：base64 那一段解出来的字节的 SHA-256，base64 不带 =）
fn fp(pubkey: &str) -> String {
    spec::fp_of_line(pubkey)
}

impl ServerFx {
    /// 重起服务器（状态都在文件里，接着用）：限流的配额从头算
    fn restart(&mut self) {
        self.srv.stop();
        self.srv = Server::start(&self.d, &self.rootpub);
    }

    fn pubkey(&self, key: &str) -> &str {
        match key {
            "root" => &self.rootpub,
            "dev" => &self.devpub,
            "dev2" => &self.dev2pub,
            "evil" => &self.evilpub,
            _ => panic!("没有 {key} 这把密钥"),
        }
    }

    fn call(&self, op: &str, by: By, extra: &Value) -> (u16, Value) {
        let key_fp = fp(by.pubkey.unwrap_or_else(|| self.pubkey(by.key)));
        fleet::signed_call(&self.srv, op, &key_fp, &self.d.join(by.key), by.ns, by.tamper, extra)
    }

    /// 管理员登记这几台设备：(编号, 公钥, 名字)
    fn enroll(&self, devs: &[(&str, &str, &str)]) -> Value {
        let mut devices = Obj::new();
        for (dev, pubkey, name) in devs {
            devices.insert(*dev, obj! { "pub" => *pubkey, "name" => *name });
        }
        let (st, j) = self.call("team_enroll", BY_ROOT, &obj! { "devices" => devices });
        assert_eq!(st, 200, "{j}");
        j
    }

    fn both(&self) -> Value {
        self.enroll(&[(DEV, &self.devpub, "alice-mbp"), (DEV2, &self.dev2pub, "bob-mini")])
    }

    fn revoke(&self, dev: &str) {
        let (st, j) = self.call("team_revoke", BY_ROOT, &obj! { "dev" => dev });
        assert_eq!(st, 200, "{j}");
    }

    /// 问这个账号在各台设备上的状态（平常是 dev2 问）：({设备名: 那一行}, 整个回复)
    fn view_by(&self, aid: &str, key: &str) -> (BTreeMap<String, Value>, Value) {
        let (st, j) = self.call("view", By { key, ..BY_DEV }, &obj! { "accounts" => list![aid] });
        assert_eq!(st, 200, "{j}");
        (by_name(&j["accounts"][aid]), j)
    }

    fn view(&self, aid: &str) -> BTreeMap<String, Value> {
        self.view_by(aid, "dev2").0
    }
}

fn health_and_self_test(t: &mut Ctx) {
    let fx = server_fx(t);
    let (st, j) = fx.srv.get("/v1/health");
    assert_eq!(st, 200);
    assert!(truthy(&j["ok"]) && truthy(&j["sig_ok"]));
    assert_eq!(
        (j["mode"].str(), j["seq"].i64(), j["team"].str()),
        ("team", 0, spec::team_id(&fx.rootpub).as_str()),
        "不写 --mode 也是 Team 的服务器"
    );
    let pin = pki::openssl_pin(fx.d.join("cert.pem"));
    let shape = regex::Regex::new(r"^sha256//[A-Za-z0-9+/]{43}=$").expect("写死的正则");
    assert!(shape.is_match(&pin), "P-256 证书的公钥指纹（设备钉住的 pins）：{pin}");
}

/// 以前的维护者的服务器的设备清单（/v1/machines，发布密钥签的）、3.5.3 以前的自己的同步服务器的 op：都不认（404）
fn old_ops(t: &mut Ctx) {
    let fx = server_fx(t);
    fx.both();
    for key in ["root", "dev"] {
        let doc = dumps(
            &obj! { "version" => 1, "seq" => 1, "machines" => list![obj! { "key" => fx.evilpub.as_str(), "name" => "evil" }] },
        );
        let sig = pki::sign(fx.d.join(key), "cca-devices", doc.as_bytes());
        assert_eq!(fx.srv.req("POST", "/v1/machines", doc.as_bytes(), Some(&sig), &[]).0, 404, "{key}");
        let by = By { key, ..BY_DEV };
        assert_eq!(fx.call("machines", by, &obj! { "version" => 1, "seq" => 1, "machines" => list![] }).0, 404, "{key}");
    }
    for op in ["join", "members", "relink", "remove"] {
        assert_eq!(fx.srv.req("POST", &format!("/v1/{op}"), b"{}", Some(b"sig"), &[]).0, 404, "{op}");
    }
    assert_eq!(fx.call("report", BY_EVIL, &obj! { "units" => obj! {} }).0, 403, "没登记上");
}

fn states(t: &mut Ctx) {
    let fx = server_fx(t);
    fx.both();
    let aid = hex16();
    let report = |by: By, units: Value, end: Option<Value>| {
        let mut extra = obj! { "units" => units };
        if let Some(end) = end {
            extra.as_obj_mut().expect("对象").insert("end", end);
        }
        fx.call("report", by, &extra)
    };
    let one = |aid: &str, cli: &[[i64; 2]], app: Value| obj! { aid => unit(cli, app) };
    let (st, _) = report(BY_DEV, one(&aid, &[[100, 1], [50, 9]], Value::Null), Some(Value::Null));
    assert_eq!(st, 200);
    let (rows, j) = fx.view_by(&aid, "dev2");
    let r = &rows["alice-mbp"];
    assert_eq!((r["state"].str(), r["cli"].i64(), r["app"].bool()), ("in_use", 2, false));
    assert!((99.0..=101.0).contains(&r["since_age"].f64()) && r["idle_age"].f64() <= 2.0 && r["used_age"].f64() <= 2.0, "{r}");
    assert_eq!(j["me_name"].str(), "bob-mini", "问的那台设备叫什么");
    report(BY_DEV, one(&aid, &[[200, 10]], list![30, 5]), None);
    let r = &fx.view(&aid)["alice-mbp"];
    assert_eq!((r["state"].str(), r["cli"].i64(), r["app"].bool()), ("idle", 1, true), "都超过 3 秒没动静：闲置");
    assert!((4.0..=6.0).contains(&r["idle_age"].f64()), "{r}");
    report(BY_DEV, obj! {}, Some(Value::from("none")));
    let r = &fx.view(&aid)["alice-mbp"];
    assert_eq!((r["state"].str(), r["cli"].i64(), r["app"].bool()), ("none", 0, false), "会话都关了：没在用，留着上次使用");
    assert!(!r["used_age"].is_null());
    // 这里测的就是时间过去（ttl 是 2 秒）
    std::thread::sleep(Duration::from_millis(2300));
    assert_eq!(fx.view(&aid)["alice-mbp"]["state"].str(), "none", "报过“什么都没开了”：不会因为没有心跳变成离线");
    report(BY_DEV, one(&aid, &[[5, 0]], Value::Null), None);
    assert_eq!(fx.view(&aid)["alice-mbp"]["state"].str(), "in_use");
    std::thread::sleep(Duration::from_millis(2300));
    let r = &fx.view(&aid)["alice-mbp"];
    assert_eq!((r["state"].str(), r["cli"].i64()), ("offline", 1), "开着会话却 ttl 秒没心跳：离线（可能睡眠、断网、被杀掉）");
    let other_aid = hex16();
    report(BY_DEV, one(&aid, &[[5, 0]], Value::Null), None);
    report(BY_DEV, one(&aid, &[[5, 0]], Value::Null), Some(Value::from("down")));
    assert_eq!(fx.view(&aid)["alice-mbp"]["state"].str(), "offline", "上报进程被停掉：开着会话的账号马上算离线");
    report(BY_DEV, one(&other_aid, &[[5, 0]], Value::Null), None);
    report(BY_DEV, one(&aid, &[[5, 0]], Value::Null), Some(Value::from("down")));
    assert_eq!(fx.view(&other_aid)["alice-mbp"]["state"].str(), "none", "没开会话的账号不跟着算离线");
    let other = hex16();
    report(BY_DEV2, one(&other, &[[5, 0]], Value::Null), None);
    assert!(!fx.view(&aid).contains_key("bob-mini"), "别的账号的会话不出现在这个账号下");
}

fn auth(t: &mut Ctx) {
    let fx = server_fx(t);
    fx.both();
    let aid = hex16();
    let u = obj! { "units" => obj! { aid.as_str() => obj! { "cli" => list![list![1, 0]], "app" => Value::Null } } };
    let (st, j) = fx.call("report", BY_EVIL, &u);
    assert_eq!((st, j["error"].str()), (403, "unknown_device"), "不在名单里的设备");
    assert_eq!(fx.call("report", By { tamper: true, ..BY_DEV }, &u), (403, obj! { "error" => "denied" }), "签过之后又改过的 body");
    assert_eq!(fx.call("report", By { ns: "cca-devices", ..BY_DEV }, &u).0, 403, "命名空间不对");
    assert_eq!(fx.call("report", By { pubkey: Some(&fx.dev2pub), ..BY_DEV }, &u).0, 403, "用自己的 key 冒充别的设备");
    assert_eq!(fx.call("report", BY_ROOT, &u).0, 403, "根密钥不是设备");
    let (st, j) = fx.call("report", BY_DEV, &with(&u, "ts", fleet::now() - 900.0));
    assert_eq!((st, j["error"].str()), (400, "clock"));
    assert!(j.get("now").is_some(), "{j}");
    let body = fleet::call_body("report", &fp(&fx.devpub), &obj! { "units" => obj! {}, "end" => "none" });
    let sig = pki::sign(fx.d.join("dev"), "cca-presence", &body);
    assert_eq!(fx.srv.req("POST", "/v1/report", &body, Some(&sig), &[]).0, 200);
    let again = fx.srv.req("POST", "/v1/report", &body, Some(&sig), &[]).1;
    assert_eq!(again.get("error").and_then(Value::as_str), Some("replay"), "同一个请求再发一次");
    assert_eq!(fx.srv.req("POST", "/v1/view", &body, Some(&sig), &[]).0, 400, "签的是 report，不能拿去 view");
    assert_eq!(fx.call("report", BY_DEV, &obj! { "units" => obj! { "NOT-AN-ID" => obj! { "cli" => list![] } } }).0, 400);
    let with_end = obj! { "units" => obj! { aid.as_str() => obj! { "cli" => list![list![1, 0]] } }, "end" => "none" };
    assert_eq!(fx.call("report", BY_DEV, &with_end).0, 400, "end=none 却还有会话");
    let st = fx.srv.req("POST", "/v1/report", &vec![b'x'; MAX_BODY + 1], Some(b"sig"), &[]).0;
    assert!(matches!(st, 413 | DROPPED), "太大的 body 不读：{st}");
    assert_eq!(fx.srv.req("POST", "/v1/report", b"{}", None, &[]).0, 400, "没有签名");
}

/// 别的设备拿这台的指纹乱发（签名不对）：扣不到这台的配额
fn rate_limit_after_verify(t: &mut Ctx) {
    let fx = server_fx(t);
    fx.both();
    for _ in 0..40 {
        assert_eq!(fx.call("report", By { pubkey: Some(&fx.devpub), ..BY_DEV2 }, &obj! { "units" => obj! {} }).0, 403);
    }
    assert_eq!(fx.call("report", BY_DEV, &obj! { "units" => obj! {}, "end" => "none" }).0, 200);
}

/// 同一个 IP 最多 8 个连接：不说话的连接占不满服务器，关掉之后照常
fn connection_caps(t: &mut Ctx) {
    let fx = server_fx(t);
    let addr = SocketAddr::from(([127, 0, 0, 1], fx.srv.port));
    let connect = || TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap_or_else(|e| panic!("连不上 {addr}：{e}"));
    let socks: Vec<TcpStream> = (0..MAX_CONN_IP).map(|_| connect()).collect();
    // 等服务器把这 8 个都接进来（它那边没有看得到的记号，照原来等 0.3 秒）
    std::thread::sleep(Duration::from_millis(300));
    let mut extra = connect();
    extra.set_read_timeout(Some(Duration::from_secs(3))).expect("设时限");
    let mut one = [0u8; 1];
    match extra.read(&mut one) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("第 9 个马上被断开：{other:?}"),
    }
    drop(extra);
    drop(socks);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fx.srv.get("/v1/health").0, 200);
}

fn revoke(t: &mut Ctx) {
    let fx = server_fx(t);
    fx.both();
    let aid = hex16();
    fx.call("report", BY_DEV, &obj! { "units" => obj! { aid.as_str() => unit(&[[1, 0]], Value::Null) } });
    assert!(fx.view(&aid).contains_key("alice-mbp"));
    fx.revoke(DEV);
    assert!(!fx.view(&aid).contains_key("alice-mbp"), "移除的设备：记录一起删掉");
    assert_eq!(fx.call("report", BY_DEV, &obj! { "units" => obj! {} }).0, 403);
}

fn persist(t: &mut Ctx) {
    let mut fx = server_fx(t);
    fx.both();
    let aid = hex16();
    fx.call("report", BY_DEV, &obj! { "units" => obj! { aid.as_str() => unit(&[[60, 0]], Value::Null) } });
    fx.restart();
    assert_eq!(fx.view(&aid)["alice-mbp"]["state"].str(), "in_use", "重启后照样认得名单和会话");
    assert_eq!(fsx::mode(fx.d.join("state.json")), 0o600);
}

/// API key 的密文：设备传上来的整份换掉旧的，问的时候回现在的（谁传的、多久以前）；服务器不懂里面是什么，只查形状和长度。
/// （“存满了”要把上限换小才测得了：cli/src/server/store/tests.rs 的 key_cap）
fn keys(t: &mut Ctx) {
    let mut fx = server_fx(t);
    fx.restart(); // 限流的配额从头算（前面的用例用掉了不少）
    fx.both();
    let (a1, a2) = (hex16(), hex16());
    let (a1, a2) = (a1.as_str(), a2.as_str());
    let b1 = blob(256);
    let (st, j) = fx.call("keys", BY_DEV, &obj! { "put" => obj! { a1 => &b1 } });
    assert_eq!((st, j["me_name"].str(), key_list(&j["keys"])), (200, "alice-mbp", vec![a1]), "{j}");
    let r = &j["keys"][a1];
    assert_eq!((&r["blob"], r["me"].bool(), r["name"].str()), (&b1, true, "alice-mbp"));
    let (st, j) = fx.call("keys", BY_DEV2, &obj! { "get" => list![a1, a2, a1] });
    assert_eq!((st, key_list(&j["keys"])), (200, vec![a1]), "没有的账号不回；问重复的只回一次");
    let r = &j["keys"][a1];
    assert_eq!((&r["blob"], r["me"].bool(), r["name"].str()), (&b1, false, "alice-mbp"));
    assert!(r["age"].f64() <= 2.0, "{r}");
    let b2 = blob(512);
    let (st, j) = fx.call("keys", BY_DEV2, &obj! { "put" => obj! { a1 => &b2 }, "get" => list![a2] });
    assert_eq!((st, sorted_keys(&j["keys"])), (200, vec![a1]), "传的那些也回（问的里没有的不回）");
    let (_, j) = fx.call("keys", BY_DEV, &obj! { "get" => list![a1] });
    let r = &j["keys"][a1];
    assert_eq!((&r["blob"], r["name"].str(), r["me"].bool()), (&b2, "bob-mini", false), "新传的整个换掉旧的");
    let ok = blob(256);
    let many: Vec<String> = (0..65).map(|_| hex16()).collect();
    for bad in [
        obj! { "put" => obj! { a1 => with(&ok, "x", "1") } },
        obj! { "put" => obj! { a1 => with(&ok, "n", &ok["n"].str()[..20]) } },
        obj! { "put" => obj! { a1 => with(&ok, "t", format!("{}AAAA", ok["t"].str())) } },
        obj! { "put" => obj! { a1 => with(&ok, "c", "") } },
        obj! { "put" => obj! { a1 => with(&ok, "c", "not base64!") } },
        obj! { "put" => obj! { a1 => with(&ok, "c", 1) } },
        obj! { "put" => obj! { a1 => with(&ok, "c", "A".repeat(8196)) } },
        obj! { "put" => obj! { "NOT-AN-ID" => &ok } },
        obj! { "put" => list![&ok] },
        obj! { "get" => a1 },
        obj! { "get" => list!["x"] },
        obj! { "get" => many },
        obj! { "put" => obj! { a1 => "x" } },
    ] {
        assert_eq!(fx.call("keys", BY_DEV, &bad).0, 400, "{bad}");
    }
    let mut big = Obj::new();
    for _ in 0..20 {
        big.insert(hex16(), blob(4352));
    }
    assert_eq!(fx.call("keys", BY_DEV, &obj! { "put" => big }).0, 200, "一次传 20 个最长的 key（超过心跳的 64KB）也收");
    let raw = fsx::read_text(fx.d.join("keys.json"));
    assert_eq!(fsx::mode(fx.d.join("keys.json")), 0o600);
    assert_in(b2["c"].str(), &raw);
    fx.restart();
    let (_, j) = fx.call("keys", BY_DEV, &obj! { "get" => list![a1] });
    assert_eq!(j["keys"][a1]["blob"], b2, "重启后还在");
    fx.revoke(DEV2);
    let (st, j) = fx.call("keys", BY_DEV, &obj! { "get" => list![a1] });
    let r = &j["keys"][a1];
    assert_eq!(
        (st, &r["blob"], &r["name"], r["me"].bool()),
        (200, &b2, &Value::Null, false),
        "移除了传它的设备：密文留着（是账号的，登录了这个账号的设备还要用），不再记着是它传的"
    );
    let raw = fsx::read_text(fx.d.join("keys.json"));
    assert!(!(raw.contains(&fp(&fx.dev2pub)) || raw.contains("bob-mini")), "文件里没有它的指纹、名字");
    assert_eq!(fx.call("keys", BY_DEV2, &obj! { "get" => list![a1] }).0, 403, "移除的设备不能再问");
}

/// 折合金额的密文：每个账号、每台设备一份，设备只写自己的那一份；问的时候回各台设备的（认证码没变的、自己的不带密文），
/// 顺带说哪些设备用过这个账号却没传过；移除设备时它传的删掉（以前的版本留下的，启动时删掉）；服务器只查形状和长度。
/// （各个上限要把上限换小、时间自己给才测得了：cli/src/server/store/tests.rs 的 cost_caps）
fn cost(t: &mut Ctx) {
    let mut fx = server_fx(t);
    fx.restart(); // 限流的配额从头算
    fx.both();
    let (a1, a2) = (hex16(), hex16());
    let (a1, a2) = (a1.as_str(), a2.as_str());
    let (fa, fb) = (fp(&fx.devpub), fp(&fx.dev2pub));
    let fa = fa.as_str();
    let (_, j) = fx.srv.get("/v1/health");
    assert!(j["ops"].list().iter().any(|op| op.as_str() == Some("ledger")), "health 说支持哪些 op（管理员看服务器是不是新的）");
    let old = obj! { "accounts" => obj! { a1 => obj! { "since" => Value::Null, "h" => list![list![1, 2]] } } };
    assert_eq!(fx.call("cost", BY_DEV, &old).0, 404, "早先试过的 /v1/cost（另一种格式）不认：那时的客户端照旧版服务器处理");
    let b1 = blob(2048);
    let (st, j) = fx.call("ledger", BY_DEV, &obj! { "put" => obj! { a1 => &b1 }, "get" => list![a1] });
    assert_eq!((st, j["me_name"].str(), &j["missing"]), (200, "alice-mbp", &obj! { a1 => list![] }), "{j}");
    let r = only(&j["cost"][a1]);
    assert_eq!(
        (r["fp"].str(), r["name"].str(), r["me"].bool(), r["revoked"].bool(), r["tag"].str(), r.get("blob").is_some()),
        (fa, "alice-mbp", true, false, b1["t"].str(), false),
        "自己的不回密文"
    );
    let (st, j) = fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1, a2, a1] });
    let mut both = vec![a1, a2];
    both.sort_unstable();
    assert_eq!((st, sorted_keys(&j["cost"])), (200, both), "{j}");
    assert_eq!(j["cost"][a2], list![]);
    let r = only(&j["cost"][a1]);
    assert_eq!((&r["blob"], r["me"].bool(), r["name"].str()), (&b1, false, "alice-mbp"));
    assert!(r["age"].f64() <= 2.0, "{r}");
    let (_, j) = fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1], "have" => obj! { a1 => obj! { fa => b1["t"].str() } } });
    assert!(j["cost"][a1][0].get("blob").is_none(), "认证码没变：不回密文");
    let other_tag = blob(2048);
    let (_, j) =
        fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1], "have" => obj! { a1 => obj! { fa => other_tag["t"].str() } } });
    assert_eq!(j["cost"][a1][0]["blob"], b1, "变了：回");
    let b2 = blob(4096);
    assert_eq!(fx.call("ledger", BY_DEV2, &obj! { "put" => obj! { a1 => &b2 } }).0, 200);
    let (_, j) = fx.call("ledger", BY_DEV, &obj! { "get" => list![a1] });
    let rows = by_name(&j["cost"][a1]);
    assert_eq!((&rows["bob-mini"]["blob"], rows["alice-mbp"].get("blob").is_some()), (&b2, false), "各台设备一份，互不覆盖");
    let b3 = blob(1024);
    fx.call("ledger", BY_DEV, &obj! { "put" => obj! { a1 => &b3 } });
    let (_, j) = fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1] });
    let mine_only: BTreeMap<String, Option<Value>> =
        BTreeMap::from([("alice-mbp".to_string(), Some(b3.clone())), ("bob-mini".to_string(), None)]);
    assert_eq!(blobs_by_name(&j["cost"][a1]), mine_only, "新传的换掉这台设备旧的");
    let ok = blob(2048);
    let many: Vec<String> = (0..65).map(|_| hex16()).collect();
    for bad in [
        obj! { "put" => obj! { a1 => with(&ok, "x", "1") } },
        obj! { "put" => obj! { a1 => with(&ok, "c", "A".repeat(98308)) } },
        obj! { "put" => obj! { a1 => with(&ok, "c", "not base64!") } },
        obj! { "put" => obj! { "NOT-AN-ID" => &ok } },
        obj! { "put" => list![&ok] },
        obj! { "get" => a1 },
        obj! { "get" => list!["x"] },
        obj! { "have" => list!["x"] },
        obj! { "have" => obj! { a1 => obj! { "bad-fp" => ok["t"].str() } } },
        obj! { "have" => obj! { a1 => obj! { fa => "short" } } },
        obj! { "have" => obj! { "x" => obj! {} } },
        obj! { "get" => many },
    ] {
        assert_eq!(fx.call("ledger", BY_DEV, &bad).0, 400, "{bad}");
    }
    let mut big = Obj::new();
    for _ in 0..10 {
        big.insert(hex16(), blob(70000));
    }
    assert_eq!(fx.call("ledger", BY_DEV, &obj! { "put" => big }).0, 200, "一次传 10 个最长的（超过心跳的 64KB）也收");
    // 用过这个账号（上报过）、却没传过折合金额的设备：多半是 cca 太旧
    fx.call("report", BY_DEV2, &obj! { "units" => obj! { a2 => unit(&[[100, 0]], Value::Null) }, "end" => Value::Null });
    let (_, j) = fx.call("ledger", BY_DEV, &obj! { "get" => list![a2] });
    let names: Vec<&str> = j["missing"][a2].list().iter().map(|m| m["name"].str()).collect();
    assert_eq!(names, ["bob-mini"]);
    assert!(j["missing"][a2][0]["used_age"].f64() <= 2.0, "{j}");
    assert_eq!(
        fx.call("ledger", BY_DEV2, &obj! { "get" => list![a2] }).1["missing"][a2],
        list![],
        "bob-mini 自己第一次问（还没传）：不把它自己算成没传过的（它这一轮接着就传）"
    );
    fx.call("ledger", BY_DEV2, &obj! { "put" => obj! { a2 => blob(2048) } });
    assert_eq!(fx.call("ledger", BY_DEV, &obj! { "get" => list![a2] }).1["missing"][a2], list![], "传过了就不提");
    let raw = fsx::read_text(fx.d.join("cost-slots.json"));
    assert_eq!(fsx::mode(fx.d.join("cost-slots.json")), 0o600);
    assert_in(b2["c"].str(), &raw);
    fx.restart();
    let (_, j) = fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1] });
    assert_eq!(blobs_by_name(&j["cost"][a1]), mine_only, "重启后还在");
    fx.revoke(DEV2);
    let (_, j) = fx.call("ledger", BY_DEV, &obj! { "get" => list![a1, a2] });
    let left: Vec<(&str, bool)> = j["cost"][a1].list().iter().map(|x| (x["name"].str(), x["revoked"].bool())).collect();
    assert_eq!((left, &j["cost"][a2]), (vec![("alice-mbp", false)], &list![]), "移除了的设备：它传的删掉（服务器上不留它的用量）");
    let raw = fsx::read_text(fx.d.join("cost-slots.json"));
    assert!(!(raw.contains(&fb) || raw.contains(b2["c"].str())), "文件里也没有了");
    assert_eq!(fx.call("ledger", BY_DEV2, &obj! { "get" => list![a1] }).0, 403, "移除的设备不能再问");
}

// 以前的版本留在服务器上的折合金额启动时删掉：cli/src/server/store/tests.rs 的 leftovers_of_removed_devices_are_cleaned_at_start

// ==== Team 的同步服务器——管理员拿根密钥登记、邀请、移除设备，存各台设备的配置；设备拿邀请加入、取自己的那一份。
// 邀请链接、配置的密文和签名照 server/README.md「Team 的同步服务器」在 kit::spec 里独立实现 ====

/// 配置里的盐
const SALT: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// 这一组用例共用的：一个目录（证书，六把密钥 root、root2、a、b、c、evil）和照它起的服务器
struct TeamFx {
    /// 平常是照 root 的根公钥起的；flow 最后把它换成了照 root2 起的
    srv: Server,
    keys: SignedCalls,
    /// Team 的编号（照 root 的根公钥算的）
    tid: String,
    d: PathBuf,
    /// 放在最后：服务器先停，目录再删
    _tmp: TempDir,
}

static TEAM_FX: Mutex<Option<TeamFx>> = Mutex::new(None);

fn team_fx(t: &mut Ctx) -> Held<TeamFx> {
    held(t, &TEAM_FX, || {
        let tmp = TempDir::new("cca-team-test-");
        let d = tmp.path().to_path_buf();
        pki::make_cert(&d);
        let keys = SignedCalls::new(&d, &["root", "root2", "a", "b", "c", "evil"]);
        let tid = spec::team_id(keys.pubkey("root"));
        let srv = Server::start(&d, keys.pubkey("root"));
        TeamFx { srv, keys, tid, d, _tmp: tmp }
    })
}

/// 一个请求是谁发的（原来 call 的那几个带默认值的参数）
#[derive(Clone, Copy, Debug)]
struct Who<'a> {
    /// 请求里写谁的指纹（密钥的名字）
    who: &'a str,
    /// 拿另一把密钥签（冒用 who 的指纹）
    signer: Option<&'a str>,
    /// 签完再改 body
    tamper: bool,
}

/// 设备 a（原来 call 的 who 的默认值）
const AS_A: Who = Who { who: "a", signer: None, tamper: false };
const AS_B: Who = Who { who: "b", ..AS_A };
const AS_C: Who = Who { who: "c", ..AS_A };
/// 管理员：根密钥（原来 admin 的 who 的默认值）
const AS_ROOT: Who = Who { who: "root", ..AS_A };

/// 发出去的一个邀请：那串随机数、令牌、设备编号、配置的密钥
struct Invite {
    secret: String,
    token: String,
    dev: String,
    keys: spec::Keys,
}

/// 服务器回绝时回的：(状态码, {"error": 原因})
fn refused(status: u16, why: &str) -> (u16, Value) {
    (status, obj! { "error" => why })
}

/// 32 个十六进制字符（Team 的编号、账号摘要的样子）
fn is_hex32(s: &str) -> bool {
    regex::Regex::new(r"^[0-9a-f]{32}$").expect("写死的正则").is_match(s)
}

impl TeamFx {
    fn pubkey(&self, who: &str) -> &str {
        self.keys.pubkey(who)
    }

    fn call_on(&self, srv: &Server, op: &str, by: Who, extra: &Value) -> (u16, Value) {
        self.keys.call_as(srv, op, by.who, by.signer, by.tamper, extra)
    }

    fn call(&self, op: &str, by: Who, extra: &Value) -> (u16, Value) {
        self.call_on(&self.srv, op, by, extra)
    }

    /// 管理员的请求：拿根密钥签名（限额另算：每秒 2 个、攒得下 120 个；用完了等一会儿再试）
    fn admin_on(&self, srv: &Server, op: &str, by: Who, extra: &Value) -> (u16, Value) {
        let mut reply = self.call_on(srv, op, by, extra);
        for _ in 0..4 {
            if reply.0 != 429 {
                break;
            }
            // 等的就是限流的配额攒回来
            std::thread::sleep(Duration::from_millis(2100));
            reply = self.call_on(srv, op, by, extra);
        }
        reply
    }

    fn admin(&self, op: &str, extra: &Value) -> (u16, Value) {
        self.admin_on(&self.srv, op, AS_ROOT, extra)
    }

    /// 同 admin，换一个人发（设备的密钥、冒用根公钥指纹的、改过的）
    fn admin_as(&self, op: &str, by: Who, extra: &Value) -> (u16, Value) {
        self.admin_on(&self.srv, op, by, extra)
    }

    /// 发一个邀请
    fn invite_on(&self, srv: &Server) -> Invite {
        let secret = hex16();
        let (token, dev, keys) = spec::invite_parts(&secret).expect("那串随机数是 32 个十六进制字符");
        let ask = obj! { "token" => crypto::sha256_hex(token.as_bytes()), "dev" => dev.as_str(), "ttl" => 3600 };
        let (st, j) = self.admin_on(srv, "team_invite", AS_ROOT, &ask);
        assert_eq!(st, 200, "{j}");
        Invite { secret, token, dev, keys }
    }

    fn invite(&self) -> Invite {
        self.invite_on(&self.srv)
    }

    /// 拿邀请加入：pub 是 by.who 的公钥、name 是「who-mac」；over 里的（pub、name、proof）换掉同名的、没有的接在后面
    fn join_on(&self, srv: &Server, by: Who, token: &str, over: &Value) -> (u16, Value) {
        let mut extra = Obj::new();
        extra.insert("pub", self.pubkey(by.who));
        extra.insert("name", format!("{}-mac", by.who));
        extra.insert("token", token);
        for (key, value) in over.as_obj().into_iter().flat_map(Obj::iter) {
            extra.insert(key, value);
        }
        self.call_on(srv, "team_join", by, &Value::Obj(extra))
    }

    fn join(&self, by: Who, token: &str, over: &Value) -> (u16, Value) {
        self.join_on(&self.srv, by, token, over)
    }

    /// 管理员给一台设备的配置：密文和根密钥的签名
    fn doc(&self, keys: &spec::Keys, dev: &str, seq: i64, content: &Value) -> Value {
        let ct = spec::team_doc_seal(keys, &self.tid, dev, seq, content);
        let sig = spec::team_doc_sign(self.d.join("root"), &self.tid, dev, seq, &ct);
        obj! { "seq" => seq, "ct" => ct, "sig" => sig }
    }
}

/// 跑一次服务器的命令（起不来的那几种写法），等它退出：(退出码, 它在 stderr 上说的)。limit 秒还没退出算失败
fn server_run(args: &[&str], env: &[(&str, &str)], limit: u64) -> (i32, String) {
    let mut proc = ServerProc::spawn(args, env);
    let code = proc.wait(Duration::from_secs(limit)).unwrap_or_else(|| panic!("{limit} 秒了服务器还没退出：{args:?}"));
    (code, proc.stop())
}

/// 403 unknown_device 带着的那个编号（服务器上这一份记录的）；不是这样的回复是 None
fn unknown(reply: (u16, Value)) -> Option<String> {
    fleet::unknown(&reply)
}

fn flow(t: &mut Ctx) {
    let mut fx = team_fx(t);
    let (pubs, tid, d) = (fx.keys.pubs.clone(), fx.tid.clone(), fx.d.clone());
    let (tid, d) = (tid.as_str(), d.as_path());
    let (st, j) = fx.srv.get("/v1/health");
    assert_eq!((st, j["mode"].str(), j["team"].str()), (200, "team", tid));
    assert!(is_hex32(tid), "{tid}");
    let ops: Vec<&str> = j["ops"].list().iter().map(Value::str).collect();
    assert!(TEAM_OPS.iter().chain(&["report", "view", "keys", "ledger"]).all(|op| ops.contains(op)), "{ops:?}");
    assert!(
        !["join", "members", "relink", "remove"].iter().any(|op| ops.contains(op)),
        "3.5.3 以前的自己的同步服务器的那几个 op 没有了"
    );
    let old_join = obj! { "pub" => pubs["a"].as_str(), "name" => "a", "admit" => "ab".repeat(16), "check" => "cd".repeat(16) };
    assert_eq!(fx.call("join", AS_A, &old_join).0, 404);
    let doc =
        dumps(&obj! { "version" => 1, "seq" => 1, "machines" => list![obj! { "key" => pubs["evil"].as_str(), "name" => "evil" }] });
    let sig = pki::sign(d.join("evil"), "cca-devices", doc.as_bytes());
    assert_eq!(fx.srv.req("POST", "/v1/machines", doc.as_bytes(), Some(&sig), &[]).0, 404);
    let rec = unknown(fx.call("team_get", AS_A, &Value::Null)).expect("不在名单里：回这一份记录的编号");
    let rec = rec.as_str();
    // 管理员的 op 只认根密钥：设备的密钥、冒用根公钥指纹的别的密钥、改过的请求都是 403
    let da = "0a0a0a0a0a0a0a0a";
    let enroll = obj! { "devices" => obj! { da => obj! { "pub" => pubs["a"].as_str(), "name" => "studio" } } };
    for bad in [AS_A, Who { signer: Some("evil"), ..AS_ROOT }, Who { tamper: true, ..AS_ROOT }] {
        assert_eq!(fx.admin_as("team_enroll", bad, &enroll), refused(403, "denied"), "{bad:?}");
    }
    assert_eq!(fx.admin_as("team_members", Who { who: "evil", ..AS_ROOT }, &Value::Null), refused(403, "denied"));
    assert_eq!(
        unknown(fx.call("report", AS_ROOT, &obj! { "units" => obj! {}, "end" => Value::Null })).as_deref(),
        Some(rec),
        "根密钥不是设备：别的 op 不认它"
    );
    // 登记（管理员自己的那台）。（parse_enroll 不认的各种写法：cli/src/server/wire.rs 的 checks_match_python）
    assert_eq!(fx.admin("team_enroll", &obj! { "devices" => obj! {} }).0, 400);
    let root_as_device = obj! { "devices" => obj! { da => obj! { "pub" => pubs["root"].as_str(), "name" => "root" } } };
    assert_eq!(fx.admin("team_enroll", &root_as_device).0, 400, "根密钥不是设备：登记不了");
    let (st, j) = fx.admin("team_enroll", &enroll);
    assert_eq!((st, &j["names"], j["members"].i64(), j["gen"].str()), (200, &obj! { da => "studio" }, 1, rec), "{j}");
    let (st, j) = fx.call("team_get", AS_A, &Value::Null);
    assert_eq!(
        (st, j["dev"].str(), j["me_name"].str(), j["seq"].i64(), &j["doc"], j["gen"].str()),
        (200, da, "studio", 0, &Value::Null, rec),
        "{j}"
    );
    let again = obj! { "devices" => obj! { "0c".repeat(8) => obj! { "pub" => pubs["a"].as_str(), "name" => "again" } } };
    assert_eq!(fx.admin("team_enroll", &again), refused(409, "taken"), "这把公钥已经是别的编号的设备");
    // 邀请：服务器只存令牌的哈希
    for bad in [
        obj! { "token" => "zz", "dev" => da, "ttl" => 3600 },
        obj! { "token" => "0".repeat(64), "dev" => "nope", "ttl" => 3600 },
        obj! { "token" => "0".repeat(64), "dev" => "0b".repeat(8), "ttl" => 5 },
        obj! { "token" => "0".repeat(64), "dev" => "0b".repeat(8), "ttl" => 40 * 86400 },
    ] {
        assert_eq!(fx.admin("team_invite", &bad).0, 400, "{bad}");
    }
    assert_eq!(
        fx.admin("team_invite", &obj! { "token" => "0".repeat(64), "dev" => da, "ttl" => 3600 }),
        refused(409, "taken"),
        "这个编号的设备已经加入了"
    );
    let Invite { secret, token, dev: db, keys: kb } = fx.invite();
    let (token, db) = (token.as_str(), db.as_str());
    let state = fsx::read_text(d.join("state.json"));
    assert!(!state.contains(token) && !state.contains(&secret), "令牌、那串随机数不落在服务器上");
    assert_in(&crypto::sha256_hex(token.as_bytes()), &state);
    // 加入：没有这个邀请、签名不对、pub 和 key 对不上：一样的 403
    let zeros = "00".repeat(16);
    for (by, tok, over) in [
        (AS_B, zeros.as_str(), Value::Null),
        (AS_B, "nope", Value::Null),
        (Who { tamper: true, ..AS_B }, token, Value::Null),
        (AS_B, token, obj! { "pub" => pubs["c"].as_str() }),
        (Who { signer: Some("evil"), ..AS_B }, token, Value::Null),
    ] {
        assert_eq!(fx.join(by, tok, &over), refused(403, "denied"), "{by:?} {tok} {over}");
    }
    assert_eq!(fx.join(AS_B, token, &obj! { "name" => "x".repeat(65) }).0, 400);
    assert_eq!(fx.join(AS_ROOT, token, &Value::Null), refused(403, "denied"), "根密钥不是设备：拿着邀请也加入不了");
    assert_eq!(fx.join(AS_A, token, &Value::Null), refused(409, "taken"), "这把密钥已经是一台设备了");
    assert_eq!(fx.join(AS_B, token, &obj! { "proof" => "not-a-tag" }).0, 400, "认证码的样子不对");
    let proof = spec::team_pub_proof(&secret, &pubs["b"]);
    let (st, j) = fx.join(AS_B, token, &obj! { "name" => "studio", "proof" => proof.as_str() });
    assert_eq!((st, j["name"].str(), j["dev"].str(), j["members"].i64(), j["gen"].str()), (200, "studio-2", db, 2, rec), "{j}");
    assert_eq!(fx.join(AS_C, token, &Value::Null), refused(403, "used"), "邀请只能用一次（签名对的才问得出来是用过了）");
    assert_eq!(fx.join(Who { signer: Some("evil"), ..AS_C }, token, &Value::Null), refused(403, "denied"));
    let reissue = obj! { "token" => crypto::sha256_hex(token.as_bytes()), "dev" => "0d".repeat(8), "ttl" => 3600 };
    assert_eq!(fx.admin("team_invite", &reissue), refused(409, "used"), "用过的令牌不能再发一次");
    // 同一个编号再发一个邀请：前一个作废
    let old = fx.invite();
    let (st, j) = fx.admin("team_invite", &obj! { "token" => "1".repeat(64), "dev" => old.dev.as_str(), "ttl" => 3600 });
    assert_eq!((st, j["invites"].i64()), (200, 1), "{j}");
    assert_eq!(fx.join(AS_C, &old.token, &Value::Null), refused(403, "denied"));
    assert_eq!(fx.admin("team_revoke", &obj! { "dev" => old.dev.as_str() }).0, 200, "还没用掉的邀请也能撤回");
    // 配置：每台设备一份，序号只增不减；别的设备的取不到
    let content = obj! { "v" => 1, "name" => "测试组", "accounts" => obj! { "claude" => obj! { "alice@example.com" => "US-A" } } };
    let d1 = fx.doc(&kb, db, 1, &content);
    let ee = "ee".repeat(8);
    let ee = ee.as_str();
    let (st, j) = fx.admin("team_put", &obj! { "docs" => obj! { db => &d1, ee => &d1 } });
    assert_eq!((st, &j["stored"], &j["stale"], &j["unknown"]), (200, &list![db], &obj! {}, &list![ee]), "{j}");
    let (st, j) = fx.call("team_get", AS_B, &Value::Null);
    assert_eq!((st, j["dev"].str(), j["seq"].i64(), &j["doc"]), (200, db, 1, &d1), "{j}");
    let (ct, sig) = (&d1["ct"], d1["sig"].str());
    assert!(spec::team_doc_verify(&pubs["root"], tid, db, 1, ct, sig), "根密钥签的");
    assert!(!spec::team_doc_verify(&pubs["evil"], tid, db, 1, ct, sig));
    assert!(!spec::team_doc_verify(&pubs["root"], tid, db, 2, ct, sig), "换个序号签名就不对");
    assert_eq!(spec::team_doc_open(&kb, tid, db, 1, ct), Some(content.clone()));
    assert_eq!(spec::team_doc_open(&kb, tid, da, 1, ct), None, "挪到别的设备上认证不过");
    assert_eq!(spec::team_doc_open(&kb, tid, db, 2, ct), None, "换个序号认证不过");
    assert_eq!(crypto::b64_decode(ct["c"].str()).expect("密文是 base64").len() % 1024, 0, "补齐到 1024 的整数倍");
    assert_eq!(fx.call("team_get", AS_B, &obj! { "have" => 1 }).1["doc"], Value::Null, "手里的就是这一份：不带");
    assert_eq!(fx.call("team_get", AS_B, &obj! { "have" => 0 }).0, 400);
    assert_eq!(fx.call("team_get", AS_A, &Value::Null).1["doc"], Value::Null, "别的设备取不到它的");
    let put = |doc: &Value| fx.admin("team_put", &obj! { "docs" => obj! { db => doc } }).1;
    assert_eq!(put(&d1)["stored"], list![db], "同一份再存一次（没收到回复时）");
    assert_eq!(put(&fx.doc(&kb, db, 1, &obj! { "v" => 1 }))["stale"], obj! { db => 1 }, "同一个序号、不同的内容");
    let d3 = fx.doc(&kb, db, 3, &with(&content, "name", "新的"));
    assert_eq!(put(&d3)["stored"], list![db]);
    assert_eq!(put(&fx.doc(&kb, db, 2, &content))["stale"], obj! { db => 3 }, "序号往回走的不存");
    assert_eq!(fx.call("team_get", AS_B, &obj! { "have" => 1 }).1["doc"], d3);
    assert_eq!(
        fx.call("team_get", AS_B, &obj! { "have" => 7 }).1["doc"],
        d3,
        "设备手里的比存着的新（服务器的那一份被换成旧的了）：照样给，设备自己看序号"
    );
    assert_eq!(fx.call("team_get", AS_B, &obj! { "have" => 1i64 << 53 }).0, 400);
    // （parse_team_docs 不认的各种写法：cli/src/server/wire.rs 的 checks_match_python）
    assert_eq!(fx.admin("team_put", &obj! { "docs" => obj! { db => with(&d1, "seq", 0) } }).0, 400);
    assert_eq!(fx.call("team_put", AS_B, &obj! { "docs" => obj! { db => &d1 } }), refused(403, "denied"), "设备存不了配置");
    // 还没加入的设备的配置可以先存着：加入以后马上同步得到
    let Invite { token: tc, dev: dc, keys: kc, .. } = fx.invite();
    let dc = dc.as_str();
    let d0 = fx.doc(&kc, dc, 1, &content);
    assert_eq!(fx.admin("team_put", &obj! { "docs" => obj! { dc => &d0 } }).1["stored"], list![dc]);
    let (st, j) = fx.admin("team_members", &Value::Null);
    let rows = by_name(&j["members"]);
    let names: Vec<&str> = rows.keys().map(String::as_str).collect();
    assert_eq!((st, names, j["gen"].str()), (200, vec!["studio", "studio-2"], rec), "{j}");
    let (studio, studio2) = (&rows["studio"], &rows["studio-2"]);
    assert_eq!(
        (studio["dev"].str(), studio["pub"].str(), studio["fp"].str(), studio["seq"].i64()),
        (da, pubs["a"].as_str(), fx.keys.fp("a").as_str(), 0)
    );
    assert_eq!(
        (studio2["dev"].str(), studio2["seq"].i64(), &studio2["have"], &studio["have"]),
        (db, 3, &Value::from(7), &Value::Null),
        "各台设备同步配置时说的手里那一份的序号"
    );
    assert_eq!(
        (&studio2["proof"], &studio["proof"]),
        (&Value::from(proof.as_str()), &Value::Null),
        "设备加入时带的认证码原样回给管理员；直接登记的没有"
    );
    let recent = |age: &Value| (0.0..=120.0).contains(&age.f64());
    assert!(rows.values().all(|r| recent(&r["joined_age"]) && recent(&r["seen_age"])), "{rows:?}");
    let invites: Vec<(&str, i64, bool)> = j["invites"]
        .list()
        .iter()
        .map(|i| (i["dev"].str(), i["seq"].i64(), 3500.0 < i["expires_in"].f64() && i["expires_in"].f64() <= 3600.0))
        .collect();
    assert_eq!(invites, [(dc, 1, true)], "{}", j["invites"]);
    assert_eq!(fx.call("team_members", AS_B, &Value::Null), refused(403, "denied"), "设备看不了名单");
    assert_eq!(fx.join(AS_C, &tc, &Value::Null).0, 200);
    assert_eq!(fx.call("team_get", AS_C, &Value::Null).1["doc"], d0);
    // 别的 op 照旧
    let aid = spec::aid_of(SALT, "claude", "alice@example.com");
    let aid = aid.as_str();
    let report = obj! { "units" => obj! { aid => unit(&[[10, 1]], Value::Null) }, "end" => Value::Null };
    assert_eq!(fx.call("report", AS_B, &report).0, 200);
    let (_, j) = fx.call("view", AS_A, &obj! { "accounts" => list![aid] });
    let seen: Vec<(&str, &str)> = j["accounts"][aid].list().iter().map(|r| (r["name"].str(), r["state"].str())).collect();
    assert_eq!(seen, [("studio-2", "in_use")]);
    let sealed = blob(1024);
    assert_eq!(fx.call("keys", AS_B, &obj! { "put" => obj! { aid => &sealed } }).0, 200);
    assert_eq!(fx.call("ledger", AS_B, &obj! { "put" => obj! { aid => &sealed } }).0, 200);
    assert_eq!(fx.call("keys", AS_A, &obj! { "get" => list![aid] }).1["keys"][aid]["name"].str(), "studio-2");
    // 请求的大小：管理员存配置的最多 1 MB，别的 64 KB
    let big = vec![b'x'; MAX_BODY + 1];
    assert_eq!(fx.srv.req("POST", "/v1/team_put", &big, Some(b"sig"), &[]).0, 400, "过了大小这一关（内容不是 JSON）");
    let st = fx.srv.req("POST", "/v1/team_enroll", &big, Some(b"sig"), &[]).0;
    assert!(matches!(st, 413 | DROPPED), "{st}");
    let st = fx.srv.req("POST", "/v1/team_put", &vec![b'x'; MAX_SYNC_BODY + 1], Some(b"sig"), &[]).0;
    assert!(matches!(st, 413 | DROPPED), "{st}");
    // 移除：密钥就此不认，给它的配置一起删掉
    assert_eq!(fx.admin("team_revoke", &obj! { "dev" => "nope" }).0, 400);
    assert_eq!(fx.admin("team_revoke", &obj! { "dev" => ee }), refused(404, "no_device"));
    assert_eq!(fx.call("team_revoke", AS_C, &obj! { "dev" => db }), refused(403, "denied"), "设备移除不了别的设备");
    let (st, j) = fx.admin("team_revoke", &obj! { "dev" => db });
    assert_eq!((st, j["members"].i64()), (200, 2));
    assert_eq!(unknown(fx.call("team_get", AS_B, &Value::Null)).as_deref(), Some(rec), "被移除的设备：编号照旧（是被移除了）");
    assert_eq!(put(&fx.doc(&kb, db, 9, &content))["unknown"], list![db]);
    assert_eq!(fx.call("view", AS_A, &obj! { "accounts" => list![aid] }).1["accounts"][aid], list![], "它上报的记录跟着删掉");
    // 退出
    let (st, j) = fx.call("team_leave", AS_C, &Value::Null);
    assert_eq!((st, j["members"].i64()), (200, 1));
    assert_eq!(unknown(fx.call("team_get", AS_C, &Value::Null)).as_deref(), Some(rec));
    assert!(!fsx::read_json(d.join("team-docs.json"))["docs"].obj().contains_key(dc));
    // 换了密钥的设备：管理员照编号重新登记，原来那把就此不认
    let rekeyed = obj! { "devices" => obj! { da => obj! { "pub" => pubs["c"].as_str(), "name" => "studio" } } };
    let (st, j) = fx.admin("team_enroll", &rekeyed);
    assert_eq!((st, &j["names"], j["members"].i64()), (200, &obj! { da => "studio" }, 1), "{j}");
    assert_eq!(unknown(fx.call("team_get", AS_A, &Value::Null)).as_deref(), Some(rec));
    assert_eq!(fx.call("team_get", AS_C, &Value::Null).1["dev"].str(), da);
    let Invite { token: tv, dev: dv, .. } = fx.invite();
    let direct = obj! { "devices" => obj! { dv.as_str() => obj! { "pub" => pubs["b"].as_str(), "name" => "direct" } } };
    assert_eq!(fx.admin("team_enroll", &direct).0, 200);
    assert_eq!(
        fx.join(Who { who: "evil", ..AS_A }, &tv, &Value::Null),
        refused(403, "denied"),
        "这个编号直接登记了：还没用掉的邀请作废"
    );
    assert_eq!(fx.admin("team_revoke", &obj! { "dev" => dv.as_str() }).1["members"].i64(), 1);
    // 重启：同一个根公钥接着用；换一个就从头开始
    let (_, _, ka) = spec::invite_parts(&"ab".repeat(16)).expect("32 个十六进制字符");
    let da_doc = fx.doc(&ka, da, 1, &content);
    assert_eq!(fx.admin("team_put", &obj! { "docs" => obj! { da => &da_doc } }).1["stored"], list![da]);
    fx.srv.stop();
    fx.srv = Server::start(d, &pubs["root"]);
    let (st, j) = fx.call("team_get", AS_C, &Value::Null);
    assert_eq!((st, j["gen"].str(), &j["doc"]), (200, rec, &da_doc), "重启后照样认得设备、配置还在，编号不变");
    assert_eq!(fsx::mode(d.join("team-docs.json")), 0o600);
    fx.srv.stop();
    fx.srv = Server::start(d, &pubs["root2"]);
    assert_eq!(fx.srv.get("/v1/health").1["team"].str(), spec::team_id(&pubs["root2"]));
    let rec2 = unknown(fx.call("team_get", AS_C, &Value::Null));
    assert!(rec2.as_deref().is_some_and(|r| r != rec), "换了一个 Team：以前的设备不认了，记录换了编号");
    assert_eq!(fx.admin("team_members", &Value::Null), refused(403, "denied"), "原来的根密钥不认了");
    let (st, j) = fx.admin_as("team_members", Who { who: "root2", ..AS_ROOT }, &Value::Null);
    assert_eq!((st, &j["members"], &j["invites"]), (200, &list![], &list![]));
    assert_eq!(fsx::read_json(d.join("team-docs.json"))["docs"], obj! {}, "以前存的配置都删掉");
}

/// 服务器的记录没了（状态文件丢了，配置的根公钥照旧）：设备自己加不回来（邀请只能用一次），管理员照 team_members 回过的
/// 公钥登记回去、再存一次配置
fn records_lost(t: &mut Ctx) {
    let fx = team_fx(t);
    let d = fx.d.join("lost");
    std::fs::create_dir(&d).expect("建目录");
    let (root, proxied) = (fx.pubkey("root"), ServerOpts::new().proxied(true));
    // 服务器离开作用域时停掉（原来的 addCleanup）
    let mut srv = Server::start_with(&d, root, &proxied);
    let da = "0a0a0a0a0a0a0a0a";
    let studio = obj! { "devices" => obj! { da => obj! { "pub" => fx.pubkey("a"), "name" => "studio" } } };
    assert_eq!(fx.admin_on(&srv, "team_enroll", AS_ROOT, &studio).0, 200);
    let Invite { token, dev: db, keys: kb, .. } = fx.invite_on(&srv);
    let db = db.as_str();
    assert_eq!(fx.join_on(&srv, AS_B, &token, &Value::Null).0, 200);
    let doc = fx.doc(&kb, db, 4, &obj! { "v" => 1 });
    let put = obj! { "docs" => obj! { db => &doc } };
    assert_eq!(fx.admin_on(&srv, "team_put", AS_ROOT, &put).1["stored"], list![db]);
    let (_, j) = fx.admin_on(&srv, "team_members", AS_ROOT, &Value::Null);
    let rec = j["gen"].str();
    let mut saved = Obj::new();
    for r in j["members"].list() {
        saved.insert(r["dev"].str(), obj! { "pub" => &r["pub"], "name" => &r["name"] });
    }
    srv.stop();
    for f in ["state.json", "team-docs.json"] {
        std::fs::remove_file(d.join(f)).unwrap_or_else(|e| panic!("删 {f}：{e}"));
    }
    let srv = Server::start_with(&d, root, &proxied);
    let rec2 = unknown(fx.call_on(&srv, "team_get", AS_B, &Value::Null));
    assert!(rec2.as_deref().is_some_and(|r| r != rec), "记录换了编号：设备据此知道不是被移除了");
    let rec2 = rec2.expect("上面看过了");
    assert_eq!(fx.join_on(&srv, AS_B, &token, &Value::Null), refused(403, "denied"), "用过的邀请加不回来");
    let (st, j) = fx.admin_on(&srv, "team_enroll", AS_ROOT, &obj! { "devices" => saved });
    assert_eq!(
        (st, &j["names"], j["members"].i64(), j["gen"].str()),
        (200, &obj! { da => "studio", db => "b-mac" }, 2, rec2.as_str()),
        "{j}"
    );
    let (st, j) = fx.call_on(&srv, "team_get", AS_B, &obj! { "have" => 4 });
    assert_eq!((st, j["dev"].str(), j["seq"].i64(), &j["doc"]), (200, db, 0, &Value::Null), "还没有配置：设备接着用手里的那一份");
    assert_eq!(fx.admin_on(&srv, "team_put", AS_ROOT, &put).1["stored"], list![db], "同一个序号再存一次");
    assert_eq!(fx.call_on(&srv, "team_get", AS_B, &obj! { "have" => 4 }).1["doc"], Value::Null);
    assert_eq!(fx.call_on(&srv, "team_get", AS_B, &Value::Null).1["doc"], doc);
}

// Store 的 team_* 那几个方法（邀请的期限和上限、成员的上限、配置的总量……）：cli/src/server/store/tests.rs 的
// invites_members_and_a_broken_state_file、invite_and_doc_caps、starts_over_for_another_server

/// 问一个只说 HTTP 的服务器（--behind-proxy、托管平台上的）的 /v1/health
fn plain_health(port: &str) -> Value {
    let url = format!("http://127.0.0.1:{}/v1/health", port.trim());
    let reply = Request::new("GET", &url).timeout(Duration::from_secs(10)).send().unwrap_or_else(|e| panic!("{url}：{e}"));
    reply.json().unwrap_or_else(|e| panic!("{url} 回的不是 JSON：{e}"))
}

/// 启动参数：根公钥从 --team-root 或者环境变量 CCA_TEAM_ROOT 来，不是 ssh-ed25519 公钥就不启动；--mode 只剩 team（安装脚本照旧
/// 写着它），以前的维护者的服务器（--mode maintainer、--release-key）起不来
fn cli(t: &mut Ctx) {
    let fx = team_fx(t);
    let d = fx.d.join("cli");
    std::fs::create_dir(&d).expect("建目录");
    let (state, port_file, release) = (d.join("state.json"), d.join("port"), d.join("release.pub"));
    let base = ["--listen", "127.0.0.1:0", "--behind-proxy", "--state", p(&state), "--mode", "team"];
    let no_root: [(&[&str], &[(&str, &str)]); 4] =
        [(&[], &[]), (&["--team-root", "nope"], &[]), (&["--team-root", "ssh-rsa AAAA"], &[]), (&[], &[("CCA_TEAM_ROOT", "x")])];
    for (extra, env) in no_root {
        let (code, said) = server_run(&[&base[..], extra].concat(), env, 20);
        assert_eq!(code, 2, "{extra:?} {env:?}");
        assert_in("根公钥", &said);
    }
    fsx::write(&release, format!("{} test-release\n", fx.pubkey("evil")));
    for old in [["--mode", "maintainer"], ["--release-key", p(&release)]] {
        let (code, said) = server_run(&[&base[..base.len() - 2], &old[..], &["--team-root", fx.pubkey("root")]].concat(), &[], 20);
        assert_eq!(code, 2, "{old:?}");
        assert_in(old[0], &said);
    }
    assert!(!state.exists(), "起不来的时候什么都不写");
    let with_note = format!("{} 备注", fx.pubkey("root"));
    let mut srv = ServerProc::spawn(&[&base[..], &["--port-file", p(&port_file)]].concat(), &[("CCA_TEAM_ROOT", &with_note)]);
    let port = srv.wait_file(&port_file, "服务器起来", Duration::from_secs(20));
    let j = plain_health(&port);
    let said = srv.stop();
    assert_eq!(j["team"].str(), fx.tid);
    assert_in("Team 的同步服务器", &said);
    // 只写后面那一段（安装脚本、托管平台的环境变量里这样写：没有空格）；托管平台上（CCA_HUB_HOSTED=1）给了根公钥就是 Team 的服务器，
    // 不用再写 --mode
    std::fs::remove_file(&port_file).expect("删端口文件");
    let hosted_state = d.join("hosted.json");
    let key_only = fx.pubkey("root").split_whitespace().nth(1).expect("公钥那一行的第二段");
    let hosted = [("CCA_HUB_HOSTED", "1"), ("CCA_TEAM_ROOT", key_only)];
    let mut srv =
        ServerProc::spawn(&["--listen", "127.0.0.1:0", "--state", p(&hosted_state), "--port-file", p(&port_file)], &hosted);
    let port = srv.wait_file(&port_file, "服务器起来", Duration::from_secs(20));
    let j = plain_health(&port);
    srv.stop();
    assert_eq!((j["mode"].str(), j["team"].str()), ("team", fx.tid.as_str()));
    if !Path::new("/data").is_dir() {
        // 托管平台上没挂存储（默认的状态文件在 /data 下）：不启动
        let (code, said) = server_run(&[], &hosted, 30);
        assert_eq!(code, 2);
        assert_in("没有 /data", &said);
    }
}

/// --behind-proxy（托管平台、反向代理后面）：只说 HTTP，不用证书；按 X-Forwarded-For 的最后一项限流（前面的是客户端自己写的，
/// 不算）；同一个 IP 的连接数不限
fn behind_proxy(t: &mut Ctx) {
    let fx = team_fx(t);
    let d = fx.d.join("proxied");
    std::fs::create_dir(&d).expect("建目录");
    // 服务器离开作用域时停掉（原来的 addCleanup）
    let srv = Server::start_with(&d, fx.pubkey("root"), &ServerOpts::new().proxied(true));
    let (st, j) = srv.get("/v1/health");
    assert_eq!((st, j["mode"].str()), (200, "team"));
    // 服务器按 IP 限流的那个桶（ip_bucket：Buckets(2, 120)）攒得下这么多个
    let burst = 120;
    let flood = |xff: &dyn Fn(usize) -> String| {
        let got: Vec<u16> =
            (0..burst + 5).map(|i| srv.req("POST", "/v1/report", b"{}", None, &[("X-Forwarded-For", &xff(i))]).0).collect();
        got.iter().filter(|st| **st == 429).count()
    };
    assert_eq!(flood(&|i| format!("198.51.100.{}", i % 250)), 0, "各是各的 IP：不限");
    assert!(flood(&|i| format!("203.0.113.{}, 192.0.2.7", i % 250)) > 0, "最后一项一样：同一个 IP（前面的随便写也没用）");
    let addr = SocketAddr::from(([127, 0, 0, 1], srv.port));
    let socks: Vec<TcpStream> = (0..MAX_CONN_IP + 2)
        .map(|_| TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap_or_else(|e| panic!("连不上 {addr}：{e}")))
        .collect();
    // 等服务器把它们都接进来（它那边没有看得到的记号，照原来等 0.3 秒）
    std::thread::sleep(Duration::from_millis(300));
    let st = srv.get("/v1/health").0;
    drop(socks);
    assert_eq!(st, 200, "连过来的都是代理：不按同一个 IP 的连接数断开");
    let x_state = fx.d.join("x.json");
    let with_cert =
        ["--behind-proxy", "--cert", "x", "--key", "y", "--mode", "team", "--team-root", fx.pubkey("root"), "--state", p(&x_state)];
    let (code, said) = server_run(&with_cert, &[], 30);
    assert_eq!(code, 2, "--behind-proxy 不用证书");
    assert_in("不用 --cert", &said);
}

// ==== 客户端（cca 的辅助程序）——账号摘要、上报、cca devices、共享记下的 API key、折合金额的同步，经 curl + 公钥钉住连
// 真的 Team 的同步服务器。设备都在 Team「测试组」里（它给 alice 的 Claude 账号；bob 是 alice-mbp 自己的账号）。只能在进程内测的
// 几处在辅助程序自己的单元测试里（cargo test），下面各留了一行说它在哪 ====

/// 这一组用例共用的：一个目录（证书、Team 的根密钥、服务器的状态文件；各台设备的 HOME 也建在里面）、Team、服务器
struct ClientFx {
    /// 每个用例换一个新的进程
    srv: Option<Server>,
    team: Team,
    /// 服务器证书的公钥指纹（设备钉住的）
    pin: String,
    d: PathBuf,
    _tmp: TempDir,
}

static CLIENT_FX: Mutex<Option<ClientFx>> = Mutex::new(None);

/// 一个用例的环境
struct Client {
    fx: Held<ClientFx>,
    /// alice 的 Claude 账号的摘要
    aid: String,
    /// 这个用例建过的设备（名字、HOME）：收尾时看它们有没有调到 security
    devices: Arc<Mutex<Vec<(String, PathBuf)>>>,
    /// alice-mbp：账号表里有 alice（Team 给的）和 bob（它自己的）
    dev: Device,
}

/// 每个用例一个新的服务器进程（名单、存的密文都在文件里，接着用）——限流的配额从头算；一台设备 alice-mbp；
/// 收尾时核对没有哪台设备调到了 security（钥匙串）
fn client(t: &mut Ctx) -> Client {
    let mut fx = held(t, &CLIENT_FX, || {
        let tmp = TempDir::new("cca-presence-client-");
        let d = tmp.path().to_path_buf();
        pki::make_cert(&d);
        let team = Team::new(&d);
        let srv = Server::fresh(&d, &team.root);
        let pin = pki::openssl_pin(d.join("cert.pem"));
        ClientFx { srv: Some(srv), team, pin, d, _tmp: tmp }
    });
    if let Some(mut old) = fx.srv.take() {
        old.stop();
    }
    fx.srv = Some(Server::fresh(&fx.d, &fx.team.root));
    let aid = spec::aid_of(&fx.team.salt, "claude", "alice@example.com");
    let devices = Arc::new(Mutex::new(Vec::new()));
    let table = obj! { Device::KEY => "US-A", "bob@example.com" => "US-B" };
    let dev = new_device(&fx, &devices, "alice-mbp", None, Some(&table));
    let seen = devices.clone();
    t.defer(move || {
        let called: Vec<String> =
            locked(&seen).iter().filter(|(_, home)| home.join("security.log").exists()).map(|(name, _)| name.clone()).collect();
        assert_eq!(called, Vec::<String>::new(), "调到了 security（钥匙串）");
    });
    Client { fx, aid, devices, dev }
}

/// 一台设备：自己的 ~/.cca，在 Team 里、同一份配置；uuid 不是 None 时 alice 在它上面登录着；table 是账号表（默认只有 alice）
fn new_device(
    fx: &ClientFx,
    devices: &Mutex<Vec<(String, PathBuf)>>,
    name: &str,
    uuid: Option<&str>,
    table: Option<&Value>,
) -> Device {
    let home = fx.d.join(format!("{name}-{}", crypto::token_hex(4)));
    let alice_only = obj! { Device::KEY => "US-A" };
    let mut d = Device::new(&home, name, table.unwrap_or(&alice_only));
    d.join(&fx.team, &fx.srv.as_ref().expect("服务器起着").url(), &[&fx.pin]);
    if uuid.is_some() {
        d.login(uuid);
    }
    locked(devices).push((name.to_string(), home));
    d
}

impl Client {
    fn srv(&self) -> &Server {
        self.fx.srv.as_ref().expect("服务器起着")
    }

    fn port(&self) -> u16 {
        self.srv().port
    }

    fn team(&self) -> &Team {
        &self.fx.team
    }

    /// 设备上这个 Team 的服务器的状态文件的前缀
    fn lane(&self) -> &str {
        &self.fx.team.lane
    }

    /// Team 的编号
    fn tid(&self) -> &str {
        &self.fx.team.id
    }

    fn salt(&self) -> &str {
        &self.fx.team.salt
    }

    /// 服务器的目录（状态文件在里面）
    fn d(&self) -> &Path {
        &self.fx.d
    }

    /// Team 给设备的配置（服务器是这一次的）；over 里的换掉同名的
    fn doc(&self, over: &Value) -> Value {
        self.fx.team.doc(&self.srv().url(), &[&self.fx.pin], over)
    }

    /// 一台设备（账号表里只有 alice）；uuid 不是 None 时 alice 在它上面登录着
    fn device(&self, name: &str, uuid: Option<&str>) -> Device {
        new_device(&self.fx, &self.devices, name, uuid, None)
    }

    /// 同 device，账号表另给
    fn device_with(&self, name: &str, table: &Value) -> Device {
        new_device(&self.fx, &self.devices, name, None, Some(table))
    }

    /// 服务器上的名单正好是这几台（管理员拿根密钥登记、移除；名字就是设备的名字）
    fn machines(&self, devs: &[&Device]) {
        self.fx.team.only(self.port(), devs);
    }

    /// 服务器上存着的这个账号的 API key 的密文
    fn server_blob(&self, aid: &str) -> Value {
        fsx::read_json(self.d().join("keys.json"))["acc"][aid]["blob"].clone()
    }

    /// 服务器上存着的这个账号的折合金额：{设备的指纹: 那一份}
    fn slots(&self, aid: &str) -> Value {
        fsx::read_json(self.d().join("cost-slots.json"))["acc"].get(aid).cloned().unwrap_or_else(|| obj! {})
    }
}

/// cca devices …：(stdout, 这次问服务器的那个请求；没问是 null)。rc 是该有的退出码，env 是这一次另设的环境变量
#[track_caller]
fn view(dev: &Device, args: &[&str], rc: i32, env: &[(&str, &str)]) -> (String, Value) {
    let before = dev.sent(None).len();
    let argv: Vec<&str> = ["presence-view"].into_iter().chain(args.iter().copied()).collect();
    let r = dev.run_with(&argv, "", env);
    assert_eq!((r.code, r.err.as_str()), (rc, ""), "{}", r.out);
    (r.out, dev.sent(None).into_iter().nth(before).unwrap_or(Value::Null))
}

/// 等这台设备发出这个接口的请求（回来了才算），给出到那时为止的那几个
fn wait_sent(what: &str, dev: &Device, op: &str) -> Vec<Value> {
    wait_for(what, WAIT, || Some(dev.sent(Some(op))).filter(|sent| !sent.is_empty()))
}

/// 上报里这个账号的命令行会话正好一个：(开了多久, 闲了多久)（`[[opened, idle]] = report['units'][aid]['cli']`）
#[track_caller]
fn one_cli(report: &Value, aid: &str) -> (f64, f64) {
    let pair = only(&report["units"][aid]["cli"]).list();
    assert_eq!(pair.len(), 2, "{pair:?}");
    (pair[0].f64(), pair[1].f64())
}

/// 把文件的访问时间、修改时间分别改掉（`os.utime(path, (atime, mtime))`；1970 年以来的秒）
fn utime(path: &Path, atime: f64, mtime: f64) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("路径里没有 NUL");
    let spec = |secs: f64| libc::timespec { tv_sec: secs.floor() as _, tv_nsec: (secs.fract() * 1e9) as libc::c_long };
    let times = [spec(atime), spec(mtime)];
    // SAFETY: c 是以 NUL 结尾的路径，times 是两个有效的 timespec；utimensat 只读它们
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "改不了 {} 的时间：{}", path.display(), std::io::Error::last_os_error());
}

fn identity(t: &mut Ctx) {
    let c = client(t);
    let dev = &c.dev;
    let mut pubfile = dev.keyfile().as_os_str().to_os_string();
    pubfile.push(".pub");
    let listed = Cmd::new("ssh-keygen").args(["-l", "-E", "sha256", "-f"]).arg(&pubfile).pass_env(&["PATH"]).output();
    let listed = listed.expect("ssh-keygen 运行得了").check().out();
    let key_fp = listed.split_whitespace().nth(1).expect("ssh-keygen -l 的第二段是指纹").to_string();
    c.machines(&[dev]);
    let (_, asked) = view(dev, &["--json"], 0, &[]);
    assert_eq!(
        (asked["key"].str(), asked["code"].i64()),
        (key_fp.as_str(), 200),
        "和 ssh-keygen -l 的指纹一样（服务器照登记的公钥认它）"
    );
    let a = only(&asked["accounts"]).str().to_string();
    assert!(regex::Regex::new(r"^[0-9a-f]{32}$").expect("写死的正则").is_match(&a), "{a}");
    assert_eq!(a, spec::aid_of(c.salt(), "claude", "alice@example.com"), "账号摘要和文档写的一样");
    fsx::write(dev.cca.join("accounts.json"), dumps(&obj! { "alice(Alice@Example.com)" => "US-A" }));
    assert_eq!(view(dev, &["--json"], 0, &[]).1["accounts"], list![a.as_str()], "邮箱不分大小写");
    fsx::write(dev.cca.join("codex.json"), dumps(&obj! { "alice@example.com" => "US-A" }));
    let both_tools = obj! { "claude" => obj! { "alice@example.com" => "US-A" }, "codex" => obj! { "alice@example.com" => "US-A" } };
    dev.set_doc(c.team(), &c.doc(&obj! { "accounts" => both_tools }));
    let asked = view(dev, &["--json"], 0, &[("CCA_TOOL", "codex")]).1;
    let x = only(&asked["accounts"]).str().to_string();
    assert_ne!(a, x, "Claude 账号和 Codex 账号不撞");
    assert_eq!(x, spec::aid_of(c.salt(), "codex", "alice@example.com"));
    dev.set_doc(c.team(), &c.doc(&obj! { "salt" => "11".repeat(16) }));
    let asked = view(dev, &["--json"], 0, &[]).1;
    let y = only(&asked["accounts"]).str().to_string();
    assert_ne!(a, y, "换了盐就认不出");
    assert_eq!(y, spec::aid_of(&"11".repeat(16), "claude", "alice@example.com"));
    // 用不了的几种（cca devices 说原因，退出码 2）
    dev.config_edit(|c| {
        c.insert("presence", false);
    });
    assert_in("关掉了", &view(dev, &[], 2, &[]).0);
    dev.config_edit(|c| {
        c.remove("presence");
    });
    let doc = dev.cca.join("teams").join(c.tid()).join("doc.json");
    std::fs::rename(&doc, doc.with_file_name("doc.bak")).expect("挪开 doc.json"); // 配置还没同步下来
    assert_in("「测试组」的配置还没同步下来（cca team sync）", &view(dev, &[], 2, &[]).0);
    std::fs::rename(doc.with_file_name("doc.bak"), &doc).expect("挪回 doc.json");
    std::fs::rename(dev.keyfile(), dev.keyfile().with_file_name("id.bak")).expect("挪开密钥"); // 这台设备在 Team 里的密钥不见了
    assert_in("这台设备用不了「测试组」的同步服务器（密钥不见了", &view(dev, &[], 2, &[]).0);
    assert_eq!(dev.sent(None).len(), 4, "用不了的时候不问服务器");
}

/// 命令行会话的闲置时间 = 现在 - 它终端的最后一次输入 / 输出；登记还在、进程没了或者进程号被别的进程重用了的不算。
/// 拿固定的时刻逐项核对：cli/src/cmd/presence.rs 的单元测试 scan_cli_idle_from_tty
fn scan_cli_idle_from_tty(t: &mut Ctx) {
    let c = client(t);
    let dev = &c.dev;
    c.machines(&[dev]);
    // 一对伪终端：两头都开着到用例结束
    let pty = kit::pty::Pty::open(24, 80).expect("开伪终端");
    let tty = pty.slave_path().to_path_buf();
    let now = fleet::now();
    utime(&tty, now - 120.0, now - 200.0); // 最后一次输入 120 秒前、输出 200 秒前
                                           // ps 的写法：macOS 是 ttys005，Linux 是 pts/5
    let name = p(&tty).strip_prefix("/dev/").expect("终端在 /dev 下").to_string();
    let since = fleet::session(dev, &CliSession { opened: 300.0, tty: &name, ..CliSession::default() });
    // 进程号被重用（启动时间对不上）
    fleet::session(dev, &CliSession { pid: SESSION_PID - 1, started: fleet::t0_epoch() + 3, ..CliSession::default() });
    // cca 被 kill -9 留下的登记：进程没了
    fleet::session(dev, &CliSession { pid: SESSION_PID - 2, running: false, ..CliSession::default() });
    fleet::agent(t, dev);
    let reports = wait_sent("上报进程报上去", dev, "report");
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    assert_eq!((report["code"].i64(), key_list(&report["units"])), (200, vec![c.aid.as_str()]));
    let (opened, idle) = one_cli(report, &c.aid); // 只有终端上的那一个算
    let done = report["done"].f64();
    assert!(opened >= 300.0 && (since + opened - done).abs() < 15.0, "开了多久");
    assert!(idle >= 120.0 && (now - 120.0 + idle - done).abs() < 15.0, "闲置 = 现在 - 最后一次输入 / 输出");
    drop(pty);
}

// 桌面 App 看进程树的 CPU（隔 30 秒采样、过了阈值才算有动静，别的资料目录不算）：
//   cli/src/cmd/presence.rs 的单元测试 scan_app_cpu_like_python_test（还有 scan_app_sampling_like_python、
//   app_proc_re_matches_both_spellings_and_escapes）

fn post_and_devices(t: &mut Ctx) {
    let c = client(t);
    let dev = &c.dev;
    let (out, asked) = view(dev, &["--json"], 1, &[]);
    assert_eq!(
        (asked["code"].i64(), asked["reply"].get("error").and_then(Value::as_str)),
        (403, Some("unknown_device")),
        "服务器的名单里还没有这台设备"
    );
    assert_in("不认这台设备", &out);
    c.machines(&[dev]);
    let since = fleet::session(dev, &CliSession::default()); // 100 秒前开的
                                                             // 这台设备自己的账号（Team 没给的）也开着会话
    fleet::session(dev, &CliSession { key: "bob@example.com", pid: SESSION_PID - 1, ..CliSession::default() });
    fleet::agent(t, dev);
    let reports = wait_sent("上报进程报上去", dev, "report");
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    assert_eq!(
        (report["code"].i64(), key_list(&report["units"]), &report["end"]),
        (200, vec![c.aid.as_str()], &Value::Null),
        "只报 Team 给的官方账号"
    );
    let (opened, idle) = one_cli(report, &c.aid);
    assert_eq!((idle, &report["units"][c.aid.as_str()]["app"]), (0.0, &Value::Null), "没有终端：在跑就算在干活；没开桌面 App");
    let done = report["done"].f64();
    assert!(opened >= 100.0 && (since + opened - done).abs() < 15.0, "开了多久：到上报的时候为止");
    let doc = json::parse(&view(dev, &["--json"], 0, &[]).0).expect("cca devices --json 给的是 JSON");
    assert_eq!((doc["kind"].str(), doc["me"].str(), doc["tool"].str()), ("devices", "alice-mbp", "claude"));
    let a = only(&doc["accounts"]);
    assert_eq!((a["key"].str(), a["hub"].str(), a["team"].str()), (Device::KEY, "team", c.tid()));
    let d = only(&a["devices"]);
    assert_eq!(
        (d["name"].str(), d["me"].bool(), d["state"].str(), d["cli"].i64(), d["app"].bool()),
        ("alice-mbp", true, "in_use", 1, false)
    );
    assert!(
        (d["since_epoch"].f64() - since).abs() < 15.0 && (d["last_used_epoch"].f64() - done).abs() < 15.0,
        "时间换算成这台 Mac 的时钟（什么时候开的、最近一次用是上报的那一刻）"
    );
    let out = view(dev, &[], 0, &[]).0;
    assert_in("alice-mbp（这台 Mac）  在用 · 命令行里 1 个会话 · ", &out);
    assert_in(" 起", &out);
    assert_in("是这台设备自己的账号", &view(dev, &["bob"], 2, &[]).0);
    let n = dev.sent(None).len();
    dev.team_json(c.team(), |t| {
        t.insert("pins", list![format!("sha256//{}=", "A".repeat(43))]);
    });
    let out = view(dev, &[], 1, &[]).0;
    assert!(out.contains("钉住的指纹对不上"), "证书不是钉住的那张：不发\n{out}");
    let codes: Vec<i64> = dev.sent(None)[n..].iter().map(|r| r["code"].i64()).collect();
    assert_eq!(codes, [0]);
    dev.team_json(c.team(), |t| {
        t.insert("url", "https://127.0.0.1:9");
        t.insert("pins", list![c.fx.pin.as_str()]);
    });
    assert_in("连不上 Team 的同步服务器（127.0.0.1:9）", &view(dev, &[], 1, &[]).0);
}

// cca devices 里过去的时刻怎么写（今天、昨天、星期几、日期）：cli/src/cmd/presence.rs 的单元测试 past_time_like_python_test

/// 报不出去（服务器连不上、不认这台设备）：按退避再试，心跳到期、会话变了都不会马上重发。
/// 隔多久再试的那几个数（心跳 60 秒上下，退避 10、20 秒……，不认这台设备时 10 分钟）：cli/src/cmd/presence.rs 的单元测试 retry_backs_off
fn agent_backoff(t: &mut Ctx) {
    let c = client(t);
    let ok = &c.dev;
    let (denied, down) = (c.device("bob-mini", Some("u-alice-0007")), c.device("carol-air", Some("u-alice-0007")));
    let recent = c.device("dan-air", Some("u-alice-0007"));
    c.machines(&[ok, &recent]); // bob-mini 不在名单里（403）；carol-air 连不上服务器
    ok.login(Some("u-alice-0007"));
    down.team_json(c.team(), |t| {
        t.insert("url", "https://127.0.0.1:9");
    });
    // dan-air 半分钟前刚同步过记下的 API key
    let presence = recent.cca.join("presence");
    DirBuilder::new().mode(0o700).create(&presence).expect("建 presence 目录");
    let synced = obj! { "v" => 1, "acc" => obj! {}, "synced" => fleet::now() - 30.0 };
    fsx::write(presence.join(format!("{}keys.json", c.lane())), dumps(&synced));
    let everyone = [ok, &denied, &down, &recent];
    for d in everyone {
        fleet::session(d, &CliSession::default());
        fleet::agent(t, d);
    }
    for (d, code) in [(ok, 200), (&denied, 403), (&down, 0), (&recent, 200)] {
        let reports = wait_sent(&format!("{} 的第一次上报", d.name), d, "report");
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0]["code"].i64(), code, "{}", d.name);
    }
    let why = |d: &Device| wait_for("没报成的原因", WAIT, || fleet::status(d, c.lane()).filter(|why| !why.is_empty()));
    assert_in("不认这台设备", &why(&denied));
    assert_in("连不上 Team 的同步服务器", &why(&down));
    assert!(matches!(fleet::status(ok, c.lane()).as_deref(), None | Some("")), "报成了：没有没报成的原因");
    // 开着会话：马上同步一次记下的 API key（不 force：最近同步过就不发）
    let key_codes: Vec<Vec<i64>> =
        everyone.iter().map(|d| d.sent(Some("keys")).iter().map(|r| r["code"].i64()).collect()).collect();
    assert_eq!(key_codes, [vec![200], vec![403], vec![0], vec![]]);
    for d in everyone {
        fleet::session(d, &CliSession { pid: SESSION_PID - 2, ..CliSession::default() });
        // 会话变了
    }
    fleet::more_rounds(&everyone, 3);
    // 两次上报之间隔了多久
    let gaps = |reports: &[Value]| -> Vec<f64> { reports.windows(2).map(|w| w[1]["done"].f64() - w[0]["done"].f64()).collect() };
    let sent = ok.sent(Some("report"));
    let sessions: Vec<usize> = sent.iter().take(2).map(|r| r["units"][c.aid.as_str()]["cli"].list().len()).collect();
    assert_eq!(sessions, [1, 2], "报成了的：会话变了马上再报");
    assert!(gaps(&sent[1..]).iter().all(|gap| *gap > 30.0), "没到心跳的时候（60 秒上下）转多少圈都不再发");
    assert_eq!(denied.sent(Some("report")).len(), 1, "服务器不认这台设备：转了几圈、会话变了都不重发（10 分钟后再试）");
    assert!(
        gaps(&down.sent(Some("report"))).iter().all(|gap| *gap > 5.0),
        "连不上：转了几圈、会话变了都不重发，退避到点（10 秒）才再试"
    );
    let key_syncs: Vec<usize> = everyone.iter().map(|d| d.sent(Some("keys")).len()).collect();
    assert_eq!(key_syncs, [1, 1, 1, 0], "记下的 API key 隔 10 分钟才再同步，不是每圈都同步");
}

// 共享 API key：官方账号记下备查的那一把，经服务器加密同步给登录了这个账号的其他设备

/// 测试用的 key：sk-ant-api03- 后面 n 个同一个字符（原来的 K1 是 40 个 a、K2 是 50 个 b、K3 是 30 个 c）
fn api_key(fill: char, n: usize) -> String {
    format!("sk-ant-api03-{}", fill.to_string().repeat(n))
}

/// K1、K2 的掩码
const HINT1: &str = "sk-ant-…aaaa";
const HINT2: &str = "sk-ant-…bbbb";

/// [`Device::sync`] 说同步成了
fn synced() -> Option<String> {
    Some(String::new())
}

/// `item in (v or [])`：v 是数组时看有没有这一项，是对象时看有没有这个键；没有 v、是 null 都算没有
fn holds(v: Option<&Value>, item: &str) -> bool {
    match v {
        Some(Value::List(l)) => l.iter().any(|x| x.as_str() == Some(item)),
        Some(Value::Obj(o)) => o.contains_key(item),
        _ => false,
    }
}

/// 密文的格式：两把密钥由盐、工具、邮箱、登录身份派生；加密后认证（带着账号摘要）；明文补齐到 256 字节的整数倍。
/// 每次的 nonce 不同、形状不对的密文不认、认证过内容不对的不认（逐个）：cli/src/cmd/keyshare.rs 的单元测试
/// seal_then_open_round_trips_with_a_random_nonce、open_refuses_junk_like_python_test。
/// （“服务器收这个形状”这里不单独查：服务器收下了才取得回来；parse_blob 认什么见 cli/src/server/wire.rs 的 checks_match_python）
fn keyshare_crypto(t: &mut Ctx) {
    const ALICE: &str = "alice(Alice@Example.com)";
    let c = client(t);
    // spec-air：照文档自己封、自己发的另一台设备
    let (dev, other, aid) = (&c.dev, c.device("spec-air", None), c.aid.as_str());
    c.machines(&[dev, &other]);
    fsx::write(dev.cca.join("accounts.json"), dumps(&obj! { ALICE => "US-A" })); // 邮箱不分大小写
    dev.login_as(Some("u-1234-5678"), ALICE);
    let keyset = |s: &str| dev.keyset_as(s, ALICE);
    let stored = || dev.stored_as(ALICE);
    let acct = fleet::ident("Alice@Example.com");
    let at = || dev.config()["key_hints"][acct.as_str()]["at"].clone();
    let k = spec::derive(spec::KEYSHARE, c.salt(), "claude", "alice@example.com", "u-1234-5678");
    let key = api_key('x', 60);
    keyset(&key);
    assert_eq!(dev.sync(), synced());
    let b = c.server_blob(aid);
    assert_eq!(
        spec::keyshare_open(&k, aid, &b),
        Some(obj! { "v" => 1, "key" => key.as_str(), "at" => at() }),
        "照文档能解开：密钥和文档写的一样"
    );
    // 服务器上的那一份换成 blob，这台设备再同步一次
    let put = |blob: &Value| {
        assert_eq!(other.post(c.port(), "keys", &obj! { "put" => obj! { aid => blob } }).0, 200);
        assert_eq!(dev.sync(), synced());
    };
    // 比这台设备记下的新
    let newer = obj! { "v" => 1, "key" => api_key('y', 60), "at" => fleet::now() };
    let newer_is_stored = ((0, newer["key"].str().to_string()), newer["at"].clone());
    put(&spec::keyshare_seal(&k, aid, &newer));
    assert_eq!((stored(), at()), newer_is_stored, "照文档封的 cca 也认");
    // 认证不过的、内容不对的都不认：这台设备上的不动，服务器上的换回这台设备的（自己修好）
    // 更新的：认了的话这台设备上的会换成它
    let bad = obj! { "v" => 1, "key" => api_key('z', 60), "at" => fleet::now() };
    let sealed = spec::keyshare_seal(&k, aid, &bad);
    let mut flipped = crypto::b64_decode(sealed["c"].str()).expect("密文是 base64");
    flipped[5] ^= 1;
    let keys_for = |salt: &str, tool: &str, who: &str| spec::derive(spec::KEYSHARE, salt, tool, "alice@example.com", who);
    for (what, blob) in [
        ("没登录这个账号（身份不对）", spec::keyshare_seal(&keys_for(c.salt(), "claude", "u-other-0000"), aid, &bad)),
        ("换了盐", spec::keyshare_seal(&keys_for(&"11".repeat(16), "claude", "u-1234-5678"), aid, &bad)),
        ("同一个邮箱的 Codex 账号", spec::keyshare_seal(&keys_for(c.salt(), "codex", "u-1234-5678"), aid, &bad)),
        ("挪到别的账号上：认证不过", spec::keyshare_seal(&k, &"b".repeat(32), &bad)),
        ("改过的", with(&sealed, "c", crypto::b64(&flipped))),
        ("版本不对", spec::keyshare_seal(&k, aid, &with(&bad, "v", 2))),
        ("key 太短", spec::keyshare_seal(&k, aid, &with(&bad, "key", "short"))),
        ("key 里有空格、中文", spec::keyshare_seal(&k, aid, &with(&bad, "key", "带空格 的 key"))),
    ] {
        put(&blob);
        assert_eq!((stored(), at()), newer_is_stored, "{what}");
        assert_eq!(spec::keyshare_open(&k, aid, &c.server_blob(aid)), Some(newer.clone()), "{what}");
    }
    // 删除和 key 一样长：服务器分不出来
    dev.ok(&["key-forget", ALICE]);
    assert_eq!(dev.sync(), synced());
    let tomb = c.server_blob(aid);
    let gone = spec::keyshare_open(&k, aid, &tomb).expect("照文档解得开");
    assert_eq!((&gone["key"], stored().0, gone["at"].f64() > newer["at"].f64()), (&Value::Null, 44, true));
    assert_eq!(tomb["c"].str().len(), b["c"].str().len(), "删除和 key 一样长：服务器分不出来");
    keyset(&"k".repeat(4096));
    assert_eq!(dev.sync(), synced());
    let longest = c.server_blob(aid);
    assert_eq!(spec::keyshare_open(&k, aid, &longest).expect("照文档解得开")["key"].str(), "k".repeat(4096));
}

/// 两台登录了 alice 的设备（alice-mbp、bob-mini）和一台没登录的（carol-air）：记下、换掉、删掉都同步，新的胜出；
/// 服务器、没登录的设备看不到 key；重放旧的、伪造的不认，传回自己的（自己修好）；删账号只删这台设备上的
fn keyshare(t: &mut Ctx) {
    let cl = client(t);
    let (k1, k2, k3) = (api_key('a', 40), api_key('b', 50), api_key('c', 30));
    let a = &cl.dev;
    let b = cl.device("bob-mini", Some("u-alice-0001"));
    let c = cl.device("carol-air", None);
    cl.machines(&[a, &b, &c]);
    a.login(Some("u-alice-0001"));
    let (aid, acct) = (cl.aid.as_str(), Device::acct_ident());
    let acct = acct.as_str();
    let k = spec::derive(spec::KEYSHARE, cl.salt(), "claude", "alice@example.com", "u-alice-0001");
    let on_server = || spec::keyshare_open(&k, aid, &cl.server_blob(aid)).expect("照文档解得开");
    assert_eq!(
        (a.keyset_as(&k1, "bob@example.com"), a.ok(&["key-share", "bob@example.com"])),
        (String::new(), String::new()),
        "这台设备自己的账号（Team 没给的）：只记在这台设备上，不同步"
    );
    assert_eq!(a.keyset(&k1).trim(), "shared", "Team 给的账号、开着设备统计：说一声会同步");
    let kicks = wait_for("后台起同步", WAIT, || Some(a.kicks()).filter(|kicks| !kicks.is_empty()));
    assert_eq!(kicks, ["_key-sync force"], "马上在后台同步（就这一次：bob@example.com 的那一把没起）");
    assert_eq!(fsx::mode(a.cca.join("presence").join(format!("{}keys.json", cl.lane()))), 0o600);
    assert_eq!(a.sync(), synced());
    let raw = fsx::read_text(cl.d().join("keys.json"));
    for secret in [k1.as_str(), "alice@example.com", "u-alice-0001", &"a".repeat(40)] {
        assert!(!raw.contains(secret), "服务器上没有 key、邮箱、登录身份：{secret}");
    }
    assert_eq!(key_list(&json::parse(&raw).expect("keys.json 是 JSON")["acc"]), [aid]);
    let at1 = a.config()["key_hints"][acct]["at"].clone();
    assert_eq!(on_server()["at"], at1, "传上去的时间是记下的时间");
    assert_eq!(c.sync(), synced(), "没登录 alice 的设备：不问也不传");
    assert_eq!(c.sent(None), Vec::<Value>::new());
    assert!(!c.cca.join("secrets").exists());
    assert_eq!(b.sync(), synced());
    assert_eq!(b.stored(), (0, k1.clone()), "登录了 alice 的另一台设备取回来了");
    let sync_error = b.keys_json().expect("同步过了：keys.json 在").get("error").cloned().unwrap_or(Value::Null);
    assert_eq!(
        (b.hint(), b.ok(&["key-share", Device::KEY]).trim(), sync_error),
        (obj! { "saved" => true, "hint" => HINT1, "from_device" => "alice-mbp", "saved_epoch" => &at1 }, "1", Value::Null),
        "App 照记下的说：从哪台设备来的、什么时候记下的；会同步、没有没同步成的原因"
    );
    let ha = a.hint();
    assert_eq!((&ha["from_device"], &ha["saved_epoch"]), (&Value::Null, &at1), "这台设备自己记下的：没有来源");
    let blob1 = cl.server_blob(aid);
    // bob-mini 换了一把：新的胜出，alice-mbp 取回来
    b.keyset(&k2);
    assert_eq!((b.sync(), a.sync()), (synced(), synced()));
    assert_eq!(a.stored(), (0, k2.clone()));
    let ha = a.hint();
    assert_eq!((ha["from_device"].str(), ha["hint"].str()), ("bob-mini", HINT2));
    assert_eq!(a.sync(), synced(), "再同步一次：没有新的，什么都不变");
    assert_eq!(a.stored(), (0, k2.clone()));
    // carol-air（在名单里、没登录 alice）把旧的那一份传回去（重放），再传一份伪造的：都不认，传回新的
    let forged = obj! { "n" => &blob1["n"], "c" => crypto::b64(&crypto::random(256)), "t" => &blob1["t"] };
    for bad in [&blob1, &forged] {
        assert_eq!(c.post(cl.port(), "keys", &obj! { "put" => obj! { aid => bad } }).0, 200);
        assert_eq!(a.sync(), synced());
        assert_eq!(a.stored(), (0, k2.clone()), "旧的、认证不过的不认");
        assert_eq!(on_server()["key"].str(), k2, "服务器上的旧了、坏了：传回这台设备的（自己修好）");
    }
    // alice-mbp 删掉：bob-mini 上同步来的也删掉
    a.ok(&["key-forget", Device::KEY]);
    assert_eq!(a.stored().0, 44);
    assert_eq!(a.sync(), synced());
    assert_eq!(cl.server_blob(aid)["c"].str().len(), blob1["c"].str().len(), "删除和 key 一样长");
    assert_eq!(b.sync(), synced());
    assert_eq!(b.stored().0, 44, "删掉也同步");
    assert!(!holds(b.config().get("ref_keys"), acct));
    assert!(!b.hint()["saved"].bool());
    // 记下 K3、两边都有了；bob-mini 删掉这个账号（cca rm）：只删它自己的，不同步，也不再取回来
    a.keyset(&k3);
    assert_eq!((a.sync(), b.sync()), (synced(), synced()));
    assert_eq!(b.stored(), (0, k3.clone()));
    fsx::write(b.cca.join("accounts.json"), "{}");
    b.ok(&["secret-del", Device::KEY]);
    assert_eq!((b.sync(), a.sync()), (synced(), synced()));
    assert_eq!((b.stored().0, a.stored()), (44, (0, k3.clone())));
    // 账号加回来、还登录着：服务器上的取回来（钥匙串里的被删了也一样）
    fsx::write(b.cca.join("accounts.json"), dumps(&obj! { Device::KEY => "US-A" }));
    assert_eq!(b.sync(), synced());
    assert_eq!(b.stored(), (0, k3.clone()));
    // 退出登录了：不问也不传（解不开）；本机的 key 还在
    b.login(None);
    b.keyset(&k1);
    let n = b.sent(None).len();
    assert_eq!(b.sync(), synced());
    assert_eq!(b.sent(None).len(), n);
    assert_eq!(a.stored(), (0, k3.clone()));
    assert_eq!(a.sync(), synced());
    assert_eq!(a.stored(), (0, k3.clone()), "没登录时记下的：登录以前不传");
    b.login(Some("u-alice-0001"));
    assert_eq!((b.sync(), a.sync()), (synced(), synced()));
    assert_eq!(a.stored(), (0, k1.clone()), "登录以后：按记下的时间比，新的胜出");
    let kicks = wait_for("后台起同步", WAIT, || Some(a.kicks()).filter(|kicks| kicks.len() >= 3));
    assert_eq!(kicks, ["_key-sync force"; 3], "在这台设备上记下、删掉（K1、删掉、K3）各起一次同步；照服务器上的改不起");
}

/// 没开设备统计时记下的（不记 keys.json）、旧版本记下的（key_hints 里没有时间）：开了以后按记下的时间算
fn keyshare_before_presence(t: &mut Ctx) {
    let cl = client(t);
    let (k1, k2) = (api_key('a', 40), api_key('b', 50));
    let a = &cl.dev;
    let b = cl.device("bob-mini", Some("u-alice-0002"));
    cl.machines(&[a, &b]);
    a.login(Some("u-alice-0002"));
    let (aid, acct) = (cl.aid.as_str(), Device::acct_ident());
    fsx::write(a.cca.join("config.json"), dumps(&obj! { "presence" => false }));
    assert_eq!(a.keyset(&k1), "", "关掉了设备统计：不说会同步");
    assert!(!a.cca.join("presence").exists(), "也不建 presence 目录");
    assert_eq!(a.sync(), None);
    a.config_edit(|c| {
        c.remove("presence").expect("config.json 里有 presence");
    });
    // bob-mini 的是旧版本记下的：key_hints 里没有时间，按密钥文件的修改时间算（比 alice-mbp 记下的早）
    fsx::write(b.cca.join("config.json"), dumps(&obj! { "presence" => false })); // 记的时候不经设备统计（keys.json 里没有它）
    b.keyset(&k2);
    b.config_edit(|c| {
        c.remove("presence").expect("config.json 里有 presence");
        let hints = c.get_mut("key_hints").and_then(Value::as_obj_mut).expect("config.json 里有 key_hints");
        let hint = hints.get_mut(&acct).and_then(Value::as_obj_mut).expect("key_hints 里有这个账号");
        hint.remove("at").expect("记下的时间");
    });
    assert!(!b.cca.join("presence").exists());
    let old = fleet::now() - 86400.0;
    utime(&b.cca.join("secrets").join(format!("cca-ref-{acct}")), old, old);
    assert_eq!((b.sync(), a.sync()), (synced(), synced()));
    assert_eq!(a.stored(), (0, k1.clone()), "alice-mbp 记下的新：不动");
    assert_eq!(b.sync(), synced());
    assert_eq!(b.stored(), (0, k1.clone()), "bob-mini 的旧：换成新的");
    let k = spec::derive(spec::KEYSHARE, cl.salt(), "claude", "alice@example.com", "u-alice-0002");
    assert_eq!(spec::keyshare_open(&k, aid, &cl.server_blob(aid)).expect("照文档解得开")["key"].str(), k1);
    let kicks: Vec<String> = a.kicks().into_iter().chain(b.kicks()).collect();
    assert_eq!(kicks, Vec::<String>::new(), "关掉了设备统计时记下的：不起同步");
}

/// 账号的 .claude.json 读不出（半写、手改坏了）：当作没登录，这一轮不同步它，不退出；上报进程里同步出的错（config.json 读不了时
/// 退出的那种也算）不停掉上报；其他设备上删掉的、这台设备的钥匙串里没删掉（锁着）：不记成删掉了，下一轮再删
fn keyshare_bad_files(t: &mut Ctx) {
    let cl = client(t);
    let k1 = api_key('a', 40);
    let a = &cl.dev;
    let mut b = cl.device("bob-mini", Some("u-alice-0005"));
    b.fake("security", fleet::KEYCHAIN); // bob-mini 的 key 存在（假的）钥匙串里
    b.env.remove("CCA_KEYSTORE").expect("设备的环境里本来有 CCA_KEYSTORE");
    cl.machines(&[a, &b]);
    let cj = a.login(Some("u-alice-0005"));
    a.keyset(&k1);
    let good = fsx::read_text(&cj);
    let on_server = || fsx::try_read_text(cl.d().join("keys.json"));
    let before = on_server();
    fsx::write(&cj, "{\"oauthAccount\": ");
    assert_eq!(a.sync(), synced(), "读不出登录身份：当作没登录，不退出（以前 die：上报进程跟着退出）");
    assert_eq!(on_server(), before, "没传");
    fsx::write(&cj, &good);
    assert_eq!((a.sync(), b.sync()), (synced(), synced()));
    assert_eq!(b.stored(), (0, k1.clone()));
    a.ok(&["key-forget", Device::KEY]);
    assert_eq!(a.sync(), synced());
    let raw = b.keys_json().expect("同步过了：keys.json 在")["acc"].clone();
    fsx::write(b.home.join("keychain_stuck"), ""); // 钥匙串锁着：删不掉
    let err = b.sync().expect("这一轮同步过了");
    assert_in("其他设备上删掉的 API key 没能从这台设备的钥匙串里删掉", &err);
    assert_eq!(b.stored(), (0, k1.clone()));
    assert_eq!(b.keys_json().expect("keys.json 在")["acc"], raw, "没删掉：不记成删掉了");
    std::fs::remove_file(b.home.join("keychain_stuck")).expect("删 keychain_stuck");
    assert_eq!(b.sync(), synced(), "下一轮再删");
    assert_eq!(b.stored().0, 44);
    // 上报进程：同步 API key 时出错退出的那种（config.json 读不了）也接着报
    fsx::write(a.cca.join("config.json"), "{oops");
    let j = a.keys_json().expect("同步过了：keys.json 在");
    // 上次同步是一小时以前的事：这一圈该同步了
    let hour_ago = with(&j, "synced", j["synced"].f64() - 3600.0);
    fsx::write(a.cca.join("presence").join(format!("{}keys.json", cl.lane())), dumps(&hour_ago));
    fleet::session(a, &CliSession::default());
    fleet::agent(t, a);
    let reports = wait_sent("照常上报", a, "report");
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!((reports[0]["code"].i64(), key_list(&reports[0]["units"])), (200, vec![cl.aid.as_str()]));
    assert_in("config.json", &fsx::read_text(a.home.join("agent.err"))); // 同步出的错说了一声
}

/// 记下、删掉之后的同步（force）碰上正在跑的一轮：等它跑完再同步，不放弃（不然这次的改动要等到下一轮才传出去）
fn keyshare_force_waits(t: &mut Ctx) {
    let cl = client(t);
    let dev = &cl.dev;
    cl.machines(&[dev]);
    dev.login(Some("u-alice-0004"));
    dev.keyset(&api_key('a', 40));
    let lock = dev.cca.join("presence").join(format!("{}keys-sync.lock", cl.lane()));
    // 像正在跑的一轮那样拿着锁（flock）；文件关上就放了
    let hold = || {
        let held = OpenOptions::new().append(true).create(true).open(&lock).expect("打开锁文件");
        // SAFETY: held 是刚打开的文件，flock 只是给它加锁
        let rc = unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) };
        assert_eq!(rc, 0, "加锁：{}", std::io::Error::last_os_error());
        held
    };
    let held = hold();
    // 这里测的就是等：0.6 秒以后放锁
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(held);
    });
    let t0 = Instant::now();
    assert_eq!(dev.sync(), synced());
    let waited = t0.elapsed();
    release.join().expect("放锁的那条线");
    assert!(waited >= Duration::from_millis(500), "等正在跑的那一轮放锁：只等了 {waited:?}");
    let asked: Vec<Vec<&str>> =
        dev.sent(None).iter().map(|r| ["get", "put"].into_iter().filter(|k| r.get(k).is_some()).collect()).collect();
    assert_eq!(asked, [vec!["get"], vec!["put"]], "放锁以后照常同步：问一次，传上去");
    // 上次同步是一小时以前的事（到时候了）：不是 force 的碰上正在跑的一轮就走
    let j = dev.keys_json().expect("同步过了：keys.json 在");
    let hour_ago = with(&j, "synced", j["synced"].f64() - 3600.0);
    fsx::write(dev.cca.join("presence").join(format!("{}keys.json", cl.lane())), dumps(&hour_ago));
    let held = hold();
    let t0 = Instant::now();
    assert_eq!(dev.sync_mode(""), None, "不是 force 的：有一轮在跑就走");
    // 等的话要等到放锁（这里一直不放：force 的那种等满 35 秒）
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert_eq!(dev.sent(None).len(), 2);
    drop(held);
    assert_eq!(dev.sync_mode(""), synced(), "没有别的在跑：到时候了就同步");
}

// 折合金额：各台设备算自己的，经服务器交换密文

/// 一个请求最多这么多个账号的折合金额：服务器的 MAX_ACCOUNTS（一次上报 / 查询最多几个账号），辅助程序照它分批（它的 COST_BATCH）。
/// 原来的测试里也是直接写的 64
const COST_BATCH: usize = 64;

/// `len(v or ())`：数组有几项、对象有几个键；没有、null 是 0
fn size(v: Option<&Value>) -> usize {
    match v {
        Some(Value::List(l)) => l.len(),
        Some(Value::Obj(o)) => o.len(),
        _ => 0,
    }
}

/// 各行的名字（`[x['name'] for x in rows]`）
fn names<'a>(rows: impl IntoIterator<Item = &'a Value>) -> Vec<&'a str> {
    rows.into_iter().map(|row| row["name"].str()).collect()
}

/// 现在这个小时的前一个小时是从哪一秒开始的（折合金额按小时记）
fn last_hour() -> i64 {
    (fleet::now() / 3600.0).floor() as i64 * 3600 - 3600
}

/// Team 给的账号比一个请求放得下的多（服务器一次最多 64 个账号：COST_BATCH）：分几次问、几次传，都同步得上
fn cost_many_accounts(t: &mut Ctx) {
    let cl = client(t);
    let emails: Vec<String> = (0..70).map(|i| format!("u{i:02}@example.com")).collect();
    let (mut table, mut given) = (Obj::new(), Obj::new());
    for (i, e) in emails.iter().enumerate() {
        table.insert(format!("u{i:02}({e})"), "US-A");
        given.insert(e.as_str(), "US-A");
    }
    let (table, accounts) = (Value::Obj(table), obj! { "accounts" => obj! { "claude" => given } });
    let (a, b) = (cl.device_with("many-mbp", &table), cl.device_with("many-mini", &table));
    for d in [&a, &b] {
        d.set_doc(cl.team(), &cl.doc(&accounts));
    }
    cl.machines(&[&a, &b]);
    let hour = last_hour().to_string();
    let rows: Vec<Value> = (1..=70).map(|n| obj! { hour.as_str() => list![1000000 * n, 1, 1, 1, 0, 0, 0] }).collect();
    let (mut who, mut acc) = (Obj::new(), Obj::new());
    for (e, row) in emails.iter().zip(&rows) {
        who.insert(format!("claude:u-{e}"), e.as_str());
        acc.insert(format!("claude:u-{e}"), row);
    }
    std::fs::create_dir_all(a.cca.join("cost")).expect("建 cost 目录");
    let local = obj! { "v" => 1, "build" => "x", "who" => who, "acc" => acc, "um" => obj! {}, "orphan" => obj! {} };
    fsx::write(a.cca.join("cost").join("local.json"), dumps(&local));
    for d in [&a, &b] {
        assert_eq!(d.cost_sync(true)["state"].str(), "ok");
        let sizes: Vec<usize> = d
            .sent(Some("ledger"))
            .iter()
            .map(|r| ["get", "have", "put"].iter().map(|k| size(r.get(k))).max().unwrap_or(0))
            .collect();
        assert!(sizes.len() >= 2 && sizes.iter().all(|n| *n <= COST_BATCH), "{sizes:?}");
    }
    let slots = fsx::read_json(cl.d().join("cost-slots.json"))["acc"].clone();
    let uploaded: Vec<Vec<&str>> =
        emails.iter().map(|e| slots.get(&spec::aid_of(cl.salt(), "claude", e)).map(sorted_keys).unwrap_or_default()).collect();
    assert_eq!(uploaded, vec![vec![a.fp()]; 70], "b 没用过、没登录：不传");
    let got = b.cost(&format!("{}remote.json", cl.lane()))["acc"].clone();
    for (e, row) in emails.iter().zip(&rows) {
        let hours: Vec<&Value> = got[format!("claude:{e}").as_str()].obj().values().map(|x| &x["h"]).collect();
        assert_eq!(hours, [row], "b 取到了 70 个账号的：{e}");
    }
}

/// alice-mbp、bob-mini 登录着 alice，carol-air 没有：三台都取得到别的设备的（3.6.6 起密钥整个 Team 共用，不用登录），
/// 只有用过、登录着的传自己的那一份；服务器上只有密文；伪造的、重放的不认；没变的不重传、不重取；移除了的设备传的服务器删掉、
/// 哪儿都不列；用过却没传过的设备说一声
fn cost_sync(t: &mut Ctx) {
    let cl = client(t);
    let a = &cl.dev;
    let b = cl.device("bob-mini", Some("u-alice-0005"));
    let c = cl.device("carol-air", None);
    cl.machines(&[a, &b, &c]);
    a.login(Some("u-alice-0005"));
    let (aid, slot) = (cl.aid.as_str(), "claude:alice@example.com");
    let (remote, sync_file) = (format!("{}remote.json", cl.lane()), format!("{}sync.json", cl.lane()));
    let hour = last_hour();
    let (this_hour, day_before) = (hour.to_string(), (hour - 86400).to_string());
    let (this_hour, day_before) = (this_hour.as_str(), day_before.as_str());
    let a_rows = obj! { this_hour => list![5000000, 2, 100, 10, 0, 0, 0] };
    let b_rows = obj! { day_before => list![3000000, 1, 50, 5, 0, 0, 0] };
    a.cost_local("u-alice-0005", &a_rows);
    b.cost_local("u-alice-0005", &b_rows);
    c.cost_local_as("u-carol-0001", &obj! { this_hour => list![1000000, 1, 1, 1, 0, 0, 0] }, "carol@example.com");
    let r = a.cost_sync(true);
    assert_eq!(r["state"].str(), "ok", "{r}");
    let raw = fsx::read_text(cl.d().join("cost-slots.json"));
    for secret in ["alice@example.com", "u-alice-0005", "5000000", this_hour] {
        assert!(!raw.contains(secret), "服务器上没有邮箱、身份、金额、时间：{secret}");
    }
    let fa = a.fp();
    assert_eq!(key_list(&cl.slots(aid)), [fa]);
    assert_eq!(b.cost_sync(true)["state"].str(), "ok");
    let rb = b.cost(&remote)["acc"][slot].clone();
    let seen: Vec<(&str, &Value)> = rb.obj().values().map(|e| (e["name"].str(), &e["h"])).collect();
    assert_eq!(seen, [("alice-mbp", &a_rows)], "bob-mini 取到了 alice-mbp 的");
    assert_eq!(c.cost_sync(true)["state"].str(), "ok", "carol-air 没登录 alice 也取得到");
    let rc = c.cost(&remote)["acc"][slot].clone();
    let mut seen: Vec<(&str, &Value)> = rc.obj().values().map(|e| (e["name"].str(), &e["h"])).collect();
    seen.sort_by_key(|(name, _)| *name);
    assert_eq!(seen, [("alice-mbp", &a_rows), ("bob-mini", &b_rows)], "carol-air 也看得到另外两台的");
    // App 看到的这个账号（就这一个）
    let (tid, asked) = (Value::from(cl.tid()), list![list!["claude", Device::KEY, Value::Null, cl.tid()]]);
    let app = |d: &Device| only(&d.cost_view(&tid, &asked, &list![])["accounts"]).clone();
    let v = app(&c);
    assert_eq!(
        (&v["summary"]["days_usd"], names(v["devices"].list()), v["sync"]["state"].str()),
        (&Value::from(8.0), vec!["alice-mbp", "bob-mini"], "ok"),
        "carol-air 的 App 里：这个账号两台设备的用量（它自己没用过）"
    );
    let mut both = vec![fa, b.fp()];
    both.sort_unstable();
    assert_eq!(sorted_keys(&cl.slots(aid)), both, "carol-air 没用过、没登录 alice：不传自己的那一份");
    let at_a = cl.slots(aid)[fa]["at"].clone();
    let n = a.sent(None).len();
    assert_eq!(a.cost_sync(true)["state"].str(), "ok");
    let posts = a.sent(None).split_off(n);
    assert_eq!((posts.len(), posts[0].get("put").is_some()), (1, false), "这台设备的没变：只问，不传");
    assert_eq!(cl.slots(aid)[fa]["at"], at_a);
    let ra = a.cost(&remote)["acc"][slot].clone();
    assert_eq!(names(ra.obj().values()), ["bob-mini"]);
    let n = a.sent(None).len();
    assert_eq!(a.cost_sync(true)["state"].str(), "ok");
    let posts = a.sent(None).split_off(n);
    let have = posts[0]["have"][aid].obj();
    assert_eq!(have.values().filter(|tag| tag.is_null()).count(), 0);
    assert_eq!(have.len(), 2, "带着已经有的认证码（自己的、bob-mini 的）：没变的服务器不回密文");
    let v = app(a);
    assert_eq!(
        (&v["summary"]["days_usd"], names(v["devices"].list()), v["sync"]["state"].str()),
        (&Value::from(8.0), vec!["alice-mbp", "bob-mini"], "ok"),
        "App 看到的：两台设备加起来"
    );
    // carol-air 在名单里，在自己的那一份里放伪造的（不是这个 Team 的密钥）、重放 alice-mbp 的：认证里带着设备指纹，都不认
    let huge = obj! { "v" => 1, "at" => fleet::now(), "h" => obj! { this_hour => list![1_000_000_000, 1, 0, 0, 0, 0, 0] }, "um" => list![] };
    let fake = spec::cost_seal(&spec::team_keys(&"11".repeat(16), spec::COST), aid, c.fp(), &huge, spec::COST);
    let replayed = cl.slots(aid)[fa]["blob"].clone();
    let with_hours = |acc: &Value| -> Vec<String> {
        acc.obj().values().filter(|e| e.get("h").is_some()).map(|e| e["name"].str().to_string()).collect()
    };
    for bad in [&fake, &replayed] {
        assert_eq!(c.post(cl.port(), "ledger", &obj! { "put" => obj! { aid => bad } }).0, 200);
        assert_eq!(a.cost_sync(true)["state"].str(), "ok");
        assert_eq!(with_hours(&a.cost(&remote)["acc"][slot]), ["bob-mini"], "伪造的、重放的都不认");
    }
    // 移除 bob-mini：服务器删掉它传的，这台设备上也不再有它；dan（用过 alice、没传过）
    let d = cl.device("dan-air", Some("u-alice-0005"));
    cl.machines(&[a, &c, &d]);
    assert!(cl.slots(aid).get(b.fp()).is_none(), "服务器上删掉了");
    let report = obj! { "units" => obj! { aid => unit(&[[60, 0]], Value::Null) }, "end" => Value::Null };
    assert_eq!(d.post(cl.port(), "report", &report).0, 200);
    assert_eq!(a.cost_sync(true)["state"].str(), "ok");
    let ra = a.cost(&remote)["acc"][slot].clone();
    assert_eq!(
        (with_hours(&ra), ra.get(b.fp()).is_some()),
        (Vec::<String>::new(), false),
        "没有 bob-mini 了（carol-air 那一份是上面重放的，解不开）"
    );
    let sync = a.cost(&sync_file);
    assert_eq!(names(sync["acc"][slot]["missing"].list()), ["dan-air"]);
    let v = app(a);
    let listed: Vec<(&str, bool)> = v["devices"].list().iter().map(|x| (x["name"].str(), x["revoked"].bool())).collect();
    assert_eq!(
        (listed, &v["summary"]["days_usd"], names(v["sync"]["missing"].list())),
        (vec![("alice-mbp", false)], &Value::from(5.0), vec!["dan-air"]),
        "合计里也没有它了"
    );
    for f in [&remote, &sync_file] {
        assert_eq!(fsx::mode(a.cca.join("cost").join(f)), 0o600, "{f}");
    }
    for f in ["remote.json", "sync.json"] {
        assert!(!a.cca.join("cost").join(f).exists(), "没有别的服务器的那一份（不带前缀的）");
    }
}

// 服务器自己的验签和 ssh-keygen 对：cli/src/server/sshsig.rs 的 real_ssh_keygen_signatures、s_plus_the_group_order_is_refused
