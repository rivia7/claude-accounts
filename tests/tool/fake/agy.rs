//! fake-agy 参数…：假的 agy（照 1.5.16 的行为），原来的 fake_agy.py（假 agy 用 exec -a "$CCA_T/fake_agy" 起它，回归按这个名字找进程）。
//! --gemini_dir=目录 换数据目录（写在子命令前面）。acp --login 在 antigravity-cli/log/acp-login-*.log 里说登录存成了文件：设了
//! AGY_CLI_FORCE_FILE_TOKEN_STORAGE（1 或 true）时说“…because AGY_CLI_FORCE_FILE_TOKEN_STORAGE is set”（$CCA_T/agy_noswitch：
//! 模拟不认这个开关的旧版本），不然设了 SSH_CONNECTION 时说“…because SSH session detected”（1.5.5 的做法；$CCA_T/agy_keychain：
//! 模拟 1.5.16 在钥匙串没锁时不再这样）；打印 Google 的登录网址（state 取 $CCA_T/agy_state）和问授权码的那一句（没有换行），从 stdin
//! 读一行：读到头说 login cancelled、退出码 1；授权码（或整个网址里的 code）以 good 开头算成功（不是就报错再问），token 写进
//! antigravity-cli/antigravity-oauth-token（id_token_email 取 CCA_ACCOUNT 里的邮箱，一次性的 $CCA_T/agy_login_as 模拟登错号，
//! $CCA_T/agy_noemail 时不写邮箱，档位取 $CCA_T/agy_tier），说 Logged in as 邮箱.；换 token 时的代理记进 agy.log。
//! -p /usage --output-format json 照 1.5.16 回额度（一行 JSON，额度在 command.data；按 token 里的邮箱取 $CCA_T/agy_usage/<邮箱>.json，
//! 没有就用一份默认的，重置时间按现在算），agy.log 里记 AGY-USAGE（有没有控制终端、自动更新关没关、工作目录）；没有 token 或者
//! $CCA_T/agy_usage_auth 时像没登录一样在 stderr 上打出登录网址、等 60 秒；$CCA_T/agy_usage_error（内容是原因）回 ERROR；
//! $CCA_T/agy_usage_old 像不认 /usage 的旧版本，当成一句话回答（没有 command）；$CCA_T/agy_usage_hang 一直不回。
//! 其余（包括 cca 带着 --gemini_dir 传进来的 --version）当作一次会话：装机标识不是正好 36 个字符就重新生成（像真的），写共享的会话、历史、技能，
//! 设置用临时文件 + 改名保存；交互的会话（不是 --version、打印模式、子命令）像 agy 画状态栏（`statusline`：--settings 或者 settings.json
//! 里的 statusLine.command 用 sh -c 跑，stdin 是状态栏的 JSON，plan_tier 取 $CCA_T/agy_plan_tier；状态取 $CCA_T/agy_states，一行画一次），退出码 3

use super::codex::{regex, tag_of, PY_WS};
use super::*;
use kit::json::{Obj, Style};
use kit::list;
use std::io::BufRead;
use std::os::unix::fs::OpenOptionsExt;

/// 标记文件：不在是 None；在的话是去掉首尾空白的内容，空的算 Some("")（原来的 True）
fn mark(t: &str, name: &str) -> R<Option<String>> {
    let p = join(&[t, name]);
    if !exists(&p) {
        return Ok(None);
    }
    Ok(Some(py::strip(&read_text(&p)?).to_string()))
}

pub fn run(a: &[String]) -> R<i32> {
    let t = cca_t()?;
    let log = |s: &str| append_text(&join(&[&t, "agy.log"]), &format!("{s}\n"));
    let mut args: Vec<String> = a.to_vec();
    let (mut g, mut gd) = (join(&[&home_dir(), ".gemini"]), false);
    while args.first().is_some_and(|x| x.starts_with("--gemini_dir") || x.starts_with("--gemini-dir")) {
        let x = args.remove(0);
        gd = true;
        if let Some((_, v)) = x.split_once('=') {
            g = v.to_string();
        } else if !args.is_empty() {
            g = args.remove(0);
        }
    }
    let cli = join(&[&g, "antigravity-cli"]);
    makedirs(&join(&[&cli, "log"]))?;
    if args.first().map(String::as_str) == Some("--version") && !gd {
        print("1.5.5");
        return Ok(0);
    }
    if args.len() >= 2 && args[0] == "acp" && args[1] == "--login" {
        return login(&t, &g, &cli, &log);
    }
    if args.len() >= 2 && args[0] == "-p" && args[1] == "/usage" {
        return usage(&t, &g, &cli, &log);
    }
    // 会话一直开着：回归里 touch $CCA_T/hold，删掉它会话才往下走、退出
    while exists(&join(&[&t, "hold"])) {
        sleep(0.1);
    }
    let acct = env_or("CCA_ACCOUNT", "x");
    let tag = tag_of(&acct);
    let iid = join(&[&cli, "installation_id"]);
    if !exists(&iid) || read_text(&iid)?.chars().count() != 36 {
        write_text(&iid, &uuid4())?;
    }
    let js = join(&[&cli, "jetski_state.pbtxt"]);
    let uuid = if exists(&js) {
        let re = regex(&format!(r#"(?m)^installation_uuid:[{PY_WS}]*"([^"]*)""#))?;
        re.captures(&read_text(&js)?).map(|c| c[1].to_string())
    } else {
        None
    };
    log(&format!("AGY-DEVICE {} {} @ {g}", read_text(&iid)?, uuid.as_deref().unwrap_or("-")))?;
    let conv = join(&[&cli, "conversations"]);
    makedirs(&conv)?;
    write_text(&join(&[&conv, &format!("from-{tag}.pb")]), &format!("conversation {tag}"))?;
    append_text(&join(&[&cli, "history.jsonl"]), &format!("{}\n", dumps(&obj! { "display" => format!("hi from {tag}") })))?;
    // 像 agy 保存设置：在原路径旁边写临时文件再改名（会把链接换成普通文件）
    let sp = join(&[&cli, "settings.json"]);
    let mut cur = load_json_or_empty(&sp)?;
    set_item(&mut cur, "lastWriter", Value::from(tag))?;
    write_text(&format!("{sp}.tmp"), &dumps(&cur))?;
    replace(&format!("{sp}.tmp"), &sp)?;
    let lc = join(&[&cli, "cache", "last_conversations.json"]);
    makedirs(&dirname(&lc))?;
    let mut j = load_json_or_empty(&lc)?;
    set_item(&mut j, &getcwd()?, Value::from(format!("conv-{tag}")))?;
    write_text(&lc, &dumps(&j))?;
    makedirs(&join(&[&g, "config", "skills", &format!("from-{tag}")]))?;
    // 像真的 agy（1.5.x）：config/ 里没有就建一个 0 字节的 mcp_config.json
    let mcp = join(&[&g, "config", "mcp_config.json"]);
    if !lexists(&mcp) {
        write_text(&mcp, "")?;
    }
    statusline(&t, &cli, &args, &log)?;
    if let Some(conv) = mark(&t, "agy_cost")?.filter(|c| !c.is_empty()) {
        reply(&cli, &conv)?;
    }
    Ok(3)
}

/// 像 agy 画状态栏：交互的会话（不是 --version、打印模式、帮助、子命令）里，--settings（两种写法，最后一个算）里的 statusLine，
/// 没有就用数据目录 settings.json 里的；它的 command（enabled 不是 false）用 sh -c 跑（开头的 ~ 换成 HOME，像 agy），stdin 是
/// 1.5.18 给的那种 JSON（email 取 token 里的 id_token_email；plan_tier 取 $CCA_T/agy_plan_tier，conversation_title 取 $CCA_T/agy_title，
/// 没有这个文件就不给）。状态变了 agy 就画一次：$CCA_T/agy_states 一行画一次（「状态」或者「状态 ask」：tool_confirmation_pending
/// 是 true），没有这个文件画一次 idle。agy.log 里记 AGY-STATUSLINE-CMD 命令（一次），每画一次记 AGY-STATUSLINE rc=退出码 out=输出
/// （换行写成 \n）
fn statusline(t: &str, cli: &str, args: &[String], log: &dyn Fn(&str) -> R<()>) -> R<()> {
    const ONE_SHOT: &[&str] = &["--version", "-p", "--print", "--prompt", "--help", "-h"];
    const SUBCOMMANDS: &[&str] =
        &["acp", "agent", "agents", "changelog", "help", "install", "mcp", "mic-serve", "models", "plugin", "plugins", "update"];
    if args.iter().any(|a| ONE_SHOT.contains(&a.split('=').next().unwrap_or_default()))
        || args.first().is_some_and(|a| SUBCOMMANDS.contains(&a.as_str()))
    {
        return Ok(());
    }
    let mut given = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--settings" {
            given = it.next().cloned();
        } else if let Some(v) = a.strip_prefix("--settings=") {
            given = Some(v.to_string());
        }
    }
    let given = match given {
        Some(s) => json::parse(&s).or_else(|_| fail("agy: --settings 不是 JSON"))?,
        None => Value::Null,
    };
    let saved = load_json_or_empty(&join(&[cli, "settings.json"]))?;
    let line = [&given, &saved].into_iter().find_map(|s| s.as_obj().and_then(|o| o.get("statusLine")).filter(|l| !l.is_null()));
    let Some(line) = line.and_then(Value::as_obj) else { return Ok(()) };
    if line.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Ok(());
    }
    let Some(cmd) = line.get("command").and_then(Value::as_str).filter(|c| !c.trim().is_empty()) else { return Ok(()) };
    let cmd = match cmd.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", home_dir()),
        _ => cmd.to_string(),
    };
    let token = load_json_or_empty(&join(&[cli, "antigravity-oauth-token"]))?;
    let (tier, title) = (mark(t, "agy_plan_tier")?, mark(t, "agy_title")?);
    let states = mark(t, "agy_states")?.unwrap_or_else(|| "idle".to_string());
    log(&format!("AGY-STATUSLINE-CMD {cmd}"))?;
    for line in states.lines().filter(|l| !l.trim().is_empty()) {
        let mut words = line.split_whitespace();
        let (state, ask) = (words.next().unwrap_or("idle"), words.next() == Some("ask"));
        let mut p = obj! { "cwd" => getcwd()?, "session_id" => "", "version" => "1.5.18", "product" => "antigravity", "agent_state" => state, "tool_confirmation_pending" => ask, "terminal_width" => 100 };
        if let Some(e) = token.as_obj().and_then(|o| o.get("id_token_email")) {
            set_item(&mut p, "email", e.clone())?;
        }
        if let Some(tier) = &tier {
            set_item(&mut p, "plan_tier", Value::from(tier.as_str()))?;
        }
        if let Some(title) = &title {
            set_item(&mut p, "conversation_title", Value::from(title.as_str()))?;
        }
        let d = run_capture(&["sh".to_string(), "-c".to_string(), cmd.clone()], Some(dumps(&p).as_bytes()), Some((5.0, "5")))?;
        log(&format!("AGY-STATUSLINE rc={} out={}", d.code, d.stdout.replace('\n', "\\n")))?;
    }
    Ok(())
}

/// 折合金额（$CCA_T/agy_cost 的内容是会话 id）：像 agy 1.5.16，在这次运行的日志（cli-<本地时间>.log）里写下用的会话，往共享的
/// 会话库（conversations/<会话 id>.db，SQLite：用系统的 sqlite3 写）里加一次 Gemini 4 Neon 的回复：gen_metadata 一行（模型编号 1532、
/// 名字），steps 一行（时间是现在，100 万输入、10 万输出、2 万缓存读：按 3 / 15 / 0.15 美元每百万折合 4.503 美元）。protobuf 的字段号照
/// cli/src/cmd/cost/agy.rs
fn reply(cli: &str, conv: &str) -> R<()> {
    fn vint(mut x: u64, out: &mut Vec<u8>) {
        loop {
            let b = (x & 0x7f) as u8;
            x >>= 7;
            if x == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn fint(n: u64, x: u64, out: &mut Vec<u8>) {
        vint(n << 3, out);
        vint(x, out);
    }
    fn fbytes(n: u64, b: &[u8], out: &mut Vec<u8>) {
        vint((n << 3) | 2, out);
        vint(b.len() as u64, out);
        out.extend_from_slice(b);
    }
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let rid = format!("rid{}x{now}", std::process::id());
    let mut usage = Vec::new();
    fint(1, 1532, &mut usage);
    fint(2, 1_000_000, &mut usage);
    fint(3, 100_000, &mut usage);
    fint(5, 20_000, &mut usage);
    fint(6, 24, &mut usage);
    fbytes(7, rid.as_bytes(), &mut usage);
    let (mut at, mut user, mut step) = (Vec::new(), Vec::new(), Vec::new());
    fint(1, now, &mut at);
    fbytes(1, &at, &mut user); // 用户输入的那一步：没有用量
    fbytes(1, &at, &mut step);
    fbytes(9, &usage, &mut step);
    fint(11, 1532, &mut step);
    let (mut inner, mut gen) = (Vec::new(), Vec::new());
    fbytes(4, &usage, &mut inner);
    fbytes(19, b"gemini-4-neon", &mut inner);
    fbytes(1, &inner, &mut gen);
    append_text(
        &join(&[cli, "log", &format!("cli-{}.log", local_stamp())]),
        &format!("I1125 10:00:00.000000 1 server.go:1233] Created conversation {conv} (backend=BACKEND_TYPE_UNSPECIFIED)\n"),
    )?;
    let db = join(&[cli, "conversations", &format!("{conv}.db")]);
    let sql = format!(
        "create table if not exists steps (idx integer primary key, step_type integer not null default 0, status integer not null default 0, metadata blob);\n\
         create table if not exists gen_metadata (idx integer primary key, data blob, size integer not null default 0);\n\
         insert into gen_metadata (data) values (X'{}');\n\
         insert into steps (step_type, status, metadata) values (14, 3, X'{}');\n\
         insert into steps (step_type, status, metadata) values (15, 3, X'{}');\n",
        hex(&gen),
        hex(&user),
        hex(&step)
    );
    let mut child = std::process::Command::new("sqlite3")
        .arg(&db)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()?;
    child.stdin.take().expect("piped").write_all(sql.as_bytes())?;
    if !child.wait()?.success() {
        return fail(format!("sqlite3 写不了 {db}"));
    }
    Ok(())
}

fn login(t: &str, g: &str, cli: &str, log: &dyn Fn(&str) -> R<()>) -> R<i32> {
    let name = format!("acp-login-{}-{}.log", local_stamp(), std::process::id());
    let mut text = String::from("I1124 11:01:12.000000 1 main.go:1] agy 1.5.16\n");
    let forced = matches!(py::strip(&env_or("AGY_CLI_FORCE_FILE_TOKEN_STORAGE", "")).to_lowercase().as_str(), "1" | "true");
    if forced && mark(t, "agy_noswitch")?.is_none() {
        text.push_str(
            "I1124 11:01:12.000001 1 composite_token_storage.go:207] Using file-based token storage because AGY_CLI_FORCE_FILE_TOKEN_STORAGE is set\n",
        );
    } else if env("SSH_CONNECTION").is_some_and(|s| !s.is_empty()) && mark(t, "agy_keychain")?.is_none() {
        text.push_str(
            "I1124 11:01:12.000001 1 composite_token_storage.go:125] Using file-based token storage because SSH session detected\n",
        );
    }
    write_text(&join(&[cli, "log", &name]), &text)?;
    let state = mark(t, "agy_state")?.filter(|s| !s.is_empty()).unwrap_or_else(|| "st-agy".to_string());
    let q = [
        ("access_type", "offline"),
        ("client_id", "000000000000-test.apps.googleusercontent.com"),
        ("code_challenge", "chal-test"),
        ("code_challenge_method", "S256"),
        ("prompt", "consent"),
        ("redirect_uri", "https://antigravity.google/oauth-callback"),
        ("response_type", "code"),
        ("scope", "openid email profile"),
        ("state", state.as_str()),
    ];
    print_flush(&format!(
        "Open this link in a browser to log in with Google:\n\nhttps://accounts.google.com/o/oauth2/auth?{}\n",
        urlencode(&q)
    ));
    let stdin = io::stdin();
    let mut input = stdin.lock();
    loop {
        write_out("Then paste the authorization code, or the full URL of the page you land on, and press Enter: ");
        flush();
        let mut buf = Vec::new();
        if input.read_until(b'\n', &mut buf)? == 0 {
            print_flush("\nlogin cancelled");
            return Ok(1);
        }
        let line = String::from_utf8_lossy(&buf).into_owned();
        let mut code = py::strip(&line).to_string();
        if code.starts_with("http") {
            let qq = parse_qsl(&urlsplit(&code)?.query);
            if qs_get(&qq, "state") != Some(state.as_str()) {
                print_flush("Error: state mismatch");
                return Ok(1);
            }
            code = qs_get(&qq, "code").unwrap_or_default().to_string();
        }
        log(&format!(
            "AGY-EXCHANGE code={code} https_proxy={} HTTPS_PROXY={}",
            env_or("https_proxy", "unset"),
            env_or("HTTPS_PROXY", "unset")
        ))?;
        if code.starts_with("good") {
            break;
        }
        print_flush("Error: failed to exchange the authorization code: invalid_grant");
    }
    let acct = env_or("CCA_ACCOUNT", "");
    let re = regex(&format!("([^(){PY_WS}]+@[^(){PY_WS}]+)"))?;
    let mut email = match re.captures(&acct) {
        Some(c) => c[1].trim_end_matches(')').to_string(),
        None => "someone@example.com".to_string(),
    };
    let alt = join(&[t, "agy_login_as"]);
    if exists(&alt) {
        email = py::strip(&read_text(&alt)?).to_string();
        remove(&alt)?;
    }
    let mut tok = Obj::new();
    tok.insert("access_token", "ya29.test");
    tok.insert("token_type", "Bearer");
    tok.insert("refresh_token", "1//test");
    tok.insert("expiry", "2026-11-12T12:00:00Z");
    if mark(t, "agy_noemail")?.is_none() {
        tok.insert("id_token_email", email.as_str());
    }
    tok.insert("user_tier", mark(t, "agy_tier")?.filter(|s| !s.is_empty()).unwrap_or_else(|| "g1-pro-tier".to_string()));
    let p = join(&[cli, "antigravity-oauth-token"]);
    let tmp = format!("{p}.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| Fail(os_text(&e, Some(&tmp))))?;
    f.write_all(dumps(&Value::Obj(tok)).as_bytes())?;
    drop(f);
    replace(&tmp, &p)?;
    log(&format!("AGY-LOGIN {email} @ {g}"))?;
    print_flush(&format!("Logged in as {email}."));
    Ok(0)
}

/// 打印模式的一行回答（`json.dumps(j, separators=(',', ':'))`）
fn answer(status: &str, error: &str, command: Option<Value>, response: &str, cid: &str) {
    let mut j = Obj::new();
    j.insert("conversation_id", cid);
    j.insert("status", status);
    j.insert("response", response);
    j.insert("error", error);
    j.insert("duration_seconds", 0.4);
    j.insert("num_turns", 0);
    j.insert(
        "usage",
        obj! { "input_tokens" => 0, "output_tokens" => 0, "thinking_tokens" => 0, "cache_read_tokens" => 0, "total_tokens" => 0 },
    );
    if let Some(command) = command {
        j.insert("command", command);
    }
    j.insert("finished_reason", if status == "ERROR" { "error" } else { "stop" });
    print_flush(&json::dumps(&Value::Obj(j), &Style { compact: true, ..Style::default() }));
}

/// `time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() + hours * 3600))`
fn utc_after(hours: f64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let d = kit::plist::date_from_unix((now + hours * 3600.0).floor() as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", d.year, d.month, d.day, d.hour, d.minute, d.second)
}

fn err_out(text: &str) {
    let mut err = io::stderr();
    let _ = err.write_all(text.as_bytes());
    let _ = err.flush();
}

/// `-p /usage --output-format json`：让 agy 自己查额度（打印模式）
fn usage(t: &str, g: &str, cli: &str, log: &dyn Fn(&str) -> R<()>) -> R<i32> {
    // 有没有控制终端：/dev/tty 打不打得开
    let tty = if std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").is_ok() { "yes" } else { "no" };
    log(&format!("AGY-USAGE tty={tty} auto_update={} cwd={} @ {g}", env_or("AGY_CLI_DISABLE_AUTO_UPDATE", "unset"), getcwd()?))?;
    let tokp = join(&[cli, "antigravity-oauth-token"]);
    while mark(t, "agy_usage_hang")?.is_some() {
        sleep(0.1);
    }
    if !exists(&tokp) || mark(t, "agy_usage_auth")?.is_some() {
        err_out(
            "Authentication required. Please visit the URL to log in:\n  https://accounts.google.com/o/oauth2/auth?access_type=offline&state=st-usage\n\n\
             Waiting for authentication (timeout 60s)...\nOr, paste the authorization code here and press Enter:\n",
        );
        sleep(60.0);
        err_out("Error: authentication timed out.\nerror: authentication failed or timed out\n");
        answer("ERROR", "authentication failed or timed out", None, "", "");
        return Ok(1);
    }
    if mark(t, "agy_usage_old")?.is_some() {
        answer("SUCCESS", "", None, "I cannot look up your quota from here.", &uuid4());
        return Ok(0);
    }
    if let Some(bad) = mark(t, "agy_usage_error")? {
        answer("ERROR", if bad.is_empty() { "quota service unavailable" } else { &bad }, None, "", "");
        return Ok(1);
    }
    let token = load_json(&tokp)?;
    let email = get(&token, "id_token_email")?.map(py_str).unwrap_or_default();
    log(&format!("AGY-USAGE-AS {email} https_proxy={}", env_or("https_proxy", "unset")))?;
    let p = join(&[t, "agy_usage", &format!("{email}.json")]);
    let data = if exists(&p) {
        load_json(&p)?
    } else {
        let bucket = |id: &str, name: &str, window: &str, left: f64, hours: f64| {
            obj! { "id" => id, "name" => name, "window" => window, "remaining_fraction" => left, "reset_time" => utc_after(hours) }
        };
        obj! {
            "description" => "Quota for Antigravity models",
            "groups" => list![
                obj! { "name" => "Gemini Pro", "buckets" => list![
                    bucket("gemini-pro-5h", "Five Hour Limit Remaining", "5h", 0.87, 2.5),
                    bucket("gemini-pro-week", "Weekly Limit Remaining", "weekly", 0.25, 80.0),
                ] },
                obj! { "name" => "Claude", "description" => "Claude and GPT-OSS models", "buckets" => list![
                    bucket("claude-5h", "Five Hour Limit Remaining", "5h", 0.05, 1.2),
                ] },
                obj! { "name" => "Gemini Flash", "buckets" => list![obj! { "id" => "flash", "name" => "Gemini Flash", "disabled" => true }] },
            ],
        }
    };
    answer("SUCCESS", "", Some(obj! { "name" => "usage", "data" => data }), "", &uuid4());
    Ok(0)
}
