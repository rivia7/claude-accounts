//! 邮箱助手（cca mail，只在 macOS 上）：Claude 账号的邮箱转发到一个 Gmail。账号登录时 Anthropic 发来的登录邮件，cca 经 IMAP
//! （imap.gmail.com:993，Gmail 的应用专用密码）只读地找到它，把里面的登录链接交给这个账号专属的 Chrome 打开。
//! 对应 PY_SRC 的 `mail_knob` 到 `cmd_mail_link_check`：mail-setup / status / ready / get-user / forget / trust-forwarder / test /
//! open / watch / link-check、gui-mail-list。
//!
//! 分层（各在 mail/ 下的一份文件里）：路（route：照 macOS 的系统代理，不读环境变量——cca 起的进程里 https_proxy 是账号的出口）→
//! 隧道和 TLS（route：照常校验证书）→ 只读的 IMAP（imap：EXAMINE、UID SEARCH、BODY.PEEK，没有通用的命令入口）→ 核对
//! （judge：只信 Gmail 自己写的验证结果和 Anthropic 签过名的邮件头）→ 取链接（`MailRule::link_ok`）→ 盯邮箱（hunt）→ 打开
//! （cca _mail-open，链接经 stdin）。各站点的认法只在 `MAIL_RULES` 里，`Tool::mail_rules` 指过去。
//!
//! 链接是一次性的登录凭证：不打印、不记日志、不存盘、不进环境变量，只交给这个账号的 Chrome。应用专用密码只从钥匙串读、
//! 只在进程里经 TLS 交给 Gmail 的 LOGIN，不进任何进程的命令行、环境变量、文件和出错信息。
//!
//! 原来靠 Python 标准库的 imaplib、email、html.parser、ssl；这里是照 CPython 3.14 的行为手写的（codec、message、header、
//! html、imap 各一份），对照数据在 tests/vectors/mail.json（tools/gen_mail_vectors.py 生成）。

pub mod codec;
pub mod header;
pub mod html;
mod html5;
pub mod judge;
pub mod message;

use super::secrets::ks_del;
use super::Cmd;
use crate::core::base::CCA_HOME;
use crate::core::ctx::cfg_save;
use crate::core::model::{acct_id, cfg_load};
use crate::core::tool::claude;
use crate::dict;
use crate::py::fs::load_json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::re::Re;
use crate::py::text;
use crate::py::uni;
use crate::py::url::urlsplit;
use crate::py::{Fail, R};
use crate::re;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

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
    let v = match getenv(name).filter(|s| !s.is_empty()) {
        Some(s) => text::to_float(&s).unwrap_or(default),
        None => default,
    };
    if v > 0.0 {
        v
    } else {
        default
    }
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
    let live: Vec<String> = match load_json(&claude().table)? {
        None => Vec::new(),
        Some(V::Dict(table)) => table.keys().iter().map(|k| acct_id(k, claude())).collect(),
        Some(_) => Vec::new(),
    };
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

    /// 对照数据的一节。环境变量 CCA_MAIL_VECTORS_EXTRA 指着另一份同样格式的文件时（本机多跑一批随机输入用，
    /// `gen_mail_vectors.py --fuzz`），它的这一节接在后面
    pub fn vectors(section: &str) -> Vec<Dict> {
        let pick = |doc: &V| -> Vec<Dict> {
            let cases = doc.as_dict().and_then(|d| d.dict("cases")).and_then(|c| c.list(section));
            cases.map(|l| l.items().iter().filter_map(|v| expand(v).as_dict()).collect()).unwrap_or_default()
        };
        let mut out = pick(&json::loads(include_str!("../../tests/vectors/mail.json")).unwrap());
        assert!(!out.is_empty(), "tests/vectors/mail.json 里没有 {section} 这一节");
        if let Some(extra) = std::env::var_os("CCA_MAIL_VECTORS_EXTRA") {
            let text = std::fs::read_to_string(&extra).unwrap();
            out.extend(pick(&json::loads(&text).unwrap()));
        }
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

    /// tests/mail_view.py 的 Rules
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
        assert!(!s("Your receipt from Anthropic") && !s("Secure link to log in to Claude-ai") && !s("Your secure link to Claudexyz is here"));
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
}
