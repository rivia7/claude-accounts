//! 读写文件：JSON 文件的读法、原子替换的写法照原来的 `load_json` / `load_quiet` / `save_json`。

use super::fail::{die, Fail, R};
use super::json;
use super::obj::V;
use super::path;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// `open(path, encoding='utf-8').read()`：不是 UTF-8 是 ValueError（UnicodeDecodeError），换行统一成 `\n`
pub fn read_text(p: &str) -> R<String> {
    let raw = fs::read(p)?;
    match String::from_utf8(raw) {
        Ok(s) if s.contains('\r') => Ok(s.replace("\r\n", "\n").replace('\r', "\n")),
        Ok(s) => Ok(s),
        Err(e) => Err(Fail::Value(json::utf8_error_text(e.as_bytes(), &e.utf8_error()))),
    }
}

/// `load_json(path, default)`：没有这个文件是 None（调用方用默认值）；格式不对直接报错退出；别的读不了（没权限、
/// 是个目录）在 Python 里是没人接的异常，这里照样往上传
pub fn load_json(p: &str) -> R<Option<V>> {
    let raw = match fs::read(p) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    match json::loads_file_bytes(&raw) {
        Ok(v) => Ok(Some(v)),
        Err(e) => die(format!("JSON 格式错误 {p}：{e}")),
    }
}

/// `open(path)` 失败时 `str(e)` 的样子：`[Errno 2] No such file or directory: '/路径'`（带着文件名的 repr）
pub fn open_error_text(e: &io::Error, p: &str) -> String {
    format!("{}: {}", crate::py::fail::os_error_text(e), crate::py::obj::repr_str(p))
}

/// `load_quiet(path)`：读不了、格式不对都当作没有（缓存之类，不值得为它报错退出）
pub fn load_quiet(p: &str) -> Option<V> {
    json::loads_file_bytes(&fs::read(p).ok()?).ok()
}

/// 文件的权限位（`os.stat(p).st_mode & 0o777`，跟着链接看）
pub fn mode_of(p: &str) -> io::Result<u32> {
    Ok(fs::metadata(p)?.permissions().mode() & 0o777)
}

/// `os.makedirs(p, exist_ok=True)`
pub fn makedirs(p: &str) -> io::Result<()> {
    if p.is_empty() {
        return Err(io::Error::from_raw_os_error(libc::ENOENT)); // makedirs('') 在 Python 里是 FileNotFoundError
    }
    fs::create_dir_all(p)
}

/// `os.makedirs(p, mode, exist_ok=True)`：新建的每一层都是这个权限（还要过 umask）
pub fn makedirs_mode(p: &str, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(mode).create(p)
}

/// `os.chmod(p, mode)`
pub fn chmod(p: &str, mode: u32) -> io::Result<()> {
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
}

/// `save_json(path, data, mode=None, follow_symlinks=True)`：`indent=2, ensure_ascii=False`，末尾一个换行；
/// 先写旁边的临时文件再改名换上。mode 没给时沿用原文件的权限，原来没有这个文件就是 600。
/// follow_symlinks：path 是链接时写它指向的文件（默认）；False 时把链接本身换成普通文件
pub fn save_json(p: &str, data: &V, mode: Option<u32>, follow_symlinks: bool) -> R<()> {
    let p = if follow_symlinks { path::realpath(p) } else { path::abspath(p) };
    makedirs(&path::dirname(&p))?;
    let mode = match mode {
        Some(m) => m,
        None => match mode_of(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0o600,
            Err(e) => return Err(e.into()),
        },
    };
    let tmp = format!("{}.cca-{}.tmp", p, std::process::id());
    let mut text = json::dumps(data, json::FILE);
    text.push('\n');
    // 和 Python 的 open(tmp, 'w') 一样：已有的截断，新建的是 666 过 umask，写完才改成要的权限
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(&tmp)?.write_all(text.as_bytes())?;
    chmod(&tmp, mode)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

/// `sorted(os.listdir(p))`：名字不是 UTF-8 的条目跳过
pub fn listdir_sorted(p: &str) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(p)?.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;

    fn tmpdir(tag: &str) -> String {
        let d = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-fs-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    #[test]
    fn save_and_load() {
        let d = tmpdir("save");
        let f = format!("{d}/sub/x.json");
        save_json(&f, &V::Dict(dict! { "名字" => "值", "n" => 1 }), None, true).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "{\n  \"名字\": \"值\",\n  \"n\": 1\n}\n");
        assert_eq!(mode_of(&f).unwrap(), 0o600);
        chmod(&f, 0o644).unwrap();
        save_json(&f, &V::Dict(dict! {}), None, true).unwrap();
        assert_eq!((fs::read_to_string(&f).unwrap().as_str(), mode_of(&f).unwrap()), ("{}\n", 0o644));
        save_json(&f, &V::from(1), Some(0o640), true).unwrap();
        assert_eq!(mode_of(&f).unwrap(), 0o640);
        assert!(load_json(&f).unwrap().unwrap().py_eq(&V::Int(1)));
        assert!(load_json(&format!("{d}/none.json")).unwrap().is_none());
        assert!(listdir_sorted(&format!("{d}/sub")).unwrap() == ["x.json"]); // 临时文件没留下
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn symlinks() {
        let d = tmpdir("link");
        fs::write(format!("{d}/real.json"), "{}").unwrap();
        std::os::unix::fs::symlink(format!("{d}/real.json"), format!("{d}/link.json")).unwrap();
        save_json(&format!("{d}/link.json"), &V::from(1), None, true).unwrap();
        assert!(path::islink(&format!("{d}/link.json")));
        assert_eq!(fs::read_to_string(format!("{d}/real.json")).unwrap(), "1\n");
        save_json(&format!("{d}/link.json"), &V::from(2), None, false).unwrap();
        assert!(!path::islink(&format!("{d}/link.json")));
        assert_eq!(fs::read_to_string(format!("{d}/real.json")).unwrap(), "1\n");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bad_json() {
        let d = tmpdir("bad");
        let f = format!("{d}/bad.json");
        fs::write(&f, "{oops").unwrap();
        match load_json(&f) {
            Err(Fail::Die(m)) => {
                assert_eq!(m, format!("JSON 格式错误 {f}：Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"))
            }
            other => panic!("{other:?}"),
        }
        assert!(load_quiet(&f).is_none());
        assert!(load_quiet(&format!("{d}/none")).is_none());
        fs::write(&f, b"\xff").unwrap();
        assert!(matches!(read_text(&f), Err(Fail::Value(m)) if m.contains("invalid start byte")));
        fs::write(&f, "a\r\nb\rc").unwrap();
        assert_eq!(read_text(&f).unwrap(), "a\nb\nc");
        fs::remove_dir_all(&d).unwrap();
    }
}
