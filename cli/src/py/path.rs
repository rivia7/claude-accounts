//! `os.path`：路径一律是 UTF-8 的字符串，规则照 Python 的 posixpath（`realpath` 遇到不存在的那一段照样往下拼，
//! Rust 的 `canonicalize` 会报错；`dirname("/a/b/")` 是 `/a/b`）。

use std::os::unix::fs::MetadataExt;

/// `os.path.expanduser(p)`：只认 `~` 和 `~/…`（`~别人` 原来的代码没用到，原样返回）
pub fn expanduser(p: &str) -> String {
    if p != "~" && !p.starts_with("~/") {
        return p.to_string();
    }
    let home = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => h,
        _ => home_from_passwd().unwrap_or_default(),
    };
    if home.is_empty() {
        return p.to_string();
    }
    let home = home.trim_end_matches('/');
    let rest = &p[1..];
    if home.is_empty() {
        if rest.is_empty() { "/".into() } else { rest.to_string() }
    } else {
        format!("{home}{rest}")
    }
}

/// 用户数据库里记的主目录（不看 HOME）
pub fn home_from_passwd() -> Option<String> {
    // SAFETY: getpwuid 返回的指针指向 C 库自己的静态缓冲，马上拷出来
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

/// `os.path.join(a, b)`
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `os.path.join(a, b, c, …)`
pub fn join_all(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts {
        out = join(&out, p);
    }
    out
}

/// `os.path.dirname(p)`
pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        None => String::new(),
        Some(i) => {
            let head = &p[..i + 1];
            let cut = head.trim_end_matches('/');
            if cut.is_empty() { head.to_string() } else { cut.to_string() }
        }
    }
}

/// `os.path.basename(p)`
pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        None => p,
        Some(i) => &p[i + 1..],
    }
}

/// `os.path.normpath(p)`
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let lead = if p.starts_with("//") && !p.starts_with("///") { 2 } else if p.starts_with('/') { 1 } else { 0 };
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (lead == 0 && parts.is_empty()) || parts.last() == Some(&"..") {
            parts.push(c);
        } else if !parts.is_empty() {
            parts.pop();
        }
    }
    let body = parts.join("/");
    let out = format!("{}{}", "/".repeat(lead), body);
    if out.is_empty() { ".".into() } else { out }
}

/// `os.path.abspath(p)`
pub fn abspath(p: &str) -> String {
    if p.starts_with('/') {
        return normpath(p);
    }
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
    normpath(&join(&cwd, p))
}

/// `os.path.realpath(p)`：符号链接都解开；不存在的、解不开的（链接成环）那一段原样留着
pub fn realpath(p: &str) -> String {
    let start = if p.starts_with('/') { "/".to_string() } else { std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into()) };
    let mut seen = std::collections::HashMap::new();
    let (path, _) = join_real(&start, p, &mut seen);
    abspath(&path)
}

/// posixpath._joinrealpath：把 rest 一段段接到 path 上，遇到链接就解。返回（路径，解成了没有）
fn join_real(path: &str, rest: &str, seen: &mut std::collections::HashMap<String, Option<String>>) -> (String, bool) {
    let mut path = path.to_string();
    let mut rest = rest.to_string();
    if rest.starts_with('/') {
        rest = rest[1..].to_string();
        path = "/".into();
    }
    while !rest.is_empty() {
        let (name, tail) = match rest.find('/') {
            Some(i) => (rest[..i].to_string(), rest[i + 1..].to_string()),
            None => (rest.clone(), String::new()),
        };
        rest = tail;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if path.is_empty() {
                path = "..".into();
            } else {
                let (parent, base) = (dirname(&path), basename(&path).to_string());
                path = if base == ".." { join(&path, "..") } else { parent };
            }
            continue;
        }
        let newpath = join(&path, &name);
        let is_link = std::fs::symlink_metadata(&newpath).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(done) = seen.get(&newpath) {
            match done {
                Some(resolved) => {
                    path = resolved.clone();
                    continue;
                }
                // 链接成环：这一段和剩下的原样接上
                None => return (join(&newpath, &rest), false),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = match std::fs::read_link(&newpath) {
            Ok(t) => t.to_string_lossy().into_owned(),
            Err(_) => {
                path = newpath;
                continue;
            }
        };
        let (resolved, ok) = join_real(&path, &target, seen);
        if !ok {
            return (join(&resolved, &rest), false);
        }
        seen.insert(newpath, Some(resolved.clone()));
        path = resolved;
    }
    (path, true)
}

/// `os.path.relpath(p, start)`
pub fn relpath(p: &str, start: &str) -> String {
    let (p, s) = (abspath(p), abspath(start));
    let a: Vec<&str> = p.split('/').filter(|x| !x.is_empty()).collect();
    let b: Vec<&str> = s.split('/').filter(|x| !x.is_empty()).collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<&str> = vec![".."; b.len() - common];
    parts.extend(&a[common..]);
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// `os.path.exists(p)`（跟着链接看）
pub fn exists(p: &str) -> bool {
    std::fs::metadata(p).is_ok()
}

/// `os.path.lexists(p)`
pub fn lexists(p: &str) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// `os.path.isdir(p)`
pub fn isdir(p: &str) -> bool {
    std::fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// `os.path.isfile(p)`
pub fn isfile(p: &str) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// `os.path.islink(p)`
pub fn islink(p: &str) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// `os.path.samefile(a, b)`：读不了算不是同一个
pub fn samefile(a: &str, b: &str) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_functions_like_posixpath() {
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/a/", "b"), "/a/b");
        assert_eq!(join("/a", "/b"), "/b");
        assert_eq!(join("", "b"), "b");
        assert_eq!(join("/a", ""), "/a/");
        assert_eq!(join_all(&["/a", "b", "c.json"]), "/a/b/c.json");
        assert_eq!([dirname("/a/b"), dirname("/a/b/"), dirname("a"), dirname("/a"), dirname("//a")], ["/a", "/a/b", "", "/", "//"]);
        assert_eq!([basename("/a/b"), basename("/a/b/"), basename("a")], ["b", "", "a"]);
        for (p, want) in [
            ("", "."),
            ("/a/./b//c/..", "/a/b"),
            ("a/../..", ".."),
            ("/..", "/"),
            ("//a", "//a"),
            ("///a", "/a"),
            ("./a/", "a"),
            ("../a/../b", "../b"),
        ] {
            assert_eq!(normpath(p), want, "{p}");
        }
        assert_eq!(relpath("/a/b/c", "/a/x"), "../b/c");
        assert_eq!(relpath("/a/b", "/a/b"), ".");
    }

    #[test]
    fn realpath_keeps_missing_tail() {
        let d = std::env::temp_dir().join(format!("cca-path-{}", std::process::id()));
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(d.file_name().unwrap());
        std::fs::create_dir_all(d.join("real/sub")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("link")).unwrap();
        std::os::unix::fs::symlink("loop2", d.join("loop1")).unwrap();
        std::os::unix::fs::symlink("loop1", d.join("loop2")).unwrap();
        let s = d.to_str().unwrap();
        assert_eq!(realpath(&format!("{s}/link/sub/../sub/missing/x.json")), format!("{s}/real/sub/missing/x.json"));
        assert_eq!(realpath(&format!("{s}/nope/a")), format!("{s}/nope/a"));
        assert_eq!(realpath(&format!("{s}/loop1/x")), format!("{s}/loop1/x"));
        assert!(islink(&format!("{s}/link")) && isdir(&format!("{s}/link")) && !isfile(&format!("{s}/link")));
        assert!(samefile(&format!("{s}/link"), &format!("{s}/real")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn expanduser_uses_home() {
        // 只读环境变量，不改：测试是并行跑的
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expanduser("~"), home.trim_end_matches('/'));
        assert_eq!(expanduser("~/x/y"), format!("{}/x/y", home.trim_end_matches('/')));
        assert_eq!(expanduser("/a/~"), "/a/~");
        assert_eq!(expanduser("~other/x"), "~other/x");
    }
}
