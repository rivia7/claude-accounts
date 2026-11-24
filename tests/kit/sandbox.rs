//! 临时的 HOME（里面放 ~/.cca）和在里面跑辅助程序（cca-core）的办法：把辅助程序当黑盒跑的那几份测试共用
//! （device_identity、shared_state、router_merge、local_udp、usage_view），加上它们都要的几样小东西。
//!
//! HOME 是经一个链接去到的：`<临时目录>/var/home`，`<临时目录>/var` 是指向 `private` 的链接——和 macOS 上
//! `/var → /private/var` 一个样子。原来的 Python 测试在回归里跑的时候临时目录在 `/var/folders` 下（`TMPDIR`），HOME 就是这样
//! 一个没解过链接的写法：辅助程序照 HOME 的写法说路径、记“原写法和真实路径”两份，这些断言在解过链接的 HOME 下分不出来。
//! 所以这里不用 [`Ctx::tmp`] 给的（解过链接的）路径直接当 HOME。
//!
//! 被测的程序看到的环境只有 [`Sandbox::set_env`] 设过的那几个（一开始只有 HOME），不带开发者 shell 里的任何东西。

use crate::harness::{self, Ctx};
use crate::proc::Cmd;
use crate::{crypto, fsx};
use std::collections::BTreeMap;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};

/// 两个官方账号（device_identity、shared_state 的夹具）
pub const ACCOUNTS: [&str; 2] = ["alice@example.com", "bob@example.com"];

/// 只会报错的替身的脚本体（前面接 `#!/bin/sh`）：调到了就在 ~/unexpected-commands.log 里记一行（"$0 $*"）、退出码 127
pub const GUARD: &str = "printf '%s\\n' \"$0 $*\" >> \"$HOME/unexpected-commands.log\"\nexit 127\n";

/// 一个临时的 HOME，和在里面跑程序时给的环境
pub struct Sandbox {
    /// HOME（经链接的写法，见模块开头的说明）
    pub home: PathBuf,
    /// HOME 下的 .cca（没建：各份测试自己建）
    pub cca: PathBuf,
    env: Vec<(String, String)>,
}

impl Sandbox {
    /// 在这个用例的临时目录（前缀 prefix）里建一个 HOME；环境里先只有 HOME
    pub fn new(t: &mut Ctx, prefix: &str) -> Sandbox {
        let base = t.tmp(prefix);
        let real = base.join("private").join("home");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&real)
            .unwrap_or_else(|e| panic!("建不了目录 {}：{e}", real.display()));
        let link = base.join("var");
        std::os::unix::fs::symlink("private", &link).unwrap_or_else(|e| panic!("建不了链接 {}：{e}", link.display()));
        let home = link.join("home");
        let mut sandbox = Sandbox { cca: home.join(".cca"), home: home.clone(), env: Vec::new() };
        sandbox.set_env("HOME", &home);
        sandbox
    }

    /// 被测的程序的环境里设一个（已经有的换掉值）
    pub fn set_env(&mut self, key: &str, value: impl AsRef<OsStr>) -> &mut Sandbox {
        let value = value.as_ref().to_string_lossy().into_owned();
        match self.env.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.env.push((key.to_string(), value)),
        }
        self
    }

    /// 环境里的一个（没设是 None）
    pub fn env(&self, key: &str) -> Option<&str> {
        self.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// ~/bin（假命令放在这里；PATH 要自己设成 `<~/bin>:/usr/bin:/bin` 这样）
    pub fn bin(&self) -> PathBuf {
        self.home.join("bin")
    }

    /// ~/bin 下放一个假命令：`#!/bin/sh` 接上 body，755
    pub fn fake(&self, name: &str, body: &str) {
        fsx::write_script(self.bin().join(name), &format!("#!/bin/sh\n{body}"));
    }

    /// names 里的命令换成只会报错的替身（[`GUARD`]；fakes 里有这个名字的，替身前面先跑那一段：假命令答得了的就答，
    /// 答不了的照样算调错了），fakes 里另外的名字也一样放上。登记一件收尾：用例完了 ~/unexpected-commands.log 得是空的
    pub fn guard(&self, t: &mut Ctx, names: &[&str], fakes: &[(&str, &str)]) {
        let extra = fakes.iter().map(|(name, _)| *name).filter(|name| !names.contains(name));
        for name in names.iter().copied().chain(extra) {
            let fake = fakes.iter().find(|(n, _)| *n == name).map_or("", |(_, body)| *body);
            self.fake(name, &format!("{fake}{GUARD}"));
        }
        let stray = self.home.join("unexpected-commands.log");
        t.defer(move || {
            let said = if stray.exists() { fsx::read_text(&stray) } else { String::new() };
            assert_eq!(said, "", "调到了没有准备假命令的外部命令");
        });
    }

    /// 在这个环境里跑辅助程序（`kit::harness::helper()`）：环境只有设过的那几个；要多设的、喂的输入、时限接着点下去，
    /// 再交给 [`ran`]
    pub fn helper<S: AsRef<OsStr>>(&self, args: impl IntoIterator<Item = S>) -> Cmd {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(args);
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}

/// 跑完的一条命令：退出码（Python 的 returncode：被信号杀掉的是负的信号号）、标准输出、标准错误
#[derive(Debug)]
pub struct Ran {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// 跑一条命令、收齐输出（`subprocess.run(capture_output=True, text=True)`）：起不来、过了时限就 panic；输出不是 UTF-8 也 panic
/// （Python 那边解码就出错了）
pub fn ran(cmd: &mut Cmd) -> Ran {
    let out = cmd.output().unwrap_or_else(|e| panic!("{e}"));
    let text = |bytes: Vec<u8>, what: &str| {
        String::from_utf8(bytes).unwrap_or_else(|e| panic!("{what}不是 UTF-8：{:?}", String::from_utf8_lossy(e.as_bytes())))
    };
    Ran { code: out.status.py(), out: text(out.stdout, "标准输出"), err: text(out.stderr, "标准错误") }
}

/// 账号目录的名字（辅助程序的 acct_id）：邮箱转小写，字母数字以外的连成一个 -，去掉两头的 -，最多 20 个字符（再去掉末尾的 -；
/// 什么都不剩时是 acct），后面接 - 和邮箱的 SHA-1 的前 6 位
pub fn ident(email: &str) -> String {
    let mut s = String::new();
    for c in email.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(20).collect();
    let s = s.trim_matches('-');
    let s = if s.is_empty() { "acct" } else { s };
    format!("{s}-{}", &crypto::sha1_hex(email.as_bytes())[..6])
}

/// 账号表的键里的邮箱：`别名(邮箱)` 的括号里那一段，没有别名就是它自己（`key.split('(')[-1].rstrip(')')`）
pub fn email_of(key: &str) -> &str {
    key.rsplit('(').next().unwrap_or(key).trim_end_matches(')')
}

/// 两个路径是不是同一个文件（`os.path.samefile`：跟着链接走，比设备号和 inode）；有一个不在就 panic
pub fn samefile(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let meta = |p: &Path| std::fs::metadata(p).unwrap_or_else(|e| panic!("看不了 {}：{e}", p.display()));
    let (a, b) = (meta(a), meta(b));
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// 把访问和修改时间都改成 secs（1970 年以来的秒；`os.utime(path, (secs, secs))`）。follow 是 false 时改的是链接自己
/// （`follow_symlinks=False`）
pub fn utime(path: &Path, secs: i64, follow: bool) {
    let c = CString::new(path.as_os_str().as_bytes()).unwrap_or_else(|_| panic!("路径里有 NUL：{}", path.display()));
    let at = libc::timespec { tv_sec: secs as _, tv_nsec: 0 };
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    // SAFETY: c 是以 NUL 结尾的路径，times 是两个有效的 timespec；utimensat 只读它们
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [at, at].as_ptr(), flags) };
    if rc != 0 {
        panic!("改不了 {} 的时间：{}", path.display(), std::io::Error::last_os_error());
    }
}

/// path 相对 start 的写法（`os.path.relpath`）：两个都是绝对路径，只按字面算，不解链接
pub fn relpath(path: &Path, start: &Path) -> PathBuf {
    let parts = |p: &Path| -> Vec<std::ffi::OsString> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_os_string()),
                _ => None,
            })
            .collect()
    };
    let (to, from) = (parts(path), parts(start));
    let common = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut rel = PathBuf::new();
    for _ in common..from.len() {
        rel.push("..");
    }
    for part in &to[common..] {
        rel.push(part);
    }
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

/// 在 dir 下建一个目录，名字是 prefix 接 8 个随机字符（700），给出 dir 原样写法下的路径（`tempfile.mkdtemp(prefix=…, dir=…)`：
/// 不解链接；不会自己删——跟着 dir 一起删）
pub fn mkdtemp(dir: &Path, prefix: &str) -> PathBuf {
    const CHARS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    loop {
        let name: String = crypto::random(8).iter().map(|b| CHARS[*b as usize % 36] as char).collect();
        let path = dir.join(format!("{prefix}{name}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("建不了目录 {}：{e}", path.display()),
        }
    }
}

/// 按 shell 的规矩拆成一个个词（`shlex.split`：空白隔开，单引号里原样，双引号里只有 \\ 和 \" 是转义，引号外的反斜杠转义下一个字符）。
/// 引号没配对、反斜杠在最后就 panic（shlex 也是报错）
pub fn shell_split(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => word.push(x),
                        None => panic!("引号没配对：{text:?}"),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x) if x == '"' || x == '\\' => word.push(x),
                            Some(x) => {
                                word.push('\\');
                                word.push(x);
                            }
                            None => panic!("反斜杠后面没有字符：{text:?}"),
                        },
                        Some(x) => word.push(x),
                        None => panic!("引号没配对：{text:?}"),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(x) => word.push(x),
                    None => panic!("反斜杠后面没有字符：{text:?}"),
                }
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// 辅助程序打出来的一个个 `名字=值`（值照 shell 的写法加了引号）：[`shell_split`] 以后按第一个 = 分开
/// （`dict(kv.split('=', 1) for kv in shlex.split(text))`）。有一个词里没有 = 就 panic
pub fn shell_vars(text: &str) -> BTreeMap<String, String> {
    shell_split(text)
        .into_iter()
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => panic!("不是 名字=值：{kv:?}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ident_like_the_helper() {
        // 对照值是 Python 照原来测试里那一句算的
        assert_eq!(ident("alice@example.com"), "alice-example-com-fc2398");
        assert_eq!(ident("Bob.Smith+x@Example.COM"), "bob-smith-x-example-869fb5");
        assert_eq!(ident("@@@"), "acct-ffab3b");
        assert_eq!(email_of("工作(alice@example.com)"), "alice@example.com");
        assert_eq!(email_of("bob@example.com"), "bob@example.com");
    }

    #[test]
    fn relpath_like_python() {
        let rel = |a: &str, b: &str| relpath(Path::new(a), Path::new(b)).display().to_string();
        assert_eq!(rel("/h/.claude/x.json", "/h/.cca/accounts/a/claude"), "../../../../.claude/x.json");
        assert_eq!(rel("/h/a/b", "/h/a"), "b");
        assert_eq!(rel("/h/a", "/h/a"), ".");
        assert_eq!(rel("/h", "/h/a/b"), "../..");
    }

    #[test]
    fn shell_words_like_shlex() {
        assert_eq!(shell_split("A=1 B='' C='x y'\nD=\"q\\\"\\$\" E=a\\ b"), ["A=1", "B=", "C=x y", "D=q\"\\$", "E=a b"]);
        assert_eq!(shell_split("  "), Vec::<String>::new());
        assert_eq!(shell_split("X='it'\\''s'"), ["X=it's"]);
        let vars = shell_vars("UDP_NODE=''\nUDP_STATE=off\nUDP_ROOT=/a/b=c\n");
        assert_eq!((vars["UDP_NODE"].as_str(), vars["UDP_STATE"].as_str(), vars["UDP_ROOT"].as_str()), ("", "off", "/a/b=c"));
    }

    #[test]
    fn mkdtemp_and_utime() {
        let dir = fsx::TempDir::new("sandbox-unit-");
        let a = mkdtemp(&dir, "config-");
        let b = mkdtemp(&dir, "config-");
        assert_ne!(a, b);
        assert!(a.is_dir() && a.file_name().unwrap().to_str().unwrap().starts_with("config-"));
        assert_eq!(fsx::mode(&a), 0o700);
        fsx::write(a.join("f"), "x");
        std::os::unix::fs::symlink(a.join("f"), a.join("l")).unwrap();
        utime(&a.join("l"), 100, false);
        utime(&a.join("f"), 200, true);
        let secs = |m: std::fs::Metadata| m.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs(std::fs::symlink_metadata(a.join("l")).unwrap()), 100);
        assert_eq!(secs(std::fs::metadata(a.join("f")).unwrap()), 200);
    }
}
