//! 桌面客户端和终端工具的更新（`CU_FEED` 到 `cmd_client_updates`）：client-updates。
//!
//! 中央桌面包检查：Claude（com.anthropic.claudefordesktop，Squirrel）和 ChatGPT（com.openai.codex，Sparkle）。auto 仅自动检查和暂存，
//! 安装要用户确认；用户打开「允许自动更新」（config.json 的 client_auto_install，`cu_auto_install`）后，检查完马上装（`cu_auto`）。
//! 不读取账号目录，也不修改原版客户端的更新器设置。
//!
//! - 这一份：四种客户端的定义、更新器自己的目录和锁、状态文件、当前客户端的快照、起外部命令、下载、给界面看的状态文档、
//!   检查（`cu_check`）、自动安装（`cu_auto`）、子命令 client-updates
//! - `clients/feed.rs`：官方版本列表（Sparkle 的 appcast、Squirrel 的 JSON）和取它的 `cu_metadata`
//! - `clients/extract.rs`：安装包解压（`cu_extract`）
//! - `clients/install.rs`：核对进程、替换程序、事务记录和恢复（`cu_no_processes` 到 `cu_install`）
//! - `clients/cli.rs`：终端工具（`cu_cli_run` 到 `cu_cli_install`）
//! - `clients/pyv.rs`：state.json 里类型不对时照 Python 出哪一类异常；`clients/urljoin.rs`：`urllib.parse.urljoin`；
//!   `clients/casefold.rs`：`str.casefold()`
//!
//! 这一块的函数都移植了。state.json、transaction.json 是从文件读进来的任意 JSON：类型不对时原来出的是哪一类异常，这里照样
//! （`Fail::Other("TypeError" / "KeyError" / "AttributeError", …)`），所以哪些被当成「无法安全处理客户端更新状态」、哪些是没人接的
//! 错（退出码 1），和原来一样。
//!
//! 原来全局的 `CU`（正在处理的那一种客户端）、`CU_BUDGET`（这次 check 的期限）在这里是线程里的变量（辅助程序只有一条线程）。

mod casefold;
mod cli;
mod extract;
mod feed;
mod install;
mod pyv;
#[cfg(test)]
mod tests;
mod urljoin;

use super::{arg, arity, Cmd};
use crate::core::base::{Lock, CCA_HOME, CFG, HOME};
use crate::py::fs::{self, load_json, load_quiet, save_json};
use crate::py::hash::{compare_digest, sha256_hex, Sha256Stream};
use crate::py::obj::{Dict, List, V};
use crate::py::plist::{self, P};
use crate::py::proc::{Cmd as Proc, Done, In, ProcError};
use crate::py::rand::token_hex;
use crate::py::shutil::TemporaryDirectory;
use crate::py::time::time;
use crate::py::url::{urlsplit, Url};
use crate::py::{die, json, path, signal, text};
use crate::py::{Fail, R};
use crate::{dict, err, out_raw, re};
use pyv::*;
use std::cell::Cell;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("client-updates", cmd_client_updates)];

const CU_FEED: &str = "https://persistent.oaistatic.com/codex-app-prod/appcast.xml";
const CU_MAX_PACKAGE: u64 = 2 * 1024 * 1024 * 1024;
const CU_NS: &str = "{http://www.andymatuschak.org/xml-namespaces/sparkle}";
/// 一次 check 最多用多久：App 等 900 秒，超时会杀掉 cca
const CU_CHECK_SECONDS: f64 = 840.0;
/// 后台多久检查一次：每次只是两个小请求，发现新版才下载
const CU_CHECK_INTERVAL: i64 = 6 * 3600;
/// 允许自动更新、下载好了而桌面客户端开着（等它退出再装）：后台多久再来看一次
const CU_AUTO_RETRY: i64 = 15 * 60;

/// 一种客户端。桌面客户端（`CuClient`）：产品标识、签名团队、更新源的规则；终端工具（`CuCli`，命令行的 Claude Code、Codex）：只查版本，
/// 不下载软件包，更新用它们自己的 update 命令（自己校验、自己替换），用户确认后才运行，命令在哪是 bash 找到的（env，空的就是没装）。
/// 两种在原来是两个类，各自没有的属性这里留空
pub struct Client {
    pub id: &'static str,
    pub label: &'static str,
    pub cli: bool,
    pub bundle: &'static str,
    pub team: &'static str,
    /// 终端工具没有这个属性：原来读它是 AttributeError
    pub app: Option<&'static str>,
    pub names: &'static [&'static str],
    pub env: &'static str,
    pub app_env: &'static str,
    /// `CFBundleVersion` 的样子：ChatGPT 是整数，Claude 和版本号一样是点分的
    pub dotted_build: bool,
    /// 更新器进程的名字里有的字样（`Sparkle|Autoupdate|Updater\.app`、`ShipIt|Squirrel`）
    pub updater: &'static [&'static str],
    pub command: &'static str,
    pub own_dir: &'static str,
    pub npm: &'static str,
}

const NOT_CLI: (&str, &str, &str) = ("", "", "");

pub static CU_CODEX: Client = Client {
    id: "codex",
    label: "ChatGPT",
    cli: false,
    bundle: "com.openai.codex",
    team: "2DC432GLL2",
    app: Some("ChatGPT.app"),
    names: &["ChatGPT.app", "Codex.app"],
    env: "CCA_CLIENT_CODEX_APP",
    app_env: "CCA_CODEX_APP",
    dotted_build: false,
    updater: &["Sparkle", "Autoupdate", "Updater.app"],
    command: NOT_CLI.0,
    own_dir: NOT_CLI.1,
    npm: NOT_CLI.2,
};
pub static CU_CLAUDE: Client = Client {
    id: "claude",
    label: "Claude",
    cli: false,
    bundle: "com.anthropic.claudefordesktop",
    team: "Q6L2SF6YDW",
    app: Some("Claude.app"),
    names: &["Claude.app"],
    env: "CCA_CLIENT_CLAUDE_APP",
    app_env: "CCA_CLAUDE_APP",
    dotted_build: true,
    updater: &["ShipIt", "Squirrel"],
    command: NOT_CLI.0,
    own_dir: NOT_CLI.1,
    npm: NOT_CLI.2,
};
pub static CU_CLAUDE_CLI: Client = Client {
    id: "claude-cli",
    label: "Claude Code（终端）",
    cli: true,
    bundle: "",
    team: "",
    app: None,
    names: &[],
    env: "CCA_CLI_CLAUDE",
    app_env: "",
    dotted_build: false,
    updater: &[],
    command: "claude",
    own_dir: ".local/share/claude/versions",
    npm: "@anthropic-ai/claude-code",
};
pub static CU_CODEX_CLI: Client = Client {
    id: "codex-cli",
    label: "Codex（终端）",
    cli: true,
    bundle: "",
    team: "",
    app: None,
    names: &[],
    env: "CCA_CLI_CODEX",
    app_env: "",
    dotted_build: false,
    updater: &[],
    command: "codex",
    own_dir: ".codex/packages/standalone",
    npm: "@openai/codex",
};
pub static CU_CLIENTS: [&Client; 4] = [&CU_CLAUDE, &CU_CODEX, &CU_CLAUDE_CLI, &CU_CODEX_CLI];

thread_local! {
    /// 正在处理的那一种客户端（和账号的 T 一样）
    static CU: Cell<&'static Client> = const { Cell::new(&CU_CODEX) };
    /// 这次 check 的期限（时间戳）；None 是不限
    static CU_BUDGET: Cell<Option<f64>> = const { Cell::new(None) };
}

fn cu() -> &'static Client {
    CU.with(|c| c.get())
}

fn set_cu(client: &'static Client) {
    CU.with(|c| c.set(client));
}

fn is(a: &Client, b: &Client) -> bool {
    std::ptr::eq(a, b)
}

impl Client {
    /// 官方安装器以外的装法 App 不代劳（npm、Homebrew 要用用户自己的环境和权限）：告诉用户在终端里运行什么
    fn advice(&self, method: &str, cask: &str) -> String {
        match method {
            "brew" => format!("brew upgrade --cask {cask}"),
            "npm" => format!("npm install -g {}@latest", self.npm),
            _ => format!("{} update", self.command),
        }
    }

    /// npm 上的最新版本从哪查
    fn npm_url(&self) -> String {
        if is(self, &CU_CLAUDE_CLI) {
            format!("https://registry.npmjs.org/@anthropic-ai%2Fclaude-code/{}", cu_claude_channel())
        } else {
            "https://registry.npmjs.org/@openai%2Fcodex/latest".to_string()
        }
    }

    /// 官方的最新版本从哪查
    fn latest(&self) -> String {
        if is(self, &CU_CLAUDE_CLI) {
            format!("https://downloads.claude.ai/claude-code-releases/{}", cu_claude_channel())
        } else {
            "https://api.github.com/repos/openai/codex/releases/latest".to_string()
        }
    }

    /// 运行这个命令时在空环境上另加的变量
    fn run_env(&self, folder: &str) -> R<Vec<(String, String)>> {
        if is(self, &CU_CLAUDE_CLI) {
            cu_claude_env(folder)
        } else {
            Ok(vec![("CODEX_NON_INTERACTIVE".into(), "1".into())])
        }
    }

    /// `CU.url_ok(p, package)`
    fn url_ok(&self, p: &Url, package: bool) -> bool {
        if self.cli {
            cu_url_cli(p, package)
        } else if is(self, &CU_CODEX) {
            cu_url_codex(p, package)
        } else {
            cu_url_claude(p, package)
        }
    }
}

// --- 目录和环境变量在哪 ----------------------------------------------------------------------------------------------------
// 平时就是 `core::base` 的 HOME / CCA_HOME / CFG 和这个进程的环境；单元测试里每个测试有自己的一套（`tests::World`），
// 才能在一个进程里把整条检查、安装的路走很多遍

fn home() -> String {
    #[cfg(test)]
    if let Some(w) = tests::world() {
        return w.home;
    }
    HOME.clone()
}

fn cca_home() -> String {
    #[cfg(test)]
    if let Some(w) = tests::world() {
        return w.cca_home;
    }
    CCA_HOME.clone()
}

fn cfg_path() -> String {
    #[cfg(test)]
    if let Some(w) = tests::world() {
        return path::join(&w.cca_home, "config.json");
    }
    CFG.clone()
}

/// `os.environ.get(k)`
fn getenv(k: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(w) = tests::world() {
        return w.env.get(k).cloned();
    }
    crate::py::proc::getenv(k)
}

/// `with Lock():`
fn sync_lock() -> R<Box<dyn std::any::Any>> {
    #[cfg(test)]
    if let Some(w) = tests::world() {
        return Ok(Box::new(tests::world_lock(&w)?));
    }
    Ok(Box::new(Lock::acquire()?))
}

fn getuid() -> u32 {
    // SAFETY: 不带参数，不会失败
    unsafe { libc::getuid() }
}

// --- 更新源的规则 ---------------------------------------------------------------------------------------------------------

/// `/前缀…后缀`，中间那一段照 `ok` 核对
fn between<'a>(s: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)?.strip_suffix(suffix)
}

fn cu_url_codex(p: &Url, package: bool) -> bool {
    if p.hostname().as_deref() != Some("persistent.oaistatic.com") || !p.query.is_empty() {
        return false;
    }
    if package {
        // /codex-app-prod/ChatGPT-darwin-(arm64|x64)-\d+(?:\.\d+){1,5}\.zip
        return ["arm64", "x64"].iter().any(|a| between(&p.path, &format!("/codex-app-prod/ChatGPT-darwin-{a}-"), ".zip").is_some_and(|v| dotted(v, 1, 5)));
    }
    p.path == "/codex-app-prod/appcast.xml"
}

fn cu_url_claude(p: &Url, package: bool) -> bool {
    const ARCHES: [&str; 3] = ["arm64", "x64", "universal"];
    if package {
        // /releases/darwin/(arm64|x64|universal)/\d+(?:\.\d+){1,5}/Claude-[0-9a-f]{7,64}\.zip
        let seg: Vec<&str> = p.path.split('/').collect();
        return p.hostname().as_deref() == Some("downloads.claude.ai")
            && p.query.is_empty()
            && seg.len() == 6
            && seg[..3] == ["", "releases", "darwin"]
            && ARCHES.contains(&seg[3])
            && dotted(seg[4], 1, 5)
            && between(seg[5], "Claude-", ".zip").is_some_and(|h| (7..=64).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
    p.hostname().as_deref() == Some("api.anthropic.com")
        && ARCHES.iter().any(|a| p.path == format!("/api/desktop/darwin/{a}/squirrel/update"))
        && re!(r"device_id=[0-9a-f-]{36}&version=[0-9.]+&os_version=[0-9.]+").fullmatch(&p.query).is_some()
}

/// 终端工具的最新版本从哪查（host → path）：和它们自己检查更新用的地址一样，都是不带身份的公开地址
const CU_CLI_SOURCES: [(&str, &[&str]); 4] = [
    ("downloads.claude.ai", &["/claude-code-releases/latest", "/claude-code-releases/stable"]),
    ("api.github.com", &["/repos/openai/codex/releases/latest"]),
    ("registry.npmjs.org", &["/@anthropic-ai%2Fclaude-code/latest", "/@anthropic-ai%2Fclaude-code/stable", "/@openai%2Fcodex/latest"]),
    ("formulae.brew.sh", &["/api/cask/claude-code.json", "/api/cask/claude-code@latest.json", "/api/cask/codex.json"]),
];

fn cu_url_cli(p: &Url, package: bool) -> bool {
    let host = p.hostname();
    !package && p.query.is_empty() && CU_CLI_SOURCES.iter().any(|(h, paths)| host.as_deref() == Some(*h) && paths.contains(&p.path.as_str()))
}

/// Claude Code 跟哪个发布通道（用户在共享的 settings.json 里设的，默认 latest）
pub fn cu_claude_channel() -> &'static str {
    match load_quiet(&path::join_all(&[&home(), ".claude", "settings.json"])).and_then(|s| s.as_dict()).and_then(|s| s.str("autoUpdatesChannel")).as_deref() {
        Some("stable") => "stable",
        _ => "latest",
    }
}

/// claude 用一次性的配置目录运行：不碰账号和 ~/.claude，不带设备标识（官方的更新命令不要登录）；更新通道和用户设的一样
pub fn cu_claude_env(folder: &str) -> R<Vec<(String, String)>> {
    let conf = path::join(folder, "claude");
    fs::makedirs(&conf)?;
    if cu_claude_channel() != "latest" {
        save_json(&path::join(&conf, "settings.json"), &V::Dict(dict! { "autoUpdatesChannel" => cu_claude_channel() }), Some(0o600), true)?;
    }
    Ok(vec![("CLAUDE_CONFIG_DIR".into(), conf), ("DISABLE_TELEMETRY".into(), "1".into()), ("DISABLE_ERROR_REPORTING".into(), "1".into())])
}

// --- 更新器自己的目录、锁、状态 --------------------------------------------------------------------------------------------

/// 更新器自己的路径只容许当前用户的普通文件/目录，不跟随链接。
fn cu_private(path: &str, directory: bool, create: bool) -> R<()> {
    if create && !path::lexists(path) {
        if directory {
            std::fs::DirBuilder::new().mode(0o700).create(path)?;
        } else {
            std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW).mode(0o600).open(path)?;
        }
    }
    let Ok(s) = std::fs::symlink_metadata(path) else {
        return cu_error("blocked", "无法读取更新器的私有文件");
    };
    let valid = if directory { s.is_dir() } else { s.is_file() && s.nlink() == 1 };
    if !valid || s.uid() != getuid() || s.mode() & 0o077 != 0 {
        return cu_error("blocked", "更新器目录或文件权限不安全");
    }
    Ok(())
}

/// `os.makedirs(p, mode=0o700)`：上面缺的各层是默认的权限，最里面那一层 700；已经有了是错
fn makedirs_new(p: &str) -> std::io::Result<()> {
    let parent = path::dirname(p.trim_end_matches('/'));
    if !parent.is_empty() && !path::exists(&parent) {
        match fs::makedirs(&parent) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    std::fs::DirBuilder::new().mode(0o700).create(p)
}

/// client-updates 目录（manager.lock 在这里）；各客户端的状态、软件包在它下面各一个目录
fn cu_base(create: bool) -> R<String> {
    let home = cca_home();
    let root = path::join(&home, "client-updates");
    if create {
        if !path::exists(&home) {
            makedirs_new(&home)?;
        }
        let s = std::fs::symlink_metadata(&home)?;
        if !s.is_dir() || s.uid() != getuid() || s.mode() & 0o022 != 0 {
            return cu_error("blocked", "cca 配置目录不能用于安全暂存更新");
        }
    }
    if create || path::lexists(&root) {
        cu_private(&root, true, create)?;
    }
    Ok(root)
}

fn cu_root(create: bool) -> R<String> {
    let root = path::join(&cu_base(create)?, cu().id);
    if create || path::lexists(&root) {
        cu_private(&root, true, create)?;
    }
    Ok(root)
}

/// `with ClientUpdateLock(path):`：拿不到马上报「另一项客户端更新正在进行」，不等。离开作用域时放开
pub struct ClientUpdateLock {
    _file: std::fs::File,
}

impl ClientUpdateLock {
    pub fn acquire(path: &str) -> R<ClientUpdateLock> {
        use std::os::fd::AsRawFd;
        cu_private(path, false, true)?;
        let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
        let s = file.metadata()?;
        if !s.is_file() || s.nlink() != 1 || s.uid() != getuid() || s.mode() & 0o077 != 0 {
            return cu_error("blocked", "客户端更新锁不安全");
        }
        // SAFETY: file 开着，fd 有效
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return cu_error("busy", "另一项客户端更新正在进行");
        }
        Ok(ClientUpdateLock { _file: file })
    }
}

fn cu_read_state() -> R<Dict> {
    let path = path::join(&cu_root(false)?, "state.json");
    if !path::lexists(&path) {
        return Ok(Dict::new());
    }
    cu_private(&path, false, false)?;
    match load_quiet(&path).and_then(|d| d.as_dict()) {
        Some(data) if data.get("v").is_some_and(|v| v.eq_py(&V::Int(1))) => Ok(data),
        _ => cu_error("blocked", "更新缓存格式无法识别，请重新检查"),
    }
}

fn cu_save_state(state: &Dict) -> R<()> {
    let path = path::join(&cu_root(true)?, "state.json");
    if path::lexists(&path) {
        cu_private(&path, false, false)?;
    }
    let data = state.copy();
    data.set("v", 1);
    save_json(&path, &V::Dict(data), Some(0o600), false)
}

fn cu_mode() -> &'static str {
    match load_quiet(&cfg_path()).and_then(|c| c.as_dict()).and_then(|c| c.get("client_updates")) {
        Some(V::Str(v)) if &*v == "off" => "off",
        _ => "auto",
    }
}

/// 允许自动更新（config.json 的 client_auto_install，默认不允许）：自动检查开着时才算数（不检查也就查不到要装的）
fn cu_auto_install() -> bool {
    let on = load_quiet(&cfg_path()).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("client_auto_install"), Some(V::Bool(true))));
    on && cu_mode() == "auto"
}

/// `plistlib.load(f).get(…)`：读 Info.plist。读不了是 OSError，格式不对是 ValueError（XML 写坏了是 ExpatError，原来哪里都没接），
/// 最外面不是字典时 `.get` 是 AttributeError
fn load_info(plist_path: &str) -> R<Vec<(String, P)>> {
    match plist::loads(&std::fs::read(plist_path)?)? {
        P::Dict(items) => Ok(items),
        _ => attr_error("属性列表最外面不是字典，没有 get"),
    }
}

fn info_get<'a>(info: &'a [(String, P)], k: &str) -> Option<&'a P> {
    info.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}

/// 在这几个目录里按名字找客户端（`cu_target` 没被环境变量指定时）
fn cu_target_in(bases: &[String]) -> R<String> {
    let cu = cu();
    for base in bases {
        for name in cu.names {
            let path = path::join(base, name);
            match load_info(&path::join_all(&[&path, "Contents", "Info.plist"])) {
                Ok(info) => {
                    if info_get(&info, "CFBundleIdentifier").and_then(|v| v.as_str()) == Some(cu.bundle) {
                        return Ok(path);
                    }
                }
                Err(e) if e.is_os_or_value() || e.is("TypeError") || e.is("AttributeError") => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(String::new())
}

/// 空的显式路径代表 shell 没找到客户端，不绕过它另找一个程序。
fn cu_target() -> R<String> {
    let cu = cu();
    if let Some(v) = getenv(cu.env) {
        return Ok(v);
    }
    if let Some(v) = getenv(cu.app_env).filter(|v| !v.is_empty()) {
        return Ok(v);
    }
    cu_target_in(&["/Applications".to_string(), path::join(&home(), "Applications")])
}

/// `(st_dev, st_ino, st_size, st_mtime_ns, st_ctime_ns)`
fn stat_fields(s: &std::fs::Metadata) -> [i64; 5] {
    [s.dev() as i64, s.ino() as i64, s.size() as i64, s.mtime() * 1_000_000_000 + s.mtime_nsec(), s.ctime() * 1_000_000_000 + s.ctime_nsec()]
}

fn cu_hash(path: &str) -> R<String> {
    #[cfg(test)]
    tests::on_hash(path);
    let before = std::fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 {
        return cu_error("blocked", "程序或安装包不是独立的普通文件");
    }
    let mut h = Sha256Stream::new();
    let mut f = std::fs::File::open(path)?;
    let opened = f.metadata()?;
    if (opened.dev(), opened.ino()) != (before.dev(), before.ino()) {
        return cu_error("needs_recheck", "程序或安装包正在变化，请重新检查");
    }
    let mut block = vec![0u8; 1024 * 1024];
    loop {
        let n = match f.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        h.update(&block[..n]);
        interrupted()?;
    }
    let after = f.metadata()?;
    drop(f);
    if stat_fields(&before) != stat_fields(&after) || stat_fields(&after) != stat_fields(&std::fs::symlink_metadata(path)?) {
        return cu_error("needs_recheck", "程序或安装包正在变化，请重新检查");
    }
    Ok(h.hexdigest())
}

/// `[st_dev, st_ino, st_size, st_mtime_ns, st_ctime_ns]`
fn cu_file_id(path: &str) -> R<V> {
    let s = std::fs::symlink_metadata(path)?;
    Ok(V::List(List::from_vec(stat_fields(&s).iter().map(|n| V::Int(*n)).collect())))
}

/// 当前客户端的快照：版本、构建号、关键文件的身份（full 时加上摘要）、程序目录自己的身份
fn cu_snapshot(path: &str, full: bool) -> R<Dict> {
    let cu = cu();
    if path.is_empty() || !path::isdir(path) {
        return cu_error("missing", format!("未找到当前 {} 桌面客户端", cu.label));
    }
    if !path.starts_with('/') || path::islink(path) {
        return cu_error("blocked", "客户端路径必须是实际的绝对目录");
    }
    let path = path::realpath(path);
    let before = std::fs::metadata(&path)?;
    let plist_path = path::join_all(&[&path, "Contents", "Info.plist"]);
    let plist_identity = cu_file_id(&plist_path)?;
    let info = load_info(&plist_path)?;
    let text_of = |k: &str| info_get(&info, k).and_then(|v| v.as_str());
    let (executable, version, build) = (text_of("CFBundleExecutable"), text_of("CFBundleShortVersionString"), text_of("CFBundleVersion"));
    let identified = text_of("CFBundleIdentifier") == Some(cu.bundle);
    let (executable, version, build) = match (executable, version, build) {
        (Some(e), Some(v), Some(b))
            if identified
                && !e.is_empty()
                && path::basename(e) == e
                && e != "."
                && e != ".."
                && dotted(v, 1, 5)
                && (if cu.dotted_build { dotted(b, 1, 5) } else { digits_1_12(b) }) =>
        {
            (e, v, b)
        }
        _ => return cu_error("blocked", "客户端标识、版本或程序布局无法识别"),
    };
    let names = ["Contents/Info.plist".to_string(), format!("Contents/MacOS/{executable}"), "Contents/Resources/app.asar".to_string(), "Contents/_CodeSignature/CodeResources".to_string()];
    let (hashes, identities) = (Dict::new(), Dict::new());
    for name in &names {
        let full_path = path::join(&path, name);
        if path::realpath(&full_path) != full_path {
            return cu_error("blocked", "客户端关键程序文件不能通过链接替换");
        }
        identities.set(name.as_str(), cu_file_id(&full_path)?);
        if full {
            hashes.set(name.as_str(), cu_hash(&full_path)?);
        }
    }
    let mut changed = !identities.at("Contents/Info.plist").eq_py(&plist_identity);
    if !changed {
        for (name, identity) in identities.items() {
            if !cu_file_id(&path::join(&path, &name))?.eq_py(&identity) {
                changed = true;
                break;
            }
        }
    }
    if changed {
        return cu_error("needs_recheck", "客户端在检查期间发生变化，请重新检查");
    }
    let after = std::fs::metadata(&path)?;
    let dir_fields = |s: &std::fs::Metadata| {
        let f = stat_fields(s);
        (f[0], f[1], f[3], f[4])
    };
    if dir_fields(&before) != dir_fields(&after) {
        return cu_error("needs_recheck", "客户端正在被其他更新器修改，请稍后重新检查");
    }
    let a = stat_fields(&after);
    Ok(dict! {
        "path" => path, "version" => version, "build" => build, "hashes" => hashes, "files" => identities,
        "device" => a[0], "inode" => a[1], "modified_ns" => a[3], "changed_ns" => a[4],
    })
}

/// Ctrl-C 来过吗：来过就是 KeyboardInterrupt（`py::signal::check`）。算摘要、解压这种不阻塞的长循环里隔一段看一次。
/// 单元测试里不看：那个标志整个进程只有一份，`py::signal` 自己的测试正在用，这边看一眼就把它那一笔拿走了
fn interrupted() -> R<()> {
    if cfg!(test) {
        return Ok(());
    }
    signal::check()
}

/// 从空环境构建：不携带账号 ID、代理、Cookie、证书覆盖或 shell 注入配置。
fn cu_environment(folder: &str) -> Vec<(String, String)> {
    [("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"), ("HOME", folder), ("CURL_HOME", folder), ("TMPDIR", folder), ("LANG", "C"), ("LC_ALL", "C")]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn cu_run(args: &[String], folder: &str, timeout: f64, env: Option<Vec<(String, String)>>) -> R<Done> {
    #[cfg(test)]
    if let Some(r) = tests::on_run(args, folder, timeout, env.as_deref()) {
        return r;
    }
    let env = env.filter(|e| !e.is_empty()).unwrap_or_else(|| cu_environment(folder));
    // subprocess 遇到参数、环境里带 \0 是 ValueError（embedded null byte），不是 OSError
    if args.iter().any(|a| a.contains('\0')) || env.iter().any(|(k, v)| k.contains('\0') || v.contains('\0')) {
        return Err(Fail::Value("embedded null byte".into()));
    }
    match Proc::new(args).stdin(In::Null).capture().env(env).timeout(timeout).run() {
        Ok(done) => Ok(done),
        Err(ProcError::Interrupted) => {
            interrupted()?; // Ctrl-C：原来是 KeyboardInterrupt 从这里冒出去
            cu_error("error", "更新校验命令失败或超时")
        }
        Err(ProcError::Os(_) | ProcError::Timeout { .. }) => cu_error("error", "更新校验命令失败或超时"),
    }
}

fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// `bytes.strip()`：两头的 ASCII 空白（空格、\t \n \r \x0b \x0c）
fn bytes_strip(b: &[u8]) -> &[u8] {
    let white = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = b.iter().position(|c| !white(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !white(c)).map_or(start, |i| i + 1);
    &b[start..end]
}

/// `re.search(r'[\x00-\x20\x7f\\]', url)`
fn url_has_bad_char(url: &str) -> bool {
    url.chars().any(|c| c <= ' ' || c == '\x7f' || c == '\\')
}

/// 更新地址合不合官方来源的规则（`url` 可能来自 state.json、更新源：不是字符串也算不合）
fn cu_url_v(url: &V, package: bool) -> R<String> {
    match url {
        V::Str(s) => cu_url(s, package),
        _ => cu_error("blocked", "更新地址不符合官方来源规则"),
    }
}

fn cu_url(url: &str, package: bool) -> R<String> {
    let valid = !url_has_bad_char(url)
        && urlsplit(url).is_ok_and(|p| {
            p.scheme == "https"
                && matches!(p.port(), Ok(None | Some(443)))
                && p.username().is_none_or(|u| u.is_empty())
                && p.password().is_none_or(|u| u.is_empty())
                && p.fragment.is_empty()
                && cu().url_ok(&p, package)
        });
    if !valid {
        return cu_error("blocked", "更新地址不符合官方来源规则");
    }
    Ok(url.to_string())
}

/// 一次下载最多用多久：不超过这次 check 剩下的时间
fn cu_time(seconds: i64) -> R<i64> {
    let Some(budget) = CU_BUDGET.with(|b| b.get()) else {
        return Ok(seconds);
    };
    let left = (budget - time()) as i64; // int()：向 0 取整
    if left < 20 {
        return cu_error("error", "这次检查的时间用完了，稍后会重新检查");
    }
    Ok(seconds.min(left))
}

/// `time.sleep(秒)`（Ctrl-C 时马上醒）
fn cu_sleep(secs: f64) -> R<()> {
    #[cfg(test)]
    if tests::on_sleep(secs) {
        return Ok(());
    }
    signal::sleep(secs)
}

/// 回应的头里 `Location:` 后面的地址（`open(header, encoding='iso-8859-1')` 一行一行看）
fn header_locations(raw: &[u8]) -> Vec<String> {
    let text: String = raw.iter().map(|b| *b as char).collect();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.to_lowercase().starts_with("location:") {
            out.push(text::strip(line.split_once(':').map_or("", |x| x.1)).to_string());
        }
    }
    out
}

/// curl 不自动跳转；每一跳先验证，三次重试不复用半包或持久 Cookie。empty 里的状态码（没有更新）原样返回
fn cu_download(url: &str, destination: &str, folder: &str, package: bool, expected_size: Option<&V>, empty: &[&str]) -> R<Option<String>> {
    #[cfg(test)]
    if let Some(r) = tests::on_download(url, destination, folder, package, expected_size, empty) {
        return r;
    }
    let header = path::join(folder, "response.headers");
    let initial = cu_url(url, package)?;
    let limit = if package { CU_MAX_PACKAGE } else { 2 * 1024 * 1024 };
    for attempt in 0..3 {
        let mut current = initial.clone();
        for _hop in 0..6 {
            let seconds = cu_time(if package { 300 } else { 30 })?;
            let args = [
                "/usr/bin/curl", "-q", "--silent", "--show-error", "--proto", "=https", "--proto-redir", "=https", "--proxy", "", "--noproxy", "*", "--no-netrc",
                "--no-netrc-optional", "--netrc-file", "/dev/null", "--connect-timeout", "15", "--max-time", &seconds.to_string(), "--max-filesize", &limit.to_string(),
                "--max-redirs", "0", "--dump-header", &header, "--output", destination, "--write-out", "%{http_code}", &current,
            ];
            let result = match cu_run(&strs(&args), folder, (seconds + 10) as f64, None) {
                Ok(result) => result,
                Err(e) if cu_status(&e) == "error" => break,
                Err(e) => return Err(e),
            };
            if result.rc != 0 {
                break;
            }
            // result.stdout.decode('ascii', 'replace').strip()
            let code: String = result.stdout.iter().map(|b| if b.is_ascii() { *b as char } else { '\u{fffd}' }).collect();
            let code = text::strip(&code);
            if empty.contains(&code) {
                return Ok(Some(code.to_string()));
            }
            if matches!(code, "301" | "302" | "303" | "307" | "308") {
                let locations = header_locations(&std::fs::read(&header)?);
                if locations.len() != 1 {
                    return cu_error("blocked", "官方更新地址返回了无法识别的跳转");
                }
                current = cu_url(&urljoin::urljoin(&current, &locations[0])?, package)?;
                continue;
            }
            if code != "200" {
                break;
            }
            let size = std::fs::metadata(destination)?.len();
            let size_ok = match expected_size {
                None | Some(V::None) => true,
                Some(want) => V::Int(size as i64).eq_py(want),
            };
            if !(0 < size && size <= limit) || !size_ok {
                return cu_error("error", "下载大小与官方元数据不符");
            }
            fs::chmod(destination, 0o600)?;
            return Ok(None);
        }
        if attempt < 2 {
            cu_sleep((attempt + 1) as f64)?;
        }
    }
    cu_error("error", "无法从官方来源下载更新，请稍后重试")
}

/// `platform.machine()`
fn platform_machine() -> String {
    // SAFETY: uts 清零后交给 uname 填；machine 是以 0 结尾的字符串
    unsafe {
        let mut uts: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut uts) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(uts.machine.as_ptr()).to_string_lossy().into_owned()
    }
}

/// `platform.mac_ver()[0]`：SystemVersion.plist 里的 ProductVersion；不是 macOS 是空串
fn mac_ver() -> R<String> {
    let mut file = "/System/Library/CoreServices/SystemVersion.plist".to_string();
    if !path::exists(&file) {
        match crate::py::proc::getenv("SDKROOT") {
            Some(root) if path::exists(&format!("{root}{file}")) => file = format!("{root}{file}"),
            _ => return Ok(String::new()),
        }
    }
    let info = load_info(&file)?;
    match info_get(&info, "ProductVersion") {
        Some(P::Str(s)) => Ok(s.clone()),
        Some(_) => type_error("ProductVersion 不是文字"),
        None => key_error("ProductVersion"),
    }
}

/// (处理器架构, macOS 版本)：arm64 / x64
fn cu_host() -> R<(String, String)> {
    #[cfg(test)]
    if let Some(host) = tests::on_host() {
        return Ok(host);
    }
    let mut arch = platform_machine();
    match cu_run(&strs(&["/usr/sbin/sysctl", "-n", "hw.optional.arm64"]), "/private/tmp", 10.0, None) {
        Ok(result) => {
            if result.rc == 0 && bytes_strip(&result.stdout) == b"1" {
                arch = "arm64".into();
            }
        }
        Err(e) if is_cu(&e) => {}
        Err(e) => return Err(e),
    }
    if !matches!(arch.as_str(), "arm64" | "x86_64") {
        return cu_error("blocked", "仅支持 macOS 上经过识别的处理器架构");
    }
    let system = mac_ver()?;
    if system.is_empty() {
        return cu_error("blocked", "仅支持 macOS 上经过识别的处理器架构");
    }
    Ok((if arch == "x86_64" { "x64".to_string() } else { arch }, system))
}

/// 版本号比大小用的六段：`(1, 2, 0, 0, 0, 0)`
fn cu_version(value: &V) -> R<Vec<PyInt>> {
    match value {
        V::Str(s) => cu_version_str(s),
        _ => cu_error("blocked", "官方版本元数据无法识别"),
    }
}

fn cu_version_str(value: &str) -> R<Vec<PyInt>> {
    if !dotted(value, 0, 5) {
        return cu_error("blocked", "官方版本元数据无法识别");
    }
    let mut out = value.split('.').map(PyInt::parse).collect::<R<Vec<_>>>()?;
    out.resize(6, PyInt::zero());
    Ok(out)
}

/// 构建号比大小：ChatGPT 是整数，Claude 和版本号一样是点分的
fn cu_build(value: &V) -> R<Vec<PyInt>> {
    // '.' in value
    let has_dot = match value {
        V::Str(s) => s.contains('.'),
        V::List(l) => l.has_str("."),
        V::Dict(d) => d.has("."),
        other => return type_error(format!("argument of type '{}' is not a container or iterable", type_name(other))),
    };
    if has_dot {
        return cu_version(value);
    }
    match value {
        V::Str(s) => Ok(vec![PyInt::parse(s)?]),
        other => type_error(format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", type_name(other))),
    }
}

fn cu_verifier() -> R<String> {
    let verifier = getenv("CCA_CLIENT_VERIFIER").unwrap_or_default();
    if verifier.is_empty() || !verifier.starts_with('/') || !path::isfile(&verifier) {
        return cu_error("blocked", "需要已验证的 cca 图形界面来校验官方安装包签名");
    }
    Ok(verifier)
}

/// `os.path.getsize(path) != metadata['length']`
fn size_differs(path: &str, metadata: &V) -> R<bool> {
    let size = std::fs::metadata(path)?.len();
    Ok(!V::Int(size as i64).eq_py(&sub(metadata, "length")?))
}

/// ChatGPT 的包验 Ed25519（固定公钥在 cca.app 里）；Claude 的更新源带 SHA-256，解压后还要过代码签名和 Gatekeeper
fn cu_verify_archive(path: &str, metadata: &V, folder: &str) -> R<String> {
    if get(metadata, "sha256")?.is_some_and(|v| v.truthy()) {
        if size_differs(path, metadata)? {
            return cu_error("blocked", "暂存安装包大小已变化，请重新检查");
        }
        let digest = cu_hash(path)?;
        if !V::from(digest.as_str()).eq_py(&sub(metadata, "sha256")?) {
            return cu_error("blocked", "官方安装包校验值不一致，未解压或安装");
        }
        return Ok(digest);
    }
    let verifier = cu_verifier()?;
    if size_differs(path, metadata)? {
        return cu_error("blocked", "暂存安装包大小已变化，请重新检查");
    }
    let (identity, digest) = (cu_file_id(path)?, cu_hash(path)?);
    let signature = path_str(&sub(metadata, "signature")?)?;
    let result = cu_run(&[verifier, "--verify-client-update".into(), path.to_string(), signature], folder, 120.0, None)?;
    if result.rc != 0 {
        return cu_error("blocked", "官方安装包签名校验失败，未解压或安装");
    }
    if !cu_file_id(path)?.eq_py(&identity) || cu_hash(path)? != digest {
        return cu_error("needs_recheck", "安装包在验签期间发生变化，请重新检查");
    }
    Ok(digest)
}

fn cu_token(stage: &Dict) -> R<String> {
    let payload = dict! { "nonce" => token_hex(32), "package_sha256" => at(stage, "package_sha256")?, "target" => at(stage, "target")?, "metadata" => at(stage, "metadata")? };
    Ok(sha256_hex(json::dumps(&V::Dict(payload), json::Fmt::new().compact().sort_keys())))
}

/// `re.search(r'^' + 这一行 + r'$', text, re.M)`：有没有哪一行正好是它
fn has_line(text: &str, line: &str) -> bool {
    text.split('\n').any(|l| l == line)
}

/// 签名、产品、版本和架构核验，不声称验证了未来版本的全部隔离行为。
fn cu_bundle(path: &str, folder: &str, metadata: Option<&V>) -> R<Dict> {
    let cu = cu();
    let snapshot = cu_snapshot(path, true)?;
    for command in [["/usr/bin/codesign", "--verify", "--deep", "--strict", path], ["/usr/sbin/spctl", "--assess", "--type", "execute", path]] {
        if cu_run(&strs(&command), folder, 120.0, None)?.rc != 0 {
            return cu_error("blocked", "客户端代码签名或系统安全评估未通过");
        }
    }
    let identity = cu_run(&strs(&["/usr/bin/codesign", "--display", "--verbose=4", path]), folder, 30.0, None)?;
    let said = String::from_utf8_lossy(&[identity.stdout.as_slice(), identity.stderr.as_slice()].concat()).into_owned();
    if identity.rc != 0 || !has_line(&said, &format!("TeamIdentifier={}", cu.team)) || !has_line(&said, &format!("Identifier={}", cu.bundle)) {
        return cu_error("blocked", "客户端签名团队或产品标识与官方程序不符");
    }
    let info = load_info(&path::join_all(&[path, "Contents", "Info.plist"]))?;
    let (arch, system) = cu_host()?;
    let executable = match info_get(&info, "CFBundleExecutable") {
        Some(P::Str(s)) => s.clone(),
        Some(_) => return type_error("CFBundleExecutable 不是文字"),
        None => return key_error("CFBundleExecutable"),
    };
    let binary = path::join_all(&[path, "Contents", "MacOS", &executable]);
    let result = cu_run(&strs(&["/usr/bin/lipo", "-archs", &binary]), folder, 30.0, None)?;
    // result.stdout.decode('ascii', 'replace').split()
    let listed: String = result.stdout.iter().map(|b| if b.is_ascii() { *b as char } else { '\u{fffd}' }).collect();
    let native = if arch == "x64" { "x86_64" } else { arch.as_str() };
    // info.get('LSMinimumSystemVersion', '')：不是文字的 cu_version 不认
    let minimum = || match info_get(&info, "LSMinimumSystemVersion") {
        None => cu_version_str(""),
        Some(P::Str(s)) => cu_version_str(s),
        Some(_) => cu_error("blocked", "官方版本元数据无法识别"),
    };
    if result.rc != 0 || !text::split_ws(&listed).contains(&native) || minimum()? > cu_version_str(&system)? {
        return cu_error("unsupported", "客户端不支持当前 macOS 版本或处理器");
    }
    if let Some(metadata) = metadata.filter(|m| m.truthy()) {
        let differs = |k: &str| -> R<bool> { Ok(!snapshot.at(k).eq_py(&sub(metadata, k)?)) };
        let mismatch = differs("version")? || differs("build")? || {
            let a = sub(metadata, "arch")?;
            !(a.eq_py(&V::from(arch.as_str())) || a.eq_py(&V::from("universal")))
        } || {
            let m = sub(metadata, "minimum_system")?;
            m.truthy() && minimum()? != cu_version(&m)?
        };
        if mismatch {
            return cu_error("blocked", "安装包内的产品版本或系统要求与官方列表不符");
        }
    }
    if !V::Dict(cu_snapshot(path, true)?).eq_py(&V::Dict(snapshot.clone())) {
        return cu_error("needs_recheck", "客户端在签名检查期间发生变化，请重新检查");
    }
    Ok(snapshot)
}

/// `{key: snapshot[key] for key in ('version', 'build', 'hashes')}`
fn cu_content(snapshot: &V) -> R<Dict> {
    let content = Dict::new();
    for key in ["version", "build", "hashes"] {
        content.set(key, sub(snapshot, key)?);
    }
    Ok(content)
}

fn cu_prepare_package(archive: &str, folder: &str, metadata: &V) -> R<Dict> {
    let app = extract::cu_extract(archive, folder)?;
    cu_content(&V::Dict(cu_bundle(&app, folder, Some(metadata))?))
}

/// `hmac.compare_digest(a, b)`（a 是核对过的 64 位十六进制）：b 不是 ASCII 的文字是 TypeError
fn compare_token(token: &str, other: &V) -> R<bool> {
    match other {
        V::Str(s) if s.is_ascii() => Ok(compare_digest(token.as_bytes(), s.as_bytes())),
        V::Str(_) => type_error("comparing strings with non-ASCII characters is not supported"),
        other => type_error(format!("unsupported operand types(s) or combination of types: 'str' and '{}'", type_name(other))),
    }
}

/// 安装凭据还作数吗（凭据对得上、客户端和暂存的包都没变）；返回要装的那一份（stage）。
/// token 多半是命令行给的文字；自动安装时是 state.json 里记的那个，什么类型都可能
fn cu_validate_plan(state: &Dict, token: &V) -> R<V> {
    let stage = state.get("stage");
    // re.fullmatch(r'[0-9a-f]{64}', token or '')：算假的当空串，别的不是文字是 TypeError
    let token = match token {
        V::Str(s) => s.to_string(),
        other if !other.truthy() => String::new(),
        other => return type_error(format!("expected string or bytes-like object, got '{}'", type_name(other))),
    };
    let valid = hex64(&token)
        && match &stage {
            Some(V::Dict(stage)) => compare_token(&token, &stage.get("token").unwrap_or_else(|| V::from("")))?,
            _ => false,
        };
    let (true, Some(V::Dict(stage))) = (valid, stage) else {
        return cu_error("needs_recheck", "安装确认已失效，请重新检查更新");
    };
    if cu().cli {
        return Ok(V::Dict(cli::cu_cli_validate(&stage)?));
    }
    let target = cu_snapshot(&cu_target()?, true)?;
    if !stage.get("target").is_some_and(|t| V::Dict(target.clone()).eq_py(&t)) {
        return cu_error("needs_recheck", "客户端已被其他更新器修改，请重新检查");
    }
    if cu_build(&sub2(&V::Dict(stage.clone()), "metadata", "build")?)? <= cu_build(&target.at("build"))? {
        return cu_error("needs_recheck", "不会覆盖相同或更新的客户端版本");
    }
    let archive = path::join(&cu_root(false)?, "package.zip");
    cu_private(&archive, false, false)?;
    let digest = V::from(cu_hash(&archive)?);
    if !stage.get("package_sha256").is_some_and(|h| h.eq_py(&digest)) {
        return cu_error("needs_recheck", "暂存安装包已变化，请重新检查");
    }
    Ok(V::Dict(stage))
}

/// `{k: v for k, v in snapshot.items() if k != 'hashes'}`：不是字典是 AttributeError
fn cu_light(snapshot: &V) -> R<V> {
    match snapshot {
        V::Dict(d) => Ok(V::Dict(Dict::from_pairs(d.items().into_iter().filter(|(k, _)| &**k != "hashes")))),
        other => attr_error(format!("'{}' object has no attribute 'items'", type_name(other))),
    }
}

/// 一种客户端给界面看的那一行
fn cu_view(state: &Dict, problem: Option<&(String, String)>) -> R<Dict> {
    let cu = cu();
    if cu.cli {
        return cli::cu_cli_view(state, problem);
    }
    let target = cu_target()?;
    let row = dict! {
        "id" => cu.id, "label" => cu.label, "kind" => "desktop", "path" => if target.is_empty() { V::None } else { V::from(target.as_str()) },
        "installed_version" => V::None, "available_version" => V::None, "staged_version" => V::None,
        "package_version" => V::None, "package_build" => V::None, "install_token" => V::None, "can_install" => false,
        "status" => state.get("status").unwrap_or_else(|| V::from("unchecked")), "message" => state.get("message").unwrap_or_else(|| V::from("尚未检查官方更新")),
        "checked_epoch" => state.get("checked_epoch"), "auto_install_pending" => false,
    };
    let fill = || -> R<()> {
        let snapshot = cu_snapshot(&cu_target()?, false)?;
        row.set("installed_version", snapshot.at("version"));
        if let Some(V::Dict(metadata)) = state.get("metadata") {
            row.set("available_version", metadata.get("version"));
        }
        if let Some(V::Dict(stage)) = state.get("stage") {
            let metadata = at(&stage, "metadata")?;
            let (version, build) = (sub(&metadata, "version")?, sub(&metadata, "build")?);
            row.set("staged_version", version.clone());
            row.set("package_version", version.clone());
            row.set("package_build", build.clone());
            let archive = path::join(&cu_root(false)?, "package.zip");
            cu_private(&archive, false, false)?;
            let staged_for = stage.get("target").unwrap_or_else(|| V::Dict(Dict::new()));
            let moved = !cu_light(&V::Dict(snapshot.clone()))?.eq_py(&cu_light(&staged_for)?) || {
                let now = cu_file_id(&archive)?;
                !stage.get("archive_identity").is_some_and(|id| now.eq_py(&id))
            };
            if moved {
                row.set("status", "needs_recheck");
                row.set("message", "客户端版本或程序内容已变化，请重新检查更新");
            } else if matches!(state.get("status"), Some(V::Str(s)) if matches!(&*s, "staged" | "running")) && cu_build(&build)? > cu_build(&snapshot.at("build"))? {
                row.set("install_token", at(&stage, "token")?);
                row.set("can_install", true);
                // 允许自动更新、上次要装时客户端开着（cu_auto）：等它退出
                if state.truthy("auto_wait") && cu_auto_install() {
                    row.set("auto_install_pending", true);
                    row.set("message", format!("已下载好 {}：{} 还开着，退出后自动安装", version.py_str(), cu.label));
                }
            }
        }
        Ok(())
    };
    match fill() {
        Ok(()) => {}
        Err(e) if caught(&e) => {
            let (status, message) = cu_parts(&e).unwrap_or(("blocked", "无法安全读取客户端状态"));
            row.set("status", status);
            row.set("message", message);
        }
        Err(e) => return Err(e),
    }
    if path::lexists(&path::join_all(&[&cca_home(), "client-updates", cu.id, "transaction.json"])) {
        row.set("status", "needs_recheck");
        row.set("message", "上次安装尚未完成，请运行检查以安全恢复");
        row.set("install_token", V::None);
        row.set("can_install", false);
        row.set("auto_install_pending", false);
    }
    if let Some((status, message)) = problem {
        row.set("status", status.as_str());
        row.set("message", message.as_str());
        row.set("install_token", V::None);
        row.set("can_install", false);
        row.set("auto_install_pending", false);
    }
    Ok(row)
}

/// 下次该后台检查的时间：App 睡到这时候再来问，不用隔一会儿起一个进程。关了自动检查、一个客户端都没装时是 None；
/// 有下载好、等桌面客户端退出后自动安装的（auto_install_pending），最多 CU_AUTO_RETRY 以后再来
fn cu_next_check(rows: &[Dict]) -> R<V> {
    // times = [r.get('checked_epoch') or 0 for r in rows if r.get('status') != 'missing']
    let times: Vec<V> = rows
        .iter()
        .filter(|r| !matches!(r.get("status"), Some(V::Str(s)) if &*s == "missing"))
        .map(|r| r.get("checked_epoch").filter(|t| t.truthy()).unwrap_or(V::Int(0)))
        .collect();
    if times.is_empty() || cu_mode() == "off" {
        return Ok(V::None);
    }
    // min(times)：一样大的留前面那个（整数和小数写出来不一样）。有不是数的：比大小或者下面那个加法是 TypeError（这里没人接）
    let mut least = Num::of(&times[0], "+")?;
    for t in &times[1..] {
        let n = Num::of(t, "<")?;
        if n.lt(&least) {
            least = n;
        }
    }
    let due = least.add_int(CU_CHECK_INTERVAL);
    if rows.iter().any(|r| r.truthy("auto_install_pending")) {
        let retry = Num::Float(time() + CU_AUTO_RETRY as f64);
        return Ok(if retry.lt(&due) { retry } else { due }.to_v());
    }
    Ok(due.to_v())
}

/// 这次操作遇到的问题：{客户端 id: (status, message)}，先后照记下的顺序
#[derive(Default)]
struct Problems(Vec<(&'static str, (String, String))>);

impl Problems {
    fn set(&mut self, id: &'static str, problem: (String, String)) {
        match self.0.iter_mut().find(|(k, _)| *k == id) {
            Some(slot) => slot.1 = problem,
            None => self.0.push((id, problem)),
        }
    }

    fn get(&self, id: &str) -> Option<&(String, String)> {
        self.0.iter().find(|(k, _)| *k == id).map(|(_, p)| p)
    }
}

/// 所有客户端的状态文档；problems 是 {客户端 id: 这次操作遇到的问题}
fn cu_doc(problems: &Problems) -> R<Dict> {
    let current = cu();
    let mut rows = Vec::new();
    for client in CU_CLIENTS {
        set_cu(client);
        let mut problem = problems.get(client.id).cloned();
        let state = match cu_read_state() {
            Ok(state) => state,
            Err(e) if is_cu(&e) => {
                if problem.is_none() {
                    problem = cu_parts(&e).map(|(s, m)| (s.to_string(), m.to_string()));
                }
                Dict::new()
            }
            Err(e) => return Err(e),
        };
        rows.push(cu_view(&state, problem.as_ref())?);
    }
    set_cu(current);
    let generated = time();
    let (mode, auto_install) = (cu_mode(), cu_auto_install());
    let next = cu_next_check(&rows)?;
    Ok(dict! {
        "version" => 1, "kind" => "client_updates", "generated_epoch" => generated,
        "mode" => mode, "auto_install" => auto_install, "next_check_epoch" => next, "clients" => List::from_vec(rows.iter().map(V::from).collect()),
    })
}

/// `with tempfile.TemporaryDirectory(prefix=…, dir=…) as folder:`：做完（成不成都）连里面的东西一起删掉
fn with_tempdir<T>(prefix: &str, dir: &str, body: impl FnOnce(&str) -> R<T>) -> R<T> {
    let tmp = TemporaryDirectory::new(prefix, dir)?;
    let result = body(&tmp.name);
    tmp.cleanup()?;
    result
}

/// 检查一种客户端：到时间了（或者 force）就问官方有没有新版，有就下载、验证、暂存。返回的还是那个 state
fn cu_check(state: &Dict, force: bool) -> R<()> {
    let root = cu_root(true)?;
    if !force {
        // time.time() - state.get('checked_epoch', 0) < CU_CHECK_INTERVAL
        if cu_mode() == "off" || time() - Num::of(&state.get("checked_epoch").unwrap_or(V::Int(0)), "-")?.f()? < CU_CHECK_INTERVAL as f64 {
            return Ok(());
        }
    }
    if cu().cli {
        return cli::cu_cli_check(state, &root);
    }
    let target = cu_snapshot(&cu_target()?, true)?;
    state.set("checked_epoch", time());
    let body = || -> R<()> {
        cu_save_state(state)?; // 先记下检查时间：中途被杀掉时，下一轮不会马上从头再下载一遍
        with_tempdir("check-", &root, |folder| {
            let target_v = V::Dict(target.clone());
            if !V::Dict(cu_bundle(&target.at("path").py_str(), folder, None)?).eq_py(&target_v) {
                return cu_error("needs_recheck", "客户端正在被其他更新器修改，请重新检查");
            }
            let Some(metadata) = feed::cu_metadata(&target, folder)? else {
                state.pop("stage");
                state.pop("metadata");
                state.set("status", "current");
                state.set("message", "当前客户端已是最新版本");
                return Ok(());
            };
            let metadata_v = V::Dict(metadata.clone());
            state.set("metadata", metadata.clone());
            if cu_build(&metadata.at("build"))? <= cu_build(&target.at("build"))? {
                state.pop("stage");
                state.set("status", "current");
                state.set("message", "当前客户端已是相同或更新版本");
                return Ok(());
            }
            let stage = state.get("stage");
            if is(cu(), &CU_CODEX) {
                cu_verifier()?;
            }
            if let Some(V::Dict(stage)) = &stage {
                if stage.get("metadata").is_some_and(|m| m.eq_py(&metadata_v)) {
                    let archive = path::join(&root, "package.zip");
                    cu_private(&archive, false, false)?;
                    let digest = V::from(cu_hash(&archive)?);
                    if stage.get("package_sha256").is_some_and(|h| h.eq_py(&digest)) {
                        cu_verify_archive(&archive, &metadata_v, folder)?;
                        if !stage.get("target").is_some_and(|t| target_v.eq_py(&t)) {
                            stage.set("target", target.clone());
                            stage.set("token", cu_token(stage)?);
                        }
                        stage.set("archive_identity", cu_file_id(&archive)?);
                        state.set("status", "staged");
                        state.set("message", "官方更新已暂存，等待确认安装");
                        return Ok(());
                    }
                }
            }
            let archive = path::join(folder, "package.zip");
            cu_download(&path_str(&metadata.at("url"))?, &archive, folder, true, Some(&metadata.at("length")), &[])?;
            let digest = cu_verify_archive(&archive, &metadata_v, folder)?;
            let package = cu_prepare_package(&archive, folder, &metadata_v)?;
            if !V::Dict(cu_snapshot(&cu_target()?, true)?).eq_py(&target_v) {
                return cu_error("needs_recheck", "下载期间客户端发生变化，请重新检查");
            }
            let stage = dict! { "metadata" => metadata.clone(), "target" => target.clone(), "package_sha256" => digest, "package" => package };
            stage.set("token", cu_token(&stage)?);
            let destination = path::join(&root, "package.zip");
            if path::lexists(&destination) {
                cu_private(&destination, false, false)?;
            }
            std::fs::rename(&archive, &destination)?;
            stage.set("archive_identity", cu_file_id(&destination)?);
            state.set("stage", stage);
            state.set("status", "staged");
            state.set("message", "官方更新已暂存，等待确认安装");
            Ok(())
        })
    };
    let result = body();
    if let Err(e) = &result {
        if let Some((status, message)) = cu_parts(e) {
            state.set("status", status);
            state.set("message", message);
            state.pop("stage");
        }
    }
    cu_save_state(state)?; // finally
    result
}

/// 允许自动更新时（cu_auto_install），检查完马上把查到的更新装上：打开开关就是同意，不再问。桌面客户端开着时不装、不算出错：
/// 记下 auto_wait，等它退出（cu_next_check 把下次检查提前到 CU_AUTO_RETRY 以后，App 看到客户端退出时也来查一次）；终端工具只装
/// 官方安装器装的（npm、Homebrew 装的要用户自己的环境，照旧提示在终端里运行什么）；claude 平时会自己更新，已经不比现在的新就不装。
/// 装的时候出错照常往上传（这次检查的结果里说原因）
fn cu_auto(state: &Dict) -> R<()> {
    let cu = cu();
    let Some(V::Dict(stage)) = state.get("stage") else { return Ok(()) };
    if !stage.truthy("token") {
        return Ok(());
    }
    let stage_v = V::Dict(stage.clone());
    let version = sub2(&stage_v, "metadata", "version")?;
    if cu.cli {
        if !matches!(stage.get("method"), Some(V::Str(m)) if &*m == "self") {
            return Ok(());
        }
        match cli::cu_cli_validate(&stage) {
            Ok(_) => {}
            Err(e) if is_cu(&e) => return Ok(()), // 装的位置变了、它已经自己更新过了：照 status 说的，下次检查重新算
            Err(e) => return Err(e),
        }
    } else if !matches!(state.get("status"), Some(V::Str(s)) if matches!(&*s, "staged" | "running")) {
        return Ok(());
    }
    let install = || -> R<bool> {
        if !cu.cli {
            install::cu_no_processes(&[sub2(&stage_v, "target", "path")?])?; // 先只看开没开着：开着的不用先校验几百 MB 的安装包
        }
        if !cu_auto_install() {
            return Ok(false); // 装之前再看一眼：这一会儿用户关掉了（clients auto-install off、mode off）就不装
        }
        install::cu_install(state, &at(&stage, "token")?)?;
        Ok(true)
    };
    match install() {
        Ok(true) => {}
        Ok(false) => return Ok(()),
        Err(e) if is_cu(&e) => {
            if cu_status(&e) == "running" {
                // 开着（或者刚好又打开了）：等它退出
                if !state.truthy("auto_wait") {
                    state.set("auto_wait", true);
                    cu_save_state(state)?;
                }
                return Ok(());
            }
            state.pop("auto_wait"); // 别的原因装不了：不再每 15 分钟试一次，照常 6 小时后再检查
            cu_save_state(state)?;
            return Err(e);
        }
        Err(e) => return Err(e),
    }
    state.pop("auto_wait");
    state.set("message", if cu.cli { format!("已自动更新到 {}，下次启动时用新版本", version.py_str()) } else { format!("已自动安装官方更新 {}", version.py_str()) });
    cu_save_state(state)
}

/// 安装凭据是哪个客户端的；都对不上时停在第一个，由 cu_validate_plan 说凭据失效
fn cu_token_client(token: &str) -> R<()> {
    if hex64(token) {
        for client in CU_CLIENTS {
            set_cu(client);
            let stage = match cu_read_state() {
                Ok(state) => state.get("stage"),
                Err(e) if is_cu(&e) => continue,
                Err(e) => return Err(e),
            };
            if let Some(V::Dict(stage)) = stage {
                // hmac.compare_digest(token, str(stage.get('token', '')))
                let theirs = stage.get("token").unwrap_or_else(|| V::from("")).py_str();
                if compare_token(token, &V::from(theirs))? {
                    return Ok(());
                }
            }
        }
    }
    set_cu(CU_CLIENTS[0]);
    Ok(())
}

/// 这次操作里出的事记成 (status, message)；不是 ClientUpdateError 的统称「无法安全处理客户端更新状态」
fn cu_problem(e: &Fail) -> (String, String) {
    match cu_parts(e) {
        Some((status, message)) => (status.to_string(), message.to_string()),
        None => ("blocked".to_string(), "无法安全处理客户端更新状态".to_string()),
    }
}

/// 没有待安装的更新时不留着安装包（一个几百 MB）；删不掉也不影响这次操作
fn cu_drop_package() -> R<()> {
    let drop = || -> R<()> {
        let path = path::join(&cu_root(false)?, "package.zip");
        if path::lexists(&path) {
            cu_private(&path, false, false)?;
            std::fs::remove_file(&path)?;
        }
        Ok(())
    };
    match drop() {
        Err(e) if !(is_cu(&e) || e.is_os()) => Err(e),
        _ => Ok(()),
    }
}

/// 一个一个检查（只在 manager 锁内调用）：一个客户端出问题不影响另一个；没装的不算问题
fn cu_check_all(force: bool, problems: &mut Problems) -> R<()> {
    CU_BUDGET.with(|b| b.set(Some(time() + CU_CHECK_SECONDS)));
    // 终端工具先查：只是一个小请求，不被桌面客户端的大下载占完时间
    for client in CU_CLIENTS.iter().filter(|c| c.cli).chain(CU_CLIENTS.iter().filter(|c| !c.cli)) {
        set_cu(client);
        if cu_target()?.is_empty() && !path::lexists(&path::join_all(&[&cu_base(false)?, client.id, "transaction.json"])) {
            continue;
        }
        let one = || -> R<()> {
            let root = cu_root(true)?;
            let state = cu_read_state()?;
            {
                let _lock = ClientUpdateLock::acquire(&path::join(&root, "install.lock"))?;
                install::cu_recover(&state)?;
            }
            cu_check(&state, force)?;
            if cu_auto_install() {
                // 允许自动更新：查到了就装（桌面客户端开着时等它退出）。每个客户端查完现读：检查途中关掉了就不装
                cu_auto(&state)?;
            }
            if !state.truthy("stage") {
                cu_drop_package()?;
            }
            Ok(())
        };
        match one() {
            Ok(()) => {}
            Err(e) if caught(&e) => problems.set(client.id, cu_problem(&e)),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Ctrl-C：原来 Python 收到 SIGINT 是 KeyboardInterrupt，沿途的 `finally`、`with` 照常收尾（下到一半的安装包连临时目录一起删掉、
/// 解压到一半的候选程序删掉、检查时间存进 state.json），然后才结束。这里的处理函数只记一笔，等外部命令、睡眠、算摘要的地方看到了
/// 就当成那个异常往上传。进来时 SIGINT 被忽略着（在后台跑）就不接，和 Python 一样。只有 check、install 装：别的操作里等
/// `sync.lock` 是不限时的，装了反而 Ctrl-C 不掉
fn catch_interrupt() {
    // SAFETY: 只是读 SIGINT 现在的处理方式；old 是这里的局部变量
    let is_default = unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_DFL
    };
    if is_default {
        signal::raise_on(libc::SIGINT, "KeyboardInterrupt");
    }
}

/// 没人接的 KeyboardInterrupt：Python 说完调用栈以后把 SIGINT 改回默认、再给自己发一次，让上一层看到“是被 Ctrl-C 结束的”
fn die_interrupted() -> ! {
    crate::py::io::finish();
    err!("KeyboardInterrupt");
    crate::py::io::finish();
    // SAFETY: 把 SIGINT 改回默认处理再发给自己：进程就此结束
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::kill(libc::getpid(), libc::SIGINT);
    }
    std::process::exit(130)
}

/// client-updates [status|check|mode|auto-install|plan|install] [参数]
pub fn cmd_client_updates(args: &[String]) -> R<()> {
    arity("cmd_client_updates", args, 0, 2)?;
    let mut printed = String::new();
    let result = client_updates(arg(args, 0, "status"), arg(args, 1, ""), &mut printed);
    out_raw!("{printed}");
    match result {
        Err(e) if e.is("KeyboardInterrupt") => die_interrupted(),
        other => other,
    }
}

/// 子命令的本体。要打到 stdout 的攒在 printed 里（原来都是做完事以后才 print 的；这样单元测试也拿得到），
/// 要说到 stderr 的在返回的 `Fail::Die` 里
fn client_updates(action: &str, arg: &str, printed: &mut String) -> R<()> {
    if !matches!(action, "status" | "check" | "mode" | "auto-install" | "plan" | "install") {
        return die("未知客户端更新操作");
    }
    let mut problems = Problems::default();
    let mut plan: Option<String> = None;
    let body = |problems: &mut Problems, plan: &mut Option<String>| -> R<()> {
        match action {
            "mode" => {
                if !matches!(arg, "auto" | "off") {
                    return cu_error("blocked", "客户端更新模式只能是 auto 或 off");
                }
                let _lock = sync_lock()?;
                // c['client_updates'] = arg：config.json 最外面不是对象时是 TypeError
                let c = match load_json(&cfg_path())? {
                    None => Dict::new(),
                    Some(V::Dict(c)) => c,
                    Some(other) => return type_error(format!("'{}' object does not support item assignment", type_name(&other))),
                };
                c.set("client_updates", arg);
                if arg == "off" {
                    c.pop("client_auto_install"); // 不自动检查就查不到要装的：允许自动更新一起关掉
                }
                save_json(&cfg_path(), &V::Dict(c), None, true)?;
            }
            "auto-install" => {
                if !matches!(arg, "on" | "off") {
                    return cu_error("blocked", "允许自动更新只能是 on 或 off");
                }
                let _lock = sync_lock()?;
                // c.update(…) / c.pop(…, None)：不是对象时没有这个方法（AttributeError，没人接）；列表有 pop，但不收两个参数（TypeError）
                let c = match load_json(&cfg_path())? {
                    None => Dict::new(),
                    Some(V::Dict(c)) => c,
                    Some(V::List(_)) if arg == "off" => return type_error("pop expected at most 1 argument, got 2"),
                    Some(other) => return attr_error(format!("'{}' object has no attribute '{}'", type_name(&other), if arg == "on" { "update" } else { "pop" })),
                };
                if arg == "on" {
                    c.set("client_auto_install", true); // 自动安装要先自动检查
                    c.set("client_updates", "auto");
                } else {
                    c.pop("client_auto_install");
                }
                save_json(&cfg_path(), &V::Dict(c), None, true)?;
            }
            "plan" => {
                cu_token_client(arg)?;
                let stage = cu_validate_plan(&cu_read_state()?, &V::from(arg))?;
                let cu = cu();
                let note = if cu.cli { format!("用 {} update 更新（它自己校验和替换）。开着的会话不受影响，下次启动时用新版本。", cu.command) } else { String::new() };
                let fields = [
                    ("CU_VERSION", sub2(&stage, "metadata", "version")?),
                    ("CU_INSTALLED", sub2(&stage, "target", "version")?),
                    ("CU_TARGET", sub2(&stage, "target", "path")?),
                    ("CU_LABEL", V::from(cu.label)),
                    ("CU_NOTE", V::from(note)),
                ];
                // sh(CU_VERSION=…, …)：一行一个，值照 str() 再加引号
                *plan = Some(fields.iter().map(|(k, v)| format!("{}={}\n", k, text::sh_quote(&v.py_str()))).collect());
            }
            "check" | "install" => {
                if action == "check" && !matches!(arg, "" | "force") {
                    return cu_error("blocked", "检查更新的参数无法识别");
                }
                if !cfg!(test) {
                    catch_interrupt();
                }
                let _lock = ClientUpdateLock::acquire(&path::join(&cu_base(true)?, "manager.lock"))?;
                if action == "check" {
                    cu_check_all(arg == "force", problems)?;
                } else {
                    cu_token_client(arg)?;
                    cu_root(true)?;
                    install::cu_install(&cu_read_state()?, &V::from(arg))?;
                    cu_drop_package()?;
                }
            }
            _ => {}
        }
        Ok(())
    };
    match body(&mut problems, &mut plan) {
        Ok(()) => {}
        Err(e) if caught(&e) => {
            if action == "plan" {
                return die(cu_problem(&e).1);
            }
            problems.set(cu().id, cu_problem(&e));
        }
        Err(e) => return Err(e),
    }
    if let Some(plan) = plan {
        printed.push_str(&plan);
        return Ok(());
    }
    printed.push_str(&json::dumps(&V::Dict(cu_doc(&problems)?), json::LINE));
    printed.push('\n');
    if !problems.0.is_empty() && action != "status" {
        // sys.stderr.write(各条说明 + '\n'); sys.exit(1)
        return die(problems.0.iter().map(|(_, p)| p.1.as_str()).collect::<Vec<_>>().join("\n"));
    }
    Ok(())
}
