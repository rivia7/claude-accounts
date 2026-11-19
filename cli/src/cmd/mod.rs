//! 各个子命令，按原来 PY_SRC 里的分节一节一个模块。
//!
//! `NAMES` 是原来的 `CMDS` 里的全部名字（先后照它）；每个模块自己有一张 `CMDS`，列着它移植好的子命令。
//! 移植一个子命令：在它所属的模块里写 `pub fn cmd_xxx(args: &[String]) -> R<()>`，加进那个模块的 `CMDS`，别的文件不用动。

pub mod accounts;
pub mod clients;
pub mod codex;
pub mod corenet;
pub mod cost;
pub mod dock;
pub mod gui;
pub mod keyshare;
pub mod launchers;
pub mod leak;
pub mod link;
pub mod login;
pub mod mail;
pub mod presence;
pub mod router;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod shell;
pub mod table;
pub mod usage;

use crate::py::{Fail, R};

pub type Run = fn(&[String]) -> R<()>;

pub struct Cmd {
    pub name: &'static str,
    pub run: Run,
    /// 对哪几个工具移植好了（开发时的混合版按它决定要不要交给 Python）
    pub tools: &'static [&'static str],
}

impl Cmd {
    /// 三种工具都移植好了
    pub const fn all(name: &'static str, run: Run) -> Cmd {
        Cmd { name, run, tools: &["claude", "codex", "agy"] }
    }

    /// 先只移植了 Claude 的（别的工具在混合版里照旧交给 Python）
    pub const fn claude_only(name: &'static str, run: Run) -> Cmd {
        Cmd { name, run, tools: &["claude"] }
    }

    /// 只移植了这几个工具的
    pub const fn only(name: &'static str, run: Run, tools: &'static [&'static str]) -> Cmd {
        Cmd { name, run, tools }
    }
}

/// 各模块的 CMDS
const MODULES: &[&[Cmd]] = &[
    table::CMDS,
    accounts::CMDS,
    router::CMDS,
    link::CMDS,
    secrets::CMDS,
    login::CMDS,
    mail::CMDS,
    corenet::CMDS,
    launchers::CMDS,
    settings::CMDS,
    leak::CMDS,
    usage::CMDS,
    codex::CMDS,
    gui::CMDS,
    session::CMDS,
    presence::CMDS,
    keyshare::CMDS,
    cost::CMDS,
    dock::CMDS,
    clients::CMDS,
    shell::CMDS,
];

/// 移植好的子命令
pub fn find(name: &str) -> Option<&'static Cmd> {
    MODULES.iter().flat_map(|m| m.iter()).find(|c| c.name == name)
}

/// 原来有这个子命令（移植了没有另说），或者是后来加的（`shell` 模块的 `sh-…`：bash 里原来内嵌的小段 Python）
pub fn known(name: &str) -> bool {
    NAMES.contains(&name) || find(name).is_some()
}

/// 参数个数不对：Python 里是没人接的 TypeError（打印调用栈，退出码 1）
pub fn arity(name: &str, args: &[String], min: usize, max: usize) -> R<()> {
    if args.len() < min || args.len() > max {
        return Err(Fail::Die(format!("TypeError: {} 的参数个数不对（给了 {} 个）", name, args.len())));
    }
    Ok(())
}

/// 第 i 个参数，没给就是默认值
pub fn arg<'a>(args: &'a [String], i: usize, default: &'a str) -> &'a str {
    args.get(i).map_or(default, |s| s.as_str())
}

/// 原来的 `CMDS` 里的名字，先后照它（主干增删子命令时跟着改）
pub const NAMES: &[&str] = &[
    "init", "env", "router-env", "router-set", "keys", "resolve", "validate", "email", "check", "add", "rm", "rename",
    "reorder", "set-last", "session-open", "name", "login-url", "private-profile", "runtime-record", "browser-profile",
    "get", "set", "nodes-allowed", "set-nodes", "nodes-import", "restricted", "nodes-follow", "nodes-rename",
    "accounts-json", "router-merge", "router-evict", "slot-name", "ports", "select-lines", "digest", "sync-state",
    "keychain", "settings-check", "launch-check", "settings-explain", "settings-fix", "coffee", "rows", "node-rows",
    "node-default", "link", "sync", "device", "launchers", "local-env", "local-set", "udp-env", "udp-node", "udp-build",
    "udp-probe", "local-node", "core-config", "core-nodes-save", "core-missing", "core-summary", "node-check",
    "node-testconfig", "node-save", "node-ls", "node-rm", "core-stale", "core-keep", "install-env", "install-node",
    "file-replace", "slot-rows", "add-api", "api-key-out", "secret-check", "secret-set", "secret-del", "secret-status",
    "secrets-missing", "secrets-forget", "key-show", "key-forget", "key-share", "key-sync", "usage-query", "usage-render",
    "login-plan-forget", "reset-consume", "reset-plan", "reset-done", "tool-env", "login-flow", "login-check",
    "login-forward", "login-catch", "codex-fallback-off", "agy-login", "agy-probe", "node-fp", "login-page-acct",
    "update-check", "client-updates", "policy-check", "policy-node", "policy-summary", "nodes-preselect", "nodes-lost",
    "edit-form", "edit-apply", "policy-request", "policy-lock", "policy-rows", "policy-repo", "policy-ask",
    "policy-decline", "policy-set", "policy-have", "policy-forget", "fill-check", "fill-summary", "nodes-fill",
    "leak-probe", "leak-verdict", "gui-state", "gui-live", "onboard-done", "health-save", "health-note", "presence-agent",
    "presence-wake", "presence-stop", "presence-note", "presence-view", "dock-tidy", "cost-worker", "cost-sync",
    "cost-stop", "prompt-emit", "check-render", "mail-setup", "mail-status", "mail-ready", "mail-forget",
    "mail-trust-forwarder", "mail-test", "mail-open", "mail-watch", "mail-link-check", "mail-get-user", "gui-mail-list",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_original_cmds() {
        assert_eq!(NAMES.len(), 149);
        let mut names = NAMES.to_vec();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 149);
        assert_eq!((NAMES[0], NAMES[148]), ("init", "gui-mail-list"));
    }

    #[test]
    fn every_ported_command_is_known_and_listed_once() {
        let mut seen = Vec::new();
        for c in MODULES.iter().flat_map(|m| m.iter()) {
            assert!(known(c.name), "{} 不在 NAMES 里", c.name);
            assert!(!seen.contains(&c.name), "{} 登记了两次", c.name);
            assert!(!c.tools.is_empty());
            seen.push(c.name);
        }
    }
}
