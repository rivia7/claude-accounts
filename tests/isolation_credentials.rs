//! 凭证来源的隔离：用 cca 里真的那几个设环境的函数（child_env、claude_env……），客户端是替身。
//!
//! 默认用替身客户端。设了 CCA_TEST_CLAUDE_BIN（装好的 Claude Code 的路径）时，另外拿真的客户端来测：临时 HOME、假的 token、
//! 假的钥匙串（security），只在本机跑 `auth status`，不交给它真的凭证（原来的 tests/isolation_credentials.py）。
//! 原来的 Python 里带真客户端的那一组继承了前两个用例、设了变量时把它们再跑一遍（多放一个假的 security）；这里前两个只跑一遍。

use kit::case;
use kit::cca_src;
use kit::fsx;
use kit::harness::{self, Ctx, Module};
use kit::json::{self, Value};
use kit::proc::Output;
use std::collections::BTreeSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_credentials",
    src: include_str!("isolation_credentials.rs"),
    default: true,
    tests: &[
        case!(account_directory_wins_and_credential_descriptors_are_removed),
        case!(desktop_explicitly_pins_both_configuration_and_keychain_directories),
        case!(native_keychain_names_are_compatible_and_different_between_accounts),
        case!(native_inherited_oauth_descriptor_cannot_replace_account_login),
    ],
};

const DESCRIPTORS: [&str; 4] = [
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
    "CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR",
];

/// cca 里从 INHERITED_RE 到 pyenv 之前：继承变量的名单和 child_env、claude_env 这些
fn env_section() -> String {
    cca_src::between("INHERITED_RE='^", "\npyenv()")
}

/// cca 里从 app_open 到 claude_app_first 之前：起桌面 App 的那几个函数
fn app_open_section() -> String {
    cca_src::between("app_open() {", "\nclaude_app_first()")
}

/// 一个用例的沙箱：HOME 是临时目录，替身放在 bin 里，账号目录是 alice
struct Fx {
    root: PathBuf,
    bin: PathBuf,
    home: PathBuf,
}

fn fixture(t: &mut Ctx) -> Fx {
    let root = t.tmp("cca-credential-isolation-");
    let (bin, home) = (root.join("bin"), root.join("alice"));
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&home).unwrap();
    Fx { root, bin, home }
}

impl Fx {
    fn executable(&self, name: &str, content: &str) -> String {
        kit::bash::double(&self.bin, name, content).display().to_string()
    }

    /// 继承变量的名单和设环境的函数 + body，交给 bash -c。环境照原来的给（原来的 PYTHON 换成 CCA_TEST：替身经它找 cca-test），
    /// extra 盖掉同名的；fds 是要带给 bash 的描述符（号 → 这边的那一份）
    fn shell(&self, body: &str, extra: &[(&str, &str)], fds: Vec<(i32, OwnedFd)>) -> Output {
        let script = format!("{}\ntool() {{ claude_env; }}\n{body}", env_section());
        let mut cmd = kit::bash::c(&script);
        cmd.cwd(&self.root)
            .env("HOME", &self.root)
            .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()))
            .env("USER", "cca-isolation-fixture")
            .env("CCA_TEST", harness::tool())
            .env("ACC_HOME", &self.home)
            .env("ACC_KEY", "alice@example.com")
            .env("ACC_KIND", "oauth")
            .env("ACC_PROXY", "http://127.0.0.1:9")
            .env("CCA_NO_PROXY", "")
            .env("CCA_PROXY_BYPASS", "")
            .env("CCA_HOME", self.root.join(".cca"))
            .env("AUDIT_ROOT", &self.root)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("DISABLE_AUTOUPDATER", "1")
            .envs(extra.iter().copied())
            .timeout(Duration::from_secs(20));
        for (number, fd) in fds {
            cmd.fd(number, fd);
        }
        kit::bash::run(&mut cmd)
    }
}

fn account_directory_wins_and_credential_descriptors_are_removed(t: &mut Ctx) {
    let fx = fixture(t);
    let client = fx.executable("client", "exec \"$CCA_TEST\" env-json\n");
    let home = fx.home.display().to_string();
    for kind in ["oauth", "api"] {
        t.sub(format!("kind={kind}"), |_| {
            let mut dirty: Vec<(&str, &str)> = DESCRIPTORS.iter().map(|name| (*name, "8")).collect();
            dirty.extend([
                ("CLAUDE_CONFIG_DIR", "/wrong-config"),
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/wrong-keychain"),
                ("CLAUDE_CODE_OAUTH_TOKEN", "dummy-token"),
                ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "1000"),
                ("CLIENT", client.as_str()),
                ("ACC_KIND", kind),
            ]);
            let result = fx.shell("child_env\nenv \"${UNSETS[@]}\" \"${SETS[@]}\" \"$CLIENT\" 3<&-\n", &dirty, Vec::new());
            assert_eq!(result.status.py(), 0, "{}", result.err());
            let actual = json::parse(&result.out()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{}", result.out()));
            assert_eq!(actual["CLAUDE_CONFIG_DIR"].str(), home);
            assert_eq!(actual["CLAUDE_SECURESTORAGE_CONFIG_DIR"].str(), home);
            for name in DESCRIPTORS.iter().chain(&["CLAUDE_CODE_OAUTH_TOKEN"]) {
                assert!(actual.get(name).is_none(), "{name} 不该在 {actual}");
            }
            assert_eq!(actual["CLAUDE_CODE_MAX_OUTPUT_TOKENS"].str(), "1000");
        });
    }
}

fn desktop_explicitly_pins_both_configuration_and_keychain_directories(t: &mut Ctx) {
    let fx = fixture(t);
    fx.executable("open", "exec \"$CCA_TEST\" argv-json \"$@\"\n");
    let result = fx.shell(
        &(app_open_section() + "\nclaude_app_open /fixture.app \"$AUDIT_ROOT/desktop\"\n"),
        &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/wrong-keychain")],
        Vec::new(),
    );
    assert_eq!(result.status.py(), 0, "{}", result.err());
    let args = json::parse(&result.out()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{}", result.out()));
    let args: Vec<&str> = args.list().iter().map(Value::str).collect();
    for name in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"] {
        let want = format!("{name}={}", fx.home.display());
        let index = args.iter().position(|a| *a == want).unwrap_or_else(|| panic!("{want:?} 不在 {args:?} 里"));
        assert_eq!(args[index - 1], "--env");
    }
}

/// 真的 Claude Code（CCA_TEST_CLAUDE_BIN，解过链接的）；没设就跳过这个用例
fn native(t: &Ctx) -> String {
    match std::env::var("CCA_TEST_CLAUDE_BIN") {
        Ok(bin) if !bin.is_empty() => {
            std::fs::canonicalize(&bin).unwrap_or_else(|e| panic!("CCA_TEST_CLAUDE_BIN（{bin}）不在：{e}")).display().to_string()
        }
        _ => t.skip("set CCA_TEST_CLAUDE_BIN for the isolated native-client contract"),
    }
}

/// 真客户端那一组的沙箱：再放一个假的 security（记下怎么被调的、总是失败）
struct Native {
    fx: Fx,
    native: String,
    log: PathBuf,
}

fn native_fixture(t: &mut Ctx) -> Native {
    let native = native(t);
    let fx = fixture(t);
    let log = fx.root.join("keychain.log");
    fx.executable("security", "printf \"%s\\n\" \"$*\" >> \"$AUDIT_ROOT/keychain.log\"\nexit 44\n");
    Native { fx, native, log }
}

impl Native {
    /// 真客户端的 `auth status --json`：经 child_env 包一层（wrapped），或者直接跑、再带上会把流量引走的代理变量。
    /// 给出它说的 JSON 和它问过钥匙串的那几个服务名（假 security 的 -s 后面）
    fn status(&self, home: &Path, dirty: &[(&str, &str)], wrapped: bool, fds: Vec<(i32, OwnedFd)>) -> (Value, BTreeSet<String>) {
        fsx::write(&self.log, "");
        let home = home.display().to_string();
        let mut env: Vec<(&str, &str)> = dirty.to_vec();
        env.extend([("CLIENT", self.native.as_str()), ("ACC_HOME", home.as_str()), ("CLAUDE_CONFIG_DIR", home.as_str())]);
        let body = if wrapped {
            "child_env\nenv \"${UNSETS[@]}\" \"${SETS[@]}\" \"$CLIENT\" auth status --json 3<&-\n"
        } else {
            env.extend([
                ("HTTP_PROXY", "http://127.0.0.1:9"),
                ("HTTPS_PROXY", "http://127.0.0.1:9"),
                ("ALL_PROXY", "http://127.0.0.1:9"),
                ("NO_PROXY", ""),
            ]);
            "\"$CLIENT\" auth status --json 3<&-\n"
        };
        let result = self.fx.shell(body, &env, fds);
        assert!([0, 1].contains(&result.status.py()), "{}", result.err());
        let said = json::parse(&result.out()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{}", result.out()));
        let rx = regex::Regex::new(r"-s (.+)").expect("写死的正则");
        let stores = rx.captures_iter(&fsx::read_text(&self.log)).map(|c| c[1].to_string()).collect();
        (said, stores)
    }
}

fn native_keychain_names_are_compatible_and_different_between_accounts(t: &mut Ctx) {
    let n = native_fixture(t);
    let mut stores = Vec::new();
    for account in ["alice", "bob"] {
        let home = n.fx.root.join(account);
        std::fs::create_dir_all(&home).unwrap();
        let (baseline, old_stores) = n.status(&home, &[], false, Vec::new());
        let shared = n.fx.root.join("shared").display().to_string();
        let (checked, new_stores) = n.status(&home, &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", &shared)], true, Vec::new());
        assert!(!baseline["loggedIn"].bool());
        assert!(!checked["loggedIn"].bool());
        assert!(!old_stores.is_empty(), "native client must use the fake security executable");
        assert_eq!(old_stores, new_stores);
        stores.push(new_stores);
    }
    assert!(stores[0].is_disjoint(&stores[1]), "{stores:?}");
}

fn native_inherited_oauth_descriptor_cannot_replace_account_login(t: &mut Ctx) {
    let n = native_fixture(t);
    let token = n.fx.root.join("dummy-token");
    fsx::write(&token, "dummy-isolation-token-not-a-real-secret");
    let source = std::fs::File::open(&token).unwrap();
    // SAFETY: fcntl 成了给的是一个新的描述符（10 号以上头一个空着的），马上交给 OwnedFd 管
    let held = unsafe {
        let fd = libc::fcntl(source.as_raw_fd(), libc::F_DUPFD, 10);
        assert!(fd >= 0, "复制不了描述符：{}", std::io::Error::last_os_error());
        OwnedFd::from_raw_fd(fd)
    };
    drop(source);
    let descriptor = held.as_raw_fd();
    let number = descriptor.to_string();
    let dirty = [("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", number.as_str())];
    // 带给 bash 的是同一个打开的文件（共用读到哪里）：子进程里还是这个号
    let pass = || vec![(descriptor, held.try_clone().unwrap())];
    let (baseline, _) = n.status(&n.fx.home, &dirty, false, pass());
    assert!(baseline["loggedIn"].bool());
    assert_eq!(baseline["authMethod"].str(), "oauth_token");
    // SAFETY: 只是把这个描述符读到的地方挪回开头
    assert_eq!(unsafe { libc::lseek(descriptor, 0, libc::SEEK_SET) }, 0);
    let (checked, _) = n.status(&n.fx.home, &dirty, true, pass());
    assert!(!checked["loggedIn"].bool());
}
