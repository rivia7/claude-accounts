//! 这个模块要用、兼容层（`cli/src/py/`）里还没有的 Python 标准库行为：`os.walk`、`shutil` 的 copy2 / copytree / rmtree / move、
//! `filecmp.cmp`、`tempfile.TemporaryDirectory`、`uuid.UUID`、`bytes.splitlines`、只给最后一层权限的 `os.makedirs`……
//! 行为照 CPython 3.14（测试里的期望值都是拿真的 Python 跑出来的）。先放在这里只给本模块用，以后该移进兼容层。
//!
//! 路径照兼容层的约定是 UTF-8 的字符串。名字不是 UTF-8 的条目：只是列一列的地方跳过（和 `py::fs::listdir_sorted` 一样）；
//! 要整棵拷走、并回、删掉的地方报错停下——Python 能照常处理这种名字，这里处理不了，但不能当它不存在（跳过就等于丢东西）。

use crate::py::path;
use crate::py::rand;
use crate::py::uni;
use crate::py::R;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

fn cstr(p: &str) -> io::Result<CString> {
    CString::new(p).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "embedded null byte"))
}

fn not_utf8(dir: &str, name: &std::ffi::OsStr) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("文件名不是 UTF-8，处理不了：{}/{}", dir, name.to_string_lossy()))
}

/// 系统说“不支持这个操作”（ENOTSUP / EOPNOTSUPP：macOS 上是两个数，Linux 上是同一个）
fn unsupported(e: &io::Error) -> bool {
    let n = e.raw_os_error();
    n == Some(libc::ENOTSUP) || n == Some(libc::EOPNOTSUPP)
}

/// `st.st_mtime`：小数的秒数。CPython 里是一个 C 表达式 `(double)秒 + 1e-9 * 纳秒`（不是「纳秒 ÷ 1e9」，差最后一位）。
/// arm64 上编译器把它合成一次乘加（只舍入一次）：拿 CPython 3.14（macOS arm64）实测，8 万个随机时间全都等于 fma 的结果，
/// 先乘后加的在秒数小的时候对不上。x86-64 的通用版本没有乘加指令，是先乘后加（没实测）。
/// 两个挨得很近的修改时间比新旧（`merge_file`、`adopt_task_lists`）靠的就是这最后一位
pub fn st_mtime(m: &fs::Metadata) -> f64 {
    let (sec, nsec) = (m.mtime() as f64, m.mtime_nsec() as f64);
    if cfg!(target_arch = "aarch64") {
        1e-9f64.mul_add(nsec, sec)
    } else {
        sec + 1e-9 * nsec
    }
}

/// `os.path.getmtime(p)`（跟着链接看）
pub fn getmtime(p: &str) -> io::Result<f64> {
    Ok(st_mtime(&fs::metadata(p)?))
}

/// `os.lstat(p).st_mtime`
pub fn lstat_mtime(p: &str) -> io::Result<f64> {
    Ok(st_mtime(&fs::symlink_metadata(p)?))
}

/// `os.readlink(p)`，当文字用（不是 UTF-8 的字节换成 U+FFFD：只拿去比较、显示）
pub fn readlink(p: &str) -> io::Result<String> {
    Ok(fs::read_link(p)?.to_string_lossy().into_owned())
}

/// `os.symlink(target, link)`
pub fn symlink(target: impl AsRef<Path>, link: &str) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// `os.listdir(p)`：系统给的先后（不排序）；名字不是 UTF-8 的条目跳过
pub fn listdir(p: &str) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for e in fs::read_dir(p)? {
        if let Ok(n) = e?.file_name().into_string() {
            names.push(n);
        }
    }
    Ok(names)
}

/// `os.listdir(p)`，遇到不是 UTF-8 的名字报错：后面要把这个目录整个并走、删掉的地方用它
pub fn listdir_strict(p: &str) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for e in fs::read_dir(p)? {
        names.push(e?.file_name().into_string().map_err(|n| not_utf8(p, &n))?);
    }
    Ok(names)
}

/// `for root, dirs, files in os.walk(top)`：自上而下，不跟着链接进目录。每到一个目录调一次 visit(root, dirs, files)：
/// dirs 是里面的目录（指向目录的链接也算），files 是别的；先后是系统给的，visit 可以改 dirs（排序、去掉不想进的）。
/// 读不了的目录跳过、不报（os.walk 默认就是这样）
pub fn walk(top: &str, mut visit: impl FnMut(&str, &mut Vec<String>, &[String]) -> R<()>) -> R<()> {
    let mut stack = vec![top.to_string()];
    while let Some(top) = stack.pop() {
        let Ok(entries) = fs::read_dir(&top).and_then(|rd| rd.collect::<io::Result<Vec<_>>>()) else {
            continue;
        };
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        for e in entries {
            let name = e.file_name().into_string().map_err(|n| not_utf8(&top, &n))?;
            // entry.is_dir()：跟着链接看；看不了的算不是目录
            if path::isdir(&path::join(&top, &name)) {
                dirs.push(name);
            } else {
                files.push(name);
            }
        }
        visit(&top, &mut dirs, &files)?;
        for d in dirs.iter().rev() {
            let sub = path::join(&top, d);
            if !path::islink(&sub) {
                stack.push(sub);
            }
        }
    }
    Ok(())
}

/// `filecmp.cmp(a, b, shallow=False)`：两个都是普通文件、一样大、内容逐字节一样
pub fn filecmp(a: &str, b: &str) -> io::Result<bool> {
    use std::io::Read;
    let (ma, mb) = (fs::metadata(a)?, fs::metadata(b)?);
    if !ma.is_file() || !mb.is_file() || ma.len() != mb.len() {
        return Ok(false);
    }
    // 一块一块比（原来也是）：共享目录里的大文件不用整个读进内存
    let (fa, fb) = (fs::File::open(a)?, fs::File::open(b)?);
    let (mut ba, mut bb) = (Vec::new(), Vec::new());
    loop {
        ba.clear();
        bb.clear();
        let n = (&fa).take(65536).read_to_end(&mut ba)?;
        let m = (&fb).take(65536).read_to_end(&mut bb)?;
        if n != m || ba != bb {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// `with open(p, 'w', encoding='utf-8') as f: f.write(text)`：已有的截断（跟着链接），新建的是 666 过 umask
pub fn write_text(p: &str, text: &str) -> io::Result<()> {
    use std::io::Write;
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(p)?.write_all(text.as_bytes())
}

/// `os.makedirs(p, mode=mode, exist_ok=True)`：mode 只管最后一层，中间新建的各层是默认的 777（都还要过 umask）。
/// 兼容层的 `py::fs::makedirs_mode` 把 mode 用在了每一层上，和 Python（3.7 起）不一样，所以这里另写
pub fn makedirs_leaf_mode(p: &str, mode: u32) -> io::Result<()> {
    let head = path::dirname(p.trim_end_matches('/'));
    if !head.is_empty() && !path::exists(&head) {
        fs::create_dir_all(&head)?;
    }
    match fs::DirBuilder::new().mode(mode).create(p) {
        Err(e) if !path::isdir(p) => Err(e),
        _ => Ok(()),
    }
}

/// `shutil.copyfile(src, dst)`：只拷内容。dst 已有就截断重写（跟着链接），没有就新建
fn copyfile(src: &str, dst: &str) -> io::Result<()> {
    if path::samefile(src, dst) {
        return Err(io::Error::other(format!("{src:?} and {dst:?} are the same file")));
    }
    for p in [src, dst] {
        if fs::metadata(p).is_ok_and(|m| m.file_type().is_fifo()) {
            return Err(io::Error::other(format!("`{p}` is a named pipe")));
        }
    }
    let mut from = fs::File::open(src)?;
    if from.metadata()?.is_dir() {
        // Python 的 open(src, 'rb') 在这里就是 IsADirectoryError：别先把 dst 建出来
        return Err(io::Error::from_raw_os_error(libc::EISDIR));
    }
    let mut to = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(dst)?;
    io::copy(&mut from, &mut to)?;
    Ok(())
}

/// `shutil._copyxattr`：Python 只在 Linux 上拷扩展属性（macOS 上的 os 模块没有 listxattr）。
/// 列不出来（不支持）就算了；单个属性读不了、写不进（没权限、不支持）跳过，别的错照报
#[cfg(target_os = "linux")]
fn copyxattr(src: &str, dst: &str, follow: bool) -> io::Result<()> {
    let (s, d) = (cstr(src)?, cstr(dst)?);
    let errno_in = |e: &io::Error, set: &[i32]| e.raw_os_error().is_some_and(|n| set.contains(&n));
    // CPython 先拿小缓冲试，放不下（ERANGE）换成系统的上限 64K，再放不下就报
    let mut list = vec![0u8; 256];
    let len = loop {
        // SAFETY: s 以 0 结尾；list 是有效的可写内存，长度如实给出
        let n = unsafe {
            if follow {
                libc::listxattr(s.as_ptr(), list.as_mut_ptr().cast(), list.len())
            } else {
                libc::llistxattr(s.as_ptr(), list.as_mut_ptr().cast(), list.len())
            }
        };
        if n >= 0 {
            break n as usize;
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ERANGE) && list.len() < 65536 {
            list.resize(65536, 0);
        } else if errno_in(&e, &[libc::ENOTSUP, libc::ENODATA, libc::EINVAL]) {
            return Ok(());
        } else {
            return Err(e);
        }
    };
    for name in list[..len].split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = CString::new(name).expect("按 0 切出来的名字里没有 0");
        let one = || -> io::Result<()> {
            let mut value = vec![0u8; 128];
            let n = loop {
                // SAFETY: 两个字符串都以 0 结尾；value 是有效的可写内存，长度如实给出
                let n = unsafe {
                    if follow {
                        libc::getxattr(s.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
                    } else {
                        libc::lgetxattr(s.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
                    }
                };
                if n >= 0 {
                    break n as usize;
                }
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::ERANGE) && value.len() < 65536 {
                    value.resize(65536, 0);
                } else {
                    return Err(e);
                }
            };
            // SAFETY: 同上；value 的前 n 个字节是刚读到的
            let rc = unsafe {
                if follow {
                    libc::setxattr(d.as_ptr(), name.as_ptr(), value.as_ptr().cast(), n, 0)
                } else {
                    libc::lsetxattr(d.as_ptr(), name.as_ptr(), value.as_ptr().cast(), n, 0)
                }
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        };
        if let Err(e) = one() {
            if !errno_in(&e, &[libc::EPERM, libc::ENOTSUP, libc::ENODATA, libc::EINVAL, libc::EACCES]) {
                return Err(e);
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn copyxattr(_src: &str, _dst: &str, _follow: bool) -> io::Result<()> {
    Ok(())
}

/// `shutil.copystat(src, dst, follow_symlinks=…)`：访问和修改时间（到纳秒）、权限、macOS 上的文件标志、Linux 上的扩展属性。
/// follow_symlinks 为 false 时，只有两边都是链接才作用在链接本身上（Python 就是这么定的）
fn copystat(src: &str, dst: &str, follow_symlinks: bool) -> io::Result<()> {
    let follow = follow_symlinks || !(path::islink(src) && path::islink(dst));
    let st = if follow { fs::metadata(src)? } else { fs::symlink_metadata(src)? };
    let c = cstr(dst)?;
    let times = [
        libc::timespec { tv_sec: st.atime() as _, tv_nsec: st.atime_nsec() as _ },
        libc::timespec { tv_sec: st.mtime() as _, tv_nsec: st.mtime_nsec() as _ },
    ];
    // SAFETY: c 以 0 结尾，times 是两项的数组
    if unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW }) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // 扩展属性要先拷：权限改成只读以后就写不进去了
    copyxattr(src, dst, follow)?;
    let mode = st.mode() & 0o7777;
    if follow {
        fs::set_permissions(dst, fs::Permissions::from_mode(mode))?;
    } else if cfg!(target_os = "macos") {
        // 链接本身的权限：只有 macOS 能改（Linux 上 os.chmod 不支持 follow_symlinks=False，copystat 这一步什么都不做）
        // SAFETY: c 以 0 结尾
        if unsafe { libc::fchmodat(libc::AT_FDCWD, c.as_ptr(), mode as libc::mode_t, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            let e = io::Error::last_os_error();
            if !unsupported(&e) {
                return Err(e);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        // SAFETY: c 以 0 结尾
        let rc = unsafe {
            if follow {
                libc::chflags(c.as_ptr(), st.st_flags())
            } else {
                libc::lchflags(c.as_ptr(), st.st_flags())
            }
        };
        if rc != 0 {
            let e = io::Error::last_os_error();
            if !unsupported(&e) {
                return Err(e);
            }
        }
    }
    Ok(())
}

/// `shutil.copy2(src, dst)`：内容，加上权限、时间这些（修改时间跟着 src：`merge_file` 靠它比新旧）。
/// dst 是目录时拷进去、名字照 src。返回实际写的那个路径
pub fn copy2(src: &str, dst: &str) -> io::Result<String> {
    let dst = if path::isdir(dst) { path::join(dst, path::basename(src)) } else { dst.to_string() };
    copyfile(src, &dst)?;
    copystat(src, &dst, true)?;
    Ok(dst)
}

/// `shutil.copytree(src, dst, symlinks=True)`：dst 不能已经有；里面的链接照样建成链接。
/// 某一项出了错接着拷别的，最后一起报（原来就是这样：能拷的都拷了才报错）
pub fn copytree(src: &str, dst: &str) -> io::Result<()> {
    copytree_at(src, dst).map_err(|errors| io::Error::other(errors.join("; ")))
}

fn copytree_at(src: &str, dst: &str) -> Result<(), Vec<String>> {
    let one = |e: io::Error| vec![format!("{src} -> {dst}: {e}")];
    let entries = fs::read_dir(src).and_then(|rd| rd.collect::<io::Result<Vec<_>>>()).map_err(one)?;
    // os.makedirs(dst)：上面几层没有就建，dst 自己已经有就是错
    let head = path::dirname(dst.trim_end_matches('/'));
    if !head.is_empty() && !path::exists(&head) {
        fs::create_dir_all(&head).map_err(one)?;
    }
    fs::create_dir(dst).map_err(one)?;
    let mut errors = Vec::new();
    for e in entries {
        let name = match e.file_name().into_string() {
            Ok(n) => n,
            Err(n) => {
                errors.push(not_utf8(src, &n).to_string());
                continue;
            }
        };
        let (s, d) = (path::join(src, &name), path::join(dst, &name));
        let item = |e: io::Error| vec![format!("{s} -> {d}: {e}")];
        let done = match e.file_type() {
            Err(e) => Err(item(e)),
            Ok(t) if t.is_symlink() => fs::read_link(&s).and_then(|to| symlink(to, &d)).and_then(|()| copystat(&s, &d, false)).map_err(item),
            Ok(t) if t.is_dir() => copytree_at(&s, &d),
            Ok(_) => copy2(&s, &d).map(drop).map_err(item),
        };
        if let Err(mut errs) = done {
            errors.append(&mut errs);
        }
    }
    if let Err(e) = copystat(src, dst, true) {
        errors.extend(one(e));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// `shutil.rmtree(p)`：p 是链接时不删（报错）。先删这一层的文件，再进子目录（后列出来的先删），和 Python 的先后一样；
/// 出错就停在那里
pub fn rmtree(p: &str) -> io::Result<()> {
    if fs::symlink_metadata(p)?.file_type().is_symlink() {
        return Err(io::Error::other("Cannot call rmtree on a symbolic link"));
    }
    rmtree_dir(Path::new(p))
}

fn rmtree_dir(p: &Path) -> io::Result<()> {
    let mut dirs = Vec::new();
    for e in fs::read_dir(p)?.collect::<io::Result<Vec<_>>>()? {
        // entry.is_dir(follow_symlinks=False)；看不出来的当文件删
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            dirs.push(e.path());
            continue;
        }
        match fs::remove_file(e.path()) {
            Err(x) if x.kind() != io::ErrorKind::NotFound => return Err(x),
            _ => {}
        }
    }
    while let Some(d) = dirs.pop() {
        match rmtree_dir(&d) {
            Err(x) if x.kind() != io::ErrorKind::NotFound => return Err(x),
            _ => {}
        }
    }
    fs::remove_dir(p)
}

/// `shutil.move(src, dst)`：dst 是目录就搬进去；能改名就改名，改不了（跨文件系统；macOS 上把没有写权限的目录搬到别的目录下……）
/// 就拷过去再删。返回实际搬到的路径
pub fn shutil_move(src: &str, dst: &str) -> io::Result<String> {
    let mut real_dst = dst.to_string();
    if path::isdir(dst) {
        if path::samefile(src, dst) && !path::islink(src) {
            // 不分大小写的文件系统上同一个目录的两种写法：照样改名
            fs::rename(src, dst)?;
            return Ok(real_dst);
        }
        real_dst = path::join(dst, path::basename(src.trim_end_matches('/')));
        if path::exists(&real_dst) {
            return Err(io::Error::other(format!("Destination path '{real_dst}' already exists")));
        }
    }
    if fs::rename(src, &real_dst).is_err() {
        move_by_copy(src, dst, &real_dst)?;
    }
    Ok(real_dst)
}

/// shutil.move 里改名不成以后的那一半：链接照原样重建，目录整棵拷过去再删，文件拷过去再删
fn move_by_copy(src: &str, dst: &str, real_dst: &str) -> io::Result<()> {
    if path::islink(src) {
        let to = fs::read_link(src)?;
        symlink(to, real_dst)?;
        fs::remove_file(src)
    } else if path::isdir(src) {
        let slashed = |p: &str| {
            let p = path::abspath(p);
            if p.ends_with('/') { p } else { p + "/" }
        };
        if slashed(dst).starts_with(&slashed(src)) {
            return Err(io::Error::other(format!("Cannot move a directory '{src}' into itself '{dst}'.")));
        }
        // access、listdir、是不是 macOS 照原来的先后看（listdir 读不了在哪个系统上都是错）
        if is_immutable(src)? || (!writable(src)? && !listdir(src)?.is_empty() && cfg!(target_os = "macos")) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Cannot move the non-empty directory '{src}': Lacking write permission to '{src}'."),
            ));
        }
        copytree(src, real_dst)?;
        rmtree(src)
    } else {
        copy2(src, real_dst)?;
        fs::remove_file(src)
    }
}

/// `shutil._is_immutable(p)`：文件标志正好是「不可改」（只有 macOS / BSD 的 stat 有这一项）
fn is_immutable(p: &str) -> io::Result<bool> {
    let st = fs::metadata(p)?;
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        Ok(matches!(st.st_flags(), libc::UF_IMMUTABLE | libc::SF_IMMUTABLE))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = st;
        Ok(false)
    }
}

/// `os.access(p, os.W_OK)`
fn writable(p: &str) -> io::Result<bool> {
    let c = cstr(p)?;
    // SAFETY: c 以 0 结尾
    Ok(unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

/// `tempfile.TemporaryDirectory(prefix=…, dir=…)`：dir 里一个只有自己能进的临时目录（名字后面 8 个随机字符）。
/// 用完调 `cleanup()` 连里面的东西一起删掉；中途出错直接丢开时也删（那时删不掉不再报）
pub struct TemporaryDirectory {
    pub name: String,
    done: bool,
}

impl TemporaryDirectory {
    pub fn new(prefix: &str, dir: &str) -> io::Result<TemporaryDirectory> {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
        for _ in 0..10000 {
            let tail: String = rand::urandom(8).iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect();
            let p = path::join(dir, &format!("{prefix}{tail}"));
            match fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => return Ok(TemporaryDirectory { name: path::abspath(&p), done: false }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "No usable temporary directory name found"))
    }

    pub fn cleanup(mut self) -> io::Result<()> {
        self.done = true;
        rmtree_reset(&self.name)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if !self.done {
            let _ = rmtree_reset(&self.name);
        }
    }
}

/// TemporaryDirectory 收尾时的删法：因为没权限删不动的（拷过来的只读目录、带「不可改」标志的文件），
/// 把权限改回 700、标志清掉再删
fn rmtree_reset(p: &str) -> io::Result<()> {
    match rmtree(p) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => return other,
    }
    fn reset(p: &Path) {
        let Ok(st) = fs::symlink_metadata(p) else { return };
        #[cfg(target_os = "macos")]
        if let Some(c) = p.to_str().and_then(|s| CString::new(s).ok()) {
            // SAFETY: c 以 0 结尾
            unsafe { libc::lchflags(c.as_ptr(), 0) };
        }
        if st.file_type().is_symlink() {
            return;
        }
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o700));
        if st.is_dir() {
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                reset(&e.path());
            }
        }
    }
    reset(Path::new(p));
    match rmtree(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `str(uuid.UUID(s))`：认得出就给出标准写法（小写、8-4-4-4-12），认不出是 None（Python 里是 ValueError）。
/// Python 认的写法比看上去宽：`urn:`、`uuid:` 出现在哪都去掉，两头的花括号、所有的连字符去掉，剩下的正好 32 个字符，
/// 再交给 `int(…, 16)`——所以 `0x` 开头、`+` 号、数字之间的下划线、别的文字的十进制数字也都认（不够 32 位的在前面补 0）
pub fn uuid_str(s: &str) -> Option<String> {
    let hex = s.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(['{', '}']).replace('-', "");
    if hex.chars().count() != 32 {
        return None;
    }
    let n = int16(&hex)?;
    let h = format!("{n:032x}");
    Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

/// `int(s, 16)`，s 不超过 32 个字符（放得进 u128）；负数、认不出的是 None
fn int16(s: &str) -> Option<u128> {
    // 先照 CPython 换成 ASCII：别的文字的空白当空格、十进制数字当 0–9，别的非 ASCII 字符（和 0x7f）都不认
    let mut b = Vec::with_capacity(s.len());
    for c in s.chars() {
        b.push(match c {
            c if (c as u32) < 127 => c as u8,
            c if uni::is_space(c) => b' ',
            c => b'0' + uni::decimal_value(c)? as u8,
        });
    }
    let space = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let mut i = b.iter().take_while(|c| space(c)).count();
    match b.get(i) {
        Some(b'+') => i += 1,
        Some(b'-') => return None, // 负数不是 UUID（连字符前面已经去掉，其实到不了这里）
        _ => {}
    }
    if b.get(i) == Some(&b'0') && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        if b.get(i) == Some(&b'_') {
            i += 1; // 前缀后面可以有一个下划线
        }
    }
    if b.get(i) == Some(&b'_') {
        return None;
    }
    let (mut n, mut digits, mut prev) = (0u128, 0, 0u8);
    while let Some(&c) = b.get(i) {
        if c == b'_' {
            if prev == b'_' {
                return None;
            }
        } else if let Some(d) = (c as char).to_digit(16) {
            n = n.checked_mul(16)?.checked_add(d as u128)?;
            digits += 1;
        } else {
            break;
        }
        prev = c;
        i += 1;
    }
    if digits == 0 || prev == b'_' {
        return None;
    }
    i += b[i..].iter().take_while(|c| space(c)).count();
    (i == b.len()).then_some(n)
}

/// `bytes.splitlines()`：只在 `\n`、`\r`、`\r\n` 处断行（字符串的 splitlines 认的更多），末尾不多出一个空行
pub fn splitlines_bytes(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < data.len() {
        let c = data[i];
        if c == b'\n' || c == b'\r' {
            out.push(&data[start..i]);
            i += if c == b'\r' && data.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
            start = i;
        } else {
            i += 1;
        }
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::text;

    fn tmpdir(tag: &str) -> String {
        let d = fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-pylib-{}-{}", tag, std::process::id()));
        if d.exists() {
            rmtree_reset(d.to_str().unwrap()).unwrap();
        }
        fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    fn put(p: &str, data: &str, mode: u32, mtime: (i64, i64)) {
        fs::create_dir_all(path::dirname(p)).unwrap();
        fs::write(p, data).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
        stamp(p, mtime);
    }

    /// 这一项自己的访问、修改时间（不跟着链接）
    fn stamp(p: &str, (sec, nsec): (i64, i64)) {
        let c = cstr(p).unwrap();
        let ts = libc::timespec { tv_sec: sec, tv_nsec: nsec };
        // SAFETY: c 以 0 结尾，times 是两项的数组
        assert_eq!(unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [ts, ts].as_ptr(), libc::AT_SYMLINK_NOFOLLOW) }, 0);
    }

    /// 一项的样子：种类（链接指向哪、文件内容）、权限、修改时间（纳秒）、macOS 上的文件标志
    fn look(p: &str) -> String {
        let st = fs::symlink_metadata(p).unwrap();
        let kind = if st.file_type().is_symlink() {
            format!("link -> {}", readlink(p).unwrap())
        } else if st.is_dir() {
            "dir".to_string()
        } else {
            format!("file {:?}", fs::read_to_string(p).unwrap())
        };
        #[cfg(target_os = "macos")]
        let flags = std::os::macos::fs::MetadataExt::st_flags(&st);
        #[cfg(not(target_os = "macos"))]
        let flags = 0;
        format!("{} mode={:o} mtime={} flags={:#x}", kind, st.mode() & 0o7777, st.mtime() * 1_000_000_000 + st.mtime_nsec(), flags)
    }

    /// 整棵树每一项的样子（相对路径排过序）
    fn tree(root: &str) -> Vec<String> {
        let mut out = Vec::new();
        walk(root, |top, dirs, files| {
            for n in dirs.iter().chain(files) {
                let p = path::join(top, n);
                out.push(format!("{}: {}", path::relpath(&p, root), look(&p)));
            }
            Ok(())
        })
        .unwrap();
        out.sort();
        out
    }

    #[cfg(target_os = "macos")]
    fn hide(p: &str) {
        let c = cstr(p).unwrap();
        // SAFETY: c 以 0 结尾
        assert_eq!(unsafe { libc::chflags(c.as_ptr(), 0x8000) }, 0); // UF_HIDDEN
    }

    #[test]
    fn uuid_like_python() {
        // 右边是 CPython 3.14 的 str(uuid.UUID(左边.strip()))，认不出（ValueError）的是 None
        let a30 = "a".repeat(30);
        let a31 = "a".repeat(31);
        let cases: Vec<(String, Option<&str>)> = [
            ("abcdef01-2345-6789-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("ABCDEF01-2345-6789-ABCD-EF0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            (" abcdef01-2345-6789-abcd-ef0123456789\u{a}", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("{abcdef01-2345-6789-abcd-ef0123456789}", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("{{abcdef01-2345-6789-abcd-ef0123456789}}", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("}abcdef01-2345-6789-abcd-ef0123456789{", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("urn:uuid:abcdef01-2345-6789-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("uuid:urn:abcdef01-2345-6789-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("abcdef01-2345-6789urn:-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("uurn:uid:abcdef01-2345-6789-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("abcdef0123456789abcdef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("a-b-c-d-e-f-0-1-2-3-4-5-6-7-8-9-a-b-c-d-e-f-0-1-2-3-4-5-6-7-8-9", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("---------abcdef01-2345-6789-abcd-ef0123456789", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("abcdef01-2345-6789-abcd-ef0123456789{}{}", Some("abcdef01-2345-6789-abcd-ef0123456789")),
            ("abcdef01-2345-6789-abcd-ef012345678", None),
            ("abcdef01-2345-6789-abcd-ef01234567890", None),
            ("gbcdef01-2345-6789-abcd-ef0123456789", None),
            ("", None),
            ("not-a-uuid", None),
            ("00000000000000000000000000000000", Some("00000000-0000-0000-0000-000000000000")),
            ("ffffffffffffffffffffffffffffffff", Some("ffffffff-ffff-ffff-ffff-ffffffffffff")),
            ("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("00aaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("0XBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", Some("00bbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")),
            ("+aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("0aaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("+0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("000aaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("+0X_FFFFFFFFFFFFFFFFFFFFFFFFFFFF", Some("0000ffff-ffff-ffff-ffff-ffffffffffff")),
            ("0x_aaaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("000aaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("0xaaaaaaaaaaaaaa_aaaaaaaaaaaaaaa", Some("000aaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_", None),
            ("_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_", None),
            ("aaaaaaaaaaaaaaa__aaaaaaaaaaaaaaa", None),
            ("0_xaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("0x__aaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("+_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("++aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("0x+aaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("0xxaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", None),
            ("0xgggggggggggggggggggggggggggggg", None),
            ("0b111111111111111111111111111111", Some("0b111111-1111-1111-1111-111111111111")),
            ("0o777777777777777777777777777777", None),
            ("aaaaaaaaaaaaaaaa aaaaaaaaaaaaaaa", None),
            ("{ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}", Some("0aaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa }", Some("0aaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{\u{9}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{a}}", Some("00aaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{\u{b}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{c}}", Some("00aaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{\u{1c}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}", None),
            ("{\u{3000}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{a0}}", Some("00aaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{\u{85}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}", Some("0aaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{ +0xaaaaaaaaaaaaaaaaaaaaaaaaaaa }", Some("00000aaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("{+ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa}", None),
            ("{                                }", None),
            ("{0x                              }", None),
            ("\u{ff11}\u{ff12}\u{ff13}\u{ff14}aaaaaaaaaaaaaaaaaaaaaaaaaaaa", Some("1234aaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")),
            ("1111111111111111111111111111111\u{b2}", None),
            ("\u{3000}abcdef01-2345-6789-abcd-ef0123456789\u{2028}", Some("abcdef01-2345-6789-abcd-ef0123456789")),
        ]
        .into_iter()
        .map(|(s, want)| (s.to_string(), want))
        .chain([
            ("\u{663}".repeat(32), Some("33333333-3333-3333-3333-333333333333")), // 别的文字的十进制数字
            ("\u{ff41}".repeat(32), None),                                        // 全角的 a 不是数字
            ("\u{e9}".repeat(32), None),
            (format!("{a31}\u{7f}"), None),
            (format!("{a31}\u{0}"), None),
            (format!("{{\u{1f}{a30}}}"), None),
        ])
        .collect();
        for (s, want) in cases {
            assert_eq!(uuid_str(text::strip(&s)).as_deref(), want, "{s:?}");
        }
    }

    #[test]
    fn bytes_splitlines_like_python() {
        let lines = |d: &'static [u8]| splitlines_bytes(d).into_iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect::<Vec<_>>();
        // \v、\f、\x1c、\x85 在 bytes 里不断行（str 才断）
        assert_eq!(lines(b"a\nb\r\nc\rd\x0be\x0cf\x1cg\x85h\n\ni\n"), ["a", "b", "c", "d\u{b}e\u{c}f\u{1c}g\u{fffd}h", "", "i"]);
        assert!(lines(b"").is_empty());
        assert_eq!(lines(b"\n"), [""]);
        assert_eq!(lines(b"x"), ["x"]);
        assert_eq!(lines(b"\r\n\r"), ["", ""]);
    }

    #[test]
    fn makedirs_gives_the_mode_to_the_last_level_only() {
        let d = tmpdir("makedirs");
        // os.makedirs('a/b/c', mode=0o700)：a、b 是 755，c 是 700（umask 022 下）
        makedirs_leaf_mode(&format!("{d}/a/b/c"), 0o700).unwrap();
        let mode = |p: &str| fs::metadata(format!("{d}/{p}")).unwrap().mode() & 0o777;
        assert_eq!((mode("a") & 0o700, mode("a/b") & 0o700, mode("a/b/c")), (0o700, 0o700, 0o700));
        assert_eq!(mode("a"), mode("a/b"));
        assert_eq!(mode("a"), fs::metadata(&d).unwrap().mode() & 0o777); // 和普通建的目录一样（过了 umask 的 777）
        // 已经有：不动它的权限；是个文件：报错
        fs::set_permissions(format!("{d}/a/b/c"), fs::Permissions::from_mode(0o755)).unwrap();
        makedirs_leaf_mode(&format!("{d}/a/b/c"), 0o700).unwrap();
        assert_eq!(mode("a/b/c"), 0o755);
        fs::write(format!("{d}/file"), "").unwrap();
        assert!(makedirs_leaf_mode(&format!("{d}/file"), 0o700).is_err());
        assert!(makedirs_leaf_mode(&format!("{d}/file/sub"), 0o700).is_err());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copy2_keeps_mode_and_times() {
        let d = tmpdir("copy2");
        let (src, dst) = (format!("{d}/src"), format!("{d}/dst"));
        put(&src, "data", 0o640, (200, 123_456_789));
        #[cfg(target_os = "macos")]
        hide(&src);
        // 新建、盖掉已有的（原来的权限不留）、拷进目录里
        assert_eq!(copy2(&src, &dst).unwrap(), dst);
        assert_eq!(look(&dst), look(&src));
        put(&dst, "old content that is longer", 0o600, (5, 0));
        copy2(&src, &dst).unwrap();
        assert_eq!(look(&dst), look(&src));
        fs::create_dir(format!("{d}/dir")).unwrap();
        assert_eq!(copy2(&src, &format!("{d}/dir")).unwrap(), format!("{d}/dir/src"));
        assert_eq!(look(&format!("{d}/dir/src")), look(&src));
        // dst 是链接：写进它指向的文件，链接留着；src 是链接：拷的是它指向的内容
        put(&format!("{d}/real"), "x", 0o600, (5, 0));
        symlink(format!("{d}/real"), &format!("{d}/to-real")).unwrap();
        copy2(&src, &format!("{d}/to-real")).unwrap();
        assert!(path::islink(&format!("{d}/to-real")));
        assert_eq!(look(&format!("{d}/real")), look(&src));
        symlink("src", &format!("{d}/to-src")).unwrap();
        copy2(&format!("{d}/to-src"), &format!("{d}/copy-of-link")).unwrap();
        assert_eq!(look(&format!("{d}/copy-of-link")), look(&src));
        // 同一个文件、没有、是目录：报错，而且不把 dst 建出来
        assert!(copy2(&src, &format!("{d}/to-src")).is_err());
        assert!(copy2(&format!("{d}/none"), &format!("{d}/x1")).is_err());
        assert!(copy2(&format!("{d}/dir"), &format!("{d}/x2")).is_err());
        assert!(!path::lexists(&format!("{d}/x1")) && !path::lexists(&format!("{d}/x2")));
        assert_eq!(fs::read_to_string(&src).unwrap(), "data");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn filecmp_compares_regular_files_by_content() {
        let d = tmpdir("filecmp");
        let f = |n: &str| format!("{d}/{n}");
        fs::write(f("a"), "same").unwrap();
        fs::write(f("b"), "same").unwrap();
        fs::write(f("c"), "diff").unwrap();
        fs::write(f("longer"), "same and more").unwrap();
        fs::create_dir(f("dir")).unwrap();
        symlink("a", &f("link")).unwrap();
        assert!(filecmp(&f("a"), &f("b")).unwrap() && filecmp(&f("a"), &f("link")).unwrap());
        assert!(!filecmp(&f("a"), &f("c")).unwrap() && !filecmp(&f("a"), &f("longer")).unwrap() && !filecmp(&f("a"), &f("dir")).unwrap());
        assert!(filecmp(&f("a"), &f("none")).is_err());
        // 比一块大的文件：只差最后一个字节也比得出
        let mut big = vec![7u8; 200_000];
        fs::write(f("big1"), &big).unwrap();
        fs::write(f("big2"), &big).unwrap();
        assert!(filecmp(&f("big1"), &f("big2")).unwrap());
        *big.last_mut().unwrap() = 8;
        fs::write(f("big2"), &big).unwrap();
        assert!(!filecmp(&f("big1"), &f("big2")).unwrap());
        fs::write(f("empty1"), "").unwrap();
        fs::write(f("empty2"), "").unwrap();
        assert!(filecmp(&f("empty1"), &f("empty2")).unwrap());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn mtime_as_python_float() {
        let d = tmpdir("mtime");
        let f = format!("{d}/f");
        fs::write(&f, "").unwrap();
        // 右边是 CPython 3.14（macOS arm64）的 repr(os.stat(f).st_mtime)。第三个：先乘后加是 1.999999999，
        // arm64 上的 CPython（一次乘加）是 1.9999999990000001
        let fused = if cfg!(target_arch = "aarch64") { 1.9999999990000001 } else { 1.999999999 };
        for (sec, nsec, want) in [
            (1795058400, 123_456_789, 1795058400.1234567_f64),
            (100, 0, 100.0),
            (1, 999_999_999, fused),
            (1795058400, 100, 1795058400.0),
            (1795058400, 500, 1795058400.0000005),
        ] {
            stamp(&f, (sec, nsec));
            assert_eq!(getmtime(&f).unwrap(), want, "{sec}.{nsec}");
            assert_eq!(lstat_mtime(&f).unwrap(), want);
        }
        symlink("f", &format!("{d}/link")).unwrap();
        stamp(&format!("{d}/link"), (7, 0));
        assert_eq!((lstat_mtime(&format!("{d}/link")).unwrap(), getmtime(&format!("{d}/link")).unwrap()), (7.0, 1795058400.0000005));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn walk_like_os_walk() {
        let d = tmpdir("walk");
        for p in ["a/x", "a/sub/y", "b/z", "top"] {
            put(&format!("{d}/{p}"), "", 0o644, (1, 0));
        }
        fs::create_dir(format!("{d}/empty")).unwrap();
        symlink("a", &format!("{d}/dirlink")).unwrap();
        symlink("top", &format!("{d}/filelink")).unwrap();
        symlink("gone", &format!("{d}/dangling")).unwrap();
        let mut seen = Vec::new();
        walk(&d, |top, dirs, files| {
            dirs.sort();
            let mut files = files.to_vec();
            files.sort();
            seen.push(format!("{} {:?} {:?}", path::relpath(top, &d), dirs, files));
            dirs.retain(|n| n != "b"); // 去掉的不进去
            Ok(())
        })
        .unwrap();
        // 指向目录的链接算在 dirs 里，但不进去；别的链接算文件
        assert_eq!(
            seen,
            [
                r#". ["a", "b", "dirlink", "empty"] ["dangling", "filelink", "top"]"#,
                r#"a ["sub"] ["x"]"#,
                r#"a/sub [] ["y"]"#,
                r#"empty [] []"#,
            ]
        );
        // 顶上那个是链接：照样进去；不存在、不是目录：什么都不访问，也不报错
        let mut count = 0;
        walk(&format!("{d}/dirlink"), |_, _, _| {
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2);
        walk(&format!("{d}/none"), |_, _, _| panic!("不该访问")).unwrap();
        walk(&format!("{d}/top"), |_, _, _| panic!("不该访问")).unwrap();
        // visit 报的错原样传出来，后面的不再访问
        let stopped = walk(&d, |_, _, _| crate::py::die("stop"));
        assert!(matches!(stopped, Err(crate::py::Fail::Die(m)) if m == "stop"));
        fs::remove_dir_all(&d).unwrap();
    }

    /// 和 shutil.move 的对照用的那棵树（见 move_by_copy_like_python）
    fn sample_tree(src: &str) {
        put(&format!("{src}/a.md"), "a", 0o640, (100, 0));
        put(&format!("{src}/sub/b.md"), "b", 0o644, (200, 123_456_789));
        fs::set_permissions(format!("{src}/sub"), fs::Permissions::from_mode(0o750)).unwrap();
        fs::create_dir(format!("{src}/empty")).unwrap();
        fs::set_permissions(format!("{src}/empty"), fs::Permissions::from_mode(0o555)).unwrap();
        symlink("a.md", &format!("{src}/link")).unwrap();
        symlink("/nonexistent", &format!("{src}/sub/dangling")).unwrap();
        symlink("sub", &format!("{src}/dirlink")).unwrap();
        #[cfg(target_os = "macos")]
        hide(&format!("{src}/a.md"));
        stamp(&format!("{src}/link"), (222, 456));
        stamp(&format!("{src}/sub/dangling"), (333, 0));
        stamp(&format!("{src}/dirlink"), (444, 0));
        for p in ["sub", "empty", ""] {
            stamp(&format!("{src}/{p}"), (300, 0));
        }
    }

    #[test]
    fn move_by_copy_like_python() {
        // 改名不成（跨文件系统）时 shutil.move 的那一半。期望值是真的 Python 在同一棵树上的结果
        // （把 os.rename 换成总是报 EXDEV 再调 shutil.move）：文件带着权限、修改时间、标志；链接照样是链接，
        // 链接自己的修改时间也跟着；目录的权限、修改时间在里面的东西拷完以后才定
        let d = tmpdir("move-copy");
        let (src, dst) = (format!("{d}/src"), format!("{d}/dst"));
        sample_tree(&src);
        let before = tree(&src);
        move_by_copy(&src, &dst, &dst).unwrap();
        assert!(!path::lexists(&src));
        assert_eq!(tree(&dst), before);
        let hidden = if cfg!(target_os = "macos") { "0x8000" } else { "0x0" };
        assert_eq!(
            tree(&dst),
            [
                format!("a.md: file \"a\" mode=640 mtime=100000000000 flags={hidden}"),
                "dirlink: link -> sub mode=755 mtime=444000000000 flags=0x0".to_string(),
                "empty: dir mode=555 mtime=300000000000 flags=0x0".to_string(),
                "link: link -> a.md mode=755 mtime=222000000456 flags=0x0".to_string(),
                "sub/b.md: file \"b\" mode=644 mtime=200123456789 flags=0x0".to_string(),
                "sub/dangling: link -> /nonexistent mode=755 mtime=333000000000 flags=0x0".to_string(),
                "sub: dir mode=750 mtime=300000000000 flags=0x0".to_string(),
            ]
        );
        assert_eq!(look(&dst), "dir mode=755 mtime=300000000000 flags=0x0");
        // 文件、链接
        put(&format!("{d}/f.txt"), "file", 0o600, (100, 0));
        move_by_copy(&format!("{d}/f.txt"), &format!("{d}/f2.txt"), &format!("{d}/f2.txt")).unwrap();
        assert!(!path::lexists(&format!("{d}/f.txt")));
        assert_eq!(look(&format!("{d}/f2.txt")), "file \"file\" mode=600 mtime=100000000000 flags=0x0");
        symlink("f2.txt", &format!("{d}/l1")).unwrap();
        move_by_copy(&format!("{d}/l1"), &format!("{d}/l2"), &format!("{d}/l2")).unwrap();
        assert!(!path::lexists(&format!("{d}/l1")));
        assert_eq!(readlink(&format!("{d}/l2")).unwrap(), "f2.txt");
        // 搬进自己里面、目的地已经有：报错，原来的不动
        fs::create_dir_all(format!("{d}/par/child")).unwrap();
        let e = move_by_copy(&format!("{d}/par"), &format!("{d}/par/child/x"), &format!("{d}/par/child/x")).unwrap_err();
        assert_eq!(e.to_string(), format!("Cannot move a directory '{d}/par' into itself '{d}/par/child/x'."));
        assert!(move_by_copy(&dst, &format!("{d}/par"), &format!("{d}/par")).is_err());
        assert!(path::isdir(&dst) && path::isdir(&format!("{d}/par/child")));
        rmtree_reset(&d).unwrap();
    }

    #[test]
    fn move_renames_when_it_can() {
        let d = tmpdir("move");
        put(&format!("{d}/a/x.txt"), "x", 0o600, (100, 0));
        stamp(&format!("{d}/a"), (150, 0));
        // 改名：整个搬过去，什么都不变
        assert_eq!(shutil_move(&format!("{d}/a"), &format!("{d}/b")).unwrap(), format!("{d}/b"));
        assert_eq!(tree(&format!("{d}/b")), ["x.txt: file \"x\" mode=600 mtime=100000000000 flags=0x0"]);
        assert_eq!(lstat_mtime(&format!("{d}/b")).unwrap(), 150.0);
        // 目的地是目录（或指向目录的链接）：搬进去，名字照原来的（原来的写法带不带结尾的斜杠都一样）
        fs::create_dir(format!("{d}/into")).unwrap();
        symlink("into", &format!("{d}/into-link")).unwrap();
        assert_eq!(shutil_move(&format!("{d}/b/"), &format!("{d}/into")).unwrap(), format!("{d}/into/b"));
        fs::write(format!("{d}/g.txt"), "g").unwrap();
        assert_eq!(shutil_move(&format!("{d}/g.txt"), &format!("{d}/into-link")).unwrap(), format!("{d}/into-link/g.txt"));
        assert!(path::isfile(&format!("{d}/into/g.txt")));
        // 里面已经有同名的：报错，两边都不动
        fs::write(format!("{d}/g.txt"), "g2").unwrap();
        let e = shutil_move(&format!("{d}/g.txt"), &format!("{d}/into")).unwrap_err();
        assert_eq!(e.to_string(), format!("Destination path '{d}/into/g.txt' already exists"));
        assert_eq!((fs::read_to_string(format!("{d}/g.txt")).unwrap().as_str(), fs::read_to_string(format!("{d}/into/g.txt")).unwrap().as_str()), ("g2", "g"));
        // 目的地是已有的文件：改名盖掉；链接搬的是链接本身；没有的搬不了
        fs::write(format!("{d}/old.txt"), "old").unwrap();
        shutil_move(&format!("{d}/g.txt"), &format!("{d}/old.txt")).unwrap();
        assert_eq!(fs::read_to_string(format!("{d}/old.txt")).unwrap(), "g2");
        symlink("old.txt", &format!("{d}/l1")).unwrap();
        shutil_move(&format!("{d}/l1"), &format!("{d}/l2")).unwrap();
        assert_eq!(readlink(&format!("{d}/l2")).unwrap(), "old.txt");
        assert!(shutil_move(&format!("{d}/none"), &format!("{d}/x")).is_err());
        assert!(!path::lexists(&format!("{d}/x")));
        fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn move_of_readonly_directories_on_macos() {
        // macOS 上没有写权限的目录改名搬不到别的目录下（EACCES）：空的改成拷过去再删；有东西的 shutil.move 直接拒绝
        let d = tmpdir("move-ro");
        fs::create_dir_all(format!("{d}/from/empty")).unwrap();
        fs::create_dir_all(format!("{d}/to")).unwrap();
        fs::set_permissions(format!("{d}/from/empty"), fs::Permissions::from_mode(0o555)).unwrap();
        stamp(&format!("{d}/from/empty"), (100, 0));
        assert!(fs::rename(format!("{d}/from/empty"), format!("{d}/to/probe")).is_err());
        shutil_move(&format!("{d}/from/empty"), &format!("{d}/to/empty")).unwrap();
        assert!(!path::lexists(&format!("{d}/from/empty")));
        assert_eq!(look(&format!("{d}/to/empty")), "dir mode=555 mtime=100000000000 flags=0x0");
        put(&format!("{d}/from/full/x"), "x", 0o644, (1, 0));
        fs::set_permissions(format!("{d}/from/full"), fs::Permissions::from_mode(0o555)).unwrap();
        let e = shutil_move(&format!("{d}/from/full"), &format!("{d}/to/full")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(e.to_string(), format!("Cannot move the non-empty directory '{d}/from/full': Lacking write permission to '{d}/from/full'."));
        assert!(path::isfile(&format!("{d}/from/full/x")) && !path::lexists(&format!("{d}/to/full")));
        rmtree_reset(&d).unwrap();
    }

    #[test]
    fn copytree_copies_what_it_can_then_reports() {
        let d = tmpdir("copytree");
        let (src, dst) = (format!("{d}/src"), format!("{d}/dst"));
        sample_tree(&src);
        copytree(&src, &dst).unwrap();
        assert_eq!(tree(&dst), tree(&src));
        assert_eq!(look(&dst), look(&src));
        // 已经有：报错，不动它
        assert!(copytree(&src, &dst).is_err());
        // 上面几层没有的建出来；顶上是指向目录的链接：拷的是它指向的目录
        symlink("src", &format!("{d}/src-link")).unwrap();
        copytree(&format!("{d}/src-link"), &format!("{d}/deep/er/copy")).unwrap();
        assert_eq!(tree(&format!("{d}/deep/er/copy")), tree(&src));
        // 有一项拷不了（读不了的文件）：别的照拷，目录的权限、时间照定，最后报错
        put(&format!("{src}/sub/secret"), "s", 0o000, (1, 0));
        stamp(&format!("{src}/sub"), (300, 0));
        let e = copytree(&src, &format!("{d}/partial")).unwrap_err();
        assert!(e.to_string().contains("sub/secret"), "{e}");
        fs::set_permissions(format!("{src}/sub/secret"), fs::Permissions::from_mode(0o600)).unwrap();
        let mut want = tree(&src);
        want.retain(|l| !l.starts_with("sub/secret:")); // 读不了的那个没拷（源文件打不开就不建目的文件），别的一样不少
        assert_eq!(tree(&format!("{d}/partial")), want);
        assert!(copytree(&format!("{d}/none"), &format!("{d}/x")).is_err() && !path::lexists(&format!("{d}/x")));
        rmtree_reset(&d).unwrap();
    }

    #[test]
    fn rmtree_removes_trees_but_not_links() {
        let d = tmpdir("rmtree");
        sample_tree(&format!("{d}/t"));
        put(&format!("{d}/outside/keep"), "k", 0o644, (1, 0));
        symlink(format!("{d}/outside"), &format!("{d}/t/sub/out")).unwrap();
        symlink("t", &format!("{d}/t-link")).unwrap();
        // 链接不删（报错）；里面指向外面的链接只删链接本身
        assert_eq!(rmtree(&format!("{d}/t-link")).unwrap_err().to_string(), "Cannot call rmtree on a symbolic link");
        rmtree(&format!("{d}/t")).unwrap();
        assert!(!path::lexists(&format!("{d}/t")) && path::isfile(&format!("{d}/outside/keep")));
        assert!(rmtree(&format!("{d}/t")).is_err() && rmtree(&format!("{d}/outside/keep")).is_err());
        assert!(path::isfile(&format!("{d}/outside/keep")));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn temporary_directory_is_private_and_goes_away() {
        let d = tmpdir("tempdir");
        let t = TemporaryDirectory::new(".cca-unshare-", &d).unwrap();
        let name = t.name.clone();
        let tail = name.strip_prefix(&format!("{d}/.cca-unshare-")).unwrap();
        assert!(tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "{tail}");
        assert_eq!(fs::metadata(&name).unwrap().mode() & 0o777, 0o700);
        // 里面有只读的目录也收得掉
        put(&format!("{name}/copy/sub/f"), "f", 0o400, (1, 0));
        fs::set_permissions(format!("{name}/copy/sub"), fs::Permissions::from_mode(0o555)).unwrap();
        symlink(&d, &format!("{name}/link")).unwrap();
        t.cleanup().unwrap();
        assert!(!path::lexists(&name) && path::isdir(&d));
        // 没调 cleanup 就丢开（半路出错）：照样删掉
        let name = {
            let t = TemporaryDirectory::new("x-", &d).unwrap();
            fs::write(format!("{}/f", t.name), "").unwrap();
            t.name.clone()
        };
        assert!(!path::lexists(&name));
        assert!(TemporaryDirectory::new("x-", &format!("{d}/none")).is_err());
        fs::remove_dir_all(&d).unwrap();
    }
}
