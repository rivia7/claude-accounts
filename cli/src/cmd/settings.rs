//! 共享设置的检查、说明和修复（PY_SRC 的 `cmd_settings_check` 到 `cmd_codex_fallback_off`）：settings-check、launch-check、
//! settings-explain、settings-fix、codex-fallback-off。
//!
//! 这些检查决定一个账号能不能启动：共享的 settings.json / config.toml / .env、项目设置、启动参数里有会改接口地址、
//! 换凭证、改代理的设置就拦住。输出只有来源和配置键，不带值、解析器的原文和凭证。读不了、解析不了的一律算有问题，
//! 不能当成空配置放行；TOML 用照搬过来的解析器（`py::toml`，原来是 tomllib / 内嵌的 tomli），不退回粗略的扫描。

use super::{arg, arity, Cmd};
use crate::core::base::{Lock, HOME, SETTINGS_BAD};
use crate::core::tool::{agy, claude, codex, codex_profile_file, codex_profile_name, t, Kind, Tool};
use crate::py::fail::{errno_text, os_error_text};
use crate::py::fs::{self, save_json};
use crate::py::hash::{sha256_hex, Sha256Stream};
use crate::py::json;
use crate::py::obj::{str_list, Dict, List, V};
use crate::py::path;
use crate::py::re::{self, Re};
use crate::py::text;
use crate::py::time;
use crate::py::toml;
use crate::py::{exit, Fail, R};
use crate::{out, re};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

// 套得太深的配置在 Python 里是 RecursionError（调用栈超过 sys.getrecursionlimit()，默认 1000 层），原来各处都接着它、
// 当作读不了。Rust 没有这个上限（套得太深是把栈用完、直接崩掉），所以照同样的数自己数。Python 那边到上限时已经在
// 调用栈的第几层取决于从哪条路调过来：下面两个数是 `py settings-check` 那条路上的，别的路比它深几层、浅几层用
// `shifted` 说明（数的是直接跑 PY_SRC 时的层数；cca 经 PY_BOOT 起的还要再深一层）。

/// `tomllib.loads` 在调用栈的第几层：<module> → cmd_settings_check → settings_problems → codex_settings_problems →
/// codex_config_file_problems → read_toml_config → toml_loads → loads
const TOML_LOADS_FRAME: usize = 8;
/// 最外层的 `codex_config_problems` 在第几层；profiles 里每套一层 profiles 再深一层
const CODEX_CONFIG_FRAME: usize = 6;

thread_local! {
    /// 现在这条路在 Python 的调用栈上比 settings-check 那条路深几层（负的是浅几层）
    static FRAME_SHIFT: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// 作用域里的调用在 Python 那边比 settings-check 那条路深 by 层；离开作用域还原
struct Shifted(isize);

fn shifted(by: isize) -> Shifted {
    Shifted(FRAME_SHIFT.replace(by))
}

impl Drop for Shifted {
    fn drop(&mut self) {
        FRAME_SHIFT.set(self.0);
    }
}

/// settings-check 那条路上在第 base 层的调用，现在这条路上在第几层
fn frame(base: usize) -> usize {
    base.saturating_add_signed(FRAME_SHIFT.get())
}
/// JSON 最多套几层。CPython 3.11 及以前（macOS 自带的 /usr/bin/python3 是 3.9）JSON 每套一层算一层递归，一千层里扣掉
/// 调用栈上已有的十来层；3.12 起改成按 C 的栈算，能套两万多层。这里固定成 1000
const JSON_NESTING_MAX: usize = 1000;
/// JSON 里的整数最多几位（`sys.get_int_max_str_digits()` 的默认值）
const JSON_INT_DIGITS_MAX: usize = 4300;

/// `class ConfigReadError(ValueError)`：配置读不了、解析不了，带着一个代码。`raise ConfigReadError('x')` 写成
/// `Err(ConfigReadError("x").into())`（照守则是 `Fail::Other("ConfigReadError", [代码])`），`except ConfigReadError`
/// 写成 `e.is(ConfigReadError::CLASS)`
pub struct ConfigReadError(pub &'static str);

impl ConfigReadError {
    pub const CLASS: &'static str = "ConfigReadError";
}

impl From<ConfigReadError> for Fail {
    fn from(e: ConfigReadError) -> Fail {
        Fail::Other(ConfigReadError::CLASS, vec![e.0.to_string()])
    }
}

fn recursion_error() -> Fail {
    Fail::from(toml::TomlError::Recursion)
}

/// `except RecursionError`
fn is_recursion(e: &Fail) -> bool {
    e.is("RecursionError")
}

/// `dict.fromkeys(items)`：去掉重复的，先后不变
fn fromkeys(items: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    items.iter().filter(|x| seen.insert(x.as_str())).collect()
}

/// `rx.match(s)`，rx 带着 `re.I`。Python 不分大小写时把 İ（U+0130）和 ı（U+0131）也当成 i，`regex` 库不认这两个：
/// 先换成 i 再匹配（这里用到的正则里都只有 ASCII 的字母）
fn imatch(rx: &Re, s: &str) -> bool {
    if s.contains(['İ', 'ı']) {
        rx.matches(&s.replace(['İ', 'ı'], "i"))
    } else {
        rx.matches(s)
    }
}

/// `json.loads(text)`，连同 Python 那边的两样上限（见 `json_limits`）
fn json_loads(text: &str) -> R<V> {
    json_limits(text)?;
    Ok(json::loads(text)?)
}

/// Python 的 `json.loads` 有、兼容层的没有的两样上限，解析之前先看一遍：
/// - 方括号、花括号套得太深是 RecursionError（字符串里的不算；解析器在出错之前递归的层数不会比这里数的多）；
/// - 整数超过 4300 位是 ValueError（`int()` 的上限；带小数点、指数的是小数，不限）。
///
/// 原来各处把这两种和格式不对一样当作读不了，所以和别的语法错误谁先谁后不要紧
fn json_limits(text: &str) -> R<()> {
    let b = text.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'"' => {
                // 字符串：走到收尾的引号（转义的那个字符跳过去）
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'[' | b'{' => {
                depth += 1;
                if depth > JSON_NESTING_MAX {
                    return Err(recursion_error());
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            b'0'..=b'9' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let digit_at = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
                let exponent_at = |j: usize| matches!(b.get(j), Some(b'e' | b'E'));
                // 这一串是小数点、指数后面的，或者后面跟着小数部分、指数：是小数的一部分
                let fraction = start >= 1 && (b[start - 1] == b'.' || exponent_at(start - 1))
                    || start >= 2 && matches!(b[start - 1], b'+' | b'-') && exponent_at(start - 2)
                    || b.get(i) == Some(&b'.') && digit_at(i + 1)
                    || exponent_at(i) && (digit_at(i + 1) || matches!(b.get(i + 1), Some(b'+' | b'-')) && digit_at(i + 2));
                if i - start > JSON_INT_DIGITS_MAX && !fraction {
                    return Err(Fail::Value(format!(
                        "Exceeds the limit ({JSON_INT_DIGITS_MAX} digits) for integer string conversion: value has {} digits; \
                         use sys.set_int_max_str_digits() to increase the limit",
                        i - start
                    )));
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// `with open(path, encoding='utf-8') as f: json.load(f)`
fn json_load(path: &str) -> R<V> {
    json_loads(&fs::read_text(path)?)
}

/// `os.path.expanduser(p)`，`~用户名` 也认：启动参数是用户写的（兼容层的只认 `~` 和 `~/…`）
fn expanduser(p: &str) -> String {
    if !p.starts_with('~') {
        return p.to_string();
    }
    let i = p.find('/').unwrap_or(p.len());
    if i == 1 {
        return path::expanduser(p);
    }
    // 用户数据库里没有这个用户：原样
    let Some(userhome) = home_of(&p[1..i]) else {
        return p.to_string();
    };
    let out = format!("{}{}", userhome.trim_end_matches('/'), &p[i..]);
    if out.is_empty() {
        "/".into()
    } else {
        out
    }
}

/// `pwd.getpwnam(name).pw_dir`
fn home_of(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: name 以 0 结尾；getpwnam 返回的指针指向 C 库自己的静态缓冲，马上拷出来
    unsafe {
        let pw = libc::getpwnam(name.as_ptr());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

impl Tool {
    /// `T.settings_problems()`：共享的设置里会让各账号换到同一个接口、凭证、出口上的地方
    pub fn settings_problems(&self) -> Vec<String> {
        match self.kind {
            Kind::Claude => claude_settings_problems(),
            Kind::Codex => codex_settings_problems(),
            Kind::Agy => agy_settings_problems(),
        }
    }
}

/// 校验共享配置；输出仅含来源和配置键，不输出值、解析器原文或凭证。
pub fn cmd_settings_check(args: &[String]) -> R<()> {
    arity("cmd_settings_check", args, 0, 0)?;
    settings_check_result(&t().settings_problems())
}

pub fn settings_check_result(bad: &[String]) -> R<()> {
    if !bad.is_empty() {
        out!("{}", fromkeys(bad).iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
        return exit(1);
    }
    Ok(())
}

/// 不存在的可选配置略过（None）；坏链接、无权限、目录和非 UTF-8 都不能当成空配置。
pub fn config_text(path: &str) -> R<Option<String>> {
    match fs::read_text(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.is_not_found() && !path::lexists(path) => Ok(None),
        Err(e) if e.is_os_or_value() => Err(ConfigReadError("unreadable_config").into()),
        Err(e) => Err(e),
    }
}

pub fn claude_config_problems(data: &V, source: &str) -> Vec<String> {
    let Some(data) = data.as_dict() else {
        return vec![format!("{source}:invalid_config")];
    };
    let env = match data.get("env") {
        None => Dict::new(),
        Some(V::Dict(env)) => env,
        Some(_) => return vec![format!("{source}:env:invalid_config")],
    };
    let mut bad: Vec<String> = env.keys().iter().filter(|k| imatch(&SETTINGS_BAD, k)).map(|k| format!("{source}:{k}")).collect();
    if data.truthy("apiKeyHelper") {
        bad.push(format!("{source}:apiKeyHelper"));
    }
    bad
}

/// source 没给（或是空的）时来源写成路径
pub fn claude_config_file_problems(path: &str, source: Option<&str>) -> Vec<String> {
    let source = source.filter(|s| !s.is_empty()).unwrap_or(path);
    let found = config_text(path).and_then(|content| match content {
        None => Ok(Vec::new()),
        Some(content) => Ok(claude_config_problems(&json_loads(&content)?, source)),
    });
    match found {
        Ok(bad) => bad,
        Err(e) if e.is(ConfigReadError::CLASS) || e.is_value() || is_recursion(&e) => {
            vec![format!("{source}:invalid_or_unreadable_config")]
        }
        Err(e) => panic!("{e}"),
    }
}

pub fn claude_settings_problems() -> Vec<String> {
    let master = &claude().master;
    ["settings.json", "settings.local.json"]
        .into_iter()
        .flat_map(|name| claude_config_file_problems(&path::join(master, name), Some(name)))
        .collect()
}

// 官方账号不能被配置改到其它地址、凭证库、工作空间或系统代理。
const CODEX_BAD_KEYS: &[&str] = &[
    "openai_base_url",
    "chatgpt_base_url",
    "experimental_realtime_ws_base_url",
    "experimental_realtime_webrtc_call_base_url",
    "forced_chatgpt_workspace_id",
];
const CODEX_MUST_BE: &[(&str, &str)] = &[("model_provider", "openai"), ("cli_auth_credentials_store", "file")];
const CODEX_MUST_NOT: &[(&str, &str)] = &[("forced_login_method", "api")];
static CODEX_ENV_BAD: LazyLock<Re> = LazyLock::new(|| Re::with_flags(r"^((HTTPS?|ALL|NO)_PROXY|OPENAI_\w+|CODEX_\w+)$", re::I));

/// `tomllib.loads(content)`。解析不了是 ValueError（`e.is_value()`），行内数组、行内表套得太深是 RecursionError
pub fn toml_loads(content: &str) -> R<Dict> {
    Ok(toml::loads_at(content, frame(TOML_LOADS_FRAME))?)
}

/// 没有这个文件是空的表；读不了、解析不了是 ConfigReadError
pub fn read_toml_config(path: &str) -> R<Dict> {
    let Some(content) = config_text(path)? else {
        return Ok(Dict::new());
    };
    match toml_loads(&content) {
        Err(e) if e.is_value() || is_recursion(&e) => Err(ConfigReadError("invalid_toml").into()),
        parsed => parsed,
    }
}

/// 只按解析后的键层级检查，不把带点号的字面键误认成嵌套表。profiles 套得太深是 RecursionError
pub fn codex_config_problems(data: &V, source: &str, overrides: bool, allow_base_fallback: bool) -> R<Vec<String>> {
    codex_config_problems_at(data, source, overrides, allow_base_fallback, frame(CODEX_CONFIG_FRAME))
}

/// frame：这一次调用在 Python 调用栈的第几层
fn codex_config_problems_at(data: &V, source: &str, overrides: bool, allow_base_fallback: bool, frame: usize) -> R<Vec<String>> {
    if frame > toml::RECURSION_LIMIT {
        return Err(recursion_error());
    }
    let (mut bad, profiles) = codex_config_level(data, source, overrides, allow_base_fallback);
    for (name, profile) in profiles {
        bad.extend(codex_config_problems_at(&profile, &format!("{source}:profiles.{name}"), overrides, false, frame + 1)?);
    }
    Ok(bad)
}

/// 这一层自己的问题，和还要往下查的 profiles（原来在同一个函数里，profiles 是最后查的；拆开是为了让一层套一层的那个
/// 函数占的栈小）
#[inline(never)]
fn codex_config_level(
    data: &V,
    source: &str,
    overrides: bool,
    allow_base_fallback: bool,
) -> (Vec<String>, Vec<(std::rc::Rc<str>, V)>) {
    let Some(data) = data.as_dict() else {
        return (vec![format!("{source}:invalid_config")], Vec::new());
    };
    let mut bad: Vec<String> = CODEX_BAD_KEYS.iter().filter(|key| data.has(key)).map(|key| format!("{source}:{key}")).collect();
    if let Some(profile) = data.get("profile") {
        if !profile.as_str().is_some_and(codex_profile_name) {
            bad.push(format!("{source}:profile:invalid_profile"));
        }
    }
    for (key, want) in CODEX_MUST_BE {
        if data.get(key).is_some_and(|v| overrides || !v.py_eq(&V::from(*want))) {
            bad.push(format!("{source}:{key}"));
        }
    }
    for (key, value) in CODEX_MUST_NOT {
        if data.get(key).is_some_and(|v| overrides || v.py_eq(&V::from(*value))) {
            bad.push(format!("{source}:{key}"));
        }
    }
    match data.get("features") {
        None => {}
        Some(V::Dict(features)) => {
            for key in ["respect_system_proxy", "system_proxy_fallback"] {
                let flagged = match features.get(key) {
                    None => false,
                    Some(V::Bool(on)) => on && (overrides || key == "respect_system_proxy" || !allow_base_fallback),
                    Some(_) => true,
                };
                if flagged {
                    bad.push(format!("{source}:features.{key}"));
                }
            }
        }
        Some(_) => bad.push(format!("{source}:features:invalid_config")),
    }
    match data.get("model_providers") {
        None => {}
        // 不允许替换内置官方 provider 的地址或认证配置。
        Some(V::Dict(providers)) => {
            if providers.has("openai") {
                bad.push(format!("{source}:model_providers.openai"));
            }
        }
        Some(_) => bad.push(format!("{source}:model_providers:invalid_config")),
    }
    match data.get("profiles") {
        None => (bad, Vec::new()),
        Some(V::Dict(profiles)) => (bad, profiles.items()),
        Some(_) => {
            bad.push(format!("{source}:profiles:invalid_config"));
            (bad, Vec::new())
        }
    }
}

/// source 没给（或是空的）时来源写成路径
pub fn codex_config_file_problems(path: &str, source: Option<&str>, allow_base_fallback: bool) -> Vec<String> {
    codex_file_problems(source.filter(|s| !s.is_empty()).unwrap_or(path), read_toml_config(path), allow_base_fallback)
}

/// `codex_config_problems`，查完把整份配置拆掉（见 `toml::dispose`：键写了上万段的配置不能让 Rust 递归着释放）
fn codex_config_checked(data: Dict, source: &str, overrides: bool, allow_base_fallback: bool) -> R<Vec<String>> {
    let found = codex_config_problems(&V::Dict(data.clone()), source, overrides, allow_base_fallback);
    toml::dispose(data);
    found
}

/// data 是 `read_toml_config` 的结果
fn codex_file_problems(source: &str, data: R<Dict>, allow_base_fallback: bool) -> Vec<String> {
    match data.and_then(|data| codex_config_checked(data, source, false, allow_base_fallback)) {
        Ok(bad) => bad,
        Err(e) if e.is(ConfigReadError::CLASS) || is_recursion(&e) => vec![format!("{source}:invalid_or_unreadable_config")],
        Err(e) => panic!("{e}"),
    }
}

/// `sorted(name for name in os.listdir(CODEX.master) if codex_profile_file(name))`，每项是（显示的名字, 文件）。
/// 名字不是 UTF-8 的 profile（Linux 上才可能有）也要查，不能跳过：文件照原样的名字读，先后照 Python 的排法
/// （那几个字节在 Python 里是 U+DC80–U+DCFF），只有显示的名字里它们写成 U+FFFD
fn codex_profile_files(names: Vec<OsString>) -> Vec<(String, OsString)> {
    use std::os::unix::ffi::OsStrExt;
    let key = |name: &OsString| -> Vec<u32> {
        let mut key = Vec::new();
        for chunk in name.as_bytes().utf8_chunks() {
            key.extend(chunk.valid().chars().map(|c| c as u32));
            key.extend(chunk.invalid().iter().map(|b| 0xdc00 + *b as u32));
        }
        key
    };
    let mut found: Vec<(Vec<u32>, OsString)> = names
        .into_iter()
        .filter(|name| match name.to_str() {
            Some(name) => codex_profile_file(name),
            None => !name.as_bytes().starts_with(b".") && name.as_bytes().ends_with(b".config.toml"),
        })
        .map(|name| (key(&name), name))
        .collect();
    found.sort();
    found.into_iter().map(|(_, name)| (name.to_string_lossy().into_owned(), name)).collect()
}

pub fn codex_settings_problems() -> Vec<String> {
    let master = &codex().master;
    let mut bad: Vec<String> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = vec![("config.toml".into(), PathBuf::from(path::join(master, "config.toml")))];
    match std::fs::read_dir(master).and_then(|entries| entries.map(|e| e.map(|e| e.file_name())).collect::<io::Result<Vec<_>>>()) {
        Ok(names) => {
            files.extend(codex_profile_files(names).into_iter().map(|(shown, name)| (shown, PathBuf::from(master).join(name))))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound && !path::lexists(master) => {}
        Err(_) => bad.push("~/.codex:unreadable_config".into()),
    }
    for (name, file) in &files {
        let allow_base_fallback = name == "config.toml";
        bad.extend(match file.to_str() {
            Some(file) => codex_config_file_problems(file, Some(name), allow_base_fallback),
            None => codex_file_problems(name, read_toml_file(file), allow_base_fallback),
        });
    }
    match config_text(&path::join(master, ".env")) {
        Ok(content) => {
            for line in text::splitlines(content.as_deref().unwrap_or("")) {
                if let Some(m) = re!(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=").match_(line) {
                    let name = m.g(1);
                    if CODEX_ENV_BAD.matches(name) && name != "CODEX_CA_CERTIFICATE" {
                        bad.push(format!(".env:{name}"));
                    }
                }
            }
        }
        Err(e) if e.is(ConfigReadError::CLASS) => bad.push(".env:invalid_or_unreadable_config".into()),
        Err(e) => panic!("{e}"),
    }
    bad
}

/// `read_toml_config`，给名字不是 UTF-8 的文件用（它既然列得出来就是存在的）
fn read_toml_file(file: &std::path::Path) -> R<Dict> {
    let unreadable = || Fail::from(ConfigReadError("unreadable_config"));
    let content = String::from_utf8(std::fs::read(file).map_err(|_| unreadable())?).map_err(|_| unreadable())?;
    // 和 open() 读文字一样，换行统一成 \n
    match toml_loads(&content.replace("\r\n", "\n").replace('\r', "\n")) {
        Err(e) if e.is_value() || is_recursion(&e) => Err(ConfigReadError("invalid_toml").into()),
        parsed => parsed,
    }
}

/// 企业版许可的 Google Cloud 项目：共享的设置里写了，所有账号都记到这个项目上
const AGY_SETTINGS_BAD: &[&str] = &["gcp", "gcp.project", "gcp.location"];

pub fn agy_settings_keys(j: &V, source: &str) -> Vec<String> {
    match j.as_dict() {
        None => vec![format!("{source}:unreadable")],
        Some(j) => AGY_SETTINGS_BAD.iter().filter(|k| j.has(k)).map(|k| format!("{source}:{k}")).collect(),
    }
}

/// 共享的 agy 设置（~/.gemini/antigravity-cli/settings.json）里会把各账号换到同一个 Google Cloud 项目上的设置
pub fn agy_settings_problems() -> Vec<String> {
    match json_load(&path::join_all(&[&agy().master, "antigravity-cli", "settings.json"])) {
        Ok(j) => agy_settings_keys(&j, "antigravity-cli/settings.json"),
        Err(e) if e.is_not_found() => Vec::new(),
        Err(e) if e.is_os_or_value() || is_recursion(&e) => vec!["antigravity-cli/settings.json:unreadable".into()],
        Err(e) => panic!("{e}"),
    }
}

/// agy --settings：一段 JSON 或者文件路径（只给这次会话的设置），查法和共享的设置一样
pub fn agy_session_settings_problems(value: &str, cwd: &str) -> Vec<String> {
    let j = if text::lstrip(value).starts_with('{') {
        json_loads(value)
    } else {
        json_load(&path::abspath(&path::join(cwd, &expanduser(value))))
    };
    match j {
        Ok(j) => agy_settings_keys(&j, "--settings"),
        Err(e) if e.is_os_or_value() || is_recursion(&e) => vec!["--settings:unreadable_config".into()],
        Err(e) => panic!("{e}"),
    }
}

/// 只检查当前目录及祖先；不扫描项目的无关子目录或相邻项目。
pub fn project_settings_problems(cwd: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let mut current = path::realpath(cwd);
    loop {
        match t().kind {
            Kind::Claude => {
                for name in ["settings.json", "settings.local.json"] {
                    let file = path::join_all(&[&current, ".claude", name]);
                    if file != path::realpath(&path::join(&claude().master, name)) {
                        bad.extend(claude_config_file_problems(&file, None));
                    }
                }
            }
            // agy 项目里的 .agents/ 只有规则、技能、MCP 这些，没有会换账号、出口的设置
            Kind::Agy => return bad,
            Kind::Codex => {
                let file = path::join_all(&[&current, ".codex", "config.toml"]);
                if file != path::realpath(&path::join(&codex().master, "config.toml")) {
                    bad.extend(codex_config_file_problems(&file, None, false));
                }
            }
        }
        let parent = path::dirname(&current);
        if parent == current {
            return bad;
        }
        current = parent;
    }
}

// 必填参数值可能恰好是 -- 或 --settings，必须先消费，才能认出真正的选项边界。
const CLAUDE_VALUES: &[&str] = &[
    "--agent",
    "--agents",
    "--append-system-prompt",
    "--autocompact",
    "--debug-file",
    "--effort",
    "--environment",
    "--fallback-model",
    "--input-format",
    "--json-schema",
    "--max-budget-usd",
    "--model",
    "-n",
    "--name",
    "--output-format",
    "--permission-mode",
    "--permission-prompts",
    "--plugin-dir",
    "--plugin-url",
    "--remote-control-session-name-prefix",
    "--session-id",
    "--setting-sources",
    "--settings",
    "--system-prompt",
    "--system-prompt-snapshot",
    "--system-prompt-file",
    "--append-system-prompt-file",
    "--add-dir",
    "--allowedTools",
    "--allowed-tools",
    "--betas",
    "--disallowedTools",
    "--disallowed-tools",
    "--file",
    "--mcp-config",
    "--tools",
];
const AGY_VALUES: &[&str] = &[
    "--add-dir",
    "--agent",
    "--conversation",
    "--effort",
    "--input-format",
    "--json-schema",
    "--log-file",
    "--mode",
    "--model",
    "--output-format",
    "--print-timeout",
    "--project",
    "--settings",
    "-p",
    "--print",
    "--prompt",
    "-i",
    "--prompt-interactive",
    "--gemini_dir",
    "--gemini-dir",
];
const CODEX_VALUES: &[&str] = &[
    "-c",
    "--config",
    "-p",
    "--profile",
    "-C",
    "--cd",
    "--enable",
    "--disable",
    "-m",
    "--model",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-i",
    "--image",
    "--local-provider",
    "--remote-auth-token-env",
    "--remote",
    "--output-schema",
    "-o",
    "--output-last-message",
];

/// 启动参数、共享的设置、这个目录和上层目录的项目设置里的问题
pub fn launch_settings_problems(args: &[String]) -> R<Vec<String>> {
    let mut bad: Vec<String> = Vec::new();
    let original_cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    let mut cwd = original_cwd.clone();
    let mut settings: Vec<&str> = Vec::new();
    let kind = t().kind;
    let takes_value = match kind {
        Kind::Codex => CODEX_VALUES,
        Kind::Agy => AGY_VALUES,
        Kind::Claude => CLAUDE_VALUES,
    };
    let (mut i, mut positional_seen) = (0, false);
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--" {
            break;
        }
        let mut option = arg.split('=').next().unwrap_or(arg);
        let mut value: Option<&str> = None;
        let short = ["-c", "-p", "-C"].into_iter().find(|o| arg.starts_with(o));
        if let (Kind::Codex, true, Some(short)) = (kind, arg.len() > 2, short) {
            option = short;
            let rest = &arg[2..];
            value = Some(rest.strip_prefix('=').unwrap_or(rest));
        } else if let Some((_, v)) = arg.split_once('=') {
            value = Some(v);
        }
        if takes_value.contains(&option) && value.is_none() {
            if i == args.len() {
                bad.push(format!("{option}:missing_value"));
                continue;
            }
            value = Some(args[i].as_str());
            i += 1;
        }
        match kind {
            Kind::Claude => {
                if option == "--settings" {
                    settings.extend(value);
                } else if option == "--desktop" {
                    bad.push("--desktop:use_cca_app".into());
                }
                continue;
            }
            Kind::Agy => {
                if option == "--gemini_dir" || option == "--gemini-dir" {
                    bad.push(format!("{option}:account_override")); // 换掉数据目录就换掉了登录和设备标识
                } else if let ("--settings", Some(value)) = (option, value) {
                    bad.extend(agy_session_settings_problems(value, &cwd));
                }
                continue;
            }
            Kind::Codex => {}
        }
        if !arg.starts_with('-') {
            if !positional_seen && arg == "app" {
                bad.push("app:use_cca_codex_app".into());
            }
            positional_seen = true;
        }
        match (option, value) {
            ("--remote" | "--remote-auth-token-env" | "--oss" | "--local-provider", _) => {
                bad.push(format!("{option}:account_override"))
            }
            // root/exec 各自的 --cd 可同时出现；后者覆盖前者，均相对初始目录解析。
            ("-C" | "--cd", Some(value)) => cwd = path::abspath(&path::join(&original_cwd, &expanduser(value))),
            ("-p" | "--profile", Some(value)) => {
                if !codex_profile_name(value) {
                    bad.push(format!("{option}:invalid_profile"));
                }
            }
            ("--enable", Some(value @ ("system_proxy_fallback" | "respect_system_proxy"))) => bad.push(format!("{option}:{value}")),
            ("-c" | "--config", Some(value)) => {
                let Some((key, raw)) = value.split_once('=') else {
                    bad.push(format!("{option}:invalid_config"));
                    continue;
                };
                // Codex 接受未引号的字符串值；只对值应用同样的回退，键仍交完整 TOML 解析器。
                // （这里是 launch_settings_problems 直接调的 toml_loads、codex_config_problems：比 settings-check 那条路浅 3 层、2 层）
                let parsed = {
                    let _frames = shifted(-3);
                    match toml_loads(value) {
                        Err(e) if e.is_value() => toml_loads(&format!("{key} = {}", json::dump_str(raw, true))),
                        parsed => parsed,
                    }
                };
                let _frames = shifted(-2);
                match parsed.and_then(|parsed| codex_config_checked(parsed, option, true, false)) {
                    Ok(found) => bad.extend(found),
                    Err(e) if e.is_value() || is_recursion(&e) => bad.push(format!("{option}:invalid_config")),
                    Err(e) => return Err(e),
                }
            }
            _ => {}
        }
    }
    for value in settings {
        let source = "--settings";
        if text::lstrip(value).starts_with('{') {
            match json_loads(value) {
                Ok(data) => bad.extend(claude_config_problems(&data, source)),
                Err(e) if e.is_value() || is_recursion(&e) => bad.push(format!("{source}:invalid_config")),
                Err(e) => return Err(e),
            }
        } else {
            let file = path::abspath(&path::join(&cwd, &expanduser(value)));
            if !path::exists(&file) {
                bad.push(format!("{source}:unreadable_config"));
            } else {
                bad.extend(claude_config_file_problems(&file, None));
            }
        }
    }
    {
        // 共享的设置在这里比 settings-check 多隔着 launch_settings_problems 这一层
        let _frames = shifted(1);
        bad.extend(t().settings_problems());
    }
    bad.extend(project_settings_problems(&cwd));
    Ok(bad)
}

pub fn cmd_launch_check(args: &[String]) -> R<()> {
    settings_check_result(&launch_settings_problems(args)?)
}

// ---- 共享设置的说明和修复（bash 的 settings_warn、cca settings fix；只给 Claude）----

/// 会盖掉账号自己设置的一类
struct SettingsKind {
    id: &'static str,
    /// 认哪些名字（other 没有：别的都认不出的归它）
    rx: Option<LazyLock<Re>>,
    /// 叫法
    name: &'static str,
    /// 带着它启动会怎样
    effect: &'static str,
}

/// 顺序就是说明里的先后，最后一个是 SETTINGS_OTHER：CRED_ENV 以后加了这里认不出的名字，算 other
static SETTINGS_KINDS: [SettingsKind; 5] = [
    SettingsKind {
        id: "api",
        rx: Some(LazyLock::new(|| Re::with_flags(r"^(ANTHROPIC_(BASE_URL|AUTH_TOKEN|API_KEY)|apiKeyHelper)$", re::I))),
        name: "第三方 API",
        effect: "每个账号都会改用这个第三方 API，官方账号的登录信息还可能被发到那个地址",
    },
    SettingsKind {
        id: "cloud",
        rx: Some(LazyLock::new(|| Re::with_flags(r"^CLAUDE_CODE_(USE_[A-Z_]+|PROVIDER_MANAGED_BY_HOST)$", re::I))),
        name: "云厂商或网关的开关",
        effect: "每个账号都会改走那个云厂商或网关",
    },
    SettingsKind {
        id: "proxy",
        rx: Some(LazyLock::new(|| Re::with_flags(r"^(HTTPS?|ALL|NO)_PROXY$", re::I))),
        name: "代理",
        effect: "账号会绕开 cca 给它分的出口",
    },
    SettingsKind {
        id: "login",
        rx: Some(LazyLock::new(|| {
            Re::with_flags(r"^CLAUDE_(CONFIG_DIR|SECURESTORAGE_CONFIG_DIR|CODE_OAUTH_TOKEN|CODE_[A-Z_]*_FILE_DESCRIPTOR)$", re::I)
        })),
        name: "账号目录或登录凭证",
        effect: "各账号会混用同一份登录",
    },
    SettingsKind { id: "other", rx: None, name: "会换掉账号设置的变量", effect: "账号会被换到别处" },
];
const SETTINGS_OTHER: usize = 4;
const SETTINGS_UNREADABLE: &[&str] = &["invalid_config", "invalid_or_unreadable_config", "unreadable_config"];
static PROJECT_SETTINGS: LazyLock<Re> = LazyLock::new(|| Re::new(r"/\.claude/settings(\.local)?\.json$"));
// 去掉第三方 API、云厂商或网关时跟着去掉的：官方账号用不了的模型（值不是 Claude 的模型名、别名）、发给它的请求头
static SETTINGS_MODEL_ENV: LazyLock<Re> =
    LazyLock::new(|| Re::new(r"^(ANTHROPIC_(MODEL|SMALL_FAST_MODEL|DEFAULT_[A-Z0-9_]+_MODEL)|CLAUDE_CODE_SUBAGENT_MODEL)$"));
static CLAUDE_MODEL_NAME: LazyLock<Re> =
    LazyLock::new(|| Re::with_flags(r"^(claude-[a-z0-9-]+|default|best|opus|sonnet|haiku|fable|opusplan)(\[1m\])?$", re::I));

pub fn home_tilde(p: &str) -> String {
    let home: &str = &HOME;
    if p == home || p.starts_with(&format!("{home}/")) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

/// 给人复制进终端的路径：主目录写成 ~
fn shell_path(p: &str) -> String {
    match p.strip_prefix(&format!("{}/", *HOME)) {
        Some(rest) => format!("~/{}", text::sh_quote(rest)),
        None => text::sh_quote(p),
    }
}

/// 这个名字归哪一类（SETTINGS_KINDS 里的位置）
fn settings_kind(name: &str) -> usize {
    SETTINGS_KINDS.iter().position(|kind| kind.rx.as_ref().is_some_and(|rx| imatch(rx, name))).unwrap_or(SETTINGS_OTHER)
}

/// [(类, [写法…])]，按 SETTINGS_KINDS 的先后；env 里的写成 env.名字
pub fn settings_groups(names: &[String]) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    for kind in 0..SETTINGS_KINDS.len() {
        let hit: Vec<String> = names
            .iter()
            .filter(|n| settings_kind(n) == kind)
            .map(|n| if n == "apiKeyHelper" { n.clone() } else { format!("env.{n}") })
            .collect();
        if !hit.is_empty() {
            out.push((kind, hit));
        }
    }
    out
}

fn settings_kinds_text(groups: &[(usize, Vec<String>)]) -> String {
    groups
        .iter()
        .map(|(kind, hit)| format!("{}（{}）", SETTINGS_KINDS[*kind].name, hit.join("、")))
        .collect::<Vec<_>>()
        .join("，还有")
}

/// settings_warn 的说明（Claude）：哪份文件、哪几项、带着它启动会怎样、怎么办，一句一行。只说路径和名字，不说值
pub fn settings_explain(bad: &[String]) -> Vec<String> {
    let cmd = t().cmd;
    let mut by_src: Vec<(&str, Vec<&str>)> = Vec::new();
    for label in fromkeys(bad) {
        let (src, name) = match label.strip_suffix(":env:invalid_config") {
            Some(src) => (src, "invalid_config"),
            None => match text::rpartition(label, ":") {
                ("", _, _) => (label.as_str(), ""),
                (src, _, name) => (src, name),
            },
        };
        match by_src.iter_mut().find(|(s, _)| *s == src) {
            Some((_, names)) => names.push(name),
            None => by_src.push((src, vec![name])),
        }
    }
    let (mut head, mut rest, mut shared) = (Vec::new(), Vec::new(), Vec::new());
    for (src, names) in &by_src {
        let src = *src;
        let broken = names.iter().any(|n| SETTINGS_UNREADABLE.contains(n));
        let groups = settings_groups(
            &names.iter().filter(|n| !n.is_empty() && !SETTINGS_UNREADABLE.contains(n)).map(|n| n.to_string()).collect::<Vec<_>>(),
        );
        if src == "settings.json" || src == "settings.local.json" {
            // 共享的 ~/.claude/settings.json 这些（claude_settings_problems）
            let file = home_tilde(&path::join(&claude().master, src));
            if broken {
                head.push(format!(
                    "{file} 读不了，或者不是合法的 JSON：cca 确认不了里面有没有会换掉账号的设置，所以没有继续。把它改对（或者挪走）再试。"
                ));
            } else if !groups.is_empty() {
                shared.push((file, groups));
            }
        } else if src == "--settings" || src.starts_with('/') {
            // 启动参数给的设置，这个目录和上层目录的项目设置
            let project = src != "--settings" && PROJECT_SETTINGS.found(src);
            let place = if project {
                format!("项目设置 {}", home_tilde(src))
            } else if src == "--settings" {
                "启动参数 --settings".to_string()
            } else {
                format!("启动参数 --settings 给的 {}", home_tilde(src))
            };
            if broken {
                rest.push(format!("{place} 读不了，或者不是合法的 JSON，所以 cca 没有继续。"));
            } else if !groups.is_empty() {
                let api = groups.iter().any(|(kind, _)| SETTINGS_KINDS[*kind].id == "api");
                rest.push(format!("{place} 里设置了{}。", settings_kinds_text(&groups)));
                rest.push(format!(
                    "{}带着它启动的话{}，所以 cca 没有继续。{}{}",
                    if project { "在这个目录下启动时 Claude Code 也会读它，" } else { "" },
                    groups.iter().map(|(kind, _)| SETTINGS_KINDS[*kind].effect).collect::<Vec<_>>().join("；"),
                    if project { "删掉这几项再启动" } else { "去掉它再启动" },
                    if api { format!("（第三方 API 用 {cmd} add api 加成单独的账号）。") } else { "。".to_string() }
                ));
            }
        } else {
            // 别的启动参数
            for name in names {
                rest.push(match *name {
                    "missing_value" => format!("启动参数 {src} 后面少了值。"),
                    "use_cca_app" => format!("启动参数里有 {src}：要打开这个账号的 Claude App，用 {cmd} app。"),
                    "" => format!("启动参数 {src} 不能经 cca 用。"),
                    name => format!("启动参数 {src} 不能经 cca 用（{name}）。"),
                });
            }
        }
    }
    if !shared.is_empty() {
        let kinds: Vec<&SettingsKind> = (0..SETTINGS_KINDS.len())
            .filter(|kind| shared.iter().any(|(_, groups)| groups.iter().any(|(k, _)| k == kind)))
            .map(|kind| &SETTINGS_KINDS[kind])
            .collect();
        head.extend(shared.iter().map(|(file, groups)| format!("{file} 里设置了{}。", settings_kinds_text(groups))));
        head.push(format!(
            "{}所有账号共用，带着它启动的话{}，所以 cca 没有继续。",
            if shared.len() == 1 { "这份设置" } else { "这几份设置" },
            kinds.iter().map(|kind| kind.effect).collect::<Vec<_>>().join("；")
        ));
        head.push(format!("运行 {cmd} settings fix 修复：先把整份文件备份在旁边，再只去掉这几项，别的设置不动。"));
        if kinds.iter().any(|kind| kind.id == "api") {
            head.push(format!("修复以后还要用这个第三方 API 的话，用 {cmd} add api 把它加成一个单独的账号。"));
        }
    }
    head.extend(rest);
    head
}

/// py settings-explain shared | launch [启动参数…]：settings_warn 给人看的说明（只给 Claude），一句一行；
/// shared 照 settings-check 查，launch 照 launch-check 查（同样的启动参数）
pub fn cmd_settings_explain(args: &[String]) -> R<()> {
    arity("cmd_settings_explain", args, 1, usize::MAX)?;
    let bad = if args[0] == "launch" { launch_settings_problems(&args[1..])? } else { t().settings_problems() };
    let lines = if bad.is_empty() { Vec::new() } else { settings_explain(&bad) };
    if !lines.is_empty() {
        out!("{}", lines.join("\n"));
    }
    Ok(())
}

fn claude_model_name(value: &V) -> bool {
    value.as_str().is_some_and(|s| imatch(&CLAUDE_MODEL_NAME, text::strip(s)))
}

/// 一份 Claude 设置里 cca settings fix 要去掉的 (env 里的名字, 顶层的名字)：会盖掉账号的（和 claude_config_problems 同一套）；
/// 去掉了第三方 API、云厂商或网关时，连同官方账号用不了的模型名（留着的话官方账号的请求会因为模型不对失败）和发给它的请求头
pub fn settings_fix_keys(data: &Dict) -> (Vec<String>, Vec<String>) {
    let env = data.dict("env").unwrap_or_default();
    let mut drop_env: Vec<String> = env.keys().iter().filter(|k| imatch(&SETTINGS_BAD, k)).map(|k| k.to_string()).collect();
    let mut drop_top: Vec<String> = if data.truthy("apiKeyHelper") { vec!["apiKeyHelper".into()] } else { Vec::new() };
    if !drop_top.is_empty() || drop_env.iter().any(|k| matches!(SETTINGS_KINDS[settings_kind(k)].id, "api" | "cloud")) {
        let more: Vec<String> = env
            .items()
            .iter()
            .filter(|(k, v)| {
                !drop_env.iter().any(|d| **d == **k)
                    && (&**k == "ANTHROPIC_CUSTOM_HEADERS" || (SETTINGS_MODEL_ENV.matches(k) && !claude_model_name(v)))
            })
            .map(|(k, _)| k.to_string())
            .collect();
        drop_env.extend(more);
        if data.get("model").is_some_and(|model| !claude_model_name(&model)) {
            drop_top.push("model".into());
        }
    }
    (drop_env, drop_top)
}

/// 要去掉的几项按类说：['env.A、env.B（第三方 API）', …]
fn settings_fix_parts(drop_env: &[String], drop_top: &[String]) -> Vec<String> {
    let bad: Vec<String> = drop_env
        .iter()
        .filter(|k| imatch(&SETTINGS_BAD, k))
        .chain(drop_top.iter().filter(|k| *k == "apiKeyHelper"))
        .cloned()
        .collect();
    let mut parts: Vec<String> =
        settings_groups(&bad).iter().map(|(kind, hit)| format!("{}（{}）", hit.join("、"), SETTINGS_KINDS[*kind].name)).collect();
    let model: Vec<String> = drop_env
        .iter()
        .filter(|k| SETTINGS_MODEL_ENV.matches(k))
        .map(|k| format!("env.{k}"))
        .chain(drop_top.iter().filter(|k| *k == "model").cloned())
        .collect();
    if !model.is_empty() {
        parts.push(format!("{}（官方账号用不了的模型名）", model.join("、")));
    }
    if drop_env.iter().any(|k| k == "ANTHROPIC_CUSTOM_HEADERS") {
        parts.push("env.ANTHROPIC_CUSTOM_HEADERS（发给第三方接口的请求头）".into());
    }
    parts
}

/// cca settings fix 要改的一份文件
pub struct SettingsFix {
    pub path: String,
    /// 原文
    pub raw: Vec<u8>,
    /// 改好的
    pub new: Dict,
    /// 去掉 env 里的
    pub drop_env: Vec<String>,
    /// 去掉顶层的
    pub drop_top: Vec<String>,
}

/// cca settings fix 要改的。只改共享的 ~/.claude/settings.json、settings.local.json（项目设置、启动参数给的不改）；
/// 有一份读不了、不是 JSON 的，返回 Err(说明)，整个都不改
pub fn settings_fix_plan() -> Result<Vec<SettingsFix>, String> {
    let mut plan = Vec::new();
    for name in ["settings.json", "settings.local.json"] {
        let file = path::join(&claude().master, name);
        let raw = match std::fs::read(&file) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if path::lexists(&file) {
                    return Err(format!("{} 是个指向不存在的文件的链接，cca 不改它", home_tilde(&file)));
                }
                continue;
            }
            Err(_) => return Err(format!("{} 读不了，cca 不改它", home_tilde(&file))),
        };
        let data = std::str::from_utf8(&raw).ok().and_then(|text| json_loads(text).ok()).and_then(|data| data.as_dict());
        let Some(data) = data.filter(|data| matches!(data.get("env"), None | Some(V::Dict(_)))) else {
            return Err(format!("{} 不是合法的 JSON 设置，cca 不改它：先把它改对（或者挪走）再试", home_tilde(&file)));
        };
        let (drop_env, drop_top) = settings_fix_keys(&data);
        if !drop_env.is_empty() || !drop_top.is_empty() {
            let new = Dict::from_pairs(data.items().into_iter().filter(|(k, _)| !drop_top.iter().any(|d| **d == **k)));
            if !drop_env.is_empty() {
                let env = Dict::from_pairs(
                    data.dict("env").unwrap_or_default().items().into_iter().filter(|(k, _)| !drop_env.iter().any(|d| **d == **k)),
                );
                if env.is_empty() {
                    new.pop("env");
                } else {
                    new.set("env", env);
                }
            }
            plan.push(SettingsFix { path: file, raw, new, drop_env, drop_top });
        }
    }
    Ok(plan)
}

/// 这次要改的摘要：同意的范围（界面 App 同意后带着它重跑；文件变了就对不上，重新问）
pub fn settings_fix_digest(plan: &[SettingsFix]) -> String {
    let mut h = Sha256Stream::new();
    for fix in plan {
        let item: Vec<V> = vec![
            V::from(&fix.path),
            V::from(sha256_hex(&fix.raw)),
            V::List(str_list(&fix.drop_env)),
            V::List(str_list(&fix.drop_top)),
        ];
        h.update(json::dumps(&V::List(List::from_vec(item)), json::Fmt::new()));
    }
    text::head(&h.hexdigest(), 16).to_string()
}

/// 原文整份备份在 path 旁边：是链接时在链接旁边（不写进它指向的目录，比如 dotfiles 的仓库）。600，名字带日期时间，不覆盖已有的
pub fn settings_backup(file: &str, raw: &[u8]) -> io::Result<String> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let stamp = time::strftime_local("%Y%m%d-%H%M%S", time::time());
    for i in 0..100 {
        let backup = if i == 0 { format!("{file}.cca-backup-{stamp}") } else { format!("{file}.cca-backup-{stamp}-{i}") };
        // os.open(backup, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        let exclusive =
            std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(&backup);
        let mut f = match exclusive {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        f.write_all(raw)?;
        // SAFETY: f 开着，fd 有效
        if unsafe { libc::fsync(f.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(backup);
    }
    // raise FileExistsError(path)：只有这一个参数，说明就是路径
    Err(io::Error::new(io::ErrorKind::AlreadyExists, file.to_string()))
}

/// py settings-fix plan | apply 摘要（cca settings fix）。plan：第一行是摘要（同意的范围），后面是给人看的说明；没有要改的
/// 退出码 3。apply：拿着 sync.lock（py link 也在锁里改共享的配置）照同一个摘要改，每份先整份备份在旁边再原子替换（是链接的照旧
/// 写到它指向的文件）；摘要对不上（刚被改过）退出码 2，什么都不动。读不了、不是 JSON、写不了：退出码 4，stdout 是说明
pub fn cmd_settings_fix(args: &[String]) -> R<()> {
    arity("cmd_settings_fix", args, 1, 2)?;
    let (action, digest) = (args[0].as_str(), arg(args, 1, ""));
    if action == "plan" {
        let plan = match settings_fix_plan() {
            Ok(plan) => plan,
            Err(why) => {
                out!("{why}");
                return exit(4);
            }
        };
        if plan.is_empty() {
            return exit(3);
        }
        out!("{}", settings_fix_digest(&plan));
        for fix in &plan {
            out!("{} 里要去掉：{}。", home_tilde(&fix.path), settings_fix_parts(&fix.drop_env, &fix.drop_top).join("；"));
        }
        out!("别的设置不动；改之前先把整份原文备份在旁边（文件名后面加 .cca-backup-日期时间，只有你自己能读）。");
        out!("改了以后，不经 cca、直接运行 claude 也不再用这几项；想恢复原样，把备份拷回去就行。");
        return Ok(());
    }
    // (路径, 备份, 去掉了几项)
    let mut done: Vec<(String, String, usize)> = Vec::new();
    let mut api = false;
    {
        let _lock = Lock::acquire()?;
        let plan = match settings_fix_plan() {
            Ok(plan) => plan,
            Err(why) => {
                out!("{why}");
                return exit(4);
            }
        };
        if plan.is_empty() || settings_fix_digest(&plan) != digest {
            return exit(2);
        }
        let mut backups: Vec<String> = Vec::new();
        let mut cur = plan[0].path.clone();
        let mut apply = || -> R<()> {
            for fix in &plan {
                cur = fix.path.clone();
                backups.push(settings_backup(&fix.path, &fix.raw)?);
            }
            for fix in &plan {
                if std::fs::read(&fix.path)? != fix.raw {
                    // 备份的这会儿又被改过：都不动
                    for backup in &backups {
                        std::fs::remove_file(backup)?;
                    }
                    return exit(2);
                }
            }
            for (fix, backup) in plan.iter().zip(&backups) {
                cur = fix.path.clone();
                save_json(&fix.path, &V::Dict(fix.new.clone()), None, true)?;
                done.push((fix.path.clone(), backup.clone(), fix.drop_env.len() + fix.drop_top.len()));
                api = api
                    || fix.drop_top.iter().any(|k| k == "apiKeyHelper")
                    || fix.drop_env.iter().any(|k| SETTINGS_KINDS[settings_kind(k)].id == "api");
            }
            Ok(())
        };
        match apply() {
            Ok(()) => {}
            Err(Fail::Os(e)) => {
                for backup in &backups[done.len()..] {
                    let _ = std::fs::remove_file(backup);
                }
                // e.strerror or e
                let why = match e.raw_os_error() {
                    Some(errno) => errno_text(errno),
                    None => os_error_text(&e),
                };
                out!("{} 改不了（{}）{}", home_tilde(&cur), why, if done.is_empty() { "，什么都没改" } else { "，前面的已经改好" });
                return exit(4);
            }
            Err(e) => return Err(e),
        }
    }
    for (file, backup, n) in &done {
        out!("已修复 {}：去掉了 {} 项", home_tilde(file), n);
        out!("原来的整份文件备份在 {}（只有你自己能读）", home_tilde(backup));
        out!("想恢复原样：cp {} {}", shell_path(backup), shell_path(file));
    }
    if api {
        out!("还要用这个第三方 API 的话：{} add api 把它加成一个单独的账号（接口地址、模型和 key 都在备份文件里）", t().cmd);
    }
    Ok(())
}

/// 桌面 App 只能依赖共享配置；无效或不可读不能等同于已关闭回退。
pub fn cmd_codex_fallback_off(args: &[String]) -> R<()> {
    arity("cmd_codex_fallback_off", args, 0, 0)?;
    // 这里直接调的 read_toml_config、codex_settings_problems：比 settings-check 那条路浅 3 层、1 层
    let data = {
        let _frames = shifted(-3);
        read_toml_config(&path::join(&codex().master, "config.toml"))
    };
    let disabled = match data {
        Ok(data) => {
            let off =
                data.dict("features").is_some_and(|features| matches!(features.get("system_proxy_fallback"), Some(V::Bool(false))));
            toml::dispose(data);
            let _frames = shifted(-1);
            off && codex_settings_problems().is_empty()
        }
        Err(e) if e.is(ConfigReadError::CLASS) => false,
        Err(e) => return Err(e),
    };
    exit(if disabled { 0 } else { 1 })
}

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("settings-check", cmd_settings_check),
    Cmd::all("launch-check", cmd_launch_check),
    Cmd::all("settings-explain", cmd_settings_explain),
    Cmd::all("settings-fix", cmd_settings_fix),
    Cmd::all("codex-fallback-off", cmd_codex_fallback_off),
];

#[cfg(test)]
mod tests {
    // 只测不看 HOME 的那些：按路径查的函数放在临时目录里测；经子命令走的路在 tests/*.rs（suite 的各个模块）和回归里
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn tmpdir(tag: &str) -> String {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-settings-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    #[test]
    fn json_limits_like_python() {
        let nested = |n: usize| format!("{{\"x\": {}{}}}", "[".repeat(n), "]".repeat(n));
        assert!(json_limits(&nested(JSON_NESTING_MAX - 1)).is_ok());
        assert!(is_recursion(&json_limits(&nested(JSON_NESTING_MAX)).unwrap_err()));
        // 没合上的也先数出来，不会递归到把栈用完
        assert!(is_recursion(&json_loads(&"[".repeat(1_000_000)).unwrap_err()));
        // 字符串里的括号、数字不算
        assert!(json_limits(&format!("{{\"x\": \"{}\\\"{}\"}}", "[".repeat(5000), "9".repeat(5000))).is_ok());
        // 整数最多 4300 位（说法是 CPython 3.14 的 str(ValueError)）；小数、指数不限
        assert!(json_loads(&format!("[{}]", "9".repeat(4300))).is_ok());
        let e = json_loads(&format!("[-{}]", "9".repeat(4301))).unwrap_err();
        assert!(e.is_value());
        assert_eq!(
            e.to_string(),
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit"
        );
        let (nines, ones) = ("9".repeat(5000), "1".repeat(5000));
        for float in [format!("{nines}.5"), format!("0.{nines}"), format!("1e{ones}"), format!("1E-{ones}"), format!("{nines}e5")] {
            assert!(json_loads(&float).is_ok(), "{}…", &float[..12]);
        }
        // 后面的小数点、e 没跟着数字：前面那一串还是整数
        assert!(json_limits(&format!("{nines}.")).is_err());
        assert!(json_limits(&format!("{nines}e+")).is_err());
    }

    #[test]
    fn ignorecase_matches_what_python_matches() {
        // Python 的 re.I 把 İ、ı 当 i（ſ 当 s、K 当 k 两边都认）。兼容层的正则认不出前两个：它补上以后这个测试会提醒把 imatch 去掉
        for name in ["ANTHROP\u{131}C_BASE_URL", "ANTHROP\u{130}C_API_KEY"] {
            assert!(!SETTINGS_BAD.matches(name) && imatch(&SETTINGS_BAD, name), "{name}");
        }
        assert!(SETTINGS_BAD.matches("HTTP\u{17f}_PROXY") && SETTINGS_BAD.matches("ANTHROPIC_API_\u{212a}EY"));
        assert_eq!(SETTINGS_KINDS[settings_kind("ANTHROP\u{131}C_BASE_URL")].id, "api");
        assert_eq!(SETTINGS_KINDS[settings_kind("CLAUDE_CONF\u{130}G_DIR")].id, "login");
        assert_eq!(SETTINGS_KINDS[settings_kind("EDITOR")].id, "other");
        assert!(
            claude_model_name(&V::from(" ha\u{131}ku[1M] "))
                && !claude_model_name(&V::from("kimi-k2"))
                && !claude_model_name(&V::from(1))
        );
    }

    #[test]
    fn expanduser_knows_other_users() {
        // os.path.expanduser：~root 换成 root 的主目录（各系统不一样），没有的用户原样
        let root = expanduser("~root/x");
        assert!(root.starts_with('/') && root.ends_with("/x") && !root.contains('~'), "{root}");
        assert_eq!(expanduser("~root"), root[..root.len() - 2]);
        assert_eq!(expanduser("~no-such-user-cca-test/x"), "~no-such-user-cca-test/x");
        assert_eq!(expanduser("a~root"), "a~root");
        assert_eq!(expanduser(""), "");
        assert_eq!(expanduser("~/x"), path::expanduser("~/x"));
        assert_eq!(expanduser("~"), path::expanduser("~"));
    }

    #[test]
    fn profile_files_in_python_order_even_when_not_utf8() {
        let names: Vec<&[u8]> = vec![
            b"b.config.toml",
            b"a\xff.config.toml",
            b"a.config.toml",
            b"\xff.config.toml",
            "é.config.toml".as_bytes(),
            "\u{ff21}.config.toml".as_bytes(),
            "😀.config.toml".as_bytes(),
            b".h\xff.config.toml",
            b"\xffnotes.toml",
            b"\xe4\xb8.config.toml",
            "\u{e000}.config.toml".as_bytes(),
            "\u{d7ff}.config.toml".as_bytes(),
            b"Z.config.toml",
            b"a\x80b.config.toml",
            b"a\xc3.config.toml",
            b"~.config.toml",
            b"config.toml",
            b".config.toml",
        ];
        let found = codex_profile_files(names.into_iter().map(|n| OsString::from_vec(n.to_vec())).collect());
        // CPython 的 sorted(os.listdir(…)) 里算 profile 的那些，照它给出的先后（不是 UTF-8 的字节在它那里是 U+DC80–U+DCFF）
        let want: Vec<&[u8]> = vec![
            b"Z.config.toml",
            b"a.config.toml",
            b"a\x80b.config.toml",
            b"a\xc3.config.toml",
            b"a\xff.config.toml",
            b"b.config.toml",
            b"~.config.toml",
            b"\xc3\xa9.config.toml",
            b"\xed\x9f\xbf.config.toml",
            b"\xe4\xb8.config.toml",
            b"\xff.config.toml",
            b"\xee\x80\x80.config.toml",
            b"\xef\xbc\xa1.config.toml",
            b"\xf0\x9f\x98\x80.config.toml",
        ];
        assert_eq!(found.iter().map(|(_, name)| name.clone().into_vec()).collect::<Vec<_>>(), want);
        assert_eq!(found[2].0, "a\u{fffd}b.config.toml");
        assert_eq!(found[7].0, "é.config.toml");
    }

    #[test]
    fn unreadable_is_never_empty() {
        let d = tmpdir("read");
        let file = |name: &str| format!("{d}/{name}");
        assert_eq!(config_text(&file("missing")).unwrap(), None);
        assert_eq!(config_text(&file("no-such-dir/missing")).unwrap(), None);
        std::fs::write(file("ok"), "a\r\nb\rc").unwrap();
        assert_eq!(config_text(&file("ok")).unwrap().as_deref(), Some("a\nb\nc"));
        std::fs::create_dir(file("dir")).unwrap();
        std::os::unix::fs::symlink("nowhere", file("dangling")).unwrap();
        std::os::unix::fs::symlink("loop", file("loop")).unwrap();
        std::fs::write(file("binary"), b"\xff").unwrap();
        std::fs::write(file("secret"), "{}").unwrap();
        fs::chmod(&file("secret"), 0o000).unwrap();
        // SAFETY: 只是问一下是谁在跑
        let root = unsafe { libc::geteuid() } == 0;
        for name in ["dir", "dangling", "loop", "binary", "ok/under-a-file", "secret"] {
            if name == "secret" && root {
                continue; // root 什么都读得了
            }
            let e = config_text(&file(name)).unwrap_err();
            assert!(e.is(ConfigReadError::CLASS) && e.to_string() == "unreadable_config", "{name}：{e:?}");
            assert_eq!(claude_config_file_problems(&file(name), Some("s")), ["s:invalid_or_unreadable_config"], "{name}");
            assert_eq!(
                codex_config_file_problems(&file(name), None, false),
                [format!("{}:invalid_or_unreadable_config", file(name))],
                "{name}"
            );
            assert_eq!(agy_session_settings_problems(name, &d), ["--settings:unreadable_config"], "{name}");
        }
        assert!(
            claude_config_file_problems(&file("missing"), None).is_empty()
                && codex_config_file_problems(&file("missing"), None, false).is_empty()
        );
        assert_eq!(agy_session_settings_problems("missing", &d), ["--settings:unreadable_config"]);
        fs::chmod(&file("secret"), 0o600).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn toml_config_reads_real_toml() {
        // tests/isolation_settings.rs 的 rollout_repair_uses_real_toml_and_reports_invalid_config 里 read_toml_config 的那部分
        let d = tmpdir("toml");
        let file = format!("{d}/config.toml");
        assert!(read_toml_config(&file).unwrap().is_empty());
        std::fs::write(
            &file,
            "sqlite_home = \"\"\"\n/data/bases\"\"\"\nwhen = 2026-11-08\n[features]\nsystem_proxy_fallback = false\n",
        )
        .unwrap();
        let data = read_toml_config(&file).unwrap();
        assert_eq!(data.str("sqlite_home").as_deref(), Some("/data/bases"));
        assert!(data.at("when").is_none()); // 日期：不是字符串，也不是别的哪一种
        assert_eq!(V::Dict(data.dict("features").unwrap()).py_repr(), "{'system_proxy_fallback': False}");
        for bad in ["sqlite_home = [", "a = 1\na = 2", &format!("a = {}", "[".repeat(600))] {
            std::fs::write(&file, bad).unwrap();
            let e = read_toml_config(&file).unwrap_err();
            assert!(e.is(ConfigReadError::CLASS) && e.to_string() == "invalid_toml", "{e:?}");
        }
        // toml_loads 自己：解析不了是 ValueError，套得太深是 RecursionError（-c 的值只在前一种时当成没加引号的字符串再试）
        assert!(toml_loads("a = [").unwrap_err().is_value());
        assert!(is_recursion(&toml_loads(&format!("a = {}", "[".repeat(600))).unwrap_err()));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn codex_overrides_and_nesting() {
        let check = |text: &str, overrides: bool, allow: bool| {
            codex_config_problems(&V::Dict(toml_loads(text).unwrap()), "s", overrides, allow).unwrap()
        };
        // 共享的 config.toml 里可以开着 system_proxy_fallback（启动时由 --disable 关掉），profile、-c 里不行
        assert!(check("features.system_proxy_fallback = true", false, true).is_empty());
        assert_eq!(check("features.system_proxy_fallback = true", false, false), ["s:features.system_proxy_fallback"]);
        assert_eq!(check("features.system_proxy_fallback = true", true, true), ["s:features.system_proxy_fallback"]);
        assert_eq!(
            check("[profiles.p.features]\nsystem_proxy_fallback = true", false, true),
            ["s:profiles.p:features.system_proxy_fallback"]
        );
        // -c 里出现就算，不看值
        assert_eq!(
            check("model_provider = \"openai\"\nforced_login_method = \"chatgpt\"", true, false),
            ["s:model_provider", "s:forced_login_method"]
        );
        assert!(check("model_provider = \"openai\"\nforced_login_method = \"chatgpt\"", false, false).is_empty());
        assert_eq!(codex_config_problems(&V::from(1), "s", false, false).unwrap(), ["s:invalid_config"]);
        // profiles 一层套一层：到 Python 的递归上限就是 RecursionError
        let nested = |n: usize| {
            let leaf = crate::dict! { "chatgpt_base_url" => "u" };
            (0..n).fold(V::Dict(leaf), |inner, _| V::Dict(crate::dict! { "profiles" => crate::dict! { "a" => inner } }))
        };
        let deepest = toml::RECURSION_LIMIT - CODEX_CONFIG_FRAME;
        assert_eq!(
            codex_config_problems(&nested(deepest), "s", false, false).unwrap(),
            [format!("s{}:chatgpt_base_url", ":profiles.a".repeat(deepest))]
        );
        assert!(is_recursion(&codex_config_problems(&nested(deepest + 1), "s", false, false).unwrap_err()));
    }

    #[test]
    fn backup_is_private_and_never_overwrites() {
        let d = tmpdir("backup");
        let file = format!("{d}/settings.json");
        let first = settings_backup(&file, b"one").unwrap();
        assert!(re!(r"^.*/settings\.json\.cca-backup-\d{8}-\d{6}$").matches(&first), "{first}");
        assert_eq!((std::fs::read(&first).unwrap(), fs::mode_of(&first).unwrap()), (b"one".to_vec(), 0o600));
        // 这个名字有了（哪怕是个链接）：不覆盖、不跟着链接写过去，名字后面加序号
        let stamp = &first[file.len() + ".cca-backup-".len()..];
        for i in 1..=3 {
            std::os::unix::fs::symlink(format!("{d}/elsewhere"), format!("{file}.cca-backup-{stamp}-{i}")).unwrap();
        }
        let next = settings_backup(&file, b"two").unwrap();
        assert!(next != first && std::fs::read(&next).unwrap() == b"two" && !path::exists(&format!("{d}/elsewhere")), "{next}");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        // 目录没了：OSError
        assert_eq!(settings_backup(&format!("{d}/gone/settings.json"), b"x").unwrap_err().raw_os_error(), Some(libc::ENOENT));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn digest_and_paths() {
        // 摘要是 Python 的 settings_fix_digest 对同样的内容给出的
        let fix = SettingsFix {
            path: "/home/u/.claude/settings.json".into(),
            raw: b"{\"env\": {\"NO_PROXY\": \"x\"}}".to_vec(),
            new: Dict::new(),
            drop_env: vec!["NO_PROXY".into(), "ANTHROP\u{131}C_BASE_URL".into()],
            drop_top: vec!["apiKeyHelper".into()],
        };
        assert_eq!(settings_fix_digest(std::slice::from_ref(&fix)), "ab664a80aca65bdc");
        assert_eq!(settings_fix_digest(&[]), "e3b0c44298fc1c14");
        let home: &str = &HOME;
        assert_eq!(home_tilde("/elsewhere/x"), "/elsewhere/x");
        assert_eq!(home_tilde(home), "~");
        assert_eq!(home_tilde(&format!("{home}/a b")), "~/a b");
        assert_eq!(home_tilde(&format!("{home}x")), format!("{home}x"));
        assert_eq!(shell_path(&format!("{home}/a b/it's")), "~/'a b/it'\"'\"'s'");
        assert_eq!(shell_path("/else where"), "'/else where'");
    }
}
