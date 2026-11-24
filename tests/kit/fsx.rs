//! 临时目录、读写文件、等到条件成立（wait_for：到时限还没等到就 panic，说清楚在等什么）
//!
//! 读写的那几个出了错直接 panic、说清楚是哪个文件：测试里文件读不了就是用例失败，不用每处都写一遍 unwrap。

use crate::json;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 环境里设了 KEEP（不是空的）：临时目录用完不删，留着看
pub fn keep() -> bool {
    std::env::var_os("KEEP").is_some_and(|v| !v.is_empty())
}

/// 临时目录建在哪：`CCA_SUITE_TMP`（运行器给每个模块指一个，模块跑完整个删掉），没设就是系统的临时目录。
/// 给出来的是解过链接的路径（macOS 的 /var 是 /private/var 的链接：辅助程序说出来的路径是解过的，两边要对得上）
pub fn tmp_base() -> PathBuf {
    let base = match std::env::var_os("CCA_SUITE_TMP") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::temp_dir(),
    };
    std::fs::canonicalize(&base).unwrap_or(base)
}

/// 一个临时目录：名字是 前缀 + 6 个随机字符，只有自己能进（700）；离开作用域时连里面的东西一起删掉（设了 KEEP 就留着）。
/// 当 `&Path` 用就行：`dir.join("home")`、`dir.exists()`
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// 在 [`tmp_base`] 下建一个；建不了就 panic
    pub fn new(prefix: &str) -> TempDir {
        TempDir::new_in(&tmp_base(), prefix)
    }

    /// 在 base 下建一个（base 没有就先建）；建不了就 panic
    pub fn new_in(base: &Path, prefix: &str) -> TempDir {
        if let Err(e) = std::fs::create_dir_all(base) {
            panic!("建不了目录 {}：{e}", base.display());
        }
        let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
        loop {
            let path = base.join(format!("{prefix}{}", random_name()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return TempDir { path },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("建不了临时目录 {}：{e}", path.display()),
            }
        }
    }

    /// 它的路径（解过链接的）
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !keep() {
            remove_tree(&self.path);
        }
    }
}

/// 6 个随机字符（小写字母和数字）
fn random_name() -> String {
    const CHARS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    crate::crypto::random(6).iter().map(|b| CHARS[*b as usize % 36] as char).collect()
}

/// 连里面的东西一起删掉；本来就没有也算删掉了。测试会把目录改成不能写、不能进的：删不动时先把权限放开再删一遍。
/// 链接只删链接自己，不跟进去。还是删不掉就算了（不 panic：这是收尾，别盖住用例自己的失败）
pub fn remove_tree(path: &Path) {
    if std::fs::remove_dir_all(path).is_ok() || std::fs::symlink_metadata(path).is_err() {
        return;
    }
    unlock(path);
    let _ = std::fs::remove_dir_all(path);
}

/// 把这棵目录树里的目录都改成自己能读、能写、能进
fn unlock(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if !meta.is_dir() {
        return;
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            unlock(&entry.path());
        }
    }
}

/// 读整个文件
pub fn read(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    std::fs::read(path).unwrap_or_else(|e| panic!("读不了 {}：{e}", path.display()))
}

/// 读整个文件，当文字（不是 UTF-8 就 panic）
pub fn read_text(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    String::from_utf8(read(path)).unwrap_or_else(|_| panic!("{} 不是 UTF-8 的文字", path.display()))
}

/// 读整个文件，当文字；文件不在（或者读不了、不是 UTF-8）是 None——等一个文件出现、等它的内容变成某个样子时用
pub fn try_read_text(path: impl AsRef<Path>) -> Option<String> {
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

/// 写整个文件（文字、字节都行）；上面的目录没有就先建
pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            panic!("建不了目录 {}：{e}", dir.display());
        }
    }
    std::fs::write(path, data).unwrap_or_else(|e| panic!("写不了 {}：{e}", path.display()));
}

/// 写整个文件，再把权限改成 mode（`0o600` 这样写）
pub fn write_mode(path: impl AsRef<Path>, data: impl AsRef<[u8]>, mode: u32) {
    write(&path, data);
    chmod(path, mode);
}

/// 写一个能执行的脚本（755）：PATH 上的假命令就是这样放的
pub fn write_script(path: impl AsRef<Path>, text: &str) {
    write_mode(path, text, 0o755);
}

/// 读一个 JSON 文件；读不了、不是 JSON 就 panic
pub fn read_json(path: impl AsRef<Path>) -> json::Value {
    let path = path.as_ref();
    json::parse_bytes(&read(path)).unwrap_or_else(|e| panic!("{} 不是 JSON：{e}", path.display()))
}

/// 写一个 JSON 文件：缩进 2 格、非 ASCII 原样写、末尾没有换行。要别的写法就 `write(path, json::dumps(v, &style))`
pub fn write_json(path: impl AsRef<Path>, value: &json::Value) {
    write(path, value.pretty());
}

/// 文件的权限位（`0o600`、`0o755` 这样的数；链接看的是它指的那个文件）
pub fn mode(path: impl AsRef<Path>) -> u32 {
    let path = path.as_ref();
    match std::fs::metadata(path) {
        Ok(meta) => meta.permissions().mode() & 0o7777,
        Err(e) => panic!("看不了 {} 的权限：{e}", path.display()),
    }
}

/// 改文件的权限位
pub fn chmod(path: impl AsRef<Path>, mode: u32) {
    let path = path.as_ref();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("改不了 {} 的权限：{e}", path.display()));
}

/// [`wait_for`] 的条件给出来的东西：`bool`（成立了没有），或者 `Option<T>`（等到了就是那个值）
pub trait Waited {
    type Out;
    fn ready(self) -> Option<Self::Out>;
}

impl Waited for bool {
    type Out = ();
    fn ready(self) -> Option<()> {
        self.then_some(())
    }
}

impl<T> Waited for Option<T> {
    type Out = T;
    fn ready(self) -> Option<T> {
        self
    }
}

/// 等到 cond 成立：每 50 毫秒看一次，成立了就返回（`Option` 的话返回里面的值）；到 timeout 还没成立就 panic，
/// 说“没等到：what”。等得久只在出毛病时才碰得到——机器再忙也该等到的，所以时限给宽一点，别拿它当 sleep 用。
///
/// ```ignore
/// wait_for("服务器写下端口", Duration::from_secs(30), || fsx::try_read_text(&port_file));
/// wait_for("会话结束", Duration::from_secs(10), || !sock.exists());
/// ```
pub fn wait_for<R: Waited>(what: &str, timeout: Duration, mut cond: impl FnMut() -> R) -> R::Out {
    let end = Instant::now() + timeout;
    loop {
        if let Some(got) = cond().ready() {
            return got;
        }
        if Instant::now() >= end {
            panic!("没等到：{what}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 本机一个现在空着的端口（在 127.0.0.1 上绑一下 0 号端口、记下系统给的、马上放开）。别的进程可能抢在你前面用掉它：
/// 能让被测的程序自己挑端口（绑 0 号）再读回来的地方，优先那样做
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|e| panic!("在 127.0.0.1 上绑不了端口：{e}"));
    let port = listener.local_addr().unwrap_or_else(|e| panic!("问不到绑上的端口：{e}")).port();
    drop(listener);
    settle_port(port);
    port
}

/// 等 127.0.0.1 上这个端口连不上（没人听）为止，最多 5 秒；给出等没等到。这个进程刚关掉的监听端口不一定马上连不上：别的线程
/// 正好在起子进程时，内核里那个还没换成新程序的子进程带着这边所有描述符的一份（换的时候才关），这一小会儿里连接照样接得进来、
/// 随后被断开——对方看到的是 ConnectionReset、BrokenPipe，不是 ConnectionRefused；马上在这个端口上再绑也绑不上。
/// macOS 上实测：四个线程不停地起子进程时，关掉就连的 3000 次里有 42 次连上了（没有别的线程起子进程时一次都没有）
pub fn settle_port(port: u16) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return true,
            _ if Instant::now() >= end => return false,
            _ => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panic_text(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let err = std::panic::catch_unwind(f).expect_err("应该 panic");
        err.downcast_ref::<String>().cloned().or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
    }

    #[test]
    fn temp_dir_is_private_unique_and_removed() {
        let base = TempDir::new("fsx-unit-");
        let (a, b) = (TempDir::new_in(&base, "x-"), TempDir::new_in(&base, "x-"));
        assert_ne!(a.path(), b.path());
        assert!(a.is_dir() && a.is_absolute());
        assert_eq!(mode(&a), 0o700);
        let name = a.file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.starts_with("x-") && name.len() == 8, "{name}");
        assert_eq!(std::fs::canonicalize(a.path()).unwrap(), a.path(), "给出来的是解过链接的路径");
        // 里面有不能写、不能进的目录也删得掉；链接不跟进去
        let outside = base.join("outside");
        write(outside.join("keep.txt"), "留着");
        write(a.join("locked/deep/file"), "x");
        std::os::unix::fs::symlink(&outside, a.join("link")).unwrap();
        chmod(a.join("locked/deep"), 0o500);
        chmod(a.join("locked"), 0);
        let path = a.path().to_path_buf();
        drop(a);
        assert!(!path.exists());
        assert_eq!(read_text(outside.join("keep.txt")), "留着");
        assert!(b.exists());
        // new_in：base 没有就先建
        let nested = TempDir::new_in(&base.join("not/yet"), "n-");
        assert!(nested.is_dir());
    }

    #[test]
    fn read_write_helpers() {
        let dir = TempDir::new("fsx-unit-");
        let file = dir.join("a/b/c.txt");
        write(&file, "文字\n");
        assert_eq!(read_text(&file), "文字\n");
        assert_eq!(read(&file), "文字\n".as_bytes());
        write(&file, [0xffu8, 0x00]);
        assert_eq!(read(&file), [0xff, 0x00]);
        assert_eq!(try_read_text(&file), None, "不是 UTF-8");
        assert_eq!(try_read_text(dir.join("nope")), None);
        assert!(panic_text(|| drop(read_text(dir.join("nope")))).starts_with("读不了 "));

        write_mode(dir.join("key"), "secret", 0o600);
        assert_eq!(mode(dir.join("key")), 0o600);
        write_script(dir.join("bin/fake"), "#!/bin/sh\nexit 3\n");
        assert_eq!(mode(dir.join("bin/fake")), 0o755);
        chmod(dir.join("key"), 0o640);
        assert_eq!(mode(dir.join("key")), 0o640);

        let value = json::parse(r#"{"名字":"值","n":[1,2.5,null]}"#).unwrap();
        write_json(dir.join("v.json"), &value);
        assert_eq!(read_text(dir.join("v.json")), "{\n  \"名字\": \"值\",\n  \"n\": [\n    1,\n    2.5,\n    null\n  ]\n}");
        assert_eq!(read_json(dir.join("v.json")), value);
        write(dir.join("bad.json"), "{");
        assert!(panic_text(|| drop(read_json(dir.join("bad.json")))).contains("不是 JSON"));
    }

    #[test]
    fn wait_for_returns_the_value_or_panics_with_what() {
        // 成立了就返回：bool 的没有值，Option 的给里面的值
        wait_for("马上成立", Duration::from_secs(5), || true);
        let mut calls = 0;
        let got = wait_for("第三次给出值", Duration::from_secs(5), || {
            calls += 1;
            (calls == 3).then_some("值")
        });
        assert_eq!((got, calls), ("值", 3));
        // 没等到：说在等什么；时限到之前至少每 50 毫秒看一次，到了以后不再多等
        let start = Instant::now();
        let mut looked = 0;
        let text = panic_text(std::panic::AssertUnwindSafe(|| {
            wait_for("永远不会有的文件", Duration::from_millis(220), || {
                looked += 1;
                false
            })
        }));
        assert_eq!(text, "没等到：永远不会有的文件");
        assert!(start.elapsed() >= Duration::from_millis(220) && start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
        assert!((2..=6).contains(&looked), "{looked}");
        // 时限是 0 也看一次
        assert_eq!(wait_for("看一次", Duration::ZERO, || Some(7)), 7);
    }

    #[test]
    fn free_port_can_be_bound() {
        let port = free_port();
        assert!(port >= 1024);
        std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    }
}
