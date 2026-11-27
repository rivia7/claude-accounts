//! 邮箱助手（cca mail，只在 macOS 上）：Claude 账号的邮箱转发到一个 Gmail。账号登录时 Anthropic 发来的登录邮件，cca 经 IMAP
//! （imap.gmail.com:993，Gmail 的应用专用密码）只读地找到它，把里面的登录链接交给这个账号专属的 Chrome 打开。
//! 对应 PY_SRC 的 `mail_knob` 到 `cmd_mail_link_check`：mail-setup / status / ready / get-user / forget / trust-forwarder / test /
//! open / watch / link-check、gui-mail-list。
//!
//! 分层（各在 mail/ 下的一份文件里）：路（route：照 macOS 的系统代理，不读环境变量——cca 起的进程里 https_proxy 是账号的出口）→
//! 隧道和 TLS（route：照常校验证书）→ 只读的 IMAP（imap：EXAMINE、UID SEARCH、BODY.PEEK，没有通用的命令入口）→ 核对
//! （judge：只信 Gmail 自己写的验证结果和 Anthropic 签过名的邮件头）→ 取链接（`MailRule::link_ok`）→ 盯邮箱（hunt）→ 打开
//! （cca _mail-open，链接经 stdin）。各站点的认法只在 `MAIL_RULES` 里，`Tool::mail_rules` 指过去。子命令在 cmds 里，
//! 这一份放常量、设置（config.json 的 mail 一节：`mail_cfg`、`mail_view`、`mail_ready`、`mail_last`）和各层共用的小东西。
//!
//! 链接是一次性的登录凭证：不打印、不记日志、不存盘、不进环境变量，只交给这个账号的 Chrome。应用专用密码只从钥匙串读、
//! 只在进程里经 TLS 交给 Gmail 的 LOGIN，不进任何进程的命令行、环境变量、文件和出错信息。
//!
//! 原来靠 Python 标准库的 imaplib、email、html.parser、ssl；这里是照 CPython 3.14 的行为手写的（codec、message、header、
//! html、imap 各一份），对照数据在 tests/vectors/mail.json（移植时从真的 Python 取的固定数据）。

mod cmds;
pub mod codec;
pub mod header;
pub mod html;
mod html5;
pub mod hunt;
pub mod imap;
pub mod judge;
pub mod message;
pub mod route;

use super::secrets::{keystore_name, ks_del, ks_read};
use super::Cmd;
use crate::core::base::CCA_HOME;
use crate::core::ctx::cfg_save;
use crate::core::model::{acct_id, cfg_load, parse_key};
use crate::core::policy::policy_sources;
use crate::core::team::{grants, unreadable_grants};
use crate::core::tool::{claude, t, Kind};
use crate::dict;
use crate::py::fs::{self, load_json};
use crate::py::obj::{str_list, Dict, V};
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::re::Re;
use crate::py::text;
use crate::py::time;
use crate::py::uni;
use crate::py::url::urlsplit;
use crate::py::{die, Fail, R};
use crate::re;
use route::{mail_route, Route};
use std::sync::LazyLock;

/// 这个模块移植好的子命令（各工具的差别只在 `Tool::mail_rules`、账号表和账号目录名上，三种工具走的是同一份代码）
pub const CMDS: &[Cmd] = &[
    Cmd::all("mail-setup", cmds::cmd_mail_setup),
    Cmd::all("mail-status", cmds::cmd_mail_status),
    Cmd::all("mail-ready", cmds::cmd_mail_ready),
    Cmd::all("mail-forget", cmds::cmd_mail_forget),
    Cmd::all("mail-trust-forwarder", cmds::cmd_mail_trust_forwarder),
    Cmd::all("mail-test", cmds::cmd_mail_test),
    Cmd::all("mail-open", cmds::cmd_mail_open),
    Cmd::all("mail-watch", cmds::cmd_mail_watch),
    Cmd::all("mail-link-check", cmds::cmd_mail_link_check),
    Cmd::all("mail-get-user", cmds::cmd_mail_get_user),
    Cmd::all("gui-mail-list", cmds::cmd_gui_mail_list),
];

pub const MAIL_HOST: &str = "imap.gmail.com";
pub const MAIL_PORT: u16 = 993;
/// 钥匙串（没有钥匙串的系统是密钥文件）里应用专用密码的服务名；账户名是 Gmail 地址：换了地址取不到旧的
pub const MAIL_SVC: &str = "cca-mail";
/// 700：used.json（打开过的登录邮件，只记摘要）、last.json（上次的结果）、used.lock
pub static MAIL_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "mail"));
/// cca mail open：最近半小时里收到的才算
pub const MAIL_RECENT: f64 = 1800.0;
/// 两边时钟的误差：登录开始前一分钟内收到的也算
pub const MAIL_SKEW: f64 = 60.0;
/// 一封邮件最多取这么多字节（登录邮件只有几十 KB）
pub const MAIL_FETCH_MAX: usize = 262144;
/// 按 UID 取新邮件的头时，一个邮件夹一轮最多取这么多封（最新的）
pub const MAIL_BURST: u64 = 50;
/// 登录时最多盯这么久（秒）
pub const MAIL_WATCH_MAX: f64 = 900.0;
pub static MAIL_DOMAIN_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[a-z0-9-]+(\.[a-z0-9-]+)+$"));
/// 公共邮箱的域名：Team 的「信任的转发中转」不收它们（也不收它们上面、下面的域名）——谁都能在那里开一个邮箱，信任它就等于谁发的登录邮件都认。
/// 只管 Team 的名单（管理员写的时候拒，设备上读的时候跳过）；这台设备自己登记的（cca mail trust-forwarder）照旧
#[rustfmt::skip]
pub const MAIL_PUBLIC: &[&str] = &[
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com", "yahoo.com", "ymail.com", "icloud.com", "me.com",
    "mac.com", "aol.com", "proton.me", "protonmail.com", "pm.me", "gmx.com", "gmx.net", "mail.com", "yandex.com", "yandex.ru", "zoho.com",
    "fastmail.com", "tutanota.com", "hey.com", "qq.com", "foxmail.com", "163.com", "126.com", "yeah.net", "sina.com", "sohu.com",
    "aliyun.com", "139.com",
];
/// Gmail 地址（小写；也是钥匙串的账户名，交给 security -i）
pub static MAIL_ADDR_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[a-z0-9._%+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+$"));
/// 先只取这些头来核对（Reply-To 给信任中转的判定：中转标注原件来自 Anthropic）
pub const MAIL_HDRS: &[&str] = &["FROM", "TO", "SUBJECT", "REPLY-TO", "MESSAGE-ID", "AUTHENTICATION-RESULTS", "DKIM-SIGNATURE"];
/// 这几个头必须在 Anthropic 的签名里、各只有一个：签名之外的头谁都能改、能加
pub const MAIL_SIGNED: &[&str] = &["from", "to", "subject"];
pub const MAIL_MONTHS: &[&str] = &["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub const MAIL_PROXY_HINT: &str = "连 Gmail 的方式可以指定：cca mail setup --proxy socks5://127.0.0.1:7897（direct 是直连）";
pub const MAIL_ORDER: &[&str] = &["old", "subject", "dup", "from", "dkim", "unsigned", "to", "nolink"];

/// 没通过核对的原因（按核对的先后）；refused 是 cca _mail-open 不肯打开（不是 mail_judge 的结果，不进 MAIL_ORDER）
pub fn mail_why(code: &str) -> Option<&'static str> {
    Some(match code {
        "old" => "是这次登录开始以前收到的",
        "subject" => "标题不像登录邮件",
        "dup" => "发件人、收件人或标题的邮件头有好几个（邮件被加过东西）",
        "from" => "发件人不是 Anthropic",
        "dkim" => "Gmail 的验证结果里没有 Anthropic 的签名通过（转发时邮件被改过，或者不是 Anthropic 发的）",
        "unsigned" => "发件人、收件人、标题不全在 Anthropic 的签名里，认不出是不是发给这个账号的",
        "to" => "收件人不是这个账号的邮箱",
        "nolink" => "邮件里没找到登录链接（邮件的格式变了？）",
        "refused" => "打开之前再核对一遍链接时没通过",
        _ => return None,
    })
}

/// 测试用的时间开关（CCA_MAIL_POLL 几秒看一次邮箱、CCA_MAIL_RETRY 连不上 Gmail、浏览器没打开时先等多久）：不是正数时用默认值
pub fn mail_knob(name: &str, default: f64) -> f64 {
    knob_value(getenv(name).as_deref(), default)
}

/// `float(值 or default)`：认不出、不是正数（nan 也算）时用默认值。\x1c–\x1f 在 `str.strip()` 里算空白，float() 不认
/// （`py::text::to_float` 是照 strip 写的，这一点和 Python 不一样：带这几个字符的这里先挡掉）
fn knob_value(raw: Option<&str>, default: f64) -> f64 {
    let v = match raw.filter(|s| !s.is_empty()) {
        Some(s) if s.contains(|c| ('\x1c'..='\x1f').contains(&c)) => default,
        Some(s) => text::to_float(s).unwrap_or(default),
        None => default,
    };
    if v > 0.0 {
        v
    } else {
        default
    }
}

/// `int(s)` 对一段文字：前后的空白、正负号、数字之间的下划线、各种文字的十进制数字都认。返回 (是不是负的, 十进制的写法：
/// ASCII 数字、没有前导 0)；认不出、位数超过 4300（Python 给 int() 定的上限，前导 0 也算）是 None（原来的 ValueError）。
/// 两头的空白只认 ASCII 的空格、\t \n \v \f \r 和别的文字里的空白：\x1c–\x1f 在 `str.strip()` 里算空白，int() 不认
/// （`py::text::to_int` 是照 strip 写的，这一点和 Python 不一样；它也不管位数的上限、放不进 64 位的当认不出）
pub(crate) fn py_int(s: &str) -> Option<(bool, String)> {
    let blank = |c: char| matches!(c, ' ' | '\t'..='\r') || (!c.is_ascii() && uni::is_space(c));
    let s = s.trim_matches(blank);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut digits = String::new();
    let mut prev_digit = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        digits.push(char::from_digit(uni::decimal_value(c)?, 10)?);
        prev_digit = true;
    }
    if digits.is_empty() || !prev_digit || digits.len() > codec::INT_MAX_STR_DIGITS {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() { (false, "0".to_string()) } else { (neg, trimmed.to_string()) })
}

/// 一类登录邮件的认法：MAIL_RULES 里一条一项（以后加 Console 的登录链接也是加一条规则；只认链接，邮件里的验证码装不下）
pub struct MailRule {
    pub name: &'static str,
    pub what: &'static str,
    /// 发件人和 DKIM 签名的域名（按标签认后缀：mail.anthropic.com 算，evilanthropic.com 不算）
    pub domains: &'static [&'static str],
    /// 标题：原来是不分大小写的正则，用了前瞻；这里一条正则对一个手写的判断
    pub subjects: &'static [fn(&str) -> bool],
    /// 在 Gmail 上粗筛的 IMAP 搜索条件（只为少取几封，本地照样逐项核对）
    pub search: &'static [&'static str],
    /// 链接的主机（原样相等）
    pub hosts: &'static [&'static str],
    /// 链接的路径（原样相等；/ 结尾的按前缀）
    pub paths: &'static [&'static str],
}

impl MailRule {
    pub fn domain_ok(&self, d: &str) -> bool {
        let d = d.to_lowercase();
        self.domains.iter().any(|x| d == *x || d.ends_with(&format!(".{x}")))
    }

    pub fn subject_ok(&self, s: &str) -> bool {
        self.subjects.iter().any(|p| p(s))
    }

    /// 是不是这一类的登录链接：https、主机原样相等（挡住 claude.ai.evil.com、claude.ai@evil.com、带端口、大写这些写法）、
    /// 路径在名单里；只有可见的 ASCII，没有引号、尖括号、反斜杠，不超过 2048 个字符
    pub fn link_ok(&self, url: &str) -> bool {
        if text::len(url) > 2048 || !re!(r"^[\x21-\x7e]+$").matches(url) || re!(r"[\x22\x27<>\\]").found(url) {
            return false;
        }
        let Ok(u) = urlsplit(url) else {
            return false;
        };
        u.scheme == "https"
            && self.hosts.contains(&u.netloc.as_str())
            && self.paths.iter().any(|p| u.path == *p || (p.ends_with('/') && u.path.starts_with(p)))
    }
}

/// Python 的 `\w`：`isalnum()` 的字符和下划线
fn is_word(c: char) -> bool {
    c == '_' || uni::is_alnum(c)
}

/// 正则里一个小写字母在 `re.I` 下认哪些字符：大小写两种，再加 Python 的 re 另外认的那几个（K 是开尔文的符号，
/// İ、ı 当 i，ſ 当 s；`regex` 库不认 İ、ı）
fn ci(pattern: char, c: char) -> bool {
    c.to_ascii_lowercase() == pattern
        || match pattern {
            'i' => matches!(c, '\u{130}' | '\u{131}'),
            's' => c == '\u{17f}',
            'k' => c == '\u{212a}',
            _ => false,
        }
}

/// 从 at 起是不是这段字（小写字母不分大小写，别的原样）：是的话给出后面的位置
fn ci_at(cs: &[char], at: usize, literal: &str) -> Option<usize> {
    let mut i = at;
    for p in literal.chars() {
        if !cs.get(i).is_some_and(|c| ci(p, *c)) {
            return None;
        }
        i += 1;
    }
    Some(i)
}

/// `re.search(r'\bsecure link to (log in to )?claude(\.ai)?(?![\w.-])', s, re.I)`。
/// 标题里 claude 后面那个词是应用名（attacker 能把 OAuth 应用取名叫别的）：只认应用名是 Claude / Claude.ai 的
fn subject_claude_login(s: &str) -> bool {
    let cs: Vec<char> = s.chars().collect();
    // (?![\w.-])：后面不能再跟着字、点、连字符
    let ends = |i: usize| !cs.get(i).is_some_and(|c| is_word(*c) || matches!(c, '.' | '-'));
    let tail = |i: usize| -> bool {
        let Some(i) = ci_at(&cs, i, "claude") else { return false };
        ci_at(&cs, i, ".ai").is_some_and(ends) || ends(i)
    };
    (0..cs.len()).any(|start| {
        // \b：前面不是字
        if start > 0 && is_word(cs[start - 1]) {
            return false;
        }
        let Some(i) = ci_at(&cs, start, "secure link to ") else { return false };
        ci_at(&cs, i, "log in to ").is_some_and(tail) || tail(i)
    })
}

/// claude.ai 用邮箱登录时发的登录邮件。真邮件（2026-11-16 核对）标题是 Your secure link to Claude is here，发件人
/// no-reply-<随机>@mail.anthropic.com，Gmail 验 dkim=pass mail.anthropic.com。帮助中心写的是 Secure link to log in to
/// Claude.ai、也有人收到过 Your secure link to Claude.ai is here，一并认。标题里 claude 后面那个词是应用名（attacker 能把 OAuth
/// 应用取名叫别的，比如见过 Your secure link to Verification Portal is here）：所以只认应用名是 Claude / Claude.ai 的，别的不开
pub static MAIL_RULES: [MailRule; 1] = [MailRule {
    name: "claude-login",
    what: "claude.ai 的登录链接",
    domains: &["anthropic.com"],
    subjects: &[subject_claude_login],
    search: &["OR", "FROM", "\"anthropic.com\"", "SUBJECT", "\"secure link\""],
    hosts: &["claude.ai"],
    paths: &["/magic-link"],
}];

/// `MAIL_RULES[名字]`
pub fn mail_rule(name: &str) -> Option<&'static MailRule> {
    MAIL_RULES.iter().find(|r| r.name == name)
}

/// 连 Gmail 出的错（原来的 MailError）：code 是 net、proxy、tls、auth、imap、login、proto 之一，text 给人看。
/// 过一会儿可能就好了（网络、代理、Gmail 那边断开）：整个会话丢掉，等一会儿重连
pub fn mail_retry(code: &str, text: impl Into<String>) -> Fail {
    Fail::Other("MailRetry", vec![code.to_string(), text.into()])
}

/// 不会自己好：密码不对、没开 IMAP、证书校验不过、连接方式写错了
pub fn mail_fatal(code: &str, text: impl Into<String>) -> Fail {
    Fail::Other("MailFatal", vec![code.to_string(), text.into()])
}

/// `except MailError as e`：(code, text)
pub fn mail_error(e: &Fail) -> Option<(&str, &str)> {
    match e {
        Fail::Other("MailRetry" | "MailFatal", args) if args.len() == 2 => Some((&args[0], &args[1])),
        _ => None,
    }
}

/// 别人写的文字（邮件头、Gmail 的回话）打到终端以前：控制字符（C0、DEL、C1）和看不见的、改变显示顺序的格式字符换成空格，
/// 免得里面的转义序列擦掉、改写前面的输出
pub fn mail_clean(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0x00..=0x1f | 0x7f..=0x9f | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2069 | 0xfeff => ' ',
            _ => c,
        })
        .collect()
}

/// 出错的原因（给人看的一小段：一行、不带控制字符，imaplib 的 b'…' 去掉）。text 是原来的
/// `str(getattr(e, 'strerror', None) or e or type(e).__name__)`，class 是 `type(e).__name__`（text 洗完是空的时候用）
pub fn mail_oserr(text: &str, class: &str) -> String {
    let s = re!(r"^b[\x22\x27](.*)[\x22\x27]$").sub(r"\1", text);
    let out = text::split_ws(&mail_clean(&s)).join(" ");
    let out = text::head(&out, 160);
    if out.is_empty() {
        class.to_string()
    } else {
        out.to_string()
    }
}

/// Ctrl-C 按过了（`cca mail open` 等邮件的时候）：原来是 Python 在主线程里抛 KeyboardInterrupt，这里是信号处理函数记一笔，
/// 等待、读写出错的地方看到了就照 KeyboardInterrupt 往上传
static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 从现在起 Ctrl-C 不直接结束进程，而是记下来（不带 SA_RESTART：正卡着的读写、睡眠会被打断，回来就看得到）。
/// 进程起来时 SIGINT 就是被忽略的（放在后台跑的）照旧忽略：Python 也只在它不是被忽略的时候才装自己的处理函数
pub(crate) fn catch_sigint() {
    // SAFETY: sigaction 是纯数据，全 0 是合法的值；处理函数只存一个原子变量
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN {
            return;
        }
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_sigint as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

/// 原来的 KeyboardInterrupt（不是 Exception 的子类：`except Exception`、`except MailError` 都接不住它）
pub(crate) fn keyboard_interrupt() -> Fail {
    Fail::Other("KeyboardInterrupt", Vec::new())
}

/// 按过 Ctrl-C 了就别往下做了
pub(crate) fn check_interrupt() -> R<()> {
    if interrupted() {
        return Err(keyboard_interrupt());
    }
    Ok(())
}

thread_local! {
    /// 原来的 Python 里，现在在子命令的那个函数（cmd_mail_test……）下面第几层函数调用里
    static PY_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 原来这里是一层 Python 的函数调用（见 `py_frame`）；离开作用域时退出这一层
pub(crate) struct PyFrame(());

/// 进一层原来的 Python 函数。地址里的注释、邮件的结构一层套一层时，原来是到 Python 调用栈的一千层为止（RecursionError），
/// 能套几层就看解析它的那个函数自己在第几层：`cmd_mail_test` 里直接解析收件人，和登录时
/// `cmd_mail_watch → mail_watch → mail_hunt → mail_judge_any → mail_judge` 里解析发件人，差四层。从子命令到解析之间的
/// 每个函数开头调一下这个，上限就跟着对上（CPython 3.14.0 上各条路实测过，见 header.rs 的 `ADDR_FRAMES`、message.rs 的 `MAX_DEPTH`）
pub(crate) fn py_frame() -> PyFrame {
    PY_DEPTH.with(|d| d.set(d.get() + 1));
    PyFrame(())
}

impl Drop for PyFrame {
    fn drop(&mut self) {
        PY_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

pub(crate) fn py_depth() -> usize {
    PY_DEPTH.with(|d| d.get())
}

/// 邮件的结构、地址里的注释原来是一层一层递归着解析的，这里照样（层数到了 Python 的上限报 RecursionError）。
/// 别人写的邮件可以故意套上千层：这种放到一条栈够大的线程里跑，免得把栈用完、整个进程崩掉
pub(crate) fn with_deep_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().stack_size(256 << 20).spawn_scoped(scope, f).expect("起不了解析用的线程");
        worker.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// config.json 的 mail 或 mail.accounts[*] 一项 → {user, auto, proxy}；地址不对是 None。
/// 没写 auto / proxy 的按给的默认值（账号覆盖没写时继承默认那一项的）
pub fn mail_one(m: &V, auto_default: bool, proxy_default: &str) -> Option<Dict> {
    let m = m.as_dict()?;
    let user = m.str("user")?.to_lowercase();
    if !MAIL_ADDR_RE.matches(&user) {
        return None;
    }
    let auto = match m.get("auto") {
        Some(a) => !matches!(a, V::Bool(false)),
        None => auto_default,
    };
    let proxy = m.str("proxy").filter(|p| !p.is_empty()).map_or_else(|| proxy_default.to_string(), |p| p.to_string());
    Some(dict! { "user" => user, "auto" => auto, "proxy" => proxy })
}

/// 默认的邮箱助手设置（key 是 None），或某账号解析后的设置（覆盖 config.json 的 mail.accounts[acct_id] → 默认 mail）。
/// 返回 {user, auto, proxy, source: 'account'|'default', forwarder} 或 None（没有可连的 Gmail）。
/// forwarder 是这个账号单独登记的「信任转发中转」域名（不安全、默认不登记；不继承默认；只在 key 给定时有意义），没有是 None
pub fn mail_cfg(c: &Dict, key: Option<&str>) -> Option<Dict> {
    let m = c.dict("mail")?;
    let default = mail_one(&V::Dict(m.clone()), true, "system");
    let ov = key.and_then(|k| m.dict("accounts")?.dict(&acct_id(k, t())));
    let one = ov.as_ref().and_then(|ov| match &default {
        Some(d) => mail_one(&V::Dict(ov.clone()), d.at("auto").truthy(), &d.at("proxy").py_str()),
        None => mail_one(&V::Dict(ov.clone()), true, "system"),
    });
    let one = match (one, default) {
        (Some(one), _) => {
            one.set("source", "account");
            one
        }
        (None, Some(default)) => {
            default.set("source", "default");
            default
        }
        (None, None) => return None,
    };
    let fwd = ov.and_then(|ov| ov.str("forwarder")).map(|f| f.to_lowercase()).filter(|f| MAIL_DOMAIN_RE.matches(f));
    one.set("forwarder", fwd);
    Some(one)
}

/// 账号页的邮箱助手状态（_gui state；只读 config.json，不碰钥匙串）：{configured, user, source, auto, forwarder, team_forwarders}。
/// source：account（这个账号单独配的 Gmail）| default（用默认那个）| None（没配置）。
/// forwarder：这个账号登记的「信任转发中转」域名（不安全开关，默认 null）。team：这个账号是哪个 Team 给的（gui 已经算好的编号）：
/// 它信任哪些转发中转照那个 Team 的（team_forwarders：{team, team_name, domains, local}，local 是这台设备登记过的、现在不算数的那个），
/// forwarder 是 null；不是 Team 给的账号 team_forwarders 是 null
pub fn mail_view(c: &Dict, key: &str, team: Option<&str>) -> Dict {
    let d = match mail_cfg(c, Some(key)) {
        Some(m) => {
            dict! { "configured" => true, "user" => m.at("user"), "source" => m.at("source"), "auto" => m.at("auto"), "forwarder" => m.at("forwarder") }
        }
        None => dict! { "configured" => false, "user" => V::None, "source" => V::None, "auto" => true, "forwarder" => V::None },
    };
    match team.and_then(team_fwd) {
        Some(f) => {
            let local = d.at("forwarder");
            d.set("forwarder", V::None);
            d.set(
                "team_forwarders",
                dict! { "team" => f.tid.as_str(), "team_name" => f.team.as_str(), "domains" => str_list(&f.domains), "local" => local },
            );
        }
        None => d.set("team_forwarders", V::None),
    }
    d
}

/// Team 的转发中转名单最多几项、每一项最长几个字节（域名的上限）：管理员写的、导入的超了当场不收（不然存进去以后，给拿到 Claude 账号的
/// 设备发布时 doc_problems 都不收，这些设备什么改动都收不到）；设备拿到的配置 doc_problems 照它查样子
pub const FWD_MAX: usize = 64;
pub const FWD_LEN: usize = 253;

/// Team 的名单里的一项：去掉空白、转小写以后是一个能信任的中转域名；不是的话说为什么（不是域名、太长、公共邮箱的域名、像 co.uk 这样的公共后缀）
pub fn team_fwd_check(raw: &str) -> Result<String, String> {
    let d = text::strip(raw).to_lowercase();
    if !MAIL_DOMAIN_RE.matches(&d) {
        return Err(format!("「{}」不是域名：写成 relay.example.com 这样（不带 @、http://、端口和空格）", text::strip(raw)));
    }
    if d.len() > FWD_LEN || d.split('.').any(|l| l.len() > 63) {
        return Err(format!("「{d}」太长了：域名最长 {FWD_LEN} 个字符，每一段最多 63 个"));
    }
    if MAIL_PUBLIC.iter().any(|p| judge::mail_fwd_ok(p, &d) || judge::mail_fwd_ok(&d, p)) {
        return Err(format!("「{d}」是公共邮箱的域名：谁都能在那里开一个邮箱，信任它就等于谁发的登录邮件都认"));
    }
    let labels: Vec<&str> = d.split('.').collect();
    let suffix = ["co", "com", "net", "org", "gov", "edu", "ac", "ne", "or", "go"];
    if labels.len() == 2 && suffix.contains(&labels[0]) && labels[1].len() == 2 {
        return Err(format!("「{d}」是公共的后缀：写中转自己的域名（像 relay.example.co.uk）"));
    }
    Ok(d)
}

/// 规整一份名单：去重，去掉被别的项盖住的（信任 example.com 就信任了 relay.example.com），排好序
pub fn fwd_norm(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = items.into_iter().collect();
    v.sort();
    v.dedup();
    let all = v.clone();
    v.retain(|d| !all.iter().any(|f| f != d && judge::mail_fwd_ok(d, f)));
    v
}

/// new 比 old 宽：多了 old 盖不住的域名（管理员改的时候要先问同意）
pub fn fwd_widens(old: &[String], new: &[String]) -> bool {
    new.iter().any(|d| !old.iter().any(|f| judge::mail_fwd_ok(d, f)))
}

/// 一份 Team 的名单（Team 给这台设备的配置、admin.json 的 config 里的 mail_forwarders）：逐项过 team_fwd_check，不合格的跳过（以后公共邮箱的
/// 名单变长了，也不会让整份配置作废），再 fwd_norm
pub fn doc_forwarders(d: &Dict) -> Vec<String> {
    let items = d.list("mail_forwarders").map(|l| l.items()).unwrap_or_default();
    fwd_norm(items.iter().filter_map(|x| team_fwd_check(x.as_str()?).ok()))
}

/// 一个 Team 给的 Claude 账号信任哪些转发中转
pub struct TeamFwd {
    pub tid: String,
    pub team: String,
    /// 可以是空的：一个都不信任
    pub domains: Vec<String>,
}

/// 编号是 tid 的 Team 的名单（它给这台设备的配置里的；这台设备不在它里面、还没同步到配置是 None）
pub fn team_fwd(tid: &str) -> Option<TeamFwd> {
    let gs = grants();
    let g = gs.iter().find(|g| g.id == tid)?;
    Some(TeamFwd { tid: g.id.clone(), team: g.name.clone(), domains: doc_forwarders(&g.doc) })
}

/// 这个邮箱的 Claude 账号是 Team 给的：它信任哪些转发中转由那个 Team 定（同一个账号在两个 Team 里，先加入的算数：policy_sources）
pub fn team_fwd_of(email: &str) -> Option<TeamFwd> {
    team_fwd(&policy_sources(claude()).str(&email.to_lowercase())?)
}

/// 这个账号信任的转发中转（判定登录邮件时用）：Team 给的 Claude 账号照 Team 的名单（这台设备登记的不算），别的照这台设备登记的（0 或 1 个）。
/// 有 Team 的配置读不出来时（unreadable_grants：说不准这个账号是不是它给的、它信不信），Claude 账号这台设备登记的也先不算：一个都不信
pub fn mail_forwarders(c: &Dict, key: &str) -> Vec<String> {
    let is_claude = t().kind == Kind::Claude;
    if is_claude {
        if let Some(f) = team_fwd_of(&parse_key(key).1) {
            return f.domains;
        }
    }
    let Some(f) = mail_cfg(c, Some(key)).and_then(|m| m.str("forwarder")) else { return Vec::new() };
    if is_claude && !unreadable_grants().is_empty() {
        return Vec::new();
    }
    vec![f.to_string()]
}

pub fn mail_pw_problem(rc: i32) -> String {
    if rc == 44 {
        format!("{}里没有应用专用密码：运行 cca mail setup", keystore_name())
    } else {
        format!("{}读不了应用专用密码（锁着？）：解锁后再试", keystore_name())
    }
}

/// (设置, 密码, 路)：没配置、密码取不到、连接方式写错了就说清楚、退出。key 给定时按这个账号解析（覆盖→默认）
pub fn mail_ready(c: Option<&Dict>, key: Option<&str>) -> R<(Dict, String, Route)> {
    let loaded;
    let c = match c.filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => {
            loaded = cfg_load()?;
            &loaded
        }
    };
    let Some(m) = mail_cfg(c, key) else {
        return die("还没配置邮箱助手：运行 cca mail setup");
    };
    let (rc, pw) = ks_read(MAIL_SVC, &m.at("user").py_str());
    if pw.is_empty() {
        return die(format!("邮箱助手：{}", mail_pw_problem(rc)));
    }
    match mail_route(Some(&m.at("proxy").py_str()), c, None) {
        Ok(route) => Ok((m, pw, route)),
        Err(e) if e.is("MailFatal") => die(mail_error(&e).map_or("", |x| x.1)),
        Err(e) => Err(e),
    }
}

/// 上次的结果（cca mail status 看）
pub fn mail_last(name: &str, text: &str) -> R<()> {
    let save = || -> R<()> {
        fs::makedirs_mode(&MAIL_DIR, 0o700)?;
        let last = dict! { "at" => time::time().trunc() as i64, "name" => name, "text" => text };
        fs::save_json(&path::join(&MAIL_DIR, "last.json"), &V::Dict(last), Some(0o600), true)
    };
    match save() {
        Err(e) if e.is_os() => Ok(()),
        done => done,
    }
}

/// 这个工具认哪几种登录邮件（`[MAIL_RULES[n] for n in T.mail_rules]`）
pub fn mail_rules() -> Vec<&'static MailRule> {
    t().mail_rules.iter().filter_map(|n| mail_rule(n)).collect()
}

/// config.json 里引用到的所有 Gmail 地址（默认 + 各账号覆盖），小写：换地址、移除时据此清钥匙串。
/// 原来是集合；这里按出现的先后（默认的在前），不重复
pub fn mail_all_addrs(c: &Dict) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(m) = c.dict("mail") else { return out };
    let overrides = m.dict("accounts").map(|a| a.values()).unwrap_or_default();
    for one in std::iter::once(V::Dict(m)).chain(overrides) {
        if let Some(user) = mail_one(&one, true, "system").map(|o| o.at("user").py_str()) {
            if !out.contains(&user) {
                out.push(user);
            }
        }
    }
    out
}

/// 清掉 config.json 里已经不在 Claude 账号表里的账号的 mail 覆盖（cca rm / edit 删了账号后；cmd_secrets_forget 调），
/// 地址没人再用了就删钥匙串里的应用专用密码。默认那个（mail.user）不动
pub fn mail_forget_orphans() -> R<()> {
    let c = cfg_load()?;
    let Some(m) = c.dict("mail") else { return Ok(()) };
    let Some(accts) = m.dict("accounts").filter(|a| !a.is_empty()) else { return Ok(()) };
    let mut live: Vec<String> = match load_json(&claude().table)? {
        None => Vec::new(),
        Some(V::Dict(table)) => table.keys().iter().map(|k| acct_id(k, claude())).collect(),
        Some(_) => Vec::new(),
    };
    // 收起来的账号（Team 不再给的；3.6.0 以前关掉共享时收起来的）：不在账号表里，单独配的邮箱照样留着（再给回来时加回来）
    live.extend(crate::core::policy::parked(&c, claude()).keys().iter().map(|a| a.to_string()));
    let orphan: Vec<_> = accts.keys().into_iter().filter(|aid| !live.iter().any(|l| **l == **aid)).collect();
    if orphan.is_empty() {
        return Ok(());
    }
    let before = mail_all_addrs(&c);
    for aid in &orphan {
        accts.pop(aid);
    }
    if accts.is_empty() {
        m.pop("accounts");
    }
    if m.is_empty() {
        c.pop("mail");
    }
    cfg_save(&c)?;
    let after = mail_all_addrs(&c);
    // 原来是两个集合相减，先后不定；这里按地址排好再删
    let mut gone: Vec<&String> = before.iter().filter(|a| !after.contains(a)).collect();
    gone.sort();
    for a in gone {
        ks_del(MAIL_SVC, Some(a))?;
    }
    Ok(())
}

/// 单元测试读对照数据（tests/vectors/mail.json）用的
#[cfg(test)]
pub(crate) mod testing {
    use crate::py::json;
    use crate::py::obj::{Dict, V};

    /// 长的、有重复段的字符串在文件里写成 {"rep": [[段, 次数], …]}（4301 个 9 这种）：读的时候展开
    fn expand(v: &V) -> V {
        match v {
            V::Dict(d) => match d.list("rep") {
                Some(rep) if d.len() == 1 => {
                    let part = |pair: &V| {
                        let pair = pair.as_list().expect("rep 的每一项是 [段, 次数]");
                        pair.at(0).py_str().repeat(pair.at(1).as_int().expect("次数") as usize)
                    };
                    V::from(rep.items().iter().map(part).collect::<String>())
                }
                _ => V::Dict(Dict::from_pairs(d.items().into_iter().map(|(k, x)| (k, expand(&x))))),
            },
            V::List(l) => V::from(l.items().iter().map(expand).collect::<Vec<_>>()),
            other => other.clone(),
        }
    }

    /// 对照数据的一节
    pub fn vectors(section: &str) -> Vec<Dict> {
        let pick = |doc: &V| -> Vec<Dict> {
            let cases = doc.as_dict().and_then(|d| d.dict("cases")).and_then(|c| c.list(section));
            cases.map(|l| l.items().iter().filter_map(|v| expand(v).as_dict()).collect()).unwrap_or_default()
        };
        let out = pick(&json::loads(include_str!("../../tests/vectors/mail.json")).unwrap());
        assert!(!out.is_empty(), "tests/vectors/mail.json 里没有 {section} 这一节");
        out
    }

    /// 字节在对照数据里写成 Latin-1 的文字：一个字符一个字节
    pub fn bytes_of(v: &V) -> Vec<u8> {
        v.py_str().chars().map(|c| u8::try_from(c as u32).expect("对照数据里的字节应该写成 256 以下的字符")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testing::vectors;

    #[test]
    fn clean_and_oserr_like_python() {
        let cases = vectors("oserr");
        assert!(cases.len() > 40);
        for c in cases {
            let (text, class) = (c.at("text").py_str(), c.at("class").py_str());
            assert_eq!(mail_clean(&text), c.at("clean").py_str(), "{text:?}");
            assert_eq!(mail_oserr(&text, &class), c.at("oserr").py_str(), "{text:?}");
        }
    }

    /// 登录链接只认正好是 Claude 登录链接的那一种：协议、主机、端口、路径差一点的，带引号、尖括号、空格、非 ASCII 字符的，太长的都不认
    #[test]
    fn link_must_be_exactly_a_claude_login_link() {
        let ok = |u: &str| MAIL_RULES[0].link_ok(u);
        assert!(ok("https://claude.ai/magic-link#tok-abc:YWxpY2U"));
        assert!(ok("https://claude.ai/magic-link?x=1#t"));
        let long = format!("https://claude.ai/magic-link#{}", "a".repeat(2048));
        for bad in [
            "http://claude.ai/magic-link#t",
            "https://claude.ai.evil.com/magic-link#t",
            "https://claude.ai@evil.com/magic-link#t",
            "https://evil.com/claude.ai/magic-link",
            "https://claude.ai:443/magic-link#t",
            "https://Claude.ai/magic-link#t",
            "https://claude.ai/magic-linkx#t",
            "https://claude.ai/magic-link/../logout",
            "https://claude.ai/login#t",
            "https://claude.ai/magic-link#t\"onload",
            "https://claude.ai/magic-link#t'x",
            "https://claude.ai/magic-link#a<b",
            "https://claude.ai/magic-link#a\\b",
            "https://claude.ai/magic-link# t",
            "https://claude.ai/magic-link#\u{4e2d}",
            long.as_str(),
            "javascript:alert(1)",
            "",
        ] {
            assert!(!ok(bad), "{bad}");
        }
    }

    #[test]
    fn sender_domain_by_label_suffix() {
        let d = |x: &str| MAIL_RULES[0].domain_ok(x);
        assert!(d("anthropic.com") && d("mail.anthropic.com") && d("MAIL.Anthropic.COM"));
        assert!(!d("evilanthropic.com") && !d("anthropic.com.evil.com") && !d(""));
    }

    #[test]
    fn subject() {
        let s = |v: &str| MAIL_RULES[0].subject_ok(&header::mail_text_str(v));
        assert!(s("Your secure link to Claude is here")); // 真邮件（2026-11-16 核对）；转发会加后缀
        assert!(s("Your secure link to Claude is here | 2026-11-16 10:36:30"));
        assert!(s("轉寄: Your secure link to Claude is here"));
        assert!(s("Secure link to log in to Claude.ai | 2026-11-15")); // 帮助中心写的说法
        assert!(s("Your secure link to Claude.ai is here"));
        assert!(s("=?utf-8?q?Secure_link_to_log_in_to_Claude=2Eai?="));
        // 应用名不是 Claude 的不认：Verification Portal 是真从 mail.anthropic.com 发出、dkim 也过的（见过），只靠签名挡不住
        assert!(!s("Your secure link to Verification Portal is here"));
        assert!(
            !s("Your receipt from Anthropic")
                && !s("Secure link to log in to Claude-ai")
                && !s("Your secure link to Claudexyz is here")
        );
    }

    /// config.json 的 mail 一节怎么读：默认的那一项、各账号的覆盖、信任的中转、引用到的地址
    #[test]
    fn settings_like_python() {
        let cases = vectors("cfg");
        assert!(cases.len() > 25);
        // 照 repr 比：键的先后、True 和 1 的差别都算
        let same =
            |got: Option<Dict>, want: V, what: &str| assert_eq!(got.map_or(V::None, V::Dict).py_repr(), want.py_repr(), "{what}");
        for c in cases {
            let cfg = c.dict("c").unwrap();
            let what = V::Dict(cfg.clone()).py_repr();
            for row in c.list("keys").unwrap().items() {
                let row = row.as_list().unwrap();
                let key = row.at(0);
                same(mail_cfg(&cfg, key.as_str()), row.at(1), &format!("mail_cfg({what}, {key:?})"));
                if let Some(k) = key.as_str() {
                    let v = mail_view(&cfg, k, None);
                    assert!(v.pop("team_forwarders").is_some_and(|x| x.is_none()), "不是 Team 给的账号：team_forwarders 是 null");
                    same(Some(v), row.at(2), &format!("mail_view({what}, {k:?})"));
                }
            }
            let mut addrs = mail_all_addrs(&cfg);
            addrs.sort();
            let want: Vec<String> = c.list("addrs").unwrap().strs().iter().map(|s| s.to_string()).collect();
            assert_eq!(addrs, want, "{what}");
            let mail = cfg.get("mail").unwrap_or(V::None);
            same(mail_one(&mail, true, "system"), c.at("one"), &what);
            same(mail_one(&mail, false, "direct"), c.at("one_off"), &what);
        }
    }

    /// 各工具的 mail_rules 里写的名字 MAIL_RULES 里都有
    /// （写错了的名字 `mail_rules()` 是跳过、不出错，所以更要守着），Codex 没有规则
    #[test]
    fn every_rule_is_known() {
        for tool in crate::core::tool::tools() {
            for name in tool.mail_rules {
                assert!(mail_rule(name).is_some(), "{name}");
            }
        }
        assert!(crate::core::tool::codex().mail_rules.is_empty());
        assert_eq!(crate::core::tool::claude().mail_rules, ["claude-login"]);
    }

    /// CCA_MAIL_POLL / CCA_MAIL_RETRY 写成各种样子时取到多少（默认值 3）
    #[test]
    fn knob_values_like_python() {
        let cases = vectors("knob");
        assert!(cases.len() > 40);
        for c in cases {
            let raw = c.at("in").py_str();
            assert_eq!(V::Float(knob_value(Some(&raw), 3.0)).py_repr(), c.at("out").py_str(), "{raw:?}");
        }
        assert_eq!(knob_value(None, 3.0), 3.0);
    }

    #[test]
    fn knob() {
        // 没设这个环境变量：默认值
        assert_eq!(mail_knob("CCA_MAIL_NO_SUCH_KNOB", 3.0), 3.0);
        assert_eq!(mail_why("to"), Some("收件人不是这个账号的邮箱"));
        assert!(mail_why("opened").is_none());
        assert!(MAIL_ORDER.iter().all(|w| mail_why(w).is_some()));
        let e = mail_retry("net", "x");
        assert!(e.is("MailRetry") && mail_error(&e) == Some(("net", "x")) && mail_error(&Fail::Value("x".into())).is_none());
    }

    /// Team 的「信任的转发中转」：一项项怎么查、整份怎么规整、什么算宽了、设备上读配置里的那一份
    #[test]
    fn team_forwarder_lists() {
        assert_eq!(team_fwd_check(" Relay.Example.com ").as_deref(), Ok("relay.example.com"));
        for bad in [
            "gmail.com",
            "mail.gmail.com",
            "QQ.com",
            "com.cn",
            "co.uk",
            "relay.example.com:25",
            "a@relay.example.com",
            "localhost",
            "",
        ] {
            assert!(team_fwd_check(bad).is_err(), "{bad}");
        }
        assert!(team_fwd_check("relay.example.co.uk").is_ok(), "公共后缀下面的域名可以");
        let (l63, l60) = ("a".repeat(63), "a".repeat(60));
        assert!(team_fwd_check(&format!("{l63}.example.com")).is_ok());
        assert!(team_fwd_check(&format!("a{l63}.example.com")).is_err(), "一段最多 63 个字符");
        assert!(team_fwd_check(&format!("{l60}.{l60}.{l60}.{l60}.example.com")).is_err(), "最长 253 个字符");
        let v = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            fwd_norm(v(&["relay.example.com", "b.example.net", "example.com", "b.example.net"])),
            v(&["b.example.net", "example.com"]),
            "去重、去掉被盖住的、排好序"
        );
        assert!(fwd_widens(&[], &v(&["relay.example.com"])));
        assert!(!fwd_widens(&v(&["example.com"]), &v(&["relay.example.com"])), "原来的盖得住：不算宽");
        assert!(!fwd_widens(&v(&["a.example.com", "b.example.com"]), &v(&["a.example.com"])));
        assert!(!fwd_widens(&v(&["a.example.com"]), &[]));
        assert!(fwd_widens(&v(&["relay.example.com"]), &v(&["example.com"])));
        let doc = |s: &str| crate::py::json::loads(s).unwrap().as_dict().unwrap();
        assert_eq!(
            doc_forwarders(&doc(r#"{"mail_forwarders": ["Relay.Example.com", "gmail.com", 1, "relay.example.com", "co.uk"]}"#)),
            v(&["relay.example.com"]),
            "设备上读的时候不合格的跳过"
        );
        assert!(doc_forwarders(&doc(r#"{"mail_forwarders": "relay.example.com"}"#)).is_empty(), "不是列表：一个都不信任");
        assert!(doc_forwarders(&doc("{}")).is_empty());
    }
}
