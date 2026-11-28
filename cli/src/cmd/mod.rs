//! 各个子命令，按原来 PY_SRC 里的分节一节一个模块。
//!
//! 每个模块自己有一张 `CMDS`，列着它的子命令。加一个子命令：在它所属的模块里写 `pub fn cmd_xxx(args: &[String]) -> R<()>`，
//! 加进那个模块的 `CMDS`，别的文件不用动。`NAMES` 是原来 Python 的 `CMDS` 里的全部名字（先后照它），测试拿它核对一个不少。

pub mod accounts;
pub mod agy;
pub mod clients;
pub mod codex;
pub mod corenet;
pub mod cost;
pub mod dock;
pub mod guard;
pub mod gui;
pub mod hub;
pub mod keyshare;
pub mod launchers;
pub mod leak;
pub mod link;
pub mod login;
pub mod mail;
pub mod pack;
pub mod presence;
pub mod remote;
pub mod router;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod shell;
pub mod table;
pub mod team;
pub mod usage;

use crate::py::{Fail, R};

pub type Run = fn(&[String]) -> R<()>;

pub struct Cmd {
    pub name: &'static str,
    pub run: Run,
}

impl Cmd {
    /// 一个子命令（三种工具走同一个函数，差别在 `core::tool::t()`）
    pub const fn all(name: &'static str, run: Run) -> Cmd {
        Cmd { name, run }
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
    agy::CMDS,
    codex::CMDS,
    gui::CMDS,
    session::CMDS,
    presence::CMDS,
    keyshare::CMDS,
    cost::CMDS,
    dock::CMDS,
    clients::CMDS,
    shell::CMDS,
    remote::CMDS,
    hub::CMDS,
    team::CMDS,
    pack::CMDS,
    crate::server::CMDS,
];

/// 不是这台机器上 cca 的一部分的子命令：Team 的同步服务器、证书助手（server/）和 pack（release.sh 在维护者的机器上对着仓库里的文件
/// 运行）。main 直接运行它们：不认当前是哪种工具，不读 HOME 下 cca 的文件
pub fn standalone(name: &str) -> Option<&'static Cmd> {
    crate::server::standalone(name).or_else(|| pack::CMDS.iter().find(|c| c.name == name))
}

/// 按名字找子命令
pub fn find(name: &str) -> Option<&'static Cmd> {
    MODULES.iter().flat_map(|m| m.iter()).find(|c| c.name == name)
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

/// 原来的 `CMDS` 里的名字，先后照它（主干增删子命令时跟着改）；切换以后新加的接在相关的后面（session-start）
#[cfg(test)]
pub const NAMES: &[&str] = &[
    "init",
    "env",
    "router-env",
    "router-set",
    "keys",
    "resolve",
    "validate",
    "email",
    "check",
    "add",
    "rm",
    "rename",
    "reorder",
    "set-last",
    "session-open",
    "name",
    "login-url",
    "private-profile",
    "runtime-record",
    "browser-profile",
    "get",
    "set",
    "nodes-allowed",
    "set-nodes",
    "nodes-import",
    "restricted",
    "nodes-follow",
    "nodes-rename",
    "accounts-json",
    "router-merge",
    "router-evict",
    "custom-clashes",
    "slot-name",
    "ports",
    "select-lines",
    "digest",
    "sync-state",
    "keychain",
    "settings-check",
    "launch-check",
    "settings-explain",
    "settings-fix",
    "coffee",
    "rows",
    "node-rows",
    "node-default",
    "link",
    "sync",
    "device",
    "launchers",
    "local-env",
    "local-set",
    "udp-env",
    "udp-node",
    "udp-build",
    "udp-probe",
    "local-node",
    "core-config",
    "core-nodes-save",
    "core-missing",
    "core-summary",
    "node-check",
    "node-testconfig",
    "node-save",
    "node-ls",
    "node-rm",
    "node-rename",
    "core-stale",
    "core-keep",
    "install-env",
    "install-node",
    "file-replace",
    "slot-rows",
    "add-api",
    "api-key-out",
    "secret-check",
    "secret-set",
    "secret-del",
    "secret-status",
    "secrets-missing",
    "secrets-forget",
    "key-show",
    "key-forget",
    "key-share",
    "key-sync",
    "usage-query",
    "usage-render",
    "login-plan-forget",
    "reset-consume",
    "reset-plan",
    "reset-done",
    "tool-env",
    "login-flow",
    "login-check",
    "login-forward",
    "login-catch",
    "codex-fallback-off",
    "agy-login",
    "agy-probe",
    "node-fp",
    "login-page-acct",
    "update-check",
    "client-updates",
    "policy-check",
    "policy-node",
    "policy-summary",
    "nodes-preselect",
    "nodes-lost",
    "edit-form",
    "edit-apply",
    "node-assign",
    "policy-request",
    "policy-lock",
    "policy-rows",
    "policy-repo",
    "policy-ask",
    "policy-decline",
    "policy-set",
    "policy-have",
    "policy-forget",
    "fill-check",
    "fill-summary",
    "nodes-fill",
    "leak-probe",
    "leak-verdict",
    "gui-state",
    "gui-live",
    "onboard-done",
    "health-save",
    "health-note",
    "presence-agent",
    "presence-wake",
    "presence-stop",
    "presence-note",
    "presence-view",
    "dock-tidy",
    "cost-worker",
    "cost-sync",
    "cost-stop",
    "session-start",
    "prompt-emit",
    "check-render",
    "mail-setup",
    "mail-status",
    "mail-ready",
    "mail-forget",
    "mail-trust-forwarder",
    "mail-test",
    "mail-open",
    "mail-watch",
    "mail-link-check",
    "mail-get-user",
    "gui-mail-list",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_original_cmds() {
        assert_eq!(NAMES.len(), 153);
        let mut names = NAMES.to_vec();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 153);
        assert_eq!((NAMES[0], NAMES[152]), ("init", "gui-mail-list"));
    }

    /// 切换以后新加的子命令（`sh-…`、`remote-…` 之外的）。policy-retire：升级到 3.6.0 时去掉 config.json 里映射的那几项；
    /// pack：把辅助程序接到 cca 后面（原来是 cli/pack.py，不在 PY_SRC 里）
    const ADDED: &[&str] =
        &["unpark", "policy-retire", "cost-hook", "agy-session-end", "pack", "launch-args", "agy-statusline", "agy-turn-wait"];
    /// 切换以后去掉的：共享默认关以后终端里启动时不再引导配置映射（policy-ask 是问不问）；3.6.0 去掉了维护者的映射和维护者的服务器，
    /// 只给它们用的几个也去掉了
    const REMOVED: &[&str] = &[
        "policy-ask",
        "policy-check",
        "policy-summary",
        "policy-request",
        "policy-lock",
        "policy-rows",
        "policy-repo",
        "policy-decline",
        "policy-set",
        "fill-check",
        "fill-summary",
        "presence-note",
    ];

    /// main 一开头就分出去的那几条（不认当前是哪种工具、不读账号表）：服务端的两条和 pack，都也登记在平常的表里
    #[test]
    fn standalone_commands() {
        for name in ["team-server", "team-acme", "team-push", "pack"] {
            assert_eq!(standalone(name).unwrap().name, name);
            assert!(find(name).is_some(), "{name}");
        }
        assert!(standalone("env").is_none() && standalone("team-join").is_none() && standalone("").is_none());
    }

    /// 原来的 149 个子命令（加上切换以后新加的）除了特意去掉的（REMOVED）一个不少；另外只有 `sh-…`（bash 里原来内嵌的那几小段 Python）、
    /// `remote-…`（远程会话，辅助程序里新写的，见 remote.rs）、`hub-…`（一键部署的安装脚本，见 hub/deploy.rs）、`team-…`（Team，
    /// 见 team.rs、team/admin.rs；team-server、team-acme 是 Team 的同步服务器和证书助手，见 server/）和 ADDED；没有登记两次的
    #[test]
    fn every_original_command_is_there_and_listed_once() {
        let mut seen = Vec::new();
        for c in MODULES.iter().flat_map(|m| m.iter()) {
            let added = c.name.starts_with("sh-")
                || c.name.starts_with('_')
                || c.name.starts_with("remote-")
                || c.name.starts_with("hub-")
                || c.name.starts_with("team-")
                || ADDED.contains(&c.name);
            assert!(NAMES.contains(&c.name) || added, "{} 不是原来的子命令", c.name);
            assert!(!REMOVED.contains(&c.name), "{} 已经去掉了", c.name);
            assert!(!seen.contains(&c.name), "{} 登记了两次", c.name);
            seen.push(c.name);
        }
        for name in NAMES.iter().filter(|n| !REMOVED.contains(n)) {
            assert!(find(name).is_some(), "{name} 没有移植");
        }
    }
}
