//! 终端夹具：一个伪终端和跑在上面的那个程序，远程会话的几份测试共用（原来是 tests/remote_host.py 的 Term、AppTerm），
//! 还有把假客户端（cca-test fake-client，原来那份内嵌的脚本 CLIENT）写成文件的 [`write_client`]。
//!
//! 被测的程序（会话宿主 cca-core remote-host、接入端 remote-attach）要在真的终端里才是它本来的样子：
//! - [`Term::start`]：像在终端里运行那样起（自己一个会话、这个终端是它的控制终端；原来的 pty.fork()）
//! - [`Term::start_app`]：照界面 App 的办法起（macos/PROTOCOL.md 9.3：posix_spawn、自己一个会话、伪终端从的那头接成
//!   0/1/2）。这样起来的进程一开始没有控制终端（macOS 上打开终端不会让它成为控制终端）
//!
//! 之后 [`Term::send`] 当键盘、[`Term::expect`] 看屏幕、[`Term::resize`] 改窗口大小、[`Term::hangup`] 关掉终端、
//! [`Term::wait`] 等它退出（退出码、用掉的 CPU 时间）。
//!
//! 输出只在 expect、wait、pump 的时候读，没有另外的线程一直在读：有的用例测的正是“终端先不收”时被测的程序怎么办。
//! 离开作用域时先关掉终端、再停掉上面的程序（TERM，2 秒后 KILL）。先后不能反：macOS 上它写在终端上、还没人读走的输出
//! 会让它退不完（见 kit::pty 开头）。

use crate::proc::{Child, Cmd, Session};
use crate::pty::Pty;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// [`Term::expect`]、[`Term::wait`] 最多等多久（原来 Term.expect、Term.wait 的默认值）
pub const WAIT: Duration = Duration::from_secs(10);

/// 一个伪终端和跑在上面的进程
#[derive(Debug)]
pub struct Term {
    pty: Pty,
    child: Child,
}

impl Term {
    /// 开一个 rows 行 cols 列的伪终端，像在终端里运行那样把 cmd 起在上面：自己一个会话、这个终端是它的控制终端，
    /// 标准输入、输出、错误都是它（cmd 里给的环境、当前目录照用，标准输入输出设了也不算数）。起不来就 panic
    pub fn start(cmd: &mut Cmd, rows: u16, cols: u16) -> Term {
        let mut pty = Pty::open(rows, cols).unwrap_or_else(|e| panic!("开不了伪终端：{e}"));
        let child = pty.spawn(cmd).unwrap_or_else(|e| panic!("{e}"));
        Term { pty, child }
    }

    /// 照界面 App 的办法起：先换成自己一个会话，再把伪终端从的那头打开、接成 0/1/2。Linux 上它就成了控制终端，
    /// macOS 上不会——被测的程序得自己把它认下来（接入端的 adopt_tty），不然窗口大小变了它收不到信号。
    ///
    /// 这边留着从的那头的一份，到 [`Term::hangup`]（或者离开作用域）才关：那个程序是自己去打开终端的，这边要是起完就关，
    /// 它还没打开的那一小会儿从的那头一个开着的都没有，主的那头会读到头；没认控制终端的程序退出时，没读走的输出也靠它留着
    pub fn start_app(cmd: &mut Cmd, rows: u16, cols: u16) -> Term {
        let pty = Pty::open(rows, cols).unwrap_or_else(|e| panic!("开不了伪终端：{e}"));
        let child = cmd.session(Session::NewOpenTty(pty.slave_path().to_path_buf())).spawn().unwrap_or_else(|e| panic!("{e}"));
        Term { pty, child }
    }

    /// 上面那个程序的进程号
    pub fn pid(&self) -> i32 {
        self.child.pid()
    }

    /// 到现在为止读到的全部输出（`out()[mark..]`：某个位置以后的）
    pub fn out(&self) -> &[u8] {
        &self.pty.out
    }

    /// 现在读到哪了（`len(out)`）：记下来，之后只看这以后的输出（[`Term::expect_since`]、`out()[mark..]`）
    pub fn mark(&self) -> usize {
        self.pty.out.len()
    }

    /// 全部输出当文字（`out.decode()`）；不是 UTF-8 就 panic
    pub fn text(&self) -> String {
        match std::str::from_utf8(&self.pty.out) {
            Ok(text) => text.to_string(),
            Err(e) => panic!("终端的输出不是 UTF-8（{e}）：{:?}", String::from_utf8_lossy(&self.pty.out)),
        }
    }

    /// 改窗口的大小（上面在前台的程序收到 SIGWINCH）
    pub fn resize(&self, rows: u16, cols: u16) {
        self.pty.resize(rows, cols).unwrap_or_else(|e| panic!("改不了窗口大小：{e}"));
    }

    /// 往终端里“打字”
    pub fn send(&mut self, data: impl AsRef<[u8]>) {
        self.pty.send(data).unwrap_or_else(|e| panic!("往终端里写不进去：{e}"));
    }

    /// 等最多 wait 那么久，把现在读得到的输出收进来。false：对面关了，或者这个终端已经关掉了
    pub fn pump(&mut self, wait: Duration) -> bool {
        self.pty.pump(wait)
    }

    /// 等输出里出现 pattern（按字节匹配的正则，见 [`Pty::wait_output`]），最多等 [`WAIT`]；给出匹配末尾的位置。
    /// 没等到就 panic，带上输出的末尾
    pub fn expect(&mut self, pattern: &str) -> usize {
        self.expect_in(pattern, WAIT, 0)
    }

    /// 同 [`Term::expect`]，只看第 since 个字节以后的输出
    pub fn expect_since(&mut self, pattern: &str, since: usize) -> usize {
        self.expect_in(pattern, WAIT, since)
    }

    /// 同 [`Term::expect_since`]，最多等 limit
    pub fn expect_in(&mut self, pattern: &str, limit: Duration, since: usize) -> usize {
        self.pty.expect_since(pattern, limit, since)
    }

    /// 等上面的程序退出，最多等 [`WAIT`]，一边等一边收输出（退出以后剩下的也收完）。给出 Python 的 returncode：
    /// 退出码，被信号杀掉的是负的信号号。到时限还没退出就 panic（不杀它），带上输出的末尾
    pub fn wait(&mut self) -> i32 {
        self.wait_in(WAIT)
    }

    /// 同 [`Term::wait`]，最多等 limit
    pub fn wait_in(&mut self, limit: Duration) -> i32 {
        match self.pty.wait(&mut self.child, limit) {
            Ok(exit) => exit.py(),
            Err(e) => panic!("{e}"),
        }
    }

    /// 上面的程序用掉的 CPU 时间（用户态加内核态）。等到它退出（[`Term::wait`]）才有，之前问就 panic
    pub fn cpu(&self) -> Duration {
        self.child.rusage().unwrap_or_else(|| panic!("进程 {} 还没等到退出，不知道它用了多少 CPU 时间", self.pid())).cpu()
    }

    /// 上面的程序还在跑（没退出）
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 给上面的程序发一个信号（`libc::SIGTERM` 这样的号）；已经退出、收走了的不发
    pub fn signal(&self, sig: i32) {
        self.child.signal(sig).unwrap_or_else(|e| panic!("给进程 {} 发不了信号 {sig}：{e}", self.pid()));
    }

    /// 关掉这个终端（上面的会话收到 SIGHUP）。之后 send、resize 都不成了，[`Term::wait`] 照样能等
    pub fn hangup(&mut self) {
        self.pty.hangup();
        self.pty.close_slave();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.hangup();
        let _ = self.child.stop();
    }
}

/// hay 里头一处 needle 在哪（Python 的 `needle in out`、`out.index(needle)`：终端的输出是字节，不能当文字找）
pub fn find(hay: &[u8], needle: impl AsRef<[u8]>) -> Option<usize> {
    let needle = needle.as_ref();
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|window| window == needle)
}

/// 放进单引号里交给 sh：里面的单引号写成 '\''
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// 那个两行的脚本：把自己换成 `tool fake-client 脚本自己的路径 参数…`
fn client_script(tool: &Path) -> String {
    let tool = tool.to_str().unwrap_or_else(|| panic!("cca-test 的路径不是 UTF-8：{tool:?}"));
    format!("#!/bin/sh\nexec {} fake-client \"$0\" \"$@\"\n", sh_quote(tool))
}

/// 把假客户端写成 dir/client 这个脚本，给出它的路径：`cca-core remote-host 工具 账号的键 名字 -- 这个路径 参数…` 拿它当客户端起。
///
/// 假客户端是 `cca-test fake-client`（它说什么、认哪些命令见 tests/tool/fake_client.rs）；脚本只有两行，exec 过去，所以进程号、
/// 会话、控制终端都还是宿主起的那个。脚本把自己的路径当第一个参数带过去：那个进程的命令行里于是有 dir——测试的临时目录——
/// 没收场的找得到
pub fn write_client(dir: &Path) -> PathBuf {
    let path = dir.join("client");
    let tool = crate::proc::tool().unwrap_or_else(|e| panic!("{e}"));
    crate::fsx::write_script(&path, &client_script(&tool));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_is_bytewise() {
        let out = b"ab\x1b[6n\xff\x00cd\x1b[6n";
        assert_eq!(find(out, b"\x1b[6n"), Some(2));
        assert_eq!(find(out, [0xffu8, 0x00]), Some(6));
        assert_eq!(find(&out[3..], "\x1b[6n"), Some(7));
        assert_eq!(find(out, "cd\x1b[6n"), Some(8));
        assert_eq!(find(out, b"\x1b[6nx"), None);
        assert_eq!(find(b"ab", b"abc"), None);
        assert_eq!((find(out, b""), find(b"", b""), find(b"", b"a")), (Some(0), Some(0), None));
    }

    #[test]
    fn client_script_execs_the_tool_with_its_own_path_first() {
        // 路径里有空格、单引号、美元号的“cca-test”：脚本照样找得到它，参数原样交过去
        let dir = crate::fsx::TempDir::new("term-unit-");
        let tool = dir.join("it's a $tool/cca test");
        crate::fsx::write_script(&tool, "#!/bin/sh\nfor a in \"$@\"; do echo \"[$a]\"; done\n");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        let script = dir.join("client");
        crate::fsx::write_script(&script, &client_script(&tool));
        assert_eq!(crate::fsx::read_text(&script).lines().count(), 2);
        let out = Cmd::new(&script).args(["--flag", "two words", "", "它's"]).output().unwrap().check();
        assert_eq!(out.out(), format!("[fake-client]\n[{}]\n[--flag]\n[two words]\n[]\n[它's]\n", script.display()));
    }
}
