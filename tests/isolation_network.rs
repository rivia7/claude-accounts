//! 账号的网络边界：路由器、内置代理切节点，出口探测、控制接口的 curl 走哪条路。从 cca 里切出函数来跑，路由器、curl 都是替身，
//! 只用临时文件、不联网（原来的 tests/isolation_network.py）。

use kit::case;
use kit::cca_src;
use kit::fsx;
use kit::harness::{Ctx, Module};
use kit::proc::Output;
use std::path::PathBuf;
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_network",
    src: include_str!("isolation_network.rs"),
    default: true,
    tests: &[
        case!(router_transport_and_selection_failures_stop_before_port_check),
        case!(missing_router_cannot_use_an_unverified_open_port),
        case!(renamed_node_requires_successful_deployment_and_recheck),
        case!(failed_deployment_cannot_be_reported_as_recovered),
        case!(successful_reload_requires_a_successful_recheck),
        case!(usage_route_stays_empty_on_router_failure),
        case!(unverified_usage_route_keeps_port_diagnosis_without_authentication),
        case!(check_does_not_probe_unverified_router),
        case!(router_and_local_selection_confirm_actual_node),
        case!(missing_runtime_and_missing_source_node_fail_without_restart),
        case!(missing_local_group_can_be_rebuilt_and_checked),
        case!(builtin_selection_failure_cannot_be_repaired_by_unrequested_reload),
        case!(failed_local_ensure_cannot_pass_with_a_correct_current_slot),
        case!(router_reload_does_not_hide_failed_node_selection),
        case!(exit_probes_ignore_host_bypass_and_curl_config),
        case!(api_probe_has_explicit_proxy_or_direct_route),
        case!(control_plane_never_inherits_host_proxy),
    ],
};

/// 切出来的函数前面接的替身
const PROLOGUE: &str = r#"
fail_warn() { printf 'FAIL:%s:%s\n' "$1" "$2" >&2; }
warn() { printf 'WARN:%s\n' "$*" >&2; }
info() { :; }
load_router() { :; }
router_ready() { return 0; }
nodes_follow() { return 1; }
allow_reload() { return 1; }
port_open() { touch checked-port; return 0; }
core_nodes_hook() { touch core-hook; }
"#;

/// 每个用例一个临时目录：HOME 和当前目录
struct Fx {
    root: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    Fx { root: t.tmp("cca-network-") }
}

impl Fx {
    /// 替身 + 切出来的这几个函数 + code，交给 bash -c。环境：账号的出口、路由器，再加上开发者的 shell 里可能有的、不该被用上的
    /// 代理设置（NO_PROXY、HTTPS_PROXY 这些）；extra 盖掉同名的
    fn run_shell(&self, names: &[&str], code: &str, extra: &[(&str, &str)]) -> Output {
        let script = format!("{PROLOGUE}{}\n{code}", cca_src::functions(names));
        let mut cmd = kit::bash::c(&script);
        cmd.cwd(&self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("ACC_PROXY", "http://192.0.2.10:7901")
            .env("ACC_PORT", "7901")
            .env("ACC_KEY", "alice@example.com")
            .env("ACC_KIND", "oauth")
            .env("PROXY_HOST", "192.0.2.10")
            .env("RDIR", "/fake/cca")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .env("HTTPS_PROXY", "http://example.invalid:9999")
            .env("ALL_PROXY", "http://example.invalid:9998")
            .envs(extra.iter().copied())
            .timeout(Duration::from_secs(10));
        kit::bash::run(&mut cmd)
    }

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

    /// 替身 curl 记下的每一次调用的参数（每次以一行 END 结尾）
    fn curl_calls(&self) -> Vec<Vec<String>> {
        let (mut calls, mut one) = (Vec::new(), Vec::new());
        for arg in self.read("curl-args").lines() {
            if arg == "END" {
                calls.push(std::mem::take(&mut one));
            } else {
                one.push(arg.to_string());
            }
        }
        calls
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// list.index(item)：没有就 panic
fn index(list: &[String], item: &str) -> usize {
    list.iter().position(|a| a == item).unwrap_or_else(|| panic!("{item:?} 不在 {list:?} 里"))
}

fn router_transport_and_selection_failures_stop_before_port_check(t: &mut Ctx) {
    let fx = fixture(t);
    for rc in [1, 3, 255] {
        t.sub(format!("rc={rc}"), |_| {
            let result = fx.run_shell(&["router_ensure", "route_router"], &format!("rsh() {{ return {rc}; }}\nroute_router"), &[]);
            assert_ne!(result.status.py(), 0);
            assert!(result.err().contains("sync_failed"), "{}", result.err());
            assert!(!fx.exists("checked-port"));
            assert!(!fx.exists("core-hook"));
        });
    }
}

fn missing_router_cannot_use_an_unverified_open_port(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["router_ensure", "route_router"],
        r#"
router_ready() { return 1; }
rsh() { touch unexpected-ssh; }
route_router
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert!(!fx.exists("checked-port"));
    assert!(!fx.exists("unexpected-ssh"));
}

/// 路由器认不出节点时的补救：节点改了名（rename）就推送、不然问过后重载，之后都要再查一次。几个数是各个替身的退出码
fn recovery_script(rename: bool, push: i32, reload: i32, last: i32) -> String {
    format!(
        r#"
rsh() {{
    if [ -f attempt ]; then echo again >> attempt; return {last}; fi
    echo first > attempt; return {first}
}}
nodes_follow() {{ return {follow}; }}
router_push() {{ touch pushed; return {push}; }}
allow_reload() {{ return 0; }}
router_reload() {{ touch reloaded; return {reload}; }}
py() {{ echo 'ACC_NODE=US-B'; }}
router_ensure
"#,
        first = if rename { 1 } else { 3 },
        follow = if rename { 0 } else { 1 },
    )
}

fn renamed_node_requires_successful_deployment_and_recheck(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(&["router_ensure"], &recovery_script(true, 0, 0, 0), &[]);
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert!(fx.exists("pushed"));
    assert_eq!(lines(&fx.read("attempt")), ["first", "again"]);
}

fn failed_deployment_cannot_be_reported_as_recovered(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(&["router_ensure"], &recovery_script(true, 7, 0, 0), &[]);
    assert_ne!(result.status.py(), 0);
    assert_eq!(lines(&fx.read("attempt")), ["first"]);
}

fn successful_reload_requires_a_successful_recheck(t: &mut Ctx) {
    let fx = fixture(t);
    for last in [0, 255] {
        t.sub(format!("final={last}"), |_| {
            fx.unlink("attempt");
            let result = fx.run_shell(&["router_ensure"], &recovery_script(false, 0, 0, last), &[]);
            assert_eq!(result.status.py() == 0, last == 0, "{}", result.err());
            assert!(fx.exists("reloaded"));
            assert_eq!(lines(&fx.read("attempt")), ["first", "again"]);
        });
    }
}

fn usage_route_stays_empty_on_router_failure(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["usage_route"],
        r#"
want_local() { return 1; }
router_ensure() { return 1; }
USAGE_HOST=stale
usage_route; rc=$?
printf '%s' "$USAGE_HOST" > usage-host
exit "$rc"
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert_eq!(fx.read("usage-host"), "");
}

fn unverified_usage_route_keeps_port_diagnosis_without_authentication(t: &mut Ctx) {
    let fx = fixture(t);
    for opened in [false, true] {
        t.sub(format!("opened={opened}"), |_| {
            let code = format!(
                r#"
want_local() {{ return 1; }}
router_ensure() {{ return 1; }}
port_open() {{ return {open}; }}
py() {{ [ "$1" = email ] && {{ echo alice@example.com; return 0; }}; touch unexpected-query; }}
child_env() {{ touch unexpected-query; }}
lower() {{ printf '%s' "$1"; }}
usage_route || :
ACC_PROXY="${{USAGE_HOST:+http://${{USAGE_HOST}}:${{ACC_PORT}}}}"
usage_one usage-result
"#,
                open = if opened { 0 } else { 1 },
            );
            let result = fx.run_shell(&["usage_route", "usage_one"], &code, &[]);
            assert_eq!(result.status.py(), 0, "{}", result.err());
            assert_eq!(fx.read("usage-result.skip"), if opened { "route\t\n" } else { "port\t7901\n" });
            assert!(!fx.exists("unexpected-query"));
        });
    }
}

fn check_does_not_probe_unverified_router(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["cmd_check"],
        r#"
py() {
    case "$1" in
        keys) echo alice@example.com ;;
        env) echo 'ACC_PORT=7901 ACC_KIND=oauth' ;;
    esac
}
sync_if_changed() { :; }
want_local() { return 1; }
router_ensure() { return 1; }
coffee_check() { touch unexpected-probe; }
cmd_check
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert!(!fx.exists("checked-port"));
    assert!(!fx.exists("unexpected-probe"));
}

/// 切节点：假的控制接口可以收下 PUT 却不照做（noop）、报错（fail）、认证不过（auth）、没有这个组（missing）。
/// 写出要切成的节点（selected），给出整段脚本（最后一行是 select_all 或 local_select_all）
fn selection_script(fx: &Fx, local: bool, mode: &str) -> String {
    let setup = r#"
current=US-OLD
jstr() { printf '"%s"' "$1"; }
py() { printf 'CCA-1\tUS-A\n'; }
ctl() {
    if [ "$1" = GET ]; then
        CODE=200
        case "$MODE" in auth) CODE=401 ;; missing) CODE=404 ;; esac
    else
        CODE=204
        case "$3" in
            *REJECT*) current=REJECT; touch rejected ;;
            *) case "$MODE" in ok) current=US-A ;; fail) CODE=500 ;; esac ;;
        esac
    fi
    printf '{"now":"%s"}' "$current" > "$CTLOUT"
}
lctl() { ctl "$@"; LCODE="$CODE"; LBODY=$(cat "$CTLOUT"); }
"#;
    fsx::write(fx.path("selected"), "CCA-1\tUS-A\n");
    format!(
        "MODE={mode}\nSEL=selected\nCTLOUT=control.json\nTAB=$'\\t'\n{setup}{}",
        if local { "local_select_all" } else { "select_all" }
    )
}

fn router_and_local_selection_confirm_actual_node(t: &mut Ctx) {
    let fx = fixture(t);
    for local in [false, true] {
        for mode in ["ok", "noop", "fail", "auth", "missing"] {
            t.sub(format!("local={local}, mode={mode}"), |_| {
                fx.unlink("rejected");
                let names: &[&str] = if local { &["local_select_all", "json_get"] } else { &["select_all", "now_of", "json_str"] };
                let result = fx.run_shell(names, &selection_script(&fx, local, mode), &[]);
                let expected = match mode {
                    "ok" => 0,
                    "missing" => 2,
                    _ => 1,
                };
                assert_eq!(result.status.py(), expected, "{}{}", result.err(), result.out());
                if mode == "noop" || mode == "fail" {
                    assert!(fx.exists("rejected"));
                }
            });
        }
    }
}

fn missing_runtime_and_missing_source_node_fail_without_restart(t: &mut Ctx) {
    let fx = fixture(t);
    let script = selection_script(&fx, false, "fail");
    // 去掉最后的 select_all（rsplit('select_all', 1)[0]），节点换成 US-D
    let setup = script[..script.rfind("select_all").expect("脚本里有 select_all")].replace("US-A", "US-D");
    fsx::write(fx.path("selected"), "CCA-1\tUS-D\n");
    let result = fx.run_shell(
        &["select_all", "now_of", "json_str", "apply_now"],
        &(setup
            + r#"
BK=oc R="$HOME" D="$HOME" NAME=OpenClash
fill_source() { :; }
runtime_config() { echo "$HOME/missing-runtime.yaml"; }
running() { return 0; }
missing_ports() { :; }
in_source() { printf '%s' "$1" > missing-source; return 1; }
restart_now() { touch unexpected-restart; return 0; }
apply_now
"#),
        &[],
    );
    assert_eq!(result.status.py(), 1, "{}{}", result.err(), result.out());
    assert!(result.out().contains("切换到 US-D 失败"), "{}", result.out());
    assert!(fx.read("missing-source").contains("US-D"));
    assert!(!fx.exists("unexpected-restart"));
}

fn missing_local_group_can_be_rebuilt_and_checked(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["core_ensure"],
        r#"
core_possible() { return 0; }
core_fetch() { return 0; }
py() { return 0; }
core_sock() { echo unused; }
core_up() { return 0; }
local_ports_ok() { return 0; }
local_select_all() { [ -f rebuilt ] && return 0; return 2; }
core_reload() { touch rebuilt; return 0; }
LOCAL_APPLIED=same LOCAL_WANT=same
core_ensure
"#,
        &[],
    );
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert!(fx.exists("rebuilt"));
}

fn builtin_selection_failure_cannot_be_repaired_by_unrequested_reload(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["core_ensure"],
        r#"
core_possible() { return 0; }
core_fetch() { return 0; }
py() { return 0; }
core_sock() { echo unused; }
core_up() { return 0; }
local_ports_ok() { return 0; }
local_select_all() { return 1; }
core_reload() { touch unexpected-reload; return 0; }
LOCAL_APPLIED=same LOCAL_WANT=same
core_ensure
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert!(!fx.exists("unexpected-reload"));
}

fn failed_local_ensure_cannot_pass_with_a_correct_current_slot(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["route_local", "json_get"],
        r#"
load_local() { :; }
local_ready() { return 0; }
local_ensure() { return 1; }
lctl() { LCODE=200; LBODY='{"now":"US-A"}'; }
py() { echo US-A; }
gui_fail() { :; }
ACC_NODE=US-A ACC_SLOT=1
route_local
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert!(!fx.exists("checked-port"));
}

fn router_reload_does_not_hide_failed_node_selection(t: &mut Ctx) {
    let fx = fixture(t);
    fsx::write(fx.path("runtime.yaml"), "fake runtime\n");
    std::fs::create_dir(fx.path("tmp")).unwrap();
    let result = fx.run_shell(
        &["apply_now"],
        r#"
BK=oc R="$HOME" D="$HOME" ACC=unused CCA_ALLOW_RELOAD=1 NAME=OpenClash
fill_source() { :; }
runtime_config() { echo "$HOME/runtime.yaml"; }
running() { return 0; }
fill_waiting() { return 1; }
ruby() { return 0; }
structure() { echo "$1"; }
core_test() { return 0; }
cca_ports() { echo 7901; }
put_runtime() { cp -f "$2" "$1"; }
reload() { return 0; }
reload_wait() { return 0; }
select_all() { return 1; }
apply_now
"#,
        &[],
    );
    assert_ne!(result.status.py(), 0);
    assert!(!result.out().contains("已重载并建立账号端口池"), "{}", result.out());
}

/// 替身 curl：先把参数记下来（每次以一行 END 结尾），再给写死的回答：不起真的 curl、不联网
const FAKE_CURL: &str = r#"
curl() {
    printf '%s\n' "$@" >> curl-args
    printf '%s\n' END >> curl-args
    case "$*" in
        *cdn-cgi/trace*) printf 'ip=203.0.113.10\nloc=US\n' ;;
        *iprisk*) echo '{}' ;;
        *http_code*) printf '{}\n200' ;;
    esac
}
"#;

/// curl 的头一个参数是 -q（先于别的选项，不读 .curlrc），--noproxy 后面是 bypass
fn assert_curl_route(args: &[String], bypass: &str) {
    assert_eq!(args[0], "-q", "curl must ignore .curlrc before parsing other options");
    assert_eq!(args[index(args, "--noproxy") + 1], bypass);
}

fn exit_probes_ignore_host_bypass_and_curl_config(t: &mut Ctx) {
    let fx = fixture(t);
    fsx::write(fx.path(".curlrc"), "proxy = \"http://example.invalid:9997\"\nnoproxy = \"*\"\n");
    let result = fx.run_shell(
        &["coffee_check"],
        &(FAKE_CURL.to_string()
            + r#"
leak_start() { :; }
leak_finish() { :; }
py() { echo verified; }
TRACE_HOST=example.com CCA_MIN_TRUST=0 ACC_NODE=US-A
coffee_check
"#),
        &[],
    );
    assert_eq!(result.status.py(), 0, "{}", result.err());
    let calls = fx.curl_calls();
    assert_eq!(calls.len(), 2);
    for args in &calls {
        assert_curl_route(args, "");
        assert_eq!(args[index(args, "-x") + 1], "http://192.0.2.10:7901");
    }
}

fn api_probe_has_explicit_proxy_or_direct_route(t: &mut Ctx) {
    let fx = fixture(t);
    for proxy in ["http://192.0.2.10:7901", ""] {
        t.sub(format!("proxy={proxy:?}"), |_| {
            fx.unlink("curl-args");
            let result = fx.run_shell(
                &["api_check"],
                &(FAKE_CURL.to_string()
                    + r#"
health_note() { :; }
ACC_BASE_URL=https://api.example.com
api_check
"#),
                &[("ACC_PROXY", proxy)],
            );
            assert_eq!(result.status.py(), 0, "{}", result.err());
            assert_curl_route(&fx.curl_calls()[0], if proxy.is_empty() { "*" } else { "" });
        });
    }
}

fn control_plane_never_inherits_host_proxy(t: &mut Ctx) {
    let fx = fixture(t);
    let result = fx.run_shell(
        &["lctl"],
        &(FAKE_CURL.to_string()
            + r#"
LOCAL_CTL=http://192.0.2.10:9090
lctl GET /version
test "$LCODE" = 200
"#),
        &[],
    );
    assert_eq!(result.status.py(), 0, "{}", result.err());
    assert_curl_route(&fx.curl_calls()[0], "*");
}

// 辅助程序里的探针（leak_curl）不继承主机的代理设置、不读 curl 的配置：cli/src/cmd/leak.rs 的
// leak_probe_has_explicit_route_and_no_curl_config（cargo test）
