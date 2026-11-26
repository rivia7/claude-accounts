//! linux-claude 沙箱目录 脚本的路径 参数…：tests/linux.sh 的假 claude。原来是 linux.sh 写进 $D/bin/claude 的一段 Python，现在
//! $D/bin/claude 是两行 sh（exec cca-test linux-claude "沙箱目录" "$0" "$@"），要做的都在这里。打印的、记进 $D/claude.log 的、
//! 写出来的文件、退出码、先后都照原来：linux.sh 照样 grep 这些。
//!
//! 第一个参数是沙箱目录（原来写死在脚本开头的 D）。第二个是那个 sh 脚本的路径（相当于 Python 的 sys.argv[0]），这里不看：
//! 带着它，进程的命令行里才有 “$D/bin/claude auth login” 这一截——linux.sh 取消登录以后读 /proc 查的就是它还在不在。
//!
//! 后面是 claude 的参数。登录的状态在 $CLAUDE_CONFIG_DIR/.claude.json（没设这个变量时是 /nonexistent/.claude.json：读不了当空的）：
//!
//!   --version        打印 9.9.9 (Claude Code)
//!   auth status      打印 {"loggedIn": 有没有邮箱, "email": oauthAccount.emailAddress}
//!   auth logout      去掉 oauthAccount（json.dump 的默认写法）、记一行 LOGOUT。凭证文件不动：留给 cca 删
//!   auth login [--email 邮箱]（没写是 someone@example.com）
//!                    照真的 Claude Code 走：state 是 $D/oauth_state 里写的；在 127.0.0.1 的随机端口上开回调（redirect_uri 写
//!                    localhost），打印 “Opening browser to sign in…” 和不带换行的 “Paste code here if prompted > ”（马上写出去），
//!                    运行 $BROWSER 登录网址（等它退出）。回调的路径不是 /callback、state 不对：回 400，登录失败
//!                    （Login failed: Invalid state parameter，退出码 1）；对的回 302 到成功页。等 60 秒没等到回调：
//!                    Login failed: timed out。拿到了就“换 token”：记一行 EXCHANGE code=… https_proxy=…（环境里没有是 unset），
//!                    code 不是 good 开头的失败（Login failed: Request failed with status code 400），是的写 oauthAccount、
//!                    把 {"claudeAiOauth": {}} 写进旁边 600 的 .credentials.json，打印 Login successful.
//!   打印模式         -p、--print（-cp 这样几个短选项写在一起的也算；-- 后面的不算）：cca 不用，也不让经 cca 用。记一行
//!                    PRINT-MODE 参数… 进 $D/print-mode.log（linux.sh 最后查它是空的），stderr 上说一句，退出码 1
//!   别的             当作一次会话：--settings 后面的 JSON 里有 apiKeyHelper 时像 Claude Code 一样执行它（/bin/sh -c，标准输入是
//!                    /dev/null），取到的 key 写进 $D/claude.key；记一行 SESSION https_proxy=… args=…（--settings 和它的 JSON
//!                    不记）；$D/hold 在时一直开着（0.1 秒看一次）；最后打印 fake claude session
//!
//! 原来的 Python 里没人接的异常（没有 oauth_state、没设 BROWSER、--email 后面没东西、.claude.json 不是对象……）在这里也是
//! stderr 上一行、退出码 1。

use super::claude::{arg, str_arg};
use super::*;
use kit::json::Obj;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Condvar};

/// load()：读不了、不是 JSON 都当成空的
fn load(p: &str) -> Value {
    load_json(p).unwrap_or_else(|_| Value::Obj(Obj::new()))
}

/// 回调送来的（原来的 got 字典：err、code 各记各的）
#[derive(Default)]
struct Got {
    err: Option<String>,
    code: Option<String>,
    done: bool,
}

/// 登录网址（回调在本机的这个端口上）
pub(super) fn login_url(port: u16, state: &str, email: &str) -> String {
    let redirect = format!("http://localhost:{port}/callback");
    let q = [
        ("code", "true"),
        ("client_id", "c"),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("code_challenge", "CC"),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("login_hint", email),
    ];
    format!("https://claude.com/cai/oauth/authorize?{}", urlencode(&q))
}

/// SESSION 那一行里的 args：--settings 开头的、{ 开头的（设置的 JSON）不记
pub(super) fn session_args(args: &[String]) -> String {
    args.iter().filter(|a| !a.starts_with("--settings") && !a.starts_with('{')).map(String::as_str).collect::<Vec<_>>().join(" ")
}

pub fn run(a: &[String]) -> R<i32> {
    let (Some(d), Some(_script)) = (a.first(), a.get(1)) else {
        flush();
        eprintln!("用法：cca-test linux-claude 沙箱目录 脚本的路径 claude 的参数…");
        return Ok(2);
    };
    let args = &a[2..];
    let p = join(&[&env_or("CLAUDE_CONFIG_DIR", "/nonexistent"), ".claude.json"]);
    let log = |s: &str| append_text(&join(&[d, "claude.log"]), &format!("{s}\n"));
    let word = |i: usize| args.get(i).map(String::as_str);
    if word(0) == Some("--version") {
        print("9.9.9 (Claude Code)");
        return Ok(0);
    }
    if (word(0), word(1)) == (Some("auth"), Some("status")) {
        let j = load(&p);
        let none = Value::Obj(Obj::new());
        let acct = get(&j, "oauthAccount")?.filter(|v| truthy(v)).unwrap_or(&none);
        let e = get(acct, "emailAddress")?.cloned().unwrap_or(Value::Null);
        print(&dumps(&obj! { "loggedIn" => truthy(&e), "email" => e }));
        return Ok(0);
    }
    if (word(0), word(1)) == (Some("auth"), Some("logout")) {
        let mut j = load(&p);
        match &mut j {
            Value::Obj(o) => {
                o.remove("oauthAccount");
            }
            // j.pop('oauthAccount', None)：列表也有 pop，只是不收两个参数
            Value::List(_) => return fail("TypeError: pop expected at most 1 argument, got 2"),
            other => return fail(format!("AttributeError: '{}' object has no attribute 'pop'", type_name(other))),
        }
        write_text(&p, &dumps(&j))?;
        log("LOGOUT")?;
        return Ok(0);
    }
    if (word(0), word(1)) == (Some("auth"), Some("login")) {
        return login(d, &p, args);
    }
    let print_mode = |a: &String| {
        a == "-p"
            || a == "--print"
            || a.starts_with("--print=")
            || (a.len() > 2
                && !a.starts_with("--")
                && a.starts_with('-')
                && a[1..].chars().all(|c| c.is_ascii_alphabetic())
                && a.contains('p'))
    };
    if args.iter().take_while(|a| *a != "--").any(print_mode) {
        append_text(&join(&[d, "print-mode.log"]), &format!("PRINT-MODE {}\n", args.join(" ")))?;
        flush();
        eprintln!("fake claude: cca 不该用打印模式");
        return Ok(1);
    }
    if let Some(i) = args.iter().position(|x| x == "--settings") {
        let s = json::parse(arg(args, i + 1)?).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))?;
        if let Some(helper) = get(&s, "apiKeyHelper")?.filter(|v| truthy(v)) {
            let r = run_capture(&["/bin/sh".into(), "-c".into(), str_arg(helper)?.into()], None, None)?;
            write_text(&join(&[d, "claude.key"]), &r.stdout)?;
        }
    }
    log(&format!("SESSION https_proxy={} args={}", env_or("https_proxy", "unset"), session_args(args)))?;
    while exists(&join(&[d, "hold"])) {
        sleep(0.1);
    }
    print("fake claude session");
    Ok(0)
}

fn login(d: &str, p: &str, args: &[String]) -> R<i32> {
    let email = match args.iter().position(|x| x == "--email") {
        Some(i) => arg(args, i + 1)?.to_string(),
        None => "someone@example.com".to_string(),
    };
    let state = py::strip(&read_text(&join(&[d, "oauth_state"]))?).to_string();
    let got = Arc::new((Mutex::new(Got::default()), Condvar::new()));
    let (g, want) = (Arc::clone(&got), state.clone());
    let port = serve(move |req| {
        let Ok(u) = urlsplit(&req.path) else { return };
        let q = parse_qsl(&u.query);
        let (lock, cv) = &*g;
        if u.path != "/callback" || qs_get(&q, "state") != Some(want.as_str()) {
            req.send_response(400);
            req.end_headers();
            let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
            st.err = Some("Invalid state parameter".into());
            st.done = true;
            cv.notify_all();
            return;
        }
        req.send_response(302);
        req.send_header("Location", "https://platform.claude.com/oauth/code/success");
        req.end_headers();
        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
        st.code = Some(qs_get(&q, "code").unwrap_or_default().to_string());
        st.done = true;
        cv.notify_all();
    })?;
    print("Opening browser to sign in\u{2026}");
    write_out("Paste code here if prompted > ");
    flush();
    let browser = env("BROWSER").ok_or_else(|| Fail("KeyError: 'BROWSER'".into()))?;
    run_wait(&[browser, login_url(port, &state, &email)])?;
    // done.wait(60)
    let (lock, cv) = &*got;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !st.done {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
        st = cv.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    if !st.done || st.err.is_some() {
        print(&format!("Login failed: {}", st.err.as_deref().unwrap_or("timed out")));
        return Ok(1);
    }
    let code = st.code.clone().unwrap_or_default();
    drop(st);
    append_text(&join(&[d, "claude.log"]), &format!("EXCHANGE code={code} https_proxy={}\n", env_or("https_proxy", "unset")))?;
    if !code.starts_with("good") {
        print("Login failed: Request failed with status code 400");
        return Ok(1);
    }
    let mut j = load(p);
    set_item(&mut j, "oauthAccount", obj! { "emailAddress" => email.as_str() })?;
    write_text(p, &dumps(&j))?;
    // os.open(c, O_WRONLY | O_CREAT | O_TRUNC, 0o600)：新建时才是 600，本来就有的权限不动
    let c = join(&[&dirname(p), ".credentials.json"]);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&c)
        .map_err(|e| Fail(os_text(&e, Some(&c))))?;
    f.write_all(b"{\"claudeAiOauth\": {}}")?;
    print("Login successful.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // 期望的值是原来那段 Python 给的：
    // up.urlencode({'code': 'true', 'client_id': 'c', 'response_type': 'code', 'redirect_uri': 'http://localhost:%d/callback' % 49152,
    //               'code_challenge': 'CC', 'code_challenge_method': 'S256', 'state': 'st-linux', 'login_hint': 'a+b@example.com'})
    #[test]
    fn login_url_is_the_python_one() {
        assert_eq!(
            login_url(49152, "st-linux", "a+b@example.com"),
            "https://claude.com/cai/oauth/authorize?code=true&client_id=c&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A49152%2Fcallback&code_challenge=CC&code_challenge_method=S256&state=st-linux&login_hint=a%2Bb%40example.com"
        );
    }

    // ' '.join(a for a in args if not a.startswith('--settings') and not a.startswith('{'))
    #[test]
    fn session_line_leaves_the_settings_out() {
        assert_eq!(session_args(&strs(&["--add-dir", "/x", "--settings", "{\"a\": 1}", "hi"])), "--add-dir /x hi");
        assert_eq!(session_args(&strs(&["--settings={}", "{x", "a {b}"])), "a {b}");
        assert_eq!(session_args(&[]), "");
    }

    #[test]
    fn unreadable_state_is_empty() {
        assert_eq!(load("/nonexistent/.claude.json"), Value::Obj(Obj::new()));
    }
}
