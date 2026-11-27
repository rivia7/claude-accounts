//! 沙箱里的假命令要的那些角色（tests/setup.sh 写出来的 shell 脚本调它们，"$CCA_T/cca-test" 角色 参数…）：假的 claude、codex、
//! agy 的会话和登录，假的用量接口、OAuth 回调、浏览器、STUN、GitHub 的发布列表、程序坞的配置……一个假命令一个文件。
//!
//! 原来都是 Python（setup.sh 写出来的 .py 和假命令里内嵌的 python3 -c / python3 - <<PY），这里照原样重写：打印的、追加进日志的、
//! 写出来的文件（JSON 的缩进、键的先后、转不转义非 ASCII）、退出码、等待前后各做了什么都和原来一样，回归照样 grep 这些日志。
//! - Python 往重定向了的 stdout 上 print 是攒着、退出时才写出去的（没有 flush 的那些）：这里也攒着（print / write_out），
//!   角色结束时一起写出去；原来 flush 了的地方照样 flush
//! - Python 里没人接的异常：stderr 上说一句、退出码 1（Fail；原来的 traceback 只留最后那一行的意思）
//! - 起子进程像 subprocess（close_fds=True）：子进程只拿到 0、1、2（起之前 kit::proc::cloexec_all），环境照传（钩子、apiKeyHelper 要 cca 的环境）
//! - 空白、拆行、真假、repr 这些 Python 的语义用 kit::py 的那一份（py::strip、splitlines、truthy、py_str、repr_str……）
//!
//! regress.sh 用 pgrep -f / pkill -f 找其中几个（fake_oauth、fake_usage、fake_codex_server、fake_agy）：假命令用
//! exec -a "$CCA_T/fake_oauth" "$CCA_T/cca-test" fake-oauth … 起它们，命令行里就有这个名字。

mod agy;
mod browser;
mod claude;
mod codex;
mod codex_server;
mod core_load;
mod dock;
mod gh;
mod launchd;
mod linux_claude;
mod linux_codex;
mod linux_netstat;
mod oauth;
mod stun;
mod usage;

pub use kit::crypto::uuid4;
pub use kit::json::{self, Value};
pub use kit::obj;
pub use kit::proc::cloexec_all;
pub use kit::py::{self, py_repr, py_str, repr_str, splitlines, truthy};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 角色跑完：攒着的 stdout 写出去，没人接的异常说一句（退出码 1）
macro_rules! role {
    ($f:path) => {
        |a: &[String]| -> i32 { finish($f(a)) }
    };
}

pub const CMDS: &[(&str, crate::Run)] = &[
    // 假 claude（setup.sh 的 $T/bin/claude）
    ("claude-settings", role!(claude::settings)), // 设置 JSON  CCA_T  是否会话(1/0)：记下 env、跑 apiKeyHelper 和 SessionStart 钩子
    ("claude-login-device", role!(claude::login_device)), // .claude.json：LOGIN-DEVICE 那一行
    ("claude-login", role!(claude::login)),       // .claude.json 邮箱：写 oauthAccount
    ("claude-logout", role!(claude::logout)),     // .claude.json：去掉 oauthAccount
    ("claude-status", role!(claude::status)),     // .claude.json：auth status 的 JSON
    ("claude-session", role!(claude::session)),   // 配置目录 账号：一次会话写进配置目录的东西
    ("fake-usage", role!(usage::run)),            // stream-json 的 get_usage
    ("fake-oauth", role!(oauth::run)),            // .claude.json 邮箱：本机回调的登录
    ("browser", role!(browser::run)),             // state code：扮演账号的 Chrome 跳回回调
    // 假 codex（$T/bin/codex）
    ("codex-features", role!(codex::features)),      // config.toml 开关：features disable
    ("codex-provider", role!(codex::provider)),      // -c 里的 model_providers.cca  CCA_T：执行 auth 的命令取 key
    ("codex-notify", role!(codex::notify)),          // -c 里的 notify  JSON：一轮做完了起 notify 的命令
    ("codex-session", role!(codex::session)),        // CODEX_HOME  会话库目录  账号：一次会话写进 CODEX_HOME 的东西
    ("fake-codex-server", role!(codex_server::run)), // [--write-auth]：app-server
    // 假 agy（$T/bin/agy）
    ("fake-agy", role!(agy::run)), // agy 的参数…
    // 别的
    ("stun", role!(stun::run)),           // 沙箱目录：假的 STUN 服务器
    ("core-load", role!(core_load::run)), // 配置文件：假内核读配置（原来的 $T/core_load）
    ("gh-releases", role!(gh::run)),      // $CCA_T/gh：假 GitHub 的发布列表
    ("dockpl", role!(dock::run)),         // dock.plist set|get|add …：程序坞的设置
    ("plist", role!(launchd::run)),       // get 文件 路径…：属性列表里的几个值，制表符隔开（假 launchctl 用）
    // tests/linux.sh 的沙箱（原来是它现写的三段 Python）
    ("linux-claude", role!(linux_claude::run)), // 沙箱目录 脚本的路径 参数…：假 claude（登录的回调、会话）
    ("linux-codex", role!(linux_codex::run)),   // 沙箱目录 脚本的路径 参数…：假 codex（app-server 的登录和用量、会话）
    ("netstat", role!(linux_netstat::run)),     // 文件… [-- 参数…]：假路由器上的 netstat，读 /proc/net/tcp
];

// ---- 异常、stdout ----

/// Python 里没人接的异常：说的是什么
#[derive(Debug)]
pub struct Fail(pub String);
pub type R<T> = Result<T, Fail>;

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Fail {
        Fail(os_text(&e, None))
    }
}

pub fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail(msg.into()))
}

/// OSError 的说法：FileNotFoundError: [Errno 2] No such file or directory: '路径'
pub fn os_text(e: &io::Error, path: Option<&str>) -> String {
    let kind = match e.raw_os_error() {
        Some(libc::ENOENT) => "FileNotFoundError",
        Some(libc::EEXIST) => "FileExistsError",
        Some(libc::EISDIR) => "IsADirectoryError",
        Some(libc::ENOTDIR) => "NotADirectoryError",
        Some(libc::EACCES | libc::EPERM) => "PermissionError",
        _ => "OSError",
    };
    let no = e.raw_os_error().unwrap_or(0);
    // io::Error 的说法后面带着 (os error N)：去掉，留下 strerror 的那一句
    let msg = e.to_string();
    let msg = msg.split(" (os error").next().unwrap_or(&msg).to_string();
    match path {
        Some(p) => format!("{kind}: [Errno {no}] {msg}: {}", repr_str(p)),
        None => format!("{kind}: [Errno {no}] {msg}"),
    }
}

fn os_fail(path: &str) -> impl Fn(io::Error) -> Fail + '_ {
    move |e| Fail(os_text(&e, Some(path)))
}

static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// print(s)：攒着，flush 或者角色结束时才写出去
pub fn print(s: &str) {
    let mut o = OUT.lock().unwrap_or_else(|e| e.into_inner());
    o.extend_from_slice(s.as_bytes());
    o.push(b'\n');
}

/// sys.stdout.write(s)：同上，不加换行
pub fn write_out(s: &str) {
    OUT.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(s.as_bytes());
}

/// sys.stdout.flush()：对方走了（EPIPE）就不说话、直接退出
pub fn flush() {
    let mut o = OUT.lock().unwrap_or_else(|e| e.into_inner());
    if o.is_empty() {
        return;
    }
    let mut so = io::stdout().lock();
    if so.write_all(&o).and_then(|()| so.flush()).is_err() {
        std::process::exit(1);
    }
    o.clear();
}

/// print(s, flush=True)：几个线程一起说话时一句一句地出去（原来是拿着锁 print）
pub fn print_flush(s: &str) {
    let mut o = OUT.lock().unwrap_or_else(|e| e.into_inner());
    o.extend_from_slice(s.as_bytes());
    o.push(b'\n');
    let mut so = io::stdout().lock();
    if so.write_all(&o).and_then(|()| so.flush()).is_err() {
        std::process::exit(1);
    }
    o.clear();
}

/// 角色结束（正常返回、sys.exit、没人接的异常）：攒着的写出去
pub fn finish(r: R<i32>) -> i32 {
    flush();
    match r {
        Ok(code) => code,
        Err(Fail(msg)) => {
            eprintln!("{msg}");
            1
        }
    }
}

// ---- 环境、文件、路径（照 Python 的 os、os.path） ----

pub fn env(k: &str) -> Option<String> {
    std::env::var_os(k).map(|v| v.to_string_lossy().into_owned())
}

/// os.environ.get(k, 'unset') 这一类：没有就是给的那个
pub fn env_or(k: &str, default: &str) -> String {
    env(k).unwrap_or_else(|| default.to_string())
}

/// os.environ['CCA_T']
pub fn cca_t() -> R<String> {
    env("CCA_T").ok_or_else(|| Fail("KeyError: 'CCA_T'".into()))
}

pub fn getcwd() -> R<String> {
    Ok(std::env::current_dir()?.to_string_lossy().into_owned())
}

/// os.path.exists：顺着链接，出错算不存在
pub fn exists(p: &str) -> bool {
    std::fs::metadata(p).is_ok()
}

/// os.path.lexists：链接本身在就算
pub fn lexists(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// open(p).read()：按 UTF-8 读，换行统一成 \n（文本模式）
pub fn read_text(p: &str) -> R<String> {
    let raw = std::fs::read(p).map_err(os_fail(p))?;
    let s = String::from_utf8(raw).map_err(|e| {
        Fail(format!("UnicodeDecodeError: 'utf-8' codec can't decode byte at position {}", e.utf8_error().valid_up_to()))
    })?;
    Ok(if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s })
}

/// open(p, 'w').write(s)：顺着链接写进原文件
pub fn write_text(p: &str, s: &str) -> R<()> {
    std::fs::write(p, s).map_err(os_fail(p))
}

/// open(p, 'a').write(s)
pub fn append_text(p: &str, s: &str) -> R<()> {
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(p).map_err(os_fail(p))?;
    f.write_all(s.as_bytes()).map_err(os_fail(p))
}

/// os.replace
pub fn replace(src: &str, dst: &str) -> R<()> {
    std::fs::rename(src, dst).map_err(os_fail(src))
}

/// os.makedirs(p, exist_ok=True)
pub fn makedirs(p: &str) -> R<()> {
    match std::fs::create_dir_all(p) {
        Ok(()) => Ok(()),
        Err(_) if std::path::Path::new(p).is_dir() => Ok(()),
        Err(e) => Err(Fail(os_text(&e, Some(p)))),
    }
}

pub fn remove(p: &str) -> R<()> {
    std::fs::remove_file(p).map_err(os_fail(p))
}

/// os.path.join（POSIX）
pub fn join(parts: &[&str]) -> String {
    let mut path = String::new();
    for (i, b) in parts.iter().enumerate() {
        if i == 0 {
            path.push_str(b);
        } else if b.starts_with('/') {
            path = b.to_string();
        } else if path.is_empty() || path.ends_with('/') {
            path.push_str(b);
        } else {
            path.push('/');
            path.push_str(b);
        }
    }
    path
}

/// os.path.dirname
pub fn dirname(p: &str) -> String {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    let head = &p[..i];
    if !head.is_empty() && head.bytes().any(|b| b != b'/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
}

/// os.path.basename
pub fn basename(p: &str) -> &str {
    &p[p.rfind('/').map_or(0, |i| i + 1)..]
}

/// os.path.expanduser('~')：HOME，没有时查用户表
pub fn home_dir() -> String {
    if let Some(h) = std::env::var_os("HOME") {
        let h = h.to_string_lossy().trim_end_matches('/').to_string();
        return if h.is_empty() { "/".to_string() } else { h };
    }
    // SAFETY: getpwuid 返回的是 C 库里的静态数据（或空指针），马上拷出来；单线程地用
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return "~".to_string();
        }
        std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().trim_end_matches('/').to_string()
    }
}

/// os.path.realpath（strict=False）：一段一段地顺着链接走，走不下去的照原样接上
pub fn realpath(filename: &str) -> R<String> {
    let mut rest: Vec<Option<String>> = filename.trim_end_matches('/').split('/').rev().map(|s| Some(s.to_string())).collect();
    let mut part_count = rest.len();
    let mut path = if filename.starts_with('/') { "/".to_string() } else { getcwd()? };
    let mut seen: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    while part_count > 0 {
        let Some(item) = rest.pop() else { break };
        let Some(name) = item else {
            // 一个链接整个解析完了：记下它指到哪儿
            if let Some(Some(link)) = rest.pop() {
                seen.insert(link, Some(path.clone()));
            }
            continue;
        };
        part_count -= 1;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            path = match path.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => path[..i].to_string(),
            };
            continue;
        }
        let newpath = if path == "/" { format!("/{name}") } else { format!("{path}/{name}") };
        let target = match std::fs::symlink_metadata(&newpath) {
            Ok(m) if !m.file_type().is_symlink() => {
                path = newpath;
                continue;
            }
            Ok(_) => {
                if let Some(cached) = seen.get(&newpath) {
                    match cached {
                        Some(p) => {
                            path = p.clone();
                            continue;
                        }
                        // 链接绕成了圈：已经解析的部分接上剩下的
                        None => {
                            let tail: Vec<String> = rest.into_iter().rev().flatten().collect();
                            return Ok(join(&[&newpath, &tail.join("/")]).trim_end_matches('/').to_string());
                        }
                    }
                }
                match std::fs::read_link(&newpath) {
                    Ok(t) => t.to_string_lossy().into_owned(),
                    Err(_) => {
                        path = newpath;
                        continue;
                    }
                }
            }
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        if target.starts_with('/') {
            path = "/".to_string();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let parts: Vec<&str> = target.split('/').collect();
        part_count += parts.len();
        rest.extend(parts.into_iter().rev().map(|s| Some(s.to_string())));
    }
    Ok(path)
}

// ---- JSON ----

/// json.load(open(p))
pub fn load_json(p: &str) -> R<Value> {
    let text = read_text(p)?;
    json::parse(&text).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))
}

/// json.load(open(p)) if os.path.exists(p) else {}
pub fn load_json_or_empty(p: &str) -> R<Value> {
    if exists(p) {
        load_json(p)
    } else {
        Ok(Value::Obj(json::Obj::new()))
    }
}

/// json.dumps(v)：一行，", " 和 ": "，非 ASCII 转义
pub fn dumps(v: &Value) -> String {
    json::dumps(v, &json::Style::default())
}

/// json.dump(v, f, indent=2)：非 ASCII 照样转义
pub fn dumps_indent2(v: &Value) -> String {
    json::dumps(v, &json::Style::default().indent(2))
}

/// 字典里的一项（d.get(k)）；不是字典时像 Python 一样算异常
pub fn get<'a>(d: &'a Value, k: &str) -> R<Option<&'a Value>> {
    match d {
        Value::Obj(o) => Ok(o.get(k)),
        other => fail(format!("AttributeError: '{}' object has no attribute 'get'", type_name(other))),
    }
}

/// d[k]：没有这个键是 KeyError
pub fn item<'a>(d: &'a Value, k: &str) -> R<&'a Value> {
    match d {
        Value::Obj(o) => o.get(k).ok_or_else(|| Fail(format!("KeyError: {}", repr_str(k)))),
        other => fail(format!("TypeError: '{}' object is not subscriptable", type_name(other))),
    }
}

/// d[k] = v（d 得是字典）
pub fn set_item(d: &mut Value, k: &str, v: Value) -> R<()> {
    match d {
        Value::Obj(o) => {
            o.insert(k, v);
            Ok(())
        }
        // 列表的下标得是整数：Python 先说的是这个
        Value::List(_) => fail("TypeError: list indices must be integers or slices, not str"),
        other => fail(format!("TypeError: '{}' object does not support item assignment", type_name(other))),
    }
}

/// d.setdefault(k, {})，返回那一项
pub fn setdefault_obj<'a>(d: &'a mut Value, k: &str) -> R<&'a mut Value> {
    let Value::Obj(o) = d else { return fail(format!("AttributeError: '{}' object has no attribute 'setdefault'", type_name(d))) };
    if !o.contains_key(k) {
        o.insert(k, Value::Obj(json::Obj::new()));
    }
    Ok(o.get_mut(k).expect("刚放进去"))
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::List(_) => "list",
        Value::Obj(_) => "dict",
    }
}

pub fn sleep(secs: f64) {
    std::thread::sleep(Duration::from_secs_f64(secs));
}

/// time.strftime('%Y%m%d_%H%M%S')：本地时间
pub fn local_stamp() -> String {
    // SAFETY: tm 是纯数据，全 0 合法；localtime_r 只写给它的 tm
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:04}{:02}{:02}_{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

// ---- 子进程 ----

/// subprocess.run 的结果：退出码（被信号杀掉是负的信号值），文本模式读出来的 stdout、stderr
pub struct Done {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn exit_code(st: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    st.code().unwrap_or_else(|| -st.signal().unwrap_or(0))
}

/// 文本模式：按 UTF-8 解码（解不了是异常），换行统一成 \n
fn text_mode(b: Vec<u8>) -> R<String> {
    let s = String::from_utf8(b).map_err(|e| {
        Fail(format!("UnicodeDecodeError: 'utf-8' codec can't decode byte at position {}", e.utf8_error().valid_up_to()))
    })?;
    Ok(if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s })
}

/// subprocess.run(argv, stdin=DEVNULL)：stdout、stderr 照用这边的，等它退出（退出码不管）
pub fn run_wait(argv: &[String]) -> R<i32> {
    cloexec_all();
    let st = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| Fail(os_text(&e, Some(&argv[0]))))?;
    Ok(exit_code(st))
}

/// subprocess.run(argv, input=…|stdin=DEVNULL, capture_output=True, text=True, timeout=…)：环境照传；
/// 超时（秒数和它在 Python 里 str() 的样子）到了杀掉、等它退出，算没人接的异常（TimeoutExpired）
pub fn run_capture(argv: &[String], input: Option<&[u8]>, timeout: Option<(f64, &str)>) -> R<Done> {
    use std::process::{Command, Stdio};
    cloexec_all();
    let start = Instant::now();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Fail(os_text(&e, Some(&argv[0]))))?;
    let (tx, rx) = std::sync::mpsc::channel::<(u8, Vec<u8>)>();
    if let (Some(mut si), Some(data)) = (child.stdin.take(), input.map(<[u8]>::to_vec)) {
        std::thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    for (tag, pipe) in [
        (1u8, child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
        (2, child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
    ] {
        let tx = tx.clone();
        if let Some(mut p) = pipe {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                let _ = p.read_to_end(&mut v);
                let _ = tx.send((tag, v));
            });
        }
    }
    drop(tx);
    // communicate：两个管道都读到头（有超时就到点为止），再等进程
    let (mut out, mut err) = (None, None);
    while out.is_none() || err.is_none() {
        let got = match timeout {
            Some((t, shown)) => {
                // 超时是 0、负的都算马上到点；太大（inf）就是不限
                let left = match Duration::try_from_secs_f64(t.max(0.0)) {
                    Ok(d) => d.checked_sub(start.elapsed()),
                    Err(_) => Some(Duration::MAX),
                };
                match left.map(|d| rx.recv_timeout(d)) {
                    Some(Ok(x)) => Some(x),
                    Some(Err(std::sync::mpsc::RecvTimeoutError::Disconnected)) => None,
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return fail(format!(
                            "subprocess.TimeoutExpired: Command '{}' timed out after {shown} seconds",
                            py_repr(&Value::from(argv.to_vec()))
                        ));
                    }
                }
            }
            None => rx.recv().ok(),
        };
        match got {
            Some((1, v)) => out = Some(v),
            Some((_, v)) => err = Some(v),
            None => break,
        }
    }
    let st = child.wait()?;
    Ok(Done { code: exit_code(st), stdout: text_mode(out.unwrap_or_default())?, stderr: text_mode(err.unwrap_or_default())? })
}

// ---- URL（照 urllib.parse） ----

/// quote_plus：字母、数字、_.-~ 以外的都转义，空格是 +
pub fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// urlencode：按给的先后
pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v))).collect::<Vec<_>>().join("&")
}

/// unquote：%XX 换回字节，按 UTF-8 解（解不了的换成 U+FFFD）；不成样子的 % 原样留着
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// parse_qsl(qs)（keep_blank_values=False）：空值的那一项不要
pub fn parse_qsl(qs: &str) -> Vec<(String, String)> {
    let mut r = Vec::new();
    for nv in qs.split('&') {
        if nv.is_empty() {
            continue;
        }
        let Some((n, v)) = nv.split_once('=') else { continue };
        if !v.is_empty() {
            r.push((unquote(&n.replace('+', " ")), unquote(&v.replace('+', " "))));
        }
    }
    r
}

/// dict(parse_qsl(qs)).get(k)：同一个键出现几次取最后一个
pub fn qs_get<'a>(q: &'a [(String, String)], k: &str) -> Option<&'a str> {
    q.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// urlsplit 的结果
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
}

/// urlsplit（allow_fragments=True）
pub fn urlsplit(url: &str) -> R<Split> {
    let url: String = url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut url = url.as_str();
    let mut scheme = String::new();
    if let Some(i) = url.find(':') {
        let head = &url[..i];
        if i > 0
            && head.as_bytes()[0].is_ascii_alphabetic()
            && head.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            scheme = head.to_ascii_lowercase();
            url = &url[i + 1..];
        }
    }
    let mut netloc = "";
    if url.starts_with("//") {
        let delim = ['/', '?', '#'].iter().filter_map(|c| url[2..].find(*c).map(|j| j + 2)).min().unwrap_or(url.len());
        netloc = &url[2..delim];
        url = &url[delim..];
        if netloc.contains('[') != netloc.contains(']') {
            return fail("ValueError: Invalid IPv6 URL");
        }
    }
    let url = url.split_once('#').map_or(url, |(u, _)| u);
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    Ok(Split { scheme, netloc: netloc.to_string(), path: path.to_string(), query: query.to_string() })
}

impl Split {
    /// .port：没写是 None，写的不是数字、不在 0-65535 里是 ValueError
    pub fn port(&self) -> R<Option<u16>> {
        let hostinfo = self.netloc.rsplit_once('@').map_or(self.netloc.as_str(), |(_, h)| h);
        let port = match hostinfo.split_once('[') {
            Some((_, br)) => br.split_once(']').map_or("", |(_, p)| p).split_once(':').map_or("", |(_, p)| p),
            None => hostinfo.split_once(':').map_or("", |(_, p)| p),
        };
        if port.is_empty() {
            return Ok(None);
        }
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return fail(format!("ValueError: Port could not be cast to integer value as {}", repr_str(port)));
        }
        port.parse::<u16>().map(Some).map_err(|_| Fail("ValueError: Port out of range 0-65535".into()))
    }
    /// .hostname（小写；方括号去掉）
    pub fn hostname(&self) -> String {
        let hostinfo = self.netloc.rsplit_once('@').map_or(self.netloc.as_str(), |(_, h)| h);
        let host = match hostinfo.split_once('[') {
            Some((_, br)) => br.split_once(']').map_or(br, |(h, _)| h),
            None => hostinfo.split_once(':').map_or(hostinfo, |(h, _)| h),
        };
        host.to_ascii_lowercase()
    }
}

// ---- 本机的 HTTP 回调（照 socketserver.TCPServer + BaseHTTPRequestHandler） ----

/// 一个连上来的请求：回答照 BaseHTTPRequestHandler 写（HTTP/1.0，先攒着头、end_headers 时一起发），处理完关掉连接
pub struct Req {
    stream: TcpStream,
    pub path: String,
    head: Vec<u8>,
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        404 => "Not Found",
        501 => "Not Implemented",
        _ => "",
    }
}

impl Req {
    pub fn send_response(&mut self, code: u16) {
        let line = format!(
            "HTTP/1.0 {code} {}\r\nServer: BaseHTTP/0.6 cca-test\r\nDate: {}\r\n",
            reason(code),
            kit::httpd::http_date(std::time::SystemTime::now())
        );
        self.head.extend_from_slice(line.as_bytes());
    }
    pub fn send_header(&mut self, k: &str, v: &str) {
        self.head.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    pub fn end_headers(&mut self) {
        self.head.extend_from_slice(b"\r\n");
        let head = std::mem::take(&mut self.head);
        let _ = self.stream.write_all(&head);
    }
    /// wfile.write：马上发出去（对方走了不管）
    pub fn write(&mut self, body: &[u8]) {
        let _ = self.stream.write_all(body);
    }
    /// send_error：没处理的请求（不是 GET、请求行不对）
    fn send_error(&mut self, code: u16) {
        let body = format!("<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {code}</p>\n        <p>Message: {}.</p>\n    </body>\n</html>\n", reason(code));
        self.send_response(code);
        self.send_header("Connection", "close");
        self.send_header("Content-Type", "text/html;charset=utf-8");
        self.send_header("Content-Length", &body.len().to_string());
        self.end_headers();
        self.write(body.as_bytes());
    }
}

/// 在 127.0.0.1 的随机端口上听，后台一个线程一个一个地处理（像 serve_forever）；返回端口
pub fn serve(handler: impl Fn(&mut Req) + Send + 'static) -> R<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            handle(stream, &handler);
        }
    });
    Ok(port)
}

fn handle(mut stream: TcpStream, handler: &impl Fn(&mut Req)) {
    // 读到头和正文之间的空行（GET 没有正文）
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    while !data.windows(4).any(|w| w == b"\r\n\r\n") && !data.windows(2).any(|w| w == b"\n\n") && data.len() < 65536 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    let line_end = data.iter().position(|b| *b == b'\n').unwrap_or(data.len());
    let line: String = data[..line_end].iter().map(|b| *b as char).collect();
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut req = Req { stream, path: String::new(), head: Vec::new() };
    if words.is_empty() {
        return;
    }
    if words.len() >= 3 && !words[words.len() - 1].starts_with("HTTP/") || !(2..=3).contains(&words.len()) {
        req.send_error(400);
    } else if words[0] != "GET" {
        req.send_error(if words.len() == 2 { 400 } else { 501 });
    } else {
        let mut path = words[1].to_string();
        if path.starts_with("//") {
            path = format!("/{}", path.trim_start_matches('/'));
        }
        req.path = path;
        handler(&mut req);
    }
    let _ = req.stream.flush();
    let _ = req.stream.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_like_posixpath() {
        assert_eq!(join(&["a", "b"]), "a/b");
        assert_eq!(join(&["a/", "b"]), "a/b");
        assert_eq!(join(&["", "b"]), "b");
        assert_eq!(join(&["a", "/b", "c"]), "/b/c");
        assert_eq!(join(&["a", "b", ""]), "a/b/");
        assert_eq!(dirname("a/b/.claude.json"), "a/b");
        assert_eq!(dirname("x//.claude.json"), "x");
        assert_eq!(dirname("/f"), "/");
        assert_eq!(dirname("f"), "");
        assert_eq!(basename("a/b/core_test"), "core_test");
        assert_eq!(basename("a/b/"), "");
    }

    #[test]
    fn urls_like_urllib() {
        assert_eq!(quote_plus("http://localhost:1/callback a@b"), "http%3A%2F%2Flocalhost%3A1%2Fcallback+a%40b");
        assert_eq!(urlencode(&[("scope", "openid email"), ("x", "~_.-")]), "scope=openid+email&x=~_.-");
        assert_eq!(unquote("a%2Fb%zz%e4%b8%ad%"), "a/b%zz中%");
        assert_eq!(
            parse_qsl("code=&state=s%231&x&y=1+2&state=t"),
            [("state".to_string(), "s#1".to_string()), ("y".into(), "1 2".into()), ("state".into(), "t".into())]
        );
        let s = urlsplit("HTTP://user@localhost:8080/auth/callback?code=x&state=y#frag").unwrap();
        assert_eq!(
            (s.scheme.as_str(), s.netloc.as_str(), s.path.as_str(), s.query.as_str()),
            ("http", "user@localhost:8080", "/auth/callback", "code=x&state=y")
        );
        assert_eq!(s.port().ok().flatten(), Some(8080));
        assert_eq!(s.hostname(), "localhost");
        let s = urlsplit("/callback?code=1").unwrap();
        assert_eq!((s.path.as_str(), s.query.as_str()), ("/callback", "code=1"));
        assert!(urlsplit("http://h:x/").unwrap().port().is_err());
        assert_eq!(urlsplit("http://[::1]:9/").unwrap().port().ok().flatten(), Some(9));
    }
}
