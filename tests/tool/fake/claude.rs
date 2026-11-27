//! 假 claude 里原来内嵌的几段 Python：--settings 带进来的设置（记下 env，像 Claude Code 一样跑 apiKeyHelper 和 SessionStart 钩子）、
//! auth login 前记下设备标识、login / logout / status 读写 .claude.json、一次会话往配置目录里写的东西。

use super::*;
use kit::json::{Obj, Style};

/// 第 i 个参数（原来的 sys.argv[i + 1]）
pub fn arg(a: &[String], i: usize) -> R<&str> {
    a.get(i).map(String::as_str).ok_or_else(|| Fail("IndexError: list index out of range".into()))
}

/// claude-settings 设置的JSON CCA_T 会话(1/0)：stdout 接在 claude.log 后面（攒到最后一起写）
pub fn settings(a: &[String]) -> R<i32> {
    let (s, t, session) = (arg(a, 0)?, arg(a, 1)?, arg(a, 2)? == "1");
    let s = json::parse(s).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))?;
    let env = get(&s, "env")?.cloned().unwrap_or(Value::Null);
    print(&format!("SETTINGS: env={}", json::dumps(&env, &Style::default().sorted())));
    if let Some(helper) = get(&s, "apiKeyHelper")?.filter(|v| truthy(v)) {
        let helper = str_arg(helper)?;
        let r = run_capture(&["/bin/sh".into(), "-c".into(), helper.into()], None, None)?;
        write_text(&format!("{t}/claude.key"), &r.stdout)?;
        print(&format!("HELPER: rc={}", r.code));
    }
    if !session {
        return Ok(0);
    }
    // (s.get('hooks') or {}).get('SessionStart') or []
    let none = Value::Obj(Obj::new());
    let hooks = get(&s, "hooks")?.filter(|v| truthy(v)).unwrap_or(&none);
    let starts = get(hooks, "SessionStart")?.filter(|v| truthy(v)).cloned().unwrap_or(Value::List(Vec::new()));
    for m in iter_list(&starts)? {
        let hs = get(&m, "hooks")?.filter(|v| truthy(v)).cloned().unwrap_or(Value::List(Vec::new()));
        for h in iter_list(&hs)? {
            let cmd = str_arg(item(&h, "command")?)?.to_string();
            // h.get('timeout', 60)：没写是 60 秒，null 是不限
            let given = get(&h, "timeout")?.cloned();
            let secs = match &given {
                None => Some(60.0),
                Some(Value::Null) => None,
                Some(Value::Int(n)) => Some(*n as f64),
                Some(Value::Float(f)) => Some(*f),
                Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
                Some(other) => {
                    return fail(format!("TypeError: '<' not supported between instances of '{}' and 'int'", type_name(other)))
                }
            };
            let shown = given.as_ref().map_or_else(|| "60".to_string(), py_str);
            let timeout = secs.map(|t| (t, shown.as_str()));
            let sid = uuid4();
            let ev = obj! {
                "session_id" => sid.as_str(),
                "transcript_path" => format!("/w/{sid}.jsonl"),
                "cwd" => getcwd()?,
                "hook_event_name" => "SessionStart",
                "source" => "startup",
            };
            let r = run_capture(&["/bin/sh".into(), "-c".into(), cmd], Some(dumps(&ev).as_bytes()), timeout)?;
            print(&format!(
                "HOOK: SessionStart rc={} stdout={} stderr={} sid={sid}",
                r.code,
                r.stdout.chars().count(),
                r.stderr.chars().count()
            ));
        }
    }
    Ok(0)
}

/// 给子进程的参数：得是字符串（subprocess 不收别的）
pub fn str_arg(v: &Value) -> R<&str> {
    v.as_str().ok_or_else(|| Fail(format!("TypeError: expected str, bytes or os.PathLike object, not {}", type_name(v))))
}

/// for x in v：数组逐项；对象是逐个键、字符串是逐个字符（原来的代码接着会对它们 .get，这里直接算出错）
pub fn iter_list(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::List(l) => Ok(l.clone()),
        Value::Obj(o) => Ok(o.keys().map(Value::from).collect()),
        Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        other => fail(format!("TypeError: '{}' object is not iterable", type_name(other))),
    }
}

/// claude-login-device .claude.json：auth login 一开始记下设备标识（LOGIN-DEVICE userID:machineID @ 配置目录）
pub fn login_device(a: &[String]) -> R<i32> {
    let p = arg(a, 0)?;
    let j = load_json_or_empty(p)?;
    let uid = get(&j, "userID")?.map(py_str).unwrap_or_default();
    let mid = get(&j, "machineID")?.map(py_str).unwrap_or_default();
    print(&format!("LOGIN-DEVICE {uid}:{mid} @ {}", dirname(p)));
    Ok(0)
}

/// claude-login .claude.json 邮箱：登录好了（oauthAccount 换成这个邮箱，indent=2 写回）
pub fn login(a: &[String]) -> R<i32> {
    let (p, email) = (arg(a, 0)?, arg(a, 1)?);
    let mut j = load_json_or_empty(p)?;
    set_item(&mut j, "oauthAccount", obj! { "emailAddress" => email })?;
    write_text(p, &dumps_indent2(&j))?;
    Ok(0)
}

/// claude-logout .claude.json：去掉 oauthAccount
pub fn logout(a: &[String]) -> R<i32> {
    let p = arg(a, 0)?;
    let mut j = load_json(p)?;
    match &mut j {
        Value::Obj(o) => {
            o.remove("oauthAccount");
        }
        other => return fail(format!("AttributeError: '{}' object has no attribute 'pop'", type_name(other))),
    }
    write_text(p, &dumps_indent2(&j))?;
    Ok(0)
}

/// claude-status .claude.json：auth status 的 JSON（{"loggedIn": …, "email": …}）
pub fn status(a: &[String]) -> R<i32> {
    let p = arg(a, 0)?;
    let j = load_json_or_empty(p)?;
    let none = Value::Obj(Obj::new());
    let acct = get(&j, "oauthAccount")?.filter(|v| truthy(v)).unwrap_or(&none);
    let e = get(acct, "emailAddress")?.cloned().unwrap_or(Value::Null);
    print(&dumps(&obj! { "loggedIn" => truthy(&e), "email" => e }));
    Ok(0)
}

/// claude-session 配置目录 账号：一次会话往配置目录里写的东西（信任的项目、MCP、设置、输出风格、历史、记忆）
pub fn session(a: &[String]) -> R<i32> {
    let (cd, acct) = (arg(a, 0)?, arg(a, 1)?);
    let tag = acct.split('@').next().unwrap_or_default();
    let p = join(&[cd, ".claude.json"]);
    let mut j = load_json_or_empty(&p)?;
    set_item(setdefault_obj(&mut j, "projects")?, &format!("/p-from-{tag}"), obj! { "hasTrustDialogAccepted" => true })?;
    set_item(setdefault_obj(&mut j, "mcpServers")?, &format!("mcp-{tag}"), obj! { "command" => "echo" })?;
    write_text(&p, &dumps_indent2(&j))?;
    let s = join(&[cd, "settings.json"]);
    let mut cur = load_json_or_empty(&s)?;
    set_item(&mut cur, "lastWriter", Value::from(tag))?;
    write_text(&format!("{s}.tmp"), &dumps(&cur))?;
    replace(&format!("{s}.tmp"), &s)?;
    makedirs(&join(&[cd, "output-styles"]))?;
    write_text(&join(&[cd, "output-styles", &format!("{tag}.md")]), &format!("style {tag}"))?;
    append_text(&join(&[cd, "history.jsonl"]), &format!("{}\n", dumps(&obj! { "display" => format!("hi from {tag}") })))?;
    write_text(&join(&[cd, "projects", "-p1", "memory", &format!("{tag}.md")]), &format!("memory from {tag}"))?;
    Ok(0)
}
