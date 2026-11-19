//! 路由器端口池的内容和同步摘要（`router_accounts`、`sync_digest`）。
//!
//! 摘要存在 config.json 的 `synced` 里，和上次同步时的比，不一样就重新部署路由器端。所以这里输出的每个字节都要和
//! 原来一样：差一个字节，所有已安装的机器升级后都会重新部署一次。

use super::base::FIRST_PORT;
use super::ctx::pool_size;
use super::model::{acct_id, node_of, pool};
use super::policy::policy_fill;
use crate::dict;
use crate::py::hash::sha1_hex;
use crate::py::json;
use crate::py::obj::{Dict, List, V};

/// 路由器端口池的内容：各工具走节点的账号（直连的第三方 API 账号不占槽位）。有维护者的参考节点时带上 fill：
/// 路由器生成运行配置时把缺的补上（没有参考节点时和以前逐字节一样）
pub fn router_accounts(c: &Dict) -> Dict {
    let accounts: Vec<V> = pool(c)
        .into_iter()
        .map(|(tool, k, v, p)| {
            let slot = match p.as_int() {
                Some(n) => V::Int(n - FIRST_PORT + 1),
                None => panic!("TypeError: 端口不是整数（{}）", p.py_repr()),
            };
            V::Dict(dict! { "slot" => slot, "name" => format!("{}{}", tool.pool_prefix, acct_id(&k, tool)), "node" => node_of(&v) })
        })
        .collect();
    let out = dict! { "base" => FIRST_PORT - 1, "slots" => pool_size(c), "nodes" => c.at("nodes"), "accounts" => List::from_vec(accounts) };
    if let Some(f) = policy_fill() {
        out.set("fill", f);
    }
    out
}

/// 账号表 + 路由器端代码的摘要（和 config.json 的 synced 比，不一样就要同步）
pub fn sync_digest(c: &Dict, code_text: &str) -> String {
    let code = sha1_hex(code_text);
    let doc = V::from(vec![c.get("proxy_host").unwrap_or(V::None), V::Dict(router_accounts(c)), V::from(code)]);
    sha1_hex(json::dumps(&doc, json::Fmt::new().utf8().sort_keys()))
}
