//! fake-oauth .claude.json 邮箱：假 claude 的 auth login 在 $CCA_T/oauth_flow 存在时（内容是这次的 state）照真的 Claude Code 2.1.322 走——
//! 本机 127.0.0.1 随机端口开回调（redirect_uri 写 localhost），打印手动登录的网址和“Paste code here”，经 $BROWSER 打开自动的那个登录网址；
//! 回调的 state 不对回 400、整个登录失败，对的回 302 到成功页，再“换 token”：code 以 good 开头算成功（写 oauthAccount），否则失败。
//! 换 token 时的代理记进 claude.log。原来的 fake_oauth.py（假 claude 用 exec -a "$CCA_T/fake_oauth" 起它，回归按这个名字找进程）。

use super::claude::arg;
use super::*;
use std::sync::{Arc, Condvar};

/// 回调送来的（像原来的 got 字典：err、code 各记各的）
#[derive(Default)]
struct Got {
    err: Option<String>,
    code: Option<String>,
    done: bool,
}

pub fn run(a: &[String]) -> R<i32> {
    let (p, email) = (arg(a, 0)?.to_string(), arg(a, 1)?.to_string());
    let t = cca_t()?;
    let state = py_strip(&read_text(&join(&[&t, "oauth_flow"]))?).to_string();
    let state = if state.is_empty() { "st-claude".to_string() } else { state };
    let got = Arc::new((Mutex::new(Got::default()), Condvar::new()));
    let (g, want) = (Arc::clone(&got), state.clone());
    let port = serve(move |req| {
        let Ok(u) = urlsplit(&req.path) else { return };
        let q = parse_qsl(&u.query);
        if u.path != "/callback" {
            req.send_response(404);
            req.end_headers();
            return;
        }
        let (lock, cv) = &*g;
        if qs_get(&q, "state") != Some(want.as_str()) {
            req.send_response(400);
            req.end_headers();
            req.write(b"Invalid state parameter");
            let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
            st.err = Some("Invalid state parameter".into());
            st.done = true;
            cv.notify_all();
            return;
        }
        req.send_response(302);
        req.send_header("Location", "https://platform.claude.com/oauth/code/success?app=claude-code");
        req.end_headers();
        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
        st.code = Some(qs_get(&q, "code").unwrap_or_default().to_string());
        st.done = true;
        cv.notify_all();
    })?;
    let url = |redirect: &str| {
        let q = [
            ("code", "true"),
            ("client_id", "c"),
            ("response_type", "code"),
            ("redirect_uri", redirect),
            ("scope", "user:inference"),
            ("code_challenge", "CC"),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("login_hint", email.as_str()),
        ];
        format!("https://claude.com/cai/oauth/authorize?{}", urlencode(&q))
    };
    print("Opening browser to sign in\u{2026}");
    print(&format!("If the browser didn't open, visit: {}", url("https://platform.claude.com/oauth/code/callback")));
    write_out("Paste code here if prompted > ");
    flush();
    let log = |s: &str| append_text(&join(&[&t, "claude.log"]), &format!("{s}\n"));
    log(&format!("OAUTH-START port={port}"))?;
    if let Some(browser) = env("BROWSER").filter(|b| !b.is_empty()) {
        run_wait(&[browser, url(&format!("http://localhost:{port}/callback"))])?;
    }
    // done.wait(60)
    let (lock, cv) = &*got;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !st.done {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
        st = cv.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    if !st.done {
        print("Login failed: timed out");
        return Ok(1);
    }
    if let Some(err) = &st.err {
        print(&format!("Login failed: {err}"));
        return Ok(1);
    }
    let code = st.code.clone().unwrap_or_default();
    drop(st);
    log(&format!("OAUTH-EXCHANGE code={code} https_proxy={}", env_or("https_proxy", "unset")))?;
    if !code.starts_with("good") {
        print("Login failed: Request failed with status code 400");
        return Ok(1);
    }
    let mut j = load_json_or_empty(&p)?;
    set_item(&mut j, "oauthAccount", obj! { "emailAddress" => email.as_str() })?;
    write_text(&p, &dumps_indent2(&j))?;
    log(&format!("LOGIN {email} @ {}", dirname(&p)))?;
    print("Login successful.");
    Ok(0)
}
