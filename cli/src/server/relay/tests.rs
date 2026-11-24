//! relay 的单元测试。两种：
//! - 真的套接字（127.0.0.1 上临时的端口，中转的线程照常跑）：两头各连进来、说开场白、对接、两个方向收发、一头先关；开场白不对时
//!   客户端收到的那几个字节；网页端交进来的那种连接（socketpair）。时限要等的地方不真等：先让连接走到那一步，再把那一项时限
//!   调成 0（`Net::tune`）。
//! - 不起线程、直接调（`bare`）：时限和上限照 Python 的常量一条条核对，时间由用例给。

use super::*;
use crate::dict;
use crate::py::fs::TempDir;
use crate::py::obj::Dict;
use crate::server::limits::Limits;
use crate::server::remote::handle_op;
use std::net::{Shutdown, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

pub(in crate::server) const CN: &str = "0123456789abcdef";
const BLOB: &str = r#"{"n":"AAAAAAAAAAAAAAAAAAAAAA==","c":"AAAA","t":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
const SLOW: &[u8] = b"CCA-RELAY/1 ERR slow\n";

fn fp(i: u32) -> String {
    format!("SHA256:{i:043}")
}

pub(in crate::server) fn hex(n: u32) -> String {
    format!("{n:032x}")
}

fn blob() -> V {
    json::loads(BLOB).unwrap()
}

fn name_of(_: &str) -> Option<String> {
    Some("dev".to_string())
}

/// 客户端连数据端口时说的那一行，和服务器该回的那一行：照抄客户端的 `relay_line`（cli/src/cmd/remote/crypto.rs，那个模块不对外），
/// 不用这边的 `mac`、`ok_line`——两边各算各的，下面拿客户端那边固定的对照数据把两边都核对一遍
pub(in crate::server) fn relay_line(role: &str, ident: &str, key: &[u8], cn: &str) -> (String, String) {
    let mac = |lead: &str| {
        let sum = hash::hmac_sha256(key, format!("{lead}{ident}{cn}").as_bytes());
        base64::urlsafe_b64encode(&sum).trim_end_matches('=').to_string()
    };
    let lead = if role == "join" { "j".to_string() } else { format!("c{role}") };
    (format!("CCA-RELAY/1 {role} {ident} {cn} {}\n", mac(&lead)), format!("CCA-RELAY/1 OK {}\n", mac("s")))
}

/// 等到 f 成立（最多 10 秒）
pub(super) fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(10), "等不到：{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 读到换行为止（带着换行）
pub(in crate::server) fn read_line(s: &mut impl Read) -> Vec<u8> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while line.last() != Some(&b'\n') {
        assert_eq!(s.read(&mut b).unwrap(), 1, "还没读到一整行就断了：{:?}", String::from_utf8_lossy(&line));
        line.push(b[0]);
    }
    line
}

pub(in crate::server) fn read_n(s: &mut impl Read, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).unwrap();
    buf
}

/// 对面关了：读到头（被重置也算）。还有别的字节不算
pub(in crate::server) fn closed(s: &mut impl Read) -> bool {
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(n) => n == 0,
        Err(e) => e.kind() == io::ErrorKind::ConnectionReset,
    }
}

/// 控制连接上读一帧
pub(super) fn read_ctl(s: &mut impl Read) -> (u8, Vec<u8>) {
    let head = read_n(s, 3);
    (head[0], read_n(s, usize::from(u16::from_be_bytes([head[1], head[2]]))))
}

/// 有规律的一大块字节（哪里错了位一比就知道）
pub(in crate::server) fn pattern(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

/// 一个临时目录里的 Remote 和跑着的中转
pub(in crate::server) struct Net {
    _dir: TempDir,
    /// remote.json 在哪
    pub(super) path: String,
    pub(in crate::server) sh: Arc<Shared>,
    pub(in crate::server) handle: Arc<Handle>,
    addr: SocketAddr,
    knobs: Knobs,
}

impl Drop for Net {
    fn drop(&mut self) {
        self.handle.probe.lock().unwrap().stop = true;
        self.handle.wake();
    }
}

impl Net {
    fn new() -> Net {
        Net::with(Limits::DEFAULT, |_| {})
    }

    /// 开着网页端的（front 的测试把整个服务器接在一起时用）：web 是网页端的地址
    pub(in crate::server) fn with_web(web: &str) -> Net {
        Net::open(Limits::DEFAULT, 18444, Some(web.to_string()), |_| {})
    }

    fn with(lim: Limits, tune: impl FnOnce(&mut Knobs)) -> Net {
        Net::open(lim, 18444, None, tune)
    }

    /// port：告诉客户端的数据端口；web：网页端的地址（开着网页端时）。
    /// 时限都是长的（用例里不靠它们到点：要到点的先走到那一步，再用 tune 把那一项调成 0）；只是醒得勤、清得勤
    pub(super) fn open(lim: Limits, port: u16, web: Option<String>, tune: impl FnOnce(&mut Knobs)) -> Net {
        let dir = TempDir::new().unwrap();
        let path = format!("{}/remote.json", dir.path);
        let sh = Arc::new(Shared::new(Remote::open(Path::new(&path), port, web, lim)));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        // 开场白的时限放长：机器忙的时候（别的会话也在编东西）连上、还没来得及说话的不该被当成“太慢”
        let mut knobs = Knobs { poll_ms: 5, sweep_every: 0.01, preamble_wait: 600.0, ..Knobs::DEFAULT };
        tune(&mut knobs);
        let handle = start_with(l, sh.clone(), knobs).unwrap();
        Net { _dir: dir, path, sh, handle, addr, knobs }
    }

    /// 换时限：中转的线程下一轮就用上
    pub(super) fn tune(&mut self, f: impl FnOnce(&mut Knobs)) {
        f(&mut self.knobs);
        self.handle.probe.lock().unwrap().knobs = Some(self.knobs);
        self.handle.wake();
    }

    /// 中转现在管着几条连接（它每一轮记一次：刚连上、刚关掉的要等一下才看得到）
    pub(in crate::server) fn conns(&self) -> usize {
        self.handle.probe.lock().unwrap().conns
    }

    /// 第 dev 台设备发一个请求（时间是现在）
    fn op(&self, op: &str, dev: u32, j: Dict) -> (u16, Dict) {
        self.op_at(op, dev, j, time::time())
    }

    fn op_at(&self, op: &str, dev: u32, j: Dict, now: f64) -> (u16, Dict) {
        let (st, v) = handle_op(&self.sh, op, &fp(dev), &V::Dict(j), now, None, &name_of).unwrap();
        (st, v.as_dict().unwrap())
    }

    /// 设备 a、b 配对：配对编号
    pub(in crate::server) fn pair(&self, a: u32, b: u32, n: u32) -> String {
        let (rid, pid) = (hex(1000 + n), hex(n));
        assert_eq!(self.op("pair_open", a, dict! { "rid" => &rid }).0, 200);
        assert_eq!(self.op("pair_put", b, dict! { "rid" => &rid, "m" => blob() }).0, 200);
        assert_eq!(self.op("pair_commit", a, dict! { "rid" => &rid, "pid" => &pid }).0, 200);
        assert_eq!(self.op("pair_commit", b, dict! { "rid" => &rid, "pid" => &pid }).0, 200);
        pid
    }

    /// 换一张票据：编号和密钥
    fn ticket(&self, op: &str, dev: u32, j: Dict) -> (String, Vec<u8>) {
        let (st, t) = self.op(op, dev, j);
        assert_eq!(st, 200, "{op}");
        (t.str("tid").unwrap().to_string(), base64::b64decode(&t.str("tkey").unwrap(), true).unwrap())
    }

    fn host_ticket(&self, dev: u32, sid: &str, pids: &[&str]) -> (String, Vec<u8>) {
        let pids: Vec<V> = pids.iter().map(V::str).collect();
        self.ticket("relay_host", dev, dict! { "sid" => sid, "pids" => pids })
    }

    pub(in crate::server) fn view_ticket(&self, dev: u32, sid: &str, pid: &str) -> (String, Vec<u8>) {
        self.ticket("relay_attach", dev, dict! { "sid" => sid, "pid" => pid })
    }

    /// 连上数据端口（读写都有时限：出了错是用例失败，不是卡住）
    pub(super) fn dial(&self) -> TcpStream {
        let s = TcpStream::connect(self.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        s
    }

    /// 连上、说开场白：连接，和服务器该回的那一行
    fn enter(&self, role: &str, ident: &str, key: &[u8]) -> (TcpStream, Vec<u8>) {
        let mut s = self.dial();
        let (line, ok) = relay_line(role, ident, key, CN);
        s.write_all(line.as_bytes()).unwrap();
        (s, ok.into_bytes())
    }

    /// 连上、说一句话：服务器回的那一行
    fn say(&self, raw: &[u8]) -> Vec<u8> {
        let mut s = self.dial();
        s.write_all(raw).unwrap();
        let line = read_line(&mut s);
        assert!(closed(&mut s), "回了一行以后该关掉");
        line
    }

    /// 宿主开（或者接回）一个会话的控制连接：连接和那张票据的密钥
    pub(in crate::server) fn host(&self, dev: u32, sid: &str, pids: &[&str]) -> (TcpStream, Vec<u8>) {
        let (tid, key) = self.host_ticket(dev, sid, pids);
        let (mut s, ok) = self.enter("host", &tid, &key);
        assert_eq!(read_line(&mut s), ok);
        (s, key)
    }

    /// 宿主的控制连接上收到 JOIN，拿着令牌另连一条：那一条连接（OK 已经读过）
    pub(in crate::server) fn join(&self, host: &mut TcpStream, key: &[u8], pid: &str) -> TcpStream {
        let (kind, payload) = read_ctl(host);
        assert_eq!(kind, F_JOIN);
        let text = String::from_utf8(payload).unwrap();
        let j = json::loads(&text).unwrap().as_dict().unwrap();
        let token = j.str("token").unwrap();
        assert_eq!(text, format!(r#"{{"token":"{token}","pid":"{pid}"}}"#));
        let (mut s, ok) = self.enter("join", &token, key);
        assert_eq!(read_line(&mut s), ok);
        s
    }

    /// 设备 dev 接入会话 sid：接入方的连接和宿主为它另连的那一条（两头都收到了 OK）
    fn attach(&self, host: &mut TcpStream, key: &[u8], dev: u32, sid: &str, pid: &str) -> (TcpStream, TcpStream) {
        let (tid, vkey) = self.view_ticket(dev, sid, pid);
        let (mut view, ok) = self.enter("view", &tid, &vkey);
        let join = self.join(host, key, pid);
        assert_eq!(read_line(&mut view), ok);
        (view, join)
    }

    /// 设备 dev 看配对 pid 的另一头开着哪些会话
    fn rows(&self, dev: u32, pid: &str) -> Vec<Dict> {
        let (st, j) = self.op("relay_list", dev, dict! { "pids" => vec![V::str(pid)] });
        assert_eq!(st, 200);
        j.dict("sessions").unwrap().list(pid).map_or_else(Vec::new, |l| l.items().iter().map(|x| x.as_dict().unwrap()).collect())
    }

    /// 那个会话现在接着几处、控制连接连着没有；没有这个会话是 None
    fn session(&self, sid: &str) -> Option<(usize, bool)> {
        self.sh.lock().sessions.get(sid).map(|x| (x.viewers, x.conn.is_some()))
    }
}

// ---------------------------------------------------------------- 认证码、帧

/// 开场白和回的那一行：和客户端那边（cli/src/cmd/remote/crypto.rs，照 tests/spec_wire.py 算的对照数据）逐字节一样
#[test]
fn preamble_macs_match_the_client_vectors() {
    let key = [7u8; 32];
    let host = "ab".repeat(16);
    let (line, ok) = relay_line("host", &host, &key, CN);
    assert_eq!(line, format!("CCA-RELAY/1 host {host} {CN} gLe_8CQuzYdrXbCC4GE_-a3pjz7bYwCZ_jvmEMJ1HfY\n"));
    assert_eq!(mac(&key, &[b"chost", host.as_bytes(), CN.as_bytes()]), "gLe_8CQuzYdrXbCC4GE_-a3pjz7bYwCZ_jvmEMJ1HfY");
    assert_eq!(ok_line(&key, &host, CN), ok.as_bytes());
    assert_eq!(ok, "CCA-RELAY/1 OK WM1jPui86RX1yQXMbJMbO9Awmn04Y80TpSgn3DhFQRA\n");
    let token = "cd".repeat(16);
    let (line, ok) = relay_line("join", &token, &key, CN);
    assert_eq!(ok, "CCA-RELAY/1 OK kGt4eo8Pp0W6i0ECgylnJCowEO__254sY0NtEEhgCRU\n");
    assert_eq!(line, format!("CCA-RELAY/1 join {token} {CN} bfdoPmRWMZnFPMOjFkxZv-H6gfMKherzxOgNV6krD6s\n"));
    assert_eq!(mac(&key, &[b"j", token.as_bytes(), CN.as_bytes()]), "bfdoPmRWMZnFPMOjFkxZv-H6gfMKherzxOgNV6krD6s");
    assert_eq!(ok_line(&key, &token, CN), b"CCA-RELAY/1 OK kGt4eo8Pp0W6i0ECgylnJCowEO__254sY0NtEEhgCRU\n");
    // 接入方的那一行开头是 cview
    let (line, _) = relay_line("view", &host, &key, CN);
    assert!(line.ends_with(&format!(" {}\n", mac(&key, &[b"cview", host.as_bytes(), CN.as_bytes()]))));
    // 帧：类型、两个字节的长度（高位在前）、内容
    assert_eq!(frame(F_JOIN, b"ab"), [0x02, 0, 2, b'a', b'b']);
    assert_eq!(frame(F_PONG, b""), [0x07, 0, 0]);
    assert_eq!(frame(F_PONG, &[1u8; 300])[..3], [0x07, 1, 44]);
}

/// 编号、密钥都定好的票据：客户端那边固定的那一行发过来，回的就是固定的那一行
#[test]
fn the_frozen_host_line_gets_the_frozen_answer() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let (tid, sid) = ("ab".repeat(16), hex(50));
    net.sh.lock().test_ticket(&tid, &[7u8; 32], &fp(1), &sid, &[&pid], time::time() + 30.0);
    let mut s = net.dial();
    s.write_all(format!("CCA-RELAY/1 host {tid} {CN} gLe_8CQuzYdrXbCC4GE_-a3pjz7bYwCZ_jvmEMJ1HfY\n").as_bytes()).unwrap();
    assert_eq!(read_line(&mut s), b"CCA-RELAY/1 OK WM1jPui86RX1yQXMbJMbO9Awmn04Y80TpSgn3DhFQRA\n");
    wait_for("会话开着", || net.session(&sid) == Some((0, true)));
}

// ---------------------------------------------------------------- 两头对接

#[test]
fn host_view_join_and_the_pipe() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    // 宿主说这个会话的说明：配对的另一头看得到
    host.write_all(&frame(F_ANNOUNCE, format!(r#"{{"{pid}":{BLOB}}}"#).as_bytes())).unwrap();
    wait_for("另一头看到会话", || net.rows(2, &pid).len() == 1);
    let row = &net.rows(2, &pid)[0];
    assert_eq!(
        json::dumps(&V::Dict(row.clone()), Fmt::new().compact()),
        format!(r#"{{"sid":"{sid}","blob":{BLOB},"age":0,"viewers":0,"online":true}}"#)
    );
    // 接入：宿主收到 JOIN、另连一条，两头各收到自己的 OK
    let (mut view, mut join) = net.attach(&mut host, &key, 2, &sid, &pid);
    wait_for("算一处接入", || net.session(&sid) == Some((1, true)));
    // 两个方向原样转
    view.write_all(b"keys from the viewer").unwrap();
    assert_eq!(read_n(&mut join, 20), b"keys from the viewer");
    join.write_all(b"screen from the host").unwrap();
    assert_eq!(read_n(&mut view, 20), b"screen from the host");
    for (i, n) in [1usize, 2, 65535, 65536, 65537, 300_000].into_iter().enumerate() {
        let (up, down) = (pattern(n, i as u32), pattern(n, 100 + i as u32));
        view.write_all(&up).unwrap();
        join.write_all(&down).unwrap();
        assert!(read_n(&mut join, n) == up && read_n(&mut view, n) == down, "{n}");
    }
    // 同一个会话再接一处（另一条 join）；控制连接照常
    let (mut view2, mut join2) = net.attach(&mut host, &key, 2, &sid, &pid);
    wait_for("两处接入", || net.session(&sid) == Some((2, true)));
    view2.write_all(b"second").unwrap();
    assert_eq!(read_n(&mut join2, 6), b"second");
    host.write_all(&frame(F_PING, b"12345678")).unwrap();
    assert_eq!(read_ctl(&mut host), (F_PONG, b"12345678".to_vec()));
    // 接入方说完最后一句、先关写的那一半：宿主那一条读到这一句，然后读到头；这一处接入算结束
    view.write_all(b"bye").unwrap();
    view.shutdown(Shutdown::Write).unwrap();
    assert_eq!(read_n(&mut join, 3), b"bye");
    assert!(closed(&mut join) && closed(&mut view));
    wait_for("剩一处接入", || net.session(&sid) == Some((1, true)));
    // 宿主那一条先关：接入方读到头
    drop(join2);
    assert!(closed(&mut view2));
    wait_for("没有接入了", || net.session(&sid) == Some((0, true)));
}

#[test]
fn big_transfers_with_a_slow_reader() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    let (view, mut join) = net.attach(&mut host, &key, 2, &sid, &pid);
    // 一头一口气写 3 MB，另一头过一会儿才开始读：中转攒到上限就先不读写的那一头，一个字节不丢、不乱
    let data = pattern(3 << 20, 7);
    let (mut w, sent) = (view.try_clone().unwrap(), data.clone());
    let writer = std::thread::spawn(move || w.write_all(&sent).unwrap());
    std::thread::sleep(Duration::from_millis(60));
    assert!(read_n(&mut join, data.len()) == data);
    writer.join().unwrap();
    // 两个方向同时
    let (up, down) = (pattern(1 << 20, 8), pattern(1 << 20, 9));
    let (mut w1, mut w2, up2, down2) = (view.try_clone().unwrap(), join.try_clone().unwrap(), up.clone(), down.clone());
    let (t1, t2) =
        (std::thread::spawn(move || w1.write_all(&up2).unwrap()), std::thread::spawn(move || w2.write_all(&down2).unwrap()));
    let mut view = view;
    let reader = std::thread::spawn(move || read_n(&mut view, 1 << 20));
    assert!(read_n(&mut join, 1 << 20) == up);
    assert!(reader.join().unwrap() == down);
    t1.join().unwrap();
    t2.join().unwrap();
}

#[test]
fn closing_one_end_flushes_the_other() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    let (mut view, mut join) = net.attach(&mut host, &key, 2, &sid, &pid);
    // 宿主那一条写完最后一大段马上关：接入方照样全收到，然后才读到头
    let last = pattern(400_000, 3);
    join.write_all(&last).unwrap();
    drop(join);
    assert!(read_n(&mut view, last.len()) == last);
    assert!(closed(&mut view));
    wait_for("这一处接入算结束", || net.session(&sid) == Some((0, true)));
}

// ---------------------------------------------------------------- 开场白不对

#[test]
fn bad_preambles_get_one_line_and_the_door() {
    let net = Net::new();
    let (ident, cn, mac) = ("ab".repeat(16), CN, "A".repeat(43));
    let bad = b"CCA-RELAY/1 ERR bad\n";
    let lines = [
        "hello\n".to_string(),
        "\n".to_string(),
        "GET / HTTP/1.1\r\n".to_string(),
        format!("CCA-RELAY/1 host {ident} {cn} {mac}\r\n"),
        format!("CCA-RELAY/2 host {ident} {cn} {mac}\n"),
        format!("CCA-RELAY/1 peek {ident} {cn} {mac}\n"),
        format!("CCA-RELAY/1 host {} {cn} {mac}\n", "AB".repeat(16)),
        format!("CCA-RELAY/1 host {ident} {cn} {}\n", "A".repeat(42)),
        format!("CCA-RELAY/1 host {ident} {cn} {}\n", "A".repeat(44)),
        format!("CCA-RELAY/1 host {ident} {cn}x {mac}\n"),
        format!("CCA-RELAY/1 host  {ident} {cn} {mac}\n"),
        format!(" CCA-RELAY/1 host {ident} {cn} {mac}\n"),
        format!("CCA-RELAY/1 host {ident} {cn} {mac} \n"),
        format!("CCA-RELAY/1 host {ident} {cn} {}é\n", "A".repeat(42)),
        "x".repeat(257),
        "x".repeat(4000) + "\n",
    ];
    for line in &lines {
        assert_eq!(net.say(line.as_bytes()), bad, "{line:?}");
    }
    assert_eq!(net.say(b"\xff\xfe\n"), bad);
    // 256 个字节还没有换行：接着等；来了换行才看这一行
    let mut s = net.dial();
    s.write_all(&[b'x'; 256]).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    s.write_all(b"\n").unwrap();
    assert_eq!(read_line(&mut s), bad);
    assert!(closed(&mut s));
    // 样子对、票据没有：ticket
    assert_eq!(net.say(format!("CCA-RELAY/1 host {ident} {cn} {mac}\n").as_bytes()), b"CCA-RELAY/1 ERR ticket\n");
    assert_eq!(net.say(format!("CCA-RELAY/1 view {ident} {cn} {mac}\n").as_bytes()), b"CCA-RELAY/1 ERR ticket\n");
    assert_eq!(net.say(format!("CCA-RELAY/1 join {ident} {cn} {mac}\n").as_bytes()), b"CCA-RELAY/1 ERR ticket\n");
    // 这些都不耽误别的连接
    let pid = net.pair(1, 2, 1);
    let (mut host, _) = net.host(1, &hex(50), &[&pid]);
    host.write_all(&frame(F_PING, b"")).unwrap();
    assert_eq!(read_ctl(&mut host), (F_PONG, Vec::new()));
}

#[test]
fn tickets_at_the_door() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let ticket = b"CCA-RELAY/1 ERR ticket\n";
    let refused = |role: &str, ident: &str, key: &[u8]| {
        let (mut s, _) = net.enter(role, ident, key);
        let line = read_line(&mut s);
        assert!(closed(&mut s));
        line
    };
    // 证明不了有密钥的、拿宿主的票据当接入用的：不行，票据也不算用掉
    let (tid, key) = net.host_ticket(1, &sid, &[&pid]);
    assert_eq!(refused("host", &tid, &[9u8; 32]), ticket);
    assert_eq!(refused("view", &tid, &key), ticket);
    assert_eq!(refused("join", &tid, &key), ticket);
    let (mut host, ok) = net.enter("host", &tid, &key);
    assert_eq!(read_line(&mut host), ok);
    // 一张只能用一次
    assert_eq!(refused("host", &tid, &key), ticket);
    // 接入用的票据也一样；拿它当宿主的不行
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    assert_eq!(refused("view", &vt, &[9u8; 32]), ticket);
    assert_eq!(refused("host", &vt, &vkey), ticket);
    let (mut view, vok) = net.enter("view", &vt, &vkey);
    let _join = net.join(&mut host, &key, &pid);
    assert_eq!(read_line(&mut view), vok);
    // 过期的（30 秒）
    let (st, t) = net.op_at("relay_host", 1, dict! { "sid" => hex(51), "pids" => vec![V::str(&pid)] }, time::time() - 31.0);
    assert_eq!(st, 200);
    let old = (t.str("tid").unwrap(), base64::b64decode(&t.str("tkey").unwrap(), true).unwrap());
    assert_eq!(refused("host", &old.0, &old.1), ticket);
    assert_eq!(net.session(&hex(51)), None);
}

#[test]
fn a_viewer_needs_a_connected_host() {
    let net = Net::new();
    let (p1, p2) = (net.pair(1, 2, 1), net.pair(1, 3, 2));
    let sid = hex(50);
    let nohost = b"CCA-RELAY/1 ERR nohost\n";
    let (host, _) = net.host(1, &sid, &[&p1, &p2]);
    let (t1, k1) = net.view_ticket(2, &sid, &p1);
    let (t2, k2) = net.view_ticket(3, &sid, &p2);
    // 宿主接回来时不再给 p2 看：p2 的那张接入的票据进不来
    let (host2, _) = net.host(1, &sid, &[&p1]);
    let mut host = host;
    assert!(closed(&mut host), "旧的控制连接被顶掉");
    let (mut s, _) = net.enter("view", &t2, &k2);
    assert_eq!(read_line(&mut s), nohost);
    assert!(closed(&mut s));
    // 控制连接断了（会话还留着等它重连）：接入的进不来
    host2.shutdown(Shutdown::Both).unwrap();
    wait_for("控制连接断了", || net.session(&sid) == Some((0, false)));
    let (mut s, _) = net.enter("view", &t1, &k1);
    assert_eq!(read_line(&mut s), nohost);
    assert!(closed(&mut s));
}

#[test]
fn sessions_are_checked_again_at_the_door() {
    // 两台设备都在会话还没开的时候换了同一个会话编号的票据：先连上的得到它，后来的是 taken
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (t1, k1) = net.host_ticket(1, &sid, &[&pid]);
    let (t2, k2) = net.host_ticket(2, &sid, &[&pid]);
    let (mut second, ok) = net.enter("host", &t2, &k2);
    assert_eq!(read_line(&mut second), ok);
    let (mut first, _) = net.enter("host", &t1, &k1);
    assert_eq!(read_line(&mut first), b"CCA-RELAY/1 ERR taken\n");
    assert!(closed(&mut first));
    second.write_all(&frame(F_PING, b"still here")).unwrap();
    assert_eq!(read_ctl(&mut second), (F_PONG, b"still here".to_vec()));
    assert_eq!(net.sh.lock().sessions.get(&sid).unwrap().fp, fp(2));
    // 攒着票据一起连上来：超过一台设备能开的会话数的在这里拦住
    let net = Net::with(Limits { sessions_dev: 1, ..Limits::DEFAULT }, |_| {});
    let pid = net.pair(1, 2, 1);
    let ((t1, k1), (t2, k2)) = (net.host_ticket(1, &hex(50), &[&pid]), net.host_ticket(1, &hex(51), &[&pid]));
    let (mut a, ok) = net.enter("host", &t1, &k1);
    assert_eq!(read_line(&mut a), ok);
    let (mut b, _) = net.enter("host", &t2, &k2);
    assert_eq!(read_line(&mut b), b"CCA-RELAY/1 ERR sessions\n");
    assert!(closed(&mut b));
    assert_eq!((net.session(&hex(50)), net.session(&hex(51))), (Some((0, true)), None));
}

#[test]
fn join_needs_the_token_and_the_control_key() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let ticket = b"CCA-RELAY/1 ERR ticket\n";
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    let (mut view, vok) = net.enter("view", &vt, &vkey);
    let (kind, payload) = read_ctl(&mut host);
    assert_eq!(kind, F_JOIN);
    let token = json::loads(std::str::from_utf8(&payload).unwrap()).unwrap().as_dict().unwrap().str("token").unwrap();
    // 光知道令牌、没有控制连接那张票据的密钥：进不来，等着的接入方不受影响
    for key in [&vkey[..], &[9u8; 32]] {
        let (mut s, _) = net.enter("join", &token, key);
        assert_eq!(read_line(&mut s), ticket);
        assert!(closed(&mut s));
    }
    let (mut join, ok) = net.enter("join", &token, &key);
    assert_eq!((read_line(&mut join), read_line(&mut view)), (ok, vok));
    join.write_all(b"!").unwrap();
    assert_eq!(read_n(&mut view, 1), b"!");
    // 令牌只能用一次
    let (mut s, _) = net.enter("join", &token, &key);
    assert_eq!(read_line(&mut s), ticket);
    assert!(closed(&mut s));
}

#[test]
fn nothing_may_follow_the_preamble_before_ok() {
    let net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    // 接入方的开场白后面紧跟着别的话：不回话就关
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    let mut view = net.dial();
    view.write_all(format!("{}extra", relay_line("view", &vt, &vkey, CN).0).as_bytes()).unwrap();
    assert!(closed(&mut view));
    assert_eq!(read_ctl(&mut host).0, F_JOIN, "JOIN 已经发给宿主了");
    // 宿主另连的那一条也一样：它一关，等着的接入方先收到那句 OK（已经排上了），然后读到头
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    let (mut view, vok) = net.enter("view", &vt, &vkey);
    let (_, payload) = read_ctl(&mut host);
    let token = json::loads(std::str::from_utf8(&payload).unwrap()).unwrap().as_dict().unwrap().str("token").unwrap();
    let mut join = net.dial();
    join.write_all(format!("{}extra", relay_line("join", &token, &key, CN).0).as_bytes()).unwrap();
    assert_eq!(read_line(&mut view), vok);
    assert!(closed(&mut view));
    wait_for("不算接入", || net.session(&sid) == Some((0, true)));
    // 控制连接的开场白后面紧跟着帧：照常处理
    let (tid, key) = net.host_ticket(1, &hex(51), &[&pid]);
    let mut host = net.dial();
    let (line, ok) = relay_line("host", &tid, &key, CN);
    host.write_all(&[line.as_bytes(), &frame(F_PING, b"at once")].concat()).unwrap();
    assert_eq!(read_line(&mut host), ok.as_bytes());
    assert_eq!(read_ctl(&mut host), (F_PONG, b"at once".to_vec()));
}

// ---------------------------------------------------------------- 控制连接

#[test]
fn control_frames() {
    let net = Net::new();
    let (p1, p2) = (net.pair(1, 2, 1), net.pair(1, 3, 2));
    let sid = hex(50);
    let (mut host, _) = net.host(1, &sid, &[&p1]);
    // PING：原样回前 16 个字节；不认识的帧不理；一帧分几次到、几帧一次到都行
    host.write_all(&frame(F_PING, &[b'p'; 20])).unwrap();
    assert_eq!(read_ctl(&mut host), (F_PONG, vec![b'p'; 16]));
    host.write_all(
        &[frame(0x55, b"what"), frame(F_JOIN, b""), frame(F_PONG, b"x"), frame(F_PING, b"a"), frame(F_PING, b"b")].concat(),
    )
    .unwrap();
    assert_eq!((read_ctl(&mut host), read_ctl(&mut host)), ((F_PONG, b"a".to_vec()), (F_PONG, b"b".to_vec())));
    let ping = frame(F_PING, b"slowly");
    for part in ping.chunks(2) {
        host.write_all(part).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(read_ctl(&mut host), (F_PONG, b"slowly".to_vec()));
    // ANNOUNCE：整份换掉；不是这个会话给看的配对的不要（哪怕它那一份不合格）
    host.write_all(&frame(F_ANNOUNCE, format!(r#"{{"{p2}":"junk","{p1}":{BLOB},"zz":1}}"#).as_bytes())).unwrap();
    wait_for("看到会话", || net.rows(2, &p1).len() == 1);
    assert!(net.rows(3, &p2).is_empty());
    host.write_all(&frame(F_ANNOUNCE, b"{}")).unwrap();
    wait_for("说明没了", || net.rows(2, &p1).is_empty());
    assert_eq!(net.session(&sid), Some((0, true)));
    // 最长的一帧（32 KB）收得下：里面的说明太长（4096 个字符以内才行）就断开
    let long = format!(r#"{{"{p1}":{{"n":"{}==","c":"{}","t":"{}="}}}}"#, "A".repeat(22), "A".repeat(4096), "A".repeat(43));
    host.write_all(&frame(F_ANNOUNCE, long.as_bytes())).unwrap();
    wait_for("看到会话", || net.rows(2, &p1).len() == 1);
    // 不对的：断开这一条（会话留着等它重连）
    let over = format!(r#"{{"{p1}":{{"n":"{}==","c":"{}","t":"{}="}}}}"#, "A".repeat(22), "A".repeat(4100), "A".repeat(43));
    let deep = format!(r#"{{"x":{}1{}}}"#, "[".repeat(5000), "]".repeat(5000));
    let bad: [Vec<u8>; 8] = [
        frame(F_ANNOUNCE, over.as_bytes()),
        frame(F_ANNOUNCE, format!(r#"{{"{p1}":{{"n":"x"}}}}"#).as_bytes()),
        frame(F_ANNOUNCE, format!(r#"{{"{p1}":null}}"#).as_bytes()),
        frame(F_ANNOUNCE, b"[]"),
        frame(F_ANNOUNCE, b"{"),
        frame(F_ANNOUNCE, b"\xff"),
        frame(F_ANNOUNCE, deep.as_bytes()),
        vec![F_PING, 0x80, 0x01],
    ];
    for raw in &bad {
        host.write_all(raw).unwrap();
        assert!(closed(&mut host), "{:?}", String::from_utf8_lossy(&raw[..raw.len().min(40)]));
        wait_for("控制连接断了", || net.session(&sid) == Some((0, false)));
        assert_eq!(net.rows(2, &p1).len(), 1, "原来的说明还在");
        host = net.host(1, &sid, &[&p1]).0;
        wait_for("接回来了", || net.session(&sid) == Some((0, true)));
    }
    // 正好 32 KB 的一帧不算太长（里面不是 JSON：照样断开，但不是因为长度）；多一个字节的只看头就断开，上面试过了
    // BYE：会话结束，马上没有（不等重连）
    host.write_all(&frame(F_BYE, b"")).unwrap();
    assert!(closed(&mut host));
    wait_for("会话没了", || net.session(&sid).is_none());
    assert!(net.rows(2, &p1).is_empty());
    assert_eq!(net.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &p1 }).0, 404);
}

#[test]
fn a_new_control_connection_replaces_the_old_one() {
    let net = Net::new();
    let (p1, p2) = (net.pair(1, 2, 1), net.pair(1, 3, 2));
    let sid = hex(50);
    let (mut old, key) = net.host(1, &sid, &[&p1, &p2]);
    old.write_all(&frame(F_ANNOUNCE, format!(r#"{{"{p1}":{BLOB},"{p2}":{BLOB}}}"#).as_bytes())).unwrap();
    wait_for("两个配对都看到", || net.rows(2, &p1).len() == 1 && net.rows(3, &p2).len() == 1);
    let (mut view, mut join) = net.attach(&mut old, &key, 2, &sid, &p1);
    // 接回来（这次只给 p1 看）：旧的那一条被关掉，会话一直开着；不再给看的配对的说明拿掉；正接着的不受影响
    let (mut new, _) = net.host(1, &sid, &[&p1]);
    assert!(closed(&mut old));
    assert_eq!(net.session(&sid), Some((1, true)));
    assert!(net.rows(2, &p1).len() == 1 && net.rows(3, &p2).is_empty());
    assert_eq!(net.sh.lock().sessions.get(&sid).unwrap().pids, std::slice::from_ref(&p1));
    view.write_all(b"still attached").unwrap();
    assert_eq!(read_n(&mut join, 14), b"still attached");
    new.write_all(&frame(F_PING, b"new")).unwrap();
    assert_eq!(read_ctl(&mut new), (F_PONG, b"new".to_vec()));
    // 给新的控制连接的 JOIN 用的是新票据的密钥：拿旧的那把进不来
    let (vt, vkey) = net.view_ticket(2, &sid, &p1);
    let (_view2, _) = net.enter("view", &vt, &vkey);
    let (_, payload) = read_ctl(&mut new);
    let token = json::loads(std::str::from_utf8(&payload).unwrap()).unwrap().as_dict().unwrap().str("token").unwrap();
    let (mut s, _) = net.enter("join", &token, &key);
    assert_eq!(read_line(&mut s), b"CCA-RELAY/1 ERR ticket\n");
}

#[test]
fn a_lost_control_connection_keeps_the_session_for_a_while() {
    let mut net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, _) = net.host(1, &sid, &[&pid]);
    host.write_all(&frame(F_ANNOUNCE, format!(r#"{{"{pid}":{BLOB}}}"#).as_bytes())).unwrap();
    wait_for("看到会话", || net.rows(2, &pid).len() == 1);
    drop(host);
    // 断了：另一头看到它不在线，会话还在；接回来又在线
    wait_for("不在线", || net.rows(2, &pid).first().is_some_and(|x| x.get("online").unwrap().as_bool() == Some(false)));
    let (host, _) = net.host(1, &sid, &[&pid]);
    wait_for("在线", || net.rows(2, &pid).first().is_some_and(|x| x.get("online").unwrap().as_bool() == Some(true)));
    // 再断，过了留着的时间：会话没了
    drop(host);
    wait_for("不在线", || net.session(&sid) == Some((0, false)));
    net.tune(|k| k.grace = 0.0);
    wait_for("会话没了", || net.session(&sid).is_none());
}

// ---------------------------------------------------------------- 时限（先走到那一步，再把那一项调成 0）

#[test]
fn slow_and_idle_connections_are_closed() {
    let mut net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    let (mut view, mut join) = net.attach(&mut host, &key, 2, &sid, &pid);
    // 等着宿主那一头的接入方：到点回 nohost，令牌作废
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    let (mut waiting, _) = net.enter("view", &vt, &vkey);
    let (_, payload) = read_ctl(&mut host);
    let token = json::loads(std::str::from_utf8(&payload).unwrap()).unwrap().as_dict().unwrap().str("token").unwrap();
    net.tune(|k| k.join_wait = -k.preamble_wait);
    assert_eq!(read_line(&mut waiting), b"CCA-RELAY/1 ERR nohost\n");
    assert!(closed(&mut waiting));
    let (mut late, _) = net.enter("join", &token, &key);
    assert_eq!(read_line(&mut late), b"CCA-RELAY/1 ERR ticket\n");
    // 连上不说话的、开场白说了一半的：到点回 slow
    let mut silent = net.dial();
    let mut half = net.dial();
    half.write_all(b"CCA-RELAY/1 host").unwrap();
    std::thread::sleep(Duration::from_millis(30));
    net.tune(|k| k.preamble_wait = 0.0);
    assert_eq!((read_line(&mut silent), read_line(&mut half)), (SLOW.to_vec(), SLOW.to_vec()));
    assert!(closed(&mut silent) && closed(&mut half));
    // 进来了的不受这一项管
    view.write_all(b"x").unwrap();
    assert_eq!(read_n(&mut join, 1), b"x");
    host.write_all(&frame(F_PING, b"")).unwrap();
    assert_eq!(read_ctl(&mut host).0, F_PONG);
    // 太久一个字节都没有的：控制连接、对接着的都断开
    net.tune(|k| k.idle = 0.0);
    assert!(closed(&mut host) && closed(&mut view) && closed(&mut join));
    wait_for("都断了", || net.session(&sid) == Some((0, false)));
}

#[test]
fn dropped_pairings_and_removed_devices_cut_their_connections() {
    let net = Net::new();
    let (p1, p2) = (net.pair(1, 2, 1), net.pair(1, 3, 2));
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&p1, &p2]);
    let (mut view, mut join) = net.attach(&mut host, &key, 2, &sid, &p1);
    let (mut view3, mut join3) = net.attach(&mut host, &key, 3, &sid, &p2);
    let (vt, vkey) = net.view_ticket(2, &sid, &p1);
    let (mut waiting, _) = net.enter("view", &vt, &vkey);
    assert_eq!(read_ctl(&mut host).0, F_JOIN);
    // 配对取消了：这个配对正接着的、等着接的都断开（不回话）；别的配对的照旧
    assert_eq!(net.op("pair_drop", 2, dict! { "pid" => &p1 }).0, 200);
    assert!(closed(&mut view) && closed(&mut join) && closed(&mut waiting));
    view3.write_all(b"3").unwrap();
    assert_eq!(read_n(&mut join3, 1), b"3");
    host.write_all(&frame(F_PING, b"")).unwrap();
    assert_eq!(read_ctl(&mut host).0, F_PONG);
    wait_for("剩一处", || net.session(&sid) == Some((1, true)));
    // 宿主那台设备被移出了名单：会话没了，控制连接断开；它的配对没了，正接着的也断开
    net.sh.lock().keep_only(&[fp(2), fp(3)], time::time()).unwrap();
    assert!(closed(&mut host) && closed(&mut view3) && closed(&mut join3));
    assert_eq!(net.session(&sid), None);
}

// ---------------------------------------------------------------- 网页端交进来的连接、连接数

#[test]
fn adopted_connections_are_treated_alike() {
    let mut net = Net::new();
    let pid = net.pair(1, 2, 1);
    let sid = hex(50);
    let (mut host, key) = net.host(1, &sid, &[&pid]);
    let pair = || {
        let (a, b) = UnixStream::pair().unwrap();
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        b.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        (a, b)
    };
    // 开场白不对：一样回一行就关
    let (a, mut b) = pair();
    net.handle.adopt(a, "198.51.100.7".to_string());
    b.write_all(b"hello\n").unwrap();
    assert_eq!(read_line(&mut b), b"CCA-RELAY/1 ERR bad\n");
    assert!(closed(&mut b));
    // 接入：和数据端口上进来的一样对接、收发
    let (vt, vkey) = net.view_ticket(2, &sid, &pid);
    let (a, mut view) = pair();
    net.handle.adopt(a, "198.51.100.7".to_string());
    let (line, ok) = relay_line("view", &vt, &vkey, CN);
    view.write_all(line.as_bytes()).unwrap();
    let mut join = net.join(&mut host, &key, &pid);
    assert_eq!(read_line(&mut view), ok.as_bytes());
    let (up, down) = (pattern(500_000, 1), pattern(500_000, 2));
    let (mut w, up2) = (view.try_clone().unwrap(), up.clone());
    let writer = std::thread::spawn(move || w.write_all(&up2).unwrap());
    assert!(read_n(&mut join, up.len()) == up);
    writer.join().unwrap();
    join.write_all(&down).unwrap();
    assert!(read_n(&mut view, down.len()) == down);
    drop(join);
    assert!(closed(&mut view));
    // 几条一起交进来：一条不落；不说话的到点一样回 slow
    let mut many: Vec<UnixStream> = (0..5)
        .map(|_| {
            let (a, b) = pair();
            net.handle.adopt(a, "198.51.100.7".to_string());
            b
        })
        .collect();
    std::thread::sleep(Duration::from_millis(30));
    net.tune(|k| k.preamble_wait = 0.0);
    for b in &mut many {
        assert_eq!(read_line(b), SLOW);
        assert!(closed(b));
    }
}

#[test]
fn connection_limits() {
    // 一共最多几条：多了的直接关掉（不回话）；腾出来以后又进得来
    let net = Net::with(Limits::DEFAULT, |k| k.conn_max = 2);
    let (a, b) = (net.dial(), net.dial());
    let mut c = net.dial();
    assert!(closed(&mut c));
    drop(a);
    wait_for("腾出一条", || {
        let mut s = net.dial();
        s.write_all(b"x\n").unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).is_ok() && got == b"CCA-RELAY/1 ERR bad\n"
    });
    drop(b);
    // 同一个 IP 最多几条；交进来的照它带的 IP 算，互不影响
    let net = Net::with(Limits::DEFAULT, |k| k.conn_ip = 1);
    let _a = net.dial();
    let mut b = net.dial();
    assert!(closed(&mut b));
    let adopt = |ip: &str| {
        let (a, b) = UnixStream::pair().unwrap();
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        net.handle.adopt(a, ip.to_string());
        b
    };
    let (mut x, mut y, mut z) = (adopt("198.51.100.7"), adopt("198.51.100.7"), adopt("198.51.100.8"));
    assert!(closed(&mut y));
    for s in [&mut x, &mut z] {
        s.write_all(b"x\n").unwrap();
        assert_eq!(read_line(s), b"CCA-RELAY/1 ERR bad\n");
    }
    // 它关了以后那个 IP 又进得来
    wait_for("那个 IP 腾出来", || {
        let mut s = adopt("198.51.100.7");
        s.write_all(b"x\n").unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).is_ok() && got == b"CCA-RELAY/1 ERR bad\n"
    });
}

// ---------------------------------------------------------------- 不起线程、直接调：时限和上限照 Python 的常量

/// 没起线程的中转，和它用的 Remote
fn bare() -> (Relay, Arc<Shared>, TempDir) {
    let dir = TempDir::new().unwrap();
    let sh = Arc::new(Shared::new(Remote::open(Path::new(&format!("{}/remote.json", dir.path)), 18444, None, Limits::DEFAULT)));
    let (wake_r, wake_w) = UnixStream::pair().unwrap();
    let handle = Arc::new(Handle { adopted: Mutex::new(Vec::new()), wake_w, probe: Mutex::new(Probe::default()) });
    let relay = Relay {
        lsock: TcpListener::bind("127.0.0.1:0").unwrap(),
        handle,
        wake_r,
        conns: HashMap::new(),
        last_id: 0,
        per_ip: HashMap::new(),
        waiting: HashMap::new(),
        knobs: Knobs::DEFAULT,
        scratch: vec![0; CHUNK],
    };
    (relay, sh, dir)
}

impl Relay {
    /// 收下一条连接：它的编号和测试手里的那一头
    fn plug(&mut self, ip: &str, now: f64) -> (u64, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        self.admit(Sock::Unix(a), ip.to_string(), now);
        (self.last_id, b)
    }

    /// 两条连接对接上（不经开场白）
    fn splice(&mut self, a: u64, b: u64) {
        for (x, y) in [(a, b), (b, a)] {
            let c = self.conns.get_mut(&x).unwrap();
            (c.peer, c.state) = (Some(y), State::Pipe);
        }
    }
}

#[test]
fn knobs_are_the_python_constants() {
    let k = Knobs::DEFAULT;
    assert_eq!((k.conn_max, k.conn_ip, k.queue), (256, 32, 256 * 1024));
    assert_eq!((k.idle, k.grace, k.preamble_wait, k.join_wait, k.drain_wait), (90.0, 60.0, 5.0, 10.0, 5.0));
    assert_eq!((k.poll_ms, k.sweep_every, CHUNK), (1000, 1.0, 65536));
}

#[test]
fn sweep_follows_the_python_time_limits() {
    let (mut relay, sh, _dir) = bare();
    let mut lz = Lazy::new(&sh);
    // 没说完开场白的：5 秒整还在，过了就回 slow
    let (pre, mut pre_end) = relay.plug("192.0.2.1", 100.0);
    relay.sweep(&mut lz, 105.0);
    assert!(relay.conns.contains_key(&pre));
    relay.sweep(&mut lz, 105.001);
    assert!(!relay.conns.contains_key(&pre));
    assert_eq!(read_line(&mut pre_end), SLOW);
    assert!(closed(&mut pre_end));
    // 等宿主的接入方：从连上算起 15 秒（5 + 10）
    let (waiting, mut waiting_end) = relay.plug("192.0.2.1", 100.0);
    relay.conns.get_mut(&waiting).unwrap().state = State::Wait;
    relay.conns.get_mut(&waiting).unwrap().pid = Some(hex(1));
    relay.waiting.insert(hex(9), (waiting, vec![1; 32]));
    lz.get().test_edge(&hex(1), &fp(1), &fp(2), 0.0);
    relay.sweep(&mut lz, 115.0);
    assert!(relay.conns.contains_key(&waiting) && relay.waiting.len() == 1);
    relay.sweep(&mut lz, 115.001);
    assert!(!relay.conns.contains_key(&waiting) && relay.waiting.is_empty());
    assert_eq!(read_line(&mut waiting_end), b"CCA-RELAY/1 ERR nohost\n");
    // 控制连接、对接着的：90 秒没有一个字节
    lz.get().test_session(&hex(50), &fp(1), &[&hex(1)], 0.0);
    let (ctl, mut ctl_end) = relay.plug("192.0.2.1", 100.0);
    let (a, mut a_end) = relay.plug("192.0.2.2", 100.0);
    let (b, mut b_end) = relay.plug("192.0.2.2", 100.0);
    relay.splice(a, b);
    for id in [a, b] {
        relay.conns.get_mut(&id).unwrap().pid = Some(hex(1));
    }
    let c = relay.conns.get_mut(&ctl).unwrap();
    (c.state, c.sid) = (State::Ctl, Some(hex(50)));
    lz.get().sessions.get_mut(&hex(50)).unwrap().conn = Some(ctl);
    relay.conns.get_mut(&b).unwrap().seen = 150.0;
    relay.sweep(&mut lz, 190.0);
    assert_eq!(relay.conns.len(), 3);
    relay.sweep(&mut lz, 190.001);
    // 控制连接断了：会话留着（记下什么时候断的）；对接的一对里一头到点，两头都关（另一头没有要写的）
    assert!(relay.conns.is_empty() && relay.per_ip.is_empty());
    assert!(closed(&mut ctl_end) && closed(&mut a_end) && closed(&mut b_end));
    let x = lz.get().sessions.get(&hex(50)).unwrap().clone();
    assert_eq!((x.conn, x.lost), (None, Some(190.001)));
    // 会话：控制连接断了 60 秒整还在，过了就没了
    relay.sweep(&mut lz, 250.001);
    assert!(lz.get().sessions.contains(&hex(50)));
    relay.sweep(&mut lz, 250.002);
    assert!(!lz.get().sessions.contains(&hex(50)));
}

#[test]
fn a_draining_connection_gets_five_seconds() {
    let (mut relay, sh, _dir) = bare();
    let mut lz = Lazy::new(&sh);
    lz.get().test_session(&hex(50), &fp(1), &[&hex(1)], 0.0);
    lz.get().sessions.get_mut(&hex(50)).unwrap().viewers = 1;
    lz.get().test_edge(&hex(1), &fp(1), &fp(2), 0.0);
    let (a, a_end) = relay.plug("192.0.2.1", 100.0);
    let (b, mut b_end) = relay.plug("192.0.2.1", 100.0);
    relay.splice(a, b);
    for id in [a, b] {
        let c = relay.conns.get_mut(&id).unwrap();
        (c.sid, c.pid) = (Some(hex(50)), Some(hex(1)));
    }
    // a 那一头发来的还没转给 b；a 关了：b 改成“写完就关”，不再读它，这一处接入算结束（只算一次）
    a_end.shutdown(Shutdown::Write).unwrap();
    relay.received(&mut lz, a, b"last words", 101.0).unwrap();
    relay.readable(&mut lz, a, 102.0).unwrap();
    assert!(!relay.conns.contains_key(&a));
    let c = relay.conns.get(&b).unwrap();
    assert_eq!((c.state, c.paused, c.peer, c.born, c.wants()), (State::Drain, true, None, 102.0, libc::POLLOUT));
    assert_eq!(lz.get().sessions.get(&hex(50)).unwrap().viewers, 0);
    // 5 秒整还等着；写完了马上关
    relay.sweep(&mut lz, 107.0);
    assert!(relay.conns.contains_key(&b));
    relay.writable(&mut lz, b, 107.0);
    assert!(relay.conns.is_empty());
    assert_eq!(read_n(&mut b_end, 10), b"last words");
    assert!(closed(&mut b_end));
    assert_eq!(lz.get().sessions.get(&hex(50)).unwrap().viewers, 0);
    // 写不出去的：过了 5 秒不等了
    let (a, a_end) = relay.plug("192.0.2.1", 200.0);
    let (b, _b_end) = relay.plug("192.0.2.1", 200.0);
    relay.splice(a, b);
    relay.received(&mut lz, a, b"never read", 201.0).unwrap();
    drop(a_end);
    relay.readable(&mut lz, a, 202.0).unwrap();
    assert_eq!(relay.conns.get(&b).unwrap().state, State::Drain);
    relay.sweep(&mut lz, 207.0);
    assert!(relay.conns.contains_key(&b));
    relay.sweep(&mut lz, 207.001);
    assert!(relay.conns.is_empty());
}

#[test]
fn a_full_queue_pauses_the_other_end_until_half_is_written() {
    let (mut relay, sh, _dir) = bare();
    let mut lz = Lazy::new(&sh);
    let queue = relay.knobs.queue;
    let (a, _a_end) = relay.plug("192.0.2.1", 100.0);
    let (b, mut b_end) = relay.plug("192.0.2.1", 100.0);
    relay.splice(a, b);
    // 攒到正好上限：接着读；多一个字节：先不读 a
    relay.received(&mut lz, a, &vec![7u8; queue], 101.0).unwrap();
    assert!(!relay.conns.get(&a).unwrap().paused);
    assert_eq!(relay.conns.get(&a).unwrap().wants(), libc::POLLIN);
    relay.received(&mut lz, a, &[7u8], 102.0).unwrap();
    assert!(relay.conns.get(&a).unwrap().paused);
    assert_eq!((relay.conns.get(&a).unwrap().wants(), relay.conns.get(&b).unwrap().wants()), (0, libc::POLLIN | libc::POLLOUT));
    assert_eq!((relay.conns.get(&b).unwrap().seen, relay.conns.get(&a).unwrap().seen), (102.0, 102.0));
    // b 写出去，剩下的不到上限的一半才接着读 a
    b_end.set_nonblocking(true).unwrap();
    let (mut got, mut buf) = (0usize, vec![0u8; 65536]);
    while !relay.conns.get(&b).unwrap().out.is_empty() {
        relay.writable(&mut lz, b, 103.0);
        while let Ok(n) = b_end.read(&mut buf) {
            assert!(n > 0 && buf[..n].iter().all(|x| *x == 7));
            got += n;
        }
        let left = relay.conns.get(&b).unwrap().out.len();
        assert_eq!(relay.conns.get(&a).unwrap().paused, left >= queue / 2, "{left}");
    }
    while let Ok(n) = b_end.read(&mut buf) {
        got += n;
    }
    assert_eq!(got, queue + 1);
    assert_eq!((relay.conns.get(&a).unwrap().wants(), relay.conns.get(&b).unwrap().wants()), (libc::POLLIN, libc::POLLIN));
}

#[test]
fn a_control_connection_is_not_buffered_for() {
    let (mut relay, sh, _dir) = bare();
    let mut lz = Lazy::new(&sh);
    lz.get().test_session(&hex(50), &fp(1), &[&hex(1)], 0.0);
    let (ctl, _end) = relay.plug("192.0.2.1", 100.0);
    let c = relay.conns.get_mut(&ctl).unwrap();
    (c.state, c.sid) = (State::Ctl, Some(hex(50)));
    lz.get().sessions.get_mut(&hex(50)).unwrap().conn = Some(ctl);
    // 控制连接上只发不收：它不读、攒过了上限就断开（会话留着等它重连）
    let queue = relay.knobs.queue;
    relay.send(&mut lz, ctl, &vec![0u8; queue], 101.0);
    assert!(relay.conns.contains_key(&ctl));
    relay.send(&mut lz, ctl, &[0u8], 102.0);
    assert!(relay.conns.is_empty());
    let x = lz.get().sessions.get(&hex(50)).unwrap().clone();
    assert_eq!((x.conn, x.lost), (None, Some(102.0)));
}
