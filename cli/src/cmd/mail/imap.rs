//! 只读的 IMAP：原来是 Python 的 imaplib（`mail_imap`）加上包着它的 `MailBox`、`MailSource`。
//!
//! `Imap` 照 CPython 3.14 的 imaplib 手写，只有邮箱助手用到的那几条命令：开头的 CAPABILITY、LOGIN、LIST、EXAMINE、
//! UID SEARCH、UID FETCH、LOGOUT。不 SELECT、不 STORE、不删不移，也没有通用的命令入口。发出去的字节（命令的写法、引号）、
//! 回应怎么读（带 `{n}` 的 literal、方括号里的回应码）、出错算哪一类（error / abort / OSError）和说法都照 imaplib：
//! `MailBox` 靠这些把错分成「过会儿再试」和「不用再试」，说法会显示给用户。出错时整个会话丢掉，下次重连。
//!
//! 应用专用密码只在 `login` 里经连接交给 Gmail，不进任何出错信息。

use super::codec::{bytes_repr, int_too_long, latin1, INT_MAX_STR_DIGITS};
use super::route::{io_text, mail_dial, mail_tls, Route};
use super::{check_interrupt, mail_fatal, mail_oserr, mail_retry, MailRule, MAIL_BURST, MAIL_FETCH_MAX, MAIL_HDRS, MAIL_MONTHS};
use crate::py::fail::os_error_text;
use crate::py::rand::urandom;
use crate::py::time::{gmtime, timegm};
use crate::py::{text, uni};
use crate::py::{Fail, R};
use crate::re;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};

/// 一行最多这么长（imaplib 的 `_MAXLINE`）
const MAXLINE: usize = 1_000_000;

/// 连好、做好 TLS 的连接（测试里是本机的明文连接）
pub trait Wire: Read + Write {
    /// 关掉（`IMAP4.shutdown`）
    fn close(self: Box<Self>);
}

impl Wire for crate::py::tls::TlsStream {
    fn close(self: Box<Self>) {
        (*self).close();
    }
}

impl Wire for std::net::TcpStream {
    fn close(self: Box<Self>) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

/// 回应里的一项数据：一行，或者带 literal 的那一对（前面的那一行, literal 的字节）；`None` 是「没有这种回应」
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    None,
    Bytes(Vec<u8>),
    Pair(Vec<u8>, Vec<u8>),
}

/// imaplib 里的几类异常
#[derive(Debug)]
pub enum ImapErr {
    /// `IMAP4.error`：这边的请求不对、服务器回了 BAD……
    Error(String),
    /// `IMAP4.abort`（`readonly` 也是它）：连接上的问题，重连再试
    Abort(String),
    /// OSError：(`e.strerror or str(e)`, 类名)——读超时、套接字出错
    Os(String, &'static str),
    /// 别的、原来没有人接的异常（参数不是 ASCII、数字太长……）
    Py(Fail),
}

impl ImapErr {
    /// `mail_oserr(e)`
    fn oserr(&self) -> String {
        match self {
            ImapErr::Error(m) => mail_oserr(m, "error"),
            ImapErr::Abort(m) => mail_oserr(m, "abort"),
            ImapErr::Os(m, class) => mail_oserr(m, class),
            ImapErr::Py(f) => f.to_string(),
        }
    }
}

type Ir<T> = Result<T, ImapErr>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Logout,
    NonAuth,
    Auth,
    Selected,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Logout => "LOGOUT",
            State::NonAuth => "NONAUTH",
            State::Auth => "AUTH",
            State::Selected => "SELECTED",
        }
    }
}

/// 这条命令在哪些状态下能发（imaplib 的 `Commands`，只留用到的）
fn allowed(name: &str) -> &'static [State] {
    match name {
        "CAPABILITY" | "LOGOUT" => &[State::NonAuth, State::Auth, State::Selected, State::Logout],
        "LOGIN" => &[State::NonAuth],
        "LIST" | "EXAMINE" => &[State::Auth, State::Selected],
        "UID" | "SEARCH" | "FETCH" => &[State::Selected],
        _ => &[],
    }
}

/// `str([…])`：列表的 repr（`b'…'`、`None`、`(b'…', b'…')`）
fn items_repr(items: &[Item]) -> String {
    let one = |i: &Item| match i {
        Item::None => "None".to_string(),
        Item::Bytes(b) => bytes_repr(b),
        Item::Pair(a, b) => format!("({}, {})", bytes_repr(a), bytes_repr(b)),
    };
    format!("[{}]", items.iter().map(one).collect::<Vec<_>>().join(", "))
}

/// `data.decode('ascii')`：有别的字节是 UnicodeDecodeError（原来没有人接）
fn ascii_strict(data: &[u8]) -> Ir<String> {
    match data.iter().position(|b| !b.is_ascii()) {
        None => Ok(data.iter().map(|b| *b as char).collect()),
        Some(i) => Err(ImapErr::Py(Fail::Other(
            "UnicodeDecodeError",
            vec![format!("'ascii' codec can't decode byte 0x{:02x} in position {}: ordinal not in range(128)", data[i], i)],
        ))),
    }
}

/// 回应的类型名：`[A-Z-]+`（tagged 的那种不带连字符）
fn type_len(data: &[u8], dash: bool) -> usize {
    data.iter().take_while(|b| b.is_ascii_uppercase() || (dash && **b == b'-')).count()
}

/// `( (?P<data>.*))?`：后面是空格就是数据，不然没有数据
fn after_space(data: &[u8]) -> Option<Vec<u8>> {
    data.strip_prefix(b" ").map(|d| d.to_vec())
}

/// `.*{(?P<size>\d+)}$`：这一行以 `{数字}` 结尾，后面跟着这么多字节的 literal
fn literal_size(dat: &[u8]) -> Option<&[u8]> {
    let body = dat.strip_suffix(b"}")?;
    let digits = body.iter().rev().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || body.len() == digits || body[body.len() - digits - 1] != b'{' {
        return None;
    }
    Some(&body[body.len() - digits..])
}

/// `\[(?P<type>[A-Z-]+)( (?P<data>.*))?\]`：开头方括号里的回应码 → (类型, 数据)。数据取到最后一个 `]` 为止（贪婪的）
fn response_code(dat: &[u8]) -> Option<(String, Option<Vec<u8>>)> {
    let rest = dat.strip_prefix(b"[")?;
    let n = type_len(rest, true);
    if n == 0 {
        return None;
    }
    let typ = String::from_utf8_lossy(&rest[..n]).into_owned();
    let after = &rest[n..];
    if let Some(body) = after.strip_prefix(b" ") {
        if let Some(end) = body.iter().rposition(|b| *b == b']') {
            return Some((typ, Some(body[..end].to_vec())));
        }
    }
    // 类型后面直接是 ]（类型短一点再配也配不上别的写法）
    after.starts_with(b"]").then_some((typ, None))
}

/// imaplib.IMAP4 的会话
pub struct Imap {
    wire: Option<Box<dyn Wire>>,
    state: State,
    tagpre: Vec<u8>,
    tagnum: u64,
    /// 发出去、还在等回应的命令：tag → 回应（还没来是 None）
    tagged: HashMap<Vec<u8>, Option<(String, Vec<Item>)>>,
    /// 攒着的不带 tag 的回应：类型 → 各项数据
    untagged: HashMap<String, Vec<Item>>,
    is_readonly: bool,
    /// 读进来还没用的字节
    readbuf: Vec<u8>,
    capabilities: Vec<String>,
}

impl Imap {
    /// `IMAP4.__init__`：读欢迎的那一行、问 CAPABILITY。出错时连接已经关了
    pub fn connect(wire: Box<dyn Wire>) -> Ir<Imap> {
        // tag 的前缀：四个 A–P 的字母（Int2AP(random.randint(4096, 65535))）
        let r = urandom(2);
        let num = 4096 + (u32::from(r[0]) << 8 | u32::from(r[1])) % (65535 - 4096 + 1);
        let tagpre: Vec<u8> = (0..4).rev().map(|k| b"ABCDEFGHIJKLMNOP"[((num >> (4 * k)) & 15) as usize]).collect();
        let mut im = Imap {
            wire: Some(wire),
            state: State::Logout,
            tagpre,
            tagnum: 0,
            tagged: HashMap::new(),
            untagged: HashMap::new(),
            is_readonly: false,
            readbuf: Vec::new(),
            capabilities: Vec::new(),
        };
        match im.hello() {
            Ok(()) => Ok(im),
            Err(e) => {
                im.shutdown();
                Err(e)
            }
        }
    }

    /// `_connect`
    fn hello(&mut self) -> Ir<()> {
        let welcome = self.get_response()?;
        if self.untagged.contains_key("PREAUTH") {
            self.state = State::Auth;
        } else if self.untagged.contains_key("OK") {
            self.state = State::NonAuth;
        } else {
            return Err(ImapErr::Error(welcome.map_or_else(|| "None".to_string(), |w| bytes_repr(&w))));
        }
        // _get_capabilities
        let (typ, dat) = self.simple_command("CAPABILITY", &[])?;
        let (_, dat) = self.untagged_response(typ, dat, "CAPABILITY");
        let caps = match dat.last() {
            Some(Item::None) if dat.len() == 1 => return Err(ImapErr::Error("no CAPABILITY response from server".into())),
            Some(Item::Bytes(b)) => ascii_strict(b)?,
            _ => return Err(ImapErr::Py(Fail::Other("TypeError", vec!["decoding to str: need a bytes-like object".into()]))),
        };
        self.capabilities = text::split_ws(&caps.to_uppercase()).into_iter().map(str::to_string).collect();
        if !self.capabilities.iter().any(|c| c == "IMAP4REV1" || c == "IMAP4") {
            return Err(ImapErr::Error("server not IMAP4 compliant".into()));
        }
        Ok(())
    }

    /// 从连接上再读一些进来：Ok(false) 是对方关了（连接被重置这类也算：原来 `except ConnectionError: break`）
    fn fill(&mut self) -> Ir<bool> {
        let Some(wire) = self.wire.as_mut() else { return Ok(false) };
        let mut chunk = [0u8; 8192];
        loop {
            match wire.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.readbuf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) => match e.kind() {
                    io::ErrorKind::Interrupted => {
                        if super::interrupted() {
                            return Err(ImapErr::Py(super::keyboard_interrupt()));
                        }
                    }
                    io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::BrokenPipe => return Ok(false),
                    // TLS 的套接字读超时，Python 是这句话
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => return Err(ImapErr::Os("The read operation timed out".into(), "TimeoutError")),
                    _ => {
                        let (why, class) = io_text(&e);
                        return Err(ImapErr::Os(why, class));
                    }
                },
            }
        }
    }

    /// `readline()`：到换行为止的一行（带着换行）；对方关了时是手上剩下的（可能是空的）
    fn readline(&mut self) -> Ir<Vec<u8>> {
        let mut scanned = 0;
        loop {
            if let Some(pos) = self.readbuf[scanned..].iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.readbuf.drain(..scanned + pos + 1).collect();
                if line.len() > MAXLINE {
                    return Err(ImapErr::Error(format!("got more than {MAXLINE} bytes")));
                }
                return Ok(line);
            }
            scanned = self.readbuf.len();
            if scanned >= MAXLINE {
                if scanned > MAXLINE {
                    return Err(ImapErr::Error(format!("got more than {MAXLINE} bytes")));
                }
                return Ok(std::mem::take(&mut self.readbuf));
            }
            if !self.fill()? {
                return Ok(std::mem::take(&mut self.readbuf));
            }
        }
    }

    /// `read(size)`：读这么多字节的 literal；对方关了时有多少给多少
    fn read(&mut self, size: usize) -> Ir<Vec<u8>> {
        while self.readbuf.len() < size {
            if !self.fill()? {
                break;
            }
        }
        let n = size.min(self.readbuf.len());
        Ok(self.readbuf.drain(..n).collect())
    }

    /// `_get_line()`：一行回应，去掉结尾的 CRLF
    fn get_line(&mut self) -> Ir<Vec<u8>> {
        let mut line = self.readline()?;
        if line.is_empty() {
            return Err(ImapErr::Abort("socket error: EOF".into()));
        }
        if !line.ends_with(b"\r\n") {
            return Err(ImapErr::Abort(format!("socket error: unterminated line: {}", bytes_repr(&line))));
        }
        line.truncate(line.len() - 2);
        Ok(line)
    }

    fn append_untagged(&mut self, typ: &str, dat: Item) {
        self.untagged.entry(typ.to_string()).or_default().push(dat);
    }

    /// `_get_response()`：读一条回应存起来。返回这一行；`+` 开头的（等着发 literal）是 None
    fn get_response(&mut self) -> Ir<Option<Vec<u8>>> {
        let resp = self.get_line()?;
        let (typ, dat): (String, Vec<u8>);
        // 带 tag 的：命令的结果
        let tagged = resp.strip_prefix(self.tagpre.as_slice()).and_then(|rest| {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            let after = rest[digits..].strip_prefix(b" ").filter(|_| digits > 0)?;
            let n = type_len(after, false);
            let data = after[n..].strip_prefix(b" ").filter(|_| n > 0)?;
            Some((resp[..self.tagpre.len() + digits].to_vec(), String::from_utf8_lossy(&after[..n]).into_owned(), data.to_vec()))
        });
        if let Some((tag, t, d)) = tagged {
            if !self.tagged.contains_key(&tag) {
                return Err(ImapErr::Abort(format!("unexpected tagged response: {}", bytes_repr(&resp))));
            }
            self.tagged.insert(tag, Some((t.clone(), vec![Item::Bytes(d.clone())])));
            (typ, dat) = (t, d);
        } else {
            // `* 类型 数据`，或者 `* 数字 类型 数据`（EXISTS、FETCH……）
            let star = resp.strip_prefix(b"* ");
            let plain = star.and_then(|rest| {
                let n = type_len(rest, true);
                (n > 0).then(|| (String::from_utf8_lossy(&rest[..n]).into_owned(), after_space(&rest[n..]), None))
            });
            let status = || {
                let rest = star?;
                let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
                let after = rest[digits..].strip_prefix(b" ").filter(|_| digits > 0)?;
                let n = type_len(after, true);
                (n > 0).then(|| (String::from_utf8_lossy(&after[..n]).into_owned(), Some(rest[..digits].to_vec()), after_space(&after[n..])))
            };
            let Some((t, data, data2)) = plain.or_else(status) else {
                if resp.starts_with(b"+") {
                    return Ok(None); // 等着发 literal 的那种：邮箱助手不发 literal
                }
                return Err(ImapErr::Abort(format!("unexpected response: {}", bytes_repr(&resp))));
            };
            let mut d = data.unwrap_or_default();
            if let Some(d2) = data2.filter(|d2| !d2.is_empty()) {
                d.push(b' ');
                d.extend_from_slice(&d2);
            }
            // 后面跟着 literal 的：照说的字节数读，再读这一条回应剩下的那一行（可能还有 literal）
            while let Some(digits) = literal_size(&d) {
                if digits.len() > INT_MAX_STR_DIGITS {
                    return Err(ImapErr::Py(int_too_long(digits.len())));
                }
                let size = std::str::from_utf8(digits).ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(usize::MAX);
                let data = self.read(size)?;
                self.append_untagged(&t, Item::Pair(d, data));
                d = self.get_line()?;
            }
            self.append_untagged(&t, Item::Bytes(d.clone()));
            (typ, dat) = (t, d);
        }
        // 方括号里的回应码（[UIDVALIDITY 11]、[READ-ONLY]……）也记一份
        if matches!(typ.as_str(), "OK" | "NO" | "BAD") {
            if let Some((code, data)) = response_code(&dat) {
                self.append_untagged(&code, Item::Bytes(data.unwrap_or_default()));
            }
        }
        Ok(Some(resp))
    }

    /// `_check_bye()`：服务器说了 BYE 就是连接要断了
    fn check_bye(&self) -> Ir<()> {
        match self.untagged.get("BYE").and_then(|b| b.last()) {
            None => Ok(()),
            Some(Item::Bytes(b)) => Err(ImapErr::Abort(super::codec::ascii_replace(b))),
            Some(_) => Err(ImapErr::Py(Fail::Other("AttributeError", vec!["'tuple' object has no attribute 'decode'".into()]))),
        }
    }

    /// `_command(name, *args)`：发一条命令，返回它的 tag
    fn command(&mut self, name: &str, args: &[&str]) -> Ir<Vec<u8>> {
        if !allowed(name).contains(&self.state) {
            return Err(ImapErr::Error(format!(
                "command {} illegal in state {}, only allowed in states {}",
                name,
                self.state.name(),
                allowed(name).iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
            )));
        }
        for typ in ["OK", "NO", "BAD"] {
            self.untagged.remove(typ);
        }
        if self.untagged.contains_key("READ-ONLY") && !self.is_readonly {
            return Err(ImapErr::Abort("mailbox status changed to READ-ONLY".into()));
        }
        let mut tag = self.tagpre.clone();
        tag.extend_from_slice(self.tagnum.to_string().as_bytes());
        self.tagnum += 1;
        self.tagged.insert(tag.clone(), None);
        let mut data = tag.clone();
        data.push(b' ');
        data.extend_from_slice(name.as_bytes());
        for arg in args {
            if let Some(i) = arg.chars().position(|c| !c.is_ascii()) {
                let c = arg.chars().nth(i).unwrap_or(' ');
                return Err(ImapErr::Py(Fail::Other(
                    "UnicodeEncodeError",
                    vec![format!("'ascii' codec can't encode character {} in position {i}: ordinal not in range(128)", crate::py::obj::repr_str(&c.to_string()))],
                )));
            }
            data.push(b' ');
            data.extend_from_slice(arg.as_bytes());
        }
        data.extend_from_slice(b"\r\n");
        let Some(wire) = self.wire.as_mut() else {
            return Err(ImapErr::Abort("socket error: [Errno 9] Bad file descriptor".into()));
        };
        if let Err(e) = wire.write_all(&data).and_then(|()| wire.flush()) {
            let why = match e.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "The write operation timed out".to_string(),
                _ => os_error_text(&e),
            };
            return Err(ImapErr::Abort(format!("socket error: {why}")));
        }
        Ok(tag)
    }

    /// `_get_tagged_response(tag, expect_bye)`
    fn get_tagged_response(&mut self, tag: &[u8], expect_bye: bool) -> Ir<(String, Vec<Item>)> {
        loop {
            if let Some(Some(_)) = self.tagged.get(tag) {
                return Ok(self.tagged.remove(tag).flatten().unwrap_or_default());
            }
            if expect_bye {
                // LOGOUT：服务器回 BYE 就算完了
                if let Some(bye) = self.untagged.remove("BYE") {
                    return Ok(("BYE".to_string(), bye));
                }
            }
            self.check_bye()?;
            self.get_response()?;
        }
    }

    /// `_command_complete(name, tag)`
    fn command_complete(&mut self, name: &str, tag: &[u8]) -> Ir<(String, Vec<Item>)> {
        let logout = name == "LOGOUT";
        if !logout {
            self.check_bye()?;
        }
        let (typ, data) = match self.get_tagged_response(tag, logout) {
            Ok(r) => r,
            Err(ImapErr::Abort(val)) => return Err(ImapErr::Abort(format!("command: {name} => {val}"))),
            Err(ImapErr::Error(val)) => return Err(ImapErr::Error(format!("command: {name} => {val}"))),
            Err(other) => return Err(other),
        };
        if !logout {
            self.check_bye()?;
        }
        if typ == "BAD" {
            return Err(ImapErr::Error(format!("{} command error: {} {}", name, typ, items_repr(&data))));
        }
        Ok((typ, data))
    }

    fn simple_command(&mut self, name: &str, args: &[&str]) -> Ir<(String, Vec<Item>)> {
        let tag = self.command(name, args)?;
        self.command_complete(name, &tag)
    }

    /// `_untagged_response(typ, dat, name)`：这条命令攒下的那一类回应（取走）
    fn untagged_response(&mut self, typ: String, dat: Vec<Item>, name: &str) -> (String, Vec<Item>) {
        if typ == "NO" {
            return (typ, dat);
        }
        match self.untagged.remove(name) {
            Some(data) => (typ, data),
            None => (typ, vec![Item::None]),
        }
    }

    /// `login(user, password)`：密码总是加引号（`_quote`）
    pub fn login(&mut self, user: &str, password: &str) -> Ir<()> {
        let quoted = format!("\"{}\"", password.replace('\\', "\\\\").replace('"', "\\\""));
        let (typ, dat) = self.simple_command("LOGIN", &[user, &quoted])?;
        if typ != "OK" {
            return Err(ImapErr::Error(match dat.last() {
                Some(Item::Bytes(b)) => bytes_repr(b),
                Some(Item::Pair(a, b)) => format!("({}, {})", bytes_repr(a), bytes_repr(b)),
                _ => "None".to_string(),
            }));
        }
        self.state = State::Auth;
        Ok(())
    }

    /// `list()`：`LIST "" *`
    pub fn list(&mut self) -> Ir<(String, Vec<Item>)> {
        let (typ, dat) = self.simple_command("LIST", &["\"\"", "*"])?;
        Ok(self.untagged_response(typ, dat, "LIST"))
    }

    /// `select(mailbox, readonly=True)`：EXAMINE（只读地打开）
    pub fn examine(&mut self, mailbox: &str) -> Ir<(String, Vec<Item>)> {
        self.untagged.clear();
        self.is_readonly = true;
        let (typ, dat) = self.simple_command("EXAMINE", &[mailbox])?;
        if typ != "OK" {
            self.state = State::Auth;
            return Ok((typ, dat));
        }
        self.state = State::Selected;
        Ok((typ, self.untagged.get("EXISTS").cloned().unwrap_or_else(|| vec![Item::None])))
    }

    /// `response(code)[1]`：攒着的这一类回应（取走）
    pub fn response(&mut self, code: &str) -> Vec<Item> {
        self.untagged.remove(code).unwrap_or_else(|| vec![Item::None])
    }

    /// `uid(command, *args)`：command 只有 SEARCH、FETCH
    pub fn uid(&mut self, command: &str, args: &[&str]) -> Ir<(String, Vec<Item>)> {
        if !allowed(command).contains(&self.state) {
            return Err(ImapErr::Error(format!(
                "command {} illegal in state {}, only allowed in states {}",
                command,
                self.state.name(),
                allowed(command).iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
            )));
        }
        let mut all = vec![command];
        all.extend_from_slice(args);
        let (typ, dat) = self.simple_command("UID", &all)?;
        Ok(self.untagged_response(typ, dat, command))
    }

    /// `logout()`：发 LOGOUT、等 BYE、关连接（中途出错时连接留着，原来就这样；进程退出时自然关掉）
    pub fn logout(&mut self) -> Ir<()> {
        self.state = State::Logout;
        self.simple_command("LOGOUT", &[])?;
        self.shutdown();
        Ok(())
    }

    /// `shutdown()`：直接关
    pub fn shutdown(&mut self) {
        if let Some(wire) = self.wire.take() {
            wire.close();
        }
    }
}

// --- 时间、搜索条件、FETCH 的回应 --------------------------------------------------------------------------------

/// INTERNALDATE（15-Nov-2026 10:00:00 +0800：Gmail 收到这封邮件的时间，发件人写不了）→ 时间戳；认不出是 None。
/// 年份是 0000 时原来是 calendar.timegm 里没人接的 ValueError
pub fn mail_epoch(s: &str) -> R<Option<i64>> {
    let Some(m) = re!(r"^\s*(\d{1,2})-([A-Za-z]{3})-(\d{4}) (\d\d):(\d\d):(\d\d) ([+-])(\d\d)(\d\d)$").match_(s) else {
        return Ok(None);
    };
    let mon = m.g(2);
    let title: String = mon.chars().enumerate().map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }).collect();
    let Some(month) = MAIL_MONTHS.iter().position(|x| *x == title) else {
        return Ok(None);
    };
    let int = |i: usize| text::to_int(m.g(i)).unwrap_or(0);
    if int(3) == 0 {
        return Err(Fail::Value("year 0 is out of range".into()));
    }
    let t = timegm(int(3), month as i64 + 1, int(1), int(4), int(5), int(6));
    let off = (int(8) * 60 + int(9)) * 60;
    Ok(Some(if m.g(7) == "+" { t - off } else { t + off }))
}

/// IMAP 的 SINCE（只到天、不看时区）：往前多算一天，本地再按 INTERNALDATE 细比
pub fn mail_since(t: f64) -> String {
    let g = gmtime(t - 86400.0);
    format!("{:02}-{}-{}", g.mday, MAIL_MONTHS[(g.mon - 1) as usize], g.year)
}

/// Gmail 上粗筛的搜索条件：SINCE 加上各条规则的条件（好几条时 OR 起来）
pub fn mail_crit(rules: &[&MailRule], since: f64) -> Vec<String> {
    let mut terms: Vec<Vec<String>> = rules.iter().map(|r| r.search.iter().map(|s| s.to_string()).collect()).collect();
    while terms.len() > 1 {
        let (a, b) = (terms.remove(0), terms.remove(0));
        terms.insert(0, vec!["OR".to_string(), format!("({})", a.join(" ")), format!("({})", b.join(" "))]);
    }
    let mut out = vec!["SINCE".to_string(), mail_since(since)];
    out.extend(terms.into_iter().next().unwrap_or_default());
    out
}

fn is_word(c: char) -> bool {
    c == '_' || uni::is_alnum(c)
}

/// `re.search(r'\b' + word + ' ', meta)` 之后的各个位置：word 前面不是字（Python 的 \b）
fn after_word<'a>(cs: &'a [char], word: &'a str) -> impl Iterator<Item = usize> + 'a {
    let w: Vec<char> = word.chars().collect();
    let n = w.len();
    (0..cs.len().saturating_sub(n - 1)).filter(move |&i| cs[i..].starts_with(&w) && (i == 0 || !is_word(cs[i - 1]))).map(move |i| i + n)
}

/// UID 这些数字：位数太多原来是 `int()` 的 ValueError；放不进 64 位的当没有（IMAP 里它们是 32 位的）
fn number(digits: &str) -> R<Option<u64>> {
    let count = text::len(digits);
    if count > INT_MAX_STR_DIGITS {
        return Err(int_too_long(count));
    }
    Ok(text::to_int(digits).and_then(|n| u64::try_from(n).ok()))
}

/// UID FETCH 的回应 → {uid: (INTERNALDATE 的时间戳, literal 的字节)}：各项的先后不定，UID 也可能写在 literal 后面
pub fn mail_fetch_parse(data: &[Item]) -> R<BTreeMap<u64, (i64, Vec<u8>)>> {
    let mut recs: Vec<(String, &Vec<u8>)> = Vec::new();
    for item in data {
        match item {
            Item::Pair(head, lit) => recs.push((latin1(head), lit)),
            Item::Bytes(b) => {
                // 不是下一条的开头（`数字 (`）：接在上一条后面
                let digits = b.iter().take_while(|x| x.is_ascii_digit()).count();
                let next = digits > 0 && b[digits..].starts_with(b" (");
                if let (Some(last), false) = (recs.last_mut(), next) {
                    last.0.push(' ');
                    last.0.push_str(&latin1(b));
                }
            }
            Item::None => {}
        }
    }
    let mut out = BTreeMap::new();
    for (meta, lit) in recs {
        let cs: Vec<char> = meta.chars().collect();
        // \bUID (\d+)
        let uid = after_word(&cs, "UID ").find_map(|i| {
            let n = cs[i..].iter().take_while(|c| uni::is_decimal(**c)).count();
            (n > 0).then(|| cs[i..i + n].iter().collect::<String>())
        });
        // \bINTERNALDATE "([^"]+)"
        let date = after_word(&cs, "INTERNALDATE \"").find_map(|i| {
            let n = cs[i..].iter().take_while(|c| **c != '"').count();
            (n > 0 && i + n < cs.len()).then(|| cs[i..i + n].iter().collect::<String>())
        });
        let t = match date {
            Some(d) => mail_epoch(&d)?,
            None => None,
        };
        if let (Some(u), Some(t)) = (uid, t) {
            if let Some(u) = number(&u)? {
                out.insert(u, (t, lit.clone()));
            }
        }
    }
    Ok(out)
}

/// LIST 的一行 → (属性, 邮件夹的名字)：`MAIL_LIST_RE`
fn list_row(item: &str) -> Option<(String, String)> {
    let m = re!(r#"^\(([^)]*)\) (?:\x22(?:[^\x22\\]|\\.)*\x22|NIL) (\x22(?:[^\x22\\]|\\.)*\x22|[^\s\x22()]+)$"#).match_(item)?;
    Some((m.g(1).to_string(), m.g(2).to_string()))
}

// --- MailBox -----------------------------------------------------------------------------------------------------

/// `headers` 要哪些邮件：UID 的列表，或者 (起, 止) 的区间（含两头）
pub enum Uids {
    List(Vec<u64>),
    Range(u64, u64),
    /// 一个 UID 的十进制写法（cca mail open --msg 里手写的：放不进 64 位的也照样发给 Gmail，原来就这样）
    Text(String),
}

/// 怎么连上去：测试里换成本机的假服务器
pub type Connector = Box<dyn Fn() -> R<Box<dyn Wire>>>;

/// 登录好的 Gmail 会话，只读：只用 LOGIN、CAPABILITY、LIST、EXAMINE、UID SEARCH、UID FETCH 的 BODY.PEEK（不标已读）、LOGOUT。
/// 不 SELECT、不 STORE、不删不移，也没有通用的命令入口。出错时整个会话丢掉（超时以后读不回来），下次重连
pub struct MailBox {
    user: String,
    pw: String,
    connector: Connector,
    im: Option<Imap>,
}

impl MailBox {
    pub fn new(user: &str, pw: &str, route: &Route) -> MailBox {
        let route = route.clone();
        MailBox::with_connector(user, pw, Box::new(move || Ok(Box::new(mail_tls(mail_dial(&route, 15.0)?, 20.0)?) as Box<dyn Wire>)))
    }

    pub fn with_connector(user: &str, pw: &str, connector: Connector) -> MailBox {
        MailBox { user: user.to_string(), pw: pw.to_string(), connector, im: None }
    }

    pub fn is_open(&self) -> bool {
        self.im.is_some()
    }

    pub fn open(&mut self) -> R<()> {
        check_interrupt()?;
        let wire = (self.connector)()?;
        check_interrupt()?;
        let mut im = match Imap::connect(wire) {
            Ok(im) => im,
            Err(ImapErr::Py(e)) => return Err(e),
            Err(e) => {
                check_interrupt()?;
                return Err(mail_retry("net", format!("Gmail 没有正常应答（{}）", e.oserr())));
            }
        };
        let logged = im.login(&self.user, &self.pw);
        self.im = Some(im);
        match logged {
            Ok(()) => Ok(()),
            Err(ImapErr::Py(e)) => Err(e),
            Err(e @ (ImapErr::Abort(_) | ImapErr::Os(..))) => {
                self.close(false);
                check_interrupt()?;
                Err(mail_retry("net", format!("和 Gmail 的连接断了（{}）", e.oserr())))
            }
            Err(e @ ImapErr::Error(_)) => {
                // NO：按 Gmail 的原话分，认不出的当作过一会儿再试（临时出错、同时连得太多……）
                self.close(false);
                let why = e.oserr();
                if re!(r"not enabled for IMAP|IMAP (access )?is disabled", crate::py::re::I).found(&why) {
                    return Err(mail_fatal("imap", "这个 Gmail 没开 IMAP：Gmail 的设置 → 转发和 POP/IMAP → 启用 IMAP（公司的邮箱要管理员允许）"));
                }
                if re!(r"AUTHENTICATIONFAILED|Invalid credentials|Application-specific password required", crate::py::re::I).found(&why) {
                    return Err(mail_fatal(
                        "auth",
                        "Gmail 不认这个密码：要用应用专用密码（16 个字母），不是 Gmail 的登录密码；没开两步验证的账号建不了应用专用密码",
                    ));
                }
                Err(mail_retry("login", format!("Gmail 这次没让登录（{why}）")))
            }
        }
    }

    /// `self.cmd(fn, *args)`：跑一条命令，出错、回的不是 OK 都把会话丢掉
    fn cmd(&mut self, run: impl FnOnce(&mut Imap) -> Ir<(String, Vec<Item>)>) -> R<Vec<Item>> {
        let Some(im) = self.im.as_mut() else {
            return Err(mail_retry("net", "和 Gmail 的连接断了"));
        };
        check_interrupt()?;
        let (typ, data) = match run(im) {
            Ok(r) => r,
            Err(ImapErr::Py(e)) => return Err(e),
            Err(e @ (ImapErr::Abort(_) | ImapErr::Os(..))) => {
                self.close(false);
                check_interrupt()?;
                return Err(mail_retry("net", format!("和 Gmail 的连接断了（{}）", e.oserr())));
            }
            Err(e @ ImapErr::Error(_)) => {
                // BAD：Gmail 不认这个请求
                self.close(false);
                return Err(mail_retry("proto", format!("Gmail 不认 cca 的请求（{}）", e.oserr())));
            }
        };
        if typ != "OK" {
            self.close(false);
            // mail_oserr(data[-1] if data else typ)
            let why = match data.last() {
                None => mail_oserr(&typ, "str"),
                Some(Item::None) => mail_oserr("NoneType", "NoneType"),
                Some(Item::Bytes(b)) if b.is_empty() => mail_oserr("bytes", "bytes"),
                Some(Item::Bytes(b)) => mail_oserr(&bytes_repr(b), "bytes"),
                Some(Item::Pair(a, b)) => mail_oserr(&format!("({}, {})", bytes_repr(a), bytes_repr(b)), "tuple"),
            };
            return Err(mail_retry("proto", format!("Gmail 不认 cca 的请求（{why}）")));
        }
        Ok(data)
    }

    /// 原来各个方法先取 `self.im.list` 这样的方法再交给 cmd：会话已经关掉（`self.im` 是 None）时，在进 cmd 之前就是
    /// 没人接的 AttributeError，cmd 里「连接断了」的那一句其实走不到。各条命令的流程里出了错都不再往下调，实际碰不到
    fn bound(&self, method: &str) -> R<()> {
        if self.im.is_none() {
            return Err(Fail::Other("AttributeError", vec![format!("'NoneType' object has no attribute '{method}'")]));
        }
        Ok(())
    }

    /// 要看的邮件夹 [(给人看的名字, IMAP 里的名字)]：所有邮件和垃圾邮件，按 Gmail 的特殊用途标记（\All、\Junk）认，不认名字
    /// （名字会本地化）；没有 \All（Gmail 的设置里没让它在 IMAP 里显示）时看收件箱
    pub fn folders(&mut self) -> R<Vec<(String, String)>> {
        self.bound("list")?;
        let mut rows: Vec<(Vec<String>, String)> = Vec::new();
        for item in self.cmd(|im| im.list())? {
            if let Item::Bytes(b) = item {
                if let Some((attrs, name)) = list_row(&latin1(&b)) {
                    rows.push((text::split_ws(&attrs.to_lowercase()).into_iter().map(str::to_string).collect(), name));
                }
            }
        }
        let with = |flag: &str| rows.iter().find(|(a, _)| a.iter().any(|x| x == flag)).map(|(_, n)| n.clone());
        let mut out = vec![match with("\\all") {
            Some(all) => ("所有邮件".to_string(), all),
            None => ("收件箱".to_string(), "INBOX".to_string()),
        }];
        if let Some(junk) = with("\\junk") {
            out.push(("垃圾邮件".to_string(), junk));
        }
        Ok(out)
    }

    /// 只读地打开一个邮件夹（EXAMINE），返回 (UIDVALIDITY, UIDNEXT)：没给的是 None
    pub fn examine(&mut self, folder: &str) -> R<(Option<u64>, Option<u64>)> {
        self.bound("select")?;
        self.cmd(|im| im.examine(folder))?;
        let Some(im) = self.im.as_mut() else { return Ok((None, None)) };
        let mut vals = [None, None];
        for (slot, code) in vals.iter_mut().zip(["UIDVALIDITY", "UIDNEXT"]) {
            if let Some(Item::Bytes(v)) = im.response(code).last() {
                if !v.is_empty() && v.iter().all(u8::is_ascii_digit) {
                    *slot = number(&latin1(v))?;
                }
            }
        }
        Ok((vals[0], vals[1]))
    }

    pub fn search(&mut self, crit: &[String]) -> R<Vec<u64>> {
        self.bound("uid")?;
        let mut args: Vec<&str> = Vec::with_capacity(crit.len());
        args.extend(crit.iter().map(String::as_str));
        let data = self.cmd(|im| im.uid("SEARCH", &args))?;
        let mut out = Vec::new();
        for item in data {
            if let Item::Bytes(b) = item {
                // bytes.split()：按 ASCII 的空白切
                for x in b.split(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)) {
                    if !x.is_empty() && x.iter().all(u8::is_ascii_digit) {
                        if let Some(n) = number(&latin1(x))? {
                            out.push(n);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// {uid: (Gmail 收到的时间, 头的字节)}
    pub fn headers(&mut self, uids: &Uids) -> R<BTreeMap<u64, (i64, Vec<u8>)>> {
        let spec = match uids {
            Uids::Range(a, b) => format!("{a}:{b}"),
            Uids::List(list) if !list.is_empty() => list.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
            Uids::List(_) => return Ok(BTreeMap::new()),
            Uids::Text(uid) => uid.clone(),
        };
        self.bound("uid")?;
        let what = format!("(UID INTERNALDATE BODY.PEEK[HEADER.FIELDS ({})])", MAIL_HDRS.join(" "));
        mail_fetch_parse(&self.cmd(|im| im.uid("FETCH", &[&spec, &what]))?)
    }

    pub fn message(&mut self, uid: u64) -> R<Vec<u8>> {
        self.bound("uid")?;
        let what = format!("(UID INTERNALDATE BODY.PEEK[]<0.{MAIL_FETCH_MAX}>)");
        let mut got = mail_fetch_parse(&self.cmd(|im| im.uid("FETCH", &[&uid.to_string(), &what]))?)?;
        match got.remove(&uid) {
            Some((_, raw)) => Ok(raw),
            None => Err(mail_retry("proto", "Gmail 没给出这封邮件")),
        }
    }

    /// polite：先 LOGOUT 再关；不然直接关
    pub fn close(&mut self, polite: bool) {
        let Some(mut im) = self.im.take() else { return };
        if polite {
            let _ = im.logout();
        } else {
            im.shutdown();
        }
    }
}

// --- MailSource --------------------------------------------------------------------------------------------------

/// 一封候选的邮件（只有头）
#[derive(Clone, Debug, PartialEq)]
pub struct Cand {
    /// IMAP 里的名字
    pub folder: String,
    /// 给人看的名字
    pub label: String,
    pub uid: u64,
    /// Gmail 收到它的时间
    pub got: f64,
    pub hdr: Vec<u8>,
    pub valid: Option<u64>,
}

/// 盯邮箱时的新邮件来源（mail_hunt 只用 poll 和 full）
pub trait Source {
    fn poll(&mut self, since: f64) -> R<Vec<Cand>>;
    fn full(&mut self, cand: &Cand) -> R<Vec<u8>>;
}

/// `MailSource` 用到的那几样（测试里换成假的 MailBox）
pub trait Session {
    fn is_open(&self) -> bool;
    fn open(&mut self) -> R<()>;
    fn folders(&mut self) -> R<Vec<(String, String)>>;
    fn examine(&mut self, folder: &str) -> R<(Option<u64>, Option<u64>)>;
    fn search(&mut self, crit: &[String]) -> R<Vec<u64>>;
    fn headers(&mut self, uids: &Uids) -> R<BTreeMap<u64, (i64, Vec<u8>)>>;
    fn message(&mut self, uid: u64) -> R<Vec<u8>>;
    fn close(&mut self, polite: bool);
}

impl Session for MailBox {
    fn is_open(&self) -> bool {
        MailBox::is_open(self)
    }
    fn open(&mut self) -> R<()> {
        MailBox::open(self)
    }
    fn folders(&mut self) -> R<Vec<(String, String)>> {
        MailBox::folders(self)
    }
    fn examine(&mut self, folder: &str) -> R<(Option<u64>, Option<u64>)> {
        MailBox::examine(self, folder)
    }
    fn search(&mut self, crit: &[String]) -> R<Vec<u64>> {
        MailBox::search(self, crit)
    }
    fn headers(&mut self, uids: &Uids) -> R<BTreeMap<u64, (i64, Vec<u8>)>> {
        MailBox::headers(self, uids)
    }
    fn message(&mut self, uid: u64) -> R<Vec<u8>> {
        MailBox::message(self, uid)
    }
    fn close(&mut self, polite: bool) {
        MailBox::close(self, polite)
    }
}

/// 盯邮箱时的新邮件来源：每一轮各邮件夹 EXAMINE 一次。断了下一轮重连，看过的不再给
pub struct MailSource<B: Session> {
    pub bx: B,
    rules: Vec<&'static MailRule>,
    /// [(给人看的名字, IMAP 里的名字)]
    places: Option<Vec<(String, String)>>,
    /// IMAP 里的名字 → (UIDVALIDITY, 下一轮从这个 UID 看起)
    next: HashMap<String, (u64, u64)>,
    /// (IMAP 里的名字, UIDVALIDITY, UID)
    seen: HashSet<(String, Option<u64>, u64)>,
}

impl<B: Session> MailSource<B> {
    pub fn new(bx: B, rules: &[&'static MailRule]) -> MailSource<B> {
        MailSource { bx, rules: rules.to_vec(), places: None, next: HashMap::new(), seen: HashSet::new() }
    }

    pub fn close(&mut self) {
        self.bx.close(true);
    }
}

impl<B: Session> Source for MailSource<B> {
    /// 这一轮的新邮件（只有头）。第一次（UIDVALIDITY 变了也是）按日期和规则搜索，再加上最新的 MAIL_BURST 封；之后只按 UID 取
    /// UIDNEXT 往后的、在本地筛，不经 Gmail 的搜索（刚到的邮件它可能还没编进索引）。
    /// 整轮都成了才记下看过了（中途断了的下一轮从头再看，免得漏掉）
    fn poll(&mut self, since: f64) -> R<Vec<Cand>> {
        if !self.bx.is_open() {
            self.bx.open()?;
            if self.places.as_ref().is_none_or(|w| w.is_empty()) {
                self.places = Some(self.bx.folders()?);
            }
        }
        let mut out: Vec<Cand> = Vec::new();
        let mut nxt: Vec<(String, (u64, u64))> = Vec::new();
        for (label, name) in self.places.clone().unwrap_or_default() {
            let (valid, unext) = self.bx.examine(&name)?;
            let last = self.next.get(&name).copied();
            let mut got: BTreeMap<u64, (i64, Vec<u8>)> = BTreeMap::new();
            match (last, valid, unext) {
                (Some((lv, ln)), Some(v), Some(un)) if lv == v => {
                    if un > ln {
                        got = self.bx.headers(&Uids::Range(ln.max(un.saturating_sub(MAIL_BURST)), un - 1))?;
                    }
                }
                _ => {
                    let hits = self.bx.search(&super::imap::mail_crit(&self.rules, since))?;
                    got = self.bx.headers(&Uids::List(hits))?;
                    if let Some(un) = unext.filter(|un| *un > 1) {
                        got.extend(self.bx.headers(&Uids::Range(un.saturating_sub(MAIL_BURST).max(1), un - 1))?);
                    }
                }
            }
            for (u, (t, hdr)) in got {
                if !self.seen.contains(&(name.clone(), valid, u)) {
                    out.push(Cand { folder: name.clone(), label: label.clone(), uid: u, got: t as f64, hdr, valid });
                }
            }
            if let (Some(v), Some(un)) = (valid, unext) {
                let from = match last {
                    Some((lv, ln)) if lv == v => un.max(ln),
                    _ => un,
                };
                nxt.push((name.clone(), (v, from)));
            }
        }
        self.next.extend(nxt);
        self.seen.extend(out.iter().map(|c| (c.folder.clone(), c.valid, c.uid)));
        Ok(out)
    }

    fn full(&mut self, cand: &Cand) -> R<Vec<u8>> {
        if !self.bx.is_open() {
            self.bx.open()?;
        }
        self.bx.examine(&cand.folder)?;
        self.bx.message(cand.uid)
    }
}

#[cfg(test)]
mod tests;
