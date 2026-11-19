//! 安装包解压（`cu_extract`）。

use super::casefold::normalized;
use super::pyv::*;
use super::{cu, CU_MAX_PACKAGE};
use crate::py::fs;
use crate::py::shutil::symlink;
use crate::py::zip::{ZipError, ZipFile, ZipInfo};
use crate::py::{path, Fail, R};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

/// zipfile 出的错照原来的异常类往上传
fn zip_fail(e: ZipError) -> Fail {
    match e {
        ZipError::Bad(msg) => Fail::Other("BadZipFile", vec![msg]),
        ZipError::Os(e) => Fail::Os(e),
        ZipError::NotImplemented(msg) => Fail::Other("NotImplementedError", vec![msg]),
        ZipError::Runtime(msg) => Fail::Other("RuntimeError", vec![msg]),
        ZipError::Zlib(msg) => Fail::Other("zlib.error", vec![msg]),
        ZipError::Eof => Fail::Other("EOFError", Vec::new()),
        ZipError::Unicode(msg) => Fail::Value(msg),
        ZipError::Overflow(msg) => Fail::Other("OverflowError", vec![msg]),
        ZipError::Memory => Fail::Other("MemoryError", Vec::new()),
        ZipError::Value(msg) => Fail::Value(msg),
    }
}

/// `re.search(r'[\x00-\x1f\x7f\\:]', s)`
fn has_unsafe_char(s: &str) -> bool {
    s.chars().any(|c| c < ' ' || c == '\x7f' || c == '\\' || c == ':')
}

/// `posixpath.normpath(posixpath.join(posixpath.dirname(name), target))`
fn link_resolved(name: &str, target: &str) -> String {
    path::normpath(&path::join(&path::dirname(name), target))
}

/// `str(Path(p).resolve(strict=True))`，也就是 `os.path.realpath(p, strict=True)`（CPython 3.14）：链接都解开，哪一段不存在、
/// 中间哪一段不是目录、链接成环都是 OSError。不用 `std::fs::canonicalize`：macOS 上它给的是磁盘上实际的写法（大小写照磁盘上的），
/// 原来是照着路径的写法一段一段解的，后面拿结果和解压目录比开头，写法不一样就对不上
fn resolve_strict(filename: &str) -> std::io::Result<String> {
    let errno = |n: i32| std::io::Error::from_raw_os_error(n);
    // 还没解的各段，倒着放；None 是记号：它下面那一项是一个链接的路径，解到这里时 path 就是这个链接最后指向的地方
    let mut rest: Vec<Option<String>> = filename.split('/').rev().map(|s| Some(s.to_string())).collect();
    let mut part_count = rest.len();
    let mut path =
        if filename.starts_with('/') { "/".to_string() } else { std::env::current_dir()?.to_string_lossy().into_owned() };
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    while part_count > 0 {
        let Some(name) = rest.pop().expect("part_count 说还有") else {
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
                Some(i) if i > 0 => path[..i].to_string(),
                _ => "/".to_string(),
            };
            continue;
        }
        let newpath = if path == "/" { format!("/{name}") } else { format!("{path}/{name}") };
        let st = std::fs::symlink_metadata(&newpath)?;
        if !st.file_type().is_symlink() {
            if part_count > 0 && !st.is_dir() {
                return Err(errno(libc::ENOTDIR));
            }
            path = newpath;
            continue;
        }
        match seen.get(&newpath) {
            Some(Some(resolved)) => {
                path = resolved.clone();
                continue;
            }
            Some(None) => return Err(errno(libc::ELOOP)), // 还没解完又绕回来了
            None => {}
        }
        let target = std::fs::read_link(&newpath)?.to_string_lossy().into_owned();
        if target.starts_with('/') {
            path = "/".to_string();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let before = rest.len();
        rest.extend(target.split('/').rev().map(|s| Some(s.to_string())));
        part_count += rest.len() - before;
    }
    Ok(path)
}

/// 只接受实证的单根 ZIP（根目录是这个客户端的 .app）；先验证整张目录表，再写普通文件，最后建内部链接。
pub fn cu_extract(archive: &str, folder: &str) -> R<String> {
    #[cfg(test)]
    super::tests::on_extract(archive);
    match extract(archive, folder) {
        // except (zipfile.BadZipFile, UnicodeError, OSError, RuntimeError, ValueError)：里面自己报的 ClientUpdateError 不在其中
        Err(e) if e.is("BadZipFile") || e.is_os_or_value() || e.is("RuntimeError") || e.is("NotImplementedError") => {
            cu_error("blocked", "无法安全解压官方安装包")
        }
        other => other,
    }
}

fn extract(archive: &str, folder: &str) -> R<String> {
    let Some(app_name) = cu().app else {
        return attr_error("'CuCli' object has no attribute 'app'");
    };
    let z = ZipFile::open(archive).map_err(zip_fail)?;
    let entries = z.infolist();
    if entries.is_empty()
        || entries.len() > 100000
        || entries.iter().map(|x| x.file_size as u128).sum::<u128>() > 8 * 1024 * 1024 * 1024
    {
        return cu_error("blocked", "官方安装包条目数量或解压大小超出限制");
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut paths: Vec<(&ZipInfo, String, u32)> = Vec::new();
    let mut links: HashMap<String, String> = HashMap::new();
    for item in entries {
        let name = item.filename.trim_end_matches('/').to_string();
        let parts: Vec<&str> = name.split('/').collect();
        let mode = item.external_attr >> 16;
        let kind = mode & S_IFMT;
        let key = normalized(&name);
        if name.is_empty()
            || parts[0] != app_name
            || parts.iter().any(|x| matches!(*x, "" | "." | ".."))
            || has_unsafe_char(&name)
            || seen.contains(&key)
            || item.flag_bits & 1 != 0
            || !matches!(kind, 0 | S_IFREG | S_IFDIR | S_IFLNK)
            || item.file_size > CU_MAX_PACKAGE
        {
            return cu_error("blocked", "官方安装包包含不支持的路径或重复条目");
        }
        seen.insert(key.clone());
        if kind == S_IFLNK {
            if item.file_size > 4096 {
                return cu_error("blocked", "安装包链接长度不安全");
            }
            let raw = z.read(item).map_err(zip_fail)?;
            let target = match String::from_utf8(raw) {
                Ok(target) => target,
                Err(e) => return Err(Fail::Value(crate::py::json::utf8_error_text(e.as_bytes(), &e.utf8_error()))),
            };
            let resolved = link_resolved(&name, &target);
            if target.is_empty()
                || target.starts_with('/')
                || has_unsafe_char(&target)
                || !resolved.starts_with(&format!("{app_name}/"))
            {
                return cu_error("blocked", "安装包包含指向程序外的链接");
            }
            links.insert(key, target);
        }
        paths.push((item, name, mode));
    }
    for (_, name, _) in &paths {
        let parts: Vec<&str> = name.split('/').collect();
        if (1..parts.len()).any(|n| links.contains_key(&normalized(&parts[..n].join("/")))) {
            return cu_error("blocked", "安装包不能通过链接写入其他目录");
        }
    }
    for (item, name, mode) in &paths {
        super::interrupted()?;
        let target = path::join(folder, name);
        if mode & S_IFMT == S_IFLNK {
            continue;
        }
        if item.is_dir() || mode & S_IFMT == S_IFDIR {
            fs::makedirs_mode(&target, 0o755)?;
        } else {
            fs::makedirs_mode(&path::dirname(&target), 0o755)?;
            let mut source = z.open_member(item).map_err(zip_fail)?;
            let mut destination = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o666).open(&target)?;
            loop {
                let block = source.read(1024 * 1024).map_err(zip_fail)?;
                if block.is_empty() {
                    break;
                }
                destination.write_all(&block)?;
            }
            drop(destination);
            fs::chmod(&target, if mode & 0o111 != 0 { 0o755 } else { 0o644 })?;
        }
    }
    for (_, name, mode) in &paths {
        if mode & S_IFMT == S_IFLNK {
            let target = path::join(folder, name);
            fs::makedirs_mode(&path::dirname(&target), 0o755)?;
            symlink(&links[&normalized(name)], &target)?;
        }
    }
    let app = path::join(folder, app_name);
    for (_, name, mode) in &paths {
        if mode & S_IFMT == S_IFLNK {
            let resolved = resolve_strict(&path::join(folder, name))?;
            if !resolved.starts_with(&format!("{app}/")) {
                return cu_error("blocked", "安装包链接未留在程序目录内");
            }
        }
    }
    Ok(app)
}
