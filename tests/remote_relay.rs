//! 远程会话经服务器中转（docs/dev/remote.md、server/README.md「远程会话的中转」）：配对、会话的列表、端到端加密的通道。
//!
//! 真的服务器进程（一个 Team 的同步服务器，127.0.0.1、随机端口，开着中转；用哪一份照 kit::server）、真的 ssh-keygen；每台设备一个
//! 临时 HOME，在 Team「测试组」里、有自己在 Team 里的密钥（kit::fleet 的 Device、Team），跑的是 cca 的辅助程序，会话在真的伪终端上
//! （kit::term 的 Term 和假客户端 cca-test fake-client）。设备和数据端口之间隔着一个转发器（kit::relay 的 Sniffer），经过的字节都
//! 记下来：服务器看得到的就是这些。另一头有时由 kit::spec（照文档独立实现的那一份）扮演：两份实现要能互相配对、握手、收发。不联网。
//!
//! 夹具（Base、转发器、服务器、照文档说话的另一头）在 kit::relay 里，别的几份测试也用。
//!
//! 服务器的限额和记账（一台设备同时开着 4 个配对、最多和 16 台配对、手里 16 张没用掉的票据、8 个会话，
//! 一个会话 4 处接入，没用过的配对 180 天删掉）时间得由用例给，从外面测不了，不在这里：在服务器自己的单元测试里
//! （cli/src/server/remote/tests.rs 的 limits、idle_pairings_are_dropped_but_used_ones_stay）。

use kit::fleet::{self, Device, WAIT};
use kit::fsx::{self, wait_for};
use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::py::splitlines;
use kit::relay::{self, connect, ctl, dial, frame, read_all, read_ctl, read_exact, read_line, recv, sendall};
use kit::relay::{cols, has, matches, named_like, names, only, parsed, rows_of, secs, walk};
use kit::relay::{Base, Popen, Reader, RelayOpts, RelayServer, SpecPeer};
use kit::relay::{DETACH, EXIT, F_ANNOUNCE, F_BYE, F_JOIN, F_PING, F_PONG, HELLO, INPUT, OUTPUT, RESIZE};
use kit::term::{self, Term};
use kit::{case, crypto, list, obj, spec};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "remote_relay",
    src: include_str!("remote_relay.rs"),
    default: true,
    tests: &[
        case!(pairing),
        case!(sessions_tickets_and_the_pipe),
        case!(one_bad_connection_does_not_stop_the_relay),
        case!(revoked_device_and_dropped_pairing),
        case!(pairings_survive_a_restart),
        case!(relay_is_off_unless_asked),
        case!(pair_list_attach),
        case!(cca_hooks_reach_the_other_machine),
        case!(codex_notes_reach_the_other_machine),
        case!(what_the_other_side_may_do),
        case!(the_other_implementation_and_what_the_host_enforces),
        case!(a_phone_sets_the_size_only_when_it_types),
        case!(the_link_process_is_killed),
        case!(the_server_restarts_and_the_session_ends_meanwhile),
        case!(local_sessions_never_touch_the_network),
        case!(pairing_errors),
        case!(terminal),
        case!(in_the_app),
    ],
};

// ---- 几份用例共用的小东西 ----

/// 服务器回的出错：(状态码, {"error": 代码})
fn err(status: u16, code: &str) -> (u16, Value) {
    (status, obj! { "error" => code })
}

/// `json.dumps(v).encode()`
fn dumps(v: &Value) -> Vec<u8> {
    json::dumps(v, &Style::default()).into_bytes()
}

/// `base64.b64decode(v)`（票据的密钥）
fn b64(v: &Value) -> Vec<u8> {
    crypto::b64_decode(v.str()).unwrap_or_else(|e| panic!("不是 base64（{e}）：{v}"))
}

/// dev 经配对 pid 看到的：relay_list 回的整个 JSON
fn listed(fx: &Base, dev: &Device, pid: &str) -> Value {
    fx.good(dev, "relay_list", &obj! { "pids" => list![pid] })
}

/// relay_list 没有什么可给时回的
fn nothing_listed() -> Value {
    obj! { "v" => 1, "ok" => true, "peers" => obj! {}, "sessions" => obj! {} }
}

/// 没有一行（`== []`）
fn no_rows() -> Vec<String> {
    Vec::new()
}

/// 没有一项（`== []`）
fn no_values() -> Vec<Value> {
    Vec::new()
}

/// 目录里（不往下走）各个 .json 文件的内容接在一起（`''.join(p.read_text() for p in d.glob('*.json'))`）
fn json_files(dir: &Path) -> String {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files.iter().map(fsx::read_text).collect()
}

/// 等终端的输出里出现 needle（照字面找），最多等 limit：等到了没有。到时限、终端那头关了还没有是 false
/// （`Term::expect` 没等到是 panic；这里是“试一次，不行过会儿再试”）
fn shows(t: &mut Term, needle: &str, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    loop {
        if has(t.out(), needle) {
            return true;
        }
        if Instant::now() > end || !t.pump(Duration::from_millis(50)) {
            return has(t.out(), needle);
        }
    }
}

/// 进程 pid 的父进程；问不到是 0。Linux 上读 /proc/<pid>/stat（busybox 的 ps 不认 `-o ppid= -p`）：
/// "pid (comm) state ppid …"，comm 里可以有空格和括号，从最后一个 ) 往后数
#[cfg(target_os = "linux")]
fn parent_of(pid: i32) -> i32 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return 0 };
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|ppid| ppid.parse().ok()).unwrap_or(0)
}

/// 进程 pid 的父进程（`ps -o ppid= -p pid`）；问不到是 0
#[cfg(not(target_os = "linux"))]
fn parent_of(pid: i32) -> i32 {
    let mut ps = kit::proc::Cmd::new("/bin/ps");
    ps.args(["-o", "ppid=", "-p", &pid.to_string()]).timeout(secs(30));
    ps.output().map(|out| out.out().trim().parse().unwrap_or(0)).unwrap_or(0)
}

/// `os.kill(pid, 9)`
fn kill9(pid: i32) {
    // SAFETY: kill 只是发信号；pid 是这条用例刚问到的那个转接进程
    let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
    assert!(rc == 0, "kill -9 {pid}：{}", std::io::Error::last_os_error());
}

/// 路径照字面放进 pgrep 的正则里：只转义 ERE 里有别的意思的那几个字符（regex::escape 连 - # & ~ 也转义，
/// 那是它自己的写法，别的 pgrep 不一定认）
fn ere(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if r".[]()*+?{}|^$\".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 事件里 ev 是 kind 的那些，各取 key 这一项（`[e[key] for e in es if e['ev'] == kind]`）
fn field_of(es: &[Value], kind: &str, key: &str) -> Vec<String> {
    es.iter().filter(|e| e["ev"].str() == kind).map(|e| e[key].str().to_string()).collect()
}

// ---- 服务器这一头：两边都照文档自己说话（不经 cca） ----

/// 这一组用例的准备：三台设备 box、laptop、other，都登记在服务器上。返回 (夹具, box, laptop, other)
fn server_fx(t: &mut Ctx) -> (Base, Device, Device, Device) {
    let fx = Base::new(t);
    let (a, b, c) = (fx.device("box"), fx.device("laptop"), fx.device("other"));
    fx.machines(&[&a, &b, &c]);
    (fx, a, b, c)
}

/// 配对到两边都放了密文为止。返回 (这次配对的编号, 该报的配对编号)
fn pair_until_commit(fx: &Base, a: &Device, b: &Device) -> (String, String) {
    let (kc, keys, rid) = spec::pair_keys(&crypto::random(10));
    let (na, nb) = (crypto::random(32), crypto::random(32));
    fx.good(a, "pair_open", &obj! { "rid" => rid.as_str() });
    fx.good(a, "pair_put", &obj! { "rid" => rid.as_str(), "m" => spec::pair_msg(&keys, &rid, "a", a.fp(), &na, &a.name) });
    fx.good(b, "pair_put", &obj! { "rid" => rid.as_str(), "m" => spec::pair_msg(&keys, &rid, "b", b.fp(), &nb, &b.name) });
    let pid = spec::pair_secret(&kc, &na, &nb, a.fp(), b.fp()).1;
    (rid, pid)
}

/// dev 的会话 sid 连上控制连接（给 pids 这几个配对看）。返回 (连接, 票据的密钥)
fn host_conn(fx: &Base, dev: &Device, sid: &str, pids: &[&str]) -> (TcpStream, Vec<u8>) {
    let t = fx.good(dev, "relay_host", &obj! { "sid" => sid, "pids" => pids.to_vec() });
    let key = b64(&t["tkey"]);
    let (mut s, ok) = dial(fx.srv.data_port(), "host", t["tid"].str(), &key);
    assert_eq!(read_line(&mut s), ok);
    (s, key)
}

fn pairing(t: &mut Ctx) {
    let (fx, a, b, c) = server_fx(t);
    let (fx, a, b, c) = (&fx, &a, &b, &c);
    let (kc, keys, rid) = spec::pair_keys(&crypto::random(10));
    let rid = rid.as_str();
    let (na, nb) = (crypto::random(32), crypto::random(32));
    let (ma, mb) = (spec::pair_msg(&keys, rid, "a", a.fp(), &na, "box"), spec::pair_msg(&keys, rid, "b", b.fp(), &nb, "laptop"));
    // 没开的编号：不认；编号的样子不对：拒
    assert_eq!(fx.call(b, "pair_put", &obj! { "rid" => rid, "m" => &mb }), err(404, "no_pairing"));
    assert_eq!(fx.call(a, "pair_open", &obj! { "rid" => "xyz" }).0, 400);
    fx.good(a, "pair_open", &obj! { "rid" => rid });
    assert_eq!(fx.call(c, "pair_open", &obj! { "rid" => rid }), err(409, "taken"), "同一个编号不能再开一次");
    assert_eq!(
        fx.good(a, "pair_get", &obj! { "rid" => rid }),
        obj! { "v" => 1, "ok" => true, "m" => Value::Null, "fp" => Value::Null, "name" => Value::Null, "done" => false }
    );
    fx.good(a, "pair_put", &obj! { "rid" => rid, "m" => &ma });
    assert_eq!(fx.call(a, "pair_put", &obj! { "rid" => rid, "m" => &ma }).0, 400, "一边只能放一次");
    assert_eq!(fx.call(a, "pair_put", &obj! { "rid" => rid, "m" => obj! { "n" => "x" } }).0, 400);
    // 还没人输入配对码：不知道配对码的设备拿不到显示方放的那一份（它不是这次配对的哪一边）
    assert_eq!(fx.call(c, "pair_get", &obj! { "rid" => rid }), err(409, "taken"));
    assert_eq!(fx.call(a, "pair_commit", &obj! { "rid" => rid, "pid" => "0".repeat(32) }).0, 400, "对方还没来");
    fx.good(b, "pair_put", &obj! { "rid" => rid, "m" => &mb });
    assert_eq!(fx.call(c, "pair_put", &obj! { "rid" => rid, "m" => &mb }), err(409, "taken"), "第三台进不来");
    assert_eq!(fx.good(b, "pair_get", &obj! { "rid" => rid })["m"], ma);
    // 等对方：b 还没报，a 等 1 秒等不到；b 报了以后 a 等的那一次马上回来
    let (p, pid) = spec::pair_secret(&kc, &na, &nb, a.fp(), b.fp());
    let pid = pid.as_str();
    let commit = obj! { "rid" => rid, "pid" => pid };
    assert_eq!(fx.good(a, "pair_commit", &commit)["done"], Value::Bool(false));
    let t0 = Instant::now();
    assert_eq!(fx.good(a, "pair_get", &obj! { "rid" => rid, "wait" => 1, "until" => "done" })["done"], Value::Bool(false));
    assert!(t0.elapsed() > Duration::from_millis(900), "{:?}", t0.elapsed());
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| fx.good(a, "pair_get", &obj! { "rid" => rid, "wait" => 10, "until" => "done" }));
        std::thread::sleep(Duration::from_millis(300)); // 让它先等上
        let t0 = Instant::now();
        assert_eq!(fx.good(b, "pair_commit", &commit)["done"], Value::Bool(true));
        let end = Instant::now() + Duration::from_secs(8);
        while !waiter.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(waiter.is_finished(), "a 等的那一次过了 8 秒还没回来");
        let got = waiter.join().expect("a 等的那一次没成");
        assert_eq!(got.get("done"), Some(&Value::Bool(true)));
        assert!(t0.elapsed() < Duration::from_secs(5), "不用等满 10 秒");
    });
    let e = fx.edges();
    assert_eq!((names(&e), e[pid]["a"].str(), e[pid]["b"].str()), (vec![pid.to_string()], a.fp(), b.fp()));
    let files = fsx::read_text(fx.d.join("remote.json")) + &fsx::read_text(fx.d.join("state.json"));
    assert!(!files.contains(&crypto::hex(&p)), "密钥不该在服务器的文件里");

    // 两边报的编号不一样：不记，这次配对作废
    let (p2, pid2) = pair_until_commit(fx, a, c);
    fx.good(a, "pair_commit", &obj! { "rid" => p2.as_str(), "pid" => pid2.as_str() });
    assert_eq!(fx.call(c, "pair_commit", &obj! { "rid" => p2.as_str(), "pid" => "f".repeat(32) }), err(409, "mismatch"));
    assert_eq!(fx.call(a, "pair_get", &obj! { "rid" => p2.as_str() }), err(404, "no_pairing"));
    assert_eq!(names(&fx.edges()), [pid]);

    // 显示配对码的那一边中途不做了：这次配对作废，对方马上知道；不是这次配对的设备关不掉它
    let (rid4, _) = pair_until_commit(fx, a, c);
    assert_eq!(fx.call(b, "pair_close", &obj! { "rid" => rid4.as_str() }), err(409, "taken"));
    fx.good(a, "pair_close", &obj! { "rid" => rid4.as_str() });
    assert_eq!(fx.call(c, "pair_get", &obj! { "rid" => rid4.as_str(), "wait" => 5, "until" => "done" }), err(404, "no_pairing"));
    assert_eq!(names(&fx.edges()), [pid], "已经记下的配对不受影响");

    // 同两台设备再配一次：换掉以前的
    let (_, pid3) = fx.spec_pair(b, a);
    assert_eq!(names(&fx.edges()), [pid3.as_str()]);
    // 不是这个配对的设备取消不了；是的那两台谁取消都行
    fx.good(c, "pair_drop", &obj! { "pid" => pid3.as_str() });
    assert_eq!(names(&fx.edges()), [pid3.as_str()]);
    fx.good(a, "pair_drop", &obj! { "pid" => pid3.as_str() });
    assert_eq!(fx.edges(), obj! {});
}

fn sessions_tickets_and_the_pipe(t: &mut Ctx) {
    let (fx, a, b, c) = server_fx(t);
    let (fx, a, b, c) = (&fx, &a, &b, &c);
    let rport = fx.srv.data_port();
    let (p, pid) = fx.spec_pair(a, b);
    let pid = pid.as_str();
    let sid = crypto::token_hex(16);
    let sid = sid.as_str();
    let attach = obj! { "sid" => sid, "pid" => pid };
    // 不是这个配对的设备开不了这个配对的会话；没有会话时接不了
    assert_eq!(fx.call(c, "relay_host", &obj! { "sid" => sid, "pids" => list![pid] }), err(404, "no_pairing"));
    assert_eq!(fx.call(b, "relay_attach", &attach), err(404, "no_session"));
    let t = fx.good(a, "relay_host", &obj! { "sid" => sid, "pids" => list![pid] });
    assert_eq!((t["port"].i64(), t["ttl"].i64()), (i64::from(fx.sniff.port), 30), "告诉客户端的是 --relay-port 给的端口");
    let key = b64(&t["tkey"]);
    // 开场白：认证码不对、乱说的都进不来；票据只能用一次
    let wrong_mac = spec::relay_line("host", t["tid"].str(), &[b'x'; 32]).0;
    for line in [wrong_mac.as_slice(), b"GET / HTTP/1.0\r\n\r\n", b"CCA-RELAY/1 host zz 00 00\n"] {
        let mut s = connect(rport, Duration::from_secs(10));
        sendall(&mut s, line);
        let said = read_line(&mut s);
        assert!(
            [&b"CCA-RELAY/1 ERR ticket\n"[..], b"CCA-RELAY/1 ERR bad\n"].contains(&said.as_slice()),
            "{:?}",
            String::from_utf8_lossy(&said)
        );
        assert_eq!(recv(&mut s, 1), b"");
    }
    let (mut host, ok) = dial(rport, "host", t["tid"].str(), &key);
    assert_eq!(read_line(&mut host), ok, "服务器也证明它认得这张票据");
    let (mut again, _) = dial(rport, "host", t["tid"].str(), &key);
    assert_eq!(read_line(&mut again), b"CCA-RELAY/1 ERR ticket\n");
    drop(again);

    let said = obj! {
        "v" => 1, "machine" => "box", "tool" => "claude", "name" => "工作", "cwd" => "~/项目", "since" => 1, "access" => "control",
    };
    let blob = spec::dir_seal(&p, pid, sid, &said);
    sendall(&mut host, &[ctl(F_ANNOUNCE, &dumps(&obj! { pid => &blob })), ctl(F_PING, b"hi")].concat());
    assert_eq!(read_ctl(&mut host), (Some(F_PONG), b"hi".to_vec()));
    let j = fx.good(b, "relay_list", &obj! { "pids" => list![pid, "ab".repeat(16)] });
    assert_eq!(j["peers"], obj! { pid => obj! { "fp" => a.fp(), "name" => "box" } });
    let row = only(&j["sessions"][pid]);
    assert_eq!((row["sid"].str(), &row["blob"], row["viewers"].i64(), row["online"].bool()), (sid, &blob, 0, true));
    assert_eq!(spec::dir_open(&p, pid, sid, &row["blob"]).expect("说明解得开")["cwd"].str(), "~/项目");
    assert!(!json::dumps(&j, &Style::default().unicode()).contains("项目"), "说明是密文");
    assert_eq!(listed(fx, a, pid)["sessions"], obj! { pid => list![] }, "自己的会话不列给自己");
    assert_eq!(listed(fx, c, pid), nothing_listed());
    assert_eq!(fx.call(c, "relay_attach", &attach), err(404, "no_session"));
    assert_eq!(fx.call(a, "relay_attach", &attach), err(404, "no_session"), "宿主自己不经中转接自己");

    // 接入：服务器在控制连接上说 JOIN，宿主拿令牌另连一条，两条对接
    let tv = fx.good(b, "relay_attach", &attach);
    let (mut view, vok) = dial(rport, "view", tv["tid"].str(), &b64(&tv["tkey"]));
    let (kind, payload) = read_ctl(&mut host);
    assert_eq!(kind, Some(F_JOIN));
    let j = json::parse_bytes(&payload).unwrap_or_else(|e| panic!("JOIN 带的不是 JSON：{e}"));
    assert_eq!(j["pid"].str(), pid);
    let (mut bad, _) = dial(rport, "join", j["token"].str(), &[b'y'; 32]);
    assert_eq!(read_line(&mut bad), b"CCA-RELAY/1 ERR ticket\n", "令牌要配控制连接那张票据的密钥");
    drop(bad);
    let (mut join, jok) = dial(rport, "join", j["token"].str(), &key);
    assert_eq!((read_line(&mut join), read_line(&mut view)), (jok, vok));
    assert_eq!(listed(fx, b, pid)["sessions"][pid][0]["viewers"].i64(), 1);
    sendall(&mut view, b"from the viewer");
    assert_eq!(read_exact(&mut join, 15), b"from the viewer");
    // 大块的、一头读得慢：一个字节不丢；宿主说完就关，最后那几个字节也到
    let big = crypto::random(3 * 1024 * 1024);
    let all = [big.as_slice(), b"<the end>"].concat();
    let got = std::thread::scope(|scope| {
        let all = &all;
        let sender = scope.spawn(move || {
            let mut join = join; // 这条线带走它：说完就关
            sendall(&mut join, all);
        });
        std::thread::sleep(Duration::from_millis(500)); // 读的这一头慢半拍
        let got = read_all(&mut view);
        let _ = sender.join();
        got
    });
    assert_eq!((got.len(), &got[..64], &got[got.len() - 9..]), (big.len() + 9, &big[..64], &b"<the end>"[..]));
    assert!(got[..got.len() - 9] == big[..], "中间的字节不对");
    drop(view);
    wait_for("这一处接入算结束", WAIT, || listed(fx, b, pid)["sessions"][pid][0]["viewers"].i64() == 0);

    // 宿主那头不来接：接入方等到服务器说连不上
    let tv = fx.good(b, "relay_attach", &attach);
    let (mut view, _) = dial(rport, "view", tv["tid"].str(), &b64(&tv["tkey"]));
    assert_eq!(read_ctl(&mut host).0, Some(F_JOIN));
    let t0 = Instant::now();
    assert_eq!(read_line(&mut view), b"CCA-RELAY/1 ERR nohost\n");
    assert!(t0.elapsed() < Duration::from_secs(19), "{:?}", t0.elapsed());
    drop(view);

    // 会话结束（BYE）：不再列出来、接不了
    sendall(&mut host, &ctl(F_BYE, b""));
    assert_eq!(recv(&mut host, 1), b"");
    assert_eq!(listed(fx, b, pid)["sessions"], obj! { pid => list![] });
    assert_eq!(fx.call(b, "relay_attach", &attach), err(404, "no_session"));
}

fn one_bad_connection_does_not_stop_the_relay(t: &mut Ctx) {
    let (mut fx, a, b, _c) = server_fx(t);
    let (a, b) = (&a, &b);
    let rport = fx.srv.data_port();
    let (p, pid) = fx.spec_pair(a, b);
    let pid = pid.as_str();
    let sid = crypto::token_hex(16);
    let sid = sid.as_str();
    let blob = spec::dir_seal(&p, pid, sid, &obj! { "v" => 1 });
    // 嵌套几万层的 JSON（解析时是 RecursionError）、不是 JSON、密文的样子不对：只断开这一条
    for bad in [vec![b'['; 30000], vec![0xff, 0xfe], dumps(&obj! { pid => obj! { "n" => "x" } }), b"[]".to_vec()] {
        let (mut host, _key) = host_conn(&fx, a, sid, &[pid]);
        sendall(&mut host, &ctl(F_ANNOUNCE, &bad));
        assert_eq!(recv(&mut host, 1), b"", "{:?}", String::from_utf8_lossy(&bad[..bad.len().min(20)]));
    }
    // 只发 PING、不收 PONG：服务器不替它攒着，攒到上限就断开。要先把内核的缓冲区填满，服务器那头才攒得起来——回环上
    // macOS 只有几百 KB，Linux 能到十几兆（一次 7.5 MB 的 PONG 它全收得下，服务器那头一点没攒）：一直发到它断开为止
    let (mut host, _key) = host_conn(&fx, a, sid, &[pid]);
    for limit in [host.set_read_timeout(Some(Duration::from_secs(20))), host.set_write_timeout(Some(Duration::from_secs(20)))] {
        limit.unwrap_or_else(|e| panic!("设不了连接的时限：{e}"));
    }
    let pings = ctl(F_PING, b"").repeat(1_500_000);
    for _ in 0..40 {
        if host.write_all(&pings).is_err() {
            break;
        }
    }
    let mut buf = vec![0u8; 65536];
    let closed = loop {
        match host.read(&mut buf) {
            Ok(0) => break true,
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::ConnectionReset => break true,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break false,
        }
    };
    assert!(closed, "只发不收的控制连接没有被断开");
    // 中转还在：会话照样开得了、列得出、接得上
    let (mut host, _key) = host_conn(&fx, a, sid, &[pid]);
    sendall(&mut host, &[ctl(F_ANNOUNCE, &dumps(&obj! { pid => &blob })), ctl(F_PING, b"ok")].concat());
    assert_eq!(read_ctl(&mut host), (Some(F_PONG), b"ok".to_vec()));
    assert_eq!(listed(&fx, b, pid)["sessions"][pid][0]["online"], Value::Bool(true));
    assert_eq!(fx.srv.proc.poll(), None);

    // 控制连接断了（宿主那头网络断了一下）：会话留着、标成连不上，这时接入马上被告知；重连以后原来的说明还在
    drop(host);
    wait_for("会话标成连不上", WAIT, || listed(&fx, b, pid)["sessions"][pid][0]["online"] == Value::Bool(false));
    let tv = fx.good(b, "relay_attach", &obj! { "sid" => sid, "pid" => pid });
    let (mut view, _) = dial(rport, "view", tv["tid"].str(), &b64(&tv["tkey"]));
    assert_eq!(read_line(&mut view), b"CCA-RELAY/1 ERR nohost\n");
    let (_host, _key) = host_conn(&fx, a, sid, &[pid]);
    let j = listed(&fx, b, pid);
    let row = only(&j["sessions"][pid]);
    assert_eq!((row["online"].bool(), &row["blob"]), (true, &blob));
}

fn revoked_device_and_dropped_pairing(t: &mut Ctx) {
    let (fx, a, b, c) = server_fx(t);
    let (fx, a, b, c) = (&fx, &a, &b, &c);
    let rport = fx.srv.data_port();
    let (p, pid) = fx.spec_pair(a, b);
    let (p2, pid2) = fx.spec_pair(a, c);
    let (pid, pid2) = (pid.as_str(), pid2.as_str());
    let sid = crypto::token_hex(16);
    let sid = sid.as_str();
    let (mut host, key) = host_conn(fx, a, sid, &[pid, pid2]);
    let one = obj! { "v" => 1 };
    let blobs = obj! { pid => spec::dir_seal(&p, pid, sid, &one), pid2 => spec::dir_seal(&p2, pid2, sid, &one) };
    sendall(&mut host, &ctl(F_ANNOUNCE, &dumps(&blobs)));
    wait_for("两个配对都看得到", WAIT, || {
        [(b, pid), (c, pid2)].iter().all(|(d, q)| !listed(fx, d, q)["sessions"][*q].list().is_empty())
    });
    // 两台都接着
    let mut attach = |dev: &Device, q: &str| {
        let tv = fx.good(dev, "relay_attach", &obj! { "sid" => sid, "pid" => q });
        let (mut view, vok) = dial(rport, "view", tv["tid"].str(), &b64(&tv["tkey"]));
        let said = json::parse_bytes(&read_ctl(&mut host).1).unwrap_or_else(|e| panic!("JOIN 带的不是 JSON：{e}"));
        let (mut join, jok) = dial(rport, "join", said["token"].str(), &key);
        assert_eq!((read_line(&mut join), read_line(&mut view)), (jok, vok));
        (view, join)
    };
    let (mut view_b, mut join_b) = attach(b, pid);
    let (mut view_c, mut join_c) = attach(c, pid2);
    // 取消一个配对：那台设备马上看不到、接不了，正接着的那一处两头都被断开；另一台照旧
    fx.good(c, "pair_drop", &obj! { "pid" => pid2 });
    assert_eq!([recv(&mut view_c, 1), recv(&mut join_c, 1)], [b"".to_vec(), b"".to_vec()]);
    sendall(&mut view_b, b"still piped");
    assert_eq!(read_exact(&mut join_b, 11), b"still piped");
    assert_eq!(listed(fx, c, pid2), nothing_listed());
    assert_eq!(fx.call(c, "relay_attach", &obj! { "sid" => sid, "pid" => pid2 }), err(404, "no_session"));
    assert_eq!(listed(fx, b, pid)["sessions"][pid].list().len(), 1);
    // 管理员把一台设备移出了 Team：它的配对、会话都没了（重启服务器以后也一样：配对记在文件里）
    fx.machines(&[b, c]);
    assert_eq!(fx.edges(), obj! {});
    assert_eq!(fx.call(a, "relay_list", &obj! { "pids" => list![pid] }).0, 403);
    assert_eq!(listed(fx, b, pid), nothing_listed());
    assert_eq!([recv(&mut view_b, 1), recv(&mut join_b, 1)], [b"".to_vec(), b"".to_vec()], "被移除的设备正接着的也断开");
    assert_eq!(recv(&mut host, 1), b"", "它的控制连接也断开");
}

fn pairings_survive_a_restart(t: &mut Ctx) {
    let (mut fx, a, b, _c) = server_fx(t);
    let (_p, pid) = fx.spec_pair(&a, &b);
    fx.srv.stop();
    fx.srv = RelayServer::start(&fx.d, &RelayOpts::new().sniff(&fx.sniff));
    assert_eq!(listed(&fx, &b, &pid)["peers"], obj! { pid.as_str() => obj! { "fp" => a.fp(), "name" => "box" } });
    assert_eq!(fsx::mode(fx.d.join("remote.json")) & 0o777, 0o600);
}

// ---- 没开中转的服务器（和旧版一样）：那几个接口不认 ----

fn relay_is_off_unless_asked(t: &mut Ctx) {
    let fx = Base::with(t, false, |_| Vec::new());
    let a = fx.device("box");
    fx.machines(&[&a]);
    for op in ["pair_open", "relay_list", "relay_host"] {
        assert_eq!(fx.call(&a, op, &obj! { "rid" => "0".repeat(32), "pids" => list![] }), err(404, "not_found"));
    }
    assert!(!fx.d.join("remote.json").exists());
    let r = a.run(&["remote-pair-show"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""), "服务器不支持时不给配对码");
    assert!(r.err.contains("「测试组」的服务器还没开远程会话的中转（管理员要重新部署、把中转打开：cca team deploy）"), "{}", r.err);
    // 不在哪个 Team 里的机器：没有中转的服务器，什么都不发、说不行；会话照常能在这台机器上用
    let lone = Device::new(&fx.d.join("lone"), "lone", &obj! { Device::KEY => "US-A" });
    for (argv, stdin) in [(["remote-pair-show"], ""), (["remote-pair-join"], "0000-0000-0000-0000\n")] {
        let r = lone.run_with(&argv, stdin, &[]);
        assert_eq!((r.code, r.out.as_str()), (1, ""), "{argv:?}");
        assert!(!r.err.trim().is_empty(), "{argv:?}");
    }
    assert_eq!(lone.sent(None), no_values());
}

// ---- 两台设备上都是真的 cca（辅助程序）：box 上跑会话，laptop 接入 ----

/// 每台设备上一次问服务器“别的机器上有哪些会话”是什么时候（原来记在用例的 asked 里）
#[derive(Default)]
struct Asked(RefCell<BTreeMap<String, Instant>>);

impl Asked {
    /// dev 看到的别的机器上的会话：一行一个。服务器给每台设备的请求限了流（每 2 秒一个、攒得下 30 个）：
    /// 等某个状态时一圈圈地问，两次之间隔开一秒，别把配额问光（之后那台设备的请求会被拒）
    fn far(&self, dev: &Device) -> Vec<String> {
        let last = self.0.borrow().get(&dev.name).copied();
        if let Some(gap) = last.and_then(|at| Duration::from_secs(1).checked_sub(at.elapsed())) {
            std::thread::sleep(gap);
        }
        self.0.borrow_mut().insert(dev.name.clone(), Instant::now());
        let r = dev.run(&["remote-list", "--far"]);
        assert_eq!(r.code, 0, "{}", r.err);
        splitlines(&r.out).into_iter().map(str::to_string).collect()
    }

    /// 一圈圈地问，到列出了会话为止（最多 30 秒）：那几行
    fn listed(&self, dev: &Device, what: &str) -> Vec<String> {
        wait_for(what, secs(30), || Some(self.far(dev)).filter(|rows| !rows.is_empty()))
    }
}

/// 这一组用例的准备：box、laptop 两台，都登记在服务器上。返回 (夹具, box, laptop, 问服务器的节奏)
fn client_fx(t: &mut Ctx) -> (Base, Device, Device, Asked) {
    let fx = Base::new(t);
    let (a, b) = (fx.device("box"), fx.device("laptop"));
    fx.machines(&[&a, &b]);
    (fx, a, b, Asked::default())
}

/// dev 上起一个在宿主里运行的会话（alice 的账号）
fn session(dev: &Device) -> Term {
    relay::session(dev, Device::KEY, "alice", &[])
}

/// shower 显示配对码，joiner 输入。返回 (配对码, 配对编号)
fn pair(shower: &Device, joiner: &Device) -> (String, String) {
    let mut p = Popen::spawn(&mut relay::helper_cmd(shower, &["remote-pair-show"]));
    let line = p.out_line(); // 配对码马上写出来（调用方拿去显示），配好以后才是对方的名字
    assert!(matches(r"^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", &line), "{line:?}");
    let code = line.split_whitespace().nth(1).expect("配对码").to_string();
    let r = joiner.run_with(&["remote-pair-join"], &format!("{}\n", code.to_lowercase().replace('-', " ")), &[]);
    assert_eq!((r.code, r.out, r.err), (0, format!("{}\n", shower.name), String::new()));
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out, said), (0, format!("peer {}\n", joiner.name), String::new()));
    let pids = names(&relay::peers(joiner));
    assert_eq!(pids.len(), 1, "{pids:?}");
    (code, pids[0].clone())
}

fn pair_list_attach(t: &mut Ctx) {
    let (fx, a, b, asked) = client_fx(t);
    assert_eq!(asked.far(&b), no_rows(), "没配对过：不问服务器");
    assert_eq!(b.sent(None), no_values());
    let mut h = session(&a); // 配对之前就开着的会话，配对以后也接得上
    let (code, pid) = pair(&a, &b);
    let pid = pid.as_str();

    // 两边各存了同一把密钥（密钥文件，600），配对表里是对方的指纹和名字；默认只有显示配对码的那台让对方接入
    let (pa, pb) = (relay::peers(&a), relay::peers(&b));
    assert_eq!(names(&pa), [pid]);
    assert_eq!(cols(&pa[pid], &["fp", "name", "incoming", "shown"]), list![b.fp(), "laptop", "control", true]);
    assert_eq!(cols(&pb[pid], &["fp", "name", "incoming", "shown"]), list![a.fp(), "box", "none", false]);
    let key = relay::secret(&a, pid).expect("box 存着这个配对的密钥");
    assert!(matches("^[0-9a-f]{64}$", &key), "{key}");
    assert_eq!(relay::secret(&b, pid), Some(key.clone()));
    for dev in [&a, &b] {
        assert_eq!(fsx::mode(dev.cca.join("secrets").join(format!("cca-remote-{pid}"))) & 0o777, 0o600);
        assert_eq!(fsx::mode(dev.cca.join("remote").join("peers.json")) & 0o777, 0o600);
        assert!(!fsx::read_text(dev.cca.join("remote").join("peers.json")).contains(&key));
    }
    // 配对码、密钥都没有发给服务器（请求里只有从配对码派生的编号和密文）
    let raw = json_files(&a.home.join("sent")) + &json_files(&b.home.join("sent"));
    assert!(raw.contains("pair_commit"));
    let (kc, _, rid) = spec::pair_keys(&spec::code_bytes(&code).expect("配对码"));
    for secret in [code.clone(), code.replace('-', ""), key.clone(), crypto::hex(&kc)] {
        assert!(!raw.contains(&secret), "{secret}");
    }
    assert_eq!(rid, a.sent(Some("pair_open"))[0]["rid"].str());
    assert_eq!(names(&fx.edges()), [pid]);
    let rows = a.run(&["remote-peers", "--check"]).out;
    let want = format!(r"^{}  laptop  对方可以看和操作这台机器的会话  \d{{4}}-\d\d-\d\d 配对  经「测试组」的服务器\n$", &pid[..8]);
    assert!(matches(&want, &rows), "配对记在哪个 Team 的服务器上也写出来：{rows:?}");
    let rows = b.run(&["remote-peers"]).out;
    assert!(matches(&format!("^{}  box  对方不能接入这台机器  ", &pid[..8]), &rows), "{rows:?}");

    // laptop 看得到 box 上的会话（box 看不到 laptop 的：没有，也不让）
    let row = asked.listed(&b, "laptop 列出 box 上的会话").remove(0);
    assert!(matches(r"^[0-9a-f]{8}  claude  alice  ~/work  \d\d-\d\d \d\d:\d\d 开始  在「box」上  「测试组」$", &row), "{row:?}");
    let sid = &row[..8];
    assert_eq!(asked.far(&a), no_rows());
    assert_eq!(b.run(&["remote-list", "--local"]).out, "");
    assert_eq!(b.run(&["remote-list"]).out, format!("{row}\n"));
    // 给界面 App 的那一份（PROTOCOL 9.1）：laptop 上是 box 的会话和“对方接不进来”，box 上是自己的会话和“对方能操作”
    let doc = parsed(&b.ok(&["remote-list", "--json"]));
    let hubs = list![obj! { "team" => fx.team.id.as_str(), "name" => "「测试组」的服务器" }];
    assert_eq!(
        cols(&doc, &["version", "kind", "enabled", "relay", "errors"]),
        list![1, "remote", false, obj! { "available" => true, "message" => Value::Null, "hubs" => hubs }, list![]]
    );
    let s = only(&doc["sessions"]);
    let shown = ["short", "where", "peer", "machine", "tool", "name", "cwd", "state", "access", "local_terminal"];
    assert_eq!(
        (cols(s, &shown), &s["id"].str()[..8], s["id"].str().len()),
        (list![sid, "peer", pid, "box", "claude", "alice", "~/work", "ok", "control", Value::Null], sid, 32)
    );
    assert!((s["since_epoch"].f64() - fleet::now()).abs() < 600.0, "{s}");
    assert_eq!(
        rows_of(&doc["peers"], &["id", "short", "name", "incoming", "shown", "state"]),
        [list![pid, &pid[..8], "box", "none", false, "ok"]]
    );
    let doc = parsed(&a.ok(&["remote-list", "--json"]));
    let s = only(&doc["sessions"]);
    assert_eq!(
        cols(s, &["short", "where", "peer", "machine", "state", "access", "local_terminal", "viewers"]),
        list![sid, "local", Value::Null, Value::Null, "ok", "control", true, 0]
    );
    assert_eq!(rows_of(&doc["peers"], &["name", "incoming", "shown", "state"]), [list!["laptop", "control", true, "ok"]]);
    assert!(!json::dumps(&doc, &Style::default()).contains("example.com"), "给 App 的也不带邮箱");
    let doc = parsed(&a.ok(&["remote-list", "--json", "--local"]));
    assert_eq!(
        (doc["sessions"].list().len(), rows_of(&doc["peers"], &["state"])),
        (1, vec![list!["unknown"]]),
        "--local 不问服务器"
    );

    // 接入：先收到回放，之后两边同步；按键到会话里；Ctrl-\ d 断开，会话接着运行
    let mut v = relay::term_sized(&b, &["remote-attach", sid], 30, 100);
    v.expect(&format!("正在接入「box」上的会话 {sid}"));
    v.expect("<ready 24 80");
    h.expect("<size 24 80>|<ready"); // 两个终端都画得下的大小还是 24x80
    v.send("say typed-on-the-laptop\n");
    h.expect("<typed-on-the-laptop>");
    v.expect("<typed-on-the-laptop>");
    h.send("say typed-on-the-box\n");
    v.expect("<typed-on-the-box>");
    // 会话里的程序写剪贴板（OSC 52）：刚在 laptop 上按过键，laptop 是主的那一处，laptop 自己也放进它的剪贴板（pbcopy，macOS 上）
    let clip = b.home.join("pbcopied");
    fsx::write_script(
        b.home.join("bin/pbcopy"),
        &format!("#!/bin/sh\ncat > '{}.tmp' && mv '{}.tmp' '{}'\n", clip.display(), clip.display(), clip.display()),
    );
    v.send("clip\n");
    v.expect("<clipped>");
    if cfg!(target_os = "macos") {
        assert_eq!(wait_for("laptop 的剪贴板", WAIT, || fsx::try_read_text(&clip)), "clip");
        fsx::write(&clip, "");
        // 在 box 上按的键：box 成了主的那一处，laptop 收不到这条，剪贴板不动
        h.send("clip\n");
        h.expect("<clipped>");
        v.expect("<clipped>");
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(fsx::read_text(&clip), "", "不是正在操作的那一处：不写剪贴板");
    }
    v.send("say typed-on-the-laptop-again\n");
    h.expect("<typed-on-the-laptop-again>");
    assert!(a.run(&["remote-list", "--local"]).out.contains("1 处接入"));
    wait_for("列表里看得到有人接着", WAIT, || asked.far(&b).concat().contains("，1 处接入"));
    // 会话里的钩子推一条通知（cca remote notify）：别的机器上接着的那一端写成 OSC 777 弹出来；这台机器上的终端不给
    // （它的钩子自己会弹）；会话外面调的不在哪个宿主里：65
    let notify = format!("run {} remote-notify --title 做完了 --subtitle alice 第一行回复", relay::helper(&a).display());
    let since = h.out().len();
    h.send(format!("{notify}\n"));
    h.expect("<ran 0>");
    v.expect(r"\x1b\]777;notify;做完了 · alice;第一行回复\x1b\\");
    assert!(!has(&h.out()[since..], "777;notify"), "启动它的终端上没有");
    let r = a.run(&["remote-notify", "--title", "外面", "正文"]);
    assert_eq!(r.code, 65, "{}", r.err);
    assert_eq!(a.run(&["remote-notify"]).code, 64, "什么字都没有");
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);
    v.expect(&format!("已断开，会话还在运行（再接入：cca remote attach {sid}）"));
    h.send("say still-here\n");
    h.expect("<still-here>");

    // 再接入；会话结束时两边都拿到客户端的退出码，列表里没有了
    let mut v = relay::term(&b, &["remote-attach"]); // 只有这一个：不用写编号
    v.expect("<still-here>");
    v.send("exit 7\n");
    assert_eq!((h.wait(), v.wait()), (7, 7));
    v.expect("会话已经结束（退出码 7）");
    wait_for("会话从列表里消失", secs(20), || asked.far(&b).is_empty());
    let r = b.run(&["remote-attach", sid]);
    assert_eq!(r.code, 65);
    assert!(r.err.contains(&format!("没有编号以 {sid} 开头的会话")), "{}", r.err);

    // 服务器（和路上的人）看到的：开场白、密文。终端里的字、按键一个都没有明文
    let seen = fx.sniff.snapshot();
    assert!(has(&seen, "CCA-RELAY/1 view "));
    assert!(has(&seen, "CCA-RELAY/1 join "));
    assert!(seen.len() > 2000, "{}", seen.len());
    for word in ["typed-on-the-laptop", "typed-on-the-box", "still-here", "ready 24 80", "alice", "work", key.as_str()] {
        assert!(!has(&seen, word), "{word}");
    }
    let state = json_files(&fx.d);
    for word in ["typed-on", "alice@example.com", "/work", key.as_str()] {
        assert!(!state.contains(word), "{word}");
    }
}

/// cca 自己带的 Claude Code 钩子（remote-hook，cli/src/cmd/remote/hook.rs）：会话里的 Claude Code 照钩子的输入（stdin 上的 JSON）
/// 调它，别的机器上接着的那一端弹出来——做完了（最后一条回复的第一行）、问了一阵没人答（PermissionRequest 时记下的那一句，
/// Notification 的 permission_prompt 来了才推）、别的在等你的（Claude Code 的原话）。副标题是账号的别名（CCA_ACCOUNT）和项目的
/// 文件夹。后台任务还在跑的“做完了”不推；用户自己的钩子（cca remote notify）为同一件事推的一模一样的那条不再推；会话外面调的
/// 什么都不做。服务器看不到通知里的字
fn cca_hooks_reach_the_other_machine(t: &mut Ctx) {
    let (fx, mut a, b, asked) = client_fx(t);
    let work = relay::work(&a).to_str().expect("临时目录的路径是 UTF-8").to_string();
    a.env.insert("CCA_ACCOUNT".into(), "工作(alice@example.com)".into());
    a.env.insert("CLAUDE_PROJECT_DIR".into(), work);
    let mut h = session(&a);
    pair(&a, &b);
    let row = asked.listed(&b, "laptop 列出 box 上的会话").remove(0);
    let mut v = relay::term(&b, &["remote-attach", &row[..8]]);
    v.expect("<ready 24 80");
    let helper = relay::helper(&a);
    let ask = |sid: &str| a.cca.join("remote").join("ask").join(sid);
    let mut n = 0;
    let mut hook = |h: &mut Term, input: &str| {
        n += 1;
        let f = a.home.join(format!("hook-{n}.json"));
        fsx::write(&f, input);
        let m = h.mark();
        h.send(format!("runin {} {} remote-hook\n", f.display(), helper.display()));
        h.expect_since("<ran 0>", m);
    };
    let osc = |title: &str, body: &str| format!(r"\x1b\]777;notify;{title} · 工作 · work;{body}\x1b\\");
    let m = v.mark();
    hook(
        &mut h,
        r#"{"hook_event_name": "Stop", "session_id": "s-1", "last_assistant_message": "**改好了** 两处\n细节", "background_tasks": [], "session_crons": []}"#,
    );
    v.expect_since(&osc("Claude Code 做完了", "改好了 两处"), m);
    // 用户自己的钩子照同样的写法也推了一条：一模一样，不再推
    let mine = a.home.join("my-hook");
    let said = "--title 'Claude Code 做完了' --subtitle '工作 · work' '改好了 两处'";
    fsx::write_script(&mine, &format!("#!/bin/sh\nexec '{}' remote-notify {said}\n", helper.display()));
    let at = h.mark();
    h.send(format!("run {}\n", mine.display()));
    h.expect_since("<ran 0>", at);
    hook(
        &mut h,
        r#"{"hook_event_name": "Stop", "session_id": "s-1", "last_assistant_message": "不该推这一条", "background_tasks": [{"type": "shell"}], "session_crons": []}"#,
    );
    hook(
        &mut h,
        r#"{"hook_event_name": "PermissionRequest", "session_id": "s-1", "tool_name": "Bash", "tool_input": {"command": "rm -rf build", "description": "清掉编译的"}}"#,
    );
    assert!(ask("s-1").exists(), "问的时候只记下要说的话");
    hook(
        &mut h,
        r#"{"hook_event_name": "Notification", "session_id": "s-1", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}"#,
    );
    v.expect_since(&osc("Claude Code 要你确认", "运行：清掉编译的"), m);
    assert!(!ask("s-1").exists(), "推了就删掉");
    hook(
        &mut h,
        r#"{"hook_event_name": "Notification", "session_id": "s-2", "notification_type": "elicitation_dialog", "message": "**github** 要你填一个值"}"#,
    );
    v.expect_since(&osc("Claude Code 在等你", "github 要你填一个值"), m);
    hook(
        &mut h,
        r#"{"hook_event_name": "PermissionRequest", "session_id": "s-3", "tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "用哪个？"}]}}"#,
    );
    hook(
        &mut h,
        r#"{"hook_event_name": "Notification", "session_id": "s-3", "notification_type": "agent_needs_input", "message": "后台的会话要你回答"}"#,
    );
    v.expect_since(&osc("Claude Code 在等你", "后台的会话要你回答"), m);
    assert!(!ask("s-3").exists(), "不是在等确认的那一种：记下的那一句不拿来用，也删掉");
    let shown = String::from_utf8_lossy(&v.out()[m..]).to_string();
    assert_eq!(shown.matches("改好了 两处").count(), 1, "{shown:?}");
    assert!(!shown.contains("不该推"), "后台任务还在跑：做完了会叫醒会话，那时再说");
    assert!(!has(h.out(), "777;notify"), "启动它的终端上没有（这台机器上的钩子自己会弹）");
    // 不在宿主里（界面 App 里启动的会话也带着这几个钩子）：什么都不做，总是成功
    let r = a.run(&["remote-hook"]);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
    h.send("exit 0\n");
    assert_eq!((h.wait(), v.wait()), (0, 0));
    let seen = fx.sniff.snapshot();
    for word in ["改好了", "清掉编译的", "github", "后台的会话", "Claude Code"] {
        assert!(!has(&seen, word), "服务器看得到通知里的字：{word}");
    }
}

/// Codex 的会话（宿主当它是 codex：cca 放进宿主时给它的 tui.notifications 让它把在等你的那几种写成 OSC 9）：宿主认出来推出去，
/// 接着它的另一台机器上弹出来；原来的序列不给接入方（不然弹两次），这台 Mac 上弹了（假的 osascript 记下）就不给启动它的终端。
/// 一轮做完了的是 Codex 起的 notify（`remote-hook --codex JSON`）：一样推给接着的各处、这台 Mac 上弹。config.json 的 popup 是
/// false 时这台 Mac 上不弹，序列照给启动它的终端
fn codex_notes_reach_the_other_machine(t: &mut Ctx) {
    let (fx, mut a, b, asked) = client_fx(t);
    a.env.insert("CCA_ACCOUNT".into(), "工作(alice@example.com)".into());
    let mut h = relay::session_as(&a, "codex", Device::KEY, "alice", &[]);
    pair(&a, &b);
    let row = asked.listed(&b, "laptop 列出 box 上的会话").remove(0);
    let mut v = relay::term(&b, &["remote-attach", &row[..8]]);
    v.expect("<ready 24 80");
    let osc = |title: &str, body: &str| format!(r"\x1b\]777;notify;{title} · 工作 · work;{body}\x1b\\");
    let popped = || fsx::try_read_text(a.home.join("osascript.log")).unwrap_or_default();
    let said = |h: &mut Term, line: &str, back: &str| {
        let at = h.mark();
        h.send(format!("{line}\n"));
        h.expect_since(back, at);
        at
    };
    let m = v.mark();
    let at = said(&mut h, "osc Approval requested: cargo test", "<osc>");
    v.expect_since(&osc("Codex 要你确认", "运行：cargo test"), m);
    let pop = "OSA [-] [Codex 要你确认] [工作 · work] [运行：cargo test] [Ping]";
    wait_for("这台 Mac 上弹了「Codex 要你确认」", WAIT, || popped().lines().any(|l| l == pop));
    assert!(!has(&h.out()[at..], "\x1b]9;"), "弹了：启动它的终端不再给");
    // 一轮做完了：Codex 起 cca 给它的 notify，JSON 加在最后（会话里的钩子那样：客户端的子进程）
    let helper = relay::helper(&a);
    let done = r#"{"type":"agent-turn-complete","cwd":"/x/work","last-assistant-message":"**改好了**两处"}"#;
    said(&mut h, &format!("run {} remote-hook --codex {done}", helper.display()), "<ran 0>");
    v.expect_since(&osc("Codex 做完了", "改好了两处"), m);
    let pop = "OSA [-] [Codex 做完了] [工作 · work] [改好了两处] [Glass]";
    wait_for("这台 Mac 上弹了「Codex 做完了」", WAIT, || popped().lines().any(|l| l == pop));
    // 用户自己又打开了一轮做完了的那一句：不推（notify 推过了），也不给接入方；这台 Mac 上弹着，启动它的终端也不给
    let at = said(&mut h, "osc Agent turn complete", "<osc>");
    assert!(!has(&h.out()[at..], "\x1b]9;"), "弹着的时候不推的那一句也不给启动它的终端");
    // 这台 Mac 上不弹的时候：照样推，序列照给启动它的终端（它认得的话自己会弹）
    fsx::write(a.cca.join("config.json"), r#"{"popup": false}"#);
    let at = said(&mut h, "osc Question: 用哪个库？", "<osc>");
    v.expect_since(&osc("Codex 在问你", "用哪个库？"), m);
    said(&mut h, "osc Agent turn complete", "<osc>");
    assert!(has(&h.out()[at..], "\x1b]9;Question: 用哪个库？\x07"), "没弹：启动它的终端照给");
    assert!(has(&h.out()[at..], "\x1b]9;Agent turn complete\x07"), "不推的那一句启动它的终端照给");
    let shown = String::from_utf8_lossy(&v.out()[m..]).to_string();
    assert!(!shown.contains("\x1b]9;"), "接入方只收推出去的那一条：{shown:?}");
    assert!(!shown.contains("notify;Codex 在等你"), "一轮做完了的那一句没换成在等你：{shown:?}");
    assert_eq!(popped().lines().filter(|l| l.starts_with("OSA ")).count(), 2, "{}", popped());
    h.send("exit 0\n");
    assert_eq!((h.wait(), v.wait()), (0, 0));
    let seen = fx.sniff.snapshot();
    for word in ["cargo test", "改好了", "用哪个库", "Codex"] {
        assert!(!has(&seen, word), "服务器看得到通知里的字：{word}");
    }
}

fn what_the_other_side_may_do(t: &mut Ctx) {
    let (fx, a, mut b, asked) = client_fx(t);
    let (_code, pid) = pair(&a, &b);
    let pid = pid.as_str();
    let mut h = session(&a);
    let sid = asked.listed(&b, "列出会话")[0][..8].to_string();
    let sid = sid.as_str();
    let mut v = relay::term(&b, &["remote-attach", sid]);
    v.expect("<ready 24 80");
    v.send("say before\n");
    h.expect("<before>");

    // 改成只让看：正操作着的那一处被断开。它自己重新接上，这回是只看（说一句），按键到不了会话
    assert_eq!(a.ok(&["remote-allow", "laptop", "view"]), "laptop\n");
    let mark = v.mark();
    v.expect_in("连接断了，正在重新接上", secs(20), mark);
    v.expect_in("「box」现在只让这台机器看这个会话，不能操作", secs(40), mark);
    v.expect_since("<before>", mark); // 接上以后的回放
    let rows = wait_for("列表里说只能看", secs(30), || {
        let rows: Vec<String> = asked.far(&b).into_iter().filter(|r| r.contains("只能看")).collect();
        Some(rows).filter(|rows| !rows.is_empty())
    });
    assert!(rows[0].contains("在「box」上，只能看"), "{rows:?}");
    let mark = h.mark();
    v.send("say sneaky\n");
    h.send("say from-the-box\n");
    v.expect("<from-the-box>");
    assert!(!has(&h.out()[mark..], "sneaky"));
    assert!(a.run(&["remote-peers"]).out.contains("对方只能看这台机器的会话"));
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);
    // 新接入的一开始就说
    let mut v = relay::term(&b, &["remote-attach", sid]);
    v.expect("「box」只让这台机器看这个会话，不能操作");
    v.expect("<from-the-box>");
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);

    // 不让接入了：断开，对方的列表里没有这个会话了，也接不上。正接着的那一处试一阵（这里调成 3 秒）以后说没接上
    b.env.insert("CCA_REMOTE_RETRY".to_string(), "3".to_string());
    let mut v = relay::term(&b, &["remote-attach", "-r", sid]);
    v.expect("<from-the-box>");
    b.env.remove("CCA_REMOTE_RETRY");
    assert_eq!(a.ok(&["remote-allow", &pid[..6], "none"]), "laptop\n");
    assert_eq!(v.wait_in(secs(30)), 68);
    v.expect("连接断了，正在重新接上");
    v.expect("没重新接上：那台机器上(现在|已经)没有这个会话");
    wait_for("会话从 laptop 的列表里消失", secs(30), || asked.far(&b).is_empty());
    assert_eq!(b.run(&["remote-attach", sid]).code, 65);
    // 再让：又看得到、能操作
    a.ok(&["remote-allow", "laptop", "control"]);
    asked.listed(&b, "又列出来了");
    let mut v = relay::term(&b, &["remote-attach", sid]);
    v.expect("<from-the-box>");
    v.send("say back-again\n");
    h.expect("<back-again>");
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);

    // 取消配对（laptop 这边）：密钥删掉、告诉服务器；box 那边看得出对方取消了，会话不再给它
    assert_eq!(b.run(&["remote-unpair", "box"]).out, "box\n");
    assert_eq!((relay::peers(&b), relay::secret(&b, pid), fx.edges()), (obj! {}, None, obj! {}));
    assert!(a.run(&["remote-peers", "--check"]).out.contains("（对方已经取消了配对：cca remote unpair 把它删掉）"));
    assert_eq!(asked.far(&b), no_rows());
    let r = b.run(&["remote-unpair", "box"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("没有叫「box」的配对"), "{}", r.err);
    // cca restore 用的：全部取消，密钥都删掉
    assert_eq!(a.ok(&["remote-forget"]), "");
    assert_eq!((relay::peers(&a), relay::secret(&a, pid)), (obj! {}, None));
    h.send("exit 0\n");
    assert_eq!(h.wait(), 0);
}

/// 另一头是照文档独立实现的那一份：能和 cca 配对、握手、收发；宿主那头的规矩不靠接入方自觉
fn the_other_implementation_and_what_the_host_enforces(t: &mut Ctx) {
    let (fx, a, b, _asked) = client_fx(t);
    let other = fx.device("other");
    fx.machines(&[&a, &b, &other]);
    let mut peer = SpecPeer::new(&fx, &other);
    let mut p = Popen::spawn(&mut relay::helper_cmd(&a, &["remote-pair-show"]));
    let line = p.out_line();
    peer.join(line.split_whitespace().nth(1).expect("配对码"));
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out.as_str()), (0, "peer other\n"), "{said}");
    assert_eq!(relay::secret(&a, &peer.pid), Some(crypto::hex(&peer.p)));

    let mut h = session(&a);
    let found = wait_for("会话的说明", secs(30), || Some(peer.sessions()).filter(|found| !found.is_empty()));
    assert_eq!(found.len(), 1, "{found:?}");
    let s = &found[0];
    assert_eq!(
        (s["machine"].str() != "", cols(s, &["tool", "name", "cwd", "access"])),
        (true, list!["claude", "alice", "~/work", "control"])
    );
    assert!(!json::dumps(s, &Style::default()).contains("example.com"), "账号只给短名字");
    let sid = s["sid"].str();
    let hello = frame(HELLO, &[1, 0, 0, 80, 0, 24]);

    // 握手过了才有输出；按键到会话里；能操作的也能改窗口大小（取都画得下的）
    let (sock, ch) = peer.attach(sid);
    let mut r = Reader::new(sock, ch.expect("握手：宿主的证明对得上"));
    r.send(&hello);
    r.until(|out| has(out, "<ready 24 80"));
    r.send(&frame(INPUT, b"say from-the-other-implementation\n"));
    h.expect("<from-the-other-implementation>");
    r.until(|out| has(out, "<from-the-other-implementation>"));
    r.send(&frame(RESIZE, &[0, 70, 0, 20]));
    h.expect("<size 20 70>");
    r.send(&frame(RESIZE, &[0, 80, 0, 24]));
    h.expect("<size 24 80>");
    // 宿主的状态（里面有账号的邮箱、完整的路径、进程号）：问了不替它转，宿主自己发的欢迎帧也不转——到接入方的只有输出。
    // “结束这个会话”（0x07，cca remote end）只有那台机器上说得了：能操作的接入方发了也不转，会话照常
    r.send(&frame(0x06, b""));
    r.send(&frame(0x07, b""));
    r.send(&frame(HELLO, &[1, 0, 0, 80, 0, 24])); // 再说一遍 HELLO 也不转
    r.send(&frame(INPUT, b"say after-status\n"));
    r.until(|out| has(out, "<after-status>"));
    assert_eq!(r.kinds, BTreeSet::from([OUTPUT]));
    let work = relay::work(&a);
    for word in ["example.com", work.to_str().expect("临时目录的路径是 UTF-8"), "\"pid\""] {
        assert!(!has(&r.raw, word), "{word}");
    }
    // 改过的记录、重放的记录：宿主那头马上断开，不往会话里送
    let mut rec = r.ch.seal(&frame(INPUT, b"say tampered\n"));
    rec[12] ^= 1;
    let mark = h.mark();
    sendall(&mut r.s, &rec);
    read_all(&mut r.s); // 到宿主那头把连接关上为止
    drop(r);
    h.send("say host-still-fine\n");
    h.expect("<host-still-fine>");
    assert!(!has(&h.out()[mark..], "tampered"));

    // 没有配对密钥的（服务器自己、偷了设备在 Team 里的密钥的人）：接入方那头看得出宿主的证明不对；
    // 硬着头皮发一个证明过去，宿主不认、什么都不给
    let (mut sock, ch) = peer.attach_with(sid, &crypto::random(32));
    assert!(ch.is_none());
    sendall(&mut sock, &[&b"CR\x03"[..], &crypto::random(32)].concat());
    assert_eq!(read_all(&mut sock), b"");
    drop(sock);

    // 只让看：接入方不守规矩（HELLO 里不说只看、报一个很小的窗口、照发按键、改窗口大小）也没用：
    // 按键到不了会话，会话的窗口大小不跟着它变（接入时让客户端重画的那一下是 23 行再回到 24 行）
    a.ok(&["remote-allow", "other", "view"]);
    wait_for("说明里改成只看", secs(30), || peer.sessions().iter().any(|s| s["access"].str() == "view"));
    let mark = h.mark();
    let (sock, ch) = peer.attach(sid);
    let mut r = Reader::new(sock, ch.expect("握手：宿主的证明对得上"));
    r.send(&frame(HELLO, &[1, 0, 0, 20, 0, 5]));
    r.until(|out| has(out, "<host-still-fine>"));
    r.send(&frame(INPUT, b"say forced-input\n"));
    r.send(&frame(RESIZE, &[0, 2, 0, 2]));
    h.send("say host-typed\n");
    r.until(|out| has(out, "<host-typed>"));
    h.expect_since("<host-typed>", mark);
    assert!(!has(&h.out()[mark..], "forced-input"));
    let sizes = regex::bytes::Regex::new(r"(?-u)<size (\d+ \d+)>").expect("正则");
    for size in sizes.captures_iter(&h.out()[mark..]) {
        assert!([&b"23 80"[..], b"24 80"].contains(&&size[1]), "{:?}", String::from_utf8_lossy(&size[1]));
    }
    assert!(a.run(&["remote-list", "--local"]).out.contains("另有 1 处接入"));
    // 会话结束：接入方收到退出码，连接由宿主那头关上
    h.send("exit 3\n");
    assert_eq!(h.wait(), 3);
    assert!(has(r.until(|out| has(out, "<EXIT")), "<EXIT 3>"));
    assert_eq!(r.kinds, BTreeSet::from([OUTPUT, EXIT]));
}

/// 能缩放的接入方（HELLO 的标志 4：iOS 上的 App，会话比它的屏幕大时缩小了画）：接上、看着不改会话的大小，会话多大宿主告诉它
/// （SIZE，在回放前面）。它打了字会话照它排，这边的终端打了字又照这边排；它改窗口大小、滚着看、切进来不算打字；它打过字以后
/// 走了，回到这边的终端的大小
fn a_phone_sets_the_size_only_when_it_types(t: &mut Ctx) {
    let (fx, a, b, _asked) = client_fx(t);
    let other = fx.device("other");
    fx.machines(&[&a, &b, &other]);
    let mut peer = SpecPeer::new(&fx, &other);
    let mut p = Popen::spawn(&mut relay::helper_cmd(&a, &["remote-pair-show"]));
    let line = p.out_line();
    peer.join(line.split_whitespace().nth(1).expect("配对码"));
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out.as_str()), (0, "peer other\n"), "{said}");
    let mut h = session(&a);
    let found = wait_for("会话的说明", secs(30), || Some(peer.sessions()).filter(|found| !found.is_empty()));
    let sid = found[0]["sid"].str();
    let re = regex::bytes::Regex::new(r"(?-u)<size (\d+ \d+)>").expect("正则");
    let sizes = |h: &Term, mark: usize| -> Vec<String> {
        re.captures_iter(&h.out()[mark..]).map(|c| String::from_utf8_lossy(&c[1]).into_owned()).collect()
    };

    // 接上、看着：会话照旧是这边的终端的大小（只有接入时让客户端重画的那一下：23 行再回到 24 行）
    let mark = h.mark();
    let (sock, ch) = peer.attach(sid);
    let mut r = Reader::new(sock, ch.expect("握手：宿主的证明对得上"));
    r.send(&frame(HELLO, &[1, 4, 0, 40, 0, 20])); // 能缩放，窗口 40 列 20 行
    let first = r.until(|out| has(out, "<ready 24 80")).to_vec();
    assert!(first.starts_with(b"<SIZE 80 24>"), "{:?}", String::from_utf8_lossy(&first[..first.len().min(60)]));
    h.send("say host-sync\n");
    r.until(|out| has(out, "<host-sync>"));
    h.expect_since("<host-sync>", mark);
    assert!(sizes(&h, mark).iter().all(|s| s == "23 80" || s == "24 80"), "{:?}", sizes(&h, mark));

    // 它打了字：照它的窗口排，也告诉它；这边打了字：又照这边的终端排
    let (mark, seen) = (h.mark(), r.out.len());
    r.send(&frame(INPUT, b"say from-the-phone\n"));
    h.expect_since("<size 20 40>", mark);
    r.until(|out| has(&out[seen..], "<SIZE 40 20>"));
    let (mark, seen) = (h.mark(), r.out.len());
    h.send("say from-the-host\n");
    h.expect_since("<size 24 80>", mark);
    r.until(|out| has(&out[seen..], "<SIZE 80 24>"));

    // 它改了窗口大小、滚着看、切进来（滚轮、焦点进出）：都不算打字，会话不动。这边的终端回显了收到的滚轮：宿主已经看过这几帧了
    let mark = h.mark();
    r.send(&frame(RESIZE, &[0, 50, 0, 30]));
    r.send(&frame(INPUT, b"\x1b[<64;5;5M\x1b[I"));
    h.expect_since(r"(\^\[|\x1b)\[<64;5;5M", mark);
    h.send("\x15say after-the-wheel\n"); // 先清掉这一行里回显的那一截（行模式：不清的话这一行不是 say 开头）
    h.expect_since("<after-the-wheel>", mark);
    assert_eq!(sizes(&h, mark), no_rows());

    // 它又打了字（照它现在的窗口：比这边的终端高，行数取这边的），然后走了：回到这边的终端的大小
    let mark = h.mark();
    r.send(&frame(INPUT, b"say phone-again\n"));
    h.expect_since("<size 24 50>", mark);
    let mark = h.mark();
    r.send(&frame(DETACH, b""));
    h.expect_since("<size 24 80>", mark);
    drop(r);
    h.send("exit 0\n");
    assert_eq!(h.wait(), 0);
}

fn the_link_process_is_killed(t: &mut Ctx) {
    let (_fx, a, b, asked) = client_fx(t);
    pair(&a, &b);
    let mut h = session(&a);
    let sid = asked.listed(&b, "列出会话")[0][..8].to_string();
    let sid = sid.as_str();
    let mut v = relay::term(&b, &["remote-attach", sid]);
    v.expect("<ready 24 80");
    let pattern = format!("remote-link {sid}");
    let links = relay::pgrep(&pattern);
    assert_eq!(links.len(), 2, "转接进程，和它为这个接入方分出来的那一个");
    // 父进程不在这两个里的那个
    let link = *links.iter().min_by_key(|pid| links.contains(&parent_of(**pid))).expect("两个里的一个");
    kill9(link);
    // 没人管着的那个子进程自己走（不然配对表变了它也照转）：接入方那头断开，说一句、自己接着试；会话不受影响
    let mark = v.mark();
    v.expect_in("连接断了，正在重新接上", secs(20), mark);
    wait_for("子进程都走了", secs(20), || relay::pgrep(&pattern).len() <= 1);
    h.send("say link-was-killed\n");
    h.expect("<link-was-killed>");
    v.send("say typed-while-cut-off\n"); // 断着的时候敲的不攒着：接上以后不会迟到
                                         // 宿主过一会儿再起一个转接进程：接入方自己接上了，断着的时候那边的输出在回放里，又能操作
    v.expect_in("还没接上：那台机器暂时没连着服务器。接着试…", secs(30), mark);
    v.expect_in("<link-was-killed>", secs(90), mark);
    v.send("say back-again\n");
    h.expect("<back-again>");
    assert!(!has(h.out(), "typed-while-cut-off"));
    v.send("exit 0\n");
    assert_eq!((h.wait(), v.wait()), (0, 0));
}

fn the_server_restarts_and_the_session_ends_meanwhile(t: &mut Ctx) {
    let (mut fx, a, mut b, asked) = client_fx(t);
    pair(&a, &b);
    let mut h = session(&a);
    let sid = asked.listed(&b, "列出会话")[0][..8].to_string();
    let sid = sid.as_str();
    let mut v = relay::term(&b, &["remote-attach", sid]);
    v.expect("<ready 24 80");
    v.send("alt\n"); // 会话打开了备用屏幕、藏了光标、开了括号粘贴
    v.expect("<alt>");
    // 服务器重启（两头的连接都断了）：等的时候先把会话在这个终端上打开的模式关掉再说话；两头各自连回来，自己接上，
    // 模式由那边照新接入的补上
    let port = fx.srv.port;
    let restart = |fx: &mut Base| fx.srv = RelayServer::start(&fx.d, &RelayOpts::new().sniff(&fx.sniff).port(port));
    let mark = v.mark();
    fx.srv.stop();
    v.expect_in("连接断了，正在重新接上", secs(20), mark);
    let cut = v.out()[mark..].to_vec();
    let (modes_off, said) =
        (term::find(&cut, b"\x1b[?1049l").expect("退出备用屏幕"), term::find(&cut, "连接断了").expect("说了连接断了"));
    assert!(modes_off < said);
    assert!(has(&cut, b"\x1b[?2004l"));
    restart(&mut fx);
    let again = v.expect_in(r"\x1b\[\?1049h", secs(90), mark + cut.len());
    v.send("say after-the-restart\n");
    h.expect("<after-the-restart>");
    v.expect_since("<after-the-restart>", again);

    // 等的时候断开的键照样管用
    fx.srv.stop();
    v.expect_in("连接断了，正在重新接上", secs(20), again);
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);
    v.expect("已断开，会话还在运行");
    // 断着的时候会话结束了：服务器回来以后说没有这个会话了，试一阵（这里调成 10 秒：第 1、3、7、15 秒各试一次，
    // 机器慢的时候服务器晚几秒回来也赶得上最后那一次）说没接上，退出码 68
    restart(&mut fx);
    b.env.insert("CCA_REMOTE_RETRY".to_string(), "10".to_string());
    let mut attached = None;
    for _ in 0..60 {
        // 宿主那头的转接要过一阵才连回来
        let mut t = relay::term(&b, &["remote-attach", sid]);
        if shows(&mut t, "<after-the-restart>", secs(3)) {
            attached = Some(t);
            break;
        }
        drop(t);
        std::thread::sleep(secs(1)); // 再问之前隔一秒：服务器给每台设备的请求限了流
    }
    let mut v = attached.expect("服务器重启以后会话没有再登记上");
    fx.srv.stop();
    v.expect_in("连接断了，正在重新接上", secs(20), 0);
    h.send("exit 0\n");
    assert_eq!(h.wait(), 0);
    restart(&mut fx); // 马上回来：最后那一次问得到它，原因是“没有这个会话”
    assert_eq!(v.wait_in(secs(60)), 68);
    v.expect("没重新接上：那台机器上(现在|已经)没有这个会话");
    assert!(String::from_utf8_lossy(v.out()).contains(&format!("cca remote attach {sid}")));
}

fn local_sessions_never_touch_the_network(t: &mut Ctx) {
    let (mut fx, a, b, _asked) = client_fx(t);
    pair(&a, &b);
    // 原来是会话起来以后才数的。转接进程是会话一起来就起的：机器忙的时候，等到这里数，它已经问过服务器了（数出来的和后面
    // 那一次一样多，用例白白失败）。在起会话之前数：查的还是“起了会话，这台设备发过请求”
    let before = a.sent(None).len();
    let mut h = session(&a);
    wait_for("转接进程连上了服务器", secs(30), || !a.sent(Some("relay_host")).is_empty());
    let n = a.sent(None).len();
    assert!(n > before);
    // 这台机器上的会话：列出来、接进去都不问服务器
    assert_eq!(splitlines(&a.run(&["remote-list", "--local"]).out).len(), 1);
    let mut v = relay::term(&a, &["remote-attach"]);
    v.expect("<ready 24 80");
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);
    assert_eq!(a.sent(None)[n..].iter().map(|r| r["op"].str().to_string()).collect::<Vec<_>>(), no_rows());
    // 服务器停了：会话照常，这台机器上照样接得进去；laptop 那边说清楚没查到
    fx.srv.stop();
    h.send("say server-is-down\n");
    h.expect("<server-is-down>");
    let mut v = relay::term(&a, &["remote-attach"]);
    v.expect("<server-is-down>");
    v.send(b"\x1cd");
    assert_eq!(v.wait(), 0);
    let r = b.run(&["remote-list"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""));
    let why = format!("别的机器上的会话没查到：「测试组」的同步服务器：连不上 Team 的同步服务器（127.0.0.1:{}）", fx.srv.port);
    assert!(r.err.contains(&why), "{}", r.err);
    let doc = parsed(&a.ok(&["remote-list", "--json"])); // 给 App 的：没问到不算失败，这台机器上的照给，原因在 errors 里
    assert_eq!(
        (doc["sessions"].list().len(), rows_of(&doc["errors"], &["code"]), rows_of(&doc["peers"], &["state"])),
        (1, vec![list!["relay_unreachable"]], vec![list!["unknown"]])
    );
    h.send("exit 0\n");
    assert_eq!(h.wait(), 0);
}

fn pairing_errors(t: &mut Ctx) {
    let (fx, a, b, _asked) = client_fx(t);
    for bad in ["", "hello", "0000-0000-0000-000", "0000-0000-0000-0000-0"] {
        let r = b.run_with(&["remote-pair-join"], &format!("{bad}\n"), &[]);
        assert_eq!(r.code, 1, "{bad}");
        assert!(r.err.contains("配对码不对"), "{}", r.err);
    }
    assert_eq!(b.sent(None), no_values(), "样子不对的配对码不发请求");
    let r = b.run_with(&["remote-pair-join"], "0000-0000-0000-0000\n", &[]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("没有这个配对码"), "{}", r.err);
    // 自己显示的配对码自己输入：不行；配对码不能用第二次
    let mut p = Popen::spawn(&mut relay::helper_cmd(&a, &["remote-pair-show"]));
    let line = p.out_line();
    let code = format!("{}\n", line.split_whitespace().nth(1).expect("配对码"));
    let r = a.run_with(&["remote-pair-join"], &code, &[]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("这个配对码用不了"), "{}", r.err);
    assert_eq!(b.run_with(&["remote-pair-join"], &code, &[]).code, 0);
    assert_eq!(p.wait(secs(60)), 0);
    drop(p);
    let r = b.run_with(&["remote-pair-join"], &code, &[]);
    assert_eq!(r.code, 1);
    assert_eq!(relay::peers(&a).obj().len(), 1);
    // 反过来再配一次（laptop 显示）：换成新的一把密钥，以前让的照旧让，现在两边都能接入对方
    let old = names(&relay::peers(&a));
    assert_eq!(old.len(), 1, "{old:?}");
    let old = old[0].as_str();
    let (_code, pid) = pair(&b, &a);
    assert_ne!(pid, old);
    assert_eq!((relay::secret(&a, old), relay::secret(&b, old), names(&fx.edges())), (None, None, vec![pid]));
    let incoming = |dev: &Device| relay::peers(dev).obj().values().map(|p| p["incoming"].str().to_string()).collect::<Vec<_>>();
    assert_eq!((incoming(&a), incoming(&b)), (vec!["control".to_string()], vec!["control".to_string()]));
}

// ---- bash 这一层：真的 cca 脚本（仓库里的那一份），两台设备。终端里的提问用答案文件（CCA_TTY），
// 界面 App 的提问会话由 cca-test gui-session 扮演（事件文件、答案管道） ----

/// 这一组用例的准备：box、laptop 两台，都登记在服务器上
fn shell_fx(t: &mut Ctx) -> (Base, Device, Device) {
    let fx = Base::new(t);
    let (a, b) = (fx.device("box"), fx.device("laptop"));
    fx.machines(&[&a, &b]);
    (fx, a, b)
}

fn terminal(t: &mut Ctx) {
    let (fx, a, b) = shell_fx(t);
    // 不同意就不出配对码
    let r = fx.cca_with(&a, &["remote", "pair"], Some(&["n"]), None);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("那台机器可以看和操作这台机器上开着远程会话的会话"), "{}", r.err);
    assert!(r.err.contains("已取消"), "{}", r.err);
    assert!(!r.err.contains("配对码："), "{}", r.err);
    assert_eq!(a.sent(None), no_values(), "没同意：什么都没发给服务器");
    // 同意：打印配对码、等对方；另一台输入（小写、空格也行）
    let mut p = Popen::spawn(&mut fx.cca_cmd(&a, &["remote", "pair"], &[("CCA_TTY", &fx.answers(&["y"]))]));
    let code = loop {
        let line = p.err_line();
        assert!(!line.is_empty(), "没等到配对码");
        if line.contains("配对码：") {
            break line.trim().split('：').nth(1).expect("配对码：后面是配对码").to_string();
        }
    };
    assert!(matches("^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", &code), "{code:?}");
    let r = fx.cca_with(&b, &["remote", "join"], Some(&[&code.to_lowercase().replace('-', " ")]), None);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert!(r.err.contains("已经和「box」配对：这台机器可以接入它上面的会话"), "{}", r.err);
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out.as_str()), (0, ""), "{said}");
    assert!(said.contains("已经和「laptop」配对：它可以接入这台机器上开着远程会话的会话"), "{said}");
    assert!(said.contains("cca remote allow laptop view"), "{said}");
    let left: Vec<PathBuf> = walk(&fx.d).into_iter().filter(|f| named_like(f, "cca-pair.")).collect();
    assert_eq!(left, Vec::<PathBuf>::new());
    // 看、改、取消：stdout 只有数据（peers 的行），说明都在 stderr
    let r = fx.cca(&a, &["remote", "peers"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(matches("^[0-9a-f]{8}  laptop  对方可以看和操作这台机器的会话  ", &r.out), "{:?}", r.out);
    let r = fx.cca(&a, &["remote", "allow", "laptop", "view"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert!(r.err.contains("「laptop」只能看这台机器上的会话"), "{}", r.err);
    let r = fx.cca(&a, &["remote", "allow", "nobody", "view"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("没有叫「nobody」的配对"), "{}", r.err);
    let r = fx.cca(&a, &["remote"]);
    assert!(r.err.contains("配对过 1 台机器"), "{}", r.err);
    let r = fx.cca(&b, &["remote", "list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(rows_of(&parsed(&r.out)["peers"], &["name"]), [list!["box"]]);
    let r = fx.cca(&b, &["remote", "unpair", "box"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert!(r.err.contains("已经取消和「box」的配对"), "{}", r.err);
    assert_eq!(fx.edges(), obj! {});
    let r = fx.cca_with(&b, &["remote", "join"], Some(&["0000-0000-0000-0000"]), None);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("没有这个配对码"), "{}", r.err);
    assert_eq!(fx.cca_with(&b, &["remote", "join"], Some(&[""]), None).code, 1);
}

fn in_the_app(t: &mut Ctx) {
    let (fx, a, b) = shell_fx(t);
    let hello = parsed(&fx.cca(&a, &["_gui", "hello"]).out);
    assert_eq!(hello.get("remote"), Some(&Value::from(1)));
    // 同意在会话里当场问；答否：按取消收场，不出配对码
    let (mut p, ev) = fx.app_session(&a, &obj! { "consent.remote_pair" => list![obj! { "v" => "n" }] }, &["remote", "pair"], &[]);
    assert_ne!(p.wait(secs(60)), 0);
    let es = relay::events(&ev);
    assert_eq!(field_of(&es, "ask", "q"), ["consent.remote_pair"]);
    assert_eq!(field_of(&es, "fail", "code"), ["cancelled"]);
    // 显示配对码的那一页上取消（App 关掉答案管道）：辅助程序跟着停下——不然它会在后台把配对做完
    let yes = obj! { "consent.remote_pair" => list![obj! { "v" => "y" }] };
    let (mut p, ev) = fx.app_session(&a, &yes, &["remote", "pair"], &["--eof-at", "remote.pair"]);
    assert_ne!(p.wait(secs(60)), 0);
    let es = relay::events(&ev);
    assert_eq!(field_of(&es, "ask", "q"), ["consent.remote_pair", "remote.pair"]);
    assert_eq!(field_of(&es, "fail", "code").last().map(String::as_str), Some("cancelled"));
    let pages: Vec<&Value> = es.iter().filter(|e| e.get("q") == Some(&Value::from("remote.pair"))).collect();
    assert_eq!(pages.len(), 1, "{pages:?}");
    let field = only(&pages[0]["fields"]);
    assert_eq!((field["kind"].str(), field["id"].str()), ("code", "code"));
    let cancelled = field["value"].str();
    // 并行跑的别的测试也起 remote-pair-show：只找这个用例的临时目录里的那个（辅助程序经设备 ~/bin 里的链接起）
    let shows_code = format!("{}/.*remote-pair-show", ere(&fx.d.to_string_lossy()));
    wait_for("辅助程序停下", secs(10), || relay::pgrep(&shows_code).is_empty());
    let t0 = Instant::now();
    let r = fx.cca_with(&b, &["remote", "join"], Some(&[cancelled]), None); // 那个配对码作废了：输进去马上知道，不用干等
    assert_ne!(r.code, 0);
    assert!(r.err.contains("没有这个配对码"), "{}", r.err);
    assert!(t0.elapsed() < secs(20), "{:?}", t0.elapsed());
    assert_eq!((relay::peers(&a), relay::peers(&b), fx.edges()), (obj! {}, obj! {}, obj! {}));
    // 配好：一页显示配对码（不用回答），另一台在它自己的会话里输入；两边各有一条结果
    let table = obj! { "consent.remote_pair" => list![obj! { "v" => "y" }], "remote.pair" => list![Value::Null] };
    let (mut p, ev) = fx.app_session(&a, &table, &["remote", "pair"], &[]);
    let code = wait_for("显示配对码的那一页", secs(30), || {
        let es = relay::events(&ev);
        let page = es.iter().find(|e| e.get("q") == Some(&Value::from("remote.pair")));
        page.map(|e| e["fields"][0]["value"].str().to_string())
    });
    assert!(matches("^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", &code), "{code:?}");
    // 原来没有的一条：上面那个“辅助程序停下”是照这个样子找的，这里它正等着对方，得找得到——不然上面那条什么都没查
    assert!(!relay::pgrep(&shows_code).is_empty(), "照 {shows_code:?} 找不到正在等对方的辅助程序");
    let (mut p2, ev2) =
        fx.app_session(&b, &obj! { "remote.code" => list![obj! { "v" => code.as_str() }] }, &["remote", "join"], &[]);
    let codes = (p2.wait(secs(60)), p.wait(secs(60)));
    assert_eq!(codes, (0, 0), "{:?}", (p2.err_rest(), p.err_rest()));
    for (events_file, other) in [(&ev, "「laptop」"), (&ev2, "「box」")] {
        let es = relay::events(events_file);
        let results: Vec<&Value> = es.iter().filter(|e| e["ev"].str() == "result").collect();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!((results[0]["code"].str(), results[0]["message"].str().contains(other)), ("remote_paired", true));
    }
    let es = relay::events(&ev2);
    let asks: Vec<&Value> = es.iter().filter(|e| e["ev"].str() == "ask").collect();
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!((asks[0]["q"].str(), rows_of(&asks[0]["fields"], &["kind"])), ("remote.code", vec![list!["text"]]));
    assert_eq!(fx.edges().obj().len(), 1);
    // 不用问的命令：做完有结果事件，没做成有原因
    let ev3 = fx.d.join("ev.cmd.jsonl");
    let updated = [list!["result", "remote_updated"]];
    let r = fx.cca_with(&a, &["remote", "allow", "laptop", "none"], None, Some(&ev3));
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(rows_of(&Value::from(relay::events(&ev3)), &["ev", "code"]), updated);
    let r = fx.cca_with(&a, &["remote", "allow", "nobody", "view"], None, Some(&ev3));
    assert_eq!(r.code, 1);
    let es = relay::events(&ev3);
    assert_eq!(es.len(), 1, "{es:?}");
    let fail = &es[0];
    assert_eq!(
        (fail["ev"].str(), fail["code"].str(), fail["message"].str().contains("没有叫「nobody」的配对")),
        ("fail", "error", true)
    );
    for argv in [&["remote", "on"][..], &["remote", "off"], &["remote", "unpair", "laptop"]] {
        let r = fx.cca_with(&a, argv, None, Some(&ev3));
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(rows_of(&Value::from(relay::events(&ev3)), &["ev", "code"]), updated, "{argv:?}");
    }
    // 配对码不对：会话里说清楚原因
    let wrong = obj! { "remote.code" => list![obj! { "v" => "0000-0000-0000-0000" }] };
    let (mut p, ev) = fx.app_session(&b, &wrong, &["remote", "join"], &[]);
    assert_ne!(p.wait(secs(60)), 0);
    let es = relay::events(&ev);
    let fails: Vec<&Value> = es.iter().filter(|e| e["ev"].str() == "fail").collect();
    assert!(fails.last().expect("有一条 fail")["message"].str().contains("没有这个配对码"), "{fails:?}");
}
