//! env、rows / node-rows / node-default、check、add / add-api / rename / reorder / rm、set-last、edit-form / edit-apply、prompt-emit、
//! nodes-allowed / set-nodes / nodes-preselect / nodes-lost / select-lines、sync-state、slot-rows、accounts-json、file-replace（PY_SRC 的 `cmd_env` 到 `cmd_rm`、
//! `cmd_nodes_allowed` 到 `cmd_set_nodes`、`cmd_slot_rows`）。
//!
//! 移植了：env。其余还没移植。

use super::codex::codex_probe;
use super::table::need;
use super::{arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::ctx;
use crate::core::model::*;
use crate::core::tool::{t, Kind, Tool, CODEX_PROVIDER};
use crate::dict;
use crate::py::fs::load_quiet;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::env_or_empty;
use crate::py::text::{self, sh_quote};
use crate::py::R;
use crate::re;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("env", cmd_env)];

/// 每个账号都输出全部变量（不适用的为空）：bash 里循环 eval 时不会留着上一个账号的值。
/// ACC_HOME 是交给工具的配置目录（CLAUDE_CONFIG_DIR / CODEX_HOME），ACC_ARGS 是启动时加在用户参数前面的，
/// ACC_FLAGS 是 cca 运行这个工具时都要加的
pub fn cmd_env(args: &[String]) -> R<()> {
    arity("cmd_env", args, 1, 1)?;
    let (c, table, _) = ctx()?;
    let mut key = args[0].clone();
    if !table.has(&key) {
        // 会话开着时改了别名：按邮箱找回同一个账号
        let id = acct_id(&key, t());
        if let Some(k) = table.keys().into_iter().find(|k| acct_id(k, t()) == id) {
            key = k.to_string();
        }
    }
    need(&table, &key)?;
    let v = table.at(&key);
    let api = v.as_dict();
    let host = c.get("proxy_host").filter(|h| h.truthy()).unwrap_or_else(|| c.dict("router").and_then(|r| r.get("host")).unwrap_or_else(|| "".into()));
    // 直连的第三方 API 账号没有端口
    let port = t().state(&c, false).at("ports").as_dict().and_then(|p| p.get(&key)).filter(|p| p.truthy());
    let port_num = port.as_ref().map(|p| p.as_int().unwrap_or_else(|| panic!("TypeError: 端口不是整数（{}）", p.py_repr())));
    let (a, e) = parse_key(&key);
    let id = acct_id(&key, t());
    let dir = adir(&key);
    let no_proxy = c.at("no_proxy");
    let empty = || V::from("");
    sh_vals(&[
        ("ACC_KEY", V::from(&key)),
        ("ACC_ID", V::from(&id)),
        ("ACC_ALIAS", V::from(&a)),
        ("ACC_EMAIL", V::from(&e)),
        ("ACC_NAME", V::from(if a.is_empty() { &e } else { &a })),
        ("ACC_LABEL", V::from(label_of(&key))),
        ("ACC_KIND", V::from(if api.is_some() { "api" } else { "oauth" })),
        ("ACC_NODE", V::from(node_of(&v)).or("直连")),
        ("ACC_PORT", port.clone().unwrap_or_else(empty)),
        // 路由器端口池里这个账号的名字（route_router 回读校验槽位用）
        ("ACC_POOLNAME", V::from(format!("{}{}", t().pool_prefix, id))),
        ("ACC_SLOT", port_num.map_or_else(empty, |p| V::Int(p - FIRST_PORT + 1))),
        ("ACC_PROXY", match port_num {
            Some(p) if host.truthy() => V::from(format!("http://{}:{}", host.py_str(), p)),
            _ => empty(),
        }),
        ("ACC_BASE_URL", api.as_ref().map_or_else(empty, |d| V::from(api_url(d)))),
        ("ACC_MODEL", api.as_ref().and_then(|d| d.get("model")).filter(|m| m.truthy()).unwrap_or_else(empty)),
        ("ACC_DIR", V::from(&dir)),
        ("ACC_HOME", V::from(cdir(&key))),
        ("ACC_MASTER", V::from(&t().master)),
        ("ACC_CHROME_DIR", V::from(path::join(&dir, "chrome"))),
        ("ACC_DESKTOP_DIR", V::from(path::join(&dir, "desktop"))),
        ("CCA_NO_PROXY", no_proxy.clone()),
        ("CCA_PROXY_BYPASS", V::from(bypass_list(no_proxy.as_str().unwrap_or_else(|| panic!("AttributeError: no_proxy 不是字符串（{}）", no_proxy.py_repr()))))),
        ("CCA_MIN_TRUST", c.at("min_trust")),
    ]);
    sh_array("ACC_ARGS", &t().run_args(&key, &v));
    sh_array("ACC_FLAGS", &t().flags());
    Ok(())
}

/// 第三方 API 账号交给 Claude Code 的环境：接口地址、关掉发往 Anthropic 的遥测和更新检查、模型；
/// 表里 env 写的变量最后覆盖（地址和凭证除外，校验时已经拒绝）
pub fn api_env(v: &Dict) -> Dict {
    let env = dict! { "ANTHROPIC_BASE_URL" => api_url(v), "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" => "1" };
    if let Some(m) = v.get("model").filter(model_ok) {
        for n in ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"] {
            env.set(n, &m);
        }
    }
    if let Some(x) = v.dict("env") {
        for (k, val) in x.items() {
            if ENV_NAME_RE.matches(&k) && !ENV_RESERVED.matches(&k) {
                env.set(k, val.py_str());
            }
        }
    }
    env
}

/// 第三方 API 账号的 claude --settings：key 由 apiKeyHelper 现取（回调 cca _api-key，不进环境变量），
/// 地址、模型写在 env 里。--settings 的优先级高于共享的 settings.json 和进程环境，谁也盖不掉它们
pub fn api_settings(key: &str, v: &Dict) -> String {
    let helper = format!("{} _api-key {}", sh_quote(&SELF_PATH), sh_quote(&acct_id(key, t())));
    json::dumps(&V::Dict(dict! { "apiKeyHelper" => helper, "env" => api_env(v) }), json::LINE)
}

/// ~/.cca 里绝不给 Claude 读写的（在不在工作目录里都挡，用户同意了、bypassPermissions 时也一样）：登录凭证和浏览器资料（映射 deploy key
/// 的私钥、维护者参考节点和内置代理里的节点密钥、Codex 的登录 token、各账号的 Chrome 和桌面 App 资料目录里的 cookie，包括这个账号自己的；
/// 没有钥匙串的系统上 Claude Code 的登录凭证、cca 存的 API key）。~/.cca 里新加了凭证类的文件要补进来
pub const CCA_DIR_PRIVATE: &[&str] = &[
    "policy/id_ed25519",
    "policy/nodes.json",
    "codex/accounts/*/codex/auth.json",
    "accounts/*/chrome/**",
    "accounts/*/desktop/**",
    "codex/accounts/*/chrome/**",
    "codex/accounts/*/desktop/**",
    "core/nodes.json",
    "core/custom-nodes.json",
    "core/config.yaml",
    "accounts/*/claude/.credentials.json",
    "secrets/**",
    "agy/accounts/*/gemini/antigravity-cli/antigravity-oauth-token",
    "agy/accounts/*/chrome/**",
];

/// Claude Code 默认只对这个账号自己的目录（adir：~/.cca/accounts/<账号目录名>）放开权限：加成工作目录（--add-dir），
/// 它的用量、出口检测、Claude Code 自己的配置读起来不用确认，改动照权限模式走。整个 ~/.cca 不加：账号表、config.json、映射、
/// 别的账号的目录都不在工作目录里，读之前照权限模式确认。登录凭证和浏览器资料用 --disallowedTools 挡住整个 ~/.cca 的
/// （CCA_DIR_PRIVATE，包括这个账号自己的 Chrome、桌面 App 资料目录；规则里 // 开头是绝对路径，Read 规则也管 cat、head
/// 这类命令）。两个参数都写成 --x=值：它们能跟好几个值，分开写会把后面用户写的提示词也吞进去。
/// 路径里有空格、逗号、括号、引号时规则写不准，就整个不加。config.json 的 claude_add_dir 设成 false 不加
pub fn cca_dir_args(key: &str) -> Vec<String> {
    let off = load_quiet(&CFG).and_then(|c| c.as_dict()).and_then(|c| c.get("claude_add_dir")).is_some_and(|x| x.as_bool() == Some(false));
    let root = path::realpath(&CCA_HOME);
    if off || re!(r"[\s,()\x22\x27\\]").found(&root) {
        return Vec::new();
    }
    let mut rules = Vec::new();
    for p in CCA_DIR_PRIVATE {
        for tool in ["Read", "Edit"] {
            rules.push(format!("{tool}(/{root}/{p})"));
        }
    }
    vec![format!("--add-dir={}", adir(key)), format!("--disallowedTools={}", rules.join(","))]
}

/// TOML 的基本字符串（JSON 的字符串写法正好是它的子集）
pub fn toml_str(s: &str) -> String {
    json::dump_str(s, false)
}

/// 第三方 API 账号交给 codex 的 -c：只给这次启动用的模型服务（OpenAI Responses 接口）。key 由 auth.command 回调
/// cca _api-key codex 账号目录名 现取，不进环境变量和命令行；-c 的优先级高于共享的 config.toml
pub fn codex_api_args(key: &str, v: &Dict) -> Vec<String> {
    let id = acct_id(key, t());
    let args: Vec<String> = ["_api-key", "codex", id.as_str()].iter().map(|x| toml_str(x)).collect();
    let auth = format!("{{command = {}, args = [{}]}}", toml_str(&SELF_PATH), args.join(", "));
    let prov = format!("{{name = {}, base_url = {}, wire_api = {}, auth = {}}}", toml_str(&name_of(key)), toml_str(&api_url(v)), toml_str("responses"), auth);
    let mut out = vec![
        "-c".to_string(),
        format!("model_provider={}", toml_str(CODEX_PROVIDER)),
        "-c".to_string(),
        format!("model_providers.{CODEX_PROVIDER}={prov}"),
    ];
    if let Some(m) = v.get("model").filter(model_ok) {
        out.push("-c".to_string());
        out.push(format!("model={}", toml_str(&m.py_str())));
    }
    out
}

/// NO_PROXY 写法 → Chromium --proxy-bypass-list 写法（和 Claude App 自己的转换一致）
pub fn bypass_list(no_proxy: &str) -> String {
    let mut out = Vec::new();
    for e in no_proxy.split(',').map(text::strip) {
        if !e.is_empty() {
            out.push(if e.contains(':') {
                format!("[{e}]")
            } else if e.starts_with('.') {
                format!("*{e}")
            } else {
                e.to_string()
            });
        }
    }
    out.join(";")
}

impl Tool {
    /// `T.flags()`：cca 运行这个工具时都要加的参数
    pub fn flags(&self) -> Vec<String> {
        match self.kind {
            Kind::Codex => probe_flags(&codex_probe(&env_or_empty("CCA_BIN"))),
            Kind::Claude | Kind::Agy => Vec::new(),
        }
    }

    /// `T.run_args(key, v)`：启动账号时加在用户参数前面的
    pub fn run_args(&self, key: &str, v: &V) -> Vec<String> {
        match self.kind {
            // 这个账号自己的目录加成工作目录（cca_dir_args）；第三方 API 账号再加 --settings（地址、模型和 apiKeyHelper）
            Kind::Claude => {
                let mut out = cca_dir_args(key);
                if let Some(d) = v.as_dict() {
                    out.push("--settings".to_string());
                    out.push(api_settings(key, &d));
                }
                out
            }
            // --no-daemon：不挂到常驻的后台进程上（它的代理定死在它启动那一刻）；第三方 API 账号再加模型服务的 -c
            Kind::Codex => {
                let x = codex_probe(&env_or_empty("CCA_BIN"));
                let mut out = if x.at("no_daemon").truthy() { vec!["--no-daemon".to_string()] } else { Vec::new() };
                out.extend(probe_flags(&x));
                if let Some(d) = v.as_dict() {
                    out.extend(codex_api_args(key, &d));
                }
                out
            }
            // 数据目录换成账号目录：--gemini_dir 要写在子命令前面，所以放在用户参数前面
            Kind::Agy => vec![format!("--gemini_dir={}", cdir(key))],
        }
    }
}

/// `codex_probe(…)['flags']` 里的每一项（照 `str()`）
fn probe_flags(x: &Dict) -> Vec<String> {
    match x.at("flags") {
        V::List(l) => l.items().iter().map(|f| f.py_str()).collect(),
        other => panic!("TypeError: codex 探测结果里的 flags 不是列表（{}）", other.py_repr()),
    }
}
