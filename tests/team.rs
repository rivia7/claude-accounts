//! Team（cca team）：辅助程序（cca-core）这一头的建 Team、接上服务器、邀请、加入、同步配置、授权、移除、退出，对着真的服务器
//! （127.0.0.1、随机端口；辅助程序的 team-server，见 kit::server）。各台设备各一个临时 HOME（kit::fleet 的 Device）。
//! 另一头有时由 kit::spec（照 server/README.md 独立实现的那一份）扮演：两份实现封的、签的要能互相解开、验过。不联网。
//! 要 ssh-keygen、openssl、curl（和原来一样：没有就是失败，不跳过）。
//!
//! 原来是 tests/team.py。夹具（服务器、设备、假命令、会话和上报进程）在 kit::fleet、kit::pki。原来另起一个 Python 进程扮演的
//! “不是 Team 的同步服务器的 HTTPS 服务”在这里是这个进程里的一条线（NotTeam）。服务器的状态文件（state.json、team-docs.json、
//! keys.json、cost-slots.json）照原来那样直接读、直接改：两份服务器写的是同一个样子。

use kit::fleet::{self, ident, CliSession, Device, Server, ServerOpts, SESSION_PID, T0, WAIT};
use kit::fsx::{self, wait_for};
use kit::harness::{Ctx, Module};
use kit::json::{self, Obj, Style, Value};
use kit::py::{splitlines, truthy};
use kit::{case, crypto, list, obj, pki, spec};
use regex::Regex;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "team",
    src: include_str!("team.rs"),
    default: true,
    tests: &[
        case!(admin_local),
        case!(lifecycle),
        case!(server_level),
        case!(rename),
        case!(aliases),
        case!(alias_per_tool),
        case!(alias_migrate),
        case!(alias_lock),
        case!(mail_forwarders),
        case!(nodes),
        case!(spec_admin),
        case!(records_lost_and_move),
        case!(accounts_follow),
        case!(pipelines),
        case!(agent),
        case!(ca_mode),
        case!(delete_plan),
        case!(invite_for_a_key),
        case!(import_mine),
        case!(config_file),
        case!(new_with_accounts),
        case!(server_lies),
        case!(co_admins),
    ],
};

/// 一行都没有（空的 stdout 拆出来的）
const NONE: [&str; 0] = [];

// ---- 小东西 ----

/// 路径当参数用（临时目录都是 UTF-8 的）
fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display()))
}

/// json.dumps(v) 默认的样子（原来改 admin.json、账号表、服务器的状态文件都是这样写回去的）
fn dumps(v: &Value) -> String {
    json::dumps(v, &Style::default())
}

/// 把 v 写成一个 JSON 文件（write_text(json.dumps(v))）
fn put(path: impl AsRef<Path>, v: &Value) {
    fsx::write(path, dumps(v));
}

/// dict(base, key=value)：抄一份、换掉（或者添上）一项
fn with(base: &Value, key: &str, value: impl Into<Value>) -> Value {
    let mut o = base.obj().clone();
    o.insert(key, value);
    Value::Obj(o)
}

/// 一层层往下取到一个对象来改（a['x']['y'][…] = … 的左边）：没有这个键、不是对象就 panic
fn sub<'a>(v: &'a mut Value, path: &[&str]) -> &'a mut Obj {
    let mut cur = v;
    for key in path {
        let o = cur.as_obj_mut().unwrap_or_else(|| panic!("{key} 的上一层不是对象"));
        cur = o.get_mut(key).unwrap_or_else(|| panic!("没有 {key} 这一项"));
    }
    cur.as_obj_mut().unwrap_or_else(|| panic!("{path:?} 不是对象"))
}

/// haystack 里有 needle（assertIn）
#[track_caller]
fn assert_in(needle: &str, haystack: &str) {
    assert!(haystack.contains(needle), "没有 {needle:?}：\n{haystack}");
}

/// haystack 里没有 needle（assertNotIn）
#[track_caller]
fn assert_not_in(needle: &str, haystack: &str) {
    assert!(!haystack.contains(needle), "不该有 {needle:?}：\n{haystack}");
}

/// 对象的键，照它里面的先后（list(d)）
fn key_list(v: &Value) -> Vec<&str> {
    v.obj().keys().collect()
}

/// 对象的键、或者数组里的各个字符串，照先后（list(x)：两种都这样数）
fn items(v: &Value) -> Vec<&str> {
    match v.as_obj() {
        Some(o) => o.keys().collect(),
        None => v.list().iter().map(Value::str).collect(),
    }
}

/// 同 items，排好（sorted(x)）
fn sorted_items(v: &Value) -> Vec<&str> {
    let mut out = items(v);
    out.sort_unstable();
    out
}

/// 一行里要的那几项（(r['a'], r['b'], …)：缺一项就 panic）
fn pick(r: &Value, keys: &[&str]) -> Value {
    Value::List(keys.iter().map(|k| r[*k].clone()).collect())
}

/// 数组里的各行按某一项记成对象（{r[key]: r for r in rows}：同名的后面的盖掉前面的，位置照头一次出现的）
fn keyed(rows: &Value, key: &str) -> Value {
    let mut by = Obj::new();
    for r in rows.list() {
        by.insert(r[key].str(), r.clone());
    }
    Value::Obj(by)
}

/// 对象里的每一项换成 f 给的（{k: f(r) for k, r in by.items()}）
fn mapped(by: &Value, f: impl Fn(&Value) -> Value) -> Value {
    let mut out = Obj::new();
    for (k, r) in by.obj().iter() {
        out.insert(k, f(r));
    }
    Value::Obj(out)
}

/// 数组里头一个 r[key] 是 want 的那一行（next(r for r in rows if r[key] == want)）；没有就 panic
#[track_caller]
fn find<'a>(rows: &'a Value, key: &str, want: &str) -> &'a Value {
    let found = rows.list().iter().find(|r| r[key] == Value::from(want));
    found.unwrap_or_else(|| panic!("没有 {key} 是 {want} 的一行：{rows}"))
}

/// 目录里各项的名字，排好（sorted(p.name for p in d.iterdir())）
fn names_in(dir: &Path) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("读 {}：{e}", dir.display()));
    let mut names: Vec<String> = entries.map(|e| e.expect("目录里的一项").file_name().to_string_lossy().into_owned()).collect();
    names.sort_unstable();
    names
}

/// 这个目录下（一层层往里）所有文件的内容接在一起，kicks.log 除外；不是 UTF-8 的字节换成替代符
fn all_text(root: &Path) -> String {
    let mut out = String::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("读 {}：{e}", dir.display())) {
            let entry = entry.expect("目录里的一项");
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                todo.push(path);
            } else if path.is_file() && entry.file_name() != "kicks.log" {
                out.push_str(&String::from_utf8_lossy(&fsx::read(&path)));
            }
        }
    }
    out
}

/// 配置里带着的节点的名字，照先后（[p['name'] for p in doc['nodes']['proxies']]）
fn proxy_names(doc: &Value) -> Vec<String> {
    doc["nodes"]["proxies"].list().iter().map(|p| p["name"].str().to_string()).collect()
}

// ---- 一份现成的 Team 配置 ----

/// 一个节点的定义（socks5，192.0.2.10，端口照名字的最后一个字母：US-A 是 1145）
fn node(name: &str) -> Value {
    let last = name.chars().last().expect("节点的名字不是空的") as u32;
    obj! { "name" => name, "type" => "socks5", "server" => "192.0.2.10", "port" => 1080 + last }
}

/// 一份现成的 Team 配置（管理员那台 admin.json 里的 config，见 load）：Claude 的 alice → US-A（别名 alice）、bob → US-B（经 US-C
/// 连出去），Codex 的 carol → US-A；各台设备经内置代理安装客户端用 US-D；节点的定义 US-A…US-E
fn config() -> Value {
    obj! {
        "accounts" => obj! {
            "claude" => obj! { "alice@example.com" => "US-A", "bob@example.com" => "US-B" },
            "codex" => obj! { "carol@example.com" => "US-A" },
            "agy" => obj! {},
        },
        "tool_aliases" => obj! { "claude" => obj! { "alice@example.com" => "alice" } },
        "aliases" => obj! {},
        "install_node" => "US-D",
        "nodes" => obj! {
            "group" => "OpenAi",
            "proxies" => list![node("US-A"), with(&node("US-B"), "dialer-proxy", "US-C"), node("US-C"), node("US-D"), node("US-E")],
        },
    }
}

// ---- 不是 Team 的同步服务器的一个 HTTPS 服务 ----

/// 不管对方问的是哪个名字，都给这一张证书。不用 kit::tlsd 的接入器：它要 rustls 读得了证书（核对证书和私钥配不配），
/// macOS 自带的 LibreSSL 出的自签证书是 v1 的，rustls 读不了——curl 不在乎这个（照公钥的指纹认），这里也就不核对
#[derive(Debug)]
struct OneCert(Arc<CertifiedKey>);

impl ResolvesServerCert for OneCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// 不是 Team 的同步服务器的一个 HTTPS 服务：什么请求都回 404（像 3.6.0 以前维护者的服务器、别的网站）。在这个进程里的一条线上
/// 一条条地接；丢掉它就停
struct NotTeam {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NotTeam {
    /// 拿这张证书和私钥（PEM 文件）在 127.0.0.1 上随便一个端口起来
    fn start(cert: &Path, key: &Path) -> NotTeam {
        let certs: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(&fsx::read(cert)).collect::<Result<_, _>>().expect("证书读得出来");
        let key = kit::tlsd::private_key(&fsx::read(key)).expect("私钥读得出来");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = provider.key_provider.load_private_key(key).expect("私钥用得了");
        let cfg = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS 的版本")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(OneCert(Arc::new(CertifiedKey::new(certs, key)))));
        let cfg = Arc::new(cfg);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑端口");
        let port = listener.local_addr().expect("绑上的端口").port();
        // 不等在 accept 上：隔一会儿看一眼该不该停（停的时候不用靠再连一次把它叫醒）
        listener.set_nonblocking(true).expect("监听的端口改成不等");
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    // 握不成、对方中途断开的不管
                    Ok((sock, _)) => drop(not_found(&cfg, sock)),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        });
        NotTeam { port, stop, thread: Some(thread) }
    }
}

impl Drop for NotTeam {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 一条连接：握手，读完一个请求（头和 Content-Length 说的那么长的内容），回 404 和一小段 JSON，关上
/// （HTTP/1.0，一条连接一个请求：原来的 http.server 就是这样）
fn not_found(cfg: &Arc<ServerConfig>, mut sock: TcpStream) -> io::Result<()> {
    let limit = Some(Duration::from_secs(10));
    // 接进来的连接在 macOS 上跟着监听的那个也是不等的：改回等着读写（有时限）
    sock.set_nonblocking(false)?;
    sock.set_read_timeout(limit)?;
    sock.set_write_timeout(limit)?;
    let mut conn = ServerConnection::new(cfg.clone()).map_err(io::Error::other)?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock)?;
    }
    let mut tls = StreamOwned::new(conn, sock);
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = tls.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
    let length: usize =
        head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    while buf.len() < head_end + length {
        let n = tls.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = br#"{"error":"not_found"}"#;
    write!(tls, "HTTP/1.0 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len())?;
    tls.write_all(body)?;
    tls.conn.send_close_notify();
    tls.flush()
}

// ---- 每个用例的夹具 ----

/// 一个用例的临时目录：各台设备的 HOME、各台服务器的目录、现生成的密钥都建在里面
struct Fx {
    d: PathBuf,
}

fn fx(t: &mut Ctx) -> Fx {
    Fx { d: t.tmp("cca-team-cli-") }
}

impl Fx {
    /// 一台设备（账号表是空的）
    fn device(&self, name: &str) -> Device {
        Device::new(&self.d.join(name), name, &obj! {})
    }

    /// 起一台 Team 的服务器（自己的目录 name、自己的证书）：(服务器, 地址, 证书公钥的指纹)。离开作用域时停掉
    fn server(&self, root: &str, name: &str) -> (Server, String, String) {
        self.server_on(root, 0, name)
    }

    /// 同 server，监听给的端口（0 是随便挑一个）：服务器停下、改了它的状态文件以后在原来的端口上再起
    fn server_on(&self, root: &str, port: u16, name: &str) -> (Server, String, String) {
        let d = self.d.join(name);
        if !d.exists() {
            fs::create_dir(&d).unwrap_or_else(|e| panic!("建 {}：{e}", d.display()));
            pki::make_cert(&d);
        }
        let srv = Server::start_with(&d, root, &ServerOpts::new().port(port));
        let url = format!("https://127.0.0.1:{}", srv.port);
        (srv, url, pki::openssl_pin(d.join("cert.pem")))
    }

    /// 一台管理员的设备、建好的 Team「测试组」（管理员这台叫 studio）、照它的根公钥起的服务器、接上：
    /// (管理员, 编号, 根公钥, 服务器, 地址, 指纹)
    fn new_team(&self) -> (Device, String, String, Server, String, String) {
        let admin = self.device("admin");
        let (tid, root, _) = three(lines(&admin, &["team-new", "测试组", "--device", "studio"]));
        let (srv, url, pin) = self.server(&root, "srv");
        let host = format!("127.0.0.1:{}", srv.port);
        assert_eq!(lines(&admin, &["team-attach", &tid, &url, &pin]), [host.as_str(), "studio", "1", "1", ""]);
        (admin, tid, root, srv, url, pin)
    }

    /// 起一个不是 Team 的同步服务器的 HTTPS 服务（NotTeam）：(服务, 地址, 证书公钥的指纹)
    fn not_team(&self) -> (NotTeam, String, String) {
        let d = self.d.join("not-team");
        fs::create_dir(&d).unwrap_or_else(|e| panic!("建 {}：{e}", d.display()));
        pki::make_cert(&d);
        let srv = NotTeam::start(&d.join("cert.pem"), &d.join("key.pem"));
        let url = format!("https://127.0.0.1:{}", srv.port);
        (srv, url, pki::openssl_pin(d.join("cert.pem")))
    }
}

/// stdout 拆成一行一项（out.split('\n')[:-1]：最后一个换行后面的不算）
fn split_lines(out: &str) -> Vec<String> {
    let mut parts: Vec<String> = out.split('\n').map(str::to_string).collect();
    parts.pop();
    parts
}

/// 一条该成的命令的 stdout，一行一项
fn lines(dev: &Device, argv: &[&str]) -> Vec<String> {
    lines_in(dev, argv, "")
}

/// 同 lines，stdin 是喂给它的输入（邀请链接、节点的定义）
fn lines_in(dev: &Device, argv: &[&str], stdin: &str) -> Vec<String> {
    split_lines(&dev.ok_with(argv, stdin, &[]))
}

/// 没成的命令的原因码（stdout 是「原因码\t原因」，退出码 1）
#[track_caller]
fn code(dev: &Device, argv: &[&str]) -> String {
    code_in(dev, argv, "")
}

/// 同 code，带输入
#[track_caller]
fn code_in(dev: &Device, argv: &[&str], stdin: &str) -> String {
    let r = dev.run_with(argv, stdin, &[]);
    assert_eq!((r.code, r.err.as_str()), (1, ""), "{}", r.out);
    r.out.split('\t').next().unwrap_or_default().to_string()
}

/// 正好三行（a, b, c = lines(...)）
#[track_caller]
fn three(got: Vec<String>) -> (String, String, String) {
    let [a, b, c]: [String; 3] = got.try_into().unwrap_or_else(|got| panic!("该正好三行：{got:?}"));
    (a, b, c)
}

/// 一条该成的命令给的 JSON（json.loads(dev.ok(...))）
#[track_caller]
fn jl(dev: &Device, argv: &[&str]) -> Value {
    let out = dev.ok(argv);
    json::parse(&out).unwrap_or_else(|e| panic!("{} 给的不是 JSON：{e}：{out}", argv.join(" ")))
}

/// 这台设备上某个 Team 的目录
fn tdir(dev: &Device, tid: &str) -> PathBuf {
    dev.cca.join("teams").join(tid)
}

/// 这台设备手里的配置：(序号, 内容)
fn doc(dev: &Device, tid: &str) -> (i64, Value) {
    let j = fsx::read_json(tdir(dev, tid).join("doc.json"));
    (j["seq"].i64(), j["doc"].clone())
}

/// 给这个 Team 一份现成的配置（config()）：整份写进管理员这台的 admin.json（3.6.0 以前是 cca team import 导入维护者映射仓库里的
/// policy.json、nodes.json），再经 cca team map 编别名（没写的编成「Team 的名字-N」）、发布（还没接上服务器的只改本机的）。返回存上了几台的配置
fn load(admin: &Device, tid: &str) -> Vec<String> {
    let path = tdir(admin, tid).join("admin.json");
    let mut a = fsx::read_json(&path);
    sub(&mut a, &[]).insert("config", config());
    put(&path, &a);
    lines(admin, &["team-map", tid, "claude", "alice@example.com", "US-A"])
}

/// 发一个邀请：(链接, 这台设备在 Team 里的编号)
#[track_caller]
fn invite(admin: &Device, tid: &str, label: &str, more: &[&str]) -> (String, String) {
    let mut argv = vec!["team-invite", tid, label];
    argv.extend_from_slice(more);
    let (link, dev, problem) = three(lines(admin, &argv));
    assert_eq!(problem, "");
    (link, dev)
}

/// 管理员看到的名单（team-devices --json）
fn devices(admin: &Device, tid: &str) -> Value {
    jl(admin, &["team-devices", tid, "--json"])
}

/// 名单里各台设备的状态（{r['label']: r['state'] for r in devices}）
fn states(admin: &Device, tid: &str) -> Value {
    mapped(&keyed(&devices(admin, tid)["devices"], "label"), |r| r["state"].clone())
}

/// team-sync 的头一行拆开的各栏（编号、名字、状态、序号、原因）
fn sync_fields(dev: &Device) -> Vec<String> {
    lines(dev, &["team-sync"])[0].split('\t').map(str::to_string).collect()
}

/// 照 Team 的配置改账号表（team-apply；tool 不是 None 时换一个工具的账号表）：该成、stdout 上没有话，返回它在 stderr 上说的
#[track_caller]
fn team_apply_as(dev: &Device, tool: Option<&str>) -> String {
    let env: Vec<(&str, &str)> = tool.map(|tool| ("CCA_TOOL", tool)).into_iter().collect();
    let r = dev.run_with(&["team-apply"], "", &env);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    r.err
}

/// team-apply，Claude 的账号表
#[track_caller]
fn team_apply(dev: &Device) -> String {
    team_apply_as(dev, None)
}

/// 这台设备的路由器上有 US-A…US-D 这几个节点（config.json 的 nodes）
fn with_nodes(dev: &Device) {
    dev.config_edit(|c| {
        c.insert("nodes", list!["US-A", "US-B", "US-C", "US-D"]);
    });
}

/// 这台设备的 Claude 账号表（accounts.json）
fn table(dev: &Device) -> Value {
    fsx::read_json(dev.cca.join("accounts.json"))
}

// ---- 用例 ----

/// 建 Team：只在本机（还没有服务器）；文件都只给自己读写；接不上的各种原因
fn admin_local(t: &mut Ctx) {
    let fx = fx(t);
    let admin = fx.device("admin");
    assert_eq!(code(&admin, &["team-new", ""]), "name");
    assert_eq!(lines(&admin, &["team-list"]), NONE);
    let (tid, root, n) = three(lines(&admin, &["team-new", "测试组", "--device", "studio"]));
    let (tid, root) = (tid.as_str(), root.as_str());
    assert_eq!(n, "0", "没写 --accounts：一个账号都不放");
    assert_eq!((tid, tid.len()), (spec::team_id(root).as_str(), 32));
    let td = tdir(&admin, tid);
    assert_eq!(
        names_in(&td),
        ["admin.json", "id_ed25519", "id_ed25519.pub", "root_ed25519", "root_ed25519.pub", "secret", "team.json"]
    );
    let private = [td.clone(), td.join("admin.json"), td.join("root_ed25519"), td.join("secret"), td.join("team.json")];
    assert_eq!(private.map(fsx::mode), [0o700, 0o600, 0o600, 0o600, 0o600]);
    let index = admin.cca.join("teams").join("index.json");
    assert_eq!(fsx::read_json(&index), obj! { "v" => 1, "order" => list![tid] });
    let unfinished: Vec<String> = names_in(&admin.cca.join("teams")).into_iter().filter(|n| n.starts_with(".new.")).collect();
    assert_eq!(unfinished, NONE);
    let rows = jl(&admin, &["team-list", "--json"]);
    let teams: Vec<Value> =
        rows["teams"].list().iter().map(|r| pick(r, &["id", "name", "role", "state", "host", "device"])).collect();
    assert_eq!(
        list![&rows["kind"], teams],
        list!["team_list", list![list![tid, "测试组", "admin", "local", Value::Null, "studio"]]]
    );
    assert_eq!(lines(&admin, &["team-list"]), [format!("{tid}\t测试组\tadmin\t-\tlocal\t-\t-\t-\t-")], "没有的栏写 -");
    // 建到一半被打断（目录有了、还没记进 index.json）的也认得出来
    fs::remove_file(&index).expect("删 index.json");
    let ids: Vec<Value> = jl(&admin, &["team-list", "--json"])["teams"].list().iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, [Value::from(tid)]);
    // team.json 坏了、管理员的密钥还在：拿这个 Team 的邀请来加入不能当成没有（不然根密钥就删掉了）
    let good = fsx::read_text(td.join("team.json"));
    fsx::write(td.join("team.json"), "{");
    let link = spec::invite_encode("https://127.0.0.1:9", &[], root, &crypto::token_hex(16), "测试组", None, None);
    assert_eq!(code_in(&admin, &["team-join"], &link), "joined");
    assert!(td.join("root_ed25519").exists() && td.join("admin.json").exists());
    fsx::write(td.join("team.json"), &good);
    // 每台设备那串随机数不存：照主密钥算。管理员自己的那台是第 0 台
    let a = fsx::read_json(td.join("admin.json"));
    let k0 = fsx::read_text(td.join("secret")).trim().to_string();
    let dev0 = spec::invite_parts(&k0).expect("secret 是那串随机数的样子").1;
    assert_eq!(
        (key_list(&a["devices"]), a["devices"][dev0.as_str()]["state"].str(), fsx::read_json(td.join("team.json"))["dev"].str()),
        (vec![dev0.as_str()], "local", dev0.as_str())
    );
    assert_not_in(&k0, &fsx::read_text(td.join("admin.json")));
    let master = crypto::unhex(a["master"].str()).expect("主密钥是十六进制");
    assert_eq!(k0, crypto::hex(&crypto::hmac_sha256(&master, b"cca-team-k-v1\x000")[..16]));
    // 还没接上服务器：发不了邀请；改配置只改本机的
    assert_eq!(code(&admin, &["team-invite", tid, "bob"]), "not_attached");
    assert_eq!(load(&admin, tid), ["0"]);
    assert_eq!(lines(&admin, &["team-map", tid, "claude", "Dave@Example.com", "US-E"]), ["0"]);
    assert_eq!(
        fsx::read_json(td.join("admin.json"))["config"]["accounts"]["claude"],
        obj! { "alice@example.com" => "US-A", "bob@example.com" => "US-B", "dave@example.com" => "US-E" }
    );
    // 接不上：地址、指纹写得不对；不是这个 Team 的服务器；不是 Team 的服务器
    let no_pin = format!("sha256//{}=", "A".repeat(43));
    assert_eq!(code(&admin, &["team-attach", tid, "http://127.0.0.1:1"]), "url");
    assert_eq!(code(&admin, &["team-attach", tid, "https://127.0.0.1:1", "sha256//x"]), "pin");
    assert_eq!(code(&admin, &["team-attach", tid, "https://127.0.0.1:9", &no_pin]), "unreachable");
    let other = pki::keygen(fx.d.join("other-root"));
    let (_srv, url, pin) = fx.server(&other, "other");
    assert_eq!(code(&admin, &["team-attach", tid, &url, &pin]), "denied");
    assert_eq!(code(&admin, &["team-attach", tid, &url, &no_pin]), "pin");
    let (_not_team, not_url, not_pin) = fx.not_team();
    assert_eq!(code(&admin, &["team-attach", tid, &not_url, &not_pin]), "old");
    assert_eq!(fsx::read_json(td.join("team.json"))["url"].str(), "", "没接上：本机照旧");
}

fn lifecycle(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, root, srv, url, pin) = fx.new_team();
    let (tid, root, url, pin) = (tid.as_str(), root.as_str(), url.as_str(), pin.as_str());
    let td = tdir(&admin, tid);
    let host = format!("127.0.0.1:{}", srv.port);
    assert_eq!(doc(&admin, tid).0, 1, "接上以后管理员自己这台也取到了配置");
    assert_eq!(load(&admin, tid), ["1"]);
    assert_eq!(lines(&admin, &["team-sync"]), [format!("{tid}\t测试组\tok\t2\t")]);
    let listing = devices(&admin, tid);
    assert_eq!(
        list![
            &listing["name"],
            sorted_items(&listing["accounts"]["claude"]),
            &listing["aliases"],
            &listing["tool_aliases"],
            sorted_items(&listing["nodes"])
        ],
        list![
            "测试组",
            list!["alice@example.com", "bob@example.com"],
            obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "carol@example.com" => "测试组-1" },
            obj! {
                "claude" => obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1" },
                "codex" => obj! { "carol@example.com" => "测试组-1" },
            },
            list!["US-A", "US-B", "US-C", "US-D", "US-E"]
        ],
        "管理页要的：Team 的名字、配置里的账号 → 节点、别名（按工具的和按邮箱的）、带着定义的节点"
    );
    let (_, d) = doc(&admin, tid);
    assert_eq!(
        list![&d["accounts"], &d["tool_aliases"], &d["aliases"], &d["install_node"], &d["name"], &d["server"]],
        list![
            obj! {
                "claude" => obj! { "alice@example.com" => "US-A", "bob@example.com" => "US-B" },
                "codex" => obj! { "carol@example.com" => "US-A" },
            },
            obj! {
                "claude" => obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1" },
                "codex" => obj! { "carol@example.com" => "测试组-1" },
            },
            obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "carol@example.com" => "测试组-1" },
            "US-D",
            "测试组",
            obj! { "u" => url, "p" => list![pin] }
        ],
        "配置里没写别名的编成「Team 的名字-N」：各工具各编各的（Claude 的 bob、Codex 的 carol 都是 测试组-1）"
    );
    assert_eq!(proxy_names(&d), ["US-A", "US-B", "US-C", "US-D"], "用得着的节点（连同 dialer-proxy 连着的）");
    // 邀请：链接只在 stdout 里出现一次；那串随机数、令牌不落盘、不进命令行
    assert_eq!(code(&admin, &["team-invite", tid, "bob", "--accounts", "nope"]), "usage");
    assert_eq!(code(&admin, &["team-invite", tid, "bob", "--ttl", "5"]), "usage");
    let (link, dev_b) = invite(&admin, tid, "bob-mini", &["--accounts", "claude:Alice@example.com"]);
    let (link, dev_b) = (link.as_str(), dev_b.as_str());
    let inv = spec::invite_decode(link).expect("邀请链接读得出来");
    let k = inv["k"].str();
    let (token, dev, keys) = spec::invite_parts(k).expect("链接里的 k 是那串随机数的样子");
    assert_eq!(
        list![
            &inv["u"],
            &inv["p"],
            format!("ssh-ed25519 {}", inv["r"].str()),
            &inv["n"],
            &inv["d"],
            &dev,
            inv.obj().contains_key("s")
        ],
        list![url, list![pin], root, "测试组", "bob-mini", dev_b, false],
        "邀请里带着管理员给这台设备起的名字"
    );
    assert!((inv["e"].f64() - (fleet::now() + 7.0 * 86400.0)).abs() < 120.0, "默认 7 天过期");
    let everything = all_text(&admin.home);
    assert!(
        !everything.contains(k) && !everything.contains(&token),
        "那串随机数、令牌不在管理员这台的任何文件里（请求、curl 的命令行）"
    );
    let state = fsx::read_text(fx.d.join("srv").join("state.json"));
    assert!(!state.contains(k) && !state.contains(&token) && state.contains(&crypto::sha256_hex(token.as_bytes())));
    // 加入
    let bob = fx.device("bob");
    assert_eq!(code_in(&bob, &["team-join"], "nope"), "link");
    assert_eq!(code_in(&bob, &["team-join"], &link[..link.len() - 3]), "link");
    assert_eq!(code_in(&bob, &["team-join", "--name", &"x".repeat(65)], link), "name");
    let teams = bob.cca.join("teams");
    assert!(!(teams.exists() && !names_in(&teams).is_empty()), "没成：什么都不留");
    assert!(!bob.config().obj().contains_key("device_id"));
    let got = lines_in(&bob, &["team-join"], link);
    assert_eq!(got, ["测试组", host.as_str(), "bob-mini", "2", tid, "ok", "1"], "没写 --name：用邀请里的名字，不用主机名");
    let device_id = Regex::new(r"^[0-9a-f]{16}$").expect("正则");
    assert!(
        device_id.is_match(bob.config().get("device_id").map_or("", Value::str)),
        "加入了才生成这台机器的标识（没有路由器的机器靠它：折合金额的 dv）"
    );
    assert!(device_id.is_match(admin.config().get("device_id").map_or("", Value::str)), "新建 Team 的那台也有");
    let argv = fsx::read_text(bob.home.join("curl.argv"));
    assert!(!argv.contains(k) && !argv.contains(&token) && !argv.contains(link), "邀请不进 curl 的命令行");
    let tb = tdir(&bob, tid);
    assert_eq!(names_in(&tb), ["doc.json", "id_ed25519", "id_ed25519.pub", "lock", "secret", "sync.json", "team.json"]);
    assert_eq!(
        [tb.clone(), tb.join("doc.json"), tb.join("secret"), tb.join("id_ed25519")].map(fsx::mode),
        [0o700, 0o600, 0o600, 0o600]
    );
    let (seq, d) = doc(&bob, tid);
    assert_eq!(
        list![seq, &d["accounts"], &d["aliases"], proxy_names(&d)],
        list![
            1,
            obj! { "claude" => obj! { "alice@example.com" => "US-A" } },
            obj! { "alice@example.com" => "alice" },
            list!["US-A", "US-D"]
        ],
        "只有给了它的账号和这些账号要的节点（安装客户端用的节点也带着）"
    );
    assert_eq!(d["salt"].str().len(), 32);
    assert_eq!(d["salt"], doc(&admin, tid).1["salt"], "盐是整个 Team 一份");
    let rows = jl(&bob, &["team-list", "--json"]);
    let rows: Vec<Value> =
        rows["teams"].list().iter().map(|r| pick(r, &["id", "role", "state", "seq", "accounts", "device", "error"])).collect();
    assert_eq!(rows, [list![tid, "member", "ok", 1, 1, "bob-mini", Value::Null]]);
    assert_eq!(code_in(&bob, &["team-join"], link), "joined");
    let eve = fx.device("eve");
    assert_eq!(code_in(&eve, &["team-join"], link), "used");
    let eve_teams = eve.cca.join("teams");
    let joined: Vec<String> = if eve_teams.exists() {
        names_in(&eve_teams).into_iter().filter(|n| eve_teams.join(n).join("team.json").exists()).collect()
    } else {
        Vec::new()
    };
    assert_eq!(joined, NONE);
    // 管理员的那几条成员用不了
    for cmd in [
        vec!["team-invite", tid, "x"],
        vec!["team-devices", tid],
        vec!["team-map", tid, "claude", "x@example.com", "US-A"],
        vec!["team-publish", tid],
    ] {
        assert_eq!(code(&bob, &cmd), "not_admin", "{cmd:?}");
    }
    // 照文档独立实现的那一份也加入得了、解得开、验得过（辅助程序封的、签的）
    let (link2, dev_c) = invite(&admin, tid, "spec", &["--accounts", "codex:carol@example.com,claude:bob@example.com"]);
    let dev_c = dev_c.as_str();
    let k2 = spec::invite_decode(&link2).expect("邀请链接读得出来")["k"].str().to_string();
    let (token2, _dev2, keys2) = spec::invite_parts(&k2).expect("链接里的 k 是那串随机数的样子");
    let spec_key = fx.d.join("spec-key");
    let public = pki::keygen(&spec_key);
    let (st, j) = spec::post(
        &spec_key,
        srv.port,
        "team_join",
        &obj! { "pub" => &public, "name" => "spec", "token" => token2, "proof" => spec::team_pub_proof(&k2, &public) },
    );
    assert_eq!((st, j["dev"].str()), (200, dev_c));
    let (_, j) = spec::post(&spec_key, srv.port, "team_get", &obj! {});
    let env = &j["doc"];
    assert!(spec::team_doc_verify(root, tid, dev_c, env["seq"].i64(), &env["ct"], env["sig"].str()), "根密钥签的");
    let spec_doc = spec::team_doc_open(&keys2, tid, dev_c, env["seq"].i64(), &env["ct"]).expect("自己那份配置的密钥解得开");
    assert_eq!(
        list![&spec_doc["accounts"], proxy_names(&spec_doc)],
        list![
            obj! { "claude" => obj! { "bob@example.com" => "US-B" }, "codex" => obj! { "carol@example.com" => "US-A" } },
            list!["US-A", "US-B", "US-C", "US-D"]
        ]
    );
    assert!(spec::team_doc_open(&keys, tid, dev_c, env["seq"].i64(), &env["ct"]).is_none(), "别的设备的密钥解不开");
    // 设备的名单
    let rows = keyed(&devices(&admin, tid)["devices"], "label");
    assert_eq!(
        mapped(&rows, |r| pick(r, &["state", "name", "me", "seq"])),
        obj! {
            "studio" => list!["joined", "studio", true, 2],
            "bob-mini" => list!["joined", "bob-mini", false, 1],
            "spec" => list!["joined", "spec", false, 1],
        }
    );
    assert_eq!(lines(&admin, &["team-list"]), [format!("{tid}\t测试组\tadmin\t{host}\tok\t2\t3\t-\t-")]);
    assert_eq!(
        list![&rows["bob-mini"]["have"], &rows["bob-mini"]["grants"], &rows["studio"]["grants"]],
        list![Value::Null, obj! { "claude" => list!["alice@example.com"] }, "all"]
    );
    let problems: Vec<Value> = rows.obj().values().map(|r| r["problem"].clone()).collect();
    assert_eq!(problems, [Value::Null, Value::Null, Value::Null]);
    // 公钥要过认证码才记下（设备加入时拿邀请里的 k 算的：服务器不知道 k，换不了）：辅助程序加入的、独立实现带了认证码的都记下了
    let saved = fsx::read_json(td.join("admin.json"))["devices"].clone();
    let bob_pub = spec::pub_line(&fsx::read_text(tb.join("id_ed25519.pub")));
    assert_eq!((saved[dev_b]["pub"].str(), saved[dev_c]["pub"].str()), (bob_pub.as_str(), public.as_str()));
    // 没带认证码的（照旧文档自己实现的）：照样发布配置给它，公钥不记，名单里说一声
    let (link_n, dev_n) = invite(&admin, tid, "no-proof", &["--accounts", "none"]);
    let noproof_key = fx.d.join("noproof-key");
    let pub_n = pki::keygen(&noproof_key);
    let k_n = spec::invite_decode(&link_n).expect("邀请链接读得出来")["k"].str().to_string();
    let token_n = spec::invite_parts(&k_n).expect("链接里的 k 是那串随机数的样子").0;
    let joined =
        spec::post(&noproof_key, srv.port, "team_join", &obj! { "pub" => pub_n, "name" => "no-proof", "token" => token_n });
    assert_eq!(joined.0, 200);
    let listing = devices(&admin, tid);
    let row = find(&listing["devices"], "label", "no-proof");
    assert_eq!(row["state"].str(), "joined");
    assert_in("认证码", row["problem"].str());
    assert!(!fsx::read_json(td.join("admin.json"))["devices"][dev_n.as_str()].obj().contains_key("pub"));
    let out = admin.ok(&["team-devices", tid]);
    let listing: Vec<&str> = out.split('\n').collect();
    assert_eq!(
        (listing[0], &listing[4].split('\t').collect::<Vec<_>>()[2..]),
        ("0", &["joined", "0 个账号", "1", "-", row["problem"].str(), "member"][..]),
        "第一行是别名锁定着没有；最后一栏是角色（3.6.15 起）"
    );
    assert_eq!(lines(&admin, &["team-revoke", "apply", tid, "no-proof"]), ["no-proof"]);
    // 改授权、改映射：发布以后设备同步到新的一版
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "+claude:bob@example.com"]), ["2 个账号"]);
    assert_eq!(lines(&bob, &["team-sync", &tid[..6]]), [format!("{tid}\t测试组\tok\t2\t")]);
    let (_, d) = doc(&bob, tid);
    assert_eq!(
        list![&d["accounts"], proxy_names(&d)],
        list![
            obj! { "claude" => obj! { "alice@example.com" => "US-A", "bob@example.com" => "US-B" } },
            list!["US-A", "US-B", "US-C", "US-D"]
        ]
    );
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "-claude:alice@example.com"]), ["1 个账号"]);
    assert_eq!(
        lines(&admin, &["team-grant", tid, "bob-mini", "all", "-codex:carol@example.com"]),
        ["2 个账号"],
        "从全部里去掉一个"
    );
    assert_eq!(code(&admin, &["team-grant", tid, "bob-mini", "claude:x@example.com"]), "usage");
    assert_eq!(code(&admin, &["team-grant", tid, "nobody", "all"]), "no_device");
    assert_eq!(
        lines(&admin, &["team-map", tid, "claude", "alice@example.com", "US-E"]),
        ["2"],
        "管理员自己的和 bob 的变了，spec 的没变"
    );
    assert_eq!(lines(&admin, &["team-map", tid, "codex", "carol@example.com", "-"]), ["2"], "从 Team 里去掉一个账号");
    assert_eq!(lines(&bob, &["team-sync"]), [format!("{tid}\t测试组\tok\t5\t")]);
    assert_eq!(
        doc(&bob, tid).1["accounts"],
        obj! { "claude" => obj! { "alice@example.com" => "US-E", "bob@example.com" => "US-B" } }
    );
    let rows = keyed(&devices(&admin, tid)["devices"], "label");
    assert_eq!(list![&rows["bob-mini"]["seq"], &rows["bob-mini"]["have"]], list![5, 2], "它上次取的时候说手里是第 2 版");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["0", ""], "没变的不重新发布");
    // 移除：设备同步配置时知道被移除了；拿新的邀请再加入
    assert_eq!(code(&admin, &["team-revoke", "plan", tid, "studio"]), "self");
    assert_eq!(lines(&admin, &["team-revoke", "plan", tid, "bob-mini"]), [format!("{tid}\t{dev_b}\tbob-mini\tjoined")]);
    assert_eq!(lines(&admin, &["team-revoke", "apply", tid, "bob-mini"]), ["bob-mini"]);
    assert!(!fsx::read_json(td.join("admin.json"))["devices"].obj().contains_key(dev_b), "移除了的这边也不再记着");
    let line = sync_fields(&bob);
    assert_eq!(line[..4], [tid, "测试组", "removed", "5"]);
    assert_in("移除", &line[4]);
    assert_eq!(jl(&bob, &["team-list", "--json"])["teams"][0]["state"].str(), "removed");
    let (link3, _dev_b3) = invite(&admin, tid, "bob-again", &[]);
    assert_eq!(
        lines_in(&bob, &["team-join", "--name", "bob-mini"], &link3)[5..],
        ["ok", "2"],
        "被移除了的拿新的邀请直接加入（以前记着的不要了）"
    );
    assert_eq!(doc(&bob, tid).0, 1);
    let rows = states(&admin, tid);
    assert_eq!(
        (rows["bob-again"].str(), rows.obj().contains_key("bob-mini"), rows.obj().contains_key("no-proof")),
        ("joined", false, false),
        "管理员和服务器对过一遍：知道它加入了；移除了的不列"
    );
    // 退出：服务器上没有它了；管理员那边不再记它，不登记回去
    assert_eq!(lines(&bob, &["team-leave", "plan"]), [format!("{tid}\t测试组\tmember\t{host}")]);
    assert_eq!(code(&bob, &["team-leave", "apply", &tid[..8]]), "no_team", "真退出要写完整的编号");
    assert_eq!(lines(&bob, &["team-leave", "apply", tid]), ["测试组", host.as_str(), ""]);
    assert!(!tb.exists());
    assert_eq!(fsx::read_json(bob.cca.join("teams").join("index.json"))["order"], list![]);
    assert_eq!(code(&bob, &["team-sync", tid]), "no_team");
    let both = obj! { "studio" => "joined", "spec" => "joined" };
    assert_eq!(states(&admin, tid), both, "自己退出了的、移除了的都不列");
    assert_eq!(fsx::read_json(td.join("admin.json"))["devices"].obj().len(), 2, "admin.json 里也只剩这两台");
    // 3.6.5 及以前留下的记录（移除了的 revoked、自己退出了的 left）：对一遍时删掉
    let mut a = fsx::read_json(td.join("admin.json"));
    let mut old: Vec<String> = Vec::new();
    for (i, state) in ["revoked", "left"].into_iter().enumerate() {
        let dev = format!("{:016x}", 0xdead0000u64 + i as u64);
        let record = obj! { "n" => 90 + i, "label" => format!("gone-{i}"), "state" => state, "grants" => "all", "seq" => 1 };
        sub(&mut a, &["devices"]).insert(dev.as_str(), record);
        old.push(dev);
    }
    put(td.join("admin.json"), &a);
    assert_eq!(states(&admin, tid), both);
    let kept = fsx::read_json(td.join("admin.json"));
    assert!(!old.iter().any(|dev| kept["devices"].obj().contains_key(dev)));
    // 以前记成 left、又出现在服务器上的（3.6.5 及以前：对过以后它又登记回来了）：恢复成 joined
    let mut a = fsx::read_json(td.join("admin.json"));
    let spec_dev = {
        let found = a["devices"].obj().iter().find(|(_, d)| d["label"] == Value::from("spec"));
        found.expect("admin.json 里记着 spec 那一台").0.to_string()
    };
    sub(&mut a, &["devices", &spec_dev]).insert("state", "left");
    put(td.join("admin.json"), &a);
    assert_eq!(states(&admin, tid), both, "服务器上还有它：恢复成 joined");
    assert_eq!(
        lines(&admin, &["team-leave", "plan", "测试组"]),
        [format!("{tid}\t测试组\tadmin\t{host}\troot@127.0.0.1\t22\t/opt/cca-team-{}\t0", &tid[..8])],
        "管理员：后面是删除时去哪卸载它的同步服务器（手动接上的、没记下部署到哪：照部署的默认猜，最后一项是 0）"
    );
    assert!(td.exists());
    assert_eq!(lines(&admin, &["team-leave", "apply", tid])[..2], ["测试组", host.as_str()]);
    assert!(!td.exists());
}

/// 服务器要不要更新（3.7.1 起）：同步成了顺便问服务器的级别（GET /v1/version，不用签名），记进 sync.json，team-list 给出来。
/// 没有这个地址的旧服务器（~/reply/version 扮演）照 GET /v1/health：中转那一项里有 guests 的是 3.7.0，算 1；没有的算 0；比这一版要的
/// （3：3.7.10 起）低的要更新，原因在 team-list 的最后一栏；问不到时照记着的；同步没成时不问
fn server_level(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, _tid, _root, _srv, _url, _pin) = fx.new_team();
    let reply = admin.home.join("reply");
    let row = || -> Value {
        let r = &jl(&admin, &["team-list", "--json"])["teams"][0];
        list![&r["server_level"], &r["server_update"]]
    };
    let last = || lines(&admin, &["team-list"])[0].rsplit('\t').next().unwrap_or_default().to_string();
    let gets = || fsx::read_text(admin.home.join("curl.argv")).lines().filter(|l| l.ends_with("/v1/version")).count();
    let why = "3.7.10 以前的服务器一台设备最多只能开 8 个远程会话（更新以后能开 100 个）";
    let old = obj! { "level" => 0, "need" => 3, "why" => why };
    // 这一版的服务器：3，不用更新
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![3, Value::Null]);
    assert_eq!(last(), "-");
    assert_eq!(admin.sent(Some("version")).len(), 0, "不用签名的 GET 不算在签过名的请求里");
    // 3.7.0 以前的：没有 /v1/version，health 里的中转没有 guests
    fsx::write(reply.join("version"), "404\n{\"error\":\"not_found\"}");
    fsx::write(reply.join("health"), "200\n{\"v\":1,\"ok\":true,\"mode\":\"team\",\"relay\":{\"port\":18447}}");
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![0, old.clone()]);
    assert_eq!(last(), old["why"].str());
    // 问不到（服务器回 503）：照记着的
    fsx::write(reply.join("version"), "503\n{}");
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![0, old.clone()], "没问到：照记着的");
    // 同步没成：不问
    fsx::write(reply.join("team_get"), "500\n{}");
    let n = gets();
    assert_eq!(sync_fields(&admin)[2], "error");
    assert_eq!(gets(), n, "同步没成时不问服务器的级别");
    fs::remove_file(reply.join("team_get")).expect("删掉 reply/team_get");
    // 3.7.0：health 的中转里有 guests，算 1
    fsx::write(reply.join("version"), "404\n{\"error\":\"not_found\"}");
    fsx::write(reply.join("health"), "200\n{\"v\":1,\"ok\":true,\"mode\":\"team\",\"relay\":{\"port\":18447,\"guests\":true}}");
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![1, obj! { "level" => 1, "need" => 3, "why" => why }], "3.7.0 也要更新");
    // 更新了：问到真的服务器的，提示没了
    fsx::write(reply.join("health"), "200\n{\"v\":1,\"ok\":true,\"mode\":\"team\",\"relay\":{\"port\":18447}}");
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![0, old]);
    fs::remove_dir_all(&reply).expect("删掉 reply");
    assert_eq!(sync_fields(&admin)[2], "ok");
    assert_eq!(row(), list![3, Value::Null], "更新以后同步一次，提示就没了");
}

/// 管理员给设备改名字：这边记着的、服务器上的一起改（同一把公钥再登记：加入的时间、认证码、配对认的指纹都不变），设备下次同步配置时
/// 跟着改；服务器上别的设备叫这个名字时服务器加 -2；还没加入的只改这边记着的；自己退出了的（对一遍时就不记了）改不了；
/// 还在的别的设备叫这个名字不行
fn rename(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    let td = tdir(&admin, tid);
    let state_file = fx.d.join("srv").join("state.json");
    // 服务器的名单里某台设备的那几条记录：[(公钥的指纹, 记录)]
    let on_server = |dev: &str| -> Vec<(String, Value)> {
        let machines = fsx::read_json(&state_file)["machines"].clone();
        let of_dev = machines.obj().iter().filter(|(_, m)| m["dev"] == Value::from(dev));
        of_dev.map(|(f, m)| (f.to_string(), m.clone())).collect()
    };
    let fps = |records: &[(String, Value)]| -> Vec<String> { records.iter().map(|(f, _)| f.clone()).collect() };
    let (link, dev_b) = invite(&admin, tid, "bob-mini", &[]);
    let dev_b = dev_b.as_str();
    let bob = fx.device("bob");
    lines_in(&bob, &["team-join", "--name", "bob-host"], &link);
    let before = on_server(dev_b);
    let names: Vec<&str> = before.iter().map(|(_, m)| m["name"].str()).collect();
    assert_eq!(names, ["bob-host"], "加入时写了 --name：先照它报");
    assert!(before[0].1.get("proof").is_some_and(truthy));
    let device = || jl(&bob, &["team-list", "--json"])["teams"][0]["device"].str().to_string();
    // 和服务器对一遍时名字以管理员起的为准：服务器上照它再登记（还是那把公钥，加入的时间、认证码照旧），设备下次同步配置时跟着改
    let rows = keyed(&devices(&admin, tid)["devices"], "dev");
    assert_eq!(
        list![&rows[dev_b]["label"], &rows[dev_b]["name"], &rows[dev_b]["problem"]],
        list!["bob-mini", "bob-mini", Value::Null]
    );
    let aligned = on_server(dev_b);
    assert_eq!(fps(&aligned), fps(&before), "同一把公钥：远程会话的配对按它认，照旧");
    let (f, m0) = &before[0];
    let m1 = &aligned[0].1;
    assert_eq!(list![&m1["name"], &m1["joined"], m1.get("proof")], list!["bob-mini", &m0["joined"], &m0["proof"]], "{f}");
    assert_eq!(device(), "bob-host");
    assert_eq!(lines(&bob, &["team-sync"]), [format!("{tid}\t测试组\tok\t1\t")]);
    assert_eq!(device(), "bob-mini");
    // 改：这边记着的、服务器上的都是新名字；服务器上还是那把公钥，加入的时间、认证码照旧
    assert_eq!(lines(&admin, &["team-rename", tid, "bob-mini", "小王的 MacBook"]), ["bob-mini", "小王的 MacBook", ""]);
    let d = fsx::read_json(td.join("admin.json"))["devices"][dev_b].clone();
    assert_eq!(list![&d["label"], &d["name"], &d["state"]], list!["小王的 MacBook", "小王的 MacBook", "joined"]);
    let after = on_server(dev_b);
    assert_eq!(fps(&after), fps(&before));
    let m2 = &after[0].1;
    assert_eq!(list![&m2["name"], &m2["joined"], m2.get("proof")], list!["小王的 MacBook", &m0["joined"], &m0["proof"]], "{f}");
    // 设备下次同步配置时跟着改（配置本身没变：序号照旧）
    assert_eq!(device(), "bob-mini");
    assert_eq!(lines(&bob, &["team-sync"]), [format!("{tid}\t测试组\tok\t1\t")]);
    assert_eq!(device(), "小王的 MacBook");
    let listing = devices(&admin, tid);
    assert!(find(&listing["devices"], "dev", dev_b)["problem"].is_null(), "改过名字以后对一遍照样核对得了它的公钥");
    // 不行的：名字不对、找不到设备、还在的别的设备叫这个名字（管理员起的、服务器上的都算）
    assert_eq!(code(&admin, &["team-rename", tid, dev_b, " "]), "name");
    assert_eq!(code(&admin, &["team-rename", tid, dev_b, &"x".repeat(65)]), "name");
    assert_eq!(code(&admin, &["team-rename", tid, "nobody", "x"]), "no_device");
    assert_eq!(code(&admin, &["team-rename", tid, dev_b, "studio"]), "name");
    assert_eq!(code(&bob, &["team-rename", tid, dev_b, "x"]), "not_admin");
    assert_eq!(code(&admin, &["team-invite", tid, "小王的 MacBook"]), "name", "发邀请也一样：还在的别的设备叫这个名字不行");
    assert_eq!(code(&admin, &["team-invite", tid, "studio"]), "name");
    // 服务器上有一台管理员这边没记着的叫这个名字：服务器在后面加 -2，这边记着的是管理员起的
    let pub_x = pki::keygen(fx.d.join("x-key"));
    let enroll = obj! { "devices" => obj! { "0d".repeat(8) => obj! { "pub" => pub_x, "name" => "mini" } } };
    assert_eq!(spec::post(td.join("root_ed25519"), srv.port, "team_enroll", &enroll).0, 200);
    assert_eq!(lines(&admin, &["team-rename", tid, dev_b, "mini"]), ["小王的 MacBook", "mini-2", ""]);
    let d = fsx::read_json(td.join("admin.json"))["devices"][dev_b].clone();
    assert_eq!(list![&d["label"], &d["name"]], list!["mini-2", "mini-2"], "一台设备只有一个名字：这边也记成服务器的");
    let enrolls = || fsx::read_text(admin.home.join("curl.argv")).matches("team_enroll").count();
    let n = enrolls();
    admin.ok(&["team-devices", tid]);
    assert_eq!(enrolls(), n, "名字一样了：对一遍时不再登记");
    // 管理员自己这台：team.json 里的名字一起改
    assert_eq!(lines(&admin, &["team-rename", tid, "studio", "studio-2026"]), ["studio", "studio-2026", ""]);
    assert_eq!(fsx::read_json(td.join("team.json"))["device"].str(), "studio-2026");
    assert_eq!(lines(&admin, &["team-sync"]), [format!("{tid}\t测试组\tok\t1\t")]);
    assert_eq!(fsx::read_json(td.join("team.json"))["device"].str(), "studio-2026", "同步配置时服务器说的也是它");
    // 还没加入的：只改这边记着的；它照邀请里的（改名之前的）名字加入，对一遍时服务器上换成改好的
    let (link_c, dev_c) = invite(&admin, tid, "carol", &[]);
    let dev_c = dev_c.as_str();
    let got = lines(&admin, &["team-rename", tid, "carol", "carol-mac"]);
    assert_eq!(got[..2], ["carol", "carol-mac"]);
    assert_in("还没加入", &got[2]);
    assert_eq!(fsx::read_json(td.join("admin.json"))["devices"][dev_c]["label"].str(), "carol-mac");
    let carol = fx.device("carol");
    assert_eq!(lines_in(&carol, &["team-join"], &link_c)[2], "carol");
    let rows = keyed(&devices(&admin, tid)["devices"], "dev");
    assert_eq!(list![&rows[dev_c]["label"], &rows[dev_c]["name"]], list!["carol-mac", "carol-mac"]);
    carol.ok(&["team-sync"]);
    assert_eq!(jl(&carol, &["team-list", "--json"])["teams"][0]["device"].str(), "carol-mac");
    // 自己退出了的：不登记回去，这边也不再记着它
    lines(&bob, &["team-leave", "apply", tid]);
    assert_eq!(code(&admin, &["team-rename", tid, dev_b, "bob-old"]), "no_device");
    assert!(on_server(dev_b).is_empty(), "退出了的没被改名字登记回去");
    assert!(!fsx::read_json(td.join("admin.json"))["devices"].obj().contains_key(dev_b));
    // 已经不在的设备以前的名字可以再用
    assert_eq!(lines(&admin, &["team-rename", tid, "carol-mac", "mini-2"])[..2], ["carol-mac", "mini-2"]);
}

/// Team 里账号的别名：没写的编成「Team 的名字-1」「Team 的名字-2」……（加账号、发布时；同一个 Team 里不重复，用过的号不再用，--mine 带进这台设备上
/// 起过的）；cca team alias 改。成员设备上只有邮箱的补上、照 Team 给的用着的跟着改、自己改过的不动；管理员这台
/// 设备上的不动；两个 Team 给的别名撞了时后来的那个叫「Team 名-别名」
fn aliases(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    let admin_json = tdir(&admin, tid).join("admin.json");
    let alias_map = || fsx::read_json(&admin_json)["config"]["aliases"].clone();
    load(&admin, tid);
    assert_eq!(
        alias_map(),
        obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "carol@example.com" => "测试组-1" }
    );
    // 多了一个账号：原来的编号不变，新的接着编
    lines(&admin, &["team-map", tid, "claude", "Dave@Example.com", "US-A"]);
    assert_eq!(
        alias_map(),
        obj! {
            "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "carol@example.com" => "测试组-1",
            "dave@example.com" => "测试组-2",
        }
    );
    // 去掉一个账号、再加一个：去掉的那个的号不给新加的
    lines(&admin, &["team-map", tid, "claude", "dave@example.com", "-"]);
    lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-A"]);
    assert_eq!(
        alias_map(),
        obj! {
            "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "carol@example.com" => "测试组-1",
            "erin@example.com" => "测试组-3",
        }
    );
    // 另一个 Team：默认编的带着它的名字（给 frank 的是 设计组-1），和「测试组」的 测试组-1 不撞；给 hank 起的 own 和这台设备上的撞了
    let admin2 = fx.device("admin2");
    let (tid2, root2, _) = three(lines(&admin2, &["team-new", "设计 组", "--device", "desk"]));
    let tid2 = tid2.as_str();
    let (_srv2, url2, pin2) = fx.server(&root2, "srv2");
    lines(&admin2, &["team-attach", tid2, &url2, &pin2]);
    lines(&admin2, &["team-map", tid2, "claude", "frank@example.com", "US-C"]);
    lines(&admin2, &["team-map", tid2, "claude", "hank@example.com", "US-C"]);
    lines(&admin2, &["team-alias", tid2, "hank@example.com", "own"]);
    // 成员：只有邮箱的补上，加进来的带着 Team 给的；这台设备上起过的不动；Team 给的别名和这台设备上的（或者先加入的 Team 给的）撞了：叫「设计组-own」
    let bob = Device::new(&fx.d.join("bob"), "bob", &obj! { "alice@example.com" => "US-A", "own(erin@example.com)" => "US-A" });
    with_nodes(&bob);
    lines_in(&bob, &["team-join", "--name", "bob-mini"], &invite(&admin, tid, "bob-mini", &[]).0);
    lines_in(&bob, &["team-join", "--name", "bob-mini"], &invite(&admin2, tid2, "bob-mini", &[]).0);
    let said = team_apply(&bob);
    assert_in("按「测试组」的配置给 Claude 账号 alice@example.com 起了别名 alice", &said);
    assert_eq!(
        table(&bob),
        obj! {
            "alice(alice@example.com)" => "US-A", "own(erin@example.com)" => "US-A", "测试组-1(bob@example.com)" => "US-B",
            "设计组-1(frank@example.com)" => "US-C", "设计组-own(hank@example.com)" => "US-C",
        }
    );
    assert_eq!(team_apply(&bob), "", "再对一遍没有要改的");
    // 管理员改别名：照 Team 给的用着的跟着改（撞了以后换成的那个也是），自己起过的不动
    assert_eq!(code(&admin, &["team-alias", tid, "bob@example.com", "a b"]), "alias");
    assert_eq!(code(&admin, &["team-alias", tid, "bob@example.com", "alice"]), "alias", "同一个 Team 里别的账号叫这个");
    assert_eq!(code(&admin, &["team-alias", tid, "nobody@example.com", "x"]), "no_account");
    assert_eq!(code(&bob, &["team-alias", tid, "bob@example.com", "x"]), "not_admin");
    assert_eq!(
        lines(&admin, &["team-alias", tid, "claude:Bob@Example.com", "小王"]),
        ["测试组-1", "2"],
        "管理员自己这台和 bob 的配置变了"
    );
    assert_eq!(lines(&admin, &["team-alias", tid, "erin@example.com", "erin-x"]), ["测试组-3", "2"]);
    assert_eq!(lines(&admin2, &["team-alias", tid2, "frank@example.com", "f1"]), ["设计组-1", "2"]);
    assert_eq!(lines(&admin2, &["team-alias", tid2, "hank@example.com", "h1"]), ["own", "2"]);
    bob.ok(&["team-sync"]);
    let said = team_apply(&bob);
    assert_in("按「测试组」的配置把 Claude 账号 测试组-1（bob@example.com） 的别名改成 小王", &said);
    assert_in("按「设计 组」的配置把 Claude 账号 设计组-1（frank@example.com） 的别名改成 f1", &said);
    assert_in("按「设计 组」的配置把 Claude 账号 设计组-own（hank@example.com） 的别名改成 h1", &said);
    assert_eq!(
        table(&bob),
        obj! {
            "alice(alice@example.com)" => "US-A", "own(erin@example.com)" => "US-A", "小王(bob@example.com)" => "US-B",
            "f1(frank@example.com)" => "US-C", "h1(hank@example.com)" => "US-C",
        }
    );
    // 这台设备上自己改了名字的：管理员再改，不跟着
    let mut mine = table(&bob);
    let node = sub(&mut mine, &[]).remove("小王(bob@example.com)").expect("账号表里有 小王(bob@example.com)");
    sub(&mut mine, &[]).insert("mybob(bob@example.com)", node);
    put(bob.cca.join("accounts.json"), &mine);
    team_apply(&bob);
    lines(&admin, &["team-alias", tid, "bob@example.com", "老王"]);
    bob.ok(&["team-sync"]);
    assert_not_in("别名改成", &team_apply(&bob));
    assert!(table(&bob).obj().contains_key("mybob(bob@example.com)"));
    // 管理员这台设备上的不动：只有邮箱的不补，以前照 Team 加进来的（还是以前默认的 team-1）也不跟着改
    let own = obj! { "team-1(bob@example.com)" => "US-B", "erin@example.com" => "US-A" };
    put(admin.cca.join("accounts.json"), &own);
    with_nodes(&admin);
    admin.ok(&["team-sync"]);
    team_apply(&admin);
    let now = table(&admin);
    let kept: Vec<(String, Value)> = now
        .obj()
        .iter()
        .filter(|(k, _)| k.contains("bob") || k.contains("erin"))
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    assert_eq!(Value::Obj(Obj::from_pairs(kept)), own);
    // 发布时补上：以前的 Team 里还没有别名的账号（3.5.3-beta.5 以前加进来的）也编上
    let admin2_json = tdir(&admin2, tid2).join("admin.json");
    let mut a = fsx::read_json(&admin2_json);
    sub(&mut a, &["config", "accounts", "claude"]).insert("gina@example.com", "US-D");
    put(&admin2_json, &a);
    assert_eq!(lines(&admin2, &["team-publish", tid2]), ["2", ""]);
    assert_eq!(
        fsx::read_json(&admin2_json)["config"]["aliases"],
        obj! { "frank@example.com" => "f1", "hank@example.com" => "h1", "gina@example.com" => "设计组-3" }
    );
}

/// 别名按工具记（3.5.3-beta.6 起）：各工具各自从 1 号编（「Team 的名字-1」）；同一个邮箱在两个工具里各有各的号，成员设备上各工具的账号表各用各的；
/// cca team alias 写 工具:邮箱 只改这个工具里的、只写邮箱改它在的每个工具里的；3.5.3-beta.5 的 Team（alias_n 是一个数、没有 tool_aliases）
/// 发布时照各工具重新编，成员设备上没有记录的旧编号跟着换；几个账号对调别名一次换好；cca team name 改 Team 的名字，成员设备跟着改，
/// 默认编的别名跟着换成新名字的
fn alias_per_tool(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    let admin_json = tdir(&admin, tid).join("admin.json");
    let adm = || fsx::read_json(&admin_json);
    load(&admin, tid);
    assert_eq!(
        list![&adm()["config"]["tool_aliases"], &adm()["alias_n"]],
        list![
            obj! {
                "claude" => obj! { "alice@example.com" => "alice", "bob@example.com" => "测试组-1" },
                "codex" => obj! { "carol@example.com" => "测试组-1" },
            },
            obj! { "claude" => 1, "codex" => 1 }
        ]
    );
    // 3.5.3-beta.5 的 Team：alias_n 是一个数，别名只有按邮箱的那一份（各工具一起编：carol 是 team-2）
    let mut a = adm();
    sub(&mut a, &[]).insert("alias_n", 2);
    sub(&mut a, &["config"]).remove("tool_aliases").expect("config 里有 tool_aliases");
    let by_email = obj! { "alice@example.com" => "alice", "bob@example.com" => "team-1", "carol@example.com" => "team-2" };
    sub(&mut a, &["config"]).insert("aliases", by_email);
    put(&admin_json, &a);
    let listing = devices(&admin, tid);
    assert_eq!(
        list![&listing["tool_aliases"]["codex"], &listing["aliases"]["carol@example.com"]],
        list![obj! { "carol@example.com" => "测试组-1" }, "测试组-1"],
        "还没发布：管理页给的已经是按工具编好的样子"
    );
    assert!(!adm()["config"].obj().contains_key("tool_aliases"), "team devices 不改 admin.json 里的别名");
    // 成员设备：Codex 的账号表里有 carol，别名是旧版本照 team-2 补上的（没有记录）
    let m = Device::new(&fx.d.join("m"), "m", &obj! {});
    with_nodes(&m);
    put(m.cca.join("codex.json"), &obj! { "team-2(carol@example.com)" => "US-A" });
    let tables = || list![fsx::read_json(m.cca.join("accounts.json")), fsx::read_json(m.cca.join("codex.json"))];
    // 依次照 Team 的配置改这几个工具的账号表：它们在 stderr 上说的接在一起
    let apply = |tools: &[&str]| -> String { tools.iter().map(|tool| team_apply_as(&m, Some(*tool))).collect() };
    lines_in(&m, &["team-join", "--name", "m-mini"], &invite(&admin, tid, "m-mini", &[]).0);
    assert_eq!(
        list![&adm()["config"]["tool_aliases"]["codex"], &adm()["alias_n"]],
        list![obj! { "carol@example.com" => "测试组-1" }, obj! { "claude" => 1, "codex" => 1 }],
        "发布（发邀请）时照各工具重新编，alias_n 换成按工具的"
    );
    let said = apply(&["claude", "codex"]);
    assert!(
        said.contains("按「测试组」的配置把 Codex 账号 team-2（carol@example.com） 的别名改成 测试组-1"),
        "没有记录的旧编号跟着换：{said}"
    );
    let claude_table = obj! { "alice(alice@example.com)" => "US-A", "测试组-1(bob@example.com)" => "US-B" };
    assert_eq!(tables(), list![&claude_table, obj! { "测试组-1(carol@example.com)" => "US-A" }]);
    assert_eq!(apply(&["claude", "codex"]), "", "再对一遍没有要改的");
    // bob 在 Codex 里也有：Codex 里接着编（测试组-2）；Claude 里还是 测试组-1，按邮箱的那一份用它最前面那个工具（Claude）的
    lines(&admin, &["team-map", tid, "codex", "bob@example.com", "US-A"]);
    assert_eq!(
        list![&adm()["config"]["tool_aliases"]["codex"], &adm()["config"]["aliases"]["bob@example.com"]],
        list![obj! { "carol@example.com" => "测试组-1", "bob@example.com" => "测试组-2" }, "测试组-1"]
    );
    m.ok(&["team-sync"]);
    let d = doc(&m, tid).1;
    assert_eq!(
        list![&d["tool_aliases"]["codex"]["bob@example.com"], &d["aliases"]["bob@example.com"]],
        list!["测试组-2", "测试组-1"],
        "配置里两份都带着：旧版本的设备照按邮箱的"
    );
    apply(&["claude", "codex"]);
    assert_eq!(
        tables(),
        list![&claude_table, obj! { "测试组-1(carol@example.com)" => "US-A", "测试组-2(bob@example.com)" => "US-A" }]
    );
    // 改别名：写了工具的只改这个工具里的；同一个工具里重名不行，别的工具里的可以；只写邮箱改它在的每个工具里的
    assert_eq!(code(&admin, &["team-alias", tid, "codex:bob@example.com", "测试组-1"]), "alias", "Codex 里 carol 已经叫 测试组-1");
    assert_eq!(code(&admin, &["team-alias", tid, "agy:bob@example.com", "x"]), "no_account");
    assert_eq!(
        lines(&admin, &["team-alias", tid, "codex:bob@example.com", "alice"]),
        ["测试组-2", "2"],
        "Claude 里的 alice 不碍事"
    );
    m.ok(&["team-sync"]);
    assert_in("按「测试组」的配置把 Codex 账号 测试组-2（bob@example.com） 的别名改成 alice", &apply(&["claude", "codex"]));
    assert_eq!(
        tables(),
        list![&claude_table, obj! { "测试组-1(carol@example.com)" => "US-A", "alice(bob@example.com)" => "US-A" }]
    );
    assert_eq!(lines(&admin, &["team-alias", tid, "bob@example.com", "小王"]), ["测试组-1、alice", "2"]);
    m.ok(&["team-sync"]);
    apply(&["claude", "codex"]);
    assert_eq!(
        tables(),
        list![
            obj! { "alice(alice@example.com)" => "US-A", "小王(bob@example.com)" => "US-B" },
            obj! { "测试组-1(carol@example.com)" => "US-A", "小王(bob@example.com)" => "US-A" }
        ]
    );
    // 两个账号对调别名：成员设备上一次换好（要换走的账号原来的别名不算占着）
    lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-C"]);
    m.ok(&["team-sync"]);
    apply(&["claude", "codex"]);
    assert_eq!(tables()[0]["测试组-2(erin@example.com)"].str(), "US-C");
    for (account, new) in
        [("claude:bob@example.com", "tmp"), ("claude:erin@example.com", "小王"), ("claude:bob@example.com", "测试组-2")]
    {
        lines(&admin, &["team-alias", tid, account, new]);
    }
    m.ok(&["team-sync"]);
    let said = apply(&["claude"]);
    assert_eq!(
        tables()[0],
        obj! { "alice(alice@example.com)" => "US-A", "测试组-2(bob@example.com)" => "US-B", "小王(erin@example.com)" => "US-C" },
        "{said}"
    );
    assert_eq!(apply(&["claude", "codex"]), "", "对调完了，再对一遍没有要改的");
    // 改 Team 的名字：管理员这台马上改，成员设备同步到新的一版时跟着改；这台设备上别的 Team 已经叫这个的不行
    assert_eq!(code(&admin, &["team-name", tid, " "]), "name");
    lines(&admin, &["team-new", "别的组", "--device", "studio"]);
    assert_eq!(code(&admin, &["team-name", tid, "别的组"]), "name", "这台设备上已经有叫这个的 Team");
    assert_eq!(code(&m, &["team-name", tid, "新组"]), "not_admin");
    assert_eq!(lines(&admin, &["team-name", tid, " 新组 "]), ["测试组", "新组", "2"], "管理员自己这台和 m 的配置变了");
    assert_eq!(fsx::read_json(tdir(&admin, tid).join("team.json"))["name"].str(), "新组");
    m.ok(&["team-sync"]);
    assert_eq!(list![&fsx::read_json(tdir(&m, tid).join("team.json"))["name"], &doc(&m, tid).1["name"]], list!["新组", "新组"]);
    // 照原来的名字默认编的别名换成新名字的（号不变），管理员起的不动；m 上照 Team 给的用着的跟着换
    assert_eq!(
        adm()["config"]["tool_aliases"],
        obj! {
            "claude" => obj! { "alice@example.com" => "alice", "bob@example.com" => "新组-2", "erin@example.com" => "小王" },
            "codex" => obj! { "carol@example.com" => "新组-1", "bob@example.com" => "小王" },
        }
    );
    let said = apply(&["claude", "codex"]);
    assert_in("按「新组」的配置把 Claude 账号 测试组-2（bob@example.com） 的别名改成 新组-2", &said);
    assert_in("按「新组」的配置把 Codex 账号 测试组-1（carol@example.com） 的别名改成 新组-1", &said);
    assert_eq!(
        tables(),
        list![
            obj! { "alice(alice@example.com)" => "US-A", "新组-2(bob@example.com)" => "US-B", "小王(erin@example.com)" => "US-C" },
            obj! { "新组-1(carol@example.com)" => "US-A", "小王(bob@example.com)" => "US-A" }
        ]
    );
    // 和服务器对不上（假 curl：连不上）时名字不改，照原来的命令再来一次就行；发布没成时名字已经改了，原因里说照新名字发布
    let named = || fsx::read_json(tdir(&admin, tid).join("team.json"))["name"].str().to_string();
    let reply = admin.home.join("reply");
    fs::create_dir(&reply).expect("建 reply 目录");
    fsx::write(reply.join("team_members"), "000\n");
    assert_eq!((code(&admin, &["team-name", tid, "新组二"]).as_str(), named().as_str()), ("server", "新组"));
    fs::remove_file(reply.join("team_members")).expect("删 reply/team_members");
    fsx::write(reply.join("team_put"), "000\n");
    let r = admin.run(&["team-name", tid, "新组二"]);
    assert_eq!((r.code, r.out.split('\t').next().unwrap_or_default(), named().as_str()), (1, "publish", "新组二"), "{}", r.out);
    assert_in("名字已经改成「新组二」", &r.out);
    assert_in("cca team publish 新组二", &r.out);
    assert_eq!(
        adm()["config"]["tool_aliases"]["claude"]["bob@example.com"].str(),
        "新组二-2",
        "别名跟着名字一起改了，下次发布带上"
    );
    fs::remove_file(reply.join("team_put")).expect("删 reply/team_put");
    assert_eq!(lines(&admin, &["team-publish", "新组二"]), ["2", ""]);
    m.ok(&["team-sync"]);
    assert_eq!(fsx::read_json(tdir(&m, tid).join("team.json"))["name"].str(), "新组二");
    assert_in("Claude 账号 新组-2（bob@example.com） 的别名改成 新组二-2", &apply(&["claude", "codex"]));
}

/// 以前（3.7.8 及以前）默认编的 team-N：管理页上已经是换好的样子，下一次发布时换成「Team 的名字-N」存下来（号不变；换成的那个被别的账号
/// 用着的留着 team-N），再算还是一样；成员设备上没有记录的 team-N 也跟着换；cca team alias 不收 team-数字
fn alias_migrate(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    let admin_json = tdir(&admin, tid).join("admin.json");
    let adm = || fsx::read_json(&admin_json);
    load(&admin, tid);
    lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-C"]);
    lines(&admin, &["team-map", tid, "claude", "dave@example.com", "US-C"]);
    // 3.7.8 的 Team：bob、erin、Codex 的 carol 是默认编的 team-N；dave 是管理员起的「测试组-2」（erin 要换成的就是它）
    let old = obj! {
        "claude" => obj! {
            "alice@example.com" => "alice", "bob@example.com" => "team-1", "erin@example.com" => "team-2", "dave@example.com" => "测试组-2",
        },
        "codex" => obj! { "carol@example.com" => "team-1" },
    };
    let mut a = adm();
    sub(&mut a, &["config"]).insert("tool_aliases", old.clone());
    put(&admin_json, &a);
    let want = obj! {
        "claude" => obj! {
            "alice@example.com" => "alice", "bob@example.com" => "测试组-1", "erin@example.com" => "team-2", "dave@example.com" => "测试组-2",
        },
        "codex" => obj! { "carol@example.com" => "测试组-1" },
    };
    assert_eq!(devices(&admin, tid)["tool_aliases"], want, "管理页上已经是换好的样子");
    assert_eq!(adm()["config"]["tool_aliases"], old, "只算，不改 admin.json");
    // 成员设备：账号表里的 bob 是旧版本照 team-1 补上的（没有记录）
    let m = Device::new(&fx.d.join("m"), "m", &obj! { "team-1(bob@example.com)" => "US-B" });
    with_nodes(&m);
    lines_in(&m, &["team-join", "--name", "m-mini"], &invite(&admin, tid, "m-mini", &[]).0);
    assert_eq!(adm()["config"]["tool_aliases"], want, "发布（发邀请）时换好、存下来");
    assert_eq!(adm()["alias_n"], obj! { "claude" => 3, "codex" => 1 }, "号不变");
    let said = team_apply(&m);
    assert_in("按「测试组」的配置把 Claude 账号 team-1（bob@example.com） 的别名改成 测试组-1", &said);
    let now = table(&m);
    assert_eq!(
        list![&now["测试组-1(bob@example.com)"], &now["team-2(erin@example.com)"], &now["测试组-2(dave@example.com)"]],
        list!["US-B", "US-C", "US-C"],
        "{said}"
    );
    assert_eq!(team_apply(&m), "", "再对一遍没有要改的");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["1", ""], "发邀请只给新的那一台发布：管理员自己这台的这回换上");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["0", ""], "再算还是一样：配置没变");
    // 不收以前默认编号的写法（发布时会换掉），说照名字编的那个；那个能写
    let r = admin.run(&["team-alias", tid, "alice@example.com", "team-7"]);
    assert_eq!((r.code, r.out.split('\t').next().unwrap_or_default()), (1, "alias"), "{}", r.out);
    assert_in("「测试组-7」", &r.out);
    assert_eq!(lines(&admin, &["team-alias", tid, "alice@example.com", "测试组-7"])[0], "alice");
}

/// 管理员锁定别名（cca team alias Team --lock）：配置里带 alias_lock，成员设备上照 Team 给的用（自己改过的换回来，cca rename
/// 改不了：退出码 4），没别名的也补上；管理员这台的账号表不动；解开以后又能改，现在用着的不变
fn alias_lock(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let m = Device::new(&fx.d.join("m"), "m", &obj! { "mine(alice@example.com)" => "US-A", "bob@example.com" => "US-B" });
    with_nodes(&m);
    lines_in(&m, &["team-join"], &invite(&admin, tid, "m-mini", &[]).0);
    team_apply(&m);
    assert_eq!(
        table(&m),
        obj! { "mine(alice@example.com)" => "US-A", "测试组-1(bob@example.com)" => "US-B" },
        "没锁定：自己起的不动"
    );
    assert!(!doc(&m, tid).1.obj().contains_key("alias_lock"), "没锁定时配置里没有这一项（和以前一样）");
    assert_eq!(m.run(&["rename", "测试组-1(bob@example.com)", "b2"]).code, 0);
    // 锁定：管理员自己这台和 m 的配置变了；名单里说锁定着
    assert_eq!(code(&m, &["team-alias-lock", tid, "on"]), "not_admin");
    assert_eq!(lines(&admin, &["team-alias-lock", tid, "on"]), ["测试组", "2"]);
    assert_eq!(lines(&admin, &["team-alias-lock", tid, "on"]), ["测试组", "0"], "已经锁定着：配置没变");
    assert_eq!(devices(&admin, tid)["alias_lock"], Value::Bool(true));
    assert_eq!(admin.ok(&["team-devices", tid]).split('\n').next(), Some("1"));
    m.ok(&["team-sync"]);
    assert_eq!(doc(&m, tid).1["alias_lock"], Value::Bool(true));
    let said = team_apply(&m);
    assert_in("按「测试组」的配置把 Claude 账号 mine（alice@example.com） 的别名改成 alice", &said);
    let given = obj! { "alice(alice@example.com)" => "US-A", "测试组-1(bob@example.com)" => "US-B" };
    assert_eq!(table(&m), given, "自己改过的换回 Team 给的");
    assert_eq!(team_apply(&m), "", "再对一遍没有要改的");
    let r = m.run(&["rename", "alice(alice@example.com)", "x"]);
    assert_eq!(r.code, 4, "{}", r.out);
    assert_in("Team「测试组」", &r.out);
    assert_in("锁定", &r.out);
    assert!(table(&m).obj().contains_key("alice(alice@example.com)"));
    // 手改账号表也换回来；去掉了别名的也补上
    put(m.cca.join("accounts.json"), &obj! { "own(alice@example.com)" => "US-A", "bob@example.com" => "US-B" });
    team_apply(&m);
    assert_eq!(table(&m), given);
    // 不在 Team 里的账号照样能改
    put(m.cca.join("accounts.json"), &with(&table(&m), "zed@example.com", "US-C"));
    assert_eq!(m.run(&["rename", "zed@example.com", "z"]).code, 0);
    // 管理员这台的账号表不动
    put(admin.cca.join("accounts.json"), &obj! { "mybob(bob@example.com)" => "US-B" });
    with_nodes(&admin);
    admin.ok(&["team-sync"]);
    team_apply(&admin);
    assert!(table(&admin).obj().contains_key("mybob(bob@example.com)"));
    assert_eq!(admin.run(&["rename", "mybob(bob@example.com)", "b3"]).code, 0);
    // 改别名照样能改，各台设备跟着换
    lines(&admin, &["team-alias", tid, "bob@example.com", "小王"]);
    m.ok(&["team-sync"]);
    team_apply(&m);
    assert!(table(&m).obj().contains_key("小王(bob@example.com)"));
    // 解开：配置里没有这一项了；又能自己改，现在用着的不变
    assert_eq!(lines(&admin, &["team-alias-lock", tid, "off"]), ["测试组", "2"]);
    assert_eq!(devices(&admin, tid)["alias_lock"], Value::Bool(false));
    m.ok(&["team-sync"]);
    assert!(!doc(&m, tid).1.obj().contains_key("alias_lock"));
    team_apply(&m);
    assert!(table(&m).obj().contains_key("小王(bob@example.com)"));
    assert_eq!(m.run(&["rename", "小王(bob@example.com)", "mybob"]).code, 0);
    assert!(table(&m).obj().contains_key("mybob(bob@example.com)"));
}

/// Team 信任的转发中转（cca team forwarder）：管理员写的时候查（不是域名、公共邮箱的域名、公共的后缀不收）、规整；比现在宽了要先同意
/// （--ok 带新名单的摘要，在锁里核对）；发给拿到 Claude 账号的设备，没拿到的配置不变；设备上照它（这台设备上登记不了：mail-trust-forwarder
/// 退出码 3），第一次拿到、变了说一声，说过的不再说，去掉了说「不再信任」（建 Team 的那台不说）；导出的文件里带着，导入一份更宽的同样要同意
fn mail_forwarders(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    // m 拿到 Claude 的 alice，c 只拿到 Codex 的 carol
    let m = fx.device("m");
    with_nodes(&m);
    lines_in(&m, &["team-join"], &invite(&admin, tid, "m-mini", &["--accounts", "claude:alice@example.com"]).0);
    let c = fx.device("c");
    with_nodes(&c);
    lines_in(&c, &["team-join"], &invite(&admin, tid, "c-mini", &["--accounts", "codex:carol@example.com"]).0);
    team_apply(&m);
    let c_seq = doc(&c, tid).0;
    let mac = cfg!(target_os = "macos"); // 邮箱助手只在 Mac 上有：别处不说转发中转的事
    assert_eq!(lines(&admin, &["team-forwarder", tid]), [tid, "测试组", ""], "没设：一个都不信任");
    assert_eq!(devices(&admin, tid)["mail_forwarders"], list![]);
    for bad in ["gmail.com", "mail.qq.com", "co.uk", "relay.example.com:25", "off,relay.example.com"] {
        assert_eq!(code(&admin, &["team-forwarder", tid, "check", bad]), "domain", "{bad}");
    }
    // 最多 64 个、每一项最长 253 个字符：超了当场不收（不然存进去以后，发布时 doc_problems 不收，这些设备什么改动都收不到）
    let many: Vec<String> = (1..=65).map(|i| format!("r{i}.example.com")).collect();
    let mut argv = vec!["team-forwarder", tid, "check"];
    argv.extend(many.iter().map(String::as_str));
    assert_eq!(code(&admin, &argv), "domain", "65 个");
    let l60 = "a".repeat(60);
    assert_eq!(code(&admin, &["team-forwarder", tid, "check", &format!("{l60}.{l60}.{l60}.{l60}.example.com")]), "domain", "太长");
    assert_eq!(code(&admin, &["team-forwarder", tid, "check"]), "usage");
    assert_eq!(code(&m, &["team-forwarder", tid]), "not_admin");
    let ck = lines(&admin, &["team-forwarder", tid, "check", "Relay.Example.com,b.example.net", "a.relay.example.com"]);
    assert_eq!(ck[..5], [tid, "测试组", "", "b.example.net,relay.example.com", "1"], "规整：小写、去掉被盖住的、排好序；比现在宽");
    let ok = ck[5].clone();
    assert_eq!(ok.len(), 16, "{ck:?}");
    // 宽了：没带同意、同意的是另一份名单的都不改
    assert_eq!(code(&admin, &["team-forwarder", tid, "set", "relay.example.com", "b.example.net"]), "consent");
    assert_eq!(code(&admin, &["team-forwarder", tid, "set", "relay.example.com", "--ok", &ok]), "consent", "同意的是另一份名单");
    assert_eq!(lines(&admin, &["team-forwarder", tid]), [tid, "测试组", ""]);
    assert_eq!(lines(&admin, &["team-forwarder", tid, "set", "b.example.net,relay.example.com", "--ok", &ok]), ["测试组", "2"]);
    assert_eq!(lines(&admin, &["team-forwarder", tid]), [tid, "测试组", "b.example.net,relay.example.com"]);
    assert_eq!(devices(&admin, tid)["mail_forwarders"], list!["b.example.net", "relay.example.com"]);
    m.ok(&["team-sync"]);
    c.ok(&["team-sync"]);
    assert_eq!(doc(&m, tid).1["mail_forwarders"], list!["b.example.net", "relay.example.com"]);
    assert_eq!(doc(&c, tid).0, c_seq, "没拿到 Claude 账号的设备：配置不变");
    if mac {
        // App 起的命令（CCA_GUI：stderr 交给 App，只在出错时给人看）、只读的都不说，也不记成说过了：留给终端里的下一条
        for env in [("CCA_GUI", "1"), ("CCA_RO", "1")] {
            let r = m.run_with(&["team-apply"], "", &[env]);
            assert_eq!(r.code, 0, "{env:?}: {}", r.err);
            assert_not_in("转发中转", &r.err);
        }
    }
    let said = team_apply(&m);
    if mac {
        assert_in("按「测试组」的配置，它给的 Claude 账号信任转发中转 b.example.net、relay.example.com（不安全", &said);
        assert_eq!(team_apply(&m), "", "说过了不再说");
    }
    // 设备上登记不了；不是 Team 给的账号照旧
    let key = key_list(&table(&m)).into_iter().find(|k| k.contains("alice@example.com")).expect("alice 加进来了").to_string();
    let r = m.run(&["mail-trust-forwarder", &key, "relay.example.com"]);
    assert_eq!((r.code, r.out.as_str()), (3, format!("{tid}\t测试组\tb.example.net,relay.example.com\n").as_str()), "{}", r.err);
    assert_eq!(m.run(&["mail-trust-forwarder", &key, "--check"]).code, 3);
    let r = m.run(&["mail-trust-forwarder", "zed@example.com", "--check"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""));
    // 窄了不用同意；一样的不改
    assert_eq!(lines(&admin, &["team-forwarder", tid, "check", "relay.example.com"])[4], "0", "窄了：不用问同意");
    assert_eq!(lines(&admin, &["team-forwarder", tid, "set", "relay.example.com"]), ["测试组", "2"]);
    assert_eq!(lines(&admin, &["team-forwarder", tid, "set", "relay.example.com"]), ["测试组", "0"], "一样的：配置没变");
    m.ok(&["team-sync"]);
    let said = team_apply(&m);
    if mac {
        assert_in("信任转发中转 relay.example.com（不安全", &said);
    }
    // 建 Team 的那台照它（它也拿到了这些账号），但不说
    admin.ok(&["team-sync"]);
    assert_eq!(doc(&admin, tid).1["mail_forwarders"], list!["relay.example.com"]);
    assert_not_in("转发中转", &team_apply(&admin));
    // 导出的文件里带着；导入一份更宽的：check 给出新名单的摘要，apply 不带它不换
    let f = fx.d.join("team.json");
    let fs_ = path_str(&f);
    lines(&admin, &["team-config", "export", tid, fs_]);
    let mut j = fsx::read_json(&f);
    assert_eq!(j["mail_forwarders"], list!["relay.example.com"]);
    sub(&mut j, &[]).insert("mail_forwarders", list!["relay.example.com", "b.example.net"]);
    put(&f, &j);
    let ck = lines(&admin, &["team-config", "check", tid, fs_]);
    assert_eq!((ck[2].as_str(), ck[6].len(), ck[7].as_str()), ("1", 16, "b.example.net,relay.example.com"), "{ck:?}");
    assert_in(
        "Team 给的 Claude 账号信任的转发中转：relay.example.com（不安全） → b.example.net、relay.example.com（不安全）",
        &ck[8..].join("\n"),
    );
    assert_eq!(code(&admin, &["team-config", "apply", tid, fs_, "--sha", &ck[4]]), "consent");
    assert_eq!(lines(&admin, &["team-config", "apply", tid, fs_, "--sha", &ck[4], "--fwd", &ck[6]]), ["2"]);
    assert_eq!(devices(&admin, tid)["mail_forwarders"], list!["b.example.net", "relay.example.com"]);
    // 去掉：配置里没有这一项了，设备上说「不再信任」；不信任也是 Team 定的
    assert_eq!(lines(&admin, &["team-forwarder", tid, "set", "off"]), ["测试组", "2"]);
    assert_eq!(devices(&admin, tid)["mail_forwarders"], list![]);
    m.ok(&["team-sync"]);
    assert!(!doc(&m, tid).1.obj().contains_key("mail_forwarders"));
    if mac {
        assert_in("「测试组」不再信任转发中转", &team_apply(&m));
        assert_eq!(team_apply(&m), "");
    }
    let r = m.run(&["mail-trust-forwarder", &key, "--check"]);
    assert_eq!((r.code, r.out.as_str()), (3, format!("{tid}\t测试组\t\n").as_str()));
    // Team 的配置文件坏了（说不准它给不给哪个账号、信不信）：这台设备给 Claude 账号登记的转发中转也先不算、一个都不信，
    // mail status 说是哪个 Team；读得出来了照旧
    if mac {
        assert_eq!(m.run(&["mail-trust-forwarder", "zed@example.com", "relay.example.com"]).code, 0);
        let status = || {
            let r = m.run(&["mail-status"]);
            assert_eq!(r.code, 0, "{}", r.err);
            r.out
        };
        assert_in("relay.example.com（不安全；这台设备登记的）", &status());
        let f = tdir(&m, tid).join("doc.json");
        let keep = std::fs::read(&f).expect("有配置");
        std::fs::write(&f, "{").expect("写得进去");
        assert_in("这台设备登记的 relay.example.com 先不算数（「测试组」的配置读不出来", &status());
        std::fs::write(&f, keep).expect("写得进去");
        assert_in("relay.example.com（不安全；这台设备登记的）", &status());
    }
}

/// Team 的节点（cca team node）：列出来不带密钥；加、复制（整份复制，密钥不出这台设备）、去掉；还有账号、别的节点、安装客户端
/// 用着的去不掉；没人用的节点不改各台设备的配置，有账号走它了才发出去；实测用的临时内核配置；照实测的结果判断
fn nodes(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let bob = fx.device("bob");
    lines_in(&bob, &["team-join"], &invite(&admin, tid, "bob", &["--accounts", "claude:alice@example.com"]).0);
    let seq = doc(&bob, tid).0;
    let j = jl(&admin, &["team-node-ls", tid, "--json"]);
    assert_eq!(
        list![&j["kind"], &j["team"], &j["name"], &j["group"], &j["install_node"]],
        list!["team_nodes", tid, "测试组", "OpenAi", "US-D"]
    );
    assert_eq!(
        j["accounts"],
        obj! {
            "claude" => obj! { "alice@example.com" => "US-A", "bob@example.com" => "US-B" },
            "codex" => obj! { "carol@example.com" => "US-A" },
            "agy" => obj! {},
        }
    );
    let by = keyed(&j["nodes"], "name");
    assert_eq!(key_list(&by), ["US-A", "US-B", "US-C", "US-D", "US-E"]);
    assert_eq!(
        list![&by["US-A"]["accounts"], &by["US-B"]["dialer"], &by["US-D"]["install"], &by["US-E"]["accounts"]],
        list![list!["claude:alice@example.com", "codex:carol@example.com"], "US-C", true, list![]]
    );
    let listed = lines(&admin, &["team-node-ls", tid]);
    assert!(listed.iter().any(|line| line == "US-E\tsocks5\t192.0.2.10\t1149\t-\t-\t0"), "没有的栏写 -：{listed:?}");
    assert_eq!(code(&bob, &["team-node-ls", tid]), "not_admin");
    // 加：带密码的节点；列出来没有密码
    let hk = obj! {
        "name" => "HK-A", "type" => "socks5", "server" => "192.0.2.20", "port" => 1090, "username" => "u",
        "password" => "s3cret-pass",
    };
    assert_eq!(lines_in(&admin, &["team-node-add", tid], &dumps(&hk)), ["HK-A", "0"], "还没有账号走它：哪台设备的配置都不变");
    assert_eq!(code_in(&admin, &["team-node-add", tid], &dumps(&hk)), "name");
    assert_eq!(code_in(&admin, &["team-node-add", tid], "[1]"), "node");
    assert_eq!(code_in(&admin, &["team-node-add", tid], &dumps(&with(&hk, "name", "CCA-1"))), "node", "照成员设备查配置的那一套查");
    for argv in [vec!["team-node-ls", tid], vec!["team-node-ls", tid, "--json"]] {
        assert_not_in("s3cret-pass", &admin.ok(&argv));
    }
    // 复制：定义一样（连密码），名字、端口换掉；模板不跟着变
    assert_eq!(lines(&admin, &["team-node-copy", tid, "HK-A", "HK-B", "--port", "1091"]), ["HK-B", "0"]);
    assert_eq!(code(&admin, &["team-node-copy", tid, "HK-X", "HK-C"]), "no_node");
    assert_eq!(code(&admin, &["team-node-copy", tid, "HK-A", "US-A"]), "name");
    assert_eq!(code(&admin, &["team-node-copy", tid, "HK-A", "HK-C", "--port", "0"]), "usage");
    let admin_json = tdir(&admin, tid).join("admin.json");
    let px = keyed(&fsx::read_json(&admin_json)["config"]["nodes"]["proxies"], "name");
    assert_eq!(list![&px["HK-B"], &px["HK-A"]["port"]], list![with(&with(&hk, "name", "HK-B"), "port", 1091), 1090]);
    bob.ok(&["team-sync"]);
    assert_eq!(doc(&bob, tid).0, seq, "加了没人用的节点：bob 的配置没变");
    // 有账号走它了才发出去（bob 有 alice）；这时去不掉
    assert_eq!(lines(&admin, &["team-map", tid, "claude", "alice@example.com", "HK-B"]), ["2"]);
    bob.ok(&["team-sync"]);
    assert_eq!(proxy_names(&doc(&bob, tid).1), ["US-D", "HK-B"], "还有安装客户端用的");
    assert_eq!(code(&admin, &["team-node-rm", tid, "HK-B"]), "node_used");
    let r = admin.run(&["team-node-rm", tid, "HK-B"]);
    assert_in("claude:alice@example.com", &r.out);
    assert_eq!(
        ["US-C", "US-D", "NOPE"].map(|n| code(&admin, &["team-node-rm", tid, n])),
        ["node_used", "node_used", "no_node"],
        "别的节点经它连出去、安装客户端用它、没有这个节点"
    );
    lines(&admin, &["team-map", tid, "claude", "alice@example.com", "US-A"]);
    assert_eq!(lines(&admin, &["team-node-rm", tid, "HK-B"]), ["0"]);
    assert_eq!(lines(&admin, &["team-node-rm", tid, "HK-A"]), ["0"]);
    let left = jl(&admin, &["team-node-ls", tid, "--json"]);
    assert!(!left["nodes"].list().iter().any(|n| n["name"] == Value::from("HK-A")));
    // 安装客户端用的节点：只能是这个 Team 的；换了每台设备的配置都带着新的（连同它的定义），原来的没人用了就不再带；- 是不再指定；
    // stdout 是原来的、存上了几台
    assert_eq!(["NOPE", " "].map(|n| code(&admin, &["team-node-install", tid, n])), ["no_node", "usage"]);
    assert_eq!(code(&bob, &["team-node-install", tid, "US-E"]), "not_admin");
    assert_eq!(lines(&admin, &["team-node-install", tid, "US-E"]), ["US-D", "2"], "管理员自己这台和 bob 的配置里都有它");
    bob.ok(&["team-sync"]);
    let d = doc(&bob, tid).1;
    assert_eq!(list![&d["install_node"], proxy_names(&d)], list!["US-E", list!["US-A", "US-E"]]);
    assert_eq!(jl(&admin, &["team-node-ls", tid, "--json"])["install_node"].str(), "US-E");
    assert_eq!(code(&admin, &["team-node-rm", tid, "US-E"]), "node_used");
    assert_eq!(lines(&admin, &["team-node-install", tid, "US-E"]), ["US-E", "0"], "没变：不重新发布");
    assert_eq!(lines(&admin, &["team-node-install", tid, "-"]), ["US-E", "2"]);
    bob.ok(&["team-sync"]);
    let d = doc(&bob, tid).1;
    assert_eq!(list![&d["install_node"], proxy_names(&d)], list!["", list!["US-A"]]);
    assert_eq!(lines(&admin, &["team-node-install", tid, "-"]), ["", "0"]);
    assert_eq!(lines(&admin, &["team-node-install", tid, "US-D"]), ["", "2"]);
    // 实测用的临时内核的配置：每个节点一个入站、带上 dialer-proxy 连着的；要测的网站看走这些节点的账号是哪些工具的
    let f = fx.d.join("test.json");
    let plan = lines(&admin, &["team-node-testconfig", tid, path_str(&f), "US-A", "US-B"]);
    assert_eq!(plan[0], "claude.ai chatgpt.com www.cloudflare.com");
    let rows: Vec<Vec<&str>> = plan[1..].iter().map(|x| x.split('\t').collect()).collect();
    let picked: Vec<(&str, &str, &str)> = rows.iter().map(|r| (r[0], r[2], r[3])).collect();
    assert_eq!(picked, [("US-A", "192.0.2.10", "1145"), ("US-B", "192.0.2.10", "1146")]);
    assert_eq!(fsx::mode(&f), 0o600);
    let cfg = fsx::read_json(&f);
    // str(x['port'])：端口是数就写成十进制，本来是字符串就原样
    let text = |v: &Value| v.as_i64().map_or_else(|| v.str().to_string(), |n| n.to_string());
    let listeners: Vec<Value> =
        cfg["listeners"].list().iter().map(|x| list![&x["name"], &x["listen"], text(&x["port"]), &x["proxy"]]).collect();
    assert_eq!(
        listeners,
        [list!["cca-test-1", "127.0.0.1", rows[0][1], "US-A"], list!["cca-test-2", "127.0.0.1", rows[1][1], "US-B"]]
    );
    let proxies: Vec<&str> = cfg["proxies"].list().iter().map(|p| p["name"].str()).collect();
    assert_eq!(list![proxies, &cfg["rules"]], list![list!["US-A", "US-B", "US-C"], list!["MATCH,REJECT"]]);
    let open: Vec<&str> = ["external-controller", "external-controller-unix", "mixed-port", "tun", "dns"]
        .into_iter()
        .filter(|key| cfg.obj().contains_key(key))
        .collect();
    assert_eq!(open, NONE);
    assert_ne!(admin.run(&["team-node-testconfig", tid, path_str(&f), "US-A"]).code, 0, "文件已经有了：不覆盖");
    fs::remove_file(&f).expect("删 test.json");
    assert_eq!(lines(&admin, &["team-node-testconfig", tid, path_str(&f), "US-E", "--tool", "agy"])[0], "www.cloudflare.com");
    fs::remove_file(&f).expect("删 test.json");
    assert_eq!(
        lines(&admin, &["team-node-testconfig", tid, path_str(&f), "US-E"])[0],
        "claude.ai www.cloudflare.com",
        "没有账号走它：按 Claude"
    );
    fs::remove_file(&f).expect("删 test.json");
    assert_eq!(lines(&admin, &["team-node-testconfig", tid, path_str(&f)]).len(), 6, "没写节点：全部");
    assert_eq!(code(&admin, &["team-node-testconfig", tid, path_str(&fx.d.join("x.json")), "NOPE"]), "no_node");
    // 照实测的结果判断：B 和 A 撞了 IP（3）；C 不通、端口连不上（1）；都不通（2）
    let txt = "claude.ai www.cloudflare.com\nA\tclaude.ai\t203.0.113.1\tUS\tSJC\nA\twww.cloudflare.com\t198.51.100.1\tUS\tSJC\n\
               B\tclaude.ai\t203.0.113.1\tUS\tSJC\nB\twww.cloudflare.com\t198.51.100.1\tUS\tSJC\n";
    let r = admin.run_with(&["team-node-verdict"], txt, &[]);
    assert_eq!(r.code, 3);
    let marks: Vec<Vec<&str>> = splitlines(&r.out).into_iter().map(|x| x.split('\t').take(2).collect()).collect();
    assert_eq!(marks, [["flag", "A"], ["!", "A"], ["flag", "B"], ["!", "B"]]);
    let with_c = format!("{txt}C\tclaude.ai\t-\tcurl: (7) refused\nC\ttcp\tclosed\n");
    let r = admin.run_with(&["team-node-verdict"], &with_c, &[]);
    assert_eq!(r.code, 1);
    let out = admin.run_with(&["team-node-verdict", "--json"], &with_c, &[]).out;
    let j = json::parse(&out).unwrap_or_else(|e| panic!("team-node-verdict --json 给的不是 JSON：{e}：{out}"));
    let verdicts: Vec<Value> = j["nodes"].list().iter().map(|n| pick(n, &["name", "state", "tcp"])).collect();
    assert_eq!(
        list![&j["kind"], &j["hosts"], verdicts],
        list![
            "team_node_test",
            list!["claude.ai", "www.cloudflare.com"],
            list![list!["A", "flag", Value::Null], list!["B", "flag", Value::Null], list!["C", "down", "closed"]]
        ]
    );
    assert_eq!(
        list![&j["nodes"][0]["exits"]["claude.ai"], &j["nodes"][2]["exits"]["claude.ai"]],
        list![obj! { "ip" => "203.0.113.1", "loc" => "US", "colo" => "SJC" }, obj! { "error" => "curl: (7) refused" }]
    );
    assert_eq!(admin.run_with(&["team-node-verdict"], "claude.ai www.cloudflare.com\nA\tclaude.ai\t-\t超时\n", &[]).code, 2);
    // 还没接上服务器的 Team：只改本机；没有节点了是 null
    let lone = fx.device("lone");
    let lid = lines(&lone, &["team-new", "一个人", "--device", "mini"])[0].clone();
    let lid = lid.as_str();
    let j = jl(&lone, &["team-node-ls", lid, "--json"]);
    assert_eq!(list![&j["group"], &j["nodes"], &j["accounts"]], list![Value::Null, list![], obj! {}]);
    assert_eq!(lines_in(&lone, &["team-node-add", lid], &dumps(&hk)), ["HK-A", "0"]);
    assert_eq!(jl(&lone, &["team-node-ls", lid, "--json"])["group"].str(), "OpenAi");
    let (set, unset) = (lines(&lone, &["team-node-install", lid, "HK-A"]), lines(&lone, &["team-node-install", lid, "-"]));
    assert_eq!((set, unset), (vec!["".to_string(), "0".to_string()], vec!["HK-A".to_string(), "0".to_string()]));
    assert_eq!(lines(&lone, &["team-node-rm", lid, "HK-A"]), ["0"]);
    assert!(fsx::read_json(tdir(&lone, lid).join("admin.json"))["config"]["nodes"].is_null());
}

/// 管理员由照文档独立实现的那一份扮演：它封的、签的辅助程序解得开、验得过；伪造的、调包的、旧的不要
fn spec_admin(t: &mut Ctx) {
    let fx = fx(t);
    let rk = fx.d.join("root");
    let root = pki::keygen(&rk);
    let tid = spec::team_id(&root);
    let tid = tid.as_str();
    let (srv, url, pin) = fx.server(&root, "srv");
    let host = format!("127.0.0.1:{}", srv.port);
    let secret = crypto::token_hex(16);
    let (token, dev, keys) = spec::invite_parts(&secret).expect("现造的那串随机数");
    let dev = dev.as_str();
    let invited = obj! { "token" => crypto::sha256_hex(token.as_bytes()), "dev" => dev, "ttl" => 600 };
    let (st, j) = spec::post(&rk, srv.port, "team_invite", &invited);
    assert_eq!(st, 200, "{j}");
    let base = obj! {
        "v" => 1, "at" => 1790000000, "name" => "独立实现的组", "salt" => "cd".repeat(16),
        "server" => obj! { "u" => &url, "p" => list![&pin] },
        "accounts" => obj! { "claude" => obj! { "alice@example.com" => "US-A" } }, "aliases" => obj! {}, "install_node" => "",
        "nodes" => Value::Null,
    };
    // 往服务器上存一份给 dev 的配置：第 seq 版，signer 签名，封给 sealed_for（拿它的密钥 k）
    let store = |seq: i64, content: &Value, signer: &Path, sealed_for: &str, k: &spec::Keys| {
        let ct = spec::team_doc_seal(k, tid, sealed_for, seq, content);
        let env = obj! { "seq" => seq, "ct" => &ct, "sig" => spec::team_doc_sign(signer, tid, sealed_for, seq, &ct) };
        let (st, j) = spec::post(&rk, srv.port, "team_put", &obj! { "docs" => obj! { dev => env } });
        assert_eq!(list![st, j.get("stored")], list![200, list![dev]], "{j}");
    };
    store(3, &base, &rk, dev, &keys);
    let bob = fx.device("bob");
    let link = spec::invite_encode(&url, &[&pin], &root, &secret, "链接里的名字", None, None);
    assert_eq!(
        lines_in(&bob, &["team-join", "--name", "bob"], &link),
        ["独立实现的组", host.as_str(), "bob", "1", tid, "ok", "1"],
        "名字照配置里的（管理员改过名）"
    );
    assert_eq!(doc(&bob, tid), (3, base.clone()));
    // team-sync 说的：(状态, 序号, 原因)
    let sync = || -> (String, i64, String) {
        let fields = sync_fields(&bob);
        let [state, seq, why] = &fields[2..] else { panic!("team-sync 该有五栏：{fields:?}") };
        (state.clone(), seq.parse().expect("序号是数"), why.clone())
    };
    let fine = |seq: i64| ("ok".to_string(), seq, String::new());
    assert_eq!(sync(), fine(3));
    // 不是根密钥签的
    let evil = fx.d.join("evil");
    pki::keygen(&evil);
    store(4, &with(&base, "name", "伪造的"), &evil, dev, &keys);
    let (state, seq, why) = sync();
    assert_eq!((state.as_str(), seq), ("error", 3));
    assert_in("签名", &why);
    // 根密钥签了、却是别的设备的那一份（服务器调包）
    let other = spec::invite_parts(&crypto::token_hex(16)).expect("现造的那串随机数");
    store(5, &with(&base, "name", "别人的"), &rk, &other.1, &other.2);
    let (state, seq, _) = sync();
    assert_eq!((state.as_str(), seq), ("error", 3));
    // 内容样子不对（盐不是十六进制）
    store(6, &with(&base, "salt", "xyz"), &rk, dev, &keys);
    let (state, seq, why) = sync();
    assert_eq!((state.as_str(), seq), ("error", 3));
    assert_in("salt", &why);
    assert_eq!(doc(&bob, tid), (3, base.clone()), "不对的都没换上");
    assert_eq!(jl(&bob, &["team-list", "--json"])["teams"][0]["state"].str(), "error");
    // 对的一版：换上；配置里说服务器搬了，跟着换
    let (srv2, url2, pin2) = fx.server(&root, "srv2");
    let moved = with(&with(&base, "at", 1790000100), "server", obj! { "u" => &url2, "p" => list![&pin2] });
    store(7, &moved, &rk, dev, &keys);
    assert_eq!(sync(), fine(7));
    let tj = fsx::read_json(tdir(&bob, tid).join("team.json"));
    assert_eq!(list![&tj["url"], &tj["pins"]], list![&url2, list![&pin2]]);
    // 新的那台上没有这台设备（记录的编号不一样）：等管理员登记回去
    let (state, seq, _) = sync();
    assert_eq!((state.as_str(), seq), ("lost", 7));
    let public = spec::pub_line(&fsx::read_text(tdir(&bob, tid).join("id_ed25519.pub")));
    let enroll = obj! { "devices" => obj! { dev => obj! { "pub" => public, "name" => "bob" } } };
    let (st, j) = spec::post(&rk, srv2.port, "team_enroll", &enroll);
    assert_eq!(st, 200, "{j}");
    assert_eq!(sync(), fine(7));
    // 服务器给的比手里的旧（新的那台上存了一份旧的）
    let ct = spec::team_doc_seal(&keys, tid, dev, 2, &base);
    let env = obj! { "seq" => 2, "ct" => &ct, "sig" => spec::team_doc_sign(&rk, tid, dev, 2, &ct) };
    assert_eq!(spec::post(&rk, srv2.port, "team_put", &obj! { "docs" => obj! { dev => env } }).1["stored"], list![dev]);
    let (state, seq, why) = sync();
    assert_eq!((state.as_str(), seq), ("error", 7));
    assert_in("旧", &why);
    assert_eq!(doc(&bob, tid), (7, moved));
}

/// 服务器的记录没了：管理员照留着的公钥登记回去、重新发布；换一台服务器：设备从原来那台上知道新的地址，自己换过去
fn records_lost_and_move(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, root, mut srv, url, _pin) = fx.new_team();
    let (tid, root, url) = (tid.as_str(), root.as_str(), url.as_str());
    load(&admin, tid);
    let (link, _dev_b) = invite(&admin, tid, "bob-mini", &[]);
    let bob = fx.device("bob");
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5..], ["ok", "3"]);
    let (link_p, _dev_p) = invite(&admin, tid, "pending", &[]);
    assert_eq!(states(&admin, tid), obj! { "studio" => "joined", "bob-mini" => "joined", "pending" => "invited" });
    let port = srv.port;
    srv.stop();
    for f in ["state.json", "team-docs.json"] {
        fs::remove_file(fx.d.join("srv").join(f)).unwrap_or_else(|e| panic!("删服务器的 {f}：{e}"));
    }
    let (mut srv, _, _) = fx.server_on(root, port, "srv");
    assert_eq!(sync_fields(&bob)[2], "lost");
    assert_eq!(code_in(&fx.device("late"), &["team-join"], &link_p), "denied", "没用掉的邀请跟着没了");
    assert_eq!(states(&admin, tid), obj! { "studio" => "joined", "bob-mini" => "joined", "pending" => "expired" });
    assert_eq!(lines(&admin, &["team-publish", tid]), ["2", ""], "各台的配置重新发布");
    assert_eq!(sync_fields(&bob)[2..4], ["ok", "2"]);
    assert_eq!(sync_fields(&admin)[2], "ok");
    // 换一台服务器（另一张证书）。还没用掉的邀请写的是原来那台的地址：到原来那台上撤回，这边记成过期了（要重新发）
    let (link_q, _dev_q) = invite(&admin, tid, "pending-2", &[]);
    let (mut srv2, url2, pin2) = fx.server(root, "srv2");
    let host2 = format!("127.0.0.1:{}", srv2.port);
    assert_eq!(lines(&admin, &["team-attach", tid, &url2, &pin2]), [host2.as_str(), "studio", "2", "2", ""]);
    assert_eq!(code_in(&fx.device("late-2"), &["team-join"], &link_q), "denied", "原来那台上撤回了");
    let rows = states(&admin, tid);
    assert_eq!((rows["pending-2"].str(), rows["bob-mini"].str()), ("expired", "joined"));
    assert!(!fsx::read_json(tdir(&admin, tid).join("admin.json")).obj().contains_key("moved_from"));
    assert_eq!(fsx::read_json(tdir(&bob, tid).join("team.json"))["url"].str(), url, "还没同步过：还连着原来那台");
    assert_eq!(sync_fields(&bob)[2..4], ["ok", "3"], "从原来那台上取到指着新地址的配置");
    let tj = fsx::read_json(tdir(&bob, tid).join("team.json"));
    assert_eq!(list![&tj["url"], &tj["pins"]], list![&url2, list![&pin2]]);
    srv.stop();
    assert_eq!(sync_fields(&bob)[2..4], ["ok", "3"], "以后找新的那台：手里的就是它存的那一版");
    assert_eq!(lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-A"]), ["2"]);
    assert_eq!(sync_fields(&bob)[2..4], ["ok", "4"]);
    // 原来那台连不上时换：说一声（还连着它的设备要重新邀请）
    srv2.stop();
    let (srv3, url3, pin3) = fx.server(root, "srv3");
    let host3 = format!("127.0.0.1:{}", srv3.port);
    let got = lines(&admin, &["team-attach", tid, &url3, &pin3]);
    assert_eq!(got[..4], [host3.as_str(), "studio", "2", "2"]);
    assert_in("连不上", &got[4]);
}

/// 账号表照 Team 的配置改：给了的账号加进来、节点由它定，这台机器本来就有的只改节点；不再给了的收起来（登录留着）、再给回来照原样；
/// 被移除以后这台机器本来就有的留着（节点不再被固定）
fn accounts_follow(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let (link, _dev_b) = invite(&admin, tid, "bob-mini", &["--accounts", "claude:alice@example.com,claude:bob@example.com"]);
    // 两个账号都是这台机器本来就有的：bob 起过别名，alice 只有邮箱、走 US-C
    let own = obj! { "mine(bob@example.com)" => "US-D", "alice@example.com" => "US-C" };
    let bob = Device::new(&fx.d.join("bob"), "bob", &own);
    with_nodes(&bob);
    let now_table = || table(&bob);
    let apply = || team_apply(&bob);
    assert_eq!(apply(), "", "还没加入 Team：没有要改的");
    assert_eq!(now_table(), own);
    // 加入：Team 给了 alice（US-A，别名 alice）和 bob（US-B）
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5..], ["ok", "2"]);
    let said = apply();
    assert_in("alice（alice@example.com） 的出口按「测试组」的配置固定为 US-A（账号表里写的是 US-C，已改回）", &said);
    assert_in("mine（bob@example.com） 的出口按「测试组」的配置固定为 US-B（账号表里写的是 US-D，已改回）", &said);
    assert_in("按「测试组」的配置给 Claude 账号 alice@example.com 起了别名 alice", &said);
    let fixed = obj! { "mine(bob@example.com)" => "US-B", "alice(alice@example.com)" => "US-A" };
    assert_eq!(now_table(), fixed);
    assert_eq!(apply(), "", "再对一遍没有要改的");
    let cfg = || bob.config();
    assert!(!dumps(&cfg()).contains("team_added"), "这两个账号都是这台机器本来就有的：Team 只管节点，不记成它加的");
    // 管理员多给一个 erin：这台机器上没有，加进来（记成 Team 加的）
    assert_eq!(lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-D"]), ["1"]);
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "+claude:erin@example.com"]), ["3 个账号"]);
    assert_eq!(sync_fields(&bob)[2], "ok");
    let said = apply();
    assert!(
        said.contains("按「测试组」的配置添加了 Claude 账号 测试组-2（erin@example.com），出口 US-D"),
        "新加的账号编成 测试组-2（Claude 里的第二个：Codex 的 carol 另编）：{said}"
    );
    assert_eq!(now_table()["测试组-2(erin@example.com)"].str(), "US-D");
    let erin = ident("erin@example.com");
    assert_eq!(cfg()["team_added"], obj! { &erin => tid });
    // 手里那一份配置一时读不出来（文件坏了）：说不准还给不给，不把 Team 加的账号收起来
    let doc_json = tdir(&bob, tid).join("doc.json");
    let good = fsx::read_text(&doc_json);
    fsx::write(&doc_json, "{");
    assert_not_in("收起来", &apply());
    assert!(now_table().obj().contains_key("测试组-2(erin@example.com)"));
    fsx::write(&doc_json, &good);
    apply();
    assert_eq!(list![&now_table()["测试组-2(erin@example.com)"], &now_table()["alice(alice@example.com)"]], list!["US-D", "US-A"]);
    // 不再给 erin：收起来（登录留着），再给回来照原来的键
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "-claude:erin@example.com"]), ["2 个账号"]);
    bob.ok(&["team-sync"]);
    assert_in("Team 不再给这台设备 Claude 账号 测试组-2（erin@example.com）：收起来了", &apply());
    assert!(!now_table().obj().contains_key("测试组-2(erin@example.com)"));
    assert_eq!(
        list![&cfg()["parked"], cfg().obj().contains_key("team_added")],
        list![obj! { &erin => obj! { "key" => "测试组-2(erin@example.com)", "node" => "US-D", "team" => tid } }, false]
    );
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "+claude:erin@example.com"]), ["3 个账号"]);
    bob.ok(&["team-sync"]);
    assert_in("添加了 Claude 账号 测试组-2（erin@example.com）", &apply());
    assert_eq!((now_table().obj().contains_key("测试组-2(erin@example.com)"), cfg().obj().contains_key("parked")), (true, false));
    // 不再给 bob@example.com：它是这台机器自己的账号，留着（节点不再被固定）
    assert_eq!(lines(&admin, &["team-grant", tid, "bob-mini", "-claude:bob@example.com"]), ["2 个账号"]);
    bob.ok(&["team-sync"]);
    assert_eq!(apply(), "");
    assert_eq!(now_table()["mine(bob@example.com)"].str(), "US-B");
    // 被管理员移除了：手里那一份不再算数。Team 加的 erin 收起来；这台机器本来就有的 alice 留着（节点不再被固定）
    assert_eq!(lines(&admin, &["team-revoke", "apply", tid, "bob-mini"]), ["bob-mini"]);
    assert_eq!(sync_fields(&bob)[2], "removed");
    assert_in("Team 不再给这台设备 Claude 账号 测试组-2（erin@example.com）：收起来了", &apply());
    assert_eq!(now_table(), fixed);
    assert_eq!(apply(), "", "再对一遍没有要改的");
}

/// Team 给的账号：记下的 API key、折合金额、设备统计都经这个 Team 的同步服务器（盐是配置里的那一份、签名用这台设备在 Team 里的密钥），
/// 不去别的服务器；被移除以后不再经它
fn pipelines(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let (link, _dev_b) = invite(&admin, tid, "bob-mini", &["--accounts", "claude:alice@example.com"]);
    let bob = fx.device("bob");
    with_nodes(&bob);
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5..], ["ok", "1"]);
    assert_eq!(bob.run(&["team-apply"]).code, 0);
    let key = "alice(alice@example.com)";
    assert_eq!(table(&bob), obj! { key => "US-A" });
    bob.login_as(Some("u-alice-0001"), key);
    let salt = doc(&bob, tid).1["salt"].str().to_string();
    let aid = spec::aid_of(&salt, "claude", "alice@example.com");
    let fp = spec::fp_of(tdir(&bob, tid).join("id_ed25519.pub"));
    // 这台设备向服务器发过的这种请求：{(签名的设备指纹, 涉及的账号摘要…)}
    let asked = |op: &str| -> BTreeSet<(String, Vec<String>)> {
        let involved = |r: &Value| -> Vec<String> {
            let named = ["get", "put", "accounts"].into_iter().filter_map(|k| r.get(k).filter(|v| truthy(v)));
            let all: BTreeSet<String> = named.flat_map(|v| items(v)).map(str::to_string).collect();
            all.into_iter().collect()
        };
        bob.sent(Some(op)).iter().map(|r| (r["key"].str().to_string(), involved(r))).collect()
    };
    // 记下的 API key
    let k1 = format!("sk-ant-api03-{}", "a".repeat(40));
    assert_eq!(bob.keyset_as(&k1, key).trim(), "shared");
    bob.ok(&["key-sync", "force"]);
    let state = fsx::read_json(bob.cca.join("presence").join(format!("team.{tid}.keys.json")));
    assert_eq!(list![state.get("error"), state.get("synced").map_or(0.0, Value::f64) > 0.0], list![Value::Null, true], "{state}");
    assert_eq!(asked("keys"), BTreeSet::from([(fp.clone(), vec![aid.clone()])]));
    let blob = fsx::read_json(fx.d.join("srv").join("keys.json"))["acc"][aid.as_str()]["blob"].clone();
    let share_keys = spec::derive(spec::KEYSHARE, &salt, "claude", "alice@example.com", "u-alice-0001");
    let got = spec::keyshare_open(&share_keys, &aid, &blob).expect("服务器上存的照文档解得开");
    assert_eq!(got["key"].str(), k1, "服务器上存的照文档解得开：盐是 Team 的配置里的那一份");
    assert!(!bob.cca.join("presence").join("keys.json").exists(), "不带前缀的那一份（3.6.0 以前维护者的服务器的）没有");
    // 折合金额
    let hour = (fleet::now() / 3600.0).floor() as i64 * 3600 - 3600;
    fs::create_dir_all(bob.cca.join("cost")).expect("建 cost 目录");
    let local = obj! {
        "v" => 1, "build" => "x", "who" => obj! { "claude:u-alice-0001" => "alice@example.com" },
        "acc" => obj! { "claude:u-alice-0001" => obj! { hour => list![1000000, 1, 1, 1, 0, 0, 0] } },
        "um" => obj! {}, "orphan" => obj! {},
    };
    put(bob.cca.join("cost").join("local.json"), &local);
    let r = bob.cost_sync(true);
    let hubs: Vec<Value> = r["hubs"].list().iter().map(|h| list![&h["kind"], h.get("team"), &h["state"]]).collect();
    assert_eq!(list![&r["state"], hubs], list!["ok", list![list!["team", tid, "ok"]]], "{r}");
    let oaid = spec::orphan_aid(&salt);
    let mut both = vec![aid.clone(), oaid.clone()];
    both.sort_unstable();
    assert_eq!(asked("ledger"), BTreeSet::from([(fp.clone(), both)]), "这个账号的，和认不出是哪个账号的那一份");
    let slots = fsx::read_json(fx.d.join("srv").join("cost-slots.json"))["acc"].clone();
    let cost_keys = spec::team_keys(&salt, spec::COST);
    let slot =
        |aid: &str| spec::cost_open(&cost_keys, aid, &fp, &slots[aid][fp.as_str()]["blob"], spec::COST).expect("照文档解得开");
    let hours: Vec<Value> = slot(&aid)["h"].obj().values().cloned().collect();
    assert_eq!(hours, [list![1000000, 1, 1, 1, 0, 0, 0]]);
    assert_eq!(slot(&oaid)["h"], obj! {}, "认不出的那一份：这台设备没有，传空的");
    assert!(bob.cca.join("cost").join(format!("team.{tid}.sync.json")).exists());
    // 设备统计
    let v = jl(&bob, &["presence-view", "--json"]);
    let accounts: Vec<Value> = v["accounts"].list().iter().map(|x| pick(x, &["key", "hub", "team"])).collect();
    let hubs: Vec<Value> = v["hubs"].list().iter().map(|h| pick(h, &["kind", "team"])).collect();
    assert_eq!(
        list![accounts, hubs],
        list![list![list![key, "team", tid]], list![list!["team", tid]]],
        "是哪个 Team 的写在旁边（可能同时在几个 Team 里）"
    );
    assert_eq!(asked("view"), BTreeSet::from([(fp.clone(), vec![aid.clone()])]));
    // 被管理员移除了：不再经它；经它同步时留下的状态文件删掉（里面有解开了的别的设备的折合金额）
    let left = || -> Vec<String> {
        let dirs = ["presence", "cost"].map(|d| bob.cca.join(d));
        let found = dirs.iter().filter(|d| d.exists()).flat_map(|d| names_in(d)).filter(|n| n.starts_with("team."));
        let mut names: Vec<String> = found.collect();
        names.sort_unstable();
        names
    };
    let json_left: Vec<String> = left().into_iter().filter(|n| n.ends_with(".json")).collect();
    assert_eq!(json_left, [format!("team.{tid}.keys.json"), format!("team.{tid}.sync.json")]);
    assert_eq!(lines(&admin, &["team-revoke", "apply", tid, "bob-mini"]), ["bob-mini"]);
    assert_eq!(sync_fields(&bob)[2], "removed");
    assert_eq!(left(), NONE);
    let n = bob.sent(None).len();
    bob.ok(&["key-sync", "force"]);
    assert_eq!(bob.sent(None).len(), n, "没有用得了的服务器：不发请求");
}

/// 上报进程里 Team 的那一路：没有会话开着时一次都不报；开着时拿这台设备在 Team 里的密钥报给它的服务器；进程开着时加入的另一个
/// Team 接着就有一路（各报各的账号）；被管理员移除以后那一路停了，另一路照旧
fn agent(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let (link, _) = invite(&admin, tid, "bob-mini", &["--accounts", "claude:alice@example.com"]);
    let bob = fx.device("bob");
    with_nodes(&bob);
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5..], ["ok", "1"]);
    assert_eq!(bob.run(&["team-apply"]).code, 0);
    let alice = "alice(alice@example.com)";
    bob.login_as(Some("u-alice-0001"), alice);
    // 这台设备在某个 Team 里报某个账号时用的：(签名的设备指纹, 账号摘要)
    let who = |team: &str, email: &str| -> (String, String) {
        let salt = doc(&bob, team).1["salt"].str().to_string();
        (spec::fp_of(tdir(&bob, team).join("id_ed25519.pub")), spec::aid_of(&salt, "claude", email))
    };
    let (fp, aid) = who(tid, "alice@example.com");
    let reports = |f: &str| -> Vec<Value> { bob.sent(Some("report")).into_iter().filter(|r| r["key"] == Value::from(f)).collect() };
    // 等到这一路报了：报过的那几次（等到的时候不是空的）
    let reported = |what: &str, f: &str| wait_for(what, WAIT, || Some(reports(f)).filter(|got| !got.is_empty()));
    // 没有会话开着：一次都不报（不必告诉那台服务器这台设备开着），进程自己退出
    // 假 ps 的进程表里只有一个不相干的进程（一行都没有时上报进程当成没问到进程表，一直等）
    fsx::write(bob.home.join("ps.out"), format!("{:5} {:5} {:<8} {:>9} {} {}\n", 1, 0, "??", "0:00.50", T0, "/sbin/launchd"));
    assert_eq!(fleet::agent(t, &bob).wait(Duration::from_secs(30)), 0);
    let nothing: [Value; 0] = [];
    assert_eq!(bob.sent(Some("report")), nothing);
    // 开着一个会话
    fleet::session(&bob, &CliSession { key: alice, ..Default::default() });
    fleet::agent(t, &bob);
    let got = reported("报给 Team 的服务器", &fp);
    let [r] = &got[..] else { panic!("该正好报了一次：{got:?}") };
    assert_eq!(list![&r["code"], items(&r["units"])], list![200, list![&aid]]);
    // 进程开着时加入第二个 Team（另一台服务器），它给的是 bob@example.com
    let admin2 = fx.device("admin2");
    let (tid2, root2, _) = three(lines(&admin2, &["team-new", "第二组", "--device", "imac"]));
    let tid2 = tid2.as_str();
    let (_srv2, url2, pin2) = fx.server(&root2, "srv2");
    lines(&admin2, &["team-attach", tid2, &url2, &pin2]);
    load(&admin2, tid2);
    let (link2, _) = invite(&admin2, tid2, "bob-mini", &["--accounts", "claude:bob@example.com"]);
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link2)[5..], ["ok", "1"]);
    assert_eq!(bob.run(&["team-apply"]).code, 0);
    bob.login_as(Some("u-bob-0001"), "bob@example.com");
    fleet::session(&bob, &CliSession { key: "bob@example.com", pid: SESSION_PID - 1, ..Default::default() });
    let (fp2, aid2) = who(tid2, "bob@example.com");
    let got2 = reported("新加入的那个 Team 也有了一路", &fp2);
    let [r2] = &got2[..] else { panic!("该正好报了一次：{got2:?}") };
    assert_eq!(list![&r2["code"], items(&r2["units"])], list![200, list![&aid2]]);
    assert!(reports(&fp).iter().all(|x| items(&x["units"]) == [aid.as_str()]), "各报各的账号");
    // 被第一个 Team 的管理员移除：同步配置时知道了，那一路停了（alice 又开一个会话也不报）；第二个 Team 那一路照旧
    assert_eq!(lines(&admin, &["team-revoke", "apply", tid, "bob-mini"]), ["bob-mini"]);
    assert_eq!(sync_fields(&bob)[2], "removed");
    fleet::more_rounds(&[&bob], 2);
    let n = reports(&fp).len();
    fleet::session(&bob, &CliSession { key: alice, pid: SESSION_PID - 2, ..Default::default() });
    fleet::session(&bob, &CliSession { key: "bob@example.com", pid: SESSION_PID - 3, ..Default::default() });
    wait_for("第二个 Team 那一路报了新开的会话", WAIT, || reports(&fp2).len() >= 2);
    fleet::more_rounds(&[&bob], 2);
    assert_eq!(reports(&fp).len(), n, "被移除的那一路不再报");
    let codes: Vec<Value> = reports(&fp2).iter().map(|x| x["code"].clone()).collect();
    assert_eq!(codes, vec![Value::from(200); codes.len()]);
}

/// 没有指纹的服务器（托管平台上的、反向代理后面的）：照系统的证书机构验证书（不带 -k、--pinnedpubkey）；验不过时接不上、
/// 加入不了，什么都不留
fn ca_mode(t: &mut Ctx) {
    let fx = fx(t);
    let mut admin = fx.device("admin");
    let (tid, root, _) = three(lines(&admin, &["team-new", "托管组", "--device", "studio"]));
    let tid = tid.as_str();
    let d = fx.d.join("ca");
    fs::create_dir(&d).expect("建 ca 目录");
    pki::make_ca_cert(&d);
    let srv = Server::start(&d, &root);
    let url = format!("https://127.0.0.1:{}", srv.port);
    let host = format!("127.0.0.1:{}", srv.port);
    let ca = path_str(&d.join("ca.pem")).to_string();
    assert_eq!(code(&admin, &["team-attach", tid, &url]), "cert", "系统的证书机构不认这个证书");
    let argv = fsx::read_text(admin.home.join("curl.argv"));
    assert!(!argv.contains("--pinnedpubkey") && !argv.contains(" -k "), "{argv}");
    assert_eq!(fsx::read_json(tdir(&admin, tid).join("team.json"))["url"].str(), "", "没接上：本机照旧");
    admin.env.insert("CURL_CA_BUNDLE".to_string(), ca.clone());
    assert_eq!(lines(&admin, &["team-attach", tid, &url])[0], host);
    assert_eq!(fsx::read_json(tdir(&admin, tid).join("team.json"))["pins"], list![]);
    assert_eq!(
        lines(&admin, &["team-leave", "plan", tid]),
        [format!("{tid}\t托管组\tadmin\t{host}")],
        "照证书机构验的（托管平台、自己配的反向代理）不是安装脚本装的：删除 Team 时不猜去哪卸载"
    );
    let (link, _) = invite(&admin, tid, "bob-mini", &[]);
    let mut bob = fx.device("bob");
    assert_eq!(code_in(&bob, &["team-join", "--name", "bob-mini"], &link), "cert");
    let teams = bob.cca.join("teams");
    let dirs: Vec<String> =
        if teams.exists() { names_in(&teams).into_iter().filter(|n| teams.join(n).is_dir()).collect() } else { Vec::new() };
    assert_eq!(dirs, NONE, "什么都不留");
    bob.env.insert("CURL_CA_BUNDLE".to_string(), ca);
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5], "ok");
    let argv = fsx::read_text(bob.home.join("curl.argv"));
    assert!(!argv.contains("--pinnedpubkey") && !argv.contains(" -k "), "{argv}");
}

/// 删除 Team（管理员的 team-leave）时去哪卸载它的同步服务器：cca team deploy 记下的照它（装好了没接上、再手动接上的也是），从接着的
/// 一台换到别的地址就不算了，照部署的默认猜；中间空着的项照样占一栏（bash 按 \037 分）。apply --server-gone（刚卸掉了）不再告诉
/// 服务器。team-undeploy 只收 SSH 的几项和 --dir
fn delete_plan(t: &mut Ctx) {
    let fx = fx(t);
    let admin = fx.device("admin");
    let (tid, root, _) = three(lines(&admin, &["team-new", "机房组", "--device", "studio"]));
    let (tid, root) = (tid.as_str(), root.as_str());
    let plan = || lines(&admin, &["team-leave", "plan", tid]);
    assert_eq!(plan(), [format!("{tid}\t机房组\tadmin\t")], "没接上、没部署过：没有要卸的");
    let admin_json = tdir(&admin, tid).join("admin.json");
    let mut a = fsx::read_json(&admin_json);
    // cca team deploy 装好了、没接上
    sub(&mut a, &[]).insert("deploy", obj! { "target" => "ops@vps", "ssh_port" => "", "dir" => "/srv/team" });
    put(&admin_json, &a);
    assert_eq!(plan(), [format!("{tid}\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1")]);
    let key = fx.d.join("vps_key");
    fsx::write(&key, "私钥\n");
    // 部署时写了 -i（记下的是绝对路径）
    sub(&mut a, &["deploy"]).insert("identity", path_str(&key));
    put(&admin_json, &a);
    assert_eq!(
        plan(),
        [format!("{tid}\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1\t{}", path_str(&key))],
        "部署时写了私钥：最后一栏是它（App 里删除时照它登录）"
    );
    fs::remove_file(&key).expect("删 vps_key");
    assert_eq!(plan(), [format!("{tid}\t机房组\tadmin\t\tops@vps\t\t/srv/team\t1")], "私钥文件不在了：不给（照 ssh 的配置登录）");
    let (srv, url, pin) = fx.server(root, "srv");
    lines(&admin, &["team-attach", tid, &url, &pin]);
    assert_eq!(
        plan(),
        [format!("{tid}\t机房组\tadmin\t127.0.0.1:{}\tops@vps\t\t/srv/team\t1", srv.port)],
        "再手动接上：照旧是记下的那台"
    );
    let (mut srv2, url2, pin2) = fx.server(root, "srv2");
    lines(&admin, &["team-attach", tid, &url2, &pin2]);
    assert_eq!(
        plan(),
        [format!("{tid}\t机房组\tadmin\t127.0.0.1:{}\troot@127.0.0.1\t22\t/opt/cca-team-{}\t0", srv2.port, &tid[..8])],
        "换到别的地址：记下的那台不算了，照部署的默认猜"
    );
    assert!(!fsx::read_json(&admin_json).obj().contains_key("deploy"));
    assert_eq!(code(&admin, &["team-undeploy", tid]), "usage", "要写服务器");
    for bad in [&["--port", "18445"][..], &["--no-web"], &["--dir", "/etc/cca"], &["--ssh-port", "0"]] {
        let mut argv = vec!["team-undeploy", tid, "root@192.0.2.10"];
        argv.extend_from_slice(bad);
        assert_eq!(code(&admin, &argv), "usage", "{bad:?}");
    }
    assert_eq!(code(&fx.device("bob"), &["team-undeploy", tid, "root@192.0.2.10"]), "no_team");
    srv2.stop();
    let host2 = format!("127.0.0.1:{}", srv2.port);
    assert_eq!(
        lines(&admin, &["team-leave", "apply", tid, "--server-gone"]),
        ["机房组", host2.as_str(), ""],
        "服务器刚卸掉了：不再去告诉它（连不上也不说）"
    );
    assert!(!tdir(&admin, tid).exists());
}

/// 给一把指定的公钥的邀请（cca team invite --key：设备拿已经有的密钥加入，这里由照文档独立实现的那一份扮演）：拿这把密钥加入的
/// 照常；邀请被别的密钥用了，管理员对账时认出来、名单里说，发布时不再给它账号（加入时取走的那一份收不回来），授权照旧
fn invite_for_a_key(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let keyfile = |n: usize| fx.d.join(format!("key-{n}"));
    let pubs: Vec<String> = (1..=3).map(|n| pki::keygen(keyfile(n))).collect();
    assert_eq!(code(&admin, &["team-invite", tid, "m1", "--key", "不是公钥"]), "usage");
    let (link1, d1) = invite(&admin, tid, "m1", &["--key", &pubs[0]]);
    let (link2, d2) = invite(&admin, tid, "m2", &["--key", &pubs[1]]);
    // 照文档拿 key-n 那把密钥加入：(设备编号, 它那份配置的两把密钥)
    let join = |link: &str, n: usize, name: &str| -> (String, spec::Keys) {
        let k = spec::invite_decode(link).expect("邀请链接读得出来")["k"].str().to_string();
        let (token, dev, keys) = spec::invite_parts(&k).expect("链接里的 k 是那串随机数的样子");
        let public = &pubs[n - 1];
        let asking = obj! { "pub" => public, "name" => name, "token" => token, "proof" => spec::team_pub_proof(&k, public) };
        let (st, j) = spec::post(keyfile(n), srv.port, "team_join", &asking);
        assert_eq!(st, 200, "{j}");
        (dev, keys)
    };
    // key-n 那台设备现在取得到的配置里给了几个账号
    let accounts = |n: usize, (dev, keys): &(String, spec::Keys)| -> usize {
        let (st, j) = spec::post(keyfile(n), srv.port, "team_get", &obj! {});
        assert_eq!(st, 200, "{j}");
        let env = &j["doc"];
        let opened = spec::team_doc_open(keys, tid, dev, env["seq"].i64(), &env["ct"]).expect("自己那份配置的密钥解得开");
        opened["accounts"].obj().values().map(|of_tool| items(of_tool).len()).sum()
    };
    // m2 的邀请被别的机器（key-3）用了
    let (m1, m2) = (join(&link1, 1, "m1"), join(&link2, 3, "m2"));
    assert_eq!((&m1.0, &m2.0), (&d1, &d2));
    assert_eq!((accounts(1, &m1), accounts(3, &m2)), (3, 3), "加入时取得到给这个邀请的那一份（全部账号）");
    let rows = keyed(&devices(&admin, tid)["devices"], "label");
    let seen: Vec<Value> = ["m1", "m2"].into_iter().map(|x| pick(&rows[x], &["state", "grants"])).collect();
    assert_eq!(seen, [list!["joined", "all"], list!["joined", "all"]], "授权没动（服务器一时说错、后来对上了就照旧）");
    assert!(rows["m1"]["problem"].is_null(), "拿指定的那把密钥加入的：没问题");
    assert!(truthy(&rows["m2"]["problem"]), "名单里说：拿这个邀请加入的不是指定的那把密钥");
    lines(&admin, &["team-publish", tid]);
    assert_eq!((accounts(1, &m1), accounts(3, &m2)), (3, 0), "发布以后不再给它账号；拿对了密钥的照旧");
}

/// 把这台设备上自己的官方账号加进 Team：别的 Team 给的（这台设备只是成员）、第三方 API 账号不算；节点、别名照这台设备上的
fn import_mine(t: &mut Ctx) {
    let fx = fx(t);
    let own = obj! {
        "me(me@example.com)" => "US-C", "alice(alice@example.com)" => "US-A", "plain@example.com" => "US-D",
        "api(k@example.com)" => obj! { "node" => "US-B", "base_url" => "https://api.example.com" },
    };
    let admin = Device::new(&fx.d.join("admin"), "admin", &own);
    // 别人当管理员的 Team：给这台设备 alice
    let other = fx.device("other");
    let (tid2, root2, _) = three(lines(&other, &["team-new", "别人的组", "--device", "desk"]));
    let tid2 = tid2.as_str();
    let (_srv2, url2, pin2) = fx.server(&root2, "srv2");
    lines(&other, &["team-attach", tid2, &url2, &pin2]);
    lines(&other, &["team-map", tid2, "claude", "alice@example.com", "US-A"]);
    assert_eq!(lines_in(&admin, &["team-join", "--name", "studio"], &invite(&other, tid2, "studio", &[]).0)[5], "ok");
    let (tid, root, _) = three(lines(&admin, &["team-new", "我的设备", "--device", "studio"]));
    let tid = tid.as_str();
    assert_eq!(lines(&admin, &["team-import", tid, "--mine"]), ["2", "0", "0"]);
    let a = fsx::read_json(tdir(&admin, tid).join("admin.json"));
    assert_eq!(
        list![&a["config"]["accounts"]["claude"], &a["config"]["aliases"]],
        list![
            obj! { "me@example.com" => "US-C", "plain@example.com" => "US-D" },
            obj! { "me@example.com" => "me", "plain@example.com" => "我的设备-1" }
        ],
        "这台设备上起过的别名带进去，没起过的编成「我的设备-1」"
    );
    // 接上服务器以后发布：管理员自己这台拿到的就是这两个账号
    let (_srv, url, pin) = fx.server(&root, "srv");
    lines(&admin, &["team-attach", tid, &url, &pin]);
    assert_eq!(sorted_items(&doc(&admin, tid).1["accounts"]["claude"]), ["me@example.com", "plain@example.com"]);
    assert_eq!(code(&admin, &["team-import", tid, "--mine", "x"]), "usage");
}

/// 导出、导入、修改 Team 的配置（team-config：cca team export / import / edit 用）：导出的文件只给自己读写、各工具都列出来、每个账号带着
/// 别名；check 说会改什么（不改），apply 照文件整份换掉、发布（拿着这些账号的设备同步到新的一版）；check 之后文件又改过了不换；写错了的
/// 不导；--edit 那一份在 Team 的目录里，--base 提醒这中间在别处改过；testconfig 拼的内核配置带着全部的节点定义
fn config_file(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    load(&admin, tid);
    let member = fx.device("member");
    let (link, _) = invite(&admin, tid, "member", &["--accounts", "claude:alice@example.com,claude:bob@example.com"]);
    assert_eq!(lines_in(&member, &["team-join", "--name", "member"], &link)[5], "ok");
    let seq0 = doc(&member, tid).0;
    // 导出
    let out = fx.d.join("conf.json");
    let file = path_str(&out);
    assert_eq!(lines(&admin, &["team-config", "export", tid, file])[..5], [file, tid, "测试组", "3", "5"]);
    assert_eq!(fsx::mode(&out), 0o600, "有节点的密钥：只给自己读写");
    let e = fsx::read_json(&out);
    assert_eq!(
        list![&e["kind"], &e["version"], &e["team"], &e["alias_lock"], &e["install_node"]],
        list!["cca-team-config", 1, "测试组", false, "US-D"]
    );
    assert_eq!(key_list(&e["accounts"]), ["claude", "codex", "agy"], "各工具都列出来（没有账号的是空的）");
    assert_eq!(
        e["accounts"]["claude"],
        obj! {
            "alice@example.com" => obj! { "node" => "US-A", "alias" => "alice" },
            "bob@example.com" => obj! { "node" => "US-B", "alias" => "测试组-1" },
        }
    );
    assert_eq!(e["nodes"], config()["nodes"]);
    assert_eq!(code(&admin, &["team-config", "export", tid, file]), "file", "文件已经有了：不覆盖");
    lines(&admin, &["team-config", "export", tid, file, "--force"]);
    assert_eq!(code(&member, &["team-config", "export", tid, path_str(&fx.d.join("m.json"))]), "not_admin");
    assert_eq!(code(&admin, &["team-config", "nope"]), "usage");
    // 原样导入：没有要改的
    let same = lines(&admin, &["team-config", "check", tid, file]);
    assert_eq!((same[2].as_str(), same[3].as_str(), same[5].as_str(), same.len()), ("0", "0", "0", 8));
    assert_eq!(same[6..], ["-", ""], "信任的转发中转没变宽（没设）：不用另外问");
    // 改：去掉 alice、bob 换到 US-E、Codex 的 carol 改名、加 dave（只写节点：编成 测试组-2）、锁定别名
    let mut e2 = e.clone();
    sub(&mut e2, &["accounts", "claude"]).remove("alice@example.com");
    sub(&mut e2, &["accounts", "claude", "bob@example.com"]).insert("node", "US-E");
    sub(&mut e2, &["accounts", "codex", "carol@example.com"]).insert("alias", "小李");
    sub(&mut e2, &["accounts", "claude"]).insert("dave@example.com", "US-A");
    sub(&mut e2, &[]).insert("alias_lock", true);
    let f2 = fx.d.join("conf2.json");
    put(&f2, &e2);
    let chk = lines(&admin, &["team-config", "check", tid, path_str(&f2)]);
    assert_eq!(chk[..4], [tid, "测试组", "5", "1"]);
    assert_eq!((chk[4].len(), chk[5].as_str()), (64, "0"), "文件的 SHA-256、没有新的节点定义");
    assert_eq!(chk[6..8], ["-", ""]);
    assert_eq!(
        chk[8..],
        [
            "加 1 个账号：Claude dave@example.com（US-A）",
            "去掉 1 个账号：Claude alice@example.com（各台设备上收起来，登录留着）",
            "换节点：Claude bob@example.com：US-B → US-E",
            "改别名：Codex carol@example.com：测试组-1 → 小李",
            "锁定别名（各台设备上照 Team 给的别名用，自己改不了）",
        ]
    );
    assert_eq!(fsx::read_json(tdir(&admin, tid).join("admin.json"))["config"]["accounts"], config()["accounts"], "check 不改");
    assert_eq!(code(&admin, &["team-config", "apply", tid, path_str(&f2), "--sha", "0"]), "file", "问过同意的不是这一份：不换");
    assert_eq!(
        lines(&admin, &["team-config", "apply", tid, path_str(&f2), "--sha", &chk[4]]),
        ["2"],
        "两台的配置都变了（别名锁定了）"
    );
    let a = fsx::read_json(tdir(&admin, tid).join("admin.json"));
    assert_eq!(
        list![&a["config"]["accounts"], &a["config"]["tool_aliases"], &a["config"]["alias_lock"], &a["config"]["install_node"]],
        list![
            obj! { "claude" => obj! { "bob@example.com" => "US-E", "dave@example.com" => "US-A" }, "codex" => obj! { "carol@example.com" => "US-A" } },
            obj! { "claude" => obj! { "bob@example.com" => "测试组-1", "dave@example.com" => "测试组-2" }, "codex" => obj! { "carol@example.com" => "小李" } },
            true,
            "US-D"
        ]
    );
    member.ok(&["team-sync"]);
    let (seq1, d1) = doc(&member, tid);
    assert!(seq1 > seq0, "拿着这些账号的设备同步到新的一版：{seq0} → {seq1}");
    assert_eq!(list![&d1["accounts"], &d1["alias_lock"]], list![obj! { "claude" => obj! { "bob@example.com" => "US-E" } }, true]);
    // 写错了的不导：说是哪一处
    let bad = fx.d.join("bad.json");
    for (v, why) in [
        (with(&e, "accounts", obj! { "claude" => obj! { "nope" => "US-A" } }), "Claude 的 nope：不是邮箱"),
        (with(&e, "version", 2), "这份配置是新版本的 cca 导出的（version 2）：先更新 cca（cca update）"),
        (with(&e, "install_node", "US-Z"), "install_node「US-Z」不是 nodes 里定义的节点"),
        (with(&e, "extra", 1), "不认得「extra」"),
        (obj! { "version" => 1, "claude" => obj! { "alice@example.com" => "US-A" } }, "不是 cca team export 导出的 Team 配置"),
    ] {
        put(&bad, &v);
        let r = admin.run_with(&["team-config", "check", tid, path_str(&bad)], "", &[]);
        assert_eq!((r.code, r.out.split('\t').next()), (1, Some("file")), "{}", r.out);
        assert_in(why, &r.out);
    }
    // --edit：Team 的目录里那一份（只给自己读写）；这中间在别处改过的提醒一句
    let ed = lines(&admin, &["team-config", "export", tid, "--edit"]);
    assert!(ed[0].ends_with(&format!("/teams/{tid}/edit.json")), "{ed:?}");
    assert_eq!(fsx::mode(&ed[0]), 0o600);
    assert_eq!(lines(&admin, &["team-config", "check", tid, &ed[0], "--base", &ed[5]])[2], "0", "没有在别处改过：不提醒");
    lines(&admin, &["team-map", tid, "claude", "erin@example.com", "US-A"]);
    let chk = lines(&admin, &["team-config", "check", tid, &ed[0], "--base", &ed[5]]);
    assert_eq!(
        chk[8..],
        [
            "去掉 1 个账号：Claude erin@example.com（各台设备上收起来，登录留着）",
            "! 打开以后「测试组」的配置在别处改过（App 里、别的命令）：照这份换掉的话，那些改动就没了",
        ]
    );
    // testconfig：全部的节点定义（经 dialer-proxy 连着的一起：一起校验）
    let tc = jl(&admin, &["team-config", "testconfig", path_str(&f2)]);
    assert_eq!(
        list![&tc["proxies"], &tc["proxy-groups"][0]["proxies"]],
        list![&config()["nodes"]["proxies"], list!["US-A", "US-B", "US-C", "US-D", "US-E"]]
    );
}

/// 新建 Team 时放进这台设备上的账号（team-candidates 列出来、team-new --accounts 选）：自己加的，和这台设备当管理员的 Team 里的
/// （那个 Team 带着节点的定义的，连同经 dialer-proxy 连着的一起带过来）；这台设备只是成员的 Team 给的、第三方 API 账号不能放。
/// 节点、别名照这台设备上的（默认样子的另编：以前的 team-N、Team 编的「名字-N」）；写错了什么都不建
fn new_with_accounts(t: &mut Ctx) {
    let fx = fx(t);
    // 「测试组」：这台设备是它的管理员
    let (admin, tid, _root, _srv, _url, _pin) = fx.new_team();
    let tid = tid.as_str();
    // alice → US-A、bob → US-B（经 US-C 连出去）、Codex 的 carol → US-A，节点的定义都在里面
    load(&admin, tid);
    // 别人当管理员的 Team：这台设备只是成员，它给的 frank 不能转手
    let other = fx.device("other");
    let (tid2, root2, _) = three(lines(&other, &["team-new", "别人的组", "--device", "desk"]));
    let tid2 = tid2.as_str();
    let (_srv2, url2, pin2) = fx.server(&root2, "srv2");
    lines(&other, &["team-attach", tid2, &url2, &pin2]);
    lines(&other, &["team-map", tid2, "claude", "frank@example.com", "US-E"]);
    assert_eq!(lines_in(&admin, &["team-join", "--name", "studio-2"], &invite(&other, tid2, "studio-2", &[]).0)[5], "ok");
    // 配置写好以后这台设备自己的那一份也同步下来：alice、bob、carol 是「测试组」给的
    admin.ok(&["team-sync"]);
    let claude_table = obj! {
        "me(me@example.com)" => "US-C", "team-7(old@example.com)" => "US-D", "alice(alice@example.com)" => "US-A",
        "测试组-1(bob@example.com)" => "US-B", "frank@example.com" => "US-E",
        "api(k@example.com)" => obj! { "node" => "US-B", "base_url" => "https://api.example.com" },
    };
    put(admin.cca.join("accounts.json"), &claude_table);
    put(admin.cca.join("codex.json"), &obj! { "carol@example.com" => "US-A", "cx(cx@example.com)" => "US-B" });
    assert_eq!(
        lines(&admin, &["team-candidates"]),
        [
            "claude\tClaude\tme@example.com\tme\t-\tUS-C",
            "claude\tClaude\told@example.com\tteam-7\t-\tUS-D",
            "claude\tClaude\talice@example.com\talice\t测试组\tUS-A",
            "claude\tClaude\tbob@example.com\t测试组-1\t测试组\tUS-B",
            "codex\tCodex\tcarol@example.com\t-\t测试组\tUS-A",
            "codex\tCodex\tcx@example.com\tcx\t-\tUS-B",
        ]
    );
    for (chosen, want) in [
        ("claude:frank@example.com", "accounts"),
        ("claude:k@example.com", "accounts"),
        ("claude:me@example.com,claude:nobody@example.com", "accounts"),
        ("me@example.com", "usage"),
    ] {
        assert_eq!(code(&admin, &["team-new", "新组", "--device", "studio", "--accounts", chosen]), want, "{chosen}");
    }
    assert_eq!(lines(&admin, &["team-list"]).len(), 2, "写错了什么都不建");
    // 照账号表里的先后放（不照 --accounts 写的先后）；别名带进去，默认样子的另编
    let chosen = "codex:carol@example.com,claude:bob@example.com,claude:me@example.com,claude:old@example.com";
    let (tid3, root3, n) = three(lines(&admin, &["team-new", "新组", "--device", "studio", "--accounts", chosen]));
    let tid3 = tid3.as_str();
    let cfg = fsx::read_json(tdir(&admin, tid3).join("admin.json"))["config"].clone();
    assert_eq!(
        list![n, &cfg["accounts"], &cfg["tool_aliases"]],
        list![
            "4",
            obj! {
                "claude" => obj! { "me@example.com" => "US-C", "old@example.com" => "US-D", "bob@example.com" => "US-B" },
                "codex" => obj! { "carol@example.com" => "US-A" },
            },
            obj! {
                "claude" => obj! { "me@example.com" => "me", "old@example.com" => "新组-1", "bob@example.com" => "新组-2" },
                "codex" => obj! { "carol@example.com" => "新组-1" },
            }
        ]
    );
    assert_eq!(
        list![&cfg["nodes"]["group"], proxy_names(&cfg)],
        list!["OpenAi", list!["US-B", "US-C", "US-A"]],
        "「测试组」带着的定义带过来（US-B 经 US-C 连出去）；自己加的账号的节点各台设备自己有，不带"
    );
    // 接上服务器、发布：这台设备拿到的配置里就是这几个账号和要用的节点
    let (_srv3, url3, pin3) = fx.server(&root3, "srv3");
    lines(&admin, &["team-attach", tid3, &url3, &pin3]);
    let d = doc(&admin, tid3).1;
    let mut carried = proxy_names(&d);
    carried.sort_unstable();
    assert_eq!(
        list![sorted_items(&d["accounts"]["claude"]), carried],
        list![list!["bob@example.com", "me@example.com", "old@example.com"], list!["US-A", "US-B", "US-C"]]
    );
    // all 是列出来的全部
    assert_eq!(lines(&admin, &["team-new", "全都放", "--device", "studio", "--accounts", "all"])[2], "6");
}

/// 服务器说的没有签名，管理员只拿它当线索：换了的公钥不照它的；它那边的配置丢了、旧了就重新发布；它乱说序号不信
fn server_lies(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, root, mut srv, _url, _pin) = fx.new_team();
    let (tid, root) = (tid.as_str(), root.as_str());
    load(&admin, tid);
    let (link, dev_b) = invite(&admin, tid, "bob-mini", &[]);
    let dev_b = dev_b.as_str();
    let bob = fx.device("bob");
    assert_eq!(lines_in(&bob, &["team-join", "--name", "bob-mini"], &link)[5], "ok");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["0", ""]);
    let admin_json = tdir(&admin, tid).join("admin.json");
    let aj = || fsx::read_json(&admin_json);
    let real = aj()["devices"][dev_b]["pub"].clone();
    // 服务器把 bob 的公钥换成别人的（停下来改它的状态文件），把给它的配置删了
    let port = srv.port;
    srv.stop();
    let evil = pki::keygen(fx.d.join("evil"));
    let state_file = fx.d.join("srv").join("state.json");
    let mut state = fsx::read_json(&state_file);
    let fp = {
        let found = state["machines"].obj().iter().find(|(_, m)| m["dev"] == Value::from(dev_b));
        found.expect("服务器的名单里有 bob 那一台").0.to_string()
    };
    let record = sub(&mut state, &["machines"]).remove(&fp).expect("刚找到的那一条");
    sub(&mut state, &["machines"]).insert(spec::fp_of(fx.d.join("evil.pub")), with(&record, "key", evil));
    put(&state_file, &state);
    let docs_file = fx.d.join("srv").join("team-docs.json");
    let mut docs = fsx::read_json(&docs_file);
    sub(&mut docs, &["docs"]).remove(dev_b).expect("服务器上存着给 bob 的配置");
    put(&docs_file, &docs);
    let (mut srv, _, _) = fx.server_on(root, port, "srv");
    let listing = devices(&admin, tid);
    assert_in("公钥", find(&listing["devices"], "label", "bob-mini")["problem"].str());
    assert_eq!(aj()["devices"][dev_b]["pub"], real, "记下的公钥不照服务器的改");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["1", ""], "服务器上 bob 的那一份没了：重新发布");
    let seq = aj()["devices"][dev_b]["seq"].i64();
    // 服务器乱说它存着的序号（假 curl 照 reply/team_put 回话）：比记着的小、大得离谱的都不信，记着的序号不动
    let team_put = admin.home.join("reply").join("team_put");
    fs::create_dir(admin.home.join("reply")).expect("建 reply 目录");
    // 让假 curl 这样回 team_put：哪台都没存上，stale、unknown 是给的这两项
    let answer = |stale: Value, unknown: Value| {
        let body = obj! { "v" => 1, "ok" => true, "stored" => list![], "stale" => stale, "unknown" => unknown };
        fsx::write(&team_put, format!("200\n{}", dumps(&body)));
    };
    // （每次换一种授权：内容和上次发布成的那一版不一样才会去存）
    let grants =
        ["none", "+claude:alice@example.com", "+claude:bob@example.com", "-claude:alice@example.com", "+codex:carol@example.com"];
    let lies = [Value::from(0), Value::from(seq - 1), Value::from((1i64 << 53) - 1), Value::from(i64::MAX), Value::from("x")];
    for (lie, grant) in lies.iter().zip(grants) {
        answer(obj! { dev_b => lie }, list![]);
        let r = admin.run(&["team-grant", tid, "bob-mini", grant]);
        assert_eq!((r.code, r.out.split('\t').next().unwrap_or_default()), (1, "publish"), "{lie}：{}", r.out);
        assert_in("对不上", &r.out);
        let recorded = aj()["devices"][dev_b].clone();
        assert_eq!(list![&recorded["seq"], &recorded["state"]], list![seq, "joined"], "{lie}");
    }
    // 回话里哪一种都没说这台设备的：不当成它不在了
    answer(obj! {}, list![]);
    let r = admin.run(&["team-publish", tid]);
    assert_eq!((r.code, aj()["devices"][dev_b]["state"].str()), (1, "joined"), "{}", r.out);
    assert_in("没说", &r.out);
    // 说得过去的（比记着的大一些：管理员的状态是从备份恢复的）：接着它的往上加，再存一次
    answer(obj! { dev_b => seq + 5 }, list![]);
    let r = admin.run(&["team-publish", tid]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(aj()["devices"][dev_b]["seq"], Value::from(seq + 5), "第二次它还说不够大：不再追，停在它说的那个数");
    fs::remove_file(&team_put).expect("删 reply/team_put");
    assert_eq!(lines(&admin, &["team-publish", tid]), ["1", ""]);
    assert_eq!(aj()["devices"][dev_b]["seq"], Value::from(seq + 6));
    // 发布时服务器说不认得（对过名单以后它才走的）：加入过的不再记，邀请了还没加入的记成 expired
    let (_link_c, dev_c) = invite(&admin, tid, "carol", &[]);
    let dev_c = dev_c.as_str();
    let mut a = aj();
    for dev in [dev_b, dev_c] {
        // 都重新发布一次
        sub(&mut a, &["devices", dev]).remove("sha");
    }
    put(&admin_json, &a);
    answer(obj! {}, list![dev_b, dev_c]);
    let r = admin.run(&["team-publish", tid]);
    let after = aj();
    assert_eq!(
        (r.code, after["devices"].obj().contains_key(dev_b), after["devices"][dev_c]["state"].str()),
        (1, false, "expired"),
        "{}",
        r.out
    );
    assert_in("服务器不认得", &r.out);
    fs::remove_file(&team_put).expect("删 reply/team_put");
    // 服务器的名单里没有管理员自己这台（gen 没变；正常不会这样）：不当它退出了，这边照旧记着
    let me = fsx::read_json(tdir(&admin, tid).join("team.json"))["dev"].str().to_string();
    let port = srv.port;
    srv.stop();
    let mut state = fsx::read_json(&state_file);
    let others = state["machines"].obj().iter().filter(|(_, m)| m["dev"] != Value::from(me.as_str()));
    let others: Vec<(String, Value)> = others.map(|(f, m)| (f.to_string(), m.clone())).collect();
    sub(&mut state, &[]).insert("machines", Obj::from_pairs(others));
    put(&state_file, &state);
    let (_srv, _, _) = fx.server_on(root, port, "srv");
    let rows = keyed(&devices(&admin, tid)["devices"], "dev");
    assert!(aj()["devices"].obj().contains_key(&me) && truthy(&rows[me.as_str()]["me"]), "{rows}");
}

/// 几位管理员（3.6.15 起）：建 Team 的设备授予、收回管理员身份；别的管理员邀请、授权、发布（拿它自己的密钥签，配置里带着 by），
/// 管理员的设置经服务器共用（密文，服务器看不到主密钥）；成员照根密钥签的管理员名单验它签的配置；收回以后服务器不认它、管理员的密钥换了，
/// 服务器拿旧的名单配被收回的管理员签的配置，见过新名单的设备不认
fn co_admins(t: &mut Ctx) {
    let fx = fx(t);
    let (admin, tid, root, srv, _url, _pin) = fx.new_team();
    let (tid, root) = (tid.as_str(), root.as_str());
    let rk = tdir(&admin, tid).join("root_ed25519");
    assert_eq!(load(&admin, tid), ["1"]);
    let (link_b, dev_b) = invite(&admin, tid, "bob-mini", &[]);
    let bob = fx.device("bob");
    lines_in(&bob, &["team-join"], &link_b);
    let (link_c, dev_c) = invite(&admin, tid, "carol-pc", &["--accounts", "claude:alice@example.com"]);
    let carol = fx.device("carol");
    lines_in(&carol, &["team-join"], &link_c);
    devices(&admin, tid); // 对一遍：记下两台的公钥（认证码核对过的）
    let (tb, tc) = (tdir(&bob, tid), tdir(&carol, tid));
    let bob_key = tb.join("id_ed25519");
    let bob_pub = pub_of(&bob_key);

    // 成员不是管理员；建 Team 的设备自己、不是管理员的不能收回
    assert_eq!(code(&bob, &["team-devices", tid]), "not_admin");
    assert_eq!(code(&bob, &["team-admin", "ls", tid]), "not_admin");
    assert_eq!(code(&admin, &["team-admin", "add", tid, "studio"]), "self");
    assert_eq!(code(&admin, &["team-admin", "remove", tid, "bob-mini"]), "not_admin_dev");
    assert_eq!(lines(&admin, &["team-admin", "plan", "add", tid, "bob-mini"]), [format!("{tid}\t{dev_b}\tbob-mini")]);
    let (st, _) = spec::post(&bob_key, srv.port, "team_members", &obj! {});
    assert_eq!(st, 403, "授予以前服务器不认 bob 的管理员请求");

    // 授予：名单（根密钥签的）和共用的设置（密文）放到服务器上，各台的配置带上名单的纪元重新发布
    assert_eq!(lines(&admin, &["team-admin", "add", tid, "bob-mini"]), ["3"]);
    assert_eq!(code(&admin, &["team-admin", "add", tid, "bob-mini"]), "already");
    let a = fsx::read_json(tdir(&admin, tid).join("admin.json"));
    let (master, akey) = (a["master"].str().to_string(), a["akey"].str().to_string());
    let state = fsx::read_json(fx.d.join("srv").join("state.json"));
    let listed = &state["team"]["admins"];
    assert!(spec::team_admins_verify(root, listed["list"].str(), listed["sig"].str()), "名单是根密钥签的");
    assert_eq!(listed["list"].str(), spec::team_admins_text(tid, 1, &[(&bob_pub, "bob-mini")]), "照文档的样子");
    let astate = &state["team"]["astate"];
    let shared = spec::astate_open(&spec::astate_keys(&akey), tid, astate["rev"].i64(), &astate["ct"]).expect("照文档解得开");
    assert_eq!(
        list![&shared["master"], shared.get("akey").is_some(), shared.get("deploy").is_some()],
        list![&master, false, false]
    );
    let on_server = all_text(&fx.d.join("srv"));
    assert!(!on_server.contains(&master) && !on_server.contains(&akey), "服务器上没有主密钥、管理员的密钥的明文");
    assert_eq!(doc(&carol, tid).1.get("admin"), None, "只有给管理员的那一份带着管理员的密钥");

    // bob 同步配置：拿到管理员的密钥，是别的管理员了
    assert_eq!(sync_fields(&bob)[2], "ok");
    assert_eq!(doc(&bob, tid).1["admin"]["key"].str(), akey);
    assert_eq!(fsx::mode(tb.join("admin_key")), 0o600);
    let row = &jl(&bob, &["team-list", "--json"])["teams"][0];
    assert_eq!(pick(row, &["role", "coadmin"]), list!["member", true]);
    assert_eq!(split_lines(&bob.ok(&["team-list"]))[0].split('\t').nth(2), Some("coadmin"));
    let (st, _) = spec::post(&bob_key, srv.port, "team_members", &obj! {});
    assert_eq!(st, 200, "服务器照名单认 bob 的管理员请求");
    let (st, j) =
        spec::post(&bob_key, srv.port, "team_admins", &obj! { "list" => listed["list"].str(), "sig" => listed["sig"].str() });
    assert_eq!((st, j["error"].str()), (403, "denied"), "换名单只认根密钥");
    let same = obj! { "list" => listed["list"].str(), "sig" => listed["sig"].str() };
    let (st, j) = spec::post(&rk, srv.port, "team_admins", &same);
    assert_eq!((st, j["error"].str(), j["epoch"].i64()), (409, "stale", 1), "纪元不比存着的大：不换");
    let next = spec::team_admins_text(tid, 2, &[(&bob_pub, "bob-mini")]);
    let by_bob = obj! { "list" => next.as_str(), "sig" => spec::team_admins_sign(&bob_key, &next) };
    let (st, _) = spec::post(&rk, srv.port, "team_admins", &by_bob);
    assert_eq!(st, 400, "名单不是根密钥签的：不收");
    let rev = astate["rev"].i64();
    let (st, j) = spec::post(&rk, srv.port, "team_astate_put", &obj! { "base" => rev - 1, "ct" => &astate["ct"] });
    assert_eq!((st, j["error"].str(), j["rev"].i64()), (409, "stale", rev), "照着改的不是存着的那一版：不存");

    // bob 当管理员：看名单、授权、发布（它签的）；只有建 Team 的设备能做的不行
    let ls = lines(&bob, &["team-admin", "ls", tid]);
    assert_eq!(ls.len(), 2);
    assert!(ls[0].ends_with("\tstudio\towner") && ls[1] == format!("{dev_b}\tbob-mini\tadmin"), "{ls:?}");
    let roles = mapped(&keyed(&devices(&bob, tid)["devices"], "label"), |r| r["role"].clone());
    assert_eq!(roles, obj! { "studio" => "owner", "bob-mini" => "admin", "carol-pc" => "member" });
    assert_eq!(code(&bob, &["team-admin", "add", tid, "carol-pc"]), "owner");
    assert_eq!(code(&bob, &["team-revoke", "apply", tid, "studio"]), "owner");
    assert_eq!(code(&admin, &["team-revoke", "apply", tid, "bob-mini"]), "admin_dev", "管理员的设备先收回身份才能移除");
    assert_eq!(code(&bob, &["team-undeploy", tid, "root@192.0.2.10"]), "owner");
    lines(&bob, &["team-grant", tid, "carol-pc", "all"]);
    let docs = fsx::read_json(fx.d.join("srv").join("team-docs.json"));
    assert_eq!(docs["docs"][dev_c.as_str()]["by"].str(), spec::pub_line(&bob_pub), "bob 签的配置带着它的公钥");
    assert!(docs["docs"][dev_c.as_str()].get("by").is_some());
    assert_eq!(sync_fields(&carol)[2], "ok", "carol 照名单验过 bob 签的配置");
    assert_eq!(sorted_items(&doc(&carol, tid).1["accounts"]["claude"]), ["alice@example.com", "bob@example.com"]);
    assert_eq!(fsx::read_json(tc.join("admins.json"))["epoch"].i64(), 1, "记下了验过的名单");
    // 建 Team 的设备看得到 bob 改的（共用的设置）
    let grants = mapped(&keyed(&devices(&admin, tid)["devices"], "label"), |r| r["grants"].clone());
    assert_eq!(grants["carol-pc"].str(), "all");

    // 不在名单里的密钥签的、带着 by 的配置：不认
    let port = srv.port;
    let evil = fx.d.join("evil");
    let evil_pub = pki::keygen(&evil);
    let (_, _, keys_c) = spec::invite_parts(fsx::read_text(tc.join("secret")).trim()).expect("carol 的那串随机数");
    let forged = |signer: &Path, by: &str, seq: i64, name: &str| {
        let content = with(&doc(&carol, tid).1, "name", name);
        let ct = spec::team_doc_seal(&keys_c, tid, &dev_c, seq, &content);
        let env = obj! { "seq" => seq, "ct" => &ct, "sig" => spec::team_doc_sign(signer, tid, &dev_c, seq, &ct), "by" => by };
        let (st, j) = spec::post(&rk, port, "team_put", &obj! { "docs" => obj! { dev_c.as_str() => env } });
        assert_eq!(st, 200, "{j}");
    };
    let seq_c = doc(&carol, tid).0;
    forged(&evil, &evil_pub, seq_c + 1, "伪造的");
    let f = sync_fields(&carol);
    assert_eq!(f[2], "error");
    assert_in("管理员", &f[4]);

    // 收回：名单换成第 2 版，管理员的密钥换一把，各台重新发布；bob 同步以后不是管理员了，服务器也不认它
    let old_list = listed.clone();
    assert_eq!(lines(&admin, &["team-admin", "remove", tid, "bob-mini"]).len(), 1);
    let a2 = fsx::read_json(tdir(&admin, tid).join("admin.json"));
    assert_ne!(a2["akey"].str(), akey, "换了一把管理员的密钥");
    assert_eq!(a2["admins_epoch"].i64(), 2);
    assert_eq!(code(&bob, &["team-devices", tid]), "not_admin");
    assert!(!tb.join("admin_key").exists() && !tb.join("admin.json").exists(), "管理员的密钥、取回来的设置删掉了");
    assert_eq!(jl(&bob, &["team-list", "--json"])["teams"][0]["coadmin"], Value::Bool(false));
    let (st, _) = spec::post(&bob_key, srv.port, "team_members", &obj! {});
    assert_eq!(st, 403, "收回以后服务器不认 bob 的管理员请求");
    let state = fsx::read_json(fx.d.join("srv").join("state.json"));
    let shared = &state["team"]["astate"];
    assert!(
        spec::astate_open(&spec::astate_keys(&akey), tid, shared["rev"].i64(), &shared["ct"]).is_none(),
        "旧的密钥解不开新存的设置"
    );
    assert_eq!(sync_fields(&carol)[2], "ok");
    assert_eq!(fsx::read_json(tc.join("admins.json"))["floor"].i64(), 2, "根密钥签的配置说名单到第 2 版了");

    // 服务器作恶：换回旧的名单，配一份 bob 签的配置。carol 见过第 2 版：不认
    drop(srv);
    let mut s = fsx::read_json(fx.d.join("srv").join("state.json"));
    sub(&mut s, &["team"]).insert("admins", old_list);
    put(fx.d.join("srv").join("state.json"), &s);
    let (srv, _, _) = fx.server_on(root, port, "srv");
    let seq_c = doc(&carol, tid).0;
    forged(&bob_key, &bob_pub, seq_c + 1, "被收回的管理员签的");
    let f = sync_fields(&carol);
    assert_eq!((f[2].as_str(), f[3].parse::<i64>().unwrap_or(0)), ("error", seq_c), "{f:?}");
    assert_ne!(doc(&carol, tid).1["name"].str(), "被收回的管理员签的");
    drop(srv);
}

/// 公钥文件（私钥旁边的 .pub）的前两段
fn pub_of(key: &Path) -> String {
    let mut p = key.as_os_str().to_os_string();
    p.push(".pub");
    spec::pub_line(&fsx::read_text(PathBuf::from(p)))
}
