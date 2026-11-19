//! 程序坞整理（`dock_prefs` 到 `cmd_dock_tidy`）：dock-tidy。
//!
//! 这一块的子命令都移植了。程序坞的设置平时经 CoreFoundation 读写（cfprefsd 里的 com.apple.dock，只在 macOS 上有）；
//! 测试时 `CCA_DOCK_PREFS` 指向沙箱里的文件，直接读写它。

use super::{arg, arity, Cmd};
use crate::core::base::CFG;
use crate::py::fs::{self, load_quiet};
use crate::py::obj::V;
use crate::py::path;
use crate::py::plist::{self, P};
use crate::py::proc::{getenv, Cmd as Proc, In, Out};
use crate::py::text;
use crate::py::time::{monotonic, sleep, time};
use crate::py::url::{unquote, urlsplit};
use crate::py::{Fail, R};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("dock-tidy", cmd_dock_tidy)];

pub const DOCK_DOMAIN: &str = "com.apple.dock";
/// 最近一次退出以后至少等这么久：程序坞在 App 退出一两秒后才把它记进「最近使用的 App」（实测 1.5～2.6 秒）
const DOCK_QUIET: f64 = 3.0;
/// 重启以后过这么久再看一眼：程序坞起来、把在跑的实例加回去，再把设置写下来，要几秒
const DOCK_LOOK: f64 = 10.0;
/// 一起整理的 App 里有的记录一直不清时，别的最多陪它等这么久，然后先整理别的
const DOCK_GRACE: f64 = 60.0;
/// 记录一直不清的 App 最多等这么久（越等查得越慢：dock_poll），过了就不管它了
const DOCK_WAIT_MAX: f64 = 3600.0;

/// 经 CoreFoundation 读写 cfprefsd 里的设置（原来用 ctypes 调的同一组函数）。
/// 用到的时候才载入这个库（dlopen，和原来的 ctypes 一样）：写成 `#[link]` 的话，每一条子命令起来都要先载入它和它带着的一串库
/// ——实测多 2600 万条指令、5 MB 内存，而用得着它的只有整理程序坞这一条
#[cfg(target_os = "macos")]
mod cf {
    use crate::py::plist::{self, P};
    use std::ffi::{c_char, c_long, c_ulong, c_void, CStr, CString};
    use std::sync::OnceLock;

    type Ref = *const c_void;

    /// 用到的那几个函数
    struct Lib {
        string_create_with_cstring: unsafe extern "C" fn(Ref, *const c_char, u32) -> Ref,
        preferences_copy_app_value: unsafe extern "C" fn(Ref, Ref) -> Ref,
        preferences_set_app_value: unsafe extern "C" fn(Ref, Ref, Ref),
        preferences_app_synchronize: unsafe extern "C" fn(Ref) -> u8,
        property_list_create_data: unsafe extern "C" fn(Ref, Ref, c_long, c_ulong, *mut Ref) -> Ref,
        property_list_create_with_data: unsafe extern "C" fn(Ref, Ref, c_ulong, *mut c_long, *mut Ref) -> Ref,
        data_create: unsafe extern "C" fn(Ref, *const u8, c_long) -> Ref,
        data_get_length: unsafe extern "C" fn(Ref) -> c_long,
        data_get_byte_ptr: unsafe extern "C" fn(Ref) -> *const u8,
        release: unsafe extern "C" fn(Ref),
    }

    /// 库里的一个函数；T 是它的函数指针类型
    ///
    /// # Safety
    /// handle 是 dlopen 给的；T 必须是和这个符号的 C 声明一致的 `extern "C" fn` 指针类型
    unsafe fn symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Option<T> {
        assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        // SAFETY: handle 有效，name 以 0 结尾
        let p = unsafe { libc::dlsym(handle, name.as_ptr()) };
        // SAFETY: T 和指针一样大（上面核对过），调用方保证它就是这个符号的类型
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }

    /// CoreFoundation：第一次用到时载入，载不了是 None（原来是 ctypes 的 OSError）
    fn lib() -> Option<&'static Lib> {
        static LIB: OnceLock<Option<Lib>> = OnceLock::new();
        LIB.get_or_init(|| {
            // SAFETY: 路径以 0 结尾；库载入以后一直留着（不 dlclose），里面的函数指针一直有效
            let h = unsafe { libc::dlopen(c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation".as_ptr(), libc::RTLD_LAZY) };
            if h.is_null() {
                return None;
            }
            // SAFETY: 各个字段的类型就是这些函数在 CoreFoundation 头文件里的声明
            unsafe {
                Some(Lib {
                    string_create_with_cstring: symbol(h, c"CFStringCreateWithCString")?,
                    preferences_copy_app_value: symbol(h, c"CFPreferencesCopyAppValue")?,
                    preferences_set_app_value: symbol(h, c"CFPreferencesSetAppValue")?,
                    preferences_app_synchronize: symbol(h, c"CFPreferencesAppSynchronize")?,
                    property_list_create_data: symbol(h, c"CFPropertyListCreateData")?,
                    property_list_create_with_data: symbol(h, c"CFPropertyListCreateWithData")?,
                    data_create: symbol(h, c"CFDataCreate")?,
                    data_get_length: symbol(h, c"CFDataGetLength")?,
                    data_get_byte_ptr: symbol(h, c"CFDataGetBytePtr")?,
                    release: symbol(h, c"CFRelease")?,
                })
            }
        })
        .as_ref()
    }

    /// 载得了 CoreFoundation
    pub fn available() -> bool {
        lib().is_some()
    }

    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
    const XML: c_long = 100; // kCFPropertyListXMLFormat_v1_0

    /// 用完自己放掉的 CF 对象
    struct Held(Ref);

    impl Drop for Held {
        fn drop(&mut self) {
            if let (false, Some(cf)) = (self.0.is_null(), lib()) {
                // SAFETY: 是我们自己拿到（Create / Copy）的对象，只放一次
                unsafe { (cf.release)(self.0) }
            }
        }
    }

    fn string(cf: &Lib, s: &str) -> Held {
        let c = CString::new(s).unwrap_or_default();
        // SAFETY: c 是以 0 结尾的 UTF-8
        Held(unsafe { (cf.string_create_with_cstring)(std::ptr::null(), c.as_ptr(), UTF8) })
    }

    /// CF 的属性列表对象 → 我们的值（经它自己写出来的 XML）
    fn to_plist(cf: &Lib, v: &Held) -> Option<P> {
        // SAFETY: v 是有效的属性列表对象；返回的 data 由 Held 放掉；字节在 data 活着的时候拷出来
        let bytes = unsafe {
            let data = Held((cf.property_list_create_data)(std::ptr::null(), v.0, XML, 0, std::ptr::null_mut()));
            if data.0.is_null() {
                return None;
            }
            std::slice::from_raw_parts((cf.data_get_byte_ptr)(data.0), (cf.data_get_length)(data.0) as usize).to_vec()
        };
        plist::loads(&bytes).ok()
    }

    /// 我们的值 → CF 的属性列表对象（经 XML）
    fn from_plist(cf: &Lib, value: &P) -> crate::py::R<Option<Held>> {
        let raw = plist::dumps_xml(value)?;
        // SAFETY: raw 在调用期间活着；data 用完就放
        let v = unsafe {
            let data = Held((cf.data_create)(std::ptr::null(), raw.as_ptr(), raw.len() as c_long));
            if data.0.is_null() {
                return Ok(None);
            }
            Held((cf.property_list_create_with_data)(std::ptr::null(), data.0, 0, std::ptr::null_mut(), std::ptr::null_mut()))
        };
        Ok(if v.0.is_null() { None } else { Some(v) })
    }

    pub fn get(domain: &str, key: &str) -> Option<P> {
        let cf = lib()?;
        let (d, k) = (string(cf, domain), string(cf, key));
        // SAFETY: d、k 是有效的 CFString；Copy 出来的对象由 Held 放掉
        let v = unsafe {
            (cf.preferences_app_synchronize)(d.0); // 程序坞刚写的也要读到（不用这个进程里缓存的）
            Held((cf.preferences_copy_app_value)(k.0, d.0))
        };
        if v.0.is_null() {
            return None;
        }
        to_plist(cf, &v)
    }

    pub fn put(domain: &str, key: &str, value: &P) -> crate::py::R<bool> {
        let Some(cf) = lib() else { return Ok(false) };
        let Some(v) = from_plist(cf, value)? else { return Ok(false) };
        let (d, k) = (string(cf, domain), string(cf, key));
        // SAFETY: 三个都是有效的 CF 对象
        Ok(unsafe {
            (cf.preferences_set_app_value)(k.0, v.0, d.0);
            (cf.preferences_app_synchronize)(d.0) != 0
        })
    }

    #[cfg(test)]
    pub fn round_trip(value: &P) -> Option<P> {
        let cf = lib()?;
        to_plist(cf, &from_plist(cf, value).ok()??)
    }
}

/// 程序坞的设置在哪：文件（测试的沙箱），还是 cfprefsd 里的那个域
enum Prefs {
    File(String),
    #[cfg(target_os = "macos")]
    Domain(String),
}

impl Prefs {
    /// `dock_prefs(domain)`；用不了（不是 macOS，原来是载不了 CoreFoundation 的 OSError）是 None
    fn open(domain: &str) -> Option<Prefs> {
        if domain.starts_with('/') {
            return Some(Prefs::File(domain.to_string()));
        }
        #[cfg(target_os = "macos")]
        return cf::available().then(|| Prefs::Domain(domain.to_string()));
        #[cfg(not(target_os = "macos"))]
        None
    }

    /// 文件里的整份设置：读不了、不是属性列表、不是字典都当空的（XML 写坏了是 ExpatError，原来就没人接）
    fn load(file: &str) -> R<P> {
        let d = match std::fs::read(file).map_err(Fail::from).and_then(|raw| plist::loads(&raw)) {
            Ok(d) => d,
            Err(e) if e.is_os_or_value() => P::Dict(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(if d.is_dict() { d } else { P::Dict(Vec::new()) })
    }

    fn get(&self, key: &str) -> R<Option<P>> {
        match self {
            Prefs::File(file) => Ok(Prefs::load(file)?.get(key).cloned()),
            #[cfg(target_os = "macos")]
            Prefs::Domain(domain) => Ok(cf::get(domain, key)),
        }
    }

    fn put(&self, key: &str, value: P) -> R<bool> {
        match self {
            Prefs::File(file) => {
                let mut d = Prefs::load(file)?;
                d.set(key, value);
                let raw = plist::dumps_binary(&d)?;
                let tmp = format!("{}.cca-{}.tmp", file, std::process::id());
                let written = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(&tmp).and_then(|mut f| f.write_all(&raw));
                Ok(written.and_then(|()| std::fs::rename(&tmp, file)).is_ok())
            }
            #[cfg(target_os = "macos")]
            Prefs::Domain(domain) => cf::put(domain, key, &value),
        }
    }

    /// 设置里的一个列表（不是列表当空的）
    fn list(&self, key: &str) -> R<Vec<P>> {
        Ok(match self.get(key)? {
            Some(P::Array(items)) => items,
            _ => Vec::new(),
        })
    }
}

/// 程序坞的一个图标指着的 App 的位置（真实路径）；不是文件的图标是 None
fn dock_tile_path(tile: &P) -> R<Option<String>> {
    let Some(u) = tile.get("tile-data").and_then(|d| d.get("file-data")).and_then(|f| f.get("_CFURLString")).and_then(|u| u.as_str()) else {
        return Ok(None);
    };
    if !u.starts_with("file://") {
        return Ok(None);
    }
    let p = unquote(&urlsplit(u).map_err(Fail::Value)?.path);
    let p = p.trim_end_matches('/');
    Ok(Some(path::realpath(if p.is_empty() { "/" } else { p })))
}

/// 程序坞的一个图标是不是这个 App：按 bundle id 认，图标里没有 bundle id 时按位置认
fn dock_tile_is(tile: &P, bid: Option<&str>, app: Option<&str>) -> R<bool> {
    let Some(d) = tile.get("tile-data").filter(|d| d.is_dict()) else { return Ok(false) };
    if bid.is_some_and(|b| !b.is_empty() && d.get("bundle-identifier").and_then(|x| x.as_str()) == Some(b)) {
        return Ok(true);
    }
    match app.filter(|p| !p.is_empty()) {
        Some(p) => Ok(dock_tile_path(tile)?.as_deref() == Some(p)),
        None => Ok(false),
    }
}

/// App 的 (bundle id, 主程序的路径)：从 Info.plist 读，读不到的是 None
fn dock_app(app: &str) -> R<(Option<String>, Option<String>)> {
    let info = match std::fs::read(path::join_all(&[app, "Contents", "Info.plist"])).map_err(Fail::from).and_then(|raw| plist::loads(&raw)) {
        Ok(info) => info,
        Err(e) if e.is_os_or_value() => P::Null,
        Err(e) => return Err(e),
    };
    let text = |k: &str| info.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
    Ok((text("CFBundleIdentifier"), text("CFBundleExecutable").map(|exe| path::join_all(&[app, "Contents", "MacOS", &exe]))))
}

/// 在跑的各个进程的主程序（`ps -axo comm=`）；ps 用不了是 None
fn running_programs() -> Option<Vec<String>> {
    let done = Proc::new(["ps", "-axo", "comm="]).stdin(In::Null).capture().timeout(10.0).run().ok()?;
    let out = done.out().replace("\r\n", "\n").replace('\r', "\n");
    Some(text::splitlines(&out).into_iter().map(|line| text::strip(line).to_string()).collect())
}

/// 重启程序坞前的「最近使用的 App」：apps（App 的路径）退出后留下的图标去掉；apps 都没有多出来的图标、看不清（ps 失败）时是 None。
/// 在跑的实例各占一个图标（主程序是 Info.plist 写的那个的进程；Chrome、Electron 的辅助进程是别的程序，不算），固定在程序坞上的
/// App 有一个实例用固定的那个图标；比这多出来的就是退出后留下的。要重启时开着的 App、LaunchServices 还留着记录的 App（exited，
/// dock_exited 给的）的图标也都拿掉：新的程序坞给每个在跑的实例、每条这样的记录重新加一个，不认原来的（同一个 App 多开的实测会多出一个）
fn dock_tidied(prefs: &Prefs, apps: &[String], exited: &[(Option<String>, Option<String>)]) -> R<Option<Vec<P>>> {
    let Some(procs) = running_programs() else { return Ok(None) };
    let (recent, pinned) = (prefs.list("recent-apps")?, prefs.list("persistent-apps")?);
    let mut known: HashMap<String, usize> = HashMap::new();
    let mut running = |app: &str| -> R<usize> {
        if let Some(n) = known.get(app) {
            return Ok(*n);
        }
        let n = match dock_app(app)?.1 {
            Some(exe) => procs.iter().filter(|p| **p == exe).count(),
            None => 0,
        };
        known.insert(app.to_string(), n);
        Ok(n)
    };
    let mut mine: Vec<(Option<String>, Option<String>)> = Vec::new();
    for p in apps {
        mine.push((dock_app(p)?.0, Some(p.clone())));
    }
    let mut extra = false;
    for (b, p) in &mine {
        let (b, p) = (b.as_deref(), p.as_deref());
        let mut shown = 0;
        for t in &recent {
            if dock_tile_is(t, b, p)? {
                shown += 1;
            }
        }
        let mut is_pinned = false;
        for t in &pinned {
            if dock_tile_is(t, b, p)? {
                is_pinned = true;
                break;
            }
        }
        let live = running(p.unwrap_or(""))?;
        if shown > live.saturating_sub(is_pinned as usize) {
            extra = true;
            break;
        }
    }
    if !extra {
        return Ok(None);
    }
    let gone: Vec<&(Option<String>, Option<String>)> = mine.iter().chain(exited).collect();
    let mut kept = Vec::new();
    for t in recent {
        let mut drop_it = false;
        for (b, p) in &gone {
            if dock_tile_is(&t, b.as_deref(), p.as_deref())? {
                drop_it = true;
                break;
            }
        }
        if !drop_it {
            if let Some(at) = dock_tile_path(&t)?.filter(|at| !at.is_empty()) {
                drop_it = running(&at)? > 0;
            }
        }
        if !drop_it {
            kept.push(t);
        }
    }
    Ok(Some(kept))
}

/// `re.search(r'名字=\x22([^\x22\n]+)\x22', entry)`：entry 里第一处 `名字="…"`（引号里不空、不带换行）
fn quoted_after<'a>(entry: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = entry;
    while let Some(i) = rest.find(name) {
        let after = &rest[i + name.len()..];
        if let Some(body) = after.strip_prefix('"') {
            let end = body.find(['"', '\n']).unwrap_or(body.len());
            if end > 0 && body[end..].starts_with('"') {
                return Some(&body[..end]);
            }
        }
        rest = &rest[i + 1..];
    }
    None
}

/// `re.split(r'\n(?=\s*\d+\) )', out)`：在“换行后面（隔着空白）是 `数字) `”的那些换行处断开
fn lsappinfo_entries(out: &str) -> Vec<&str> {
    let starts_entry = |rest: &str| {
        let rest = text::lstrip(rest);
        let digits = rest.chars().take_while(|c| crate::py::uni::is_decimal(*c)).map(char::len_utf8).sum::<usize>();
        digits > 0 && rest[digits..].starts_with(") ")
    };
    let mut entries = Vec::new();
    let mut from = 0;
    for (i, _) in out.match_indices('\n') {
        if starts_entry(&out[i + 1..]) {
            entries.push(&out[from..i]);
            from = i + 1;
        }
    }
    entries.push(&out[from..]);
    entries
}

/// LaunchServices 里退出了、还留着记录的 App：[(bundle id, 真实路径)]（exited-with-subordinates：它起的进程、交给 launchd 的任务还在；
/// 同一个 App 开着的实例是正常的记录，不算）。lsappinfo 用不了时是 []
fn dock_exited() -> Vec<(Option<String>, Option<String>)> {
    let Ok(done) = Proc::new(["lsappinfo", "list"]).stdin(In::Null).capture().timeout(10.0).run() else { return Vec::new() };
    let out = done.out();
    let mut found = Vec::new();
    for entry in lsappinfo_entries(&out) {
        if !entry.contains("(exited") {
            continue;
        }
        let (bid, app) = (quoted_after(entry, "bundleID="), quoted_after(entry, "bundle path="));
        if bid.is_some() || app.is_some() {
            found.push((bid.map(|b| b.to_string()), app.map(path::realpath)));
        }
    }
    found
}

/// 记录不清时多久再查一次（lsappinfo 一次三四十毫秒）：刚开始 2 秒，等了一分钟以后 10 秒，十分钟以后 30 秒
fn dock_poll(waited: f64) -> f64 {
    if waited < 60.0 {
        2.0
    } else if waited < 600.0 {
        10.0
    } else {
        30.0
    }
}

/// HOME 换过、不是这个用户真正的主目录（测试的沙箱；认不出也算）：只给测试的开关只在这时才认，测试也碰不到用户真正的设置
pub fn home_swapped() -> bool {
    match path::home_from_passwd() {
        Some(real) => path::realpath(&path::expanduser("~")) != path::realpath(&real),
        None => true,
    }
}

/// 整理时的状态：等着整理的 App、各自什么时候记下的、重启过几次
struct Tidy {
    prefs: Prefs,
    pending: Vec<String>,
    added: HashMap<String, f64>,
    tries: HashMap<String, u32>,
    /// lockdir/exited：一行一个刚退出的 App；它的修改时间就是最近一次退出
    note: Option<String>,
    quiet: f64,
    look: f64,
    grace: f64,
}

impl Tidy {
    /// 记在 exited 里的 App 一起整理：先改名再读，改名以后才退出的记进新的文件
    fn take(&mut self) {
        let Some(note) = &self.note else { return };
        let mine = format!("{}.{}", note, std::process::id());
        if std::fs::rename(note, &mine).is_err() {
            return;
        }
        if let Ok(raw) = std::fs::read(&mine) {
            let content = String::from_utf8_lossy(&raw).replace("\r\n", "\n").replace('\r', "\n");
            for p in content.split('\n') {
                // 新退出的（以前整理过、放弃过的也重新算）
                if !p.is_empty() && !self.pending.iter().any(|x| x == p) {
                    self.pending.push(p.to_string());
                    self.added.insert(p.to_string(), monotonic());
                    self.tries.remove(p);
                }
            }
        }
        let _ = std::fs::remove_file(&mine);
    }

    /// 拿着锁整理 pending 里的，直到都整理完、放弃了
    fn run(&mut self) -> R<()> {
        // since：最近一次重启（开始时是退出）；held：没记录的 App 从什么时候起在陪着等
        let (mut since, mut held): (f64, Option<f64>) = (time(), None);
        loop {
            let last = self.note.as_ref().and_then(|n| std::fs::metadata(n).ok()).map_or(0.0, |m| m.mtime() as f64 + m.mtime_nsec() as f64 / 1e9);
            let left = since.max(last) + self.quiet - time();
            if left > 0.0 {
                sleep(left.min(5.0));
                continue;
            }
            self.take();
            let now = monotonic();
            let added = &self.added;
            self.pending.retain(|p| now - added[p] < DOCK_WAIT_MAX); // 等太久的不管了
            if self.pending.is_empty() {
                return Ok(());
            }
            let exited = dock_exited();
            let mut stuck = Vec::new();
            for p in &self.pending {
                let bid = dock_app(p)?.0;
                if exited.iter().any(|(b, r)| b.as_ref().is_some_and(|b| !b.is_empty() && Some(b) == bid.as_ref()) || r.as_deref() == Some(p.as_str())) {
                    stuck.push(p.clone());
                }
            }
            let ready: Vec<String> = self.pending.iter().filter(|p| !stuck.contains(p)).cloned().collect();
            if !stuck.is_empty() {
                let since_held = *held.get_or_insert(now);
                if ready.is_empty() || now - since_held < self.grace {
                    let oldest = stuck.iter().map(|p| self.added[p]).fold(f64::INFINITY, f64::min);
                    sleep(dock_poll(now - oldest));
                    continue;
                }
            }
            let Some(new) = dock_tidied(&self.prefs, &ready, &exited)? else {
                // 都没有多出来的图标了（或者看不清）
                self.pending = stuck;
                continue;
            };
            if !self.prefs.put("recent-apps", P::Array(new))? {
                return Ok(());
            }
            let _ = Proc::new(["killall", "Dock"]).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).timeout(10.0).run();
            sleep(self.look); // 新的程序坞起来、把在跑的实例加回去以后再看
            for p in &ready {
                *self.tries.entry(p.clone()).or_insert(0) += 1;
            }
            // 又被加回来的（重启时记录还在）再等一轮、再重启一次，最多两次
            let mut again = stuck;
            for p in &ready {
                if self.tries[p] < 2 && dock_tidied(&self.prefs, std::slice::from_ref(p), &[])?.is_some() {
                    again.push(p.clone());
                }
            }
            self.pending = again;
            since = time();
            held = None;
        }
    }
}

/// cca _dock-tidy App：去掉程序坞「最近使用的 App」里这个 App 退出后留下的图标（bash 的 dock_watch 等它退出以后调）。
/// 先在 lockdir/exited 里记一笔（这个文件的修改时间就是最近一次退出）；锁（tidy.lock）有人拿着就走，拿着锁的那个会把记着的一起整理。
/// 拿到锁的等最近一次退出满 DOCK_QUIET 秒、记着的 App 在 LaunchServices 里的记录没了，把它们一起整理、只重启一次程序坞；
/// 有的记录一直不清时，别的最多陪它等 DOCK_GRACE 秒就先整理，它自己最多等 DOCK_WAIT_MAX 秒。放锁以后再看一眼有没有新记的，
/// 有就再拿锁接着整理（刚才没拿到锁就走了的那个记的不会没人管）。后台跑的：什么都不说，出错就算了
pub fn cmd_dock_tidy(args: &[String]) -> R<()> {
    arity("cmd_dock_tidy", args, 0, 2)?;
    let (app, lockdir) = (arg(args, 0, ""), arg(args, 1, ""));
    if app.is_empty() {
        return Ok(());
    }
    if load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("dock_tidy"), Some(V::Bool(false)))) {
        return Ok(());
    }
    // 测试里是沙箱里的文件
    let domain = getenv("CCA_DOCK_PREFS").filter(|d| !d.is_empty()).unwrap_or_else(|| DOCK_DOMAIN.to_string());
    if domain == DOCK_DOMAIN && home_swapped() {
        return Ok(()); // 测试的沙箱：程序坞的设置还是这个用户真正的那一份，不碰
    }
    let Some(prefs) = Prefs::open(&domain) else { return Ok(()) };
    if matches!(prefs.get("show-recents")?, Some(P::Bool(false))) {
        return Ok(()); // 不显示最近使用的 App：退出了图标就没了
    }
    // 测试：等待都换成这么久
    let settle = getenv("CCA_DOCK_SETTLE").and_then(|s| text::to_float(&s)).map(|s| if s > 0.0 { s } else { 0.0 });
    let (quiet, look, grace) = match settle {
        Some(s) => (s, s, s),
        None => (DOCK_QUIET, DOCK_LOOK, DOCK_GRACE),
    };
    let first = path::realpath(app);
    let mut tidy = Tidy { prefs, pending: vec![first.clone()], added: HashMap::from([(first.clone(), monotonic())]), tries: HashMap::new(), note: None, quiet, look, grace };
    let mut lock: Option<std::fs::File> = None;
    if !lockdir.is_empty() {
        let exited = path::join(lockdir, "exited");
        let noted = fs::makedirs_mode(lockdir, 0o700)
            .and_then(|()| std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&exited))
            .and_then(|mut f| f.write_all(format!("{first}\n").as_bytes()));
        if noted.is_ok() {
            tidy.note = Some(exited);
            lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(lockdir, "tidy.lock")).ok();
        }
    }
    loop {
        if let Some(lock) = &lock {
            // SAFETY: lock 开着，fd 有效
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Ok(()); // 有人拿着锁：他会把记着的一起整理
            }
        }
        tidy.run()?;
        let Some(lock) = &lock else { return Ok(()) };
        // SAFETY: 同上
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        if !tidy.note.as_ref().is_some_and(|n| path::exists(n)) {
            return Ok(());
        }
        // 放锁前后有人记了一笔（他没拿到锁就走了）：再拿锁接着整理
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsappinfo_output_is_split_like_the_regex() {
        let out = " 1) \"Finder\" ASN:0x0-0x1:\n    bundleID=\"com.apple.finder\"\n    bundle path=\"/System/Library/CoreServices/Finder.app\"\n 2) \"Claude\" ASN:0x0-0x2: (exited-with-subordinates)\n    bundleID=\"com.example.claude\"\n\n  3) \"x\"\n    note 4) not an entry\n10) \"y\"";
        let entries = lsappinfo_entries(out);
        assert_eq!(entries.len(), 5, "{entries:?}");
        assert!(entries[1].starts_with(" 2) ") && entries[1].contains("com.example.claude") && !entries[1].contains("3)"));
        assert_eq!(entries[2], "");
        assert!(entries[3].contains("note 4) not an entry") && entries[4].starts_with("10) "));
        assert_eq!(quoted_after(entries[1], "bundleID="), Some("com.example.claude"));
        assert_eq!(quoted_after(entries[1], "bundle path="), None);
        assert_eq!(quoted_after("x=\"\" x=\"a\nb\" x=\"ok\"", "x="), Some("ok"));
        assert_eq!(lsappinfo_entries(""), vec![""]);
    }

    #[test]
    fn poll_intervals() {
        assert_eq!((dock_poll(0.0), dock_poll(59.9), dock_poll(60.0), dock_poll(599.0), dock_poll(600.0)), (2.0, 2.0, 10.0, 10.0, 30.0));
    }

    #[test]
    fn tiles() {
        let tile = |bid: Option<&str>, url: &str| {
            let mut data = P::dict([("file-data", P::dict([("_CFURLString", P::from(url))]))]);
            if let Some(b) = bid {
                data.set("bundle-identifier", P::from(b));
            }
            P::dict([("tile-data", data)])
        };
        let t = tile(Some("com.example.app"), "file:///Applications/Example%20App.app/");
        assert_eq!(dock_tile_path(&t).unwrap().as_deref(), Some("/Applications/Example App.app"));
        assert!(dock_tile_is(&t, Some("com.example.app"), None).unwrap());
        assert!(dock_tile_is(&t, Some("other"), Some("/Applications/Example App.app")).unwrap());
        assert!(!dock_tile_is(&t, Some("other"), Some("/Applications/Other.app")).unwrap());
        assert!(!dock_tile_is(&t, None, None).unwrap() && !dock_tile_is(&t, Some(""), Some("")).unwrap());
        assert_eq!(dock_tile_path(&tile(None, "https://example.com/x")).unwrap(), None);
        assert_eq!(dock_tile_path(&tile(None, "file:///")).unwrap().as_deref(), Some("/"));
        assert_eq!(dock_tile_path(&P::from("not a tile")).unwrap(), None);
        assert!(!dock_tile_is(&P::dict([("tile-data", P::from("x"))]), Some("a"), Some("/b")).unwrap());
    }

    /// CoreFoundation 的那几个函数声明得对不对：一个值经它转一圈回来不变（只在内存里，不碰任何设置）
    #[cfg(target_os = "macos")]
    #[test]
    fn core_foundation_round_trip() {
        let value = P::dict([
            ("recent-apps", P::Array(vec![P::dict([("tile-data", P::dict([("bundle-identifier", P::from("com.example.app")), ("book", P::Data(vec![0, 1, 2, 255]))])), ("GUID", P::Int(4242))])])),
            ("show-recents", P::from(true)),
            ("tilesize", P::Real(48.5)),
            ("名字", P::from("中文 & <标记>")),
        ]);
        let back = cf::round_trip(&value).expect("转不回来");
        // 字典的键读回来是排好序的（XML 里按键排）：每一层都排好再比
        fn sorted(p: &P) -> P {
            match p {
                P::Dict(items) => {
                    let mut items: Vec<(String, P)> = items.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
                    items.sort_by(|a, b| a.0.cmp(&b.0));
                    P::Dict(items)
                }
                P::Array(items) => P::Array(items.iter().map(sorted).collect()),
                other => other.clone(),
            }
        }
        assert_eq!(sorted(&back), sorted(&value));
    }

    /// 读得到这台机器程序坞的设置（只读）：声明、内存管理有错的话这里会崩
    #[cfg(target_os = "macos")]
    #[test]
    fn reads_a_real_preference_without_crashing() {
        for key in ["tilesize", "show-recents", "recent-apps", "no-such-key-cca-test"] {
            let _ = cf::get(DOCK_DOMAIN, key);
        }
    }
}
