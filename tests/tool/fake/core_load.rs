//! core-load 配置文件：假内核读配置（cca 生成的内置代理配置是 JSON 写的），原来的 $T/core_load。假 mihomo 起来、假控制接口重载时用。
//! 组成员、dialer-proxy、入站指向的都要有，入站只能在 127.0.0.1 上；core/reject_text 里的文字出现在配置里就不收（模拟内核不认某个节点）。
//! 收下的：listening、groups/、sel/（选组里第一个，像 store-selected: false），控制接口的 socket 记在 sock，配置留一份 last_config.json。
//! 状态目录是 FAKE_CORE_DIR（假 mihomo 起辅助安装、TUN、实测 Team 节点的临时内核时各给一个），没有就是 $CCA_T/core。不收时说一句、退出码 1

use super::claude::arg;
use super::*;

pub fn run(a: &[String]) -> R<i32> {
    let cd = match env("FAKE_CORE_DIR").filter(|d| !d.is_empty()) {
        Some(d) => d,
        None => join(&[&cca_t()?, "core"]),
    };
    let text = read_text(arg(a, 0)?)?;
    let rj = join(&[&cd, "reject_text"]);
    if exists(&rj) {
        let r = read_text(&rj)?;
        let r = py::strip(&r);
        if !r.is_empty() && text.contains(r) {
            return fail("proxy 1: unsupport proxy type");
        }
    }
    let c = json::parse(&text).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))?;
    let gs = list_of(get(&c, "proxy-groups")?)?;
    let proxies = list_of(get(&c, "proxies")?)?;
    let mut known: Vec<Value> = Vec::new();
    for p in proxies.iter().chain(&gs) {
        known.push(item(p, "name")?.clone());
    }
    known.extend([Value::from("REJECT"), Value::from("DIRECT")]);
    let has = |v: &Value| known.iter().any(|k| k == v);
    for p in &proxies {
        if let Some(d) = get(p, "dialer-proxy")?.filter(|v| truthy(v)) {
            if !has(d) {
                return fail(format!("proxy {}: dialer-proxy {} not found", py_str(item(p, "name")?), py_str(d)));
            }
        }
    }
    for g in &gs {
        for m in super::claude::iter_list(item(g, "proxies")?)? {
            if !has(&m) {
                return fail(format!("proxy group {}: {} not found", py_str(item(g, "name")?), py_str(&m)));
            }
        }
    }
    let listeners = list_of(get(&c, "listeners")?)?;
    for l in &listeners {
        if !has(get(l, "proxy")?.unwrap_or(&Value::Null)) || get(l, "listen")? != Some(&Value::from("127.0.0.1")) {
            return fail(format!("listener {}: bad", py_str(get(l, "name")?.unwrap_or(&Value::Null))));
        }
    }
    let mut listening = String::new();
    for l in &listeners {
        match int_fmt(item(l, "port")?) {
            Ok(n) => listening.push_str(&format!("{n}\n")),
            Err(e) => return truncated(&join(&[&cd, "listening"]), e),
        }
    }
    write_text(&join(&[&cd, "listening"]), &listening)?;
    if basename(&cd) == "core_test" {
        // 实测 Team 节点的临时内核：listeners/<端口> 是「入站名\t节点」（假 curl 照它回出口）
        let dir = join(&[&cd, "listeners"]);
        makedirs(&dir)?;
        for f in glob_all(&dir) {
            remove(&f)?;
        }
        for l in &listeners {
            let body = format!("{}\t{}\n", py_str(item(l, "name")?), py_str(item(l, "proxy")?));
            write_text(&join(&[&dir, &py_str(item(l, "port")?)]), &body)?;
        }
    }
    let mut old = glob_all(&join(&[&cd, "groups"]));
    old.extend(glob_all(&join(&[&cd, "sel"])));
    for f in old {
        remove(&f)?;
    }
    for g in &gs {
        let name = name_of(item(g, "name")?)?;
        let members = super::claude::iter_list(item(g, "proxies")?)?;
        let mut body = String::new();
        for m in &members {
            match name_of(m) {
                Ok(m) => body.push_str(&format!("{m}\n")),
                Err(e) => return truncated(&join(&[&cd, "groups", name]), e),
            }
        }
        write_text(&join(&[&cd, "groups", name]), &body)?;
        match members.first().map(name_of) {
            Some(Ok(first)) => write_text(&join(&[&cd, "sel", name]), first)?,
            Some(Err(e)) => return truncated(&join(&[&cd, "sel", name]), e),
            None => return truncated(&join(&[&cd, "sel", name]), Fail("IndexError: list index out of range".into())),
        }
    }
    let sock = get(&c, "external-controller-unix")?.cloned().unwrap_or_else(|| Value::from(""));
    match name_of(&sock) {
        Ok(s) => write_text(&join(&[&cd, "sock"]), s)?,
        Err(e) => return truncated(&join(&[&cd, "sock"]), e),
    }
    write_text(&join(&[&cd, "last_config.json"]), &text)?;
    Ok(0)
}

/// open(p, 'w').write(算出错的东西)：文件先被清空了，再是那个异常
fn truncated(p: &str, e: Fail) -> R<i32> {
    write_text(p, "")?;
    Err(e)
}

/// c.get(k) or []
fn list_of(v: Option<&Value>) -> R<Vec<Value>> {
    match v.filter(|v| truthy(v)) {
        None => Ok(Vec::new()),
        Some(v) => super::claude::iter_list(v),
    }
}

/// 要拿来拼路径、写进文件的：得是字符串
fn name_of(v: &Value) -> R<&str> {
    v.as_str().ok_or_else(|| Fail(format!("TypeError: expected str, got {}", type_name(v))))
}

/// '%d' % v
fn int_fmt(v: &Value) -> R<String> {
    match v {
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(u8::from(*b).to_string()),
        Value::Float(f) if f.is_finite() => Ok(format!("{}", f.trunc() as i128)),
        other => fail(format!("TypeError: %d format: a real number is required, not {}", type_name(other))),
    }
}

/// glob.glob(dir + '/*')：目录里不以 . 开头的（目录不在就是空的）
fn glob_all(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .map(|n| join(&[dir, &n]))
        .collect()
}
