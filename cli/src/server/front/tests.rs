//! front 的单元测试：真的连接（本机的环回地址），一头是这里的 `Front`，一头是测试自己的套接字。
//!
//! `replays_the_python_server`：tests/vectors/server_http.json 是真的 Python 服务器（Python 3.14，--behind-proxy）对一串原样的
//! 请求回的字节——请求行和请求头的各种写法、Content-Length 和签名那个头的各种写法、验签名之前各步的先后。这里把同样的字节发给
//! Rust 的，回的要一个字节不差（Date 的值除外）。

use super::*;
use crate::py::fs::TempDir;
use crate::py::json;
use crate::server::handler::tests::{signed, Bench, Signer};
use crate::server::limits::{Guards, Limits};
use crate::server::store::tests::said;
use crate::server::store::{Store, StoreCfg};
use crate::server::x509::testkit::{new_key, pem, self_signed};
use crate::server::x509::San;
use crate::server::{tls, MAX_CONN, MAX_CONN_IP};
use std::io::Read;
use std::os::fd::AsRawFd;

fn timing(deadline: u64, handshake: u64, request: u64) -> Timing {
    let ms = Duration::from_millis;
    Timing { deadline: ms(deadline), handshake: ms(handshake), request: ms(request), stall: ms(request) }
}

fn front(tls: Tls, proxied: bool, ctx: Arc<Ctx>, timing: Timing) -> Arc<Front> {
    Arc::new(Front { tls, proxied, ctx, slots: Mutex::new(Slots::for_listener()), timing })
}

/// 起一个监听（环回地址、随便一个端口），另一个线程里一直接连接交给 front
fn listen(front: &Arc<Front>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let front = Arc::clone(front);
    thread::spawn(move || {
        while let Ok((sock, peer)) = l.accept() {
            front.accept(sock, peer);
        }
    });
    addr
}

fn connect(addr: SocketAddr) -> TcpStream {
    let c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    c
}

/// 读到对方关掉为止（对方先关、这边还有没发完的：收到的是“连接被重置”，读到的那些照样算）
fn read_all(c: &mut impl Read) -> Vec<u8> {
    let (mut out, mut buf) = (Vec::new(), [0u8; 16384]);
    loop {
        match c.read(&mut buf) {
            Ok(0) => return out,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe) => return out,
            Err(e) => panic!("读回复：{e}（已经读到 {} 个字节）", out.len()),
        }
    }
}

/// 发一段、读到对方关掉。eof：发完把发的那一半关掉
fn talk(addr: SocketAddr, send: &[u8], eof: bool) -> Vec<u8> {
    let mut c = connect(addr);
    let _ = c.write_all(send); // 对方不等读完就回了、关了的（请求行太长……）：写不进去不算错
    if eof {
        let _ = c.shutdown(Shutdown::Write);
    }
    read_all(&mut c)
}

fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

fn health(addr: SocketAddr) -> String {
    text(&talk(addr, b"GET /v1/health HTTP/1.0\r\n\r\n", false))
}

fn used(front: &Front) -> usize {
    lock(&front.slots).used()
}

/// 等到 ok 为真（最多 5 秒）
fn wait(what: &str, ok: impl Fn() -> bool) {
    let begin = Instant::now();
    while !ok() {
        assert!(begin.elapsed() < Duration::from_secs(5), "等不到：{what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// 真的 Python 服务器回的每个字节（Date 的值除外）
#[test]
fn replays_the_python_server() {
    let doc = json::loads(include_str!("../../../tests/vectors/server_http.json")).unwrap().as_dict().unwrap();
    let dir = TempDir::new().unwrap();
    let path = format!("{}/state.json", dir.path);
    std::fs::write(&path, &*doc.str("state").unwrap()).unwrap();
    let cfg = StoreCfg {
        path: path.into(),
        ttl: 150.0,
        idle: 600.0,
        retain: 30.0 * 86400.0,
        keys_path: None,
        cost_path: None,
        cost_keep: 100.0 * 86400.0,
    };
    let store = Store::open(&cfg, &doc.str("root").unwrap(), Limits::DEFAULT, 0.0).unwrap();
    let ctx =
        Ctx { store: Mutex::new(store), remote: None, relay: None, web: None, guards: Mutex::new(Guards::new()), sig_ok: true };
    let addr = listen(&front(Tls::Plain, true, Arc::new(ctx), Timing::python()));
    // 文件里每个字符是一个字节
    let bytes = |s: &str| -> Vec<u8> { s.chars().map(|c| u8::try_from(u32::from(c)).unwrap()).collect() };
    let cases = doc.list("cases").unwrap().items();
    assert!(cases.len() >= 140);
    let begin = Instant::now();
    for case in cases {
        let case = case.as_dict().unwrap();
        let name = case.str("name").unwrap();
        let mut send = Vec::new();
        for part in case.list("send").unwrap().items() {
            match part.as_list() {
                Some(repeated) => {
                    let unit = bytes(repeated.get(0).unwrap().as_str().unwrap());
                    for _ in 0..repeated.get(1).unwrap().as_int().unwrap() {
                        send.extend_from_slice(&unit);
                    }
                }
                None => send.extend_from_slice(&bytes(part.as_str().unwrap())),
            }
        }
        let got = talk(addr, &send, case.get("eof").unwrap().as_bool().unwrap());
        let mut got: String = got.iter().map(|b| char::from(*b)).collect();
        if let Some(at) = got.find("\r\nDate: ").filter(|_| got.starts_with("HTTP/1.0 ")) {
            let end = at + 8 + got[at + 8..].find("\r\n").unwrap();
            got.replace_range(at + 8..end, "@");
        }
        assert_eq!(got, &*case.str("recv").unwrap(), "{name}");
    }
    assert!(begin.elapsed() < Duration::from_secs(10), "哪一条在等到期限：{:?}", begin.elapsed());
    assert!(doc.str("log").unwrap().is_empty() && said().is_empty(), "Python 的服务器这一路什么都没说");
}

/// 签了名的请求从连接上走一遍：登记、上报、看；正文分几次到、头和正文一起到都行
#[test]
fn signed_requests_over_a_connection() {
    let b = Bench::new();
    let f = front(Tls::Plain, false, Arc::clone(&b.ctx), Timing::python());
    let addr = listen(&f);
    let a = Signer::new(1);
    let now = time::time();
    let wire = |op: &str, who: &Signer, more: &str| -> Vec<u8> {
        let body = format!(r#"{{"v":1,"op":"{op}","key":"{}","ts":{now},"nonce":"{}"{more}}}"#, who.fp, b.nonce());
        let req = signed(op, who, &body);
        let mut out = format!("POST {} HTTP/1.1\r\nHost: team.example.com\r\n", req.target).into_bytes();
        for (name, value) in &req.headers {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&req.body);
        out
    };
    let more = format!(r#","devices":{{"0101010101010101":{{"pub":"{}","name":"书房"}}}}"#, a.line);
    let got = text(&talk(addr, &wire("team_enroll", &b.root, &more), false));
    assert!(got.starts_with("HTTP/1.0 200 OK\r\nServer: cca-presence/1 \r\nDate: "), "{got}");
    assert!(
        got.contains("\r\nContent-Type: application/json\r\n") && got.contains(r#""names":{"0101010101010101":"书房"}"#),
        "{got}"
    );
    // 头和正文分开到、正文一个字节一个字节到
    let req = wire("report", &a, r#","units":{}"#);
    let split = req.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let mut c = connect(addr);
    c.set_nodelay(true).unwrap();
    c.write_all(&req[..split]).unwrap();
    thread::sleep(Duration::from_millis(30));
    for byte in &req[split..split + 20] {
        c.write_all(&[*byte]).unwrap();
        thread::sleep(Duration::from_millis(2));
    }
    c.write_all(&req[split + 20..]).unwrap();
    let got = text(&read_all(&mut c));
    assert!(got.starts_with("HTTP/1.0 200 OK\r\n") && got.ends_with(r#"{"v":1,"ok":true,"ttl":150.0,"idle":600.0}"#), "{got}");
    // 正文没发完就把发的那一半关了：400；同一个请求再发一次：nonce 没记下，照常
    let req = wire("report", &a, r#","units":{}"#);
    let got = text(&talk(addr, &req[..req.len() - 1], true));
    assert!(got.starts_with("HTTP/1.0 400 Bad Request\r\n") && got.ends_with(r#"{"error":"bad"}"#), "{got}");
    assert!(text(&talk(addr, &req, false)).starts_with("HTTP/1.0 200 OK\r\n"));
    assert!(text(&talk(addr, &req, false)).starts_with("HTTP/1.0 409 Conflict\r\n"), "这一次是重放");
    // 回完就关：名额都还了
    wait("名额还回来", || used(&f) == 0);
    said();
}

/// TLS：握手在处理这个连接的线程里做；握不成、不是 TLS 的不回、不说话
#[test]
fn tls_connections() {
    let dir = TempDir::new().unwrap();
    let key = new_key();
    let cert = self_signed(&key, Some(&[San::Ip("127.0.0.1".into())]));
    let put = |name: &str, text: String| {
        let path = std::path::Path::new(&dir.path).join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let config = tls::load_pair(&put("cert.pem", pem("CERTIFICATE", &cert)), &put("key.pem", pem("PRIVATE KEY", &key))).unwrap();
    let b = Bench::new();
    let f = front(Tls::Fixed(config), false, Arc::clone(&b.ctx), timing(3000, 300, 3000));
    let addr = listen(&f);
    let client = |cert: &[u8]| {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(cert.to_vec())).unwrap();
        let config = tls::client_config(roots).unwrap();
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
        rustls::StreamOwned::new(rustls::ClientConnection::new(config, name).unwrap(), connect(addr))
    };
    let mut c = client(&cert);
    c.write_all(b"GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut got = Vec::new();
    c.read_to_end(&mut got).unwrap(); // 服务器关之前说了 close_notify：读到头不算出错
    let got = text(&got);
    assert!(got.starts_with("HTTP/1.0 200 OK\r\n") && got.contains(r#""mode":"team""#), "{got}");
    // 一个比一条 TLS 记录大的回复（记录最大 16 KiB）：一份 60 KiB 的密文存进去再取回来
    let (a, other) = (Signer::new(1), Signer::new(3));
    b.enroll(&a, &"01".repeat(8), "a");
    b.enroll(&other, &"03".repeat(8), "c");
    let now = time::time();
    let acct = "ab".repeat(16);
    let blob = format!(r#"{{"n":"{}","c":"{}","t":"{}"}}"#, "A".repeat(22) + "==", "A".repeat(60000), "A".repeat(43) + "=");
    let post = |who: &Signer, more: &str| -> String {
        let body = format!(r#"{{"v":1,"op":"ledger","key":"{}","ts":{now},"nonce":"{}"{more}}}"#, who.fp, b.nonce());
        let req = signed("ledger", who, &body);
        let mut c = client(&cert);
        let head = format!(
            "POST /v1/ledger HTTP/1.1\r\nContent-Length: {}\r\nX-Cca-Sig: {}\r\n\r\n",
            body.len(),
            req.header("X-Cca-Sig").unwrap()
        );
        c.write_all(head.as_bytes()).unwrap();
        c.write_all(body.as_bytes()).unwrap();
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        text(&got)
    };
    let stored = post(&a, &format!(r#","put":{{"{acct}":{blob}}},"get":[]"#));
    assert!(stored.starts_with("HTTP/1.0 200 OK\r\n"), "{stored}");
    let fetched = post(&other, &format!(r#","put":{{}},"get":["{acct}"]"#)); // 别的设备来取：带着密文
    assert!(fetched.starts_with("HTTP/1.0 200 OK\r\n") && fetched.contains(&"A".repeat(60000)), "{} 个字节", fetched.len());
    // 不认这张证书的客户端：握手不成
    let other = self_signed(&new_key(), Some(&[San::Ip("127.0.0.1".into())]));
    let mut c = client(&other);
    assert!(c.write_all(b"GET /v1/health HTTP/1.0\r\n\r\n").is_err() || c.read_to_end(&mut Vec::new()).is_err());
    // 不是 TLS 的：不回 HTTP 的回复，关掉（rustls 关之前给一条 TLS 的 alert：7 个字节；Python 的 ssl 遇到明文的 HTTP 什么都不给）
    let got = talk(addr, b"GET /v1/health HTTP/1.0\r\n\r\n", false);
    assert!(got.is_empty() || (got.len() == 7 && got[0] == 21), "{got:?}");
    // 连上来什么都不发：握手那一步最多等 300 毫秒
    let begin = Instant::now();
    assert_eq!(read_all(&mut connect(addr)), b"");
    let waited = begin.elapsed();
    assert!(waited >= Duration::from_millis(250) && waited < Duration::from_millis(2500), "{waited:?}");
    // 握完手就走、握完手不说话：都不出声
    drop(client(&cert));
    let mut idle = client(&cert);
    idle.conn.complete_io(&mut idle.sock).unwrap();
    drop(idle);
    wait("名额还回来", || used(&f) == 0);
    assert!(said().is_empty());
}

/// 网页端的证书还没有：这个端口上的连接不握手，直接断开
#[test]
fn no_certificate_yet() {
    let dir = TempDir::new().unwrap();
    let missing = |name: &str| std::path::Path::new(&dir.path).join(name);
    let certs = Arc::new(CertCtx::new(&missing("fullchain.pem"), &missing("privkey.pem"), time::monotonic()));
    let b = Bench::new();
    let f = front(Tls::Reload(certs), false, Arc::clone(&b.ctx), Timing::python());
    let addr = listen(&f);
    let begin = Instant::now();
    assert_eq!(talk(addr, b"", false), b"");
    assert_eq!(talk(addr, b"GET /v1/health HTTP/1.0\r\n\r\n", false), b"");
    assert!(begin.elapsed() < Duration::from_secs(2));
    wait("名额还回来", || used(&f) == 0);
}

/// 同时处理的连接：同一个 IP 8 个、一共 64 个，多了的直接断开；--behind-proxy 时只看总数
#[test]
fn connection_caps() {
    let b = Bench::new();
    let f = front(Tls::Plain, false, Arc::clone(&b.ctx), timing(4000, 4000, 4000));
    let addr = listen(&f);
    let mut idle: Vec<TcpStream> = (0..MAX_CONN_IP).map(|_| connect(addr)).collect();
    wait("8 个都占上名额", || used(&f) == MAX_CONN_IP);
    let begin = Instant::now();
    assert_eq!(talk(addr, b"GET /v1/health HTTP/1.0\r\n\r\n", false), b"", "同一个 IP 第 9 个：直接断开");
    assert!(begin.elapsed() < Duration::from_secs(1), "不排队");
    assert_eq!(used(&f), MAX_CONN_IP);
    // 关掉一个，名额就回来一个
    idle.pop();
    wait("还回一个名额", || used(&f) == MAX_CONN_IP - 1);
    assert!(health(addr).starts_with("HTTP/1.0 200 OK\r\n"));
    // 占着不说话的到了期限被断开，名额都回来
    for c in &mut idle {
        assert_eq!(read_all(c), b"");
    }
    wait("名额都还回来", || used(&f) == 0);

    // 代理后面：连过来的都是代理，同一个 IP 可以过 8 个，到总数为止
    let p = front(Tls::Plain, true, Arc::clone(&b.ctx), timing(4000, 4000, 4000));
    let addr = listen(&p);
    let idle: Vec<TcpStream> = (0..MAX_CONN).map(|_| connect(addr)).collect();
    wait("64 个都占上名额", || used(&p) == MAX_CONN);
    assert_eq!(talk(addr, b"GET /v1/health HTTP/1.0\r\n\r\n", false), b"", "第 65 个");
    drop(idle);
    wait("名额都还回来", || used(&p) == 0);
    assert!(health(addr).starts_with("HTTP/1.0 200 OK\r\n"));
    assert!(said().is_empty());
}

/// 期限：整条连接从接进来起算，一个字节一个字节慢慢发也占不住；一次读最多等那么久。到点不回、不说话
#[test]
fn deadlines() {
    let b = Bench::new();
    // 整条连接 400 毫秒，一次读写 5 秒
    let f = front(Tls::Plain, false, Arc::clone(&b.ctx), timing(400, 5000, 5000));
    let addr = listen(&f);
    let begin = Instant::now();
    let mut slow = connect(addr);
    let writer = slow.try_clone().unwrap();
    let drip = thread::spawn(move || {
        for byte in b"GET /v1/health HTTP/1.0\r\nX-Slow: 0123456789012345678901234567890123456789\r\n\r\n" {
            if (&writer).write_all(&[*byte]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    assert_eq!(read_all(&mut slow), b"", "到点就断，不回");
    let waited = begin.elapsed();
    assert!(waited >= Duration::from_millis(350) && waited < Duration::from_millis(3000), "{waited:?}");
    drip.join().unwrap();
    // 正文读到一半到点了：也是不回
    let begin = Instant::now();
    assert_eq!(talk(addr, b"POST /v1/report HTTP/1.0\r\nContent-Length: 10\r\nX-Cca-Sig: QUJD\r\n\r\n{", false), b"");
    assert!(begin.elapsed() >= Duration::from_millis(350));
    // 期限以内说完的照常
    assert!(health(addr).starts_with("HTTP/1.0 200 OK\r\n"));

    // 一次读最多等 150 毫秒（整条连接 5 秒）
    let f = front(Tls::Plain, false, Arc::clone(&b.ctx), timing(5000, 5000, 150));
    let addr = listen(&f);
    let begin = Instant::now();
    assert_eq!(talk(addr, b"GET /v1/health HTTP/1.0\r\n", false), b"");
    let waited = begin.elapsed();
    assert!(waited >= Duration::from_millis(120) && waited < Duration::from_millis(2500), "{waited:?}");
    wait("名额还回来", || used(&f) == 0);
    assert!(said().is_empty());
}

/// 读写出错（不是等过了头）：日志里说一句；等过了头、对方正常关掉的不说
#[test]
fn io_errors_are_logged_once() {
    let b = Bench::new();
    let f = front(Tls::Plain, false, Arc::clone(&b.ctx), timing(2000, 2000, 2000));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    // 在这个线程里处理（say 说的话是记在线程里的）
    let serve_one = |client: thread::JoinHandle<()>| {
        let (sock, peer) = l.accept().unwrap();
        client.join().unwrap(); // 对方做完它的事（发、重置、关）再处理：读到的就是那个结果
        assert!(lock(&f.slots).take("127.0.0.1", false));
        f.connection(sock, peer.ip(), Slot { slots: &f.slots, ip: "127.0.0.1" }, Instant::now());
        assert_eq!(used(&f), 0);
        said()
    };
    // 发了半个请求就把连接重置了（SO_LINGER 0：关的时候发 RST）
    let reset = thread::spawn(move || {
        let c = TcpStream::connect(addr).unwrap();
        (&c).write_all(b"POST /v1/report HTTP/1.0\r\nContent-Le").unwrap();
        let linger = libc::linger { l_onoff: 1, l_linger: 0 };
        let size = std::mem::size_of::<libc::linger>() as libc::socklen_t;
        // SAFETY: c 是开着的套接字，linger 是这个选项要的那个结构
        assert_eq!(
            unsafe { libc::setsockopt(c.as_raw_fd(), libc::SOL_SOCKET, libc::SO_LINGER, (&raw const linger).cast(), size) },
            0
        );
    });
    let want = os_error_text(&io::Error::from_raw_os_error(libc::ECONNRESET));
    assert_eq!(serve_one(reset), [format!("cca-presence: 处理请求出错：{want}")]);
    // 正常关掉的（发了半个请求、什么都没发）：照读到头处理，不说话
    for sent in [&b"GET /v1/hea"[..], b""] {
        let closed = thread::spawn(move || {
            let c = TcpStream::connect(addr).unwrap();
            (&c).write_all(sent).unwrap();
        });
        assert!(serve_one(closed).is_empty());
    }
}
