//! 各个子命令，按原来 PY_SRC 里的分节一节一个模块；`TABLE` 是原来的 `CMDS`（名字和先后都照它）。

pub mod presence;
pub mod table;

use crate::py::{Fail, R};

pub type Run = fn(&[String]) -> R<()>;

pub struct Cmd {
    pub name: &'static str,
    /// 还没移植的是 None
    pub run: Option<Run>,
    /// 对哪几个工具移植好了（开发时的混合版按它决定要不要交给 Python）
    pub tools: &'static [&'static str],
}

const ALL: &[&str] = &["claude", "codex", "agy"];
const CLAUDE: &[&str] = &["claude"];

const fn done(name: &'static str, run: Run) -> Cmd {
    Cmd { name, run: Some(run), tools: ALL }
}

const fn only(name: &'static str, run: Run, tools: &'static [&'static str]) -> Cmd {
    Cmd { name, run: Some(run), tools }
}

const fn todo(name: &'static str) -> Cmd {
    Cmd { name, run: None, tools: &[] }
}

pub fn find(name: &str) -> Option<&'static Cmd> {
    TABLE.iter().find(|c| c.name == name)
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

pub const TABLE: &[Cmd] = &[
    done("init", table::cmd_init),
    todo("env"),
    done("router-env", table::cmd_router_env),
    done("router-set", table::cmd_router_set),
    done("keys", table::cmd_keys),
    done("resolve", table::cmd_resolve),
    done("validate", table::cmd_validate),
    only("email", table::cmd_email, CLAUDE),
    todo("check"),
    todo("add"),
    todo("rm"),
    todo("rename"),
    todo("reorder"),
    todo("set-last"),
    todo("session-open"),
    done("name", table::cmd_name),
    todo("login-url"),
    todo("private-profile"),
    todo("runtime-record"),
    todo("browser-profile"),
    done("get", table::cmd_get),
    done("set", table::cmd_set),
    todo("nodes-allowed"),
    todo("set-nodes"),
    done("nodes-import", table::cmd_nodes_import),
    done("restricted", table::cmd_restricted),
    todo("nodes-follow"),
    todo("nodes-rename"),
    todo("accounts-json"),
    todo("router-merge"),
    todo("router-evict"),
    todo("slot-name"),
    done("ports", table::cmd_ports),
    todo("select-lines"),
    done("digest", table::cmd_digest),
    todo("sync-state"),
    todo("keychain"),
    todo("settings-check"),
    todo("launch-check"),
    todo("settings-explain"),
    todo("settings-fix"),
    todo("coffee"),
    todo("rows"),
    todo("node-rows"),
    todo("node-default"),
    todo("link"),
    todo("sync"),
    todo("device"),
    todo("launchers"),
    todo("local-env"),
    todo("local-set"),
    todo("udp-env"),
    todo("udp-node"),
    todo("udp-build"),
    todo("udp-probe"),
    todo("local-node"),
    todo("core-config"),
    todo("core-nodes-save"),
    todo("core-missing"),
    todo("core-summary"),
    todo("node-check"),
    todo("node-testconfig"),
    todo("node-save"),
    todo("node-ls"),
    todo("node-rm"),
    todo("core-stale"),
    todo("core-keep"),
    todo("install-env"),
    todo("install-node"),
    todo("file-replace"),
    todo("slot-rows"),
    todo("add-api"),
    todo("api-key-out"),
    todo("secret-check"),
    todo("secret-set"),
    todo("secret-del"),
    todo("secret-status"),
    todo("secrets-missing"),
    todo("secrets-forget"),
    todo("key-show"),
    todo("key-forget"),
    todo("key-share"),
    todo("key-sync"),
    todo("usage-query"),
    todo("usage-render"),
    todo("login-plan-forget"),
    todo("reset-consume"),
    todo("reset-plan"),
    todo("reset-done"),
    todo("tool-env"),
    todo("login-flow"),
    todo("login-check"),
    todo("login-forward"),
    todo("login-catch"),
    todo("codex-fallback-off"),
    todo("agy-login"),
    todo("agy-probe"),
    todo("node-fp"),
    todo("login-page-acct"),
    todo("update-check"),
    todo("client-updates"),
    todo("policy-check"),
    done("policy-node", table::cmd_policy_node),
    done("policy-summary", table::cmd_policy_summary),
    todo("nodes-preselect"),
    todo("nodes-lost"),
    todo("edit-form"),
    todo("edit-apply"),
    todo("policy-request"),
    todo("policy-lock"),
    done("policy-rows", table::cmd_policy_rows),
    done("policy-repo", table::cmd_policy_repo),
    done("policy-ask", table::cmd_policy_ask),
    done("policy-decline", table::cmd_policy_decline),
    done("policy-set", table::cmd_policy_set),
    done("policy-have", table::cmd_policy_have),
    done("policy-forget", table::cmd_policy_forget),
    todo("fill-check"),
    todo("fill-summary"),
    todo("nodes-fill"),
    todo("leak-probe"),
    todo("leak-verdict"),
    todo("gui-state"),
    todo("gui-live"),
    todo("onboard-done"),
    todo("health-save"),
    todo("health-note"),
    todo("presence-agent"),
    todo("presence-wake"),
    todo("presence-stop"),
    todo("presence-note"),
    todo("presence-view"),
    todo("dock-tidy"),
    todo("cost-worker"),
    todo("cost-sync"),
    todo("cost-stop"),
    todo("prompt-emit"),
    todo("check-render"),
    todo("mail-setup"),
    todo("mail-status"),
    todo("mail-ready"),
    todo("mail-forget"),
    todo("mail-trust-forwarder"),
    todo("mail-test"),
    todo("mail-open"),
    todo("mail-watch"),
    todo("mail-link-check"),
    todo("mail-get-user"),
    todo("gui-mail-list"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_the_original_cmds() {
        // 名字和先后照 PY_SRC 的 CMDS（切换前随主干增删子命令一起改）
        assert_eq!(TABLE.len(), 149);
        let mut names: Vec<&str> = TABLE.iter().map(|c| c.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 149);
        assert_eq!((TABLE[0].name, TABLE[148].name), ("init", "gui-mail-list"));
    }
}
