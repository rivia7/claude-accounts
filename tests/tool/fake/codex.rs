//! 假 codex 里原来内嵌的几段 Python：features disable 改 config.toml、第三方 API 账号执行 -c model_providers.cca 里 auth 的命令取 key、
//! 一次会话往 CODEX_HOME 里写的东西（会话库用 /usr/bin/sqlite3）。

use super::claude::{arg, str_arg};
use super::*;

/// Python 的 \s（str.isspace() 认的那些）：写进正则的字符类里
pub const PY_WS: &str = r"\t\n\x0B\x0C\r\x1C-\x1F \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

pub fn regex(re: &str) -> R<regex::Regex> {
    regex::Regex::new(re).map_err(|e| Fail(format!("re.error: {e}")))
}

/// codex-features config.toml 开关：features disable（像真的 codex：顺着链接写回原文件，临时文件 + 改名，放在原文件旁边）
pub fn features(a: &[String]) -> R<i32> {
    let (cfg, key) = (arg(a, 0)?, arg(a, 1)?);
    let p = realpath(cfg)?;
    let s = if exists(&p) { read_text(&p)? } else { String::new() };
    let set = regex(&format!("^[{PY_WS}]*{}[{PY_WS}]*=", regex::escape(key)))?;
    let (mut result, mut inside, mut found) = (Vec::<String>::new(), false, false);
    for line in splitlines(&s) {
        let st = py::strip(line);
        if st.starts_with('[') {
            inside = st == "[features]";
            result.push(line.to_string());
            if inside {
                found = true;
                result.push(format!("{key} = false"));
            }
        } else if !(inside && set.is_match(line)) {
            result.push(line.to_string());
        }
    }
    if !found {
        result.extend([String::new(), "[features]".to_string(), format!("{key} = false")]);
    }
    let tmp = format!("{p}.tmp");
    write_text(&tmp, &format!("{}\n", result.join("\n")))?;
    replace(&tmp, &p)?;
    Ok(0)
}

/// re.search(…).group(1)：没找到是异常（原来的 'NoneType' object has no attribute 'group'）
fn group1<'a>(re: &str, s: &'a str) -> R<&'a str> {
    let c = regex(re)?.captures(s).ok_or_else(|| Fail("AttributeError: 'NoneType' object has no attribute 'group'".into()))?;
    Ok(c.get(1).map_or("", |m| m.as_str()))
}

fn loads(s: &str) -> R<Value> {
    json::parse(s).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))
}

/// codex-provider model_providers.cca的值 CCA_T：stdout 接在 codex.log 后面（攒到最后一起写）
pub fn provider(a: &[String]) -> R<i32> {
    let (s, t) = (arg(a, 0)?, arg(a, 1)?);
    let cmd = loads(group1(r#"command = ("(?:[^"\\]|\\.)*")"#, s)?)?;
    let args = loads(&format!("[{}]", group1(r"args = \[(.*?)\]", s)?))?;
    let base = loads(group1(r#"base_url = ("(?:[^"\\]|\\.)*")"#, s)?)?;
    let wire = group1(r#"wire_api = "(\w+)""#, s)?;
    print(&format!("PROVIDER: base_url={} wire_api={wire}", py_str(&base)));
    let mut argv = vec![str_arg(&cmd)?.to_string()];
    for x in args.as_list().unwrap_or_default() {
        argv.push(str_arg(x)?.to_string());
    }
    let r = run_capture(&argv, None, None)?;
    write_text(&format!("{t}/codex.key"), &r.stdout)?;
    print(&format!("AUTHCMD: rc={}", r.code));
    Ok(0)
}

/// codex-notify notify的值 JSON：像 Codex 一轮做完了起 notify 的命令。notify 的值是 -c 给的 TOML 字符串数组（cca 的
/// codex_launch_args 写的那种：每一项是 TOML 的基本字符串，转义和 JSON 的一样），拆开以后 JSON 加在最后起它、等它结束；
/// stdout 是一行 NOTIFY rc=退出码 程序（接在 codex.log 后面）
pub fn notify(a: &[String]) -> R<i32> {
    let (v, payload) = (arg(a, 0)?, arg(a, 1)?);
    let mut argv = Vec::new();
    for m in regex(r#""(?:[^"\\]|\\.)*""#)?.find_iter(v) {
        argv.push(str_arg(&loads(m.as_str())?)?.to_string());
    }
    let Some((prog, rest)) = argv.split_first() else { return Err(Fail(format!("notify 的值里没有命令：{v}"))) };
    let st = std::process::Command::new(prog).args(rest).arg(payload).status();
    let code = st.ok().and_then(|s| s.code()).unwrap_or(-1);
    print(&format!("NOTIFY rc={code} {prog}"));
    Ok(0)
}

/// 账号的短名：acct.split('(')[-1].split('@')[0].strip(')')
pub fn tag_of(acct: &str) -> &str {
    let last = acct.rsplit('(').next().unwrap_or_default();
    last.split('@').next().unwrap_or_default().trim_matches(')')
}

/// codex-session CODEX_HOME 会话库所在的目录 账号：一次会话往 CODEX_HOME 里写的东西
pub fn session(a: &[String]) -> R<i32> {
    let (home, dbhome, acct) = (arg(a, 0)?, arg(a, 1)?, arg(a, 2)?);
    let tag = tag_of(acct);
    // 像 codex 改 config.toml：顺着链接，在原文件旁边写临时文件再改名
    let cfg = join(&[home, "config.toml"]);
    let text = format!("{}# touched by {tag}\n", if exists(&cfg) { read_text(&cfg)? } else { String::new() });
    let real = realpath(&cfg)?;
    write_text(&format!("{real}.tmp"), &text)?;
    replace(&format!("{real}.tmp"), &real)?;
    append_text(&join(&[home, "history.jsonl"]), &format!("{}\n", dumps(&obj! { "text" => format!("hi from {tag}") })))?;
    // 像 codex delete：整个重写再改名（不顺着链接，会把链接换成普通文件）
    let idx = join(&[home, "session_index.jsonl"]);
    let old = if exists(&idx) { read_text(&idx)? } else { String::new() };
    let mut lines: Vec<String> = splitlines(&old).into_iter().map(str::to_string).collect();
    lines.push(dumps(&obj! { "id" => tag, "thread_name" => format!("from {tag}") }));
    write_text(&format!("{idx}.tmp"), &format!("{}\n", lines.join("\n")))?;
    replace(&format!("{idx}.tmp"), &idx)?;
    let tid = uuid4();
    let d = join(&[home, "sessions", "2026", "11", "03"]);
    makedirs(&d)?;
    let rollout = join(&[&d, &format!("rollout-{tid}.jsonl")]);
    write_text(&rollout, "{}\n")?;
    for (n, v) in [("installation_id", uuid4()), ("models_cache.json", "{}".to_string())] {
        let f = join(&[home, n]);
        if !exists(&f) {
            write_text(&f, &v)?;
        }
    }
    makedirs(&join(&[home, "worktrees", tag]))?;
    makedirs(&join(&[home, "skills", &format!("from-{tag}")]))?;
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let sql = format!(
        ".timeout 5000\nCREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, title TEXT);\nINSERT INTO threads VALUES ({}, {}, {});\n",
        q(&tid),
        q(&rollout),
        q(tag)
    );
    let db = join(&[dbhome, "state_5.sqlite"]);
    cloexec_all();
    let mut child = std::process::Command::new("/usr/bin/sqlite3")
        .args(["-batch", "-bail", "-init", "/dev/null", &db])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Fail(os_text(&e, Some("/usr/bin/sqlite3"))))?;
    if let Some(mut si) = child.stdin.take() {
        si.write_all(sql.as_bytes())?;
    }
    let st = child.wait()?;
    if !st.success() {
        return fail(format!("sqlite3.OperationalError: /usr/bin/sqlite3 exited with {st}"));
    }
    Ok(0)
}
