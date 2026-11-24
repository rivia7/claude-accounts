//! 假的 Gmail（原来的 tests/mail_fake.py）：邮箱助手（cca mail）的测试替身，不联网。
//!
//! - 测试用的证书（[`make_pki`]）：一个 CA 和它签的 imap.gmail.com 的证书（EC P-256，openssl 生成）
//! - 假的 Gmail（[`Server`]）：真的 TLS（[`crate::tlsd`]），只实现 cca 用得到的那一点 IMAP（CAPABILITY、LOGIN、LIST、EXAMINE、
//!   UID SEARCH、UID FETCH、LOGOUT、NOOP）；会改邮箱的命令（SELECT、STORE、COPY、MOVE、EXPUNGE、APPEND、CREATE……）和不带 PEEK 的
//!   BODY[]（会标已读）一律回 BAD，并在日志里记一行 VIOLATION
//! - 假的代理（[`Proxy`]）：一个端口同时认 SOCKS5 和 HTTP CONNECT，不管要连哪个主机都接到假 Gmail 上，日志里记下要连的目标
//! - 登录邮件（[`Eml`]、[`put_eml`]）：Anthropic 的样子（DKIM 签名、Gmail 写的 Authentication-Results、HTML 和纯文本），各处都能改；
//!   随机的部分（记号、签名、Message-ID、分隔线）和 Date 的时间从 [`Source`] 来：平时 [`Random`]，要逐字节可比时 [`Pinned`]
//!
//! 邮箱放在 目录/box/<邮件夹>/<UID>.eml（all、spam、inbox），Gmail 收到的时间（INTERNALDATE）取文件的修改时间；每次 EXAMINE、SEARCH、
//! FETCH 都重新读目录，测试往里放文件就是来了新邮件。目录/ctl.json 改假 Gmail 的行为（每个命令都重新读）：
//!   user、password（默认 inbox@example.com / abcdefghijklmnop）、imap_disabled、login_no（LOGIN 一律回 NO 和这段话）、
//!   no_all（没有 \All）、index_lag（这么多秒以内收到的 SEARCH 搜不到：模拟 Gmail 的索引还没跟上）、
//!   uid_after_literal（FETCH 的 UID 写在 literal 后面）、drop（收到的命令里有这段文字时直接断开连接）、
//!   proxy（refuse：代理不肯连；auth：SOCKS5 要认证）
//! 日志在 目录/imap.log，一行一条：IMAP 命令 参数（LOGIN 的密码换成 ***）、DROP、VIOLATION、PROXY socks5|http 目标——测试按字 grep 它们。
//!
//! 在测试里用（和原来 mail_view.py 的用法一一对应）：
//!
//! ```ignore
//! let ca = make_pki(&dir.join("pki"))?;                 // mf.make_pki(…)
//! for (k, _, _) in FOLDERS { fs::create_dir_all(dir.join("box").join(k))?; }
//! let srv = Server::start(&dir)?;                       // mf.Server(dir) + start()；丢掉它或 close() 就不再接新连接
//! let px = Proxy::start(&srv)?;                         // mf.Proxy(srv) + start()；px.port
//! let raw = Eml::new("alice@example.com").ar(Ar::Fail).link("https://claude.ai/magic-link#tok-X").build();
//! put_eml(&dir.join("box/all/3.eml"), &raw, Some(now - 60.0))?;   // mf.put_eml(…, time.time() - 60)
//! ```
//!
//! 命令行（regress 用，tests/tool/mail.rs）：`cca-test mail serve 目录`（起假 Gmail 和代理，端口写进 目录/ports.json，一直跑到被 kill）、
//! `cca-test mail eml 文件 --to 地址 [--link …] [--subject …] [--sign …] [--ar …] [--from …] [--dkim-d …] [--reply-to …] [--at …] [--msgid …]`。

/// 编一次、记下来的正则（用到才编）
macro_rules! re {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::Regex::new($p).expect("正则写错了"))
    }};
}

/// 同上，按字节匹配的
macro_rules! re_bytes {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::bytes::Regex::new($p).expect("正则写错了"))
    }};
}

mod eml;
mod hdr;
mod imap;
mod pki;
mod proxy;
mod py;

pub use eml::{formatdate, good_link, put_eml, Ar, Eml, Pinned, Random, Source, ALNUM, B64};
pub use hdr::{hdr_text, header_fields};
pub use imap::{idate, search_eval, tokens, uid_set, Tok};
pub use pki::make_pki;
pub use proxy::Proxy;

use crate::json::{self, Obj, Value};
use crate::tlsd::Acceptor;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// float(字符串)：Python 的写法（前后的空白、inf、nan、指数、数字之间的下划线都认）；命令行的 --at 用
pub fn py_float(s: &str) -> Option<f64> {
    py::float(s)
}

/// 邮件夹：(目录名, IMAP 里的名字, LIST 回的属性)
pub const FOLDERS: [(&str, &str, &str); 3] = [
    ("inbox", "INBOX", r"\HasNoChildren"),
    ("all", "[Gmail]/All Mail", r"\All \HasNoChildren"),
    ("spam", "[Gmail]/Spam", r"\HasNoChildren \Junk"),
];

/// IMAP 里的名字（可以带引号）→ 目录名
pub fn folder_dir(name: &str) -> Option<&'static str> {
    FOLDERS.iter().find(|(_, imap, _)| name == *imap || name == format!("\"{imap}\"")).map(|(k, _, _)| *k)
}

/// ctl.json 的内容（补上了 user、password 的默认值）
pub type Ctl = Obj;

/// 邮箱里的一封：UID、Gmail 收到的时间（文件的修改时间）、原样的字节
#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub uid: i128,
    pub t: f64,
    pub raw: Vec<u8>,
}

/// 原来抛出的异常：这一个连接断开
pub use imap::Abort;

/// 假 Gmail 的目录（原来的 Box）
#[derive(Clone, Debug)]
pub struct Mailbox {
    pub d: PathBuf,
}

impl Mailbox {
    pub fn new(d: &Path) -> Mailbox {
        Mailbox { d: d.to_path_buf() }
    }

    /// ctl.json（读不出来、不是 JSON 是空的）加上 user、password 的默认值。内容不是对象时是原来的 AttributeError
    pub fn ctl(&self) -> Result<Ctl, Abort> {
        let mut c = match fs::read(self.d.join("ctl.json")).ok().and_then(|b| json::parse_bytes(&b).ok()) {
            None => Obj::new(),
            Some(Value::Obj(o)) => o,
            Some(_) => return Err(Abort),
        };
        if !c.contains_key("user") {
            c.insert("user", "inbox@example.com");
        }
        if !c.contains_key("password") {
            c.insert("password", "abcdefghijklmnop");
        }
        Ok(c)
    }

    /// 这个邮件夹里的邮件，按 UID 排。文件名不是 数字.eml 的不算、读不了的跳过；数字长得 Python 转不了的是 Err（ValueError 的说法）
    pub fn msgs(&self, key: &str) -> Result<Vec<Msg>, String> {
        let d = self.d.join("box").join(key);
        let names: Vec<_> = match fs::read_dir(&d).and_then(|rd| rd.collect::<io::Result<Vec<_>>>()) {
            Ok(es) => es.into_iter().map(|e| e.file_name()).collect(),
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for n in names {
            let Some(name) = n.to_str() else { continue };
            let Some(m) = re!(r"^([0-9]+)\.eml\n?$").captures(name) else { continue };
            let p = d.join(name);
            // 先打开（目录、读不了的跳过），再转 UID、看修改时间、读内容：和原来的先后一样
            let Ok(mut f) = File::open(&p) else { continue };
            if f.metadata().is_ok_and(|md| md.is_dir()) {
                continue;
            }
            let uid = py::int(&m[1])?;
            let Ok(md) = fs::metadata(&p) else { continue };
            let t = md.mtime() as f64 + md.mtime_nsec() as f64 * 1e-9;
            let mut raw = Vec::new();
            if f.read_to_end(&mut raw).is_err() {
                continue;
            }
            out.push(Msg { uid, t, raw });
        }
        out.sort_by(|a, b| a.uid.cmp(&b.uid).then(a.t.total_cmp(&b.t)).then(a.raw.cmp(&b.raw)));
        Ok(out)
    }

    /// box/<邮件夹>/.uidvalidity 里的数，没有、写错了是 11
    pub fn uidvalidity(&self, key: &str) -> i128 {
        fs::read(self.d.join("box").join(key).join(".uidvalidity"))
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| py::int(&s).ok())
            .unwrap_or(11)
    }
}

/// 假 Gmail 和假代理共用的：目录、写日志的锁、IMAP 的端口
pub(crate) struct Shared {
    mb: Mailbox,
    lock: Mutex<()>,
    port: u16,
}

impl Shared {
    /// imap.log 里记一行
    fn log(&self, s: &str) -> io::Result<()> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = OpenOptions::new().append(true).create(true).open(self.mb.d.join("imap.log"))?;
        f.write_all(format!("{s}\n").as_bytes())
    }
}

/// 跑着的假 Gmail（127.0.0.1，真的 TLS，证书是 目录/pki/cert.pem 和 key.pem）。一个连接一个线程；
/// close()（或者丢掉它）以后不再接新的连接，已经接上的照常到对方断开
pub struct Server {
    /// IMAP 的端口
    pub port: u16,
    /// 假 Gmail 的目录（box/、ctl.json、imap.log 在里面）
    pub dir: PathBuf,
    sh: Arc<Shared>,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// 在随便一个空闲端口上起
    pub fn start(dir: &Path) -> io::Result<Server> {
        Server::start_on(dir, 0)
    }

    /// 在指定的端口上起（0 是随便一个空闲的）
    pub fn start_on(dir: &Path, port: u16) -> io::Result<Server> {
        let tls = Acceptor::from_pem_files(&dir.join("pki").join("cert.pem"), &dir.join("pki").join("key.pem"))?;
        let ls = TcpListener::bind(("127.0.0.1", port))?;
        let port = ls.local_addr()?.port();
        let sh = Arc::new(Shared { mb: Mailbox::new(dir), lock: Mutex::new(()), port });
        let stop = Arc::new(AtomicBool::new(false));
        let (sh2, flag) = (sh.clone(), stop.clone());
        thread::spawn(move || {
            for s in ls.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(s) = s else { return };
                let (sh, tls) = (sh2.clone(), tls.clone());
                thread::spawn(move || conn(sh, &tls, s));
            }
        });
        Ok(Server { port, dir: dir.to_path_buf(), sh, stop })
    }

    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.sh.clone()
    }

    /// 这个假 Gmail 的目录
    pub fn mailbox(&self) -> &Mailbox {
        &self.sh.mb
    }

    /// 不再接新的连接（原来的 ls.close()）
    pub fn close(&self) {
        if !self.stop.swap(true, Ordering::SeqCst) {
            // 叫醒还等在 accept 上的线程，它看到 stop 就退出、关掉监听的端口
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

/// 读一行（带 \n）；对方关了时是剩下的那一截，什么都没有了是 None。没有 close_notify 就断开也当作读完了（ssl 的 suppress_ragged_eofs）
fn read_line(st: &mut impl Read, buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(i) = buf.iter().position(|b| *b == b'\n') {
            return Some(buf.drain(..=i).collect());
        }
        match st.read(&mut chunk) {
            Ok(0) => return (!buf.is_empty()).then(|| std::mem::take(buf)),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return (!buf.is_empty()).then(|| std::mem::take(buf)),
            Err(_) => return None,
        }
    }
}

/// 一个 IMAP 连接（原来的 Conn.run）：握手、欢迎、一行一行地处理，出错就断开（不发 close_notify）
fn conn(sh: Arc<Shared>, tls: &Acceptor, sock: TcpStream) {
    let Ok(mut st) = tls.accept(sock) else { return };
    let mut sess = imap::Session::new(sh);
    if sess.greet(&mut st).is_err() {
        return;
    }
    let mut buf = Vec::new();
    while let Some(line) = read_line(&mut st, &mut buf) {
        let mut l = &line[..];
        while let Some(b'\r' | b'\n') = l.last() {
            l = &l[..l.len() - 1];
        }
        let text = String::from_utf8_lossy(l);
        if !matches!(sess.handle(&text, &mut st), Ok(true)) {
            break;
        }
    }
}

/// 原来的 serve(d) 去掉最后一直等的那一步：没有证书先生成，建好三个邮件夹，起假 Gmail 和代理，端口写进 目录/ports.json
/// （{"imap": …, "proxy": …}，写好了再换上去）。两个都丢掉以后就不再接新的连接
pub fn serve_dir(d: &Path) -> io::Result<(Server, Proxy)> {
    if !d.join("pki").join("cert.pem").exists() {
        make_pki(&d.join("pki"))?;
    }
    for (key, _, _) in FOLDERS {
        fs::create_dir_all(d.join("box").join(key))?;
    }
    let srv = Server::start(d)?;
    let px = Proxy::start(&srv)?;
    let tmp = d.join("ports.json.tmp");
    fs::write(&tmp, format!("{{\"imap\": {}, \"proxy\": {}}}", srv.port, px.port))?;
    fs::rename(&tmp, d.join("ports.json"))?;
    Ok((srv, px))
}
