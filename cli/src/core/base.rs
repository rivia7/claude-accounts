//! 各处都用的底子：目录在哪、校验用的正则、`notice`、只读模式、给 bash 的变量赋值、全局的锁。
//! 对应 PY_SRC 开头到 `sh_arrays` 那一段（常量的名字和原来一样，方便对着找）。

use crate::py::fs;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::re::{self, Re};
use crate::py::text::sh_quote;
use crate::py::R;
use std::cell::RefCell;
use std::sync::LazyLock;

/// `HOME = os.path.expanduser('~')`
pub static HOME: LazyLock<String> = LazyLock::new(|| path::expanduser("~"));
/// `~/.cca`（`CCA_HOME` 环境变量可以换掉）
pub static CCA_HOME: LazyLock<String> =
    LazyLock::new(|| path::abspath(&getenv("CCA_HOME").filter(|s| !s.is_empty()).unwrap_or_else(|| path::join(&HOME, ".cca"))));
pub static CFG: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "config.json"));
pub static MASTER_JSON: LazyLock<String> = LazyLock::new(|| path::join(&HOME, ".claude.json"));
/// cca 脚本自己（apiKeyHelper 等回调它）
pub static SELF_PATH: LazyLock<String> =
    LazyLock::new(|| getenv("CCA_SELF").filter(|s| !s.is_empty()).unwrap_or_else(|| path::join_all(&[&CCA_HOME, "bin", "cca"])));

pub const FIRST_PORT: i64 = 7901;
pub const SECRET_MSG: &str = "key 应该是一串可见的 ASCII 字符（至少 8 个，不能有空格或中文）";
pub const MODEL_MSG: &str = "模型名里不能有空格或引号";

/// 会盖掉账号自己的代理、接口地址、凭证，或者把请求转去别的服务商（云厂商、网关）的变量。
/// 共享的 settings.json 的 env 里不能有，第三方 API 账号的 env 里也不能设；bash 的 INHERITED_RE 跟着改
pub const CRED_ENV: &str = concat!(
    r"(HTTPS?|ALL|NO)_PROXY|ANTHROPIC_(BASE_URL|AUTH_TOKEN|API_KEY)",
    r"|CLAUDE_(CONFIG_DIR|SECURESTORAGE_CONFIG_DIR)|CLAUDE_CODE_(OAUTH_TOKEN|[A-Z_]*_FILE_DESCRIPTOR)",
    r"|CLAUDE_CODE_USE_(BEDROCK|VERTEX|FOUNDRY|GATEWAY|MANTLE|ANTHROPIC_AWS|ANTHROPIC_GOOGLE_CLOUD)",
    r"|CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST"
);

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        pub static $name: LazyLock<Re> = LazyLock::new(|| Re::new($pat));
    };
    ($name:ident, $pat:expr, $flags:expr) => {
        pub static $name: LazyLock<Re> = LazyLock::new(|| Re::with_flags($pat, $flags));
    };
}

lazy_re!(KEY_RE, r#"^[^\s"\\]{1,100}$"#);
lazy_re!(EMAIL_RE, r#"^[^@\s"\\()（）]+@[^@\s"\\()（）]+\.[^@\s"\\()（）]+$"#);
lazy_re!(PAIR_RE, r"^(.*?)[(（]([^()（）]*)[)）]$");
lazy_re!(ALIAS_BAD, r#"[\s"\\()（）@]"#);
lazy_re!(HINT_RE, r"^[\x21-\x7e]{0,8}…[\x21-\x7e]{4}$");
lazy_re!(MODEL_RE, r"^[^\s\x22\x27\\]{1,100}$");
lazy_re!(HOST_RE, r"^([A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?|[0-9A-Fa-f:]{2,39})$");
lazy_re!(SSH_USER_RE, r"^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$");
lazy_re!(CONTROLLER_RE, r"^(unix:/\S+|(https?://)?[^\s/]+:[0-9]{1,5}/?)$");
lazy_re!(ENV_NAME_RE, r"^[A-Za-z_][A-Za-z0-9_]{0,63}$");
pub static SETTINGS_BAD: LazyLock<Re> = LazyLock::new(|| Re::with_flags(&format!("^({CRED_ENV})$"), re::I));
pub static ENV_RESERVED: LazyLock<Re> = LazyLock::new(|| Re::with_flags(&format!(r"^({CRED_ENV}|CCA_\w*|BROWSER)$"), re::I));
lazy_re!(ENV_SECRETISH, r"(^|_)(API_?KEY|KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIALS?)$", re::I);

/// `SECRET_RE.match(s)`（`^[\x21-\x7e]{8,4096}$`）：8 到 4096 个可见的 ASCII 字符。和原来的正则一样，结尾多一个换行也算
pub fn secret_re_match(s: &str) -> bool {
    let body = s.strip_suffix('\n').unwrap_or(s);
    (8..=4096).contains(&body.len()) && body.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// 与 ip.net.coffee/claude 页面一致：这些地区直接判 0 分
pub const RESTRICTED: &[(&str, &str)] = &[
    ("CN", "中国大陆"),
    ("HK", "香港"),
    ("MO", "澳门"),
    ("RU", "俄罗斯"),
    ("KP", "朝鲜"),
    ("IR", "伊朗"),
    ("SY", "叙利亚"),
    ("CU", "古巴"),
    ("BY", "白俄罗斯"),
    ("VE", "委内瑞拉"),
];

/// 只读（cca _gui state / live，CCA_RO=1）：按 Team 的配置对账、分配端口都只在内存里做，不写 config.json 和账号表，
/// 也不往 stderr 说；本来会做的改动记在 NOTICES，给图形界面看
pub static RO: LazyLock<bool> = LazyLock::new(|| getenv("CCA_RO").as_deref() == Some("1"));

thread_local! {
    /// 这次运行里说过的 notice（每条是带着 `code` 的字典）
    pub static NOTICES: RefCell<Vec<Dict>> = const { RefCell::new(Vec::new()) };
    /// 只读时读不了的账号文件 [(工具名, 路径)]
    pub static RO_BAD: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// `notice(code, text, **kv)`：记下一条（kv 在前、code 在后），不是只读时把 text 说到 stderr
pub fn notice(code: &str, text: &str, kv: Dict) {
    kv.set("code", code);
    NOTICES.with(|n| n.borrow_mut().push(kv));
    if !*RO {
        crate::err!("{text}");
    }
}

/// `sh(K=v, …)`：给 bash `eval` 的赋值，一行一个，值用 `shlex.quote`
pub fn sh(kv: &[(&str, &str)]) {
    for (k, v) in kv {
        crate::out!("{}={}", k, sh_quote(v));
    }
}

/// `sh(K=v, …)`，值是 Python 的值：照 `str(v)` 写（None 就是 `None`，原来就这样）
pub fn sh_vals(kv: &[(&str, V)]) {
    for (k, v) in kv {
        crate::out!("{}={}", k, sh_quote(&v.py_str()));
    }
}

/// `sh_arrays(NAME=[…])`：bash 数组 `NAME=(项 …)`
pub fn sh_array(name: &str, items: &[String]) {
    crate::out!("{}=({})", name, items.iter().map(|x| sh_quote(x)).collect::<Vec<_>>().join(" "));
}

/// `with Lock():`：拿着 `~/.cca/sync.lock` 的排他锁，放手（离开作用域）时解开。等锁没有时限，不能重入
pub struct Lock {
    _file: std::fs::File,
}

impl Lock {
    pub fn acquire() -> R<Lock> {
        use std::os::fd::AsRawFd;
        fs::makedirs(&CCA_HOME)?;
        let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path::join(&CCA_HOME, "sync.lock"))?;
        loop {
            // SAFETY: file 开着，fd 有效
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Lock { _file: file });
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
    }
}

/// `str(v or '')`：没有、算假的都是空串，别的照 `str()`
pub fn str_or_empty(v: &Option<V>) -> String {
    match v {
        Some(x) if x.truthy() => x.py_str(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        assert!(KEY_RE.matches("主号(me@example.com)") && !KEY_RE.matches("a b") && !KEY_RE.matches(&"x".repeat(101)));
        assert!(EMAIL_RE.matches("me@example.com") && !EMAIL_RE.matches("me@example") && !EMAIL_RE.matches("a(b)@c.d"));
        let m = PAIR_RE.match_("主号（me@example.com）").unwrap();
        assert_eq!((m.g(1), m.g(2)), ("主号", "me@example.com"));
        assert!(ALIAS_BAD.found("a@b") && !ALIAS_BAD.found("工作"));
        assert!(
            SETTINGS_BAD.matches("https_proxy")
                && SETTINGS_BAD.matches("CLAUDE_CODE_USE_BEDROCK")
                && !SETTINGS_BAD.matches("API_TIMEOUT_MS")
        );
        assert!(ENV_RESERVED.matches("CCA_X") && ENV_RESERVED.matches("browser"));
        assert!(ENV_SECRETISH.found("MY_API_KEY") && ENV_SECRETISH.found("token") && !ENV_SECRETISH.found("KEYBOARD"));
        assert!(
            secret_re_match("sk-12345")
                && secret_re_match("sk-12345\n")
                && !secret_re_match("short")
                && !secret_re_match("has space1")
        );
        assert!(HOST_RE.matches("192.0.2.10") && HOST_RE.matches("::1") && !HOST_RE.matches("-oProxyCommand"));
    }

    #[test]
    fn credential_sources_are_reserved() {
        // tests/isolation_settings.py 的 test_claude_configuration_cannot_replace_credential_sources：换掉账号目录、登录凭证来源的
        // 变量，共享的设置里不能有（SETTINGS_BAD），第三方 API 账号的 env 里也不能设（ENV_RESERVED）。后两项是原来的 Python 给的
        for (name, reserved, settings_bad) in [
            ("CLAUDE_CONFIG_DIR", true, true),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", true, true),
            ("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", true, true),
            ("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR", true, true),
            ("CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR", true, true),
            ("CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR", true, true),
            ("claude_config_dir", true, true),
            ("CLAUDE_CODE_OAUTH_TOKEN", true, true),
            ("CLAUDE_CODE__FILE_DESCRIPTOR", true, true),
            ("https_proxy", true, true),
            // cca 自己的变量、BROWSER：只是账号的 env 里不能设
            ("CCA_TOOL", true, false),
            ("cca_x", true, false),
            ("CCA_", true, false),
            ("BROWSER", true, false),
            ("browser", true, false),
            ("EDITOR", false, false),
            ("CLAUDE_CODE_FILE_DESCRIPTOR", false, false),
            ("CLAUDE_CONFIG_DIR2", false, false),
            ("XCLAUDE_CONFIG_DIR", false, false),
            ("CCAX", false, false),
            ("API_TIMEOUT_MS", false, false),
            ("ANTHROPIC_MODEL", false, false),
        ] {
            assert_eq!(ENV_RESERVED.matches(name), reserved, "ENV_RESERVED：{name}");
            assert_eq!(SETTINGS_BAD.matches(name), settings_bad, "SETTINGS_BAD：{name}");
        }
    }
}
