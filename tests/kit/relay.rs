//! 远程会话经服务器中转的那几份测试共用的夹具（原来 tests/remote_relay.py 里、team_relay、web_relay、web_demo 也拿去用的那些；
//! docs/dev/remote.md、server/README.md「远程会话的中转」）。把辅助程序和服务器都当黑盒，协议的另一头照文档自己说（kit::spec）。
//!
//! - 帧的类型和读写连接的几个小函数：[`ctl`] / [`read_ctl`]（控制连接上的帧）、[`frame`]（通道里的帧）、[`connect`]、[`dial`]
//!   （连数据端口、说开场白）、[`recv`]、[`read_line`]、[`read_exact`]、[`read_all`]、[`sendall`]。读的那几个认的是 `Read`：
//!   别的像连接的东西（网页端测试的 WebSocket）也能交给它们
//! - [`Sniffer`]：设备和数据端口之间的转发器，经过的字节都记下来——服务器（和路上的人）看得到的就是这些
//! - [`RelayServer`]：一个真的服务器进程（Team「测试组」的同步服务器；用哪一份照 kit::server），开着中转
//! - [`Base`]：每个用例的准备（原来的 Base.setUp）——临时目录、证书、Team、转发器、服务器；在它上面建设备（[`Base::device`]）、
//!   照文档发请求（[`Base::call`]、[`Base::good`]）、照文档配对（[`Base::spec_pair`]）、跑真的 cca 脚本（[`Base::cca`]、
//!   [`Base::app_session`]：界面 App 的提问会话）
//! - 设备上的东西：[`work`]、[`client`]、[`helper`]、[`helper_cmd`]、[`term`]、[`session`]（宿主里的会话）、[`peers`]、[`secret`]
//! - [`Popen`]：一个接着管道的子进程（等它的一行输出、之后收齐剩下的；原来的 subprocess.Popen）；[`pgrep`]
//! - [`SpecPeer`]、[`Reader`]：照文档自己说话的另一头（原来的 PyPeer、Reader）
//! - 用例里写断言用的小东西：[`secs`]、[`names`]、[`only`]、[`has`]、[`matches`]、[`parsed`]、[`cols`] / [`rows_of`]、[`walk`]、[`named_like`]
//!
//! 和原来不一样的几处（都是 Rust 里本来的样子）：
//! - 连接、终端离开作用域就关掉，所以没有 `SOCKS` / `close_socks()`、`self.terms`：用例里把它们声明在 [`Base`] 后面
//!   （后声明的先收场：连接和终端先关，再停服务器、转发器）；要提前关就 `drop`
//! - 转发器是这个进程里的几条线（原来另起一个进程，只因为 Python 开伪终端要 fork）
//! - 设备的 ~/bin 里多一个指着辅助程序的链接（[`helper`]）：在终端里起的、cca 脚本起的辅助程序，命令行里于是带着测试的临时目录
//!   ——按命令行找进程的地方只找得到这个用例自己的，运行器也认得出没收场的
//!
//! ```ignore
//! let fx = Base::new(t);                                   // 用例完了自己收场
//! let (a, b) = (fx.device("box"), fx.device("laptop"));
//! fx.machines(&[&a, &b]);                                  // 管理员在服务器上登记这两台
//! let (p, pid) = fx.spec_pair(&a, &b);                     // 照文档配对：密钥、配对编号
//! let t = fx.good(&a, "relay_host", &obj! { "sid" => sid.as_str(), "pids" => list![pid.as_str()] });
//! let (mut host, ok) = dial(fx.srv.data_port(), "host", t["tid"].str(), &crypto::b64_decode(t["tkey"].str()).unwrap());
//! assert_eq!(read_line(&mut host), ok);
//! ```

use crate::fleet::{self, Device, ServerProc, Team};
use crate::fsx;
use crate::harness::{self, Ctx};
use crate::json::{self, Style, Value};
use crate::proc::{Child, Cmd, Stdio};
use crate::sandbox::{self, Ran};
use crate::spec::{self, Channel};
use crate::term::{self, Term};
use crate::{crypto, obj, pki, py};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// 控制连接（宿主那头的转接进程和服务器之间）上的帧的类型
/// 服务器 → 宿主：有人要接入（带着令牌）
pub const F_JOIN: u8 = 0x02;
pub const F_PING: u8 = 0x06;
pub const F_PONG: u8 = 0x07;
/// 宿主 → 服务器：会话的说明（每个配对一份密文）
pub const F_ANNOUNCE: u8 = 0x08;
/// 宿主 → 服务器：会话结束了
pub const F_BYE: u8 = 0x09;

// 通道里（两头之间、加密的）的帧的类型
pub const HELLO: u8 = 0x01;
pub const INPUT: u8 = 0x02;
pub const RESIZE: u8 = 0x03;
pub const DETACH: u8 = 0x04;
pub const WELCOME: u8 = 0x11;
pub const OUTPUT: u8 = 0x12;
pub const EXIT: u8 = 0x13;

/// [`dial`] 的连接等多久算超时（原来 create_connection 的 timeout=20）
pub const DIAL_WAIT: Duration = Duration::from_secs(20);
/// [`Popen`] 等一行输出最多等多久（原来的 readline 没有时限：卡住了要等到用例超时才知道）
pub const LINE_WAIT: Duration = Duration::from_secs(60);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 路径当参数、当环境变量的值用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

// ---- 连接上的读写 ----

/// 连 127.0.0.1:port；连、读、写各最多等 timeout（`socket.create_connection(('127.0.0.1', port), timeout=…)`）。连不上就 panic
pub fn connect(port: u16, timeout: Duration) -> TcpStream {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let s = TcpStream::connect_timeout(&addr, timeout).unwrap_or_else(|e| panic!("连不上 127.0.0.1:{port}：{e}"));
    s.set_read_timeout(Some(timeout)).unwrap_or_else(|e| panic!("设不了读的时限：{e}"));
    s.set_write_timeout(Some(timeout)).unwrap_or_else(|e| panic!("设不了写的时限：{e}"));
    s
}

/// 读一次（`s.recv(n)`）：有多少给多少、最多 n 个字节；那头关了（或者把连接断了）是空的。到了连接的时限还没有东西就 panic
/// （原来是抛 timeout：用例失败）
pub fn recv<R: Read>(s: &mut R, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    loop {
        match s.read(&mut buf) {
            Ok(got) => {
                buf.truncate(got);
                return buf;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                panic!("等到时限，连接上还是没有东西可读（{e}）")
            }
            Err(e) => panic!("连接读不了：{e}"),
        }
    }
}

/// 读到换行为止（带着换行）；那头先关了就是读到的那些
pub fn read_line<R: Read>(s: &mut R) -> Vec<u8> {
    let mut out = Vec::new();
    while !out.ends_with(b"\n") {
        let c = recv(s, 1);
        if c.is_empty() {
            break;
        }
        out.extend_from_slice(&c);
    }
    out
}

/// 读满 n 个字节；那头先关了就是读到的那些
pub fn read_exact<R: Read>(s: &mut R, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let c = recv(s, n - out.len());
        if c.is_empty() {
            break;
        }
        out.extend_from_slice(&c);
    }
    out
}

/// 读到那头关上为止
pub fn read_all<R: Read>(s: &mut R) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let c = recv(s, 65536);
        if c.is_empty() {
            return out;
        }
        out.extend_from_slice(&c);
    }
}

/// 全写出去（`s.sendall(data)`）；写不出去就 panic
pub fn sendall<W: Write>(s: &mut W, data: &[u8]) {
    s.write_all(data).unwrap_or_else(|e| panic!("连接写不进去：{e}"));
}

/// 控制连接上的一帧：类型(1) 长度(2，大端) 内容
pub fn ctl(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(payload.len()).unwrap_or_else(|_| panic!("控制连接上的一帧放不下 {} 个字节", payload.len()));
    [&[kind][..], &len.to_be_bytes(), payload].concat()
}

/// 读控制连接上的一帧：(类型, 内容)；头三个字节都没读全（那头关了）是 (None, 空的)
pub fn read_ctl<R: Read>(s: &mut R) -> (Option<u8>, Vec<u8>) {
    let head = read_exact(s, 3);
    if head.len() < 3 {
        return (None, Vec::new());
    }
    (Some(head[0]), read_exact(s, u16::from_be_bytes([head[1], head[2]]) as usize))
}

/// 通道里的一帧：类型(1) 长度(4，大端) 内容
pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or_else(|_| panic!("一帧放不下 {} 个字节", payload.len()));
    [&[kind][..], &len.to_be_bytes(), payload].concat()
}

/// 连数据端口、说开场白（role：host / view 用票据的编号和密钥，join 用令牌和控制连接那张票据的密钥）。
/// 返回 (连接, 服务器该回的那一行)。连接的时限是 [`DIAL_WAIT`]
pub fn dial(port: u16, role: &str, ident: &str, key: &[u8]) -> (TcpStream, Vec<u8>) {
    let mut s = connect(port, DIAL_WAIT);
    let (line, ok) = spec::relay_line(role, ident, key);
    sendall(&mut s, &line);
    (s, ok)
}

// ---- 转发器 ----

/// 转发器手里的一对连接（设备那头、服务器那头），收场时拿来断开
type Pair = (u64, TcpStream, TcpStream);

/// 设备和数据端口之间的转发器：在 127.0.0.1 上另听一个端口，接进来一条就连到服务器的数据端口（端口现读 d/rport：服务器重启、
/// 换了数据端口也照转），两个方向经过的字节都追加到 d/sniffed 里。[`Sniffer::snapshot`] 是到现在为止经过的全部字节。
/// 离开作用域时停掉（正转着的都断开）
pub struct Sniffer {
    /// 经过的字节记在这个文件里（d/sniffed）
    pub capture: PathBuf,
    /// 它听的端口（也写在 d/sport 里）
    pub port: u16,
    stop: Arc<AtomicBool>,
    pairs: Arc<Mutex<Vec<Pair>>>,
    accept: Option<JoinHandle<()>>,
}

impl Sniffer {
    /// 起转发器：服务器的数据端口到时候从 d/rport 读
    pub fn start(d: &Path) -> Sniffer {
        let capture = d.join("sniffed");
        fsx::write(&capture, b"");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|e| panic!("转发器在 127.0.0.1 上绑不了端口：{e}"));
        let port = listener.local_addr().unwrap_or_else(|e| panic!("问不到转发器绑上的端口：{e}")).port();
        fsx::write(d.join("sport.tmp"), port.to_string());
        std::fs::rename(d.join("sport.tmp"), d.join("sport")).unwrap_or_else(|e| panic!("写不了 {}/sport：{e}", d.display()));
        let out = OpenOptions::new().append(true).open(&capture).unwrap_or_else(|e| panic!("打不开 {}：{e}", capture.display()));
        let out = Arc::new(Mutex::new(out));
        let stop = Arc::new(AtomicBool::new(false));
        let pairs: Arc<Mutex<Vec<Pair>>> = Arc::new(Mutex::new(Vec::new()));
        let rport_file = d.join("rport");
        let (stopping, known) = (stop.clone(), pairs.clone());
        let accept = std::thread::spawn(move || {
            let mut next = 0u64;
            loop {
                let conn = listener.accept();
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok((c, _)) = conn else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                // 服务器的数据端口：接进来一条才读；读不到、连不上就把这一条关掉
                let upstream = std::fs::read_to_string(&rport_file)
                    .ok()
                    .and_then(|t| t.trim().parse::<u16>().ok())
                    .and_then(|p| TcpStream::connect(("127.0.0.1", p)).ok());
                let Some(u) = upstream else { continue };
                let (id, out, known) = (next, out.clone(), known.clone());
                next += 1;
                std::thread::spawn(move || pipe_pair(id, c, u, &out, &known));
            }
        });
        Sniffer { capture, port, stop, pairs, accept: Some(accept) }
    }

    /// 到现在为止经过的全部字节（两个方向的，照经过的先后）
    pub fn snapshot(&self) -> Vec<u8> {
        fsx::read(&self.capture)
    }

    /// 停掉：不再接新的，正转着的都断开（原来是杀掉那个进程）。停过了再停什么都不做
    pub fn close(&mut self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // 接连接的那条线正等在 accept 上：连它一下，它醒过来看到要停了
        drop(TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_secs(5)));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        for (_, c, u) in locked(&self.pairs).drain(..) {
            let _ = c.shutdown(Shutdown::Both);
            let _ = u.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Sniffer {
    fn drop(&mut self) {
        self.close();
    }
}

/// 一对连接对接起来：两个方向各一条线，都转完了把两条连接关上
fn pipe_pair(id: u64, c: TcpStream, u: TcpStream, out: &Arc<Mutex<File>>, known: &Arc<Mutex<Vec<Pair>>>) {
    let (Ok(c_back), Ok(u_back)) = (c.try_clone(), u.try_clone()) else { return };
    if let (Ok(c_kept), Ok(u_kept)) = (c.try_clone(), u.try_clone()) {
        locked(known).push((id, c_kept, u_kept));
    }
    let back_out = out.clone();
    let back = std::thread::spawn(move || pump(&u_back, &c_back, &back_out));
    pump(&c, &u, out);
    let _ = back.join();
    // 登记表里还留着这两条连接的另一份：光是离开作用域关不掉，明着关
    let _ = c.shutdown(Shutdown::Both);
    let _ = u.shutdown(Shutdown::Both);
    locked(known).retain(|(i, _, _)| *i != id);
}

/// 一个方向：从 a 读、记下来、写给 b；a 读到头（或者哪头出了错）就把 b 的写、a 的读关上
fn pump(mut a: &TcpStream, mut b: &TcpStream, out: &Mutex<File>) {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match a.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let _ = locked(out).write_all(&buf[..n]);
        if b.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = b.shutdown(Shutdown::Write);
    let _ = a.shutdown(Shutdown::Read);
}

// ---- 服务器 ----

/// [`RelayServer`] 怎么起（原来 RelayServer 的那几个带默认值的参数）
#[derive(Clone, Debug)]
pub struct RelayOpts {
    /// 开不开中转（默认开）；false：没开，和旧版服务器一样
    pub relay: bool,
    /// 告诉客户端连这个端口（转发器的）；None：不说，客户端直接连数据端口
    pub sniff: Option<u16>,
    /// 监听这个端口（重启时用回原来的：设备记着的服务器地址里写着它）；0 是随便挑一个
    pub port: u16,
    /// 后面多带的参数（网页端的那几个）
    pub extra: Vec<String>,
}

impl Default for RelayOpts {
    fn default() -> RelayOpts {
        RelayOpts { relay: true, sniff: None, port: 0, extra: Vec::new() }
    }
}

impl RelayOpts {
    /// 默认的那一套：开着中转、不经转发器、端口随便挑、不多带参数
    pub fn new() -> RelayOpts {
        RelayOpts::default()
    }

    /// 开不开中转（`relay=False`）
    pub fn relay(mut self, relay: bool) -> RelayOpts {
        self.relay = relay;
        self
    }

    /// 告诉客户端连这个转发器的端口（`sniff=…`：服务器带上 `--relay-port`）
    pub fn sniff(mut self, sniff: &Sniffer) -> RelayOpts {
        self.sniff = Some(sniff.port);
        self
    }

    /// 监听这个端口（`port=…`）
    pub fn port(mut self, port: u16) -> RelayOpts {
        self.port = port;
        self
    }

    /// 后面多带几个参数（`extra=…`）
    pub fn extra<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> RelayOpts {
        self.extra.extend(args.into_iter().map(Into::into));
        self
    }
}

/// 一个真的服务器进程（Team「测试组」的同步服务器：根公钥是 d 下的 root.pub，[`Base`] 建的；证书是 d 下的 cert.pem、key.pem）。
/// 名单、配对记在 d 下的文件里（state.json、remote.json）：重启以后照旧。数据端口换了没关系，转发器每次现读。
/// 离开作用域时停掉
pub struct RelayServer {
    /// 它的目录
    pub d: PathBuf,
    /// 底下的进程（`srv.proc.poll()`：还在运行是 None）
    pub proc: ServerProc,
    /// 监听的端口
    pub port: u16,
    /// 数据端口（中转的）；没开中转是 None。要拿来连的用 [`RelayServer::data_port`]
    pub rport: Option<u16>,
}

impl RelayServer {
    /// 照 opts 起，等它写下端口（开着中转的话还有数据端口）；没起来就退出了马上 panic（哪条命令、退出码、它说的话）
    pub fn start(d: &Path, opts: &RelayOpts) -> RelayServer {
        for name in ["port", "rport", "wport"] {
            let _ = std::fs::remove_file(d.join(name));
        }
        let root = spec::pub_line(&fsx::read_text(d.join("root.pub")));
        let mut args: Vec<String> = Vec::new();
        args.extend(["--listen".to_string(), format!("127.0.0.1:{}", opts.port)]);
        args.extend(["--port-file".to_string(), text(&d.join("port"))]);
        args.extend(["--cert".to_string(), text(&d.join("cert.pem")), "--key".to_string(), text(&d.join("key.pem"))]);
        args.extend(["--team-root".to_string(), root]);
        args.extend(["--state".to_string(), text(&d.join("state.json"))]);
        args.extend(["--ttl", "60", "--idle", "600"].map(String::from));
        if opts.relay {
            args.extend(["--relay-listen".to_string(), "127.0.0.1:0".to_string()]);
            args.extend(["--relay-port-file".to_string(), text(&d.join("rport"))]);
        }
        if let Some(port) = opts.sniff {
            args.extend(["--relay-port".to_string(), port.to_string()]);
        }
        args.extend(opts.extra.iter().cloned());
        let mut proc = ServerProc::spawn(&args, &[]);
        let port = port_in(&proc.wait_file(&d.join("port"), "服务器起来", fleet::WAIT));
        let rport = opts.relay.then(|| port_in(&proc.wait_file(&d.join("rport"), "数据端口起来", fleet::WAIT)));
        RelayServer { d: d.to_path_buf(), proc, port, rport }
    }

    /// 数据端口（`srv.rport`）；没开中转就 panic
    pub fn data_port(&self) -> u16 {
        self.rport.unwrap_or_else(|| panic!("这台服务器没开中转：没有数据端口"))
    }

    /// 停掉（TERM；5 秒还不退出就杀掉）。停过了再停什么都不做
    pub fn stop(&mut self) {
        if self.proc.poll().is_some() {
            return;
        }
        self.proc.signal(libc::SIGTERM);
        if self.proc.wait(Duration::from_secs(5)).is_none() {
            self.proc.kill();
        }
    }
}

impl Drop for RelayServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 端口文件里写的端口
fn port_in(written: &str) -> u16 {
    written.trim().parse().unwrap_or_else(|_| panic!("端口文件里写的不是端口：{written:?}"))
}

// ---- 接着管道的子进程 ----

/// 一路输出：另一条线一直在读，读到的攒在这里
#[derive(Default)]
struct Pipe {
    /// (到现在为止读到的, 读到头了没有)
    got: Mutex<(Vec<u8>, bool)>,
}

impl Pipe {
    fn drain(mut from: File) -> Arc<Pipe> {
        let pipe = Arc::new(Pipe::default());
        let sink = pipe.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match from.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => locked(&sink.got).0.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            locked(&sink.got).1 = true;
        });
        pipe
    }

    /// 从第 at 个字节起的一行（带着换行）；读到头了、最后一行没有换行就是剩下的那些（可能是空的）。到 end 还没有是 None
    fn line(&self, at: usize, end: Instant) -> Option<Vec<u8>> {
        loop {
            {
                let got = locked(&self.got);
                if let Some(nl) = got.0[at..].iter().position(|b| *b == b'\n') {
                    return Some(got.0[at..=at + nl].to_vec());
                }
                if got.1 {
                    return Some(got.0[at..].to_vec());
                }
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// 等到读到头（最多到 end），给出从第 at 个字节起的全部；第二项：读到头了没有
    fn rest(&self, at: usize, end: Instant) -> (Vec<u8>, bool) {
        loop {
            {
                let got = locked(&self.got);
                if got.1 {
                    return (got.0[at..].to_vec(), true);
                }
            }
            if Instant::now() >= end {
                return (locked(&self.got).0[at..].to_vec(), false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// 到现在为止读到的，当文字（出错的说明里用）
    fn so_far(&self) -> String {
        String::from_utf8_lossy(&locked(&self.got).0).into_owned()
    }
}

/// 管道里读到的当文字（`text=True`）；不是 UTF-8 就 panic（Python 那边解码就出错了）
fn utf8(bytes: Vec<u8>, what: &str) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| panic!("{what}不是 UTF-8：{:?}", String::from_utf8_lossy(e.as_bytes())))
}

/// 一个起来了的子进程，标准输出、标准错误接着管道（原来的 `subprocess.Popen(…, stdin=DEVNULL, stdout=PIPE, stderr=PIPE, text=True)`）：
/// 先等它的一行（[`Popen::out_line`]：配对码），做完别的再收齐剩下的（[`Popen::communicate`]）。两路输出另有线一直在读：
/// 说得再多也不会把它卡住。离开作用域时还没退出的话停掉（TERM，2 秒后 KILL）
pub struct Popen {
    child: Child,
    out: Arc<Pipe>,
    err: Arc<Pipe>,
    /// 标准输出、标准错误各读走了多少
    out_at: usize,
    err_at: usize,
}

impl Popen {
    /// 起 cmd（环境、参数、当前目录照它的；标准输入是 /dev/null，两路输出在这里接成管道）。起不来就 panic
    pub fn spawn(cmd: &mut Cmd) -> Popen {
        let mut child = cmd.stdin(Stdio::Null).stdout(Stdio::Piped).stderr(Stdio::Piped).spawn().unwrap_or_else(|e| panic!("{e}"));
        let out = Pipe::drain(child.stdout.take().expect("标准输出接着管道"));
        let err = Pipe::drain(child.stderr.take().expect("标准错误接着管道"));
        Popen { child, out, err, out_at: 0, err_at: 0 }
    }

    /// 进程号
    pub fn pid(&self) -> i32 {
        self.child.pid()
    }

    fn take_line(&mut self, stderr: bool) -> String {
        let (pipe, at, what) =
            if stderr { (&self.err, &mut self.err_at, "标准错误") } else { (&self.out, &mut self.out_at, "标准输出") };
        let Some(line) = pipe.line(*at, Instant::now() + LINE_WAIT) else {
            panic!(
                "等了 {} 秒，{what}上还是没有下一行。标准输出到现在：{:?}，标准错误：{:?}",
                LINE_WAIT.as_secs(),
                self.out.so_far(),
                self.err.so_far()
            )
        };
        *at += line.len();
        utf8(line, what)
    }

    /// 标准输出的下一行（带着换行；`p.stdout.readline()`）；它关了标准输出、没有更多了是空的。最多等 [`LINE_WAIT`]，没等到就 panic
    pub fn out_line(&mut self) -> String {
        self.take_line(false)
    }

    /// 标准错误的下一行（`p.stderr.readline()`）
    pub fn err_line(&mut self) -> String {
        self.take_line(true)
    }

    /// 还在运行是 None；退出了：退出码（Python 的 returncode：被信号杀掉的是负的信号号）
    pub fn poll(&mut self) -> Option<i32> {
        self.child.try_wait().ok().flatten().map(|exit| exit.py())
    }

    /// 等它退出，最多等 limit（`p.wait(秒)`）：给出退出码；到时限还没退出就 panic，带上它到那时说的话
    pub fn wait(&mut self, limit: Duration) -> i32 {
        match self.child.wait_timeout(limit).unwrap_or_else(|e| panic!("{e}")) {
            Some(exit) => exit.py(),
            None => panic!(
                "进程 {} 过了 {} 秒还没退出。标准输出到现在：{:?}，标准错误：{:?}",
                self.pid(),
                limit.as_secs_f64(),
                self.out.so_far(),
                self.err.so_far()
            ),
        }
    }

    /// 等它退出、把两路输出收齐（`p.communicate(timeout=秒)`）：(退出码, 标准输出里还没读走的, 标准错误里还没读走的)。
    /// 到时限还没退出、输出还没到头就 panic
    pub fn communicate(&mut self, limit: Duration) -> (i32, String, String) {
        let end = Instant::now() + limit;
        let code = self.wait(limit);
        let (out, out_done) = self.out.rest(self.out_at, end);
        let (err, err_done) = self.err.rest(self.err_at, end);
        assert!(
            out_done && err_done,
            "进程 {} 退出了，它的输出过了 {} 秒还没到头（有别的进程拿着管道）",
            self.pid(),
            limit.as_secs_f64()
        );
        self.out_at += out.len();
        self.err_at += err.len();
        (code, utf8(out, "标准输出"), utf8(err, "标准错误"))
    }

    /// 标准输出里还没读走的（`p.stdout.read()`）。它还没关标准输出的话最多等 2 秒，给出到那时为止的
    pub fn out_rest(&mut self) -> String {
        let (out, _) = self.out.rest(self.out_at, Instant::now() + Duration::from_secs(2));
        self.out_at += out.len();
        String::from_utf8_lossy(&out).into_owned()
    }

    /// 标准错误里还没读走的（`p.stderr.read()`：出错的说明里用）。它还没关标准错误的话最多等 2 秒，给出到那时为止的
    pub fn err_rest(&mut self) -> String {
        let (err, _) = self.err.rest(self.err_at, Instant::now() + Duration::from_secs(2));
        self.err_at += err.len();
        String::from_utf8_lossy(&err).into_owned()
    }

    /// 给它发一个信号（`p.terminate()` 是 `libc::SIGTERM`）；已经退出了的不发
    pub fn signal(&self, sig: i32) {
        self.child.signal(sig).unwrap_or_else(|e| panic!("给进程 {} 发不了信号 {sig}：{e}", self.pid()));
    }

    /// 杀掉（KILL）、收走
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// 停掉：还没退出的话发 TERM，等 2 秒，还不退出就 KILL，收走（cmd 设了新会话的，连它那个进程组一起）。
    /// 离开作用域时做的就是这个
    pub fn stop(&mut self) {
        let _ = self.child.stop();
    }
}

/// 命令行里找得到 pattern 的进程（就是 `pgrep -f pattern` 找的那些；pattern 是正则，路径这样要照字面找的先转义）：它们的
/// 进程号。并行跑的别的测试也起辅助程序：pattern 里要带上只有这个用例才有的东西（会话的编号、临时目录里的路径：[`helper`]）。
/// 自己看进程表（[`crate::proc::processes`]），不起 pgrep：最小安装的 Linux 上没有它
pub fn pgrep(pattern: &str) -> Vec<i32> {
    let re = regex::Regex::new(pattern).unwrap_or_else(|e| panic!("pgrep 的正则写错了：{pattern}：{e}"));
    let table = crate::proc::processes().unwrap_or_else(|e| panic!("进程表看不了：{e}"));
    table.into_iter().filter(|(_, command)| re.is_match(command)).map(|(pid, _)| pid).collect()
}

// ---- 每个用例的准备 ----

/// 每个用例的准备（原来的 Base.setUp）：临时目录 d（前缀 cca-relay-），里面是服务器的证书、Team「测试组」（根密钥 d/root：
/// 测试扮演管理员，在服务器上登记、移除设备）、转发器、服务器（告诉客户端连转发器的端口）。离开作用域时先停服务器、再停转发器；
/// 临时目录等用例完了由运行器删
pub struct Base {
    /// 临时目录：服务器的文件、各台设备的 HOME（d/<名字>）都在里面
    pub d: PathBuf,
    /// Team「测试组」
    pub team: Team,
    /// 设备和数据端口之间的转发器
    pub sniff: Sniffer,
    /// 服务器。重启：`fx.srv.stop(); fx.srv = RelayServer::start(&fx.d, &RelayOpts::new().sniff(&fx.sniff).port(port));`
    pub srv: RelayServer,
    /// 服务器证书的公钥指纹（设备钉住它）
    pub pin: String,
    /// 答案文件、事件文件、答案表编到第几个了
    n: AtomicUsize,
}

impl Base {
    /// 开着中转的服务器，不多带参数
    pub fn new(t: &mut Ctx) -> Base {
        Base::with(t, true, |_| Vec::new())
    }

    /// relay：服务器开不开中转（原来的类属性 relay）；extra：服务器多带的参数（原来子类的 server_extra(d)：拿到临时目录，
    /// 可以先在里面放好要用的文件；网页端的测试用）
    pub fn with(t: &mut Ctx, relay: bool, extra: impl FnOnce(&Path) -> Vec<String>) -> Base {
        Base::in_dir(&t.tmp("cca-relay-"), relay, extra)
    }

    /// 同 [`Base::with`]，建在给的目录 d 里（已经有、空的；路径要是解过链接的）：不在运行器里用的时候（在本机跑起来看的小工具）
    /// 自己管这个目录
    pub fn in_dir(d: &Path, relay: bool, extra: impl FnOnce(&Path) -> Vec<String>) -> Base {
        pki::make_cert(d);
        let team = Team::new(d);
        let sniff = Sniffer::start(d);
        let extra = extra(d);
        let srv = RelayServer::start(d, &RelayOpts::new().relay(relay).sniff(&sniff).extra(extra));
        let pin = pki::openssl_pin(d.join("cert.pem"));
        Base { d: d.to_path_buf(), team, sniff, srv, pin, n: AtomicUsize::new(0) }
    }

    /// 设备记着的服务器地址
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.srv.port)
    }

    /// 一台在 Team 里的设备（Team 给它 alice 的 Claude 账号：alice 的会话经这个 Team 的服务器中转）；登记到服务器上用
    /// [`Base::machines`]。它的 HOME 是 d/<名字>，里面有 work/（会话起在这里）、假客户端（[`client`]）、
    /// 指着辅助程序的链接（[`helper`]）；环境里多一个 TERM=xterm-256color
    pub fn device(&self, name: &str) -> Device {
        let mut dev = self.device_unjoined(name);
        dev.join(&self.team, &self.url(), &[self.pin.as_str()]);
        dev
    }

    /// 同 [`Base::device`]，但还没加入 Team（原来的 join=False）：之后自己
    /// `dev.join(&fx.team, &fx.url(), &[fx.pin.as_str()])` 或者 `join_with`
    pub fn device_unjoined(&self, name: &str) -> Device {
        let mut dev = Device::new(&self.d.join(name), name, &obj! { Device::KEY => "US-A" });
        dev.env.insert("TERM".to_string(), "xterm-256color".to_string());
        std::fs::create_dir(work(&dev)).unwrap_or_else(|e| panic!("建 {}：{e}", work(&dev).display()));
        term::write_client(&dev.home);
        helper(&dev);
        dev
    }

    /// 服务器上 Team 的名单正好是这几台设备（管理员登记上没登记的、移除别的）
    pub fn machines(&self, devs: &[&Device]) {
        self.team.only(self.srv.port, devs);
    }

    /// 服务器回给不在名单里的设备的：403 unknown_device，带着这个 Team 在服务器上的这一份记录的编号（设备拿它分清是被移除了、
    /// 还是服务器的记录清掉了）。访客的请求不认时回的也是这一个：分不出是没有这个访客、还是签名不对
    pub fn unknown(&self) -> (u16, Value) {
        let (st, j) = self.team.admin(self.srv.port, "team_members", &Value::Null);
        assert_eq!(st, 200, "{j}");
        (403, obj! { "error" => "unknown_device", "gen" => &j["gen"] })
    }

    /// dev 照文档自己签一个请求发给服务器（不经 cca）：(状态码, 回的 JSON)。extra 是请求里另外的几项（null 就是没有）
    pub fn call(&self, dev: &Device, op: &str, extra: &Value) -> (u16, Value) {
        dev.post(self.srv.port, op, extra)
    }

    /// 同 [`Base::call`]，该成的：不是 200 就 panic，返回回的 JSON
    #[track_caller]
    pub fn good(&self, dev: &Device, op: &str, extra: &Value) -> Value {
        let (st, j) = self.call(dev, op, extra);
        assert_eq!(st, 200, "{op}：{j}");
        j
    }

    /// 两台设备照文档配对（a 显示配对码）。返回 (密钥, 配对编号)（原来的 py_pair）
    pub fn spec_pair(&self, a: &Device, b: &Device) -> ([u8; 32], String) {
        let c = crypto::random(10);
        let (kc, keys, rid) = spec::pair_keys(&c);
        let (na, nb) = (crypto::random(32), crypto::random(32));
        let rid = rid.as_str();
        self.good(a, "pair_open", &obj! { "rid" => rid });
        self.good(a, "pair_put", &obj! { "rid" => rid, "m" => spec::pair_msg(&keys, rid, "a", a.fp(), &na, &a.name) });
        self.good(b, "pair_put", &obj! { "rid" => rid, "m" => spec::pair_msg(&keys, rid, "b", b.fp(), &nb, &b.name) });
        let (ja, jb) = (self.good(a, "pair_get", &obj! { "rid" => rid }), self.good(b, "pair_get", &obj! { "rid" => rid }));
        assert_eq!(
            (ja["fp"].str(), ja["name"].str(), jb["fp"].str(), jb["name"].str()),
            (b.fp(), b.name.as_str(), a.fp(), a.name.as_str())
        );
        let from_b = spec::pair_open(&keys, rid, "b", &ja["m"]).expect("b 放的那一份认证得过");
        assert_eq!(from_b["fp"].str(), b.fp());
        let from_a = spec::pair_open(&keys, rid, "a", &jb["m"]).expect("a 放的那一份认证得过");
        assert_eq!(from_a["n"].str(), crypto::hex(&na));
        let (p, pid) = spec::pair_secret(&kc, &na, &nb, a.fp(), b.fp());
        let commit = obj! { "rid" => rid, "pid" => pid.as_str() };
        assert_eq!(self.good(a, "pair_commit", &commit)["done"], Value::Bool(false));
        assert_eq!(self.good(b, "pair_commit", &commit)["done"], Value::Bool(true));
        assert_eq!(self.good(a, "pair_get", &obj! { "rid" => rid, "until" => "done" })["done"], Value::Bool(true));
        (p, pid)
    }

    /// 服务器记着的配对（d/remote.json 的 edges：{配对编号: {a, b, …}}）；还没有这个文件是 {}
    pub fn edges(&self) -> Value {
        edges_in(&self.d)
    }

    // --- bash 这一层：真的 cca 脚本（仓库里的那一份），辅助程序是设备 ~/bin 里的那个链接 ---

    /// 在 dev 上跑 cca 脚本时的环境：设备的环境，加上 CCA_HELPER（[`helper`]）、LANG=en_US.UTF-8、TERM=dumb，再加 more
    /// （同名的以 more 为准）
    pub fn shell_env(&self, dev: &Device, more: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env = dev.env.clone();
        env.insert("CCA_HELPER".to_string(), text(&helper(dev)));
        env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        env.insert("TERM".to_string(), "dumb".to_string());
        for (k, v) in more {
            env.insert(k.to_string(), v.to_string());
        }
        env
    }

    fn next(&self) -> usize {
        self.n.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 写一个答案文件（终端里的回答，一行一个；给 CCA_TTY），返回它的路径
    pub fn answers(&self, lines: &[&str]) -> String {
        let f = self.d.join(format!("answers.{}", self.next()));
        fsx::write(&f, lines.iter().map(|line| format!("{line}\n")).collect::<String>());
        text(&f)
    }

    /// `/bin/bash <仓库>/cca 参数…` 这条命令（还没起），环境是 [`Base::shell_env`]（dev, more），标准输入是 /dev/null。
    /// 要一边跑一边看它输出的交给 [`Popen::spawn`]
    pub fn cca_cmd(&self, dev: &Device, argv: &[&str], more: &[(&str, &str)]) -> Cmd {
        let mut cmd = Cmd::new("/bin/bash");
        cmd.arg(harness::repo().join("cca")).args(argv).envs(self.shell_env(dev, more)).stdin(Stdio::Null);
        cmd
    }

    /// 在 dev 上运行 `cca 参数…`（不带回答、不带事件文件），等它跑完（最多 120 秒）：退出码、标准输出、标准错误
    pub fn cca(&self, dev: &Device, argv: &[&str]) -> Ran {
        self.cca_with(dev, argv, None, None)
    }

    /// 同 [`Base::cca`]；answers：终端里的回答（一行一个，写成答案文件给 CCA_TTY）；events：事件文件（先清空，给 CCA_EVENTS）
    pub fn cca_with(&self, dev: &Device, argv: &[&str], answers: Option<&[&str]>, events: Option<&Path>) -> Ran {
        let mut cmd = self.cca_cmd(dev, argv, &[]);
        if let Some(lines) = answers {
            cmd.env("CCA_TTY", self.answers(lines));
        }
        if let Some(events) = events {
            fsx::write(events, "");
            cmd.env("CCA_EVENTS", events);
        }
        sandbox::ran(cmd.timeout(Duration::from_secs(120)))
    }

    /// 界面 App 那样开一条提问会话（cca-test gui-session 扮演 App：读事件文件里的提问、照答案表 table 从答案管道回答；
    /// opts 是它的选项，比如 `&["--eof-at", "remote.pair"]`）：在 dev 上运行 `cca 参数…`，环境里多 CCA_EVENTS 和 CCA_PROMPT=1。
    /// 返回 (进程, 事件文件)；进程的退出码就是 cca 的。它离开作用域时还没完的话停掉：答案管道跟着关上，cca 按取消收场
    pub fn app_session(&self, dev: &Device, table: &Value, argv: &[&str], opts: &[&str]) -> (Popen, PathBuf) {
        let n = self.next();
        let (ev, ans) = (self.d.join(format!("ev.{n}.jsonl")), self.d.join(format!("table.{n}.json")));
        fsx::write(&ev, "");
        fsx::write(&ans, json::dumps(table, &Style::default()));
        let mut cmd = Cmd::new(harness::tool());
        cmd.arg("gui-session").arg(&ev).arg(&ans).args(opts).args(["--", "/bin/bash"]).arg(harness::repo().join("cca")).args(argv);
        cmd.envs(self.shell_env(dev, &[("CCA_EVENTS", &text(&ev)), ("CCA_PROMPT", "1")]));
        (Popen::spawn(&mut cmd), ev)
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        // 先停服务器、再停转发器（原来收尾的先后）
        self.srv.stop();
        self.sniff.close();
    }
}

/// 目录是 d 的那台服务器记着的配对（d/remote.json 的 edges）；还没有这个文件是 {}
pub fn edges_in(d: &Path) -> Value {
    let p = d.join("remote.json");
    if p.exists() {
        fsx::read_json(&p)["edges"].clone()
    } else {
        obj! {}
    }
}

/// 事件文件里的事件，一行一个（空行不算）
pub fn events(path: &Path) -> Vec<Value> {
    let text = fsx::read_text(path);
    let lines = py::splitlines(&text).into_iter().filter(|line| !line.trim().is_empty());
    lines.map(|line| json::parse(line).unwrap_or_else(|e| panic!("事件文件里有一行不是 JSON（{e}）：{line}"))).collect()
}

// ---- 用例里写断言用的小东西（remote_relay、web_relay、team_relay 三个模块共用） ----

pub fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// 对象的键，照里面的先后（`list(d)`）
pub fn names(v: &Value) -> Vec<String> {
    v.obj().keys().map(str::to_string).collect()
}

/// 列表里正好一项（`(row,) = …`）
pub fn only(v: &Value) -> &Value {
    let rows = v.list();
    assert_eq!(rows.len(), 1, "该正好一项：{v}");
    &rows[0]
}

/// 字节里有没有这一截（`needle in out`）
pub fn has(hay: &[u8], needle: impl AsRef<[u8]>) -> bool {
    term::find(hay, needle).is_some()
}

/// Python 的 re.search(pattern, text) 找不找得到。正则照原来的写；只有末尾的 $ 换一种写法：Python 里它在最后那个换行的
/// 前面也算数，regex 这个库的 $ 只认真正的末尾
pub fn matches(pattern: &str, text: &str) -> bool {
    let pattern = match pattern.strip_suffix('$') {
        Some(head) => format!("{head}\n?\\z"),
        None => pattern.to_string(),
    };
    regex::Regex::new(&pattern).unwrap_or_else(|e| panic!("正则 {pattern:?} 写得不对：{e}")).is_match(text)
}

/// `json.loads(text)`
pub fn parsed(text: &str) -> Value {
    json::parse(text).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{text}"))
}

/// 对象里这几个键的值，照给的先后排成一个数组（原来拿元组比的地方：`(d['a'], d['b']) == (…)`）
pub fn cols(row: &Value, keys: &[&str]) -> Value {
    Value::List(keys.iter().map(|k| row[*k].clone()).collect())
}

/// 数组里每一项的这几个键（`[(p['a'], p['b']) for p in rows]`）
pub fn rows_of(rows: &Value, keys: &[&str]) -> Vec<Value> {
    rows.list().iter().map(|row| cols(row, keys)).collect()
}

/// 目录底下所有的文件和目录（一层层往下走，不跟着链接走）
pub fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found
}

/// 文件名是不是以 prefix 开头
pub fn named_like(path: &Path, prefix: &str) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix))
}

// ---- 设备上的东西 ----

/// 设备上会话起在哪个目录：~/work（原来的 dev.work）
pub fn work(dev: &Device) -> PathBuf {
    dev.home.join("work")
}

/// 设备上的假客户端（cca-test fake-client 写成的脚本，见 kit::term::write_client；原来是 `python3 ~/client.py`）：
/// `remote-host 工具 账号的键 名字 -- 这个路径 参数…`
pub fn client(dev: &Device) -> PathBuf {
    dev.home.join("client")
}

/// 设备 ~/bin 里指着辅助程序的链接（还没有就建上）：留在后台、开着终端的那些辅助程序经它起，命令行里就带着这台设备的 HOME
/// （测试的临时目录）
pub fn helper(dev: &Device) -> PathBuf {
    let link = dev.home.join("bin").join("cca-core");
    if link.symlink_metadata().is_err() {
        std::os::unix::fs::symlink(harness::helper(), &link).unwrap_or_else(|e| panic!("建 {}：{e}", link.display()));
    }
    link
}

/// 在 dev 上跑辅助程序的一条命令（还没起）：经 [`helper`] 那个链接，环境是这一刻的 dev.env，当前目录是 [`work`]。
/// 交给 [`Popen::spawn`]（`remote-pair-show`）或者自己接着点下去
pub fn helper_cmd(dev: &Device, argv: &[&str]) -> Cmd {
    let mut cmd = Cmd::new(helper(dev));
    cmd.args(argv).envs(&dev.env).cwd(work(dev));
    cmd
}

/// 在 dev 上开一个 24 行 80 列的终端运行辅助程序（`Term([HELPER, *argv], dev.env, str(dev.work))`）
pub fn term(dev: &Device, argv: &[&str]) -> Term {
    term_sized(dev, argv, 24, 80)
}

/// 同 [`term`]，终端是 rows 行 cols 列
pub fn term_sized(dev: &Device, argv: &[&str], rows: u16, cols: u16) -> Term {
    Term::start(&mut helper_cmd(dev, argv), rows, cols)
}

/// dev 上起一个在宿主里运行的会话（`remote-host claude 账号的键 名字 -- 假客户端 参数…`），等假客户端报到（`<ready 24 80`）
pub fn session(dev: &Device, key: &str, name: &str, client_args: &[&str]) -> Term {
    let client = text(&client(dev));
    let mut argv = vec!["remote-host", "claude", key, name, "--", client.as_str()];
    argv.extend(client_args);
    let mut h = term(dev, &argv);
    h.expect("<ready 24 80");
    h
}

/// dev 的配对表（~/.cca/remote/peers.json 的 peers：{配对编号: {fp, name, incoming, shown, hub…}}）；还没有是 {}
pub fn peers(dev: &Device) -> Value {
    let p = dev.cca.join("remote").join("peers.json");
    if p.exists() {
        fsx::read_json(&p)["peers"].clone()
    } else {
        obj! {}
    }
}

/// dev 存着的这个配对的密钥（密钥文件 ~/.cca/secrets/cca-remote-<配对编号> 里的 key）；没有是 None
pub fn secret(dev: &Device, pid: &str) -> Option<String> {
    let p = dev.cca.join("secrets").join(format!("cca-remote-{pid}"));
    p.exists().then(|| fsx::read_json(&p)["key"].str().to_string())
}

// ---- 照文档自己说话的另一头 ----

/// 照文档自己说话的一台设备（不经 cca；原来的 PyPeer）：配对、列会话、接入
pub struct SpecPeer<'a> {
    fx: &'a Base,
    dev: &'a Device,
    /// 两台机器之间的密钥（[`SpecPeer::join`] 以后才有）
    pub p: [u8; 32],
    /// 配对编号
    pub pid: String,
    /// 对面那台设备的指纹
    pub peer_fp: String,
}

impl<'a> SpecPeer<'a> {
    pub fn new(fx: &'a Base, dev: &'a Device) -> SpecPeer<'a> {
        SpecPeer { fx, dev, p: [0; 32], pid: String::new(), peer_fp: String::new() }
    }

    /// 输入配对码（对面是真的 cca，已经显示了配对码）：配好以后 p、pid、peer_fp 就有了
    pub fn join(&mut self, code: &str) {
        let (fx, dev) = (self.fx, self.dev);
        let code = spec::code_bytes(code).unwrap_or_else(|| panic!("不是配对码：{code:?}"));
        let (kc, keys, rid) = spec::pair_keys(&code);
        let rid = rid.as_str();
        let nb = crypto::random(32);
        fx.good(dev, "pair_put", &obj! { "rid" => rid, "m" => spec::pair_msg(&keys, rid, "b", dev.fp(), &nb, &dev.name) });
        let j = fx.good(dev, "pair_get", &obj! { "rid" => rid });
        let theirs = spec::pair_open(&keys, rid, "a", &j["m"]).expect("对面放的那一份认证得过");
        assert_eq!(theirs["fp"], j["fp"]);
        self.peer_fp = theirs["fp"].str().to_string();
        let na = crypto::unhex(theirs["n"].str()).unwrap_or_else(|e| panic!("对面的随机数不是十六进制：{e}"));
        (self.p, self.pid) = spec::pair_secret(&kc, &na, &nb, &self.peer_fp, dev.fp());
        fx.good(dev, "pair_commit", &obj! { "rid" => rid, "pid" => self.pid.as_str() });
    }

    /// 经这个配对看得到的会话：解开的说明，各添上 sid
    pub fn sessions(&self) -> Vec<Value> {
        let j = self.fx.good(self.dev, "relay_list", &obj! { "pids" => vec![self.pid.as_str()] });
        let rows = j["sessions"].get(&self.pid).map(|rows| rows.list().to_vec()).unwrap_or_default();
        rows.iter()
            .map(|r| {
                let sid = r["sid"].str();
                let mut doc = spec::dir_open(&self.p, &self.pid, sid, &r["blob"]).expect("会话的说明认证得过");
                doc.as_obj_mut().expect("会话的说明是对象").insert("sid", sid);
                doc
            })
            .collect()
    }

    /// 接到会话上，握手。返回 (连接, 通道)；宿主的证明不对时通道是 None
    pub fn attach(&self, sid: &str) -> (TcpStream, Option<Channel>) {
        self.attach_with(sid, &self.p)
    }

    /// 同 [`SpecPeer::attach`]，握手用的密钥是给的 p（原来的 `attach(sid, p=…)`：没有配对密钥的人拿一把随便的）
    pub fn attach_with(&self, sid: &str, p: &[u8]) -> (TcpStream, Option<Channel>) {
        let t = self.fx.good(self.dev, "relay_attach", &obj! { "sid" => sid, "pid" => self.pid.as_str() });
        let port = u16::try_from(t["port"].i64()).unwrap_or_else(|_| panic!("票据里的端口不对：{t}"));
        let tkey = crypto::b64_decode(t["tkey"].str()).unwrap_or_else(|e| panic!("票据的密钥不是 base64：{e}"));
        let (mut s, ok) = dial(port, "view", t["tid"].str(), &tkey);
        assert_eq!(read_line(&mut s), ok);
        let mut ch = Channel::new("viewer", p, &self.pid, sid, &self.peer_fp, self.dev.fp());
        sendall(&mut s, &ch.hello());
        let Some(confirm) = ch.confirm(&read_exact(&mut s, 68)) else { return (s, None) };
        sendall(&mut s, &confirm);
        (s, Some(ch))
    }
}

/// 照文档自己说话的接入方：在握过手的通道上收发帧。out 是收到的输出（会话结束记成 `<EXIT 码>`），kinds 是收到过的帧的类型，
/// raw 是解出来的全部明文。连接和通道归它管（`r.s`、`r.ch` 照样拿得到）：它离开作用域连接就关了
pub struct Reader {
    /// 连接
    pub s: TcpStream,
    /// 握过手的通道
    pub ch: Channel,
    buf: Vec<u8>,
    plain: Vec<u8>,
    /// 收到的输出
    pub out: Vec<u8>,
    /// 收到过的帧的类型
    pub kinds: BTreeSet<u8>,
    /// 解出来的全部明文
    pub raw: Vec<u8>,
}

impl Reader {
    /// s、ch：[`SpecPeer::attach`] 给的连接和通道
    pub fn new(s: TcpStream, ch: Channel) -> Reader {
        Reader { s, ch, buf: Vec::new(), plain: Vec::new(), out: Vec::new(), kinds: BTreeSet::new(), raw: Vec::new() }
    }

    /// 封成通道上的一条记录发出去（`sock.sendall(ch.seal(data))`；data 多半是 [`frame`] 拼的一帧）
    pub fn send(&mut self, data: &[u8]) {
        let record = self.ch.seal(data);
        sendall(&mut self.s, &record);
    }

    /// 读到 want(out) 为真，返回 out；每次读最多等 15 秒。连接断了、等不到、记录认证不过都算失败（panic）
    pub fn until(&mut self, want: impl Fn(&[u8]) -> bool) -> &[u8] {
        self.until_in(want, Duration::from_secs(15))
    }

    /// 同 [`Reader::until`]，每次读最多等 limit（原来的 secs）
    pub fn until_in(&mut self, want: impl Fn(&[u8]) -> bool, limit: Duration) -> &[u8] {
        self.s.set_read_timeout(Some(limit)).unwrap_or_else(|e| panic!("设不了读的时限：{e}"));
        while !want(&self.out) {
            let data = recv(&mut self.s, 65536);
            if data.is_empty() {
                let tail = &self.out[self.out.len().saturating_sub(300)..];
                panic!("连接断了，已收到的输出：{:?}", String::from_utf8_lossy(tail));
            }
            self.buf.extend_from_slice(&data);
            loop {
                match self.ch.open(&mut self.buf) {
                    Ok(Some(rec)) => {
                        self.plain.extend_from_slice(&rec);
                        self.raw.extend_from_slice(&rec);
                    }
                    Ok(None) => break,
                    Err(why) => panic!("通道上的一条记录不对：{why}"),
                }
            }
            while self.plain.len() >= 5 {
                let n = u32::from_be_bytes([self.plain[1], self.plain[2], self.plain[3], self.plain[4]]) as usize;
                if self.plain.len() < 5 + n {
                    break;
                }
                let kind = self.plain[0];
                let payload: Vec<u8> = self.plain[5..5 + n].to_vec();
                self.plain.drain(..5 + n);
                self.kinds.insert(kind);
                if kind == OUTPUT {
                    self.out.extend_from_slice(&payload);
                } else if kind == EXIT {
                    // 退出码是内容的头 4 个字节（大端）；不够 4 个就是有的那些
                    let code = payload.iter().take(4).fold(0u32, |acc, b| acc << 8 | *b as u32);
                    self.out.extend_from_slice(format!("<EXIT {code}>").as_bytes());
                }
            }
        }
        &self.out
    }
}
