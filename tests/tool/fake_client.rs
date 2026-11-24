//! fake-client：远程会话的测试里，会话宿主（cca-core remote-host）起的那个“客户端”，顶替 Claude Code。
//! 原来是 tests/remote_host.py 里内嵌的 Python 脚本 CLIENT（remote_relay、team_relay、web_relay 也用它）：说出来的字节、
//! 退出码都和原来一样，那几份测试照着匹配。
//!
//! 用法：cca-test fake-client 脚本的路径 [参数…]
//!
//! 测试不直接起它：kit::term::write_client 在临时目录里写一个两行的 sh 脚本（exec cca-test fake-client "$0" "$@"），把那个
//! 脚本当客户端交给宿主。所以第一个参数是那个脚本的路径，相当于 Python 的 sys.argv[0]，这里不看——带着它，进程的命令行里
//! 就有测试的临时目录，没收场的找得到（pgrep -f、运行器的 LEFTOVER）。
//!
//! 一启动报一行 `<ready 行 列 leader=… ctty=… tty=… args=[…]>`：窗口大小、是不是会话的头、打不打得开 /dev/tty（有没有控制
//! 终端）、0/1/2 是不是都是终端、收到的参数（照 Python 的写法：True / False、列表的 repr）。之后一行一条命令：
//!
//!   exit N     退出，退出码 N
//!   say 文字    回 <文字>
//!   query      问终端两件事（光标在哪 CSI 6n、背景色 OSC 11），再回 <asked>
//!   evil       一串不怀好意的字节，最后 <done>：去掉中间那条查询以后，前后两截不能在别的终端上拼成 ESC ] 52…（写剪贴板）
//!   clip       写剪贴板（OSC 52），再回 <clipped>
//!   alt        进备用屏幕、藏光标、开括号粘贴，再回 <alt>
//!   kill       拿 SIGKILL 杀掉自己
//!   flood      两万行，最后 <flooded>（一次 write）
//!   dump       四万行（两兆多），最后 <dumped>，马上退出，退出码 6
//!   closetty   回 <closing>，把 0/1/2 都关掉，过 1.5 秒退出，退出码 4
//!
//! 别的行不理。标准输入读到头：回 <eof>，退出码 9。窗口大小变了（SIGWINCH）：回 <size 行 列>。收到 SIGTERM：回 <term>，
//! 退出码 5。每句话一次 write、以 \r\n 结尾（它的终端是行模式，到了外面是 \r\r\n）。
//!
//! 照原来的地方：信号不在处理函数里办，记下来、等手头这一步做完再办（Python 的处理函数也是在两条语句之间跑的），一句话
//! 不会被 <size …> 从中间插开；Python 里没人接的异常（0 号不是终端、写不出去、exit 后面不是数……）在 stderr 上说一句、
//! 退出码 1。不一样的只有一处：一句话写到一半被信号打断时接着写完（原来剩下的就丢了）。

use crate::fake::os_text;
use kit::py::{self, repr_str};
use std::io::Write;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

pub const CMDS: &[(&str, crate::Run)] = &[("fake-client", run)];

/// 信号处理函数往这个描述符里写一个字节（信号的号）：管道的另一头由主循环来读
static WAKE: AtomicI32 = AtomicI32::new(-1);

/// 这条线程的 errno 在哪
fn errno_ptr() -> *mut libc::c_int {
    // SAFETY: 两个函数都只是给出这条线程的 errno 的地址
    unsafe {
        #[cfg(target_os = "macos")]
        return libc::__error();
        #[cfg(not(target_os = "macos"))]
        return libc::__errno_location();
    }
}

extern "C" fn on_signal(sig: libc::c_int) {
    let byte = sig as u8;
    // SAFETY: write 可以在信号处理函数里调，写的是栈上的一个字节（管道满了写不进去就算了：里面已经有没办的）；
    // errno 先存起来、完了放回去，被打断的那一步还要看它
    unsafe {
        let errno = errno_ptr();
        let saved = *errno;
        libc::write(WAKE.load(Ordering::Relaxed), std::ptr::from_ref(&byte).cast(), 1);
        *errno = saved;
    }
}

/// Python 里没人接的异常：stderr 上说一句（原来是一段 traceback，只留最后那一行的意思），退出码 1
fn die(what: &str) -> ! {
    let _ = writeln!(std::io::stderr(), "{what}");
    std::process::exit(1)
}

/// 刚才那个系统调用没成：OSError
fn os_die() -> ! {
    die(&os_text(&std::io::Error::last_os_error(), None))
}

/// os.write(1, …)：不经缓冲，一句话一次 write（被信号打断了接着写完）
fn say(text: impl AsRef<[u8]>) {
    let mut rest = text.as_ref();
    while !rest.is_empty() {
        // SAFETY: 写的就是 rest 的那些字节
        let n = unsafe { libc::write(1, rest.as_ptr().cast(), rest.len()) };
        if n >= 0 {
            rest = &rest[n as usize..];
        } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            os_die();
        }
    }
}

/// 0 号那个终端的窗口大小：（行，列）
fn size() -> (u16, u16) {
    // SAFETY: ws 清零后是有效的，TIOCGWINSZ 往里写
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(0, libc::TIOCGWINSZ as _, &mut ws) != 0 {
            os_die();
        }
        (ws.ws_row, ws.ws_col)
    }
}

/// 把到了的信号办掉。号小的先办，同一种攒了几次只办一次（Python 也是这样）
fn signals(pipe: libc::c_int) {
    let (mut term, mut winch) = (false, false);
    let mut buf = [0u8; 64];
    loop {
        // SAFETY: 往 buf 里读，不超过它的长度；管道设了不阻塞
        let n = unsafe { libc::read(pipe, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break; // 读空了（被打断的话，剩下的下一圈再读）
        }
        for sig in &buf[..n as usize] {
            term |= libc::c_int::from(*sig) == libc::SIGTERM;
            winch |= libc::c_int::from(*sig) == libc::SIGWINCH;
        }
    }
    if term {
        say("<term>\r\n");
        // SAFETY: 马上退出，不经收尾（原来是 os._exit）
        unsafe { libc::_exit(5) };
    }
    if winch {
        let (rows, cols) = size();
        say(format!("<size {rows} {cols}>\r\n"));
    }
}

/// 等 fds 里哪个读得了，最多等 limit（None：一直等）；给出读得了的那几个。被信号打断了就是一个都没有
fn readable(fds: &[libc::c_int], limit: Option<Duration>) -> Vec<libc::c_int> {
    // select，不用 poll：macOS 的 poll 对终端这类设备不保证管用
    // SAFETY: set 清零再 FD_ZERO 就是有效的；fds 里都是这个进程开着的小号描述符；timeval 是有效的
    unsafe {
        let mut set: libc::fd_set = std::mem::zeroed();
        libc::FD_ZERO(&mut set);
        for fd in fds {
            libc::FD_SET(*fd, &mut set);
        }
        let mut tv = limit.map(|d| libc::timeval { tv_sec: d.as_secs() as _, tv_usec: d.subsec_micros() as _ });
        let tv_ptr = tv.as_mut().map_or(std::ptr::null_mut(), std::ptr::from_mut);
        let max = fds.iter().copied().max().unwrap_or(0);
        if libc::select(max + 1, &mut set, std::ptr::null_mut(), std::ptr::null_mut(), tv_ptr) <= 0 {
            return Vec::new();
        }
        fds.iter().copied().filter(|fd| libc::FD_ISSET(*fd, &set)).collect()
    }
}

/// True / False
fn py_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

/// sys.exit(int(文字)) 的退出码；文字不是整数是 None。认正负号、十进制数字（数字之间可以夹一个下划线）；
/// 大得 C 的 long 放不下的，Python 拿 -1 退出
fn exit_code(text: &str) -> Option<i32> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let well_formed = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__");
    if !well_formed {
        return None;
    }
    let mut n: i64 = 0;
    for b in digits.bytes().filter(|b| *b != b'_') {
        match n.checked_mul(10).and_then(|n| n.checked_add(i64::from(b - b'0'))) {
            Some(next) => n = next,
            None => return Some(-1),
        }
    }
    // C 里把 long 截成 int
    Some((if negative { -n } else { n }) as i32)
}

/// 一行命令。要退出的在里面直接退出
fn command(line: &[u8], pipe: libc::c_int) {
    // sys.stdin 是照 UTF-8 读的：不是 UTF-8 的字节留在字符串里（surrogateescape），哪条命令都对不上，原样说回去时才出错
    let text = String::from_utf8_lossy(line);
    let cmd = py::strip(&text);
    if cmd.starts_with("exit ") {
        // cmd.split()[1]
        let word = py::split(cmd).nth(1).unwrap_or_default();
        match exit_code(word) {
            Some(code) => std::process::exit(code),
            None => die(&format!("ValueError: invalid literal for int() with base 10: {}", repr_str(word))),
        }
    } else if let Some(words) = cmd.strip_prefix("say ") {
        if std::str::from_utf8(line).is_err() {
            die("UnicodeEncodeError: 'utf-8' codec can't encode character: surrogates not allowed");
        }
        say(format!("<{words}>\r\n"));
    } else if cmd == "query" {
        say("\x1b[6n\x1b]11;?\x07<asked>\r\n");
    } else if cmd == "evil" {
        // 去掉中间那条查询以后，前后两截不能在别的终端上拼成 ESC ] 52…（写剪贴板）
        say(format!("<evil>\x1b\x1b[c]52;c;ZXZpbA==\x07\x1b[{}c<done>\r\n", "0;".repeat(300)));
    } else if cmd == "clip" {
        say("\x1b]52;c;Y2xpcA==\x07<clipped>\r\n");
    } else if cmd == "alt" {
        say("\x1b[?1049h\x1b[?25l\x1b[?2004h<alt>\r\n");
    } else if cmd == "kill" {
        // SAFETY: 给自己发 SIGKILL
        unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
    } else if cmd == "flood" {
        let mut text: String = (0..20000).map(|i| format!("line {i:06} of the flood\r\n")).collect();
        text.push_str("<flooded>\r\n");
        say(text);
    } else if cmd == "dump" {
        let mut text: String =
            (0..40000).map(|i| format!("dump {i:07} ................................................\r\n")).collect();
        text.push_str("<dumped>\r\n");
        say(text);
        std::process::exit(6);
    } else if cmd == "closetty" {
        say("<closing>\r\n");
        // SAFETY: 关掉自己的 0、1、2 号；_exit 马上退出，不经收尾（原来是 os._exit）
        unsafe {
            for fd in 0..3 {
                libc::close(fd);
            }
            // time.sleep(1.5)：中间来了信号照样办（这时处理函数里的 write、ioctl 都不成了：没人接的异常，退出码 1）
            let end = Instant::now() + Duration::from_millis(1500);
            loop {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                if !readable(&[pipe], Some(left)).is_empty() {
                    signals(pipe);
                }
            }
            libc::_exit(4);
        }
    }
}

fn run(args: &[String]) -> i32 {
    // 信号经一根管道交给主循环：处理函数只往里写一个字节
    let mut ends = [0 as libc::c_int; 2];
    // SAFETY: pipe 往 ends 里写两个新的描述符；fcntl 只改它们的标志；sigaction 清零后填上处理函数就是有效的
    // （不带 SA_RESTART：等输入的那一步要被打断）
    unsafe {
        if libc::pipe(ends.as_mut_ptr()) != 0 {
            os_die();
        }
        for fd in ends {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        WAKE.store(ends[1], Ordering::SeqCst);
        for sig in [libc::SIGWINCH, libc::SIGTERM] {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
    let pipe = ends[0];
    // SAFETY: 都只是问一下；open 成了给的描述符马上关掉
    let (leader, ctty, tty) = unsafe {
        let leader = libc::getsid(0) == libc::getpid();
        let fd = libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR);
        if fd >= 0 {
            libc::close(fd);
        }
        (leader, fd >= 0, libc::isatty(0) == 1 && libc::isatty(1) == 1 && libc::isatty(2) == 1)
    };
    signals(pipe);
    let (rows, cols) = size();
    let argv: Vec<String> = args.iter().skip(1).map(|a| repr_str(a)).collect();
    say(format!(
        "<ready {rows} {cols} leader={} ctty={} tty={} args=[{}]>\r\n",
        py_bool(leader),
        py_bool(ctty),
        py_bool(tty),
        argv.join(", ")
    ));

    // sys.stdin.readline()：文本模式，\n、\r\n、单独的 \r 都算一行的结尾（一块的最后一个字节是 \r 时先留着，下一块
    // 到了才知道后面是不是 \n）。终端是行模式时到这里的只有 \n
    let mut line: Vec<u8> = Vec::new();
    let mut held_cr = false;
    let mut buf = [0u8; 8192];
    loop {
        let ready = readable(&[0, pipe], None);
        if ready.contains(&pipe) {
            signals(pipe);
        }
        if !ready.contains(&0) {
            continue;
        }
        // SAFETY: 往 buf 里读，不超过它的长度
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            os_die();
        }
        if n == 0 {
            // 读到头。手里还有没换行的半行：它也算一行，办完再读一次才知道是不是真的没有了
            // （终端上按 Ctrl-D 的“读到头”只管这一次，readline 也是这样）
            if line.is_empty() && !held_cr {
                say("<eof>\r\n");
                return 9;
            }
            held_cr = false;
            command(&std::mem::take(&mut line), pipe);
            signals(pipe);
            continue;
        }
        for byte in &buf[..n as usize] {
            if held_cr {
                held_cr = false;
                command(&std::mem::take(&mut line), pipe);
                signals(pipe);
                if *byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => {
                    command(&std::mem::take(&mut line), pipe);
                    signals(pipe);
                }
                b'\r' => held_cr = true,
                other => line.push(*other),
            }
        }
    }
}
