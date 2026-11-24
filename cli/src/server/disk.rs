//! 状态文件怎么落盘：写到旁边的临时文件，让系统真的写到盘上（fsync），再换过去；没写成不留下临时文件。
//!
//! Python 那一份只是“写临时文件、换过去”，不 fsync：断电、内核崩了以后，换过去的可能是一个空文件——重启时它读不了，当作
//! 从头开始，整个 Team 的设备名单就没了（管理员的设备能照留着的公钥登记回去，但那之前各台设备都同步不了）。磁盘满了写到
//! 一半时，那个临时文件也一直留在盘上。这里补上这两处；文件的内容、名字、权限和以前一样。

use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// 没成的是哪一步（各处照 Python 的 OSError 说哪个文件，说法不一样）
#[derive(Debug, PartialEq)]
pub enum Step {
    /// 建不了临时文件
    Open,
    /// 写不进去、落不了盘（临时文件删掉了）
    Write,
    /// 换不过去（临时文件删掉了）
    Rename,
}

/// 把 text 存成 path：写到 tmp（新建的话权限 600）、fsync、换过去，再让目录也落盘
pub fn replace(tmp: &Path, path: &Path, text: &[u8]) -> Result<(), (Step, io::Error)> {
    let mut f =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(tmp).map_err(|e| (Step::Open, e))?;
    let written = f.write_all(text).and_then(|()| synced(&f));
    drop(f);
    if let Err(e) = written {
        let _ = std::fs::remove_file(tmp);
        return Err((Step::Write, e));
    }
    if let Err(e) = std::fs::rename(tmp, path) {
        let _ = std::fs::remove_file(tmp);
        return Err((Step::Rename, e));
    }
    // “这个名字指着新文件”也要落盘。做不了（有的文件系统不让对目录 fsync）不算没存成：文件已经换好了
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if let Ok(d) = File::open(dir) {
        let _ = synced(&d);
    }
    Ok(())
}

/// fsync。直接调系统的：标准库的 `sync_all` 在 macOS 上是 F_FULLFSYNC（等硬盘把自己的缓存也写完，一次几十毫秒，
/// 测试里每个请求存一次受不了）；服务器跑在 Linux 上，那里 fsync 就是落盘。这个文件系统不支持（EINVAL、ENOTSUP）不算错
fn synced(f: &File) -> io::Result<()> {
    // SAFETY: f 开着，fsync 只用它的描述符
    if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EINVAL) | Some(libc::ENOTSUP) => Ok(()),
        _ => Err(e),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;

    fn names(dir: &str) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// 存成了：内容是给的，权限 600，临时文件不在了；再存一次是整个换掉
    #[test]
    fn replace_writes_the_file_and_leaves_no_temp() {
        let d = TempDir::new().unwrap();
        let (tmp, path) = (format!("{}/s.json.1.tmp", d.path), format!("{}/s.json", d.path));
        replace(Path::new(&tmp), Path::new(&path), b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":1}");
        assert_eq!(std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&path).unwrap()) & 0o777, 0o600);
        replace(Path::new(&tmp), Path::new(&path), b"{}").unwrap();
        assert_eq!((std::fs::read(&path).unwrap(), names(&d.path)), (b"{}".to_vec(), vec!["s.json".to_string()]));
    }

    /// 没成：说是哪一步；换不过去时临时文件删掉，原来的文件不动
    #[test]
    fn a_failed_replace_names_the_step_and_cleans_up() {
        let d = TempDir::new().unwrap();
        let gone = format!("{}/nowhere/s.json", d.path);
        let (step, e) = replace(Path::new(&format!("{gone}.1.tmp")), Path::new(&gone), b"x").unwrap_err();
        assert_eq!((step, e.kind()), (Step::Open, io::ErrorKind::NotFound));
        // 目标是一个不空的目录：rename 不成
        let (tmp, path) = (format!("{}/t.1.tmp", d.path), format!("{}/t", d.path));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(format!("{path}/kept"), "原来的").unwrap();
        let (step, _) = replace(Path::new(&tmp), Path::new(&path), b"x").unwrap_err();
        assert_eq!(step, Step::Rename);
        assert_eq!(
            (names(&d.path), std::fs::read_to_string(format!("{path}/kept")).unwrap()),
            (vec!["t".to_string()], "原来的".to_string())
        );
    }
}
