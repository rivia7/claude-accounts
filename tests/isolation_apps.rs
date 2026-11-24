//! 桌面 App 启动的隔离：假的 App、环境、进程和资料目录。从 cca 里切出起 App、查进程、加锁的那几段来跑，系统命令换成替身
//! （原来的 tests/isolation_apps.py）。切出来的 bash 里要用 cca 的辅助程序（"$HELPER" sh-flock、sh-ps-tree 这些）：
//! 用的是 kit::harness::helper() 那一份。

use kit::case;
use kit::cca_src;
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module};
use kit::json;
use kit::proc::{Child, Cmd, Exit, Output, Stdio};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "isolation_apps",
    src: include_str!("isolation_apps.rs"),
    default: true,
    tests: &[
        case!(desktop_filters_inherited_credentials_for_both_tools),
        case!(proxy_fallback_unknown_or_declined_stops_before_login),
        case!(profile_process_lookup_accepts_both_flag_forms_without_prefix_matches),
        case!(process_lookup_preserves_errors_and_rejects_empty_success),
        case!(destructive_commands_stop_when_process_query_fails),
        case!(uninstall_stops_when_browser_process_query_fails),
        case!(browser_process_query_failure_never_prepares_or_opens_profile),
        case!(browser_query_failure_after_launch_does_not_deliver_target_url),
        case!(app_process_query_failure_stops_each_launch_stage),
        case!(stale_profile_is_not_evidence_but_descendant_open_file_is),
        case!(lock_serializes_launches_and_releases_on_process_exit),
        case!(command_rejects_stale_profile_and_closes_launch_fd),
        case!(existing_instance_is_verified_without_ever_killing_it),
        case!(interrupted_new_launch_is_cleaned_before_pid_is_recorded),
        case!(cleanup_excludes_all_processes_present_before_launch),
        case!(actual_existing_process_holding_profile_file_is_accepted),
    ],
};

/// 等一个文件出现的时限（原来是 5 秒；只在出毛病时才等得满，给宽一点）
const WAIT: Duration = Duration::from_secs(30);

// —— 从 cca 里切的几段（和原来一样照 SOURCE.index 切）——

/// 继承变量的名单和 child_env、claude_env 这些
fn env_section() -> String {
    cca_src::between("INHERITED_RE='^", "\npyenv()")
}
/// 起桌面 App 的那几个函数
fn app_open() -> String {
    cca_src::between("app_open() {", "\nclaude_app_first()")
}
/// codex_app_prepare
fn app_prepare() -> String {
    cca_src::between("codex_app_prepare() {", "\napp_open() {")
}
/// 启动锁、进程树、资料目录在不在用、启动后的清理
fn app_checks() -> String {
    cca_src::between("app_launch_lock() {", "\ncmd_app() {")
}
/// cmd_app 到下一节的分隔线
fn app_command() -> String {
    cca_src::between_after("cmd_app() {", "\n# ---------------------------------------------------------------------------")
}
/// app_pid（查资料目录对应的进程）
fn app_pid() -> String {
    cca_src::between("app_pid() {", "\nclaude_app_pid()")
}
/// 账号专属的 Chrome：chrome_pid、加锁、open_chrome
fn chrome() -> String {
    cca_src::between("chrome_pid() {", "\ncmd_web() {")
}
/// cmd_rm
fn remove() -> String {
    cca_src::between("cmd_rm() {", "\ncmd_list() {")
}
/// cmd_restore（卸载、还原）
fn restore() -> String {
    cca_src::between("cmd_restore() {", "\nusage() {")
}

/// cmd_app 里的 py 子命令的替身：几个退出码由环境变量定
const PY_COMMAND_STUB: &str = r#"py() {
 case "$1" in
  env) echo 'ACC_NAME=test ACC_LABEL=test ACC_NODE=test' ;;
  private-profile) [ "${PROFILE_RC:-0}" = 0 ] || return "$PROFILE_RC"; echo "$AUDIT_ROOT/desktop" ;;
  runtime-record)
   printf '%s\n' "$*" >> "$AUDIT_ROOT/record.log"
   if [ "$2" = write ]; then return "${RECORD_WRITE_RC:-0}"; else return "${RECORD_CHECK_RC:-1}"; fi ;;
  codex-fallback-off) return "${FALLBACK_OFF_RC:-0}" ;;
  *) echo "unexpected py command: $1" >&2; return 9 ;;
 esac
}
"#;

/// 一个用例的沙箱：HOME、账号目录是临时目录，替身放在 bin 里，账号的 App 资料目录是 desktop
struct Fx {
    root: PathBuf,
    bin: PathBuf,
    profile: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    let root = t.tmp("cca-app-isolation-");
    let (bin, profile) = (root.join("bin"), root.join("desktop"));
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&profile).unwrap();
    Fx { root, bin, profile }
}

impl Fx {
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    fn read(&self, name: &str) -> String {
        fsx::read_text(self.path(name))
    }

    /// 删掉一个文件；本来就没有也行（`unlink(missing_ok=True)`）
    fn unlink(&self, name: &str) {
        if let Err(e) = std::fs::remove_file(self.path(name)) {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "删不了 {name}：{e}");
        }
    }

    /// 删掉一个文件：它得在（`unlink()`）
    fn unlink_existing(&self, name: &str) {
        std::fs::remove_file(self.path(name)).unwrap_or_else(|e| panic!("删不了 {name}：{e}"));
    }

    fn executable(&self, name: &str, text: &str) {
        kit::bash::double(&self.bin, name, text);
    }

    /// 账号的 ACC_HOME（Claude 的配置目录 / CODEX_HOME）
    fn acc_home(&self) -> String {
        self.path("config").display().to_string()
    }

    /// 被测的 bash 看到的环境（原来的 PYTHON 换成 CCA_TEST：替身经它找 cca-test）；extra 盖掉同名的
    fn env(&self, cmd: &mut Cmd, extra: &[(&str, &str)]) {
        let root = self.root.display().to_string();
        cmd.env("HOME", &root)
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()))
            .env("CCA_TEST", harness::tool())
            .env("HELPER", harness::helper())
            .env("ACC_DIR", &root)
            .env("AUDIT_ROOT", &root)
            .env("ACC_HOME", self.acc_home())
            .env("ACC_MASTER", self.path("master"))
            .env("ACC_KEY", "test@example.com")
            .env("ACC_KIND", "oauth")
            .env("ACC_PROXY", "http://127.0.0.1:7999")
            .env("CCA_NO_PROXY", "localhost")
            .env("CCA_PROXY_BYPASS", "localhost")
            .env("CCA_HOME", &root)
            .env("TOOL", "claude")
            .env("ROUTE", "router")
            .envs(extra.iter().copied());
    }

    /// 脚本从标准输入喂给 /bin/bash（和原来一样，不是 -c）
    fn shell(&self, text: &str, extra: &[(&str, &str)]) -> Output {
        let mut cmd = kit::bash::piped(text);
        self.env(&mut cmd, extra);
        kit::bash::run(cmd.timeout(Duration::from_secs(10)))
    }

    /// ps、lsof 的替身：进程 101 → 102 → 103 一棵树（再加一个不相干的 200），lsof 只认问这棵树的、OPEN_FILE 是 103 打开着的文件
    fn process_fixtures(&self) {
        self.executable(
            "ps",
            "if [ \"$*\" = \"-axo pid=\" ]; then echo 1; else printf \"101 1\\n102 101\\n103 102\\n200 1\\n\"; fi\n",
        );
        self.executable(
            "lsof",
            r#"case " $* " in *" -p 101,102,103 "*) ;; *) exit 9 ;; esac
[ -z "${OPEN_FILE:-}" ] || printf 'p103\nn%s\n' "$OPEN_FILE"
"#,
        );
    }
}

/// 收齐一个起来了的进程的两路输出、等它退出，最多等 limit（`communicate(timeout=…)`）：到时还没完就 panic
fn communicate(child: &mut Child, limit: Duration) -> (Exit, String, String) {
    let end = Instant::now() + limit;
    let readers: Vec<_> = [child.stdout.take(), child.stderr.take()]
        .into_iter()
        .map(|pipe| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut text = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut text);
                }
                let _ = tx.send(String::from_utf8_lossy(&text).into_owned());
            });
            rx
        })
        .collect();
    let mut texts = Vec::new();
    for rx in readers {
        let left = end.saturating_duration_since(Instant::now());
        texts.push(rx.recv_timeout(left).unwrap_or_else(|_| panic!("过了 {limit:?} 两路输出还没读完")));
    }
    let left = end.saturating_duration_since(Instant::now());
    let exit = child.wait_timeout(left).unwrap().unwrap_or_else(|| panic!("过了 {limit:?} 还没退出"));
    let err = texts.pop().unwrap();
    let out = texts.pop().unwrap();
    (exit, out, err)
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn desktop_filters_inherited_credentials_for_both_tools(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable(
        "open",
        "exec \"$CCA_TEST\" env-args-json ANTHROPIC_BASE_URL CLAUDE_CODE_OAUTH_TOKEN CLAUDE_SECURESTORAGE_CONFIG_DIR \
CODEX_ACCESS_TOKEN CODEX_SPARKLE_ENABLED OPENAI_API_KEY CODEX_CA_CERTIFICATE HTTPS_PROXY CODEX_HOME CLAUDE_CONFIG_DIR -- \"$@\"\n",
    );
    let cases: [(&str, &[(&str, &str)]); 2] = [
        (
            "claude",
            &[
                ("ANTHROPIC_BASE_URL", "https://sentinel.invalid"),
                ("CLAUDE_CODE_OAUTH_TOKEN", "dummy"),
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/another-account"),
            ],
        ),
        (
            "codex",
            &[
                ("CODEX_ACCESS_TOKEN", "dummy"),
                ("OPENAI_API_KEY", "dummy"),
                ("CODEX_CA_CERTIFICATE", "/test-ca"),
                ("CODEX_SPARKLE_ENABLED", "true"),
            ],
        ),
    ];
    for (tool, dirty) in cases {
        t.sub(format!("tool={tool}"), |_| {
            let mut setup = format!("tool() {{ \"{tool}_$1\"; }}\n");
            if tool == "codex" {
                setup += "INHERITED_RE=\"$CODEX_INHERITED_RE\"\n";
            }
            let r = fx.shell(
                &format!("{}\n{setup}{}\n{tool}_app_open /dummy.app \"$AUDIT_ROOT/desktop\"\n", env_section(), app_open()),
                dirty,
            );
            assert_eq!(r.status.py(), 0, "{}", r.err());
            let data = json::parse(&r.out()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{}", r.out()));
            let args: Vec<&str> = data["args"].list().iter().map(|a| a.str()).collect();
            for (key, _) in dirty {
                if !["CODEX_CA_CERTIFICATE", "CLAUDE_SECURESTORAGE_CONFIG_DIR"].contains(key) {
                    assert!(data["env"][*key].is_null(), "{key}：{}", data["env"]);
                }
            }
            assert_eq!(data["env"]["HTTPS_PROXY"].str(), "http://127.0.0.1:7999");
            let profile = format!("--user-data-dir={}", fx.profile.display());
            assert!(args.contains(&profile.as_str()), "{args:?}");
            if tool == "codex" {
                assert_eq!(data["env"]["CODEX_CA_CERTIFICATE"].str(), "/test-ca");
                assert_eq!(data["env"]["CODEX_HOME"].str(), fx.acc_home());
                let electron = format!("CODEX_ELECTRON_USER_DATA_PATH={}", fx.profile.display());
                assert!(args.contains(&electron.as_str()), "{args:?}");
                assert!(args.contains(&"CODEX_SPARKLE_ENABLED=false"), "{args:?}");
            } else {
                assert_eq!(data["env"]["CLAUDE_CONFIG_DIR"].str(), fx.acc_home());
                assert_eq!(data["env"]["CLAUDE_SECURESTORAGE_CONFIG_DIR"].str(), fx.acc_home());
                let pinned = format!("CLAUDE_SECURESTORAGE_CONFIG_DIR={}", fx.acc_home());
                assert!(args.contains(&pinned.as_str()), "{args:?}");
            }
        });
    }
}

fn proxy_fallback_unknown_or_declined_stops_before_login(t: &mut Ctx) {
    let fx = fixture(t);
    let stubs = r#"need_bin() { :; }
py() { return "${FALLBACK_OFF_RC:-1}"; }
die() { echo "$*" >&2; exit 1; }
warn() { :; }
interactive() { return 1; }
consent() { return 1; }
ensure_login() { echo logged-in; }
"#;
    for flags in ["()", "(--disable system_proxy_fallback)"] {
        let r = fx.shell(&format!("{stubs}ACC_FLAGS={flags}\n{}\ncodex_app_prepare\n", app_prepare()), &[]);
        assert_ne!(r.status.py(), 0);
        assert!(!r.out().contains("logged-in"), "{}", r.out());
    }
    let r = fx.shell(&format!("{stubs}ACC_FLAGS=()\n{}\ncodex_app_prepare\n", app_prepare()), &[("FALLBACK_OFF_RC", "0")]);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    assert_eq!(r.out().trim(), "logged-in");
}

fn profile_process_lookup_accepts_both_flag_forms_without_prefix_matches(t: &mut Ctx) {
    let fx = fixture(t);
    // 假 pgrep：-f 的正则在 AUDIT_COMMAND 里搜得到就说进程 101
    fx.executable("pgrep", "exec \"$CCA_TEST\" re-search \"$2\" AUDIT_COMMAND 101\n");
    let profile = fx.profile.display().to_string();
    let body = format!("{}\napp_pid \"$AUDIT_NAME\" \"$AUDIT_ROOT/desktop\"\n", app_pid());
    for name in ["Claude", "Google Chrome"] {
        let prefix = format!("/Applications/Test.app/Contents/MacOS/{name} ");
        for argument in [
            format!("--user-data-dir={profile}"),
            format!("--user-data-dir {profile}"),
            format!("--user-data-dir   {profile}"),
            format!("--user-data-dir={profile}/"),
        ] {
            t.sub(format!("name={name:?}, argument={argument:?}"), |_| {
                let command = format!("{prefix}{argument} --proxy-server=http://127.0.0.1:7999");
                let result = fx.shell(&body, &[("AUDIT_NAME", name), ("AUDIT_COMMAND", &command)]);
                assert_eq!(result.status.py(), 0, "{}", result.err());
                assert_eq!(result.out().trim(), "101", "{}", result.err());
            });
        }
        for other in ["desktop-other", "desktopX", "desktop/child"] {
            let command = format!("{prefix}--user-data-dir={}", fx.path(other).display());
            let result = fx.shell(&body, &[("AUDIT_NAME", name), ("AUDIT_COMMAND", &command)]);
            assert_eq!(result.status.py(), 1, "{}", result.err());
            assert_eq!(result.out(), "", "{}", result.err());
        }
    }
}

fn process_lookup_preserves_errors_and_rejects_empty_success(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable("pgrep", "exit \"$PGREP_RC\"\n");
    for (code, expected) in [(0, 2), (1, 1), (2, 2), (3, 3), (127, 127)] {
        t.sub(format!("code={code}"), |_| {
            let result =
                fx.shell(&format!("{}\napp_pid Claude \"$AUDIT_ROOT/desktop\"\n", app_pid()), &[("PGREP_RC", &code.to_string())]);
            assert_eq!(result.status.py(), expected, "{}", result.err());
            assert_eq!(result.out(), "");
        });
    }
}

fn destructive_commands_stop_when_process_query_fails(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable("pgrep", "exit 3\n");
    let stubs = r#"need_tty() { :; }
use_tool() { TOOL="$1"; }
tool() { app_pid Claude "$2"; }
die() { echo "$*" >&2; exit 1; }
py() {
 case "$1" in
  resolve|keys) echo test@example.com ;;
  env) echo 'ACC_NAME=test ACC_LABEL=test' ;;
  *) touch "$AUDIT_ROOT/mutated"; return 9 ;;
 esac
}
confirm() { touch "$AUDIT_ROOT/prompted"; return 1; }
ask() { touch "$AUDIT_ROOT/prompted"; }
logout_account() { touch "$AUDIT_ROOT/mutated"; }
rm() { touch "$AUDIT_ROOT/mutated"; }
"#;
    let profile = fx.profile.display().to_string();
    for (function, command) in [(remove(), "cmd_rm test"), (restore(), "cmd_restore")] {
        t.sub(format!("command={command:?}"), |_| {
            let result = fx.shell(
                &format!("{stubs}{}\n{function}\n{command}\n", app_pid()),
                &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App")],
            );
            assert_ne!(result.status.py(), 0);
            assert!(result.err().contains("无法查询 Dummy App 进程"), "{}", result.err());
            assert!(!fx.exists("prompted"));
            assert!(!fx.exists("mutated"));
        });
    }
}

/// 账号的桌面 App 没开着，账号专属的 Chrome 查不了（不是“没开着”）：停下，不问、不动
fn uninstall_stops_when_browser_process_query_fails(t: &mut Ctx) {
    let fx = fixture(t);
    let stubs = r#"need_tty() { :; }
use_tool() { TOOL="$1"; }
tool() { return 1; }
chrome_pid() { return 3; }
has_worktrees() { return 1; }
die() { echo "$*" >&2; exit 1; }
py() {
 case "$1" in
  keys) echo test@example.com ;;
  env) echo 'ACC_NAME=test ACC_LABEL=test ACC_CHROME_DIR=/nonexistent/chrome' ;;
  *) touch "$AUDIT_ROOT/mutated"; return 9 ;;
 esac
}
confirm() { touch "$AUDIT_ROOT/prompted"; return 1; }
ask() { touch "$AUDIT_ROOT/prompted"; }
logout_account() { touch "$AUDIT_ROOT/mutated"; }
rm() { touch "$AUDIT_ROOT/mutated"; }
"#;
    for command in ["cmd_restore", "cmd_restore uninstall", "cmd_restore uninstall --no-backup"] {
        t.sub(format!("command={command:?}"), |_| {
            let result = fx.shell(&format!("{stubs}{}\n{command}\n", restore()), &[("APP_LABEL", "Dummy App")]);
            assert_ne!(result.status.py(), 0);
            assert!(result.err().contains("无法查询 Chrome 进程"), "{}", result.err());
            assert!(!fx.exists("prompted"));
            assert!(!fx.exists("mutated"));
        });
    }
}

fn browser_process_query_failure_never_prepares_or_opens_profile(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable("pgrep", "exit 3\n");
    let stubs = r#"has_chrome() { return 0; }
chrome_app_path() { echo /dummy.app; }
fail_warn() { echo "$*" >&2; }
py() {
 case "$1" in
  private-profile) echo "$AUDIT_ROOT/desktop" ;;
  browser-profile) touch "$AUDIT_ROOT/browser-prepared" ;;
  *) echo "unexpected py: $*" >&2; return 9 ;;
 esac
}
"#;
    let script = format!(
        r#"{stubs}{}
{}
chrome_open_url() {{ touch "$AUDIT_ROOT/browser-opened"; }}
open_chrome https://example.com
"#,
        app_pid(),
        chrome()
    );
    let result = fx.shell(&script, &[]);
    assert_ne!(result.status.py(), 0);
    assert!(result.err().contains("无法查询 Chrome 进程"), "{}", result.err());
    assert!(!fx.exists("browser-prepared"));
    assert!(!fx.exists("browser-opened"));
}

fn browser_query_failure_after_launch_does_not_deliver_target_url(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable(
        "pgrep",
        r#"if [ ! -e "$AUDIT_ROOT/queried" ]; then
 touch "$AUDIT_ROOT/queried"
 exit 1
fi
exit 3
"#,
    );
    let stubs = r#"has_chrome() { return 0; }
chrome_app_path() { echo /dummy.app; }
fail_warn() { echo "$*" >&2; }
py() {
 case "$1" in
  private-profile) echo "$AUDIT_ROOT/desktop" ;;
  browser-profile) touch "$AUDIT_ROOT/browser-prepared" ;;
  runtime-record) touch "$AUDIT_ROOT/browser-recorded" ;;
  *) echo "unexpected py: $*" >&2; return 9 ;;
 esac
}
"#;
    let script = format!(
        r#"{stubs}{}
{}
chrome_open_url() {{ echo "$2" >> "$AUDIT_ROOT/browser-opened"; }}
open_chrome https://example.com
"#,
        app_pid(),
        chrome()
    );
    let result = fx.shell(&script, &[]);
    assert_ne!(result.status.py(), 0);
    assert!(result.err().contains("无法查询本次 Chrome 进程"), "{}", result.err());
    assert!(fx.exists("browser-prepared"));
    assert_eq!(lines(&fx.read("browser-opened")), ["about:blank"]);
    assert!(!fx.exists("browser-recorded"));
}

fn app_process_query_failure_stops_each_launch_stage(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    let stubs = r#"tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid)
   n=$(cat "$AUDIT_ROOT/query-count" 2>/dev/null || echo 0)
   n=$((n + 1)); echo "$n" > "$AUDIT_ROOT/query-count"
   [ "$n" -lt "$QUERY_FAIL_AT" ] && return 1
   return 3 ;;
  app_prepare) touch "$AUDIT_ROOT/app-prepared" ;;
  app_open) touch "$AUDIT_ROOT/app-opened" ;;
  *) echo "unexpected tool: $*" >&2; return 9 ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { touch "$AUDIT_ROOT/app-activated"; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
die() { echo "$*" >&2; exit 1; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
"#;
    let script = format!("{stubs}{PY_COMMAND_STUB}{}{}\ncmd_app test\n", app_checks(), app_command());
    let profile = fx.profile.display().to_string();
    for stage in [1, 2, 3] {
        t.sub(format!("stage={stage}"), |_| {
            for filename in ["query-count", "app-prepared", "app-opened"] {
                fx.unlink(filename);
            }
            let result = fx.shell(
                &script,
                &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App"), ("QUERY_FAIL_AT", &stage.to_string())],
            );
            assert_ne!(result.status.py(), 0);
            assert!(result.err().contains("无法查询"), "{}", result.err());
            assert_eq!(fx.exists("app-prepared"), stage > 1);
            assert_eq!(fx.exists("app-opened"), stage > 2);
            assert!(!fx.exists("app-activated"));
            assert!(!fx.exists("killed-existing"));
            assert_eq!(fx.shell(&(app_checks() + "\napp_launch_lock\n"), &[]).status.py(), 0);
        });
    }
}

fn stale_profile_is_not_evidence_but_descendant_open_file_is(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    fsx::write(fx.profile.join("Local State"), "{}");
    let body = app_checks() + "\napp_profile_in_use 101 \"$AUDIT_ROOT/desktop\"\n";
    for filename in [String::new(), fx.path("desktop-other").join("Cookies").display().to_string()] {
        t.sub(format!("filename={filename:?}"), |_| {
            assert_ne!(fx.shell(&body, &[("OPEN_FILE", &filename)]).status.py(), 0);
        });
    }
    let cookies = fx.profile.join("Cookies").display().to_string();
    assert_eq!(fx.shell(&body, &[("OPEN_FILE", &cookies)]).status.py(), 0);
}

fn lock_serializes_launches_and_releases_on_process_exit(t: &mut Ctx) {
    let fx = fixture(t);
    let script = app_checks()
        + r#"
app_launch_lock || exit 4
: > "$AUDIT_ROOT/ready"
while [ ! -e "$AUDIT_ROOT/release" ]; do sleep 0.02; done
"#;
    let mut cmd = Cmd::new("/bin/bash");
    cmd.stdin(Stdio::Piped).stdout(Stdio::Piped).stderr(Stdio::Piped);
    fx.env(&mut cmd, &[]);
    let mut first = cmd.spawn().unwrap();
    {
        let mut stdin = first.stdin.take().unwrap();
        stdin.write_all(script.as_bytes()).unwrap();
    }
    wait_for("先起的那个拿到锁（ready）", WAIT, || fx.exists("ready"));
    let r = fx.shell(&(app_checks() + "\napp_launch_lock\n"), &[]);
    assert_ne!(r.status.py(), 0);
    fsx::write(fx.path("release"), "");
    let exit = first.wait_timeout(Duration::from_secs(5)).unwrap().expect("先起的那个 5 秒内退出");
    assert_eq!(exit.py(), 0);
    drop(first);
    assert_eq!(fx.shell(&(app_checks() + "\napp_launch_lock\n"), &[]).status.py(), 0);
}

fn command_rejects_stale_profile_and_closes_launch_fd(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    fsx::write(fx.profile.join("Local State"), "{}");
    // app_open 里查 9 号描述符（启动锁）关上了没有：没关就退出码 9
    let stubs = r#"tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid) [ ! -e "$AUDIT_ROOT/opened" ] || echo 101 ;;
  app_prepare|app_first) : ;;
  app_open)
   "$CCA_TEST" fd-closed 9 || exit 9
   touch "$AUDIT_ROOT/opened" ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { :; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
gui_result() { echo "result:$1"; }
info() { :; }
okay() { :; }
route_note() { echo route; }
die() { echo "$*" >&2; exit 1; }
sleep() { :; }
kill() { echo "$*" >> "$AUDIT_ROOT/stopped"; }
"#;
    let script = format!("{stubs}{PY_COMMAND_STUB}{}{}\ncmd_app test\n", app_checks(), app_command());
    let profile = fx.profile.display().to_string();
    let cookies = fx.profile.join("Cookies").display().to_string();
    let r = fx.shell(&script, &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App")]);
    assert_ne!(r.status.py(), 0);
    assert!(r.err().contains("无法确认"), "{}", r.err());
    assert!(!r.out().contains("result:opened"), "{}", r.out());
    assert_eq!(lines(&fx.read("stopped")), ["101", "102", "103"]);
    fx.unlink_existing("opened");
    let r = fx.shell(&script, &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App"), ("OPEN_FILE", &cookies)]);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    assert!(r.out().contains("result:opened"), "{}", r.out());
    assert!(fx.read("record.log").contains("runtime-record write test@example.com app 101"), "{}", fx.read("record.log"));
    fx.unlink_existing("opened");
    fx.unlink_existing("stopped");
    let r = fx.shell(
        &script,
        &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App"), ("OPEN_FILE", &cookies), ("RECORD_WRITE_RC", "1")],
    );
    assert_ne!(r.status.py(), 0);
    assert!(!r.out().contains("result:opened"), "{}", r.out());
    assert_eq!(lines(&fx.read("stopped")), ["101", "102", "103"]);
}

fn existing_instance_is_verified_without_ever_killing_it(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    let setup = r#"
tool() { case "$1" in app_path) echo /dummy.app ;; app_pid) echo 101 ;; *) echo unexpected >&2; exit 9 ;; esac; }
pick_account() { echo test@example.com; }
activate_pid() { echo activated; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { touch "$AUDIT_ROOT/preflight"; return "${PREFLIGHT_RC:-0}"; }
prepare_account() { touch "$AUDIT_ROOT/prepared"; return "${PREPARE_RC:-0}"; }
gui_step() { :; }
info() { :; }
gui_result() { echo "result:$1"; }
die() { echo "$*" >&2; exit 1; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
"#;
    let body = format!("{setup}{PY_COMMAND_STUB}{}{}\ncmd_app test\n", app_checks(), app_command());
    let profile = fx.profile.display().to_string();
    let cookies = fx.profile.join("Cookies").display().to_string();
    let extra = [("ACC_DESKTOP_DIR", profile.as_str()), ("APP_LABEL", "Dummy App")];
    let with = |more: &[(&'static str, &'static str)], open: bool| -> Vec<(&str, &str)> {
        let mut env = extra.to_vec();
        if open {
            env.push(("OPEN_FILE", cookies.as_str()));
        }
        env.extend_from_slice(more);
        env
    };
    let refused = fx.shell(&body, &extra);
    assert_ne!(refused.status.py(), 0);
    assert!(refused.err().contains("已有"), "{}", refused.err());
    assert!(!refused.out().contains("activated"), "{}", refused.out());
    assert!(!fx.exists("killed-existing"));
    assert!(fx.exists("preflight"));
    assert!(fx.exists("prepared"));
    let missing_record = fx.shell(&body, &with(&[], true));
    assert_ne!(missing_record.status.py(), 0);
    assert!(missing_record.err().contains("启动记录"), "{}", missing_record.err());
    assert!(!missing_record.out().contains("activated"), "{}", missing_record.out());
    assert!(!fx.exists("killed-existing"));
    let accepted = fx.shell(&body, &with(&[("RECORD_CHECK_RC", "0")], true));
    assert_eq!(accepted.status.py(), 0, "{}", accepted.err());
    assert!(accepted.out().contains("result:activated"), "{}", accepted.out());
    assert!(!fx.exists("killed-existing"));
    let failures: [&[(&str, &str)]; 4] =
        [&[("PREFLIGHT_RC", "1")], &[("PREPARE_RC", "1")], &[("PROFILE_RC", "1")], &[("TOOL", "codex"), ("FALLBACK_OFF_RC", "1")]];
    for failure in failures {
        t.sub(format!("failure={failure:?}"), |_| {
            let mut more = vec![("RECORD_CHECK_RC", "0")];
            more.extend_from_slice(failure);
            let refused = fx.shell(&body, &with(&more, true));
            assert_ne!(refused.status.py(), 0);
            assert!(!refused.out().contains("activated"), "{}", refused.out());
            assert!(!fx.exists("killed-existing"));
        });
    }
}

fn interrupted_new_launch_is_cleaned_before_pid_is_recorded(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    let setup = r#"
tool() {
 case "$1" in
  app_path) echo /dummy.app ;;
  app_pid) [ ! -e "$AUDIT_ROOT/opened" ] || echo 101 ;;
  app_prepare) : ;;
  app_open)
   touch "$AUDIT_ROOT/opened"
   while :; do sleep 0.05; done ;;
 esac
}
pick_account() { echo test@example.com; }
activate_pid() { :; }
sync_if_changed() { :; }
launchers_sync() { :; }
preflight() { :; }
prepare_account() { :; }
gui_step() { :; }
gui_result() { echo "result:$1"; }
info() { :; }
okay() { :; }
die() { echo "$*" >&2; exit 1; }
kill() { echo "$*" >> "$AUDIT_ROOT/stopped"; }
"#;
    let profile = fx.profile.display().to_string();
    let mut cmd = kit::bash::c(&format!("{setup}{PY_COMMAND_STUB}{}{}\ncmd_app test", app_checks(), app_command()));
    cmd.stdout(Stdio::Piped).stderr(Stdio::Piped);
    fx.env(&mut cmd, &[("ACC_DESKTOP_DIR", &profile), ("APP_LABEL", "Dummy App")]);
    let mut process = cmd.spawn().unwrap();
    wait_for("App 打开了（opened）", WAIT, || fx.exists("opened"));
    process.signal(libc::SIGTERM).unwrap();
    let (exit, stdout, stderr) = communicate(&mut process, Duration::from_secs(5));
    assert_eq!(exit.py(), 143, "{stderr}");
    assert!(!stdout.contains("result:opened"), "{stdout}");
    assert_eq!(lines(&fx.read("stopped")), ["101", "102", "103"]);
    assert_eq!(fx.shell(&(app_checks() + "\napp_launch_lock\n"), &[]).status.py(), 0);
}

fn cleanup_excludes_all_processes_present_before_launch(t: &mut Ctx) {
    let fx = fixture(t);
    fx.process_fixtures();
    let setup = r#"
APP_LAUNCH_ACTIVE=1
APP_LAUNCH_DIR="$AUDIT_ROOT/desktop"
APP_LAUNCH_NEW_PID=101
APP_LAUNCH_BEFORE='1 101 102 103'
tool() { echo 101; }
kill() { touch "$AUDIT_ROOT/killed-existing"; }
"#;
    let result = fx.shell(&format!("{}\n{setup}\napp_launch_cleanup\n", app_checks()), &[]);
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert!(!fx.exists("killed-existing"));
}

fn actual_existing_process_holding_profile_file_is_accepted(t: &mut Ctx) {
    let fx = fixture(t);
    let holding = fx.profile.join("Cookies");
    // 一个真的进程拿着资料目录里的文件（以写的方式开着），关上它的标准输入才放手
    let mut process =
        Cmd::new(harness::tool()).arg("hold").arg(&holding).arg(fx.path("ready")).stdin(Stdio::Piped).spawn().unwrap();
    wait_for("拿着文件的进程准备好了（ready）", WAIT, || fx.exists("ready"));
    let result = fx.shell(&format!("{}\napp_profile_in_use {} \"$AUDIT_ROOT/desktop\"\n", app_checks(), process.pid()), &[]);
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert_eq!(process.try_wait().unwrap(), None, "拿着文件的进程还在");
    drop(process.stdin.take());
    assert!(process.wait_timeout(Duration::from_secs(5)).unwrap().is_some(), "叫它停了，5 秒内没退出");
}
