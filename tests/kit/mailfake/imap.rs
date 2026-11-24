//! 假 Gmail 的 IMAP 会话（原来的 Conn.handle）：一行命令进来，回应写出去，日志记一行。只认 cca 用的只读命令；会改邮箱的命令、
//! 不带 PEEK 的 BODY[]（会标已读）一律回 BAD、日志里记 VIOLATION。每个命令都重新读 ctl.json 和邮箱目录。

use super::hdr::{hdr_text, header_fields};
use super::py::{self, truthy};
use super::{Ctl, Msg, Shared, FOLDERS};
use crate::json::Value;
use std::collections::HashSet;
use std::io::{self, Write};

const WRITES: &[&str] = &[
    "SELECT",
    "STORE",
    "COPY",
    "MOVE",
    "EXPUNGE",
    "APPEND",
    "CREATE",
    "DELETE",
    "RENAME",
    "SUBSCRIBE",
    "UNSUBSCRIBE",
    "CLOSE",
    "IDLE",
    "AUTHENTICATE",
    "STARTTLS",
];

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 原来在这一步抛出的异常（ValueError、IndexError、TypeError、OSError……）：不回话，连接断开
#[derive(Debug)]
pub struct Abort;

impl From<io::Error> for Abort {
    fn from(_: io::Error) -> Abort {
        Abort
    }
}

/// INTERNALDATE 的写法：15-Nov-2025 10:00:00 +0000
pub fn idate(t: f64) -> String {
    let g = py::gmtime(t);
    format!("{:02}-{}-{} {:02}:{:02}:{:02} +0000", g.mday, MONTHS[(g.mon - 1) as usize], g.year, g.hour, g.min, g.sec)
}

/// 参数里的一个记号
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    Open,
    Close,
    Atom(String),
    /// 带引号的字符串（引号、反斜杠去掉了）
    Quoted(String),
}

impl Tok {
    /// 当参数用时的文字（原来 tuple 取 [1]，括号就是 '(' ')'）
    fn text(&self) -> &str {
        match self {
            Tok::Open => "(",
            Tok::Close => ")",
            Tok::Atom(s) | Tok::Quoted(s) => s,
        }
    }
    /// repr()：('q', '…') 或者 '…'
    fn repr(&self) -> String {
        match self {
            Tok::Quoted(s) => format!("('q', {})", py::repr_str(s)),
            other => py::repr_str(other.text()),
        }
    }
}

/// IMAP 参数 → 记号：原子、带引号的字符串（去掉引号）、括号
pub fn tokens(s: &str) -> Vec<Tok> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if py::isspace(c) {
            i += 1;
        } else if c == '(' || c == ')' {
            out.push(if c == '(' { Tok::Open } else { Tok::Close });
            i += 1;
        } else if c == '"' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < cs.len() && cs[j] != '"' {
                if cs[j] == '\\' && j + 1 < cs.len() {
                    j += 1;
                }
                buf.push(cs[j]);
                j += 1;
            }
            out.push(Tok::Quoted(buf));
            i = j + 1;
        } else {
            let mut j = i;
            while j < cs.len() && !py::isspace(cs[j]) && cs[j] != '(' && cs[j] != ')' {
                j += 1;
            }
            out.push(Tok::Atom(cs[i..j].iter().collect()));
            i = j;
        }
    }
    out
}

/// SEARCH 条件对这封邮件成不成立：只认 cca 用的 ALL、SINCE、FROM、SUBJECT、OR、括号；别的是 Err（回 BAD，带着原来 ValueError 的说法）
pub fn search_eval(toks: &[Tok], msg: &Msg) -> Result<bool, String> {
    struct P<'a> {
        toks: &'a [Tok],
        pos: usize,
        msg: &'a Msg,
    }
    impl P<'_> {
        fn take(&mut self) -> Result<&Tok, String> {
            if self.pos >= self.toks.len() {
                return Err("truncated".to_string());
            }
            self.pos += 1;
            Ok(&self.toks[self.pos - 1])
        }
        fn one(&mut self) -> Result<bool, String> {
            let k = self.take()?.clone();
            if k == Tok::Open {
                let mut ok = true;
                loop {
                    if self.pos >= self.toks.len() {
                        return Err("unclosed".to_string());
                    }
                    if self.toks[self.pos] == Tok::Close {
                        self.pos += 1;
                        return Ok(ok);
                    }
                    let r = self.one()?; // 先解析完再算
                    ok = r && ok;
                }
            }
            let Tok::Atom(k) = k else {
                return Err(format!("unexpected {}", k.repr()));
            };
            let u = py::upper(&k);
            if u == "ALL" {
                return Ok(true);
            }
            if u == "OR" {
                let a = self.one()?;
                let b = self.one()?;
                return Ok(a || b);
            }
            let arg = self.take()?.text().to_string();
            match u.as_str() {
                "SINCE" => {
                    let Some(m) = re!(r"^([0-9]{1,2})-([A-Za-z]{3})-([0-9]{4})$").captures(&arg) else {
                        return Err("bad date".to_string());
                    };
                    let mon = &m[2];
                    let title = format!("{}{}", mon[..1].to_ascii_uppercase(), mon[1..].to_ascii_lowercase());
                    let Some(mi) = MONTHS.iter().position(|x| *x == title) else {
                        return Err("tuple.index(x): x not in tuple".to_string());
                    };
                    let num = |s: &str| py::int(s).unwrap_or(0);
                    let day = (num(&m[3]), mi as i128 + 1, num(&m[1]));
                    let g = py::gmtime(self.msg.t);
                    Ok((i128::from(g.year), i128::from(g.mon), i128::from(g.mday)) >= day)
                }
                "FROM" => Ok(hdr_text(&self.msg.raw, "From").to_lowercase().contains(&arg.to_lowercase())),
                "SUBJECT" => Ok(hdr_text(&self.msg.raw, "Subject").to_lowercase().contains(&arg.to_lowercase())),
                _ => Err(format!("unsupported search key {k}")),
            }
        }
    }
    let mut p = P { toks, pos: 0, msg };
    let mut ok = true;
    while p.pos < toks.len() {
        let r = p.one()?;
        ok = r && ok;
    }
    Ok(ok)
}

/// UID 集合（3、1:5、2:*、3,7……）→ 要的 UID；数字写错了是原来的 ValueError（None）
pub fn uid_set(spec: &str, uids: &[i128]) -> Option<HashSet<i128>> {
    let top = uids.iter().copied().max().unwrap_or(0);
    let mut want = HashSet::new();
    for part in spec.split(',') {
        if let Some((a, b)) = part.split_once(':') {
            let a = if a == "*" { top } else { py::int(a).ok()? };
            let b = if b == "*" { top } else { py::int(b).ok()? };
            let (lo, hi) = (a.min(b), a.max(b));
            want.extend(uids.iter().copied().filter(|u| lo <= *u && *u <= hi));
        } else {
            want.insert(if part == "*" { top } else { py::int(part).ok()? });
        }
    }
    Some(want)
}

/// re.search(r'\bRFC822(?![.](HEADER|SIZE))\b', up)：Rust 的正则没有往后看，后面那一截手动查
fn rfc822_body(up: &str) -> bool {
    re!(r"\bRFC822").find_iter(up).any(|m| {
        let after = &up[m.end()..];
        match after.chars().next() {
            None => true,
            Some(c) if c.is_alphanumeric() || c == '_' => false,
            Some(_) => !(after.starts_with(".HEADER") || after.starts_with(".SIZE")),
        }
    })
}

/// 一个连接上的会话：选了哪个邮件夹、登录了没有
pub struct Session {
    sh: std::sync::Arc<Shared>,
    folder: Option<&'static str>,
    authed: bool,
}

fn send(out: &mut dyn Write, data: &[u8]) -> Result<(), Abort> {
    out.write_all(data)?;
    out.flush()?;
    Ok(())
}

impl Session {
    pub(super) fn new(sh: std::sync::Arc<Shared>) -> Session {
        Session { sh, folder: None, authed: false }
    }

    /// 欢迎的那一行
    pub fn greet(&mut self, out: &mut dyn Write) -> Result<(), Abort> {
        send(out, b"* OK Gimap ready for requests from 127.0.0.1 x1mb12345\r\n")
    }

    /// 一行命令（已经去掉了结尾的 \r\n）：回应写进 out。Ok(false) 是该断开了（LOGOUT、drop），Err 是原来抛出的异常（也断开）
    pub fn handle(&mut self, line: &str, out: &mut dyn Write) -> Result<bool, Abort> {
        let c = self.sh.mb.ctl()?;
        let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
        let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let mut verb = py::upper(verb);
        let mut args = args.to_string();
        if verb == "UID" {
            let (sub, a) = args.split_once(' ').unwrap_or((args.as_str(), ""));
            let (sub, a) = (py::upper(sub), a.to_string());
            verb = format!("UID {sub}");
            args = a;
        }
        let shown = if verb == "LOGIN" { format!("{} ***", args.split(' ').next().unwrap_or("")) } else { args.clone() };
        self.sh.log(&format!("IMAP {verb} {shown}"))?;
        if let Some(d) = c.get("drop").filter(|d| truthy(d)) {
            let Value::Str(d) = d else { return Err(Abort) }; // 不是字符串：in 是 TypeError
            if line.contains(d.as_str()) {
                self.sh.log(&format!("DROP {verb}"))?;
                return Ok(false);
            }
        }
        let last = verb.rsplit(' ').next().unwrap_or("");
        if WRITES.contains(&verb.as_str()) || WRITES.contains(&last) {
            self.sh.log(&format!("VIOLATION {verb} {args}"))?;
            send(out, format!("{tag} BAD Command not allowed in the test\r\n").as_bytes())?;
            return Ok(true);
        }
        let on = |k: &str| c.get(k).is_some_and(truthy);
        match verb.as_str() {
            "CAPABILITY" => send(
                out,
                format!(
                    "* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 XYZZY SASL-IR \
                     AUTH=XOAUTH2 AUTH=PLAIN AUTH=PLAIN-CLIENTTOKEN AUTH=OAUTHBEARER\r\n{tag} OK Thats all she wrote!\r\n"
                )
                .as_bytes(),
            )?,
            "LOGIN" => {
                let parts = tokens(&args);
                let user = parts.first().ok_or(Abort)?.text().to_string(); // 没有参数：IndexError
                let pw = parts.get(1).map_or("", Tok::text).to_string();
                let reply = if on("imap_disabled") {
                    format!(
                        "{tag} NO [ALERT] Your account is not enabled for IMAP use. Please visit your Gmail settings page \
                         and enable your account for IMAP access. (Failure)\r\n"
                    )
                } else if on("login_no") {
                    format!("{tag} NO {}\r\n", py::py_str(c.get("login_no").unwrap_or(&Value::Null)))
                } else if c.get("user") == Some(&Value::Str(user.clone())) && c.get("password") == Some(&Value::Str(pw)) {
                    self.authed = true;
                    format!(
                        "* CAPABILITY IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS \
                         COMPRESS=DEFLATE ENABLE MOVE CONDSTORE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL- \
                         SPECIAL-USE APPENDLIMIT=35651584\r\n{tag} OK {user} authenticated (Success)\r\n"
                    )
                } else {
                    format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n")
                };
                send(out, reply.as_bytes())?;
            }
            v if !self.authed && v != "LOGOUT" && v != "NOOP" => {
                send(out, format!("{tag} BAD Command not valid in this state\r\n").as_bytes())?;
            }
            "LIST" => {
                for (key, name, attrs) in FOLDERS {
                    if key == "all" && on("no_all") {
                        continue;
                    }
                    send(out, format!("* LIST ({attrs}) \"/\" \"{name}\"\r\n").as_bytes())?;
                }
                send(
                    out,
                    format!(
                        "* LIST (\\HasChildren \\Noselect) \"/\" \"[Gmail]\"\r\n* LIST (\\HasNoChildren \\Sent) \"/\" \"[Gmail]/Sent Mail\"\r\n\
                         {tag} OK Success\r\n"
                    )
                    .as_bytes(),
                )?;
            }
            "EXAMINE" => {
                let key = super::folder_dir(py::strip(&args)).filter(|k| !(*k == "all" && on("no_all")));
                let Some(key) = key else {
                    send(out, format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {args} (Failure)\r\n").as_bytes())?;
                    return Ok(true);
                };
                self.folder = Some(key);
                let ms = self.sh.mb.msgs(key).map_err(|_| Abort)?;
                let nxt = ms.iter().map(|m| m.uid).max().unwrap_or(0).max(0) + 1;
                send(
                    out,
                    format!(
                        "* FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen $NotPhishing $Phishing)\r\n\
                         * OK [PERMANENTFLAGS ()] Flags permitted.\r\n* OK [UIDVALIDITY {}] UIDs valid.\r\n* {} EXISTS\r\n\
                         * 0 RECENT\r\n* OK [UIDNEXT {nxt}] Predicted next UID.\r\n* OK [HIGHESTMODSEQ 1234]\r\n\
                         {tag} OK [READ-ONLY] {args} selected. (Success)\r\n",
                        self.sh.mb.uidvalidity(key),
                        ms.len()
                    )
                    .as_bytes(),
                )?;
            }
            "UID SEARCH" => {
                let Some(folder) = self.folder else {
                    send(out, format!("{tag} BAD No mailbox selected\r\n").as_bytes())?;
                    return Ok(true);
                };
                let toks = tokens(&args);
                let lag = index_lag(&c)?;
                let hits = (|| -> Result<Vec<i128>, String> {
                    let mut hits = Vec::new();
                    for m in self.sh.mb.msgs(folder)? {
                        if py::now() - m.t >= lag && search_eval(&toks, &m)? {
                            hits.push(m.uid);
                        }
                    }
                    Ok(hits)
                })();
                match hits {
                    Ok(hits) => {
                        let list: String = hits.iter().map(|u| format!(" {u}")).collect();
                        send(out, format!("* SEARCH{list}\r\n{tag} OK SEARCH completed (Success)\r\n").as_bytes())?;
                    }
                    Err(e) => send(out, format!("{tag} BAD Could not parse command ({e})\r\n").as_bytes())?,
                }
            }
            "UID FETCH" => {
                let Some(folder) = self.folder else {
                    send(out, format!("{tag} BAD No mailbox selected\r\n").as_bytes())?;
                    return Ok(true);
                };
                let (spec, items) = args.split_once(' ').unwrap_or((args.as_str(), ""));
                let up = py::upper(items);
                if re!(r"\bBODY\[").is_match(&up) || rfc822_body(&up) {
                    self.sh.log(&format!("VIOLATION UID FETCH {items}"))?; // 不带 PEEK：会标已读
                    send(out, format!("{tag} BAD Would mark as read\r\n").as_bytes())?;
                    return Ok(true);
                }
                let ms = self.sh.mb.msgs(folder).map_err(|_| Abort)?;
                let uids: Vec<i128> = ms.iter().map(|m| m.uid).collect();
                let want = uid_set(spec, &uids).ok_or(Abort)?;
                let hf = re!(r"(?i)BODY\.PEEK\[HEADER\.FIELDS \(([^)]*)\)\]").captures(items);
                let part = re!(r"(?i)BODY\.PEEK\[\]<0\.([0-9]+)>").captures(items);
                let after = on("uid_after_literal");
                for (seq, m) in ms.iter().enumerate() {
                    let seq = seq + 1;
                    if !want.contains(&m.uid) {
                        continue;
                    }
                    let (data, label): (Vec<u8>, Option<String>) = if let Some(hf) = &hf {
                        let names: HashSet<String> = py::split_ws(&hf[1]).into_iter().map(py::upper).collect();
                        (header_fields(&m.raw, &names), Some(format!("BODY[HEADER.FIELDS ({})]", &hf[1])))
                    } else if let Some(part) = &part {
                        let n = py::int(&part[1]).map_err(|_| Abort)?.clamp(0, m.raw.len() as i128) as usize;
                        (m.raw[..n].to_vec(), Some("BODY[]<0>".to_string()))
                    } else {
                        (Vec::new(), None)
                    };
                    let (u, d) = (m.uid, idate(m.t));
                    let mut buf: Vec<u8>;
                    match label {
                        None => buf = format!("* {seq} FETCH (UID {u} INTERNALDATE \"{d}\")\r\n").into_bytes(),
                        Some(label) if after => {
                            buf = format!("* {seq} FETCH (INTERNALDATE \"{d}\" {label} {{{}}}\r\n", data.len()).into_bytes();
                            buf.extend_from_slice(&data);
                            buf.extend_from_slice(format!(" UID {u})\r\n").as_bytes());
                        }
                        Some(label) => {
                            buf =
                                format!("* {seq} FETCH (UID {u} INTERNALDATE \"{d}\" {label} {{{}}}\r\n", data.len()).into_bytes();
                            buf.extend_from_slice(&data);
                            buf.extend_from_slice(b")\r\n");
                        }
                    }
                    send(out, &buf)?;
                }
                send(out, format!("{tag} OK Success\r\n").as_bytes())?;
            }
            "NOOP" => send(out, format!("{tag} OK Success\r\n").as_bytes())?,
            "LOGOUT" => {
                send(out, format!("* BYE LOGOUT Requested\r\n{tag} OK 73 good day (Success)\r\n").as_bytes())?;
                return Ok(false);
            }
            _ => {
                self.sh.log(&format!("VIOLATION {verb} {args}"))?;
                send(out, format!("{tag} BAD Unknown command\r\n").as_bytes())?;
            }
        }
        Ok(true)
    }
}

/// float(c.get('index_lag') or 0)：写错了是原来的 ValueError / TypeError
fn index_lag(c: &Ctl) -> Result<f64, Abort> {
    match c.get("index_lag") {
        None => Ok(0.0),
        Some(v) if !truthy(v) => Ok(0.0),
        Some(Value::Bool(true)) => Ok(1.0),
        Some(Value::Int(n)) => Ok(*n as f64),
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Str(s)) => py::float(s).ok_or(Abort),
        Some(_) => Err(Abort),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(raw: &[u8], t: f64) -> Msg {
        Msg { uid: 1, t, raw: raw.to_vec() }
    }

    #[test]
    fn tokens_like_python() {
        let t = tokens(r#"OR (FROM "a\"b") SUBJECT "x y" "tail\"#);
        assert_eq!(
            t,
            vec![
                Tok::Atom("OR".into()),
                Tok::Open,
                Tok::Atom("FROM".into()),
                Tok::Quoted("a\"b".into()),
                Tok::Close,
                Tok::Atom("SUBJECT".into()),
                Tok::Quoted("x y".into()),
                Tok::Quoted("tail\\".into()),
            ]
        );
    }

    #[test]
    fn search() {
        let m = msg(b"From: Anthropic <no-reply@mail.anthropic.com>\r\nSubject: Your secure link\r\n\r\n", 1763200800.0);
        let ev = |s: &str| search_eval(&tokens(s), &m);
        assert_eq!(ev(r#"SINCE 14-Nov-2025 OR FROM "anthropic.com" SUBJECT "secure link""#), Ok(true));
        assert_eq!(ev("SINCE 16-nov-2025"), Ok(false));
        assert_eq!(ev(r#"(FROM "x") ALL"#), Ok(false));
        assert_eq!(ev("SINCE 1-Foo-2025"), Err("tuple.index(x): x not in tuple".into()));
        assert_eq!(ev("SINCE x"), Err("bad date".into()));
        assert_eq!(ev("FROM"), Err("truncated".into()));
        assert_eq!(ev("(ALL"), Err("unclosed".into()));
        assert_eq!(ev(r#""q""#), Err("unexpected ('q', 'q')".into()));
        assert_eq!(ev(")"), Err("unexpected ')'".into()));
        assert_eq!(ev("Unseen"), Err("truncated".into())); // 先取参数再认关键字，和原来一样
        assert_eq!(ev("Unseen x"), Err("unsupported search key Unseen".into()));
    }

    #[test]
    fn uid_sets() {
        let u = [1, 3, 5];
        let s = |spec: &str| {
            let mut v: Vec<i128> = uid_set(spec, &u).unwrap().into_iter().collect();
            v.sort();
            v
        };
        assert_eq!(s("1:3"), [1, 3]);
        assert_eq!(s("4:*"), [5]);
        assert_eq!(s("*"), [5]);
        assert_eq!(s("9,3"), [3, 9]);
        assert!(uid_set("a", &u).is_none());
        assert!(uid_set("", &u).is_none());
    }

    #[test]
    fn fetch_items_that_would_mark_read() {
        for (items, bad) in [
            ("(BODY.PEEK[])", false),
            ("(BODY[])", true),
            ("(RFC822)", true),
            ("(RFC822.SIZE RFC822.HEADER)", false),
            ("(RFC822.TEXT)", true),
            ("(XRFC822)", false),
            ("(RFC8220)", false),
        ] {
            let up = py::upper(items);
            assert_eq!(re!(r"\bBODY\[").is_match(&up) || rfc822_body(&up), bad, "{items}");
        }
        assert_eq!(idate(1763200800.5), "15-Nov-2025 10:00:00 +0000");
    }
}
