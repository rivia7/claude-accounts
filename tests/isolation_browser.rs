//! 浏览器、登录的启动边界：从 cca 里切出 open_chrome、codex_login 这些函数，系统命令和别的函数换成替身来跑，从不真的打开程序
//! （原来的 tests/isolation_browser.py）。

use kit::case;
use kit::cca_src;
use kit::fsx;
use kit::harness::{Ctx, Module};
use kit::proc::Output;
use std::path::PathBuf;
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_browser",
    src: include_str!("isolation_browser.rs"),
    default: true,
    tests: &[
        case!(missing_chrome_never_opens_default_browser),
        case!(browser_receives_private_directory_and_account_proxy),
        case!(existing_browser_requires_matching_record_before_opening_url),
        case!(existing_verified_browser_reuses_same_profile),
        case!(new_browser_receipt_failure_never_opens_target_url),
        case!(browser_open_failure_is_propagated),
        case!(browser_address_cannot_override_launch_flags),
        case!(unsupported_codex_login_does_not_use_cli_fallback),
        case!(login_without_chrome_stops_before_authentication),
        case!(web_does_not_skip_preflight_or_open_after_failure),
        case!(web_does_not_report_success_after_open_failure),
    ],
};

/// 切出来的函数前面接的替身
const PROLOGUE: &str = r#"
fail_warn() { printf 'FAIL:%s:%s\n' "$1" "$2" >&2; }
info() { printf 'INFO:%s\n' "$*"; }
gui_result() { printf 'RESULT:%s\n' "$*"; }
gui_step() { :; }
die() { echo "$*" >&2; exit 1; }
chrome_app_path() { echo '/dummy/Google Chrome.app'; }
chrome_launch_lock() { return 0; }
chrome_pid() { [ ! -f opened ] || echo 101; }
app_profile_in_use() { return 0; }
lsof() { :; }
py() {
 case "$1" in
  private-profile) mkdir -p "$ACC_CHROME_DIR"; chmod 700 "$ACC_CHROME_DIR"; echo "$ACC_CHROME_DIR" ;;
  runtime-record) printf '%s\n' "$2" >> records ;;
 esac
}
"#;

/// 每个用例一个临时目录：HOME、当前目录、账号目录都是它
struct Fx {
    root: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    Fx { root: t.tmp("cca-browser-test-") }
}

impl Fx {
    /// 替身 + 切出来的这几个函数 + code，交给 bash -c。环境只有这几个变量（原来还带着 PYTHON：切出来的函数和替身都不用它）
    fn run_shell(&self, functions: &[&str], code: &str) -> Output {
        let mut script = PROLOGUE.to_string();
        if functions.contains(&"open_chrome") {
            script += &(cca_src::function("chrome_open_url") + "\n");
        }
        script += &format!("{}\n{code}", cca_src::functions(functions));
        let root = self.root.display().to_string();
        kit::bash::run(
            kit::bash::c(&script)
                .cwd(&self.root)
                .env("HOME", &root)
                .env("PATH", "/usr/bin:/bin")
                .env("ACC_CHROME_DIR", self.root.join("browser"))
                .env("ACC_DIR", &root)
                .env("ACC_KEY", "alice@example.com")
                .env("ACC_PROXY", "http://192.0.2.10:7901")
                .timeout(Duration::from_secs(10)),
        )
    }

    fn exists(&self, name: &str) -> bool {
        self.root.join(name).exists()
    }

    fn read(&self, name: &str) -> String {
        fsx::read_text(self.root.join(name))
    }
}

fn missing_chrome_never_opens_default_browser(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 1; }
open() { touch unexpected-open; }
open_chrome https://example.com
"#,
    );
    assert_ne!(r.status.py(), 0);
    assert!(r.err().contains("no_chrome"), "{}", r.err());
    assert!(!fx.exists("unexpected-open"));
    assert!(!fx.exists("browser"));
}

fn browser_receives_private_directory_and_account_proxy(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
open() { printf '%s\n' "$@" > open-args; touch opened; }
open_chrome 'https://example.com/?a=b&c=d'
"#,
    );
    assert_eq!(r.status.py(), 0, "{}", r.err());
    let text = fx.read("open-args");
    let args: Vec<&str> = text.lines().collect();
    let profile = format!("--user-data-dir={}", fx.root.join("browser").display());
    assert!(args.contains(&profile.as_str()), "{args:?}");
    assert!(args.contains(&"--proxy-server=http://192.0.2.10:7901"), "{args:?}");
    assert!(args.contains(&"--webrtc-ip-handling-policy=disable_non_proxied_udp"), "{args:?}");
    assert_eq!(args.last(), Some(&"https://example.com/?a=b&c=d"));
    assert_eq!(fsx::mode(fx.root.join("browser")) & 0o777, 0o700);
}

fn existing_browser_requires_matching_record_before_opening_url(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
chrome_pid() { echo 101; }
py() { [ "$1" != private-profile ] || { echo "$ACC_CHROME_DIR"; return; }; return 1; }
open() { touch unexpected-open; }
open_chrome https://example.com
"#,
    );
    assert_ne!(r.status.py(), 0);
    assert!(!fx.exists("unexpected-open"));
    assert!(r.err().contains("退出"), "{}", r.err());
}

fn existing_verified_browser_reuses_same_profile(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
chrome_pid() { echo 101; }
open() { printf '%s\n' "$@" > open-args; }
open_chrome https://example.com
"#,
    );
    assert_eq!(r.status.py(), 0, "{}", r.err());
    assert_eq!(fx.read("records").trim(), "check");
    assert!(!fx.read("open-args").contains("about:blank"));
}

fn new_browser_receipt_failure_never_opens_target_url(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
py() {
 case "$1" in
  private-profile) echo "$ACC_CHROME_DIR" ;;
  browser-profile) return 0 ;;
  *) return 1 ;;
 esac
}
open() { printf '%s\n' "$@" >> open-args; touch opened; }
open_chrome https://example.com
"#,
    );
    assert_ne!(r.status.py(), 0);
    let args = fx.read("open-args");
    assert!(args.contains("about:blank"), "{args}");
    assert!(!args.contains("https://example.com"), "{args}");
}

fn browser_open_failure_is_propagated(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
open() { return 7; }
open_chrome https://example.com
"#,
    );
    assert_eq!(r.status.py(), 7);
}

fn browser_address_cannot_override_launch_flags(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["open_chrome"],
        r#"
has_chrome() { return 0; }
open() { touch unexpected-open; }
open_chrome '--proxy-server=http://example.invalid:8000'
"#,
    );
    assert_ne!(r.status.py(), 0);
    assert!(r.err().contains("invalid_url"), "{}", r.err());
    assert!(!fx.exists("unexpected-open"));
}

fn unsupported_codex_login_does_not_use_cli_fallback(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["codex_login"],
        r#"
has_chrome() { return 0; }
child_env() { :; }
pyenv() { return 3; }
CODEX_BIN=/usr/bin/touch
ACC_FLAGS=(unexpected-cli-login)
codex_login
"#,
    );
    assert_ne!(r.status.py(), 0);
    assert!(r.err().contains("login_unsupported"), "{}", r.err());
    assert!(!fx.exists("unexpected-cli-login"));
}

fn login_without_chrome_stops_before_authentication(t: &mut Ctx) {
    let fx = fixture(t);
    for name in ["claude_login", "codex_login"] {
        t.sub(format!("name={name}"), |_| {
            let r = fx.run_shell(
                &[name],
                &(r#"
has_chrome() { return 1; }
child_env() { touch unexpected-auth; }
"#
                .to_string()
                    + name),
            );
            assert_ne!(r.status.py(), 0);
            assert!(!fx.exists("unexpected-auth"));
        });
    }
}

/// cmd_web 用到的替身
const WEB_SETUP: &str = r#"
pick_account() { echo alice@example.com; }
py() { echo 'ACC_KIND=official ACC_KEY=alice@example.com ACC_NAME=alice ACC_EMAIL=alice@example.com ACC_NODE=US-A'; }
has_chrome() { return 0; }
sync_if_changed() { :; }
prepare_account() { :; }
interactive() { return 1; }
TRACE_HOST=example.com
TOOL_BIN=unused
"#;

fn web_does_not_skip_preflight_or_open_after_failure(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["cmd_web"],
        &(WEB_SETUP.to_string()
            + r#"
preflight() { printf '%s:%s' "${CCA_SKIP_CHECK:-unset}" "$1" > preflight; return 1; }
open_chrome() { touch unexpected-open; }
cmd_web alice
"#),
    );
    assert_ne!(r.status.py(), 0);
    assert_eq!(fx.read("preflight"), "unset:app");
    assert!(!fx.exists("unexpected-open"));
}

fn web_does_not_report_success_after_open_failure(t: &mut Ctx) {
    let fx = fixture(t);
    let r = fx.run_shell(
        &["cmd_web"],
        &(WEB_SETUP.to_string()
            + r#"
preflight() { return 0; }
open_chrome() { return 7; }
cmd_web alice
"#),
    );
    assert_ne!(r.status.py(), 0);
    assert!(!r.out().contains("RESULT:opened"), "{}", r.out());
}
