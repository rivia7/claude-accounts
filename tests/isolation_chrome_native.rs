//! 可选的真 Chrome 的 WebRTC 回归：只用临时 HOME、空的资料目录和本机的 HTTP / STUN（原来的 tests/isolation_chrome_native.py）。
//!
//! 运行：`CCA_TEST_CHROME_BIN='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'`，再
//! `cargo test --profile kit -p cca-tests --test suite -- isolation_chrome_native`（模块登记成 default: false，不点名不跑，
//! 回归里也不跑）。没设这个变量时跳过。macOS 的沙箱里起 Chrome 可能要另外授权。
//! 这里验证 cca 的偏好清理和启动参数对受控 ICE 的效果，不验证硬件指纹，也不覆盖企业策略、已安装的扩展或浏览器的其他网络
//! 通道。不保存候选 IP。

use kit::case;
use kit::cca_src;
use kit::fsx;
use kit::harness::{self, Ctx, Module};
use kit::json::{self, Value};
use kit::proc::{Child, Cmd, Session, Stdio};
use kit::{list, obj};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "isolation_chrome_native",
    src: include_str!("isolation_chrome_native.rs"),
    default: false,
    tests: &[case!(prepare_removes_url_exception_and_real_chrome_blocks_udp)],
};

const ACCOUNT: &str = "native-browser@example.com";
const PROXY: &str = "http://127.0.0.1:9";
/// STUN 的 magic cookie（RFC 5389）
const COOKIE: u32 = 0x2112_A442;
/// STUN 回答里的映射地址：文档专用的假 IP
const TEST_ADDRESS: [u8; 4] = [198, 51, 100, 77];

/// 测试页：收集 ICE 候选（只记类型、协议、是不是那个假 IP），收集完或 3.5 秒后 POST 到 /result
const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><script>
(async () => {
  const result = {candidates: []};
  const peer = new RTCPeerConnection({iceServers: [{urls: 'stun:127.0.0.1:PORT'}]});
  let reported = false;
  async function report() {
    if (reported) return;
    reported = true;
    result.state = peer.iceGatheringState;
    peer.close();
    await fetch('/result', {method: 'POST', body: JSON.stringify(result)});
  }
  peer.onicecandidate = event => {
    if (event.candidate) {
      const candidate = event.candidate;
      result.candidates.push({type: candidate.type, protocol: candidate.protocol,
                             testAddress: candidate.address === '198.51.100.77'});
    } else report();
  };
  peer.createDataChannel('probe');
  await peer.setLocalDescription(await peer.createOffer());
  setTimeout(report, 3500);
})().catch(() => fetch('/result', {method: 'POST', body: '{"error":true}'}));
</script>"#;

/// 本机的 HTTP 和 STUN：只记 STUN 收到几个 UDP 包、页面报回来的候选是哪几类；STUN 回答的映射地址是文档专用的假 IP
struct Probe {
    url: String,
    /// HTTP 在听的地址（收尾时连一下，叫醒等连接的线程）
    http: SocketAddr,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    seen: Mutex<Seen>,
    done: Condvar,
}

#[derive(Default)]
struct Seen {
    packets: u64,
    result: Option<Value>,
    done: bool,
}

impl Probe {
    fn start() -> Probe {
        let shared = Arc::new(Shared::default());
        let udp = UdpSocket::bind("127.0.0.1:0").expect("开得了本机的 UDP 端口");
        udp.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let page: Arc<[u8]> = PAGE.replace("PORT", &udp.local_addr().unwrap().port().to_string()).into_bytes().into();
        let http = TcpListener::bind("127.0.0.1:0").expect("开得了本机的 TCP 端口");
        let addr = http.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/", addr.port());
        let threads = vec![
            {
                let shared = shared.clone();
                thread::spawn(move || stun(&udp, &shared))
            },
            {
                let shared = shared.clone();
                thread::spawn(move || serve(&http, page, &shared))
            },
        ];
        Probe { url, http: addr, shared, threads }
    }

    /// 等页面报回结果，最多这么久：(STUN 收到的包数, 结果)；等不到是 None
    fn wait(&self, limit: Duration) -> Option<(u64, Value)> {
        let seen = self.shared.seen.lock().unwrap();
        let (seen, _) = self.shared.done.wait_timeout_while(seen, limit, |seen| !seen.done).unwrap();
        seen.done.then(|| (seen.packets, seen.result.clone().expect("报回了结果才算完")))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // 等连接的线程卡在 accept 上：连一下叫醒它，它看到 stop 就走（http.server 的 shutdown）；STUN 那个最多 0.2 秒看一次
        let _ = TcpStream::connect(self.http);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// STUN：数 Binding 请求，回一个 XOR-MAPPED-ADDRESS 是假 IP 的成功回答
fn stun(udp: &UdpSocket, shared: &Shared) {
    let cookie = COOKIE.to_be_bytes();
    let mut mapped = vec![0, 1];
    mapped.extend_from_slice(&(54321 ^ (COOKIE >> 16) as u16).to_be_bytes());
    mapped.extend(TEST_ADDRESS.iter().zip(cookie).map(|(a, b)| a ^ b));
    let mut attribute = Vec::new();
    attribute.extend_from_slice(&0x0020u16.to_be_bytes());
    attribute.extend_from_slice(&(mapped.len() as u16).to_be_bytes());
    attribute.extend_from_slice(&mapped);
    let mut buf = [0u8; 2048];
    while !shared.stop.load(Ordering::SeqCst) {
        let (n, peer) = match udp.recv_from(&mut buf) {
            Ok(got) => got,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        let request = &buf[..n];
        if n < 20 || request[..2] != [0, 1] || request[4..8] != cookie {
            continue;
        }
        shared.seen.lock().unwrap().packets += 1;
        let mut response = Vec::new();
        response.extend_from_slice(&0x0101u16.to_be_bytes());
        response.extend_from_slice(&(attribute.len() as u16).to_be_bytes());
        response.extend_from_slice(&cookie);
        response.extend_from_slice(&request[8..20]);
        response.extend_from_slice(&attribute);
        if udp.send_to(&response, peer).is_err() {
            return;
        }
    }
}

/// HTTP（像 http.server 的 ThreadingHTTPServer：一个连接一个线程，答完就关）
fn serve(http: &TcpListener, page: Arc<[u8]>, shared: &Arc<Shared>) {
    for conn in http.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        // 接受出错（对方先断了这类）：不管它，接着等下一个
        if let Ok(conn) = conn {
            let (page, shared) = (page.clone(), shared.clone());
            thread::spawn(move || answer(conn, &page, &shared));
        }
    }
}

/// 答一个请求：GET 什么路径都给测试页；POST /result（长度 1～8191）收下页面报回的结果，回 204。原来出异常
/// （长度不是数、结果不是 JSON）的地方不回答、直接断开
fn answer(mut conn: TcpStream, page: &[u8], shared: &Shared) {
    if conn.set_read_timeout(Some(Duration::from_secs(10))).is_err() {
        return;
    }
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match conn.read(&mut chunk) {
            Ok(n) if n > 0 && data.len() < 65536 => data.extend_from_slice(&chunk[..n]),
            _ => return,
        }
    };
    let text = String::from_utf8_lossy(&data[..head]).into_owned();
    let mut lines = text.split("\r\n");
    let mut request = lines.next().unwrap_or_default().split(' ');
    let (method, path) = (request.next().unwrap_or_default(), request.next().unwrap_or_default());
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().to_string());
    match method {
        "GET" => {
            let head =
                format!("HTTP/1.0 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n", page.len());
            let _ = conn.write_all(head.as_bytes()).and_then(|()| conn.write_all(page));
        }
        "POST" => {
            let Ok(length) = length.as_deref().unwrap_or("0").parse::<i64>() else { return };
            if path != "/result" || !(0 < length && length < 8192) {
                let _ = conn.write_all(b"HTTP/1.0 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                return;
            }
            let length = length as usize;
            let mut body = data[head..].to_vec();
            while body.len() < length {
                match conn.read(&mut chunk) {
                    Ok(n) if n > 0 => body.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            body.truncate(length);
            let Ok(result) = json::parse_bytes(&body) else { return };
            shared.seen.lock().unwrap().result = Some(result);
            let _ = conn.write_all(b"HTTP/1.0 204 No Content\r\n\r\n");
            shared.seen.lock().unwrap().done = true;
            shared.done.notify_all();
        }
        _ => {
            let _ = conn.write_all(b"HTTP/1.0 501 Unsupported method\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
        }
    }
}

/// 起着的 Chrome：收尾时整个进程组一起停（Chrome 的子进程也在这个组里），启动失败、等不到结果时也一样
struct Chrome(Child);

impl Drop for Chrome {
    fn drop(&mut self) {
        let group = self.0.pid();
        let _ = self.0.signal_group(libc::SIGTERM);
        let _ = self.0.wait_timeout(Duration::from_secs(5));
        // 带头的已经收走了也照发（os.killpg）：组里还有进程时这个组号不会给别的进程用上
        // SAFETY: 只是发信号，不碰内存
        unsafe { libc::killpg(group, libc::SIGKILL) };
        let _ = self.0.wait_timeout(Duration::from_secs(5));
    }
}

/// 一个用例的沙箱：临时 HOME 和要测的 Chrome
struct Fx {
    home: PathBuf,
    chrome: String,
}

impl Fx {
    fn env(&self) -> [(&'static str, String); 3] {
        let home = self.home.display().to_string();
        [("HOME", home.clone()), ("TMPDIR", home), ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin".to_string())]
    }

    fn helper(&self, tool: &str, args: &[&str]) -> String {
        let mut cmd = Cmd::new(harness::helper());
        cmd.args(args).envs(self.env()).env("CCA_TOOL", tool).timeout(Duration::from_secs(15));
        let result = kit::bash::run(&mut cmd);
        assert_eq!(result.status.py(), 0, "{}", result.err());
        result.out().trim().to_string()
    }

    /// cca 给这个资料目录、这个网址起 Chrome 的参数：执行真正的 shell 函数，只替换 open 来捕获参数，不调用 LaunchServices
    fn launch_args(&self, profile: &Path, url: &str) -> Vec<String> {
        let shell = format!(
            "open() {{ printf \"%s\\0\" \"$@\"; }}\n{}\nchrome_open_url \"$1\" \"$2\"\n",
            cca_src::function("chrome_open_url")
        );
        let mut cmd = kit::bash::c(&shell);
        cmd.arg("cca-chrome-args")
            .arg(&self.chrome)
            .arg(url)
            .envs(self.env())
            .env("ACC_CHROME_DIR", profile)
            .env("ACC_PROXY", PROXY)
            .timeout(Duration::from_secs(5));
        let result = kit::bash::run(&mut cmd);
        assert_eq!(result.status.py(), 0, "{}", result.err());
        let out = result.out();
        let arguments: Vec<String> = out.trim_end_matches('\0').split('\0').map(str::to_string).collect();
        assert_eq!(arguments[..4], ["-n", "-a", self.chrome.as_str(), "--args"]);
        let arguments = arguments[4..].to_vec();
        for wanted in [
            "--webrtc-ip-handling-policy=disable_non_proxied_udp".to_string(),
            format!("--user-data-dir={}", profile.display()),
            format!("--proxy-server={PROXY}"),
        ] {
            assert!(arguments.contains(&wanted), "没有 {wanted}：{arguments:?}");
        }
        arguments
    }

    /// 用这个资料目录起一次 Chrome 打开测试页：(STUN 收到几个 UDP 包, 页面报回的候选)
    fn ice(&self, profile: &Path) -> (u64, Vec<Value>) {
        let probe = Probe::start();
        let arguments = self.launch_args(profile, &probe.url);
        // 实际 cca 参数保持不变；以下只关闭测试无关后台功能、使用假 keychain，并允许本机 HTTP/STUN。外部 HTTP 被送到
        // 未监听的 127.0.0.1:9
        let testing = [
            "--headless=new",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-sync",
            "--disable-breakpad",
            "--disable-crash-reporter",
            "--disable-crashpad-for-testing",
            "--disable-updater-scheduler",
            "--metrics-recording-only",
            "--password-store=basic",
            "--use-mock-keychain",
            "--allow-loopback-in-peer-connection",
            "--proxy-bypass-list=127.0.0.1;localhost",
            "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost",
        ];
        let mut cmd = Cmd::new(&self.chrome);
        cmd.args(testing)
            .args(&arguments)
            .envs(self.env())
            .session(Session::New)
            .stdin(Stdio::Null)
            .stdout(Stdio::Null)
            .stderr(Stdio::Null);
        let _chrome = Chrome(cmd.spawn().unwrap_or_else(|e| panic!("起不了 {}：{e}", self.chrome)));
        let (packets, result) =
            probe.wait(Duration::from_secs(15)).unwrap_or_else(|| panic!("Chrome 未完成本机 ICE；不能把空结果当作隔离成功"));
        assert!(matches!(result, Value::Obj(_)), "{result:?}");
        assert!(result.get("error").is_none(), "本机 WebRTC 测试页面执行失败");
        assert_eq!(result.get("state").and_then(Value::as_str), Some("complete"), "ICE 未完成；不能据此判定 UDP 已阻断");
        (packets, result.get("candidates").map(|c| c.list().to_vec()).unwrap_or_default())
    }
}

/// Path.resolve()：认符号链接，还不在的那几段原样接在后面
fn resolve(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(_) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => resolve(parent).join(name),
            _ => path.to_path_buf(),
        },
    }
}

/// os.access(路径, X_OK)
fn executable(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else { return false };
    // SAFETY: c 是以 NUL 结尾的字符串，access 只读它
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// 候选里这一项是不是这个字符串（c.get(键) == 值）
fn is(candidate: &Value, key: &str, value: &str) -> bool {
    candidate.get(key).and_then(Value::as_str) == Some(value)
}

fn prepare_removes_url_exception_and_real_chrome_blocks_udp(t: &mut Ctx) {
    let chrome = std::env::var("CCA_TEST_CHROME_BIN").unwrap_or_default();
    if chrome.is_empty() {
        t.skip("设置 CCA_TEST_CHROME_BIN 后才运行真实 Chrome（仅本机服务）");
    }
    assert!(Path::new(&chrome).is_absolute(), "CCA_TEST_CHROME_BIN 必须是绝对路径");
    assert!(executable(&chrome), "CCA_TEST_CHROME_BIN 不是可执行文件");
    let fx = Fx { home: t.tmp("cca-chrome-native-"), chrome };
    for tool in ["claude", "codex"] {
        t.sub(format!("tool={tool}"), |_| {
            let profile = PathBuf::from(fx.helper(tool, &["private-profile", ACCOUNT, "browser"]));
            let (home, resolved) = (resolve(&fx.home), resolve(&profile));
            assert!(resolved.starts_with(&home) && resolved != home, "浏览器资料目录必须位于测试临时 HOME");
            let preferences = profile.join("Default/Preferences");
            let default = preferences.parent().unwrap();
            assert!(!default.exists(), "{} 已经有了", default.display());
            std::fs::create_dir_all(default).unwrap();
            let contaminated = obj! {
                "test_marker" => "preserve",
                "webrtc" => obj! {
                    "ip_handling_policy" => "disable_non_proxied_udp",
                    "ip_handling_url" => list![obj! { "url" => "*", "handling" => "default" }],
                    "local_ips_allowed_urls" => list!["*"]
                }
            };
            let contaminated = json::dumps(&contaminated, &json::Style::default());
            fsx::write(&preferences, &contaminated);
            let (baseline_packets, baseline_candidates) = fx.ice(&profile);
            assert!(baseline_packets > 0, "baseline 必须实际收到 STUN UDP；否则测试不能证明防护有效");
            assert!(
                baseline_candidates.iter().any(|c| is(c, "type", "srflx") && c.get("testAddress").is_some_and(kit::py::truthy)),
                "baseline 未接收到受控 STUN 响应"
            );

            // Chrome 完全退出后重新注入例外，再调用生产 helper 清理。
            fsx::write(&preferences, &contaminated);
            fx.helper(tool, &["browser-profile", "prepare", ACCOUNT]);
            fx.helper(tool, &["browser-profile", "check", ACCOUNT]);
            let prepared = fsx::read_json(&preferences);
            assert_eq!(prepared["test_marker"], Value::from("preserve"));
            assert_eq!(prepared["webrtc"]["ip_handling_policy"], Value::from("disable_non_proxied_udp"));
            assert_eq!(prepared["webrtc"]["ip_handling_url"], Value::List(Vec::new()));
            assert_eq!(prepared["webrtc"]["local_ips_allowed_urls"], Value::List(Vec::new()));
            let (packets, candidates) = fx.ice(&profile);
            assert_eq!(packets, 0, "准备后的账号 profile 仍向本机 STUN 发出了非代理 UDP");
            assert!(!candidates.iter().any(|c| is(c, "protocol", "udp")), "准备后的账号 profile 仍生成 UDP ICE candidate");
        });
    }
}
