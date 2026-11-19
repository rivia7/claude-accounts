//! Codex 和 agy 的登录身份、codex 的探测和 app-server（`agy_claims` 到 `codex_fix_rollouts`）：reset-consume、login-flow。
//!
//! 移植了 `codex_probe`（`env` 要用它）和 `codex_fix_rollouts`（Codex 的 link 收尾要用它）；其余还没移植。

use super::settings::{read_toml_config, ConfigReadError};
use super::Cmd;
use crate::core::tool::{codex, CODEX_FLAGS};
use crate::py::R;
use crate::{dict, err};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::obj::{str_list, Dict, V};
use crate::py::path;
use crate::py::proc::{self, Cmd as Proc, In, Out};
use crate::py::text;
use crate::re;
use std::os::unix::fs::MetadataExt;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// 这个版本的 codex 认得哪些 cca 要用的参数：不认识的 --disable 开关、参数 codex 会直接报错退出，开关不存在了也就不会
/// 改走系统代理。按 codex 程序文件缓存（换了版本才重新问，问一次要一秒多）。问不出来时照样全加上（宁可启动失败）。
/// 返回 `{'sig', 'flags', 'no_daemon', 'version'}`（问不出来时没有 sig）
pub fn codex_probe(bin_: &str) -> Dict {
    let full = dict! { "flags" => str_list(CODEX_FLAGS), "no_daemon" => true, "version" => "" };
    if bin_.is_empty() {
        return full;
    }
    let real = path::realpath(bin_);
    let Ok(st) = std::fs::metadata(&real) else {
        return full;
    };
    let mut sig = format!("{}|{}|{}", real, st.size(), st.mtime());
    if real.ends_with(".js") {
        // npm 装的：启动脚本的大小、时间不随版本变，版本号在包里的 package.json
        let pj = load_quiet(&path::join(&path::dirname(&path::dirname(&real)), "package.json"));
        let version = match pj.as_ref().and_then(|j| j.as_dict()) {
            Some(d) => d.get("version").unwrap_or(V::None).py_str(),
            None => String::new(),
        };
        sig.push('|');
        sig.push_str(&version);
    }
    let master = &codex().master;
    let cache = path::join(&codex().sroot, "probe.json");
    if let Some(V::Dict(j)) = load_quiet(&cache) {
        if j.get("sig").is_some_and(|s| s.as_str() == Some(sig.as_str())) && j.list("flags").is_some() {
            return j;
        }
    }
    let mut env: Vec<(String, String)> =
        proc::environ().into_iter().filter(|(k, _)| !re!(r"^(CODEX_|OPENAI_|CCA_(EVENTS|GRANT|RO)$)").matches(k)).collect();
    proc::env_set(&mut env, "CODEX_HOME", master);
    let _ = fs::makedirs(master);
    let run = |args: &[&str]| -> (i32, String) {
        let argv: Vec<&str> = std::iter::once(bin_).chain(args.iter().copied()).collect();
        match Proc::new(argv).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).env(env.clone()).cwd(master.clone()).timeout(30.0).run() {
            Ok(d) => (d.rc, d.out()),
            Err(_) => (1, String::new()),
        }
    };
    let (rc, out) = run(&["features", "list"]);
    if rc != 0 || text::strip(&out).is_empty() {
        return full;
    }
    let known: Vec<&str> = text::splitlines(&out).into_iter().filter(|l| !text::strip(l).is_empty()).map(|l| text::split_ws(l)[0]).collect();
    let mut flags = Vec::new();
    for f in CODEX_FLAGS.iter().skip(1).step_by(2) {
        if known.contains(f) {
            flags.push("--disable");
            flags.push(f);
        }
    }
    let (rc, help) = run(&["--help"]);
    let ver_out = run(&["--version"]).1;
    let ver = text::split_ws(text::strip(&ver_out));
    let j = dict! {
        "sig" => sig,
        "flags" => str_list(flags),
        "no_daemon" => rc != 0 || help.contains("--no-daemon"),
        "version" => ver.last().copied().unwrap_or(""),
    };
    let _ = save_json(&cache, &V::Dict(j.clone()), Some(0o600), true); // 存不了（OSError）就算了
    j
}

/// `sqlite3` 的 `.mode quote` 输出 → 各行各列的字面量原文（字符串带着引号，里面的 `''` 没还原）。
/// 一行一条记录、逗号隔开；字符串里可以有逗号和换行。写得不对（引号没配对）是 None
fn sql_rows(out: &str) -> Option<Vec<Vec<String>>> {
    let (mut rows, mut row, mut field, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            field.push(c);
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    field.push(chars.next()?);
                } else {
                    quoted = false;
                }
            }
            continue;
        }
        match c {
            '\'' => {
                quoted = true;
                field.push(c);
            }
            ',' => row.push(std::mem::take(&mut field)),
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\r' => {}
            c => field.push(c),
        }
    }
    if quoted {
        return None;
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Some(rows)
}

/// SQL 的字符串字面量 `'…'` → 文字；不是字符串（数字、NULL、X'…'）是 None
fn sql_text(literal: &str) -> Option<String> {
    Some(literal.strip_prefix('\'')?.strip_suffix('\'')?.replace("''", "'"))
}

/// 文字 → SQL 的字符串字面量
fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 给 sqlite3 一段命令（从 stdin），拿它的 stdout；起不来、出错（库被占着、不是数据库、没有这张表……）是 None
fn sqlite3(db: &str, script: &str) -> Option<String> {
    let done = Proc::new(["sqlite3", "-batch", "-bail", db]).input(script.as_bytes()).stdout(Out::Capture).stderr(Out::Null).timeout(30.0).run().ok()?;
    (done.rc == 0).then(|| done.out())
}

/// `glob.glob(glob.escape(p) + '*')` 有没有结果：p 所在的目录里有名字以 p 的最后一段开头的东西（不带点开头的那一段不算隐藏文件）
fn any_with_prefix(p: &str) -> bool {
    let (dir, base) = (path::dirname(p), path::basename(p));
    let Ok(names) = fs::listdir_sorted(if dir.is_empty() { "." } else { &dir }) else { return false };
    names.iter().any(|n| n.starts_with(base) && (base.starts_with('.') || !n.starts_with('.')))
}

/// 经账号目录的会话路径 → ~/.codex 下的同一个文件：`^账号目录的上一层/[^/]+/子目录/(sessions|archived_sessions)/` 换成
/// `~/.codex/那一段/`；对不上原样返回
fn rollout_in_master(old: &str, pre: &str, sub: &str, master: &str) -> String {
    let moved = || {
        let (account, rest) = old.strip_prefix(pre)?.split_once('/')?;
        let rest = rest.strip_prefix(sub)?.strip_prefix('/')?;
        if account.is_empty() {
            return None;
        }
        ["sessions", "archived_sessions"].iter().find_map(|kind| Some(format!("{}/{}", path::join(master, kind), rest.strip_prefix(kind)?.strip_prefix('/')?)))
    };
    moved().unwrap_or_else(|| old.to_string())
}

/// Codex 的会话库（state_N.sqlite 的 threads 表）记着每个会话文件的路径，经由开它的那个账号目录（sessions 是指向
/// ~/.codex/sessions 的链接）。账号删掉后这些路径就断了，会话会从 codex resume 的全部会话里消失（文件还在）。
/// 这里把它们改写成 ~/.codex 下的同一个文件：只改 rollout_path 这一列，只改确实有对应文件的。库被别的 codex 占着时
/// 跳过，下次再改。
///
/// 原来用 Python 自带的 sqlite3 库；这里起系统的 `sqlite3` 命令（macOS 自带；Linux 上没装这个命令就跳过，见 FINDINGS）。
/// 路径都从 stdin 交给它，不进命令行
pub fn codex_fix_rollouts() -> R<()> {
    let tool = codex();
    let home = match read_toml_config(&path::join(&tool.master, "config.toml")).map(|cfg| cfg.get("sqlite_home")) {
        Ok(None) => String::new(),
        Ok(Some(V::Str(home))) => home.to_string(),
        // sqlite_home 不是字符串（invalid_sqlite_home）和配置读不了、解析不了一样对待
        Ok(Some(_)) => {
            err!("cca: ~/.codex/config.toml 无效或不可读，已跳过会话路径修复；启动前配置校验将拒绝此配置");
            return Ok(());
        }
        Err(e) if e.is(ConfigReadError::CLASS) => {
            err!("cca: ~/.codex/config.toml 无效或不可读，已跳过会话路径修复；启动前配置校验将拒绝此配置");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let home = Some(path::expanduser(&home)).filter(|h| !h.is_empty()).unwrap_or_else(|| tool.master.clone());
    let pre = format!("{}/", path::join(&tool.sroot, "accounts"));
    let Ok(names) = fs::listdir_sorted(&home) else { return Ok(()) };
    for name in names.iter().filter(|n| n.starts_with("state_") && n.ends_with(".sqlite") && n.len() >= "state_.sqlite".len()) {
        let db = path::join(&home, name);
        // 一次问出要看的行；没有 threads 表、没有 rollout_path 这一列、库被占着超过 2 秒都是出错：跳过这个库
        let query = format!(".timeout 2000\n.mode quote\nSELECT id, rollout_path FROM threads WHERE substr(rollout_path, 1, {}) = {};\n", text::len(&pre), sql_quote(&pre));
        let Some(rows) = sqlite3(&db, &query).and_then(|out| sql_rows(&out)) else { continue };
        let mut script = String::from(".timeout 2000\nBEGIN;\n");
        let mut fixes = 0;
        for row in rows.iter().filter(|r| r.len() == 2) {
            let Some(old) = sql_text(&row[1]) else { continue };
            let new = rollout_in_master(&old, &pre, tool.sub, &tool.master);
            if new != old && !new.contains('\0') && any_with_prefix(&new) {
                script.push_str(&format!("UPDATE threads SET rollout_path = {} WHERE id = {} AND rollout_path = {};\n", sql_quote(&new), row[0], row[1]));
                fixes += 1;
            }
        }
        if fixes > 0 {
            script.push_str("COMMIT;\n");
            sqlite3(&db, &script); // 没改成（库正被占着）就算了，下次再改
        }
    }
    Ok(())
}

#[cfg(test)]
mod rollout_tests {
    use super::*;

    #[test]
    fn quote_mode_rows() {
        let rows = sql_rows("'a','/x/it''s, here\nsecond line'\n42,NULL\nX'00ff','plain'\n").unwrap();
        assert_eq!(rows, vec![vec!["'a'", "'/x/it''s, here\nsecond line'"], vec!["42", "NULL"], vec!["X'00ff'", "'plain'"]]);
        assert_eq!(sql_text(&rows[0][1]).unwrap(), "/x/it's, here\nsecond line");
        assert_eq!(sql_text("42"), None);
        assert_eq!(sql_quote("it's"), "'it''s'");
        assert!(sql_rows("'never closed").is_none());
        assert!(sql_rows("").unwrap().is_empty());
        assert_eq!(sql_rows("'last row without newline',1").unwrap().len(), 1);
    }

    #[test]
    fn rollout_paths_move_to_master() {
        let (pre, master) = ("/h/.cca/codex/accounts/", "/h/.codex");
        let fix = |old: &str| rollout_in_master(old, pre, "codex", master);
        assert_eq!(fix("/h/.cca/codex/accounts/erin-1a2b3c/codex/sessions/2026/11/03/rollout-x.jsonl"), "/h/.codex/sessions/2026/11/03/rollout-x.jsonl");
        assert_eq!(fix("/h/.cca/codex/accounts/a/codex/archived_sessions/r.jsonl"), "/h/.codex/archived_sessions/r.jsonl");
        assert_eq!(fix("/h/.cca/codex/accounts/a/codex/sessions/"), "/h/.codex/sessions/");
        for same in [
            "/h/.cca/codex/accounts/a/codex/worktrees/x",
            "/h/.cca/codex/accounts/a/codex/sessions",
            "/h/.cca/codex/accounts//codex/sessions/x",
            "/h/.cca/codex/accounts/a/b/codex/sessions/x",
            "/h/.cca/codex/accounts/a/codexx/sessions/x",
            "/h/.cca/codex/accounts/a/codex/sessionsx/x",
            "/elsewhere/sessions/x",
            "",
        ] {
            assert_eq!(fix(same), same);
        }
    }
}
