//! 邮箱助手（`MailRule` 到 `cmd_mail_link_check`）：mail-setup / status / ready / get-user / forget / trust-forwarder / test / open /
//! watch / link-check、gui-mail-list。
//!
//! 子命令还没移植。删账号时清理要用的这几样已经移植（secrets 模块调）：`mail_one`、`mail_all_addrs`、`mail_forget_orphans`。

use super::secrets::ks_del;
use super::Cmd;
use crate::core::ctx::cfg_save;
use crate::core::model::{acct_id, cfg_load};
use crate::core::tool::claude;
use crate::dict;
use crate::py::fs::load_json;
use crate::py::obj::{Dict, V};
use crate::py::re::Re;
use crate::py::R;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// 钥匙串（没有钥匙串的系统是密钥文件）里应用专用密码的服务名；账户名是 Gmail 地址：换了地址取不到旧的
pub const MAIL_SVC: &str = "cca-mail";
pub static MAIL_DOMAIN_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[a-z0-9-]+(\.[a-z0-9-]+)+$"));
/// Gmail 地址（小写；也是钥匙串的账户名，交给 security -i）
pub static MAIL_ADDR_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[a-z0-9._%+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+$"));

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
