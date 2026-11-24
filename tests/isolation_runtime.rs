//! 客户端的运行记录（cca-core runtime-record）：临时 HOME、假的 ps，不碰真实的账号和进程（原来的 tests/isolation_runtime.py）。
//!
//! 辅助程序问 ps 要进程的启动时间和命令行；这里给它一个假的 ps，照沙箱里的两个文件回答（用例改 `started`、`command`
//! 就是改这两个文件）。账号表、config.json 是沙箱里真的文件。只在 macOS 上跑：账号专属的浏览器和桌面 App 只有 macOS 有，
//! Linux 上进程的启动时间也不问 ps（读 /proc）。

use kit::case;
use kit::fsx;
use kit::harness::{self, Ctx, Module, Need};
use kit::json::{self, Value};
use kit::plist;
use kit::proc::{Cmd, Output};
use kit::{list, obj};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_runtime",
    src: include_str!("isolation_runtime.rs"),
    default: true,
    tests: &[
        case!(valid_receipts_for_both_clients_are_private).needs(&[Need::MacOs]),
        case!(old_process_without_receipt_is_not_adopted).needs(&[Need::MacOs]),
        case!(process_restart_and_pid_reuse_are_rejected).needs(&[Need::MacOs]),
        case!(changed_proxy_node_route_or_binary_requires_restart).needs(&[Need::MacOs]),
        case!(command_must_match_actual_proxy_and_privacy_flags).needs(&[Need::MacOs]),
        case!(process_must_belong_to_the_selected_installation).needs(&[Need::MacOs]),
        case!(webrtc_url_exceptions_are_removed_before_launch_and_rejected_live).needs(&[Need::MacOs]),
        case!(symlink_profile_is_rejected_without_touching_target).needs(&[Need::MacOs]),
        case!(linked_receipt_is_not_trusted_or_written_through).needs(&[Need::MacOs]),
        case!(profile_path_cannot_be_replaced_by_another_account).needs(&[Need::MacOs]),
    ],
};

/// 假的 ps：-o pid=,lstart= 给启动时间，-o pid=,command= 给命令行（文件不在就是没有这个进程）
const PS: &str = r#"#!/bin/sh
case "$*" in
  *lstart*) cat "$HOME/ps-lstart" 2>/dev/null ;;
  *command*) cat "$HOME/ps-command" 2>/dev/null ;;
  *) printf '%s\n' "$0 $*" >> "$HOME/unexpected-commands.log"; exit 127 ;;
esac
exit 0
"#;

/// 会动到这台机器、会联网的命令：沙箱的 PATH 最前面放只会报错的替身（运行记录只该问 ps）
const GUARDED: &[&str] = &[
    "security",
    "osascript",
    "open",
    "pbcopy",
    "launchctl",
    "ssh",
    "scp",
    "curl",
    "git",
    "nc",
    "crontab",
    "defaults",
    "killall",
    "pkill",
    "sudo",
    "lsappinfo",
    "networksetup",
    "scutil",
    "codesign",
    "spctl",
    "hdiutil",
    "ditto",
    "claude",
    "codex",
    "agy",
    "mihomo",
];

const CHANGED: &str = "客户端的运行记录、账号节点或启动条件已变化";

/// 一个用例的沙箱和假的进程 123
struct Fx {
    root: PathBuf,
    key: &'static str,
    /// 账号选的那个浏览器（沙箱里一个假的 .app）
    app: PathBuf,
    /// 进程 123 的启动时间（秒）；0：这个进程已经不在了
    started: i64,
    /// 进程 123 的命令行
    command: String,
    /// runtime-record 的参数（launch 给的）
    args: Vec<String>,
    /// 账号目录：资料目录的上一层，在沙箱的 ~/.cca/accounts 里
    account: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    let root = std::fs::canonicalize(t.tmp("cca-runtime-")).unwrap();
    std::fs::create_dir(root.join(".cca")).unwrap();
    let bin = root.join("bin");
    fsx::write_script(bin.join("ps"), PS);
    kit::bash::guard(&bin, GUARDED);
    kit::bash::no_stray(t, &root);
    fsx::write(root.join(".cca/config.json"), dumps(&obj! { "version" => 2, "nodes" => list!["US-A", "US-B"] }));
    let app = root.join("Test Browser.app");
    let mut fx = Fx {
        root,
        key: "alice@example.com",
        app,
        started: 1234567890,
        command: String::new(),
        args: Vec::new(),
        account: PathBuf::new(),
    };
    fx.node("US-A");
    fsx::write(fx.app.join("Contents/MacOS/Test"), "test binary");
    info_plist(&fx.app, &[("CFBundleExecutable", "Test")]);
    fx.args = fx.launch("browser");
    fx.account = Path::new(&fx.args[3]).parent().expect("资料目录有上一层").to_path_buf();
    assert_eq!(fx.account.parent(), Some(fx.root.join(".cca/accounts").as_path()));
    fx
}

/// 写成 Python 的 json.dumps 那样（默认写法）
fn dumps(v: &Value) -> String {
    json::dumps(v, &json::Style::default())
}

/// 程序的 Info.plist（plistlib.dumps：XML）
fn info_plist(app: &Path, entries: &[(&str, &str)]) {
    let dict = plist::Value::Dict(entries.iter().map(|(k, v)| (k.to_string(), plist::Value::Str(v.to_string()))).collect());
    fsx::write(app.join("Contents/Info.plist"), plist::dumps_xml(&dict).expect("写得出 plist"));
}

/// time.strftime('%a %b %d %H:%M:%S %Y', time.localtime(秒))：ps 的 lstart 那一栏（本地时间；这个进程没设过语言环境，
/// 星期、月份是 C 的写法）
fn lstart(secs: i64) -> String {
    let when = secs as libc::time_t;
    // SAFETY: tm 是只有整数和指针的普通结构，全零是合法的值；localtime_r（头一次用时自己读时区）、strftime 只写给它们的那一份
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    assert!(!unsafe { libc::localtime_r(&when, &mut tm) }.is_null(), "localtime_r 失败");
    let mut buf = [0u8; 64];
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%a %b %d %H:%M:%S %Y".as_ptr(), &tm) };
    assert!(n > 0, "strftime 什么都没写");
    String::from_utf8(buf[..n].to_vec()).expect("C 的语言环境下是 ASCII")
}

impl Fx {
    /// 账号表里这个账号走哪个节点
    fn node(&self, name: &str) {
        let mut table = json::Obj::new();
        table.insert(self.key, name);
        fsx::write(self.root.join(".cca/accounts.json"), dumps(&Value::from(table)));
    }

    fn helper(&self, argv: &[&str]) -> Output {
        // 进程 123 现在的样子：没有启动时间就是这个进程已经不在了
        let lstart = if self.started != 0 { format!("  123 {}\n", lstart(self.started)) } else { String::new() };
        fsx::write(self.root.join("ps-lstart"), lstart);
        fsx::write(self.root.join("ps-command"), format!("  123 {}\n", self.command));
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(argv)
            .env("HOME", &self.root)
            .env("CCA_HOME", self.root.join(".cca"))
            .env("CCA_TOOL", "claude")
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("bin").display()))
            .timeout(Duration::from_secs(30));
        kit::bash::run(&mut cmd)
    }

    fn done(&self, argv: &[&str]) -> String {
        let r = self.helper(argv);
        assert_eq!((r.status.py(), r.err()), (0, String::new()));
        r.out()
    }

    /// 起这个账号的客户端（app 或 browser）：准备资料目录，进程 123 的命令行照 cca 起它的样子；给出 runtime-record 的参数
    fn launch(&mut self, kind: &str) -> Vec<String> {
        let profile = self.done(&["private-profile", self.key, kind]).trim_end_matches('\n').to_string();
        if kind == "browser" {
            self.done(&["browser-profile", "prepare", self.key]);
        }
        let proxy = "http://192.0.2.10:7901";
        self.command = format!("{}/Contents/MacOS/Test --user-data-dir={profile}", self.app.display());
        self.command += &format!(" --proxy-server={proxy}");
        if kind == "app" {
            self.command += " --proxy-bypass-list=localhost";
        } else {
            self.command += " --webrtc-ip-handling-policy=disable_non_proxied_udp";
        }
        let app = self.app.display().to_string();
        [self.key, kind, "123", &profile, &app, proxy, "router", "localhost", "localhost"].map(str::to_string).to_vec()
    }

    /// runtime-record 这个动作的参数（没给 args 就用 launch 给的）
    fn argv<'a>(&'a self, action: &'a str, args: Option<&'a [String]>) -> Vec<&'a str> {
        let mut argv = vec!["runtime-record", action];
        argv.extend(args.unwrap_or(&self.args).iter().map(String::as_str));
        argv
    }

    fn record(&self, action: &str, args: Option<&[String]>) {
        assert_eq!(self.done(&self.argv(action, args)), "");
    }

    /// 拒绝：一句说明（why 是其中的一段）、退出码 1；不是参数不对、内部出错这类
    fn refused(&self, action: &str, args: Option<&[String]>, why: &str) {
        let r = self.helper(&self.argv(action, args));
        let err = r.err();
        assert_eq!((r.status.py(), r.out()), (1, String::new()), "{err}");
        assert!(err.contains(why), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
        assert!(!err.contains("cca-core"), "{err}");
    }
}

fn valid_receipts_for_both_clients_are_private(t: &mut Ctx) {
    let mut fx = fixture(t);
    for kind in ["app", "browser"] {
        fx.args = fx.launch(kind);
        fx.record("write", None);
        fx.record("check", None);
        let p = fx.account.join(format!("{kind}-runtime.json"));
        assert_eq!(fsx::mode(&p) & 0o777, 0o600);
        assert!(fsx::read_json(&p).get("command").is_none());
    }
}

fn old_process_without_receipt_is_not_adopted(t: &mut Ctx) {
    let fx = fixture(t);
    fx.refused("check", None, CHANGED);
}

fn process_restart_and_pid_reuse_are_rejected(t: &mut Ctx) {
    let mut fx = fixture(t);
    fx.record("write", None);
    fx.started += 1;
    fx.refused("check", None, CHANGED);
    fx.started = 0;
    fx.refused("check", None, "无法核验客户端进程或账号代理");
}

fn changed_proxy_node_route_or_binary_requires_restart(t: &mut Ctx) {
    let fx = fixture(t);
    fx.record("write", None);
    fx.node("US-B");
    fx.refused("check", None, CHANGED);
    fx.node("US-A");
    for (index, value) in [(5, "http://127.0.0.1:7901"), (6, "local"), (7, "*"), (8, "*")] {
        let mut args = fx.args.clone();
        args[index] = value.to_string();
        // 代理换了：进程的命令行里还是原来的；别的几项不在命令行里，和记下的对不上
        fx.refused("check", Some(&args), if index == 5 { "客户端启动参数无法核验：--proxy-server" } else { CHANGED });
    }
    // App 更新了（Info.plist 照样读得出来，只是大小、修改时间变了）：和记下的版本对不上。这一步是改写时加的：
    // 下面那一步写进去的不是 plist，走的是「读不出版本」，原来没有哪一步走到「版本变了」
    info_plist(&fx.app, &[("CFBundleExecutable", "Test"), ("CFBundleVersion", "2")]);
    fx.refused("check", None, CHANGED);
    fsx::write(fx.app.join("Contents/Info.plist"), "new app version and build");
    fx.refused("check", None, "无法核验客户端程序版本");
}

fn command_must_match_actual_proxy_and_privacy_flags(t: &mut Ctx) {
    let mut fx = fixture(t);
    let valid = fx.command.clone();
    for (command, flag) in [
        (valid.replace(":7901", ":7902"), "proxy-server"),
        (format!("{valid} --proxy-server=http://127.0.0.1:7902"), "proxy-server"),
        (valid.replace(" --webrtc-ip-handling-policy=disable_non_proxied_udp", ""), "webrtc-ip-handling-policy"),
        (String::new(), "user-data-dir"),
    ] {
        fx.command = command;
        fx.refused("write", None, &format!("客户端启动参数无法核验：--{flag}；"));
    }
    fx.command = valid;
    fx.record("write", None);
}

fn process_must_belong_to_the_selected_installation(t: &mut Ctx) {
    let mut fx = fixture(t);
    fx.command = fx.command.replace(&fx.app.display().to_string(), "/another-installation/Test Browser.app");
    fx.refused("write", None, "客户端进程不属于当前选择的程序");
}

fn webrtc_url_exceptions_are_removed_before_launch_and_rejected_live(t: &mut Ctx) {
    let fx = fixture(t);
    let path = Path::new(&fx.args[3]).join("Default/Preferences");
    let mut data = fsx::read_json(&path);
    let prefs = data.as_obj_mut().expect("Preferences 是对象");
    prefs.insert("unrelated", obj! { "keep" => true });
    let webrtc = prefs.get_mut("webrtc").and_then(Value::as_obj_mut).expect("Preferences 里有 webrtc 这一节");
    webrtc.insert("ip_handling_url", list![obj! { "url" => "*", "handling" => "default" }]);
    fsx::write(&path, dumps(&data));
    fx.refused("write", None, "浏览器 WebRTC 偏好已变化或有 URL 例外");
    fx.done(&["browser-profile", "prepare", fx.key]);
    let data = fsx::read_json(&path);
    assert_eq!(data["webrtc"]["ip_handling_url"], Value::List(Vec::new()));
    assert_eq!(data["unrelated"], obj! { "keep" => true });
    fx.record("write", None);
}

fn symlink_profile_is_rejected_without_touching_target(t: &mut Ctx) {
    let fx = fixture(t);
    let profile = PathBuf::from(&fx.args[3]);
    std::fs::remove_file(profile.join("Default/Preferences")).unwrap();
    std::fs::remove_dir(profile.join("Default")).unwrap();
    std::fs::remove_dir(&profile).unwrap();
    let other = fx.root.join("other-account");
    std::fs::DirBuilder::new().mode(0o755).create(&other).unwrap();
    std::os::unix::fs::symlink(&other, &profile).unwrap();
    fx.refused("write", None, &format!("客户端资料目录不能是符号链接：{}", profile.display()));
    assert_eq!(fsx::mode(&other) & 0o777, 0o755);
}

fn linked_receipt_is_not_trusted_or_written_through(t: &mut Ctx) {
    let fx = fixture(t);
    fx.record("write", None);
    let receipt = fx.account.join("browser-runtime.json");
    let other = fx.root.join("shared-record.json");
    std::fs::rename(&receipt, &other).unwrap();
    std::os::unix::fs::symlink(&other, &receipt).unwrap();
    let before = fsx::read(&other);
    fx.refused("check", None, CHANGED);
    fx.record("write", None);
    assert!(!receipt.is_symlink());
    assert_eq!(fsx::read(&other), before);
    std::fs::remove_file(&receipt).unwrap();
    std::fs::hard_link(&other, &receipt).unwrap();
    fx.refused("check", None, CHANGED);
}

fn profile_path_cannot_be_replaced_by_another_account(t: &mut Ctx) {
    let fx = fixture(t);
    let other = fx.root.join("another-profile");
    std::fs::create_dir(&other).unwrap();
    let mut args = fx.args.clone();
    args[3] = other.display().to_string();
    fx.refused("write", Some(&args), "客户端资料目录与账号不符");
}
