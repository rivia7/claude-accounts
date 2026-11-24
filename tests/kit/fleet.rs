//! Team 的同步服务器和设备的夹具（原来 tests/presence.py 里、别的几份测试也拿去用的那些）。把辅助程序和服务器都当黑盒：
//! 服务器是真的进程（用哪一份照 kit::server），设备是一个临时的 HOME 加几个假命令，管理员是测试自己（照 server/README.md 签名）。
//!
//! - [`Server`]：一个真的 Team 的同步服务器进程（127.0.0.1、随机端口；[`ServerProc`] 是它底下的进程和它在 stderr 上说的话，
//!   要自己拼参数起服务器的测试直接用它）。服务器没起来就退出了时马上说出是哪条命令、退出码、它说了什么，不干等到超时
//! - [`signed_call`]、[`SignedCalls`]、[`unknown`]：照文档自己签一个请求发给服务器
//! - [`Team`]：测试里的一个 Team，测试拿着根密钥扮演管理员（登记、移除设备）
//! - [`Device`]：一台设备——自己的 HOME（~/.cca），辅助程序在里面跑；curl、ps、cca、security 是它 ~/bin 里的假命令
//!   （[`CURL`]、[`PS`]、[`SELF`]、[`NO_KEYCHAIN`]、[`KEYCHAIN`]）
//! - 命令行会话和上报进程：[`session`]、[`agent`]、[`rounds`]、[`more_rounds`]、[`status`]（原来的 Sessions）
//!
//! 名字、参数的先后、默认值都照原来的 Python；Python 里带默认值的参数这里分成两个写法（[`Device::login`] / [`Device::login_as`]），
//! 或者收进一个带 Default 的结构（[`ServerOpts`]、[`CliSession`]）。写文件用的是 `json.dumps` 默认的样子（和原来逐字节一样）。
//!
//! ```ignore
//! let d = t.tmp("cca-fleet-");
//! pki::make_cert(&d);
//! let team = Team::new(&d);                                   // 根密钥是 d/root
//! let mut srv = Server::fresh(&d, &team.root);                // 用例完了自己停掉
//! let pin = pki::openssl_pin(d.join("cert.pem"));
//! let mut dev = Device::new(&d.join("alice-mbp"), "alice-mbp", &obj! { Device::KEY => "US-A" });
//! dev.join(&team, &srv.url(), &[&pin]);
//! team.only(srv.port, &[&dev]);                               // 管理员在服务器上登记这台设备
//! let (st, j) = dev.post(srv.port, "view", &obj! { "accounts" => list![spec::aid_of(&team.salt, "claude", "alice@example.com")] });
//! assert_eq!((st, j["me_name"].str()), (200, "alice-mbp"));
//! ```

use crate::fsx::{self, wait_for};
use crate::harness::{self, Ctx};
use crate::json::{self, Obj, Style, Value};
use crate::proc::{Child, Cmd, Stdio};
use crate::sandbox;
use crate::{crypto, http, obj, pki, server, spec, tls};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use crate::sandbox::{ident, Ran};

/// 等一件事最多等多久（原来 wait_for 的默认值）：机器再忙也该等得到
pub const WAIT: Duration = Duration::from_secs(60);
/// 服务器起来（写下端口）最多等多久。原来 Server 只等 5 秒、FreshServer 再接着等 60 秒；这里一律等这么久：
/// 几个模块、整套回归同时跑的时候 5 秒不够，等得久只在出毛病时才碰得到
pub const STARTUP: Duration = Duration::from_secs(65);
/// [`Server::req`] 给的状态码：服务器没读完请求就回绝、断开了（body 太大的时候；原来是 None）
pub const DROPPED: u16 = 0;

/// 假 ps 说的启动时间（ps -o lstart 的写法）
pub const T0: &str = "Sat Nov  7 12:08:55 2026";
/// 不可能有这个进程号：上报进程盯不上它（kqueue），每秒回来看一次
pub const SESSION_PID: i64 = 999999;

/// [`T0`] 是 1970 年以来的第几秒：照这个进程的时区算（`time.mktime(time.strptime(T0))`；设备的环境里带着同一个 TZ）
pub fn t0_epoch() -> i64 {
    // SAFETY: 全是零的 tm 是有效的；mktime 只读写这一个结构
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    (tm.tm_year, tm.tm_mon, tm.tm_mday) = (2026 - 1900, 10, 7);
    (tm.tm_hour, tm.tm_min, tm.tm_sec) = (12, 8, 55);
    tm.tm_isdst = -1;
    // SAFETY: 同上
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// 现在是 1970 年以来的第几秒（`time.time()`）
pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// 设备上的 curl：请求照发（真的 curl、真的服务器），顺手记下来——~/curl.argv 是每次的命令行（查凭证从不出现在命令行里），
/// ~/sent/<第几个>.json 是签过名的请求，.op 是哪个接口，.reply 是服务器回的内容，.code 是它回的状态码（回来了才有：没发出去是 0）。
/// ~/reply/<接口> 在时不发，照它回（第一行是状态码，后面是回的内容）：扮演还不认这个接口的旧服务器
pub const CURL: &str = r#"printf '%s\n' "$*" >> "$HOME/curl.argv"
mkdir -p "$HOME/sent"
tmp="$HOME/sent/.req.$$"
cat > "$tmp"
n=0; while ! ln "$tmp" "$HOME/sent/$n.json" 2>/dev/null; do n=$((n + 1)); done
rm -f "$tmp"
for a in "$@"; do url="$a"; done
op="${url##*/}"
printf '%s' "$op" > "$HOME/sent/$n.op"
if [ -f "$HOME/reply/$op" ]; then
	out="$(tail -n +2 "$HOME/reply/$op")
$(head -n 1 "$HOME/reply/$op")"; rc=0
else
	out=$(/usr/bin/curl "$@" < "$HOME/sent/$n.json"); rc=$?
fi
printf '%s' "$out" | sed '$d' > "$HOME/sent/$n.reply"
printf '%s' "$out" | tail -n 1 > "$HOME/sent/$n.code.tmp"
mv "$HOME/sent/$n.code.tmp" "$HOME/sent/$n.code"
printf '%s' "$out"
exit $rc
"#;
/// 设备上的 ps：进程表照 ~/ps.out（没有就是一个进程都没有）；问某个进程的启动时间一律说 [`T0`]。每问一次进程表记一笔
/// （上报进程转了几圈）
pub const PS: &str = r#"case " $* " in
	*" -p "*) for a in "$@"; do pid="$a"; done; printf '%s %s\n' "$pid" "Sat Nov  7 12:08:55 2026" ;;
	*) echo x >> "$HOME/ps.log"; cat "$HOME/ps.out" 2>/dev/null ;;
esac
exit 0
"#;
/// 后台起的 cca（记下 key 以后的 _key-sync 这些）：只记一笔（自己的进程号、参数），不真的起
pub const SELF: &str = r#"echo "$$ $*" >> "$HOME/kicks.log"
"#;
/// 默认：钥匙串碰不到（key 存成文件，CCA_KEYSTORE=file）
pub const NO_KEYCHAIN: &str = r#"echo "测试不碰钥匙串：security $*" >> "$HOME/security.log"
exit 1
"#;
/// 假的钥匙串（要它的用例才装）：条目是 ~/keychain/<服务名>/<账户名> 这些文件；~/keychain_stuck 在时删不掉（钥匙串锁着）
pub const KEYCHAIN: &str = r#"KC="$HOME/keychain"
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
		add-generic-password) mkdir -p "$KC/$svc"; printf '%s' "$hex" | xxd -r -p > "$KC/$svc/${acct:-_}"; return 0 ;;
		find-generic-password) [ -n "$f" ] && [ -f "$f" ] || return 44; [ $w = 1 ] && { cat "$f"; echo; }; return 0 ;;
		delete-generic-password) [ -n "$f" ] && [ -f "$f" ] || return 44; [ -f "$HOME/keychain_stuck" ] && return 51; rm -f "$f"; return 0 ;;
	esac
	return 1
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do set -f; set -- $line; set +f; one "$@"; done
	exit 0
fi
one "$@"
"#;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 路径当参数、当环境变量的值用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

/// `json.dumps(v)` 默认的样子（", " 和 ": "、非 ASCII 转义）：原来的夹具就是这样写文件的
fn dumps(v: &Value) -> String {
    json::dumps(v, &Style::default())
}

// ---- 服务器 ----

/// 一个起来了的服务器进程（命令的开头照 kit::server：`server_argv()` 后面接给的参数），和它在 stderr 上说的话
/// （另有一条线一直在读：说得再多也不会把它卡住）。离开作用域时还没停的话停掉（TERM，2 秒后 KILL）
pub struct ServerProc {
    child: Child,
    said: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    /// 命令的开头（没起来时说的是它）
    head: String,
    stopped: bool,
}

impl ServerProc {
    /// 起服务器：`server_argv() + args`；环境只有 env 给的（服务器自己只看 CCA_TEAM_ROOT、CCA_HUB_HOSTED、CCA_WEB_* 这几个）
    /// 和 LC_ALL=en_US.UTF-8（它在 stderr 上说中文）。标准输入、输出是 /dev/null。起不来就 panic
    pub fn spawn<S: AsRef<str>>(args: &[S], env: &[(&str, &str)]) -> ServerProc {
        let mut argv = server::server_argv();
        let head = argv.join(" ");
        argv.extend(args.iter().map(|a| a.as_ref().to_string()));
        let mut cmd = Cmd::new(&argv[0]);
        cmd.args(&argv[1..]).env("LC_ALL", "en_US.UTF-8").envs(env.iter().copied());
        cmd.stdout(Stdio::Null).stderr(Stdio::Piped);
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("服务器起不来（{head}）：{e}"));
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut pipe = child.stderr.take().expect("stderr 接着管道");
        let sink = said.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                locked(&sink).extend_from_slice(&buf[..n]);
            }
        });
        ServerProc { child, said, reader: Some(reader), head, stopped: false }
    }

    /// 进程号
    pub fn pid(&self) -> i32 {
        self.child.pid()
    }

    /// 到现在为止它在 stderr 上说的话
    pub fn said(&self) -> String {
        String::from_utf8_lossy(&locked(&self.said)).into_owned()
    }

    /// 它退出了以后，等读 stderr 的那条线读到头（最多 2 秒：它留下的子进程还拿着 stderr 的话不干等）
    fn drain(&mut self) {
        let end = Instant::now() + Duration::from_secs(2);
        while self.reader.as_ref().is_some_and(|r| !r.is_finished()) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.reader.as_ref().is_some_and(|r| r.is_finished()) {
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    /// 还在运行是 None；已经退出了的话：退出码（Python 的 returncode：被信号杀掉的是负的信号号）
    pub fn poll(&mut self) -> Option<i32> {
        self.child.try_wait().ok().flatten().map(|exit| exit.py())
    }

    /// 等服务器写下端口的地方用：它没起来就退出了时，是哪条命令、退出码、在 stderr 上说了什么——不然要干等到超时，
    /// 看到的也只是“没等到”。还在运行是 None
    pub fn exited(&mut self) -> Option<String> {
        let code = self.poll()?;
        self.drain();
        let said = self.said();
        let said = said.trim();
        let tail = if said.is_empty() { String::new() } else { format!("：{said}") };
        Some(format!("服务器没起来就退出了（{}，退出码 {code}）{tail}", self.head))
    }

    /// 等服务器写下一个文件（端口文件），给出里面写的（不是空的才算写好了）。服务器没起来就退出了马上 panic、带上它说的话
    /// （见 [`ServerProc::exited`]）；到 limit 还没写下就 panic，说“没等到：what”
    pub fn wait_file(&mut self, path: &Path, what: &str, limit: Duration) -> String {
        let end = Instant::now() + limit;
        loop {
            if let Some(got) = fsx::try_read_text(path).filter(|t| !t.trim().is_empty()) {
                return got;
            }
            if let Some(why) = self.exited() {
                panic!("{why}");
            }
            if Instant::now() >= end {
                panic!("没等到：{what}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 给它发一个信号（`libc::SIGTERM` 这样的号）
    pub fn signal(&self, sig: i32) {
        let _ = self.child.signal(sig);
    }

    /// 最多等 limit：退出了给出退出码（Python 的 returncode），还在运行是 None
    pub fn wait(&mut self, limit: Duration) -> Option<i32> {
        self.child.wait_timeout(limit).unwrap_or_else(|e| panic!("等服务器退出：{e}")).map(|exit| exit.py())
    }

    /// 停掉（TERM；5 秒还不退出就 KILL），给出它在 stderr 上说的话。5 秒里没停下来的话给出的是 Err（话照样带着）
    fn halt(&mut self) -> Result<String, String> {
        self.stopped = true;
        let _ = self.child.signal(libc::SIGTERM);
        let in_time = matches!(self.child.wait_timeout(Duration::from_secs(5)), Ok(Some(_)));
        if !in_time {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.drain();
        let said = self.said();
        if in_time {
            Ok(said)
        } else {
            Err(said)
        }
    }

    /// 停掉（TERM），返回它在 stderr 上说的话（停过了再停是空的）。5 秒里没停下来：杀掉，panic（原来是 wait(5) 超时）
    pub fn stop(&mut self) -> String {
        if self.stopped {
            return String::new();
        }
        self.halt().unwrap_or_else(|said| panic!("服务器（{}）收到 TERM 5 秒还没退出：杀掉了。它说的：{said}", self.head))
    }

    /// 直接杀掉（KILL）、收走
    pub fn kill(&mut self) {
        self.stopped = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.drain();
    }
}

impl Drop for ServerProc {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.halt();
        }
    }
}

/// [`Server`] 怎么起：原来 Server 的那几个带默认值的参数
#[derive(Clone, Debug)]
pub struct ServerOpts {
    /// --ttl：开着会话却这么多秒没心跳算离线（默认 "2"）
    pub ttl: String,
    /// --idle：这么多秒没动静算闲置（默认 "3"）
    pub idle: String,
    /// --behind-proxy：只说 HTTP，不用证书
    pub proxied: bool,
    /// 监听这个端口（重启后设备照旧连得上）；0 是随便挑一个
    pub port: u16,
    /// 后面多带的参数（开中转、网页端的那几个）
    pub args: Vec<String>,
    /// 给服务器的环境变量
    pub env: Vec<(String, String)>,
}

impl Default for ServerOpts {
    /// ttl 2 秒、闲置 3 秒：测试不用干等
    fn default() -> ServerOpts {
        ServerOpts { ttl: "2".to_string(), idle: "3".to_string(), proxied: false, port: 0, args: Vec::new(), env: Vec::new() }
    }
}

impl ServerOpts {
    /// 默认的那一套（ttl 2 秒、闲置 3 秒）
    pub fn new() -> ServerOpts {
        ServerOpts::default()
    }

    /// 原来 FreshServer 的那一套：ttl 60 秒、闲置 600 秒
    pub fn fresh() -> ServerOpts {
        ServerOpts::default().ttl("60").idle("600")
    }

    pub fn ttl(mut self, ttl: &str) -> ServerOpts {
        self.ttl = ttl.to_string();
        self
    }

    pub fn idle(mut self, idle: &str) -> ServerOpts {
        self.idle = idle.to_string();
        self
    }

    pub fn proxied(mut self, proxied: bool) -> ServerOpts {
        self.proxied = proxied;
        self
    }

    pub fn port(mut self, port: u16) -> ServerOpts {
        self.port = port;
        self
    }

    /// 后面多带一个参数
    pub fn arg(mut self, arg: impl Into<String>) -> ServerOpts {
        self.args.push(arg.into());
        self
    }

    /// 后面多带几个参数
    pub fn args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> ServerOpts {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// 给服务器设一个环境变量
    pub fn env(mut self, key: &str, value: &str) -> ServerOpts {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
}

/// 一个真的 Team 的同步服务器进程。d 是它的目录：证书和私钥是 d 下的 cert.pem、key.pem（[`pki::make_cert`]；proxied 的不用），
/// 状态文件是 d/state.json（旁边的 keys.json、cost-slots.json、team-docs.json 也是它写的），端口写在 d/port。
/// team_root 是 Team 的根公钥（必须给；不写 --mode：默认就是 team）。离开作用域时停掉，不留下进程
pub struct Server {
    /// 它的目录
    pub d: PathBuf,
    /// 是不是 --behind-proxy（只说 HTTP）
    pub proxied: bool,
    /// 监听的端口
    pub port: u16,
    /// 底下的进程
    pub proc: ServerProc,
}

impl Server {
    /// 原来的 `Server(d, team_root=…)`：ttl 2 秒、闲置 3 秒，端口随便挑
    pub fn start(d: &Path, team_root: &str) -> Server {
        Server::start_with(d, team_root, &ServerOpts::default())
    }

    /// 原来的 `FreshServer(d, team_root=…)`（每个用例重起一次的服务器）：ttl 60 秒、闲置 600 秒，端口随便挑
    pub fn fresh(d: &Path, team_root: &str) -> Server {
        Server::start_with(d, team_root, &ServerOpts::fresh())
    }

    /// 原来的 `FreshServer(d, port=…, team_root=…)`：监听给的端口
    pub fn fresh_on(d: &Path, team_root: &str, port: u16) -> Server {
        Server::start_with(d, team_root, &ServerOpts::fresh().port(port))
    }

    /// 照 opts 起。命令行和原来一样：`--listen 127.0.0.1:端口 --port-file d/port`，证书（或者 `--behind-proxy`），
    /// `--team-root … --state d/state.json --ttl … --idle …`，再接 opts.args。等它写下端口（最多 [`STARTUP`]）；
    /// 没起来就退出了马上 panic（哪条命令、退出码、它说的话）
    pub fn start_with(d: &Path, team_root: &str, opts: &ServerOpts) -> Server {
        assert!(!team_root.is_empty(), "服务器只有 Team 的一种：要给根公钥");
        let port_file = d.join("port");
        let _ = fs::remove_file(&port_file);
        let mut args =
            vec!["--listen".to_string(), format!("127.0.0.1:{}", opts.port), "--port-file".to_string(), text(&port_file)];
        if opts.proxied {
            args.push("--behind-proxy".to_string());
        } else {
            args.extend(["--cert".to_string(), text(&d.join("cert.pem")), "--key".to_string(), text(&d.join("key.pem"))]);
        }
        args.extend(["--team-root".to_string(), team_root.to_string(), "--state".to_string(), text(&d.join("state.json"))]);
        args.extend(["--ttl".to_string(), opts.ttl.clone(), "--idle".to_string(), opts.idle.clone()]);
        args.extend(opts.args.iter().cloned());
        let env: Vec<(&str, &str)> = opts.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut proc = ServerProc::spawn(&args, &env);
        let written = proc.wait_file(&port_file, "服务器起来", STARTUP);
        let port = written.trim().parse().unwrap_or_else(|_| panic!("端口文件里写的不是端口：{written:?}"));
        Server { d: d.to_path_buf(), proxied: opts.proxied, port, proc }
    }

    /// 设备记着的服务器地址：`https://127.0.0.1:端口`（proxied 的是 http）
    pub fn url(&self) -> String {
        format!("{}://127.0.0.1:{}", if self.proxied { "http" } else { "https" }, self.port)
    }

    /// 停掉，返回它写的日志（停过了再停是空的）
    pub fn stop(&mut self) -> String {
        self.proc.stop()
    }

    /// 发一个请求，给出 (状态码, 回的 JSON；空的当成 {})。带 `Content-Type: application/json` 和 headers 里另给的；
    /// sig 不是 None 时带 `X-Cca-Sig`（它的 base64）。不验证书（客户端靠的是公钥钉住，另外核对）。服务器没读完就回绝、
    /// 断开了（body 太大）：状态码是 [`DROPPED`]、JSON 是 {}。连不上、回的不是 JSON 就 panic
    pub fn req(&self, method: &str, path: &str, body: &[u8], sig: Option<&[u8]>, headers: &[(&str, &str)]) -> (u16, Value) {
        // 照原来的 `dict({'Content-Type': …}, **headers)`：同名的换掉值、位置不动
        let mut all: Vec<(String, String)> = vec![("Content-Type".to_string(), "application/json".to_string())];
        let mut put = |name: &str, value: &str| match all.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = value.to_string(),
            None => all.push((name.to_string(), value.to_string())),
        };
        for (name, value) in headers {
            put(name, value);
        }
        if let Some(sig) = sig {
            put("X-Cca-Sig", &crypto::b64(sig));
        }
        let mut request = http::Request::new(method, &format!("{}{path}", self.url())).body(body).timeout(Duration::from_secs(10));
        for (name, value) in &all {
            request = request.header(name, value);
        }
        if !self.proxied {
            request = request.tls(tls::Client::insecure());
        }
        let reply = match request.send() {
            Ok(reply) => reply,
            Err(e) if dropped(&e) => return (DROPPED, obj! {}),
            Err(e) => panic!("{e}"),
        };
        let data: &[u8] = if reply.body.is_empty() { b"{}" } else { &reply.body };
        let j = json::parse_bytes(data).unwrap_or_else(|e| {
            panic!("{method} {path}：服务器回的（{}）不是 JSON：{e}：{}", reply.status, String::from_utf8_lossy(data))
        });
        (reply.status, j)
    }

    /// `req("GET", path)`：不带 body、不带签名
    pub fn get(&self, path: &str) -> (u16, Value) {
        self.req("GET", path, b"", None, &[])
    }
}

/// 服务器没读完请求就断开了：写不进去（BrokenPipe）、连接被重置、一个字都没回就关了（Python 的 RemoteDisconnected 也算
/// ConnectionResetError）
fn dropped(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionReset, UnexpectedEof};
    matches!(e.kind(), BrokenPipe | ConnectionReset | UnexpectedEof)
}

// ---- 照文档自己签的请求 ----

/// 签过名的请求的 body：v、op、key（签名的那把密钥的指纹）、ts（现在）、nonce（随机），后面接 extra（put / get 这些；
/// extra 里有同名的就换掉前面的值、位置不动：测试拿它发时间不对的请求）。紧凑的 JSON，非 ASCII 转义。extra 是 null 就是没有
pub fn call_body(op: &str, key_fp: &str, extra: &Value) -> Vec<u8> {
    let mut body = Obj::new();
    body.insert("v", 1);
    body.insert("op", op);
    body.insert("key", key_fp);
    body.insert("ts", now());
    body.insert("nonce", crypto::token_hex(16));
    if !extra.is_null() {
        for (k, v) in extra.obj().iter() {
            body.insert(k, v.clone());
        }
    }
    json::dumps(&Value::Obj(body), &Style { compact: true, ..Style::default() }).into_bytes()
}

/// 签过之后又改过的 body：`"v":1` 后面多一个空格（内容没变，字节变了：签名对不上）
pub fn tampered(raw: &[u8]) -> Vec<u8> {
    let (from, to): (&[u8], &[u8]) = (b"\"v\":1", b"\"v\":1 ");
    let mut out = Vec::with_capacity(raw.len() + 1);
    let mut i = 0;
    while i < raw.len() {
        if raw[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    out
}

/// 照文档签一个请求发给服务器（[`Server::req`]：不重试、限流也原样回）：body 是 [`call_body`]（key 写 key_fp），拿 keyfile 这把私钥
/// 在命名空间 ns 里签名（平常是 cca-presence）；tamper：签完再改 body（[`tampered`]）。key_fp 和 keyfile 可以不是同一把：
/// 测试拿它冒充别的设备
pub fn signed_call(srv: &Server, op: &str, key_fp: &str, keyfile: &Path, ns: &str, tamper: bool, extra: &Value) -> (u16, Value) {
    let mut raw = call_body(op, key_fp, extra);
    let sig = pki::sign(keyfile, ns, &raw);
    if tamper {
        raw = tampered(&raw);
    }
    srv.req("POST", &format!("/v1/{op}"), &raw, Some(&sig), &[])
}

/// 403 unknown_device 带着的 gen（服务器上这一份记录的编号）；不是这样的回复是 None
pub fn unknown(reply: &(u16, Value)) -> Option<String> {
    let (st, j) = reply;
    let o = j.as_obj()?;
    let gen = o.get("gen").and_then(Value::as_str)?;
    let ok = *st == 403
        && o.len() == 2
        && o.get("error").and_then(Value::as_str) == Some("unknown_device")
        && gen.len() == 32
        && gen.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    ok.then(|| gen.to_string())
}

/// Team 的服务器的测试用（原来的 SignedCalls）：几把有名字的密钥——pubs 是 {名字: 公钥}，私钥是 d 下同名的文件
pub struct SignedCalls {
    /// 私钥在哪个目录
    pub d: PathBuf,
    /// {名字: 公钥（`ssh-ed25519 AAAA…`）}
    pub pubs: BTreeMap<String, String>,
}

impl SignedCalls {
    /// 在 d 下给这几个名字各生成一把密钥（[`pki::keygen`]）
    pub fn new(d: &Path, names: &[&str]) -> SignedCalls {
        let pubs = names.iter().map(|n| (n.to_string(), pki::keygen(d.join(n)))).collect();
        SignedCalls { d: d.to_path_buf(), pubs }
    }

    /// who 的公钥
    pub fn pubkey(&self, who: &str) -> &str {
        self.pubs.get(who).unwrap_or_else(|| panic!("没有 {who} 这把密钥"))
    }

    /// who 的公钥的指纹（`SHA256:…`）
    pub fn fp(&self, who: &str) -> String {
        spec::fp_of_line(self.pubkey(who))
    }

    /// who 签一个请求发给 srv（原来的 `call(op, who, **extra)`）
    pub fn call(&self, srv: &Server, op: &str, who: &str, extra: &Value) -> (u16, Value) {
        self.call_as(srv, op, who, None, false, extra)
    }

    /// 同 [`SignedCalls::call`]；signer：拿另一把密钥签（冒用 who 的指纹），tamper：签完再改 body
    pub fn call_as(&self, srv: &Server, op: &str, who: &str, signer: Option<&str>, tamper: bool, extra: &Value) -> (u16, Value) {
        signed_call(srv, op, &self.fp(who), &self.d.join(signer.unwrap_or(who)), "cca-presence", tamper, extra)
    }
}

// ---- Team 和设备 ----

/// 测试里的一个 Team：根密钥是 d 下的 key，测试拿着它扮演管理员——在服务器上登记、移除设备（照 server/README.md「Team 的同步
/// 服务器」自己签名）。设备那边不经邀请：[`Device::join`] 直接写好“加入了、同步到了配置”的样子（~/.cca/teams/<编号>/）；
/// 邀请、加入、同步配置这一套在 team 那一份测试里
pub struct Team {
    /// 根密钥在哪个目录
    pub d: PathBuf,
    /// Team 的名字
    pub name: String,
    /// 配置里的盐（32 个十六进制字符）
    pub salt: String,
    /// 根密钥（私钥文件）
    pub keyfile: PathBuf,
    /// 根公钥（`ssh-ed25519 AAAA…`）
    pub root: String,
    /// Team 的编号
    pub id: String,
    /// 设备上这个 Team 的服务器的状态文件的前缀（presence/、cost/ 里）：`team.<编号>.`
    pub lane: String,
}

impl Team {
    /// 原来的 `Team(d)`：名字「测试组」，根密钥是 d/root（现生成），盐是 32 个 0
    pub fn new(d: &Path) -> Team {
        Team::named(d, "测试组", "root", &"00".repeat(16))
    }

    /// 原来的 `Team(d, name, key, salt)`：根密钥是 d 下的 key（现生成，备注 cca-team-root）
    pub fn named(d: &Path, name: &str, key: &str, salt: &str) -> Team {
        let keyfile = d.join(key);
        let root = pki::keygen_as(&keyfile, "cca-team-root");
        let id = spec::team_id(&root);
        let lane = format!("team.{id}.");
        Team { d: d.to_path_buf(), name: name.to_string(), salt: salt.to_string(), keyfile, root, id, lane }
    }

    /// 管理员给设备的配置（明文；样子见 docs/dev/team.md「设备拿到的配置」）：v、at（现在）、name、salt、server（u 是 url、p 是 pins）、
    /// accounts（默认只给 alice 的 Claude 账号：alice@example.com → US-A）、aliases、install_node、nodes。over 是一个对象：
    /// 里面的键换掉同名的（位置不动）、没有的接在后面（原来的 `accounts=` 和 `**more`）；null 就是不改
    pub fn doc(&self, url: &str, pins: &[&str], over: &Value) -> Value {
        let mut doc = Obj::new();
        doc.insert("v", 1);
        doc.insert("at", now());
        doc.insert("name", self.name.as_str());
        doc.insert("salt", self.salt.as_str());
        doc.insert("server", obj! { "u" => url, "p" => pins.to_vec() });
        doc.insert("accounts", obj! { "claude" => obj! { "alice@example.com" => "US-A" } });
        doc.insert("aliases", obj! {});
        doc.insert("install_node", "");
        doc.insert("nodes", Value::Null);
        if !over.is_null() {
            for (k, v) in over.obj().iter() {
                doc.insert(k, v.clone());
            }
        }
        Value::Obj(doc)
    }

    /// 管理员的请求：拿根密钥签名（[`spec::post`]：限流时自己等一会儿再试）。extra 是 null 就是没有
    pub fn admin(&self, port: u16, op: &str, extra: &Value) -> (u16, Value) {
        let none = obj! {};
        spec::post(&self.keyfile, port, op, if extra.is_null() { &none } else { extra })
    }

    /// 服务器上的名单正好是这几台设备：没登记的登记上（名字是设备的名字），名单里别的移除；各台设备记下服务器上这一份记录的编号（gen）
    pub fn only(&self, port: u16, devs: &[&Device]) {
        let (st, j) = self.admin(port, "team_members", &Value::Null);
        assert_eq!(st, 200, "{j}");
        let members: Vec<Membership> = devs.iter().map(|d| d.team(self)).collect();
        for m in j["members"].list() {
            let dev = m["dev"].str();
            if !members.iter().any(|k| k.dev == dev) {
                let (st, r) = self.admin(port, "team_revoke", &obj! { "dev" => dev });
                assert_eq!(st, 200, "{r}");
            }
        }
        if devs.is_empty() {
            return;
        }
        let mut devices = Obj::new();
        for (d, m) in devs.iter().zip(&members) {
            devices.insert(m.dev.as_str(), obj! { "pub" => m.pubkey.as_str(), "name" => d.name.as_str() });
        }
        let (st, j) = self.admin(port, "team_enroll", &obj! { "devices" => devices });
        let mut got: Vec<&str> =
            j.get("names").and_then(Value::as_obj).map(|n| n.values().map(Value::str).collect()).unwrap_or_default();
        let mut want: Vec<&str> = devs.iter().map(|d| d.name.as_str()).collect();
        got.sort_unstable();
        want.sort_unstable();
        assert!(st == 200 && got == want, "{st} {j}");
        let gen = j["gen"].clone();
        for d in devs {
            d.team_json(self, |t| {
                t.insert("gen", gen.clone());
            });
        }
    }
}

/// 一台设备在某个 Team 里的那几样（原来 `Device.team(team)` 给的）
#[derive(Clone, Debug, PartialEq)]
pub struct Membership {
    /// 它在 Team 里的编号（16 个十六进制字符）
    pub dev: String,
    /// 公钥（`ssh-ed25519 AAAA…`）
    pub pubkey: String,
    /// 公钥的指纹（`SHA256:…`）
    pub fp: String,
    /// 私钥文件
    pub keyfile: PathBuf,
}

/// 一台设备：自己的 HOME（~/.cca）；cca 的辅助程序在它里面跑。命令都在它自己的 bin/ 里（curl、ps、cca、security 是假的，
/// 见上面那几段脚本）。[`Device::join`] 以后它在那个 Team 里：keyfile、pubkey、fp 是它在头一个加入的 Team 里的密钥，
/// lane 是那个 Team 的服务器的状态文件的前缀
pub struct Device {
    /// 设备的名字
    pub name: String,
    /// 它的 HOME
    pub home: PathBuf,
    /// ~/.cca
    pub cca: PathBuf,
    /// 在它里面跑辅助程序时的环境（HOME、CCA_HOME、CCA_TOOL=claude、PATH=~/bin:/usr/bin:/bin、CCA_SELF=~/bin/cca、
    /// CCA_KEYSTORE=file、CCA_COST=off；这个进程设了 TZ 的话带着）。用例可以直接增删
    pub env: BTreeMap<String, String>,
    first: Option<Membership>,
    lane: Option<String>,
}

impl Device {
    /// 账号表里 alice 的键
    pub const KEY: &'static str = "alice(alice@example.com)";

    /// alice 的账号目录的名字（原来的 `Device.ACCT`）
    pub fn acct_ident() -> String {
        ident("alice@example.com")
    }

    /// 建一台设备：home（还没有，这里建）下建 .cca 和 bin，放好假命令，config.json 是 {}，accounts.json 是 table（账号表）
    pub fn new(home: &Path, name: &str, table: &Value) -> Device {
        let cca = home.join(".cca");
        fs::create_dir_all(home).unwrap_or_else(|e| panic!("建 {}：{e}", home.display()));
        fs::create_dir(&cca).unwrap_or_else(|e| panic!("建 {}：{e}", cca.display()));
        fs::create_dir(home.join("bin")).unwrap_or_else(|e| panic!("建 {}/bin：{e}", home.display()));
        let bin = home.join("bin");
        let mut env = BTreeMap::new();
        for (k, v) in [
            ("HOME", text(home)),
            ("CCA_HOME", text(&cca)),
            ("CCA_TOOL", "claude".to_string()),
            ("PATH", format!("{}:/usr/bin:/bin", text(&bin))),
            ("CCA_SELF", text(&bin.join("cca"))),
            ("CCA_KEYSTORE", "file".to_string()),
            ("CCA_COST", "off".to_string()),
        ] {
            env.insert(k.to_string(), v);
        }
        if let Ok(tz) = std::env::var("TZ") {
            env.insert("TZ".to_string(), tz);
        }
        let dev = Device { name: name.to_string(), home: home.to_path_buf(), cca, env, first: None, lane: None };
        for (cmd, script) in [("curl", CURL), ("ps", PS), ("cca", SELF), ("security", NO_KEYCHAIN)] {
            dev.fake(cmd, script);
        }
        fsx::write(dev.cca.join("config.json"), "{}");
        fsx::write(dev.cca.join("accounts.json"), dumps(table));
        dev
    }

    // --- Team ---

    /// 这台设备加入了 team、同步到了配置（`team.doc(url, pins)`）：直接写好 ~/.cca/teams/<编号>/ 下的 team.json、doc.json
    /// 和这台设备在 Team 里的密钥（不经邀请；服务器上的名单由 [`Team::only`] 登记）。它在 Team 里的编号是随机的
    pub fn join(&mut self, team: &Team, url: &str, pins: &[&str]) -> &mut Device {
        self.join_with(team, url, pins, None, None)
    }

    /// 同 [`Device::join`]；doc：同步到的配置（默认 `team.doc(url, pins)`），dev：它在 Team 里的编号（默认随机）
    pub fn join_with(&mut self, team: &Team, url: &str, pins: &[&str], doc: Option<&Value>, dev: Option<&str>) -> &mut Device {
        use std::os::unix::fs::DirBuilderExt;
        let td = self.cca.join("teams").join(&team.id);
        fs::create_dir_all(self.cca.join("teams")).unwrap_or_else(|e| panic!("建 {}/teams：{e}", self.cca.display()));
        fs::DirBuilder::new().mode(0o700).create(&td).unwrap_or_else(|e| panic!("建 {}：{e}", td.display()));
        let keyfile = td.join("id_ed25519");
        let pubkey = pki::keygen_as(&keyfile, &format!("cca-team@{}", self.name));
        let dev = dev.map_or_else(|| crypto::token_hex(8), str::to_string);
        let meta = obj! {
            "v" => 1, "id" => team.id.as_str(), "name" => team.name.as_str(), "url" => url, "pins" => pins.to_vec(),
            "root" => team.root.as_str(), "role" => "member", "dev" => dev.as_str(), "device" => self.name.as_str(), "joined_at" => now(),
        };
        fsx::write(td.join("team.json"), dumps(&meta));
        let default_doc;
        let doc = match doc {
            Some(doc) => doc,
            None => {
                default_doc = team.doc(url, pins, &Value::Null);
                &default_doc
            }
        };
        self.set_doc(team, doc);
        let index = self.cca.join("teams").join("index.json");
        let mut order = if index.exists() { fsx::read_json(&index)["order"].list().to_vec() } else { Vec::new() };
        order.push(Value::from(team.id.as_str()));
        fsx::write(&index, dumps(&obj! { "v" => 1, "order" => order }));
        if self.first.is_none() {
            let fp = spec::fp_of(td.join("id_ed25519.pub"));
            self.first = Some(Membership { dev, pubkey, fp, keyfile });
            self.lane = Some(team.lane.clone());
        }
        self
    }

    /// 加入过 Team 了没有（还没有的话 keyfile、pubkey、fp、lane 都没有：问了就 panic）
    pub fn joined(&self) -> bool {
        self.first.is_some()
    }

    fn first(&self) -> &Membership {
        self.first.as_ref().unwrap_or_else(|| panic!("{} 还没加入 Team（先 join）", self.name))
    }

    /// 它在头一个加入的 Team 里的私钥文件
    pub fn keyfile(&self) -> &Path {
        &self.first().keyfile
    }

    /// 它在头一个加入的 Team 里的公钥（`ssh-ed25519 AAAA…`；原来的 pub 属性）
    pub fn pubkey(&self) -> &str {
        &self.first().pubkey
    }

    /// 那把公钥的指纹（`SHA256:…`）
    pub fn fp(&self) -> &str {
        &self.first().fp
    }

    /// 头一个加入的 Team 的服务器的状态文件的前缀（`team.<编号>.`）
    pub fn lane(&self) -> &str {
        self.lane.as_deref().unwrap_or_else(|| panic!("{} 还没加入 Team（先 join）", self.name))
    }

    /// 这台设备在 team 里的：编号、公钥、指纹、私钥（照 ~/.cca/teams/<编号>/ 下现读）
    pub fn team(&self, team: &Team) -> Membership {
        let td = self.cca.join("teams").join(&team.id);
        Membership {
            dev: fsx::read_json(td.join("team.json"))["dev"].str().to_string(),
            pubkey: spec::pub_line(&fsx::read_text(td.join("id_ed25519.pub"))),
            fp: spec::fp_of(td.join("id_ed25519.pub")),
            keyfile: td.join("id_ed25519"),
        }
    }

    /// 改一改这台设备记着的 team.json（服务器在哪、钉住的指纹……）
    pub fn team_json(&self, team: &Team, edit: impl FnOnce(&mut Obj)) {
        let p = self.cca.join("teams").join(&team.id).join("team.json");
        let mut t = fsx::read_json(&p);
        edit(t.as_obj_mut().unwrap_or_else(|| panic!("{} 不是对象", p.display())));
        fsx::write(&p, dumps(&t));
    }

    /// 这台设备手里的配置换成 doc（像是同步到了新的一版：序号加一；头一次是 1）
    pub fn set_doc(&self, team: &Team, doc: &Value) {
        let p = self.cca.join("teams").join(&team.id).join("doc.json");
        let seq = if p.exists() { fsx::read_json(&p)["seq"].i64() + 1 } else { 1 };
        fsx::write(&p, dumps(&obj! { "v" => 1, "seq" => seq, "at" => now(), "doc" => doc }));
    }

    /// 在 ~/bin 里放一个假命令：`#!/bin/sh` 接上 script，755
    pub fn fake(&self, name: &str, script: &str) {
        fsx::write_script(self.home.join("bin").join(name), &format!("#!/bin/sh\n{script}"));
    }

    /// 在这台设备上跑辅助程序的一条命令（还没起：环境是这一刻的 [`Device::env`]，不设时限）；要另设环境变量、喂输入、
    /// 起在伪终端上的接着用它
    pub fn cmd<S: AsRef<str>>(&self, argv: &[S]) -> Cmd {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(argv.iter().map(|a| a.as_ref())).envs(&self.env);
        cmd
    }

    /// 跑辅助程序的一条命令，给出退出码、标准输出、标准错误（原来的 `run(*argv)`；最多等 120 秒）
    pub fn run<S: AsRef<str>>(&self, argv: &[S]) -> Ran {
        self.run_with(argv, "", &[])
    }

    /// 同 [`Device::run`]；stdin 是喂给它的输入，env 是这一次另设的环境变量（原来的 `run(*argv, stdin=…, **env)`）
    pub fn run_with<S: AsRef<str>>(&self, argv: &[S], stdin: &str, env: &[(&str, &str)]) -> Ran {
        let mut cmd = self.cmd(argv);
        cmd.envs(env.iter().copied()).input(stdin).timeout(Duration::from_secs(120));
        sandbox::ran(&mut cmd)
    }

    /// 跑一条该成的命令，返回 stdout；退出码不是 0、在 stderr 上说了话就 panic
    pub fn ok<S: AsRef<str>>(&self, argv: &[S]) -> String {
        self.ok_with(argv, "", &[])
    }

    /// 同 [`Device::ok`]，带输入和另设的环境变量
    pub fn ok_with<S: AsRef<str>>(&self, argv: &[S], stdin: &str, env: &[(&str, &str)]) -> String {
        let r = self.run_with(argv, stdin, env);
        if (r.code, r.err.as_str()) != (0, "") {
            let shown: Vec<&str> = argv.iter().take(2).map(|a| a.as_ref()).collect();
            panic!("{} 上 {} 没成：退出码 {}，{}{}", self.name, shown.join(" "), r.code, r.out, r.err);
        }
        r.out
    }

    /// config.json
    pub fn config(&self) -> Value {
        fsx::read_json(self.cca.join("config.json"))
    }

    /// 改一改 config.json 再写回去，给出改完的
    pub fn config_edit(&self, edit: impl FnOnce(&mut Obj)) -> Value {
        let mut c = self.config();
        edit(c.as_obj_mut().expect("config.json 是对象"));
        fsx::write(self.cca.join("config.json"), dumps(&c));
        c
    }

    /// 账号目录（key 是账号表的键，平常是 [`Device::KEY`]）
    pub fn acct(&self, key: &str) -> PathBuf {
        self.cca.join("accounts").join(ident(sandbox::email_of(key)))
    }

    /// alice 在这台设备上登录着（uuid 是它的登录身份）；None：没登录。给出写下的 .claude.json
    pub fn login(&self, uuid: Option<&str>) -> PathBuf {
        self.login_as(uuid, Device::KEY)
    }

    /// 同 [`Device::login`]，另一个账号（key 是账号表的键）
    pub fn login_as(&self, uuid: Option<&str>, key: &str) -> PathBuf {
        let cd = self.acct(key).join("claude");
        fs::create_dir_all(&cd).unwrap_or_else(|e| panic!("建 {}：{e}", cd.display()));
        let doc = match uuid.filter(|u| !u.is_empty()) {
            Some(uuid) => obj! { "oauthAccount" => obj! { "accountUuid" => uuid, "emailAddress" => sandbox::email_of(key) } },
            None => obj! {},
        };
        fsx::write(cd.join(".claude.json"), dumps(&doc));
        cd.join(".claude.json")
    }

    // --- 记下备查的 API key ---

    /// 在这台设备上给 alice 记下一把（cca key 账号；key 从 stdin 给）：返回 stdout
    pub fn keyset(&self, s: &str) -> String {
        self.keyset_as(s, Device::KEY)
    }

    /// 同 [`Device::keyset`]，另一个账号
    pub fn keyset_as(&self, s: &str, key: &str) -> String {
        self.ok_with(&["secret-set", key], s, &[])
    }

    /// 后台起过几次同步（每次的参数；先后不保证）。cca 在后台起东西经 taskpolicy -c background，机器的核都占满时这样的进程
    /// 一直轮不到跑（实测：占满多久它就等多久），所以起了、还没轮到往 kicks.log 记一笔的也算：进程表里命令行是这台设备的假 cca 的那些
    pub fn kicks(&self) -> Vec<String> {
        let waiting = waiting_kicks(&self.env["CCA_SELF"]).unwrap_or_else(|e| panic!("/bin/ps 运行不了：{e}"));
        // 先看进程表、再读文件：这中间记完退出了的在文件里；两边都有的（记了、还没退出）按进程号只算一次
        let log = fsx::try_read_text(self.home.join("kicks.log")).unwrap_or_default();
        let done: Vec<(&str, &str)> = sandbox::splitlines(&log).into_iter().map(|l| l.split_once(' ').unwrap_or((l, ""))).collect();
        let mut out: Vec<String> = done.iter().map(|(_, args)| args.to_string()).collect();
        out.extend(waiting.iter().filter(|(pid, _)| !done.iter().any(|(p, _)| p == pid)).map(|(_, args)| args.clone()));
        out
    }

    /// 这台设备在后台起的、还没跑完的假 cca（见 [`Device::kicks`]：机器忙的时候它们一直轮不到跑）：杀掉，给出杀了几个。
    /// 设备离开作用域时自己做一次，[`agent`] 起的上报进程停掉以后也做一次：用例完了不留下进程（运行器的 LEFTOVER）
    pub fn sweep(&self) -> usize {
        sweep_home(&self.home)
    }

    /// 头一个加入的 Team 的服务器的那一份 keys.json（presence/team.<编号>.keys.json）；还没有是 None
    pub fn keys_json(&self) -> Option<Value> {
        let p = self.cca.join("presence").join(format!("{}keys.json", self.lane()));
        p.exists().then(|| fsx::read_json(&p))
    }

    /// 同步一次记下备查的 API key（cca _key-sync force）：没同步成的原因，成了是 ""；没开设备统计、有一轮正在跑是 None
    pub fn sync(&self) -> Option<String> {
        self.sync_mode("force")
    }

    /// 同 [`Device::sync`]；mode 是 "" 时不 force（还不到时候、有一轮正在跑就不同步：None）
    pub fn sync_mode(&self, mode: &str) -> Option<String> {
        let synced = |j: &Option<Value>| j.as_ref().and_then(|j| j.get("synced")).cloned();
        let before = synced(&self.keys_json());
        if mode.is_empty() {
            self.ok(&["key-sync"]);
        } else {
            self.ok(&["key-sync", mode]);
        }
        let j = self.keys_json()?;
        if j.get("synced").cloned() == before {
            return None;
        }
        Some(j.get("error").map_or_else(String::new, |e| e.str().to_string()))
    }

    /// alice 在这台设备上记下备查的那一把：(0, key)；没有是 (44, "")——照密钥文件（~/.cca/secrets/<服务名>，CCA_KEYSTORE=file）
    /// 或者假钥匙串里的
    pub fn stored(&self) -> (i32, String) {
        self.stored_as(Device::KEY)
    }

    /// 同 [`Device::stored`]，另一个账号
    pub fn stored_as(&self, key: &str) -> (i32, String) {
        let svc = format!("cca-ref-{}", ident(sandbox::email_of(key)));
        let file = self.cca.join("secrets").join(&svc);
        if file.exists() {
            return (0, fsx::read_json(&file)["key"].str().to_string());
        }
        let keychain = self.home.join("keychain").join(&svc).join("reference");
        if keychain.exists() {
            return (0, fsx::read_text(&keychain));
        }
        (44, String::new())
    }

    /// config.json 里记着的 alice 的这一把（App 的账号页照它说，不读钥匙串）：
    /// {saved 存了没有, hint 掩码, from_device 从哪台设备同步来的, saved_epoch 什么时候记下的}
    pub fn hint(&self) -> Value {
        let c = self.config();
        let acct = Device::acct_ident();
        let h = c.get("key_hints").and_then(|k| k.get(&acct)).cloned().unwrap_or_else(|| obj! {});
        let saved = c.get("ref_keys").and_then(Value::as_list).is_some_and(|l| l.iter().any(|x| x.as_str() == Some(&acct)));
        obj! {
            "saved" => saved,
            "hint" => h.get("hint").cloned().unwrap_or_else(|| Value::from("")),
            "from_device" => h.get("from").cloned(),
            "saved_epoch" => h.get("at").cloned(),
        }
    }

    // --- 发给服务器的 ---

    /// 这台设备发过、服务器回过了（或者没发出去）的请求，按先后：请求里的各项，加上 op 哪个接口、code 服务器回的状态码
    /// （没发出去是 0）、reply 它回的内容（没有是 {}）、done 什么时候回来的。op 不是 None 时只要这个接口的
    pub fn sent(&self, op: Option<&str>) -> Vec<Value> {
        let dir = self.home.join("sent");
        let mut codes: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "code") {
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                    let n = stem.parse().unwrap_or_else(|_| panic!("{} 的名字不是第几个", path.display()));
                    codes.push((n, path));
                }
            }
        }
        codes.sort();
        let mut out = Vec::new();
        for (_, code) in codes {
            let this_op = fsx::read_text(code.with_extension("op"));
            if op.is_some_and(|want| want != this_op) {
                continue;
            }
            let mut r = fsx::read_json(code.with_extension("json"));
            let reply = fsx::read_text(code.with_extension("reply"));
            let reply = reply.trim();
            let status = fsx::read_text(&code);
            let status: i64 = if status.trim().is_empty() {
                0
            } else {
                status.trim().parse().unwrap_or_else(|_| panic!("{} 里不是状态码：{status:?}", code.display()))
            };
            let done = fs::metadata(&code).and_then(|m| m.modified()).unwrap_or_else(|e| panic!("{}：{e}", code.display()));
            let o = r.as_obj_mut().unwrap_or_else(|| panic!("{} 不是对象", code.with_extension("json").display()));
            o.insert("op", this_op);
            o.insert("code", status);
            o.insert(
                "reply",
                if reply.is_empty() {
                    obj! {}
                } else {
                    json::parse(reply).unwrap_or_else(|e| panic!("回的不是 JSON：{e}：{reply}"))
                },
            );
            o.insert("done", done.duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()));
            out.push(r);
        }
        out
    }

    /// 照文档自己签一个请求发给服务器（不经 cca：扮演这台设备上别的程序、有问题的程序）：拿它在头一个加入的 Team 里的密钥
    pub fn post(&self, port: u16, op: &str, extra: &Value) -> (u16, Value) {
        let none = obj! {};
        spec::post(self.keyfile(), port, op, if extra.is_null() { &none } else { extra })
    }

    // --- 折合金额 ---

    /// 这台设备以前统计好的（cost/local.json）：身份 who（alice@example.com 的）的小时数据 rows。没有会话记录、没有 scan.json 时
    /// 统计照它接上
    pub fn cost_local(&self, who: &str, rows: &Value) {
        self.cost_local_as(who, rows, "alice@example.com");
    }

    /// 同 [`Device::cost_local`]，身份是另一个邮箱的
    pub fn cost_local_as(&self, who: &str, rows: &Value, email: &str) {
        let id = format!("claude:{who}");
        let doc = obj! {
            "v" => 1, "build" => "x", "who" => obj! { id.as_str() => email }, "acc" => obj! { id.as_str() => rows },
            "um" => obj! {}, "orphan" => obj! {},
        };
        fsx::write(self.cca.join("cost").join("local.json"), dumps(&doc));
    }

    /// cca cost sync [--now] --json：统计一次、和别的设备同步；返回同步的结果 {state, message, checked_epoch}。
    /// force 是 false 时不带 --now
    pub fn cost_sync(&self, force: bool) -> Value {
        let out = if force { self.ok(&["cost-sync", "--json", "--now"]) } else { self.ok(&["cost-sync", "--json"]) };
        json::parse(&out).unwrap_or_else(|e| panic!("cost-sync --json 给的不是 JSON：{e}：{out}"))["sync"].clone()
    }

    /// cost/ 下的文件；Team 的服务器的那几份带着前缀（`format!("{}remote.json", dev.lane())` 这样写）
    pub fn cost(&self, name: &str) -> Value {
        fsx::read_json(self.cca.join("cost").join(name))
    }

    /// App 看到的（_gui state 的 cost）：经辅助程序只给测试用的 _cost-view 看。team 是同步走的那个 Team 的编号；accounts 的行是
    /// [工具, 键, usage_weeks, 谁给的（Team 的编号，这台设备自己的是 false）]，sync 的行是 [slot, 谁给的]（两个都是数组的数组）
    pub fn cost_view(&self, team: &Value, accounts: &Value, sync: &Value) -> Value {
        let spec = obj! { "config" => obj! {}, "team" => team, "accounts" => accounts, "sync" => sync, "tools" => vec!["claude"] };
        let out = self.ok(&["_cost-view", &json::float_repr(now()), &dumps(&spec)]);
        json::parse(&out).unwrap_or_else(|e| panic!("_cost-view 给的不是 JSON：{e}：{out}"))
    }
}

impl Drop for Device {
    /// 后台起的、还没轮到跑的假 cca 不留到用例之后
    fn drop(&mut self) {
        self.sweep();
    }
}

/// 进程表里命令行是这个假 cca（me 是它的路径）的那些：{进程号: 它的参数}。经 taskpolicy 起、还没换成脚本的也算
fn waiting_kicks(me: &str) -> std::io::Result<BTreeMap<String, String>> {
    let me = format!("{me} ");
    let mut ps = Cmd::new("/bin/ps");
    ps.args(["-axww", "-o", "pid=,command="]).stderr(Stdio::Null);
    let table = ps.output()?.stdout;
    let mut waiting = BTreeMap::new();
    for line in sandbox::splitlines(&String::from_utf8_lossy(&table)) {
        let (pid, cmd) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        if let Some((_, args)) = cmd.split_once(&me) {
            waiting.insert(pid.to_string(), args.trim().to_string());
        }
    }
    Ok(waiting)
}

/// HOME 是 home 的那台设备在后台起的、还没跑完的假 cca：杀掉（KILL），给出杀了几个。看不了进程表就算了
fn sweep_home(home: &Path) -> usize {
    let Ok(waiting) = waiting_kicks(&text(&home.join("bin").join("cca"))) else { return 0 };
    let mut killed = 0;
    for pid in waiting.keys().filter_map(|pid| pid.parse::<libc::pid_t>().ok()).filter(|pid| *pid > 1) {
        // SAFETY: 刚在进程表里看到这个进程号的命令行是这台设备的假 cca（临时目录里的路径）；kill 只是发信号
        if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
            killed += 1;
        }
    }
    killed
}

// ---- 开着的命令行会话、上报进程（原来的 Sessions） ----

/// 一个开着的命令行会话是什么样（[`session`] 的参数；默认值照原来的）
#[derive(Clone, Debug)]
pub struct CliSession<'a> {
    /// 哪个账号（账号表的键），默认 [`Device::KEY`]
    pub key: &'a str,
    /// 进程号，默认 [`SESSION_PID`]
    pub pid: i64,
    /// 多少秒以前开的，默认 100
    pub opened: f64,
    /// 它的终端（ps 的写法：ttys005、pts/5），默认 ?? ：没有
    pub tty: &'a str,
    /// 登记里写的 cca 的启动时间，默认 [`t0_epoch`]（和假 ps 说的对得上）
    pub started: i64,
    /// 假 ps 的进程表里有没有这个进程，默认有
    pub running: bool,
}

impl Default for CliSession<'_> {
    fn default() -> Self {
        CliSession { key: Device::KEY, pid: SESSION_PID, opened: 100.0, tty: "??", started: t0_epoch(), running: true }
    }
}

/// 这台设备上开着一个命令行会话：账号目录里的登记（cca 启动账号时写的 cli-<进程号>.json，started 是 cca 的启动时间），
/// 假 ps 的进程表里有这个进程（running；tty 是它的终端）。返回它是什么时候开的（1970 年以来的秒）
pub fn session(dev: &Device, s: &CliSession) -> f64 {
    let d = dev.acct(s.key);
    let since = now() - s.opened;
    fs::create_dir_all(&d).unwrap_or_else(|e| panic!("建 {}：{e}", d.display()));
    let id = d.file_name().and_then(|n| n.to_str()).expect("账号目录的名字");
    let entry = obj! { "v" => 1, "tool" => "claude", "id" => id, "pid" => s.pid, "started" => s.started, "since" => since };
    fsx::write(d.join(format!("cli-{}.json", s.pid)), dumps(&entry));
    if s.running {
        use std::io::Write;
        let line = format!("{:5} {:5} {:<8} {:>9} {} {}\n", s.pid, 1, s.tty, "0:00.50", T0, "claude");
        let mut f = fs::OpenOptions::new().append(true).create(true).open(dev.home.join("ps.out")).expect("打开 ps.out");
        f.write_all(line.as_bytes()).expect("写 ps.out");
    }
    since
}

/// 一台设备的上报进程（[`agent`] 起的）。抄一份还是同一个进程
#[derive(Clone)]
pub struct Agent(Arc<Mutex<Child>>);

impl Agent {
    /// 进程号
    pub fn pid(&self) -> i32 {
        locked(&self.0).pid()
    }

    /// 还在运行是 None；退出了是退出码（Python 的 returncode）
    pub fn poll(&self) -> Option<i32> {
        locked(&self.0).try_wait().ok().flatten().map(|exit| exit.py())
    }

    /// 等它退出，最多等 limit，给出退出码（原来的 `p.wait(秒)`）；到时限还没退出就 panic
    pub fn wait(&self, limit: Duration) -> i32 {
        let exit = locked(&self.0).wait_timeout(limit).unwrap_or_else(|e| panic!("等上报进程退出：{e}"));
        exit.unwrap_or_else(|| panic!("上报进程 {} 秒还没退出", limit.as_secs())).py()
    }

    /// 给它发一个信号（`libc::SIGTERM` 这样的号）
    pub fn signal(&self, sig: i32) {
        let _ = locked(&self.0).signal(sig);
    }

    /// 停掉：还在运行就发 TERM，等 15 秒，还不退出就 KILL（用例结束时一定调一次）
    pub fn stop(&self) {
        let mut child = locked(&self.0);
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.signal(libc::SIGTERM);
        if !matches!(child.wait_timeout(Duration::from_secs(15)), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 起这台设备的上报进程（cca _presence-agent）：环境是这一刻的 `dev.env`，stderr 写到 ~/agent.err。用例结束时一定停掉
/// （登记了收尾：只停这里起的这一个进程，再收掉这台设备在后台起的、还没跑完的假 cca）
pub fn agent(t: &mut Ctx, dev: &Device) -> Agent {
    let err = fs::File::create(dev.home.join("agent.err")).expect("建 agent.err");
    let mut cmd = Cmd::new(harness::helper());
    cmd.arg("presence-agent").envs(&dev.env).stdout(Stdio::Null).stderr(err);
    let child = cmd.spawn().unwrap_or_else(|e| panic!("上报进程起不来：{e}"));
    let agent = Agent(Arc::new(Mutex::new(child)));
    let (handle, home) = (agent.clone(), dev.home.clone());
    t.defer(move || {
        handle.stop();
        // 它在后台起的同步（假 cca）还没轮到跑的：一起收掉
        sweep_home(&home);
    });
    agent
}

/// 上报进程看了几次进程表（每转一圈看一次）
pub fn rounds(dev: &Device) -> usize {
    fsx::try_read_text(dev.home.join("ps.log")).map_or(0, |log| sandbox::splitlines(&log).len())
}

/// 等这几台设备的上报进程各自再转 n 圈（原来 n 默认是 3）
pub fn more_rounds(devs: &[&Device], n: usize) {
    let start: Vec<usize> = devs.iter().map(|d| rounds(d)).collect();
    wait_for(&format!("上报进程再转 {n} 圈"), WAIT, || devs.iter().zip(&start).all(|(d, s)| rounds(d) >= s + n));
}

/// 上报进程最近一次没报到某个 Team 的服务器（lane 是那一路的前缀 team.<编号>.）的原因（cca devices 显示）；
/// 报成了是 ""，还没有 status.json 是 None
pub fn status(dev: &Device, lane: &str) -> Option<String> {
    let p = dev.cca.join("presence").join(format!("{lane}status.json"));
    p.exists().then(|| fsx::read_json(&p).get("error").map_or_else(String::new, |e| e.str().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_commands_are_byte_for_byte_the_python_ones() {
        // 原来 tests/presence.py 里那几段脚本的 SHA-256（改了脚本要一起改：别的测试照着它们的文件格式读）
        for (name, script, want) in [
            ("CURL", CURL, "e9cafc9e69398d12b02ca2f17d68c45f82c16de485d890ca417bf6e01cda3155"),
            ("PS", PS, "6db6c6e835fda13c79d145213a296d3ae24a34f953b2afae10696c0b7950b743"),
            ("SELF", SELF, "5701b0f7772b8bfb65d1b66c1b266bf0dc1f8d6792a67dd8eac33dd5b49052a2"),
            ("NO_KEYCHAIN", NO_KEYCHAIN, "eba1f2a439542a8e59655df83b80d8dbd4e86ee7ceaf20f705bb0225d124fb6e"),
            ("KEYCHAIN", KEYCHAIN, "ce69c3d62c8d21d0076009b309862d99f888a730f6965831e872a5b87abf1619"),
        ] {
            assert_eq!(crypto::sha256_hex(script.as_bytes()), want, "{name}");
        }
        assert!(PS.contains(T0));
    }

    #[test]
    fn t0_is_local_time() {
        let epoch = t0_epoch();
        // 换回去是同一个时刻（时区随这台机器）
        let secs = epoch as _;
        // SAFETY: tm 是这里的，localtime_r 只往里写
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&secs, &mut tm) };
        assert_eq!((tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec), (126, 10, 7, 12, 8, 55));
    }

    #[test]
    fn request_bodies() {
        let raw = call_body("report", "SHA256:x", &obj! { "units" => obj! {}, "ts" => 5, "名字" => "测试" });
        let text = String::from_utf8(raw.clone()).expect("ASCII");
        let j = json::parse(&text).expect("JSON");
        let keys: Vec<&str> = j.obj().keys().collect();
        assert_eq!(keys, ["v", "op", "key", "ts", "nonce", "units", "名字"], "extra 里同名的换掉值、位置不动");
        assert_eq!((j["ts"].i64(), j["op"].str(), j["key"].str(), j["nonce"].str().len()), (5, "report", "SHA256:x", 32));
        assert!(text.starts_with("{\"v\":1,\"op\":\"report\",") && text.is_ascii() && !text.contains(' '), "{text}");
        assert_eq!(call_body("view", "k", &Value::Null).iter().filter(|b| **b == b',').count(), 4);
        assert_eq!(tampered(b"{\"v\":1,\"x\":{\"v\":1}}"), b"{\"v\":1 ,\"x\":{\"v\":1 }}");
    }

    #[test]
    fn unknown_device_replies() {
        let gen = "0123456789abcdef0123456789abcdef";
        assert_eq!(unknown(&(403, obj! { "error" => "unknown_device", "gen" => gen })), Some(gen.to_string()));
        for bad in [
            (200, obj! { "error" => "unknown_device", "gen" => gen }),
            (403, obj! { "error" => "denied", "gen" => gen }),
            (403, obj! { "error" => "unknown_device" }),
            (403, obj! { "error" => "unknown_device", "gen" => gen, "more" => 1 }),
            (403, obj! { "error" => "unknown_device", "gen" => "0123456789ABCDEF0123456789abcdef" }),
            (403, obj! { "error" => "unknown_device", "gen" => "abc" }),
            (403, obj! { "error" => "unknown_device", "gen" => 7 }),
            (403, Value::Null),
        ] {
            assert_eq!(unknown(&bad), None, "{bad:?}");
        }
    }
}
