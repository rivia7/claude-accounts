//! 和中转服务器打交道（格式见 server/README.md「远程会话的中转」）。中转服务器不止一台：每个 Team 的会话走这个 Team 自己的
//! 同步服务器（开着中转的话），维护者映射里的账号的会话走维护者的那一台（旧的通道），别的（这台设备自己的、第三方 API 的）走
//! 这台机器的「家」（`home_hub`：加入的第一个 Team，没有时是维护者的）——`session_hub`。配对是在某一台服务器上做的、记在它上面
//! （`Peer::hub`），经它只看得到走这台服务器的会话。
//! - 请求（配对、会话的列表、进数据端口的票据）走设备统计已有的那条路：deploy key 签名、curl 钉住证书公钥、直连
//!   （`presence_post`），这里只是换了几个 op
//! - 数据端口是普通的 TCP：连上先说一行（`crypto::relay_line`），凭票据进去。上面跑的是两头之间端到端加密的通道
//!   （`Tunnel`），服务器只转字节
//!
//! 辅助程序在 Linux 上没有 TLS，所以数据不走 HTTPS；也不经 curl 传（按键要一个个马上到）。

use super::crypto::{self, Channel, Role};
use super::local;
use super::peers::Peer;
use crate::cmd::hub::{route, HubKind, TeamTag};
use crate::cmd::presence::{presence_post, presence_server, presence_why};
use crate::core::policy::policy_sources;
use crate::core::team::teams;
use crate::core::tool::{tools, Using};
use crate::py::base64::b64decode;
use crate::py::fs::load_quiet;
use crate::py::obj::Dict;
use crate::py::rand::token_hex;
use crate::py::time::monotonic;
use crate::py::R;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

/// 服务器不认这几个 op（旧版，或者没开中转）
pub const RELAY_OLD: &str = "维护者的服务器还不支持远程会话（要更新服务器、打开中转：./policy.sh presence deploy）";

/// 服务器没开网页端（或者是旧版，不认 `pair_open` 的 web 这一项）：手机上接入不了
pub const NO_WEB: &str =
    "维护者的服务器还没有开网页端，手机上接入不了（要更新服务器、打开网页端：./policy.sh presence deploy --web-url …）";

/// 请求没成：服务器回的状态码和 error，加上给人看的原因
#[derive(Debug)]
pub struct Fault {
    pub code: i64,
    pub error: String,
    pub text: String,
}

/// 一台中转服务器（维护者的设备统计服务器，或者某个 Team 的同步服务器）：配置和它的主机名
pub struct Server {
    cfg: Dict,
    host: String,
    /// 这台机器在这台服务器上的设备指纹
    pub fp: String,
    /// 是哪一台：配对记在它上面（`Peer::hub`）
    pub kind: HubKind,
}

fn build(kind: HubKind, cfg: Dict) -> Result<Server, String> {
    let url = cfg.at("url").py_str();
    let rest = url.split_once("://").map_or(url.as_str(), |x| x.1);
    let hostport = rest.split('/').next().unwrap_or("");
    // [::1]:18443、example.com:18443、example.com
    let host = match hostport.strip_prefix('[').and_then(|h| h.split_once(']')) {
        Some((h, _)) => h.to_string(),
        None => hostport.rsplit_once(':').map_or(hostport, |x| x.0).to_string(),
    };
    if host.is_empty() {
        return Err(format!("{}的地址不对", label(kind)));
    }
    let fp = cfg.at("fp").py_str();
    Ok(Server { cfg, host, fp, kind })
}

/// 给人看的：这是谁的服务器
pub fn label(kind: HubKind) -> String {
    match kind {
        HubKind::Team(tag) => {
            let tid = tag.id();
            let name = teams().into_iter().find(|t| t.id == tid).map(|t| t.name).filter(|n| !n.is_empty());
            format!("「{}」的服务器", name.unwrap_or_else(|| format!("Team {}", &tid[..8])))
        }
        _ => "维护者的服务器".into(),
    }
}

/// 某一台中转服务器。用得了的话是 Ok；用不了是 Err(原因)：维护者的——没配置映射、映射里没有服务器、没有 deploy key；
/// Team 的——退出了、还没取到它的配置、它没接上服务器
pub fn server_of(kind: HubKind) -> R<Result<Server, String>> {
    match kind {
        HubKind::Team(tag) => {
            let tid = tag.id();
            Ok(match crate::cmd::team::hub_cfgs().into_iter().find(|(t, _)| *t == tid) {
                Some((_, cfg)) => build(kind, cfg),
                None => {
                    Err(format!("这台机器现在用不了{}（退出了这个 Team、还没取到它的配置，或者它还没接上服务器）", label(kind)))
                }
            })
        }
        HubKind::Maintainer => {
            let (why, cfg) = presence_server()?;
            Ok(match cfg {
                Some(cfg) => build(kind, cfg),
                None => Err(why),
            })
        }
    }
}

/// 这台机器现在用得了的中转服务器：各个 Team 的（照加入的先后），再是维护者的（旧的通道）。开没开中转要问了才知道
pub fn servers() -> R<Vec<Server>> {
    let mut out = Vec::new();
    for (tid, cfg) in crate::cmd::team::hub_cfgs() {
        if let Some(tag) = TeamTag::new(&tid) {
            if let Ok(s) = build(HubKind::Team(tag), cfg) {
                out.push(s);
            }
        }
    }
    if let Ok(s) = server_of(HubKind::Maintainer)? {
        out.push(s);
    }
    Ok(out)
}

/// 这台机器的「家」：不是哪个 Team 给的、也不在维护者映射里的账号（这台设备自己的、第三方 API 的）的会话走它，没说在哪台上配对时
/// 也在它上面配。是加入的第一个 Team 的服务器（照加入的先后，这台机器用得了的：取到了配置、接上了服务器），一个都没有时是维护者的
/// 那一台。开没开中转不看（要问了才知道）：家里的那一台没开中转时，这些会话就接不进去（cca remote list 说是哪一台）
pub fn home_hub() -> HubKind {
    crate::cmd::team::hub_cfgs().into_iter().find_map(|(tid, _)| TeamTag::new(&tid)).map_or(HubKind::Maintainer, HubKind::Team)
}

/// 这个账号的会话走哪台服务器中转：账号是哪个 Team 给的就走那个 Team 的；维护者映射里的走维护者的那一台（旧的通道）；别的（这台
/// 设备自己的、第三方 API 的、账号表里没有的）走这台机器的家（home_hub）
pub fn session_hub(tool: &str, key: &str) -> HubKind {
    for t in tools() {
        if t.name != tool {
            continue;
        }
        let Some(v) = load_quiet(&t.table).and_then(|v| v.as_dict()).and_then(|d| d.get(key)) else { break };
        let _using = Using::tool(t);
        if let Some(kind) = route(&policy_sources(t), key, &v) {
            return kind;
        }
        break;
    }
    home_hub()
}

/// 进数据端口的票据（30 秒内、只能用一次）
pub struct Ticket {
    pub tid: String,
    pub key: Vec<u8>,
    pub port: u16,
}

impl Server {
    /// 发一个请求：Ok(回的内容)，或者 Err(没成的原因)
    pub fn post(&self, op: &str, payload: &Dict, timeout: i64) -> R<Result<Dict, Fault>> {
        let (code, j) = presence_post(&self.cfg, op, payload, timeout)?;
        if code == 200 {
            return Ok(Ok(j));
        }
        let error = j.str("error").map(|e| e.to_string()).unwrap_or_default();
        let text = match (code, error.as_str()) {
            (404, "not_found") => self.old(),
            (404, "no_pairing") => "服务器上没有这个配对（配对码过期了，或者对方取消了配对）".into(),
            (404, "no_web") => self.no_web(),
            (404, "no_session") => "那台机器上现在没有这个会话（它结束了，或者那边不让这台机器接入）".into(),
            (409, "taken") => "这个配对码已经有另一台机器在用了：重新显示一个".into(),
            (409, "mismatch") => "配对没对上（两边算出来的不一样）：重新来一次".into(),
            (429, "busy") => "这个会话接入的已经够多了".into(),
            (429, "edges") => "配对的机器太多了：先取消几个不用的（cca remote unpair）".into(),
            (429, "sessions") => "这台机器开着的可以接入的会话太多了".into(),
            _ => presence_why(&self.cfg, code, &j)?,
        };
        Ok(Err(Fault { code, error, text }))
    }

    /// 这台服务器不认中转的那几个 op（旧版，或者没开中转）
    pub fn old(&self) -> String {
        match self.kind {
            HubKind::Team(_) => {
                format!("{}还没开远程会话的中转（管理员要重新部署、把中转打开：cca team deploy）", label(self.kind))
            }
            _ => RELAY_OLD.into(),
        }
    }

    /// 这台服务器没开网页端
    pub fn no_web(&self) -> String {
        match self.kind {
            HubKind::Team(_) => {
                format!("{}还没有开网页端，手机上接入不了（管理员要重新部署、把网页端打开：cca team deploy）", label(self.kind))
            }
            _ => NO_WEB.into(),
        }
    }

    pub fn ticket(&self, op: &str, payload: &Dict) -> R<Result<Ticket, Fault>> {
        let j = match self.post(op, payload, 10)? {
            Ok(j) => j,
            Err(f) => return Ok(Err(f)),
        };
        let got = (|| {
            Some(Ticket {
                tid: j.str("tid")?.to_string(),
                key: b64decode(&j.str("tkey")?, true).ok().filter(|k| k.len() == 32)?,
                port: u16::try_from(j.at("port").as_int()?).ok().filter(|p| *p > 0)?,
            })
        })();
        Ok(got.ok_or_else(|| Fault { code: 200, error: "bad".into(), text: "服务器回的票据看不懂".into() }))
    }

    /// 连数据端口、说开场白、等它回 OK（接入方要等宿主那一头连上来，所以最多等 wait 秒）。
    /// role：`host` / `view` 用票据，`join` 用控制连接上收到的令牌（ident）和控制连接那张票据的密钥
    pub fn dial(&self, port: u16, role: &str, ident: &str, key: &[u8], wait: f64) -> io::Result<TcpStream> {
        let addr = (self.host.as_str(), port).to_socket_addrs()?.next().ok_or_else(|| io::Error::other("解析不了服务器的地址"))?;
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(6))?;
        s.set_nodelay(true)?;
        s.set_write_timeout(Some(Duration::from_secs(6)))?;
        s.set_read_timeout(Some(Duration::from_secs_f64(wait)))?;
        let (line, expect) = crypto::relay_line(role, ident, key, &token_hex(8));
        s.write_all(line.as_bytes())?;
        // 一个字节一个字节读到换行：后面紧跟着的就是对面的话，不能多读
        let mut got = Vec::new();
        let mut b = [0u8; 1];
        while got.last() != Some(&b'\n') && got.len() < 128 {
            if s.read(&mut b)? == 0 {
                break;
            }
            got.push(b[0]);
        }
        if got != expect.as_bytes() {
            let said = String::from_utf8_lossy(&got).trim().to_string();
            return Err(io::Error::other(match said.strip_prefix("CCA-RELAY/1 ERR ") {
                // 它说的原因只认这几个词，别的不原样显示（那是服务器发来的字节，会写到终端上）
                Some("nohost") => "那台机器现在连不上（它不在线，或者会话刚结束）",
                Some("ticket") => "票据过期了",
                Some(_) => "服务器不让进",
                None => "数据端口那一头不是中转服务器",
            }));
        }
        Ok(s)
    }
}

/// 读够 n 个字节（握手用：流还是阻塞的，带着超时）；对面先关了是 Err，读到的不够的那一截带在错误里看不到
fn read_exact_or(s: &mut TcpStream, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    let mut have = 0;
    while have < n {
        match s.read(&mut buf[have..])? {
            0 => break,
            k => have += k,
        }
        // 对面拒绝时只说 4 个字节就关：`CR 7F 码`
        if have >= 4 && buf[..3] == [b'C', b'R', 0x7f] {
            return Err(io::Error::other(refusal(buf[3])));
        }
    }
    if have < n {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "握手到一半连接断了"));
    }
    Ok(buf)
}

/// 握手时拒绝对方的原因
pub const REFUSE_UNKNOWN: u8 = 1;
pub const REFUSE_VERSION: u8 = 4;

fn refusal(code: u8) -> &'static str {
    match code {
        REFUSE_UNKNOWN => "那台机器不认这个配对（它取消了配对，或者刚改成不让这台机器接入）",
        REFUSE_VERSION => "两台机器的 cca 版本差太多：都更新到最新的",
        _ => "那台机器拒绝了",
    }
}

fn refuse(s: &mut TcpStream, code: u8) {
    let _ = s.write_all(&[b'C', b'R', 0x7f, code]);
}

/// 接入方的握手：证明自己有配对的密钥，也核对对面有（没有的话什么都不发给它）。sid 是要接入的会话
pub fn handshake_viewer(s: &mut TcpStream, peer: &Peer, p: &[u8], sid: &str, my_fp: &str) -> io::Result<Channel> {
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut chan =
        Channel::new(Role::Viewer, p, &peer.pid, sid, &peer.fp, my_fp).ok_or_else(|| io::Error::other("会话的编号不对"))?;
    s.write_all(&chan.hello())?;
    let reply = read_exact_or(s, crypto::REPLY_LEN)?;
    let confirm = chan.confirm(&reply).ok_or_else(|| io::Error::other("对面证明不了它是配对过的那台机器：没有接入"))?;
    s.write_all(&confirm)?;
    Ok(chan)
}

/// 宿主的握手：看对面说的是哪个配对，allowed 给出这个配对的另一台机器和密钥（不让它接入时给 None）；
/// 核对它确实有这把密钥。返回通道和对上的那台机器
pub fn handshake_host<'a>(
    s: &mut TcpStream,
    sid: &str,
    my_fp: &str,
    allowed: impl Fn(&str) -> Option<(&'a Peer, Vec<u8>)>,
) -> io::Result<(Channel, &'a Peer)> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let hello = read_exact_or(s, crypto::HELLO_LEN)?;
    if hello[..3] == [b'C', b'R', 1] && hello[3] != crypto::VER {
        refuse(s, REFUSE_VERSION);
        return Err(io::Error::other("版本不一样"));
    }
    let Some((peer, p)) = Channel::hello_pid(&hello).and_then(|pid| allowed(&pid)) else {
        refuse(s, REFUSE_UNKNOWN);
        return Err(io::Error::other("不认识的配对"));
    };
    let mut chan =
        Channel::new(Role::Host, &p, &peer.pid, sid, my_fp, &peer.fp).ok_or_else(|| io::Error::other("会话的编号不对"))?;
    let reply = chan.reply(&hello).ok_or_else(|| io::Error::other("hello 不对"))?;
    s.write_all(&reply)?;
    let confirm = read_exact_or(s, crypto::CONFIRM_LEN)?;
    if !chan.accept(&confirm) {
        return Err(io::Error::other("对面证明不了它有配对的密钥"));
    }
    Ok((chan, peer))
}

/// 每隔这么久说一声（空的记录），免得中间的设备把闲着的连接掐了
pub const PING_EVERY: f64 = 20.0;
/// 这么久什么都没收到算断了
pub const DEAD_AFTER: f64 = 60.0;

/// 握手以后的加密通道，接在一条不阻塞的连接上：`send` 排队、`flush` 写出去，`recv` 读到多少解多少
pub struct Tunnel {
    stream: TcpStream,
    chan: Channel,
    inbuf: Vec<u8>,
    out: VecDeque<u8>,
    heard: f64,
    said: f64,
}

impl Tunnel {
    pub fn new(stream: TcpStream, chan: Channel) -> io::Result<Tunnel> {
        stream.set_nonblocking(true)?;
        let now = monotonic();
        Ok(Tunnel { stream, chan, inbuf: Vec::new(), out: VecDeque::new(), heard: now, said: now })
    }

    pub fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    /// 还没写出去的有多少
    pub fn pending(&self) -> usize {
        self.out.len()
    }

    /// 排进去（分成一条条记录）；真正写出去在 `flush`
    pub fn send(&mut self, data: &[u8]) {
        for part in data.chunks(crypto::RECORD_MAX) {
            let rec = self.chan.seal(part);
            self.out.extend(rec);
        }
        self.said = monotonic();
    }

    pub fn flush(&mut self) -> io::Result<()> {
        local::write_queue(&mut self.stream, &mut self.out)
    }

    /// 读现在有的：解出来的内容（可能是空的：还不够一条，或者只是对面说了一声）；Ok(None) 是对面关了；
    /// 序号、认证不对是 Err（有人动了这条连接：断开）。出错之前已经解出来的先交出去（对面说完“会话结束了”
    /// 马上断开时那一句不能丢），错误下一次再报
    pub fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 32 * 1024];
        let mut plain = Vec::new();
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) if plain.is_empty() => return Ok(None),
                Ok(0) => break,
                Ok(n) => self.inbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if !plain.is_empty() => break,
                Err(e) => return Err(e),
            }
            // 读一截解一截：没解的最多剩一条不完整的记录
            loop {
                match self.chan.open(&mut self.inbuf) {
                    Ok(Some(data)) => {
                        self.heard = monotonic();
                        plain.extend(data);
                    }
                    Ok(None) => break,
                    Err(()) if !plain.is_empty() => return Ok(Some(plain)), // 坏的那一条还在 inbuf 里：下一次再报
                    Err(()) => return Err(io::Error::new(io::ErrorKind::InvalidData, "记录的序号或者认证不对")),
                }
            }
            if plain.len() >= 1024 * 1024 {
                break; // 先交出去这些
            }
        }
        Ok(Some(plain))
    }

    /// 该说一声了就说一声；false：对面太久没动静了
    pub fn tick(&mut self) -> bool {
        let now = monotonic();
        if now - self.said >= PING_EVERY && self.out.is_empty() {
            self.send(b"");
        }
        now - self.heard < DEAD_AFTER
    }

    /// 这条连接不用了（断了以后要另连一条）：两个方向都关掉，服务器和对面马上知道，不用等它们自己发现
    pub fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// 收场：把还没写出去的写完（最多等 secs 秒）
    pub fn drain(&mut self, secs: f64) {
        let end = monotonic() + secs;
        while !self.out.is_empty() && monotonic() < end {
            let mut p = libc::pollfd { fd: self.fd(), events: libc::POLLOUT, revents: 0 };
            // SAFETY: p 是有效的一项
            unsafe { libc::poll(&mut p, 1, 100) };
            if self.flush().is_err() {
                break;
            }
        }
    }
}

/// 控制连接上的帧：`类型(1) 长度(2) 内容`。服务器说 JOIN（有接入方来了）和 PONG（0x07：PING 的回答，
/// 收到什么都算它还在，不用单独认）；宿主这头说 PING、ANNOUNCE、BYE
pub const F_JOIN: u8 = 0x02;
pub const F_PING: u8 = 0x06;
pub const F_ANNOUNCE: u8 = 0x08;
pub const F_BYE: u8 = 0x09;

pub fn ctl_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// buf 开头的一帧（拿掉）；还不够是 None
pub fn ctl_next(buf: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    if buf.len() < 3 {
        return None;
    }
    let n = u16::from_be_bytes([buf[1], buf[2]]) as usize;
    if buf.len() < 3 + n {
        return None;
    }
    let frame = (buf[0], buf[3..3 + n].to_vec());
    buf.drain(..3 + n);
    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::remote::peers::Incoming;
    use std::net::TcpListener;

    #[test]
    fn control_frames() {
        let mut buf = ctl_frame(F_PING, b"abc");
        buf.extend(ctl_frame(F_BYE, b""));
        buf.extend_from_slice(&[F_JOIN, 0, 5, b'x']);
        assert_eq!(ctl_next(&mut buf), Some((F_PING, b"abc".to_vec())));
        assert_eq!(ctl_next(&mut buf), Some((F_BYE, Vec::new())));
        assert_eq!(ctl_next(&mut buf), None, "这一帧还没到齐");
        assert_eq!(buf.len(), 4);
    }

    /// 两头真的握手、通道接在真的连接上：分块到、大块的、对面关了、被人改了
    #[test]
    fn handshake_and_tunnel_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let p = [7u8; 32];
        let pid = crypto::pid_of(&p);
        let sid = "00112233445566778899aabbccddeeff";
        let (fph, fpv) = ("SHA256:host", "SHA256:viewer");
        let peer_of_host = Peer {
            pid: pid.clone(),
            fp: fpv.into(),
            name: "v".into(),
            incoming: Incoming::Control,
            shown: true,
            at: 0.0,
            web: false,
            hub: HubKind::Maintainer,
        };
        let peer_of_viewer = Peer {
            pid: pid.clone(),
            fp: fph.into(),
            name: "h".into(),
            incoming: Incoming::None,
            shown: false,
            at: 0.0,
            web: false,
            hub: HubKind::Maintainer,
        };

        let host = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let allowed = |q: &str| (q == peer_of_host.pid).then(|| (&peer_of_host, p.to_vec()));
            let (chan, who) = handshake_host(&mut s, sid, fph, allowed).unwrap();
            assert_eq!(who.name, "v");
            let mut t = Tunnel::new(s, chan).unwrap();
            let mut got = Vec::new();
            while got.len() < 100_000 + 5 {
                match t.recv() {
                    Ok(Some(data)) => got.extend(data),
                    Ok(None) => break,
                    Err(e) => panic!("{e}"),
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            t.send(&got[..5]);
            t.send(&got[5..]);
            t.drain(5.0);
            got.len()
        });

        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let chan = handshake_viewer(&mut s, &peer_of_viewer, &p, sid, fpv).unwrap();
        let mut t = Tunnel::new(s, chan).unwrap();
        let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        t.send(b"hello");
        t.send(&big);
        let mut back = Vec::new();
        let end = monotonic() + 10.0;
        while back.len() < 100_005 && monotonic() < end {
            t.flush().unwrap();
            if let Some(data) = t.recv().unwrap() {
                back.extend(data);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(host.join().unwrap(), 100_005);
        assert_eq!(&back[..5], b"hello");
        assert_eq!(&back[5..], &big[..]);
        // 对面关了：读到头
        let end = monotonic() + 5.0;
        loop {
            match t.recv().unwrap() {
                None => break,
                Some(_) if monotonic() > end => panic!("对面关了却没读到头"),
                Some(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn host_refuses_unknown_pairings_and_wrong_secrets() {
        let p = [7u8; 32];
        let pid = crypto::pid_of(&p);
        let sid = "00112233445566778899aabbccddeeff";
        let peer = |fp: &str| Peer {
            pid: pid.clone(),
            fp: fp.into(),
            name: "x".into(),
            incoming: Incoming::Control,
            shown: true,
            at: 0.0,
            web: false,
            hub: HubKind::Maintainer,
        };
        // 宿主那边没有这个配对：说一声“不认识”就关，接入方知道原因
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let host = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            handshake_host(&mut s, sid, "SHA256:host", |_| None).is_err()
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let e = handshake_viewer(&mut s, &peer("SHA256:host"), &p, sid, "SHA256:viewer").err().expect("握手不该成");
        assert!(e.to_string().contains("不认这个配对"), "{e}");
        assert!(host.join().unwrap());
        // 接入方的密钥不对：它先发现对面“证明不了”（对面用的是真的那把），不再往下；宿主那边握手也没成
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hp = peer("SHA256:viewer");
        let host = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            handshake_host(&mut s, sid, "SHA256:host", |_| Some((&hp, p.to_vec()))).is_err()
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let e = handshake_viewer(&mut s, &peer("SHA256:host"), &[8u8; 32], sid, "SHA256:viewer").err().expect("握手不该成");
        assert!(e.to_string().contains("证明不了"), "{e}");
        drop(s);
        assert!(host.join().unwrap());
    }
}
