//! 钥匙串和密钥文件（`keyfile_*`、`security`、`ks_*`、`kc_*`）：api-key-out、secret-check / set / del / status、secrets-missing、
//! secrets-forget、key-show、key-forget、key-share。对应 PY_SRC 的 `keyfile_store` 到 `cmd_key_show`。
//!
//! API key（下面叫 secret，免得和账号表的键混了），都存在钥匙串里：
//! - 第三方 API 账号的：服务名 cca-api-账号目录名（Codex 账号是 cca-codex-api-账号目录名），账户名是接口地址的域名——地址换成
//!   别的服务商，旧 key 就取不到，不会被发过去。账号表里写了 key 字段时用表里的。
//! - 官方账号记下备查的（控制台里建的 API key；cca 绝不拿它登录或启动账号，那样会变成按量计费）：服务名 cca-ref-账号目录名
//!   （Codex 是 cca-codex-ref-），账户名是 REF_ACCT。
//!
//! cca 存过 key 的账号记在 config.json 的 api_keys / ref_keys：删账号（包括在 cca edit 里删、直接改账号表删）后据此清掉钥匙串里的
//! key；key_hints 记着每个 key 的掩码（key_hint）和存的时候的域名：_gui state 照它显示，不读钥匙串。
//! 没有钥匙串的系统（Linux 服务器）把 key 存成文件：~/.cca/secrets/<服务名>（服务名和钥匙串里的一样），内容是
//! {"host": 账户名, "key": key}；目录 700、文件 600，先写临时文件（建的时候就是 600）再原子替换。CCA_KEYSTORE=file 时 macOS
//! 上也这样（测试用）。bash 的 keyfile_store 是同一个判断。
//!
//! 秘密绝不进任何进程的命令行和环境变量：存钥匙串时，命令和十六进制的秘密都经 `security -i` 的 stdin 交过去。

use super::table::need;
use super::{arg, arity, Cmd};
use crate::core::base::{secret_re_match, CCA_HOME, CFG, SECRET_MSG};
use crate::core::ctx::{cfg_save, ctx};
use crate::core::model::{acct_id, api_host, cfg_load, is_api, key_hint, secret_ok, table_load, REF_ACCT};
use crate::core::policy::POLICY_FILE;
use crate::core::tool::{claude, t};
use crate::py::fs::{self, load_quiet};
use crate::py::hash::to_hex;
use crate::py::io;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::text;
use crate::py::time;
use crate::py::{die, exit, Fail, R};
use crate::{dict, err, out, out_raw, re};
use std::collections::HashSet;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::sync::LazyLock;

/// 密钥文件放在这里（`~/.cca/secrets`）
pub static SECRETS_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "secrets"));

/// 用密钥文件（不是 macOS，或者 `CCA_KEYSTORE=file`）还是钥匙串
pub fn keyfile_store() -> bool {
    if cfg!(test) {
        // 单元测试一律走钥匙串那一支、配假的 security（见 testing）：密钥文件在真的 ~/.cca/secrets 里，测试不能碰
        return false;
    }
    !cfg!(target_os = "macos") || getenv("CCA_KEYSTORE").as_deref() == Some("file")
}

pub fn keystore_name() -> &'static str {
    if keyfile_store() {
        "密钥文件"
    } else {
        "钥匙串"
    }
}

/// (退出码, key)：退出码和 security find-generic-password 一样，没有是 44，读不了是 1
pub fn keyfile_read(host: &str, svc: &str) -> (i32, String) {
    let raw = match std::fs::read(path::join(&SECRETS_DIR, svc)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (44, String::new()),
        Err(_) => return (1, String::new()),
    };
    let Ok(d) = json::loads_file_bytes(&raw) else {
        return (1, String::new());
    };
    // 域名对不上算没有，和钥匙串的账户名一个意思
    match d.as_dict().filter(|d| d.str("host").as_deref() == Some(host)).and_then(|d| d.str("key")) {
        Some(key) => (0, key.to_string()),
        None => (44, String::new()),
    }
}

/// `os.makedirs(p, mode=mode, exist_ok=True)`：Python 3.7 起 mode 只管最后一层，中间新建的各层是默认权限（777 过 umask）。
/// 兼容层的 `fs::makedirs_mode` 每一层都用 mode，对不上，所以写在这里
fn makedirs_leaf_mode(p: &str, mode: u32) -> std::io::Result<()> {
    let head = path::dirname(p);
    if !head.is_empty() && !path::basename(p).is_empty() && !path::exists(&head) {
        match fs::makedirs(&head) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    match std::fs::DirBuilder::new().mode(mode).create(p) {
        // 建不了：已经是目录（跟着链接看）就算了，别的原样报
        Err(e) if !path::isdir(p) => Err(e),
        _ => Ok(()),
    }
}

/// `os.fsync(fd)`：就是 fsync(2)。不用 `File::sync_all`：它在 macOS 上是 F_FULLFSYNC，慢得多，有的文件系统还不支持
fn fsync(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: f 开着，fd 有效
        if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// 存成密钥文件：目录 700，文件建的时候就是 600（`O_EXCL|O_NOFOLLOW` 的临时文件），fsync 后原子替换
pub fn keyfile_write(host: &str, svc: &str, s: &str) -> R<()> {
    makedirs_leaf_mode(&SECRETS_DIR, 0o700)?;
    fs::chmod(&SECRETS_DIR, 0o700)?;
    let p = path::join(&SECRETS_DIR, svc);
    let tmp = format!("{}.cca-{}.tmp", p, std::process::id());
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(&tmp)?;
    let mut text = json::dumps(&V::Dict(dict! { "v" => 1, "host" => host, "key" => s }), json::Fmt::new());
    text.push('\n');
    let written = f.write_all(text.as_bytes()).and_then(|()| fsync(&f));
    drop(f); // 先关上再改名
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, &p)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

pub fn keyfile_del(svc: &str) -> R<()> {
    match std::fs::remove_file(path::join(&SECRETS_DIR, svc)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// 运行 security，返回 (退出码, stdout)；data 从 stdin 交给它（security -i）。起不来、30 秒没跑完都是 (1, 空)
#[cfg(not(test))]
pub fn security(args: &[&str], data: Option<&[u8]>) -> (i32, Vec<u8>) {
    use crate::py::proc::{Cmd, In, Out};
    let argv = std::iter::once("security").chain(args.iter().copied());
    let cmd = Cmd::new(argv).stdout(Out::Capture).stderr(Out::Null).timeout(30.0);
    let cmd = match data {
        Some(d) => cmd.input(d),
        None => cmd.stdin(In::Null),
    };
    match cmd.run() {
        Ok(done) => (done.rc, done.stdout),
        Err(_) => (1, Vec::new()),
    }
}

/// 单元测试里绝不起真的 security：换成 `testing::set_security` 装的假的
#[cfg(test)]
pub fn security(args: &[&str], data: Option<&[u8]>) -> (i32, Vec<u8>) {
    testing::security(args, data)
}

pub fn kc_svc(aid: &str, is_ref: bool) -> String {
    format!("{}{}", if is_ref { t().kc_ref_prefix } else { t().kc_prefix }, aid)
}

/// 账号表的值 → (钥匙串里的账户名, ref)：第三方 API 账号的 key 按接口的域名存；官方账号记下备查的存在 reference 下
pub fn kc_slot(v: &V) -> (String, bool) {
    match v {
        V::Dict(d) => (api_host(d), false),
        _ => (REF_ACCT.to_string(), true),
    }
}

/// config.json 里记着存过 key 的账号目录名的那一项
pub fn kc_list(is_ref: bool) -> &'static str {
    if is_ref {
        "ref_keys"
    } else {
        "api_keys"
    }
}

pub fn kc_hints(st: &Dict) -> Dict {
    st.dict("key_hints").unwrap_or_default()
}

/// (security 的退出码, 秘密)：钥匙串里服务名 svc、账户名 acct 的那一条（没有钥匙串的系统读密钥文件）；没有（退出码 44）、
/// 读不了（锁着、超时）或不是可见的 ASCII 时是空字符串。cca 存的秘密（第三方 API 的 key、记下备查的 key、邮箱助手的应用专用
/// 密码）都经 ks_read、ks_write、ks_del
pub fn ks_read(svc: &str, acct: &str) -> (i32, String) {
    let (rc, s) = if keyfile_store() {
        keyfile_read(acct, svc)
    } else {
        let (rc, out) = security(&["find-generic-password", "-a", acct, "-s", svc, "-w"], None);
        (rc, text::strip(&String::from_utf8_lossy(&out)).to_string())
    };
    (rc, if rc == 0 && secret_re_match(&s) { s } else { String::new() })
}

/// `文字.encode('ascii')`：有别的字符是没人接的 UnicodeEncodeError
fn ascii(s: &str) -> R<&[u8]> {
    match s.chars().position(|c| !c.is_ascii()) {
        None => Ok(s.as_bytes()),
        Some(i) => Err(Fail::Value(format!("'ascii' codec can't encode character in position {i}: ordinal not in range(128)"))),
    }
}

/// 存进去：钥匙串里同一个账户名的那一条换掉（别的账户名的留着），密钥文件一个服务名只有一份。命令和十六进制的秘密都从
/// stdin 交给 security -i，不出现在任何进程的命令行里；security -i 里的命令失败了它也返回 0，所以存完读回来比对
pub fn ks_write(svc: &str, acct: &str, s: &str) -> R<bool> {
    if keyfile_store() {
        match keyfile_write(acct, svc, s) {
            Err(e) if e.is_os() => return Ok(false),
            r => r?,
        }
    } else {
        let line = format!("add-generic-password -U -a {} -s {} -X {}\n", acct, svc, to_hex(ascii(s)?));
        security(&["-i"], Some(ascii(&line)?));
    }
    Ok(ks_read(svc, acct).1 == s)
}

/// 删掉这个服务名的秘密（给了 acct 只删这个账户名的）：钥匙串里每次删一条，删到没有为止
pub fn ks_del(svc: &str, acct: Option<&str>) -> R<()> {
    if keyfile_store() {
        if acct.is_none_or(|a| keyfile_read(a, svc).0 == 0) {
            keyfile_del(svc)?;
        }
        return Ok(());
    }
    let mut args = vec!["delete-generic-password"];
    if let Some(a) = acct.filter(|a| !a.is_empty()) {
        args.extend(["-a", a]);
    }
    args.extend(["-s", svc]);
    for _ in 0..20 {
        if security(&args, None).0 != 0 {
            break;
        }
    }
    Ok(())
}

/// (security 的退出码, key)：钥匙串里这个账号、这个服务商的 key（见 ks_read）
pub fn kc_read(aid: &str, host: &str, is_ref: bool) -> (i32, String) {
    ks_read(&kc_svc(aid, is_ref), host)
}

pub fn kc_get(aid: &str, host: &str, is_ref: bool) -> String {
    kc_read(aid, host, is_ref).1
}

// config.json 里记账的 api_keys / ref_keys 本该是账号目录名的列表。手改坏了时，原实现的行为就是 Python 的 `set()`、`sorted()`、
// `in` 对那个值的行为（字符串拆成一个个字符、字典取键、数字是没人接的 TypeError……），下面三个照样

/// 没人接的 TypeError：原实现走到这里是调用栈 + 退出码 1
fn type_error<T>(msg: String) -> R<T> {
    Err(Fail::Other("TypeError", vec![format!("TypeError: {msg}")]))
}

/// `type(x).__name__`
fn type_name(v: &V) -> &'static str {
    match v {
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::List(_) => "list",
        V::Dict(_) => "dict",
    }
}

/// `set(x or [])`：重复的只留先来的（照 Python 的 ==：`True`、`1`、`1.0` 是同一个）
pub fn py_set(x: Option<V>) -> R<Vec<V>> {
    let items: Vec<V> = match x.filter(|v| v.truthy()) {
        None => return Ok(Vec::new()),
        Some(V::List(l)) => l.items(),
        Some(V::Str(s)) => s.chars().map(|ch| V::str(ch.to_string())).collect(),
        Some(V::Dict(d)) => d.keys().into_iter().map(V::from).collect(),
        Some(v) => return type_error(format!("'{}' object is not iterable", type_name(&v))),
    };
    let mut out: Vec<V> = Vec::new();
    for v in items {
        if v.is_list() || v.is_dict() {
            return type_error(format!("unhashable type: '{}'", type_name(&v)));
        }
        if !out.iter().any(|o| o.py_eq(&v)) {
            out.push(v);
        }
    }
    Ok(out)
}

/// `sorted(集合)`：字符串按码点排，数字按大小排；混着的、带 None 的两项以上比不了（TypeError）
pub fn py_sorted(mut items: Vec<V>) -> R<Vec<V>> {
    if items.len() < 2 {
        return Ok(items);
    }
    if items.iter().all(|v| v.is_str()) {
        items.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    } else if items.iter().all(|v| v.isinstance_num()) {
        items.sort_by(|a, b| match (a.as_int(), b.as_int()) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => a.as_pynum().partial_cmp(&b.as_pynum()).unwrap_or(std::cmp::Ordering::Equal),
        });
    } else {
        let other = items.iter().find(|v| type_name(v) != type_name(&items[0])).unwrap_or(&items[1]);
        return type_error(format!("'<' not supported between instances of '{}' and '{}'", type_name(other), type_name(&items[0])));
    }
    Ok(items)
}

/// `aid in (x or [])`：列表看有没有这一项，字符串看是不是它的一段，字典看有没有这个键
pub fn py_in(aid: &str, x: Option<V>) -> R<bool> {
    match x.filter(|v| v.truthy()) {
        None => Ok(false),
        Some(V::List(l)) => Ok(l.has_str(aid)),
        Some(V::Str(s)) => Ok(s.contains(aid)),
        Some(V::Dict(d)) => Ok(d.has(aid)),
        Some(v) => type_error(format!("argument of type '{}' is not a container or iterable", type_name(&v))),
    }
}

/// config.json 里记下（s 是 key）或者删掉（s 是 None）这个账号存过的 key：api_keys / ref_keys 和 key_hints（掩码、域名；
/// extra：官方账号记下备查的还有 at（什么时候记下的）、from（从哪台设备同步来的，见 keyshare_sync））
pub fn kc_book(aid: &str, host: &str, s: Option<&str>, is_ref: bool, extra: Option<&Dict>) -> R<()> {
    let c = cfg_load()?;
    let st = t().state(&c, true);
    let lst = kc_list(is_ref);
    let mut have = py_set(st.get(lst))?;
    let hints = kc_hints(&st);
    match s {
        None => {
            have.retain(|v| v.as_str() != Some(aid));
            hints.pop(aid);
        }
        // 域名总是记下（换了服务商算没存）；短的 key 没有掩码，hint 是空字符串
        Some(s) => {
            if !have.iter().any(|v| v.as_str() == Some(aid)) {
                have.push(V::str(aid));
            }
            let hint = dict! { "host" => host, "hint" => key_hint(s) };
            if let Some(extra) = extra {
                hint.update(extra);
            }
            hints.set(aid, hint);
        }
    }
    if !have.is_empty() || !is_ref {
        st.set(lst, List::from_vec(py_sorted(have)?));
    } else {
        st.pop(lst);
    }
    if !hints.is_empty() {
        st.set("key_hints", hints);
    } else {
        st.pop("key_hints");
    }
    cfg_save(&c)
}

/// 钥匙串里已经没有这个账号的 key 了：config.json 里的记录也删掉
pub fn kc_unbook(aid: &str, is_ref: bool) -> R<()> {
    if py_in(aid, t().state(&cfg_load()?, false).get(kc_list(is_ref)))? {
        kc_book(aid, "", None, is_ref, None)?;
    }
    Ok(())
}

/// 读到了 key、config.json 里却没有它的掩码和域名（旧版本存的）：补上，App 以后就能显示它
pub fn kc_backfill(aid: &str, host: &str, s: &str, is_ref: bool) -> R<()> {
    let st = t().state(&cfg_load()?, false);
    let h = kc_hints(&st).dict(aid);
    if py_in(aid, st.get(kc_list(is_ref)))?
        && !h.is_some_and(|h| h.str("host").as_deref() == Some(host) && h.str("hint").as_deref() == Some(key_hint(s).as_str()))
    {
        kc_book(aid, host, Some(s), is_ref, None)?;
    }
    Ok(())
}

/// 先删掉这个账号的旧 key（不管哪个服务商的），再存新的（ks_write：经 security -i 的 stdin，存完读回来比对）。extra 见 kc_book
pub fn kc_put(aid: &str, host: &str, s: &str, is_ref: bool, extra: Option<&Dict>) -> R<bool> {
    if host.is_empty() || re!(r"\s").found(host) {
        return Ok(false);
    }
    kc_del(aid, is_ref)?;
    if !ks_write(&kc_svc(aid, is_ref), host, s)? {
        return Ok(false);
    }
    kc_book(aid, host, Some(s), is_ref, extra)?;
    Ok(true)
}

/// 删掉这个账号在钥匙串里的 key（不管哪个服务商的，见 ks_del）
pub fn kc_del(aid: &str, is_ref: bool) -> R<()> {
    ks_del(&kc_svc(aid, is_ref), None)?;
    kc_unbook(aid, is_ref)
}

pub fn api_secret(k: &str, v: &Dict) -> String {
    match v.get("key").filter(secret_ok) {
        Some(s) => s.py_str(),
        None => kc_get(&acct_id(k, t()), &api_host(v), false),
    }
}

// --- 别的模块里的函数，还没移植 -------------------------------------------------------------------------------------
// 调用的地方照原样写着；那几个模块移植好了，把这一段删掉、换成它们的函数就行。
// 原函数在「没开设备统计」「没有单独配邮箱的账号」时什么也不做，而记下、删掉 key 的每一条路都要先经过它们：这一种这里
// 先答了（只有原函数开头那一句判断），不然这个模块的子命令一个都跑不到头。真要它们干活的时候是 unimplemented!

/// presence 模块的 `presence_load()[1]` 是不是空的，只认得出肯定没开的三种（原函数最前面的三个返回）：config.json 里写着
/// `"presence": false`、没有维护者的映射、映射里没有 presence 一节。映射里有 presence 一节时开没开要看它合不合格、
/// 有没有 deploy key（presence_problems、presence_key），那是 presence 模块的事，这里答 false（不知道）
fn presence_surely_off() -> bool {
    if load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("presence"), Some(V::Bool(false)))) {
        return true;
    }
    match load_quiet(&POLICY_FILE).and_then(|j| j.as_dict()) {
        None => true,
        Some(j) => matches!(j.get("presence"), None | Some(V::None)),
    }
}

/// keyshare 模块的 `with keyshare_edit(键, 时间) as ed`：在这台设备上记下、删掉官方账号记下备查的 key 时拿着 keys.lock，
/// 改完 `ed.mark(key 或 None)` 记下时间。没开设备统计时不记：进去、mark、出来都什么也不做
struct KeyshareEdit;

fn keyshare_edit(_key: &str, _at: f64) -> KeyshareEdit {
    if presence_surely_off() {
        return KeyshareEdit;
    }
    unimplemented!("共享记下的 API key 还没移植（keyshare：keyshare_edit）")
}

impl KeyshareEdit {
    /// `ed.mark(s)`
    fn mark(&mut self, _s: Option<&str>) {}

    /// `__exit__`；ok：with 里面没出异常
    fn exit(self, _ok: bool) {}
}

/// keyshare 模块的 `keyshare_on(key, v)`：这个账号记下备查的 key 会不会同步（映射里的 Claude、Codex 官方账号，开着设备统计）
fn keyshare_on(_key: &str, _v: &V) -> bool {
    if presence_surely_off() {
        return false;
    }
    unimplemented!("共享记下的 API key 还没移植（keyshare：keyshare_on）")
}

/// keyshare 模块的 `keyshare_kick(force)`：在后台跑一次 cca _key-sync（经 spawn_background）。keyshare_on 是真的才会调到
fn keyshare_kick(_force: bool) {
    unimplemented!("共享记下的 API key 还没移植（keyshare：keyshare_kick）")
}

/// mail 模块的 `mail_forget_orphans()`：清掉 config.json 里已经不在 Claude 账号表里的账号的 mail 覆盖（mail.accounts），
/// 地址没人再用了就删钥匙串里的应用专用密码。没有单独配邮箱的账号时什么也不做
fn mail_forget_orphans() -> R<()> {
    let c = cfg_load()?;
    if !c.dict("mail").and_then(|m| m.dict("accounts")).is_some_and(|a| !a.is_empty()) {
        return Ok(());
    }
    unimplemented!("邮箱助手还没移植（mail：mail_forget_orphans）")
}

// --- 子命令 -------------------------------------------------------------------------------------------------------

/// apiKeyHelper 回调（Claude Code 启动时和之后每 5 分钟左右来取一次）：把 key 打到 stdout。
/// 只给账号表里现有的第三方 API 账号：删掉的账号钥匙串里就算还留着 key 也不给；官方账号记下备查的 key 绝不给
pub fn cmd_api_key_out(args: &[String]) -> R<()> {
    arity("cmd_api_key_out", args, 0, 1)?;
    let aid = arg(args, 0, "");
    let table = table_load(t())?;
    let found = table.items().into_iter().find(|(k, v)| is_api(v) && acct_id(k, t()) == aid);
    let s = match found {
        Some((k, V::Dict(v))) if !k.is_empty() => api_secret(&k, &v),
        _ => String::new(),
    };
    if s.is_empty() {
        return die(format!("cca：没找到这个第三方 API 账号的 key（运行 {} key 账号名 重新存一下）", t().cmd));
    }
    out_raw!("{}", s);
    Ok(())
}

/// 从 stdin 读一个 key：合格时什么也不打印；不合格时打印原因、返回 1
pub fn cmd_secret_check(args: &[String]) -> R<()> {
    arity("cmd_secret_check", args, 0, 0)?;
    if !secret_re_match(text::strip(&io::read_stdin_text())) {
        out!("{}", SECRET_MSG);
        return exit(1);
    }
    Ok(())
}

/// 从 stdin 读 key 存进钥匙串：第三方 API 账号按现在的接口地址存；官方账号的是记下备查的（ref，cca 不拿它启动）
pub fn cmd_secret_set(args: &[String]) -> R<()> {
    arity("cmd_secret_set", args, 1, 1)?;
    let key = args[0].as_str();
    let table = table_load(t())?;
    need(&table, key)?;
    let input = io::read_stdin_text();
    let s = text::strip(&input);
    if !secret_re_match(s) {
        return die(SECRET_MSG);
    }
    let v = table.at(key);
    let (host, is_ref) = kc_slot(&v);
    let at = time::time();
    let mut ed = keyshare_edit(key, at);
    let put = (|| -> R<bool> {
        let extra = is_ref.then(|| dict! { "at" => at });
        let ok = kc_put(&acct_id(key, t()), &host, s, is_ref, extra.as_ref())?;
        if ok && is_ref {
            ed.mark(Some(s));
        }
        Ok(ok)
    })();
    ed.exit(put.is_ok());
    if !put? {
        return die(if keyfile_store() {
            "API key 存进密钥文件失败（看看 ~/.cca/secrets 的权限和磁盘空间）"
        } else {
            "API key 存进钥匙串失败（钥匙串被锁住了？）"
        });
    }
    // 官方账号记下备查的：同步给登录了这个账号的其他设备；stdout 说一声给 bash 的文案
    if is_ref && keyshare_on(key, &v) {
        out!("shared");
        keyshare_kick(true);
    }
    Ok(())
}

/// 删掉这个账号存过的 key（第三方 API 账号的、官方账号记下备查的都删）：只删这台设备上的（cca rm），不同步
pub fn cmd_secret_del(args: &[String]) -> R<()> {
    arity("cmd_secret_del", args, 1, 1)?;
    kc_del(&acct_id(&args[0], t()), false)?;
    kc_del(&acct_id(&args[0], t()), true)
}

/// cca [codex] key 账号 --forget（问过了）：删掉官方账号记下备查的 key，登录了这个账号的其他设备上的也删（同步）
pub fn cmd_key_forget(args: &[String]) -> R<()> {
    arity("cmd_key_forget", args, 1, 1)?;
    let key = args[0].as_str();
    let table = table_load(t())?;
    need(&table, key)?;
    let mut ed = keyshare_edit(key, time::time());
    let del = kc_del(&acct_id(key, t()), true);
    if del.is_ok() {
        ed.mark(None);
    }
    ed.exit(del.is_ok());
    del?;
    if keyshare_on(key, &table.at(key)) {
        keyshare_kick(true);
    }
    Ok(())
}

/// 这个账号记下备查的 API key 会不会同步给登录了它的其他设备：会就打印 1（bash 的文案用）
pub fn cmd_key_share(args: &[String]) -> R<()> {
    arity("cmd_key_share", args, 1, 1)?;
    let key = args[0].as_str();
    let table = table_load(t())?;
    need(&table, key)?;
    if keyshare_on(key, &table.at(key)) {
        out!("1");
    }
    Ok(())
}

/// key 在哪：table（账号表的 key 字段）、keychain（第三方 API 账号的，在钥匙串里）、ref（官方账号记下备查的），
/// 或者什么也不打印（还没有）。读到第三方 API 账号的 key 时顺便补上掩码（旧版本存的没有）
pub fn cmd_secret_status(args: &[String]) -> R<()> {
    arity("cmd_secret_status", args, 1, 1)?;
    let key = args[0].as_str();
    let v = match table_load(t())?.get(key) {
        None | Some(V::None) => return Ok(()),
        Some(v) => v,
    };
    let aid = acct_id(key, t());
    let V::Dict(v) = v else {
        if py_in(&aid, t().state(&cfg_load()?, false).get("ref_keys"))? {
            out!("ref");
        }
        return Ok(());
    };
    if v.get("key").is_some_and(|k| secret_ok(&k)) {
        out!("table");
        return Ok(());
    }
    let s = kc_get(&aid, &api_host(&v), false);
    if !s.is_empty() {
        kc_backfill(&aid, &api_host(&v), &s, false)?;
        out!("keychain");
    }
    Ok(())
}

/// 还没有 key 的第三方 API 账号（手写进账号表的、换了服务商的，或者钥匙串里的被删了）
pub fn cmd_secrets_missing(args: &[String]) -> R<()> {
    arity("cmd_secrets_missing", args, 0, 0)?;
    for (k, v) in table_load(t())?.items() {
        if let V::Dict(v) = v {
            if api_secret(&k, &v).is_empty() {
                out!("{k}");
            }
        }
    }
    Ok(())
}

/// 清掉钥匙串里已经不在账号表里的账号的 key（第三方 API 账号的、官方账号记下备查的）；scope=all 时全部清掉（cca restore）。
/// 官方账号记下备查的删了找不回来（控制台只在建的时候给一次全文），平时清掉时说一声（多半是 cca edit 里删掉了这个账号）
pub fn cmd_secrets_forget(args: &[String]) -> R<()> {
    arity("cmd_secrets_forget", args, 0, 1)?;
    let scope = arg(args, 0, "");
    let (c, table, _) = ctx()?;
    let st = t().state(&c, false);
    for is_ref in [false, true] {
        let keep: HashSet<String> = if scope == "all" {
            HashSet::new()
        } else {
            table.items().iter().filter(|(_, v)| is_api(v) != is_ref).map(|(k, _)| acct_id(k, t())).collect()
        };
        let gone = py_set(st.get(kc_list(is_ref)))?.into_iter().filter(|v| !v.as_str().is_some_and(|a| keep.contains(a))).collect();
        for aid in py_sorted(gone)? {
            let Some(aid) = aid.as_str() else {
                // 记着的不是字符串：原来到 kc_svc 里拼服务名时出错
                return type_error(format!("can only concatenate str (not \"{}\") to str", type_name(&aid)));
            };
            let hint = kc_hints(&st).dict(aid).and_then(|h| h.str("hint")).filter(|h| !h.is_empty());
            kc_del(aid, is_ref)?;
            if is_ref && scope != "all" {
                err!(
                    "已从{}删掉 {} 记下的 API key{}：账号表里已经没有这个账号了",
                    keystore_name(),
                    aid,
                    hint.map_or(String::new(), |h| format!("（{h}）"))
                );
            }
        }
    }
    if t().kind == claude().kind && scope != "all" {
        mail_forget_orphans()?; // 删了账号后，清掉它单独配的 Gmail 覆盖（mail.accounts）和没人再用的应用专用密码
    }
    Ok(())
}

/// 把这个账号的 key 原样打到 stdout（不换行；cca [codex] key 账号 --show，App 的「显示」「复制」经管道读它）：
/// 第三方 API 账号和启动时取的一样（账号表的 key 字段优先，再是钥匙串里这个服务商的）；官方账号是记下备查的。
/// 没有：退出码 3（钥匙串里的被删了时顺便把记录也删掉）；钥匙串读不了（锁着、超时）：退出码 4
pub fn cmd_key_show(args: &[String]) -> R<()> {
    arity("cmd_key_show", args, 1, 1)?;
    let key = args[0].as_str();
    let table = table_load(t())?;
    need(&table, key)?;
    let (v, aid) = (table.at(key), acct_id(key, t()));
    if let Some(k) = v.as_dict().and_then(|v| v.get("key")).filter(secret_ok) {
        out_raw!("{}", k.py_str());
        return Ok(());
    }
    let (host, is_ref) = kc_slot(&v);
    let (rc, s) = kc_read(&aid, &host, is_ref);
    if !s.is_empty() {
        kc_backfill(&aid, &host, &s, is_ref)?;
        out_raw!("{}", s);
        return Ok(());
    }
    if rc == 0 || rc == 44 {
        // 第三方 API 账号的不删：可能还存着旧服务商的那一把，secrets-forget 要靠记录找到它
        if is_ref {
            kc_unbook(&aid, is_ref)?;
        }
        return exit(3);
    }
    exit(4)
}

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("api-key-out", cmd_api_key_out),
    Cmd::all("secret-check", cmd_secret_check),
    Cmd::all("secret-set", cmd_secret_set),
    Cmd::all("secret-del", cmd_secret_del),
    Cmd::all("secret-status", cmd_secret_status),
    Cmd::all("secrets-missing", cmd_secrets_missing),
    Cmd::all("secrets-forget", cmd_secrets_forget),
    Cmd::all("key-show", cmd_key_show),
    Cmd::all("key-forget", cmd_key_forget),
    Cmd::all("key-share", cmd_key_share),
];

/// 单元测试里的 `security`：真的那个绝不能起（会动到这台机器的登录钥匙串），测试版里 `security()` 只走这里。
/// 做法和原来 Python 的测试一样（`ns['security'] = 假的`）：`set_security` 给这条测试线程装一个假的；没装就调到是测试写错了
#[cfg(test)]
pub(crate) mod testing {
    use std::cell::RefCell;

    type Hook = Box<dyn FnMut(&[&str], Option<&[u8]>) -> (i32, Vec<u8>)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// 这条测试线程里的 security 换成 f：拿到参数（不带开头的 security）和 stdin，返回 (退出码, stdout)
    pub(crate) fn set_security(f: impl FnMut(&[&str], Option<&[u8]>) -> (i32, Vec<u8>) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(super) fn security(args: &[&str], data: Option<&[u8]>) -> (i32, Vec<u8>) {
        HOOK.with(|h| match h.borrow_mut().as_mut() {
            Some(f) => f(args, data),
            None => panic!("单元测试里要先用 testing::set_security 装上假的 security（真的钥匙串不能碰）"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 内存里的假钥匙串，照 tests/mail_view.py 的 Keystore 里那个写（和 tests/setup.sh 的假 security 一样认 -a、-s、-X）
    #[derive(Default)]
    struct Keychain {
        /// ((服务名, 账户名), 秘密)，先存的在前
        items: Vec<((String, String), String)>,
        calls: Vec<Vec<String>>,
    }

    impl Keychain {
        fn one(&mut self, w: &[&str]) -> (i32, Vec<u8>) {
            let opt = |name: &str| w.iter().skip(1).position(|x| *x == name).and_then(|i| w.get(i + 2)).copied();
            let hit = self.items.iter().position(|((s, a), _)| Some(s.as_str()) == opt("-s") && opt("-a").is_none_or(|x| x == a));
            if w[0] == "add-generic-password" {
                let k = (opt("-s").unwrap().to_string(), opt("-a").unwrap().to_string());
                let v = String::from_utf8(crate::py::hash::from_hex(opt("-X").unwrap()).unwrap()).unwrap();
                match self.items.iter_mut().find(|(x, _)| *x == k) {
                    Some(item) => item.1 = v,
                    None => self.items.push((k, v)),
                }
                return (0, Vec::new());
            }
            let Some(hit) = hit else {
                return (44, Vec::new());
            };
            if w[0] == "find-generic-password" {
                return (0, format!("{}\n", self.items[hit].1).into_bytes());
            }
            self.items.remove(hit);
            (0, Vec::new())
        }

        fn security(&mut self, args: &[&str], data: Option<&[u8]>) -> (i32, Vec<u8>) {
            self.calls.push(args.iter().map(|s| s.to_string()).collect());
            if args[0] != "-i" {
                return self.one(args);
            }
            for line in text::splitlines(std::str::from_utf8(data.unwrap()).unwrap()) {
                self.one(&text::split_ws(line));
            }
            (0, Vec::new())
        }

        /// 装到这条测试线程上
        fn install() -> Rc<RefCell<Keychain>> {
            let kc = Rc::new(RefCell::new(Keychain::default()));
            let hooked = kc.clone();
            testing::set_security(move |args, data| hooked.borrow_mut().security(args, data));
            kc
        }
    }

    /// tests/mail_view.py 的 Keystore.test_write_replaces_the_same_account_only_and_delete_can_pick_one
    #[test]
    fn write_replaces_the_same_account_only_and_delete_can_pick_one() {
        let kc = Keychain::install();
        let (a, b) = ("abcdefghijklmnop", "ponmlkjihgfedcba");
        assert!(ks_write("svc", "a@example.com", a).unwrap());
        assert!(ks_write("svc", "b@example.com", b).unwrap());
        assert!(ks_write("svc", "b@example.com", a).unwrap());
        assert_eq!((ks_read("svc", "a@example.com"), ks_read("svc", "b@example.com")), ((0, a.to_string()), (0, a.to_string())));
        ks_del("svc", Some("a@example.com")).unwrap();
        let keys = || kc.borrow().items.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        assert_eq!(keys(), [("svc".to_string(), "b@example.com".to_string())], "只删这个账户名的");
        ks_del("svc", None).unwrap();
        assert!(keys().is_empty());
        let leaked = kc.borrow().calls.iter().any(|c| c[0] != "-i" && c.iter().any(|x| x == a || x == b));
        assert!(!leaked, "秘密不进命令行");
    }

    #[test]
    #[should_panic(expected = "真的钥匙串不能碰")]
    fn unit_tests_cannot_reach_the_real_keychain() {
        ks_read("svc", "a@example.com");
    }

    /// `sorted(set(x or []))` 的 repr；出错只说是哪一类（原文各版本的 Python 不一样，反正是没人接的调用栈）
    fn sorted_set(x: V) -> String {
        match py_set(Some(x)).and_then(py_sorted) {
            Ok(v) => V::from(v).py_repr(),
            Err(e) => {
                assert!(e.is("TypeError") && !e.is_exit() && !e.is_os_or_value());
                "TypeError".to_string()
            }
        }
    }

    /// 右边是 CPython 3.14 的结果（实测）
    #[test]
    fn set_and_sorted_like_python() {
        assert!(py_set(None).unwrap().is_empty());
        for falsy in [V::None, V::from(""), V::from(0), V::from(false), V::from(list![]), V::Dict(dict! {})] {
            assert_eq!(sorted_set(falsy), "[]");
        }
        assert_eq!(sorted_set(V::from("abca")), "['a', 'b', 'c']");
        assert_eq!(sorted_set(V::Dict(dict! { "b" => 1, "a" => 2 })), "['a', 'b']");
        assert_eq!(sorted_set(V::from(list!["b", "a", "B", "中", "a"])), "['B', 'a', 'b', '中']");
        assert_eq!(sorted_set(V::from(list![V::None])), "[None]");
        assert_eq!(sorted_set(V::from(list![true, 1])), "[True]");
        assert_eq!(sorted_set(V::from(list![1, true])), "[1]");
        assert_eq!(sorted_set(V::from(list![2, 1.5, true])), "[True, 1.5, 2]");
        // 字符串和数字、None 和别的比不了；数字、True 不能当列表拆；列表、字典进不了集合
        for bad in [
            V::from(list![1, true, 1.0, "a", "a"]),
            V::from(list!["b", V::None]),
            V::from(5),
            V::from(true),
            V::from(1.5),
            V::from(list![list!["a"]]),
            V::from(list!["a", dict! { "a" => 1 }]),
        ] {
            assert_eq!(sorted_set(bad.clone()), "TypeError", "{bad:?}");
        }
    }

    /// 右边是 CPython 3.14 的 `aid in (x or [])`（实测）
    #[test]
    fn membership_like_python() {
        let is_in = |aid: &str, x: V| py_in(aid, Some(x)).map_err(|e| e.is("TypeError"));
        assert!(!py_in("a", None).unwrap());
        assert_eq!(is_in("a", V::from("xax")), Ok(true)); // 字符串：是不是它的一段
        assert_eq!(is_in("ab", V::from("a-b")), Ok(false));
        assert_eq!(is_in("ab", V::from(list!["ab", 1])), Ok(true));
        assert_eq!(is_in("a", V::from(list![list!["a"], "ab"])), Ok(false));
        assert_eq!(is_in("ab", V::Dict(dict! { "ab" => 1 })), Ok(true));
        assert_eq!(is_in("a", V::Dict(dict! { "ab" => 1 })), Ok(false));
        assert_eq!(is_in("a", V::from(0)), Ok(false));
        assert_eq!(is_in("a", V::from(5)), Err(true));
        assert_eq!(is_in("a", V::from(true)), Err(true));
        assert_eq!(is_in("a", V::from(1.5)), Err(true));
    }

    #[test]
    fn slots_and_names() {
        assert_eq!(kc_slot(&V::from("US-A")), (REF_ACCT.to_string(), true));
        let api = V::Dict(dict! { "base_url" => "https://API.Example.com:8443/v1/" });
        assert_eq!(kc_slot(&api), ("api.example.com".to_string(), false));
        assert_eq!(kc_slot(&V::Dict(dict! {})), (String::new(), false));
        assert_eq!((kc_list(false), kc_list(true)), ("api_keys", "ref_keys"));
        let hints = dict! { "a" => dict! { "host" => "h" } };
        assert!(kc_hints(&dict! { "key_hints" => &hints }).same(&hints));
        assert!(kc_hints(&dict! { "key_hints" => list![1] }).is_empty());
        assert!(ascii("sk-ok").is_ok() && ascii("api.例子.com").is_err());
    }
}
