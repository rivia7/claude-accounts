//! 远程启动（docs/dev/launch.md）：允许远程启动的配对（配对表里的 `launch`：iPhone 上的 cca App，配对过的另一台电脑——那边是
//! start.rs 的 `cca remote launch`、Mac 上的 cca App）挑一个模板（template.rs），这台机器起一个新的会话（无头的宿主：host.rs 的
//! `--headless`），它接着接进去，和接入别的会话一样。下面说的“手机”都是这样的一台（3.7.6 只有手机）。
//!
//! - 后台进程（`remote-launcher serve`：macOS 上 LaunchAgent、Linux 上 crontab 起的；`launcher.lock` 保证只有一个）：有允许远程
//!   启动的手机时，给它们配对的每台 Team 服务器起一个转接进程；一个都没有了就退出。cca 换了版本（`CCA_SELF` 变了）就重新执行自己
//! - 转接进程（`remote-link --launcher Team编号 会话编号`）：在那台服务器上登记成一个“会话”（编号固定，记在 `launcher.json`：重启以后
//!   顶掉原来那一条），把模板的说明（`template::launch_doc`）封给每个允许的手机（ANNOUNCE；密钥和会话的说明不一样，旧的 App 解不开、
//!   当它不存在）。手机接进来（JOIN）分出一个子进程：握手、等它说起哪个（LAUNCH）、现读配对表和模板再查一遍、起会话、回话
//!   （LAUNCHED），在 `launch.log` 记一行（不记开场白）
//! - 起的会话：`remote-host --headless --sid 编号 --size 列x行 工具 账号键 名字 -- cca [codex|agy] @账号键 -- 模板的参数… [-- 开场白]`，
//!   环境只给 `KEEP_ENV` 这几个，当前目录是模板的文件夹，自己一个会话（setsid）。bash 看到 `CCA_REMOTE_HOSTED` 不再套一层宿主
//!
//! 线上的格式见 server/README.md「启动器」：服务器不用改，它看启动器就是一个会话（占这台设备 8 个名额里的一个）。

use super::far::clean;
use super::link::{self, Allowed, Join, Link, Respawn};
use super::local::{self, Asked, Decoder};
use super::peers::{self, Peer};
use super::pty::{self, Size};
use super::relay::{self, ctl_frame, Tunnel};
use super::template::{self, Template};
use super::{crypto, sig};
use crate::cmd::arity;
use crate::cmd::hub::{HubKind, TeamTag};
use crate::cmd::presence::flock;
use crate::cmd::session::proc_starts;
use crate::core::base::SELF_PATH;
use crate::dict;
use crate::py::fs;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::plist::{self, P};
use crate::py::proc::{self, In, Out};
use crate::py::rand::token_hex;
use crate::py::time::{monotonic, sleep, strftime_local, time};
use crate::py::{die, exit, R};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

const USAGE: &str = "用法：remote-launcher [status] | serve [--if-down] | stop | plist cca的路径 | wanted | log";
/// LaunchAgent 的名字（bash 的 remote_launcher_install 也用它）
const LABEL: &str = "local.cca.remote-launcher";
/// 一台服务器上最多给几台手机放说明：一台一份、最长 4 KB 多，控制连接上一帧最多 32 KB
const PEERS_MAX: usize = 6;
/// 手机接进来以后最多等这么久说要起哪个
const ASK_WAIT: f64 = 20.0;
/// LAUNCH 的内容最长多少
const ASK_MAX: usize = 4096;
/// 开场白最多几个字
const LINE_MAX: usize = 500;
/// 起了以后最多等这么久新会话的 socket
const SPAWN_WAIT: f64 = 4.0;
/// 一分钟里最多起成几个会话
const LAUNCHES_PER_MIN: usize = 4;
/// 同一台手机两次接进来至少隔这么久（秒）
const JOIN_GAP: f64 = 3.0;
/// 这台机器走这台服务器的会话已经这么多：不再起（服务器给一台设备最多 8 个，启动器自己占一个）
const SESSIONS_MAX: usize = 7;
/// 子进程起成了一个会话时的退出码（转接进程照它数一分钟起了几个）
const EXIT_LAUNCHED: i32 = 10;
/// 后台进程隔多久看一眼配对表变了没有（转接进程照 link.rs 的 PEERS_EVERY 看配对表，跟着看模板）
const CHECK_EVERY: f64 = 3.0;
/// 隔多久重新算一遍给手机的说明（账号的别名、文件夹变了也要跟着变）
const DOC_EVERY: f64 = 60.0;
/// 后台进程隔多久看一眼 cca 换没换版本
const SELF_EVERY: f64 = 30.0;
/// 用不了的服务器（转接进程退出码 3）隔多久再试（配对表变了马上试）
const OFF_RETRY: f64 = 600.0;
/// launch.log 到这么大就换成 launch.log.1
const LOG_MAX: u64 = 64 * 1024;
/// 起的会话只带这些环境变量（再加 TERM、CCA_REMOTE、CCA_REMOTE_HOSTED）：后台进程的环境是 launchd / cron 给的，别的不该带进会话。
/// CCA_KEYSTORE（密钥存成文件，只给测试）要和启动器自己一样：会话的转接进程照它找配对的密钥
const KEEP_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "SHELL",
    "SSH_AUTH_SOCK",
    "__CF_USER_TEXT_ENCODING",
    "CCA_HOME",
    "CCA_HELPER",
    "CCA_KEYSTORE",
    "TZ",
    "CCA_HEADLESS_LINGER",
    "CCA_HEADLESS_IDLE",
];

fn file(name: &str) -> String {
    path::join(&local::dir(), name)
}

/// 允许远程启动的手机、电脑（看和操作都让的才开得了：peers.rs 读的时候核对过，这里再守一道）；hub：只要在这台服务器上配的
pub fn launch_peers(hub: Option<HubKind>) -> Vec<Peer> {
    peers::load().into_iter().filter(|p| p.launch && p.can_launch() && hub.is_none_or(|h| p.hub == h)).collect()
}

/// 后台进程在不在跑：它一直拿着 launcher.lock 的锁。只读着打开、试一下（不建文件：CCA_RO 下也能问）
pub fn running() -> bool {
    std::fs::File::open(file("launcher.lock")).is_ok_and(|f| !flock(&f, libc::LOCK_SH | libc::LOCK_NB))
}

/// 给 JSON 输出的（`remote-template list --json`、`remote-list --json` 的 launcher）：后台进程在不在跑、几个模板、几台手机
pub fn summary() -> Dict {
    dict! { "running" => running(), "templates" => template::load().len(), "peers" => launch_peers(None).len() }
}

/// `remote-launcher …`
pub fn cmd_remote_launcher(args: &[String]) -> R<()> {
    match args {
        [] => status(),
        [a] if a == "status" => status(),
        [a] if a == "serve" => serve(false),
        [a, f] if a == "serve" && f == "--if-down" => serve(true),
        [a] if a == "stop" => stop_running(),
        [a, cca] if a == "plist" => {
            crate::out!("{}", plist_text(cca)?);
            Ok(())
        }
        // 有没有允许远程启动的手机：bash 的 remote_launcher_sync 照它装上 / 卸掉后台进程
        [a] if a == "wanted" => {
            if launch_peers(None).is_empty() {
                exit(1)
            } else {
                Ok(())
            }
        }
        [a] if a == "log" => print_log(),
        _ => die(USAGE),
    }
}

// ---------------------------------------------------------------------------
// 后台进程
// ---------------------------------------------------------------------------

/// 后台进程给一台服务器起的转接进程
struct HubLink {
    hub: HubKind,
    sid: String,
    child: Option<Child>,
    retry: Respawn,
}

impl HubLink {
    /// 起转接进程（它的标准输入是一根管道：后台进程不在了它读到头，告诉服务器、自己退出）；起不来当它马上退出了
    fn spawn(&mut self, now: f64) {
        self.retry.started(now);
        let child = std::env::current_exe().ok().and_then(|exe| {
            let mut c = Command::new(exe);
            c.args(["remote-link", "--launcher", &self.hub.team(), &self.sid]);
            c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
        });
        match child {
            Some(c) => self.child = Some(c),
            None => self.retry.failed(now),
        }
    }

    /// 退出了没有：这台服务器用不了（退出码 3）过一阵再试，别的按退避再起
    fn reap(&mut self, now: f64) {
        let Some(c) = self.child.as_mut() else { return };
        match c.try_wait() {
            Ok(None) => {}
            Ok(Some(st)) if st.code() == Some(link::EXIT_OFF) => {
                self.child = None;
                self.retry.next = now + OFF_RETRY;
            }
            _ => {
                self.child = None;
                self.retry.failed(now);
            }
        }
    }

    /// 不要它了：关掉它的标准输入（它告诉服务器、自己退出），交给调用方收尸
    fn retire(&mut self) -> Option<Child> {
        let mut c = self.child.take()?;
        drop(c.stdin.take());
        Some(c)
    }
}

/// launcher.json：后台进程的进程号（`pid`）、开始的时间（`started`），各台服务器上的编号（`sids`：Team 编号 → 会话编号，重启以后
/// 还用这个，服务器顶掉原来那一条）。没有、看不懂是空的（缺了哪一项也不崩：读的时候都用 `get`）
fn state() -> Dict {
    fs::load_quiet(&file("launcher.json")).and_then(|v| v.as_dict()).unwrap_or_default()
}

fn state_int(d: &Dict, k: &str) -> i64 {
    d.get(k).and_then(|v| v.as_int()).unwrap_or(0)
}

fn save_state(pid: i64, started: i64, sids: &Dict) {
    let doc = dict! { "pid" => pid, "started" => started, "sids" => sids };
    let _ = fs::save_json(&file("launcher.json"), &V::Dict(doc), Some(0o600), true);
}

/// `remote-launcher serve [--if-down]`：常驻。已经有一个在跑（--if-down：cron 隔一阵叫一次，不说话）、没有允许远程启动的手机时马上退出
/// （退出码 0：launchd 不再起它）
fn serve(if_down: bool) -> R<()> {
    let dir = local::ensure_dir()?;
    let lock = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(path::join(&dir, "launcher.lock"))?;
    // 别的进程可能只是在看它在不在跑（running：一下就放开）：多试几次，别撞上它就以为已经有一个了
    let mut got = false;
    for _ in 0..10 {
        if flock(&lock, libc::LOCK_EX | libc::LOCK_NB) {
            got = true;
            break;
        }
        sleep(0.1);
    }
    if !got {
        if !if_down {
            crate::err!("cca: 远程启动的后台进程已经在运行了（cca remote launcher 看状态）");
        }
        return Ok(());
    }
    if launch_peers(None).is_empty() {
        return Ok(());
    }
    // SAFETY: getpid 没有参数、不会失败
    let me = i64::from(unsafe { libc::getpid() });
    let started = proc_starts(&[me]).get(&me).copied().unwrap_or(0);
    let sids = state().dict("sids").unwrap_or_default();
    save_state(me, started, &sids);
    let wake = sig::install()?;
    let self_was = peers::stamp_of(&SELF_PATH);
    let mut links: Vec<HubLink> = Vec::new();
    let mut retired: Vec<Child> = Vec::new();
    let (mut checked, mut stamp, mut self_checked) = (f64::NEG_INFINITY, None, monotonic());
    // cca 换了版本、辅助程序的文件没了：收场以后经 cca 重新执行自己（进程号不变，launchd 照旧管着）
    let mut again = false;
    loop {
        let now = monotonic();
        retired.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        for l in links.iter_mut() {
            l.reap(now);
        }
        if now - checked >= CHECK_EVERY {
            checked = now;
            let s = peers::stamp();
            if stamp != Some(s) {
                stamp = Some(s);
                let mut want: Vec<HubKind> = Vec::new();
                for p in launch_peers(None) {
                    if !want.contains(&p.hub) {
                        want.push(p.hub);
                    }
                }
                if want.is_empty() {
                    break;
                }
                for l in links.iter_mut().filter(|l| !want.contains(&l.hub)) {
                    retired.extend(l.retire());
                }
                links.retain(|l| want.contains(&l.hub));
                for h in want {
                    match links.iter_mut().find(|l| l.hub == h) {
                        Some(l) => l.retry.next = l.retry.next.min(now), // 配对表变了：用不了的那台也马上再试
                        None => {
                            let sid =
                                sids.str(&h.team()).filter(|s| peers::hex32(s)).map_or_else(|| token_hex(16), |s| s.to_string());
                            sids.set(h.team(), sid.as_str());
                            links.push(HubLink { hub: h, sid, child: None, retry: Respawn::default() });
                        }
                    }
                }
                save_state(me, started, &sids);
            }
            if std::env::current_exe().map_or(true, |p| !p.exists()) {
                again = true;
                break;
            }
            for l in links.iter_mut().filter(|l| l.child.is_none() && now >= l.retry.next) {
                l.spawn(now);
            }
        }
        if now - self_checked >= SELF_EVERY {
            self_checked = now;
            let s = peers::stamp_of(&SELF_PATH);
            if s.is_some() && s != self_was {
                again = true;
                break;
            }
        }
        let mut fds = [libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 }];
        // SAFETY: fds 是有效的数组；被信号打断就是要它马上回来
        unsafe { libc::poll(fds.as_mut_ptr(), 1, 1000) };
        if fds[0].revents != 0 || sig::pending() {
            let got = sig::take(wake);
            if [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT].into_iter().any(|s| got.has(s)) {
                break;
            }
        }
    }
    // 收场：转接进程的标准输入关了，它们告诉服务器、自己退出；等一会儿
    for l in links.iter_mut() {
        retired.extend(l.retire());
    }
    let end = monotonic() + 3.0;
    while !retired.is_empty() && monotonic() < end {
        sleep(0.05);
        retired.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }
    for c in retired.iter_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if again {
        drop(lock);
        let e = Command::new(&*SELF_PATH).args(["remote", "launcher", "serve"]).exec();
        return die(format!("重新运行 {} 失败：{e}", *SELF_PATH));
    }
    Ok(())
}

/// `remote-launcher stop`：结束在跑的后台进程（卸掉 crontab 的那两行以后，bash 用它）。没在跑不算错
fn stop_running() -> R<()> {
    if !running() {
        return Ok(());
    }
    let st = state();
    let (pid, started) = (state_int(&st, "pid"), state_int(&st, "started"));
    // 进程号可能已经给了别的进程：开始的时间也要对得上
    if pid > 1 && started != 0 && proc_starts(&[pid]).get(&pid) == Some(&started) {
        // SAFETY: 给记着的那个进程（核对过是它）发 SIGTERM
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        let end = monotonic() + 5.0;
        while running() && monotonic() < end {
            sleep(0.1);
        }
    }
    Ok(())
}

/// macOS 的 LaunchAgent（~/Library/LaunchAgents/local.cca.remote-launcher.plist）：登录以后起后台进程；它出错退出了过 30 秒再起，
/// 正常退出（没有允许远程启动的手机了）不再起；它起的会话不跟着它结束（AbandonProcessGroup）。PATH、LANG 用装的时候这个 shell 的
/// （launchd 给的 PATH 里没有 ~/.local/bin、Homebrew：找不到 claude）；CCA_HOME 换过的也带上
fn plist_text(cca: &str) -> R<String> {
    let mut env = P::dict(Vec::<(String, P)>::new());
    for k in ["PATH", "LANG", "CCA_HOME"] {
        if let Some(v) = std::env::var(k).ok().filter(|v| !v.is_empty()) {
            env.set(k, P::Str(v));
        }
    }
    let argv = [cca, "remote", "launcher", "serve"].into_iter().map(|s| P::Str(s.to_string())).collect();
    let doc = P::dict([
        ("Label", P::Str(LABEL.into())),
        ("ProgramArguments", P::Array(argv)),
        ("RunAtLoad", P::Bool(true)),
        ("KeepAlive", P::dict([("SuccessfulExit", P::Bool(false))])),
        ("ThrottleInterval", P::Int(30)),
        ("AbandonProcessGroup", P::Bool(true)),
        ("EnvironmentVariables", env),
    ]);
    Ok(String::from_utf8_lossy(&plist::dumps_xml(&doc)?).trim_end().to_string())
}

/// `remote-launcher status`：给人看的
fn status() -> R<()> {
    let phones = launch_peers(None);
    if phones.is_empty() {
        crate::out!("远程启动没开：没有允许远程启动的配对（cca remote allow 名字 launch on：手机、配对过的电脑都行）");
    } else {
        let names: Vec<String> = phones.iter().map(|p| clean(&p.name, 40)).collect();
        crate::out!("远程启动开着：{} 台配对的设备能用模板新建会话（{}）", phones.len(), names.join("、"));
    }
    if running() {
        let pid = state_int(&state(), "pid");
        crate::out!("后台进程：在运行（进程号 {pid}）");
    } else if !phones.is_empty() {
        crate::out!("后台进程：没在运行（cca remote launcher serve 起它；装上了的话登录以后自己起来）");
    }
    crate::out!("模板：{} 个（cca remote template 看）", template::load().len());
    let last = std::fs::read_to_string(file("launch.log")).ok().and_then(|t| t.lines().last().map(str::to_string));
    if let Some(last) = last {
        crate::out!("最近一次：{last}");
    }
    Ok(())
}

/// `remote-launcher log`：哪台手机什么时候用哪个模板起了会话（launch.log.1 在前）
fn print_log() -> R<()> {
    let mut any = false;
    for f in [format!("{}.1", file("launch.log")), file("launch.log")] {
        if let Some(text) = std::fs::read_to_string(&f).ok().filter(|t| !t.trim().is_empty()) {
            crate::out!("{}", text.trim_end());
            any = true;
        }
    }
    if !any {
        crate::out!("还没有配对的设备远程启动过会话");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 转接进程（一台服务器一个）
// ---------------------------------------------------------------------------

/// 这台服务器上允许远程启动的手机，各带配对的密钥；读不出密钥的另外交出来
fn allowed_now(hub: HubKind) -> (Vec<Allowed>, Vec<Peer>) {
    link::with_secrets(launch_peers(Some(hub)), PEERS_MAX)
}

/// ANNOUNCE：同一份说明给每台手机各封一份
fn announce(sid: &str, allowed: &[Allowed], doc: &Dict) -> Vec<u8> {
    let blobs = Dict::new();
    for a in allowed {
        if let Ok(b) = crypto::launch_seal(&a.secret, &a.peer.pid, sid, doc) {
            blobs.set(a.peer.pid.as_str(), b);
        }
    }
    ctl_frame(relay::F_ANNOUNCE, json::dumps(&V::Dict(blobs), json::Fmt::new().utf8().compact()).as_bytes())
}

/// 限流：最近一分钟里起成了会话的时候、每台手机上一次接进来的时候（分出子进程的上限在 link.rs 的 `Link::join`）
#[derive(Default)]
struct Gate {
    launched: VecDeque<f64>,
    joined: HashMap<String, f64>,
}

impl Gate {
    /// 一台手机接进来（分得出子进程的都接）：交出这一次是不是起得太勤了——同一台手机两次接进来隔得不够 JOIN_GAP 秒，或者一分钟里
    /// 起成的会话已经有 LAUNCHES_PER_MIN 个——是的话接上以后回它 `rate`（不理它的话，手机等到服务器说这边没来，说的原因就错了）。
    /// 隔得够的记下这一次的时候
    fn admit(&mut self, pid: &str) -> bool {
        let now = monotonic();
        while self.launched.front().is_some_and(|t| now - t > 60.0) {
            self.launched.pop_front();
        }
        let again = self.joined.get(pid).is_some_and(|t| now - t < JOIN_GAP);
        if !again {
            self.joined.insert(pid.to_string(), now);
        }
        again || self.launched.len() >= LAUNCHES_PER_MIN
    }
}

/// `remote-link --launcher Team编号 会话编号`（后台进程起的；link.rs 的 cmd_remote_link 转过来）
pub fn cmd_launcher_link(args: &[String]) -> R<()> {
    arity("remote-link --launcher", args, 2, 2)?;
    let (Some(tag), true) = (TeamTag::new(&args[0]), peers::hex32(&args[1])) else {
        return die("用法：remote-link --launcher Team编号 会话编号");
    };
    let hub = HubKind::Team(tag);
    let Ok(srv) = relay::server_of(hub)? else { return exit(link::EXIT_OFF) };
    let sid = args[1].as_str();
    let mut link = Link::new(srv, sid)?;
    let mut gate = Gate::default();
    let mut unread: Vec<String> = Vec::new(); // 记过「读不出密钥」的手机：读出来以前不再记
    let (mut tstamp, mut built) = (None, f64::NEG_INFINITY);
    let (mut doc, mut doc_text) = (Dict::new(), String::new());
    let mut buf = [0u8; 256];
    loop {
        let now = monotonic();
        for code in link.reap() {
            if code == EXIT_LAUNCHED {
                gate.launched.push_back(now);
            }
        }
        // 配对表变了：不该再接着的子进程结束掉；手机变了要换一条控制连接（新的票据上是新的那几台）
        if link.refresh(now, || allowed_now(hub)) {
            // 读不出配对密钥的手机（link.rs 隔一阵再读）：它上面没有「新建会话」，在 launch.log 里说一句（cca remote launcher 看得到）
            for p in link.missing.iter().filter(|p| !unread.contains(&p.pid)) {
                log_line(&p.name, "读不出这个配对的密钥（钥匙串），过一会儿再试：读出来以前它上面没有「新建会话」");
            }
            unread = link.missing.iter().map(|p| p.pid.clone()).collect();
            // 模板变了（隔一阵也重新算一遍）：说明不一样了就在原来的控制连接上再说一遍（服务器整份换掉）
            let t = template::stamp();
            if tstamp != Some(t) || now - built >= DOC_EVERY {
                (tstamp, built) = (Some(t), now);
                let fresh = template::launch_doc(hub);
                let text = json::dumps(&V::Dict(fresh.clone()), json::Fmt::new().utf8().compact());
                if text != doc_text {
                    (doc, doc_text) = (fresh, text);
                    if let Some(c) = link.ctl.as_mut() {
                        c.out.extend(announce(sid, &link.allowed, &doc));
                    }
                }
            }
        }
        if !link.connect_if_due(now, |allowed| announce(sid, allowed, &doc))? {
            return exit(link::EXIT_OFF);
        }
        let Some(woke) = link.wait() else { break };
        // 标准输入读到头：后台进程不在了（或者不要这台服务器了）
        if woke.stdin {
            match pty::read_some(0, &mut buf) {
                Some(Ok(n)) if n > 0 => {}
                None => {}
                _ => break,
            }
        }
        for p in &woke.joins {
            link.join(p, |a| gate.admit(&a.peer.pid), |j, limited| kid_serve(j, hub, limited));
        }
    }
    link.finish();
    Ok(())
}

/// 子进程里：一台手机接进来要起会话。握手、等它说起哪个、查、起、回话、记一笔。退出码：起成了是 EXIT_LAUNCHED
fn kid_serve(j: &Join, hub: HubKind, limited: bool) -> i32 {
    let Ok(mut tun) = j.tunnel() else { return 1 };
    let Some(raw) = wait_ask(&mut tun, j.parent) else { return 1 };
    let mut what = String::new();
    let result = match Ask::parse(&raw) {
        Some(ask) => {
            what = ask.id.clone();
            launch(hub, &j.a.peer.pid, &ask, limited, &mut what)
        }
        None => Err("bad"),
    };
    let reply = match &result {
        Ok(sid) => dict! { "v" => 1, "ok" => true, "sid" => sid.as_str() },
        Err(code) => dict! { "v" => 1, "ok" => false, "code" => *code, "text" => fail_text(code) },
    };
    log(&j.a.peer.name, &what, &result);
    tun.send(&local::frame(local::LAUNCHED, json::dumps(&V::Dict(reply), json::Fmt::new().utf8().compact()).as_bytes()));
    tun.drain(2.0);
    tun.close();
    if result.is_ok() {
        EXIT_LAUNCHED
    } else {
        0
    }
}

/// 照手机说的起会话：每一样都现读、现查（配对表、模板、账号表、文件夹、参数、这台服务器上的会话数）。返回新会话的编号，
/// 不行是 LAUNCHED 的 code。what：记进日志的模板名字
fn launch(hub: HubKind, pid: &str, ask: &Ask, limited: bool, what: &mut String) -> Result<String, &'static str> {
    if !launch_peers(Some(hub)).iter().any(|p| p.pid == pid) {
        return Err("denied");
    }
    if limited {
        return Err("rate");
    }
    let Some(t) = template::load().into_iter().find(|t| t.id == ask.id) else { return Err("no_template") };
    what.clone_from(&t.name);
    if t.digest() != ask.digest {
        return Err("changed");
    }
    let Some((key, name)) = t.account_now() else { return Err("no_account") };
    if relay::session_hub(&t.tool, &key) != Some(hub) {
        return Err("other_hub");
    }
    if !path::isdir(&t.dir) {
        return Err("no_dir");
    }
    if !template::args_problems(t.tool(), &t.args, &t.dir).is_ok_and(|p| p.is_empty()) {
        return Err("bad_args");
    }
    if ask.line.is_some() && !t.line_ok() {
        return Err("bad");
    }
    let here = local::sessions(false).iter().filter(|s| relay::session_hub(&s.meta.tool, &s.meta.key) == Some(hub)).count();
    if here >= SESSIONS_MAX {
        return Err("busy");
    }
    spawn_session(&t, &key, &name, ask).ok_or("spawn")
}

/// 等手机说要起哪个（LAUNCH 帧；别的帧不理：启动器不是会话），最多 ASK_WAIT 秒；断了、分出它的进程不在了是 None
fn wait_ask(tun: &mut Tunnel, parent: libc::pid_t) -> Option<Vec<u8>> {
    let end = monotonic() + ASK_WAIT;
    let mut dec = Decoder::default();
    loop {
        loop {
            match dec.next() {
                Ok(Some((local::LAUNCH, p))) => return Some(p),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(()) => return None,
            }
        }
        let left = end - monotonic();
        // SAFETY: getppid 没有参数、不会失败
        if left <= 0.0 || unsafe { libc::getppid() } != parent || !tun.tick() || tun.flush().is_err() {
            return None;
        }
        let mut p = libc::pollfd { fd: tun.fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: p 是有效的一项
        unsafe { libc::poll(&mut p, 1, (left * 1000.0).clamp(1.0, 1000.0) as i32) };
        if p.revents != 0 {
            match tun.recv() {
                Ok(Some(data)) => dec.push(&data),
                _ => return None,
            }
        }
    }
}

/// 手机要起的（LAUNCH 的内容：`{"v":1,"t":模板编号,"h":摘要,"c":列,"r":行,"l":开场白}`）
#[derive(Debug, PartialEq)]
struct Ask {
    id: String,
    digest: String,
    size: Size,
    line: Option<String>,
}

impl Ask {
    fn parse(p: &[u8]) -> Option<Ask> {
        if p.len() > ASK_MAX {
            return None;
        }
        let d = json::loads(std::str::from_utf8(p).ok()?).ok()?.as_dict()?;
        // 缺了哪一项都是看不懂（回 `bad`）：用 `get`（`at` 取不存在的键会崩，手机那头就收不到回话了）
        let int = |k: &str| d.get(k).and_then(|v| v.as_int());
        if int("v") != Some(1) {
            return None;
        }
        let id = d.str("t").filter(|t| peers::lower_hex(t, 8))?.to_string();
        let digest = d.str("h").filter(|h| peers::lower_hex(h, 16))?.to_string();
        let cols = u16::try_from(int("c")?).ok().filter(|c| (20..=500).contains(c))?;
        let rows = u16::try_from(int("r")?).ok().filter(|r| (5..=300).contains(r))?;
        let line = match d.get("l") {
            None | Some(V::None) => None,
            Some(v) => {
                let s = v.as_str()?;
                if s.chars().count() > LINE_MAX || s.chars().any(char::is_control) || s.trim().is_empty() {
                    return None;
                }
                Some(s.to_string())
            }
        };
        Some(Ask { id, digest, size: Size { cols, rows }, line })
    }
}

/// 起一个无头的会话、等它的 socket 起来。返回会话编号；没起来是 None（起了却 SPAWN_WAIT 秒还没起来的结束掉）
fn spawn_session(t: &Template, key: &str, name: &str, ask: &Ask) -> Option<String> {
    let exe = std::env::current_exe().ok()?.to_string_lossy().into_owned();
    let sid = token_hex(16);
    let mut argv: Vec<String> = vec![exe, "remote-host".into(), "--headless".into(), "--sid".into(), sid.clone()];
    argv.extend(["--size".into(), format!("{}x{}", ask.size.cols, ask.size.rows), t.tool.clone(), key.into(), name.into()]);
    argv.extend(["--".into(), SELF_PATH.to_string()]);
    if t.tool != "claude" {
        argv.push(t.tool.clone());
    }
    argv.extend([format!("@{key}"), "--".into()]);
    argv.extend(t.args.iter().cloned());
    if let Some(l) = &ask.line {
        argv.extend(["--".into(), l.clone()]);
    }
    let mut env: Vec<(String, String)> = proc::environ().into_iter().filter(|(k, _)| KEEP_ENV.contains(&k.as_str())).collect();
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("CCA_REMOTE".into(), "1".into()));
    env.push(("CCA_REMOTE_HOSTED".into(), sid.clone()));
    let cmd = proc::Cmd::new(&argv).env(env).cwd(t.dir.as_str()).stdin(In::Null).stdout(Out::Null).stderr(Out::Null);
    let mut child = cmd.new_session().spawn().ok()?;
    let (dir, sock) = (local::dir(), format!("{}.sock", local::short(&sid)));
    let end = monotonic() + SPAWN_WAIT;
    while monotonic() < end {
        if !matches!(child.try_wait(), Ok(None)) {
            return None; // 退出了：没起来
        }
        if let Asked::Alive(st) = local::ask_status(&dir, &sock) {
            if st.meta.sid == sid {
                return Some(sid); // 起来了：不等它（这个子进程马上退出，它交给 init）
            }
        }
        sleep(0.05);
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// 给手机、电脑看的原因（LAUNCHED 的 text；新的 cca、App 照 code 自己说，这句给不认识这个 code 的）
fn fail_text(code: &str) -> &'static str {
    match code {
        "denied" => "这台设备现在不能远程启动会话（在那台机器上运行 cca remote allow 它的名字 launch on）",
        "no_template" => "那台机器上没有这个模板了：刷新一下",
        "changed" => "模板在那台机器上改过了：刷新一下再选",
        "no_account" => "模板的账号在那台机器的账号表里没有了",
        "no_dir" => "模板的文件夹在那台机器上没有了",
        "bad_args" => "模板的参数在那台机器上用不了（启动前的检查过不了）",
        "other_hub" => "模板的账号现在走别的 Team 的服务器：到那个 Team 里起",
        "busy" => "那台机器上开着的会话太多了：先结束几个",
        "rate" => "起得太勤了：过一会儿再试",
        "spawn" => "会话没起来（在那台机器上运行 cca remote launcher log 看看）",
        _ => "请求看不懂：更新一下 App",
    }
}

/// launch.log 记一行：什么时候、哪台手机、哪个模板、起没起成（不记开场白、参数）
fn log(peer: &str, what: &str, result: &Result<String, &'static str>) {
    let tail = match result {
        Ok(sid) => format!("起了会话 {}", local::short(sid)),
        Err(code) => format!("没起（{code}）"),
    };
    log_line(peer, &format!("{}  {tail}", clean(what, 40)));
}

/// launch.log 记一行 `时间  手机  text`。到 LOG_MAX 换一个文件（launch.log.1）
fn log_line(peer: &str, text: &str) {
    let f = file("launch.log");
    if std::fs::metadata(&f).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = std::fs::rename(&f, format!("{f}.1"));
    }
    let when = strftime_local("%Y-%m-%d %H:%M:%S", time());
    let line = format!("{when}  {}  {text}\n", clean(peer, 40));
    if let Ok(mut fh) = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&f) {
        let _ = fh.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(j: &str) -> Option<Ask> {
        Ask::parse(j.as_bytes())
    }

    #[test]
    fn asks() {
        let ok = ask(r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24}"#).unwrap();
        assert_eq!((ok.id.as_str(), ok.size, ok.line), ("0123abcd", Size { cols: 80, rows: 24 }, None));
        let line = ask(r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":20,"r":300,"l":"看看 README"}"#).unwrap();
        assert_eq!(line.line.as_deref(), Some("看看 README"));
        assert!(ask(r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":null}"#).unwrap().line.is_none());
        for bad in [
            r#"{"v":2,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123ABCD","h":"0123456789abcdef","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcde","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":19,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":301}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":"  "}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":"a\u001b[2J"}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":7}"#,
            "[]",
            // 缺了哪一项都是看不懂（不能崩：崩了手机那头收不到回话）
            r#"{"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24}"#,
            r#"{"v":1,"h":"0123456789abcdef","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80}"#,
            r#"{"v":"1","t":"0123abcd","h":"0123456789abcdef","c":80,"r":24}"#,
            r#"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":"80","r":24}"#,
            "{}",
        ] {
            assert!(ask(bad).is_none(), "{bad}");
        }
        let long = format!(r#"{{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":"{}"}}"#, "字".repeat(LINE_MAX + 1));
        assert!(ask(&long).is_none());
        let pad = format!(r#"{{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24{}}}"#, " ".repeat(ASK_MAX));
        assert!(ask(&pad).is_none(), "太长");
    }

    #[test]
    fn rapid_joins_get_rate() {
        let mut g = Gate::default();
        assert!(!g.admit("p1"), "头一次");
        assert!(g.admit("p1"), "同一台 3 秒内又来：接上以后回 rate");
        assert!(!g.admit("p2"), "别的手机不受影响");
        // 隔得不够的那一次不算：从上一次接上的时候算
        g.joined.insert("p1".into(), monotonic() - JOIN_GAP - 0.1);
        assert!(!g.admit("p1"), "隔够了");
        for _ in 0..LAUNCHES_PER_MIN {
            g.launched.push_back(monotonic());
        }
        assert!(g.admit("p3"), "一分钟里起成的已经够多了");
        g.launched.iter_mut().for_each(|t| *t -= 61.0);
        assert!(!g.admit("p4") && g.launched.is_empty(), "一分钟以前的不算");
    }

    #[test]
    fn every_code_has_words() {
        for code in ["denied", "no_template", "changed", "no_account", "no_dir", "bad_args", "other_hub", "busy", "rate", "spawn"] {
            assert_ne!(fail_text(code), fail_text("bad"), "{code}");
        }
    }

    #[test]
    fn the_agent_plist() {
        let text = plist_text("/Users/a b/.cca/bin/cca").unwrap();
        let p = plist::loads(text.as_bytes()).unwrap();
        assert_eq!(p.get("Label").and_then(P::as_str), Some(LABEL));
        let argv: Vec<&str> = match p.get("ProgramArguments") {
            Some(P::Array(a)) => a.iter().filter_map(P::as_str).collect(),
            _ => Vec::new(),
        };
        assert_eq!(argv, ["/Users/a b/.cca/bin/cca", "remote", "launcher", "serve"]);
        assert_eq!(p.get("KeepAlive").and_then(|k| k.get("SuccessfulExit")), Some(&P::Bool(false)));
        assert_eq!(p.get("AbandonProcessGroup"), Some(&P::Bool(true)));
    }
}
