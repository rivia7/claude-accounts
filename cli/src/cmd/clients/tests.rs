//! 客户端更新的单元测试。
//!
//! 对拍（cli/pydiff/cases/clients.py）够不着的路在这里：检查、下载、暂存、安装要经 `/usr/bin/curl`（真的会联网）、`codesign` /
//! `spctl`（假的 App 过不了），原来的 Python 单元测试（tests/client_updates.py）是把 `cu_run`、`cu_host`、`cu_download` 换成假的
//! 来走这些路的。这里照样：这三个函数（还有改名、删目录、睡眠）在测试里先问一声这条线程上装的钩子（`Hooks`），HOME、CCA_HOME、
//! 环境变量也换成这条线程自己的一套（`World`），整条路就能在一个进程里、各在各的临时目录里走。
//!
//! 期望值不是照着代码推的：tools/gen_clients_vectors.py 拿原来的 Python（cca 里的 PY_SRC）把同一批输入、同一批场景跑一遍，
//! 结果记在 tests/vectors/src_clients.json 里——
//! - 一个个函数：地址规则、版本号比大小、版本列表、解压、进程列表……（`check_functions`、`check_extract`）；
//! - 整条路的场景（`check_scenarios`）：每一步的输出、退出码、起了哪些命令、跑完以后的文件。tests/client_updates.py
//!   的每一项检查在生成时对 Python 的结果先核对一遍（不成立就不生成），这里逐个场景重放，要求每一步都和记下来的一样。
//!
//! 场景的步骤、假命令怎么回话，两边各有一个解释器（那边是 tools/gen_clients_scenarios.py 的 `Scn`）：改一边要改另一边。
//!
//! 这几组都在 `like_python` 一个测试里按顺序做，不是各占一个测试：`cargo test` 同时跑的测试数是有上限的（CPU 的核数），几个
//! 要跑一两秒的测试各占一个名额时，排在后面的测试要等名额——`cmd::login::sys::tests::socket_waits_like_settimeout` 就会时好时坏
//! （它的 socketpair 没设 FD_CLOEXEC，等名额等晚了的别的测试这时起的子进程把另一头带走了，对方“关了”却等不到 EOF；不带这个模块、
//! `--test-threads=8` 时它每次都坏）。那边修了以后可以拆回各自一个测试。

use super::pyv::*;
use super::*;
use crate::py::obj::{Dict, List, V};
use crate::py::proc::Done;
use crate::py::xml::test_blob;
use crate::py::{json, num, R};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

// --- 钩子：换掉起命令、下载这几样（原来的测试里是把模块里的函数换掉） -----------------------------------------------------

/// 这条线程上的假 HOME、假环境变量
#[derive(Clone, Default)]
pub struct World {
    pub home: String,
    pub cca_home: String,
    pub env: HashMap<String, String>,
}

type RunHook = Rc<dyn Fn(&[String], &str, f64, Option<&[(String, String)]>) -> R<Done>>;
type DownloadHook = Rc<dyn Fn(&str, &str, &str, bool) -> R<Option<String>>>;
type RenameHook = Rc<dyn Fn(&str, &str) -> R<()>>;
type RmtreeHook = Rc<dyn Fn(&str) -> Option<std::io::Result<()>>>;

#[derive(Default)]
struct Hooks {
    run: Option<RunHook>,
    download: Option<DownloadHook>,
    host: Option<(String, String)>,
    rename: Option<RenameHook>,
    rmtree: Option<RmtreeHook>,
    /// 不许算摘要 / 不许解压（调到就是测试失败）
    forbid_hash: bool,
    forbid_extract: bool,
    /// 睡眠不真的睡，记下睡了几秒
    slept: Option<Vec<f64>>,
}

thread_local! {
    static WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
    /// 改名的钩子正在跑（它里面做真的改名时不再问钩子）
    static RENAMING: RefCell<bool> = const { RefCell::new(false) };
}

pub fn world() -> Option<World> {
    WORLD.with(|w| w.borrow().clone())
}

/// `with Lock():` 在测试的世界里：这条线程自己的 CCA_HOME 下的 sync.lock
pub fn world_lock(w: &World) -> R<std::fs::File> {
    use std::os::fd::AsRawFd;
    crate::py::fs::makedirs(&w.cca_home)?;
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path::join(&w.cca_home, "sync.lock"))?;
    // SAFETY: file 开着，fd 有效
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

pub fn on_run(args: &[String], folder: &str, timeout: f64, env: Option<&[(String, String)]>) -> Option<R<Done>> {
    let hook = HOOKS.with(|h| h.borrow().run.clone())?;
    Some(hook(args, folder, timeout, env))
}

pub fn on_download(
    url: &str,
    destination: &str,
    folder: &str,
    package: bool,
    _expected_size: Option<&V>,
    _empty: &[&str],
) -> Option<R<Option<String>>> {
    let hook = HOOKS.with(|h| h.borrow().download.clone())?;
    Some(hook(url, destination, folder, package))
}

pub fn on_host() -> Option<(String, String)> {
    HOOKS.with(|h| h.borrow().host.clone())
}

pub fn on_rename(source: &str, destination: &str) -> Option<R<()>> {
    if RENAMING.with(|r| *r.borrow()) {
        return None;
    }
    let hook = HOOKS.with(|h| h.borrow().rename.clone())?;
    RENAMING.with(|r| *r.borrow_mut() = true);
    let out = hook(source, destination);
    RENAMING.with(|r| *r.borrow_mut() = false);
    Some(out)
}

pub fn on_rmtree(work: &str) -> Option<std::io::Result<()>> {
    let hook = HOOKS.with(|h| h.borrow().rmtree.clone())?;
    hook(work)
}

pub fn on_hash(path: &str) {
    if HOOKS.with(|h| h.borrow().forbid_hash) {
        panic!("这里不该算摘要：{path}");
    }
}

pub fn on_extract(archive: &str) {
    if HOOKS.with(|h| h.borrow().forbid_extract) {
        panic!("这里不该解压：{archive}");
    }
}

pub fn on_sleep(secs: f64) -> bool {
    HOOKS.with(|h| match h.borrow_mut().slept.as_mut() {
        Some(slept) => {
            slept.push(secs);
            true
        }
        None => false,
    })
}

/// 测试结束（哪怕是 panic）把这条线程上的钩子、假世界收掉
struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        WORLD.with(|w| *w.borrow_mut() = None);
        HOOKS.with(|h| *h.borrow_mut() = Hooks::default());
        set_cu(&CU_CODEX);
        CU_BUDGET.with(|b| b.set(None));
    }
}

// --- 对照数据 ------------------------------------------------------------------------------------------------------------

fn vectors() -> Dict {
    json::loads(include_str!("../../../tests/vectors/src_clients.json")).expect("对照数据是 JSON").as_dict().expect("是个对象")
}

fn stdlib_vectors() -> Dict {
    json::loads(include_str!("../../../tests/vectors/clients.json")).expect("对照数据是 JSON").as_dict().expect("是个对象")
}

fn client(id: &str) -> &'static Client {
    CU_CLIENTS.iter().copied().find(|c| c.id == id).unwrap_or_else(|| panic!("没有这种客户端：{id}"))
}

/// 一次调用的结果写成对照数据里的样子：`{"ok": …}`、`{"cu": [status, message]}`、`{"err": 异常类}`
fn outcome(r: R<V>) -> Dict {
    match r {
        Ok(v) => dict! { "ok" => v },
        Err(e) => match cu_parts(&e) {
            Some((status, message)) => dict! { "cu" => crate::list![status, message] },
            None => dict! { "err" => class_of(&e) },
        },
    }
}

/// 出的错是 Python 的哪一类异常（`type(e).__name__`）
fn class_of(e: &Fail) -> String {
    match e {
        Fail::Os(_) => "OSError".into(),
        Fail::Value(_) => "ValueError".into(),
        Fail::Other("zlib.error", _) => "error".into(), // 那个类在 zlib 模块里就叫 error
        Fail::Other(class, _) => class.to_string(),
        Fail::Die(_) | Fail::Exit(_) => "SystemExit".into(),
    }
}

/// 两边的结果一样吗：`ok` 的值照 JSON 写出来逐字比（连键的先后）；出错的比类别，OSError 的各个子类算一类
fn same_outcome(got: &Dict, want: &Dict) -> bool {
    let os = ["OSError", "FileNotFoundError", "FileExistsError", "NotADirectoryError", "IsADirectoryError", "PermissionError"];
    match (got.get("err"), want.get("err")) {
        (Some(g), Some(w)) => {
            let (g, w) = (g.py_str(), w.py_str());
            g == w
                || (os.contains(&g.as_str()) && os.contains(&w.as_str()))
                || (g == "ValueError" && matches!(w.as_str(), "UnicodeDecodeError" | "JSONDecodeError"))
        }
        (None, None) => {
            let show = |d: &Dict| json::dumps(&V::Dict(dict! { "ok" => d.get("ok"), "cu" => d.get("cu") }), json::LINE);
            show(got) == show(want)
        }
        _ => false,
    }
}

fn show(d: &Dict) -> String {
    json::dumps(&V::Dict(d.clone()), json::LINE)
}

fn versions(v: Vec<PyInt>) -> V {
    V::List(List::from_vec(v.iter().map(|n| V::from(n.to_decimal())).collect()))
}

/// 一个临时目录（realpath 过的），用完删掉
struct Temp(String);

impl Temp {
    fn new(tag: &str) -> Temp {
        let base = path::realpath(&crate::py::fs::gettempdir());
        let dir = TemporaryDirectory::new(&format!("cca-clients-{tag}-"), &base).expect("建临时目录");
        let name = dir.name.clone();
        std::mem::forget(dir);
        Temp(name)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = crate::py::shutil::rmtree(&self.0);
    }
}

/// 一个个函数：同样的输入，结果和 Python 的一样
fn check_functions(v: &Dict, wrong: &mut Vec<String>) {
    let _reset = Reset;
    let mut check = |what: String, got: &Dict, want: &Dict| {
        if !same_outcome(got, want) {
            wrong.push(format!("{what}\n   python: {}\n   rust:   {}", show(want), show(got)));
        }
    };
    let rows = |k: &str| -> Vec<List> {
        v.list(k)
            .unwrap_or_else(|| panic!("对照数据里没有 {k}"))
            .items()
            .iter()
            .map(|r| r.as_list().expect("一行是个列表"))
            .collect()
    };

    // 地址规则：四种客户端各自认哪些地址
    for row in rows("url") {
        let (id, url, package) = (row.at(0).py_str(), row.at(1), row.at(2).truthy());
        set_cu(client(&id));
        check(
            format!("cu_url {id} package={package} {}", url.py_repr()),
            &outcome(cu_url_v(&url, package).map(V::from)),
            &row.at(3).as_dict().unwrap(),
        );
    }
    // 版本号、构建号
    for row in rows("version") {
        check(
            format!("cu_version {}", row.at(0).py_repr()),
            &outcome(cu_version(&row.at(0)).map(versions)),
            &row.at(1).as_dict().unwrap(),
        );
    }
    for row in rows("build") {
        check(
            format!("cu_build {}", row.at(0).py_repr()),
            &outcome(cu_build(&row.at(0)).map(versions)),
            &row.at(1).as_dict().unwrap(),
        );
    }
    for row in rows("build_order") {
        let (a, b) = (row.at(0), row.at(1));
        let got = (|| -> R<V> {
            let (x, y) = (cu_build(&a)?, cu_build(&b)?);
            Ok(V::List(crate::list![x <= y, x < y, x > y]))
        })();
        check(format!("cu_build {} <=> {}", a.py_repr(), b.py_repr()), &outcome(got), &row.at(2).as_dict().unwrap());
    }
    // 版本列表
    set_cu(&CU_CODEX);
    for row in rows("feed") {
        let (name, data, arch, system) = (row.at(0).py_str(), test_blob(&row.at(1)), row.at(2).py_str(), row.at(3).py_str());
        let got = outcome(cu_parse_feed_for_test(&data, &arch, &system));
        if name == "utf16-with-doctype" {
            // 有意不一样（见 py/xml.rs）：UTF-16 写的版本列表里的 DOCTYPE，原来的检查看不出来、expat 也收；这里的解析器不收
            assert_eq!(show(&got), r#"{"cu": ["error", "无法读取官方版本列表"]}"#);
            continue;
        }
        check(format!("cu_parse_feed {name} {arch} {system}"), &got, &row.at(4).as_dict().unwrap());
    }
    set_cu(&CU_CLAUDE);
    for row in rows("squirrel") {
        let (name, data, arch) = (row.at(0).py_str(), test_blob(&row.at(1)), row.at(2).py_str());
        check(
            format!("cu_parse_squirrel {name} {arch}"),
            &outcome(feed::cu_parse_squirrel(&data, &arch).map(V::Dict)),
            &row.at(3).as_dict().unwrap(),
        );
    }
    set_cu(&CU_CLAUDE_CLI);
    for row in rows("cli_latest") {
        let data = test_blob(&row.at(0));
        check(
            format!("cu_cli_latest {:?}", String::from_utf8_lossy(&data)),
            &outcome(feed::cu_cli_latest(&data).map(V::from)),
            &row.at(1).as_dict().unwrap(),
        );
    }
    // 三个手写的判断（原来是带前瞻、后顾或者拼出来的正则）
    for row in rows("version_search") {
        let text = row.at(0).py_str();
        assert_eq!(V::from(cli::version_in(&text)).py_repr(), row.at(1).py_repr(), "version_in {text:?}");
    }
    for row in rows("ps_line") {
        let line = row.at(0).py_str();
        let got = install::ps_line(&line).map(|(pid, rest)| crate::list![pid, rest]);
        assert_eq!(V::from(got).py_repr(), row.at(1).py_repr(), "ps_line {line:?}");
    }
    for row in rows("path_in_command") {
        let (p, command) = (row.at(0).py_str(), row.at(1).py_str());
        assert_eq!(install::path_in_command(&p, &command), row.at(2).truthy(), "path_in_command {p:?} {command:?}");
    }
    for row in rows("environment") {
        let got =
            List::from_vec(cu_environment(&row.at(0).py_str()).into_iter().map(|(k, v)| V::List(crate::list![k, v])).collect());
        assert_eq!(V::List(got).py_repr(), row.at(1).py_repr());
    }
    // 进程列表：两次 ps 的输出是对照数据里给的
    for row in rows("processes") {
        let (name, id, command, comm) =
            (row.at(0).py_str(), row.at(1).py_str(), test_blob(&row.at(2)), (!row.at(3).is_none()).then(|| test_blob(&row.at(3))));
        let (paths, want) = (row.at(4).as_list().unwrap().items(), row.at(5).as_dict().unwrap());
        set_cu(client(&id));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let seen = calls.clone();
        HOOKS.with(|h| {
            h.borrow_mut().run =
                Some(Rc::new(move |args: &[String], folder: &str, timeout: f64, _env: Option<&[(String, String)]>| {
                    seen.borrow_mut().push(V::List(crate::list![crate::py::obj::str_list(args), folder, timeout]));
                    let stdout = match (&comm, args.last().map(String::as_str)) {
                        (Some(comm), Some("pid=,comm=")) => comm.clone(),
                        _ => command.clone(),
                    };
                    Ok(Done { rc: 0, stdout, stderr: Vec::new() })
                }))
        });
        let got = outcome(install::cu_no_processes(&paths).map(|()| V::None));
        check(format!("cu_no_processes {name}"), &got, &want);
        assert!(
            V::List(List::from_vec(calls.borrow().clone())).py_eq(&want.at("calls")),
            "cu_no_processes {name} 起的命令：{:?}",
            calls.borrow()
        );
    }
    set_cu(&CU_CODEX);
    for row in rows("processes_rc") {
        let rcs: Vec<i64> = row.at(0).as_list().unwrap().items().iter().map(|n| n.as_int().unwrap()).collect();
        let left = Rc::new(RefCell::new(rcs.clone()));
        HOOKS.with(|h| {
            h.borrow_mut().run = Some(Rc::new(move |_: &[String], _: &str, _: f64, _: Option<&[(String, String)]>| {
                Ok(Done { rc: left.borrow_mut().remove(0) as i32, stdout: b"1 x\n".to_vec(), stderr: Vec::new() })
            }))
        });
        check(
            format!("cu_no_processes rc={rcs:?}"),
            &outcome(install::cu_no_processes(&[V::from("/Applications/ChatGPT.app")]).map(|()| V::None)),
            &row.at(1).as_dict().unwrap(),
        );
    }
    HOOKS.with(|h| h.borrow_mut().run = None);
}

fn cu_parse_feed_for_test(data: &[u8], arch: &str, system: &str) -> R<V> {
    feed::cu_parse_feed(data, arch, system).map(V::Dict)
}

/// 目录里现在的样子：{相对路径: `dir 权限` / `link -> 指向` / `file 权限 摘要`}
fn listing(folder: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![folder.to_string()];
    while let Some(dir) = stack.pop() {
        for name in crate::py::shutil::listdir(&dir).expect("列目录") {
            let p = path::join(&dir, &name);
            let st = std::fs::symlink_metadata(&p).expect("lstat");
            let rel = p[folder.len() + 1..].to_string();
            if st.file_type().is_symlink() {
                out.insert(rel, format!("link -> {}", std::fs::read_link(&p).unwrap().to_string_lossy()));
            } else if st.is_dir() {
                out.insert(rel, format!("dir {:o}", st.permissions().mode() & 0o777));
                stack.push(p);
            } else {
                out.insert(
                    rel,
                    format!(
                        "file {:o} {}",
                        st.permissions().mode() & 0o7777,
                        crate::py::hash::sha256_hex(std::fs::read(&p).unwrap())
                    ),
                );
            }
        }
    }
    out
}

/// 解压：同一批压缩包，收不收、解出来什么（名字、权限、链接）和 Python 的一样
fn check_extract(v: &Dict, wrong: &mut Vec<String>) {
    let _reset = Reset;
    // 建目录的权限要过 umask：对照数据是在 022 下生成的。别的 umask 下目录的权限不比
    let temp = Temp::new("extract");
    let probe = path::join(&temp.0, "probe");
    std::fs::create_dir(&probe).unwrap();
    let umask_022 = std::fs::metadata(&probe).unwrap().permissions().mode() & 0o777 == 0o755;
    let rows = v.list("extract").unwrap().items();
    assert!(rows.len() >= 60);
    for (i, row) in rows.iter().enumerate() {
        let row = row.as_list().unwrap();
        let (name, id, want) = (row.at(0).py_str(), row.at(1).py_str(), row.at(3).as_dict().unwrap());
        let data = if row.at(2).is_none() { too_many_entries() } else { test_blob(&row.at(2)) };
        let (archive, folder) = (path::join(&temp.0, &format!("{i}.zip")), path::join(&temp.0, &format!("out-{i}")));
        std::fs::write(&archive, data).unwrap();
        std::fs::create_dir(&folder).unwrap();
        set_cu(client(&id));
        let got = outcome(extract::cu_extract(&archive, &folder).map(|app| V::from(&app[folder.len() + 1..])));
        if !same_outcome(&got, &want) {
            wrong.push(format!("cu_extract {name}\n   python: {}\n   rust:   {}", show(&want), show(&got)));
            continue;
        }
        if let V::Dict(tree) = row.at(4) {
            let want_tree: BTreeMap<String, String> = tree.items().into_iter().map(|(k, v)| (k.to_string(), v.py_str())).collect();
            let mut got_tree = listing(&folder);
            if !umask_022 {
                got_tree.retain(|_, v| !v.starts_with("dir "));
            }
            let want_tree: BTreeMap<String, String> =
                want_tree.into_iter().filter(|(_, v)| umask_022 || !v.starts_with("dir ")).collect();
            if got_tree != want_tree {
                wrong.push(format!("cu_extract {name} 解出来的东西\n   python: {want_tree:?}\n   rust:   {got_tree:?}"));
            }
        }
    }
}

/// 100001 个条目的压缩包（对照数据里不存：太大。和生成脚本造的是同一个东西，只是这里手拼）
fn too_many_entries() -> Vec<u8> {
    let (mut locals, mut central) = (Vec::new(), Vec::new());
    let n = 100001u32;
    for i in 0..n {
        let name = format!("ChatGPT.app/f{i}");
        let offset = locals.len() as u32;
        locals.extend_from_slice(b"PK\x03\x04");
        locals.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        locals.extend_from_slice(&[0; 12]);
        locals.extend_from_slice(&(name.len() as u16).to_le_bytes());
        locals.extend_from_slice(&[0, 0]);
        locals.extend_from_slice(name.as_bytes());
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    // 条目数放不进 16 位：结束记录里写 0xFFFF，前面加 zip64 的两条记录
    let (cd_offset, cd_size) = (locals.len() as u64, central.len() as u64);
    let mut out = locals;
    out.extend_from_slice(&central);
    let zip64_at = out.len() as u64;
    out.extend_from_slice(b"PK\x06\x06");
    out.extend_from_slice(&44u64.to_le_bytes());
    out.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(n as u64).to_le_bytes());
    out.extend_from_slice(&(n as u64).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(b"PK\x06\x07");
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&zip64_at.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    out.extend_from_slice(&(cd_size as u32).to_le_bytes());
    out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

/// `urljoin`：对照数据是真的 CPython 3.14 的 urllib.parse.urljoin 给的（名字查重用的 NFD + casefold 的测试在 casefold.rs 里）
fn check_urljoin(wrong: &mut Vec<String>) {
    let rows = stdlib_vectors().list("urljoin").unwrap().items();
    assert!(rows.len() > 60);
    for row in rows {
        let row = row.as_list().unwrap();
        let (base, url) = (row.at(0).py_str(), row.at(1).py_str());
        let got = outcome(urljoin::urljoin(&base, &url).map(V::from));
        if !same_outcome(&got, &row.at(2).as_dict().unwrap()) {
            wrong.push(format!("urljoin({base:?}, {url:?})\n   python: {}\n   rust:   {}", row.at(2).py_repr(), show(&got)));
        }
    }
}

#[test]
fn time_budget_stops_new_downloads() {
    // tests/client_updates.py 的 test_time_budget_stops_new_downloads
    let _reset = Reset;
    set_cu(&CU_CLAUDE);
    CU_BUDGET.with(|b| b.set(Some(time() + 5.0)));
    assert_eq!(cu_parts(&cu_time(300).unwrap_err()), Some(("error", "这次检查的时间用完了，稍后会重新检查")));
    CU_BUDGET.with(|b| b.set(Some(time() + 100.0)));
    assert!((98..=100).contains(&cu_time(300).unwrap()));
    assert_eq!(cu_time(30).unwrap(), 30);
    CU_BUDGET.with(|b| b.set(Some(time() + 20.9)));
    assert_eq!(cu_time(300).unwrap(), 20);
    CU_BUDGET.with(|b| b.set(Some(time() + 19.9)));
    assert!(cu_time(300).is_err());
    CU_BUDGET.with(|b| b.set(None));
    assert_eq!(cu_time(300).unwrap(), 300);
}

#[test]
fn target_is_found_by_bundle_id() {
    // cu_target 没被环境变量指定时翻 /Applications、~/Applications：对拍不能走这条路（会翻到这台机器上真的 App），在临时目录里测
    let _reset = Reset;
    let temp = Temp::new("target");
    let plist = |bundle: &str| crate::py::plist::dumps_xml(&P::dict([("CFBundleIdentifier", P::from(bundle))])).unwrap();
    let put = |rel: &str, data: &[u8]| {
        let p = path::join(&temp.0, rel);
        std::fs::create_dir_all(path::dirname(&p)).unwrap();
        std::fs::write(p, data).unwrap();
    };
    let bases = [path::join(&temp.0, "sys"), path::join(&temp.0, "user")];
    set_cu(&CU_CODEX);
    assert_eq!(cu_target_in(&bases).unwrap(), "");
    put("user/Codex.app/Contents/Info.plist", &plist("com.openai.codex"));
    assert_eq!(cu_target_in(&bases).unwrap(), path::join(&bases[1], "Codex.app"));
    put("user/ChatGPT.app/Contents/Info.plist", &plist("com.openai.chat")); // 同名的别的 App：不认
    assert_eq!(cu_target_in(&bases).unwrap(), path::join(&bases[1], "Codex.app"));
    put("sys/ChatGPT.app/Contents/Info.plist", b"not a plist"); // 读不出来的跳过
    put("sys/Codex.app/Contents/Info.plist", &crate::py::plist::dumps_xml(&P::from(vec![P::from("x")])).unwrap()); // 最外面不是字典：跳过
    assert_eq!(cu_target_in(&bases).unwrap(), path::join(&bases[1], "Codex.app"));
    put("sys/ChatGPT.app/Contents/Info.plist", &plist("com.openai.codex")); // 系统的那个目录先看，ChatGPT.app 先于 Codex.app
    assert_eq!(cu_target_in(&bases).unwrap(), path::join(&bases[0], "ChatGPT.app"));
    set_cu(&CU_CLAUDE);
    assert_eq!(cu_target_in(&bases).unwrap(), "");
    put("user/Claude.app/Contents/Info.plist", &plist("com.anthropic.claudefordesktop"));
    assert_eq!(cu_target_in(&bases).unwrap(), path::join(&bases[1], "Claude.app"));
    // XML 写坏了的属性列表：原来是没人接的 ExpatError
    put("sys/Claude.app/Contents/Info.plist", b"<?xml version=\"1.0\"?><plist><dict><key>x</plist>");
    assert!(cu_target_in(&bases).unwrap_err().is("ExpatError"));
    // 终端工具没有名字可翻
    set_cu(&CU_CLAUDE_CLI);
    assert_eq!(cu_target_in(&bases).unwrap(), "");

    // 环境变量：CCA_CLIENT_… 设了就照它（空的就是没装），没设再看 CCA_…_APP（空的不算）
    let with_env = |pairs: &[(&str, &str)]| {
        let env = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        WORLD.with(|w| {
            *w.borrow_mut() = Some(World { home: path::join(&temp.0, "home"), cca_home: path::join(&temp.0, "home/.cca"), env })
        });
    };
    set_cu(&CU_CODEX);
    with_env(&[("CCA_CLIENT_CODEX_APP", "/a/ChatGPT.app"), ("CCA_CODEX_APP", "/b/ChatGPT.app")]);
    assert_eq!(cu_target().unwrap(), "/a/ChatGPT.app");
    with_env(&[("CCA_CLIENT_CODEX_APP", ""), ("CCA_CODEX_APP", "/b/ChatGPT.app")]);
    assert_eq!(cu_target().unwrap(), "");
    with_env(&[("CCA_CODEX_APP", "/b/ChatGPT.app")]);
    assert_eq!(cu_target().unwrap(), "/b/ChatGPT.app");
    set_cu(&CU_CODEX_CLI);
    with_env(&[("CCA_CLI_CODEX", "/x/codex"), ("CCA_CLI_CLAUDE", "/x/claude")]);
    assert_eq!(cu_target().unwrap(), "/x/codex");
    with_env(&[]);
    assert_eq!(cu_target().unwrap(), "");
}

#[test]
fn host_is_this_mac() {
    // cu_host 问的是真的 sysctl、读的是真的 SystemVersion.plist（都是只读的）。系统版本要和 sw_vers 说的一样：原来的
    // platform.mac_ver() 在这台机器上给的也是它（3.14 和系统自带的 3.9.6 都实跑过）
    let _reset = Reset;
    let host = cu_host();
    assert_eq!(host.is_ok(), cfg!(target_os = "macos"), "{:?}", host.as_ref().err());
    match host {
        Ok((arch, system)) => {
            assert!(matches!(arch.as_str(), "arm64" | "x64") && dotted(&system, 0, 5), "{arch} {system}");
            let said = std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().expect("sw_vers");
            assert_eq!(system, String::from_utf8_lossy(&said.stdout).trim());
        }
        Err(e) => assert_eq!(cu_parts(&e), Some(("blocked", "仅支持 macOS 上经过识别的处理器架构"))),
    }
}

// --- 场景重放 ------------------------------------------------------------------------------------------------------------

/// 装着的一个钩子（场景里 `hook` 那一步）
struct Hook {
    on: String,
    when: Dict,
    once: bool,
    before: Vec<V>,
    after: Vec<V>,
    overwrite: bool,
    raises: Option<String>,
    fired: u32,
    seen: u32,
}

/// 一个场景重放时的状态：假世界的根目录、假命令照着回话的那几项（fake）、装着的钩子、这一步里记下的命令和下载
struct State {
    root: String,
    tmp: String,
    tmp_real: String,
    fake: Dict,
    hooks: Vec<Hook>,
    tokens: Vec<String>,
    calls: Vec<V>,
    downloads: Vec<String>,
    rmtree_saw: Vec<bool>,
}

impl State {
    /// 每次运行不一样的东西换掉。和生成脚本的 `Scn.norm` 是同一套规矩、同一个先后
    fn norm(&mut self, text: &str) -> String {
        use regex::{Captures, Regex};
        use std::sync::LazyLock;
        macro_rules! rx {
            ($p:expr) => {{
                static R: LazyLock<Regex> = LazyLock::new(|| Regex::new($p).expect("正则"));
                &*R
            }};
        }
        let mut text = text.replace(&self.root, "<ROOT>").replace(&self.tmp_real, "<TMP>").replace(&self.tmp, "<TMP>");
        text = rx!(r#""(install_token|token)": "([0-9a-f]{64})""#)
            .replace_all(&text, |m: &Captures| {
                let n = match self.tokens.iter().position(|t| t == &m[2]) {
                    Some(i) => i + 1,
                    None => {
                        self.tokens.push(m[2].to_string());
                        self.tokens.len()
                    }
                };
                format!("\"{}\": \"<TOKEN {}>\"", &m[1], n)
            })
            .into_owned();
        if let Some(generated) = rx!(r#""generated_epoch": (\d{9,}\.\d+)"#).captures(&text).map(|m| m[1].parse::<f64>().unwrap()) {
            text = rx!(r#""next_check_epoch": (\d{9,}\.\d+)"#)
                .replace_all(&text, |m: &Captures| {
                    format!("\"next_check_epoch\": \"<T+{}>\"", num::round((m[1].parse::<f64>().unwrap() - generated) / 60.0) * 60)
                })
                .into_owned();
        }
        text = rx!(r#""(generated_epoch|checked_epoch)": \d{9,}\.\d+"#).replace_all(&text, "\"$1\": \"<T>\"").into_owned();
        text = rx!(r"\[\s*\d+,\s*\d+,\s*\d+,\s*\d+,\s*\d+\s*\]").replace_all(&text, "[<ID>]").into_owned();
        text = rx!(r#""(device|inode|modified_ns|changed_ns)": \d+"#).replace_all(&text, "\"$1\": \"<N>\"").into_owned();
        text = rx!(r"\b(check|update|cca-cli)-[a-z0-9_]{8}").replace_all(&text, "$1-<R>").into_owned();
        text = rx!(r"\.cca-client-install-[a-z0-9_]{8}").replace_all(&text, ".cca-client-install-<R>").into_owned();
        rx!(r"device_id=[0-9a-f-]{36}").replace_all(&text, "device_id=<UUID>").into_owned()
    }

    fn path(&self, rel: &str) -> String {
        path::join(&self.root, rel)
    }

    fn blob(&self, key: &str) -> Option<Vec<u8>> {
        self.fake.get(key).filter(|v| !v.is_none()).map(|v| test_blob(&v))
    }

    fn text(&self, key: &str) -> String {
        self.fake.at(key).py_str()
    }

    /// 钩子里要做的几步（写文件、改 JSON、改假数据）
    fn run_sub(&mut self, steps: &[V]) {
        for s in steps {
            let s = s.as_dict().unwrap();
            match s.at("op").py_str().as_str() {
                "write" => write_file(&self.path(&s.at("path").py_str()), &test_blob(&s.at("b64")), None, None),
                "json_edit" => json_edit(&self.path(&s.at("path").py_str()), &s),
                "fake" => self.fake.set(s.at("key").py_str(), s.at("value")),
                other => panic!("钩子里不认识的步骤：{other}"),
            }
        }
    }

    /// 装着的钩子里对得上的：做它 before / after 里的几步；after 的还可能要抛异常
    fn fire(&mut self, on: &str, before: bool, matches: &dyn Fn(&Dict, &mut u32) -> bool) -> R<()> {
        let mut boom = None;
        for i in 0..self.hooks.len() {
            let (when, mut seen) = (self.hooks[i].when.clone(), self.hooks[i].seen);
            let hit = self.hooks[i].on == on && matches(&when, &mut seen);
            self.hooks[i].seen = seen;
            if !hit {
                continue;
            }
            if before {
                if self.hooks[i].once && self.hooks[i].fired > 0 {
                    continue;
                }
                self.hooks[i].fired += 1;
                let steps = self.hooks[i].before.clone();
                self.run_sub(&steps);
            } else {
                let steps = self.hooks[i].after.clone();
                self.run_sub(&steps);
                boom = boom.or(self.hooks[i].raises.clone());
            }
        }
        match boom.as_deref() {
            Some("PowerLoss") => Err(Fail::Other("PowerLoss", Vec::new())),
            Some("KeyboardInterrupt") => Err(Fail::Other("KeyboardInterrupt", Vec::new())),
            Some(other) => panic!("不认识的异常：{other}"),
            None => Ok(()),
        }
    }
}

fn write_file(p: &str, data: &[u8], mode: Option<u32>, dirmode: Option<u32>) {
    match dirmode {
        Some(m) => crate::py::fs::makedirs_mode(&path::dirname(p), m).unwrap(),
        None => crate::py::fs::makedirs(&path::dirname(p)).unwrap(),
    }
    // 和 Python 的 open(p, 'wb') 一样：已有的文件截断了重写，还是同一个 inode
    std::fs::write(p, data).unwrap();
    if let Some(mode) = mode {
        crate::py::fs::chmod(p, mode).unwrap();
    }
}

/// 读进 JSON、改几个键、照 json.dumps 默认的写法写回去
fn json_edit(p: &str, step: &Dict) {
    let d = json::loads(&std::fs::read_to_string(p).unwrap()).unwrap().as_dict().unwrap();
    if let Some(set) = step.dict("set") {
        d.update(&set);
    }
    for k in step.list("pop").map_or(Vec::new(), |l| l.items()) {
        d.pop(&k.py_str()).expect("要去掉的键得在");
    }
    std::fs::write(p, json::dumps(&V::Dict(d), json::Fmt::new())).unwrap();
}

/// 假的 cu_run：照 fake 里的几项回话（生成脚本的 `Scn.fake_run`）
fn fake_run(state: &Rc<RefCell<State>>, args: &[String], folder: &str, timeout: f64, env: Option<&[(String, String)]>) -> R<Done> {
    let mut st = state.borrow_mut();
    let record = dict! {
        "args" => List::from_vec(args.iter().map(|a| V::from(st.norm(a))).collect()), "folder" => st.norm(folder), "timeout" => timeout,
        "env" => env.map(|env| List::from_vec(env.iter().map(|(k, v)| V::List(crate::list![k.as_str(), st.norm(v)])).collect())),
    };
    st.calls.push(V::Dict(record.clone()));
    let done = |rc: i32, out: Vec<u8>, err: &[u8]| Ok(Done { rc, stdout: out, stderr: err.to_vec() });
    let last = args.last().cloned().unwrap_or_default();
    st.fire("run", true, &|when, seen| {
        if when.get("last_arg").map(|a| a.py_str()).as_deref() != Some(last.as_str()) {
            return false;
        }
        *seen += 1;
        when.get("nth").and_then(|n| n.as_int()).is_none_or(|n| n == *seen as i64)
    })?;
    let flag = |name: &str| args.iter().any(|a| a == name);
    let after = |name: &str| args.iter().position(|a| a == name).map(|i| args[i + 1].clone());
    if args[0] == "/usr/bin/curl" {
        let script = st.fake.list("curl").unwrap();
        if script.is_empty() {
            // 剧本用完了：那边是 [].pop(0) 的 IndexError（生成时已经核对过每个剧本正好用完，走到这里就是两边不一样）
            return Err(Fail::Other("IndexError", Vec::new()));
        }
        let r = script.remove(0).as_dict().unwrap();
        if let Some(header) = r.get("header_b64").filter(|v| !v.is_none()) {
            std::fs::write(after("--dump-header").unwrap(), test_blob(&header)).unwrap();
        }
        if let Some(body) = r.get("body_b64").filter(|v| !v.is_none()) {
            std::fs::write(after("--output").unwrap(), test_blob(&body)).unwrap();
        }
        if r.truthy("fail") {
            return cu_error("error", "更新校验命令失败或超时");
        }
        return done(
            r.get("rc").and_then(|n| n.as_int()).unwrap_or(0) as i32,
            r.get("stdout_b64").map_or(Vec::new(), |b| test_blob(&b)),
            b"",
        );
    }
    if let Some(rc) = st.fake.get("cli_run_rc").and_then(|n| n.as_int()) {
        return done(rc as i32, Vec::new(), b"");
    }
    if last == "--version" {
        return done(0, format!("{}{}", st.text("cli_installed"), st.text("cli_suffix")).into_bytes(), b"");
    }
    if last == "update" {
        let conf =
            env.and_then(|env| env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR")).map(|(_, v)| path::join(v, "settings.json"));
        if let Some(settings) = conf.and_then(|p| std::fs::read_to_string(p).ok()) {
            record.set("settings", settings);
        }
        if st.fake.truthy("cli_update_works") {
            let latest = st.fake.at("cli_latest");
            st.fake.set("cli_installed", latest);
            return done(0, b"done".to_vec(), b"");
        }
        return done(1, Vec::new(), b"first line\nnetwork is down");
    }
    let failed = st.fake.list("failed").unwrap();
    if failed.has_str(&args[0]) || (flag("--verify-client-update") && failed.has_str("verifier")) {
        return done(1, Vec::new(), b"failed");
    }
    let mut out = Vec::new();
    if flag("--verify-client-update") && st.hooks.iter().any(|h| h.on == "verify" && h.overwrite) {
        let size = std::fs::metadata(&args[2]).unwrap().len() as usize;
        // 原地改写（inode 不变、大小不变）
        use std::io::Write;
        std::fs::OpenOptions::new().write(true).open(&args[2]).unwrap().write_all(&vec![b'Z'; size]).unwrap();
    }
    if flag("--display") {
        out = match st.fake.get("display").filter(|d| !d.is_none()) {
            Some(display) => display.py_str().into_bytes(),
            None if args.join(" ").contains("ChatGPT.app") => b"TeamIdentifier=2DC432GLL2\nIdentifier=com.openai.codex\n".to_vec(),
            None => b"TeamIdentifier=Q6L2SF6YDW\nIdentifier=com.anthropic.claudefordesktop\n".to_vec(),
        };
    }
    if flag("-archs") {
        out = st.blob("archs_b64").unwrap();
    }
    if args[0] == "/bin/ps" {
        let listed = match (st.blob("comm_b64"), last.as_str()) {
            (Some(comm), "pid=,comm=") => comm,
            _ => st.blob("processes_b64").unwrap(),
        };
        // 进程列表里的 <ROOT> 是这个假世界的根目录
        out = String::from_utf8(listed).unwrap().replace("<ROOT>", &st.root).into_bytes();
    }
    done(0, out, b"")
}

/// 假的 cu_download（生成脚本的 `Scn.fake_download`）
fn fake_download(state: &Rc<RefCell<State>>, url: &str, destination: &str, package: bool) -> R<Option<String>> {
    cu_url(url, package)?; // 真规则：地址不合规就报错
    let mut st = state.borrow_mut();
    let shown = st.norm(url);
    st.downloads.push(shown);
    if st.fake.get("download_raises").is_some_and(|v| !v.is_none()) {
        return Err(Fail::Other("KeyboardInterrupt", Vec::new()));
    }
    let data = if package {
        st.blob(if url.contains("oaistatic") { "pkg_codex_b64" } else { "pkg_claude_b64" }).unwrap()
    } else if url.contains("oaistatic") {
        st.blob("feed_codex_b64").unwrap()
    } else if url.contains("squirrel/update") {
        let codes = st.fake.list("feed_codes").unwrap();
        if !codes.is_empty() {
            let code = codes.remove(0);
            if code.truthy() {
                return Ok(Some(code.py_str()));
            }
        } else if let Some(code) = st.fake.get("feed_code").filter(|c| c.truthy()) {
            return Ok(Some(code.py_str()));
        }
        st.blob("feed_claude_b64").unwrap()
    } else {
        st.blob("cli_body_b64").unwrap_or_else(|| format!("{}\n", st.text("cli_latest")).into_bytes())
    };
    std::fs::write(destination, data).unwrap();
    crate::py::fs::chmod(destination, 0o600).unwrap();
    st.fire("download", false, &|when, _| when.get("package").is_some_and(|p| p.truthy() == package))?;
    Ok(None)
}

/// 假的改名：先做钩子里 before 的几步，真的改名，再看要不要“断电”
fn fake_rename(state: &Rc<RefCell<State>>, source: &str, destination: &str) -> R<()> {
    let root = state.borrow().root.clone();
    let matches = |when: &Dict, _: &mut u32| {
        let is = |k: &str, actual: &str| when.get(k).is_none_or(|w| w.py_str().replace("<ROOT>", &root) == actual);
        is("source", source)
            && is("destination", destination)
            && source.ends_with(&when.get("source_endswith").map_or(String::new(), |s| s.py_str()))
    };
    state.borrow_mut().fire("rename", true, &matches)?;
    install::rename_exclusive(source, destination)?;
    state.borrow_mut().fire("rename", false, &matches)
}

/// 假世界里现在的样子（生成脚本的 `Scn.tree`）：先后也要一样，安装凭据是照头一次出现的先后编号的
fn tree(state: &Rc<RefCell<State>>) -> BTreeMap<String, String> {
    let root = state.borrow().root.clone();
    let mut out = BTreeMap::new();
    let mut walk = vec![root.clone()];
    // os.walk：先把这一层的条目（按名字排）都看一遍，再按名字一个个进子目录
    while let Some(dir) = walk.pop() {
        let mut names = crate::py::shutil::listdir(&dir).unwrap();
        names.sort();
        let mut dirs = Vec::new();
        for n in &names {
            let p = path::join(&dir, n);
            let st = std::fs::symlink_metadata(&p).unwrap();
            let rel = state.borrow_mut().norm(&p[root.len() + 1..]);
            let value = if st.file_type().is_symlink() {
                format!("link -> {}", state.borrow_mut().norm(&std::fs::read_link(&p).unwrap().to_string_lossy()))
            } else if st.is_dir() {
                dirs.push(p.clone());
                format!("dir {:o}", st.permissions().mode() & 0o777)
            } else {
                let data = std::fs::read(&p).unwrap();
                let shown = match std::str::from_utf8(&data) {
                    Ok(text) if data.len() <= 4000 => state.borrow_mut().norm(text),
                    _ => format!("sha256:{}", crate::py::hash::sha256_hex(&data)),
                };
                format!("file {:o} {}", st.permissions().mode() & 0o777, shown)
            };
            out.insert(rel, value);
        }
        // 栈是后进先出：倒着放，名字靠前的子目录先走；它的子目录又先于它后面的兄弟（和 os.walk 的先后一样）
        walk.extend(dirs.into_iter().rev());
    }
    out
}

/// 重放一个场景，返回哪些步骤和记下来的不一样
fn replay(scenario: &Dict) -> Vec<String> {
    let _reset = Reset;
    let name = scenario.at("name").py_str();
    let temp = Temp::new("scn");
    let tmp = crate::py::fs::gettempdir();
    let state = Rc::new(RefCell::new(State {
        root: temp.0.clone(),
        tmp_real: path::realpath(&tmp),
        tmp,
        fake: dict! {
            "processes_b64" => "MSAvc2Jpbi9sYXVuY2hkCg==", "comm_b64" => V::None, "failed" => List::new(), "display" => V::None, "archs_b64" => "YXJtNjQK",
            "host" => crate::list!["arm64", "26.0"], "feed_codex_b64" => V::None, "feed_claude_b64" => V::None, "feed_codes" => List::new(), "feed_code" => V::None,
            "pkg_codex_b64" => V::None, "pkg_claude_b64" => V::None, "cli_installed" => "2.0.0", "cli_latest" => "2.1.0", "cli_suffix" => " (Claude Code)",
            "cli_update_works" => true, "cli_body_b64" => V::None, "cli_run_rc" => V::None, "download_raises" => V::None, "rmtree_fails" => V::None, "curl" => List::new(),
        },
        hooks: Vec::new(),
        tokens: Vec::new(),
        calls: Vec::new(),
        downloads: Vec::new(),
        rmtree_saw: Vec::new(),
    }));
    let root = temp.0.clone();
    {
        let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
        HOOKS.with(|h| {
            let mut h = h.borrow_mut();
            h.run = Some(Rc::new(move |args: &[String], folder: &str, timeout: f64, env: Option<&[(String, String)]>| {
                fake_run(&s1, args, folder, timeout, env)
            }));
            h.download = Some(Rc::new(move |url: &str, destination: &str, _folder: &str, package: bool| {
                fake_download(&s2, url, destination, package)
            }));
            h.rename = Some(Rc::new(move |source: &str, destination: &str| fake_rename(&s3, source, destination)));
            h.rmtree = Some(Rc::new(move |work: &str| {
                let fails = s4.borrow().fake.get("rmtree_fails").filter(|f| !f.is_none())?.py_str();
                if !path::basename(work).starts_with(&fails) {
                    return None;
                }
                let journal = path::join(&cu_root(false).unwrap(), "transaction.json");
                s4.borrow_mut().rmtree_saw.push(path::exists(&journal));
                Some(Err(std::io::Error::other("simulated cleanup failure")))
            }));
            h.host = Some(("arm64".into(), "26.0".into()));
            h.slept = Some(Vec::new());
        });
    }
    let mut wrong = Vec::new();
    let mut last_tree: BTreeMap<String, String> = BTreeMap::new();
    let mut want_tree: BTreeMap<String, String> = BTreeMap::new();
    let mut docs: HashMap<usize, V> = HashMap::new();
    let steps = scenario.list("steps").unwrap().items();
    for (index, step) in steps.iter().enumerate() {
        let step = step.as_dict().unwrap();
        let text = |k: &str| step.at(k).py_str();
        let here = |rel: &str| path::join(&root, rel);
        let mut differs = |what: &str, got: String, want: String| {
            if got != want {
                wrong.push(format!("{name} 第 {index} 步（{}）{what}\n   python: {want}\n   rust:   {got}", show(&dict! { "op" => step.at("op"), "action" => step.get("action"), "fn" => step.get("fn"), "label" => step.get("label") })));
            }
        };
        // 记下来的树是和上一步比的变化：攒成完整的一棵再比
        let mut check_tree = |want: &Dict, differs: &mut dyn FnMut(&str, String, String)| {
            for (k, v) in want.dict("tree").unwrap().items() {
                match v {
                    V::None => want_tree.remove(&*k),
                    v => want_tree.insert(k.to_string(), v.py_str()),
                };
            }
            let now = tree(&state);
            if now != want_tree {
                let changed: Vec<String> = now
                    .keys()
                    .chain(want_tree.keys())
                    .filter(|k| now.get(*k) != want_tree.get(*k))
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                for k in changed {
                    differs(&format!("以后的文件 {k}"), format!("{:?}", now.get(&k)), format!("{:?}", want_tree.get(&k)));
                }
            }
            last_tree = now;
        };
        match text("op").as_str() {
            "world" => {
                let mut env = HashMap::new();
                env.insert("HOME".to_string(), root.clone());
                env.insert("CCA_HOME".to_string(), here(".cca"));
                for (k, v) in step.dict("env").unwrap().items() {
                    env.insert(k.to_string(), v.py_str().replace("<ROOT>", &root));
                }
                WORLD.with(|w| *w.borrow_mut() = Some(World { home: root.clone(), cca_home: here(".cca"), env }));
            }
            "env" => WORLD.with(|w| {
                let mut w = w.borrow_mut();
                let env = &mut w.as_mut().unwrap().env;
                match step.at("value") {
                    V::None => env.remove(&text("key")),
                    v => env.insert(text("key"), v.py_str().replace("<ROOT>", &root)),
                };
            }),
            "write" => write_file(
                &here(&text("path")),
                &test_blob(&step.at("b64")),
                step.get("mode").and_then(|m| m.as_int()).map(|m| m as u32),
                step.get("dirmode").and_then(|m| m.as_int()).map(|m| m as u32),
            ),
            "mkdir" => {
                let (p, mode) = (here(&text("path")), step.at("mode").as_int().unwrap() as u32);
                crate::py::fs::makedirs_mode(&p, mode).unwrap();
                crate::py::fs::chmod(&p, mode).unwrap();
            }
            "symlink" => crate::py::shutil::symlink(text("target").replace("<ROOT>", &root), &here(&text("path"))).unwrap(),
            "json_edit" => json_edit(&here(&text("path")), &step),
            "fake" => {
                state.borrow().fake.set(text("key"), step.at("value"));
                if text("key") == "host" {
                    let host = step.at("value").as_list().unwrap();
                    HOOKS.with(|h| h.borrow_mut().host = Some((host.at(0).py_str(), host.at(1).py_str())));
                }
            }
            "hook" => state.borrow_mut().hooks.push(Hook {
                on: text("on"),
                when: step.dict("when").unwrap(),
                once: step.truthy("once"),
                before: step.list("before").unwrap().items(),
                after: step.list("after").unwrap().items(),
                overwrite: step.truthy("overwrite"),
                raises: step.get("raise").filter(|r| !r.is_none()).map(|r| r.py_str()),
                fired: 0,
                seen: 0,
            }),
            "unhook" => state.borrow_mut().hooks.clear(),
            "forbid" => HOOKS.with(|h| {
                let mut h = h.borrow_mut();
                match text("what").as_str() {
                    "hash" => h.forbid_hash = step.truthy("on"),
                    _ => h.forbid_extract = step.truthy("on"),
                }
            }),
            "cu" => set_cu(client(&text("client"))),
            "cmd" => {
                let want = step.dict("want").unwrap();
                // 参数：原样的文字，或者前面某一步打出来的某个客户端的安装凭据
                let arg = match step.get("token").filter(|t| !t.is_none()) {
                    Some(reference) => {
                        let reference = reference.as_list().unwrap();
                        let doc = docs
                            .get(&(reference.at(0).as_int().unwrap() as usize))
                            .expect("那一步打出过状态文档")
                            .as_dict()
                            .unwrap();
                        let row = doc
                            .list("clients")
                            .unwrap()
                            .items()
                            .into_iter()
                            .map(|c| c.as_dict().unwrap())
                            .find(|c| c.at("id").py_str() == reference.at(1).py_str())
                            .unwrap();
                        row.at("install_token").py_str()
                    }
                    None => text("arg"),
                };
                {
                    let mut st = state.borrow_mut();
                    st.calls.clear();
                    st.downloads.clear();
                    st.rmtree_saw.clear();
                }
                HOOKS.with(|h| h.borrow_mut().slept = Some(Vec::new()));
                let mut printed = String::new();
                let result = client_updates(&text("action"), &arg, &mut printed);
                CU_BUDGET.with(|b| b.set(None));
                let (rc, stderr, raised) = match result {
                    Ok(()) => (0, String::new(), V::None),
                    Err(Fail::Die(msg)) => (1, format!("{msg}\n"), V::None),
                    Err(Fail::Exit(n)) => (n, String::new(), V::None),
                    Err(e) if e.is("PowerLoss") || e.is("KeyboardInterrupt") => (0, String::new(), V::from(class_of(&e))),
                    Err(e) => panic!("{name} 第 {index} 步出了没人接的错：{e:?}"),
                };
                if printed.starts_with('{') {
                    docs.insert(index, json::loads(&printed).unwrap());
                }
                let (stdout, stderr) = {
                    let mut st = state.borrow_mut();
                    (st.norm(&printed), st.norm(&stderr))
                };
                differs("退出码", rc.to_string(), want.at("rc").py_str());
                differs("抛出的异常", raised.py_repr(), want.at("raised").py_repr());
                differs("stdout", stdout, want.at("stdout").py_str());
                differs("stderr", stderr, want.at("stderr").py_str());
                check_tree(&want, &mut differs);
                let st = state.borrow();
                let (got_calls, want_calls) = (st.calls.clone(), want.list("calls").unwrap().items());
                for i in 0..got_calls.len().max(want_calls.len()) {
                    let one = |calls: &[V]| calls.get(i).map_or("（没有）".to_string(), |c| json::dumps(c, json::LINE));
                    if !got_calls.get(i).zip(want_calls.get(i)).is_some_and(|(a, b)| a.py_eq(b)) {
                        differs(&format!("起的第 {} 条命令", i + 1), one(&got_calls), one(&want_calls));
                    }
                }
                differs(
                    "下载过的地址",
                    format!("{:?}", st.downloads),
                    format!("{:?}", want.list("downloads").unwrap().items().iter().map(|u| u.py_str()).collect::<Vec<_>>()),
                );
                differs(
                    "删目录时事务记录还在不在",
                    format!("{:?}", st.rmtree_saw),
                    format!("{:?}", want.list("rmtree_saw").unwrap().items().iter().map(|b| b.truthy()).collect::<Vec<_>>()),
                );
                let slept = HOOKS.with(|h| h.borrow().slept.clone().unwrap_or_default());
                differs(
                    "睡了几秒",
                    format!("{slept:?}"),
                    format!("{:?}", want.list("slept").unwrap().items().iter().map(|n| n.as_pynum().unwrap()).collect::<Vec<_>>()),
                );
            }
            "call" => {
                let want = step.dict("want").unwrap();
                state.borrow_mut().calls.clear();
                HOOKS.with(|h| h.borrow_mut().slept = Some(Vec::new()));
                let extra = Dict::new();
                let got = outcome(match text("fn").as_str() {
                    "cu_download" => {
                        // 真的 cu_download：只有底下的 curl 是照剧本回的
                        let hook = HOOKS.with(|h| h.borrow_mut().download.take());
                        let dest = here(&text("dest"));
                        // 上一次留下的响应头：不清掉的话，这一次 curl 没写响应头时读到的是上一次的（stale 就是测这个）
                        if !step.truthy("stale") {
                            let _ = std::fs::remove_file(here("response.headers"));
                        }
                        let empty: Vec<String> = step.list("empty").unwrap().items().iter().map(|e| e.py_str()).collect();
                        let empty: Vec<&str> = empty.iter().map(String::as_str).collect();
                        let expected = step.at("expected_size");
                        let r = match step.at("url") {
                            V::Str(url) => cu_download(&url, &dest, &root, step.truthy("package"), Some(&expected), &empty),
                            other => cu_url_v(&other, step.truthy("package")).map(|_| None),
                        };
                        HOOKS.with(|h| h.borrow_mut().download = hook);
                        if let Ok(data) = std::fs::read(&dest) {
                            let mode = format!("{:o}", std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777);
                            extra.set("dest", crate::list![V::from(crate::py::hash::sha256_hex(&data)), mode]);
                        }
                        r.map(V::from)
                    }
                    "cu_next_check" => {
                        let rows: Vec<Dict> = step.list("rows").unwrap().items().iter().map(|r| r.as_dict().unwrap()).collect();
                        cu_next_check(&rows).map(|v| match v {
                            // 有等着自动安装的：最多 15 分钟以后，记成和现在差多少
                            V::Float(f) if f >= 1e9 => V::from(format!("<T+{}>", num::round((f - time()) / 60.0) * 60)),
                            other => other,
                        })
                    }
                    "lock_twice" => (|| {
                        let p = here(&text("path"));
                        let _first = ClientUpdateLock::acquire(&p)?;
                        let _second = ClientUpdateLock::acquire(&p)?;
                        Ok(V::from("both"))
                    })(),
                    other => panic!("不认识的函数：{other}"),
                });
                // 下载下来的东西：对照数据里记的是内容，这里比摘要
                let want_dest = want.get("dest").map(|d| {
                    let d = d.as_list().unwrap();
                    V::List(crate::list![V::from(crate::py::hash::sha256_hex(test_blob(&d.at(0)))), d.at(1)])
                });
                if !same_outcome(&got, &want) {
                    differs(
                        "结果",
                        show(&got),
                        show(&Dict::from_pairs(want.items().into_iter().filter(|(k, _)| matches!(&**k, "ok" | "cu" | "err")))),
                    );
                }
                differs("下载下来的文件", V::from(extra.get("dest")).py_repr(), V::from(want_dest).py_repr());
                let st = state.borrow();
                let (got_calls, want_calls) = (st.calls.clone(), want.list("calls").unwrap().items());
                for i in 0..got_calls.len().max(want_calls.len()) {
                    let one = |calls: &[V]| calls.get(i).map_or("（没有）".to_string(), |c| json::dumps(c, json::LINE));
                    if !got_calls.get(i).zip(want_calls.get(i)).is_some_and(|(a, b)| a.py_eq(b)) {
                        differs(&format!("起的第 {} 条命令", i + 1), one(&got_calls), one(&want_calls));
                    }
                }
                drop(st);
                let slept = HOOKS.with(|h| h.borrow().slept.clone().unwrap_or_default());
                differs(
                    "睡了几秒",
                    format!("{slept:?}"),
                    format!("{:?}", want.list("slept").unwrap().items().iter().map(|n| n.as_pynum().unwrap()).collect::<Vec<_>>()),
                );
                check_tree(&want, &mut differs);
            }
            other => panic!("不认识的步骤：{other}"),
        }
    }
    let _ = last_tree;
    wrong
}

/// 整条路的场景：一个个重放。`CCA_CLIENTS_SCENARIO=名字里的一段` 只放名字对得上的
fn check_scenarios(v: &Dict, wrong: &mut Vec<String>) {
    let scenarios = v.list("scenarios").unwrap().items();
    assert!(scenarios.len() > 80);
    let only = std::env::var("CCA_CLIENTS_SCENARIO").ok();
    for s in &scenarios {
        let s = s.as_dict().unwrap();
        if only.as_ref().is_some_and(|o| !s.at("name").py_str().contains(o.as_str())) {
            continue;
        }
        wrong.extend(replay(&s));
    }
}

/// 对照数据的几组检查（为什么放在一个测试里见这个文件开头）
#[test]
fn like_python() {
    let v = vectors();
    let mut wrong = Vec::new();
    check_functions(&v, &mut wrong);
    check_extract(&v, &mut wrong);
    check_urljoin(&mut wrong);
    check_scenarios(&v, &mut wrong);
    assert!(wrong.is_empty(), "{} 处和 Python 不一样：\n{}", wrong.len(), wrong[..wrong.len().min(30)].join("\n"));
}
