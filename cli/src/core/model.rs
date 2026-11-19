//! config.json 和账号表：读进来、键怎么拆、账号目录叫什么、谁有端口。对应 PY_SRC 的「配置与账号表」一节。
//!
//! 账号表的值是字符串（官方账号，值是节点）或字典（第三方 API 账号）：读值一律用 `is_api` / `node_of`。
//! 依赖账号先后的地方（端口池、路由器的摘要、启动器的颜色）用 `base_order`，不直接遍历账号表。

use super::base::*;
use super::tool::{claude, t, tools, Tool};
use crate::py::fs::{load_json, load_quiet};
use crate::py::hash::sha1_hex;
use crate::py::ipaddr;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::text;
use crate::py::url::urlsplit;
use crate::py::{die, R};
use crate::re;
use std::rc::Rc;

/// `cfg_load()`
pub fn cfg_load() -> R<Dict> {
    let c = match load_json(&CFG)? {
        Some(V::Dict(d)) => d,
        Some(v) if v.truthy() => panic!("config.json 不是 JSON 对象（{}）", v.py_repr()),
        _ => Dict::new(),
    };
    Ok(cfg_fill(c))
}

/// config.json 里没写的项补上默认值（补在后面，原有的项顺序不变）
pub fn cfg_fill(c: Dict) -> Dict {
    c.setdefault("version", 2);
    c.setdefault("router", Dict::new());
    c.setdefault("proxy_host", "");
    c.setdefault("no_proxy", "localhost,127.0.0.1,::1,.local,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16");
    c.setdefault("nodes", V::List(Default::default())); // 账号可选的出口节点，cca router setup 时从路由器的节点里选
    c.setdefault("min_trust", 50);
    c.setdefault("update", "auto"); // auto：自动安装新版本；notify：只提示；off：不检查
    c.setdefault("route", "auto"); // auto：连得上路由器走路由器，连不上走内置代理；router / local：固定一种
    c.setdefault("local", Dict::new());
    c.setdefault("ports", Dict::new());
    c.setdefault("last", "");
    c.setdefault("synced", "");
    c
}

/// `table_load(tool)`：这个工具的账号表；格式不对就报错退出
pub fn table_load(tool: &Tool) -> R<Dict> {
    let table = match load_json(&tool.table)? {
        None => Dict::new(),
        Some(V::Dict(d)) => d,
        Some(_) => return die(format!("{} 应该是 {{\"别名(邮箱)\": \"节点\"}} 形式的 JSON 对象", tool.table)),
    };
    tool.set_ids(&table);
    tool.set_tab(Some(table.clone()));
    Ok(table)
}

/// 当前工具的账号表（格式不对就报错退出），以及别的工具的账号表（格式不对就当它暂时不在：它记着的端口
/// 照样留着，不影响当前工具）
pub fn tables_load() -> R<()> {
    for tool in tools() {
        if tool.is_current() {
            table_load(tool)?;
            continue;
        }
        let table = if path::exists(&tool.table) { load_quiet(&tool.table) } else { Some(V::Dict(Dict::new())) };
        match table {
            Some(V::Dict(d)) => {
                tool.set_ids(&d);
                tool.set_tab(Some(d));
            }
            _ => tool.set_tab(None),
        }
    }
    Ok(())
}

/// 不读账号表的子命令（link、sync、email…）也要按 id 认第三方 API 账号；账号表读不了时当作没有
pub fn load_ids() {
    for tool in tools() {
        if let Some(V::Dict(d)) = load_quiet(&tool.table) {
            tool.set_ids(&d);
        }
    }
}

/// 这一项是第三方 API 账号（值是字典）
pub fn is_api(v: &V) -> bool {
    v.is_dict()
}

/// 账号走的出口节点；直连的第三方 API 账号是空字符串
pub fn node_of(v: &V) -> Rc<str> {
    let n = match v {
        V::Dict(d) => d.get("node"),
        x => Some(x.clone()),
    };
    match n {
        Some(V::Str(s)) => s,
        _ => Rc::from(""),
    }
}

/// 账号的先来后到（键的列表）：端口池、路由器的摘要、启动器的颜色都按它，调整账号表的顺序（cca reorder）不牵连它们。
/// 没调整过时就是账号表的顺序（和以前逐字节一样）；调整过的话，调整之前的顺序记在 order_base（账号目录名），
/// 之后新加的账号排在后面
pub fn base_order(c: &Dict, table: &Dict, tool: &Tool) -> Vec<Rc<str>> {
    let keys = table.keys();
    let base = match tool.state(c, false).get("order_base") {
        Some(V::List(l)) if !l.is_empty() => l.strs(),
        _ => return keys,
    };
    let rank = |k: &str| {
        let id = acct_id(k, tool);
        base.iter().position(|a| **a == *id).unwrap_or(base.len())
    };
    let mut ranked: Vec<(usize, Rc<str>)> = keys.into_iter().map(|k| (rank(&k), k)).collect();
    ranked.sort_by_key(|(r, _)| *r); // 稳定排序：名次一样的保持账号表里的先后
    ranked.into_iter().map(|(_, k)| k).collect()
}

/// 走节点、有端口的账号 [(键, 值, 端口)]；直连的第三方 API 账号没有端口
pub fn routed(c: &Dict, table: &Dict, tool: &Tool) -> Vec<(Rc<str>, V, V)> {
    let Some(ports) = tool.state(c, false).dict("ports") else {
        return Vec::new();
    };
    base_order(c, table, tool).into_iter().filter_map(|k| ports.get(&k).map(|p| (k.clone(), table.at(&k), p))).collect()
}

/// 端口池里的全部账号 [(工具, 键, 值, 端口)]：先 Claude 后 Codex、agy，各按 base_order。路由器和内置代理
/// 上的端口池都按它来；只有 Claude 账号时和以前一样
pub fn pool(c: &Dict) -> Vec<(&'static Tool, Rc<str>, V, V)> {
    let mut out = Vec::new();
    for tool in tools() {
        let table = tool.tab().unwrap_or_default();
        for (k, v, p) in routed(c, &table, tool) {
            out.push((tool, k, v, p));
        }
    }
    out
}

/// 跨工具列账号时的名字：Claude 账号照旧，别的工具的注明是哪个工具的
pub fn shown(tool: &Tool, key: &str) -> String {
    if tool.kind == claude().kind {
        name_of(key)
    } else {
        format!("{}（{}）", name_of(key), tool.label)
    }
}

/// `api_url(v)`：接口地址，去掉末尾的斜杠
pub fn api_url(v: &Dict) -> String {
    str_or_empty(&v.get("base_url")).trim_end_matches('/').to_string()
}

/// 接口地址的域名：菜单里显示它；钥匙串里的 key 也按它存取（换了服务商，旧 key 不会被发过去）
pub fn api_host(v: &Dict) -> String {
    let u = api_url(v);
    match urlsplit(&u) {
        Ok(p) => p.hostname().unwrap_or(u),
        Err(_) => u,
    }
}

pub fn secret_ok(s: &V) -> bool {
    s.as_str().is_some_and(secret_re_match)
}

/// 官方账号记下备查的 API key 在钥匙串里的账户名（服务名是 T.kc_ref_prefix + 账号目录名）
pub const REF_ACCT: &str = "reference";

/// 给人看的 key 掩码（App 里默认显示它，config.json 的 key_hints 里存的也是它）：开头 8 个字符里最后一个 - 以前的前缀
/// （sk-ant-、sk-proj-）+ … + 最后 4 个字符；短于 24 个字符的不给（露出的比例太大），是空字符串
pub fn key_hint(s: &str) -> String {
    if !secret_re_match(s) || text::len(s) < 24 {
        return String::new();
    }
    let head = text::head(s, 8);
    let prefix = match head.rfind('-') {
        Some(i) => &s[..i + 1],
        None => "",
    };
    format!("{}…{}", prefix, text::tail(s, 4))
}

pub fn model_ok(m: &V) -> bool {
    m.as_str().is_some_and(|s| MODEL_RE.matches(s))
}

/// 账号表的键 → (别名, 邮箱)。键可以是「别名(邮箱)」、只写邮箱，或只写别名（登录页不预填邮箱）
pub fn parse_key(key: &str) -> (String, String) {
    if let Some(m) = PAIR_RE.match_(key) {
        if EMAIL_RE.matches(m.g(2)) {
            return (m.g(1).to_string(), m.g(2).to_string());
        }
    }
    if EMAIL_RE.matches(key) {
        (String::new(), key.to_string())
    } else {
        (key.to_string(), String::new())
    }
}

/// 菜单和提示里的短名字：有别名用别名，否则用邮箱
pub fn name_of(key: &str) -> String {
    let (a, e) = parse_key(key);
    if a.is_empty() {
        e
    } else {
        a
    }
}

pub fn label_of(key: &str) -> String {
    let (a, e) = parse_key(key);
    if !a.is_empty() && !e.is_empty() {
        format!("{a}（{e}）")
    } else if a.is_empty() {
        e
    } else {
        a
    }
}

/// 账号目录的名字：看得懂的一截（小写字母数字，最多 20 个字符）加上原文的 sha1 的前 6 位。
/// 已有的账号目录都叫这个名字，一个字符都不能变
pub fn ident(k: &str) -> String {
    let slug = re!(r"[^a-z0-9]+").sub("-", &k.to_lowercase());
    let slug = text::head(slug.trim_matches('-'), 20).trim_matches('-');
    format!("{}-{}", if slug.is_empty() { "acct" } else { slug }, &sha1_hex(k)[..6])
}

/// 有邮箱就按邮箱认账号：改别名不换账号目录，登录和浏览器资料都还在；第三方 API 账号按 id 认。
/// 各工具的账号目录在不同的地方（sroot），同一个邮箱的 Claude 账号和 Codex 账号不会撞
pub fn acct_id(key: &str, tool: &Tool) -> String {
    let email = parse_key(key).1;
    if !email.is_empty() {
        return ident(&email);
    }
    match tool.id_of(key) {
        Some(id) if !id.is_empty() => ident(&id),
        _ => ident(key),
    }
}

/// 当前工具里这个账号的目录
pub fn adir(key: &str) -> String {
    path::join_all(&[&t().sroot, "accounts", &acct_id(key, t())])
}

/// 账号目录里交给工具的配置目录
pub fn cdir(key: &str) -> String {
    path::join(&adir(key), t().sub)
}

/// 第三方 API 的地址有什么问题；没问题返回空字符串
pub fn url_problem(u: &str, tool: &Tool) -> String {
    let ex = tool.url_example;
    if u.is_empty() || re!(r"[\s\x22\x27\\]").found(u) {
        return "base_url 不能为空，也不能有空格或引号".into();
    }
    let not_url = || format!("base_url 不像网址（例如 {ex}）");
    let Ok(p) = urlsplit(u) else {
        return not_url();
    };
    let host = p.hostname().unwrap_or_default();
    if p.port().is_err() {
        return not_url(); // 端口写错
    }
    if !matches!(p.scheme.as_str(), "http" | "https") || host.is_empty() {
        return format!("base_url 要以 https:// 开头（例如 {ex}）");
    }
    let truthy = |x: Option<&str>| x.is_some_and(|s| !s.is_empty());
    if truthy(p.username()) || truthy(p.password()) || !p.query.is_empty() || !p.fragment.is_empty() {
        return "base_url 里不能带用户名、密码、? 参数或 # 片段".into();
    }
    if p.scheme == "http" {
        let local = match ipaddr::ip_address(&host) {
            Ok(ip) => ip.ip.is_private() || ip.ip.is_loopback(),
            Err(_) => host == "localhost" || host.ends_with(".local"),
        };
        if !local {
            return "base_url 要用 https（http 只能用于本机、局域网地址或 .local，免得 key 明文传出去）".into();
        }
    }
    String::new()
}

/// `'、'.join(列表)`
pub fn join_names(items: &V) -> String {
    match items {
        V::List(l) => l.items().iter().map(|x| x.py_str()).collect::<Vec<_>>().join("、"),
        _ => String::new(),
    }
}

/// 第三方 API 账号这一项的问题（不带账号名的前缀，不回显 key 的值）
pub fn api_problems(c: &Dict, k: &str, v: &Dict, tool: &Tool) -> Vec<String> {
    if !tool.has_api {
        return vec![format!("{} 没有第三方 API 账号：值只能写节点名", tool.label)]; // agy：只有 Google 账号
    }
    let mut errs = Vec::new();
    if !parse_key(k).1.is_empty() || ALIAS_BAD.found(k) {
        errs.push("第三方 API 账号的键只写别名（不能有邮箱、括号、空格、引号或 @）".to_string());
    }
    let bad: Vec<Rc<str>> = v.keys().into_iter().filter(|f| !tool.api_fields.contains(&&**f)).collect();
    if !bad.is_empty() {
        errs.push(format!("不认识的字段 {}（能用的有 {}）", bad.join("、"), tool.api_fields.join("、")));
    }
    let p = match v.get("base_url") {
        Some(V::Str(u)) => url_problem(&u, tool),
        _ => "缺少 base_url（第三方 API 的地址）".to_string(),
    };
    if !p.is_empty() {
        errs.push(p);
    }
    if let Some(m) = v.get("model").filter(|m| !m.is_none()) {
        if !model_ok(&m) {
            errs.push(format!("model：{MODEL_MSG}"));
        }
    }
    match v.get("node") {
        None | Some(V::None) => {}
        Some(V::Str(n)) if n.is_empty() => {}
        Some(V::Str(n)) => {
            let nodes = c.at("nodes");
            if nodes.truthy() && !nodes.as_list().is_some_and(|l| l.has_str(&n)) {
                errs.push(format!("节点「{}」不在可选列表（{}）", n, join_names(&nodes)));
            }
        }
        Some(_) => errs.push("node 应该是节点名，直连写空字符串".to_string()),
    }
    // 没有 env 字段的工具：上面已经报过不认识
    let env = if tool.api_fields.contains(&"env") { v.get("env").filter(|e| !e.is_none()) } else { None };
    if let Some(e) = &env {
        if !e.is_dict() {
            errs.push("env 应该是 {\"变量名\": \"值\"} 形式".to_string());
        }
    }
    if let Some(V::Dict(env)) = env {
        for (x, val) in env.items() {
            if !ENV_NAME_RE.matches(&x) {
                errs.push(format!("env 里的变量名「{x}」不合法"));
            } else if ENV_RESERVED.matches(&x) {
                errs.push(format!("env 里不能设 {x}（由 cca 管：地址写 base_url，key 存钥匙串或写 key 字段）"));
            } else if ENV_SECRETISH.found(&x) {
                errs.push(format!("env 里不要放密钥（{x}）：env 的内容会出现在 claude 的命令行里；key 写 key 字段或存钥匙串"));
            }
            let plain = matches!(val, V::Str(_) | V::Int(_) | V::Big(_) | V::Float(_));
            if !plain || val.py_str().contains(['\0', '\r', '\n']) {
                errs.push(format!("env 里 {x} 的值应该是一行文字或数字"));
            }
        }
    }
    if let Some(key) = v.get("key").filter(|x| !x.is_none()) {
        if !secret_ok(&key) {
            errs.push(SECRET_MSG.to_string());
        }
    }
    if let Some(i) = v.get("id").filter(|x| !x.is_none()) {
        if !i.as_str().is_some_and(|s| KEY_RE.matches(s) && !s.contains('@')) {
            errs.push("id 应该是不含空格、引号和 @ 的一串文字（改别名时别动它）".to_string());
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::{agy, codex};
    use crate::{dict, list};

    #[test]
    fn keys_and_ids() {
        assert_eq!(parse_key("主号(me@example.com)"), ("主号".into(), "me@example.com".into()));
        assert_eq!(parse_key("me@example.com"), (String::new(), "me@example.com".into()));
        assert_eq!(parse_key("api别名"), ("api别名".into(), String::new()));
        assert_eq!(parse_key("x(not-an-email)"), ("x(not-an-email)".into(), String::new()));
        assert_eq!(label_of("主号(me@example.com)"), "主号（me@example.com）");
        assert_eq!((name_of("主号(me@example.com)"), name_of("me@example.com")), ("主号".into(), "me@example.com".into()));
        // 右边是 Python 的 ident()
        assert_eq!(ident("me@example.com"), "me-example-com-7197f7");
        assert_eq!(ident("Alice.Smith+Work@Example.COM"), "alice-smith-work-exa-93227e");
        assert_eq!(ident("中文别名"), "acct-09d4b8");
        assert_eq!(ident("a--very--long--alias--name--here"), "a-very-long-alias-na-38a4ad");
        assert_eq!(acct_id("主号(me@example.com)", claude()), ident("me@example.com"));
    }

    #[test]
    fn node_and_hint() {
        assert_eq!(&*node_of(&V::from("US-A")), "US-A");
        assert_eq!(&*node_of(&V::Dict(dict! { "node" => "HK-A" })), "HK-A");
        assert_eq!(&*node_of(&V::Dict(dict! { "node" => 1 })), "");
        assert_eq!(&*node_of(&V::Int(3)), "");
        assert_eq!(key_hint("sk-ant-api03-abcdefghijklmnopqrstuvwxyz"), "sk-ant-…wxyz");
        assert_eq!(key_hint("abcdefghijklmnopqrstuvwxyz0123"), "…0123");
        assert_eq!(key_hint("sk-short-key"), "");
    }

    #[test]
    fn key_hint_like_python_test() {
        // tests/usage_view.py 的 test_key_hint：给人看的 key 掩码
        assert_eq!(key_hint("sk-ant-api03-0123456789abcdefghijWXYZ"), "sk-ant-…WXYZ");
        assert_eq!(key_hint("sk-proj-0123456789abcdefghijEFGH"), "sk-proj-…EFGH");
        assert_eq!(key_hint("abcdefghijklmnopqrstuvwxyz"), "…wxyz");
        assert_eq!(key_hint("sk-short-1234"), ""); // 太短：露出的比例太大
        assert_eq!(key_hint("has space in the key 0123456789"), "");
        for k in ["sk-ant-api03-0123456789abcdefghijWXYZ", "abcdefghijklmnopqrstuvwxyz"] {
            assert!(crate::core::base::HINT_RE.matches(&key_hint(k)), "{k}"); // ^[!-~]{0,8}…[!-~]{4}$
        }
    }

    #[test]
    fn url_problems() {
        for (u, want) in [
            ("https://api.example.com/anthropic", ""),
            ("http://127.0.0.1:8080/v1", ""),
            ("http://192.168.1.9/v1", ""),
            ("http://box.local/v1", ""),
            ("http://localhost:1234", ""),
            ("", "base_url 不能为空，也不能有空格或引号"),
            ("https://a b", "base_url 不能为空，也不能有空格或引号"),
            ("http://api.example.com/v1", "base_url 要用 https（http 只能用于本机、局域网地址或 .local，免得 key 明文传出去）"),
            ("ftp://x/", "base_url 要以 https:// 开头（例如 https://api.example.com/anthropic）"),
            ("api.example.com", "base_url 要以 https:// 开头（例如 https://api.example.com/anthropic）"),
            ("https://u:p@x/", "base_url 里不能带用户名、密码、? 参数或 # 片段"),
            ("https://x/?a=1", "base_url 里不能带用户名、密码、? 参数或 # 片段"),
            ("https://x:99999/", "base_url 不像网址（例如 https://api.example.com/anthropic）"),
            ("https://[::1/", "base_url 不像网址（例如 https://api.example.com/anthropic）"),
        ] {
            assert_eq!(url_problem(u, claude()), want, "{u}");
        }
    }

    #[test]
    fn api_account_problems() {
        let c = dict! { "nodes" => list!["US-A"] };
        let ok = dict! { "base_url" => "https://api.example.com/v1", "model" => "m-1", "node" => "US-A", "env" => dict! { "X" => "1", "N" => 2 } };
        assert!(api_problems(&c, "别名", &ok, claude()).is_empty());
        let bad = dict! { "base_url" => 1, "model" => "a b", "node" => "ZZ", "extra" => 1, "key" => "short",
                          "env" => dict! { "1X" => "a", "HTTPS_PROXY" => "x", "MY_TOKEN" => "x", "OK" => true, "NL" => "a\nb" }, "id" => "a@b" };
        assert_eq!(
            api_problems(&c, "别名(me@example.com)", &bad, claude()),
            [
                "第三方 API 账号的键只写别名（不能有邮箱、括号、空格、引号或 @）",
                "不认识的字段 extra（能用的有 base_url、model、node、env、key、id）",
                "缺少 base_url（第三方 API 的地址）",
                "model：模型名里不能有空格或引号",
                "节点「ZZ」不在可选列表（US-A）",
                "env 里的变量名「1X」不合法",
                "env 里不能设 HTTPS_PROXY（由 cca 管：地址写 base_url，key 存钥匙串或写 key 字段）",
                "env 里不要放密钥（MY_TOKEN）：env 的内容会出现在 claude 的命令行里；key 写 key 字段或存钥匙串",
                "env 里 OK 的值应该是一行文字或数字",
                "env 里 NL 的值应该是一行文字或数字",
                "key 应该是一串可见的 ASCII 字符（至少 8 个，不能有空格或中文）",
                "id 应该是不含空格、引号和 @ 的一串文字（改别名时别动它）",
            ]
        );
        assert_eq!(api_problems(&c, "x", &ok, agy()), ["Antigravity 没有第三方 API 账号：值只能写节点名"]);
        assert_eq!(api_problems(&c, "x", &dict! { "base_url" => "https://a.example.com", "env" => dict! {} }, codex()), ["不认识的字段 env（能用的有 base_url、model、node、key、id）"]);
    }

    #[test]
    fn order_and_pool() {
        let c = cfg_fill(dict! { "ports" => dict! { "b@example.com" => 7902, "a@example.com" => 7901 } });
        let table = dict! { "b@example.com" => "US-A", "a@example.com" => "US-B", "直连" => dict! { "base_url" => "https://x.example.com" } };
        let keys = |v: Vec<Rc<str>>| v.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        assert_eq!(keys(base_order(&c, &table, claude())), ["b@example.com", "a@example.com", "直连"]);
        c.set("order_base", list![ident("a@example.com")]);
        assert_eq!(keys(base_order(&c, &table, claude())), ["a@example.com", "b@example.com", "直连"]);
        let r = routed(&c, &table, claude());
        assert_eq!(r.iter().map(|(k, _, p)| (k.to_string(), p.as_int().unwrap())).collect::<Vec<_>>(), [("a@example.com".to_string(), 7901), ("b@example.com".to_string(), 7902)]);
    }
}
