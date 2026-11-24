//! 登录邮件（原来的 build_eml、put_eml）：Anthropic 的样子——DKIM 签名、Gmail 写的 Authentication-Results、HTML 和纯文本两份正文，
//! 各处都能改。随机的部分（链接里的记号、签名、Message-ID、分隔线）和 Date 头的时间从 [`Source`] 来：测试要钉住时给一个固定的，
//! 平时用 [`Random`]。同样的随机部分和时间，写出来的字节和原来的 Python 一模一样。

use super::py;
use ring::rand::{SecureRandom, SystemRandom};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// string.ascii_letters + string.digits（先小写再大写，顺序和 Python 的一样：钉住随机数的测试按下标挑字符）
pub const ALNUM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
/// 签名用的：ALNUM 加上 + 和 /
pub const B64: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/";

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 登录链接的默认样子：GOOD_LINK % 记号
pub fn good_link(tok: &str) -> String {
    format!("https://claude.ai/magic-link#tok-{tok}:YWxpY2VAZXhhbXBsZS5jb20")
}

/// 随机的部分和现在的时间从哪来
pub trait Source {
    /// rand(n, chars)：从 chars 里随便挑 n 个字符
    fn rand(&mut self, n: usize, chars: &str) -> String;
    /// 现在的时间（Date 头用；time.time()）
    fn now(&mut self) -> f64;
}

/// 真的随机数（ring）和系统时间
pub struct Random;

impl Source for Random {
    fn rand(&mut self, n: usize, chars: &str) -> String {
        let cs: Vec<char> = chars.chars().collect();
        if cs.is_empty() || cs.len() > 256 {
            return String::new();
        }
        // 只要 256 里能整除的那一截，免得前面几个字符出现得多一点
        let limit = 256 - 256 % cs.len();
        let rng = SystemRandom::new();
        let mut out = String::with_capacity(n);
        let (mut got, mut buf) = (0, [0u8; 64]);
        while got < n {
            if rng.fill(&mut buf).is_err() {
                panic!("取不到随机数");
            }
            for &b in &buf {
                if (b as usize) < limit && got < n {
                    out.push(cs[b as usize % cs.len()]);
                    got += 1;
                }
            }
        }
        out
    }
    fn now(&mut self) -> f64 {
        py::now()
    }
}

/// 钉住的随机部分和时间（测试、和原来的 Python 逐字节对照时用）：第 k 次（从 1 数起）取 n 个字符是 chars[(i*7 + k) % chars.len()]
/// （i 从 0 数起），现在的时间一直是 now
#[derive(Clone, Debug)]
pub struct Pinned {
    calls: usize,
    now: f64,
}

impl Pinned {
    pub fn new(now: f64) -> Pinned {
        Pinned { calls: 0, now }
    }
}

impl Source for Pinned {
    fn rand(&mut self, n: usize, chars: &str) -> String {
        let cs: Vec<char> = chars.chars().collect();
        self.calls += 1;
        if cs.is_empty() {
            return String::new();
        }
        (0..n).map(|i| cs[(i * 7 + self.calls) % cs.len()]).collect()
    }
    fn now(&mut self) -> f64 {
        self.now
    }
}

/// Gmail 写的最上面那条 Authentication-Results 里 DKIM 的结果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ar {
    /// dkim=pass（默认）
    Pass,
    /// dkim=fail
    Fail,
    /// dkim=none：没有 DKIM 的结果
    None,
    /// 没有这一条 Authentication-Results
    Missing,
}

impl FromStr for Ar {
    type Err = String;
    /// pass、fail、none、missing；别的是原来的 KeyError
    fn from_str(s: &str) -> Result<Ar, String> {
        match s {
            "pass" => Ok(Ar::Pass),
            "fail" => Ok(Ar::Fail),
            "none" => Ok(Ar::None),
            "missing" => Ok(Ar::Missing),
            _ => Err(format!("KeyError: {}", py::repr_str(s))),
        }
    }
}

/// 一封登录邮件。字段和原来 build_eml 的参数一一对应（Eml::new(收件人) 是全用默认值），
/// 可以直接改字段，也可以链着写：`Eml::new("alice@example.com").ar(Ar::Fail).link("https://…").build()`
#[derive(Clone, Debug)]
pub struct Eml {
    /// To
    pub to: String,
    /// 正文里的登录链接；None 或空串时是 good_link(随机的 12 个字符)
    pub link: Option<String>,
    /// Subject（默认 Your secure link to Claude is here）
    pub subject: String,
    /// From（默认 Anthropic <no-reply@mail.anthropic.com>）；原来的参数名是 frm
    pub frm: String,
    /// DKIM 签名的 h=（默认 from:to:subject:date:message-id:mime-version:content-type）
    pub sign: String,
    /// 最上面那条 Authentication-Results 里 DKIM 的结果
    pub ar: Ar,
    /// DKIM 签名的 d=，也是 Authentication-Results 里的 header.i=@…（默认 mail.anthropic.com）
    pub dkim_d: String,
    /// Message-ID；None 或空串时是 <随机的 20 个字符@mail.anthropic.com>
    pub msgid: Option<String>,
    /// 加在 DKIM-Signature 后面、From 前面的头（整行，不带换行）
    pub extra: Vec<String>,
    /// 插在最上面的头（模拟别人加的）
    pub above: Vec<String>,
    /// 正文里放这几个链接（None 是只放 link）
    pub body_links: Option<Vec<String>>,
    /// 要不要 HTML 那一份正文
    pub html: bool,
    /// 要不要纯文本那一份正文
    pub text: bool,
    /// Reply-To（None 是没有这个头）
    pub reply_to: Option<String>,
}

impl Eml {
    /// 收件人是 to、别的全用默认值
    pub fn new(to: &str) -> Eml {
        Eml {
            to: to.to_string(),
            link: None,
            subject: "Your secure link to Claude is here".to_string(),
            frm: "Anthropic <no-reply@mail.anthropic.com>".to_string(),
            sign: "from:to:subject:date:message-id:mime-version:content-type".to_string(),
            ar: Ar::Pass,
            dkim_d: "mail.anthropic.com".to_string(),
            msgid: None,
            extra: Vec::new(),
            above: Vec::new(),
            body_links: None,
            html: true,
            text: true,
            reply_to: None,
        }
    }
    pub fn link(mut self, v: &str) -> Eml {
        self.link = Some(v.to_string());
        self
    }
    pub fn subject(mut self, v: &str) -> Eml {
        self.subject = v.to_string();
        self
    }
    pub fn frm(mut self, v: &str) -> Eml {
        self.frm = v.to_string();
        self
    }
    pub fn sign(mut self, v: &str) -> Eml {
        self.sign = v.to_string();
        self
    }
    pub fn ar(mut self, v: Ar) -> Eml {
        self.ar = v;
        self
    }
    pub fn dkim_d(mut self, v: &str) -> Eml {
        self.dkim_d = v.to_string();
        self
    }
    pub fn msgid(mut self, v: &str) -> Eml {
        self.msgid = Some(v.to_string());
        self
    }
    pub fn extra(mut self, lines: &[&str]) -> Eml {
        self.extra = lines.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn above(mut self, lines: &[&str]) -> Eml {
        self.above = lines.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn body_links(mut self, links: &[&str]) -> Eml {
        self.body_links = Some(links.iter().map(|s| s.to_string()).collect());
        self
    }
    pub fn html(mut self, v: bool) -> Eml {
        self.html = v;
        self
    }
    pub fn text(mut self, v: bool) -> Eml {
        self.text = v;
        self
    }
    pub fn reply_to(mut self, v: &str) -> Eml {
        self.reply_to = Some(v.to_string());
        self
    }

    /// 邮件的字节（随机的部分是真随机的，Date 是现在）
    pub fn build(&self) -> Vec<u8> {
        self.build_with(&mut Random)
    }

    /// 邮件的字节，随机的部分和时间从 src 来。取的先后和原来一样：链接的记号（没给链接时）、签名、Message-ID（没给时）、
    /// bh=、Date 的时间、分隔线
    pub fn build_with(&self, src: &mut dyn Source) -> Vec<u8> {
        let link = match self.link.as_deref() {
            Some(l) if !l.is_empty() => l.to_string(),
            _ => good_link(&src.rand(12, ALNUM)),
        };
        let b: Vec<char> = (src.rand(340, B64) + "==").chars().collect();
        let cut = |from: usize, to: usize| -> String { b[from.min(b.len())..to.min(b.len())].iter().collect() };
        let msgid = match self.msgid.as_deref() {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => format!("<{}@mail.anthropic.com>", src.rand(20, ALNUM)),
        };
        let mut lines: Vec<String> = self.above.clone();
        lines.push("Delivered-To: inbox@example.com".to_string());
        lines.push("Received: by 2002:a05:6000:1:b0:1 with SMTP id x; Sat, 15 Nov 2026 02:00:00 -0800 (PST)".to_string());
        let d = &self.dkim_d;
        let res = match self.ar {
            Ar::Pass => Some(format!("dkim=pass header.i=@{d} header.s=s1 header.b={}", cut(0, 8))),
            Ar::Fail => Some(format!("dkim=fail header.i=@{d} header.s=s1 header.b={}", cut(0, 8))),
            Ar::None => Some("dkim=none".to_string()),
            Ar::Missing => None,
        };
        if let Some(res) = res {
            lines.push("Authentication-Results: mx.google.com;".to_string());
            lines.push(format!("       {res};"));
            lines.push(
                "       arc=pass (i=1 spf=pass spfdomain=mail.anthropic.com dkim=pass dkdomain=mail.anthropic.com);".to_string(),
            );
            lines.push("       spf=pass (google.com: domain of bounce@mail.anthropic.com designates 192.0.2.10 as permitted sender) smtp.mailfrom=bounce@mail.anthropic.com;".to_string());
            lines.push("       dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=mail.anthropic.com".to_string());
        }
        lines.push(format!("DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d={d}; s=s1;"));
        lines.push(format!("\tt=1763200800; h={};", self.sign));
        lines.push(format!("\tbh={}=;", src.rand(43, ALNUM)));
        lines.push(format!("\tb={}", cut(0, 76)));
        let mut i = 76;
        while i < b.len() {
            lines.push(format!("\t {}", cut(i, i + 76)));
            i += 76;
        }
        lines.extend(self.extra.iter().cloned());
        lines.push(format!("From: {}", self.frm));
        lines.push(format!("To: {}", self.to));
        lines.push(format!("Subject: {}", self.subject));
        if let Some(r) = &self.reply_to {
            lines.push(format!("Reply-To: {r}"));
        }
        lines.push(format!("Date: {}", formatdate(src.now())));
        lines.push(format!("Message-ID: {msgid}"));
        lines.push("MIME-Version: 1.0".to_string());
        let links: Vec<String> = self.body_links.clone().unwrap_or_else(|| vec![link.clone()]);
        let boundary = format!("b-{}", src.rand(16, ALNUM));
        let mut parts: Vec<String> = Vec::new();
        if self.text {
            parts.push(format!(
                "Content-Type: text/plain; charset=\"utf-8\"\nContent-Transfer-Encoding: 7bit\n\n\
                 Click the link below to log in to Claude.ai:\n\n{}\n\nThis link expires in 1 hour.\n",
                links.join("\n")
            ));
        }
        if self.html {
            let anchors: String = links
                .iter()
                .map(|l| format!("<a href=3D\"{}\" style=3D\"color:#fff\">Sign in to Claude.ai</a><br>", l.replace('=', "=3D")))
                .collect();
            parts.push(format!(
                "Content-Type: text/html; charset=\"utf-8\"\nContent-Transfer-Encoding: quoted-printable\n\n\
                 <html><body><p>Click the button to log in:</p>{anchors}<p>If you did not request this, ignore it.</p></body></html>\n"
            ));
        }
        lines.push(format!("Content-Type: multipart/alternative; boundary=\"{boundary}\""));
        let mut body: String = parts.iter().map(|p| format!("--{boundary}\n{p}")).collect();
        body.push_str(&format!("--{boundary}--\n"));
        let mut out = lines.join("\r\n");
        out.push_str("\r\n\r\n");
        out.push_str(&body.replace('\n', "\r\n"));
        out.into_bytes()
    }
}

/// email.utils.formatdate(t, localtime=False)：Sat, 15 Nov 2025 10:00:00 -0000（秒以下照 datetime 四舍六入五成双到微秒，进位到秒）
pub fn formatdate(t: f64) -> String {
    let mut secs = t.trunc();
    let us = ((t - secs) * 1e6).round_ties_even();
    if us >= 1e6 {
        secs += 1.0;
    } else if us < 0.0 {
        secs -= 1.0;
    }
    let g = py::gmtime_secs(secs as i64);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} -0000",
        WEEKDAYS[g.wday as usize],
        g.mday,
        MONTHS[(g.mon - 1) as usize],
        g.year,
        g.hour,
        g.min,
        g.sec
    )
}

/// 秒（小数）→ 时间：照 os.utime，小数部分往下取整到纳秒
fn system_time(at: f64) -> io::Result<SystemTime> {
    if !at.is_finite() || at.abs() >= 9.2e18 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "timestamp out of range for platform time_t"));
    }
    let mut sec = at.trunc();
    let mut ns = ((at - sec) * 1e9).floor();
    if ns >= 1e9 {
        ns -= 1e9;
        sec += 1.0;
    } else if ns < 0.0 {
        ns += 1e9;
        sec -= 1.0;
    }
    let (sec, ns) = (sec as i64, ns as u32);
    Ok(if sec >= 0 {
        UNIX_EPOCH + Duration::new(sec as u64, ns)
    } else {
        UNIX_EPOCH - Duration::from_secs(sec.unsigned_abs()) + Duration::from_nanos(u64::from(ns))
    })
}

/// os.path.dirname：最后一个 / 前面的部分，去掉结尾的 /（全是 / 时留着）
fn dirname(p: &[u8]) -> &[u8] {
    let head = match p.iter().rposition(|b| *b == b'/') {
        Some(i) => &p[..=i],
        None => &[],
    };
    if head.iter().all(|b| *b == b'/') {
        return head;
    }
    let end = head.iter().rposition(|b| *b != b'/').map_or(0, |e| e + 1);
    &head[..end]
}

/// 把邮件放进邮箱（原来的 put_eml）：先写 路径.tmp、把修改时间（Gmail 收到的时间）设成 at（None 是现在），再换上去。
/// 目录不在就建；路径的写法照原来按字符串算（路径里没有目录部分时和原来一样出错：os.makedirs('')）
pub fn put_eml(path: &Path, data: &[u8], at: Option<f64>) -> io::Result<()> {
    let dir = dirname(path.as_os_str().as_bytes());
    if dir.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "[Errno 2] No such file or directory: ''"));
    }
    fs::create_dir_all(OsStr::from_bytes(dir))?;
    let mut tmp = OsString::from(path.as_os_str());
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    let mut f = File::create(tmp)?;
    f.write_all(data)?;
    let t = system_time(at.unwrap_or_else(py::now))?;
    f.set_times(FileTimes::new().set_accessed(t).set_modified(t))?;
    drop(f);
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_like_email_utils() {
        assert_eq!(formatdate(1763200800.9999996), "Sat, 15 Nov 2025 10:00:01 -0000");
        assert_eq!(formatdate(0.0), "Thu, 01 Jan 1970 00:00:00 -0000");
        assert_eq!(formatdate(1763200800.4), "Sat, 15 Nov 2025 10:00:00 -0000");
    }

    #[test]
    fn shape() {
        let raw = Eml::new("alice@example.com").ar(Ar::Missing).reply_to("X <x@example.com>").build_with(&mut Pinned::new(0.0));
        let s = String::from_utf8(raw).unwrap();
        assert!(s.starts_with("Delivered-To: inbox@example.com\r\nReceived: "));
        assert!(!s.contains("Authentication-Results"));
        assert!(s.contains("\r\nReply-To: X <x@example.com>\r\nDate: Thu, 01 Jan 1970 00:00:00 -0000\r\n"));
        assert!(s.contains("\r\n\r\n--b-"));
        assert!(s.ends_with("--\r\n"));
        assert!(!s.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn put_sets_the_time() {
        let d = std::env::temp_dir().join(format!("cca-mailfake-put-{}", std::process::id()));
        let p = d.join("box").join("all").join("3.eml");
        put_eml(&p, b"x", Some(1763200800.1234567)).unwrap();
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(&p).unwrap();
        assert_eq!((m.mtime(), m.mtime_nsec()), (1763200800, 123456716));
        assert!(put_eml(Path::new("x.eml"), b"x", None).is_err());
        assert_eq!(dirname(b"a/b/"), b"a/b");
        assert_eq!(dirname(b"a//b"), b"a");
        assert_eq!(dirname(b"//x"), b"//");
        assert_eq!(dirname(b"x"), b"");
        fs::remove_dir_all(&d).unwrap();
    }
}
