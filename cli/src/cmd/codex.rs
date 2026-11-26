//! Codex 和 agy 的登录身份、codex 的探测和 app-server（`agy_claims` 到 `codex_fix_rollouts`）：reset-consume、login-flow。
//!
//! 这一块都移植了：登录身份（`agy_claims`、`codex_claims`）、ChatGPT 套餐的叫法和档位、`codex_probe`、app-server 的
//! JSON-RPC（`Rpc`：查用量、重置额度、登录）、`codex_fix_rollouts`。Claude 查用量也是“起一个子进程、一行一行读它的回答”，
//! 用的是这里的 `Piped`。

use super::keyshare::is_num;
use super::settings::{read_toml_config, ConfigReadError};
use super::usage::{capitalize, div_small, float_arg, hashable, py_max, type_error};
use super::{arg, arity, Cmd};
use crate::core::base::{str_or_empty, CCA_HOME, EMAIL_RE, SELF_PATH};
use crate::core::model::{adir, cdir};
use crate::core::tool::{codex, AGY_TOKEN, CODEX_CLIENT, CODEX_FLAGS};
use crate::py::base64::urlsafe_b64decode;
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::json;
use crate::py::num::floordiv;
use crate::py::obj::{str_list, Dict, V};
use crate::py::path;
use crate::py::proc::{self, env_or_empty, Cmd as Proc, In, Out, ProcError};
use crate::py::rand::uuid4;
use crate::py::text;
use crate::py::time::{monotonic, sleep, time};
use crate::py::{die, exit, Fail, R};
use crate::{dict, err, out, re};
use std::os::unix::fs::MetadataExt;
use std::rc::Rc;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("reset-consume", cmd_reset_consume), Cmd::all("login-flow", cmd_login_flow)];

// --- Codex：登录身份、app-server（登录、查用量）、会话库里的路径 ----------------------------------------------

/// agy 账号登录的身份：账号目录里 token 文件（AGY_TOKEN，JSON）的 id_token_email 和 user_tier 两项（只读这两项，不碰
/// access / refresh token，也不保存、不转发）。token 文件在、读不出邮箱时（格式和 1.5.5 的不一样了）用 cca 登录时记下的
/// （agy 登录成功时自己说的 Logged in as，见 agy-login）。套餐：1.5 的 token 里没有 user_tier 了，先用 cca 的状态栏命令上次记下的
/// （`agy_plan_seen`：叫法，邮箱要对得上），没有再用 token 里的档位 ID。没登录、读不了都返回 {}；登录了是 {'email', 'tier'}
pub fn agy_claims(key: &str) -> Dict {
    let token = path::join(&cdir(key), AGY_TOKEN);
    let tier = |email: &str, id: Option<Rc<str>>| match super::agy::agy_plan_seen(key, email) {
        seen if !seen.is_empty() => V::from(seen),
        _ => id.map_or_else(|| V::from(""), V::Str),
    };
    if let Some(V::Dict(j)) = load_quiet(&token) {
        if let Some(email) = j.str("id_token_email").filter(|e| !e.is_empty()) {
            return dict! { "email" => &email, "tier" => tier(&email, j.str("user_tier")) };
        }
    }
    if !path::isfile(&token) {
        return Dict::new();
    }
    match load_quiet(&path::join(&adir(key), "agy-login.json")).and_then(|r| r.as_dict()).and_then(|r| r.str("email")) {
        Some(e) if EMAIL_RE.matches(&e) => dict! { "email" => &e, "tier" => tier(&e, None) },
        _ => Dict::new(),
    }
}

/// Codex 账号登录的身份：auth.json 里 id_token 的声明，只取邮箱、套餐、工作空间和用户 id（不验签名；不碰 access / refresh
/// token，也不保存、不转发）。没登录、用 API key 登录、读不了都返回 {}；登录了是 {'email', 'plan', 'account', 'user'}。
/// path_：别处的 auth.json（折合金额认 ~/.codex 里直接登录的），给了它就不看 key
pub fn codex_claims(key: &str, path_: Option<&str>) -> Dict {
    let auth_json = match path_.filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => path::join(&cdir(key), "auth.json"),
    };
    claims_from(load_quiet(&auth_json))
}

/// codex_claims 读进来以后的那一步：j 是 auth.json 的内容
fn claims_from(j: Option<V>) -> Dict {
    let tok = j.and_then(|j| j.as_dict()).and_then(|j| j.dict("tokens")).and_then(|tokens| tokens.str("id_token"));
    let Some(tok) = tok.filter(|tok| tok.matches('.').count() == 2) else { return Dict::new() };
    let p = tok.split('.').nth(1).unwrap_or("");
    let padded = format!("{}{}", p, "=".repeat((4 - text::len(p) % 4) % 4));
    let Ok(raw) = urlsafe_b64decode(&padded) else { return Dict::new() };
    let Ok(payload) = String::from_utf8(raw) else { return Dict::new() };
    let Ok(V::Dict(d)) = json::loads(&payload) else { return Dict::new() };
    let prof = d.dict("https://api.openai.com/profile").unwrap_or_default();
    let auth = d.dict("https://api.openai.com/auth").unwrap_or_default();
    let email = d.get("email").filter(|e| e.truthy()).or_else(|| prof.get("email"));
    dict! {
        "email" => email.and_then(|e| e.rc_str()).map_or_else(|| V::from(""), V::Str),
        "plan" => str_or_empty(&auth.get("chatgpt_plan_type")),
        "account" => str_or_empty(&auth.get("chatgpt_account_id")),
        "user" => str_or_empty(&auth.get("chatgpt_user_id").filter(|u| u.truthy()).or_else(|| auth.get("user_id"))),
    }
}

/// ChatGPT 套餐的叫法，和 codex 的 /status 一样（codex-rs/tui/src/subscription.rs 里 SubscriptionDisplay::Status
/// 那一列；OpenAI 加了新套餐照着补）。用量查询的 planType 和 id_token 的 chatgpt_plan_type 都是这些值，hc、education
/// 是 id_token 里的别名。第三项是档位（_gui state 的 plan_tier；原来的 CODEX_TIERS，键和叫法一一对应）
const CODEX_PLANS: &[(&str, &str, &str)] = &[
    ("free", "Free", "entry"),
    ("go", "Go", "entry"),
    ("plus", "Plus", "standard"),
    ("pro", "Pro 200", "advanced"),
    ("prolite", "Pro 100", "advanced"),
    ("promax", "Pro 500", "premium"),
    ("team", "Business", "standard"),
    ("self_serve_business_usage_based", "Business", "standard"),
    ("self_serve_business_prolite", "Business Premium", "advanced"),
    ("business", "Enterprise", "standard"),
    ("enterprise", "Enterprise", "standard"),
    ("hc", "Enterprise", "standard"),
    ("ent26", "Enterprise", "standard"),
    ("enterprise_cbp_usage_based", "Enterprise", "standard"),
    ("enterprise_cbp_automation", "Enterprise (Automation)", "standard"),
    ("edu", "Edu", "standard"),
    ("education", "Edu", "standard"),
    ("edu_plus", "Edu Plus", "standard"),
    ("edu_pro", "Edu Pro", "standard"),
    ("sci", "Academic Researchers", "standard"),
    ("sci_plus", "Academic Researchers Plus", "standard"),
];

/// `str(plan or '').strip().lower()`
fn codex_plan_key(plan: &V) -> String {
    text::strip(&str_or_empty(&Some(plan.clone()))).to_lowercase()
}

/// ChatGPT 的套餐：Plus、Pro 100 / 200 / 500、Business、Enterprise…；表里没有的新套餐只把首字母大写
pub fn codex_plan_label(plan: &V) -> String {
    let p = codex_plan_key(plan);
    let name = CODEX_PLANS.iter().find(|(k, _, _)| *k == p).map_or_else(|| capitalize(&p), |(_, name, _)| name.to_string());
    if name.is_empty() {
        "—".into()
    } else {
        name
    }
}

/// ChatGPT 套餐的档位，按原始值查；表里没有的是 ''（不猜）
pub fn codex_plan_tier(plan: &V) -> &'static str {
    let p = codex_plan_key(plan);
    CODEX_PLANS.iter().find(|(k, _, _)| *k == p).map_or("", |(_, _, tier)| tier)
}

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
    // int(st.st_mtime)：st_mtime 是小数（纳秒差一点满一秒时进位成下一秒），再往 0 取整
    let mtime = crate::py::shutil::stat_time(st.mtime(), st.mtime_nsec()) as i64;
    let mut sig = format!("{}|{}|{}", real, st.size(), mtime);
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
        match Proc::new(argv)
            .stdin(In::Null)
            .stdout(Out::Capture)
            .stderr(Out::Null)
            .env(env.clone())
            .cwd(master.clone())
            .timeout(30.0)
            .run()
        {
            Ok(d) => (d.rc, d.out()),
            Err(_) => (1, String::new()),
        }
    };
    let (rc, out) = run(&["features", "list"]);
    if rc != 0 || text::strip(&out).is_empty() {
        return full;
    }
    let known: Vec<&str> =
        text::splitlines(&out).into_iter().filter(|l| !text::strip(l).is_empty()).map(|l| text::split_ws(l)[0]).collect();
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

// --- 起一个子进程、一行一行读它的回答（Claude 的 get_usage、codex 的 app-server 都是这样）--------------------------

/// `tempfile.TemporaryFile()`：临时目录里一个马上删掉名字的文件，只有这个进程和它起的子进程拿着
pub(super) fn temp_file() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    const NAMES: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let base = fs::gettempdir();
    for _ in 0..100 {
        let name: String = crate::py::rand::urandom(8).iter().map(|b| NAMES[*b as usize % NAMES.len()] as char).collect();
        let p = path::join(&base, &format!("tmp{name}"));
        match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&p) {
            Ok(f) => {
                let _ = std::fs::remove_file(&p);
                return Ok(f);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "No usable temporary file name found"))
}

/// 程序名是空的时候 Python 报的 errno：它把 PATH 里每个目录拼上空的名字去 exec（目录不能执行，EACCES），报第一个不是
/// “没有这个文件 / 不是目录”的错，都是就报最后一个；Rust 的 execvp 遇到空名字直接是 ENOENT
fn empty_program_errno(cwd: &str) -> i32 {
    let dirs = proc::getenv("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    let mut last = libc::ENOENT;
    for dir in dirs.split(':') {
        let at = path::join(cwd, dir); // 子进程先换了目录才 exec：相对的目录从 cwd 算起
        last = if dir.is_empty() {
            libc::ENOENT
        } else if path::isdir(&at) {
            return libc::EACCES;
        } else if path::exists(&at) {
            libc::ENOTDIR
        } else {
            libc::ENOENT
        };
    }
    last
}

/// 起子进程不成（OSError）时 `str(e)` 的样子，带着文件名：Python 的子进程先 chdir 再 exec，chdir 不成报的是 cwd，
/// exec 不成报的是程序（照给的写法，不是找到的路径）
fn popen_error(e: &std::io::Error, program: &str, cwd: &str) -> Fail {
    if e.raw_os_error().is_none() {
        return Fail::Value("embedded null byte".into()); // 参数里有 0 字节：原来是 ValueError，不是 OSError
    }
    let enterable = std::ffi::CString::new(cwd).is_ok_and(|c| {
        // SAFETY: c 是以 0 结尾的路径
        path::isdir(cwd) && unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
    });
    if !enterable {
        return Fail::Other("OSError", vec![fs::open_error_text(e, cwd)]);
    }
    if program.is_empty() {
        return Fail::Other(
            "OSError",
            vec![fs::open_error_text(&std::io::Error::from_raw_os_error(empty_program_errno(cwd)), program)],
        );
    }
    Fail::Other("OSError", vec![fs::open_error_text(e, program)])
}

/// 队列里取到的：一行（带着结尾的换行）、读到头了（子进程关了 stdout）、到时间了还没有
pub(super) enum Got {
    Line(Vec<u8>),
    Eof,
    Empty,
}

/// `subprocess.Popen(argv, stdin=PIPE, stdout=PIPE, stderr=临时文件, cwd=…)`，加上一条把它的 stdout 一行一行送进队列的线程
/// （原来的 pump）。`py::proc::Cmd` 接不了“stderr 进文件”，所以这里自己起；子进程照样不带 3 号以上的文件描述符
pub(super) struct Piped {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    err: std::fs::File,
    lines: std::sync::mpsc::Receiver<Option<Vec<u8>>>,
    rc: Option<i32>,
}

/// 等的时间比这长（Python 的锁按微秒记，最多 LLONG_MAX / 1000 微秒），Python 直接报 OverflowError
const TIMEOUT_MAX: f64 = 9223372036.854775;

impl Piped {
    /// 起不来是 `Fail::Other("OSError", [str(e)])`（说明里带着文件名）
    pub(super) fn spawn<S: AsRef<str>>(argv: &[S], cwd: &str, err: std::fs::File) -> R<Piped> {
        use std::process::{Command, Stdio};
        let program = argv[0].as_ref();
        let mut c = Command::new(program);
        c.args(argv[1..].iter().map(|a| a.as_ref()));
        c.current_dir(cwd);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(err.try_clone()?));
        // 和 py::proc::Cmd 一样（Python 的 close_fds=True）：bash 那边开着的 3 号（提问的终端）、9 号（启动锁）不留给子进程。
        // 在这边标、不在子进程里标：见 cloexec_inherited（多线程的进程 fork 时子进程可能被杀）
        proc::cloexec_inherited();
        let mut child = c.spawn().map_err(|e| popen_error(&e, program, cwd))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout 接的是管道");
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut reader = std::io::BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Some(line)).is_err() {
                            return;
                        }
                    }
                    // 读不了：原来是这条线程里没人接的异常，队列里等不到“读到头了”
                    Err(e) => {
                        err!("cca-core: 读子进程的输出出错：{e}");
                        return;
                    }
                }
            }
            let _ = tx.send(None);
        });
        Ok(Piped { child, stdin, err, lines, rc: None })
    }

    /// `p.stdin.write(data); p.stdin.flush()`：写不进去（那头不读了）、已经关了都是 false
    pub(super) fn write(&mut self, data: &[u8]) -> bool {
        use std::io::Write;
        match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(data).and_then(|_| stdin.flush()).is_ok(),
            None => false,
        }
    }

    /// `q.get(timeout=max(end - time.time(), 0.01))`。interruptible：等的时候留意 Ctrl-C / SIGTERM（login-flow），
    /// 来了是 `Fail::Other("KeyboardInterrupt", …)`
    pub(super) fn get(&mut self, end: f64, interruptible: bool) -> R<Got> {
        use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
        let timeout = py_max(end - time(), 0.01);
        if interruptible && interrupt::take() {
            return Err(Fail::Other("KeyboardInterrupt", Vec::new()));
        }
        // 队列里已经有的直接拿（Python 的 Queue.get 也是没有才等，这时等多久写得对不对它都不看）
        match self.lines.try_recv() {
            Ok(Some(line)) => return Ok(Got::Line(line)),
            Ok(None) => return Ok(Got::Eof),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }
        if timeout > TIMEOUT_MAX {
            return Err(Fail::Other("OverflowError", vec!["timeout value is too large".into()]));
        }
        // 超时写成 NaN：Python 里“还剩多久”永远比不出到没到，于是一直等下去（空转着等），不出错也不超时
        let deadline = if timeout.is_nan() { f64::INFINITY } else { monotonic() + timeout };
        loop {
            if interruptible && interrupt::take() {
                return Err(Fail::Other("KeyboardInterrupt", Vec::new()));
            }
            let left = deadline - monotonic();
            if left <= 0.0 {
                return Ok(Got::Empty);
            }
            let step = left.min(if interruptible { 0.02 } else { 3600.0 });
            match self.lines.recv_timeout(std::time::Duration::from_secs_f64(step)) {
                Ok(Some(line)) => return Ok(Got::Line(line)),
                Ok(None) => return Ok(Got::Eof),
                Err(RecvTimeoutError::Timeout) => {}
                // 读的线程已经结束了（读到头的那一条早就取走了）：队列从此一直是空的，照样等到时间
                Err(RecvTimeoutError::Disconnected) => sleep(step),
            }
        }
    }

    /// `p.stdin.close()`
    pub(super) fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// `p.kill()`
    pub(super) fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// `p.wait(timeout=secs)`：到时间还没退出是 false（原来的 TimeoutExpired）
    pub(super) fn wait(&mut self, secs: f64) -> bool {
        use std::os::unix::process::ExitStatusExt;
        let deadline = monotonic() + secs;
        // 和 Python 一样：一开始 0.5 毫秒看一次，每次加倍，最多 50 毫秒
        let mut delay = 0.0005;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.rc = Some(status.code().unwrap_or_else(|| -status.signal().unwrap_or(0)));
                    return true;
                }
                Ok(None) => {}
                Err(_) => return true,
            }
            let left = deadline - monotonic();
            if left <= 0.0 {
                return false;
            }
            delay = f64::min(delay * 2.0, 0.05).min(left);
            sleep(delay);
        }
    }

    /// `p.returncode`：还没等到它退出是 None；被信号杀掉的是负的信号号
    pub(super) fn returncode(&self) -> Option<i32> {
        self.rc
    }

    /// `err.seek(0); err.read()[-800:].decode('utf-8', 'replace')`：子进程在 stderr 上说的最后 800 个字节
    pub(super) fn err_tail(&mut self) -> String {
        use std::io::{Read, Seek, SeekFrom};
        let mut said = Vec::new();
        if self.err.seek(SeekFrom::Start(0)).is_ok() {
            let _ = self.err.read_to_end(&mut said);
        }
        String::from_utf8_lossy(&said[said.len().saturating_sub(800)..]).into_owned()
    }
}

/// Ctrl-C 和 SIGTERM（login-flow 用）。原来 Python 收到 SIGINT 抛 KeyboardInterrupt（进来时 SIGINT 被忽略着就不接：
/// 在后台跑的时候是这样），login-flow 另把 SIGTERM 也接成 KeyboardInterrupt。这里的处理函数只记一笔，等的地方
/// （读队列、等 _browser）隔一小会儿看一眼
mod interrupt {
    use std::sync::atomic::{AtomicBool, Ordering};

    static PENDING: AtomicBool = AtomicBool::new(false);

    extern "C" fn note(_sig: libc::c_int) {
        PENDING.store(true, Ordering::SeqCst);
    }

    /// `signal.signal(signal.SIGTERM, stop)`，连同 Python 启动时对 SIGINT 做的那一步
    pub fn install() {
        // SAFETY: sigaction 的参数都是这里的局部变量；处理函数只写一个原子变量
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            let int_is_default =
                libc::sigaction(libc::SIGINT, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_DFL;
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = note as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut sa.sa_mask);
            if int_is_default {
                libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
            }
            libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        }
    }

    /// 有没有还没处理的信号（看了就算处理了）
    pub fn take() -> bool {
        PENDING.swap(false, Ordering::SeqCst)
    }
}

/// `json.loads(字节)`：先照 json.detect_encoding 认编码（UTF-8、带 BOM 的 UTF-8、UTF-16、UTF-32），再解析。
/// 哪一步不行都是 None（原来的 ValueError）
pub(super) fn json_loads_bytes(b: &[u8]) -> Option<V> {
    let utf16 = |b: &[u8], big: bool| -> Option<String> {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        let units = b.chunks(2).map(|c| if big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
        char::decode_utf16(units).collect::<Result<String, _>>().ok()
    };
    let utf32 = |b: &[u8], big: bool| -> Option<String> {
        if !b.len().is_multiple_of(4) {
            return None;
        }
        b.chunks(4)
            .map(|c| {
                char::from_u32(if big {
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    u32::from_le_bytes([c[0], c[1], c[2], c[3]])
                })
            })
            .collect()
    };
    let utf8 = |b: &[u8]| std::str::from_utf8(b).ok().map(str::to_string);
    let text = if b.starts_with(&[0, 0, 0xfe, 0xff]) {
        utf32(&b[4..], true)
    } else if b.starts_with(&[0xff, 0xfe, 0, 0]) {
        utf32(&b[4..], false)
    } else if b.starts_with(&[0xfe, 0xff]) {
        utf16(&b[2..], true)
    } else if b.starts_with(&[0xff, 0xfe]) {
        utf16(&b[2..], false)
    } else if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        utf8(&b[3..])
    } else if b.len() >= 4 && b[0] == 0 {
        // 00 00 -- -- 是 utf-32-be，00 XX -- -- 是 utf-16-be
        if b[1] != 0 {
            utf16(b, true)
        } else {
            utf32(b, true)
        }
    } else if b.len() >= 4 && b[1] == 0 {
        // XX 00 00 00 是 utf-32-le，XX 00 00 XX、XX 00 XX -- 是 utf-16-le
        if b[2] != 0 || b[3] != 0 {
            utf16(b, false)
        } else {
            utf32(b, false)
        }
    } else if b.len() == 2 && b[0] == 0 {
        utf16(b, true)
    } else if b.len() == 2 && b[1] == 0 {
        utf16(b, false)
    } else {
        utf8(b)
    }?;
    json::loads(&text).ok()
}

/// codex app-server：stdio 上一行一个 JSON 的 JSON-RPC（不带 jsonrpc 字段）。在 bash 设好的账号环境里启动
/// （CODEX_HOME、出口都在环境变量里），加上 cca 的参数（不许改走系统代理）。报的客户端名和 codex login 一样
pub struct Rpc {
    p: Piped,
    n: i64,
    /// 收到的通知（有 method、没有 id）
    notes: Vec<Dict>,
    probe: Dict,
    /// 等回答的时候留意 Ctrl-C / SIGTERM（login-flow）
    interruptible: bool,
}

impl Rpc {
    /// 起不来（OSError）是 `Fail::Os` 或 `Fail::Other("OSError", [说明])`
    pub fn new(bin_: &str) -> R<Rpc> {
        let err = temp_file()?;
        let probe = codex_probe(bin_);
        // 原来的 codex_probe 要重新探测时先 os.makedirs(~/.codex)，建不了（比如那里是个文件）是 OSError，从这里传出去。
        // 这边的 codex_probe 不报这个错：建不了的话探测也跑不起来，拿回来的是不带 sig 的那一份（全加上的）。
        // 拿到这一份、又不是因为没给程序 / 程序不在，就照原来的再建一次，把错报出来（已经有了就什么都不做）
        if !probe.has("sig") && !bin_.is_empty() && std::fs::metadata(path::realpath(bin_)).is_ok() {
            let master = &codex().master;
            fs::makedirs(master).map_err(|e| Fail::Other("OSError", vec![fs::open_error_text(&e, master)]))?;
        }
        let home = env_or_empty("CODEX_HOME");
        let mut argv = vec![bin_.to_string()];
        match probe.at("flags") {
            V::List(flags) => {
                for f in flags.items() {
                    match f.as_str() {
                        Some(f) => argv.push(f.to_string()),
                        None => return Err(type_error(format!("expected str, bytes or os.PathLike object, not {}", f.py_repr()))),
                    }
                }
            }
            other => return Err(type_error(format!("can only concatenate list (not {}) to list", other.py_repr()))),
        }
        argv.push("app-server".to_string());
        let cwd = if path::isdir(&home) { path::dirname(&home) } else { CCA_HOME.clone() };
        Ok(Rpc { p: Piped::spawn(&argv, &cwd, err)?, n: 0, notes: Vec::new(), probe, interruptible: false })
    }

    pub fn send(&mut self, msg: &V) -> bool {
        self.p.write(format!("{}\n", json::dumps(msg, json::Fmt::new())).as_bytes())
    }

    /// 读下一条消息，通知（有 method、没有 id）另外存进 notes；到时候了返回 None，app-server 退出了是
    /// `Fail::Other("EOFError", …)`
    pub fn next(&mut self, end: f64) -> R<Option<Dict>> {
        match self.p.get(end, self.interruptible)? {
            Got::Empty => Ok(None),
            Got::Eof => Err(Fail::Other("EOFError", Vec::new())),
            Got::Line(line) => {
                let Some(V::Dict(m)) = json_loads_bytes(&line) else { return Ok(Some(Dict::new())) };
                if m.has("method") && !m.has("id") {
                    self.notes.push(m.clone());
                }
                Ok(Some(m))
            }
        }
    }

    /// 发一个请求等回应：返回 (结果, 错误)；错误是 timeout（到时候了）、exit（没回应就退出了）或它的说明，没有错是算假的值
    pub fn call(&mut self, method: &str, params: Option<V>, end: f64) -> R<(V, V)> {
        self.n += 1;
        let msg = dict! { "id" => self.n, "method" => method };
        if let Some(params) = params {
            msg.set("params", params);
        }
        if !self.send(&V::Dict(msg)) {
            return Ok((V::None, V::from("exit")));
        }
        loop {
            let m = match self.next(end) {
                Ok(Some(m)) => m,
                Ok(None) => return Ok((V::None, V::from("timeout"))),
                Err(e) if e.is("EOFError") => return Ok((V::None, V::from("exit"))),
                Err(e) => return Err(e),
            };
            if m.get("id").is_some_and(|id| id.py_eq(&V::Int(self.n))) && (m.has("result") || m.has("error")) {
                let e = m.get("error").unwrap_or(V::None);
                let err = match &e {
                    V::Dict(d) => {
                        d.get("message").filter(|x| x.truthy()).unwrap_or_else(|| V::from(json::dumps(&e, json::Fmt::new())))
                    }
                    _ if e.truthy() => V::from(e.py_str()),
                    _ => e.clone(),
                };
                return Ok((m.get("result").unwrap_or(V::None), err));
            }
        }
    }

    /// 等一条通知（先看已经收到的）：返回它的 params；到时候了返回 None，app-server 退出了是 EOFError
    pub fn wait(&mut self, method: &str, end: f64, matches: &dyn Fn(&V) -> R<bool>) -> R<Option<V>> {
        loop {
            for i in 0..self.notes.len() {
                let m = &self.notes[i];
                let params = m.get("params").filter(|p| p.truthy()).unwrap_or_else(|| V::Dict(Dict::new()));
                if m.get("method").is_some_and(|x| x.as_str() == Some(method)) && matches(&params)? {
                    self.notes.remove(i);
                    return Ok(Some(params));
                }
            }
            if self.next(end)?.is_none() {
                return Ok(None);
            }
        }
    }

    /// call 的错误 → (原因, 说明)：unsupported（这个版本的 codex 没有这个接口）、auth（登录凭证失效了：token 刷新不了或者
    /// 被服务器拒绝，比如改过密码、在别处退出过，要重新登录）、timeout 或 error
    pub fn failure(&mut self, err: &V) -> (&'static str, String) {
        let ours = matches!(err.as_str(), Some("timeout" | "exit"));
        let msg = if ours { self.tail() } else { str_or_empty(&Some(err.clone())) };
        let said = format!("{} {}", err.py_str(), msg);
        let short = text::head(&msg, 300).to_string();
        if re!(r"(?i)method not found|unknown (method|variant|request)|unrecognized subcommand|unexpected argument").found(&said) {
            return ("unsupported", short);
        }
        if re!(r"(?i)access token could not be refreshed|\b401 unauthorized\b").found(&said) {
            return ("auth", short);
        }
        (if err.as_str() == Some("timeout") { "timeout" } else { "error" }, short)
    }

    /// initialize + initialized；返回错误（没有就是算假的值）
    pub fn start(&mut self, end: f64) -> R<V> {
        let version = self.probe.get("version").filter(|v| v.truthy()).unwrap_or_else(|| V::from("0.0.0"));
        let info = dict! { "clientInfo" => dict! { "name" => CODEX_CLIENT, "title" => V::None, "version" => version } };
        let (_, err) = self.call("initialize", Some(V::Dict(info)), end)?;
        if !err.truthy() {
            self.send(&V::Dict(dict! { "method" => "initialized" }));
        }
        Ok(err)
    }

    /// app-server 在 stderr 上说的最后一段，空白都并成一个空格
    pub fn tail(&mut self) -> String {
        text::split_ws(&self.p.err_tail()).join(" ")
    }

    pub fn close(&mut self, kill: bool) {
        if kill {
            self.p.kill();
        }
        self.p.close_stdin();
        if !self.p.wait(5.0) {
            self.p.kill();
        }
    }
}

/// 一组额度（primary 一般是 5 小时，secondary 是每周）→ 和 Claude 一样的窗口 {five_hour, seven_day}：按窗口长度认
pub fn codex_windows(snap: &Dict) -> Dict {
    let out = Dict::new();
    for (i, w) in [snap.get("primary"), snap.get("secondary")].into_iter().enumerate() {
        let Some(V::Dict(w)) = w else { continue };
        let Some(p) = w.get("usedPercent").filter(is_num) else { continue };
        let k = match w.get("windowDurationMins").filter(|mins| mins.isinstance_num()) {
            Some(mins) => {
                if mins.as_pynum().is_some_and(|mins| mins <= 24.0 * 60.0) {
                    "five_hour"
                } else {
                    "seven_day"
                }
            }
            None => ["five_hour", "seven_day"][i],
        };
        out.setdefault(k, dict! { "utilization" => p, "resets_at" => w.get("resetsAt").unwrap_or(V::None) });
    }
    out
}

/// account/rateLimits/read 的回应 → 和 Claude 的查询一样的 {rate_limits_available, rate_limits, subscription_type}
pub fn codex_usage(res: &V) -> Dict {
    let res = res.as_dict().unwrap_or_default();
    let main = res.dict("rateLimits").unwrap_or_default();
    let rl = codex_windows(&main);
    let extra = res.dict("rateLimitsByLimitId").unwrap_or_default();
    let main_id = main.get("limitId");
    let mut ms: Vec<V> = Vec::new();
    // 别的额度（某些模型单独计的）：只列每周的
    for (lid, snap) in extra.items() {
        let Some(snap) = snap.as_dict() else { continue };
        if main_id.as_ref().is_some_and(|id| id.as_str() == Some(&*lid)) {
            continue;
        }
        if let Some(V::Dict(w)) = codex_windows(&snap).get("seven_day") {
            let m = w.copy();
            m.set("display_name", snap.get("limitName").filter(|n| n.truthy()).unwrap_or_else(|| V::Str(lid.clone())));
            ms.push(V::Dict(m));
        }
    }
    if !ms.is_empty() {
        rl.set("model_scoped", ms);
    }
    if let Some(credits) = main.dict("credits") {
        rl.set("credits", credits);
    }
    // 还能重置几次额度（codex 里 /usage 的 Redeem reset）和明细
    if let Some(credits) = res.dict("rateLimitResetCredits") {
        rl.set("reset_credits", credits);
    }
    if main.truthy("planType") {
        rl.set("plan", main.at("planType")); // 存进缓存：查不到时显示上次查到的套餐（登录凭证里记的可能是旧的）
    }
    dict! {
        "rate_limits_available" => rl.has("five_hour") || rl.has("seven_day"), "rate_limits" => &rl,
        "subscription_type" => main.get("planType").unwrap_or(V::None),
    }
}

/// 起子进程不成那一类错（原来的 `except OSError`）
fn is_os_error(e: &Fail) -> bool {
    e.is_os() || e.is("OSError")
}

/// 在账号的环境里启动 codex app-server、initialize，run(r, 截止时间) 做这次要做的事，返回 (结果, 错误)；
/// stdout 打印一行 JSON：结果，或者出错时的 {ok: false, why: unsupported|timeout|error, msg}
fn codex_rpc_once(bin_: &str, timeout: &str, run: impl FnOnce(&mut Rpc, f64) -> R<(V, V)>) -> R<()> {
    let end = time() + float_arg(timeout)?;
    let mut r = match Rpc::new(bin_) {
        Ok(r) => r,
        Err(e) if is_os_error(&e) => {
            out!("{}", json::dumps(&V::Dict(dict! { "ok" => false, "why" => "error", "msg" => e.to_string() }), json::LINE));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut out = V::None;
    let mut err = r.start(end)?;
    if !err.truthy() {
        (out, err) = run(&mut r, end)?;
    }
    if err.truthy() {
        let (why, msg) = r.failure(&err);
        out = V::Dict(dict! { "ok" => false, "why" => why, "msg" => msg });
    }
    r.close(err.truthy());
    out!("{}", json::dumps(&out, json::LINE));
    Ok(())
}

/// account/rateLimits/read 取用量（和 codex 里 /status 显示的一样）。只查用量，不发模型请求、不耗额度；token 由 codex 自己
/// 读取和刷新。打印的格式和 Claude 的查询一样：{ok: true, usage: {rate_limits_available, rate_limits, subscription_type}}
pub fn codex_usage_query(bin_: &str, timeout: &str) -> R<()> {
    codex_rpc_once(bin_, timeout, |r, end| {
        let (res, err) = r.call("account/rateLimits/read", None, end)?;
        Ok((V::Dict(dict! { "ok" => true, "usage" => codex_usage(&res) }), err))
    })
}

/// codex 回的重置结果 → cca 的写法（macos/PROTOCOL.md 4.5：reset 重置了、already_redeemed 这一次早就重置过了、nothing_to_reset
/// 用量还不需要重置（不扣次数）、no_credit 没有能用的重置机会了）
fn reset_outcome(outcome: &str) -> Option<&'static str> {
    Some(match outcome {
        "reset" => "reset",
        "alreadyRedeemed" => "already_redeemed",
        "nothingToReset" => "nothing_to_reset",
        "noCredit" => "no_credit",
        _ => return None,
    })
}

/// 用掉一次重置机会（Codex 账号；和查用量一样经账号的出口）：account/rateLimitResetCredit/consume，creditId 是要用的那一次
/// （空着由 OpenAI 挑），idempotencyKey 每次新生成；重置了就再读一次用量。打印 {ok: true, outcome, usage: 查用量的格式或 null}
pub fn cmd_reset_consume(args: &[String]) -> R<()> {
    arity("cmd_reset_consume", args, 1, 3)?;
    let (bin_, credit, timeout) = (args[0].as_str(), arg(args, 1, ""), arg(args, 2, "25"));
    codex_rpc_once(bin_, timeout, |r, end| {
        let params = dict! { "idempotencyKey" => uuid4() };
        if !credit.is_empty() {
            params.set("creditId", credit);
        }
        let (res, err) = r.call("account/rateLimitResetCredit/consume", Some(V::Dict(params)), end)?;
        if err.truthy() {
            return Ok((V::None, err));
        }
        let said = res.as_dict().and_then(|res| res.get("outcome")).unwrap_or(V::None);
        hashable(&said)?;
        let Some(outcome) = said.as_str().and_then(reset_outcome) else {
            let msg = format!("codex 回的结果看不懂：{}", text::head(&json::dumps(&res, json::LINE), 200));
            return Ok((V::Dict(dict! { "ok" => false, "why" => "error", "msg" => msg }), V::None));
        };
        let out = dict! { "ok" => true, "outcome" => outcome, "usage" => V::None };
        // 重置了：再读一次用量（读不到不算失败，下次查用量时就有了）
        if matches!(outcome, "reset" | "already_redeemed") {
            let (u, e2) = r.call("account/rateLimits/read", None, py_max(end, time() + 15.0))?;
            if !e2.truthy() {
                out.set("usage", codex_usage(&u));
            }
        }
        Ok((V::Dict(out), V::None))
    })
}

/// `subprocess.run([SELF_PATH, '_browser', 网址], stdin=subprocess.DEVNULL)`。等它的时候来了 Ctrl-C / SIGTERM，照 Python
/// 的做法：再等它 0.25 秒（Ctrl-C 时它自己也收到了），还没退出就杀掉，然后是 KeyboardInterrupt
fn run_browser(url: &str) -> R<()> {
    let mut child = match Proc::new([SELF_PATH.as_str(), "_browser", url]).stdin(In::Null).spawn() {
        Ok(child) => child,
        Err(ProcError::Os(e)) if e.raw_os_error().is_none() => return Err(Fail::Value("embedded null byte".into())),
        Err(ProcError::Os(e)) => return Err(Fail::Other("OSError", vec![fs::open_error_text(&e, &SELF_PATH)])),
        Err(ProcError::Timeout { .. } | ProcError::Interrupted) => unreachable!("spawn 不等它：没有超时，也不会被信号打断"),
    };
    let mut delay = 0.0005;
    loop {
        if !matches!(child.try_wait(), Ok(None)) {
            return Ok(());
        }
        if interrupt::take() {
            let until = monotonic() + 0.25;
            while monotonic() < until && matches!(child.try_wait(), Ok(None)) {
                sleep(0.005);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(Fail::Other("KeyboardInterrupt", Vec::new()));
        }
        delay = f64::min(delay * 2.0, 0.02);
        sleep(delay);
    }
}

/// login-flow 里 try 的那一段；r、login、ok 放在外面，收尾（原来的 except / finally）要看它们
fn login_steps(bin_: &str, limit: f64, end: f64, slot: &mut Option<Rpc>, login: &mut V, ok: &mut bool) -> R<()> {
    let mut started = Rpc::new(bin_)?;
    started.interruptible = true;
    let r = slot.insert(started);
    let mut err = r.start(time() + 30.0)?;
    let mut res = V::None;
    if !err.truthy() {
        (res, err) = r.call("account/login/start", Some(V::Dict(dict! { "type" => "chatgpt" })), time() + 30.0)?;
    }
    let url = res.as_dict().and_then(|res| res.get("authUrl")).filter(|url| url.truthy());
    let (Some(res), Some(url), false) = (res.as_dict(), url, err.truthy()) else {
        let (why, msg) = if err.truthy() { r.failure(&err) } else { ("unsupported", String::new()) };
        if why == "unsupported" {
            return exit(3);
        }
        return die(if msg.is_empty() { "登录没能开始".to_string() } else { msg });
    };
    *login = res.get("loginId").unwrap_or(V::None);
    let Some(url) = url.as_str() else {
        return Err(type_error(format!("expected str, bytes or os.PathLike object, not {}", url.py_repr())));
    };
    run_browser(url)?;
    let wanted = login.clone();
    let p = r.wait("account/login/completed", end, &|x: &V| {
        if !wanted.truthy() {
            return Ok(true);
        }
        let Some(x) = x.as_dict() else {
            return Err(Fail::Other("AttributeError", vec![format!("登录完成的通知里 params 不是对象：{}", x.py_repr())]));
        };
        Ok(x.get("loginId").is_none_or(|id| id.is_none() || id.py_eq(&wanted)))
    })?;
    let Some(p) = p else {
        return die(format!("等了 {} 分钟还没登录完", whole_minutes(limit)?));
    };
    let Some(p) = p.as_dict() else {
        return Err(Fail::Other("AttributeError", vec![format!("登录完成的通知里 params 不是对象：{}", p.py_repr())]));
    };
    *ok = p.truthy("success");
    if !*ok {
        return die(p.get("error").filter(|e| e.truthy()).map_or_else(|| "登录没有成功".to_string(), |e| e.py_str()));
    }
    Ok(())
}

/// `'%d' % (int(x) // 60)`，x 是登录最多等多少秒：等了几分钟（NaN、无穷大是没人接的异常；负数照整除往小里取）。
/// 大到放不进 i64 的照十进制算——Python 的整数没有上限
fn whole_minutes(x: f64) -> R<String> {
    if x.is_nan() {
        return Err(Fail::Value("cannot convert float NaN to integer".into()));
    }
    if x.is_infinite() {
        return Err(Fail::Other("OverflowError", vec!["cannot convert float infinity to integer".into()]));
    }
    let n = x.trunc();
    if n.abs() < 9.0e18 {
        return Ok(floordiv(n as i64, 60).to_string());
    }
    let (q, r) = div_small(&format!("{:.0}", n.abs()), 60);
    if n > 0.0 {
        return Ok(q);
    }
    // 负数：有余数时商再往小里走一格（绝对值加一，从个位起进位）
    let mut digits = q.into_bytes();
    if r != 0 {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] != b'9' {
                digits[i] += 1;
                break;
            }
            digits[i] = b'0';
        }
    }
    Ok(format!("-{}", String::from_utf8_lossy(&digits)))
}

/// Codex 账号登录（在 bash 设好的账号环境里前台运行，Ctrl-C 会取消登录）：经 app-server 的 account/login/start 拿到
/// 登录网址，回调 cca _browser 在这个账号专属的 Chrome 里打开（走账号的出口；回调地址是本机 127.0.0.1:1455，Chrome
/// 不经代理），登录完成后 codex 自己把凭证写进这个账号的 auth.json。退出码：0 登录了；3 这个版本的 codex 不支持
/// （bash 改用 codex login）；1 没登录成，原因打到 stderr
pub fn cmd_login_flow(args: &[String]) -> R<()> {
    arity("cmd_login_flow", args, 1, 2)?;
    let (bin_, timeout) = (args[0].as_str(), arg(args, 1, "900"));
    let limit = float_arg(timeout)?;
    let end = time() + limit;
    // 服务器上登录时它在后台跑，cca 取消登录用 SIGTERM：和 Ctrl-C 一样告诉 app-server 取消
    interrupt::install();
    let (mut r, mut login, mut ok) = (None, V::None, false);
    let done = match login_steps(bin_, limit, end, &mut r, &mut login, &mut ok) {
        Err(e) if e.is("EOFError") => {
            let said = r.as_mut().map_or_else(String::new, |r| r.tail());
            die(if said.is_empty() { "codex app-server 退出了".to_string() } else { said })
        }
        Err(e) if e.is("KeyboardInterrupt") => {
            if let Some(r) = r.as_mut().filter(|_| login.truthy()) {
                r.send(&V::Dict(
                    dict! { "id" => 0, "method" => "account/login/cancel", "params" => dict! { "loginId" => &login } },
                ));
            }
            die("已取消登录")
        }
        Err(e) if is_os_error(&e) => die(e.to_string()),
        other => other,
    };
    // die 的那一句原来是当场说的，在收尾之前：这里也先说出来，再收尾，最后只带着退出码走
    let done = match done {
        Err(Fail::Die(msg)) => {
            err!("{msg}");
            Err(Fail::Exit(1))
        }
        other => other,
    };
    if let Some(r) = r.as_mut() {
        r.close(!ok); // 登录成功时让它自己正常退出（凭证这时一定已经写完）
    }
    done
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
    let done = Proc::new(["sqlite3", "-batch", "-bail", db])
        .input(script.as_bytes())
        .stdout(Out::Capture)
        .stderr(Out::Null)
        .timeout(30.0)
        .run()
        .ok()?;
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
        ["sessions", "archived_sessions"]
            .iter()
            .find_map(|kind| Some(format!("{}/{}", path::join(master, kind), rest.strip_prefix(kind)?.strip_prefix('/')?)))
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
        let query = format!(
            ".timeout 2000\n.mode quote\nSELECT id, rollout_path FROM threads WHERE substr(rollout_path, 1, {}) = {};\n",
            text::len(&pre),
            sql_quote(&pre)
        );
        let Some(rows) = sqlite3(&db, &query).and_then(|out| sql_rows(&out)) else { continue };
        let mut script = String::from(".timeout 2000\nBEGIN;\n");
        let mut fixes = 0;
        for row in rows.iter().filter(|r| r.len() == 2) {
            let Some(old) = sql_text(&row[1]) else { continue };
            let new = rollout_in_master(&old, &pre, tool.sub, &tool.master);
            if new != old && !new.contains('\0') && any_with_prefix(&new) {
                script.push_str(&format!(
                    "UPDATE threads SET rollout_path = {} WHERE id = {} AND rollout_path = {};\n",
                    sql_quote(&new),
                    row[0],
                    row[1]
                ));
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
mod tests {
    use super::super::usage::vectors::{check, rows};
    use super::*;

    #[test]
    fn plans_match_python() {
        // 表里的每一种套餐，加上表里没有的、不是文字的
        let all = rows("codex_plans");
        for (plan, _, _) in CODEX_PLANS {
            assert!(all.iter().any(|r| r[0].as_str() == Some(plan)), "{plan} 不在对照数据里");
        }
        for r in all {
            assert_eq!(codex_plan_label(&r[0]), r[1].py_str(), "codex_plan_label({})", r[0].py_repr());
            assert_eq!(codex_plan_tier(&r[0]), r[2].py_str(), "codex_plan_tier({})", r[0].py_repr());
        }
    }

    #[test]
    fn claims_match_python() {
        for r in rows("codex_claims") {
            check(Ok(claims_from(Some(r[0].clone()))), &r[1], &format!("codex_claims：{}", r[0].py_repr()));
        }
        assert!(claims_from(None).is_empty());
    }

    #[test]
    fn json_from_bytes_matches_python() {
        for r in rows("json_bytes") {
            let bytes: Vec<u8> = r[0].as_list().unwrap().items().iter().map(|b| b.as_int().unwrap() as u8).collect();
            let got = json_loads_bytes(&bytes).map(|v| json::dumps(&v, json::LINE));
            assert_eq!(got.as_deref(), r[1].as_str(), "json.loads({bytes:?})");
        }
    }

    #[test]
    fn rate_limits_to_windows() {
        let w = |text: &str| json::dumps(&V::Dict(codex_windows(&json::loads(text).unwrap().as_dict().unwrap())), json::Fmt::new());
        assert_eq!(
            w(
                r#"{"primary": {"usedPercent": 13, "windowDurationMins": 300, "resetsAt": 5}, "secondary": {"usedPercent": 41.5, "windowDurationMins": 10080}}"#
            ),
            r#"{"five_hour": {"utilization": 13, "resets_at": 5}, "seven_day": {"utilization": 41.5, "resets_at": null}}"#
        );
        // 没写窗口多长的按位置认；都是短的只留第一个
        assert_eq!(
            w(r#"{"primary": {"usedPercent": 1}, "secondary": {"usedPercent": 2}}"#),
            r#"{"five_hour": {"utilization": 1, "resets_at": null}, "seven_day": {"utilization": 2, "resets_at": null}}"#
        );
        assert_eq!(
            w(
                r#"{"primary": {"usedPercent": 1, "windowDurationMins": 1440}, "secondary": {"usedPercent": 2, "windowDurationMins": true}}"#
            ),
            r#"{"five_hour": {"utilization": 1, "resets_at": null}}"#
        );
        assert_eq!(
            w(r#"{"primary": {"usedPercent": true}, "secondary": {"usedPercent": 2, "windowDurationMins": NaN}}"#),
            r#"{"seven_day": {"utilization": 2, "resets_at": null}}"#
        );
        assert_eq!(reset_outcome("alreadyRedeemed"), Some("already_redeemed"));
        assert_eq!(reset_outcome("already_redeemed"), None);
    }

    #[test]
    fn login_timeout_minutes_match_python() {
        for r in rows("whole_minutes") {
            check(whole_minutes(super::super::usage::vectors::number(&r[0])), &r[1], &format!("int({}) // 60", r[0].py_str()));
        }
    }
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
        assert_eq!(
            fix("/h/.cca/codex/accounts/erin-1a2b3c/codex/sessions/2026/11/03/rollout-x.jsonl"),
            "/h/.codex/sessions/2026/11/03/rollout-x.jsonl"
        );
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
