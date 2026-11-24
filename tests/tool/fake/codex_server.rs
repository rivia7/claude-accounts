//! fake-codex-server [--write-auth]：假的 codex app-server（JSON-RPC，一行一个 JSON）。原来的 fake_codex_server.py
//! （假 codex 用 exec -a "$CCA_T/fake_codex_server" 起它，回归按这个名字找进程）。
//! initialize；account/login/start 回登录网址，等 cca 在账号的浏览器里打开它（出现在 open.log 里）就当用户登录完了，写 auth.json
//! （假的 id_token，邮箱默认取 CCA_ACCOUNT 里的，一次性的 codex_login_as 模拟登错号）；$CCA_T/oauth_flow 在时照真的 codex 0.167 开本机回调；
//! account/rateLimits/read 按登录的邮箱回 codex_usage/<邮箱>.json；account/rateLimitResetCredit/consume 用掉一次重置机会。
//! codex_rpc_off：不支持 app-server；codex_usage/unsupported、hang：不支持查用量、卡住；rpc_error：里面的话当出错说明回。
//! --write-auth：只写 auth.json（假 codex 的 login，cca 退回 codex login 时）。
//!
//! id_token 怎么写、怎么读回邮箱（b64、email_now）和 tests/linux.sh 的假 codex 是同一份（linux_codex.rs）：中间那一段只认规规矩矩的
//! base64url（原来 Python 的解码会跳过字母表以外的字符），不规矩的和别的读不出来的情况一样当成没登录。

use super::codex::regex;
use super::linux_codex::{b64, email_now};
use super::*;
use kit::json::Obj;
use std::io::BufRead;

fn log(t: &str, s: &str) -> R<()> {
    append_text(&join(&[t, "codex.log"]), &format!("{s}\n"))
}

/// email_now() or '-'
fn shown(v: &Value) -> String {
    if truthy(v) {
        py_str(v)
    } else {
        "-".to_string()
    }
}

/// email_now() + '.json'
fn json_name(v: &Value) -> R<String> {
    match v {
        Value::Str(s) => Ok(format!("{s}.json")),
        other => fail(format!("TypeError: unsupported operand type(s) for +: '{}' and 'str'", type_name(other))),
    }
}

/// 写登录凭证（假的 id_token），日志里记下设备标识和登录的邮箱
fn write_auth(t: &str, home: &str) -> R<()> {
    let device = join(&[home, "installation_id"]);
    let dev = if exists(&device) { py::strip(&read_text(&device)?).to_string() } else { String::new() };
    log(t, &format!("LOGIN-DEVICE {dev} @ {home}"))?;
    let p = join(&[t, "codex_login_as"]);
    let email = if exists(&p) {
        let e = py::strip(&read_text(&p)?).to_string();
        remove(&p)?;
        e
    } else {
        let a = env_or("CCA_ACCOUNT", "");
        match regex(r"\(([^()]+@[^()]+)\)\n?\z")?.captures(&a) {
            Some(c) => c[1].to_string(),
            None if a.contains('@') => a,
            None => "someone@example.com".to_string(),
        }
    };
    let claims = obj! {
        "email" => email.as_str(),
        "https://api.openai.com/auth" => obj! { "chatgpt_plan_type" => "plus", "chatgpt_account_id" => format!("ws-{email}") },
    };
    let tokens = obj! {
        "id_token" => format!("{}.{}.sig", b64(&obj! { "alg" => "none" }), b64(&claims)),
        "access_token" => "at-SECRET",
        "refresh_token" => "rt-SECRET",
        "account_id" => format!("ws-{email}"),
    };
    let auth = obj! { "auth_mode" => "chatgpt", "OPENAI_API_KEY" => Value::Null, "last_refresh" => "2026-11-03T00:00:00Z", "tokens" => tokens };
    write_text(&join(&[home, "auth.json"]), &dumps(&auth))?;
    log(t, &format!("LOGIN {email} @ {home}"))
}

/// 发一条（一行一个 JSON，马上发出去；几个线程一起发时一条一条地来）
fn send(m: &Value) {
    print_flush(&dumps(m));
}

fn error(rid: &Value, code: i32, msg: &str) -> Value {
    obj! { "id" => rid.clone(), "error" => obj! { "code" => code, "message" => msg } }
}

pub fn run(a: &[String]) -> R<i32> {
    let t = cca_t()?;
    let home = env("CODEX_HOME").unwrap_or_default();
    if a.iter().any(|x| x == "--write-auth") {
        write_auth(&t, &home)?;
        return Ok(0);
    }
    log(
        &t,
        &format!(
            "APPSERVER: CODEX_HOME={home} https_proxy={} NO_PROXY={}",
            env_or("https_proxy", "unset"),
            env_or("NO_PROXY", "unset")
        ),
    )?;
    if exists(&join(&[&t, "codex_rpc_off"])) {
        flush();
        eprintln!("error: unrecognized subcommand 'app-server'");
        return Ok(2);
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut buf = Vec::new();
    let empty = Value::Obj(Obj::new());
    loop {
        buf.clear();
        if input.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let Ok(m) = json::parse(&String::from_utf8_lossy(&buf)) else { continue };
        let meth = get(&m, "method")?.cloned().unwrap_or(Value::Null);
        let rid = get(&m, "id")?.cloned().unwrap_or(Value::Null);
        let meth = meth.as_str().unwrap_or("\u{0}");
        let u = join(&[&t, "codex_usage"]);
        if meth == "initialize" {
            let name = item(item(item(&m, "params")?, "clientInfo")?, "name")?;
            log(&t, &format!("RPC initialize client={}", py_str(name)))?;
            send(&obj! { "id" => rid, "result" => obj! { "userAgent" => "x" } });
        } else if meth == "initialized" {
        } else if meth == "account/login/start" && exists(&join(&[&t, "oauth_flow"])) {
            start_oauth(&t, &home, rid)?;
        } else if meth == "account/login/cancel" {
            log(&t, "RPC loginCancel")?;
            send(&obj! { "id" => rid, "result" => Value::Obj(Obj::new()) });
        } else if meth == "account/login/start" {
            let url = format!("https://auth.openai.com/oauth/authorize?client_id=app_x&state=s{}", std::process::id());
            send(&obj! { "id" => rid, "result" => obj! { "type" => "chatgpt", "loginId" => "L1", "authUrl" => url.as_str() } });
            // 等 cca 在账号的浏览器里打开登录页
            let open_log = join(&[&t, "open.log"]);
            let mut opened = false;
            for _ in 0..100 {
                if exists(&open_log) && read_text(&open_log)?.contains(&url) {
                    opened = true;
                    break;
                }
                sleep(0.1);
            }
            if !opened {
                send(&completed(false, Some("browser never opened")));
                continue;
            }
            write_auth(&t, &home)?;
            send(&completed(true, None));
        } else if meth == "account/rateLimits/read" {
            log(&t, &format!("RPC rateLimits email={} https_proxy={}", shown(&email_now(&home)), env_or("https_proxy", "unset")))?;
            if exists(&join(&[&u, "hang"])) {
                sleep(60.0);
            }
            if exists(&join(&[&u, "unsupported"])) {
                send(&error(&rid, -32601, "Method not found: account/rateLimits/read"));
                continue;
            }
            if exists(&join(&[&u, "rpc_error"])) {
                // 这句话原样当 app-server 的出错说明（登录凭证失效之类）
                send(&error(&rid, -32603, &read_text(&join(&[&u, "rpc_error"]))?));
                continue;
            }
            let p = join(&[&u, &json_name(&email_now(&home))?]);
            if !truthy(&email_now(&home)) {
                send(&error(&rid, -32600, "codex account authentication required"));
            } else if exists(&p) {
                send(&obj! { "id" => rid, "result" => load_json(&p)? });
            } else {
                let rl = obj! { "limitId" => "codex", "primary" => Value::Null, "secondary" => Value::Null, "planType" => "free" };
                send(&obj! { "id" => rid, "result" => obj! { "rateLimits" => rl } });
            }
        } else if meth == "account/rateLimitResetCredit/consume" {
            let prm = get(&m, "params")?.filter(|v| truthy(v)).unwrap_or(&empty).clone();
            reset_consume(&t, &home, &u, &prm, rid)?;
        } else if !rid.is_null() {
            send(&error(&rid, -32601, "Method not found"));
        }
    }
    Ok(0)
}

fn completed(ok: bool, err: Option<&str>) -> Value {
    obj! { "method" => "account/login/completed", "params" => obj! { "loginId" => "L1", "success" => ok, "error" => err } }
}

/// 用掉一次重置机会：按 codex_usage/<邮箱>.json 里的用量和重置机会办——没有（或指定的那一次不在了）是 noCredit，
/// 用量都是 0 是 nothingToReset，否则把 5 小时、每周清零、去掉用掉的那一次（写回文件，之后查用量就是新的）；
/// codex_usage/reset_outcome 里写着结果时照它回，reset_unsupported 模拟旧版本的 codex
fn reset_consume(t: &str, home: &str, u: &str, prm: &Value, rid: Value) -> R<()> {
    let key = get(prm, "idempotencyKey")?.filter(|v| truthy(v)).cloned().unwrap_or_else(|| Value::from(""));
    let Value::Str(key) = key else {
        return fail(format!("TypeError: expected string or bytes-like object, got '{}'", type_name(&key)));
    };
    let uuid_like = regex(r"^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\n?\z")?.is_match(&key);
    let credit_arg = get(prm, "creditId")?.filter(|v| truthy(v)).cloned();
    log(
        t,
        &format!(
            "RPC resetConsume email={} https_proxy={} credit={} key={}",
            shown(&email_now(home)),
            env_or("https_proxy", "unset"),
            credit_arg.as_ref().map_or("-".to_string(), py_str),
            if uuid_like { "uuid".to_string() } else { format!("bad:{key}") }
        ),
    )?;
    if exists(&join(&[u, "reset_unsupported"])) {
        send(&error(&rid, -32601, "Method not found: account/rateLimitResetCredit/consume"));
        return Ok(());
    }
    if !truthy(&email_now(home)) {
        send(&error(&rid, -32600, "codex account authentication required for rate limit reset credits"));
        return Ok(());
    }
    let p = join(&[u, &json_name(&email_now(home))?]);
    let mut j = if exists(&p) {
        load_json(&p)?
    } else {
        obj! { "rateLimits" => Value::Obj(Obj::new()) }
    };
    let rc_default = obj! { "availableCount" => 0, "credits" => Value::List(Vec::new()) };
    let rc = get(&j, "rateLimitResetCredits")?.filter(|v| truthy(v)).cloned().unwrap_or(rc_default);
    let credits = get(&rc, "credits")?.filter(|v| truthy(v)).cloned().unwrap_or(Value::List(Vec::new()));
    let credits = super::claude::iter_list(&credits)?;
    let mut avail = Vec::new();
    for c in &credits {
        if get(c, "status")? == Some(&Value::from("available")) {
            avail.push(c.clone());
        }
    }
    let cid = match &credit_arg {
        Some(c) => c.clone(),
        None => match avail.first() {
            Some(c) => item(c, "id")?.clone(),
            None => Value::Null,
        },
    };
    let limits = item(&j, "rateLimits")?;
    let mut wins = Vec::new();
    for name in ["primary", "secondary"] {
        if let Some(Value::Obj(_)) = get(limits, name)? {
            wins.push(name);
        }
    }
    let outcome = if exists(&join(&[u, "reset_outcome"])) {
        py::strip(&read_text(&join(&[u, "reset_outcome"]))?).to_string()
    } else if num_le_zero(get(&rc, "availableCount")?.unwrap_or(&Value::Int(0)))?
        || (credit_arg.is_some() && !any_id(&avail, &cid)?)
    {
        "noCredit".to_string()
    } else if !wins.iter().any(|w| limits.get(w).and_then(|x| x.get("usedPercent")).is_some_and(truthy)) {
        "nothingToReset".to_string()
    } else {
        let mut rc = rc;
        {
            let lim = j
                .as_obj_mut()
                .and_then(|o| o.get_mut("rateLimits"))
                .and_then(Value::as_obj_mut)
                .ok_or_else(|| Fail("TypeError".into()))?;
            for w in &wins {
                if let Some(Value::Obj(o)) = lim.get_mut(w) {
                    o.insert("usedPercent", 0);
                }
            }
        }
        let count = item(&rc, "availableCount")?.clone();
        let count = match count {
            Value::Int(n) => Value::Int(n - 1),
            Value::Float(f) => Value::Float(f - 1.0),
            Value::Bool(b) => Value::Int(i128::from(b) - 1),
            other => return fail(format!("TypeError: unsupported operand type(s) for -=: '{}' and 'int'", type_name(&other))),
        };
        set_item(&mut rc, "availableCount", count)?;
        let mut kept = Vec::new();
        for c in &credits {
            if !py_eq(get(c, "id")?.unwrap_or(&Value::Null), &cid) {
                kept.push(c.clone());
            }
        }
        set_item(&mut rc, "credits", Value::List(kept))?;
        set_item(&mut j, "rateLimitResetCredits", rc)?;
        let limits = item(&j, "rateLimits")?.clone();
        set_item(setdefault_obj(&mut j, "rateLimitsByLimitId")?, "codex", limits)?;
        write_text(&p, &dumps(&j))?;
        "reset".to_string()
    };
    send(&obj! { "id" => rid, "result" => obj! { "outcome" => outcome } });
    Ok(())
}

/// rc.get('availableCount', 0) <= 0
fn num_le_zero(v: &Value) -> R<bool> {
    match v {
        Value::Int(n) => Ok(*n <= 0),
        Value::Float(f) => Ok(*f <= 0.0),
        Value::Bool(b) => Ok(!*b),
        other => fail(format!("TypeError: '<=' not supported between instances of '{}' and 'int'", type_name(other))),
    }
}

/// any(c['id'] == cid for c in avail)
fn any_id(avail: &[Value], cid: &Value) -> R<bool> {
    for c in avail {
        if py_eq(item(c, "id")?, cid) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Python 的 ==（True == 1、1 == 1.0）
fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(x), Value::Int(n)) | (Value::Int(n), Value::Bool(x)) => i128::from(*x) == *n,
        (Value::Bool(x), Value::Float(f)) | (Value::Float(f), Value::Bool(x)) => f64::from(u8::from(*x)) == *f,
        _ => a == b,
    }
}

/// $CCA_T/oauth_flow 存在时（内容是这次的 state）照真的 codex 0.167 走：本机 127.0.0.1 随机端口开回调（redirect_uri 写 localhost），
/// 回登录网址；回调的 state 不对回 400（State mismatch）、接着等，对的就“换 token”（code 以 good 开头算成功，写 auth.json），
/// 换完才回答浏览器，再发 account/login/completed。换 token 时的代理记进 codex.log
fn start_oauth(t: &str, home: &str, rid: Value) -> R<()> {
    let state = py::strip(&read_text(&join(&[t, "oauth_flow"]))?).to_string();
    let state = if state.is_empty() { "st-codex".to_string() } else { state };
    let (tt, hh, want) = (t.to_string(), home.to_string(), state.clone());
    let port =
        serve(move |req| {
            let Ok(u) = urlsplit(&req.path) else { return };
            let q = parse_qsl(&u.query);
            if u.path != "/auth/callback" {
                req.send_response(404);
                req.end_headers();
                return;
            }
            if qs_get(&q, "state") != Some(want.as_str()) {
                req.send_response(400);
                req.end_headers();
                req.write(b"State mismatch");
                return;
            }
            let code = qs_get(&q, "code").unwrap_or_default();
            let ok = code.starts_with("good");
            let done = log(&tt, &format!("OAUTH-EXCHANGE code={code} https_proxy={}", env_or("https_proxy", "unset")))
                .and_then(|()| if ok { write_auth(&tt, &hh) } else { Ok(()) });
            if let Err(Fail(e)) = done {
                // 像 socketserver 的 handle_error：说一句，这个请求不回答
                eprintln!("Exception occurred during processing of request: {e}");
                return;
            }
            req.send_response(200);
            req.send_header("Content-Type", "text/html");
            req.end_headers();
            req.write(if ok { b"<p>Signed in</p>".as_slice() } else { b"<p>Sign-in failed</p>".as_slice() });
            send(&completed(ok, (!ok).then_some("Token exchange failed: invalid_grant")));
        })?;
    log(t, &format!("OAUTH-START port={port}"))?;
    let redirect = format!("http://localhost:{port}/auth/callback");
    let q = urlencode(&[
        ("response_type", "code"),
        ("client_id", "app_x"),
        ("redirect_uri", &redirect),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", "CC"),
        ("code_challenge_method", "S256"),
        ("state", &state),
        ("originator", "codex_cli_rs"),
    ]);
    let url = format!("https://auth.openai.com/oauth/authorize?{q}");
    send(&obj! { "id" => rid, "result" => obj! { "type" => "chatgpt", "loginId" => "L1", "authUrl" => url } });
    Ok(())
}
