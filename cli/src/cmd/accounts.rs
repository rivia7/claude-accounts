//! env、rows / node-rows / node-default、check、add / add-api / rename / reorder / rm、set-last、edit-form / edit-apply、prompt-emit、
//! nodes-allowed / set-nodes / nodes-preselect / nodes-lost / select-lines、sync-state、slot-rows、accounts-json、file-replace（PY_SRC 的 `cmd_env` 到 `cmd_rm`、
//! `cmd_nodes_allowed` 到 `cmd_set_nodes`、`cmd_slot_rows`）。
//! （select-lines、slot-rows、file-replace 登记在 corenet 里，accounts-json 在 table 里。）

use super::codex::codex_probe;
use super::table::{account_named, need};
use super::{arg, arity, Cmd};
use crate::core::base::*;
use crate::core::ctx::{cfg_save, ctx, policy_skip_add, table_problems};
use crate::core::model::*;
use crate::core::policy::{alias_problem, policy_in_use, policy_map, policy_raw};
use crate::core::tool::{t, tools, Kind, Tool, CODEX_PROVIDER};
use crate::py::fs::{load_quiet, save_json};
use crate::py::hash::sha1_hex;
use crate::py::io;
use crate::py::json;
use crate::py::obj::{Dict, List, V};
use crate::py::path;
use crate::py::proc::env_or_empty;
use crate::py::text::{self, sh_quote};
use crate::py::uni;
use crate::py::{die, exit, Fail, R};
use crate::{dict, err, out, re};
use std::collections::HashMap;
use std::rc::Rc;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("env", cmd_env),
    Cmd::all("prompt-emit", cmd_prompt_emit),
    Cmd::all("check", cmd_check),
    Cmd::all("add", cmd_add),
    Cmd::all("add-api", cmd_add_api),
    Cmd::all("rename", cmd_rename),
    Cmd::all("reorder", cmd_reorder),
    Cmd::all("rm", cmd_rm),
    Cmd::all("rows", cmd_rows),
    Cmd::all("set-last", cmd_set_last),
    Cmd::all("node-rows", cmd_node_rows),
    Cmd::all("node-default", cmd_node_default),
    Cmd::all("nodes-allowed", cmd_nodes_allowed),
    Cmd::all("nodes-preselect", cmd_nodes_preselect),
    Cmd::all("nodes-lost", cmd_nodes_lost),
    Cmd::all("set-nodes", cmd_set_nodes),
    Cmd::all("edit-form", cmd_edit_form),
    Cmd::all("edit-apply", cmd_edit_apply),
    Cmd::all("sync-state", cmd_sync_state),
    Cmd::all("node-assign", cmd_node_assign),
];

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
    let host = c
        .get("proxy_host")
        .filter(|h| h.truthy())
        .unwrap_or_else(|| c.dict("router").and_then(|r| r.get("host")).unwrap_or_else(|| "".into()));
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
        (
            "ACC_PROXY",
            match port_num {
                Some(p) if host.truthy() => V::from(format!("http://{}:{}", host.py_str(), p)),
                _ => empty(),
            },
        ),
        ("ACC_BASE_URL", api.as_ref().map_or_else(empty, |d| V::from(api_url(d)))),
        ("ACC_MODEL", api.as_ref().and_then(|d| d.get("model")).filter(|m| m.truthy()).unwrap_or_else(empty)),
        ("ACC_DIR", V::from(&dir)),
        ("ACC_HOME", V::from(cdir(&key))),
        ("ACC_MASTER", V::from(&t().master)),
        ("ACC_CHROME_DIR", V::from(path::join(&dir, "chrome"))),
        ("ACC_DESKTOP_DIR", V::from(path::join(&dir, "desktop"))),
        ("CCA_NO_PROXY", no_proxy.clone()),
        (
            "CCA_PROXY_BYPASS",
            V::from(bypass_list(
                no_proxy.as_str().unwrap_or_else(|| panic!("AttributeError: no_proxy 不是字符串（{}）", no_proxy.py_repr())),
            )),
        ),
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
        for n in
            ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"]
        {
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
/// 地址、模型写在 env 里。--settings 的优先级高于共享的 settings.json 和进程环境，谁也盖不掉它们。另带着 session_hook
pub fn api_settings(key: &str, v: &Dict) -> String {
    let helper = format!("{} _api-key {}", sh_quote(&SELF_PATH), sh_quote(&acct_id(key, t())));
    let settings = dict! { "apiKeyHelper" => helper, "env" => api_env(v) };
    if let Some(h) = session_hook(key) {
        settings.set("hooks", h);
    }
    json::dumps(&V::Dict(settings), json::LINE)
}

/// 经 --settings 交给 Claude Code 的 hooks：SessionStart（会话开始、续接、/clear、压缩以后）回调 cca _session-start 账号目录名，
/// 把会话 id 记进这个账号目录的 sessions.jsonl（cost.rs 的 cmd_session_start）。折合金额靠它认出没有 session-owner 的会话
/// 是哪个账号的（Claude Code 2.1.316 及以前不写 session-owner），第三方 API 账号的会话靠它不算进来。
/// 和用户自己的 settings.json 里的 hook 一起跑（--settings 不顶掉它们，实测 2.1.328），/hooks 里看得到这一条；
/// 用户自己再写 --settings 时 Claude Code 只认后写的那个，这一条就没了（那些会话照旧按 session-owner 认）。折合金额关掉了不加
pub fn session_hook(key: &str) -> Option<Dict> {
    if !super::cost::cost_on() {
        return None;
    }
    let command = format!("{} _session-start {}", sh_quote(&SELF_PATH), sh_quote(&acct_id(key, t())));
    let hook = dict! { "type" => "command", "command" => command, "timeout" => 10 };
    Some(dict! { "SessionStart" => crate::list![dict! { "hooks" => crate::list![hook] }] })
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
    let off = load_quiet(&CFG)
        .and_then(|c| c.as_dict())
        .and_then(|c| c.get("claude_add_dir"))
        .is_some_and(|x| x.as_bool() == Some(false));
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
    let prov = format!(
        "{{name = {}, base_url = {}, wire_api = {}, auth = {}}}",
        toml_str(&name_of(key)),
        toml_str(&api_url(v)),
        toml_str("responses"),
        auth
    );
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

/// 提问会话（PROTOCOL 4.6）：把一页提问写进事件文件，打印序号。描述从 stdin 读（k=v，一行一条；field= 开始一个字段）。
/// 序号 = 文件里已有的 ask 事件数 + 1：提问常在子 shell 里，变量带不出来
pub fn cmd_prompt_emit(args: &[String]) -> R<()> {
    arity("cmd_prompt_emit", args, 0, 0)?;
    let Some(events) = crate::py::proc::getenv("CCA_EVENTS").filter(|s| !s.is_empty()) else {
        return die("cca(py): 没有事件文件");
    };
    let fields = List::new();
    let page = dict! { "v" => 1, "ev" => "ask", "seq" => 0, "q" => "", "title" => "", "message" => "", "fields" => &fields };
    let mut cur: Option<Dict> = None;
    let input = io::read_stdin_text();
    for line in input.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        if matches!(k, "q" | "title" | "message" | "submit") {
            page.set(k, v);
            continue;
        }
        if k == "field" {
            let f = dict! { "id" => v, "kind" => "text", "label" => "", "default" => "", "value" => "", "error" => "", "hint" => "", "optional" => false };
            fields.push(&f);
            cur = Some(f);
            continue;
        }
        let Some(cur) = &cur else { continue };
        match k {
            "prompt" => {
                if !cur.truthy("label") {
                    cur.set("label", prompt_label(v));
                }
            }
            "optional" => cur.set(k, v == "1"),
            "opt" => {
                let mut parts = v.split('\t').chain(["", ""]);
                let (value, label, note) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let opts = match cur.list("options") {
                    Some(l) => l,
                    None => {
                        let l = List::new();
                        cur.set("options", &l);
                        l
                    }
                };
                opts.push(dict! { "value" => value, "label" => label, "note" => note });
            }
            "min" => {
                let n = text::to_int(v).ok_or_else(|| {
                    Fail::Value(format!("invalid literal for int() with base 10: {}", crate::py::obj::repr_str(v)))
                })?;
                cur.set(k, n);
            }
            // 多行文字（kind 是 code 的字段）：一行一条
            "code" => {
                let old = cur.at("value").py_str();
                cur.set("value", if old.is_empty() { v.to_string() } else { format!("{old}\n{v}") });
            }
            // 空的 label 不盖掉已有的（prompt 那一行可能已经给过名字）
            "kind" | "label" | "default" | "value" | "error" | "hint" | "detail" | "severity" | "skip" | "section" | "style"
                if !v.is_empty() || k != "label" =>
            {
                cur.set(k, v)
            }
            _ => {}
        }
    }
    let seen = std::fs::read(&events)?;
    let asks = seen.split_inclusive(|b| *b == b'\n').filter(|row| row.windows(10).any(|w| w == b"\"ev\":\"ask\"")).count();
    page.set("seq", asks + 1);
    let mut line = json::dumps(&V::Dict(page), json::Fmt::new().utf8().compact());
    line.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).create(true).open(&events)?.write_all(line.as_bytes())?;
    out!("{}", asks + 1);
    Ok(())
}

/// 没有专门写名字时，用终端的提示语当名字（去掉前后的空白、冒号和方括号里的默认值）
pub fn prompt_label(p: &str) -> String {
    let s = re!(r"\s*\[[^\]]*\]\s*[：:]?\s*$").sub("", text::strip(p));
    text::rstrip_chars(&s, "：: ").to_string()
}

/// 这不像邮箱、或者这个邮箱已经在账号表里了；没问题返回空字符串
pub fn email_problem(table: &Dict, email: &str) -> String {
    if !EMAIL_RE.matches(email) {
        return "这不像邮箱地址（例如 name@example.com）".into();
    }
    for k in table.keys() {
        if parse_key(&k).1.to_lowercase() == email.to_lowercase() {
            return format!("这个邮箱已经添加过了：{}", label_of(&k));
        }
    }
    String::new()
}

/// 新增账号时逐项校验：合格时打印规范化后的值（邮箱转小写），不合格时打印原因并返回 1
pub fn cmd_check(args: &[String]) -> R<()> {
    arity("cmd_check", args, 2, 2)?;
    let field = args[0].as_str();
    let mut value = text::strip(&args[1]).to_string();
    match field {
        "email" | "gmail" => value = value.to_lowercase(),
        "url" => value = value.trim_end_matches('/').to_string(),
        _ => {}
    }
    let problem = |ok: bool, msg: &str| if ok { String::new() } else { msg.to_string() };
    let p = if value.is_empty() {
        String::new()
    } else {
        match field {
            // Gmail 地址（小写；也是钥匙串的账户名，交给 security -i）
            "gmail" => problem(
                re!(r"^[a-z0-9._%+-]+@[a-z0-9-]+(\.[a-z0-9-]+)+$").matches(&value),
                "这不像邮箱地址（只认字母、数字和 . _ % + - 组成的地址）",
            ),
            "url" => url_problem(&value, t()),
            "model" => problem(MODEL_RE.matches(&value), MODEL_MSG),
            "host" => problem(HOST_RE.matches(&value), "地址写成 192.168.1.2 或者主机名（不带 http://、端口和空格）"),
            "sshuser" => problem(SSH_USER_RE.matches(&value), "用户名只能有字母、数字、点、下划线和连字符"),
            "port" => {
                let ok = re!(r"^[0-9]{1,5}$").matches(&value) && text::to_int(&value).is_some_and(|n| (1..=65535).contains(&n));
                problem(ok, "端口是 1–65535 之间的数字")
            }
            "controller" => {
                problem(CONTROLLER_RE.matches(&value), "写成 127.0.0.1:9090、http://地址:端口，Unix socket 写 unix:/路径")
            }
            "email" => email_problem(&table_load(t())?, &value),
            _ => alias_problem(&table_load(t())?, &value),
        }
    };
    out!("{}", if p.is_empty() { &value } else { &p });
    exit(if p.is_empty() { 0 } else { 1 })
}

/// 新增账号，打印账号表里的键：有别名是「别名(邮箱)」，没有就是邮箱。维护者的映射里有这个邮箱时，节点用映射的
pub fn cmd_add(args: &[String]) -> R<()> {
    arity("cmd_add", args, 3, 3)?;
    let (c, table, _) = ctx()?;
    let (email, alias) = (text::strip(&args[0]).to_lowercase(), text::strip(&args[1]).to_string());
    let mut p = email_problem(&table, &email);
    if p.is_empty() && !alias.is_empty() {
        p = alias_problem(&table, &alias);
    }
    if !p.is_empty() {
        return die(p);
    }
    let want = policy_map(t(), None).get(&email).filter(|n| n.truthy()).map(|n| n.py_str());
    let node = want.clone().unwrap_or_else(|| args[2].clone());
    let nodes = c.at("nodes");
    if nodes.truthy() && !nodes.as_list().is_some_and(|l| l.has_str(&node)) {
        let why = if want.is_some() {
            "（维护者的映射要求这个账号走它：运行 cca router setup 重新选可选节点）"
        } else {
            ""
        };
        return die(format!("节点「{node}」不在可选列表{why}"));
    }
    let key = if alias.is_empty() { email.clone() } else { format!("{alias}({email})") };
    table.set(key.as_str(), &node);
    save_json(&t().table, &V::Dict(table.clone()), Some(0o600), true)?;
    let st = t().state(&c, false);
    if let Some(V::List(skip)) = st.get("policy_skip") {
        // 删掉过的映射里的账号又加回来了：以后照常跟着映射
        if skip.has_str(&email) {
            let rest: Vec<V> = skip.items().into_iter().filter(|e| e.as_str() != Some(email.as_str())).collect();
            if rest.is_empty() {
                st.pop("policy_skip");
            } else {
                st.set("policy_skip", rest);
            }
            cfg_save(&c)?;
        }
    }
    ctx()?;
    out!("{key}");
    Ok(())
}

/// 新增第三方 API 账号，打印账号表里的键（就是别名）。id 取别名（和别的账号目录撞了就加数字），改别名时它不变
pub fn cmd_add_api(args: &[String]) -> R<()> {
    arity("cmd_add_api", args, 2, 4)?;
    let (c, table, _) = ctx()?;
    let alias = text::strip(&args[0]).to_string();
    let e = dict! { "base_url" => text::strip(&args[1]).trim_end_matches('/') };
    let model = text::strip(arg(args, 2, ""));
    if !model.is_empty() {
        e.set("model", model);
    }
    e.set("node", arg(args, 3, ""));
    let mut p = if alias.is_empty() { "需要一个别名".to_string() } else { alias_problem(&table, &alias) };
    if p.is_empty() {
        p = api_problems(&c, &alias, &e, t()).join("；");
    }
    if !p.is_empty() {
        return die(p);
    }
    let taken: Vec<String> = table.keys().iter().map(|k| acct_id(k, t())).collect();
    let (mut id, mut n) = (alias.clone(), 2);
    while taken.contains(&ident(&id)) {
        id = format!("{alias}-{n}");
        n += 1;
    }
    e.set("id", id);
    table.set(alias.as_str(), e);
    save_json(&t().table, &V::Dict(table.clone()), Some(0o600), true)?;
    ctx()?;
    out!("{alias}");
    Ok(())
}

/// 改别名（cca rename，图形界面的“改别名…”也用它）：只换账号表里的键，值和顺序都不变。官方账号按邮箱认、
/// 第三方 API 账号按 id 认，账号目录、端口、“上次使用”、路由器的摘要都跟着走（acct_id 不变）。新别名写 - 去掉别名、只留邮箱。
/// 成功时打印新的键；不行时把原因打到 stdout、退出码 3（bash 把它报成 alias_invalid）
pub fn cmd_rename(args: &[String]) -> R<()> {
    arity("cmd_rename", args, 2, 2)?;
    let (_, table, _) = ctx()?;
    let key = args[0].as_str();
    need(&table, key)?;
    let mut alias = text::strip(&args[1]).to_string();
    if alias == "-" {
        alias.clear();
    }
    let email = parse_key(key).1;
    let api = is_api(&table.at(key));
    let bad = |msg: String| -> R<()> {
        out!("{msg}");
        exit(3)
    };
    if !api && email.is_empty() {
        // 只写了别名的官方账号：按别名认，换了别名就换了账号目录
        return bad(format!(
            "{} 的键里没有邮箱：改名会换掉它的账号目录（登录、浏览器资料都在里面）。先用 {} edit 补上邮箱，写成「别名(邮箱)」",
            key,
            t().cmd
        ));
    }
    let new = if alias.is_empty() {
        if api {
            return bad("第三方 API 账号必须有别名".to_string());
        }
        email
    } else {
        let others = Dict::from_pairs(table.items().into_iter().filter(|(k, _)| &**k != key));
        let p = alias_problem(&others, &alias);
        if !p.is_empty() {
            return bad(p);
        }
        if api {
            alias
        } else {
            format!("{alias}({email})")
        }
    };
    if new != key {
        let renamed =
            Dict::from_pairs(table.items().into_iter().map(|(k, v)| (if &*k == key { new.clone() } else { k.to_string() }, v)));
        save_json(&t().table, &V::Dict(renamed.clone()), Some(0o600), true)?;
        t().set_ids(&renamed); // 第三方 API 账号：新键 → id，对账时端口、“上次使用”按 id 跟着走
        ctx()?;
    }
    out!("{new}");
    Ok(())
}

/// cca reorder：按这个顺序排账号表（没写到的账号接在后面，先后不变）。图形界面调整账号顺序时用它。
/// 只改账号表里的先后；第一次调整之前先记下原来的顺序（base_order），端口池、摘要、颜色都不跟着变
pub fn cmd_reorder(args: &[String]) -> R<()> {
    let _lock = Lock::acquire()?;
    let (c, table, _) = ctx()?;
    let mut order: Vec<String> = Vec::new();
    for n in args {
        let k = account_named(&table, n)?;
        if !order.contains(&k) {
            order.push(k);
        }
    }
    for k in table.keys() {
        if !order.iter().any(|o| **o == *k) {
            order.push(k.to_string());
        }
    }
    if !order.iter().map(String::as_str).eq(table.keys().iter().map(|k| &**k)) {
        let base: Vec<String> = base_order(&c, &table, t()).iter().map(|k| acct_id(k, t())).collect();
        t().state(&c, false).set("order_base", crate::py::obj::str_list(base));
        cfg_save(&c)?;
        let sorted = Dict::from_pairs(order.iter().map(|k| (k.as_str(), table.at(k))));
        save_json(&t().table, &V::Dict(sorted), Some(0o600), true)?;
    }
    Ok(())
}

pub fn cmd_rm(args: &[String]) -> R<()> {
    arity("cmd_rm", args, 1, 1)?;
    let table = table_load(t())?;
    let key = args[0].as_str();
    need(&table, key)?;
    let e = if is_api(&table.at(key)) { String::new() } else { parse_key(key).1.to_lowercase() };
    if !e.is_empty() && policy_raw(t()).has(&e) {
        // 映射里的账号：记下来，不然下一条命令又按映射把它加回来
        policy_skip_add(t(), std::slice::from_ref(&e))?;
        err!("• {} 在维护者的映射里：这台机器上不会再自动加回来（要加回来：{} add）", label_of(key), t().cmd);
    }
    table.pop(key);
    save_json(&t().table, &V::Dict(table.clone()), Some(0o600), true)?;
    ctx()?;
    Ok(())
}

/// 账号表 t 里用这个节点的账号。选节点只看当前工具的：每个账号最好用不同的出口，是免得同一家服务商把几个账号
/// 关联起来，Claude 账号和 Codex 账号是两家的，共用一个出口不要紧
pub fn node_users(table: &Dict, n: &str) -> Vec<String> {
    table.items().iter().filter(|(_, v)| &*node_of(v) == n).map(|(k, _)| name_of(k)).collect()
}

/// `max(int(cols) if str(cols).isdigit() else 80, 40) - 6`：菜单一行最多多宽
fn menu_width(cols: &str) -> usize {
    let n = if text::isdigit(cols) { text::to_int(cols).unwrap_or(80) } else { 80 };
    (n.max(40) - 6) as usize
}

/// 菜单里每个账号一行：别名、邮箱（第三方 API 账号写接口的域名）、状态、出口节点，官方账号再附上最近一次的
/// 用量（只读缓存，不联网）；按显示宽度对齐（没有别名的账号别名列留空）
pub fn cmd_rows(args: &[String]) -> R<()> {
    arity("cmd_rows", args, 0, 1)?;
    let (c, table, _) = ctx()?;
    if table.is_empty() {
        return Ok(());
    }
    let width = menu_width(arg(args, 0, "80"));
    let now = crate::py::time::time();
    // (键, 别名, 邮箱 / 域名, 状态, 节点, 用量)
    let mut rows: Vec<(Rc<str>, String, String, String, String, String)> = Vec::new();
    for (k, v) in table.items() {
        let (a, e) = parse_key(&k);
        if is_api(&v) {
            let node = node_of(&v);
            let node = if node.is_empty() { "直连".to_string() } else { node.to_string() };
            rows.push((k, a, api_host(&v.as_dict().expect("is_api 说它是字典")), "API".into(), node, String::new()));
        } else {
            let st = super::gui::short_state(&k)?;
            let used = if st == "已登录" { super::usage::usage_cell(&k, now)? } else { String::new() };
            rows.push((k, a, e, st.to_string(), node_of(&v).to_string(), used));
        }
    }
    let widest = |col: fn(&(Rc<str>, String, String, String, String, String)) -> &String| {
        rows.iter().map(|r| uni::width(col(r))).max().unwrap_or(0)
    };
    let (aw, ew, nw) = (widest(|r| &r.1).min(16), widest(|r| &r.2).min(32), widest(|r| &r.4));
    let last = t().state(&c, false).get("last");
    for (k, a, e, state, node, used) in &rows {
        let mut cells = Vec::new();
        if aw != 0 {
            cells.push(text::fit(a, aw, true));
        }
        if ew != 0 {
            cells.push(text::fit(e, ew, true));
        }
        cells.push(text::fit(state, 8, true));
        cells.push(text::fit(node, nw, true));
        if !used.is_empty() {
            cells.push(used.clone());
        }
        let mut line = text::rstrip(&cells.join("   ")).to_string();
        if last.as_ref().is_some_and(|l| l.as_str() == Some(&**k)) {
            line.push_str("   · 上次使用");
        }
        out!("{}", text::fit(&line, width, false));
    }
    Ok(())
}

/// 记下这次启动的账号（菜单里默认选它）。顺便打印启动横幅里套餐那一句：Claude Code 记的套餐过时了才有（login_plan_note），
/// 启动时本来就要起这个辅助程序，不为它再起一个
pub fn cmd_set_last(args: &[String]) -> R<()> {
    arity("cmd_set_last", args, 1, 1)?;
    let key = &args[0];
    let c = cfg_load()?;
    t().state(&c, true).set("last", key.as_str());
    cfg_save(&c)?;
    let old = t().outdated_login_plan(key)?;
    if !old.is_empty() {
        out!("{}", super::usage::login_plan_note(key, &old, &t().plan(key, &V::None)?));
    }
    Ok(())
}

/// 选节点菜单：每个可选节点一行，注明已被当前工具的哪些账号使用、路由器上是否存在
pub fn cmd_node_rows(args: &[String]) -> R<()> {
    arity("cmd_node_rows", args, 0, 2)?;
    let (c, table, _) = ctx()?;
    let onrouter = arg(args, 0, "");
    let have: Option<Vec<&str>> = if onrouter.is_empty() { None } else { Some(onrouter.split('\n').collect()) };
    let width = menu_width(arg(args, 1, "80"));
    let nodes: Vec<String> = c.at("nodes").as_list().map(|l| l.items().iter().map(|n| n.py_str()).collect()).unwrap_or_default();
    let nw = nodes.iter().map(|n| uni::width(n)).chain([8]).max().unwrap_or(8);
    for n in &nodes {
        let mut notes = Vec::new();
        let users = node_users(&table, n);
        if !users.is_empty() {
            notes.push(format!("已被 {} 使用", users.join("、")));
        }
        if have.as_ref().is_some_and(|h| !h.contains(&n.as_str())) {
            notes.push("路由器上没有这个节点".to_string());
        }
        let line = text::fit(&format!("{}   {}", text::fit(n, nw, true), notes.join("；")), width, false);
        out!("{}", text::rstrip(&line));
    }
    Ok(())
}

/// 默认选中第一个还没被当前工具的账号用的节点（别的工具的账号不算，见 node_users）
pub fn cmd_node_default(args: &[String]) -> R<()> {
    arity("cmd_node_default", args, 0, 0)?;
    let (c, table, _) = ctx()?;
    let used: Vec<Rc<str>> = table.values().iter().map(node_of).collect();
    let nodes = c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default();
    let first = nodes.iter().position(|n| !used.iter().any(|u| n.as_str() == Some(&**u))).unwrap_or(0);
    out!("{first}");
    Ok(())
}

pub fn cmd_nodes_allowed(args: &[String]) -> R<()> {
    arity("cmd_nodes_allowed", args, 0, 0)?;
    let (c, table, _) = ctx()?;
    for n in c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default() {
        let n = n.py_str();
        out!("{}\t{}", n, node_users(&table, &n).join("、"));
    }
    Ok(())
}

/// 路由器上的这些节点里，预先选好的编号（从 1 起，空格隔开）：现在的可选节点，加上维护者的映射里这台机器的账号要用的
/// （配置向导第 1 步拉到映射、第 2 步配置路由器时不用自己找；这台机器删掉过的账号不算）
pub fn cmd_nodes_preselect(args: &[String]) -> R<()> {
    let c = ctx()?.0;
    let mut want: Vec<String> = c.at("nodes").as_list().map(|l| l.items().iter().map(|n| n.py_str()).collect()).unwrap_or_default();
    for tool in tools() {
        if policy_in_use(tool) {
            let skip: Vec<Rc<str>> = match tool.state(&c, false).get("policy_skip") {
                Some(V::List(l)) => l.strs(),
                _ => Vec::new(),
            };
            for (e, n) in policy_map(tool, Some(&c)).items() {
                if !skip.contains(&e) {
                    want.push(n.py_str());
                }
            }
        }
    }
    let picked: Vec<String> = args.iter().enumerate().filter(|(_, n)| want.contains(n)).map(|(i, _)| (i + 1).to_string()).collect();
    out!("{}", picked.join(" "));
    Ok(())
}

/// 这次选的节点里少了哪些还有账号在用的（各工具的都看）：「节点（账号、账号）」
pub fn cmd_nodes_lost(args: &[String]) -> R<()> {
    let c = ctx()?.0;
    let mut who: Vec<(Rc<str>, Vec<String>)> = Vec::new();
    for (tool, k, v, _) in pool(&c) {
        let node = node_of(&v);
        match who.iter_mut().find(|(n, _)| *n == node) {
            Some((_, names)) => names.push(shown(tool, &k)),
            None => who.push((node, vec![shown(tool, &k)])),
        }
    }
    let mut lost = Vec::new();
    for n in c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default() {
        let n = n.py_str();
        if let Some((_, names)) = who.iter().find(|(x, _)| **x == *n) {
            if !args.contains(&n) {
                lost.push(format!("{}（{}）", n, names.join("、")));
            }
        }
    }
    out!("{}", lost.join("；"));
    Ok(())
}

pub fn cmd_set_nodes(args: &[String]) -> R<()> {
    let mut nodes: Vec<&str> = Vec::new(); // dict.fromkeys(nodes)：去重，保持先后
    for n in args {
        if !n.is_empty() && !nodes.contains(&n.as_str()) {
            nodes.push(n);
        }
    }
    if nodes.is_empty() {
        return die("至少选一个节点");
    }
    let c = cfg_load()?;
    c.set("nodes", crate::py::obj::str_list(nodes));
    cfg_save(&c)
}

/// 这个值在的时候 stderr 接到 /dev/null（`os.dup2(null, 2)`）：里面说的话（包括 die 的那一句、panic 的那一句）都不出声；
/// 没了的时候接回去
struct QuietStderr {
    saved: i32,
    null: i32,
}

impl QuietStderr {
    fn new() -> R<QuietStderr> {
        io::flush_err();
        // SAFETY: 都是对这个进程自己的描述符做 dup / open / dup2，路径是以 0 结尾的常量
        unsafe {
            let saved = libc::dup(2);
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if saved < 0 || null < 0 || libc::dup2(null, 2) < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(QuietStderr { saved, null })
        }
    }
}

impl Drop for QuietStderr {
    fn drop(&mut self) {
        io::flush_err();
        // SAFETY: saved、null 是 new 里开的，还开着
        unsafe {
            libc::dup2(self.saved, 2);
            libc::close(self.saved);
            libc::close(self.null);
        }
    }
}

/// sync_if_changed 开头要的一次给（几乎每条命令都走这里，原来要起四次 python：validate、secrets-forget、digest、
/// get synced）。账号表有错就返回 1，错误写 stderr；quiet 时校验这一步的 stderr 全不要（和 py validate 2>/dev/null 一样）。
/// 没错就清掉已经不在账号表里的 key（secrets-forget），再打印「账号表 + 路由器端代码（stdin）的摘要<TAB>上次同步的摘要」
pub fn cmd_sync_state(args: &[String]) -> R<()> {
    arity("cmd_sync_state", args, 0, 1)?;
    let quiet = !arg(args, 0, "").is_empty();
    let checked = {
        let _silence = if quiet { Some(QuietStderr::new()?) } else { None };
        ctx().map(|(_, _, errs)| {
            for e in &errs {
                err!("! {e}");
            }
            errs
        })
    };
    let errs = match checked {
        Ok(errs) => errs,
        // quiet：die 的那一句已经写进了 /dev/null；出了意外也只返回 1（同样不说），交给不带 quiet 的那次说清楚
        Err(e) if quiet && !matches!(e, Fail::Exit(_)) => return exit(1),
        Err(e) => return Err(e),
    };
    if !errs.is_empty() {
        return exit(1);
    }
    super::secrets::cmd_secrets_forget(&[])?;
    let c = ctx()?.0;
    let synced = c.get("synced").filter(|s| s.truthy()).map_or(String::new(), |s| s.py_str());
    out!("{}\t{}", crate::core::sync::sync_digest(&c, &io::read_stdin_text()), synced);
    Ok(())
}

// --- 提问会话里的账号表表单（cca [codex] edit，PROTOCOL 4.6）：节点、接口地址、模型、环境变量。App 不碰账号表，也看不到 key ---

/// 账号表的键和先后的摘要：表单按编号对应账号，表在中途变了（映射更新、别处改了别名）就重新出表单
pub fn edit_sum(table: &Dict) -> String {
    let keys = V::List(List::from_vec(table.keys().into_iter().map(V::Str).collect()));
    text::head(&sha1_hex(json::dumps(&keys, json::LINE)), 12).to_string()
}

/// 环境变量在表单里的写法：A=1 B='带空格的值'
pub fn env_text(env: Option<&V>) -> String {
    match env {
        Some(V::Dict(d)) => d.items().iter().map(|(k, v)| format!("{}={}", k, sh_quote(&v.py_str()))).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// 表单里的环境变量 → (字典, 问题)
pub fn env_parse(s: &str) -> (Dict, String) {
    let Ok(words) = text::sh_split(s) else {
        return (Dict::new(), "引号没有配对".to_string());
    };
    let out = Dict::new();
    for w in &words {
        let (k, sep, v) = text::partition(w, "=");
        if sep.is_empty() {
            return (Dict::new(), format!("写成 名字=值，多个用空格隔开（「{}」里没有 =）", text::head(k, 40)));
        }
        out.set(k, v);
    }
    (out, String::new())
}

/// 表单改不了的问题（键写错了、有不认识的字段、key 或 id 不对）：把表单能改的项换成合格的值，再校验一遍
fn edit_unfixable(c: &Dict, table: &Dict, pm: &Dict) -> Vec<String> {
    let probe = Dict::new();
    for (k, v) in table.items() {
        match v.as_dict().filter(|_| is_api(&v)) {
            Some(d) => {
                let p = Dict::from_pairs(d.items().into_iter().filter(|(f, _)| !matches!(&**f, "model" | "node" | "env")));
                p.set("base_url", "https://api.example.com");
                probe.set(k, p);
            }
            None => probe.set(k, "x"),
        }
    }
    let c = c.copy();
    c.set("nodes", crate::list!["x"]);
    table_problems(&c, &probe, t(), pm)
}

/// stdin 里「字段<TAB>值」的各行（没有 TAB 的行不算；同一个字段后写的算数）
fn read_fields() -> Vec<(String, String)> {
    let input = io::read_stdin_text();
    input
        .split('\n')
        .map(|line| text::partition(line, "\t"))
        .filter(|(_, sep, _)| !sep.is_empty())
        .map(|(k, _, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// 表单的一个字段（给 prompt-emit 的 k=v 各行；空着的不写，field 和 value 总是写）
#[derive(Default)]
struct Field<'a> {
    id: &'a str,
    kind: &'a str,
    label: &'a str,
    value: &'a str,
    section: &'a str,
    hint: &'a str,
    optional: bool,
    style: &'a str,
    error: String,
}

/// 表单的一页：打印 edit_sum，后面是这一页的描述（k=v，一行一条，给 prompt-emit）。stdin：上次填的（字段<TAB>值）和问题（!字段<TAB>原因，
/// 字段空着是整页的）。退出码 3：表单改不了（stdout 是原因，去终端）；4：还没有账号；5：还没有可选节点（stdout 是原因）
pub fn cmd_edit_form(args: &[String]) -> R<()> {
    arity("cmd_edit_form", args, 0, 0)?;
    let tool = t();
    if !path::exists(&tool.table) || !load_quiet(&tool.table).is_some_and(|j| j.is_dict()) {
        if path::exists(&tool.table) {
            out!("{} 不是正确的 JSON", tool.table);
            return exit(3);
        }
        out!("还没有账号");
        return exit(4);
    }
    let (c, table, _) = ctx()?;
    let (mut prev, mut ferr): (HashMap<String, String>, HashMap<String, String>) = Default::default();
    for (k, v) in read_fields() {
        let which = if k.starts_with('!') { &mut ferr } else { &mut prev };
        which.insert(k.trim_start_matches('!').to_string(), v);
    }
    if table.is_empty() {
        out!("还没有账号");
        return exit(4);
    }
    let pm = policy_map(tool, Some(&c));
    let bad = edit_unfixable(&c, &table, &pm);
    if !bad.is_empty() {
        out!("{}", text::head(&bad.join("；"), 500));
        return exit(3);
    }
    let nodes = c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default();
    let in_nodes = |s: &str| nodes.iter().any(|n| n.as_str() == Some(s));
    if nodes.is_empty() && table.values().iter().any(|v| !is_api(v)) {
        out!("还没有可选节点：先配置路由器");
        return exit(5);
    }
    let mut used: Vec<(Rc<str>, Vec<Rc<str>>)> = Vec::new(); // 节点 → 用它的账号
    for (k, v) in table.items() {
        let n = node_of(&v);
        if n.is_empty() {
            continue;
        }
        match used.iter_mut().find(|(x, _)| *x == n) {
            Some((_, keys)) => keys.push(k),
            None => used.push((n, vec![k])),
        }
    }
    let apis = table.values().iter().any(is_api);
    let message = ferr.get("").filter(|m| !m.is_empty()).map_or("改完点“保存”，cca 会检查，需要的话同步到路由器。", |m| m.as_str());
    let mut out: Vec<String> = vec![edit_sum(&table), "q=edit.form".into(), "submit=保存".into(), format!("message={message}")];
    let put = |out: &mut Vec<String>, f: Field| {
        let error = if f.error.is_empty() { ferr.get(f.id).cloned().unwrap_or_default() } else { f.error };
        let rows = [
            ("field", f.id),
            ("kind", f.kind),
            ("label", f.label),
            ("value", f.value),
            ("section", f.section),
            ("hint", f.hint),
            ("style", f.style),
            ("error", &error),
        ];
        out.extend(rows.iter().filter(|(k, v)| !v.is_empty() || matches!(*k, "field" | "value")).map(|(k, v)| format!("{k}={v}")));
        if f.optional {
            out.push("optional=1".into());
        }
    };
    let node_opts = |out: &mut Vec<String>, k: &str| {
        for n in &nodes {
            let users = used.iter().find(|(x, _)| n.as_str() == Some(&**x)).map(|(_, keys)| keys.as_slice()).unwrap_or_default();
            let who: Vec<String> = users.iter().filter(|x| &***x != k).map(|x| name_of(x)).collect();
            let note = if who.is_empty() { String::new() } else { format!("已被 {} 使用", who.join("、")) };
            out.push(format!("opt={0}\t{0}\t{1}", n.py_str(), note));
        }
    };
    let has_env = tool.api_fields.contains(&"env");
    for (i, (k, v)) in table.items().into_iter().enumerate() {
        let nid = format!("n{i}");
        let Some(d) = v.as_dict().filter(|_| is_api(&v)) else {
            let section = if apis { "官方账号" } else { "" };
            let email = parse_key(&k).1;
            let want = if email.is_empty() { None } else { pm.get(&email.to_lowercase()).filter(|w| w.truthy()) };
            let label = label_of(&k);
            if let Some(want) = want {
                let want = want.py_str();
                let error = if in_nodes(&want) {
                    String::new()
                } else {
                    format!("维护者的映射要求它走「{want}」，这个节点不在可选列表：先在“配置路由器”里选上它")
                };
                let (hint, style) = ("维护者固定了这个账号的节点", "menu");
                put(
                    &mut out,
                    Field {
                        id: &nid,
                        kind: "choice",
                        label: &label,
                        value: &want,
                        section,
                        hint,
                        style,
                        error,
                        ..Default::default()
                    },
                );
                out.push(format!("opt={want}\t{want}\t"));
                continue;
            }
            let cur = prev.get(&nid).cloned().unwrap_or_else(|| v.as_str().unwrap_or("").to_string());
            let ok = in_nodes(&cur);
            let error = if ok { String::new() } else { format!("节点「{cur}」不在可选列表，换一个") };
            put(
                &mut out,
                Field {
                    id: &nid,
                    kind: "choice",
                    label: &label,
                    value: if ok { &cur } else { "" },
                    section,
                    style: "menu",
                    error,
                    ..Default::default()
                },
            );
            node_opts(&mut out, &k);
            continue;
        };
        let section = &format!("第三方 API：{}", name_of(&k));
        let fid = format!("u{i}");
        let url = prev.get(&fid).cloned().unwrap_or_else(|| api_url(&d));
        put(
            &mut out,
            Field {
                id: &fid,
                kind: "text",
                label: "接口地址",
                value: &url,
                section,
                hint: "换了服务商（域名变了）要重新输入 key",
                ..Default::default()
            },
        );
        let fid = format!("m{i}");
        let model =
            prev.get(&fid).cloned().unwrap_or_else(|| d.get("model").filter(|m| m.truthy()).map_or(String::new(), |m| m.py_str()));
        let hint = if has_env {
            "填了会同时用作 opus / sonnet / haiku"
        } else {
            "服务商不认 codex 默认的模型名时请填上"
        };
        put(
            &mut out,
            Field {
                id: &fid,
                kind: "text",
                label: "模型（可选）",
                value: &model,
                section,
                hint,
                optional: true,
                ..Default::default()
            },
        );
        if !nodes.is_empty() {
            let cur = prev.get(&nid).cloned().unwrap_or_else(|| node_of(&v).to_string());
            let ok = in_nodes(&cur);
            let error =
                if cur.is_empty() || ok { String::new() } else { format!("节点「{cur}」不在可选列表，换一个") };
            put(
                &mut out,
                Field {
                    id: &nid,
                    kind: "choice",
                    label: "出口",
                    value: if ok { &cur } else { "" },
                    section,
                    optional: true,
                    style: "menu",
                    error,
                    ..Default::default()
                },
            );
            node_opts(&mut out, &k);
            out.push("skip=直连（不经代理）".into());
        }
        if has_env {
            let fid = format!("e{i}");
            let env = prev.get(&fid).cloned().unwrap_or_else(|| env_text(d.get("env").as_ref()));
            let hint = "写成 名字=值，多个用空格隔开；密钥不要写在这里";
            put(
                &mut out,
                Field {
                    id: &fid,
                    kind: "text",
                    label: "环境变量（可选）",
                    value: &env,
                    section,
                    hint,
                    optional: true,
                    ..Default::default()
                },
            );
        }
    }
    out!("{}", out.iter().map(|x| x.replace('\n', " ")).collect::<Vec<_>>().join("\n"));
    Ok(())
}

/// 表单的答案（stdin：字段<TAB>值）：合格就写进账号表，打印 changed / same；不合格退出 3，stdout 一行一个问题（!字段<TAB>原因）
pub fn cmd_edit_apply(args: &[String]) -> R<()> {
    arity("cmd_edit_apply", args, 1, 1)?;
    let tool = t();
    let (c, table, _) = ctx()?;
    if edit_sum(&table) != args[0] {
        out!("!\t账号表刚刚变了（维护者的映射更新了，或者在别处改过）：下面是最新的内容，再确认一次");
        return exit(3);
    }
    let ans: HashMap<String, String> = read_fields().into_iter().collect();
    let answer = |id: &str| ans.get(id).map_or("", |s| s.as_str());
    let pm = policy_map(tool, Some(&c));
    let nodes = c.at("nodes").as_list().map(|l| l.items()).unwrap_or_default();
    let in_nodes = |s: &str| nodes.iter().any(|n| n.as_str() == Some(s));
    let has_env = tool.api_fields.contains(&"env");
    let new = Dict::new();
    let mut bad: Vec<(String, String)> = Vec::new();
    for (i, (k, v)) in table.items().into_iter().enumerate() {
        let nid = format!("n{i}");
        let n = ans.get(&nid);
        let Some(d) = v.as_dict().filter(|_| is_api(&v)) else {
            let email = parse_key(&k).1;
            match n {
                // 维护者固定的不看答案
                Some(n) if email.is_empty() || !pm.has(&email.to_lowercase()) => {
                    if in_nodes(n) {
                        new.set(k, n.as_str());
                    } else {
                        bad.push((nid, "这个节点不在可选列表".into()));
                        new.set(k, v);
                    }
                }
                _ => new.set(k, v),
            }
            continue;
        };
        let w = d.copy();
        let uid = format!("u{i}");
        let url = text::strip(answer(&uid)).trim_end_matches('/');
        let p = url_problem(url, tool);
        if !p.is_empty() {
            bad.push((uid, p));
        } else {
            let moved = w.copy();
            moved.set("base_url", url);
            if api_host(&moved) != api_host(&d) {
                w.pop("key"); // 换了服务商：账号表里写的是旧服务商的 key，不能发给新的（和钥匙串按域名存一样），保存后问新的
            }
            w.set("base_url", url);
        }
        let mid = format!("m{i}");
        let m = text::strip(answer(&mid));
        if m.is_empty() {
            w.pop("model");
        } else if model_ok(&V::from(m)) {
            w.set("model", m);
        } else {
            bad.push((mid, MODEL_MSG.into()));
        }
        if let Some(n) = n {
            let n = text::strip(n);
            if !n.is_empty() && !in_nodes(n) {
                bad.push((nid, "这个节点不在可选列表".into()));
            } else if !n.is_empty() || w.truthy("node") {
                w.set("node", n);
            }
        }
        let eid = format!("e{i}");
        if let Some(raw) = ans.get(&eid).filter(|_| has_env) {
            let (env, mut p) = env_parse(raw);
            if p.is_empty() {
                p = api_problems(&c, &k, &dict! { "base_url" => "https://api.example.com", "env" => &env }, tool).join("；");
            }
            if !p.is_empty() {
                bad.push((eid, p));
            } else if env_text(Some(&V::Dict(env.clone()))) != env_text(d.get("env").as_ref()) {
                if env.is_empty() {
                    w.pop("env");
                } else {
                    w.set("env", env);
                }
            }
        }
        new.set(k, w);
    }
    if !bad.is_empty() {
        for (f, why) in &bad {
            out!("!{f}\t{why}");
        }
        return exit(3);
    }
    if V::Dict(new.clone()).py_eq(&V::Dict(table)) {
        out!("same");
        return Ok(());
    }
    save_json(&tool.table, &V::Dict(new), Some(0o600), true)?;
    ctx()?; // 重新对账（直连改走节点时分配端口）
    out!("changed");
    Ok(())
}

/// 账号能不能自己换出口（cca [codex|agy] node use）：维护者的映射固定了节点的官方账号不能
pub fn node_free(key: &str, v: &V, pm: &Dict) -> bool {
    let email = parse_key(key).1;
    is_api(v) || email.is_empty() || !pm.has(&email.to_lowercase())
}

/// cca [codex|agy] node use 账号 的一头一尾：只给账号（key 是 bash 经 resolve 找好的）时只查能不能换（维护者固定了节点的
/// 退出码 3，stdout 是原因）；再给节点时把这个账号改走它（第三方 API 账号改 node），打印 changed / same。节点要在可选节点里
pub fn cmd_node_assign(args: &[String]) -> R<()> {
    arity("cmd_node_assign", args, 1, 2)?;
    let tool = t();
    let (c, table, _) = ctx()?;
    let key = args[0].as_str();
    let Some(v) = table.get(key) else {
        return die(format!("没找到账号：{key}"));
    };
    if !node_free(key, &v, &policy_map(tool, Some(&c))) {
        out!("维护者的映射固定了 {} 的出口节点，不能换成自定义代理", name_of(key));
        return exit(3);
    }
    let Some(node) = args.get(1) else {
        return Ok(());
    };
    if !c.at("nodes").as_list().is_some_and(|l| l.items().iter().any(|n| n.as_str() == Some(node))) {
        return die(format!("节点「{node}」不在可选列表"));
    }
    let new = Dict::new();
    for (k, w) in table.items() {
        if &*k != key {
            new.set(k, w);
        } else if let Some(d) = w.as_dict().filter(|_| is_api(&w)) {
            let d = d.copy();
            d.set("node", node.as_str());
            new.set(k, d);
        } else {
            new.set(k, node.as_str());
        }
    }
    if V::Dict(new.clone()).py_eq(&V::Dict(table)) {
        out!("same");
        return Ok(());
    }
    save_json(&tool.table, &V::Dict(new), Some(0o600), true)?;
    ctx()?; // 重新对账（直连改走节点时分配端口）
    out!("changed");
    Ok(())
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
            // 这个账号自己的目录加成工作目录（cca_dir_args）；再加 --settings：记下会话是哪个账号的 hook（session_hook），
            // 第三方 API 账号的还有地址、模型和 apiKeyHelper
            Kind::Claude => {
                let mut out = cca_dir_args(key);
                let settings = match v.as_dict() {
                    Some(d) => Some(api_settings(key, &d)),
                    None => session_hook(key).map(|h| json::dumps(&V::Dict(dict! { "hooks" => h }), json::LINE)),
                };
                if let Some(s) = settings {
                    out.push("--settings".to_string());
                    out.push(s);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_free_only_blocks_accounts_the_mapping_pins() {
        // cca node use、can.custom_proxy：维护者的映射固定了节点的官方账号不能换（邮箱不分大小写）；映射里没有的、键里没有邮箱的、
        // 第三方 API 账号（映射只管官方账号）都能换
        let pm = json::loads(r#"{"alice@example.com": "US-A"}"#).unwrap().as_dict().unwrap();
        let official = V::from("US-B");
        let api = json::loads(r#"{"base_url": "https://api.example.com", "node": "US-A"}"#).unwrap();
        assert!(!node_free("alice@example.com", &official, &pm));
        assert!(!node_free("工作(Alice@Example.com)", &official, &pm));
        assert!(node_free("bob@example.com", &official, &pm));
        assert!(node_free("工作", &official, &pm));
        assert!(node_free("接口(alice@example.com)", &api, &pm));
    }

    #[test]
    fn api_env_never_carries_reserved_names() {
        // tests/isolation_settings.py 的 test_claude_configuration_cannot_replace_credential_sources：换掉账号目录、登录凭证来源的
        // 变量就算写进了第三方 API 账号的 env（校验账号表时会拒绝），也不交给 Claude Code。期望值是原来的 api_env 给的
        let env_of = |account: &str| json::dumps(&V::Dict(api_env(&json::loads(account).unwrap().as_dict().unwrap())), json::LINE);
        let bare = r#"{"ANTHROPIC_BASE_URL": "https://example.invalid", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"}"#;
        let reserved = [
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
            "CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR",
        ];
        for name in reserved {
            let account = format!(r#"{{"base_url": "https://example.invalid", "env": {{"{name}": "private-fixture-value"}}}}"#);
            assert_eq!(env_of(&account), bare, "{name}");
        }
        // 和别的变量混在一起：保留的名字（不分大小写，cca 自己的、BROWSER 也是）去掉，别的照带，值都写成文字
        let mixed: Vec<String> = reserved.iter().map(|name| format!(r#""{name}": "private-fixture-value""#)).collect();
        let account = format!(
            r#"{{"base_url": "https://example.invalid", "env": {{{}, "EDITOR": "vi", "API_TIMEOUT_MS": 600000, "cca_x": "1", "browser": "none"}}}}"#,
            mixed.join(", ")
        );
        assert_eq!(
            env_of(&account),
            r#"{"ANTHROPIC_BASE_URL": "https://example.invalid", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "EDITOR": "vi", "API_TIMEOUT_MS": "600000"}"#
        );
        // 模型照 model 字段，env 里写的最后覆盖；不像变量名的不带
        assert_eq!(
            env_of(
                r#"{"base_url": "https://example.invalid/v1/", "model": "m-1", "env": {"ANTHROPIC_MODEL": "other", "1BAD": "x", "A B": "y", "OK_1": 1.5, "T": true, "N": null}}"#
            ),
            concat!(
                r#"{"ANTHROPIC_BASE_URL": "https://example.invalid/v1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "ANTHROPIC_MODEL": "other", "#,
                r#""ANTHROPIC_DEFAULT_OPUS_MODEL": "m-1", "ANTHROPIC_DEFAULT_SONNET_MODEL": "m-1", "ANTHROPIC_DEFAULT_HAIKU_MODEL": "m-1", "#,
                r#""OK_1": "1.5", "T": "True", "N": "None"}"#
            )
        );
        assert_eq!(env_of(r#"{"base_url": "https://example.invalid", "env": ["x"]}"#), bare);
        assert_eq!(env_of(r#"{"base_url": "https://example.invalid", "model": "has space"}"#), bare);
    }
}
