//! 起子进程：环境从空的开始、文件描述符一个个指定、换会话 / 接终端，等它退出（带时限）；起过的都记着，测试完了一个不留
//!
//! 和 `std::process::Command` 不一样的地方，都是测试要的：
//! - **环境默认是空的**：子进程看到的只有 [`Cmd::env`] 给的和 [`Cmd::pass_env`] 点了名的，开发者 shell 里的 `HOME`、
//!   `ANTHROPIC_*` 漏不进去。命令只写了名字时，照**子进程的** `PATH` 找（假命令就是这样顶替真命令的）
//! - **标准输入默认是 /dev/null**：测试不该读开发者的终端
//! - **只带着写明的文件描述符**：0、1、2 和 [`Cmd::fd`] 给的，别的一个不带（macOS 上靠
//!   `POSIX_SPAWN_CLOEXEC_DEFAULT`；Linux 上起之前把这个进程 3 号以上的都标成 exec 时关掉）
//! - **信号是干净的**：不屏蔽任何信号，处理方式都是默认的（Rust 的程序不理 SIGPIPE，后台起的 shell 任务不理 SIGINT，
//!   这些都不留给子进程）
//! - **一律 posix_spawn，不 fork**：测试程序是多线程的，macOS 上多线程的进程 fork 出来的子进程偶尔在 exec 之前就被杀掉。
//!   换会话、认控制终端这些只能在子进程里做的事，经 `cca-test exec` 转一道（[`Session`]；进程号不变）
//! - **起过的都登记着**：[`Child`] 离开作用域时还没退出的话，先 TERM、等 2 秒、再 KILL、收走；[`kill_all`] 对所有还活着的
//!   做同一件事（运行器在用例超时、模块收尾时调）

use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// 子进程的标准输入、输出、错误接到哪
#[derive(Debug)]
pub enum Stdio {
    /// 和这个进程用同一个
    Inherit,
    /// /dev/null
    Null,
    /// 管道：这一头在 [`Child`] 的 `stdin` / `stdout` / `stderr` 里
    Piped,
    /// 给定的描述符（文件、管道的一头、伪终端从的那头……）。起完子进程，这边的这一份就关掉
    Fd(OwnedFd),
}

impl From<OwnedFd> for Stdio {
    fn from(fd: OwnedFd) -> Stdio {
        Stdio::Fd(fd)
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::Fd(file.into())
    }
}

/// 子进程在哪个会话里。后三种经 `cca-test exec` 转一道（参数得是 UTF-8）
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Session {
    /// 和这个进程同一个会话、同一个进程组
    #[default]
    Same,
    /// 自己一个新会话（`start_new_session=True`）：没有控制终端，进程组号就是它的进程号
    New,
    /// 新会话，并且把标准输入（要是伪终端从的那头）认作控制终端——在终端里运行的程序就是这个样子（`pty.fork()`）
    NewCtty,
    /// 新会话，然后打开这个终端接成 0、1、2（给 stdin / stdout / stderr 设的不算数）——界面 App 起终端的办法：
    /// Linux 上它就成了控制终端，macOS 上不会
    NewOpenTty(PathBuf),
}

/// 一个槽位：给过的描述符起一次就用掉了
#[derive(Debug)]
enum Slot {
    Inherit,
    Null,
    Piped,
    Fd(Option<OwnedFd>),
}

impl From<Stdio> for Slot {
    fn from(s: Stdio) -> Slot {
        match s {
            Stdio::Inherit => Slot::Inherit,
            Stdio::Null => Slot::Null,
            Stdio::Piped => Slot::Piped,
            Stdio::Fd(fd) => Slot::Fd(Some(fd)),
        }
    }
}

/// 一条命令怎么起。写法和 `std::process::Command` 一样，一路点下去：
///
/// ```ignore
/// let out = Cmd::new(harness::helper()).args(["email", "别名(a@example.com)"]).env("HOME", &home).pass_env(&["PATH"]).output()?;
/// assert_eq!(out.status.py(), 0, "{}", out.err());
/// ```
#[derive(Debug)]
pub struct Cmd {
    prog: OsString,
    arg0: Option<OsString>,
    args: Vec<OsString>,
    /// 照先后做：Some 是设成这个值，None 是去掉
    env: Vec<(OsString, Option<OsString>)>,
    inherit: bool,
    cwd: Option<PathBuf>,
    stdin: Option<Slot>,
    stdout: Option<Slot>,
    stderr: Option<Slot>,
    input: Option<Vec<u8>>,
    fds: Vec<(RawFd, Option<OwnedFd>)>,
    session: Session,
    timeout: Option<Duration>,
}

impl Cmd {
    /// 程序：带 / 的是路径（相对路径从子进程的当前目录算），只有名字的照子进程的 PATH 找
    pub fn new(prog: impl AsRef<OsStr>) -> Cmd {
        Cmd {
            prog: prog.as_ref().to_os_string(),
            arg0: None,
            args: Vec::new(),
            env: Vec::new(),
            inherit: false,
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
            input: None,
            fds: Vec::new(),
            session: Session::Same,
            timeout: None,
        }
    }

    /// 添一个参数
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Cmd {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    /// 添几个参数
    pub fn args<S: AsRef<OsStr>>(&mut self, args: impl IntoIterator<Item = S>) -> &mut Cmd {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// 程序看到的 argv[0]（默认就是 new 里写的那个）
    pub fn arg0(&mut self, name: impl AsRef<OsStr>) -> &mut Cmd {
        self.arg0 = Some(name.as_ref().to_os_string());
        self
    }

    /// 给子进程设一个环境变量（同一个名字后设的算数）
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Cmd {
        self.env.push((key.as_ref().to_os_string(), Some(value.as_ref().to_os_string())));
        self
    }

    /// 设几个环境变量（一个个 [`Cmd::env`] 的简写）
    pub fn envs<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, vars: impl IntoIterator<Item = (K, V)>) -> &mut Cmd {
        for (k, v) in vars {
            self.env(k, v);
        }
        self
    }

    /// 去掉一个环境变量（前面设过的、[`Cmd::env_inherit`] 带进来的）
    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Cmd {
        self.env.push((key.as_ref().to_os_string(), None));
        self
    }

    /// 把这个进程的这几个环境变量原样带给子进程（没设的不带）。原来的 Python 测试是在 `os.environ` 上改的，搬过来时
    /// 点名带 `PATH`、`LANG`、`LC_ALL`、`LC_CTYPE`、`USER`、`LOGNAME` 这几个就够了；`HOME` 一律指到临时目录，不带真的
    pub fn pass_env(&mut self, keys: &[&str]) -> &mut Cmd {
        for key in keys {
            if let Some(value) = std::env::var_os(key) {
                self.env(key, value);
            }
        }
        self
    }

    /// 整个环境都带给子进程（再照 env / env_remove 改）。只给 cca-test 里那些“替别人起程序”的角色和运行器自己用：
    /// tests/*.rs 里不许用（开发者 shell 里的东西会漏进被测的程序）
    pub fn env_inherit(&mut self) -> &mut Cmd {
        self.inherit = true;
        self
    }

    /// 子进程的当前目录
    pub fn cwd(&mut self, dir: impl AsRef<Path>) -> &mut Cmd {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    /// 标准输入；默认 /dev/null
    pub fn stdin(&mut self, to: impl Into<Stdio>) -> &mut Cmd {
        self.stdin = Some(to.into().into());
        self
    }

    /// 标准输出；[`Cmd::spawn`] 默认和这个进程用同一个，[`Cmd::output`] 默认收回来
    pub fn stdout(&mut self, to: impl Into<Stdio>) -> &mut Cmd {
        self.stdout = Some(to.into().into());
        self
    }

    /// 标准错误；默认同 [`Cmd::stdout`]
    pub fn stderr(&mut self, to: impl Into<Stdio>) -> &mut Cmd {
        self.stderr = Some(to.into().into());
        self
    }

    /// [`Cmd::output`] 时喂给标准输入的内容（喂完就关上；`subprocess.run(input=…)`）
    pub fn input(&mut self, data: impl Into<Vec<u8>>) -> &mut Cmd {
        self.input = Some(data.into());
        self
    }

    /// 多带一个描述符：子进程里它是 child_fd 号（`pass_fds`，但号码由你定）。起完子进程，这边的这一份就关掉
    pub fn fd(&mut self, child_fd: RawFd, src: OwnedFd) -> &mut Cmd {
        self.fds.push((child_fd, Some(src)));
        self
    }

    /// 子进程在哪个会话里（默认和这个进程同一个），见 [`Session`]
    pub fn session(&mut self, session: Session) -> &mut Cmd {
        self.session = session;
        self
    }

    /// 时限，从起来的那一刻算：[`Cmd::output`]、[`Child::wait`] 等到这时还没完就杀掉（KILL），返回 `TimedOut` 的错
    pub fn timeout(&mut self, limit: Duration) -> &mut Cmd {
        self.timeout = Some(limit);
        self
    }

    /// 子进程最后看到的环境
    fn final_env(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = if self.inherit { std::env::vars_os().collect() } else { Vec::new() };
        for (key, value) in &self.env {
            let at = env.iter().position(|(k, _)| k == key);
            match (value, at) {
                (Some(v), Some(i)) => env[i].1 = v.clone(),
                (Some(v), None) => env.push((key.clone(), v.clone())),
                (None, Some(i)) => drop(env.remove(i)),
                (None, None) => {}
            }
        }
        env
    }

    /// 出错的说明里怎么称呼这条命令
    fn what(&self) -> String {
        let mut text = self.prog.to_string_lossy().into_owned();
        for arg in &self.args {
            text.push(' ');
            text.push_str(&arg.to_string_lossy());
        }
        if text.chars().count() > 200 {
            text = text.chars().take(200).collect::<String>() + "…";
        }
        text
    }

    /// 起来就返回，不等它
    pub fn spawn(&mut self) -> io::Result<Child> {
        self.start(false)
    }

    /// 起来、喂完 [`Cmd::input`]、把标准输出和标准错误收齐、等它退出（`subprocess.run(capture_output=True)`）。
    /// 两路输出是一起收的，哪一路写得再多也不会卡住。设了 [`Cmd::timeout`]、到时限还没完：杀掉，返回 `TimedOut` 的错，
    /// 说明里带着到那时为止的输出
    pub fn output(&mut self) -> io::Result<Output> {
        let mut child = self.start(true)?;
        let input = self.input.clone().unwrap_or_default();
        let (stdout, stderr, in_time) = pump(&mut child, &input)?;
        let exit = if in_time { child.wait_until(child.deadline)? } else { None };
        match exit {
            Some(status) => Ok(Output { status, stdout, stderr }),
            None => {
                child.kill_and_reap();
                let tail = |b: &[u8]| String::from_utf8_lossy(&b[b.len().saturating_sub(2000)..]).into_owned();
                let limit = self.timeout.unwrap_or_default().as_secs_f64();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} 过了 {limit} 秒还没完，杀掉了\n标准输出的末尾：{}\n标准错误的末尾：{}",
                        child.what,
                        tail(&stdout),
                        tail(&stderr)
                    ),
                ))
            }
        }
    }

    fn start(&mut self, capture: bool) -> io::Result<Child> {
        let invalid = |why: String| io::Error::new(io::ErrorKind::InvalidInput, why);
        let what = self.what();
        let env = self.final_env();
        let cwd = match &self.cwd {
            Some(dir) => {
                let dir = absolute(dir, None);
                if !dir.is_dir() {
                    return Err(io::Error::new(io::ErrorKind::NotFound, format!("起不了 {what}：目录 {} 不在", dir.display())));
                }
                Some(dir)
            }
            None => None,
        };
        let prog = resolve(&self.prog, &env, cwd.as_deref())?;

        // 程序和参数：换会话的经 cca-test exec 转一道
        let name = self.arg0.clone().unwrap_or_else(|| self.prog.clone());
        let (path, argv) = if self.session == Session::Same {
            let mut argv = vec![name];
            argv.extend(self.args.iter().cloned());
            (prog, argv)
        } else {
            let mut argv: Vec<OsString> = vec!["cca-test".into(), "exec".into(), "--setsid".into()];
            match &self.session {
                Session::NewCtty => argv.push("--ctty".into()),
                Session::NewOpenTty(tty) => argv.extend(["--tty-after-setsid".into(), tty.clone().into_os_string()]),
                _ => {}
            }
            argv.extend(["--arg0".into(), name, "--".into(), prog.into_os_string()]);
            argv.extend(self.args.iter().cloned());
            if let Some(bad) = argv.iter().find(|a| a.to_str().is_none()) {
                return Err(invalid(format!("起不了 {what}：换会话要经 cca-test exec，它只收 UTF-8 的参数（{bad:?} 不是）")));
            }
            (tool()?, argv)
        };

        // 0、1、2 号各接到哪
        let stdin = match &mut self.stdin {
            _ if capture && self.input.is_some() => Slot::Piped,
            Some(slot) => take(slot, &what)?,
            None => Slot::Null,
        };
        let default_out = || if capture { Slot::Piped } else { Slot::Inherit };
        let stdout = match &mut self.stdout {
            Some(slot) => take(slot, &what)?,
            None => default_out(),
        };
        let stderr = match &mut self.stderr {
            Some(slot) => take(slot, &what)?,
            None => default_out(),
        };
        let mut plan = Plan::default();
        let mut ends: [Option<File>; 3] = [None, None, None];
        let open_tty = matches!(self.session, Session::NewOpenTty(_));
        for (target, slot) in [stdin, stdout, stderr].into_iter().enumerate() {
            let target = target as RawFd;
            match slot {
                // 这种会话里 0、1、2 由 cca-test exec 换成那个终端：这里给什么都一样，给 /dev/null
                _ if open_tty => plan.null(target),
                Slot::Null => plan.null(target),
                Slot::Inherit => plan.inherit(target),
                Slot::Fd(fd) => plan.give(target, fd.expect("take 给的是还在的")),
                Slot::Piped => {
                    let (read, write) = io::pipe()?;
                    let (ours, theirs): (OwnedFd, OwnedFd) =
                        if target == 0 { (write.into(), read.into()) } else { (read.into(), write.into()) };
                    ends[target as usize] = Some(File::from(ours));
                    plan.give(target, theirs);
                }
            }
        }
        for (child_fd, src) in &mut self.fds {
            if *child_fd < 3 {
                return Err(invalid(format!("起不了 {what}：0、1、2 号用 stdin / stdout / stderr 设，fd() 只管 3 号以上的")));
            }
            if plan.targets.iter().any(|(t, _)| t == child_fd) {
                return Err(invalid(format!("起不了 {what}：{child_fd} 号给了两次")));
            }
            let Some(src) = src.take() else { return Err(spent(&what)) };
            plan.give(*child_fd, src);
        }

        let leader = self.session != Session::Same;
        // 起和登记拿着登记表的锁一起做：kill_all 看到的进程要么还没起、要么已经登记了，不会有漏在中间的
        let pid = {
            let mut reg = registry();
            if CLOSED.load(Ordering::SeqCst) {
                return Err(io::Error::other(format!("起不了 {what}：运行器在收场，不再起子进程")));
            }
            let pid = plan
                .spawn(&path, &argv, &env, cwd.as_deref())
                .map_err(|e| io::Error::new(e.kind(), format!("起不了 {what}：{e}")))?;
            reg.push(Entry { pid, leader, done: None });
            pid
        };
        let [stdin, stdout, stderr] = ends;
        Ok(Child {
            pid,
            stdin,
            stdout,
            stderr,
            exit: None,
            usage: None,
            leader,
            deadline: self.timeout.map(|t| Instant::now() + t),
            what,
        })
    }
}

fn spent(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("起不了 {what}：这个 Cmd 里给的描述符已经交给上一个子进程了，再起要新建一个 Cmd"),
    )
}

/// 把槽位里的设置拿出来用（描述符只能拿一次）
fn take(slot: &mut Slot, what: &str) -> io::Result<Slot> {
    Ok(match slot {
        Slot::Inherit => Slot::Inherit,
        Slot::Null => Slot::Null,
        Slot::Piped => Slot::Piped,
        Slot::Fd(fd) => Slot::Fd(Some(fd.take().ok_or_else(|| spent(what))?)),
    })
}

/// 相对路径从 base（没给就是这个进程的当前目录）算
fn absolute(path: &Path, base: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match base {
        Some(base) => base.join(path),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path),
    }
}

/// 是文件、能执行
fn runnable(path: &Path) -> io::Result<()> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "不是文件"));
    }
    let c =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径里有 0 字节"))?;
    // SAFETY: c 是以 0 结尾的路径，access 只是问一下
    if unsafe { libc::access(c.as_ptr(), libc::X_OK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 在 PATH（冒号隔开的目录）里找 name：头一个“是文件、能执行”的。空的一项、相对的一项从 base 算
fn search(name: &OsStr, path: &OsStr, base: Option<&Path>) -> io::Result<PathBuf> {
    let mut denied = false;
    for dir in path.as_bytes().split(|b| *b == b':') {
        let dir = if dir.is_empty() { Path::new(".") } else { Path::new(OsStr::from_bytes(dir)) };
        let candidate = absolute(dir, base).join(name);
        match runnable(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) => denied |= e.kind() == io::ErrorKind::PermissionDenied,
        }
    }
    let (kind, why) =
        if denied { (io::ErrorKind::PermissionDenied, "找到的不能执行") } else { (io::ErrorKind::NotFound, "没有") };
    Err(io::Error::new(kind, format!("找不到命令 {}：PATH（{}）里{why}", name.to_string_lossy(), path.to_string_lossy())))
}

/// 没有 PATH 时照这个找（和 execvp 一样）
const DEFAULT_PATH: &str = "/usr/bin:/bin";

/// 要起的程序到底是哪个文件：带 / 的是路径（相对的从子进程的当前目录算），只有名字的照子进程的 PATH 找
fn resolve(prog: &OsStr, env: &[(OsString, OsString)], cwd: Option<&Path>) -> io::Result<PathBuf> {
    if prog.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "没写要起哪个程序"));
    }
    if prog.as_bytes().contains(&b'/') {
        let path = absolute(Path::new(prog), cwd);
        return match runnable(&path) {
            Ok(()) => Ok(path),
            Err(e) => Err(io::Error::new(e.kind(), format!("起不了 {}：{e}", path.display()))),
        };
    }
    let path = env.iter().find(|(k, _)| k == "PATH").map_or(OsStr::new(DEFAULT_PATH), |(_, v)| v.as_os_str());
    search(prog, path, cwd)
}

/// 这台机器上现在的进程：（进程号，整条命令行：各个参数用空格接起来）。看不了是 Err。
/// Linux 上读 /proc/<进程号>/cmdline：busybox 的 ps 不认 `-axww -o pid=,command=`，最小安装的发行版连 ps、pgrep 都没有
/// （没有命令行的——内核线程——不算）
#[cfg(target_os = "linux")]
pub fn processes() -> io::Result<Vec<(i32, String)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else { continue };
        // 列出来以后退出了的读不到：跳过
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let command = String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_string();
        if !command.is_empty() {
            found.push((pid, command));
        }
    }
    Ok(found)
}

/// 这台机器上现在的进程：（进程号，整条命令行）。看不了是 Err。问 `/bin/ps -axww -o pid=,command=`
#[cfg(not(target_os = "linux"))]
pub fn processes() -> io::Result<Vec<(i32, String)>> {
    let mut ps = Cmd::new("/bin/ps");
    ps.args(["-axww", "-o", "pid=,command="]).stderr(Stdio::Null).timeout(Duration::from_secs(20));
    let table = ps.output()?.out();
    let rows = table.lines().filter_map(|line| line.trim_start().split_once(' '));
    Ok(rows.filter_map(|(pid, command)| Some((pid.parse::<i32>().ok()?, command.trim().to_string()))).collect())
}

/// 照**这个进程**的 PATH 找一个命令（`shutil.which`）：看机器上装没装 openssl、node 这些
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| DEFAULT_PATH.into());
    search(OsStr::new(name), &path, None).ok()
}

static TOOL: OnceLock<PathBuf> = OnceLock::new();

/// 告诉这个模块 cca-test 在哪（运行器一开始就调；只有头一次算数）
pub fn set_tool(path: impl Into<PathBuf>) {
    let _ = TOOL.set(path.into());
}

/// cca-test 在哪：[`set_tool`] 给的，没有就看环境里的 `CCA_TEST_BIN`
pub fn tool() -> io::Result<PathBuf> {
    if let Some(path) = TOOL.get() {
        return Ok(path.clone());
    }
    match std::env::var_os("CCA_TEST_BIN") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err(io::Error::new(io::ErrorKind::NotFound, "不知道 cca-test 在哪（换会话、接终端要经它）：设 CCA_TEST_BIN")),
    }
}

fn cvt(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// posix_spawn 那一族出错时直接返回错误号
fn check(rc: libc::c_int) -> io::Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(rc))
    }
}

fn cstring(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{s:?} 里有 0 字节")))
}

/// 子进程的一个描述符从哪来
#[derive(Debug)]
enum Source {
    /// 打开 /dev/null
    Null,
    /// 这个进程的同一个号（0、1、2 的“继承”）
    Same,
    /// 给定的（起完就关）
    Given(OwnedFd),
}

/// 子进程的描述符表：几号接到哪
#[derive(Debug, Default)]
struct Plan {
    targets: Vec<(RawFd, Source)>,
}

impl Plan {
    fn null(&mut self, target: RawFd) {
        self.targets.push((target, Source::Null));
    }

    fn inherit(&mut self, target: RawFd) {
        self.targets.push((target, Source::Same));
    }

    fn give(&mut self, target: RawFd, fd: OwnedFd) {
        self.targets.push((target, Source::Given(fd)));
    }

    /// posix_spawn。给定的描述符在这里用掉：不管成不成，返回时这边的那几份都关了
    fn spawn(self, path: &Path, argv: &[OsString], env: &[(OsString, OsString)], cwd: Option<&Path>) -> io::Result<libc::pid_t> {
        let path_c = cstring(path.as_os_str())?;
        let argv_c = argv.iter().map(|a| cstring(a)).collect::<io::Result<Vec<_>>>()?;
        let mut env_c = Vec::with_capacity(env.len());
        for (key, value) in env {
            if key.is_empty() || key.as_bytes().contains(&b'=') {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("环境变量的名字 {key:?} 不对")));
            }
            let mut pair = key.clone().into_vec();
            pair.push(b'=');
            pair.extend_from_slice(value.as_bytes());
            env_c.push(cstring(OsStr::from_bytes(&pair))?);
        }
        let cwd_c = cwd.map(|d| cstring(d.as_os_str())).transpose()?;
        let ptrs = |list: &[CString]| {
            list.iter().map(|c| c.as_ptr() as *mut libc::c_char).chain([std::ptr::null_mut()]).collect::<Vec<_>>()
        };
        let (argv_p, env_p) = (ptrs(&argv_c), ptrs(&env_c));

        // 先把每个来源在高处另开一份（比要放的号都大，exec 时关掉），子进程里再从那一份 dup2 到要放的号：
        // 这样“3 号放到 4 号、4 号放到 3 号”也不会互相踩，来源正好就是目标号时也照样带得过去
        let floor = self.targets.iter().map(|(t, _)| *t + 1).max().unwrap_or(3).max(3);
        let mut actions = Actions::new()?;
        let mut held = Vec::new();
        for (target, source) in &self.targets {
            let from = match source {
                Source::Null => {
                    let flags = if *target == 0 { libc::O_RDONLY } else { libc::O_WRONLY };
                    // SAFETY: actions 初始化过；路径是以 0 结尾的常量
                    check(unsafe {
                        libc::posix_spawn_file_actions_addopen(actions.ptr(), *target, c"/dev/null".as_ptr(), flags, 0)
                    })?;
                    continue;
                }
                Source::Same => *target,
                Source::Given(fd) => fd.as_raw_fd(),
            };
            // SAFETY: 只是另开一份描述符；成了马上交给 OwnedFd 管
            let copy = match cvt(unsafe { libc::fcntl(from, libc::F_DUPFD_CLOEXEC, floor) }) {
                Ok(fd) => unsafe { OwnedFd::from_raw_fd(fd) },
                // 这个进程自己的 0、1、2 本来就关着：子进程里也让它关着
                Err(e) if matches!(source, Source::Same) && e.raw_os_error() == Some(libc::EBADF) => continue,
                Err(e) => return Err(e),
            };
            // SAFETY: actions 初始化过
            check(unsafe { libc::posix_spawn_file_actions_adddup2(actions.ptr(), copy.as_raw_fd(), *target) })?;
            held.push(copy);
        }
        if let Some(dir) = &cwd_c {
            // SAFETY: actions 初始化过；dir 是以 0 结尾的路径
            check(unsafe { libc::posix_spawn_file_actions_addchdir_np(actions.ptr(), dir.as_ptr()) })?;
        }
        let mut attr = Attr::new()?;
        attr.clean_signals()?;

        #[cfg(target_os = "linux")]
        cloexec_all();
        let mut pid: libc::pid_t = 0;
        // SAFETY: 各个指针指的东西（路径、参数表、环境表、actions、attr）在这次调用期间都有效，表以空指针结尾
        check(unsafe { libc::posix_spawn(&mut pid, path_c.as_ptr(), actions.ptr(), attr.ptr(), argv_p.as_ptr(), env_p.as_ptr()) })?;
        Ok(pid)
    }
}

/// posix_spawn 的 file actions（离开作用域时销毁）
struct Actions(MaybeUninit<libc::posix_spawn_file_actions_t>);

impl Actions {
    fn new() -> io::Result<Actions> {
        let mut actions = Actions(MaybeUninit::uninit());
        // SAFETY: init 把它填好
        check(unsafe { libc::posix_spawn_file_actions_init(actions.0.as_mut_ptr()) })?;
        Ok(actions)
    }

    fn ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: new 里初始化过
        unsafe { libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr()) };
    }
}

/// posix_spawn 的属性（离开作用域时销毁）
struct Attr(MaybeUninit<libc::posix_spawnattr_t>);

impl Attr {
    fn new() -> io::Result<Attr> {
        let mut attr = Attr(MaybeUninit::uninit());
        // SAFETY: init 把它填好
        check(unsafe { libc::posix_spawnattr_init(attr.0.as_mut_ptr()) })?;
        Ok(attr)
    }

    fn ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        self.0.as_mut_ptr()
    }

    /// 子进程不屏蔽任何信号，1 到 31 号的处理方式都是默认的（KILL、STOP 本来就改不了）；macOS 上再加“没写明的描述符一个不带”
    fn clean_signals(&mut self) -> io::Result<()> {
        // SAFETY: 两个信号集先清空再用；attr 初始化过
        unsafe {
            let mut none = MaybeUninit::<libc::sigset_t>::uninit();
            let mut all = MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(none.as_mut_ptr());
            libc::sigemptyset(all.as_mut_ptr());
            for sig in 1..32 {
                if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                    libc::sigaddset(all.as_mut_ptr(), sig);
                }
            }
            check(libc::posix_spawnattr_setsigmask(self.ptr(), none.as_ptr()))?;
            check(libc::posix_spawnattr_setsigdefault(self.ptr(), all.as_ptr()))?;
            #[allow(unused_mut)]
            let mut flags = libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF;
            #[cfg(target_os = "macos")]
            {
                flags |= libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            }
            check(libc::posix_spawnattr_setflags(self.ptr(), flags as libc::c_short))
        }
    }
}

/// 把这个进程 3 号以上的描述符都标成 exec 时关掉：之后起的子进程只拿得到 0、1、2（Python 的 subprocess 的 close_fds=True）。
/// Linux 上没有“没写明的描述符一个不带”的开关，[`Cmd`] 起子进程之前先做这一步（要带给子进程的是在子进程里 dup2 出来的，
/// 不受这个标记影响）；macOS 上 [`Cmd`] 用不着它，那里是给经 std::process::Command 起子进程的假命令（tests/tool/fake）用的
pub fn cloexec_all() {
    let Ok(dir) = std::fs::read_dir(if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" }) else { return };
    let fds: Vec<RawFd> = dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).filter(|fd| *fd >= 3).collect();
    for fd in fds {
        // SAFETY: 只是给这个描述符设标记（列目录用的那个这时已经关了：EBADF，不管）
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
}

/// 子进程是怎么结束的
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    /// 自己退出的：退出码
    pub code: Option<i32>,
    /// 被信号杀掉的：哪个信号
    pub signal: Option<i32>,
}

impl Exit {
    fn from_status(status: libc::c_int) -> Exit {
        if libc::WIFSIGNALED(status) {
            Exit { code: None, signal: Some(libc::WTERMSIG(status)) }
        } else {
            Exit { code: Some(libc::WEXITSTATUS(status)), signal: None }
        }
    }

    /// 退出码是 0
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Python 的 `returncode`：退出码，被信号杀掉的是负的信号号
    pub fn py(&self) -> i32 {
        match (self.code, self.signal) {
            (Some(code), _) => code,
            (None, Some(sig)) => -sig,
            (None, None) => -1,
        }
    }

    /// shell 的 `$?`：退出码，被信号杀掉的是 128 + 信号号
    pub fn shell(&self) -> i32 {
        match (self.code, self.signal) {
            (Some(code), _) => code,
            (None, Some(sig)) => 128 + sig,
            (None, None) => 255,
        }
    }
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (Some(code), _) => write!(f, "退出码 {code}"),
            (None, Some(sig)) => write!(f, "被信号 {sig} 杀掉"),
            (None, None) => write!(f, "不知道怎么结束的"),
        }
    }
}

/// 子进程用掉的 CPU 时间（等到它退出才有）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Usage {
    /// 用户态
    pub user: Duration,
    /// 内核态
    pub sys: Duration,
}

impl Usage {
    /// 用户态加内核态
    pub fn cpu(&self) -> Duration {
        self.user + self.sys
    }
}

/// [`Cmd::output`] 的结果
#[derive(Debug)]
pub struct Output {
    /// 怎么结束的
    pub status: Exit,
    /// 标准输出的原样字节（当文字用 [`Output::out`]）
    pub stdout: Vec<u8>,
    /// 标准错误的原样字节（当文字用 [`Output::err`]）
    pub stderr: Vec<u8>,
}

impl Output {
    /// 标准输出当文字（不是 UTF-8 的字节换成 U+FFFD）
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// 标准错误当文字
    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// 退出码不是 0 就 panic，带上两路输出（`check=True`）
    pub fn check(self) -> Output {
        if !self.status.success() {
            panic!("命令没成（{}）\n标准输出：{}\n标准错误：{}", self.status, self.out(), self.err());
        }
        self
    }
}

/// 登记表里的一项
struct Entry {
    pid: libc::pid_t,
    /// 是自己那个会话（进程组）的头一个：收场时连进程组一起发信号
    leader: bool,
    /// 已经收走了（[`kill_all`] 收的，留给它的 Child 来取）
    done: Option<(Exit, Usage)>,
}

/// 起过、还没被它的 [`Child`] 收走的子进程。收（wait4）和发信号都拿着这把锁做：登记着的进程号一定还是我们的子进程
/// （没收走的进程号不会被系统拿去给别的进程用），不会把信号发错
static REGISTRY: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn registry() -> MutexGuard<'static, Vec<Entry>> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

impl Entry {
    /// 收一次，不等：退出了没有
    fn poll(&mut self) -> bool {
        if self.done.is_some() {
            return true;
        }
        let mut status = 0;
        // SAFETY: status、usage 是有效的，wait4 往里写；usage 全是数字，清零就是有效的值
        let (rc, usage) = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            let rc = loop {
                let rc = libc::wait4(self.pid, &mut status, libc::WNOHANG, &mut usage);
                if rc >= 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break rc;
                }
            };
            (rc, usage)
        };
        let time = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
        if rc == self.pid {
            self.done = Some((Exit::from_status(status), Usage { user: time(usage.ru_utime), sys: time(usage.ru_stime) }));
        } else if rc < 0 {
            // 不是我们能收的了（别处用 waitpid(-1) 收走了）：怎么结束的不知道，当它结束了
            self.done = Some((Exit { code: None, signal: None }, Usage { user: Duration::ZERO, sys: Duration::ZERO }));
        }
        self.done.is_some()
    }

    fn send(&self, sig: libc::c_int) {
        // SAFETY: 登记着的进程号还是我们的子进程（见 REGISTRY）；它带头的进程组号就是它的进程号
        unsafe {
            libc::kill(self.pid, sig);
            if self.leader {
                libc::kill(-self.pid, sig);
            }
        }
    }
}

/// 等登记着的都退出，最多等 limit；返回是不是都退出了
fn all_done(limit: Duration) -> bool {
    let end = Instant::now() + limit;
    loop {
        if registry().iter_mut().all(|e| e.poll()) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 把起过、还活着的子进程都停掉：TERM，等 2 秒，剩下的 KILL，收走（新会话起的连它那个进程组一起）。
/// 返回调用时还活着几个。运行器在用例超时、模块收尾时调；测试里一般不用自己调
pub fn kill_all() -> usize {
    let mut alive = 0;
    for entry in registry().iter_mut() {
        if !entry.poll() {
            alive += 1;
            entry.send(libc::SIGTERM);
        }
    }
    if alive > 0 && !all_done(Duration::from_secs(2)) {
        for entry in registry().iter_mut() {
            if !entry.poll() {
                entry.send(libc::SIGKILL);
            }
        }
        all_done(Duration::from_secs(2));
    }
    alive
}

/// 收场了：不再起新的子进程
static CLOSED: AtomicBool = AtomicBool::new(false);

/// [`kill_all`]，并且从此 [`Cmd::spawn`]、[`Cmd::output`] 一律报错。运行器收场时调：这个进程马上要退出，
/// 还卡着的用例那条线程不能再起出没人管的进程
pub(crate) fn close_all() -> usize {
    {
        let _reg = registry();
        CLOSED.store(true, Ordering::SeqCst);
    }
    kill_all()
}

/// 一个起来了的子进程。离开作用域时还没退出的话：TERM，等 2 秒，KILL，收走（见 [`Child::stop`]）
#[derive(Debug)]
pub struct Child {
    pid: libc::pid_t,
    /// 设成 [`Stdio::Piped`] 时管道在这边的那一头：`take()` 出来写，写完 drop 掉子进程就读到头
    pub stdin: Option<File>,
    /// 设成 [`Stdio::Piped`] 时这边的那一头：读它就是读子进程的标准输出
    pub stdout: Option<File>,
    /// 设成 [`Stdio::Piped`] 时这边的那一头：子进程的标准错误
    pub stderr: Option<File>,
    exit: Option<Exit>,
    usage: Option<Usage>,
    leader: bool,
    deadline: Option<Instant>,
    what: String,
}

impl Child {
    /// 进程号（换会话的也是那个程序自己的进程号：cca-test exec 不另起进程）
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// 退出了没有（不等）。退出了的话顺手收走，以后再问都是同一个结果
    pub fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        if self.exit.is_none() {
            let mut reg = registry();
            if let Some(i) = reg.iter().position(|e| e.pid == self.pid) {
                if reg[i].poll() {
                    let (exit, usage) = reg.swap_remove(i).done.expect("poll 说收到了");
                    (self.exit, self.usage) = (Some(exit), Some(usage));
                }
            }
        }
        Ok(self.exit)
    }

    /// 等它退出。这边还开着的标准输入先关上（等输入的程序才会结束）。起的时候设了 [`Cmd::timeout`]、到时限还没退出：
    /// 杀掉（KILL）、收走，返回 `TimedOut` 的错
    pub fn wait(&mut self) -> io::Result<Exit> {
        drop(self.stdin.take());
        match self.wait_until(self.deadline)? {
            Some(exit) => Ok(exit),
            None => {
                self.kill_and_reap();
                Err(io::Error::new(io::ErrorKind::TimedOut, format!("{} 到时限还没退出，杀掉了", self.what)))
            }
        }
    }

    /// 最多等 limit：退出了是 Some；还在跑是 None（不杀它，可以接着等）
    pub fn wait_timeout(&mut self, limit: Duration) -> io::Result<Option<Exit>> {
        self.wait_until(Some(Instant::now() + limit))
    }

    /// 等到 end（None：一直等）
    fn wait_until(&mut self, end: Option<Instant>) -> io::Result<Option<Exit>> {
        let mut step = Duration::from_micros(200);
        loop {
            if let Some(exit) = self.try_wait()? {
                return Ok(Some(exit));
            }
            match end {
                // 没有时限：让系统在它退出时叫醒（不收走，收还是回到上面拿着锁做）；这一招不灵就照下面隔一会儿看一次
                None if block_until_exit(self.pid) => continue,
                Some(end) if Instant::now() >= end => return Ok(None),
                _ => {}
            }
            std::thread::sleep(step);
            step = (step * 3 / 2).min(Duration::from_millis(5));
        }
    }

    /// 给它发一个信号（`libc::SIGTERM` 这样的号）；已经收走了的不发、不算错
    pub fn signal(&self, sig: i32) -> io::Result<()> {
        self.send(sig, false)
    }

    /// 给它带头的那个进程组发信号（`os.killpg`）：只有新会话起的（[`Session`] 不是 `Same`）才有自己的进程组
    pub fn signal_group(&self, sig: i32) -> io::Result<()> {
        if !self.leader {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} 不是新会话起的，没有自己的进程组", self.what)));
        }
        self.send(sig, true)
    }

    fn send(&self, sig: i32, group: bool) -> io::Result<()> {
        let reg = registry();
        if self.exit.is_some() || !reg.iter().any(|e| e.pid == self.pid && e.done.is_none()) {
            return Ok(());
        }
        // SAFETY: 拿着登记表的锁、它还登记着没收走：这个进程号还是我们的子进程
        cvt(unsafe { libc::kill(if group { -self.pid } else { self.pid }, sig) }).map(drop)
    }

    /// 发 KILL（不等：之后 [`Child::wait`] 收走）
    pub fn kill(&mut self) -> io::Result<()> {
        self.signal(libc::SIGKILL)
    }

    /// KILL（新会话起的连进程组一起），等到收走为止（最多 5 秒）
    fn kill_and_reap(&mut self) {
        if let Some(entry) = registry().iter().find(|e| e.pid == self.pid && e.done.is_none()) {
            entry.send(libc::SIGKILL);
        }
        let _ = self.wait_until(Some(Instant::now() + Duration::from_secs(5)));
    }

    /// 停掉它：还没退出的话发 TERM，等 2 秒，还不退出就 KILL，收走（新会话起的连它那个进程组一起发）。
    /// 返回它是怎么结束的；已经退出了的什么都不做
    pub fn stop(&mut self) -> io::Result<Exit> {
        drop(self.stdin.take());
        if self.try_wait()?.is_none() {
            if let Some(entry) = registry().iter().find(|e| e.pid == self.pid && e.done.is_none()) {
                entry.send(libc::SIGTERM);
            }
            if self.wait_timeout(Duration::from_secs(2))?.is_none() {
                self.kill_and_reap();
            }
        }
        self.exit.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, format!("{} 发了 KILL 还收不走", self.what)))
    }

    /// 用掉的 CPU 时间；还没等到它退出时是 None
    pub fn rusage(&self) -> Option<Usage> {
        self.usage
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.stop();
        // 发了 KILL 还收不走的（不该有）：登记表里留着，kill_all 再试
    }
}

/// 等到这个子进程可以收了（不收走）。不成（系统不支持、已经被收走）是 false
fn block_until_exit(pid: libc::pid_t) -> bool {
    loop {
        // SAFETY: info 是有效的，waitid 往里写；WNOWAIT：只等，不收
        let rc = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT)
        };
        if rc == 0 {
            return true;
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

pub(crate) fn set_nonblocking(fd: RawFd) {
    // SAFETY: 只改这个描述符的标志
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
}

/// 喂标准输入、收标准输出和标准错误，三路一起（哪一路堵着都不耽误另外两路），到两路输出都收完为止。
/// 返回（标准输出，标准错误，是不是在时限之内）
fn pump(child: &mut Child, input: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, bool)> {
    let mut stdin = child.stdin.take();
    let mut outs = [(child.stdout.take(), Vec::new()), (child.stderr.take(), Vec::new())];
    for fd in stdin.iter().chain(outs.iter().filter_map(|(f, _)| f.as_ref())) {
        set_nonblocking(fd.as_raw_fd());
    }
    let mut written = 0;
    let mut buf = vec![0u8; 65536];
    loop {
        if written == input.len() {
            stdin = None; // 喂完了：关上，子进程读到头
        }
        let mut fds: Vec<libc::pollfd> = Vec::new();
        if let Some(f) = &stdin {
            fds.push(libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLOUT, revents: 0 });
        }
        for (f, _) in &outs {
            if let Some(f) = f {
                fds.push(libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            }
        }
        if outs.iter().all(|(f, _)| f.is_none()) && stdin.is_none() {
            break;
        }
        let wait_ms = match child.deadline {
            None => -1,
            Some(end) => {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    let [(_, out), (_, err)] = outs;
                    return Ok((out, err, false));
                }
                left.as_millis().clamp(1, 60_000) as libc::c_int
            }
        };
        // SAFETY: fds 是有效的 pollfd 数组
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait_ms) };
        if ready < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(e);
        }
        if let Some(f) = &mut stdin {
            match f.write(&input[written..]) {
                Ok(n) => written += n,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                // 子进程不读了（关了标准输入、退出了）：剩下的不喂了
                Err(_) => written = input.len(),
            }
        }
        for (file, data) in &mut outs {
            let Some(f) = file else { continue };
            match f.read(&mut buf) {
                Ok(0) => *file = None,
                Ok(n) => data.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
                Err(e) => return Err(e),
            }
        }
    }
    let [(_, out), (_, err)] = outs;
    Ok((out, err, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    #[test]
    fn env_starts_empty_and_follows_the_order_of_edits() {
        let mut cmd = Cmd::new("x");
        assert_eq!(cmd.final_env(), os(&[]));
        cmd.env("A", "1").env("B", "2").env("A", "3").env_remove("B").env("C", "4").env_remove("NOPE");
        assert_eq!(cmd.final_env(), os(&[("A", "3"), ("C", "4")]));
        cmd.envs([("D", "5"), ("C", "6")]);
        assert_eq!(cmd.final_env(), os(&[("A", "3"), ("C", "6"), ("D", "5")]));
        // pass_env：这个进程里有的才带（PATH 总是有的）
        let mut cmd = Cmd::new("x");
        cmd.pass_env(&["PATH", "CCA_KIT_NO_SUCH_VARIABLE"]);
        assert_eq!(cmd.final_env(), vec![(OsString::from("PATH"), std::env::var_os("PATH").unwrap())]);
        // env_inherit：整个带上，再照改的来
        let mut cmd = Cmd::new("x");
        cmd.env_inherit().env_remove("PATH").env("CCA_KIT_EXTRA", "1");
        let env = cmd.final_env();
        assert!(env.len() >= std::env::vars_os().count() - 1 && !env.iter().any(|(k, _)| k == "PATH"));
        assert_eq!(env.last().unwrap(), &(OsString::from("CCA_KIT_EXTRA"), OsString::from("1")));
    }

    #[test]
    fn programs_are_found_on_the_given_path() {
        let dir = crate::fsx::TempDir::new("proc-unit-");
        crate::fsx::write_script(dir.join("a/tool"), "#!/bin/sh\n");
        crate::fsx::write_script(dir.join("b/tool"), "#!/bin/sh\n");
        crate::fsx::write(dir.join("c/tool"), "不能执行");
        crate::fsx::write(dir.join("c/data"), "不能执行");
        std::fs::create_dir_all(dir.join("d/tool")).unwrap();
        let path =
            |dirs: &[&str]| os(&[("PATH", &dirs.iter().map(|d| dir.join(d).display().to_string()).collect::<Vec<_>>().join(":"))]);
        // 头一个“是文件、能执行”的：目录、不能执行的文件跳过去
        assert_eq!(resolve(OsStr::new("tool"), &path(&["nope", "d", "c", "b", "a"]), None).unwrap(), dir.join("b/tool"));
        assert_eq!(resolve(OsStr::new("tool"), &path(&["a", "b"]), None).unwrap(), dir.join("a/tool"));
        // 没有：NotFound；有但不能执行：PermissionDenied
        assert_eq!(resolve(OsStr::new("tool"), &path(&["nope"]), None).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(resolve(OsStr::new("data"), &path(&["a", "c"]), None).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        // 没有 PATH 时照 /usr/bin:/bin 找（/bin 并进了 /usr/bin 的 Linux 上，头一个找到的是 /usr/bin/sh）
        let sh = resolve(OsStr::new("sh"), &os(&[]), None).unwrap();
        assert!(sh == Path::new("/usr/bin/sh") || sh == Path::new("/bin/sh"), "{sh:?}");
        // PATH 里空的一项、相对的一项，带 / 的相对路径：都从子进程的当前目录算
        assert_eq!(
            resolve(OsStr::new("tool"), &os(&[("PATH", "/nonexistent::")]), Some(&dir.join("a"))).unwrap(),
            dir.join("a/./tool")
        );
        assert_eq!(resolve(OsStr::new("tool"), &os(&[("PATH", "b")]), Some(dir.path())).unwrap(), dir.join("b/tool"));
        assert_eq!(resolve(OsStr::new("./b/tool"), &os(&[]), Some(dir.path())).unwrap(), dir.join("./b/tool"));
        assert_eq!(resolve(OsStr::new("b/nope"), &os(&[]), Some(dir.path())).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(resolve(OsStr::new("c/tool"), &os(&[]), Some(dir.path())).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(resolve(OsStr::new(""), &os(&[]), None).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(which("sh").is_some_and(|p| p.is_absolute() && p.ends_with("sh")));
        assert_eq!(which("cca-kit-no-such-command"), None);
    }

    #[test]
    fn exit_as_python_and_shell_see_it() {
        let exited = Exit { code: Some(3), signal: None };
        let killed = Exit { code: None, signal: Some(15) };
        assert_eq!((exited.py(), exited.shell(), exited.success()), (3, 3, false));
        assert_eq!((killed.py(), killed.shell(), killed.success()), (-15, 143, false));
        assert!(Exit { code: Some(0), signal: None }.success());
        assert_eq!((exited.to_string(), killed.to_string()), ("退出码 3".to_string(), "被信号 15 杀掉".to_string()));
    }
}
