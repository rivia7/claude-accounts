//! agy 的探测和登录：agy 自己打出 Google 的登录网址、从终端读授权码（登录后 Google 跳到 antigravity.google 的页面上显示授权码，
//! 不回本机）。cca 把网址交给账号专属的 Chrome（服务器上给出在 Mac 上运行的 login-page 那一行），授权码 cca 照常从 fd 3 问、
//! 核对过再写给 agy（它从 stdin 读）。

use super::sys::{interrupted, os_read, run_browser, trap, Kid, Stop};
use super::{agy_login_target, ascii_only, login_hint_url, node_fp_here, q_get, q_some, visible_ascii};
use crate::cmd::table::need;
use crate::cmd::{arg, arity};
use crate::core::base::{EMAIL_RE, RO};
use crate::core::ctx::ctx;
use crate::core::model::{adir, cdir, name_of, node_of, parse_key};
use crate::core::tool::{agy, t, AGY_SSH};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::io;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::{self, Cmd as Proc, In, Out, ProcError};
use crate::py::rand::urandom;
use crate::py::re::{self, Re};
use crate::py::text::{self, sh_quote};
use crate::py::time::time;
use crate::py::url::{parse_qsl, urlsplit};
use crate::py::{die, exit, R};
use crate::{dict, err, err_raw, list};
use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

static AGY_LOGIN_URL: LazyLock<Re> = LazyLock::new(|| Re::new(r"https://accounts\.google\.com/[\x21-\x7e]+"));
static AGY_LOGGED_IN: LazyLock<Re> = LazyLock::new(|| Re::new(r"Logged in as (\S+?)\.?\s*$"));
/// 原来是 `paste the authorization code` 加 re.I。Python 不分大小写时 i 还认 İ（U+0130）和 ı（U+0131），这里的正则库不认，
/// 写明了才一样（s 认 ſ 两边都有）
static AGY_CODE_PROMPT: LazyLock<Re> = LazyLock::new(|| Re::with_flags(r"paste the author[iİı]zat[iİı]on code", re::I));
const AGY_FILE_TOKENS: &str = "Using file-based token storage because SSH session detected";
/// 探测时不带给 agy 的环境变量（原来的 drop；I 的认法同上）
static AGY_PROBE_DROP: LazyLock<Re> =
    LazyLock::new(|| Re::with_flags(r"^(AGY_|ANT[Iİı]GRAV[Iİı]TY_|GEM[Iİı]N[Iİı]_|GOOGLE_|SSH_|(HTTPS?|ALL|NO)_PROXY$)", re::I));

/// `int(st.st_mtime)`：st_mtime 是小数（CPython 里是 `(double)秒 + 1e-9 * 纳秒`），纳秒差一点满一秒时小数放不下、进位成
/// 下一秒。arm64 上编译器把那个式子合成一次乘加（见 link/pylib.rs 的 st_mtime）：取整以后两种算法没有差别，照样分开写
fn mtime_int(sec: i64, nsec: i64) -> i64 {
    let (sec, nsec) = (sec as f64, nsec as f64);
    (if cfg!(target_arch = "aarch64") { 1e-9f64.mul_add(nsec, sec) } else { sec + 1e-9 * nsec }) as i64
}

/// `tempfile.TemporaryDirectory(prefix=…)`：只有自己能进的临时目录（前缀加 8 个随机字符），离开作用域时整个删掉
struct TempDir {
    path: String,
}

impl TempDir {
    fn new(prefix: &str) -> std::io::Result<TempDir> {
        use std::os::unix::fs::DirBuilderExt;
        const NAMES: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
        let base = fs::gettempdir();
        for _ in 0..100 {
            let name: String = urandom(8).iter().map(|b| NAMES[*b as usize % NAMES.len()] as char).collect();
            let p = path::join(&base, &format!("{prefix}{name}"));
            match std::fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => return Ok(TempDir { path: p }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "No usable temporary directory name found"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// `sorted(glob.glob(os.path.join(dir, '*.log')))` 里各个文件的内容接在一起（`errors='replace'`；读不了的跳过）。
/// glob 把整个路径当通配符看：目录这一截里有成对的方括号时，它认的是别的名字，这个目录本身对不上（原来就这样，见
/// FINDINGS）——那种时候当作一份日志也没有。`*`、`?`、不成对的 `[` 照字面也对得上自己，不受影响
fn read_logs(dir: &str) -> String {
    if dir.split('/').any(has_bracket_class) {
        return String::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return String::new() };
    // `*.log`：以 .log 结尾；`*` 不认点开头的名字
    let mut names: Vec<std::ffi::OsString> =
        entries.filter_map(|e| Some(e.ok()?.file_name())).filter(|n| n.as_bytes().ends_with(b".log") && !n.as_bytes().starts_with(b".")).collect();
    names.sort();
    let mut logs = String::new();
    for n in names {
        if let Ok(raw) = std::fs::read(std::path::Path::new(dir).join(n)) {
            logs.push_str(&String::from_utf8_lossy(&raw));
        }
    }
    logs
}

/// 路径的这一截里有没有 fnmatch 会当成字符组的 `[…]`（`[` 后面隔着至少一个字符有 `]`；紧跟着的 `!`、`]` 不算结尾）
fn has_bracket_class(part: &str) -> bool {
    let b = part.as_bytes();
    (0..b.len()).filter(|i| b[*i] == b'[').any(|i| {
        let mut j = i + 1;
        if b.get(j) == Some(&b'!') {
            j += 1;
        }
        if b.get(j) == Some(&b']') {
            j += 1;
        }
        b[j.min(b.len())..].contains(&b']')
    })
}

/// 这个版本的 agy 设了 SSH_CONNECTION 时是不是把登录存进数据目录里的文件（不进钥匙串）：cca 靠它让每个账号的登录分开，
/// 这不是 agy 公开的做法，升级后可能变。在临时的数据目录里跑一次 acp --login（不联网：代理指向关着的端口，stdin 是空的，
/// 它打出登录网址、读不到授权码就退出），看它的日志里有没有那一句。按 agy 程序文件（路径、大小、修改时间）缓存在
/// ~/.cca/agy/probe.json。返回 {'files': bool, ...}
pub fn agy_probe(bin_: &str) -> R<Dict> {
    let real = if bin_.is_empty() { String::new() } else { path::realpath(bin_) };
    let Ok(st) = std::fs::metadata(&real) else {
        return Ok(dict! { "files" => false, "why" => "missing" });
    };
    let sig = list![&real, st.size() as i64, mtime_int(st.mtime(), st.mtime_nsec())];
    let cache = path::join(&agy().sroot, "probe.json");
    if let Some(V::Dict(j)) = load_quiet(&cache) {
        if j.get("sig").is_some_and(|s| s.py_eq(&V::List(sig.clone()))) && j.get("files").is_some_and(|f| f.is_bool()) {
            return Ok(j);
        }
    }
    // os.environ 是字典：同名的变量只有先出现的那个
    let mut env: Vec<(String, String)> = Vec::new();
    for (k, v) in proc::environ() {
        if !AGY_PROBE_DROP.matches(&k) && !env.iter().any(|(have, _)| *have == k) {
            env.push((k, v));
        }
    }
    for (k, v) in [("SSH_CONNECTION", AGY_SSH), ("HTTPS_PROXY", "http://127.0.0.1:9"), ("HTTP_PROXY", "http://127.0.0.1:9"), ("NO_PROXY", "")] {
        proc::env_set(&mut env, k, v);
    }
    let logs = {
        let d = TempDir::new("cca-agy-probe.")?;
        let g = path::join(&d.path, "gemini");
        fs::makedirs(&g)?;
        // 起不来、超时（到点杀掉）都不管：只看它留下的日志
        let _ = Proc::new([real.as_str(), &format!("--gemini_dir={g}"), "acp", "--login"])
            .stdin(In::Null)
            .stdout(Out::Null)
            .stderr(Out::Null)
            .env(env)
            .cwd(d.path.as_str())
            .timeout(30.0)
            .run();
        read_logs(&path::join_all(&[&g, "antigravity-cli", "log"]))
    };
    let j = dict! { "sig" => sig, "files" => logs.contains(AGY_FILE_TOKENS), "checked" => time() as i64 };
    if !*RO {
        save_json(&cache, &V::Dict(j.clone()), Some(0o600), true)?;
    }
    Ok(j)
}

/// bash 的 agy_guard：退出码 0 是确认了登录存进账号目录的文件
pub fn cmd_agy_probe(args: &[String]) -> R<()> {
    arity("cmd_agy_probe", args, 1, 1)?;
    let files = agy_probe(&args[0])?.get("files").is_some_and(|f| f.truthy());
    exit(if files { 0 } else { 1 })
}

/// 服务器上登录：给出在 Mac 上运行的那一行（和 bash 的 remote_login 一样带别名、节点和节点摘要）
fn agy_remote_note(key: &str, url: &str) -> R<()> {
    let (c, table, _) = ctx()?;
    let (a, e) = parse_key(key);
    let node = node_of(&table.at(key));
    let fid = node_fp_here(&c, &node);
    let line = format!(
        "{} login-page {}{} --node {}{} {}",
        t().cmd,
        sh_quote(if e.is_empty() { &a } else { &e }),
        if !a.is_empty() && !e.is_empty() { format!(" --alias {}", sh_quote(&a)) } else { String::new() },
        sh_quote(&node),
        if fid.truthy() { format!(" --node-id {}", fid.py_str()) } else { String::new() },
        sh_quote(url)
    );
    let name = name_of(key);
    err_raw!(
        "\n{name} 要登录：这台机器上没有它专属的浏览器，登录页在 Mac 上打开\n\
         1. 在 Mac 的终端里运行这一行（用 {name} 专属的 Chrome、经同一个节点 {node} 打开登录页；Mac 上还没有这个账号的话，会问要不要加上）：\n\n  \
         {line}\n\n2. 在打开的页面上登录{}。登录完页面上会显示一段授权码\n3. 回到这里粘贴授权码、回车\n\n",
        if e.is_empty() { String::new() } else { format!("（用 {e}）") }
    );
    Ok(())
}

/// 从 fd 3 读一行答案（和 bash 的 ask 一样：终端，或 CCA_TTY 答案文件），提示写 stderr。一个字节一个字节地读，不多读：
/// 答案文件后面的行还要留给 bash。没有 fd 3、读到头是 None
pub fn read_answer(prompt: &str) -> Option<String> {
    read_answer_on(3, prompt).unwrap_or(None)
}

/// `read_answer` 的主体，从 fd 读。卡在读上的时候来了要停下的信号是 `Stop::Interrupt`（原来是从 os.read 里抛出来的
/// KeyboardInterrupt）
fn read_answer_on(fd: RawFd, prompt: &str) -> Result<Option<String>, Stop> {
    err_raw!("{prompt}");
    io::flush_err();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let mut ch = [0u8; 1];
        match os_read(fd, &mut ch) {
            Ok(0) => return Ok(if buf.is_empty() { None } else { Some(String::from_utf8_lossy(&buf).into_owned()) }),
            Ok(_) if ch[0] == b'\n' => return Ok(Some(String::from_utf8_lossy(&buf).into_owned())),
            Ok(_) => buf.push(ch[0]),
            Err(Stop::Interrupt) => return Err(Stop::Interrupt),
            Err(Stop::Fail(_)) => return Ok(None), // except OSError
        }
    }
}

/// 贴回来的授权码有什么不对：整个网址时核对是不是这次登录的（state 对得上），登录页报错时说出来。没问题是空字符串
pub fn agy_code_problem(text: &str, state: &str) -> String {
    if !visible_ascii(text) {
        return "授权码应该是一行字母、数字和符号（不能有空格）".into();
    }
    if !text.starts_with("http://") && !text.starts_with("https://") {
        return String::new();
    }
    let Ok(u) = urlsplit(text) else {
        return "这个网址读不出来".into();
    };
    let q = parse_qsl(&u.query, true);
    if let Some(error) = q_some(&q, "error") {
        return format!("登录页报错：{}", ascii_only(error, None, 80));
    }
    if q_some(&q, "code").is_none() {
        return "这个网址里没有授权码".into();
    }
    if !state.is_empty() && q_get(&q, "state") != Some(state) {
        return "这不是这次登录跳到的网址（state 对不上）：贴最后一次打开的登录页登录后显示的那一个".into();
    }
    String::new()
}

/// 一次 agy 登录里记着的东西（原来 cmd_agy_login 里的 st 和几个内部函数）
struct Login<'a> {
    key: &'a str,
    mode: &'a str,
    /// 进来的时候 3 号描述符开着没有
    has_fd3: bool,
    /// agy 的 stdin
    stdin: Option<std::process::ChildStdin>,
    opened: bool,
    state: String,
    asked: u32,
    said: bool,
}

impl Login<'_> {
    /// 说一句、停下（原来是 raise KeyboardInterrupt，和 Ctrl-C 走同一条路收尾）
    fn cancel(&mut self, msg: &str) -> Result<(), Stop> {
        err!("{msg}");
        self.said = true;
        Err(Stop::Interrupt)
    }

    /// 信号来了没有。Python 在两条字节码之间就会抛出 KeyboardInterrupt；这里在每一步动手之前看一眼（卡着的地方另有 EINTR）
    fn check(&self) -> Result<(), Stop> {
        if interrupted() {
            return Err(Stop::Interrupt);
        }
        Ok(())
    }

    fn ask_code(&mut self) -> Result<(), Stop> {
        let code = loop {
            self.check()?;
            self.asked += 1;
            if self.asked > 5 {
                return self.cancel("试了好几次都没成，已停止登录");
            }
            let prompt = "  授权码（登录后页面上显示的那一段，也可以贴整个网址；直接回车取消）：";
            // 进来时就没有 3 号描述符的：原来是 os.read 报错、当作没有答案。这里不去读它——这个进程后来开的东西
            // （agy 的管道）万一落在 3 号上，会把不相干的内容当成答案
            let answer = if self.has_fd3 {
                read_answer_on(3, prompt)?
            } else {
                err_raw!("{prompt}");
                io::flush_err();
                None
            };
            let code = text::strip(&answer.unwrap_or_default()).to_string();
            if code.is_empty() {
                return self.cancel("已取消登录");
            }
            let why = agy_code_problem(&code, &self.state);
            if why.is_empty() {
                break code;
            }
            err!("  {why}");
        };
        self.check()?;
        // 它已经不读了（管道断了）不算错
        if let Some(stdin) = &mut self.stdin {
            let _ = stdin.write_all(format!("{code}\n").as_bytes());
            let _ = stdin.flush();
        }
        Ok(())
    }

    /// agy 说的一行（或者没有换行的那句提问）
    fn show(&mut self, text: &str) -> Result<(), Stop> {
        self.check()?;
        if let Some(m) = AGY_LOGIN_URL.search(text).filter(|_| !self.opened) {
            self.opened = true;
            let url = login_hint_url(self.key, m.g(0))?;
            self.state = agy_login_target(&url).unwrap_or_default();
            if self.mode == "remote" {
                agy_remote_note(self.key, &url)?;
            } else if run_browser(&url)? != 0 {
                return self.cancel("没能在账号的浏览器里打开登录页，已停止登录");
            }
            return Ok(());
        }
        let email = AGY_LOGGED_IN.search(text).map(|m| m.g(1)).filter(|e| EMAIL_RE.matches(e));
        if let Some(email) = email {
            let j = dict! { "email" => email, "at" => time() as i64 };
            save_json(&path::join(&adir(self.key), "agy-login.json"), &V::Dict(j), Some(0o600), true)?;
        } else if AGY_CODE_PROMPT.found(text) {
            self.ask_code()?;
        } else if !text::strip(text).is_empty() && !text::lstrip(text).starts_with("Open this link in a browser") {
            err!("{text}");
        }
        Ok(())
    }

    /// 把 agy 的输出读到头，一行一行交给 show；返回 agy 的退出码
    fn follow(&mut self, out: RawFd, kid: &mut Kid) -> Result<i32, Stop> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 65536];
        loop {
            self.check()?;
            let n = os_read(out, &mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            while !buf.is_empty() {
                let line: Vec<u8> = if let Some(i) = buf.iter().position(|b| *b == b'\n') {
                    let line = buf[..i].to_vec();
                    buf.drain(..=i);
                    line
                } else if AGY_CODE_PROMPT.found(&String::from_utf8_lossy(&buf)) {
                    // 问授权码的那一句后面没有换行
                    std::mem::take(&mut buf)
                } else {
                    break;
                };
                self.show(String::from_utf8_lossy(&line).trim_end_matches('\r'))?;
            }
        }
        if !buf.is_empty() {
            self.show(&String::from_utf8_lossy(&buf))?;
        }
        kid.wait()
    }
}

/// agy 账号登录（在 bash 设好的账号环境里前台运行）：agy --gemini_dir=账号目录 acp --login 打出 Google 的登录网址，补上
/// login_hint 后回调 cca _browser 在账号专属的 Chrome 里打开（mode=remote：服务器上，给出在 Mac 上运行的 login-page 那一行）。
/// 登录后 Google 跳到 antigravity.google 的页面上显示授权码：cca 照常从 fd 3 问（终端或答案文件），核对过再写给 agy 的 stdin
/// （它自己从 stdin 读；直接回车取消）。agy 的英文提示换成中文，别的输出原样转到 stderr。登录成功时 agy 说的 Logged in as 邮箱
/// 记进账号目录的 agy-login.json（只有 token 文件读不出邮箱时 agy_claims 才用）。退出码是 agy 的
pub fn cmd_agy_login(args: &[String]) -> R<()> {
    arity("cmd_agy_login", args, 1, 2)?;
    let (bin_, mode) = (args[0].as_str(), arg(args, 1, "local"));
    // SAFETY: 只是问一下 3 号描述符开着没有（这时这个进程自己还什么都没开）
    let has_fd3 = unsafe { libc::fcntl(3, libc::F_GETFD) } != -1;
    let Some(key) = proc::getenv("CCA_ACCOUNT").filter(|k| !k.is_empty()) else {
        return die("cca(py): 没有账号");
    };
    let table = ctx()?.1;
    need(&table, &key)?;
    // 和 Ctrl-C 一样：停掉 agy，不留下等授权码的进程
    trap(libc::SIGTERM, false);
    trap(libc::SIGINT, true);
    // stdin=PIPE、stdout=PIPE、stderr=STDOUT（In::Bytes 在 spawn 时就是一条开着的管道）
    let gemini = format!("--gemini_dir={}", cdir(&key));
    let mut child = match Proc::new([bin_, &gemini, "acp", "--login"]).stdin(In::Bytes(Vec::new())).stdout(Out::Capture).stderr(Out::ToStdout).spawn() {
        Ok(child) => child,
        Err(ProcError::Os(e)) => return Err(e.into()),
        Err(ProcError::Timeout { .. }) => unreachable!("spawn 不限时"),
    };
    let mut kid = Kid::of(&child);
    let out = child.stdout.take().expect("stdout 要的是管道");
    let mut login = Login { key: &key, mode, has_fd3, stdin: child.stdin.take(), opened: false, state: String::new(), asked: 0, said: false };
    match login.follow(out.as_raw_fd(), &mut kid) {
        Ok(rc) => exit(rc),
        Err(Stop::Fail(e)) => Err(e),
        Err(Stop::Interrupt) => {
            kid.send(libc::SIGTERM);
            if kid.wait_for(5.0).is_none() {
                kid.send(libc::SIGKILL);
            }
            if !login.said {
                // Ctrl-C、关掉终端
                err_raw!("\n已取消登录\n");
            }
            exit(130)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};

    #[test]
    fn prompt_and_env_regexes_match_like_python() {
        // 右边都是 Python 的 re 给的真假（re.I 下 i 还认 İ、ı，s 还认 ſ）
        for (s, want) in [
            ("Then paste the authorization code, or", true),
            ("PASTE THE AUTHORIZATION CODE", true),
            ("paste the author\u{130}zation code", true),
            ("paste the author\u{131}zation code", true),
            ("pa\u{17f}te the authorization code", true),
            ("pa\u{17f}te the author\u{130}zat\u{131}on code", true),
            ("paste the authori\u{17a}ation code", false),
            ("paste  the authorization code", false),
            ("paste the authorization c\u{f6}de", false),
            ("pa\u{ff53}te the authorization code", false),
        ] {
            assert_eq!(AGY_CODE_PROMPT.found(s), want, "{s}");
        }
        for (s, want) in [
            ("AGY_X", true), ("agy_x", true), ("AGYX", false), ("ANTIGRAVITY_GEMINI_DIR", true), ("GEMINI_API_KEY", true), ("GEM\u{130}N\u{130}_X", true),
            ("gem\u{131}n\u{131}_x", true), ("GOOGLE_API_KEY", true), ("google_x", true), ("SSH_AUTH_SOCK", true), ("\u{17f}\u{17f}h_x", true), ("S\u{17f}H_", true),
            ("HTTP_PROXY", true), ("https_proxy", true), ("HTTPS_PROXY", true), ("ALL_PROXY", true), ("all_proxy", true), ("NO_PROXY", true), ("no_proxy", true),
            ("NO_PROXYX", false), ("HTTP_PROXY\n", true), ("HTTPSS_PROXY", false), ("XHTTP_PROXY", false), ("HTTP_PROXY_X", false), ("PATH", false), ("HOME", false),
            ("CCA_ACCOUNT", false), ("ANT\u{130}GRAV\u{130}TY_", true), ("ant\u{131}grav\u{131}ty_", true), ("NO_PROXY\n\n", false), ("FTP_PROXY", false),
            ("HTTP\u{17f}_PROXY", true), ("http\u{17f}_proxy", true), ("https_proxy\n", true),
        ] {
            assert_eq!(AGY_PROBE_DROP.matches(s), want, "{s:?}");
        }
    }

    #[test]
    fn agy_output_regexes_match_like_python() {
        // 右边是 Python 的 AGY_LOGGED_IN.search(s).group(1)
        let who = |s: &str| AGY_LOGGED_IN.search(s).map(|m| m.g(1).to_string());
        for (s, want) in [
            ("Logged in as alice@example.com.", Some("alice@example.com")),
            ("Logged in as alice@example.com", Some("alice@example.com")),
            ("Logged in as a@b.c..", Some("a@b.c.")),
            ("Logged in as a@b.c (free tier)", None),
            ("ok. Logged in as a@b.c.  \n", Some("a@b.c")),
            ("Logged in as  a@b.c", None),
            ("Logged in as a@b.c.\r", Some("a@b.c")),
            ("logged in as a@b.c", None),
            ("Logged in as x Logged in as y@z.w.", Some("y@z.w")),
            ("Logged in as .", Some(".")),
            ("Logged in as a@b.c.\n\n", Some("a@b.c")),
            ("Logged in as a\u{3000}b", None),
            ("Logged in as a@b.c\u{1c}", Some("a@b.c")),
        ] {
            assert_eq!(who(s).as_deref(), want, "{s:?}");
        }
        assert!(EMAIL_RE.matches("a@b.c.") && !EMAIL_RE.matches("."));
        let url = |s: &str| AGY_LOGIN_URL.search(s).map(|m| m.g(0).to_string());
        for (s, want) in [
            ("see https://accounts.google.com/o/oauth2/auth?a=1&b=2 now", Some("https://accounts.google.com/o/oauth2/auth?a=1&b=2")),
            ("https://accounts.google.com/", None),
            ("https://accounts.google.com/x\ty", Some("https://accounts.google.com/x")),
            ("http://accounts.google.com/x", None),
            ("https://accounts.google.comX/x", None),
            ("a https://accounts.google.com/x https://accounts.google.com/y", Some("https://accounts.google.com/x")),
            ("https://accounts.google.com/中", None),
        ] {
            assert_eq!(url(s).as_deref(), want, "{s:?}");
        }
    }

    #[test]
    fn code_problems() {
        // 右边是 Python 的 agy_code_problem()
        let bad = "授权码应该是一行字母、数字和符号（不能有空格）";
        let other = "这不是这次登录跳到的网址（state 对不上）：贴最后一次打开的登录页登录后显示的那一个";
        let cb = "https://antigravity.google/oauth-callback";
        let long_error = format!("{cb}?error=%E6%8B%92%E7%BB%9Dno%0Away%7F{}", "x".repeat(100));
        for (text, state, want) in [
            ("4/0AbCd-ef_gh", "st-1", ""),
            ("a b", "st-1", bad),
            ("", "st-1", bad),
            ("中文", "st-1", bad),
            (&"x".repeat(8193), "st-1", bad),
            ("code\n", "st-1", ""),
            ("http://x", "st-1", "这个网址里没有授权码"),
            (&format!("{cb}?code=abc&state=st-1"), "st-1", ""),
            (&format!("{cb}?code=abc&state=other"), "st-1", other),
            (&format!("{cb}?code=abc&state=other"), "", ""),
            (&format!("{cb}?state=st-1"), "st-1", "这个网址里没有授权码"),
            (&format!("{cb}?code=&state=st-1"), "st-1", "这个网址里没有授权码"),
            (&format!("{cb}?error=access_denied&code=abc"), "st-1", "登录页报错：access_denied"),
            (&long_error, "st-1", &format!("登录页报错：noway{}", "x".repeat(75))),
            ("https://[bad/?code=1", "st-1", "这个网址读不出来"),
            ("HTTPS://antigravity.google/oauth-callback?state=zzz", "st-1", ""),
            (&format!("{cb}?code=abc"), "st-1", other),
            (&format!("{cb}?code=abc&state=st-1&state=st-2"), "st-1", other),
            ("https://x/?error=%00%01", "st-1", "登录页报错："),
        ] {
            assert_eq!(agy_code_problem(text, state), want, "{text}");
        }
    }

    #[test]
    fn lossy_decoding_is_pythons_replace() {
        // 右边是 Python 的 bytes.decode('utf-8', 'replace')
        for (raw, want) in [
            (&b"ab\xe4\xb8"[..], "ab\u{fffd}"),
            (b"\xf0\x9f\x98", "\u{fffd}"),
            (b"\xc0\xaf", "\u{fffd}\u{fffd}"),
            (b"\xed\xa0\x80", "\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xf4\x90\x80\x80", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xff", "\u{fffd}"),
            (b"a\xffb\xfe", "a\u{fffd}b\u{fffd}"),
            (b"\xe4\xb8\xad\xe6", "中\u{fffd}"),
            (b"\xf0\x9f", "\u{fffd}"),
            (b"\xf8\x88\x80\x80\x80", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xe0\x80\xaf", "\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xf0\x80\x80\xaf", "\u{fffd}\u{fffd}\u{fffd}\u{fffd}"),
            (b"\xc2", "\u{fffd}"),
            (b"\xe1\x80", "\u{fffd}"),
            (b"\x80\x80", "\u{fffd}\u{fffd}"),
            (b"\xef\xbf\xbd", "\u{fffd}"),
            (b"ok", "ok"),
        ] {
            assert_eq!(String::from_utf8_lossy(raw), want, "{raw:x?}");
        }
    }

    #[test]
    fn mtime_rounds_like_a_python_float() {
        // 右边是 Python 的 int(os.stat(f).st_mtime)（os.utime(f, ns=…) 设的时间）
        assert_eq!(mtime_int(1_700_000_000, 0), 1_700_000_000);
        assert_eq!(mtime_int(1_700_000_000, 999_999_000), 1_700_000_000);
        assert_eq!(mtime_int(1_700_000_000, 999_999_999), 1_700_000_001);
        assert_eq!(mtime_int(1_700_000_000, 999_999_880), 1_700_000_000);
        assert_eq!(mtime_int(1_700_000_000, 999_999_881), 1_700_000_001);
        assert_eq!(mtime_int(0, 999_999_999), 0);
        assert_eq!(mtime_int(-1, 500_000_000), 0);
        assert_eq!(mtime_int(-2, 0), -2);
    }

    #[test]
    fn bracket_classes_in_a_path_part() {
        // glob.has_magic 认的不止这些；这里只问“这一截照 fnmatch 是不是对不上它自己”
        for (part, want) in [("plain", false), ("a[b]c", true), ("a[bc", false), ("a]b[c", false), ("[]", false), ("[]]", true), ("[!]", false), ("[!]]", true), ("[!a]", true), ("x[", false), ("a*b?c", false), ("[a][", true)] {
            assert_eq!(has_bracket_class(part), want, "{part}");
        }
    }

    #[test]
    fn logs_are_read_like_glob() {
        let d = TempDir::new("cca-agy-test.").unwrap();
        assert!(path::basename(&d.path).starts_with("cca-agy-test.") && path::basename(&d.path).len() == "cca-agy-test.".len() + 8);
        assert_eq!(fs::mode_of(&d.path).unwrap(), 0o700);
        let put = |name: &str, data: &[u8]| std::fs::write(path::join(&d.path, name), data).unwrap();
        put("b.log", b" session detected\n");
        put("a.log", b"x Using file-based token storage because SSH");
        put(".hidden.log", b"HIDDEN");
        put("c.txt", b"TXT");
        put("d.LOG", b"UPPER");
        put("e.log", b"\xff tail");
        std::fs::create_dir(path::join(&d.path, "dir.log")).unwrap();
        let logs = read_logs(&d.path);
        assert_eq!(logs, "x Using file-based token storage because SSH session detected\n\u{fffd} tail");
        assert!(logs.contains(AGY_FILE_TOKENS)); // 两份日志接在一起才凑成的那一句也算（原来就这样）
        assert_eq!(read_logs(&path::join(&d.path, "nowhere")), "");
        let odd = path::join(&d.path, "t[x]mp");
        std::fs::create_dir(&odd).unwrap();
        std::fs::write(path::join(&odd, "a.log"), "there").unwrap();
        assert_eq!(read_logs(&odd), "");
        let star = path::join(&d.path, "t*m[p");
        std::fs::create_dir(&star).unwrap();
        std::fs::write(path::join(&star, "a.log"), "there").unwrap();
        assert_eq!(read_logs(&star), "there");
        let kept = d.path.clone();
        drop(d);
        assert!(!path::exists(&kept));
    }

    #[test]
    fn answers_are_read_one_byte_at_a_time() {
        let mut fds = [0; 2];
        // SAFETY: fds 放得下两个描述符；两个都交给 OwnedFd 管
        let (r, w) = unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        let mut w = std::fs::File::from(w);
        w.write_all("第一行\r\n\nlast ÿ".as_bytes()).unwrap();
        w.write_all(b"\xfe").unwrap();
        drop(w);
        let ask = || read_answer_on(r.as_raw_fd(), "").unwrap();
        // 一次只读走一行：后面的还在管道里
        assert_eq!(ask().as_deref(), Some("第一行\r"));
        assert_eq!(ask().as_deref(), Some(""));
        assert_eq!(ask().as_deref(), Some("last ÿ\u{fffd}")); // 读到头、没有换行的半行也算一行
        assert_eq!(ask(), None);
        assert_eq!(ask(), None);
        // 没有这个描述符：None
        assert_eq!(read_answer_on(-1, "").unwrap(), None);
    }
}
