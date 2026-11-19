//! Codex 和 agy 的登录身份、codex 的探测和 app-server（`agy_claims` 到 `codex_fix_rollouts`）：reset-consume、login-flow。
//!
//! 先只移植了 `codex_probe`（`env` 要用它）；其余还没移植。

use super::Cmd;
use crate::core::tool::{codex, CODEX_FLAGS};
use crate::dict;
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
