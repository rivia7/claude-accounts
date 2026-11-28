//! 三种工具（Claude Code、Codex、Antigravity）各自的账号表、目录和叫法：原来的 `Tool` / `CodexTool` / `AgyTool`。
//!
//! 取值不同的属性是 `Tool` 的字段，做法不同的方法（`email`、`run_args`、`settings_problems`……）按 `kind` 分派，
//! 写在各自的模块里（见 `impl Tool` 的其余部分）。当前管的是哪个工具（原来的全局变量 `T`）用 `t()` 取，
//! `Using::tool(…)` 临时换一个（原来的 `with using(工具)`）。

use super::base::{AGY_RESTRICTED, CCA_HOME, CLAUDE_RESTRICTED, CODEX_RESTRICTED, HOME};
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::{die, R};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Claude,
    Codex,
    Agy,
}

pub struct Tool {
    pub kind: Kind,
    pub name: &'static str,
    pub label: &'static str,
    /// 提示里的命令前缀：`cca`、`cca codex`、`cca agy`
    pub cmd: &'static str,
    /// 账号表
    pub table: String,
    /// 账号目录都在 sroot/accounts/ 下
    pub sroot: String,
    /// 共享主目录（~/.claude、~/.codex、~/.gemini）
    pub master: String,
    /// 主目录里还没有也先建好、并在账号目录里链接的共享目录
    pub precreate: &'static [&'static str],
    /// 共享条目所在的子目录（相对主目录；空的是只共享顶层的条目）
    pub nests: &'static [&'static str],
    /// 共享配置（三方合并）：主目录里还没有时也记进基线
    pub config_names: &'static [&'static str],
    /// 账号目录里交给工具的配置目录（CLAUDE_CONFIG_DIR、CODEX_HOME、--gemini_dir）
    pub sub: &'static str,
    /// 提示里“模型写在哪”
    pub settings_disp: &'static str,
    /// config.json 里这个工具的 ports、last、api_ids…… 放在哪：None 是顶层
    pub state_key: Option<&'static str>,
    /// 钥匙串里第三方 API key 的服务名前缀
    pub kc_prefix: &'static str,
    /// 官方账号记下备查的 API key 的服务名前缀
    pub kc_ref_prefix: &'static str,
    /// 路由器端口池里的账号名前缀
    pub pool_prefix: &'static str,
    /// ~/Applications 里启动器的名字前缀
    pub launcher_prefix: &'static str,
    pub launcher_id: &'static str,
    pub app_label: &'static str,
    /// 桌面 App 主进程命令行里的路径（正则）；没有桌面 App 的是 None
    pub app_proc: Option<&'static str>,
    /// 启动器调用的 cca 子命令
    pub app_cmd: &'static str,
    pub api_fields: &'static [&'static str],
    pub url_example: &'static str,
    /// 出口在受限地区时的提示
    pub region_name: &'static str,
    /// 不能用的地区（地区代码、名字），照这家官方的名单（base.rs 的 CLAUDE_RESTRICTED 这些）
    pub restricted: &'static [(&'static str, &'static str)],
    /// 出口检测：经账号的端口取这个网站看到的 IP 和地区
    pub trace_host: &'static str,
    pub web_url: &'static str,
    pub billing_url: &'static str,
    /// 登录页的域名（补 login_hint）
    pub login_hosts: &'static [&'static str],
    pub login_paths: &'static [&'static str],
    /// 邮箱助手认哪几种登录邮件
    pub mail_rules: &'static [&'static str],
    /// 查不了用量时的提示
    pub product: &'static str,
    /// cca usage --json 的窗口
    pub usage_windows: &'static [&'static str],
    pub update_cmd: &'static str,
    /// 官方安装脚本
    pub install_url: &'static str,
    pub install_shell: &'static str,
    pub has_app: bool,
    pub has_api: bool,
    /// 图形界面显示不显示这个工具的用量（_gui state 的 features.usage、can.usage）。三种都显示：agy 的格式不一样（几组额度，
    /// --json 的 quota_groups，没有 5 小时、本周这些窗口），每周用量每组各记一份（usage_weeks_groups；usage_weeks 是 null）
    pub has_usage: bool,
    pub has_cost: bool,
}

pub const CLAUDE_SHARED: &[&str] = &[
    "projects",
    "skills",
    "commands",
    "agents",
    "plugins",
    "todos",
    "tasks",
    "plans",
    "file-history",
    "ide",
    "settings.json",
    "settings.local.json",
    "history.jsonl",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    "AGENTS.override.md",
    "keybindings.json",
    "rules",
    "output-styles",
    "themes",
    "workflows",
    "agent-memory",
    "agent-memory-local",
    "memory",
    "memories",
    "prompts",
    "hooks",
];
pub const CLAUDE_SHARED_CONFIG: &[&str] = &[
    "settings.json",
    "settings.local.json",
    "keybindings.json",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    "AGENTS.override.md",
];
pub const PRECREATE: &[&str] =
    &["projects", "skills", "commands", "agents", "plugins", "todos", "tasks", "plans", "file-history", "ide"];
/// ~/.claude.json 里与账号无关、跨账号同步的键；其余（oauthAccount、订阅、功能开关缓存等）留在各账号
pub const SHARED_KEYS: &[&str] = &[
    "projects",
    "mcpServers",
    "githubRepoPaths",
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "installMethod",
    "autoUpdates",
    "autoUpdatesProtectedForNative",
    "theme",
    "editorMode",
    "verbose",
    "preferredNotifChannel",
    "autoCompactEnabled",
    "autoConnectIde",
    "autoInstallIdeExtension",
    "diffTool",
    "diffSidebarOpen",
    "showSpinnerTree",
    "fileCheckpointingEnabled",
    "terminalProgressBarEnabled",
    "respectGitignore",
    "hasIdeOnboardingBeenShown",
    "hasCompletedClaudeInChromeOnboarding",
    "claudeInChromeDefaultEnabled",
    "shiftEnterKeyBindingInstalled",
    "optionAsMetaKeyInstalled",
    "hasUsedBackslashReturn",
    "lastReleaseNotesSeen",
    "tipsHistory",
    "officialMarketplaceAutoInstallAttempted",
    "officialMarketplaceAutoInstalled",
];
pub const CODEX_SHARED: &[&str] = &[
    "config.toml",
    ".env",
    "AGENTS.md",
    "AGENTS.override.md",
    "hooks.json",
    "history.jsonl",
    "session_index.jsonl",
    "sessions",
    "archived_sessions",
    "memories",
    "skills",
    "rules",
    "prompts",
    "plugins",
    "agents",
    "themes",
];
pub const CODEX_PRECREATE: &[&str] = &["sessions", "archived_sessions", "memories", "skills", "rules", "plugins"];
pub const CODEX_SHARED_CONFIG: &[&str] = &["config.toml", "hooks.json", ".env", "AGENTS.md", "AGENTS.override.md"];
/// cca 启动的 codex（以及它的 app-server）都加上：连不上账号的代理时不许改走 macOS 的系统代理，也不许系统代理排在环境变量前面
pub const CODEX_FLAGS: &[&str] = &["--disable", "system_proxy_fallback", "--disable", "respect_system_proxy"];
/// 第三方 API 账号：cca 经 -c 定义的模型服务名
pub const CODEX_PROVIDER: &str = "cca";
/// cca 经 app-server 登录、查用量时报的客户端名：和 codex login 发出的请求一样
pub const CODEX_CLIENT: &str = "codex_cli_rs";
pub const AGY_SHARED: &[&str] = &[
    "GEMINI.md",
    "AGENTS.md",
    "antigravity-cli/conversations",
    "antigravity-cli/brain",
    "antigravity-cli/annotations",
    "antigravity-cli/implicit",
    "antigravity-cli/presence",
    "antigravity-cli/history.jsonl",
    "antigravity-cli/settings.json",
    "antigravity-cli/keybindings.json",
    "antigravity-cli/cache/last_conversations.json",
    "config/mcp_config.json",
    "config/config.json",
    "config/hooks.json",
    "config/skills.json",
    "config/plugins.json",
    "config/projects",
    "config/skills",
    "config/rules",
    "config/workflows",
    "config/agents",
    "config/plugins",
    "config/GEMINI.md",
    "config/AGENTS.md",
];
pub const AGY_NESTS: &[&str] = &["antigravity-cli", "antigravity-cli/cache", "config"];
pub const AGY_PRECREATE: &[&str] = &[
    "antigravity-cli/conversations",
    "antigravity-cli/brain",
    "antigravity-cli/annotations",
    "antigravity-cli/implicit",
    "antigravity-cli/presence",
    "config/projects",
];
pub const AGY_SHARED_CONFIG: &[&str] = &[
    "GEMINI.md",
    "AGENTS.md",
    "antigravity-cli/settings.json",
    "antigravity-cli/keybindings.json",
    "config/mcp_config.json",
    "config/config.json",
    "config/hooks.json",
    "config/skills.json",
    "config/plugins.json",
    "config/GEMINI.md",
    "config/AGENTS.md",
];
/// 登录 token：设了 AGY_FILE_ENV 时 agy 存进这个文件（不进钥匙串），cca 只读里面的邮箱和档位
pub const AGY_TOKEN: &str = "antigravity-cli/antigravity-oauth-token";
/// 给 agy 的 SSH_CONNECTION：它只看有没有，写本机地址不带出真实 IP
pub const AGY_SSH: &str = "127.0.0.1 22 127.0.0.1 22";
/// 让 agy 把登录存成数据目录里的文件（AGY_TOKEN）、不进钥匙串（钥匙串里只有一份，所有账号会共用它）的环境变量，
/// 和 bash 的 agy_env 设的一样（一起改）。AGY_CLI_FORCE_FILE_TOKEN_STORAGE 是 agy 自己的开关（1.5.16 里有，值是 1 或 true；
/// 这一版在 macOS 上要钥匙串锁着才认 SSH 会话）；SSH_CONNECTION 是以前的做法（1.5.5 看到 SSH 会话就存文件），留给不认开关的版本
pub const AGY_FILE_ENV: [(&str, &str); 2] = [("SSH_CONNECTION", AGY_SSH), ("AGY_CLI_FORCE_FILE_TOKEN_STORAGE", "1")];
/// agy 的档位 ID → (叫法, 档位)。登录 token 里的 user_tier（1.5.5）是 ID，状态栏 JSON 里的 plan_tier 是叫法，都按这张表认
/// （usage.rs 的 `agy_plan_of`）。Google AI Plus 比 Pro 便宜一档，和 ChatGPT 的 Go 一样算 entry
pub const AGY_TIERS: &[(&str, &str, &str)] = &[
    ("free-tier", "Free", "entry"),
    ("legacy-tier", "Legacy", "standard"),
    ("standard-tier", "Standard", "standard"),
    ("g1-plus-tier", "Google AI Plus", "entry"),
    ("g1-pro-tier", "Google AI Pro", "standard"),
    ("g1-ultra-tier", "Google AI Ultra", "premium"),
];

/// 端口池里先 Claude、再 Codex、最后 agy：只有 Claude 账号时，路由器上的内容和以前逐字节一样。顺序不能变
pub static TOOLS: LazyLock<[Tool; 3]> = LazyLock::new(|| {
    let home = |p: &str| path::join(&HOME, p);
    let cca = |p: &str| path::join(&CCA_HOME, p);
    [
        Tool {
            kind: Kind::Claude,
            name: "claude",
            label: "Claude",
            cmd: "cca",
            table: cca("accounts.json"),
            sroot: CCA_HOME.clone(),
            master: home(".claude"),
            precreate: PRECREATE,
            nests: &[],
            config_names: CLAUDE_SHARED_CONFIG,
            sub: "claude",
            settings_disp: "~/.claude/settings.json",
            state_key: None,
            kc_prefix: "cca-api-",
            kc_ref_prefix: "cca-ref-",
            pool_prefix: "",
            launcher_prefix: "Claude ",
            launcher_id: "local.cca.launcher.",
            app_label: "Claude App",
            app_proc: Some(r"Contents/MacOS/Claude "),
            app_cmd: "app",
            api_fields: &["base_url", "model", "node", "env", "key", "id"],
            url_example: "https://api.example.com/anthropic",
            region_name: "Claude",
            restricted: CLAUDE_RESTRICTED,
            trace_host: "claude.ai",
            web_url: "https://claude.ai/",
            billing_url: "https://platform.claude.com/settings/billing",
            login_hosts: &["claude.com", "claude.ai", "platform.claude.com", "console.anthropic.com"],
            login_paths: &["/oauth/authorize"],
            mail_rules: &["claude-login"],
            product: "Claude Code",
            usage_windows: &["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_oauth_apps"],
            update_cmd: "claude update",
            install_url: "https://claude.ai/install.sh",
            install_shell: "bash",
            has_app: true,
            has_api: true,
            has_usage: true,
            has_cost: true,
        },
        Tool {
            kind: Kind::Codex,
            name: "codex",
            label: "Codex",
            cmd: "cca codex",
            table: cca("codex.json"),
            sroot: cca("codex"),
            master: home(".codex"),
            precreate: CODEX_PRECREATE,
            nests: &[],
            config_names: CODEX_SHARED_CONFIG,
            sub: "codex",
            settings_disp: "~/.codex/config.toml",
            state_key: Some("codex"),
            kc_prefix: "cca-codex-api-",
            kc_ref_prefix: "cca-codex-ref-",
            pool_prefix: "codex-",
            launcher_prefix: "ChatGPT ",
            launcher_id: "local.cca.launcher.codex.",
            app_label: "ChatGPT App",
            app_proc: Some(r"Contents/MacOS/(?:ChatGPT|Codex) "),
            app_cmd: "codex app",
            api_fields: &["base_url", "model", "node", "key", "id"],
            url_example: "https://api.example.com/v1",
            region_name: "OpenAI",
            restricted: CODEX_RESTRICTED,
            trace_host: "chatgpt.com",
            web_url: "https://chatgpt.com/",
            billing_url: "https://platform.openai.com/settings/organization/billing/overview",
            login_hosts: &["auth.openai.com"],
            login_paths: &["/oauth/authorize"],
            mail_rules: &[],
            product: "Codex",
            usage_windows: &["five_hour", "seven_day"],
            update_cmd: "codex update",
            install_url: "https://chatgpt.com/codex/install.sh",
            install_shell: "sh",
            has_app: true,
            has_api: true,
            has_usage: true,
            has_cost: true,
        },
        Tool {
            kind: Kind::Agy,
            name: "agy",
            label: "Antigravity",
            cmd: "cca agy",
            table: cca("agy.json"),
            sroot: cca("agy"),
            master: home(".gemini"),
            precreate: AGY_PRECREATE,
            nests: AGY_NESTS,
            config_names: AGY_SHARED_CONFIG,
            sub: "gemini",
            settings_disp: "~/.gemini/antigravity-cli/settings.json",
            state_key: Some("agy"),
            kc_prefix: "cca-agy-api-",
            kc_ref_prefix: "cca-agy-ref-",
            pool_prefix: "agy-",
            launcher_prefix: "Antigravity ",
            launcher_id: "local.cca.launcher.agy.",
            app_label: "",
            app_proc: None,
            app_cmd: "",
            api_fields: &[],
            url_example: "",
            region_name: "Google",
            restricted: AGY_RESTRICTED,
            trace_host: "www.cloudflare.com",
            web_url: "https://gemini.google.com/",
            // Google AI Pro / Ultra 附带的 Google Developer Program 权益（每月的 Google Cloud 额度）在这里领、看余额
            billing_url: "https://me.developers.google.com/benefits",
            login_hosts: &["accounts.google.com"],
            login_paths: &["/o/oauth2/auth", "/o/oauth2/v2/auth"],
            mail_rules: &[],
            product: "Antigravity",
            usage_windows: &[],
            update_cmd: "agy update",
            install_url: "https://antigravity.google/cli/install.sh",
            install_shell: "bash",
            has_app: false,
            has_api: false,
            has_usage: true,
            has_cost: true,
        },
    ]
});

pub fn claude() -> &'static Tool {
    &TOOLS[0]
}

pub fn codex() -> &'static Tool {
    &TOOLS[1]
}

pub fn agy() -> &'static Tool {
    &TOOLS[2]
}

/// `TOOLS`
pub fn tools() -> &'static [Tool; 3] {
    &TOOLS
}

/// 当前工具在 TOOLS 里的位置（原来的全局变量 `T`）
static CURRENT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// 进程开头照 `CCA_TOOL` 选当前工具（没设是 claude）；认不出就报错退出
pub fn init_current() -> R<()> {
    let name = getenv("CCA_TOOL").filter(|s| !s.is_empty()).unwrap_or_else(|| "claude".into());
    match TOOLS.iter().position(|x| x.name == name) {
        Some(i) => {
            CURRENT.store(i, Ordering::Relaxed);
            Ok(())
        }
        None => die(format!("cca(py): 未知工具 {name}")),
    }
}

/// 当前工具（原来的 `T`）
pub fn t() -> &'static Tool {
    let i = CURRENT.load(Ordering::Relaxed);
    &TOOLS[if i == usize::MAX { 0 } else { i }]
}

thread_local! {
    /// 各工具的第三方 API 账号：账号表的键 → id（原来的 `Tool.ids`）
    static IDS: RefCell<[HashMap<String, String>; 3]> = RefCell::new(Default::default());
    /// 读进来的各工具账号表（工具名 → 表）；格式不对、读不了的不在里面
    static TABS: RefCell<[Option<Dict>; 3]> = const { RefCell::new([None, None, None]) };
    /// 各工具账号表的问题
    static ERRS: RefCell<[Option<Vec<String>>; 3]> = const { RefCell::new([None, None, None]) };
    /// 账号目录里的 .claude.json，一次进程里只读一次（原来的 `ACCT_JSON`，按账号的键记；换工具时清掉）
    static ACCT_JSON: RefCell<HashMap<String, Dict>> = RefCell::new(HashMap::new());
}

/// `ACCT_JSON.get(key)`
pub fn acct_json_cached(key: &str) -> Option<Dict> {
    ACCT_JSON.with(|m| m.borrow().get(key).cloned())
}

/// `ACCT_JSON[key] = j`
pub fn acct_json_store(key: &str, j: &Dict) {
    ACCT_JSON.with(|m| m.borrow_mut().insert(key.to_string(), j.clone()));
}

/// `ACCT_JSON.clear()`
pub fn acct_json_clear() {
    ACCT_JSON.with(|m| m.borrow_mut().clear());
}

impl Tool {
    pub fn index(&self) -> usize {
        self.kind as usize
    }

    /// `tool is T`
    pub fn is_current(&self) -> bool {
        self.kind == t().kind
    }

    /// `tool.ids.get(key)`
    pub fn id_of(&self, key: &str) -> Option<String> {
        IDS.with(|m| m.borrow()[self.index()].get(key).cloned())
    }

    /// `tool.set_ids(t)`：第三方 API 账号的键 → id（改别名时 id 不变，账号目录、钥匙串里的 key 和端口都跟着 id 走）
    pub fn set_ids(&self, table: &Dict) {
        let ids = table
            .items()
            .into_iter()
            .filter_map(|(k, v)| match v.as_dict()?.get("id") {
                Some(V::Str(id)) if !id.is_empty() => Some((k.to_string(), id.to_string())),
                _ => None,
            })
            .collect();
        IDS.with(|m| m.borrow_mut()[self.index()] = ids);
    }

    /// `TABS.get(tool.name)`
    pub fn tab(&self) -> Option<Dict> {
        TABS.with(|m| m.borrow()[self.index()].clone())
    }

    /// `TABS[tool.name] = t` / `TABS.pop(tool.name, None)`
    pub fn set_tab(&self, table: Option<Dict>) {
        TABS.with(|m| m.borrow_mut()[self.index()] = table);
    }

    /// `ERRS.get(tool.name, [])`
    pub fn errs(&self) -> Vec<String> {
        ERRS.with(|m| m.borrow()[self.index()].clone().unwrap_or_default())
    }

    pub fn set_errs(&self, errs: Vec<String>) {
        ERRS.with(|m| m.borrow_mut()[self.index()] = Some(errs));
    }

    /// `ERRS.clear()` 里这个工具的那一份
    pub fn clear_errs(&self) {
        ERRS.with(|m| m.borrow_mut()[self.index()] = None);
    }

    /// `tool.state(c, create)`：config.json 里属于这个工具的那部分（ports、last、api_ids、api_keys、ref_keys、
    /// key_hints、policy_skip）。Claude 的就是 c 自己。别的工具那一节不在（或不是字典）时给一个新的空字典：
    /// create 时接到 c 上，否则不接（往里写的东西不会进 c，原来就是这样）
    pub fn state(&self, c: &Dict, create: bool) -> Dict {
        let Some(key) = self.state_key else {
            return c.clone();
        };
        let s = match c.dict(key) {
            Some(s) => s,
            None => {
                let s = Dict::new();
                if create {
                    c.set(key, &s);
                }
                s
            }
        };
        s.setdefault("ports", Dict::new());
        s.setdefault("last", "");
        s
    }

    /// 账号目录里的这个条目链接到共享主目录吗
    pub fn shares(&self, n: &str) -> bool {
        match self.kind {
            Kind::Claude => CLAUDE_SHARED.contains(&n),
            Kind::Codex => CODEX_SHARED.contains(&n) || codex_profile_file(n),
            Kind::Agy => AGY_SHARED.contains(&n),
        }
    }

    pub fn shared_config(&self, n: &str) -> bool {
        match self.kind {
            Kind::Claude => CLAUDE_SHARED_CONFIG.contains(&n),
            Kind::Codex => CODEX_SHARED_CONFIG.contains(&n) || codex_profile_file(n),
            Kind::Agy => AGY_SHARED_CONFIG.contains(&n),
        }
    }
}

/// 换掉当前工具，离开作用域时换回来（原来的 `with using(工具)`）。按键缓存的 .claude.json 跟着清掉：
/// 同一个邮箱可以既是 Claude 账号又是 Codex 账号
pub struct Using {
    prev: usize,
}

impl Using {
    pub fn tool(tool: &Tool) -> Using {
        let prev = CURRENT.swap(tool.index(), Ordering::Relaxed);
        acct_json_clear();
        Using { prev }
    }
}

impl Drop for Using {
    fn drop(&mut self) {
        CURRENT.store(self.prev, Ordering::Relaxed);
        acct_json_clear();
    }
}

/// Codex 的 profile 文件名（`名字.config.toml`，不在子目录里、不以点开头）
pub fn codex_profile_file(name: &str) -> bool {
    path::basename(name) == name && !name.starts_with('.') && name.ends_with(".config.toml")
}

/// 选择的 profile 必须是会被发现并校验的同目录配置；允许 Unicode 和空格
pub fn codex_profile_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}')
        && codex_profile_file(&format!("{name}.config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;

    /// 各工具的受限地区照各家自己的名单（base.rs 里写着出处）：两个大写字母、不重复；三家都不能用的那几个三份都有，
    /// 各家不一样的地方分得开
    #[test]
    fn restricted_regions_per_tool() {
        let lists = [claude().restricted, codex().restricted, agy().restricted];
        for l in lists {
            for (i, (cc, name)) in l.iter().enumerate() {
                assert!(cc.len() == 2 && cc.chars().all(|c| c.is_ascii_uppercase()), "{cc}");
                assert!(!name.is_empty() && !l[..i].iter().any(|(x, _)| x == cc), "{cc}");
            }
        }
        let has = |l: &[(&str, &str)], cc: &str| l.iter().any(|(x, _)| *x == cc);
        for cc in ["CN", "HK", "MO", "RU", "KP", "IR", "CU", "BY"] {
            assert!(lists.iter().all(|l| has(l, cc)), "{cc}");
        }
        let [c, x, g] = lists;
        assert!(has(c, "MM") && !has(x, "MM") && has(g, "MM"));
        assert!(has(c, "VE") && has(x, "VE") && !has(g, "VE"));
        assert!(has(c, "SY") && has(x, "SY") && !has(g, "SY"));
        assert!(!has(c, "MD") && !has(x, "MD") && has(g, "MD"));
        assert!(has(c, "YE") && !has(x, "YE") && !has(g, "YE"));
        assert!(has(c, "XK") && has(x, "XK") && !has(g, "XK"));
    }

    #[test]
    fn state_semantics() {
        let c = dict! { "ports" => dict! { "a" => 7901 } };
        assert!(claude().state(&c, false).same(&c));
        let s = codex().state(&c, false);
        s.set("last", "x");
        assert!(!c.has("codex")); // 没接上：写的东西不进 c
        let s = codex().state(&c, true);
        s.dict("ports").unwrap().set("k", 7902);
        assert_eq!(c.dict("codex").unwrap().dict("ports").unwrap().get("k").unwrap().as_int(), Some(7902));
        assert_eq!(c.dict("codex").unwrap().str("last").as_deref(), Some(""));
    }

    #[test]
    fn ids_and_shares() {
        let table =
            dict! { "官方(me@example.com)" => "US-A", "api1" => dict! { "id" => "api1-x" }, "api2" => dict! { "id" => "" } };
        codex().set_ids(&table);
        assert_eq!(codex().id_of("api1").as_deref(), Some("api1-x"));
        assert_eq!(codex().id_of("api2"), None);
        assert_eq!(claude().id_of("api1"), None);
        assert!(claude().shares("projects") && !claude().shares(".credentials.json"));
        assert!(codex().shares("work.config.toml") && !codex().shares(".hidden.config.toml") && !codex().shares("a/b.config.toml"));
        assert!(agy().shared_config("config/GEMINI.md") && !agy().has_api);
        assert!(codex_profile_name("工作 1") && !codex_profile_name("a\nb") && !codex_profile_name(""));
    }

    #[test]
    fn profile_names_match_discovery() {
        // tests/isolation_settings.rs 的 profile_names_match_discovery_and_cannot_escape_checked_files：能选的 profile 就是
        // ~/.codex 里会被找到、照样校验的那个文件，名字逃不出这个目录。后两项是原来的 Python 给的：
        // codex_profile_file(名字 + '.config.toml')、codex_profile_name(名字)
        for (name, file, selectable) in [
            ("deep-review", true, true),
            ("工作 配置", true, true),
            ("review.v2", true, true),
            ("with spaces", true, true),
            ("../outside", false, false),
            ("/tmp/outside", false, false),
            ("nested/profile", false, false),
            (".hidden", false, false),
            ("", false, false),
            // 控制字符：文件找得到，但不能选
            ("bad\nname", true, false),
            ("a\tb", true, false),
            ("a\u{7f}b", true, false),
            ("tab\u{1f}", true, false),
            ("a\u{80}b", true, true),
            ("x.config.toml", true, true),
            ("a/", false, false),
            ("..", false, false),
            (".", false, false),
            (" ", true, true),
            ("é", true, true),
            ("trailing.", true, true),
        ] {
            assert_eq!(codex_profile_file(&format!("{name}.config.toml")), file, "{name:?}");
            assert_eq!(codex_profile_name(name), selectable, "{name:?}");
        }
        for (file, want) in [
            ("deep-review.config.toml", true),
            ("工作 配置.config.toml", true),
            (".config.toml", false),
            (".hidden.config.toml", false),
            ("config.toml", false),
            ("a/b.config.toml", false),
            ("/abs.config.toml", false),
            ("x.config.toml/", false),
            ("notes.toml", false),
            ("x.config.toml.bak", false),
            ("X.CONFIG.TOML", false),
        ] {
            assert_eq!(codex_profile_file(file), want, "{file:?}");
        }
    }

    #[test]
    fn using_restores() {
        assert_eq!(t().name, "claude");
        {
            let _g = Using::tool(agy());
            assert_eq!(t().name, "agy");
            assert!(agy().is_current());
        }
        assert_eq!(t().name, "claude");
    }
}
