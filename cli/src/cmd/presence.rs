//! 设备统计（`presence_problems` 到 `cmd_presence_stop`、`past_time` 到 `cmd_presence_note`）：presence-agent、presence-wake、
//! presence-stop、presence-view、presence-note。
//!
//! 都移植了；别处要用的：`presence_problems`、`presence_load`、`presence_post`、`presence_targets`、`app_proc_re`、`spawn_background`。

use super::cost::cost_kick;
use super::keyshare::{is_num, keyshare_error, keyshare_kick, keyshare_sync, KEYSHARE_EVERY};
use super::session::{cli_alive, cli_marks_at, lstart_epoch, proc_scan, proc_starts, ps_lstart, ps_rows, ProcInfo};
use super::table::account_named;
use super::usage::{ago, day_gap, span, weekday_clock};
use super::{arity, Cmd};
use crate::core::base::{CCA_HOME, CFG, SELF_PATH};
use crate::core::ctx::ctx;
use crate::core::model::{acct_id, adir, is_api, label_of, name_of, parse_key};
use crate::core::policy::{policy_doc, policy_on, policy_raw};
use crate::core::tool::{t, tools, Tool, Using};
use crate::py::base64::{b64decode, b64encode};
use crate::py::fs::{self, load_quiet, read_text, save_json};
use crate::py::hash::sha256;
use crate::py::obj::{repr_str, Dict, List, V};
use crate::py::proc::{env_set, environ, execv, Cmd as Proc, In, Out};
use crate::py::rand::{token_hex, urandom};
use crate::py::re::{self, Re};
use crate::py::time::{localtime, monotonic, sleep, time};
use crate::py::{exit, json, num, path, shutil, signal, text};
use crate::py::{Fail, R};
use crate::{dict, list, out};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("presence-agent", cmd_presence_agent),
    Cmd::all("presence-wake", cmd_presence_wake),
    Cmd::all("presence-stop", cmd_presence_stop),
    Cmd::all("presence-note", cmd_presence_note),
    Cmd::all("presence-view", cmd_presence_view),
];

pub static PRESENCE_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "presence"));

pub static PRESENCE_URL_RE: LazyLock<Re> = LazyLock::new(|| {
    Re::new(concat!(
        r"^https://(?P<host>[0-9]{1,3}(\.[0-9]{1,3}){3}|\[[0-9A-Fa-f:]{2,39}\]|[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?)",
        r":(?P<port>[0-9]{1,5})/?$"
    ))
});
pub static PRESENCE_PIN_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^sha256//[A-Za-z0-9+/]{43}=$"));
pub static PRESENCE_SALT_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^([0-9a-f]{2}){16,64}$"));

/// 映射里 presence 一节的问题：{"url": "https://地址:端口", "pins": ["sha256//…"], "salt": "十六进制"}。维护者发布前
/// 严格检查；客户端只在完全合格时启用（不合格就当没有，映射的其余部分照用）
pub fn presence_problems(p: &V) -> Vec<String> {
    let Some(p) = p.as_dict() else {
        return vec![
            "presence 应该是 {\"url\": \"https://地址:端口\", \"pins\": [\"sha256//…\"], \"salt\": \"…\"} 形式的对象".to_string()
        ];
    };
    let mut errs = Vec::new();
    let url = p.get("url");
    let port = url
        .as_ref()
        .and_then(|u| u.as_str())
        .and_then(|u| PRESENCE_URL_RE.match_(u))
        .and_then(|m| m.name("port"))
        .and_then(text::to_int);
    if !port.is_some_and(|n| (1..=65535).contains(&n)) {
        errs.push("presence.url 应该是 https://地址:端口（例如 https://203.0.113.10:18443）".to_string());
    }
    let pins_ok = p.list("pins").is_some_and(|pins| {
        (1..=4).contains(&pins.len()) && pins.items().iter().all(|x| x.as_str().is_some_and(|x| PRESENCE_PIN_RE.matches(x)))
    });
    if !pins_ok {
        errs.push("presence.pins 应该是 1～4 个服务器证书公钥的指纹（sha256//…，./policy.sh presence deploy 打印的）".to_string());
    }
    let salt = p.get("salt");
    if !salt.as_ref().and_then(|s| s.as_str()).is_some_and(|s| PRESENCE_SALT_RE.matches(s)) {
        errs.push("presence.salt 应该是 32～128 位（偶数位）小写十六进制".to_string());
    }
    errs
}

/// 公钥（base64 那一段）的指纹，和 ssh-keygen -l 的写法一样（SHA256:…）；不是 base64 是 None
pub fn ssh_fp(b64: &str) -> Option<String> {
    let raw = b64decode(b64, true).ok()?;
    Some(format!("SHA256:{}", b64encode(&sha256(&raw)).trim_end_matches('=')))
}

/// 这台 Mac 拉映射用的 deploy key：(私钥路径, 指纹)；没有、不是 ed25519 是 None。公钥文件不是 UTF-8 是 ValueError（调用方接）
pub fn presence_key() -> R<Option<(String, String)>> {
    let key = path::join_all(&[&CCA_HOME, "policy", "id_ed25519"]);
    let content = match read_text(&format!("{key}.pub")) {
        Ok(content) => content,
        Err(e) if e.is_os() => return Ok(None),
        Err(e) => return Err(e),
    };
    let fields = text::split_ws(&content);
    let fp = if fields.len() >= 2 && fields[0] == "ssh-ed25519" && path::isfile(&key) { ssh_fp(fields[1]) } else { None };
    Ok(fp.map(|fp| (key, fp)))
}

/// (用不了的原因, 配置)：能用时原因是空的，配置是 {url, pins, salt, key, fp}
pub fn presence_load() -> R<(String, Option<Dict>)> {
    let off = load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("presence"), Some(V::Bool(false))));
    if off {
        return Ok(("这台 Mac 关掉了设备统计（config.json 里写着 \"presence\": false）".into(), None));
    }
    presence_server()
}

/// 维护者的服务器的配置，不看这台 Mac 关没关设备统计：远程会话的中转（docs/dev/remote.md）也经这台服务器，
/// 那是用户自己配对了才用的，和上报不上报是两回事
pub fn presence_server() -> R<(String, Option<Dict>)> {
    let Some(j) = policy_doc() else {
        let why = if policy_on() {
            "这台 Mac 还没拉到维护者的映射（cca policy pull）"
        } else {
            "这台 Mac 没打开维护者的共享（cca policy on）"
        };
        return Ok((why.into(), None));
    };
    let Some(p) = j.get("presence").filter(|p| !p.is_none()) else {
        return Ok(("维护者的映射里还没有设备统计的服务器".into(), None));
    };
    let errs = presence_problems(&p);
    if !errs.is_empty() {
        return Ok((format!("维护者的映射里设备统计那一节不对：{}", errs.join("；")), None));
    }
    let Some((key, fp)) = presence_key()? else {
        return Ok(("这台 Mac 没有映射的 deploy key（cca policy on）".into(), None));
    };
    let p = p.as_dict().expect("presence_problems 核对过是字典");
    let cfg = dict! { "url" => p.at("url").py_str().trim_end_matches('/'), "pins" => p.at("pins"), "salt" => p.at("salt"), "key" => key, "fp" => fp };
    Ok((String::new(), Some(cfg)))
}

/// 上报用的账号摘要：服务器只看到它，看不到邮箱（盐在签名的映射里，各台 Mac 算出来的一样）
pub fn presence_aid(salt: &str, tool: &str, email: &str) -> R<String> {
    let compact: String = salt.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let key = crate::py::hash::from_hex(&compact)
        .ok_or_else(|| crate::py::Fail::Value("non-hexadecimal number found in fromhex() arg".into()))?;
    let digest = crate::py::hash::hmac_sha256(&key, format!("{}\0{}", tool, email.to_lowercase()).as_bytes());
    Ok(crate::py::hash::to_hex(&digest)[..32].to_string())
}

/// 这个账号是映射里的官方账号吗（pm 是这个工具的 policy_raw）：只有它们上报、看得到设备
pub fn presence_mapped(pm: &Dict, key: &str, v: &V) -> bool {
    let e = parse_key(key).1.to_lowercase();
    !e.is_empty() && !is_api(v) && pm.has(&e)
}

/// 后台进程不带的：App 的提问会话、事件这些
pub const BG_ENV_DROP: [&str; 7] = ["CCA_EVENTS", "CCA_GRANT", "CCA_RO", "CCA_PROMPT", "CCA_TTY", "CCA_YES", "CCA_TOOL"];

/// 在后台起 cca 的一个子命令（上报进程、同步 API key、统计折合金额）：后台优先级（只用能效核，定时器和别的唤醒合在一起）、
/// 自己一个会话、不接终端、不带 BG_ENV_DROP。起不了就算了（静默）
pub fn spawn_background(args: &[&str]) {
    const TASKPOLICY: &str = "/usr/sbin/taskpolicy";
    // SAFETY: 路径是以 0 结尾的常量
    let has_taskpolicy = unsafe { libc::access(c"/usr/sbin/taskpolicy".as_ptr(), libc::X_OK) == 0 };
    let mut argv: Vec<&str> = if has_taskpolicy { vec![TASKPOLICY, "-c", "background"] } else { Vec::new() };
    argv.push(&SELF_PATH);
    argv.extend(args);
    let env: Vec<(String, String)> = environ().into_iter().filter(|(k, _)| !BG_ENV_DROP.contains(&k.as_str())).collect();
    if let Ok(child) = Proc::new(argv).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).env(env).new_session().spawn() {
        crate::py::proc::forget(child); // 不等它；它退出以后要有人收（常驻的上报进程隔几分钟就起一次统计）
    }
}

/// key（当前工具的）是映射里的官方账号、正在用：确保这台 Mac 的上报进程在跑（在跑的会经 kqueue 自己看到新的登记），
/// 不在就在后台起一个。静默：出什么错都不影响启动
pub fn presence_wake(key: &str) {
    let wake = || -> R<()> {
        if !presence_mapped(&policy_raw(t()), key, &V::None) || presence_load()?.1.is_none() {
            return Ok(());
        }
        fs::makedirs_mode(&PRESENCE_DIR, 0o700)?;
        {
            use std::os::unix::io::AsRawFd;
            let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "agent.lock"))?;
            // SAFETY: lock 开着，fd 有效
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Ok(()); // 在跑
            }
            // SAFETY: 同上
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        }
        spawn_background(&["_presence-agent"]);
        Ok(())
    };
    match wake() {
        Err(e) if !e.is_os_or_value() => panic!("presence_wake：{e}"),
        _ => {}
    }
}

pub fn cmd_presence_wake(args: &[String]) -> R<()> {
    arity("cmd_presence_wake", args, 1, 1)?;
    presence_wake(&args[0]);
    Ok(())
}

/// cca policy 里的一句：这台 Mac 会不会把映射里账号的使用情况报告给维护者的服务器
pub fn cmd_presence_note(args: &[String]) -> R<()> {
    arity("cmd_presence_note", args, 0, 0)?;
    if !policy_doc().is_some_and(|j| j.has("presence")) {
        return Ok(());
    }
    match presence_load()? {
        (_, Some(cfg)) => crate::out!(
            "设备统计：映射里的账号在这台 Mac 上用的时候（命令行会话、cca app 打开的 App），会报告给维护者的服务器 {}，配置了同一份映射的设备在账号页能看到这台 Mac 在不在用；给这些账号记下备查的 API key（cca key 账号）也经它加密同步给登录了同一个账号的其他设备，服务器看不到 key（都不想要：config.json 里加上 \"presence\": false）",
            cfg.at("url").py_str()
        ),
        (reason, None) => crate::out!("设备统计：{reason}"),
    }
    Ok(())
}

/// cca restore 之前：停掉这台 Mac 的上报进程（它先告诉服务器“停了”）
pub fn cmd_presence_stop(args: &[String]) -> R<()> {
    arity("cmd_presence_stop", args, 0, 0)?;
    let Some(j) = load_quiet(&path::join(&PRESENCE_DIR, "agent.json")).and_then(|j| j.as_dict()) else { return Ok(()) };
    // isinstance(pid, int)、type(started) is int、started 不是 0
    let Some(V::Int(started)) = j.get("started").filter(|s| s.truthy()) else { return Ok(()) };
    let pid = match j.get("pid") {
        Some(V::Int(pid)) => pid,
        Some(V::Bool(true)) => {
            // 布尔值在 Python 里也算 int：True 当成进程号 1 去问（ps 的参数写成 True，问不到），等于不在
            if !cfg!(target_os = "linux") {
                ps_lstart("True");
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let alive = || cli_alive(pid, started, &proc_starts(&[pid]));
    if !alive() {
        return Ok(());
    }
    // SAFETY: 只是给一个进程号发信号
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        return Ok(());
    }
    for _ in 0..50 {
        if !alive() {
            return Ok(());
        }
        sleep(0.1);
    }
    Ok(())
}

// --- 上报进程（cca _presence-agent） ---------------------------------------------------------------------------------------
// 每台 Mac 一个（后台优先级）：有映射里账号的会话时才在，先报一次全部开着的会话（快照），之后 60 秒一次心跳，其余时间睡在
// kqueue 里：会话进程退出、账号目录里的登记变了马上醒来再报；最后一个关掉时报一次「什么都没开了」就退出。报不出去时按退避
// 再试（5 秒起翻倍，最多 2 分钟；服务器不认这台 Mac 时 10 分钟）。动静：命令行会话看它终端的最后输入、输出时间（和 w 算
// 闲置一样），桌面 App 看进程树的 CPU；10 分钟没动静算闲置（服务器算）。请求用 deploy key 签名（命名空间 cca-presence），
// 直连、钉住服务器证书的公钥；账号只报 HMAC(盐, 工具\0邮箱)，服务器看不到邮箱。

/// 心跳间隔（秒）：服务器 150 秒收不到、又开着会话就算离线
pub const PRESENCE_BEAT: f64 = 60.0;
/// 这么久没动静算闲置（服务器按它自己的设置算；这里只用于第一次见到的桌面 App）
pub const PRESENCE_IDLE: f64 = 600.0;
/// 桌面 App 的进程树在一拍里平均用掉一个核的这么多算有动静（2026-11 实测 Claude App：闲着约 1%，60 秒平均最高 1.3%；
/// 打开、写回复时 18%～100%）
pub const PRESENCE_APP_CPU: f64 = 0.03;
/// macOS 的 ttys003；Linux 的 pts/3（ssh、终端窗口）、tty1（控制台）
pub static TTY_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^(ttys[0-9]+|pts/[0-9]+|tty[0-9]+)$"));

/// 上报的对象：(工具, 账号表的键, 账号目录)
pub type Target = (&'static Tool, String, String);

/// 这台 Mac 上映射里的官方账号。账号表读不了是 None（这一轮不报：不能当成什么都没开）
pub fn presence_targets() -> Option<Vec<Target>> {
    let mut out = Vec::new();
    for tool in tools() {
        if !path::exists(&tool.table) {
            continue;
        }
        let table = load_quiet(&tool.table)?.as_dict()?;
        let pm = policy_raw(tool);
        let _using = Using::tool(tool);
        for (k, v) in table.items() {
            if presence_mapped(&pm, &k, &v) {
                out.push((tool, k.to_string(), adir(&k)));
            }
        }
    }
    Some(out)
}

/// ps 的 time（[天-][时:]分:秒.百分秒）→ 秒
pub fn cpu_secs(s: &str) -> f64 {
    let (d, _, hms) = text::rpartition(s, "-");
    let mut secs = 0.0;
    for x in hms.split(':') {
        match text::to_float(x) {
            Some(f) => secs = secs * 60.0 + f,
            None => return 0.0,
        }
    }
    if d.is_empty() {
        return secs;
    }
    match text::to_int(d) {
        Some(days) => secs + (days as i128 * 86400) as f64,
        None => 0.0,
    }
}

/// 全部进程（原来是按进程号查的字典，先后照 ps 列出来的顺序：算进程树的 CPU 时照这个顺序加）
#[derive(Default)]
pub struct Procs {
    rows: Vec<(i64, ProcInfo)>,
    at: HashMap<i64, usize>,
}

impl Procs {
    pub fn from_rows(rows: impl IntoIterator<Item = (i64, ProcInfo)>) -> Procs {
        let mut procs = Procs::default();
        for (pid, info) in rows {
            match procs.at.get(&pid) {
                Some(i) => procs.rows[*i].1 = info, // 同一个进程号出现两次：后来的顶掉，位置不变
                None => {
                    procs.at.insert(pid, procs.rows.len());
                    procs.rows.push((pid, info));
                }
            }
        }
        procs
    }

    pub fn get(&self, pid: i64) -> Option<&ProcInfo> {
        self.at.get(&pid).map(|i| &self.rows[*i].1)
    }

    pub fn rows(&self) -> &[(i64, ProcInfo)] {
        &self.rows
    }
}

/// 全部进程 {pid: (ppid, 终端, CPU 秒数, 启动时间戳, 命令行)}（一次 ps；Linux 上读 /proc）；读不了是 None
pub fn presence_procs() -> Option<Procs> {
    if let Some(lx) = proc_scan(None) {
        return if lx.is_empty() { None } else { Some(Procs::from_rows(lx)) };
    }
    let mut env = environ();
    env_set(&mut env, "LC_ALL", "C");
    env_set(&mut env, "LANG", "C");
    let rows = ps_rows(&["-axww", "-o", "pid=,ppid=,tty=,time=,lstart=,command="], 10.0, Some(env));
    if rows.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(rows.len());
    for (pid, rest) in rows {
        let f = text::split_ws_n(&rest, 8);
        if f.len() >= 8 && text::isdigit(f[0]) {
            let Some(ppid) = text::to_int(f[0]) else { continue };
            let info = ProcInfo {
                ppid,
                tty: f[1].to_string(),
                cpu: cpu_secs(f[2]),
                start: lstart_epoch(&f[3..8].join(" ")),
                cmd: f.get(8).map_or(String::new(), |s| s.to_string()),
            };
            out.push((pid, info));
        }
    }
    Some(Procs::from_rows(out))
}

/// 这个进程和它的子孙一共用了多少 CPU 秒
fn tree_cpu(pid: i64, procs: &Procs, kids: &HashMap<i64, Vec<i64>>) -> f64 {
    let (mut total, mut stack, mut seen) = (0.0, vec![pid], HashSet::new());
    while let Some(p) = stack.pop() {
        if seen.insert(p) {
            total += procs.get(p).map_or(0.0, |x| x.cpu);
            if let Some(k) = kids.get(&p) {
                stack.extend(k);
            }
        }
    }
    total
}

/// 这个资料目录的桌面 App 主进程的命令行（_gui state 看 App 开着没有、上报都用它）：--user-data-dir= 后面可能是
/// 账号目录里的写法，也可能是真实路径（cca app 传的是 realpath，~/.cca 经过链接时两者不同）。没有桌面 App 的工具是 None
pub fn app_proc_re(tool: &Tool, desk: &str) -> Option<std::rc::Rc<Re>> {
    let proc_ = tool.app_proc?;
    Some(re::compile(&format!("{}.*--user-data-dir=({}|{})/?( |$)", proc_, re::escape(desk), re::escape(&path::realpath(desk)))))
}

/// 桌面 App 的 CPU 采样：{(pid, 启动时间): [CPU 秒数, 采样时刻, 最后动静]}
pub type AppSamples = HashMap<(i64, i64), [f64; 3]>;

/// 登记里的整数（cli_marks 核对过：pid 和文件名里的一样，started 是整数）
fn mark_int(m: &Dict, k: &str) -> i64 {
    match m.at(k) {
        V::Int(i) => i,
        V::Bool(b) => b as i64,
        other => other.as_num().map_or(0, |f| f as i64),
    }
}

/// 这一轮开着的会话：({账号摘要: {"cli": [[开了多久, 闲置多久]…], "app": [开了多久, 闲置多久] | None}}, 要盯着退出的进程号)。
/// apps 是桌面 App 的 CPU 采样（这里更新）
pub fn presence_scan(
    salt: &str,
    targets: &[Target],
    procs: &Procs,
    apps: &mut AppSamples,
    now: f64,
    mono: f64,
) -> R<(Dict, Vec<i64>)> {
    presence_scan_with(salt, targets, procs, apps, now, mono, &super::remote::hosted_activity)
}

/// `presence_scan`，宿主里的会话最后一次有动静的时间由调用方给（宿主的进程号 → 时间；用到才取，一轮最多取一次）
pub fn presence_scan_with(
    salt: &str,
    targets: &[Target],
    procs: &Procs,
    apps: &mut AppSamples,
    now: f64,
    mono: f64,
    hosted: &dyn Fn() -> HashMap<i64, f64>,
) -> R<(Dict, Vec<i64>)> {
    let units = Dict::new();
    let mut hosts: Option<HashMap<i64, f64>> = None;
    let mut pids: Vec<i64> = Vec::new();
    let mut live: HashSet<(i64, i64)> = HashSet::new();
    let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
    let starts: HashMap<i64, i64> = procs.rows().iter().map(|(pid, p)| (*pid, p.start)).collect();
    for (pid, p) in procs.rows() {
        kids.entry(p.ppid).or_default().push(*pid);
    }
    let mut sorted_pids: Option<Vec<i64>> = None;
    let mut watch = |pid: i64| {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    };
    for (tool, key, d) in targets {
        let marks = {
            let _using = Using::tool(tool);
            cli_marks_at(d, key) // d 就是这个工具下的 adir(key)（presence_targets 给的）
        };
        let cli = List::new();
        for m in &marks {
            let pid = mark_int(m, "pid");
            if !cli_alive(pid, mark_int(m, "started"), &starts) {
                continue; // 进程没了，或者进程号被别的进程重用了
            }
            watch(pid);
            let since = m.at("since").as_pynum().unwrap_or(0.0);
            // 最后一次有动静：终端的最后一次输入、输出；会话在宿主里的（远程会话：启动它的终端没了也照样运行，别的机器上的输入
            // 也不经过那个终端）再看宿主记的那一次。宿主是 cca 的子进程
            let mut seen: Option<f64> = None;
            let tty = &procs.get(pid).expect("cli_alive 说它在 procs 里").tty;
            if TTY_RE.matches(tty) {
                if let Ok(st) = std::fs::metadata(format!("/dev/{tty}")) {
                    seen = Some(shutil::stat_time(st.atime(), st.atime_nsec()).max(shutil::st_mtime(&st)));
                }
            }
            if let Some(children) = kids.get(&pid) {
                let hosts = hosts.get_or_insert_with(hosted);
                if let Some(at) = children.iter().filter_map(|c| hosts.get(c)).copied().reduce(f64::max) {
                    seen = Some(seen.map_or(at, |s| s.max(at)));
                }
            }
            // 都没有（脚本里的 claude -p 之类，没有终端）：在跑就是在干活
            let last = seen.map_or(now, |s| since.max(s));
            cli.push(list![num::round((now - since).max(0.0)), num::round((now - last).max(0.0))]);
        }
        let mut app = V::None;
        // agy 没有桌面 App
        let apid = app_proc_re(tool, &path::join(d, "desktop")).and_then(|pat| {
            let sorted = sorted_pids.get_or_insert_with(|| {
                let mut all: Vec<i64> = procs.rows().iter().map(|(pid, _)| *pid).collect();
                all.sort_unstable();
                all
            });
            sorted.iter().copied().find(|pid| procs.get(*pid).is_some_and(|p| pat.found(&p.cmd)))
        });
        if let Some(apid) = apid.filter(|pid| *pid != 0) {
            watch(apid);
            let start = procs.get(apid).expect("刚从 procs 里找出来的").start;
            let (cpu, k) = (tree_cpu(apid, procs, &kids), (apid, start));
            live.insert(k);
            let rec = match apps.get_mut(&k) {
                // 第一次见到：刚打开的算在用，开了很久的先当作刚好闲置满 10 分钟，下一拍看 CPU
                None => *apps.entry(k).or_insert([
                    cpu,
                    mono,
                    if now - (start as f64) < PRESENCE_IDLE { start as f64 } else { now - PRESENCE_IDLE },
                ]),
                Some(rec) => {
                    if mono - rec[1] >= 30.0 {
                        // 隔得太近的两次采样不准：至少隔 30 秒
                        if (cpu - rec[0]) / (mono - rec[1]) >= PRESENCE_APP_CPU {
                            rec[2] = now;
                        }
                        rec[0] = cpu;
                        rec[1] = mono;
                    }
                    *rec
                }
            };
            app = V::from(list![num::round((now - start as f64).max(0.0)), num::round((now - rec[2]).max(0.0))]);
        }
        if !cli.is_empty() || !app.is_none() {
            units.set(presence_aid(salt, tool.name, &parse_key(key).1)?, dict! { "cli" => cli, "app" => app });
        }
    }
    apps.retain(|k, _| live.contains(k));
    Ok((units, pids))
}

/// 经 HTTPS 发一个签过名的请求：直连（不经账号的出口），钉住服务器证书的公钥。返回 (HTTP 状态码, JSON)；
/// 没发出去是 (0, {"error": "sign" | "unreachable" | "pin"})。timeout 是秒
pub fn presence_post(cfg: &Dict, op: &str, payload: &Dict, timeout: i64) -> R<(i64, Dict)> {
    let doc = payload.copy();
    doc.set("v", 1);
    doc.set("op", op);
    doc.set("key", cfg.at("fp"));
    doc.set("ts", time().trunc() as i64);
    doc.set("nonce", token_hex(16));
    let body = json::dumps(&V::Dict(doc), json::Fmt::new().utf8().compact());
    let key = cfg.at("key").py_str();
    let sign = Proc::new(["ssh-keygen", "-Y", "sign", "-q", "-n", "cca-presence", "-f", key.as_str()])
        .input(body.as_bytes())
        .stdout(Out::Capture)
        .stderr(Out::Null)
        .timeout(10.0);
    let sig = sign.run().map(|done| done.stdout).unwrap_or_default();
    signal::check()?;
    if sig.is_empty() {
        return Ok((0, dict! { "error" => "sign" }));
    }
    let pins: Vec<String> = cfg.list("pins").map(|p| p.items().iter().map(|x| x.py_str()).collect()).unwrap_or_default();
    let args: Vec<String> = [
        "curl",
        "-q",
        "-sS",
        "-k",
        "--proxy",
        "",
        "--noproxy",
        "*",
        "--proto",
        "=https",
        "--max-redirs",
        "0",
        "--connect-timeout",
        "5",
        "-m",
        &timeout.to_string(),
        "--pinnedpubkey",
        &pins.join(";"),
        "-H",
        "Content-Type: application/json",
        "-H",
        &format!("X-Cca-Sig: {}", b64encode(&sig)),
        "-H",
        "Expect:", // 不等 100 Continue（服务器是 HTTP/1.0 的，大一点的请求 curl 会白等一秒）
        "--data-binary",
        "@-",
        "-w",
        "\n%{http_code}",
        &format!("{}/v1/{}", cfg.at("url").py_str(), op),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let sent = Proc::new(args).input(body.as_bytes()).stdout(Out::Capture).stderr(Out::Null).timeout((timeout + 5) as f64).run();
    signal::check()?;
    let Ok(r) = sent else {
        return Ok((0, dict! { "error" => "unreachable" }));
    };
    if r.rc == 90 {
        // CURLE_SSL_PINNEDPUBKEYNOTMATCH：证书不是映射里钉住的那张
        return Ok((0, dict! { "error" => "pin" }));
    }
    let text_ = r.out();
    let (out, _, code) = text::rpartition(&text_, "\n");
    if r.rc != 0 || !text::isdigit(code) || code == "000" {
        return Ok((0, dict! { "error" => "unreachable" }));
    }
    let j = json::loads(out).ok().and_then(|j| j.as_dict()).unwrap_or_default();
    let code =
        text::to_int(code).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(code))))?;
    Ok((code, j))
}

/// 请求没成的原因（给人看的）
pub fn presence_why(cfg: &Dict, code: i64, j: &Dict) -> R<String> {
    let err = j.get("error");
    let err = err.as_ref().and_then(|e| e.as_str());
    Ok(if err == Some("pin") {
        "服务器的证书和映射里钉住的指纹对不上（可能有人冒充服务器，也可能维护者换了证书、映射还没更新）".into()
    } else if err == Some("sign") {
        "没法用这台 Mac 的 deploy key 签名（~/.cca/policy/id_ed25519）".into()
    } else if code == 0 {
        format!("连不上设备统计的服务器（{}）", cfg.at("url").py_str())
    } else if code == 403 {
        "服务器还不认这台 Mac：维护者要把它的 deploy key 同步到服务器的设备清单（./policy.sh presence sync）".into()
    } else if let Some(now) = j.get("now").filter(|n| err == Some("clock") && is_num(n)).and_then(|n| n.as_num()) {
        // '%d' % abs(…)：朝 0 取整；不是有限的数原来是没人接的 OverflowError / ValueError
        let gap = (time() - now).abs();
        if !gap.is_finite() {
            return Err(Fail::Value("cannot convert float NaN or infinity to integer".into()));
        }
        format!("这台 Mac 的时间和服务器差了 {:.0} 秒：打开“系统设置 › 通用 › 日期与时间”里的自动设置", gap.trunc())
    } else if code == 429 {
        "请求太频繁，稍后再试".into()
    } else {
        format!("设备统计的服务器出错了（HTTP {code}）")
    })
}

/// 上报进程最近一次没报成的原因（cca devices 显示）；报成了是空的。只在变了的时候写
pub fn presence_status(text_: &str) -> R<()> {
    let p = path::join(&PRESENCE_DIR, "status.json");
    let was = match load_quiet(&p).filter(|j| j.truthy()) {
        None => V::from(""),
        Some(V::Dict(j)) => j.get("error").unwrap_or_else(|| V::from("")),
        // 原来是没人接的 AttributeError（status.json 里不是对象）
        Some(other) => panic!("AttributeError: status.json 里是 {}，不是对象", other.py_repr()),
    };
    if !was.py_eq(&V::from(text_)) && path::isdir(&PRESENCE_DIR) {
        match save_json(&p, &V::Dict(dict! { "error" => text_, "at" => time() }), Some(0o600), true) {
            Err(e) if !e.is_os() => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// 这些目录现在的样子（修改时间到纳秒、inode；没有的是 None）：目录里建、删、换名一个文件，它的修改时间就变
pub fn dir_stamps(dirs: &[&str]) -> Vec<Option<(i64, i64, u64)>> {
    dirs.iter().map(|d| std::fs::metadata(d).ok().map(|st| (st.mtime(), st.mtime_nsec(), st.ino()))).collect()
}

/// 睡 timeout 秒（至少 1 秒）；这些进程有一个退出、这些目录里有文件建 / 删 / 换名（登记出现、消失）就提前醒。
/// 被 SIGTERM 打断是 Stop。
/// seen 是调用方去看这些目录之前记下的 `dir_stamps`：开始盯着以后先比一次，已经不一样了（看的时候、上报的时候又有登记
/// 出现或者消失）就不睡，马上回去再看。原来没有这一步：变化正好落在“看完了、还没开始盯”的那一小段里就漏掉了，要等到下一次
/// 心跳（一分钟）才报——两个会话差不多同时结束时，后结束的那个常落在这一段里（在报前一个的变化）
#[cfg(target_os = "macos")]
pub fn presence_wait(pids: &[i64], dirs: &[&str], timeout: f64, seen: &[Option<(i64, i64, u64)>]) -> R<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    signal::check()?;
    let timeout = timeout.max(1.0);
    // SAFETY: kqueue 只是要一个新的描述符
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return signal::sleep(timeout.min(5.0)); // 没有 kqueue：退回 5 秒看一次
    }
    // SAFETY: kq 是刚拿到的、还没人管的描述符；交给 OwnedFd，离开这个函数时关上
    let kq = unsafe { OwnedFd::from_raw_fd(kq) };
    // SAFETY: 同上（Python 的描述符默认不留给 exec 出来的程序）
    unsafe { libc::fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    let none = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    let add = |ident: usize, filter: i16, flags: u16, fflags: u32| -> bool {
        // SAFETY: ev 是填好的一个 kevent；不要结果（eventlist 是空的）
        unsafe {
            let mut ev: libc::kevent = std::mem::zeroed();
            ev.ident = ident;
            ev.filter = filter;
            ev.flags = flags;
            ev.fflags = fflags;
            libc::kevent(kq.as_raw_fd(), &ev, 1, std::ptr::null_mut(), 0, &none) >= 0
        }
    };
    // 让它停下的信号（SIGTERM）也交给 kqueue 盯着，登记完再看一眼标志：登记之前到的由这一眼接住，之后到的把 kevent 叫醒。
    // 不这样的话，信号正好落在“刚看过标志、还没开始等”那一下就没人知道了，要等到下一次心跳才停（cca restore 只等它 5 秒）
    add(libc::SIGTERM as usize, libc::EVFILT_SIGNAL, libc::EV_ADD, 0);
    signal::check()?;
    let mut gone = false;
    for pid in pids {
        // 已经退出了（或者是还没回收的僵尸进程）：睡一秒就回去再看，不空转
        if !add(*pid as usize, libc::EVFILT_PROC, libc::EV_ADD | libc::EV_ONESHOT, libc::NOTE_EXIT) {
            gone = true;
        }
    }
    if gone {
        return signal::sleep(1.0);
    }
    let mut fds: Vec<OwnedFd> = Vec::new();
    for d in dirs {
        let Ok(c) = std::ffi::CString::new(*d) else { continue };
        // SAFETY: c 是以 0 结尾的路径。O_EVTONLY：只为收事件打开
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            continue;
        }
        // SAFETY: fd 是刚打开的
        fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
        add(fd as usize, libc::EVFILT_VNODE, libc::EV_ADD | libc::EV_CLEAR, libc::NOTE_WRITE);
    }
    if dir_stamps(dirs) != seen {
        signal::sleep(0.3)?; // 和下面等到事件时一样：等这一批文件写完再看
        return signal::check();
    }
    let deadline = monotonic() + timeout;
    loop {
        let ts = crate::py::time::timespec(deadline - monotonic());
        // SAFETY: got 是一个可写的 kevent
        let n = unsafe {
            let mut got: libc::kevent = std::mem::zeroed();
            libc::kevent(kq.as_raw_fd(), std::ptr::null(), 0, &mut got, 1, &ts)
        };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            signal::check()?;
            continue;
        }
        if n > 0 {
            signal::sleep(0.3)?; // 一次启动会连着写好几个文件：等写完再看
        }
        return signal::check();
    }
}

/// 没有 kqueue 的系统（Linux）：退回 5 秒看一次
#[cfg(not(target_os = "macos"))]
pub fn presence_wait(_pids: &[i64], _dirs: &[&str], timeout: f64, _seen: &[Option<(i64, i64, u64)>]) -> R<()> {
    signal::sleep(if timeout.is_nan() { 1.0 } else { timeout.clamp(1.0, 5.0) })
}

/// cca 这个文件现在的样子（换了就是更新了）
fn presence_stamp() -> Option<(u64, i64, i64, u64)> {
    let st = std::fs::metadata(&*SELF_PATH).ok()?;
    Some((st.ino(), st.mtime(), st.mtime_nsec(), st.size()))
}

/// `random.uniform(a, b)`
fn uniform(a: f64, b: f64) -> f64 {
    let raw = urandom(8);
    let bits = u64::from_le_bytes(raw.try_into().expect("要了 8 个字节")) >> 11;
    a + (b - a) * (bits as f64 / (1u64 << 53) as f64)
}

fn flock(f: &std::fs::File, op: libc::c_int) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: f 开着，fd 有效
    unsafe { libc::flock(f.as_raw_fd(), op) == 0 }
}

/// 没报成：隔多久再试。服务器不认这台 Mac（403）等维护者同步，10 分钟；别的按退避，5 秒起翻倍、最多 2 分钟（fails 是连着
/// 没报成的次数，从 1 起）
fn retry_after(code: i64, fails: u32) -> f64 {
    if code == 403 {
        600.0
    } else {
        (5.0 * 2f64.powi(fails.min(16) as i32)).min(120.0)
    }
}

/// 上报进程一路带着的状态（被停掉时要拿最后一次的 units 去报 end=down）
struct Agent {
    cfg: Option<Dict>,
    /// 上次报成的样子：[(账号摘要, 命令行会话数, 桌面 App 开没开)]，排好序的；还没报成过是 None
    shape: Option<Vec<(String, usize, bool)>>,
    units: Dict,
}

/// cca _presence-agent：这台 Mac 的上报进程（见上面这一节的说明）。同时只有一个（agent.lock）；用不了、没在用时马上退出
pub fn cmd_presence_agent(args: &[String]) -> R<()> {
    arity("cmd_presence_agent", args, 0, 0)?;
    if presence_load()?.1.is_none() {
        return Ok(());
    }
    fs::makedirs_mode(&PRESENCE_DIR, 0o700)?;
    let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "agent.lock"))?;
    if !flock(&lock, libc::LOCK_EX | libc::LOCK_NB) {
        return Ok(()); // 已经有一个在跑：它会经 kqueue 看到新的登记
    }
    let me = std::process::id() as i64;
    let started = proc_starts(&[me]).get(&me).copied().unwrap_or(0);
    save_json(&path::join(&PRESENCE_DIR, "agent.json"), &V::Dict(dict! { "pid" => me, "started" => started }), Some(0o600), true)?;
    signal::raise_on(libc::SIGTERM, "Stop");
    signal::ignore(libc::SIGHUP);
    let mut agent = Agent { cfg: None, shape: None, units: Dict::new() };
    match agent_loop(lock, &mut agent) {
        Err(e) if e.is("Stop") => {
            // 被停掉（关机、注销、cca restore）：告诉服务器，马上算离线，不用等心跳超时
            if let (Some(cfg), Some(shape)) = (&agent.cfg, &agent.shape) {
                if !cfg.is_empty() && !shape.is_empty() {
                    presence_post(cfg, "report", &dict! { "units" => &agent.units, "end" => "down" }, 3)?;
                }
            }
            Ok(())
        }
        other => other,
    }
}

fn agent_loop(lock: std::fs::File, agent: &mut Agent) -> R<()> {
    let stamp = presence_stamp();
    let mut apps = AppSamples::new();
    // 下一次该报的时刻：报成了是下一次心跳，没报成是退避结束（心跳到期、会话变了都等它）
    let (mut due, mut fails) = (0.0, 0u32);
    // 下一次同步记下备查的 API key（醒来时到了就同步，不为它多醒；keyshare_sync 自己也看上次同步的时间）
    let mut key_due = 0.0;
    loop {
        agent.cfg = presence_load()?.1;
        let Some(cfg) = agent.cfg.clone() else {
            return Ok(()); // 映射里去掉了、这台 Mac 关掉了、cca restore 了：不再报
        };
        let targets = presence_targets();
        // 账号目录在看之前的样子（在问进程、读登记之前记下）：等的时候拿它比，看的这段时间里变了的不会漏
        let seen = dir_stamps(&targets.iter().flatten().map(|(_, _, d)| d.as_str()).collect::<Vec<_>>());
        let procs = match &targets {
            Some(t) if !t.is_empty() => presence_procs(),
            _ => Some(Procs::default()),
        };
        let (Some(targets), Some(procs)) = (targets, procs) else {
            presence_wait(&[], &[], 5.0, &[])?;
            continue;
        };
        signal::check()?;
        let mono = monotonic();
        if mono >= key_due {
            match keyshare_sync(false, 15) {
                // 同步时被停掉（cca restore、关机）：照常报 end=down 再退出
                Err(e) if e.is("Stop") => return Err(e),
                // 同步 API key 出了什么错都不能停掉上报（config.json 读不了时 die 是 SystemExit）
                Err(e) => e.swallowed(),
                Ok(_) => {}
            }
            cost_kick(); // 折合金额：在后台统计、和别的设备同步（最多 COST_EVERY 一次）
            key_due = monotonic() + KEYSHARE_EVERY;
        }
        let salt = cfg.at("salt").py_str();
        let (units, pids) = presence_scan(&salt, &targets, &procs, &mut apps, time(), mono)?;
        agent.units = units;
        let mut cur: Vec<(String, usize, bool)> = agent
            .units
            .items()
            .iter()
            .map(|(a, u)| {
                let u = u.as_dict().expect("presence_scan 给的都是字典");
                (a.to_string(), u.list("cli").map_or(0, |c| c.len()), u.at("app").truthy())
            })
            .collect();
        cur.sort();
        if mono >= due || (agent.shape.as_ref() != Some(&cur) && fails == 0) {
            let end = if agent.units.is_empty() { V::from("none") } else { V::None };
            let (code, j) = presence_post(&cfg, "report", &dict! { "units" => &agent.units, "end" => end }, 8)?;
            if code == 200 {
                agent.shape = Some(cur);
                fails = 0;
                due = mono + PRESENCE_BEAT * uniform(0.9, 1.1);
                presence_status("")?;
            } else {
                fails += 1;
                due = mono + retry_after(code, fails);
                presence_status(&presence_why(&cfg, code, &j)?)?;
            }
        }
        if agent.units.is_empty() && (agent.shape.as_ref().is_some_and(|s| s.is_empty()) || fails >= 3) {
            // 什么都没开了（报过了，或者一直报不出去）：先放锁再看一眼，刚好有新会话时接着报
            flock(&lock, libc::LOCK_UN);
            let again = presence_targets().filter(|t| !t.is_empty());
            let Some(again) = again else { return Ok(()) };
            let procs = presence_procs().unwrap_or_default();
            if presence_scan(&salt, &again, &procs, &mut AppSamples::new(), time(), monotonic())?.0.is_empty() {
                return Ok(());
            }
            if !flock(&lock, libc::LOCK_EX | libc::LOCK_NB) {
                return Ok(()); // 新起的那个接手了
            }
            continue;
        }
        if presence_stamp() != stamp && presence_stamp().is_some() {
            // cca 更新了：换成新版本接着报
            drop(lock);
            let _ = execv(&SELF_PATH, &[&SELF_PATH, "_presence-agent"]);
            return Ok(());
        }
        let dirs: Vec<&str> = targets.iter().map(|(_, _, d)| d.as_str()).collect();
        presence_wait(&pids, &dirs, due - monotonic(), &seen)?;
    }
}

// --- cca devices（presence-view）--------------------------------------------------------------------------------------------

/// 过去的某个时刻（和 App 的 Texts.pastTime 一样）：今天的写几点，昨天的写“昨天 几点”，一周内写星期几，再早写日期
pub fn past_time(t_: f64, now: f64) -> R<String> {
    let (lt, ln) = (localtime(t_), localtime(now));
    let days = day_gap(t_, now)?;
    Ok(if days <= 0 {
        format!("{:02}:{:02}", lt.hour, lt.min)
    } else if days == 1 {
        format!("昨天 {:02}:{:02}", lt.hour, lt.min)
    } else if days < 7 {
        weekday_clock(&lt)
    } else {
        let year = if lt.year == ln.year { String::new() } else { format!("{} 年 ", lt.year) };
        format!("{}{} 月 {} 日", year, lt.mon, lt.mday)
    })
}

/// `'%d' % x`（x 是整数或者布尔值）
fn count_of(x: &V) -> i64 {
    match x {
        V::Bool(b) => *b as i64,
        other => other.as_int().unwrap_or(0),
    }
}

/// cca devices 里一台设备的状态（和 App 的 Texts.deviceState 说法一样；离线的说明 App 里在悬停时显示）
fn presence_line(d: &Dict, now: f64) -> R<String> {
    let epoch = |k: &str| d.get(k).filter(|x| x.truthy()).and_then(|x| x.as_num());
    let mut what: Vec<String> = Vec::new();
    if d.truthy("cli") {
        what.push(format!("命令行里 {} 个会话", count_of(&d.at("cli"))));
    }
    if d.truthy("app") {
        what.push(format!("{} 开着", t().app_label));
    }
    let what = what.join("、");
    let used = match epoch("last_used_epoch") {
        Some(at) => format!(" · 上次使用 {}", past_time(at, now)?),
        None => String::new(),
    };
    let dot_what = if what.is_empty() { String::new() } else { format!(" · {what}") };
    let state = d.at("state");
    Ok(match state.as_str() {
        Some("in_use") => {
            let since = match epoch("since_epoch") {
                Some(at) => format!(" · {} 起", past_time(at, now)?),
                None => String::new(),
            };
            format!("在用{dot_what}{since}")
        }
        Some("idle") => {
            let idle = match epoch("last_active_epoch") {
                Some(at) => format!(" {}", span(now - at)?),
                None => String::new(),
            };
            format!("开着，闲置{idle}{dot_what}")
        }
        Some("offline") => {
            let seen = match epoch("seen_epoch") {
                None => "很久以前".to_string(),
                Some(at) if now - at < 60.0 => "刚刚".to_string(),
                Some(at) => format!("{}前", ago(now - at)?),
            };
            format!("离线{used}（{seen}最后一次回报，当时还开着会话：可能休眠、断网，或者被强行结束了，不知道现在还用不用）")
        }
        _ => format!("没在用{used}"),
    })
}

/// cca [codex] devices [账号…] [--json]：映射里的账号现在连着哪些设备（问维护者的服务器）。--json 是给 App 的（PROTOCOL 3.4）。
/// 退出码：0 成功；1 服务器那边没成；2 用不了（这台 Mac 没配置、关掉了，账号不在映射里）。没成时 stdout 是原因
pub fn cmd_presence_view(args: &[String]) -> R<()> {
    let as_json = args.iter().any(|a| a == "--json");
    let names: Vec<&String> = args.iter().filter(|a| *a != "--json").collect();
    let (reason, cfg) = presence_load()?;
    let Some(cfg) = cfg else {
        out!("{reason}");
        return exit(2);
    };
    let (table, pm) = (ctx()?.1, policy_raw(t()));
    let keys: Vec<String> = if names.is_empty() {
        table.items().iter().filter(|(k, v)| presence_mapped(&pm, k, v)).map(|(k, _)| k.to_string()).collect()
    } else {
        names.iter().map(|n| account_named(&table, n)).collect::<R<_>>()?
    };
    for k in &keys {
        if !presence_mapped(&pm, k, &table.at(k)) {
            out!("{} 不在维护者的映射里：只统计映射里的官方账号", name_of(k));
            return exit(2);
        }
    }
    let salt = cfg.at("salt").py_str();
    // {账号摘要: 键}：摘要一样的（同一个邮箱写了两次）后来的顶掉，位置不变
    let aids = Dict::new();
    for k in &keys {
        aids.set(presence_aid(&salt, t().name, &parse_key(k).1)?, k.as_str());
    }
    let (code, j) = if aids.is_empty() {
        (200, dict! { "accounts" => Dict::new() })
    } else {
        let wanted: Vec<V> = aids.keys().into_iter().map(V::from).collect();
        presence_post(&cfg, "view", &dict! { "accounts" => wanted }, 8)?
    };
    let recv = time();
    let Some(by_account) = j.dict("accounts").filter(|_| code == 200) else {
        out!("{}", if code != 200 { presence_why(&cfg, code, &j)? } else { "设备统计的服务器回的内容看不懂".to_string() });
        return exit(1);
    };
    let ep = |age: Option<V>| -> V {
        match age.filter(is_num).and_then(|a| a.as_num()) {
            Some(age) => V::from(num::round_to(recv - age, 3)),
            None => V::None,
        }
    };
    let order = |state: &str| ["in_use", "idle", "offline", "none"].iter().position(|s| *s == state);
    let accounts = List::new();
    for (aid, k) in aids.items() {
        let k = k.py_str();
        // j['accounts'].get(aid) or []
        let raw = match by_account.get(&aid).filter(|r| r.truthy()) {
            None => Vec::new(),
            Some(V::List(rows)) => rows.items(),
            Some(V::Dict(_)) | Some(V::Str(_)) => Vec::new(), // 逐项看到的是键 / 字符，都不是字典
            Some(other) => panic!("TypeError: '{}' object is not iterable", if other.is_bool() { "bool" } else { "number" }),
        };
        let mut rows: Vec<Dict> = Vec::new();
        for r in raw {
            let Some(r) = r.as_dict() else { continue };
            let Some(name) = r.str("name") else { continue };
            let state = r.get("state").unwrap_or(V::None);
            if state.is_list() || state.is_dict() {
                panic!("TypeError: unhashable type（服务器回的 state 是 {}）", state.py_repr());
            }
            let Some(state) = state.as_str().filter(|s| order(s).is_some()) else { continue };
            let fp = r.get("fp");
            rows.push(dict! {
                "id" => fp.clone(), "name" => name, "me" => fp.is_some_and(|f| f.py_eq(&cfg.at("fp"))), "state" => state,
                "cli" => r.get("cli").filter(|c| c.isinstance_int()).unwrap_or(V::Int(0)), "app" => matches!(r.get("app"), Some(V::Bool(true))),
                "since_epoch" => ep(r.get("since_age")), "last_active_epoch" => ep(r.get("idle_age")),
                "last_used_epoch" => ep(r.get("used_age")), "seen_epoch" => ep(r.get("seen_age")),
            });
        }
        // 这台设备在前，再按状态、最近用过的在前、名字
        rows.sort_by(|a, b| {
            let key = |r: &Dict| {
                (
                    !r.truthy("me"),
                    order(&r.at("state").py_str()),
                    -r.get("last_used_epoch").filter(|x| x.truthy()).and_then(|x| x.as_num()).unwrap_or(0.0),
                )
            };
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.cmp(&kb.1))
                .then(ka.2.partial_cmp(&kb.2).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.at("name").py_str().cmp(&b.at("name").py_str()))
        });
        let devices: Vec<V> = rows.into_iter().map(V::Dict).collect();
        accounts.push(dict! { "key" => k.as_str(), "id" => acct_id(&k, t()), "devices" => devices });
    }
    // (load_quiet(status.json) or {}).get('error') or None
    let failing = match load_quiet(&path::join(&PRESENCE_DIR, "status.json")).filter(|s| s.truthy()) {
        None => None,
        Some(V::Dict(s)) => s.get("error").filter(|e| e.truthy()),
        Some(other) => panic!("AttributeError: status.json 里是 {}，不是对象", other.py_repr()),
    };
    keyshare_kick(false); // 顺带：记下备查的 API key 和登录了同一个账号的其他设备同步（最多 KEYSHARE_EVERY 一次）
    cost_kick(); // 折合金额也统计一次、和别的设备同步（最多 COST_EVERY 一次）
    if as_json {
        let doc = dict! {
            "version" => 1, "kind" => "devices", "generated_epoch" => recv, "tool" => t().name, "me" => j.get("me_name"),
            "idle_after" => j.get("idle"), "offline_after" => j.get("ttl"), "reporting_error" => failing, "accounts" => accounts,
        };
        out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    let me = match j.get("me_name").filter(|m| m.truthy()) {
        Some(m) => format!("，这台 Mac 叫 {}", m.py_str()),
        None => String::new(),
    };
    let mut lines = vec![format!("{} 账号连着的设备（维护者的服务器上的记录{}）：", t().label, me)];
    for a in accounts.items() {
        let a = a.as_dict().expect("刚建的");
        lines.push(format!("  {}", label_of(&a.at("key").py_str())));
        let devices = a.list("devices").expect("刚建的");
        for d in devices.items() {
            let d = d.as_dict().expect("刚建的");
            lines.push(format!(
                "    · {}{}  {}",
                d.at("name").py_str(),
                if d.truthy("me") { "（这台 Mac）" } else { "" },
                presence_line(&d, recv)?
            ));
        }
        if devices.is_empty() {
            lines.push("    30 天内没有设备经 cca 用过".to_string());
        }
    }
    if let Some(failing) = failing {
        lines.push(format!("  这台 Mac 最近一次没报成：{}", failing.py_str()));
    }
    if let Some(key_err) = keyshare_error() {
        lines.push(format!("  记下的 API key 最近一次没同步成：{key_err}"));
    }
    out!("{}", lines.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::claude;

    fn vectors() -> Dict {
        json::loads(include_str!("../../tests/vectors/src_presence.json")).unwrap().as_dict().unwrap()
    }

    /// 去看账号目录和开始盯着它之间又变了（登记出现、消失）：不睡到下一次心跳，马上回去再看
    #[cfg(target_os = "macos")]
    #[test]
    fn wait_does_not_sleep_through_a_change_made_while_scanning() {
        let dir = std::env::temp_dir().join(format!("cca-presence-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        let seen = dir_stamps(&[d]);
        assert!(seen[0].is_some() && dir_stamps(&["/nonexistent/cca-presence-wait"]) == [None]);
        // 没变：睡满（这里 1 秒）
        let t0 = monotonic();
        presence_wait(&[], &[d], 1.0, &seen).unwrap();
        assert!(monotonic() - t0 >= 0.9, "{}", monotonic() - t0);
        // “看的时候”来了一个登记：给了很长的时限也马上回来
        std::fs::write(dir.join("cli-4242.json"), "{}").unwrap();
        assert_ne!(dir_stamps(&[d]), seen);
        let t0 = monotonic();
        presence_wait(&[], &[d], 60.0, &seen).unwrap();
        assert!(monotonic() - t0 < 5.0, "{}", monotonic() - t0);
        // 盯着的时候才变的：照旧被事件叫醒
        let seen = dir_stamps(&[d]);
        let path = dir.join("cli-4242.json");
        let late = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::fs::remove_file(path).unwrap();
        });
        let t0 = monotonic();
        presence_wait(&[], &[d], 60.0, &seen).unwrap();
        assert!((0.2..5.0).contains(&(monotonic() - t0)), "{}", monotonic() - t0);
        late.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 期望值是原来的 Python 算的（tools/gen_src_vectors.py）
    #[test]
    fn cpu_secs_like_python() {
        for row in vectors().list("cpu_secs").unwrap().items() {
            let row = row.as_list().unwrap();
            let (s, want) = (row.at(0).py_str(), row.at(1).py_str());
            assert_eq!(num::float_repr(cpu_secs(&s)), want, "cpu_secs({s:?})");
        }
    }

    /// 桌面 App 的动静：进程树的 CPU 照 ps 列出来的顺序加、隔 30 秒才采样、过了阈值才算有动静、App 重启 / 没了以后重新算
    #[test]
    fn scan_app_sampling_like_python() {
        assert_eq!(&**CCA_HOME, "/nonexistent/cca-unit-tests", "单元测试要在 cargo test 下跑（.cargo/config.toml 给的 CCA_HOME）");
        let scan = vectors().dict("scan").unwrap();
        let (salt, key, dir) = (scan.at("salt").py_str(), scan.at("key").py_str(), scan.at("dir").py_str());
        let targets: Vec<Target> = vec![(claude(), key, dir)];
        let mut apps = AppSamples::new();
        for step in scan.list("steps").unwrap().items() {
            let step = step.as_dict().unwrap();
            let label = step.at("label").py_str();
            let rows = step.list("procs").unwrap().items().into_iter().map(|p| {
                let p = p.as_list().unwrap();
                let int = |i: isize| p.at(i).as_int().unwrap();
                (
                    int(0),
                    ProcInfo {
                        ppid: int(1),
                        tty: p.at(2).py_str(),
                        cpu: p.at(3).as_num().unwrap(),
                        start: int(4),
                        cmd: p.at(5).py_str(),
                    },
                )
            });
            let procs = Procs::from_rows(rows);
            let (units, mut pids) = presence_scan(
                &salt,
                &targets,
                &procs,
                &mut apps,
                step.at("now").as_num().unwrap(),
                step.at("mono").as_num().unwrap(),
            )
            .unwrap();
            pids.sort_unstable();
            assert_eq!(json::dumps(&V::Dict(units), json::LINE), json::dumps(&step.at("units"), json::LINE), "{label}：units");
            assert_eq!(
                pids,
                step.list("pids").unwrap().items().iter().map(|p| p.as_int().unwrap()).collect::<Vec<_>>(),
                "{label}：pids"
            );
            let mut got: Vec<String> = apps
                .iter()
                .map(|(k, v)| {
                    format!("{} {} {} {} {}", k.0, k.1, num::float_repr(v[0]), num::float_repr(v[1]), num::float_repr(v[2]))
                })
                .collect();
            got.sort();
            let want: Vec<String> = step
                .list("apps")
                .unwrap()
                .items()
                .iter()
                .map(|a| a.as_list().unwrap().items().iter().map(|x| x.py_str()).collect::<Vec<_>>().join(" "))
                .collect();
            assert_eq!(got, want, "{label}：采样");
        }
    }

    /// tests/presence.py 的 test_identity：指纹和 `ssh-keygen -l -E sha256` 给的一样（这把公钥是编的，期望值是 ssh-keygen 算的）
    #[test]
    fn ssh_fp_like_ssh_keygen() {
        assert_eq!(
            ssh_fp("AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f").as_deref(),
            Some("SHA256:ZkAslGjFiUHdGf/WUL8rQvkib4PTvQatUV0OUQSncCA")
        );
        assert_eq!(ssh_fp("not base64!"), None);
        let a = presence_aid("abababababababababababababababab", "claude", "Alice@Example.com").unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(a, presence_aid("abababababababababababababababab", "claude", "alice@example.com").unwrap(), "邮箱不分大小写");
        assert_ne!(
            a,
            presence_aid("abababababababababababababababab", "codex", "alice@example.com").unwrap(),
            "Claude 账号和 Codex 账号不撞"
        );
        assert_ne!(a, presence_aid("11111111111111111111111111111111", "claude", "alice@example.com").unwrap(), "换了盐就认不出");
    }

    /// tests/presence.py 的 test_agent_backoff 里的几个数：报不出去按退避再试（10、20 秒……最多 2 分钟），服务器不认这台 Mac 时 10 分钟
    #[test]
    fn retry_backs_off() {
        assert_eq!((1..=7).map(|n| retry_after(0, n)).collect::<Vec<_>>(), [10.0, 20.0, 40.0, 80.0, 120.0, 120.0, 120.0]);
        assert_eq!(retry_after(500, 1), 10.0);
        assert_eq!(retry_after(0, 4_000_000), 120.0);
        assert_eq!((retry_after(403, 1), retry_after(403, 9)), (600.0, 600.0));
        for _ in 0..200 {
            let beat = PRESENCE_BEAT * uniform(0.9, 1.1);
            assert!((54.0..66.0).contains(&beat), "心跳在 60 秒上下：{beat}");
        }
        assert_eq!(KEYSHARE_EVERY, 600.0, "上报进程里记下的 API key 两次同步至少隔 10 分钟");
    }

    /// tests/presence.py 的 test_scan_app_cpu：桌面 App 看进程树的 CPU——开了很久的第一次见到先当作刚好闲置满 10 分钟，隔 30 秒以上的两次
    /// 采样之间用掉不少 CPU（含子进程）才算有动静；别的资料目录不算
    #[test]
    fn scan_app_cpu_like_python_test() {
        let (key, dir, salt, now) = ("alice@example.com", "/nonexistent/cca-unit-tests/acct", "00".repeat(16), 1795000000.25);
        let targets: Vec<Target> = vec![(claude(), key.to_string(), dir.to_string())];
        let aid = presence_aid(&salt, "claude", key).unwrap();
        let start = num::round(now - 3600.0);
        let table = |cpu: f64, cmd: String| {
            Procs::from_rows(vec![
                (500, ProcInfo { ppid: 1, tty: "??".into(), cpu, start, cmd }),
                (501, ProcInfo { ppid: 500, tty: "??".into(), cpu: 5.0, start, cmd: "Claude Helper".into() }),
            ])
        };
        let app = format!("/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir={dir}/desktop --proxy-server=x");
        let mut apps = AppSamples::new();
        let mut scan =
            |cpu: f64, t: f64, mono: f64| presence_scan(&salt, &targets, &table(cpu, app.clone()), &mut apps, t, mono).unwrap().0;
        let app_of = |units: &Dict| {
            units.dict(&aid).unwrap().list("app").unwrap().items().iter().map(|x| x.as_int().unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(app_of(&scan(10.0, now, 1000.0)), [3600, 600], "开了很久、第一次见到：先当作刚好闲置满 10 分钟");
        assert_eq!(app_of(&scan(10.5, now + 40.0, 1040.0))[1], 640, "CPU 几乎没动：还是闲置");
        assert_eq!(app_of(&scan(14.0, now + 80.0, 1080.0))[1], 0, "一拍里用掉不少 CPU（含子进程）：有动静");
        assert_eq!(app_of(&scan(14.0, now + 90.0, 1090.0))[1], 10, "隔得太近的采样不算，最后动静照旧");
        assert!(scan(14.0, now + 90.0, 1090.0).dict(&aid).unwrap().list("cli").unwrap().is_empty(), "没有命令行会话");
        let other = table(1.0, format!("/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir={dir}/desktop-x"));
        assert!(presence_scan(&salt, &targets, &other, &mut AppSamples::new(), now, 1.0).unwrap().0.is_empty(), "别的资料目录不算");
    }

    /// tests/presence.py 的 test_past_time：过去的某个时刻怎么写（今天的写几点，昨天、一周内、再早的各有写法；本地时间）
    #[test]
    fn past_time_like_python_test() {
        use crate::py::time::mktime;
        let when = |t_: f64, now: f64| past_time(t_, now).unwrap();
        let now = mktime(2026, 11, 10, 15, 30, 0, -1);
        assert_eq!(when(now - 3600.0, now), "14:30");
        assert_eq!(when(mktime(2026, 11, 9, 18, 40, 0, -1), now), "昨天 18:40");
        assert_eq!(when(mktime(2026, 11, 6, 9, 5, 0, -1), now), "周五 09:05");
        assert_eq!(when(mktime(2026, 10, 20, 9, 5, 0, -1), now), "10 月 20 日");
        assert_eq!(when(mktime(2025, 10, 20, 9, 5, 0, -1), now), "2025 年 10 月 20 日");
    }

    /// tests/presence.py 的 test_scan_cli_idle_from_tty：命令行会话的闲置时间 = 现在 - 它终端的最后一次输入 / 输出（真的开一个伪终端）
    #[test]
    fn scan_cli_idle_from_tty() {
        let dir = fs::TempDir::new().unwrap();
        let (key, now) = ("alice@example.com", time());
        let mark = dict! { "v" => 1, "tool" => "claude", "id" => crate::core::model::acct_id(key, claude()), "pid" => 4242, "started" => 1000, "since" => now - 300.0 };
        std::fs::write(path::join(&dir.path, "cli-4242.json"), json::dumps(&V::Dict(mark), json::LINE)).unwrap();
        // SAFETY: openpty 要两个可写的 int 和一段放名字的缓冲区；utimes 的路径以 0 结尾、时间是两项的数组
        let tty = unsafe {
            let (mut master, mut slave) = (0, 0);
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(libc::openpty(&mut master, &mut slave, name.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut()), 0);
            let stamp = |secs: f64| libc::timeval { tv_sec: secs.trunc() as _, tv_usec: (secs.fract() * 1e6) as _ };
            // 最后一次输入 120 秒前、输出 200 秒前
            assert_eq!(libc::utimes(name.as_ptr(), [stamp(now - 120.0), stamp(now - 200.0)].as_ptr()), 0);
            std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned()
        };
        let short = tty.strip_prefix("/dev/").unwrap().to_string();
        assert!(TTY_RE.matches(&short), "{short}");
        let targets: Vec<Target> = vec![(claude(), key.to_string(), dir.path.clone())];
        let salt = "abababababababababababababababab";
        let aid = presence_aid(salt, "claude", key).unwrap();
        let scan = |tty_: &str, start: i64, there: bool| {
            let rows = if there {
                vec![(4242, ProcInfo { ppid: 1, tty: tty_.to_string(), cpu: 1.0, start, cmd: "bash cca".into() })]
            } else {
                Vec::new()
            };
            let (units, pids) =
                presence_scan(salt, &targets, &Procs::from_rows(rows), &mut AppSamples::new(), now, 1000.0).unwrap();
            (json::dumps(&V::Dict(units), json::LINE), pids)
        };
        assert_eq!(
            scan(&short, 1000, true),
            (format!("{{\"{aid}\": {{\"cli\": [[300, 120]], \"app\": null}}}}"), vec![4242]),
            "闲置 = 现在 - 最后一次输入 / 输出；盯着这个会话的进程"
        );
        assert_eq!(scan(&short, 1003, true).0, "{}", "进程号被重用（启动时间对不上）：不算");
        assert_eq!(scan(&short, 1000, false).0, "{}", "cca 被 kill -9 留下的登记：进程没了，不算");
        assert_eq!(
            scan("??", 1000, true).0,
            format!("{{\"{aid}\": {{\"cli\": [[300, 0]], \"app\": null}}}}"),
            "没有终端：在跑就算在干活"
        );
        // 会话在宿主里（远程会话）：宿主（4300）是 cca（4242）的子进程，它记着最后一次动静
        let hosted = |tty_: &str, act: f64| {
            let rows = vec![
                (4242, ProcInfo { ppid: 1, tty: tty_.to_string(), cpu: 1.0, start: 1000, cmd: "bash cca".into() }),
                (4300, ProcInfo { ppid: 4242, tty: "??".into(), cpu: 1.0, start: 1001, cmd: "cca-core remote-host".into() }),
                (4400, ProcInfo { ppid: 1, tty: "??".into(), cpu: 1.0, start: 1001, cmd: "cca-core remote-host".into() }),
            ];
            let acts = move || HashMap::from([(4300, act), (4400, now)]);
            let (units, _) =
                presence_scan_with(salt, &targets, &Procs::from_rows(rows), &mut AppSamples::new(), now, 1000.0, &acts).unwrap();
            json::dumps(&V::Dict(units), json::LINE)
        };
        assert_eq!(
            hosted("??", now - 200.0),
            format!("{{\"{aid}\": {{\"cli\": [[300, 200]], \"app\": null}}}}"),
            "启动它的终端没了：闲置按宿主记的最后一次动静算（别的 cca 的宿主不算）"
        );
        assert_eq!(
            hosted(&short, now - 30.0),
            format!("{{\"{aid}\": {{\"cli\": [[300, 30]], \"app\": null}}}}"),
            "终端还在：取终端和宿主里晚的那个（别的机器上的输入不经过这个终端）"
        );
        assert_eq!(
            hosted(&short, now - 250.0),
            format!("{{\"{aid}\": {{\"cli\": [[300, 120]], \"app\": null}}}}"),
            "终端更晚有动静就按终端的"
        );
        assert_eq!(
            hosted("??", now - 900.0),
            format!("{{\"{aid}\": {{\"cli\": [[300, 300]], \"app\": null}}}}"),
            "不早于会话开始的时候"
        );
    }

    #[test]
    fn app_proc_re_matches_both_spellings_and_escapes() {
        let desk = "/nonexistent/acct (1)/a.b+c#d~e&f/desktop";
        let re = app_proc_re(claude(), desk).unwrap();
        let cmd = |tail: &str| format!("/Applications/Claude.app/Contents/MacOS/Claude --flag --user-data-dir={tail}");
        assert!(re.found(&cmd(desk)));
        assert!(re.found(&cmd(&format!("{desk}/ --more"))));
        assert!(re.found(&cmd(&format!("{desk} --more"))));
        assert!(!re.found(&cmd(&format!("{desk}2"))));
        assert!(!re.found(&cmd("/nonexistent/acct (1)/aXb+c#d~e&f/desktop")), "点不是通配符");
        assert!(!re.found(&format!("/usr/bin/other --user-data-dir={desk}")));
        assert!(app_proc_re(crate::core::tool::agy(), desk).is_none());
    }
}
