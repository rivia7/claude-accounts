//! 命令行会话登记、进程扫描、资料目录、运行记录（`lstart_epoch` 到 `gui_procs`）：private-profile、browser-profile、
//! runtime-record、session-open。
//!
//! 这一块的子命令都移植了。

use super::presence::presence_wake;
use super::table::need;
use super::{arg, arity, Cmd};
use crate::core::base::Lock;
use crate::core::ctx::ctx;
use crate::core::model::*;
use crate::core::tool::t;
use crate::py::fs::{self, load_json, load_quiet, save_json};
use crate::py::obj::{repr_str, Dict, List, V};
use crate::py::path;
use crate::py::plist::{self, P};
use crate::py::proc::{env_set, environ, Cmd as Proc, In, Out};
use crate::py::re::{escape, Re};
use crate::py::text;
use crate::py::time::{strptime_local, time};
use crate::py::{die, Fail, R};
use crate::{dict, out, re};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("private-profile", cmd_private_profile),
    Cmd::all("browser-profile", cmd_browser_profile),
    Cmd::all("runtime-record", cmd_runtime_record),
    Cmd::all("session-open", cmd_session_open),
];

/// 旧配置可能把设备文件链到共享目录；不能沿用共享值，更不能写穿链接
pub fn private_file(p: &str) -> R<bool> {
    if path::islink(p) {
        return Ok(false);
    }
    Ok(!path::exists(p) || std::fs::metadata(p)?.nlink() == 1)
}

/// ps 的 lstart（C 语言环境的写法：Sat Nov  7 12:08:55 2026，本地时间）换成时间戳；读不懂返回 0
pub fn lstart_epoch(s: &str) -> i64 {
    strptime_local(&text::split_ws(s).join(" "), "%a %b %d %H:%M:%S %Y").map_or(0, |t| t as i64)
}

/// 跑一次 ps（args 里第一列是 pid），返回 [(pid, 其余)]；ps 跑不了就是 []
pub fn ps_rows(args: &[&str], timeout: f64, env: Option<Vec<(String, String)>>) -> Vec<(i64, String)> {
    let mut cmd = Proc::new(["ps"].iter().chain(args)).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).timeout(timeout);
    if let Some(env) = env {
        cmd = cmd.env(env);
    }
    let Ok(done) = cmd.run() else { return Vec::new() };
    let mut rows = Vec::new();
    for l in text::splitlines(&done.out()) {
        let (pid, _, rest) = text::partition(text::strip(l), " ");
        if text::isdigit(pid) {
            if let Some(pid) = text::to_int(pid) {
                rows.push((pid, rest.to_string()));
            }
        }
    }
    rows
}

/// 一个进程：父进程、终端、CPU 秒数、启动时间戳、命令行
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub ppid: i64,
    pub tty: String,
    pub cpu: f64,
    pub start: i64,
    pub cmd: String,
}

/// Linux：从 /proc 读进程 [(pid, …)]（busybox 的 ps 不认 -p、-x、lstart；GNU 的 ps 也不用，两边一个算法）。pids 不给就是全部
/// 进程，已经没有的不在结果里。不是 Linux 时是 None（照常问 ps）。启动时间是开机时刻（/proc/stat 的 btime）加上 stat 里的
/// starttime，和 ps 的 lstart 一样只到秒；终端照 ps 的写法（pts/3、tty1），没有是 ?
pub fn proc_scan(pids: Option<&[i64]>) -> Option<Vec<(i64, ProcInfo)>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    // SAFETY: sysconf 只读系统的一个常数
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    proc_scan_at("/proc", pids, if tck > 0 { tck as f64 } else { 100.0 })
}

/// `proc_scan` 的做法；root 是 /proc（测试里是照它的样子摆的目录）
fn proc_scan_at(root: &str, pids: Option<&[i64]>, tck: f64) -> Option<Vec<(i64, ProcInfo)>> {
    let stat = std::fs::read(path::join(root, "stat")).ok()?;
    let boot = match stat.split_inclusive(|b| *b == b'\n').find(|l| l.starts_with(b"btime ")) {
        // int(l.split()[1])：不是数字是 ValueError → None
        Some(l) => Some(text::to_int(text::split_ws(&String::from_utf8_lossy(l)).get(1)?)?),
        None => None,
    };
    let all: Vec<i64>;
    let pids = match pids {
        Some(p) => p,
        None => {
            all = std::fs::read_dir(root).ok()?.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| text::isdigit(n)).map(|n| text::to_int(&n)).collect::<Option<_>>()?;
            &all
        }
    };
    let boot = boot?;
    let mut out: Vec<(i64, ProcInfo)> = Vec::new();
    for pid in pids {
        // 读不到是刚退出
        let (Ok(st), Ok(cmd)) = (std::fs::read(format!("{root}/{pid}/stat")), std::fs::read(format!("{root}/{pid}/cmdline"))) else { continue };
        let st = String::from_utf8_lossy(&st).into_owned();
        let cmd: Vec<u8> = cmd.iter().map(|b| if *b == 0 { b' ' } else { *b }).collect();
        let cmd = text::strip(&String::from_utf8_lossy(&cmd)).to_string();
        // 进程名在括号里，里面可能有空格和括号
        let rest = match st.rfind(')') {
            Some(i) => st.get(i + 2..).unwrap_or(""),
            None => text::skip(&st, 1),
        };
        let f = text::split_ws(rest);
        let int = |i: usize| f.get(i).and_then(|s| text::to_int(s));
        let (Some(ppid), Some(nr), Some(utime), Some(stime), Some(start)) = (int(1), int(4), int(11), int(12), int(19)) else { continue };
        let (major, minor) = ((nr >> 8) & 0xfff, (nr & 0xff) | ((nr >> 12) & 0xfff00));
        let tty = if (136..=143).contains(&major) {
            format!("pts/{}", (major - 136) * 256 + minor)
        } else if major == 4 && 0 < minor && minor < 64 {
            format!("tty{minor}")
        } else {
            "?".to_string()
        };
        let info = ProcInfo { ppid, tty, cpu: (utime + stime) as f64 / tck, start: (boot as f64 + start as f64 / tck) as i64, cmd };
        match out.iter_mut().find(|(p, _)| p == pid) {
            Some(slot) => slot.1 = info,
            None => out.push((*pid, info)),
        }
    }
    Some(out)
}

/// {进程号: 启动时间戳}（一次 ps -p；Linux 上读 /proc）；已经没有的进程不在里面。lstart 的写法随 locale 变，固定成 C
pub fn proc_starts(pids: &[i64]) -> HashMap<i64, i64> {
    let mut pids: Vec<i64> = pids.iter().copied().filter(|p| *p > 0).collect();
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        return HashMap::new();
    }
    if let Some(lx) = proc_scan(Some(&pids)) {
        return lx.into_iter().map(|(pid, p)| (pid, p.start)).collect();
    }
    let list = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    ps_lstart(&list).into_iter().map(|(pid, s)| (pid, lstart_epoch(&s))).filter(|(_, t)| *t != 0).collect()
}

/// `ps -o pid=,lstart= -p 进程号,…`，环境里把 LC_ALL、LANG 固定成 C
pub fn ps_lstart(pids: &str) -> Vec<(i64, String)> {
    let mut env = environ();
    env_set(&mut env, "LC_ALL", "C");
    env_set(&mut env, "LANG", "C");
    ps_rows(&["-o", "pid=,lstart=", "-p", pids], 5.0, Some(env))
}

pub fn profile_path(key: &str, kind: &str) -> R<String> {
    if !matches!(kind, "app" | "browser") {
        return die("未知客户端资料类型");
    }
    let account = adir(key);
    let p = path::join(&account, if kind == "app" { "desktop" } else { "chrome" });
    if path::islink(&account) || path::islink(&p) {
        return die(format!("客户端资料目录不能是符号链接：{p}"));
    }
    fs::makedirs_mode(&p, 0o700)?;
    fs::chmod(&p, 0o700)?;
    Ok(path::realpath(&p))
}

pub fn cmd_private_profile(args: &[String]) -> R<()> {
    arity("cmd_private_profile", args, 2, 2)?;
    let _lock = Lock::acquire()?;
    out!("{}", profile_path(&args[0], &args[1])?);
    Ok(())
}

/// Chrome 关闭时写账号专属偏好；运行时只检查，避免和 Chrome 的原子保存竞争
pub fn browser_profile(key: &str, prepare: bool) -> R<()> {
    let root = profile_path(key, "browser")?;
    let mut names = vec!["Default".to_string()];
    for name in fs::listdir_sorted(&root)? {
        if path::isfile(&path::join_all(&[&root, &name, "Preferences"])) && !names.contains(&name) {
            names.push(name);
        }
    }
    names.sort();
    for name in &names {
        let folder = path::join(&root, name);
        let p = path::join(&folder, "Preferences");
        if path::islink(&folder) || !private_file(&p)? {
            return die(format!("浏览器偏好不能通过链接共享：{p}"));
        }
        let data = load_json(&p)?.unwrap_or_else(|| V::Dict(Dict::new()));
        let Some(data) = data.as_dict().filter(|d| d.get("webrtc").is_none_or(|w| w.is_dict())) else {
            return die(format!("无法读取浏览器 WebRTC 偏好：{p}"));
        };
        let webrtc = data.setdefault("webrtc", Dict::new()).as_dict().expect("上面核对过是字典");
        let wanted = [("ip_handling_policy", V::from("disable_non_proxied_udp")), ("ip_handling_url", V::List(List::new())), ("local_ips_allowed_urls", V::List(List::new()))];
        if prepare {
            fs::makedirs_mode(&folder, 0o700)?;
            for (k, v) in wanted {
                webrtc.set(k, v);
            }
            save_json(&p, &V::Dict(data), Some(0o600), false)?;
        } else if wanted.iter().any(|(k, v)| !webrtc.get(k).unwrap_or(V::None).py_eq(v)) {
            return die("浏览器 WebRTC 偏好已变化或有 URL 例外，请退出该账号的 Chrome 后重开");
        }
    }
    Ok(())
}

pub fn cmd_browser_profile(args: &[String]) -> R<()> {
    arity("cmd_browser_profile", args, 2, 2)?;
    if !matches!(args[0].as_str(), "prepare" | "check") {
        return die("未知浏览器偏好操作");
    }
    let _lock = Lock::acquire()?;
    browser_profile(&args[1], args[0] == "prepare")
}

/// `int(s)`：不是数字是没人接的 ValueError
fn int_arg(s: &str) -> R<i64> {
    text::to_int(s).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(s))))
}

/// 客户端程序的 (Info.plist, 主程序)：读不出来、主程序的名字不像话都是 None（原来的 OSError / ValueError / TypeError / AttributeError）
fn app_binary(app: &str) -> R<Option<(String, String)>> {
    let info_path = path::join_all(&[app, "Contents", "Info.plist"]);
    let Ok(raw) = std::fs::read(&info_path) else { return Ok(None) };
    let info = match plist::loads(&raw) {
        Ok(info) => info,
        Err(e) if e.is_value() || e.is("TypeError") || e.is("AttributeError") => return Ok(None),
        Err(e) => return Err(e),
    };
    let executable = match &info {
        P::Dict(_) => match info.get("CFBundleExecutable") {
            Some(P::Str(s)) => s.as_str(),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    if path::basename(executable) != executable || matches!(executable, "" | "." | "..") {
        return Ok(None);
    }
    Ok(Some((info_path, path::join_all(&[app, "Contents", "MacOS", executable]))))
}

/// 记录可核对的启动条件；不读取进程环境、Cookie 或凭证，不声称验证客户端所有请求
#[allow(clippy::too_many_arguments)]
fn runtime_snapshot(key: &str, kind: &str, pid: &str, profile: &str, app: &str, proxy: &str, route: &str, bypass: &str, no_proxy: &str) -> R<Dict> {
    let (_, table, _) = ctx()?;
    need(&table, key)?;
    let pid = int_arg(pid)?;
    let started = proc_starts(&[pid]).get(&pid).copied().unwrap_or(0);
    if started == 0 || proxy.is_empty() {
        return die("无法核验客户端进程或账号代理");
    }
    let expected = profile_path(key, kind)?;
    if path::realpath(profile) != expected {
        return die("客户端资料目录与账号不符");
    }
    let pid_text = pid.to_string();
    let commands = ps_rows(&["-ww", "-o", "pid=,command=", "-p", &pid_text], 5.0, None);
    let command = commands.into_iter().find(|(p, _)| *p == pid).map_or(String::new(), |(_, s)| s);
    let mut flags = vec![("user-data-dir", profile), ("proxy-server", proxy)];
    if kind == "app" {
        flags.push(("proxy-bypass-list", bypass));
    } else {
        flags.push(("webrtc-ip-handling-policy", "disable_non_proxied_udp"));
        browser_profile(key, false)?;
    }
    for (flag, value) in flags {
        let mentions = Re::new(&format!(r"(?:^|\s)--{flag}(?:=|\s)")).findall(&command).len();
        let exact = Re::new(&format!(r"(?:^|\s)--{flag}={}(?:\s|$)", escape(value))).found(&command);
        if mentions != 1 || !exact {
            return die(format!("客户端启动参数无法核验：--{flag}；请退出该实例后重新打开"));
        }
    }
    let app = path::realpath(app);
    let Some((info_path, binary)) = app_binary(&app)? else { return die("无法核验客户端程序版本") };
    if !command.starts_with(&format!("{binary} ")) {
        return die("客户端进程不属于当前选择的程序，请退出旧实例后重开");
    }
    let (Ok(st), Ok(binary_st)) = (std::fs::metadata(&info_path), std::fs::metadata(&binary)) else { return die("无法核验客户端程序版本") };
    let mtime_ns = |m: &std::fs::Metadata| m.mtime() * 1_000_000_000 + m.mtime_nsec();
    let version = crate::list![st.size() as i64, mtime_ns(&st), binary_st.size() as i64, mtime_ns(&binary_st)];
    Ok(dict! {
        "v" => 1, "tool" => t().name, "id" => acct_id(key, t()), "kind" => kind, "pid" => pid, "started" => started,
        "profile" => expected, "config" => path::realpath(&cdir(key)), "app" => app, "app_version" => version, "proxy" => proxy,
        "route" => route, "node" => node_of(&table.at(key)), "bypass" => bypass, "no_proxy" => no_proxy, "launch_policy" => 1,
    })
}

pub fn cmd_runtime_record(args: &[String]) -> R<()> {
    arity("cmd_runtime_record", args, 7, 10)?;
    let (action, key, kind) = (args[0].as_str(), args[1].as_str(), args[2].as_str());
    if !matches!(action, "write" | "check") || !matches!(kind, "app" | "browser") {
        return die("未知客户端运行记录操作");
    }
    let _lock = Lock::acquire()?;
    let current = V::Dict(runtime_snapshot(key, kind, &args[3], &args[4], &args[5], &args[6], arg(args, 7, ""), arg(args, 8, ""), arg(args, 9, ""))?);
    let p = path::join(&adir(key), &format!("{kind}-runtime.json"));
    if action == "write" {
        save_json(&p, &current, Some(0o600), false)?;
    } else if !private_file(&p)? || !load_quiet(&p).is_some_and(|saved| saved.py_eq(&current)) {
        return die("客户端的运行记录、账号节点或启动条件已变化；请先退出该实例，再用 cca 重新打开");
    }
    Ok(())
}

// 命令行会话的登记（_gui state 的 cli）：run_account 启动 claude / codex 之前在账号目录写 cli-<cca 的进程号>.json，
// 工具一退出就删（bash 的 session_close）。记 cca 的进程号和它的启动时间：进程号会被重用，启动时间对得上才是同一个进程

/// 账号目录里登记的文件名 `cli-<进程号>.json` 里的进程号
fn cli_mark_pid(name: &str) -> Option<i64> {
    text::to_int(re!(r"^cli-([0-9]+)\.json$").match_(name)?.g(1))
}

/// 账号目录里登记的命令行会话（只读）；格式、工具、账号目录名、进程号和文件名对不上的不算
pub fn cli_marks(key: &str) -> Vec<Dict> {
    let (d, aid) = (adir(key), acct_id(key, t()));
    let Ok(names) = fs::listdir_sorted(&d) else { return Vec::new() };
    let mut out = Vec::new();
    for n in names {
        let Some(pid) = cli_mark_pid(&n) else { continue };
        let Some(j) = load_quiet(&path::join(&d, &n)).and_then(|j| j.as_dict()) else { continue };
        let eq = |k: &str, want: V| j.get(k).is_some_and(|v| v.py_eq(&want));
        if eq("v", V::Int(1)) && eq("tool", V::from(t().name)) && eq("id", V::from(aid.as_str())) && eq("pid", V::Int(pid)) && j.get("started").is_some_and(|s| s.is_int()) && j.get("since").is_some_and(|s| s.isinstance_num()) {
            out.push(j);
        }
    }
    out
}

/// lstart 只到秒
pub fn cli_alive(pid: i64, started: i64, starts: &HashMap<i64, i64>) -> bool {
    starts.get(&pid).is_some_and(|s| (s - started).abs() <= 1)
}

/// _gui state 里账号的 cli：还在运行的会话有几个、最早那个从什么时候开始
pub fn cli_view(marks: &[Dict], starts: &HashMap<i64, i64>) -> Dict {
    let int = |m: &Dict, k: &str| m.at(k).as_int().unwrap_or(0);
    let live: Vec<&Dict> = marks.iter().filter(|m| cli_alive(int(m, "pid"), int(m, "started"), starts)).collect();
    // min(…)：一样大时取先出现的
    let since = live.iter().map(|m| m.at("since")).reduce(|best, s| if s.as_num() < best.as_num() { s } else { best });
    dict! { "running" => !live.is_empty(), "sessions" => live.len(), "since_epoch" => since.unwrap_or(V::None) }
}

/// run_account 启动工具之前登记这次命令行会话：图形界面看到账号目录里的 cli-<进程号>.json 就知道这个账号在工作。
/// 顺手清掉这个账号对不上的旧登记（cca 被 kill -9、断电留下的；_gui state 只读，不删）。问不到 cca 自己的启动时间就
/// 不登记：宁可不显示，也不留核对不了的登记
pub fn cmd_session_open(args: &[String]) -> R<()> {
    arity("cmd_session_open", args, 2, 2)?;
    let (key, pid) = (args[0].as_str(), int_arg(&args[1])?);
    let d = adir(key);
    let Ok(names) = fs::listdir_sorted(&d) else { return Ok(()) };
    // 进程号 → 文件（cli-7.json 和 cli-007.json 是同一个进程号：后一个算数）
    let mut old: Vec<(i64, String)> = Vec::new();
    for n in names {
        if let Some(p) = cli_mark_pid(&n) {
            let file = path::join(&d, &n);
            match old.iter_mut().find(|(x, _)| *x == p) {
                Some(slot) => slot.1 = file,
                None => old.push((p, file)),
            }
        }
    }
    let mut pids: Vec<i64> = old.iter().map(|(p, _)| *p).collect();
    pids.push(pid);
    let starts = proc_starts(&pids);
    let Some(started) = starts.get(&pid).copied() else { return Ok(()) };
    for (p, file) in &old {
        let mark = load_quiet(file).and_then(|j| j.as_dict()).and_then(|j| j.get("started")).filter(|s| s.is_int()).and_then(|s| s.as_int());
        if *p == pid || !mark.is_some_and(|s| cli_alive(*p, s, &starts)) {
            let _ = std::fs::remove_file(file);
        }
    }
    let mark = dict! { "v" => 1, "tool" => t().name, "id" => acct_id(key, t()), "pid" => pid, "started" => started, "since" => time() };
    save_json(&path::join(&d, &format!("cli-{pid}.json")), &V::Dict(mark), Some(0o600), true)?;
    presence_wake(key); // 映射里的账号：报告给维护者的服务器（上报进程在后台，不拖慢启动）
    Ok(())
}

/// 在运行的进程 [(pid, 命令行)]：看各账号的桌面 App 开着没有（一次 ps，不用每个账号 pgrep 一次）
pub fn gui_procs() -> Vec<(i64, String)> {
    ps_rows(&["-axww", "-o", "pid=,command="], 10.0, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lstart_reads_c_locale_dates() {
        let t = lstart_epoch("Sat Nov  7 12:08:55 2026");
        assert!(t > 1_790_000_000 && t < 1_800_000_000, "{t}");
        assert_eq!(lstart_epoch("  Sat   Nov 7 12:08:55 2026  "), t);
        assert_eq!(lstart_epoch("六 11  7 12:08:55 2026"), 0);
        assert_eq!(lstart_epoch(""), 0);
    }

    #[test]
    fn proc_scan_reads_a_proc_tree() {
        let root = std::env::temp_dir().join(format!("cca-proc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |rel: &str, data: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        };
        put("stat", b"cpu  1 2 3\nbtime 1700000000\nprocesses 5\n");
        // 进程名里有空格和括号；tty_nr 34819 = pts/3（主设备号 136，次设备号 3）
        put("42/stat", b"42 (my (odd) name) S 7 42 42 34819 42 4194304 0 0 0 0 150 50 0 0 20 0 1 0 12345 100 200\n");
        put("42/cmdline", b"/usr/bin/thing\0--flag\0value\0");
        put("43/stat", b"43 (sh) S 1 43 43 1025 43 0 0 0 0 0 7 3 0 0 20 0 1 0 250 1 1\n");
        put("43/cmdline", b"");
        put("44/stat", b"44 (kthread) S 2 0 0 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5 0 0\n");
        put("44/cmdline", b"");
        put("45/stat", b"45 (short) S 1\n");
        put("45/cmdline", b"x");
        put("self/stat", b"ignored");
        let root = root.to_str().unwrap();
        let got = proc_scan_at(root, Some(&[42, 43, 44, 45, 99]), 100.0).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (42, ProcInfo { ppid: 7, tty: "pts/3".into(), cpu: 2.0, start: 1700000123, cmd: "/usr/bin/thing --flag value".into() }));
        assert_eq!(got[1], (43, ProcInfo { ppid: 1, tty: "tty1".into(), cpu: 0.1, start: 1700000002, cmd: String::new() }));
        assert_eq!(got[2].1.tty, "?");
        let mut all: Vec<i64> = proc_scan_at(root, None, 100.0).unwrap().into_iter().map(|(p, _)| p).collect();
        all.sort_unstable();
        assert_eq!(all, [42, 43, 44]);
        std::fs::write(format!("{root}/stat"), b"cpu 1\n").unwrap();
        assert!(proc_scan_at(root, Some(&[42]), 100.0).is_none());
        std::fs::write(format!("{root}/stat"), b"btime soon\n").unwrap();
        assert!(proc_scan_at(root, Some(&[42]), 100.0).is_none());
        std::fs::remove_dir_all(root).unwrap();
        assert!(proc_scan_at(root, Some(&[42]), 100.0).is_none());
    }
}
