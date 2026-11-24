//! 测试的运行器：一个模块一个子进程（模块里的用例一个接一个跑），几个模块同时跑；跳过、超时、每个模块一行结果
//!
//! 一份测试（tests/<名字>.rs）长这样，再在 tests/suite.rs 里加一行 `mod` 和表里的一项：
//!
//! ```ignore
//! use kit::case;
//! use kit::harness::{Ctx, Module};
//!
//! pub const MODULE: Module = Module {
//!     name: "device_identity",
//!     src: include_str!("device_identity.rs"),
//!     default: true,
//!     tests: &[case!(keeps_identity_across_logins), case!(slow_one).timeout(600)],
//! };
//!
//! fn keeps_identity_across_logins(t: &mut Ctx) {
//!     let home = t.tmp("cca-device-"); // 用例完了自动删
//!     …                                // 不对就 panic（assert!、assert_eq!、unwrap）：这个用例算失败，后面的照跑
//! }
//! ```
//!
//! 怎么跑（都在 cli/ 里）：`cargo test --profile kit -p cca-tests --test suite -- [选哪些] [开关]`。
//! - 选哪些：模块名（`team`：整个模块），或者 `模块::用例` 里的一截（`remote_host::closetty`）；`--module 名字` 只跑这个模块
//!   （可以写几次）；`--skip 一截` 去掉名字里带这一截的；`--all` 连平时不跑的模块（`default: false`）也跑；`--list` 只列出来
//! - `--jobs N`（libtest 的 `--test-threads N` 也认）：同时跑几个模块，默认 `CCA_SUITE_JOBS` 或者 4
//! - `--results 文件`：每个模块一行 `名字<TAB>pass|fail<TAB>通过<TAB>失败<TAB>跳过<TAB>秒`。pass 是没有失败的、选中的用例都报了结果；
//!   跳过的不算失败，所以用例全跳过的模块也是 pass、通过数是 0。拿这个文件把关的（tests/regress.sh 的 `suite_ok`、tests/linux.sh 的
//!   `suite_run`）是同一个标准：这一行是 pass、通过数大于 0、失败数是 0——一个用例都没过的模块不放行
//! - `--logs 目录`：`<模块>.log`（每个用例一行 `ok|FAIL|skip|TIMEOUT|SLOW|NOTRUN 用例 秒 …`，失败的后面缩进 4 格跟着它的输出）
//!   和 `timing.tsv`（`模块<TAB>用例<TAB>结果<TAB>秒`）。SLOW 是多记的一行：用掉了时限的一多半。带括号的不是用例：
//!   `(audit)` 有用例漏登记，`(teardown)` 模块的收尾没成，`(module)` 模块的进程没好好结束
//! - `--setsid`：自己换一个会话再跑（回归在后台起它时用：读 /dev/tty 马上失败，杀进程组就能全停掉）
//! - `--nocapture`：用例的输出直接打出来（平时只在失败时附在后面）；`-q`：通过的用例不一行行报
//!
//! 退出码：0 是选中的模块都过了，1 是有模块没过，2 是写错了——不认识的开关、没有这个模块、什么都没选中（点名跑一个不存在的
//! 用例不能看着像是过了；`--list` 什么都没列出来还是 0）。所以按名字跑 kit、cca-test 自己的单元测试时带上 `--lib --bins`
//! （`cargo test --profile kit -p cca-tests --lib --bins 名字`）：不带的话 cargo 把这个名字也交给 suite，它一个都选不中。
//! 被信号打断是 128 + 信号
//!
//! 环境变量：`CCA_HELPER`（辅助程序，默认仓库里编出来的）、`CCA_REPO`（仓库在哪）、`CCA_TEST_BIN`（cca-test）、
//! `CCA_SUITE_TMP`（临时目录建在哪）、`KEEP`（临时目录留着不删）。用例里经 [`helper`]、[`repo`]、[`tool`]、[`Ctx::tmp`] 拿。
//!
//! 规矩：
//! - 一个用例 panic 只算它自己失败；超过时限（默认 300 秒，`case!(名字).timeout(秒)` 另定）整个模块停下：记 TIMEOUT，
//!   跑登记过的收尾、杀掉起过的子进程，这个模块剩下的用例记 NOTRUN
//! - 收尾用 [`Ctx::defer`] 登记（`addCleanup`）：超时、被打断时也跑得到，光靠 Drop 不行。经 `kit::proc` 起的子进程不用登记：
//!   `Child` 离开作用域就停掉，超时、被打断、模块跑完时运行器把还活着的都停掉
//! - 模块里每个 `fn 名字(t: &mut Ctx) {` 都得登记在 `tests` 里：漏了的话这个模块算失败、说出是哪个。查的是原文，所以只在一种
//!   系统上跑的用例写 `case!(名字).needs(&[Need::MacOs])`，别拿 `#[cfg]` 把函数去掉；夹具、帮手带别的参数或者有返回值，不算用例
//! - 模块的子进程里 `HOME` 指着一个不存在的目录：要用 HOME 的被测程序，给它一个临时目录
//! - 模块跑完，命令行里还带着它的临时目录的进程（被测的程序留在后台、没人管的）会被停掉，记录里记一行
//!   `LEFTOVER 进程号 命令行`——模块不因此算失败，但该在用例里把它停掉

mod child;
mod fixtures;
mod parent;

use crate::fsx::TempDir;
use std::fmt::Display;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;

/// 一个用例的时限，没另外定的话（秒）
pub const DEFAULT_TIMEOUT_S: u32 = 300;

/// 用例要什么才跑得了；不满足就跳过（结果里记 skip 和原因）
#[derive(Clone, Copy, Debug)]
pub enum Need {
    /// PATH 上有这个命令（`Need::Bin("node")`）
    Bin(&'static str),
    /// 只在 macOS 上跑
    MacOs,
    /// 只在 Linux 上跑
    Linux,
}

impl Need {
    /// 不满足的话，为什么
    fn unmet(&self) -> Option<String> {
        match self {
            Need::Bin(name) => crate::proc::which(name).is_none().then(|| format!("没有 {name}")),
            Need::MacOs => (!cfg!(target_os = "macos")).then(|| "只在 macOS 上跑".to_string()),
            Need::Linux => (!cfg!(target_os = "linux")).then(|| "只在 Linux 上跑".to_string()),
        }
    }
}

/// 一个用例。用 [`case!`](crate::case) 写：`case!(名字)`、`case!(名字).timeout(600)`、`case!(名字).needs(&[Need::Bin("node")])`
#[derive(Clone, Copy)]
pub struct Test {
    /// 名字，就是函数名：结果、记录里用它，命令行上照它选
    pub name: &'static str,
    /// 用例本身：不对就 panic
    pub f: fn(&mut Ctx),
    /// 时限（秒）：到了还没跑完，整个模块停下
    pub timeout_s: u32,
    /// 要什么才跑得了：有一样不满足就跳过
    pub needs: &'static [Need],
}

impl Test {
    /// 一般不直接用：写 `case!(函数名)`
    pub const fn new(name: &'static str, f: fn(&mut Ctx)) -> Test {
        Test { name, f, timeout_s: DEFAULT_TIMEOUT_S, needs: &[] }
    }

    /// 另定时限（秒）；默认 [`DEFAULT_TIMEOUT_S`]
    pub const fn timeout(self, secs: u32) -> Test {
        Test { timeout_s: secs, ..self }
    }

    /// 这几样都满足才跑，不然记 skip 和原因
    pub const fn needs(self, needs: &'static [Need]) -> Test {
        Test { needs, ..self }
    }
}

/// 登记一个用例：`case!(函数名)`，用例的名字就是函数名
#[macro_export]
macro_rules! case {
    ($f:ident) => {
        $crate::harness::Test::new(stringify!($f), $f)
    };
}

/// 一个模块：tests/ 下的一个 .rs
#[derive(Clone, Copy)]
pub struct Module {
    /// 模块名，和文件名一样：`--module`、结果文件、记录的文件名都用它
    pub name: &'static str,
    /// 这个文件自己的原文（`include_str!("名字.rs")`）：拿来查有没有漏登记的用例
    pub src: &'static str,
    /// 用例：照这个先后一个接一个跑
    pub tests: &'static [Test],
    /// false：平时不跑，`--all` 或者点名（`--module`、写模块名）才跑
    pub default: bool,
}

/// 跳过时扔出来的东西（不经 panic 的钩子，不打印）
struct Skip(String);

/// 用例拿到的东西：建临时目录、登记收尾、分子用例、跳过
pub struct Ctx {
    module: &'static str,
    name: &'static str,
    tmps: Vec<TempDir>,
    /// 正在跑的子用例（一层套一层）
    path: Vec<String>,
    failed: Vec<String>,
    skipped: Vec<String>,
}

impl Ctx {
    fn new(module: &'static str, name: &'static str) -> Ctx {
        Ctx { module, name, tmps: Vec::new(), path: Vec::new(), failed: Vec::new(), skipped: Vec::new() }
    }

    /// 这个用例在哪个模块里
    pub fn module(&self) -> &'static str {
        self.module
    }

    /// 这个用例的名字
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 建一个临时目录（名字是 前缀 + 6 个随机字符），给出它的路径（解过链接的）。用例完了、登记的收尾都跑过以后删掉
    /// （设了 KEEP 就留着）。要在几个用例之间留着的目录用 `fsx::TempDir`，自己管它活多久
    pub fn tmp(&mut self, prefix: &str) -> PathBuf {
        let dir = TempDir::new(prefix);
        let path = dir.path().to_path_buf();
        self.tmps.push(dir);
        path
    }

    /// 登记一件收尾的事（`addCleanup`）：用例完了照登记的反序跑；用例 panic 了、超时了、整个程序被打断了也跑
    /// （超时的时候用例自己那条线还卡着，Drop 轮不到）。经 `kit::proc` 起的子进程不用登记，运行器会停掉；要登记的是别的：
    /// 被测的程序自己留在后台的进程、要好好收场才对的服务、改过的东西要改回去
    pub fn defer(&mut self, f: impl FnOnce() + Send + 'static) {
        child::push_defer(Box::new(f), false);
    }

    /// 同 [`Ctx::defer`]，但等这个模块的用例都跑完才跑（几个用例共用的东西）
    pub fn defer_module(&mut self, f: impl FnOnce() + Send + 'static) {
        child::push_defer(Box::new(f), true);
    }

    /// 一个子用例（`subTest`）：里面 panic 了记下是哪一个（label），接着往下跑；有子用例失败，整个用例算失败
    pub fn sub(&mut self, label: impl Display, f: impl FnOnce(&mut Ctx)) {
        self.path.push(label.to_string());
        let result = catch_unwind(AssertUnwindSafe(|| f(self)));
        let path = self.path.join(" / ");
        self.path.pop();
        match result {
            Ok(()) => {}
            Err(payload) => match payload.downcast_ref::<Skip>() {
                Some(Skip(why)) => {
                    eprintln!("子用例（{path}）跳过：{why}");
                    self.skipped.push(format!("{path}：{why}"));
                }
                None => {
                    // 为什么失败，panic 的钩子已经打在上面了
                    eprintln!("子用例（{path}）失败");
                    self.failed.push(path);
                }
            },
        }
    }

    /// 跳过这个用例（`skipTest`）：结果里记 skip 和原因，不算失败
    pub fn skip(&self, why: &str) -> ! {
        resume_unwind(Box::new(Skip(why.to_string())))
    }
}

/// 仓库在哪：`CCA_REPO`，没设就是编这个程序时的那一份（tests/ 的上一层）
pub fn repo() -> PathBuf {
    match std::env::var_os("CCA_REPO") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")),
    }
}

/// 辅助程序（cca-core）在哪：`CCA_HELPER`，没设就是仓库里编出来的 cli/target/release/cca-core
/// （单独跑测试之前先 `cd cli && cargo build --release`）
pub fn helper() -> PathBuf {
    match std::env::var_os("CCA_HELPER") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => repo().join("cli/target/release/cca-core"),
    }
}

/// cca-test 在哪（`CCA_TEST_BIN`，没设就是和这个程序一起编出来的那一份）
pub fn tool() -> PathBuf {
    crate::proc::tool().unwrap_or_else(|e| panic!("{e}"))
}

const USAGE: &str = "用法：suite [模块名 | 模块::用例名里的一截]… [--module 模块]… [--skip 一截]… [--all] [--list] [--jobs N] \
[--results 文件] [--logs 目录] [--setsid] [--nocapture] [-q]";

/// 命令行
#[derive(Debug, Default, PartialEq)]
struct Opts {
    filters: Vec<String>,
    skips: Vec<String>,
    modules: Vec<String>,
    jobs: Option<usize>,
    results: Option<PathBuf>,
    logs: Option<PathBuf>,
    setsid: bool,
    list: bool,
    all: bool,
    nocapture: bool,
    quiet: bool,
    /// filters 里正好是模块名的那几个（[`Opts::settle`] 照模块表认出来的）：只选那个模块，不再照一截去匹配别的
    exact: Vec<String>,
    /// 运行器自己用：这一份是某个模块的子进程
    child: Option<String>,
    /// 运行器自己用：结果经这个描述符报给上一级
    report_fd: Option<i32>,
    /// 运行器测自己用：模块表换成 fixtures.rs 里那几个故意失败、超时的
    fixtures: bool,
    /// 运行器测自己用：用例到了时限、模块的进程自己没停下，上一级再等几秒就杀掉它
    grace: Option<u64>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Opts, String> {
        let mut opts = Opts::default();
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            i += 1;
            // --名字=值 和 --名字 值 都认
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
                _ => (arg.as_str(), None),
            };
            let mut value = |what: &str| match &inline {
                Some(v) => Ok(v.clone()),
                None => {
                    i += 1;
                    args.get(i - 1).cloned().ok_or_else(|| format!("{name} 后面要跟{what}"))
                }
            };
            match name {
                "--module" => opts.modules.push(value("模块名")?),
                "--skip" => opts.skips.push(value("要去掉的名字")?),
                "--jobs" | "--test-threads" => {
                    let n = value("同时跑几个模块")?;
                    opts.jobs =
                        Some(n.parse().ok().filter(|n| *n >= 1).ok_or_else(|| format!("{name} 要一个不小于 1 的数，不是 {n}"))?);
                }
                "--results" => opts.results = Some(value("文件")?.into()),
                "--logs" => opts.logs = Some(value("目录")?.into()),
                "--child" => opts.child = Some(value("模块名")?),
                "--report-fd" => opts.report_fd = Some(value("描述符")?.parse().map_err(|_| format!("{name} 要一个数"))?),
                "--kit-grace" => opts.grace = Some(value("秒数")?.parse().map_err(|_| format!("{name} 要一个数"))?),
                "--setsid" => opts.setsid = true,
                "--list" => opts.list = true,
                "--all" => opts.all = true,
                "--nocapture" | "--no-capture" => opts.nocapture = true,
                "-q" | "--quiet" => opts.quiet = true,
                "--kit-fixtures" => opts.fixtures = true,
                // libtest 的开关：cargo test 把 -- 后面的原样交给每个测试程序，认下来、不管
                "--color" | "--format" | "--logfile" | "-Z" | "--shuffle-seed" => drop(value("值")?),
                "--exact" | "--show-output" | "--include-ignored" | "--ignored" | "--test" | "--bench" | "--report-time"
                | "--shuffle" => {}
                _ if name.starts_with('-') => return Err(format!("不认识 {arg}")),
                _ => opts.filters.push(arg.clone()),
            }
        }
        Ok(opts)
    }

    /// 照模块表把 filters 里的模块名认出来
    fn settle(&mut self, modules: &[Module]) {
        self.exact = self.filters.iter().filter(|f| modules.iter().any(|m| m.name == f.as_str())).cloned().collect();
    }

    /// 这个模块跑不跑（不看用例）
    fn wants_module(&self, module: &Module) -> bool {
        let named = self.modules.iter().chain(&self.filters).any(|n| n == module.name);
        (self.modules.is_empty() || self.modules.iter().any(|n| n == module.name)) && (module.default || self.all || named)
    }

    /// 这个用例跑不跑
    fn wants(&self, module: &Module, test: &Test) -> bool {
        let full = format!("{}::{}", module.name, test.name);
        let hit = |f: &String| if self.exact.contains(f) { f == module.name } else { full.contains(f.as_str()) };
        self.wants_module(module)
            && (self.filters.is_empty() || self.filters.iter().any(hit))
            && !self.skips.iter().any(|s| full.contains(s.as_str()))
    }

    /// 这个模块里要跑的用例，照表里的先后
    fn pick<'a>(&self, module: &'a Module) -> Vec<&'a Test> {
        module.tests.iter().filter(|t| self.wants(module, t)).collect()
    }
}

/// 模块的原文里像用例的函数（顶格写的 `fn 名字(t: &mut Ctx) {`，参数也可以叫 _t、_）有没有都登记了，表里有没有重名的。
/// 有毛病的话给出说明。带别的参数、有返回值的函数不算用例（夹具、帮手照常写）
fn audit(module: &Module) -> Option<String> {
    let rx =
        regex::Regex::new(r"(?m)^(?:pub(?:\([a-z]+\))? )?fn ([A-Za-z0-9_]+)\((?:mut )?(?:t|_t|_): &mut (?:[a-z_]+::)*Ctx\) \{")
            .expect("写死的正则");
    let listed: Vec<&str> = module.tests.iter().map(|t| t.name).collect();
    let missing: Vec<&str> =
        rx.captures_iter(module.src).map(|c| c.get(1).expect("有这一组").as_str()).filter(|n| !listed.contains(n)).collect();
    let mut twice: Vec<&str> = listed.iter().enumerate().filter(|(i, n)| listed[..*i].contains(n)).map(|(_, n)| *n).collect();
    twice.dedup();
    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!("这几个用例没登记在 {} 的 tests 里：{}", module.name, missing.join("、")));
    }
    if !twice.is_empty() {
        problems.push(format!("{} 的 tests 里登记了两次：{}", module.name, twice.join("、")));
    }
    (!problems.is_empty()).then(|| problems.join("\n"))
}

/// tests/suite.rs 的 main：`kit::harness::main(env!("CARGO_BIN_EXE_cca-test"), &[模块::MODULE, …])`。
/// tool 是和这个程序一起编出来的 cca-test（环境里有 `CCA_TEST_BIN` 就用那一份）。所有选中的模块都通过才退出码 0
pub fn main(tool: &str, modules: &[Module]) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = match Opts::parse(&args) {
        Ok(opts) => opts,
        Err(why) => {
            eprintln!("suite：{why}\n{USAGE}");
            std::process::exit(2);
        }
    };
    match std::env::var_os("CCA_TEST_BIN") {
        Some(path) if !path.is_empty() => crate::proc::set_tool(PathBuf::from(path)),
        _ => crate::proc::set_tool(tool),
    }
    let modules = if opts.fixtures { fixtures::MODULES } else { modules };
    opts.settle(modules);
    let code = match &opts.child {
        Some(name) => child::run(&opts, modules, name),
        None => parent::run(&opts, modules, &args),
    };
    std::process::exit(code)
}

/// 收到的信号（0：没有）。处理函数只记下来，主循环 / 看门的线程自己来看
static SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    SIGNAL.store(sig, std::sync::atomic::Ordering::SeqCst);
}

fn signalled() -> i32 {
    SIGNAL.load(std::sync::atomic::Ordering::SeqCst)
}

/// INT、TERM、HUP 来了先收场再退出（本来就设成不理的照旧不理：后台起的任务不理 INT）
fn catch_signals() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: 两个 sigaction 清零后是有效的；处理函数只写一个原子变量，可以在信号里做
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut old);
            if old.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            let mut new: libc::sigaction = std::mem::zeroed();
            new.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            new.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut new.sa_mask);
            libc::sigaction(sig, &new, std::ptr::null_mut());
        }
    }
}

/// 秒，两位小数
fn secs(d: std::time::Duration) -> String {
    format!("{:.2}", d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nop(_t: &mut Ctx) {}

    const A: Module =
        Module { name: "team", src: "", default: true, tests: &[Test::new("join", nop), Test::new("leave_team_relay", nop)] };
    const B: Module = Module { name: "team_relay", src: "", default: true, tests: &[Test::new("join", nop)] };
    const C: Module = Module { name: "chrome", src: "", default: false, tests: &[Test::new("real", nop)] };

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn picked(list: &[&str]) -> Vec<String> {
        let mut opts = Opts::parse(&args(list)).unwrap();
        opts.settle(&[A, B, C]);
        [A, B, C]
            .iter()
            .flat_map(|m| opts.pick(m).into_iter().map(|t| format!("{}::{}", m.name, t.name)).collect::<Vec<_>>())
            .collect()
    }

    #[test]
    fn command_line() {
        let opts = Opts::parse(&args(&[
            "team",
            "--module",
            "a",
            "--module=b",
            "x::y",
            "--jobs",
            "3",
            "--results",
            "r.tsv",
            "--logs=logs",
            "--setsid",
            "--list",
            "--all",
            "--skip",
            "slow",
            "-q",
            "--nocapture",
        ]))
        .unwrap();
        assert_eq!(
            opts,
            Opts {
                filters: args(&["team", "x::y"]),
                skips: args(&["slow"]),
                modules: args(&["a", "b"]),
                jobs: Some(3),
                results: Some("r.tsv".into()),
                logs: Some("logs".into()),
                setsid: true,
                list: true,
                all: true,
                nocapture: true,
                quiet: true,
                ..Opts::default()
            }
        );
        // libtest 的开关：认下来、不管；--test-threads 就是 --jobs
        let opts = Opts::parse(&args(&[
            "--test-threads",
            "2",
            "--color",
            "always",
            "--format=terse",
            "--exact",
            "--show-output",
            "-Z",
            "unstable-options",
            "name",
        ]))
        .unwrap();
        assert_eq!((opts.jobs, opts.filters), (Some(2), args(&["name"])));
        assert_eq!(Opts::parse(&args(&["--test-threads=7"])).unwrap().jobs, Some(7));
        for (bad, why) in [
            (&["--bogus"][..], "不认识 --bogus"),
            (&["--jobs"], "--jobs 后面要跟同时跑几个模块"),
            (&["--jobs", "0"], "--jobs 要一个不小于 1 的数，不是 0"),
            (&["--jobs=many"], "--jobs 要一个不小于 1 的数，不是 many"),
            (&["--module"], "--module 后面要跟模块名"),
            (&["-x"], "不认识 -x"),
        ] {
            assert_eq!(Opts::parse(&args(bad)).unwrap_err(), why);
        }
    }

    #[test]
    fn which_tests_run() {
        let all_default = ["team::join", "team::leave_team_relay", "team_relay::join"];
        assert_eq!(picked(&[]), all_default);
        // 模块名：正好是这个模块，不连带名字里有它的
        assert_eq!(picked(&["team"]), ["team::join", "team::leave_team_relay"]);
        assert_eq!(picked(&["team_relay"]), ["team_relay::join"]);
        // 不是模块名的：照 模块::用例 里的一截匹配
        assert_eq!(picked(&["relay"]), ["team::leave_team_relay", "team_relay::join"]);
        assert_eq!(picked(&["--module", "team_relay"]), ["team_relay::join"]);
        assert_eq!(picked(&["--module", "team", "--module", "team_relay", "join"]), ["team::join", "team_relay::join"]);
        // 模块::用例 的一截；几个里中一个就算
        assert_eq!(picked(&["team::jo"]), ["team::join"]);
        assert_eq!(picked(&["relay::join", "team::leave"]), ["team::leave_team_relay", "team_relay::join"]);
        assert_eq!(picked(&["nothing-matches"]), Vec::<String>::new());
        // --skip
        assert_eq!(picked(&["--skip", "relay"]), ["team::join"]);
        assert_eq!(picked(&["team", "--skip", "team::join"]), ["team::leave_team_relay"]);
        // 平时不跑的模块：--all、--module、写模块名才跑；照一截匹配到不算
        assert_eq!(picked(&["real"]), Vec::<String>::new());
        assert_eq!(picked(&["--all"]), [&all_default[..], &["chrome::real"]].concat());
        assert_eq!(picked(&["--all", "real"]), ["chrome::real"]);
        assert_eq!(picked(&["--module", "chrome"]), ["chrome::real"]);
        assert_eq!(picked(&["chrome"]), ["chrome::real"]);
    }

    #[test]
    fn unregistered_tests_are_found() {
        let src = "use kit::harness::Ctx;\n\
            fn listed(t: &mut Ctx) {\n}\n\
            fn orphan(t: &mut Ctx) {\n}\n\
            pub fn also_orphan(_t: &mut Ctx) {\n}\n\
            fn third(_: &mut harness::Ctx) {\n}\n\
            fn fixture(t: &mut Ctx) -> PathBuf {\n}\n\
            fn helper(t: &mut Ctx, n: usize) {\n}\n\
            \x20   fn nested(t: &mut Ctx) {\n}\n\
            // fn commented(t: &mut Ctx) {\n";
        let module = |tests| Module { name: "m", src, default: true, tests };
        const ONE: &[Test] = &[Test::new("listed", nop)];
        assert_eq!(audit(&module(ONE)).unwrap(), "这几个用例没登记在 m 的 tests 里：orphan、also_orphan、third");
        const FULL: &[Test] =
            &[Test::new("listed", nop), Test::new("orphan", nop), Test::new("also_orphan", nop), Test::new("third", nop)];
        assert_eq!(audit(&module(FULL)), None);
        const TWICE: &[Test] = &[
            Test::new("listed", nop),
            Test::new("orphan", nop),
            Test::new("also_orphan", nop),
            Test::new("third", nop),
            Test::new("listed", nop),
        ];
        assert_eq!(audit(&module(TWICE)).unwrap(), "m 的 tests 里登记了两次：listed");
    }

    #[test]
    fn test_table_helpers() {
        let t = crate::case!(nop);
        assert_eq!((t.name, t.timeout_s, t.needs.len()), ("nop", DEFAULT_TIMEOUT_S, 0));
        let t = crate::case!(nop).timeout(7).needs(&[Need::Bin("sh"), Need::MacOs]);
        assert_eq!((t.name, t.timeout_s, t.needs.len()), ("nop", 7, 2));
        assert_eq!(Need::Bin("sh").unmet(), None);
        assert_eq!(Need::Bin("cca-kit-no-such-command").unmet().unwrap(), "没有 cca-kit-no-such-command");
        assert_eq!(Need::MacOs.unmet().is_none(), cfg!(target_os = "macos"));
        assert_eq!(Need::Linux.unmet().is_none(), cfg!(target_os = "linux"));
    }
}
