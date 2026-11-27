//! fake-usage：假 claude 带着 --input-format stream-json 起来时（cca 查用量）扮演 Claude Code 的 stream-json 控制接口——
//! 按账号目录里登录的邮箱回 $CCA_T/usage/<邮箱>.json（没有这个文件就当没有订阅用量）；
//! usage/unsupported：回“不支持”；usage/hang：卡住 60 秒再回。原来的 fake_usage.py。

use super::*;
use kit::json::Obj;
use std::io::BufRead;

pub fn run(_a: &[String]) -> R<i32> {
    let t = cca_t()?;
    let cd = env("CLAUDE_CONFIG_DIR").unwrap_or_default();
    let j = load_json(&join(&[&cd, ".claude.json"])).unwrap_or_else(|_| Value::Obj(Obj::new()));
    let none = Value::Obj(Obj::new());
    let acct = get(&j, "oauthAccount")?.filter(|v| truthy(v)).unwrap_or(&none);
    let email = get(acct, "emailAddress")?.filter(|v| truthy(v)).cloned().unwrap_or_else(|| Value::from(""));
    let shown = if truthy(&email) { py_str(&email) } else { "-".to_string() };
    append_text(
        &join(&[&t, "claude.log"]),
        &format!("USAGE: email={shown} https_proxy={} cwd={}\n", env_or("https_proxy", "unset"), getcwd()?),
    )?;
    print_flush(&dumps(&obj! { "type" => "system", "subtype" => "init" }));
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if input.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let Ok(m) = json::parse(&String::from_utf8_lossy(&buf)) else { continue };
        if get(&m, "type")? != Some(&Value::from("control_request")) {
            continue;
        }
        let rid = get(&m, "request_id")?.cloned().unwrap_or(Value::Null);
        let req = get(&m, "request")?.filter(|v| truthy(v)).unwrap_or(&none);
        let sub = get(req, "subtype")?.cloned().unwrap_or(Value::Null);
        if exists(&join(&[&t, "usage", "hang"])) {
            sleep(60.0);
        }
        let Value::Str(e) = &email else {
            return fail(format!("TypeError: unsupported operand type(s) for +: '{}' and 'str'", type_name(&email)));
        };
        let p = join(&[&t, "usage", &format!("{e}.json")]);
        let r = if exists(&join(&[&t, "usage", "unsupported"])) || sub != Value::from("get_usage") {
            obj! { "subtype" => "error", "request_id" => rid, "error" => format!("Unsupported control request subtype: {}", py_str(&sub)) }
        } else if !e.is_empty() && exists(&p) {
            let mut resp = load_json(&p)?;
            let Value::Obj(o) = &mut resp else {
                return fail(format!("TypeError: '{}' object is not a mapping", type_name(&resp)));
            };
            o.insert("session", Value::Obj(Obj::new()));
            o.insert("behaviors", Value::Null);
            obj! { "subtype" => "success", "request_id" => rid, "response" => resp }
        } else {
            let resp = obj! {
                "session" => Value::Obj(Obj::new()),
                "subscription_type" => Value::Null,
                "rate_limits_available" => false,
                "rate_limits" => Value::Null,
                "behaviors" => Value::Null,
            };
            obj! { "subtype" => "success", "request_id" => rid, "response" => resp }
        };
        print_flush(&dumps(&obj! { "type" => "control_response", "response" => r }));
    }
    Ok(0)
}
