//! linux-codex 沙箱目录 脚本的路径 参数…：tests/linux.sh 的假 codex。原来是 linux.sh 写进 $D/bin/codex 的一段 Python，现在
//! $D/bin/codex 是两行 sh（exec cca-test linux-codex "沙箱目录" "$0" "$@"），要做的都在这里。打印的、记进 $D/codex.log 的、
//! 写出来的 auth.json、退出码、先后都照原来：linux.sh 照样 grep 这些。
//!
//! 第一个参数是沙箱目录（原来写死在脚本开头的 D），第二个是那个 sh 脚本的路径（不看：留在命令行里，读 /proc 找得到是谁起的）。
//! 后面是 codex 的参数。cca 加在前面的全局参数先跳过（--no-daemon；--disable、--enable、-c 各带一个值），再看子命令：
//!
//!   --version、--help（前面没有全局参数时）   codex-cli 0.167.0 / “      --no-daemon”：cca 照它们探测这个版本认哪些参数
//!   features         两行开关（respect_system_proxy、system_proxy_fallback）：cca 照它定启动时关哪些
//!
//! 像 0.172.0 那样认权限配置（cca 的凭证防护，guard.rs）：全局参数里 -c default_permissions 选的不是内置的（: 开头）、又没有
//! -c permissions.<名字>.… 定义它时，不管子命令是什么都在 $D/codex.log 里记一行 PERM-REJECT 名字、stderr 上一行、退出码 1
//!   logout           删掉 $CODEX_HOME/auth.json（没有也不算错）
//!   app-server       JSON-RPC，stdin 一行一个 JSON，回答一行一个、马上写出去；stdin 读到头就退出：
//!       initialize               {"id", "result": {"userAgent": "x"}}
//!       account/login/start      照真的 codex 0.167：state 是 $D/oauth_state 里写的，在 127.0.0.1 的随机端口上开回调（redirect_uri 写
//!                                localhost），回登录网址。回调的路径不是 /auth/callback 回 404；state 不对回 400（State mismatch）、
//!                                接着等；对的记一行 EXCHANGE code=… https_proxy=…，code 以 good 开头算换到了 token：写 auth.json
//!                                （假的 id_token，邮箱取 CCA_ACCOUNT 末尾括号里的，没有就是 someone@example.com），回 200（ok /
//!                                failed），再发 account/login/completed
//!       account/login/cancel     {"id", "result": {}}
//!       account/rateLimits/read  记一行 USAGE email=… https_proxy=…；$D/usage_hang 在时卡住不回（照样读 stdin：和真的一样，stdin
//!                                一断就退出）；没登录回 -32600；登录了回固定的用量（5 小时 13%、2 小时后重置；本周 41%、3 天后）
//!       别的带 id 的              -32601 Method not found: 方法名
//!   别的             当作一次会话：记一行 SESSION https_proxy=… home=… args=…（全部参数，全局的也在里面），$D/hold 在时一直开着，
//!                    最后打印 fake codex session
//!
//! 和原来不一样的只有读坏了的 auth.json 时：id_token 中间那一段，Python 的 base64 解码会跳过字母表以外的字符，这里只认规规矩矩的
//! （这个假 codex 自己写的就是），不规矩的和别的读不出来的情况一样当成没登录。原来没人接的异常（app-server 收到的一行不是对象、
//! 没有 oauth_state……）在这里也是 stderr 上一行、退出码 1。

use super::codex::regex;
use super::*;
use kit::json::Obj;
use std::io::BufRead;

fn log(d: &str, s: &str) -> R<()> {
    append_text(&join(&[d, "codex.log"]), &format!("{s}\n"))
}

/// base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip('=')
pub(super) fn b64(v: &Value) -> String {
    kit::crypto::b64url_nopad(dumps(v).as_bytes())
}

/// 现在登录的邮箱：auth.json 里 id_token 的 email（原样：不一定是字符串）；读不出来是 ''
pub(super) fn email_now(home: &str) -> Value {
    let found = || -> Option<Value> {
        let j = load_json(&join(&[home, "auth.json"])).ok()?;
        let tok = item(item(&j, "tokens").ok()?, "id_token").ok()?.as_str()?;
        let part = tok.split('.').nth(1)?;
        let raw = kit::crypto::b64url_nopad_decode(part.trim_end_matches('=')).ok()?;
        let claims = json::parse(std::str::from_utf8(&raw).ok()?).ok()?;
        item(&claims, "email").ok().cloned()
    };
    found().unwrap_or_else(|| Value::from(""))
}

/// CCA_ACCOUNT（别名(邮箱)）末尾括号里的邮箱；认不出来就是 someone@example.com
pub(super) fn account_email(account: &str) -> R<String> {
    // Python 的 $ 在结尾的换行前面也算数
    Ok(match regex(r"\(([^()]+@[^()]+)\)\n?\z")?.captures(account) {
        Some(c) => c[1].to_string(),
        None => "someone@example.com".to_string(),
    })
}

/// auth.json 的内容（假的 id_token）
pub(super) fn auth_doc(email: &str) -> Value {
    let claims = obj! {
        "email" => email,
        "https://api.openai.com/auth" => obj! { "chatgpt_plan_type" => "plus", "chatgpt_account_id" => "ws-x" },
    };
    let tokens = obj! {
        "id_token" => format!("{}.{}.sig", b64(&obj! { "alg" => "none" }), b64(&claims)),
        "access_token" => "at",
        "refresh_token" => "rt",
        "account_id" => "ws-x",
    };
    obj! { "auth_mode" => "chatgpt", "OPENAI_API_KEY" => Value::Null, "tokens" => tokens }
}

fn write_auth(home: &str) -> R<()> {
    let email = account_email(&env_or("CCA_ACCOUNT", ""))?;
    write_text(&join(&[home, "auth.json"]), &dumps(&auth_doc(&email)))
}

/// 登录网址（回调在本机的这个端口上）
pub(super) fn login_url(port: u16, state: &str) -> String {
    let redirect = format!("http://localhost:{port}/auth/callback");
    let q = urlencode(&[
        ("response_type", "code"),
        ("client_id", "app_x"),
        ("redirect_uri", &redirect),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", "CC"),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("originator", "codex_cli_rs"),
    ]);
    format!("https://auth.openai.com/oauth/authorize?{q}")
}

/// account/rateLimits/read 的 result：固定的用量，重置的时刻照 now（1970 年以来的秒）算
pub(super) fn rate_limits(now: i64) -> Value {
    let rl = obj! {
        "limitId" => "codex",
        "primary" => obj! { "usedPercent" => 13, "windowDurationMins" => 300, "resetsAt" => now + 7200 },
        "secondary" => obj! { "usedPercent" => 41, "windowDurationMins" => 10080, "resetsAt" => now + 3 * 86400 },
        "planType" => "plus",
    };
    obj! { "rateLimits" => rl.clone(), "rateLimitsByLimitId" => obj! { "codex" => rl } }
}

/// cca 加在前面的全局参数有几个词（--no-daemon 一个；--disable、--enable、-c 各带一个值）：子命令从这里开始
pub(super) fn globals(args: &[String]) -> usize {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-daemon" => i += 1,
            "--disable" | "--enable" | "-c" => i += 2,
            _ => break,
        }
    }
    i
}

/// 发一条（一行一个 JSON，马上发出去；回调的线程和主线程一起发时一条一条地来）
fn send(m: &Value) {
    print_flush(&dumps(m));
}

fn error(rid: &Value, code: i32, msg: &str) -> Value {
    obj! { "id" => rid.clone(), "error" => obj! { "code" => code, "message" => msg } }
}

pub fn run(a: &[String]) -> R<i32> {
    let (Some(d), Some(_script)) = (a.first(), a.get(1)) else {
        flush();
        eprintln!("用法：cca-test linux-codex 沙箱目录 脚本的路径 codex 的参数…");
        return Ok(2);
    };
    let args = &a[2..];
    let home = env_or("CODEX_HOME", "");
    let i = globals(args);
    let (mut perm, mut defined): (Option<String>, Vec<String>) = (None, Vec::new());
    let mut j = 0;
    while j < i {
        if args[j] == "-c" {
            let v = args.get(j + 1).map(String::as_str).unwrap_or("");
            if let Some(p) = v.strip_prefix("default_permissions=") {
                perm = Some(p.replace('"', ""));
            } else if let Some(p) = v.strip_prefix("permissions.") {
                defined.push(p.split('.').next().unwrap_or("").to_string());
            }
        }
        j += if args[j] == "--no-daemon" { 1 } else { 2 };
    }
    if let Some(p) = perm.filter(|p| !p.starts_with(':') && !defined.contains(p)) {
        log(d, &format!("PERM-REJECT {p}"))?;
        eprintln!("Error: failed to load configuration");
        return Ok(1);
    }
    let sub = args.get(i).map(String::as_str).unwrap_or("");
    if i == 0 && sub == "--version" {
        print("codex-cli 0.167.0");
        return Ok(0);
    }
    if i == 0 && sub == "--help" {
        print("      --no-daemon");
        return Ok(0);
    }
    if sub == "features" {
        print("respect_system_proxy                     under development  false");
        print("system_proxy_fallback                    stable             true");
        return Ok(0);
    }
    if sub == "logout" {
        let _ = std::fs::remove_file(join(&[&home, "auth.json"]));
        return Ok(0);
    }
    if sub != "app-server" {
        log(d, &format!("SESSION https_proxy={} home={home} args={}", env_or("https_proxy", "unset"), args.join(" ")))?;
        while exists(&join(&[d, "hold"])) {
            sleep(0.1);
        }
        print("fake codex session");
        return Ok(0);
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if input.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let Ok(m) = json::parse(&String::from_utf8_lossy(&buf)) else { continue };
        let meth = get(&m, "method")?.cloned().unwrap_or(Value::Null);
        let rid = get(&m, "id")?.cloned().unwrap_or(Value::Null);
        match meth.as_str() {
            Some("initialize") => send(&obj! { "id" => rid, "result" => obj! { "userAgent" => "x" } }),
            Some("account/login/start") => start_login(d, &home, rid)?,
            Some("account/login/cancel") => send(&obj! { "id" => rid, "result" => Value::Obj(Obj::new()) }),
            Some("account/rateLimits/read") => {
                let email = email_now(&home);
                let shown = if truthy(&email) { py_str(&email) } else { "-".to_string() };
                log(d, &format!("USAGE email={shown} https_proxy={}", env_or("https_proxy", "unset")))?;
                if exists(&join(&[d, "usage_hang"])) {
                    continue; // 卡住不回（照样读 stdin：和真的一样，stdin 一断就退出）
                }
                if !truthy(&email_now(&home)) {
                    send(&error(&rid, -32600, "codex account authentication required"));
                    continue;
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_secs() as i64);
                send(&obj! { "id" => rid, "result" => rate_limits(now) });
            }
            _ if !rid.is_null() => send(&error(&rid, -32601, &format!("Method not found: {}", py_str(&meth)))),
            _ => {}
        }
    }
    Ok(0)
}

/// account/login/start：开本机的回调、回登录网址；换 token、写 auth.json、发 account/login/completed 都在回调里
fn start_login(d: &str, home: &str, rid: Value) -> R<()> {
    let state = py::strip(&read_text(&join(&[d, "oauth_state"]))?).to_string();
    let (dd, hh, want) = (d.to_string(), home.to_string(), state.clone());
    let port = serve(move |req| {
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
        let good = code.starts_with("good");
        let done = log(&dd, &format!("EXCHANGE code={code} https_proxy={}", env_or("https_proxy", "unset"))).and_then(|()| {
            if good {
                write_auth(&hh)
            } else {
                Ok(())
            }
        });
        if let Err(Fail(e)) = done {
            // 像 socketserver 的 handle_error：说一句，这个请求不回答
            eprintln!("Exception occurred during processing of request: {e}");
            return;
        }
        req.send_response(200);
        req.end_headers();
        req.write(if good { b"ok".as_slice() } else { b"failed".as_slice() });
        let err = if good { Value::Null } else { Value::from("Token exchange failed") };
        send(&obj! {
            "method" => "account/login/completed",
            "params" => obj! { "loginId" => "L1", "success" => good, "error" => err },
        });
    })?;
    send(&obj! { "id" => rid, "result" => obj! { "type" => "chatgpt", "loginId" => "L1", "authUrl" => login_url(port, &state) } });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::fsx::TempDir;

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // 下面的期望值都是原来那段 Python 给的（python3 里照 linux.sh 的原文算出来的）

    #[test]
    fn global_args_are_skipped() {
        assert_eq!(globals(&strs(&["--version"])), 0);
        assert_eq!(globals(&strs(&["--no-daemon", "--disable", "x", "-c", "a=1", "--enable", "y", "exec", "hi"])), 7);
        // 最后一个带值的全局参数后面没有值：下标越过了结尾（子命令是空的）
        assert_eq!(globals(&strs(&["-c"])), 2);
        assert_eq!(globals(&[]), 0);
    }

    #[test]
    fn auth_json_is_the_python_one() {
        // json.dumps({'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None, 'tokens': {…}})，邮箱是 carol@example.com
        assert_eq!(
            dumps(&auth_doc("carol@example.com")),
            "{\"auth_mode\": \"chatgpt\", \"OPENAI_API_KEY\": null, \"tokens\": {\"id_token\": \"eyJhbGciOiAibm9uZSJ9.eyJlbWFpbCI6ICJjYXJvbEBleGFtcGxlLmNvbSIsICJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOiB7ImNoYXRncHRfcGxhbl90eXBlIjogInBsdXMiLCAiY2hhdGdwdF9hY2NvdW50X2lkIjogIndzLXgifX0.sig\", \"access_token\": \"at\", \"refresh_token\": \"rt\", \"account_id\": \"ws-x\"}}"
        );
    }

    #[test]
    fn the_account_email_comes_from_cca_account() {
        assert_eq!(account_email("carol(carol@example.com)").unwrap(), "carol@example.com");
        assert_eq!(account_email("卡萝(carol@example.com)\n").unwrap(), "carol@example.com");
        assert_eq!(account_email("carol@example.com").unwrap(), "someone@example.com");
        assert_eq!(account_email("a(b)(c@d)").unwrap(), "c@d");
        assert_eq!(account_email("a(c@d) ").unwrap(), "someone@example.com");
        assert_eq!(account_email("").unwrap(), "someone@example.com");
    }

    #[test]
    fn the_logged_in_email_is_read_back() {
        let dir = TempDir::new("cca-linux-codex-");
        let home = dir.path().to_str().unwrap().to_string();
        assert_eq!(email_now(&home), Value::from(""));
        write_text(&join(&[&home, "auth.json"]), &dumps(&auth_doc("carol@example.com"))).unwrap();
        assert_eq!(email_now(&home), Value::from("carol@example.com"));
        for broken in
            ["{", "[]", "{\"tokens\": {}}", "{\"tokens\": {\"id_token\": \"a\"}}", "{\"tokens\": {\"id_token\": \"a.!!.c\"}}"]
        {
            write_text(&join(&[&home, "auth.json"]), broken).unwrap();
            assert_eq!(email_now(&home), Value::from(""), "{broken}");
        }
    }

    #[test]
    fn login_url_is_the_python_one() {
        assert_eq!(
            login_url(49152, "st-linux"),
            "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_x&redirect_uri=http%3A%2F%2Flocalhost%3A49152%2Fauth%2Fcallback&scope=openid+profile+email+offline_access&code_challenge=CC&code_challenge_method=S256&state=st-linux&originator=codex_cli_rs"
        );
    }

    #[test]
    fn usage_is_the_fixed_one() {
        assert_eq!(
            dumps(&rate_limits(1_795_500_000)),
            "{\"rateLimits\": {\"limitId\": \"codex\", \"primary\": {\"usedPercent\": 13, \"windowDurationMins\": 300, \"resetsAt\": 1795507200}, \"secondary\": {\"usedPercent\": 41, \"windowDurationMins\": 10080, \"resetsAt\": 1795759200}, \"planType\": \"plus\"}, \"rateLimitsByLimitId\": {\"codex\": {\"limitId\": \"codex\", \"primary\": {\"usedPercent\": 13, \"windowDurationMins\": 300, \"resetsAt\": 1795507200}, \"secondary\": {\"usedPercent\": 41, \"windowDurationMins\": 10080, \"resetsAt\": 1795759200}, \"planType\": \"plus\"}}}"
        );
    }
}
