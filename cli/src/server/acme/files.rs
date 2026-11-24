//! 证书助手读写文件的那几样：`put_file`、`read`（Acme:103-124），和 Python 的 `os.makedirs(…, exist_ok=True)`。
//!
//! 没成都是 `Trouble::Os`，里面的话照 Python 的 `str(OSError)` 写：`[Errno 13] Permission denied: '/路径'`——这句话会出现在
//! “起不来：…”“…的证书放不好：…”里，装的人照它找原因。

use super::Trouble;
use crate::py::fail::{errno_text, os_error_text};
use crate::py::fs::{chmod, open_error_text};
use crate::py::obj::repr_str;
use crate::py::path;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// 一个文件最多读这么多（Acme:122 的 `f.read(1 << 20)`）
pub const READ_MAX: u64 = 1 << 20;

/// 换上一个文件：先写到旁边、改好权限，再换过去（读它的程序读不到半截）（Acme:103-116）。没成时旁边那个临时文件删掉。
/// 和 Python 不一样的一点：临时文件一建出来就只有自己读得了（Python 是先照 umask 建、写完才改权限：私钥有一小会儿别人读得了）
pub fn put_file(path: &str, data: &[u8], mode: u32) -> Result<(), Trouble> {
    let tmp = format!("{path}.{}.tmp", std::process::id());
    let done = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| open_error_text(&e, &tmp))?;
        f.write_all(data).map_err(|e| os_error_text(&e))?;
        drop(f);
        chmod(&tmp, mode).map_err(|e| open_error_text(&e, &tmp))?;
        fs::rename(&tmp, path).map_err(|e| format!("{}: {} -> {}", os_error_text(&e), repr_str(&tmp), repr_str(path)))
    })();
    done.map_err(|text| {
        let _ = fs::remove_file(&tmp);
        Trouble::Os(text)
    })
}

/// 文件开头的 cap 个字节（不够就是全部）
pub fn read_capped(path: &str, cap: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    fs::File::open(path)?.take(cap).read_to_end(&mut out)?;
    Ok(out)
}

/// 读一个文件（最多 1 MiB）；读不了是 None（Acme:119-124）
pub fn read(path: &str) -> Option<Vec<u8>> {
    read_capped(path, READ_MAX).ok()
}

/// `e.strerror or e`：系统给的那句说明，不带错误号
pub fn strerror(e: &io::Error) -> String {
    e.raw_os_error().map_or_else(|| e.to_string(), errno_text)
}

/// `os.path.split(p)`：最后一个 / 的前面和后面（前面那一段结尾的 / 去掉，除非它全是 /）
fn split(p: &str) -> (&str, &str) {
    let (head, tail) = p.split_at(p.rfind('/').map_or(0, |i| i.saturating_add(1)));
    let cut = head.trim_end_matches('/');
    (if cut.is_empty() { head } else { cut }, tail)
}

/// 没成时带着是哪个路径：上面缺的哪一层没建成，说的是那一层
fn makedirs_at(name: &str, mode: u32) -> Result<(), (io::Error, String)> {
    let (mut head, mut tail) = split(name);
    if tail.is_empty() {
        (head, tail) = split(head);
    }
    if !head.is_empty() && !tail.is_empty() && !path::exists(head) {
        match makedirs_at(head, 0o777) {
            // 别人刚好也在建这一层
            Err((e, _)) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        if tail == "." {
            return Ok(()); // xxx/newdir/. ：xxx/newdir 有了它就有了
        }
    }
    match fs::DirBuilder::new().mode(mode).create(name) {
        // 不能只认“已经有了”这一种错：系统可能先报没权限、只读（Python 的 os.makedirs 也是这么写的）
        Err(e) if !path::isdir(name) => Err((e, name.to_string())),
        _ => Ok(()),
    }
}

/// `os.makedirs(name, mode=mode, exist_ok=True)`：只有最里面那一层用这个权限（还要过 umask），上面缺的各层是默认的；
/// 已经有了的不动它。`py::fs::makedirs_mode` 做的是同一件事，这里多的是出错时那句话带着路径
pub fn makedirs(name: &str, mode: u32) -> Result<(), Trouble> {
    makedirs_at(name, mode).map_err(|(e, at)| Trouble::Os(open_error_text(&e, &at)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::{mode_of, TempDir};

    fn os(r: Result<(), Trouble>) -> String {
        match r {
            Err(Trouble::Os(text)) => text,
            other => panic!("要的是 Trouble::Os，得到 {other:?}"),
        }
    }

    /// 换上文件：内容、权限对，旁边不留临时文件；原来有的被换掉（不是改它）
    #[test]
    fn put_file_writes_then_swaps() {
        let d = TempDir::new().unwrap();
        let p = path::join(&d.path, "privkey.pem");
        for (data, mode) in [(&b"one"[..], 0o600), (b"two two", 0o644), (b"", 0o640)] {
            put_file(&p, data, mode).unwrap();
            assert_eq!((fs::read(&p).unwrap(), mode_of(&p).unwrap()), (data.to_vec(), mode));
            assert_eq!(crate::py::fs::listdir_sorted(&d.path).unwrap(), ["privkey.pem"], "没有留下半截的文件");
        }
        // 换的是整个文件：以前打开着它的还读得到旧的那一份
        put_file(&p, b"old", 0o600).unwrap();
        let mut old = fs::File::open(&p).unwrap();
        put_file(&p, b"new", 0o600).unwrap();
        let mut seen = String::new();
        old.read_to_string(&mut seen).unwrap();
        assert_eq!((seen.as_str(), fs::read(&p).unwrap()), ("old", b"new".to_vec()));
    }

    /// 没成时说的话照 Python 的 OSError，临时文件不留下
    #[test]
    fn put_file_failures_read_like_python() {
        let d = TempDir::new().unwrap();
        let pid = std::process::id();
        // 目录不在：开不了临时文件
        let p = path::join(&d.path, "nope/fullchain.pem");
        assert_eq!(os(put_file(&p, b"x", 0o644)), format!("[Errno 2] No such file or directory: '{p}.{pid}.tmp'"));
        // 要换掉的是个有东西的目录：换不过去，说的是从哪到哪
        let p = path::join(&d.path, "taken");
        fs::create_dir_all(path::join(&p, "inside")).unwrap();
        let said = os(put_file(&p, b"x", 0o644));
        assert!(said.starts_with("[Errno ") && said.ends_with(&format!(": '{p}.{pid}.tmp' -> '{p}'")), "{said}");
        assert_eq!(crate::py::fs::listdir_sorted(&d.path).unwrap(), ["taken"], "临时文件删掉了");
    }

    /// 读文件：不在、读不了是 None；太大的只要开头
    #[test]
    fn read_is_capped_and_quiet() {
        let d = TempDir::new().unwrap();
        let p = path::join(&d.path, "f");
        assert_eq!(read(&p), None);
        assert_eq!(read(&d.path), None, "目录读不了");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(read(&p), Some(b"abc".to_vec()));
        assert_eq!(read_capped(&p, 2).unwrap(), b"ab");
        fs::write(&p, vec![b'x'; (READ_MAX + 10) as usize]).unwrap();
        assert_eq!(read(&p).unwrap().len() as u64, READ_MAX);
        assert_eq!(strerror(&io::Error::from_raw_os_error(libc::EACCES)), "Permission denied");
        assert_eq!(strerror(&io::Error::other("别的")), "别的");
    }

    #[test]
    fn split_like_os_path() {
        assert_eq!(split("/a/b"), ("/a", "b"));
        assert_eq!(split("/a/b/"), ("/a/b", ""));
        assert_eq!(split("a"), ("", "a"));
        assert_eq!(split("/a"), ("/", "a"));
        assert_eq!(split("//a"), ("//", "a"));
        assert_eq!(split("a//b"), ("a", "b"));
        assert_eq!(split(""), ("", ""));
    }

    /// 建目录：缺的各层都建，最里面那层是要的权限；已经有了不算错、权限不动；没成时说的是哪一层
    #[test]
    fn makedirs_like_python() {
        let d = TempDir::new().unwrap();
        let leaf = path::join(&d.path, "a/b/c");
        makedirs(&leaf, 0o700).unwrap();
        assert!(path::isdir(&leaf));
        assert_eq!(mode_of(&leaf).unwrap(), 0o700);
        assert_ne!(mode_of(&path::join(&d.path, "a/b")).unwrap() & 0o700, 0, "上面的各层是默认的权限");
        // 已经有了：不算错，权限也不改
        chmod(&leaf, 0o755).unwrap();
        makedirs(&leaf, 0o700).unwrap();
        makedirs(&format!("{leaf}/"), 0o700).unwrap();
        assert_eq!(mode_of(&leaf).unwrap(), 0o755);
        // 结尾带 / 的、带 /. 的
        makedirs(&path::join(&d.path, "x/y/"), 0o700).unwrap();
        makedirs(&path::join(&d.path, "p/q/."), 0o700).unwrap();
        assert!(path::isdir(&path::join(&d.path, "x/y")) && path::isdir(&path::join(&d.path, "p/q")));
        // 那里是个文件
        let f = path::join(&d.path, "file");
        fs::write(&f, b"").unwrap();
        assert_eq!(os(makedirs(&f, 0o700)), format!("[Errno 17] File exists: '{f}'"));
        let under = path::join(&f, "sub");
        assert_eq!(os(makedirs(&under, 0o700)), format!("[Errno 20] Not a directory: '{under}'"));
        // 上面那层没权限建：说的是没建成的那一层，不是最里面的
        let locked = path::join(&d.path, "locked");
        makedirs(&locked, 0o500).unwrap();
        let deep = path::join(&locked, "m/n");
        if fs::write(path::join(&locked, "probe"), b"").is_err() {
            // root 不受权限管：那样这一条测不了
            assert_eq!(os(makedirs(&deep, 0o700)), format!("[Errno 13] Permission denied: '{}'", path::join(&locked, "m")));
        }
        chmod(&locked, 0o700).unwrap();
        assert_eq!(os(makedirs("", 0o700)), "[Errno 2] No such file or directory: ''");
    }
}
