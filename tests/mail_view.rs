//! 邮箱助手（cca mail）：把 cca 的辅助程序（cli/ 的 cca-core）当黑盒，经本机的假 Gmail（kit::mailfake：真的 TLS、只读 IMAP，
//! 假代理同时认 SOCKS5 和 HTTP CONNECT；和原来一样跑在这个测试进程自己的线程里）走一遍 setup / test / open / 登录时盯邮箱 / forget：
//! 只读的会话、登录和代理出的错、证书照常校验、没有「所有邮件」时看收件箱、Gmail 的搜索跟不上时照样找到新邮件、断线重连不漏邮件、
//! 最新的一封只开一次、链接只经 stdin。
//!
//! 每个用例一个临时 HOME，只连 127.0.0.1；钥匙串是假的 security、系统代理是假的 scutil（都在 PATH 最前面），打开链接的
//! cca _mail-open 换成记下链接的小脚本。应用专用密码是编的（abcdefghijklmnop），照样只从 stdin 交给辅助程序。
//!
//! 原来是 tests/mail_view.py（一个 test_ 方法一个用例，名字去掉 test_，先后照原来的文件）；再早它把内嵌的 Python 当库执行。
//! 不用连 Gmail 的那些检查（认登录邮件的规则、连 Gmail 的路、解析、核对、盯邮箱的假时钟、打开过的不再打开）现在在辅助程序自己的
//! 单元测试里，照原来的输入和结论一条条搬过去的（cargo test，回归的 unit 一节也跑）；各在哪里见下面那张表。

use kit::case;
use kit::fsx::{self, TempDir, Waited};
use kit::harness::{self, Ctx, Module};
use kit::json::{self, Style, Value};
use kit::mailfake::{self, Ar, Eml, Proxy, Server, FOLDERS};
use kit::proc::{Child, Cmd, Stdio};
use kit::{list, obj};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MODULE: Module = Module {
    name: "mail_view",
    src: include_str!("mail_view.rs"),
    default: true,
    tests: &[
        // 原来的 Wire
        case!(read_only_session_through_socks_and_http),
        case!(login_errors),
        case!(tls_is_always_verified),
        case!(proxy_errors_are_retryable),
        case!(no_all_mail_falls_back_to_inbox_and_odd_fetch_order),
        case!(new_mail_found_even_if_gmail_search_lags),
        case!(dropped_connection_reconnects),
        // 原来的 Commands
        case!(setup_checks_then_saves),
        case!(new_address_replaces_the_old_only_once_saved),
        case!(setup_when_gmail_refuses_for_now),
        case!(setup_keeps_auto_off),
        case!(mail_test_lists_without_showing_links),
        case!(open_hands_the_newest_link_over_stdin_once),
        case!(open_waits_for_a_new_mail),
        case!(open_reports_browser_failure),
        case!(watch_opens_the_mail_that_arrives_during_login),
        case!(watch_does_nothing_without_config_or_when_auto_is_off),
        case!(forget_removes_everything),
        // 原来的 Keystore
        case!(write_replaces_the_same_account_only_and_delete_can_pick_one),
    ],
};

const PW: &str = "abcdefghijklmnop";
const ALL: &str = "\"[Gmail]/All Mail\"";
const SPAM: &str = "\"[Gmail]/Spam\"";
/// 原来整个 Box 上的 skipUnless(MAC, …)
const ONLY_MAC: &str = "邮箱助手只在 macOS 上有（别的系统的辅助程序不带 TLS、没有钥匙串）";
/// 等一件事的时限（原来 wait_for 的 30 秒）
const WAIT: Duration = Duration::from_secs(30);

// 原来的各个类现在在哪（辅助程序的单元测试，名字是 Rust 的测试函数）：
// Rules：cli/src/cmd/mail.rs 的 link_must_be_exactly_a_claude_login_link、sender_domain_by_label_suffix、subject
// Route：cli/src/cmd/mail/route.rs 的 scutil_parse_top_level_only、system_proxy_socks_first_then_https_then_http、system_proxy_exceptions、explicit_specs、never_through_an_account_exit
// Parse：cli/src/cmd/mail/imap/tests.rs 的 internaldate、search_criteria、fetch_parse_any_order
// Judge、Clean、Links：cli/src/cmd/mail/judge.rs 的 every_check_in_mail_view（每条检查照字面；judgement_like_python 另外和原来 Python 的结果逐项比）
// Used：cli/src/cmd/mail/hunt.rs 的 claim_once_across_processes、old_records_are_dropped
// Hunt：cli/src/cmd/mail/hunt.rs 里同名的 14 个（opens_only_the_newest_and_keeps_watching …… refused_counts_as_rejected）
// Source：cli/src/cmd/mail/imap/tests.rs 的 first_round_searches_and_takes_the_latest_burst、later_rounds_fetch_new_uids_without_searching、uidvalidity_change_searches_again、a_round_cut_short_loses_nothing
// Lint：test_every_rule_is_known 在 cli/src/cmd/mail.rs 的 every_rule_is_known；test_python38_syntax_and_lazy_imports 查的是内嵌 Python 的写法，没有了
// Keystore：下面的 Keystore 那一个用例（经假的 security 走真的进程）；原来进程里的那一条在 cli/src/cmd/secrets.rs 的 write_replaces_the_same_account_only_and_delete_can_pick_one

/// 假的钥匙串（和 tests/setup.sh 的一样）：条目存在 ~/keychain/<服务名>/<账户名>，每次的命令行记在
/// ~/security.log，-i 从 stdin 读到的命令记在 ~/security.stdin；~/keychain_locked 在的时候存不进去
const SECURITY: &str = r##"#!/bin/sh
T="$HOME"; KC="$T/keychain"
printf 'SECURITY %s\n' "$*" >> "$T/security.log"
one() {
	[ $# -gt 0 ] || return 1
	cmd="$1"; shift
	svc=""; acct=""; hex=""; w=0; prev=""
	for a in "$@"; do
		case "$prev" in -s) svc="$a" ;; -a) acct="$a" ;; -X) hex="$a" ;; esac
		[ "$a" = -w ] && w=1
		prev="$a"
	done
	[ -n "$svc" ] || return 1
	if [ -n "$acct" ]; then f="$KC/$svc/$acct"; else f=$(ls "$KC/$svc/"* 2>/dev/null | head -n 1); fi
	case "$cmd" in
		add-generic-password)
			[ -f "$T/keychain_locked" ] && return 36
			mkdir -p "$KC/$svc"
			printf '%s' "$hex" | xxd -r -p > "$KC/$svc/${acct:-_}"
			return 0 ;;
		find-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			[ $w = 1 ] && { cat "$f"; echo; }
			return 0 ;;
		delete-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			rm -f "$f"; rmdir "$KC/$svc" 2>/dev/null
			return 0 ;;
	esac
	return 1
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do
		printf '%s\n' "$line" >> "$T/security.stdin"
		set -f; set -- $line; set +f
		one "$@"
	done
	exit 0
fi
one "$@"
"##;

/// 假的 scutil：系统代理就是假代理。用例都写明了连接方式；哪一条不小心走了「照系统代理」，也连不到真的 Gmail 上去
const SCUTIL: &str = "#!/bin/sh\ncat \"$HOME/scutil.txt\"\n";

/// 假的 cca：辅助程序起 `$CCA_SELF _mail-open <账号>`、链接从 stdin 给它。记下账号和链接；另记参数个数、环境变量里有没有链接；
/// 退出码从 rc 文件读（没有就是 0）
fn opener(opened: &Path, seen: &Path, rc: &Path) -> String {
    format!(
        r##"#!/bin/sh
[ "$1" = _mail-open ] || exit 9
IFS= read -r url
printf "%s %s\n" "$2" "$url" >> {opened}
printf "%s %s\n" "$#" "$(env | grep -c magic-link)" >> {seen}
exit "$(cat {rc} 2>/dev/null || echo 0)"
"##,
        opened = opened.display(),
        seen = seen.display(),
        rc = rc.display()
    )
}

// —— 夹具 ——

/// 原来的三个子类：各有各的假 Gmail 和假代理（setUpClass）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Wire,
    Commands,
    Keystore,
}

/// 应用专用密码存在哪（原来的 keystore 属性）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    /// 密钥文件（~/.cca/secrets/cca-mail）
    File,
    /// 钥匙串（假的 security）
    Keychain,
}

/// 一个子类共用的假 Gmail 和假代理。字段的先后就是收掉的先后：先不再接新的连接，再删目录（原来的 tearDownClass）
struct Fake {
    /// 留着才接新的连接（丢掉就关上：原来的 srv.ls.close()）
    _srv: Server,
    px: Proxy,
    /// 测试的 CA（辅助程序经 CCA_MAIL_CAFILE 信任它）
    ca: PathBuf,
    /// 假 Gmail 的目录：pki/、box/、ctl.json、imap.log
    dir: TempDir,
}

/// 原来 setUpModule 生成的证书（模块完了删掉）
static PKI: Mutex<Option<TempDir>> = Mutex::new(None);
/// 已经起了的各个子类的假 Gmail（模块完了收掉）
static FAKES: Mutex<Vec<(Class, Arc<Fake>)>> = Mutex::new(Vec::new());

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 原来的 setUpModule：一次性的 CA 和它签的 imap.gmail.com 的证书（openssl 生成），整个模块只生成一次。给出放证书的目录
fn pki(t: &mut Ctx) -> PathBuf {
    let mut slot = locked(&PKI);
    if slot.is_none() {
        let dir = TempDir::new("cca-mail-pki-");
        mailfake::make_pki(&dir.join("pki")).unwrap_or_else(|e| panic!("生成测试用的证书：{e}"));
        *slot = Some(dir);
        t.defer_module(|| drop(locked(&PKI).take()));
    }
    slot.as_ref().expect("刚放进去的").join("pki")
}

/// 原来每个子类的 setUpClass：拷一份证书、建好邮件夹，起假 Gmail 和假代理。同一个子类的用例共用，模块完了收掉
fn fake(t: &mut Ctx, class: Class) -> Arc<Fake> {
    if let Some((_, fake)) = locked(&FAKES).iter().find(|(c, _)| *c == class) {
        return fake.clone();
    }
    let pki = pki(t);
    let dir = TempDir::new("cca-mail-wire-");
    copy_dir(&pki, &dir.join("pki"));
    let ca = dir.join("pki").join("ca.pem");
    for (k, _, _) in FOLDERS {
        fs::create_dir_all(dir.join("box").join(k)).unwrap_or_else(|e| panic!("建邮件夹 {k}：{e}"));
    }
    let srv = Server::start(&dir).unwrap_or_else(|e| panic!("起假 Gmail：{e}"));
    let px = Proxy::start(&srv).unwrap_or_else(|e| panic!("起假代理：{e}"));
    let fake = Arc::new(Fake { _srv: srv, px, ca, dir });
    locked(&FAKES).push((class, fake.clone()));
    t.defer_module(move || locked(&FAKES).retain(|(c, _)| *c != class));
    fake
}

/// shutil.copytree（证书的目录只有一层文件）
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap_or_else(|e| panic!("建 {}：{e}", to.display()));
    for entry in fs::read_dir(from).unwrap_or_else(|e| panic!("读 {}：{e}", from.display())) {
        let entry = entry.unwrap_or_else(|e| panic!("读 {}：{e}", from.display()));
        fs::copy(entry.path(), to.join(entry.file_name())).unwrap_or_else(|e| panic!("拷 {}：{e}", entry.path().display()));
    }
}

/// 一条命令的结果：(退出码, stdout, stderr)
type Ran = (i32, String, String);

/// 一个用例的环境（原来的 Box：setUp 和它的那些方法）。每个用例一个临时 HOME；每个子类一个假 Gmail 和假代理（真的 TLS：
/// 证书是模块里一次性的 CA 签的，辅助程序经 CCA_MAIL_CAFILE 信任它——这个变量只在 HOME 换过、连的是本机时才算数）
struct Mail {
    fake: Arc<Fake>,
    /// 这个用例的临时目录（原来的 self.temp）
    top: PathBuf,
    home: PathBuf,
    /// 假的 cca _mail-open 记下的账号和链接
    opened: PathBuf,
    /// 假的 cca _mail-open 的退出码
    rc: PathBuf,
    /// 假的 cca _mail-open 看到的参数个数、环境变量里有几行带着链接
    seen: PathBuf,
    /// 给辅助程序的环境（原来的 self.env：只有这几个变量）
    env: Vec<(String, String)>,
    keystore: Store,
    socks: String,
    http: String,
    /// 密钥文件
    secret: PathBuf,
    /// 起过几个后台的（原来的 len(self.running)）：它们的输出文件按这个编号
    started: usize,
}

impl Mail {
    /// 原来的 setUp
    fn new(t: &mut Ctx, class: Class) -> Mail {
        if !cfg!(target_os = "macos") {
            t.skip(ONLY_MAC);
        }
        let fake = fake(t, class);
        let top = t.tmp("cca-mail-");
        let home = top.join("home");
        fs::create_dir_all(home.join(".cca")).expect("建 ~/.cca");
        fs::create_dir(home.join("bin")).expect("建 ~/bin");
        let (opened, rc, seen) = (top.join("opened.log"), top.join("rc"), top.join("seen.log"));
        let fake_cca = top.join("fake-cca");
        fsx::write_script(home.join("bin").join("security"), SECURITY);
        fsx::write_script(home.join("bin").join("scutil"), SCUTIL);
        fsx::write_script(&fake_cca, &opener(&opened, &seen, &rc));
        fsx::write(
            home.join("scutil.txt"),
            format!("<dictionary> {{\n  SOCKSEnable : 1\n  SOCKSPort : {}\n  SOCKSProxy : 127.0.0.1\n}}\n", fake.px.port),
        );
        let accounts = obj! {"alice@example.com" => "US-A", "工作(bob@example.com)" => "US-B"};
        fsx::write(home.join(".cca").join("accounts.json"), json::dumps(&accounts, &Style::default()));
        let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", home.join("bin").display());
        let mut env: Vec<(String, String)> = [
            ("HOME", home.display().to_string()),
            ("CCA_HOME", home.join(".cca").display().to_string()),
            ("CCA_TOOL", "claude".to_string()),
            ("CCA_SELF", fake_cca.display().to_string()),
            ("PATH", path.clone()),
            ("LANG", "en_US.UTF-8".to_string()),
            ("CCA_MAIL_CAFILE", fake.ca.display().to_string()),
            ("CCA_MAIL_POLL", "0.1".to_string()),
            ("CCA_MAIL_RETRY", "0.1".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let keystore = if class == Class::Keystore { Store::Keychain } else { Store::File };
        if keystore == Store::File {
            env.push(("CCA_KEYSTORE".to_string(), "file".to_string()));
        }
        assert_eq!(which_on("security", &path), Some(home.join("bin").join("security")), "真的钥匙串不能碰");
        for (k, _, _) in FOLDERS {
            let dir = fake.dir.join("box").join(k);
            for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("读 {}：{e}", dir.display())) {
                let p = entry.expect("读邮件夹").path();
                if p.file_name().is_some_and(|n| n.as_bytes().ends_with(b".eml")) {
                    fs::remove_file(&p).unwrap_or_else(|e| panic!("删 {}：{e}", p.display()));
                }
            }
        }
        let m = Mail {
            socks: format!("socks5://127.0.0.1:{}", fake.px.port),
            http: format!("http://127.0.0.1:{}", fake.px.port),
            secret: home.join(".cca").join("secrets").join("cca-mail"),
            fake,
            top,
            home,
            opened,
            rc,
            seen,
            env,
            keystore,
            started: 0,
        };
        m.ctl(&obj! {});
        fsx::write(m.fake.dir.join("imap.log"), "");
        m
    }

    // --- 假 Gmail

    /// 改假 Gmail 的行为：目录/ctl.json（先写好再换上去；假 Gmail 每个命令都重新读）
    fn ctl(&self, d: &Value) {
        let tmp = self.fake.dir.join("ctl.json.tmp");
        fsx::write(&tmp, json::dumps(d, &Style::default()));
        fs::rename(&tmp, self.fake.dir.join("ctl.json")).expect("换上 ctl.json");
    }

    /// 来了一封：放进 box/<邮件夹>/<UID>.eml，Gmail 是 ago 秒以前收到的
    fn put(&self, uid: u32, raw: &[u8], ago: f64, folder: &str) {
        let path = self.fake.dir.join("box").join(folder).join(format!("{uid}.eml"));
        mailfake::put_eml(&path, raw, Some(now() - ago)).unwrap_or_else(|e| panic!("放 {}：{e}", path.display()));
    }

    /// 假 Gmail 的日志
    fn log(&self) -> String {
        read_text(self.fake.dir.join("imap.log"))
    }

    // --- 辅助程序

    /// 这次的环境：self.env 照 over 改（None 是去掉）
    fn environ(&self, over: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
        let mut env = self.env.clone();
        for (k, v) in over {
            match (env.iter().position(|(name, _)| name == k), v) {
                (Some(i), Some(v)) => env[i].1 = v.to_string(),
                (Some(i), None) => drop(env.remove(i)),
                (None, Some(v)) => env.push((k.to_string(), v.to_string())),
                (None, None) => {}
            }
        }
        env
    }

    fn cmd(&self, argv: &[&str], env: &[(String, String)]) -> Cmd {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(argv).cwd(&self.home);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    /// 跑一条辅助程序的命令：(退出码, stdout, stderr)。stdin 是交给它的应用专用密码（setup）；over 改环境
    fn cca_with(&self, argv: &[&str], stdin: &str, timeout_s: u64, over: &[(&str, Option<&str>)]) -> Ran {
        let out = self
            .cmd(argv, &self.environ(over))
            .input(stdin)
            .timeout(Duration::from_secs(timeout_s))
            .output()
            .unwrap_or_else(|e| panic!("cca-core {argv:?}：{e}"));
        (out.status.py(), out.out(), out.err())
    }

    fn cca(&self, argv: &[&str]) -> Ran {
        self.cca_with(argv, "", 60, &[])
    }

    fn cca_in(&self, argv: &[&str], stdin: &str) -> Ran {
        self.cca_with(argv, stdin, 60, &[])
    }

    /// 在后台起一条辅助程序的命令（输出接在文件上，跑着的时候就能看）
    fn start(&mut self, argv: &[&str]) -> Running {
        let n = self.started;
        self.started += 1;
        let (out, err) = (self.top.join(format!("run-{n}.out")), self.top.join(format!("run-{n}.err")));
        let files = (File::create(&out).expect("建 run-N.out"), File::create(&err).expect("建 run-N.err"));
        let child = self.cmd(argv, &self.environ(&[])).stdout(files.0).stderr(files.1).spawn().expect("起 cca-core");
        Running { child, out, err }
    }

    /// 后台的 mail-watch（盯 alice@example.com）
    fn watch(&mut self, t: &mut Ctx) -> Watch {
        let n = self.started;
        self.started += 1;
        let (err, pidf) = (self.top.join(format!("watch-{n}.err")), self.top.join(format!("watch-{n}.pid")));
        let script = r#"err=$1; pidf=$2; shift 2; "$@" 2>"$err" </dev/null & echo $! > "$pidf"; read _"#;
        let mut cmd = Cmd::new("/bin/sh");
        cmd.args(["-c", script, "sh"]).arg(&err).arg(&pidf).arg(harness::helper()).args(["mail-watch", "alice@example.com"]);
        cmd.stdin(Stdio::Piped).stdout(Stdio::Null).cwd(&self.home);
        for (k, v) in self.environ(&[]) {
            cmd.env(k, v);
        }
        let sh = cmd.spawn().expect("起 sh");
        let text = wait("mail-watch 起来", || fsx::try_read_text(&pidf).filter(|s| s.ends_with('\n')));
        let pid: i32 = text.trim().parse().unwrap_or_else(|e| panic!("进程号 {text:?}：{e}"));
        let done = Arc::new(AtomicBool::new(false));
        // 用例超时（这条线卡住、Drop 轮不到）也要停掉它：它不是经 kit::proc 起的，运行器不知道它
        let seen_done = done.clone();
        t.defer(move || {
            if !seen_done.load(Ordering::SeqCst) && !gone(pid) {
                // SAFETY: 发给这个用例起的 mail-watch（还在，也还没见过它退出）
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        });
        Watch { sh, pid, err, done }
    }

    /// mail-setup 成功（应用专用密码带着空格和换行），给出它说的话
    #[track_caller]
    fn setup_ok(&self, proxy: Option<&str>) -> String {
        let proxy = proxy.unwrap_or(&self.socks).to_string();
        let (code, _, err) = self.cca_in(&["mail-setup", "Inbox@Example.com", &proxy], "abcd efgh ijkl mnop\n");
        assert_eq!(code, 0, "{err}");
        err
    }

    // --- 设置和存着的密码

    fn config_path(&self) -> PathBuf {
        self.home.join(".cca").join("config.json")
    }

    fn config(&self) -> Value {
        parse(&read_text(self.config_path()))
    }

    fn save(&self, c: &Value) {
        fsx::write(self.config_path(), json::dumps(c, &Style::pretty()) + "\n");
    }

    /// 存着的这个地址的应用专用密码（没有是空串）：密钥文件记着地址和密码；假钥匙串一个地址一个文件
    fn stored(&self, addr: &str) -> String {
        if self.keystore == Store::File {
            let Ok(bytes) = fs::read(&self.secret) else { return String::new() };
            let text = String::from_utf8(bytes).unwrap_or_else(|_| panic!("{} 不是 UTF-8", self.secret.display()));
            let d = parse(&newlines(&text));
            return if d.obj().get("host") == Some(&Value::from(addr)) { d["key"].str().to_string() } else { String::new() };
        }
        let f = self.home.join("keychain").join("cca-mail").join(addr);
        if f.exists() {
            read_text(f)
        } else {
            String::new()
        }
    }

    /// 辅助程序自己取不取得到这个地址的密码（原来直接调 ks_read）：把 mail.user 临时换成它，跑 mail-ready
    fn readable(&self, addr: &str) -> Ran {
        let keep = read_text(self.config_path());
        let mut c = parse(&keep);
        let mut mail = c["mail"].obj().clone();
        mail.insert("user", addr);
        c.as_obj_mut().expect("config.json 是个对象").insert("mail", mail);
        self.save(&c);
        let ran = self.cca(&["mail-ready"]);
        fsx::write(self.config_path(), keep);
        ran
    }

    /// App 的邮件列表（辅助程序的 gui-mail-list，只读）
    #[track_caller]
    fn mail_list(&self, argv: &[&str]) -> Value {
        let argv: Vec<&str> = std::iter::once("gui-mail-list").chain(argv.iter().copied()).collect();
        let (code, out, err) = self.cca_with(&argv, "", 60, &[("CCA_RO", Some("1"))]);
        assert_eq!((code, err.as_str()), (0, ""));
        parse(&out)
    }

    /// 假钥匙串里 cca-mail 的条目：账户名 → 密码
    fn entries(&self) -> BTreeMap<String, String> {
        let d = self.home.join("keychain").join("cca-mail");
        if !d.exists() {
            return BTreeMap::new();
        }
        fs::read_dir(&d)
            .unwrap_or_else(|e| panic!("读 {}：{e}", d.display()))
            .map(|entry| {
                let entry = entry.expect("读钥匙串的目录");
                (entry.file_name().to_string_lossy().into_owned(), read_text(entry.path()))
            })
            .collect()
    }
}

/// 正在跑的一条辅助程序命令：输出接在文件上，跑着的时候就能看
struct Running {
    child: Child,
    out: PathBuf,
    err: PathBuf,
}

impl Running {
    fn stderr(&self) -> String {
        read_lossy(&self.err)
    }

    /// 等它退出（最多 60 秒）：(退出码, stdout, stderr)
    fn wait(&mut self) -> Ran {
        let exit = self.child.wait_timeout(Duration::from_secs(60)).expect("等 cca-core").expect("等了 60 秒还没退出");
        (exit.py(), read_lossy(&self.out), self.stderr())
    }
}

/// 后台的 cca-core mail-watch（登录时 bash 起的那个）。中间隔一个 sh 当它的父进程，就像登录时的 cca：
/// over() 是 cca 不在了（sh 退出，mail-watch 看父进程没了自己收尾、记下上次的结果）；term() 是登录结束时的 TERM
struct Watch {
    sh: Child,
    pid: i32,
    err: PathBuf,
    /// 见过它退出了：以后不再往这个进程号发信号（号可能已经给了别的进程）
    done: Arc<AtomicBool>,
}

impl Watch {
    fn stderr(&self) -> String {
        if self.err.exists() {
            read_lossy(&self.err)
        } else {
            String::new()
        }
    }

    /// 放开当它父进程的 sh（关上 sh 的 stdin，它的 read 读到头就退出）。sh 20 秒都没退出是 false
    fn release(&mut self) -> bool {
        if matches!(self.sh.try_wait(), Ok(None)) {
            drop(self.sh.stdin.take());
            return matches!(self.sh.wait_timeout(Duration::from_secs(20)), Ok(Some(_)));
        }
        true
    }

    /// 还在的话发个信号，再放开 sh
    fn kill(&mut self, sig: i32) -> bool {
        if !gone(self.pid) {
            // SAFETY: 发给这个用例起的 mail-watch：它的父进程 sh 还没放开
            unsafe { libc::kill(self.pid, sig) };
        }
        self.release()
    }

    /// cca 不在了：它自己收尾。返回它说的话
    fn over(&mut self) -> String {
        assert!(self.release(), "sh 20 秒都没退出");
        wait("mail-watch 看父进程不在了自己退出", || gone(self.pid));
        self.done.store(true, Ordering::SeqCst);
        self.stderr()
    }

    /// 登录结束：TERM，马上走
    fn term(&mut self) -> String {
        assert!(self.kill(libc::SIGTERM), "sh 20 秒都没退出");
        wait("mail-watch 收到 TERM 退出", || gone(self.pid));
        self.done.store(true, Ordering::SeqCst);
        self.stderr()
    }
}

impl Drop for Watch {
    /// 原来的 addCleanup：还在的 KILL 掉，放开 sh
    fn drop(&mut self) {
        if self.done.load(Ordering::SeqCst) {
            self.release();
            return;
        }
        self.kill(libc::SIGKILL);
        let end = std::time::Instant::now() + WAIT;
        while !gone(self.pid) && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.done.store(gone(self.pid), Ordering::SeqCst);
    }
}

/// os.kill(pid, 0) 出错：没有这个进程了（原来的 Watch.gone）
fn gone(pid: i32) -> bool {
    // SAFETY: 信号 0 只是问一下，什么都不发
    unsafe { libc::kill(pid, 0) != 0 }
}

/// 等到 cond 成立（原来 Box.wait_for 的 30 秒）
fn wait<R: Waited>(what: &str, cond: impl FnMut() -> R) -> R::Out {
    fsx::wait_for(what, WAIT, cond)
}

/// time.sleep：这几处等的就是时间本身（这段时间里该发生的发生、不该发生的没发生）
fn sleep(secs: f64) {
    std::thread::sleep(Duration::from_secs_f64(secs));
}

/// time.time()
fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("时钟在 1970 年以前").as_secs_f64()
}

/// 读出来的文字像 Python 的文本模式那样换行：\r\n、\r 都成 \n（universal newlines）
fn newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Path.read_text()：UTF-8（不是就 panic），换行照文本模式
fn read_text(path: impl AsRef<Path>) -> String {
    newlines(&fsx::read_text(path))
}

/// read_text(encoding='utf-8', errors='replace')
fn read_lossy(path: &Path) -> String {
    newlines(&String::from_utf8_lossy(&fsx::read(path)))
}

/// json.loads
#[track_caller]
fn parse(text: &str) -> Value {
    json::parse(text).unwrap_or_else(|e| panic!("不是 JSON（{e}）：{text:?}"))
}

/// str.splitlines()：\n、\r、\r\n、\v、\f、\x1c、\x1d、\x1e、U+0085、U+2028、U+2029 都算换行；最后一行后面的换行不另算一行
fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if !matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            continue;
        }
        out.push(&s[start..i]);
        start = i + c.len_utf8();
        if c == '\r' && it.peek().is_some_and(|(_, next)| *next == '\n') {
            it.next();
            start += 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// shutil.which(name, path=…)：PATH 里头一个能执行、不是目录的
fn which_on(name: &str, path: &str) -> Option<PathBuf> {
    path.split(':').map(|dir| Path::new(dir).join(name)).find(|p| {
        let Ok(c) = CString::new(p.as_os_str().as_bytes()) else { return false };
        // SAFETY: c 是以 0 结尾的路径，access 只是问一下
        p.exists() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0 && !p.is_dir()
    })
}

/// assertIn
#[track_caller]
fn has(hay: &str, needle: &str) {
    assert!(hay.contains(needle), "没有 {needle:?}：{hay:?}");
}

/// assertIn，带说明
#[track_caller]
fn has_msg(hay: &str, needle: &str, msg: &str) {
    assert!(hay.contains(needle), "{msg}：没有 {needle:?}：{hay:?}");
}

/// assertNotIn
#[track_caller]
fn has_not(hay: &str, needle: &str) {
    assert!(!hay.contains(needle), "不该有 {needle:?}：{hay:?}");
}

/// assertNotIn，带说明
#[track_caller]
fn has_not_msg(hay: &str, needle: &str, msg: &str) {
    assert!(!hay.contains(needle), "{msg}：不该有 {needle:?}：{hay:?}");
}

fn strings(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// —— 原来的 Wire ——

fn read_only_session_through_socks_and_http(t: &mut Ctx) {
    let m = Mail::new(t, Class::Wire);
    m.put(3, &Eml::new("alice@example.com").build(), 60.0, "all");
    for (n, proxy) in [m.socks.clone(), m.http.clone()].iter().enumerate() {
        // folders() == [('所有邮件', ALL), ('垃圾邮件', '"[Gmail]/Spam"')]：给人看的名字在输出里，IMAP 里的名字在发出去的 EXAMINE 里
        has(&m.setup_ok(Some(proxy)), "看这些邮件夹：所有邮件、垃圾邮件");
        let d = m.mail_list(&["", "2"]);
        assert_eq!((&d["error"], &d["folders"]), (&Value::Null, &list!["所有邮件", "垃圾邮件"]));
        assert_eq!(d["rows"].list().len(), 1);
        let row = &d["rows"][0];
        // examine(ALL) 的 UIDVALIDITY 是 11、search(…) == [3]、headers([3]) 里有 From、message(3) 里有登录链接
        assert_eq!(row["msg"], obj! {"folder" => ALL, "uid" => 3, "uidvalidity" => 11});
        assert_eq!((&row["from"], &row["to"]), (&list!["no-reply@mail.anthropic.com"], &list!["alice@example.com"]));
        assert!(row["link_shape"].str().starts_with("https://claude.ai/magic-link"), "{row}");
        let (code, out, err) = m.cca(&["mail-test", "2"]);
        assert_eq!(code, 0, "{err}");
        has(&out, "通过核对，链接 https://claude.ai/magic-link（# 后面");
        // examine(ALL) 的 UIDNEXT 是 4：盯邮箱的第一轮除了搜到的，还按 UID 取到 UIDNEXT 前一封（1:3）
        let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0", if n > 0 { "again" } else { "" }]);
        assert_eq!(code, 0, "{err}");
    }
    let log = m.log();
    for line in [
        format!("IMAP EXAMINE {ALL}"),
        format!("IMAP EXAMINE {SPAM}"),
        "IMAP UID FETCH 3 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (".to_string(),
        "IMAP UID FETCH 1:3 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (".to_string(),
        "IMAP UID FETCH 3 (UID INTERNALDATE BODY.PEEK[]<0.".to_string(),
    ] {
        has(&log, &line);
    }
    has(&log, "PROXY socks5 imap.gmail.com:993");
    has(&log, "PROXY http imap.gmail.com:993");
    has(&log, "BODY.PEEK[HEADER.FIELDS");
    has_not(&log, "VIOLATION");
    has_not(&log, PW);
    let kinds: BTreeSet<String> = splitlines(&log)
        .into_iter()
        .filter(|l| l.starts_with("IMAP "))
        .map(|l| {
            let w: Vec<&str> = l.split(' ').collect();
            if w[1] == "UID" {
                format!("{} {}", w[1], w[2])
            } else {
                w[1].to_string()
            }
        })
        .collect();
    assert_eq!(
        kinds,
        strings(&["CAPABILITY", "LOGIN", "LIST", "EXAMINE", "UID SEARCH", "UID FETCH", "LOGOUT"]),
        "只读的会话只有这几种命令"
    );
}

/// 原来看的是 MailBox.open() 抛的是哪一类错（MailFatal / MailRetry）和它的 code；从外面看：setup 的退出码（3 是密码不对，
/// 重问密码；4 是别的）和说的话各不一样，登录时盯邮箱遇到不会自己好的错马上停、过会儿可能好的接着试
fn login_errors(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Wire);
    let setup = |m: &Mail, pw: &str| m.cca_in(&["mail-setup", "inbox@example.com", &m.socks], pw);
    let (code, _, err) = setup(&m, "wrongwrongwrongw"); // auth
    assert_eq!(code, 3, "{err}");
    has(&err, "Gmail 不认这个密码：要用应用专用密码（16 个字母）");
    m.ctl(&obj! {"imap_disabled" => true}); // imap
    let (code, _, err) = setup(&m, PW);
    assert_eq!(code, 4, "{err}");
    has(&err, "这个 Gmail 没开 IMAP");
    m.ctl(&obj! {"login_no" => "[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)"});
    let (code, _, err) = setup(&m, PW); // auth
    assert_eq!(code, 3, "{err}");
    has(&err, "Gmail 不认这个密码：要用应用专用密码（16 个字母）");
    m.ctl(&obj! {"login_no" => "[UNAVAILABLE] Temporary System Error"}); // 别的 NO：过一会儿再试，带着 Gmail 的原话
    let (code, _, err) = setup(&m, PW); // login
    assert_eq!(code, 4, "{err}");
    has(&err, "Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）");
    has(&err, "Temporary System Error");
    assert!(!m.secret.exists(), "没登录上的都不存");
    // 不会自己好的（MailFatal）：登录时盯邮箱说一句就不再看了
    m.ctl(&obj! {});
    m.setup_ok(None);
    for (fatal, said) in
        [(obj! {"password" => "ponmlkjihgfedcba"}, "Gmail 不认这个密码"), (obj! {"imap_disabled" => true}, "这个 Gmail 没开 IMAP")]
    {
        m.ctl(&fatal);
        let (code, _, err) = m.cca_with(&["mail-watch", "alice@example.com"], "", 30, &[]);
        assert_eq!(code, 0, "{err}");
        has(&err, said);
        has(&err, "不再看邮箱（登录照常进行）");
    }
    // 过一会儿可能就好了的（MailRetry）：接着试，第三次没连上时说一句
    m.ctl(&obj! {"login_no" => "[UNAVAILABLE] Temporary System Error"});
    let mut w = m.watch(t);
    wait("连不上 Gmail 的那一句", || w.stderr().contains("还在重试"));
    let err = w.term();
    has(&err, "连不上 Gmail（Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）），还在重试");
    has_not(&err, "不再看邮箱");
}

fn tls_is_always_verified(t: &mut Ctx) {
    let m = Mail::new(t, Class::Wire);
    // 不给测试的 CA：假 Gmail 的证书就是「不认识的机构签的」，校验过不去，什么都不存
    for ca in ["", "/nonexistent/ca.pem"] {
        let (code, _, err) = m.cca_with(&["mail-setup", "inbox@example.com", &m.socks], PW, 60, &[("CCA_MAIL_CAFILE", Some(ca))]);
        assert_eq!(code, 4, "{err}");
        has(&err, "Gmail 的证书校验没通过");
        has(&err, "cca 不会跳过校验");
        assert!(!m.secret.exists());
        has_not_msg(&m.log(), "IMAP ", "握手没成：一条 IMAP 命令都没发出去");
    }
    // 证书校验过不去是不会自己好的（MailFatal，code 是 tls）：登录时盯邮箱说一句就停
    m.setup_ok(None);
    let (code, _, err) = m.cca_with(&["mail-watch", "alice@example.com"], "", 30, &[("CCA_MAIL_CAFILE", Some(""))]);
    assert_eq!(code, 0, "{err}");
    has(&err, "Gmail 的证书校验没通过");
    has(&err, "不再看邮箱（登录照常进行）");
    // 原来的后一半——HOME 是用户真正的主目录时，只给测试的 CA 不认——从外面测不了（辅助程序得在真的主目录下跑）：
    // 认不认的规则在 cli/src/cmd/mail/route.rs 的单元测试 test_ca_only_in_a_swapped_home_and_to_this_machine 里
}

/// 原来看的是 MailRetry 和它的 code（proxy / net）；从外面看：setup 说的话（这两种后面都带「连接方式可以指定」那一句），
/// 登录时盯邮箱接着试、不停
fn proxy_errors_are_retryable(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Wire);
    let hint = "连 Gmail 的方式可以指定：cca mail setup --proxy socks5://127.0.0.1:7897（direct 是直连）";
    for (mode, proxy, said) in [
        ("refuse", m.http.clone(), "HTTP 代理不肯连 imap.gmail.com:993（HTTP/1.1 403 Forbidden）"),
        ("auth", m.socks.clone(), "SOCKS5 代理要认证，或者不认这个请求（cca 只用不带认证的 SOCKS5）"),
    ] {
        m.ctl(&obj! {"proxy" => mode});
        let (code, _, err) = m.cca_in(&["mail-setup", "inbox@example.com", &proxy], PW);
        assert_eq!(code, 4, "{mode}");
        has_msg(&err, &format!("{said}。{hint}"), mode); // proxy
    }
    m.ctl(&obj! {});
    let (code, _, err) = m.cca_in(&["mail-setup", "inbox@example.com", "socks5://127.0.0.1:9"], PW);
    assert_eq!(code, 4, "{err}");
    has(&err, "连不上 代理 127.0.0.1:9（"); // net；括号里是系统的说法（Connection refused）
    has(&err, &format!("）。{hint}"));
    assert!(!m.secret.exists());
    m.setup_ok(None);
    let changes: [(fn(&Mail), &str); 3] = [
        (|m| m.ctl(&obj! {"proxy" => "refuse"}), "SOCKS5 代理没能连上 imap.gmail.com:993（错误码 5）"),
        (|m| m.ctl(&obj! {"proxy" => "auth"}), "SOCKS5 代理要认证"),
        (
            |m| {
                let mut c = m.config();
                let mail = obj! {"user" => "inbox@example.com", "proxy" => "socks5://127.0.0.1:9"};
                c.as_obj_mut().expect("config.json 是个对象").insert("mail", mail);
                m.save(&c);
            },
            "连不上 代理 127.0.0.1:9（",
        ),
    ];
    for (change, said) in changes {
        change(&m);
        let mut w = m.watch(t);
        wait(said, || w.stderr().contains("还在重试"));
        let err = w.term();
        has(&err, &format!("连不上 Gmail（{said}"));
        has_not(&err, "不再看邮箱");
    }
}

fn no_all_mail_falls_back_to_inbox_and_odd_fetch_order(t: &mut Ctx) {
    let m = Mail::new(t, Class::Wire);
    m.ctl(&obj! {"no_all" => true, "uid_after_literal" => true});
    m.put(2, &Eml::new("alice@example.com").build(), 10.0, "inbox");
    // folders() == [('收件箱', 'INBOX'), ('垃圾邮件', '"[Gmail]/Spam"')]
    has(&m.setup_ok(None), "看这些邮件夹：收件箱、垃圾邮件");
    let d = m.mail_list(&["", "2"]);
    assert_eq!((&d["error"], &d["folders"]), (&Value::Null, &list!["收件箱", "垃圾邮件"]));
    let msgs: Vec<Value> = d["rows"].list().iter().map(|r| r["msg"].clone()).collect();
    assert_eq!(msgs, [obj! {"folder" => "INBOX", "uid" => 2, "uidvalidity" => 11}], "UID 写在 literal 后面的也认得出");
    // headers((1, 5)) 认出 2：按 UID 的区间取头（盯邮箱第一轮的 1:2），那封照样找到、打开
    let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(splitlines(&read_text(&m.opened)).len(), 1);
    let log = m.log();
    for line in [
        "IMAP EXAMINE INBOX".to_string(),
        format!("IMAP EXAMINE {SPAM}"),
        "IMAP UID FETCH 1:2 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (".to_string(),
    ] {
        has(&log, &line);
    }
    has_not(&log, &format!("IMAP EXAMINE {ALL}"));
    has_not(&log, "VIOLATION");
}

fn new_mail_found_even_if_gmail_search_lags(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Wire);
    m.ctl(&obj! {"index_lag" => 3600}); // 刚到的邮件 SEARCH 一律搜不到
    m.setup_ok(None);
    let mut p = m.start(&["mail-open", "alice@example.com", "20"]);
    // 第一轮什么都没有（poll() == []）：说要等
    wait("第一轮看完", || p.stderr().contains("等 alice@example.com 的登录邮件"));
    assert!(!m.opened.exists());
    m.put(1, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-LAG").build(), 0.0, "all");
    let (code, _, err) = p.wait();
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_text(&m.opened), "alice@example.com https://claude.ai/magic-link#tok-LAG\n");
    let log = m.log();
    has_msg(&log, "IMAP UID FETCH 1:1 (UID INTERNALDATE BODY.PEEK[HEADER.FIELDS (", "不靠搜索：按 UIDNEXT 往后取");
    has_not(&log, "VIOLATION");
}

fn dropped_connection_reconnects(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Wire);
    m.setup_ok(None);
    fsx::write(m.fake.dir.join("imap.log"), "");
    m.put(1, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-DROP").build(), 5.0, "all");
    m.ctl(&obj! {"drop" => "UID SEARCH"});
    let mut w = m.watch(t);
    wait("这一轮半路断开", || m.log().contains("DROP UID SEARCH")); // poll() 抛 MailRetry
    assert!(!m.opened.exists());
    m.ctl(&obj! {});
    wait("重连以后找到那一封", || m.opened.exists() && !read_text(&m.opened).is_empty()); // 再 poll()：[1]
    sleep(0.3);
    let err = w.term();
    assert_eq!(read_text(&m.opened), "alice@example.com https://claude.ai/magic-link#tok-DROP\n");
    assert!(m.log().matches("IMAP LOGIN").count() >= 2, "断了以后重新登录：{}", m.log());
    has(&err, "已连上 inbox@example.com");
    has_not(&m.log(), "VIOLATION");
}

// —— 原来的 Commands：cca mail setup / test / open 和登录时盯邮箱：跑辅助程序的 mail-setup、mail-test、mail-open、mail-watch、
// mail-forget……（打开链接的 cca _mail-open 换成记下链接的小脚本） ——

fn setup_checks_then_saves(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    let (code, _, err) = m.cca_in(&["mail-setup", "inbox@example.com", &m.socks], "wrongwrongwrongw");
    assert_eq!(code, 3, "{err}");
    has(&err, "应用专用密码");
    assert!(!m.secret.exists(), "没登录上不存");
    assert_eq!(m.cca_in(&["mail-setup", "not-an-address", ""], PW).0, 2);
    assert_eq!(m.cca_in(&["mail-setup", "a;b@example.com", ""], PW).0, 2);
    assert_eq!(m.cca_in(&["mail-setup", "inbox@example.com", "socks5://127.0.0.1:9"], PW).0, 4);
    let err = m.setup_ok(None);
    has(&err, "所有邮件、垃圾邮件");
    let c = m.config();
    assert_eq!(c["mail"], obj! {"user" => "inbox@example.com", "proxy" => m.socks.as_str()});
    let f = &m.secret;
    assert_eq!(fsx::mode(f), 0o600);
    assert_eq!(fsx::mode(f.parent().expect("密钥文件在一个目录里")), 0o700);
    assert_eq!(parse(&read_text(f))["key"], Value::from(PW), "空格去掉了");
    // ks_read('cca-mail', 'inbox@example.com') == (0, PW)：记的是这个地址和这个密码，辅助程序取得到、拿它登录得上
    assert_eq!(m.stored("inbox@example.com"), PW);
    assert_eq!(m.readable("inbox@example.com").0, 0);
    assert_eq!(m.cca(&["mail-test", "1"]).0, 0);
    // ks_read('cca-mail', 'other@example.com')[1] == ''
    assert_eq!(m.stored("other@example.com"), "", "换了地址取不到旧的");
    let (code, _, err) = m.readable("other@example.com");
    assert_eq!(code, 1, "{err}");
    has(&err, "密钥文件里没有应用专用密码");
    let (_, out, _) = m.cca(&["mail-status"]);
    has(&out, "inbox@example.com");
    has(&out, "自动打开");
    assert_eq!(m.cca(&["mail-get-user"]).1, "inbox@example.com\n");
    assert!(!m.home.join("security.log").exists(), "用密钥文件时不碰钥匙串");
}

/// 原来把 ks_write 换成总是失败的；这里用钥匙串那一支，让假的 security 存不进去（钥匙串锁着）
fn new_address_replaces_the_old_only_once_saved(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Commands);
    m.keystore = Store::Keychain;
    let at = m.env.iter().position(|(k, _)| k == "CCA_KEYSTORE").expect("用密钥文件的用例才有 CCA_KEYSTORE");
    m.env.remove(at);
    m.setup_ok(None);
    let user = |m: &Mail| m.config()["mail"]["user"].str().to_string();
    assert_eq!((m.stored("inbox@example.com"), user(&m)), (PW.to_string(), "inbox@example.com".to_string()));
    m.ctl(&obj! {"user" => "new@example.com", "password" => PW});
    fsx::write(m.home.join("keychain_locked"), ""); // 存不进去（钥匙串锁着、磁盘满了）
    let (code, _, err) = m.cca_in(&["mail-setup", "new@example.com", ""], PW);
    assert_eq!(code, 5, "{err}");
    has(&err, "应用专用密码没能存进钥匙串（钥匙串锁着？）");
    assert_eq!(
        (m.stored("inbox@example.com"), user(&m)),
        (PW.to_string(), "inbox@example.com".to_string()),
        "原来的密码、设置都还在，照样能用"
    );
    assert_eq!(m.cca(&["mail-ready"]).0, 0);
    fs::remove_file(m.home.join("keychain_locked")).expect("删 keychain_locked");
    assert_eq!(m.cca_in(&["mail-setup", "new@example.com", ""], PW).0, 0);
    assert_eq!(
        (m.stored("new@example.com"), m.stored("inbox@example.com"), user(&m)),
        (PW.to_string(), String::new(), "new@example.com".to_string())
    );
    assert_eq!(m.cca(&["mail-ready"]).0, 0);
}

fn setup_when_gmail_refuses_for_now(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.ctl(&obj! {"login_no" => "[UNAVAILABLE] Temporary System Error"});
    let (code, _, err) = m.cca_in(&["mail-setup", "inbox@example.com", &m.socks], PW);
    assert_eq!(code, 4, "不是密码不对：不重问密码");
    has(&err, "Gmail 这次没让登录（[UNAVAILABLE] Temporary System Error）");
    has_not(&err, "连 Gmail 的方式可以指定");
}

fn setup_keeps_auto_off(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.save(&obj! {"mail" => obj! {"user" => "inbox@example.com", "auto" => false}});
    m.setup_ok(None);
    // assertIs(…, False)：就是 false，不是别的假值
    assert_eq!(m.config()["mail"]["auto"], Value::Bool(false));
}

fn mail_test_lists_without_showing_links(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    m.put(5, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-SECRET1").build(), 120.0, "all");
    m.put(6, &Eml::new("carol@example.com").link("https://claude.ai/magic-link#tok-SECRET2").build(), 60.0, "all");
    m.put(7, &Eml::new("alice@example.com").ar(Ar::Fail).link("https://claude.ai/magic-link#tok-SECRET3").build(), 30.0, "all");
    let eml = Eml::new("alice@example.com")
        .subject("Secure link to log in to Claude.ai \x1b[2K\x1b[1A")
        .ar(Ar::Fail)
        .frm("\"\x1b]52;c;eA==\x07\" <x@mail.anthropic.com>");
    m.put(8, &eml.build(), 20.0, "all");
    let (code, out, err) = m.cca(&["mail-test", "2"]);
    assert_eq!(code, 0, "{err}");
    has_not(&(out.clone() + &err), "SECRET");
    has_not_msg(&out, "\x1b", "发件人写的转义序列不原样打到终端");
    has(&out, "有 4 封");
    has(&out, "通过核对，链接 https://claude.ai/magic-link（# 后面");
    has(&out, "不是账号表里的邮箱");
    has(&out, "Gmail 的验证结果里没有 Anthropic 的签名通过");
    has_not(&m.log(), "VIOLATION");
    // App 的邮件列表（_gui mail-list）也一样：只给链接的样子
    let d = m.mail_list(&["", "2"]);
    assert_eq!(d["rows"].list().len(), 4);
    has_not(&json::dumps(&d, &Style::default()), "SECRET");
}

fn open_hands_the_newest_link_over_stdin_once(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    m.put(5, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-A1").build(), 600.0, "all");
    m.put(6, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-A2").build(), 300.0, "all");
    m.put(7, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-OLD").build(), 3.0 * 3600.0, "all");
    let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_text(&m.opened), "alice@example.com https://claude.ai/magic-link#tok-A2\n");
    let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0"]);
    assert_eq!(code, 1, "{err}");
    has(&err, "已经打开过了");
    let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0", "again"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_text(&m.opened).matches("tok-A2").count(), 2);
    has_not(&err, "tok-");
    let (code, _, err) = m.cca(&["mail-open", "工作(bob@example.com)", "0"]);
    assert_eq!(code, 1, "{err}");
    has(&err, "最近 30 分钟里没有 bob@example.com 的登录邮件");
    has_not(&m.log(), "VIOLATION");
    // 链接只经 stdin 给 cca _mail-open：命令行上只有 _mail-open 和账号，环境变量里没有它；也不存盘（used.json 只记摘要）
    let seen = read_text(&m.seen);
    assert_eq!(splitlines(&seen).into_iter().collect::<BTreeSet<&str>>(), BTreeSet::from(["2 0"]));
    let mail = m.home.join(".cca").join("mail");
    assert_eq!((fsx::mode(&mail), fsx::mode(mail.join("used.json"))), (0o700, 0o600));
    for entry in fs::read_dir(&mail).expect("读 ~/.cca/mail") {
        let entry = entry.expect("读 ~/.cca/mail");
        let name = entry.file_name().to_string_lossy().into_owned();
        has_not_msg(&read_text(entry.path()), "magic", &name);
        has_not_msg(&read_text(entry.path()), "tok-", &name);
    }
}

fn open_waits_for_a_new_mail(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    let mut p = m.start(&["mail-open", "alice@example.com", "20"]);
    wait("第一轮看完、说要等", || p.stderr().contains("等 alice@example.com 的登录邮件"));
    m.put(9, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-W").build(), 0.0, "all");
    let (code, _, err) = p.wait();
    assert_eq!(code, 0, "{err}");
    has(&err, "等 alice@example.com 的登录邮件");
    has(&read_text(&m.opened), "tok-W");
}

fn open_reports_browser_failure(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    fsx::write(&m.rc, "3");
    m.put(5, &Eml::new("alice@example.com").build(), 10.0, "all");
    let (code, _, err) = m.cca(&["mail-open", "alice@example.com", "0"]);
    assert_eq!(code, 1, "{err}");
    has(&err, "没能打开浏览器");
    assert_eq!(splitlines(&read_text(&m.opened)).len(), 3);
    fsx::write(&m.rc, "0");
    assert_eq!(m.cca(&["mail-open", "alice@example.com", "0"]).0, 0, "没用掉：修好后照样能开");
}

fn watch_opens_the_mail_that_arrives_during_login(t: &mut Ctx) {
    let mut m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    m.put(4, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-before").build(), 120.0, "all");
    let mut w = m.watch(t);
    wait("连上 Gmail", || w.stderr().contains("已连上 inbox@example.com"));
    sleep(0.3);
    m.put(5, &Eml::new("bob@example.com").link("https://claude.ai/magic-link#tok-bob").build(), 0.0, "all");
    sleep(0.3);
    m.put(6, &Eml::new("alice@example.com").link("https://claude.ai/magic-link#tok-login").build(), 0.0, "all");
    wait("打开登录邮件", || m.opened.exists() && read_text(&m.opened).contains("tok-login"));
    sleep(0.5);
    let err = w.over(); // 登录结束（原来是 alive() 变假）
    assert_eq!(
        read_text(&m.opened),
        "alice@example.com https://claude.ai/magic-link#tok-login\n",
        "登录开始前就到了的、别的账号的都不开"
    );
    has(&err, "已连上 inbox@example.com");
    has_not_msg(&err, "没通过核对", "别的账号的登录邮件不说");
    has(parse(&read_text(m.home.join(".cca").join("mail").join("last.json")))["text"].str(), "打开了登录邮件里的链接");
}

fn watch_does_nothing_without_config_or_when_auto_is_off(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    let nothing: Ran = (0, String::new(), String::new());
    assert_eq!(m.cca_with(&["mail-watch", "alice@example.com"], "", 30, &[]), nothing); // 没配置：马上返回
    m.setup_ok(None);
    let mut c = m.config();
    c.as_obj_mut()
        .and_then(|c| c.get_mut("mail"))
        .and_then(Value::as_obj_mut)
        .expect("config.json 里有 mail")
        .insert("auto", false);
    m.save(&c);
    assert_eq!(m.cca_with(&["mail-watch", "alice@example.com"], "", 30, &[]), nothing);
    has_not(m.log().split("IMAP LOGOUT").last().unwrap_or_default(), "LOGIN");
}

fn forget_removes_everything(t: &mut Ctx) {
    let m = Mail::new(t, Class::Commands);
    m.setup_ok(None);
    // 原来直接调 mail_claim('x') 建出 ~/.cca/mail；这里真打开一封（used.json、last.json 都在里面）
    m.put(5, &Eml::new("alice@example.com").build(), 10.0, "all");
    assert_eq!(m.cca(&["mail-open", "alice@example.com", "0"]).0, 0);
    assert!(m.home.join(".cca").join("mail").join("used.json").exists());
    assert_eq!(m.cca(&["mail-forget"]).0, 0);
    assert!(!m.secret.exists());
    assert!(!m.home.join(".cca").join("mail").exists());
    assert!(!m.config().obj().contains_key("mail"));
    assert_eq!(m.cca(&["mail-forget"]).0, 0, "没配置过也照样能跑");
}

// —— 原来的 Keystore：ks_read / ks_write / ks_del 走钥匙串的那一支：假的 security 把条目放在 ~/keychain 里（和 tests/setup.sh 的
// 一样认 -a、-s、-X）。原来在进程里直接调这三个函数（那一条在 cli/src/cmd/secrets.rs 的单元测试里）；这里经 mail-setup / mail-forget
// 走真的进程 ——

fn write_replaces_the_same_account_only_and_delete_can_pick_one(t: &mut Ctx) {
    let m = Mail::new(t, Class::Keystore);
    let (a, b) = ("abcdefghijklmnop", "ponmlkjihgfedcba");
    let setup = |user: &str, pw: &str, acct: &str| {
        m.ctl(&obj! {"user" => user, "password" => pw});
        m.cca_in(&["mail-setup", user, &m.socks, acct], pw)
    };
    let pairs = |items: &[(&str, &str)]| -> BTreeMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    };
    assert_eq!(setup("a@example.com", a, "").0, 0); // ks_write('svc', 'a@example.com', a)
    assert_eq!(setup("b@example.com", b, "alice@example.com").0, 0); // ks_write('svc', 'b@example.com', b)
    assert_eq!(m.entries(), pairs(&[("a@example.com", a), ("b@example.com", b)]));
    assert_eq!(setup("b@example.com", a, "alice@example.com").0, 0); // ks_write('svc', 'b@example.com', a)：只换同一个账户名的
    assert_eq!(m.entries(), pairs(&[("a@example.com", a), ("b@example.com", a)]));
    // ks_read：两个地址各自取得到（默认的那个、alice 单独用的那个）
    assert_eq!((m.cca(&["mail-ready"]).0, m.cca(&["mail-ready", "alice@example.com"]).0), (0, 0));
    assert_eq!(m.cca(&["mail-forget", "default"]).0, 0); // ks_del('svc', 'a@example.com')
    assert_eq!(m.entries().into_keys().collect::<BTreeSet<String>>(), strings(&["b@example.com"]), "只删这个账户名的");
    assert_eq!(m.cca(&["mail-forget"]).0, 0); // ks_del('svc')
    assert_eq!(m.entries(), BTreeMap::new());
    let calls = read_text(m.home.join("security.log"));
    has(&calls, "SECURITY -i\n");
    has(&calls, "find-generic-password -a b@example.com -s cca-mail -w");
    for secret in [a.to_string(), b.to_string(), kit::crypto::hex(a.as_bytes()), kit::crypto::hex(b.as_bytes())] {
        has_not_msg(&calls, &secret, "秘密不进命令行");
    }
    has_msg(&read_text(m.home.join("security.stdin")), &kit::crypto::hex(a.as_bytes()), "只从 stdin 给 security -i");
    assert!(!m.secret.exists(), "用钥匙串时不写密钥文件");
}
