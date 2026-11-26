//! Team 的同步服务器的一键部署（docs/dev/team.md「部署」）。辅助程序生成的安装脚本（hub-installer）在假服务器（kit::hubbox：临时目录
//! 当作 /，假的 systemctl、docker 起安装脚本放上去的那一份服务端程序；证书助手换成假的，照登记的地址写一张自签的证书）上运行。
//! 安装脚本里带的服务端程序本该是 Linux 的两份 cca-core：这里用这台机器上的辅助程序顶替（[`cores`]：CCA_HUB_CORE_X86_64、
//! CCA_HUB_CORE_AARCH64 都指着它），假服务器上才运行得了；假服务器上没有 Python、拉不了镜像（[`clean`] 查没人去找过）。有 /bin/dash（Debian、Ubuntu
//! 的 sh）就用它运行，有的用例用 /bin/sh（macOS、Fedora 上是 bash，Alpine 上是 busybox）。装好以后管理员的设备（kit::fleet 的
//! Device：临时 HOME、假 curl）用辅助程序真的接上、发邀请，别的设备加入。cca team deploy 的那一半（team-deploy）经假的 ssh 在同一台
//! 假服务器上运行同一个脚本。
//!
//! 原来是 tests/hub_deploy.py（InstallerTest 13 个用例、DeployTest 10 个；两个类里都有 test_web，这里叫 installer_test_web、
//! deploy_test_web）；服务端从 Python 换成 cca-core 以后加了 from_python、core_checks、no_ca_bundle、runtime_image 四个，没装完时
//! 连服务一起换回原来的以后加了 switch_rolls_back。
//! 和原来不一样的还有端口怎么来：
//! - 给安装脚本的端口（[`team_port`]、[`one_port`]）：原来是“绑一下 0 号端口、记下系统给的”，可安装脚本没写中转的端口时自己用
//!   下一个（P + 1），而 macOS 给 0 号端口的办法是整台机器一个接一个往下发——同时在跑的别的测试再要一个端口，拿到的正好是
//!   P + 1，这边的服务端就起不来（Address already in use）。现在从系统不会自己发出去的那一段里挑，四个一格：三个连着的都看过
//!   是空着的，第四个当记号由这个进程绑住，别的进程里跑着的这份测试就不会挑到同一格。安装没成就是没成，不重试
//! - “没写端口：从 18445 起找一个空着的”那两处（[`teams`]）：原来看 18445 起哪个空着就让安装脚本落在哪个上，而这个模块是在
//!   回归的后台和别的测试一起跑的，18445 起头的那几个谁都可能用。现在让假的 ss 说前面一整段都有程序占着，安装脚本落到后面
//!   这个进程占住的一段里（见 [`default_slot`]）；期望的端口照样是算出来的那一个
//!
//! 假服务器起的服务都在各自的新会话里一直运行：每建一台假服务器就登记收尾（[`stop_box`]），用例失败、超时了也停得掉。

use kit::case;
use kit::crypto;
use kit::fleet::Device;
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module};
use kit::http::Request;
use kit::hubbox::{self, Box as FakeBox, Opts, SS, UFW};
use kit::json::{self, Value};
use kit::proc::{Cmd, Session, Stdio};
use kit::sandbox::{self, Ran};
use kit::{list, obj, pki, py, tls};
use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "hub_deploy",
    src: include_str!("hub_deploy.rs"),
    default: true,
    tests: &[
        // 在服务器上直接运行安装脚本（sudo sh cca-hub-install.sh --team-root …）
        case!(script_itself),
        case!(systemd),
        case!(installer_test_web),
        case!(web_two_teams),
        case!(web_old_helper),
        case!(web_skipped),
        case!(docker),
        case!(compose),
        case!(dir),
        case!(failed_update_rolls_back),
        case!(switch_rolls_back),
        case!(needs),
        case!(old_own_server),
        case!(teams),
        case!(from_python),
        case!(core_checks),
        case!(no_ca_bundle),
        case!(runtime_image),
        case!(push),
        // cca team deploy（team-deploy）：经假的 ssh 把同一个脚本交给假服务器，装好了接上
        case!(deploy),
        case!(undeploy),
        case!(recorded_host),
        case!(deploy_test_web),
        case!(args),
        case!(unreachable_then_again),
        case!(not_root),
        case!(dropped_after_start),
        case!(login_check_and_password),
        case!(ssh_and_cert),
    ],
};

/// 以前的版本（3.6.x）的安装脚本用的 python 镜像：摆成以前的样子时写进 .env
const OLD_IMAGE: &str = "public.ecr.aws/docker/library/python:3.13-slim";
/// 假服务器上放 unit 的目录（root/ 下面）
const UNITS: &str = "etc/systemd/system";
/// 这台服务器上几个 Team 共用的证书助手（/opt/cca-team-acme）
const ASVC: &str = "cca-team-acme";
/// 等的时限（原来 wait_for 默认的 60 秒）：机器再忙也该等得到
const WAIT: Duration = Duration::from_secs(60);
/// 安装脚本没写端口时从这里起找一个空着的（server/hub-install.sh 的 DEF_PORT；容器里听的也是它）
const DEF_PORT: u16 = 18445;

/// 设备上的 ssh：-G 说这台假服务器在 127.0.0.1（$HUB_SSH_HOST 换一个：ssh 走的是内网地址）；别的把最后一个参数（在服务器上运行的
/// 命令）交给假服务器的 enter（stdin 是安装脚本）
const SSH: &str = r#"printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname %s\nport 22\n' "${HUB_SSH_HOST:-127.0.0.1}"; exit 0; }; done
for a in "$@"; do cmd=$a; done
exec "$HUB_ENTER" "$cmd"
"#;
/// 要密码的服务器（$HUB_PW 文件里是密码）：不认这台设备的密钥；用密码登录时像真的 ssh 一样经 SSH_ASKPASS 取密码
/// （SSH_ASKPASS_REQUIRE=force），每次连接取一次（$HOME/askpass.n 记着取了几次）
const SSH_PW: &str = r#"printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname 127.0.0.1\nport 22\n'; exit 0; }; done
case " $* " in
	*" PubkeyAuthentication=no "*)
		[ "${SSH_ASKPASS_REQUIRE:-}" = force ] && [ -n "${SSH_ASKPASS:-}" ] || { echo 'root@box: Permission denied (no askpass).' >&2; exit 255; }
		pw=$("$SSH_ASKPASS") || exit 255
		echo x >> "$HOME/askpass.n"
		[ "$pw" = "$(cat "$HUB_PW")" ] || { echo 'root@box: Permission denied (publickey,password).' >&2; exit 255; } ;;
	*) echo 'root@box: Permission denied (publickey,password).' >&2; exit 255 ;;
esac
for a in "$@"; do cmd=$a; done
exec "$HUB_ENTER" "$cmd"
"#;
/// ssh 连不上、主机密钥不对……：stderr 是 $HUB_SSH_SAYS
const SSH_SAYS: &str = r#"printf '%s\n' "$*" >> "$HOME/ssh.argv"
for a in "$@"; do [ "$a" = -G ] && { printf 'user root\nhostname 127.0.0.1\nport 22\n'; exit 0; }; done
printf '%s\n' "$HUB_SSH_SAYS" >&2
exit 255
"#;

// ---- 安装脚本里带的服务端程序 ----

/// 安装脚本里带的两份服务端程序用哪个文件顶替（给辅助程序的环境变量，见 cli/src/cmd/hub/payload.rs：仓库里的 cca 认它们）：都是
/// 这台机器上的辅助程序。真的是 Linux 的 x86_64、aarch64 两份，在 Mac 上运行不了；在 Linux 上跑这份测试时辅助程序本来就是
/// 其中一份，另一份只是带着、用不到
fn cores() -> [(&'static str, String); 2] {
    let helper = harness::helper().to_string_lossy().into_owned();
    [("CCA_HUB_CORE_X86_64", helper.clone()), ("CCA_HUB_CORE_AARCH64", helper)]
}

/// 让辅助程序生成一份安装脚本（hub-installer），写成 dir/name：env 是带哪几份服务端程序（[`cores`]，或者别的文件）
fn installer(dir: &Path, name: &str, env: &[(&str, &str)]) -> PathBuf {
    let out = Cmd::new(harness::helper())
        .arg("hub-installer")
        .env("HOME", dir)
        .env("PATH", "/usr/bin:/bin")
        .env("CCA_VERSION", "9.9.9")
        .envs(env.iter().copied())
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap_or_else(|e| panic!("hub-installer 运行不了：{e}"));
    assert_eq!((out.status.py(), out.err().as_str()), (0, ""));
    let script = dir.join(name);
    fsx::write(&script, &out.stdout);
    script
}

// ---- 端口 ----

/// 给安装脚本的端口从这一段里挑：21000 起，四个一格，2000 格（到 28999）。系统给 0 号端口时发的那一段（macOS 49152 起，Linux
/// 32768 起）在它外面：别的测试“绑 0 号端口”拿不到这里面的
const POOL_FIRST: u16 = 21000;
const POOL_BLOCKS: u16 = 2000;

/// 这个用例拿着的记号（见 [`claim_block`]、[`default_slot`]）：用例完了放开（[`setup`] 登记的收尾，排在停假服务器的后面）
static HELD: Mutex<Vec<OwnedFd>> = Mutex::new(Vec::new());

fn keep(mark: OwnedFd) {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).push(mark);
}

fn release_ports() {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// 在 127.0.0.1 的这个端口上绑一下（只绑不听；不设 SO_REUSEADDR：有程序绑着这个端口——不管是 127.0.0.1 还是所有地址——就绑不上）。
/// 绑上了给出那个描述符：拿着它，别的程序就绑不了这个端口
fn hold_port(port: u16) -> Option<OwnedFd> {
    // SAFETY: socket 给的描述符马上交给 OwnedFd 管；addr 是填好的 sockaddr_in，长度照它的大小给
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        assert!(fd >= 0, "建不了 socket：{}", std::io::Error::last_os_error());
        let fd = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = port.to_be();
        addr.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
        let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        (libc::bind(fd.as_raw_fd(), std::ptr::addr_of!(addr).cast(), len) == 0).then_some(fd)
    }
}

/// 这个端口现在空着（原来的 port_free）：绑得上，马上放开
fn port_free(port: u16) -> bool {
    hold_port(port).is_some()
}

/// 占一格，给出它的头一个端口 P：P、P + 1、P + 2 这时都空着，给被测的服务用；第四个（P + 3）是记号，这个进程绑住它（只绑不听）
/// 一直拿到用例完。要用哪一格都得先绑上它的记号，所以同一时间别的进程里跑着的这份测试（回归的后台、别的会话：两边一样的用例、
/// 一样的节奏，最容易撞上）不会挑到同一格；这个用例里要几个端口，也各是各的一格。随机挑一格，绑不上记号、里面有端口被别的程序
/// 占着就换一格；找不到就 panic。给出去以后安装没成就是没成，不重试
fn claim_block() -> u16 {
    for _ in 0..500 {
        let r = crypto::random(2);
        let first = POOL_FIRST + 4 * (u16::from_be_bytes([r[0], r[1]]) % POOL_BLOCKS);
        let Some(mark) = hold_port(first + 3) else { continue };
        if (0..3).all(|i| port_free(first + i)) {
            keep(mark);
            return first;
        }
    }
    panic!("{POOL_FIRST} 起的 {POOL_BLOCKS} 格里找不到空着的一格");
}

/// 一个 Team 的 HTTPS 端口 P（`--port`）：P、P + 1、P + 2 都空着。后两个是安装脚本没写时自己接着往下选的——中转的数据端口
/// P + 1，网页端 P + 2（这一次不开也选好、记着，以后不写 --web-port 开的就是它）。原来是 free_port()：只看了 P，而系统
/// 给 0 号端口是整台机器一个接一个往下发的，P + 1 正好是下一个要端口的程序拿到的
fn team_port() -> u16 {
    claim_block()
}

/// 单独的一个空着的端口（`--relay-port`、`--web-port` 另写的那种）：自己占一格，不会是哪个 Team 接着往下选的那两个
fn one_port() -> u16 {
    claim_block()
}

/// HTTPS 的端口和网页端的端口（原来的 two_ports）：都空着；网页端的不是中转自动选的那一个（HTTPS 的下一个）
fn two_ports() -> (u16, u16) {
    (team_port(), one_port())
}

/// n 个 Team 各自的 (HTTPS 的端口, 网页端的端口)（原来的 apart）：都空着，互不相同，也不是谁的中转自动选的那一个
fn apart(n: usize) -> Vec<(u16, u16)> {
    (0..n).map(|_| two_ports()).collect()
}

/// “没写端口：从 18445 起找一个空着的”那两处（[`teams`]）用：给出 k——让假的 ss 说 18445 … 18445 + k - 1 这 k 个端口都有程序
/// 占着，安装脚本就该落在 18445 + k 上（怎么挑照样测到了：跳过占着的、落在头一个空着的上）。
///
/// 原来的 k 是 18445 起头一个真空着的（一般是 0）。可 18445 起头的那几个谁都可能用——别的会话里跑着的这份测试、回归里部署到
/// 假服务器的那一节、手工起的服务器——刚看过空着，装的时候可能就被占了。安装脚本最多往后找 50 个（pick_port 的上限，18445～
/// 18495），这里把 18453 起的那一段分成 7 段、每段 6 个端口（k = 8、14、…、44），随机挑一段：
/// - 18445 + k、+1：乙组的 HTTPS 和中转；+3、+4：后面丙组的。这四个是服务端真要绑的，挑的时候看过都空着
/// - 18445 + k + 2、+5：两个组“选好、记着”的网页端的端口。地址是 127.0.0.1（内网的），网页端不开，没有程序绑它们——拿它们
///   当这一段的记号：这个进程把它们绑住（只绑不听）一直拿到用例完，别的进程里跑着的这份测试绑不上，就换一段
fn default_slot() -> u16 {
    const FIRST: u16 = 8;
    const STEP: u16 = 6;
    const SLOTS: u16 = 7;
    let start = u16::from(crypto::random(1)[0]) % SLOTS;
    for i in 0..SLOTS {
        let k = FIRST + STEP * ((start + i) % SLOTS);
        let first = DEF_PORT + k;
        let (Some(a), Some(b)) = (hold_port(first + 2), hold_port(first + 5)) else { continue };
        if [0, 1, 3, 4].into_iter().all(|i| port_free(first + i)) {
            keep(a);
            keep(b);
            return k;
        }
    }
    panic!("{}～{} 的 {SLOTS} 段都有程序占着", DEF_PORT + FIRST, DEF_PORT + FIRST + STEP * SLOTS - 1);
}

// ---- 小工具 ----

/// haystack 里有 needle（assertIn）
#[track_caller]
fn assert_in(needle: &str, haystack: &str) {
    assert!(haystack.contains(needle), "没有 {needle:?}：\n{haystack}");
}

/// haystack 里没有 needle（assertNotIn）
#[track_caller]
fn assert_not_in(needle: &str, haystack: &str) {
    assert!(!haystack.contains(needle), "不该有 {needle:?}：\n{haystack}");
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn p(path: &Path) -> &str {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display()))
}

/// 目录里的名字，排好（`sorted(p.name for p in d.iterdir())`）
fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读不了 {}：{e}", dir.display()))
        .map(|e| e.expect("目录项").file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// 内容当文字（`body.decode()`）
fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("回的不是 UTF-8")
}

/// 全是数字（`str.isdigit()`）
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// 一组 (名字, 值)，都是文字
fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// 同上，当字典
fn map(items: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs(items).into_iter().collect()
}

/// d 下面这几个路径的权限位（`oct(st_mode & mask)` 的写法：0o755）
fn modes(d: &Path, paths: &[&str], mask: u32) -> Vec<(String, String)> {
    paths.iter().map(|path| (path.to_string(), format!("0o{:o}", fsx::mode(d.join(path)) & mask))).collect()
}

/// 字典里这几项的值，照给的先后（没有就 panic：原来是 KeyError）
fn pick(m: &BTreeMap<String, String>, keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| m.get(*k).cloned().unwrap_or_else(|| panic!("没有 {k}：{m:?}"))).collect()
}

/// 装在 d 里的那个 Team 的服务器的证书公钥的指纹（sha256//…）
fn pin(d: &Path) -> String {
    pki::openssl_pin(d.join("certs").join("cert.pem"))
}

/// 根公钥后面那一段（`root.split()[1]`）
fn key_of(root: &str) -> &str {
    root.split_whitespace().nth(1).unwrap_or_else(|| panic!("根公钥不是两段：{root:?}"))
}

/// team-deploy 成了时的输出：sh 的 键=值，一行一项
fn kv(out: &str) -> BTreeMap<String, String> {
    py::splitlines(out)
        .into_iter()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let first = sandbox::shell_split(l).into_iter().next().unwrap_or_else(|| panic!("这一行是空的：{out:?}"));
            match first.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => panic!("不是 键=值：{first:?}（整个输出：{out:?}）"),
            }
        })
        .collect()
}

/// 没成时的输出：`原因码<TAB>说明` 正好两栏（`code, why = r.stdout.strip().split('\t')`）
fn two_fields(out: &str) -> (&str, &str) {
    let fields: Vec<&str> = out.trim().split('\t').collect();
    assert_eq!(fields.len(), 2, "不是两栏：{out:?}");
    (fields[0], fields[1])
}

/// GET https://127.0.0.1:端口/路径（不验证书：网页端的证书是假的证书助手自签的）：(状态码, 内容)
fn https(port: u16, path: &str) -> (u16, Vec<u8>) {
    let url = format!("https://127.0.0.1:{port}{path}");
    let r = Request::get(&url)
        .tls(tls::Client::insecure())
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap_or_else(|e| panic!("GET {url}：{e}"));
    (r.status, r.body)
}

/// 服务器的 /v1/health
fn health(port: u16) -> Value {
    let (st, body) = https(port, "/v1/health");
    assert_eq!(st, 200);
    json::parse_bytes(&body).unwrap_or_else(|e| panic!("/v1/health 回的不是 JSON：{e}"))
}

/// 仓库里的 server/
fn server_dir() -> PathBuf {
    harness::repo().join("server")
}

/// 假服务器上 /opt 下的一个目录
fn opt(b: &FakeBox, name: &str) -> PathBuf {
    b.root.join("opt").join(name)
}

/// 安装目录 d 的 .env 里的一项（没有就 panic：原来是 KeyError）
fn env_of(b: &FakeBox, d: &Path, key: &str) -> String {
    b.envfile(Some(d)).get(key).cloned().unwrap_or_else(|| panic!("{}/.env 里没有 {key}", d.display()))
}

/// 这个服务的系统用户的组（假的 useradd 记下的）
fn gid(b: &FakeBox, svc: &str) -> String {
    fsx::read_text(b.d.join("users").join(svc)).trim().to_string()
}

/// 假的 ss：listening 里的端口有程序在监听
fn fake_ss(b: &FakeBox) {
    b.fake("ss", &format!("#!/bin/sh\n{SS}"));
}

/// 这台假服务器上没人找过 Python、没人拉过镜像（服务端是安装脚本带着的 cca-core，Docker 的镜像是本机建的）
#[track_caller]
fn clean(b: &FakeBox) {
    assert_eq!(b.log("python3.log"), "", "有人调了 python3");
    let pulled: Vec<String> = b.log("docker.log").lines().filter(|l| l.starts_with("pull")).map(str::to_string).collect();
    assert!(pulled.is_empty(), "有人拉了镜像：{pulled:?}");
}

/// 辅助程序（安装脚本里带的那一份服务端程序就是它）的内容
fn helper_bytes() -> Vec<u8> {
    fsx::read(harness::helper())
}

/// 设备记着的 team.json
fn team_json(dev: &Device, tid: &str) -> Value {
    fsx::read_json(dev.cca.join("teams").join(tid).join("team.json"))
}

/// `team-sync` 的第三栏（`a.ok('team-sync').split('\t')[2]`）
fn sync_state(dev: &Device) -> String {
    let out = dev.ok(&["team-sync"]);
    out.split('\t').nth(2).unwrap_or_else(|| panic!("team-sync 的输出没有第三栏：{out:?}")).to_string()
}

// ---- 假服务器 ----

/// 这台假服务器起过的服务的进程号（目录下的 server*.pid）
fn server_pids(b: &FakeBox) -> Vec<i32> {
    let Ok(rd) = std::fs::read_dir(&b.d) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("server") && name.ends_with(".pid")
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok()?.trim().parse().ok())
        .filter(|pid| *pid > 0)
        .collect()
}

/// 收尾（原来的 `addCleanup(b.stop)`）：停掉这台假服务器起的所有服务，并且等它们真的没了——服务在自己的会话里，不是这个
/// 进程的子进程，运行器不认得；光发 TERM 不等的话，模块跑完时还没退干净的会被记成 LEFTOVER。3 秒还不退的整组 KILL
fn stop_box(b: &FakeBox) {
    let pids = server_pids(b);
    b.stop(None);
    // SAFETY: 信号 0 不发信号，只问这个进程还在不在
    let alive = |pid: &i32| unsafe { libc::kill(*pid, 0) == 0 };
    let gone_within = |limit: Duration| {
        let end = Instant::now() + limit;
        while pids.iter().any(alive) {
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    };
    if !gone_within(Duration::from_secs(3)) {
        for pid in pids.iter().filter(|pid| alive(pid)) {
            // SAFETY: 进程号是这台假服务器自己记下的、它起的服务（各自一个会话，进程组号就是进程号；上面筛过是正数）
            unsafe { libc::killpg(*pid, libc::SIGKILL) };
        }
        gone_within(Duration::from_secs(2));
    }
}

/// 在服务器上直接运行（`b.install(script, *args, shell=…, env=…)`）：退出码和两路输出，成不成都给出来
fn run_on_with(b: &FakeBox, script: &Path, args: &[&str], shell: &str, env: &[(&str, &str)]) -> Ran {
    let out = b.install_with(script, args, shell, env);
    Ran { code: out.status.py(), out: out.out(), err: out.err() }
}

/// 同上，sh 是假服务器上的那个，不另加环境变量
fn run_on(b: &FakeBox, script: &Path, args: &[&str]) -> Ran {
    run_on_with(b, script, args, hubbox::sh(), &[])
}

fn plain() -> Opts {
    Opts::default()
}

fn any_ip() -> Opts {
    Opts { any_ip: true, ..Opts::default() }
}

fn with_docker() -> Opts {
    Opts { docker: true, ..Opts::default() }
}

fn docker_any_ip() -> Opts {
    Opts { docker: true, any_ip: true, ..Opts::default() }
}

/// 推送网关证书公钥的指纹（--push-pin 的样子：32 个 0 字节的 base64）。网关的地址是 192.0.2.10：没有程序去连它
const PUSH_PIN: &str = "sha256//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

// ---- 夹具 ----

/// 每个用例自己的：临时目录（假服务器、设备都建在里面）和安装脚本
struct Fx {
    dir: PathBuf,
    script: PathBuf,
}

/// 建临时目录，让辅助程序生成安装脚本（hub-installer）放进去。每个用例生成一次：
/// 把模板打出来、带上两份服务端程序（现压，几十毫秒），用例之间就没有共用的东西了
fn setup(t: &mut Ctx) -> Fx {
    // 最先登记，最后才跑：假服务器都停了，再放开这个用例占着的端口
    t.defer(release_ports);
    let dir = t.tmp("cca-hub-deploy-");
    let both = cores();
    let both: Vec<(&str, &str)> = both.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let script = installer(&dir, "cca-hub-install.sh", &both);
    Fx { dir, script }
}

impl Fx {
    /// 一台假服务器；用例完了停掉它起的服务
    fn hub(&self, t: &mut Ctx, opts: &Opts) -> FakeBox {
        let b = FakeBox::new(self.dir.join(format!("box-{}", crypto::token_hex(4))), opts);
        t.defer({
            let b = b.clone();
            move || stop_box(&b)
        });
        b
    }

    /// 一台设备，设备上的 ssh 是假的（连到 HUB_ENTER 那台假服务器）；它部署时带的服务端程序也是顶替的那一份（[`cores`]）
    fn admin(&self, name: &str) -> Device {
        let mut dev = Device::new(&self.dir.join(format!("{name}-{}", crypto::token_hex(4))), name, &obj! {});
        dev.fake("ssh", SSH);
        dev.env.extend(cores().into_iter().map(|(k, v)| (k.to_string(), v)));
        dev
    }

    /// 管理员、它建的 Team：(设备, 编号, 根公钥, 服务名)
    fn team(&self, name: &str) -> (Device, String, String, String) {
        let a = self.admin("admin");
        let (tid, root, svc) = new_team(&a, name);
        (a, tid, root, svc)
    }

    /// 在服务器上直接运行（sudo sh cca-hub-install.sh …），该成的
    fn install(&self, b: &FakeBox, args: &[&str]) -> Ran {
        self.install_with(b, args, hubbox::sh(), &[])
    }

    /// 同上，指定用哪个 sh，另加环境变量
    fn install_with(&self, b: &FakeBox, args: &[&str], shell: &str, env: &[(&str, &str)]) -> Ran {
        let r = run_on_with(b, &self.script, args, shell, env);
        assert_eq!((r.code, r.err.as_str()), (0, ""), "{}", r.out);
        r
    }

    /// 运行安装脚本，成不成都给出来（原来直接调 `b.install(self.script, …)` 的地方）
    fn try_install(&self, b: &FakeBox, args: &[&str]) -> Ran {
        run_on(b, &self.script, args)
    }
}

/// dev 建一个 Team（它是管理员）：编号、根公钥、服务名
fn new_team(dev: &Device, name: &str) -> (String, String, String) {
    let out = dev.ok(&["team-new", name, "--device", "studio"]);
    let mut lines = out.split('\n');
    let (Some(tid), Some(root)) = (lines.next(), lines.next()) else { panic!("team-new 的输出不到两行：{out:?}") };
    (tid.to_string(), root.to_string(), format!("cca-team-{}", &tid[..8]))
}

/// 跑一条该失败（退出码 1）的命令，给出原因码（stdout 的第一栏）
fn code(dev: &Device, argv: &[&str], env: &[(&str, &str)]) -> String {
    let r = dev.run_with(argv, "", env);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    r.out.split('\t').next().unwrap_or_default().to_string()
}

/// `team-deploy 编号 root@box 参数…`，假 ssh 连到 b；成不成都给出来（原来 `deploy(…, ok=False)`）
fn try_deploy(dev: &Device, tid: &str, b: &FakeBox, args: &[&str], env: &[(&str, &str)]) -> Ran {
    let mut argv = vec!["team-deploy", tid, "root@box"];
    argv.extend_from_slice(args);
    let mut all = vec![("HUB_ENTER", p(&b.enter)), ("CCA_VERSION", "9.9.9")];
    all.extend_from_slice(env);
    dev.run_with(&argv, "", &all)
}

/// 同上，该成的（原来的 `deploy(…)`）
fn deploy_ok(dev: &Device, tid: &str, b: &FakeBox, args: &[&str], env: &[(&str, &str)]) -> Ran {
    let r = try_deploy(dev, tid, b, args, env);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    r
}

// ---- 在服务器上直接运行安装脚本（sudo sh cca-hub-install.sh --team-root …）----

/// 生成的脚本：sh -n 过得去、带着版本和两份服务端程序（校验值、解出来的都是那个文件）；模板本身不能运行；参数、root 的检查（什么都不动）
fn script_itself(t: &mut Ctx) {
    let fx = setup(t);
    let s = fsx::read_text(&fx.script);
    let shells: BTreeSet<&str> = [hubbox::sh(), "/bin/sh"].into_iter().collect();
    for sh in shells {
        let out = Cmd::new(sh).arg("-n").arg(&fx.script).output().unwrap_or_else(|e| panic!("{sh} -n 运行不了：{e}"));
        assert_eq!(out.status.py(), 0, "{sh}：{}", out.err());
    }
    assert_in("\nCCA_HUB_VERSION='9.9.9'\nCCA_HUB_CORES='X86_64 AARCH64'\n", &s);
    // 带着的两份服务端程序：校验值是那个文件的；那一段（gzip 过的、base64）照安装脚本的办法解出来也是它
    let sum = crypto::sha256_hex(&helper_bytes());
    for arch in ["X86_64", "AARCH64"] {
        assert_in(&format!("\nCCA_HUB_CORE_SHA256_{arch}='{sum}'\n"), &s);
        let open = format!("\nhub_core_{arch}() {{\n\tcat <<'CCA_HUB_CORE_{arch}_EOF'\n");
        let close = format!("\nCCA_HUB_CORE_{arch}_EOF\n}}\n");
        let from = s.find(&open).unwrap_or_else(|| panic!("没有 {arch} 的那一段")) + open.len();
        let to = from + s[from..].find(&close).unwrap_or_else(|| panic!("{arch} 的那一段没有结束行"));
        let out = Cmd::new("/bin/sh")
            .args(["-c", "openssl base64 -d | gzip -dc | openssl dgst -sha256 -r"])
            .pass_env(&["PATH"])
            .input(format!("{}\n", &s[from..to]))
            .output()
            .unwrap_or_else(|e| panic!("解不了 {arch} 的那一段：{e}"));
        assert!(out.out().starts_with(&sum), "{arch} 的那一段解出来不是那个文件：{}{}", out.out(), out.err());
    }
    assert!(!(s.contains("python3") || s.contains("docker pull")), "服务器上不用 Python、不拉镜像");
    let managed = regex::Regex::new(r"(?m)^CCA_HUB_MANAGED=1$").expect("正则");
    assert!(!managed.is_match(&s), "直接在服务器上运行的那一份不带 team-deploy 放进的那几项");
    let b = fx.hub(t, &plain());
    let r = run_on(&b, &server_dir().join("hub-install.sh"), &[]);
    assert_eq!(r.code, 1);
    assert_in("模板", &r.err);
    let (_a, _tid, root, _svc) = fx.team("甲组");
    let r = fx.try_install(&b, &["--team-root", &root, "--port", "abc"]);
    assert_eq!((r.code, r.err.trim()), (1, "✗ --port 要是 1024～65535 的端口"));
    let r = fx.try_install(&b, &["--team-root", &root, "--web-port", "80"]);
    assert_eq!((r.code, r.err.trim()), (1, "✗ --web-port 要是 1024～65535 的端口"));
    let r = fx.try_install(&b, &["--dir", "relative"]);
    assert_in("绝对路径", &r.err);
    // 会落到 / 或者别处：卸载时删的是目录里的 data 这些
    for d in ["/", "//", "/.", "/opt/../etc", "/a//b", "/a b"] {
        let r = fx.try_install(&b, &["--dir", d]);
        assert_eq!(r.code, 1, "{d}");
        assert!(r.err.contains("要是 / 下面的一个目录"), "{d}：{}", r.err);
    }
    // systemd 的服务看不到、写不了
    for d in ["/home/me/hub", "/home", "/etc/cca-hub", "/usr/local/hub/", "/tmp/hub", "/run/hub"] {
        let r = fx.try_install(&b, &["--team-root", &root, "--dir", d]);
        assert_eq!(r.code, 1, "{d}");
        assert!(r.err.contains("systemd 方式的服务看不到"), "{d}：{}", r.err);
    }
    let r = fx.try_install(&b, &["--bogus"]);
    assert_eq!(r.code, 2);
    assert_in("用法", &r.err);
    let cases: [(&[&str], &str); 4] = [
        (&[], "要给 --team-root"),
        (&["--dir", "/opt/new"], "/opt/new 里没有装过 Team 的同步服务器"),
        (&["--team-root", &root, "--reset"], "没有 --reset"),
        (&["--team-root", "ssh-ed25519 AAAA"], "根公钥不对"),
    ];
    for (args, want) in cases {
        let r = fx.try_install(&b, args);
        assert_eq!(r.code, 1, "{args:?}");
        assert!(r.err.contains(want), "{args:?}：没有 {want:?}：{}", r.err);
    }
    fsx::write(b.d.join("uid"), "1000");
    let r = fx.try_install(&b, &["--team-root", &root]);
    assert_eq!(r.code, 1);
    assert_in("要用 root 运行", &r.err);
    assert_eq!(names(&b.root), ["etc", "run"], "什么都没动");
}

/// systemd：装好（服务端程序是安装脚本带着的 cca-core，不找 Python）、打出管理员接上它的那一行（地址是内网的：网页端不开，说一声）；目录、权限、unit；再运行是更新；换端口
/// 照记下的地址再打一次那一行；--uninstall
fn systemd(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let units = b.root.join(UNITS);
    let dir_arg = format!("/opt/{svc}");
    let port = team_port();
    let (ps, relay) = (port.to_string(), (port + 1).to_string());
    let r = fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &ps]);
    assert_in(&format!("✓ Team 的同步服务器在运行（/opt/{svc}，systemd，端口 {port}，服务名 {svc}）"), &r.out);
    assert_in(&format!("cca team attach Team https://127.0.0.1:{port} {}", pin(&d)), &r.out);
    assert_in("手机的网页端没开：127.0.0.1 是内网地址", &r.out);
    assert_in(&format!("要放行 TCP {port}、{}（远程会话的中转）。", port + 1), &r.out);
    let e = b.envfile(Some(&d));
    assert_eq!(
        pick(&e, &["CCA_HUB_MODE", "CCA_HUB_PORT", "CCA_TEAM_ROOT", "CCA_HUB_RELAY", "CCA_HUB_HOST", "CCA_HUB_VERSION"]),
        ["systemd", ps.as_str(), key_of(&root), relay.as_str(), "127.0.0.1", "9.9.9"]
    );
    assert!(
        is_digits(&e["CCA_HUB_WEB"]) && e["CCA_HUB_WEB"] != ps && e["CCA_HUB_WEB"] != relay,
        "网页端的端口照样选好、记着（下次再看）"
    );
    assert!(!e.contains_key("CCA_HUB_IMAGE") && !e.contains_key("CCA_HUB_WEB_URL"));
    assert_eq!(
        modes(&d, &[".", ".env", ".cca-hub", "app", "app/cca-core", "certs", "certs/key.pem", "certs/cert.pem", "data"], 0o777),
        pairs(&[
            (".", "0o755"),
            (".env", "0o600"),
            (".cca-hub", "0o644"),
            ("app", "0o755"),
            ("app/cca-core", "0o755"),
            ("certs", "0o700"),
            ("certs/key.pem", "0o600"),
            ("certs/cert.pem", "0o644"),
            ("data", "0o700"),
        ])
    );
    assert_eq!(names(&d), [".cca-hub", ".env", "app", "certs", "data"], "证书助手不在这里（共用的那一个）");
    assert!(!d.join(".env.prev").exists(), "装好了：不留原来的 .env");
    let unit = fsx::read_text(units.join(format!("{svc}.service")));
    for want in [
        format!("User={svc}\n"),
        format!("EnvironmentFile=/opt/{svc}/.env\n"),
        "NoNewPrivileges=yes".to_string(),
        "ProtectSystem=full".to_string(),
        "TasksMax=64\n".to_string(),
        format!("ExecStart=/opt/{svc}/app/cca-core team-server --mode team --listen 0.0.0.0:${{CCA_HUB_PORT}} --cert /opt/{svc}/certs/cert.pem"),
        format!("--relay-listen 0.0.0.0:{} --relay-port {}\n", port + 1, port + 1),
    ] {
        assert_in(&want, &unit);
    }
    assert_not_in("python", &unit);
    assert_not_in("--web", &unit);
    assert!(!(units.join(format!("{ASVC}.service")).exists() || opt(&b, ASVC).exists()), "网页端没开：没有证书助手");
    assert_in(&format!("-r -M -d /opt/{svc}"), &b.log("useradd.log"));
    assert_in(&format!("-R {svc} {d}/certs {d}/data\n", d = d.display()), &b.log("chown.log"));
    assert_eq!(
        b.log("systemctl.log").split('\n').take(3).collect::<Vec<_>>(),
        ["daemon-reload".to_string(), format!("enable {svc}"), format!("restart {svc}")]
    );
    assert!(fsx::read(d.join("app/cca-core")) == helper_bytes(), "装上去的服务端程序不是安装脚本带着的那一份");
    assert_eq!(names(&d.join("app")), ["cca-core"], "没写 --name：没有 name；网页端的文件编在程序里；解的时候用的那个临时文件不留");
    // 起的就是装上去的那个文件
    let started = b.log("server.argv");
    assert_in(&format!("{} team-server --mode team --listen 127.0.0.1:{port} ", d.join("app/cca-core").display()), &started);
    let url = format!("https://127.0.0.1:{port}");
    assert_eq!(a.ok(&["team-attach", &tid, &url, &pin(&d)]).split('\n').next(), Some(format!("127.0.0.1:{port}").as_str()));
    // 再运行（只写目录）：更新，端口、证书没变：不再打接上它的那一行
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in("更新了服务端程序", &r.out);
    assert_not_in("cca team attach", &r.out);
    assert_eq!(sync_state(&a), "ok");
    // 换端口：照记下的地址打出接上它的那一行（管理员重新接上，各台设备照配置里的新地址换过来）
    let p2 = team_port();
    let r = fx.install(&b, &["--dir", &dir_arg, "--port", &p2.to_string()]);
    assert_in(&format!("cca team attach Team https://127.0.0.1:{p2} {}", pin(&d)), &r.out);
    assert_eq!(env_of(&b, &d, "CCA_HUB_PORT"), p2.to_string());
    // 卸载：停掉、删掉服务器上的文件和用户
    let r = fx.install(&b, &["--uninstall", "--dir", &dir_arg]);
    assert_in("卸载了 Team 的同步服务器", &r.out);
    assert!(!(d.exists() || units.join(format!("{svc}.service")).exists() || b.running(&svc)));
    assert_eq!(names(&b.d.join("users")), Vec::<String>::new());
    assert_in(&format!("disable --now {svc}"), &b.log("systemctl.log"));
    clean(&b);
}

/// 手机的网页端（systemd）：另开一个端口；这台服务器上共用的证书助手（cca-team-acme：自己的系统用户，只给绑 80 端口的权限）照登记的
/// 地址要来证书、放进这个 Team 的那一份（证书助手的用户、这个 Team 的组，setgid），服务端用它给出页面，标题带上 Team 的名字
/// （--name）；再运行照旧；--no-web 去掉登记、停掉证书助手（没有别的 Team 用它了；证书留着）、再运行还是关着；80 端口被别的程序占着
/// 时不开（写了 --web-port 就停下）；证书没要到时说放行 80；卸载最后一个用它的 Team 时连同证书助手一起删掉
fn installer_test_web(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &any_ip());
    let (_a, _tid, root, svc) = fx.team("甲组");
    let (d, ad) = (opt(&b, &svc), opt(&b, ASVC));
    let units = b.root.join(UNITS);
    let dir_arg = format!("/opt/{svc}");
    let (port, web) = two_ports();
    let (ps, ws) = (port.to_string(), web.to_string());
    let url = format!("https://127.0.0.1:{web}/");
    let r = fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &ps, "--web-port", &ws, "--name", "甲组"]);
    assert_in(&format!("手机的网页端：{url}（证书是 Let's Encrypt 签的"), &r.out);
    assert_in(
        &format!("要放行 TCP {port}、{}（远程会话的中转）、{web} 和 80（手机的网页端；80 端口给证书机构验证）", port + 1),
        &r.out,
    );
    let e = b.envfile(Some(&d));
    assert_eq!(pick(&e, &["CCA_HUB_WEB", "CCA_HUB_WEB_URL"]), [ws.as_str(), url.as_str()]);
    assert_eq!(fsx::read_text(d.join("app/name")), "甲组\n");
    let unit = fsx::read_text(units.join(format!("{svc}.service")));
    assert_in(
        &format!(
            " --web-url {url} --web-listen 0.0.0.0:{web} --web-cert /opt/{ASVC}/webcert/{svc}/fullchain.pem --web-key \
             /opt/{ASVC}/webcert/{svc}/privkey.pem --web-name-file /opt/{svc}/app/name\n"
        ),
        &unit,
    );
    assert!(unit.contains("TasksMax=512\n"), "网页端开着：每条 WebSocket 一个线程");
    let acme = fsx::read_text(units.join(format!("{ASVC}.service")));
    for want in [
        format!("User={ASVC}\n"),
        "AmbientCapabilities=CAP_NET_BIND_SERVICE\n".to_string(),
        "CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n".to_string(),
        "NoNewPrivileges=yes".to_string(),
        format!(
            "ExecStart=/opt/{ASVC}/app/cca-core team-acme --teams /opt/{ASVC}/teams --store /opt/{ASVC}/store --out \
             /opt/{ASVC}/webcert --account /opt/{ASVC}/acme --listen 0.0.0.0:80\n"
        ),
    ] {
        assert_in(&want, &acme);
    }
    assert!(!units.join(format!("{svc}-acme.service")).exists(), "不再是每个 Team 自己一个");
    assert!(b.running(ASVC));
    assert_eq!(fsx::read_text(ad.join("teams").join(&svc)), "127.0.0.1\n", "登记了这个 Team 的地址");
    assert!(fsx::read(ad.join("app/cca-core")) == helper_bytes(), "证书助手就是那一份 cca-core（team-acme）：拷过去的");
    assert_eq!(
        (names(&ad.join("app")), fsx::mode(ad.join("app/cca-core")) & 0o777),
        (vec!["cca-core".to_string()], 0o755),
        "systemd 方式：CA 证书它自己在系统里找，不用拷一份"
    );
    assert_not_in("python", &acme);
    let mine = format!("webcert/{svc}");
    assert_eq!(
        modes(&ad, &[".", ".env", "app", "teams", "store", "acme", "webcert", &mine], 0o7777),
        pairs(&[
            (".", "0o755"),
            (".env", "0o600"),
            ("app", "0o755"),
            ("teams", "0o755"),
            ("store", "0o700"),
            ("acme", "0o700"),
            ("webcert", "0o711"),
            (&mine, "0o2750"),
        ])
    );
    assert_eq!(b.envfile(Some(&ad)), map(&[("CCA_HUB_MODE", "systemd"), ("CCA_HUB_VERSION", "9.9.9")]));
    assert!(
        b.log("chown.log").contains(&format!("-R {ASVC}:{} {}/webcert/{svc}\n", gid(&b, &svc), ad.display())),
        "这个 Team 的那一份：证书助手的用户、这个 Team 的组"
    );
    assert_in(&format!("-r -M -d /opt/{ASVC}"), &b.log("useradd.log"));
    assert_in(
        &format!("--teams {ad}/teams --store {ad}/store --out {ad}/webcert --account {ad}/acme", ad = ad.display()),
        &b.log("acme.argv"),
    );
    for f in ["fullchain.pem", "privkey.pem"] {
        assert!(fsx::read(ad.join("webcert").join(&svc).join(f)) == fsx::read(ad.join("store/127.0.0.1").join(f)), "{f}");
    }
    let (st, body) = https(web, "/");
    assert_eq!(st, 200);
    assert!(utf8(body).contains("<title>cca 远程会话（甲组）</title>"), "网页端用证书助手要来的证书给出页面，标题带上 Team 的名字");
    assert_eq!(health(port).get("web"), Some(&Value::from(url.as_str())));
    // 再运行：照旧开着；80 端口上听着的是共用的证书助手，不算被占着；没写 --name 照上次的
    fsx::write(b.d.join("listening"), "80\n");
    fake_ss(&b);
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in(&format!("手机的网页端：{url}"), &r.out);
    assert!(!r.out.contains("安全组"), "端口都没变：不再说要放行");
    assert!(b.running(ASVC));
    assert_eq!(fsx::read_text(d.join("app/name")), "甲组\n");
    // cca team deploy 没写 --host 时放进 ssh -G 的主机名当兜底：记着地址时照记着的（ssh 走的是内网地址也不拿它盖掉、不关网页端）
    let r = fx.install_with(&b, &["--dir", &dir_arg], hubbox::sh(), &[("CCA_HUB_HOST_HINT", "10.0.0.5")]);
    assert_in(&format!("手机的网页端：{url}"), &r.out);
    assert_eq!(pick(&b.envfile(Some(&d)), &["CCA_HUB_HOST", "CCA_HUB_WEB_URL"]), ["127.0.0.1", url.as_str()]);
    // --no-web：去掉登记和这个 Team 的那一份；没有别的 Team 用证书助手了：停掉它、删掉 unit（证书、账户留着，再开时接着用）
    let r = fx.install(&b, &["--dir", &dir_arg, "--no-web"]);
    let e = b.envfile(Some(&d));
    assert_eq!((e["CCA_HUB_WEB"].as_str(), e.contains_key("CCA_HUB_WEB_URL")), ("off", false));
    assert!(!(units.join(format!("{ASVC}.service")).exists() || b.running(ASVC)));
    assert!(!(ad.join("teams").join(&svc).exists() || ad.join("webcert").join(&svc).exists()));
    assert!(ad.join("store/127.0.0.1/fullchain.pem").exists(), "证书留着");
    assert_not_in("--web", &fsx::read_text(units.join(format!("{svc}.service"))));
    assert!(health(port).get("web").is_none());
    assert!(!r.out.contains("手机的网页端"), "自己关的：不说");
    fx.install(&b, &["--dir", &dir_arg]);
    assert_eq!(env_of(&b, &d, "CCA_HUB_WEB"), "off");
    // 再打开，80 端口却被别的程序占着：写了 --web-port 就停下，什么都不动
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--web-port", &ws]);
    assert_eq!(r.code, 1);
    assert_in("80 端口被别的程序占着：证书助手要用它", &r.err);
    assert_eq!(env_of(&b, &d, "CCA_HUB_WEB"), "off");
    // 80 端口腾出来了，证书却没要到（安全组没放行 80；存着的那张也没了）：照样开着，说怎么办
    fsx::write(b.d.join("listening"), "");
    std::fs::remove_dir_all(ad.join("store")).expect("删掉存着的证书");
    b.flag("acme_fail", true);
    let r = fx.install(&b, &["--dir", &dir_arg, "--web-port", &ws]);
    assert_in(&format!("手机的网页端（{url}）的证书还没拿到：证书机构要从公网连这台服务器的 80 端口"), &r.out);
    assert_in(&format!("journalctl -u {ASVC}"), &r.out);
    assert!(b.running(ASVC), "证书助手接着试");
    // 卸载最后一个用证书助手的 Team：连同它（目录、unit、系统用户）一起删掉
    let r = fx.install(&b, &["--uninstall", "--dir", &dir_arg]);
    assert_in(&format!("共用的证书助手（/opt/{ASVC}，连同证书、账户）也删掉了"), &r.out);
    assert!(!(d.exists() || ad.exists() || units.join(format!("{ASVC}.service")).exists() || b.running(ASVC) || b.running(&svc)));
    assert_in(&format!("disable --now {ASVC}"), &b.log("systemctl.log"));
    assert_eq!(names(&b.d.join("users")), Vec::<String>::new());
    clean(&b);
}

/// 一台服务器上两个 Team 都开网页端：第二个在共用的证书助手那里登记（80 端口上是它，不算被占着），同一个地址只要一张证书，
/// 两个 Team 各拿到一份（各是各的组）、各用各的端口和标题；卸载一个，另一个照旧（证书助手还在）；再卸载最后一个，证书助手一起删掉
fn web_two_teams(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &any_ip());
    let (a1, a2) = (fx.admin("a"), fx.admin("b"));
    let ((_tid1, root1, svc1), (_tid2, root2, svc2)) = (new_team(&a1, "甲组"), new_team(&a2, "乙组"));
    let ad = opt(&b, ASVC);
    let ports = apart(2);
    let ((p1, w1), (p2, w2)) = (ports[0], ports[1]);
    fx.install(
        &b,
        &["--team-root", &root1, "--host", "127.0.0.1", "--port", &p1.to_string(), "--web-port", &w1.to_string(), "--name", "甲组"],
    );
    fsx::write(b.d.join("listening"), "80\n");
    fake_ss(&b);
    let r = fx.install(
        &b,
        &["--team-root", &root2, "--host", "127.0.0.1", "--port", &p2.to_string(), "--web-port", &w2.to_string(), "--name", "乙组"],
    );
    assert!(
        r.out.contains(&format!("手机的网页端：https://127.0.0.1:{w2}/（证书是")),
        "80 端口上是共用的证书助手：第二个 Team 照样开得了"
    );
    let mut both = vec![svc1.clone(), svc2.clone()];
    both.sort();
    assert_eq!(names(&ad.join("teams")), both);
    assert_eq!(b.log("acme.issued"), "127.0.0.1\n", "同一个地址只要一张");
    for f in ["fullchain.pem", "privkey.pem"] {
        assert!(fsx::read(ad.join("webcert").join(&svc1).join(f)) == fsx::read(ad.join("webcert").join(&svc2).join(f)), "{f}");
    }
    let (g1, g2) = (gid(&b, &svc1), gid(&b, &svc2));
    assert_ne!(g1, g2);
    for (s, g) in [(&svc1, &g1), (&svc2, &g2)] {
        assert!(
            b.log("chown.log").contains(&format!("-R {ASVC}:{g} {}/webcert/{s}\n", ad.display())),
            "各是各的组：别的 Team 读不了它的私钥"
        );
    }
    for (w, n) in [(w1, "甲组"), (w2, "乙组")] {
        let (st, body) = https(w, "/");
        assert_eq!(st, 200);
        assert_in(&format!("<title>cca 远程会话（{n}）</title>"), &utf8(body));
    }
    fx.install(&b, &["--uninstall", "--dir", &format!("/opt/{svc1}")]);
    assert!(b.running(ASVC) && b.running(&svc2) && ad.join("teams").join(&svc2).exists());
    assert!(!(ad.join("teams").join(&svc1).exists() || ad.join("webcert").join(&svc1).exists()));
    assert_eq!(https(w2, "/").0, 200);
    let r = fx.install(&b, &["--uninstall", "--dir", &format!("/opt/{svc2}")]);
    assert_in("也删掉了", &r.out);
    assert!(!(ad.exists() || b.running(ASVC)));
}

/// 以前的安装脚本装的：每个 Team 自己一个证书助手（<服务名>-acme，占着 80 端口，证书在这个 Team 的 webcert/ 里）。另一个 Team 这时
/// 开不了网页端（说是哪个占着、先更新它；写了 --web-port 就停下）；用这一版更新那个 Team：停掉它自己的，证书、账户搬给共用的证书助手
/// （接着用那张，不再要），再装另一个就开得了
fn web_old_helper(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &any_ip());
    let (a1, a2) = (fx.admin("a"), fx.admin("b"));
    let ((_tid1, root1, svc1), (_tid2, root2, svc2)) = (new_team(&a1, "甲组"), new_team(&a2, "乙组"));
    let (d1, ad) = (opt(&b, &svc1), opt(&b, ASVC));
    let units = b.root.join(UNITS);
    let (dir1, dir2) = (format!("/opt/{svc1}"), format!("/opt/{svc2}"));
    let ports = apart(2);
    let ((p1, w1), (p2, w2)) = (ports[0], ports[1]);
    // 甲组：先照这一版装（网页端关着），再摆成以前的样子——它自己的证书助手的 unit、webcert/、acme/，起着
    fx.install(&b, &["--team-root", &root1, "--host", "127.0.0.1", "--port", &p1.to_string(), "--no-web"]);
    let env = fsx::read_text(d1.join(".env"))
        .replace("CCA_HUB_WEB=off\n", &format!("CCA_HUB_WEB={w1}\nCCA_HUB_WEB_URL=https://127.0.0.1:{w1}/\n"));
    fsx::write(d1.join(".env"), env);
    for x in ["webcert", "acme"] {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(d1.join(x)).unwrap_or_else(|e| panic!("建不了 {x}：{e}"));
    }
    fsx::write(d1.join("acme/account.key"), "以前的账户的私钥\n");
    fsx::write(d1.join("app/cca-acme.py"), "# 以前的证书助手（Python）\n");
    // 以前的 unit 就是这个样子（Python 的）。这台假服务器上起不了它：正在运行的那个证书助手由假的顶替（照 --id 要一张证书）
    fsx::write(
        units.join(format!("{svc1}-acme.service")),
        format!(
            "[Service]\nUser={svc1}\nEnvironmentFile=/opt/{svc1}/.env\nExecStart=/usr/bin/python3 -I /opt/{svc1}/app/cca-acme.py --id \
             ${{CCA_HUB_HOST}} --out /opt/{svc1}/webcert --account /opt/{svc1}/acme --listen 0.0.0.0:80\n"
        ),
    );
    b.stand_in(
        &format!("{svc1}-acme"),
        &[p(&harness::tool()), "acme-stub", "--id", "127.0.0.1", "--out", p(&d1.join("webcert")), "--account", p(&d1.join("acme"))],
        &[],
    );
    wait_for("以前的证书助手要来证书", WAIT, || d1.join("webcert/fullchain.pem").exists());
    let old: Vec<(&str, Vec<u8>)> =
        ["fullchain.pem", "privkey.pem"].into_iter().map(|f| (f, fsx::read(d1.join("webcert").join(f)))).collect();
    fsx::write(b.d.join("listening"), "80\n");
    fake_ss(&b);
    // 乙组：80 端口被甲组以前的证书助手占着
    let r = fx.install(&b, &["--team-root", &root2, "--host", "127.0.0.1", "--port", &p2.to_string()]);
    assert_in(&format!("手机的网页端没开：80 端口被 {svc1}-acme 占着"), &r.out);
    let r = fx.try_install(&b, &["--dir", &dir2, "--web-port", &w2.to_string()]);
    assert_eq!(r.code, 1);
    assert_in(&format!("80 端口被 {svc1}-acme 占着"), &r.err);
    assert!(!ad.exists(), "还没有共用的证书助手");
    // 用这一版更新甲组：它自己的证书助手停掉，证书、账户搬给共用的证书助手，接着用那张
    let r = fx.install(&b, &["--dir", &dir1]);
    assert_in(&format!("手机的网页端：https://127.0.0.1:{w1}/（证书是"), &r.out);
    assert_in(&format!("disable --now {svc1}-acme"), &b.log("systemctl.log"));
    assert!(!(b.running(&format!("{svc1}-acme")) || units.join(format!("{svc1}-acme.service")).exists()));
    assert!(!(d1.join("webcert").exists() || d1.join("acme").exists()), "以前的目录搬空、删掉了");
    assert_eq!(names(&d1.join("app")), ["cca-core"], "以前的证书助手的程序也删掉了");
    assert_eq!(fsx::read_text(ad.join("acme/account.key")), "以前的账户的私钥\n", "账户搬过来了");
    for (f, raw) in &old {
        assert!(fsx::read(ad.join("webcert").join(&svc1).join(f)) == *raw, "{f}：接着用原来的那张");
    }
    assert_eq!(b.log("acme.issued"), "127.0.0.1\n", "没有再要一张");
    assert!(b.running(ASVC));
    // 再装乙组：开得了
    let r = fx.install(&b, &["--dir", &dir2, "--web-port", &w2.to_string()]);
    assert_in(&format!("手机的网页端：https://127.0.0.1:{w2}/（证书是"), &r.out);
    assert!(fsx::read(ad.join("webcert").join(&svc2).join("fullchain.pem")) == old[0].1);
    assert_eq!(b.log("acme.issued"), "127.0.0.1\n");
    clean(&b);
}

/// 网页端这一次开不了：80 端口被别的程序占着、地址是内网的、认不出地址、它的端口被占着——照样装好，说一声，选的端口记着（下次再看）；
/// 不开中转就没有网页端（写了 --web-port 是写错了）
fn web_skipped(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &any_ip());
    let (_a, _tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let dir_arg = format!("/opt/{svc}");
    fsx::write(b.d.join("listening"), "80\n");
    fake_ss(&b);
    let port = team_port();
    let r = fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string()]);
    assert_in("手机的网页端没开：80 端口被别的程序占着", &r.out);
    let e = b.envfile(Some(&d));
    assert!(is_digits(&e["CCA_HUB_WEB"]) && !e.contains_key("CCA_HUB_WEB_URL"));
    assert!(!(b.root.join(UNITS).join(format!("{ASVC}.service")).exists() || opt(&b, ASVC).exists()));
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--web-port", &one_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("80 端口被别的程序占着", &r.err);
    // 80 端口腾出来了：再运行（没写什么）就开上，用记着的端口
    fsx::write(b.d.join("listening"), "");
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in(&format!("手机的网页端：https://127.0.0.1:{}/", e["CCA_HUB_WEB"]), &r.out);
    assert!(
        r.out.contains(&format!("要放行 TCP {port}、{}（远程会话的中转）、{} 和 80", port + 1, e["CCA_HUB_WEB"])),
        "网页端这次才开：说要放行"
    );
    // 网页端的端口被别的程序占着（没写 --web-port）：这一次不开
    b.stop(Some(&svc));
    // 等它真的停了再装（原来发了 TERM 就接着装）：安装脚本看到服务还在运行，就当这个端口是它自己占着的
    wait_for("服务停掉", WAIT, || !b.running(&svc));
    fsx::write(b.d.join("listening"), format!("{}\n", e["CCA_HUB_WEB"]));
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in(&format!("手机的网页端没开：端口 {} 被别的程序占着", e["CCA_HUB_WEB"]), &r.out);
    fsx::write(b.d.join("listening"), "");
    // 不开中转：没有网页端（不说），写了 --web-port 是写错了
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--no-relay", "--web-port", &one_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("不能和 --no-relay 一起", &r.err);
    let r = fx.install(&b, &["--dir", &dir_arg, "--no-relay"]);
    assert_not_in("网页端", &r.out);
    assert_eq!(env_of(&b, &d, "CCA_HUB_RELAY"), "off");
    assert!(!(b.running(ASVC) || opt(&b, ASVC).join("teams").join(&svc).exists()), "没有 Team 用证书助手了：停掉");
    // 认不出地址（以前的安装脚本没记 CCA_HUB_HOST，机器上又看不到公网 IP）：网页端不开，说加 --host
    fx.install(&b, &["--dir", &dir_arg, "--relay-port", &one_port().to_string()]);
    let env = fsx::read_text(d.join(".env"));
    let kept: String =
        py::splitlines(&env).into_iter().filter(|l| !l.starts_with("CCA_HUB_HOST=")).map(|l| format!("{l}\n")).collect();
    fsx::write(d.join(".env"), kept);
    fsx::write(b.d.join("ip"), "10.9.0.5");
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in("手机的网页端没开：认不出这台服务器的公网地址", &r.out);
    // 没记着地址时才用 cca team deploy 放进的兜底（ssh -G 的主机名），记下来
    let r = fx.install_with(&b, &["--dir", &dir_arg], hubbox::sh(), &[("CCA_HUB_HOST_HINT", "127.0.0.1")]);
    assert_eq!(env_of(&b, &d, "CCA_HUB_HOST"), "127.0.0.1");
    assert_in(&format!("手机的网页端：https://127.0.0.1:{}/", env_of(&b, &d, "CCA_HUB_WEB")), &r.out);
    // 内网地址（不是测试里的 any_ip）：不开
    let c = fx.hub(t, &plain());
    let r = fx.install(&c, &["--team-root", &root, "--host", "10.0.0.5", "--port", &team_port().to_string()]);
    assert_in("手机的网页端没开：10.0.0.5 是内网地址", &r.out);
}

/// Docker compose：容器用本机建的空镜像（docker import，不拉镜像），里面运行挂进去的 app/cca-core，用户 10001，项目名是服务名；网页端开着时共用的证书助手也是 Docker 的（自己的
/// compose 项目 cca-team-acme，80 端口），这个 Team 只读挂着它放给自己的那一份；改用 systemd 时这个 Team 的容器停掉，证书助手照旧
/// （Docker 还能用）；建不了镜像时自动改用 systemd，指定了 --docker 就停下
fn docker(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &docker_any_ip());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let units = b.root.join(UNITS);
    let dir_arg = format!("/opt/{svc}");
    let (port, web) = two_ports();
    let r = fx.install_with(
        &b,
        &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string(), "--web-port", &web.to_string()],
        "/bin/sh",
        &[],
    );
    let e = b.envfile(Some(&d));
    assert_eq!((e["CCA_HUB_MODE"].as_str(), e.contains_key("CCA_HUB_IMAGE")), ("docker", false), "镜像是本机建的：不用记");
    assert_in(&format!("✓ Team 的同步服务器在运行（/opt/{svc}，Docker compose，端口 {port}，服务名 {svc}）"), &r.out);
    assert_eq!(b.log("images"), format!("{}\n", hubbox::RUNTIME), "容器用的空镜像建好了");
    let y = fsx::read_text(d.join("compose.yaml"));
    for want in [
        format!("container_name: {svc}\n"),
        format!("    image: {}\n", hubbox::RUNTIME),
        r#"command: ["/app/cca-core", "team-server", "--mode", "team", "--listen", "0.0.0.0:18445", "#.to_string(),
        r#"user: "10001:10001""#.to_string(),
        "read_only: true".to_string(),
        "cap_drop: [ALL]".to_string(),
        "- ./app:/app:ro,z".to_string(),
        "- ./data:/data:z".to_string(),
        r#"- "${CCA_HUB_WEB:?}:18447""#.to_string(),
        format!("- /opt/{ASVC}/webcert/{svc}:/webcert:ro,z\n"),
        "pids_limit: 512".to_string(),
        r#""--web-url", "${CCA_HUB_WEB_URL:?}""#.to_string(),
        r#""--web-key", "/webcert/privkey.pem"]"#.to_string(),
        "CCA_TEAM_ROOT: ${CCA_TEAM_ROOT:?}".to_string(),
    ] {
        assert_in(&want, &y);
    }
    assert!(!y.contains("-acme:"), "证书助手不在这个 Team 的项目里");
    assert!(!y.contains("--web-name-file"), "没写 --name");
    assert!(!y.contains('@'), "记号都换掉了");
    let ad = opt(&b, ASVC);
    let ay = fsx::read_text(ad.join("compose.yaml"));
    for want in [
        format!("container_name: {ASVC}\n"),
        format!("    image: {}\n", hubbox::RUNTIME),
        r#"user: "10001:10001""#.to_string(),
        "read_only: true".to_string(),
        "cap_drop: [ALL]".to_string(),
        r#"- "80:18080""#.to_string(),
        "- ./teams:/teams:ro,z".to_string(),
        "- ./store:/store:z".to_string(),
        "- ./webcert:/webcert:z".to_string(),
        "- ./acme:/acme:z".to_string(),
        r#"command: ["/app/cca-core", "team-acme", "--teams", "/teams", "--store", "/store", "--out", "/webcert", "--account", "/acme", "--listen", "0.0.0.0:18080", "--ca", "/app/ca.pem"]"#
            .to_string(),
    ] {
        assert_in(&want, &ay);
    }
    assert_eq!(b.envfile(Some(&ad)), map(&[("CCA_HUB_MODE", "docker"), ("CCA_HUB_VERSION", "9.9.9")]));
    // 证书助手的容器也是空的：程序是拷过去的同一份，CA 证书是这台服务器的那一份拷的（--ca 指着它）
    assert_eq!(names(&ad.join("app")), ["ca.pem", "cca-core"]);
    assert!(fsx::read(ad.join("app/cca-core")) == helper_bytes());
    assert!(fsx::read(ad.join("app/ca.pem")) == fsx::read(b.root.join(hubbox::CA_BUNDLE)));
    assert_eq!(modes(&ad, &["app/cca-core", "app/ca.pem"], 0o777), pairs(&[("app/cca-core", "0o755"), ("app/ca.pem", "0o644")]));
    let chown = b.log("chown.log");
    for want in [
        format!("-R 10001:10001 {d}/certs {d}/data\n", d = d.display()),
        format!("-R 10001 {ad}/store {ad}/acme {ad}/webcert\n", ad = ad.display()),
        format!("-R 10001:10001 {}/webcert/{svc}\n", ad.display()),
    ] {
        assert_in(&want, &chown);
    }
    let log = b.log("docker.log");
    assert_in(&format!("compose -p {svc} -f compose.yaml up -d --force-recreate --remove-orphans"), &log);
    assert_in(&format!("compose -p {ASVC} -f compose.yaml up -d --force-recreate --remove-orphans"), &log);
    assert_eq!(
        log.lines().filter(|l| l.starts_with("import")).collect::<Vec<_>>(),
        [format!("import - {}", hubbox::RUNTIME)],
        "空镜像只建一次（证书助手用的是同一个）"
    );
    assert!(!(units.join(format!("{svc}.service")).exists() || units.join(format!("{ASVC}.service")).exists()));
    assert_eq!(b.log("useradd.log"), "");
    assert!(b.running(&svc) && b.running(ASVC));
    assert_eq!(https(web, "/").0, 200);
    assert_in(&format!("手机的网页端：https://127.0.0.1:{web}/（证书是 Let's Encrypt 签的"), &r.out);
    assert!(fsx::read_text(&fx.script).contains("docker logs"), "证书没要到时说看哪个容器的日志");
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    // 改用 systemd（--systemd）：这个 Team 的容器停掉，数据照旧；共用的证书助手照旧是 Docker 的（还能用），放给它的那一份换成它的组
    let r = fx.install(&b, &["--dir", &dir_arg, "--systemd"]);
    assert_in(&format!("compose -p {svc} -f compose.yaml down"), &b.log("docker.log"));
    assert!(!d.join("compose.yaml").exists());
    assert_eq!(env_of(&b, &d, "CCA_HUB_MODE"), "systemd");
    assert!(ad.join("compose.yaml").exists() && b.running(ASVC) && !units.join(format!("{ASVC}.service")).exists());
    assert_in(&format!("-R 10001:{} {}/webcert/{svc}\n", gid(&b, &svc), ad.display()), &b.log("chown.log"));
    assert_in(&format!("手机的网页端：https://127.0.0.1:{web}/（证书是"), &r.out);
    assert_eq!(sync_state(&a), "ok", "换了运行方式，照旧连得上");
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert!(r.out.contains("，systemd，端口 "), "没指定：照上次的");
    clean(&b);
    // 建不了镜像（docker import 没成）
    let c = fx.hub(t, &with_docker());
    c.flag("import_fail", true);
    let r = fx.install(&c, &["--team-root", &root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_in("Docker 建不了 cca 的运行环境（docker import 没成）：改用 systemd", &r.out);
    assert_eq!(env_of(&c, &opt(&c, &svc), "CCA_HUB_MODE"), "systemd");
    assert!(c.running(&svc));
    let f = fx.hub(t, &with_docker());
    f.flag("import_fail", true);
    let r = fx.try_install(&f, &["--team-root", &root, "--docker", "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("✗ Docker 建不了 cca 的运行环境（docker import 没成）：看看 Docker，或者换 --systemd", &r.err);
    assert!(!opt(&f, &svc).exists(), "没建成镜像：什么都还没动");
    clean(&c);
    clean(&f);
}

/// 只用 compose 插件：只有老的 docker-compose 1.x 时改用 systemd（--docker 时停下）；compose up 起不来时打出它说的、
/// 改用 systemd（--docker 时停下，第一次装的 .env 不留）；Docker 的卸载
fn compose(t: &mut Ctx) {
    let fx = setup(t);
    let (_a, _tid, root, svc) = fx.team("甲组");
    let dir_arg = format!("/opt/{svc}");
    let b = fx.hub(t, &with_docker());
    b.flag("no_compose", true);
    fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(env_of(&b, &opt(&b, &svc), "CCA_HUB_MODE"), "systemd");
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--docker"]);
    assert_eq!(r.code, 1);
    assert_in("带 compose 插件", &r.err);
    assert_eq!(b.log("docker-compose.log"), "", "不用老的 docker-compose");

    let c = fx.hub(t, &with_docker());
    c.flag("compose_fail", true);
    let d = opt(&c, &svc);
    let r = fx.try_install(&c, &["--team-root", &root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.err.contains("    Error response from daemon"), "compose 说的打出来（缩进）");
    assert_in("Docker compose 起不来（上面是它说的）：改用 systemd", &r.out);
    assert_eq!(env_of(&c, &d, "CCA_HUB_MODE"), "systemd");
    assert!(!c.envfile(Some(&d)).contains_key("CCA_HUB_IMAGE"));
    assert!(!d.join("compose.yaml").exists());
    assert_in(&format!("compose -p {svc} -f compose.yaml down"), &c.log("docker.log"));
    assert!(c.running(&svc));

    let e = fx.hub(t, &with_docker());
    e.flag("compose_fail", true);
    let d = opt(&e, &svc);
    let r = fx.try_install(&e, &["--team-root", &root, "--docker", "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("Error response from daemon", &r.err);
    assert_in("docker compose up 没成", &r.err);
    assert!(!d.join(".env").exists(), "第一次装没成：.env 不留（下一次还是第一次装）");
    e.flag("compose_fail", false);
    fx.install(&e, &["--team-root", &root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(env_of(&e, &d, "CCA_HUB_MODE"), "docker");
    fx.install(&e, &["--uninstall", "--dir", &dir_arg]);
    assert!(!(d.exists() || e.running(&svc)));
}

/// --dir：别人的目录（里面有东西）不碰，卸载也不碰；装在自己的目录（末尾的 / 去掉）；同一个 Team 一台服务器只装一个
fn dir(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, root, svc) = fx.team("甲组");
    let precious = b.root.join("srv/data/precious");
    std::fs::create_dir_all(b.root.join("srv/data")).expect("建 /srv/data");
    fsx::write(&precious, "别人的数据");
    let r = fx.try_install(&b, &["--team-root", &root, "--dir", "/srv", "--host", "127.0.0.1"]);
    assert_eq!(r.code, 1);
    assert_in("/srv 里已经有别的东西", &r.err);
    let r = fx.try_install(&b, &["--uninstall", "--dir", "/srv"]);
    assert_eq!(r.code, 1);
    assert_in("/srv 里没有装过 Team 的同步服务器", &r.err);
    assert_eq!((fsx::read_text(&precious).as_str(), b.log("chown.log").as_str()), ("别人的数据", ""));
    // 共用的证书助手的目录：装、卸载都不认（里面有 .env，不拦的话当成以前装的 Team：装会盖掉它的程序和 .env，卸载会删掉它的账户）
    let acme_env = opt(&b, ASVC).join(".env");
    std::fs::create_dir_all(opt(&b, ASVC)).expect("建证书助手的目录");
    fsx::write(&acme_env, "CCA_HUB_MODE=systemd\n");
    for args in [
        vec!["--team-root", root.as_str(), "--dir", "/opt/cca-team-acme", "--host", "127.0.0.1"],
        vec!["--uninstall", "--dir", "/opt/cca-team-acme/"],
    ] {
        let r = fx.try_install(&b, &args);
        assert_eq!(r.code, 1, "{args:?}");
        assert_in("共用的证书助手装在那里", &r.err);
    }
    assert_eq!((fsx::read_text(&acme_env).as_str(), names(&opt(&b, ASVC))), ("CCA_HUB_MODE=systemd\n", vec![".env".to_string()]));
    std::fs::remove_dir_all(opt(&b, ASVC)).expect("收掉证书助手的目录");
    let port = team_port();
    fx.install(&b, &["--team-root", &root, "--dir", "/srv/team/", "--host", "127.0.0.1", "--port", &port.to_string()]);
    let hub = b.root.join("srv/team");
    assert!(hub.join(".cca-hub").is_file());
    assert_in("EnvironmentFile=/srv/team/.env\n", &fsx::read_text(b.root.join(UNITS).join(format!("{svc}.service"))));
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&hub)]);
    // 默认的 /opt/<服务名>：这个 Team 已经装在 /srv/team 了
    let r = fx.try_install(&b, &["--team-root", &root, "--host", "127.0.0.1"]);
    assert_eq!(r.code, 1);
    assert_in("已经在 /srv/team 装了这个 Team 的同步服务器", &r.err);
    assert!(!opt(&b, &svc).exists());
    let r = fx.install(&b, &["--dir", "/srv/team"]);
    assert_in("更新了服务端程序", &r.out);
    assert_eq!(sync_state(&a), "ok");
    fx.install(&b, &["--uninstall", "--dir", "/srv/team"]);
    assert!(!hub.exists());
    assert_eq!(fsx::read_text(&precious), "别人的数据", "旁边的不动");
}

/// 更新没成：起不来、被打断——换回原来的（.env、unit、服务端程序：装着的那一份先换成转给辅助程序的一个 sh，当作以前的版本，看得出
/// 换回来的是不是它），原来的服务又起来了，管理员不用再做什么、照样连得上，不留那几份备份；再运行照旧是更新
fn failed_update_rolls_back(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let unit = b.root.join(UNITS).join(format!("{svc}.service"));
    let dir_arg = format!("/opt/{svc}");
    let port = team_port();
    fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string()]);
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    // 以前的版本：换的是目录里的文件（换名字换上），正在运行的进程照旧
    let old = d.join("app/.old");
    fsx::write_script(&old, &format!("#!/bin/sh\nexec '{}' \"$@\"\n", harness::helper().display()));
    std::fs::rename(&old, d.join("app/cca-core")).expect("换上以前的版本");
    let state = || (fsx::read_text(d.join(".env")), fsx::read(d.join("app/cca-core")), fsx::read_text(&unit));
    let before = state();
    let rolled_back = |what: &str, pid: &str| {
        assert!(state() == before, "{what}：.env、装着的程序、unit 都换回了原来的");
        assert_eq!(names(&d), [".cca-hub", ".env", "app", "certs", "data"], "{what}：不留那几份备份");
        assert_eq!(names(&d.join("app")), ["cca-core"], "{what}");
        assert!(b.running(&svc) && pid_of(&b, &svc) != pid, "{what}：原来的服务重新起来了");
        wait_for(what, WAIT, || sync_state(&a) == "ok");
    };
    // 起不来（这一次的起不来；换回原来的以后起得来：只这一次）
    let pid = pid_of(&b, &svc);
    b.flag_once("start_fail");
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--port", &team_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("systemd 起不来", &r.err);
    assert_in("换回了原来的（服务端程序、设置），原来的服务又起来了", &r.out);
    rolled_back("起不来", &pid);
    // 被打断（Ctrl-C、SSH 断了）：在起服务的时候
    let pid = pid_of(&b, &svc);
    let restart = format!("restart {svc}");
    let restarts = || b.log("systemctl.log").split('\n').filter(|l| *l == restart).count();
    let n = restarts();
    b.flag_once("start_hang");
    // 安装脚本在自己的会话里（start_new_session）：TERM 发给它那一组，卡着的假 systemctl 一起收到。中途 panic 了（没等到）
    // 也不会留下：Child 离开作用域时整组停掉
    let mut p = Cmd::new(hubbox::sh())
        .arg(&fx.script)
        .args(["--dir", &dir_arg, "--port", &team_port().to_string()])
        .envs(b.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::Null)
        .stdout(Stdio::Piped)
        .stderr(Stdio::Piped)
        .session(Session::New)
        .spawn()
        .unwrap_or_else(|e| panic!("安装脚本起不来：{e}"));
    // 假的 systemctl 记下 restart、拿掉只管一次的 start_hang 以后就卡着
    wait_for("起服务", WAIT, || restarts() > n && !b.d.join("start_hang").exists());
    assert_ne!(fsx::read_text(d.join(".env")), before.0);
    p.signal_group(libc::SIGTERM).expect("给安装脚本那一组发 TERM");
    let exit = p
        .wait_timeout(Duration::from_secs(30))
        .expect("等安装脚本退出")
        .unwrap_or_else(|| panic!("发了 TERM，30 秒了安装脚本还没退出"));
    assert_eq!(exit.py(), 1);
    rolled_back("被打断", &pid);
    // 再运行：照旧是更新，这一次的换上了
    let r = fx.install(&b, &["--dir", &dir_arg]);
    assert_in("更新了服务端程序", &r.out);
    assert_eq!(env_of(&b, &d, "CCA_HUB_PORT"), port.to_string());
    assert!(fsx::read(d.join("app/cca-core")) == helper_bytes());
    assert_eq!(names(&d), [".cca-hub", ".env", "app", "certs", "data"]);
    wait_for("更新以后照样连得上", WAIT, || sync_state(&a) == "ok");
    clean(&b);
}

/// 换运行方式没成：systemd → Docker（compose up 起不来）、Docker → systemd（起不来）——这时原来的已经停了。停掉这一次起的，
/// .env、unit / compose.yaml 换回原来的（原来没有的删掉），certs/、data/ 照原来的方式 chown 回去，原来的服务照原来的方式又起来了，
/// 管理员照样连得上
fn switch_rolls_back(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &with_docker());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let unit = b.root.join(UNITS).join(format!("{svc}.service"));
    let dir_arg = format!("/opt/{svc}");
    let port = team_port();
    fx.install(&b, &["--team-root", &root, "--systemd", "--host", "127.0.0.1", "--port", &port.to_string()]);
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    let since = |log: &str, mark: usize| b.log(log)[mark..].to_string();
    let marks = || (b.log("systemctl.log").len(), b.log("docker.log").len(), b.log("chown.log").len());
    // systemd → Docker：compose up 起不来
    let before = (fsx::read_text(d.join(".env")), fsx::read_text(&unit));
    let (sm, dm, cm) = marks();
    b.flag_once("compose_fail");
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--docker"]);
    assert_eq!(r.code, 1);
    assert_in("docker compose up 没成", &r.err);
    assert_in("换回了原来的（服务端程序、设置），原来的服务又起来了", &r.out);
    assert!((fsx::read_text(d.join(".env")), fsx::read_text(&unit)) == before, ".env、unit 换回了原来的");
    assert!(!d.join("compose.yaml").exists() && b.running(&svc), "这一次的 compose.yaml 删了，原来的 systemd 服务又起来了");
    assert_in(&format!("compose -p {svc} -f compose.yaml down"), &since("docker.log", dm));
    assert_in(&format!("enable {svc}\nrestart {svc}\n"), &since("systemctl.log", sm));
    assert!(
        since("chown.log", cm).ends_with(&format!("-R {svc} {d}/certs {d}/data\n", d = d.display())),
        "certs/、data/ 换回 systemd 的用户"
    );
    wait_for("换回 systemd 以后照样连得上", WAIT, || sync_state(&a) == "ok");
    // 换成 Docker（这次成了），再 Docker → systemd：systemd 起不来
    fx.install(&b, &["--dir", &dir_arg, "--docker"]);
    wait_for("换成 Docker 以后照样连得上", WAIT, || sync_state(&a) == "ok");
    let before = (fsx::read_text(d.join(".env")), fsx::read_text(d.join("compose.yaml")));
    let (sm, dm, cm) = marks();
    b.flag_once("start_fail");
    let r = fx.try_install(&b, &["--dir", &dir_arg, "--systemd"]);
    assert_eq!(r.code, 1);
    assert_in("systemd 起不来", &r.err);
    assert_in("换回了原来的（服务端程序、设置），原来的服务又起来了", &r.out);
    assert!((fsx::read_text(d.join(".env")), fsx::read_text(d.join("compose.yaml"))) == before, ".env、compose.yaml 换回了原来的");
    assert!(!unit.exists() && b.running(&svc), "这一次的 unit 停了、删了，原来的容器又起来了");
    assert_in(&format!("disable --now {svc}"), &since("systemctl.log", sm));
    assert_in(&format!("compose -p {svc} -f compose.yaml up -d --force-recreate --remove-orphans"), &since("docker.log", dm));
    assert!(
        since("chown.log", cm).ends_with(&format!("-R 10001:10001 {d}/certs {d}/data\n", d = d.display())),
        "certs/、data/ 换回容器的用户"
    );
    wait_for("换回 Docker 以后照样连得上", WAIT, || sync_state(&a) == "ok");
    clean(&b);
}

/// 缺东西时停下、说怎么办，什么都不动：没有 Docker 也没有 systemd、端口被占、认不出公网地址；服务器上没有 Python 不碍事
/// （systemd 方式也不找它）；机器上看得到的公网 IP 照用（网页端也开）
fn needs(t: &mut Ctx) {
    let fx = setup(t);
    let (_a, _tid, root, svc) = fx.team("甲组");
    let b = fx.hub(t, &Opts { systemd: false, ..Opts::default() });
    let r = fx.try_install(&b, &["--team-root", &root]);
    assert_eq!(r.code, 1);
    assert_in("两样都没有", &r.err);
    let r = fx.try_install(&b, &["--team-root", &root, "--systemd"]);
    assert_eq!((r.code, r.err.trim()), (1, "✗ 这台服务器上没有 systemd：换 --docker"));
    // 服务器上没有 Python（假服务器上的 python3 一调就失败、记一笔）：systemd 方式照样装得上，没人去找它
    let c = fx.hub(t, &plain());
    let r = fx.install(&c, &["--team-root", &root, "--systemd", "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_in("，systemd，端口 ", &r.out);
    assert!(c.running(&svc));
    clean(&c);
    let d = fx.hub(t, &plain());
    let port = one_port();
    fsx::write(d.d.join("listening"), format!("{port}\n"));
    fake_ss(&d);
    let r = fx.try_install(&d, &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string()]);
    assert_in(&format!("端口 {port} 被别的程序占着"), &r.err);
    // 公网地址：机器上看得到的才认；内网的（云服务器的公网 IP 常常是 NAT 进来的）又没有终端可问时要 --host
    fsx::write(d.d.join("ip"), "10.9.0.5");
    let r = fx.try_install(&d, &["--team-root", &root, "--port", &one_port().to_string()]);
    assert_in("加上 --host", &r.err);
    for x in [&b, &d] {
        assert!(!opt(x, &svc).exists());
    }
    fsx::write(d.d.join("ip"), "203.0.113.7");
    // 这一次真的装：网页端也开（公网地址），用的是接着选的 P + 2
    let p = team_port();
    let r = fx.install(&d, &["--team-root", &root, "--port", &p.to_string()]);
    assert_in(&format!("cca team attach Team https://203.0.113.7:{p} "), &r.out);
    assert!(r.out.contains("手机的网页端：https://203.0.113.7:"), "公网地址：网页端开上");
    assert_eq!(env_of(&d, &opt(&d, &svc), "CCA_HUB_HOST"), "203.0.113.7");
}

/// 3.5.3 以前的自己的同步服务器（/opt/cca-hub，服务名 cca-hub）：只能卸载；装 Team 的不往那个目录里装
fn old_own_server(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (_a, _tid, root, _svc) = fx.team("甲组");
    let old = b.dir.clone();
    std::fs::create_dir_all(old.join("data")).expect("建 /opt/cca-hub/data");
    fsx::write(old.join("data/state.json"), "{}");
    // 这里的 18445、下面 unit 里的那一行都只是以前装下的文件的内容：没有程序去绑这个端口、去运行它
    fsx::write(old.join(".env"), format!("CCA_HUB_MODE=systemd\nCCA_HUB_PORT=18445\nCCA_HUB_ADMIT_HASH={}\n", "0".repeat(64)));
    fsx::write(old.join(".cca-hub"), "# cca 的同步服务器（自己的同步服务器）装在这个目录\n# svc=cca-hub\n");
    let unit = b.root.join(UNITS).join("cca-hub.service");
    fsx::write(
        &unit,
        "[Service]\nUser=cca-hub\nEnvironmentFile=/opt/cca-hub/.env\nExecStart=/usr/bin/python3 -I /opt/cca-hub/app/cca-presence.py\n",
    );
    fsx::write(b.d.join("users/cca-hub"), "");
    let cases: [(&[&str], &str); 3] = [
        (&["--dir", "/opt/cca-hub"], "以前的自己的同步服务器（cca hub，3.5.3 起没有了"),
        (&["--team-root", &root, "--dir", "/opt/cca-hub"], "装着的是以前的自己的同步服务器（cca hub）"),
        (&["--uninstall", "--team-root", &root, "--dir", "/opt/cca-hub"], "不是要卸载的这一个"),
    ];
    for (args, want) in cases {
        let r = fx.try_install(&b, args);
        assert_eq!(r.code, 1, "{args:?}");
        assert!(r.err.contains(want), "{args:?}：没有 {want:?}：{}", r.err);
    }
    assert!(old.join("data/state.json").exists() && unit.exists());
    let r = fx.install(&b, &["--uninstall", "--dir", "/opt/cca-hub"]);
    assert_in("卸载了以前的自己的同步服务器", &r.out);
    assert!(!(old.exists() || unit.exists() || b.d.join("users/cca-hub").exists()));
    assert_in("disable --now cca-hub", &b.log("systemctl.log"));
}

/// 两个 Team 各一个服务（服务名、系统用户、目录、端口都是自己的），互不相干；没写端口时从 18445 起找空着的；再运行（只写目录）是
/// 更新；中转的端口可以换、可以关；装着另一个 Team 的目录不碰；卸载一个，另一个照旧
fn teams(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, c) = (fx.admin("a"), fx.admin("c"));
    let ((tid1, root1, svc1), (tid2, root2, svc2)) = (new_team(&a, "甲组"), new_team(&c, "乙组"));
    let (d1, d2) = (opt(&b, &svc1), opt(&b, &svc2));
    let units = b.root.join(UNITS);
    let dir1 = format!("/opt/{svc1}");
    let p1 = team_port();
    let p1s = p1.to_string();
    let r = fx.install(&b, &["--team-root", &root1, "--host", "127.0.0.1", "--port", &p1s]);
    assert!(r.out.contains(&format!("cca team attach Team https://127.0.0.1:{p1} {}", pin(&d1))), "打出管理员接上它的那一行");
    assert_eq!(health(p1).get("relay"), Some(&obj! { "port" => p1 + 1, "guests" => true }));
    assert!(b.running(&svc1) && b.d.join("users").join(&svc1).exists() && !b.running("cca-hub"), "服务名、系统用户是这个 Team 的");
    assert_eq!(
        a.ok(&["team-attach", &tid1, &format!("https://127.0.0.1:{p1}"), &pin(&d1)]).split('\n').next(),
        Some(format!("127.0.0.1:{p1}").as_str())
    );
    // 第二个 Team：只写根公钥后面那一段也行；没写端口，从 18445 起找一个空着的（前面 k 个由假的 ss 说有程序占着，见 default_slot）
    let k = default_slot();
    fsx::write(b.d.join("listening"), (0..k).map(|i| format!("{}\n", DEF_PORT + i)).collect::<String>());
    fake_ss(&b);
    fx.install(&b, &["--team-root", key_of(&root2), "--host", "127.0.0.1"]);
    assert_eq!(env_of(&b, &d2, "CCA_HUB_PORT"), (18445 + k).to_string(), "前面 {k} 个端口有程序占着");
    assert_eq!(env_of(&b, &d2, "CCA_HUB_RELAY"), (18446 + k).to_string(), "各用各的数据端口");
    assert!(b.running(&svc1) && b.running(&svc2), "两个都在运行");
    let at2 = format!("127.0.0.1:{}", 18445 + k);
    assert_eq!(c.ok(&["team-attach", &tid2, &format!("https://{at2}"), &pin(&d2)]).split('\n').next(), Some(at2.as_str()));
    assert_eq!(
        code(&c, &["team-attach", &tid2, &format!("https://127.0.0.1:{p1}"), &pin(&d1)], &[]),
        "denied",
        "各是各的：甲组的服务器不认乙组的根密钥"
    );
    // 再运行，只写目录：照这个目录里装的那个 Team 更新（端口、证书、数据都不变）
    let before = pin(&d1);
    let r = fx.install(&b, &["--dir", &dir1]);
    assert_in("更新了服务端程序", &r.out);
    assert_eq!((pin(&d1), env_of(&b, &d1, "CCA_HUB_PORT"), b.running(&svc1)), (before, p1s.clone(), true));
    assert_eq!(sync_state(&a), "ok", "管理员照旧连得上");
    assert_eq!(
        (env_of(&b, &d1, "CCA_HUB_RELAY"), health(p1).get("relay").cloned()),
        ((p1 + 1).to_string(), Some(obj! { "port" => p1 + 1, "guests" => true })),
        "中转照上次选的"
    );
    // 不开中转、换一个数据端口：记在 .env 里；写错的不动
    fx.install(&b, &["--dir", &dir1, "--no-relay"]);
    let unit = fsx::read_text(units.join(format!("{svc1}.service")));
    assert_eq!(
        (env_of(&b, &d1, "CCA_HUB_RELAY").as_str(), unit.contains("--relay"), health(p1).get("relay").is_some()),
        ("off", false, false)
    );
    fx.install(&b, &["--dir", &dir1]);
    assert_eq!(env_of(&b, &d1, "CCA_HUB_RELAY"), "off", "关了的再运行还是关着");
    for bad in [p1s.as_str(), "80", "x"] {
        let r = fx.try_install(&b, &["--dir", &dir1, "--relay-port", bad]);
        assert_ne!(r.code, 0, "{bad}");
    }
    assert_eq!(env_of(&b, &d1, "CCA_HUB_RELAY"), "off");
    let rp = one_port();
    fx.install(&b, &["--dir", &dir1, "--relay-port", &rp.to_string()]);
    assert_eq!(
        (env_of(&b, &d1, "CCA_HUB_RELAY"), health(p1).get("relay").cloned()),
        (rp.to_string(), Some(obj! { "port" => rp, "guests" => true }))
    );
    // 装着另一个 Team 的目录不碰
    let r = fx.try_install(&b, &["--team-root", &root2, "--dir", &dir1]);
    assert_ne!(r.code, 0);
    assert_in("另一个 Team 的同步服务器", &format!("{}{}", r.out, r.err));
    assert_eq!(env_of(&b, &d1, "CCA_TEAM_ROOT"), key_of(&root1));
    // 卸载时写错了是哪个（根公钥是乙组的、目录是甲组的）：不动（不然停的是乙组的服务，删的却是甲组的数据）
    let r = fx.try_install(&b, &["--uninstall", "--team-root", &root2, "--dir", &dir1]);
    assert_ne!(r.code, 0);
    assert_in("不是要卸载的这一个", &format!("{}{}", r.out, r.err));
    assert!(b.running(&svc1) && b.running(&svc2) && d1.join("data").exists());
    // .cca-hub 里记着服务名：.env 不见了也认得出是哪个 Team 的——不带根公钥再装不行，卸载照它；写了另一个 Team 的根公钥不碰
    assert_in(&format!("# svc={svc1}\n"), &fsx::read_text(d1.join(".cca-hub")));
    let keep = fsx::read_text(d1.join(".env"));
    std::fs::remove_file(d1.join(".env")).expect("删掉 .env");
    let r = fx.try_install(&b, &["--dir", &dir1]);
    assert_ne!(r.code, 0);
    assert_in("带上 --team-root", &format!("{}{}", r.out, r.err));
    let r = fx.try_install(&b, &["--team-root", &root2, "--dir", &dir1]);
    assert_in("另一个 Team 的同步服务器", &format!("{}{}", r.out, r.err));
    fsx::write(d1.join(".env"), &keep);
    // 没有 ss、netstat 的服务器上自动挑端口：别的 Team 记着用的端口也跳过
    b.fake("ss", "#!/bin/sh\nexit 127\n");
    let e = fx.admin("e");
    let (_tid3, root3, svc3) = new_team(&e, "丙组");
    let used: BTreeSet<String> = [env_of(&b, &d1, "CCA_HUB_PORT"), env_of(&b, &d2, "CCA_HUB_PORT")]
        .into_iter()
        .chain((0..=k).map(|i| (18445 + i).to_string()))
        .collect();
    let other = opt(&b, "cca-team-00000000");
    std::fs::create_dir(&other).expect("建 /opt/cca-team-00000000");
    fsx::write(other.join(".env"), (0..=k).map(|i| format!("CCA_HUB_PORT={}\n", 18445 + i)).collect::<String>());
    // 丙组该落在 18445 + k + 3 上（k + 1、k + 2 是乙组记着的中转和网页端），中转是它的下一个。安装脚本这时看不到谁在监听：真有
    // 别的程序在这十来秒里占了这两个，服务端会起不来——先看一眼，说清楚是这么回事（这一段是这个用例占着的，不该有）
    assert!(port_free(DEF_PORT + k + 3) && port_free(DEF_PORT + k + 4), "{} 或者它的下一个被别的程序占了", DEF_PORT + k + 3);
    fx.install(&b, &["--team-root", &root3, "--host", "127.0.0.1"]);
    let p3 = env_of(&b, &opt(&b, &svc3), "CCA_HUB_PORT");
    assert!(!used.contains(&p3), "{p3} 是别的 Team 记着用的（{used:?}）");
    assert_eq!(p3, (18445 + k + 3).to_string(), "记着用的都跳过，落在后面头一个没人记着的上");
    fx.install(&b, &["--uninstall", "--dir", &format!("/opt/{svc3}")]);
    // 卸载第一个：它的服务、目录、系统用户都没了，第二个照旧
    let r = fx.install(&b, &["--uninstall", "--dir", &dir1]);
    assert_in("卸载了 Team 的同步服务器", &r.out);
    assert!(
        !(b.running(&svc1)
            || d1.exists()
            || b.d.join("users").join(&svc1).exists()
            || units.join(format!("{svc1}.service")).exists())
    );
    assert!(b.running(&svc2) && b.d.join("users").join(&svc2).exists() && d2.exists());
    assert_eq!(sync_state(&c), "ok");
}

/// 这个服务的进程号（假服务器记在进程号文件里的）
fn pid_of(b: &FakeBox, svc: &str) -> String {
    fsx::read_text(b.pidfile(svc)).trim().to_string()
}

/// 把这一版装好的摆成 3.6.x 装的样子（那时候的服务端是 Python 的）：app/ 里是 cca-presence.py 和网页端的文件，没有 cca-core；
/// unit、compose.yaml 里运行的是 python3；Docker 的 .env 记着 python 镜像，本机没有那个空镜像；共用的证书助手也是 Python 的。
/// 正在运行的进程不动——它们就当是以前的那一份（假服务器上起不了 Python 的）
fn make_python_era(b: &FakeBox, svc: &str, docker: bool) {
    let (d, ad) = (opt(b, svc), opt(b, ASVC));
    let units = b.root.join(UNITS);
    let edit = |path: PathBuf, pairs: &[(&str, &str)]| {
        let mut text = fsx::read_text(&path);
        for (new, old) in pairs {
            assert!(text.contains(new), "{} 里没有 {new:?}：\n{text}", path.display());
            text = text.replace(new, old);
        }
        fsx::write(&path, text);
    };
    let image = "${CCA_HUB_IMAGE:-python:3.13-slim}";
    for (dir, program) in [(&d, "cca-presence.py"), (&ad, "cca-acme.py")] {
        for f in names(&dir.join("app")) {
            if f != "name" {
                std::fs::remove_file(dir.join("app").join(&f)).unwrap_or_else(|e| panic!("删不掉 app/{f}：{e}"));
            }
        }
        fsx::write(dir.join("app").join(program), "# 3.6.9 的程序（Python）\n");
    }
    std::fs::create_dir(d.join("app/web")).expect("建 app/web");
    fsx::write(d.join("app/web/index.html"), "<title>以前的页面</title>\n");
    let now = "CCA_HUB_VERSION=9.9.9\n";
    if docker {
        let then = format!("CCA_HUB_IMAGE={OLD_IMAGE}\nCCA_HUB_VERSION=3.6.9\n");
        edit(d.join(".env"), &[(now, &then)]);
        edit(ad.join(".env"), &[(now, &then)]);
        let run = format!("image: {}", hubbox::RUNTIME);
        let old_run = format!("image: {image}");
        edit(
            d.join("compose.yaml"),
            &[(&run, &old_run), (r#"["/app/cca-core", "team-server", "#, r#"["python3", "-I", "/app/cca-presence.py", "#)],
        );
        edit(
            ad.join("compose.yaml"),
            &[
                (&run, &old_run),
                (r#"["/app/cca-core", "team-acme", "#, r#"["python3", "-I", "/app/cca-acme.py", "#),
                (r#", "--ca", "/app/ca.pem"]"#, "]"),
            ],
        );
        std::fs::remove_file(b.d.join("images")).expect("本机没有那个空镜像");
    } else {
        edit(d.join(".env"), &[(now, "CCA_HUB_VERSION=3.6.9\n")]);
        edit(ad.join(".env"), &[(now, "CCA_HUB_VERSION=3.6.9\n")]);
        edit(
            units.join(format!("{svc}.service")),
            &[(
                &format!("ExecStart=/opt/{svc}/app/cca-core team-server "),
                &format!("ExecStart=/usr/bin/python3 -I /opt/{svc}/app/cca-presence.py "),
            )],
        );
        edit(
            units.join(format!("{ASVC}.service")),
            &[(
                &format!("ExecStart=/opt/{ASVC}/app/cca-core team-acme "),
                &format!("ExecStart=/usr/bin/python3 -I /opt/{ASVC}/app/cca-acme.py "),
            )],
        );
    }
}

/// 以前的版本（3.6.x）装的是 Python 的服务端：用这一版再运行，就地换成 cca-core——证书（指纹）、端口、数据、登记的设备都不变，
/// 管理员和成员不用重新接；以前的程序、网页端的文件、记着的镜像都清掉；共用的证书助手也换成它（证书接着用，不再要一张）。
/// 两种运行方式各走一遍
fn from_python(t: &mut Ctx) {
    t.sub("systemd", |t| python_era(t, false));
    t.sub("docker", |t| python_era(t, true));
    t.sub("rollback-systemd", |t| python_era_rollback(t, false));
    t.sub("rollback-docker", |t| python_era_rollback(t, true));
}

fn python_era(t: &mut Ctx, docker: bool) {
    let fx = setup(t);
    let b = fx.hub(t, &if docker { docker_any_ip() } else { any_ip() });
    let (a, tid, root, svc) = fx.team("甲组");
    let (d, ad) = (opt(&b, &svc), opt(&b, ASVC));
    let units = b.root.join(UNITS);
    let (port, web) = two_ports();
    // 照这一版装好、接上、请进一台设备（数据里有东西了），再摆成以前的样子
    fx.install(
        &b,
        &[
            "--team-root",
            &root,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--web-port",
            &web.to_string(),
            "--name",
            "甲组",
        ],
    );
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    let invite = a.ok(&["team-invite", &tid, "bob-mini"]);
    let bob = Device::new(&fx.dir.join(format!("bob-{}", crypto::token_hex(4))), "bob", &obj! {});
    bob.ok_with(&["team-join", "--name", "bob-mini"], invite.split('\n').next().unwrap_or_default(), &[]);
    assert_eq!(sync_state(&bob), "ok");
    fsx::write(d.join("data/keep.bin"), crypto::random(64));
    fsx::write(ad.join("acme/account.key"), "证书机构那里的账户的私钥\n"); // 假的证书助手不建账户：放一个，看它留不留着
    make_python_era(&b, &svc, docker);
    let kept = ["CCA_HUB_MODE", "CCA_HUB_PORT", "CCA_TEAM_ROOT", "CCA_HUB_RELAY", "CCA_HUB_WEB", "CCA_HUB_HOST", "CCA_HUB_WEB_URL"];
    let before = (
        pin(&d),
        fsx::read(d.join("certs/key.pem")),
        pick(&b.envfile(Some(&d)), &kept),
        fsx::read(d.join("data/state.json")),
        fsx::read(d.join("data/keep.bin")),
        fsx::read(ad.join("webcert").join(&svc).join("fullchain.pem")),
        fsx::read(ad.join("acme/account.key")),
    );
    let (old_pid, old_helper) = (pid_of(&b, &svc), pid_of(&b, ASVC));
    assert!(b.running(&svc) && b.running(ASVC), "以前的那两个进程还在运行");

    // 用这一版再运行（只写目录）：更新
    let r = fx.install(&b, &["--dir", &format!("/opt/{svc}")]);
    assert_in("更新了服务端程序", &r.out);
    assert_not_in("cca team attach", &r.out);
    assert_in(&format!("手机的网页端：https://127.0.0.1:{web}/"), &r.out);
    let e = b.envfile(Some(&d));
    let after = (
        pin(&d),
        fsx::read(d.join("certs/key.pem")),
        pick(&e, &kept),
        fsx::read(d.join("data/state.json")),
        fsx::read(d.join("data/keep.bin")),
        fsx::read(ad.join("webcert").join(&svc).join("fullchain.pem")),
        fsx::read(ad.join("acme/account.key")),
    );
    assert!(before == after, "证书、端口和地址、数据、网页端的证书、证书机构那里的账户都没变");
    assert_eq!((e["CCA_HUB_VERSION"].as_str(), e.contains_key("CCA_HUB_IMAGE")), ("9.9.9", false), "不再记镜像");
    assert_eq!(names(&d.join("app")), ["cca-core", "name"], "以前的程序、网页端的文件删掉了");
    assert!(fsx::read(d.join("app/cca-core")) == helper_bytes());
    assert!(b.running(&svc) && pid_of(&b, &svc) != old_pid, "换了一个进程：以前的那个停了");
    let started = b.log("server.argv");
    let last = started.lines().rev().find(|l| l.contains(" team-server ")).unwrap_or_default();
    assert!(last.starts_with(&format!("{} team-server --mode team ", d.join("app/cca-core").display())), "{last}");
    // 共用的证书助手也换了：同一份程序，证书接着用
    assert!(b.running(ASVC) && pid_of(&b, ASVC) != old_helper);
    assert!(fsx::read(ad.join("app/cca-core")) == helper_bytes());
    assert_eq!(b.log("acme.issued"), "127.0.0.1\n", "没有再要一张");
    let ae = b.envfile(Some(&ad));
    assert_eq!((ae["CCA_HUB_VERSION"].as_str(), ae.contains_key("CCA_HUB_IMAGE")), ("9.9.9", false));
    let texts = if docker {
        assert_eq!(names(&ad.join("app")), ["ca.pem", "cca-core"]);
        assert_eq!(b.log("images"), format!("{}\n", hubbox::RUNTIME), "容器用的空镜像这时才建");
        [fsx::read_text(d.join("compose.yaml")), fsx::read_text(ad.join("compose.yaml"))]
    } else {
        assert_eq!(names(&ad.join("app")), ["cca-core"]);
        [fsx::read_text(units.join(format!("{svc}.service"))), fsx::read_text(units.join(format!("{ASVC}.service")))]
    };
    for text in &texts {
        assert!(text.contains("/app/cca-core") && !text.contains("python"), "{text}");
    }
    // 设备都不用动：照旧连得上、网页端照旧
    assert_eq!((sync_state(&a), sync_state(&bob)), ("ok".to_string(), "ok".to_string()));
    let (st, body) = https(web, "/");
    assert_eq!(st, 200);
    assert_in("<title>cca 远程会话（甲组）</title>", &utf8(body));
    clean(&b);
}

/// 从 3.6.x 换过来没成（Docker 的 compose up、systemd 的服务起不来）：.env（Docker 的记着 python 镜像）、compose.yaml / unit 换回
/// Python 的那一份，以前的程序和网页端的文件都在，照原来的方式重新起了一次（这台假服务器上起不了 Python 的：说没起来、看哪里的
/// 日志）；再运行照样换过来
fn python_era_rollback(t: &mut Ctx, docker: bool) {
    let fx = setup(t);
    let b = fx.hub(t, &if docker { docker_any_ip() } else { any_ip() });
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let (port, web) = two_ports();
    let (ps, ws) = (port.to_string(), web.to_string());
    let mode = if docker { "--docker" } else { "--systemd" };
    fx.install(&b, &["--team-root", &root, mode, "--host", "127.0.0.1", "--port", &ps, "--web-port", &ws]);
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    make_python_era(&b, &svc, docker);
    let run = if docker { d.join("compose.yaml") } else { b.root.join(UNITS).join(format!("{svc}.service")) };
    let state = || [d.join(".env"), run.clone(), d.join("app/cca-presence.py"), d.join("app/web/index.html")].map(fsx::read_text);
    let before = state();
    let (log, started, says) = if docker {
        ("docker.log", format!("compose -p {svc} -f compose.yaml up "), format!("docker logs {svc}"))
    } else {
        ("systemctl.log", format!("restart {svc}"), format!("journalctl -u {svc}"))
    };
    let mark = b.log(log).len();
    b.flag_once(if docker { "compose_fail" } else { "start_fail" });
    let r = fx.try_install(&b, &["--dir", &format!("/opt/{svc}"), mode]);
    assert_eq!(r.code, 1);
    assert_in(if docker { "docker compose up 没成" } else { "systemd 起不来" }, &r.err);
    assert!(state() == before, ".env、compose.yaml / unit、以前的程序和网页端的文件都还是原来的");
    assert!(!(d.join(".env.prev").exists() || d.join(".compose.prev").exists() || d.join(".unit.prev").exists()), "不留那几份备份");
    let starts = b.log(log)[mark..].lines().filter(|l| l.starts_with(&started)).count();
    assert_eq!(starts, 2, "这一次的起了一次，换回去以后照原来的又起了一次");
    assert_in(&format!("原来的服务却没起来（{says} 看它说的）"), &r.out);
    // 再运行（这次起得来）：照样换成 cca-core
    let r = fx.install(&b, &["--dir", &format!("/opt/{svc}")]);
    assert_in("更新了服务端程序", &r.out);
    assert_eq!(names(&d.join("app")), ["cca-core"]);
    wait_for("换成 cca-core 以后照样连得上", WAIT, || sync_state(&a) == "ok");
    clean(&b);
}

/// 安装脚本带着的服务端程序不对时停下：这台服务器是别的机器（deps）、这一份脚本没带这种机器的（install）——都在动手之前；
/// 那一段坏了（校验值对不上）、解出来的程序在这台服务器上运行不了（install）——装着的那一份、.env 都不动，正在运行的照旧，
/// 写到一半的不留；第一次装时不留 .env
fn core_checks(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let dir_arg = format!("/opt/{svc}");
    let port = team_port().to_string();
    let first = ["--team-root", root.as_str(), "--host", "127.0.0.1", "--port", port.as_str()];
    let env = [("HUB_ENTER", p(&b.enter)), ("CCA_VERSION", "9.9.9")];
    let helper = harness::helper();
    // 别的机器（假的 uname）
    b.fake("uname", "#!/bin/sh\necho riscv64\n");
    let r = fx.try_install(&b, &first);
    assert_eq!((r.code, r.err.trim()), (1, "✗ 这台服务器是 riscv64：服务端程序只有 x86_64、aarch64 的"));
    assert_eq!(code(&a, &["team-deploy", &tid, "root@box", "--port", &port], &env), "deps");
    // 这一份脚本只带了 aarch64 的，服务器是 x86_64 的
    b.fake("uname", "#!/bin/sh\necho x86_64\n");
    let arm_only = installer(&fx.dir, "arm-only.sh", &[("CCA_HUB_CORE_AARCH64", p(&helper))]);
    assert_in("\nCCA_HUB_CORES='AARCH64'\n", &fsx::read_text(&arm_only));
    let r = run_on(&b, &arm_only, &first);
    assert_eq!((r.code, r.err.trim()), (1, "✗ 这一份安装脚本没带 X86_64 的服务端程序"));
    let only_arm = [env[0], env[1], ("CCA_HUB_CORE_X86_64", "")];
    assert_eq!(code(&a, &["team-deploy", &tid, "root@box", "--port", &port], &only_arm), "install");
    assert!(!d.exists(), "都在动手之前：什么都没建");
    std::fs::remove_file(b.bin.join("uname")).expect("拿掉假的 uname");

    // 装好、接上；之后的几次更新都该不成，装着的原样
    fx.install(&b, &first);
    a.ok(&["team-attach", &tid, &format!("https://127.0.0.1:{port}"), &pin(&d)]);
    let (env_text, pid, good) = (fsx::read_text(d.join(".env")), pid_of(&b, &svc), fsx::read(d.join("app/cca-core")));
    let untouched = |what: &str| {
        assert_eq!(fsx::read_text(d.join(".env")), env_text, "{what}：.env 没动");
        assert!(!d.join(".env.prev").exists(), "{what}");
        assert!(fsx::read(d.join("app/cca-core")) == good, "{what}：装着的程序没动");
        assert_eq!(names(&d.join("app")), ["cca-core"], "{what}：写到一半的不留");
        assert_eq!((pid_of(&b, &svc), b.running(&svc)), (pid.clone(), true), "{what}：服务照旧在运行");
    };
    // 那一段坏了（每一段各少一行）：校验值对不上
    let s = fsx::read_text(&fx.script);
    let mut lines: Vec<&str> = s.split('\n').collect();
    for arch in ["X86_64", "AARCH64"] {
        let open = format!("\tcat <<'CCA_HUB_CORE_{arch}_EOF'");
        let at = lines.iter().position(|l| *l == open).unwrap_or_else(|| panic!("没有 {arch} 的那一段"));
        lines.remove(at + 3);
    }
    let broken = fx.dir.join("broken.sh");
    fsx::write(&broken, lines.join("\n"));
    let r = run_on(&b, &broken, &["--dir", &dir_arg, "--port", &team_port().to_string()]);
    assert_eq!((r.code, r.err.trim()), (1, "✗ 解不开服务端程序（安装脚本不完整？）"));
    untouched("那一段坏了");
    // 解得开、校验值也对，可在这台服务器上运行不了
    let dud = fx.dir.join("dud");
    fsx::write_script(&dud, "#!/bin/sh\nexit 1\n");
    let duds = [("CCA_HUB_CORE_X86_64", p(&dud)), ("CCA_HUB_CORE_AARCH64", p(&dud))];
    let bad = installer(&fx.dir, "dud.sh", &duds);
    let r = run_on(&b, &bad, &["--dir", &dir_arg, "--port", &team_port().to_string()]);
    assert_eq!(
        (r.code, r.err.trim()),
        (1, format!("✗ 服务端程序在这台服务器上运行不了（/opt/{svc} 所在的盘不许执行程序？换个目录：--dir）").as_str())
    );
    untouched("程序运行不了");
    let with_duds = [env[0], env[1], duds[0], duds[1]];
    assert_eq!(code(&a, &["team-deploy", &tid, "root@box"], &with_duds), "install");
    untouched("经 cca team deploy");
    assert_eq!(sync_state(&a), "ok");
    // 第一次装就运行不了：.env 不留（下一次还是第一次装），没有服务
    let c = fx.hub(t, &plain());
    let r = run_on(&c, &bad, &["--team-root", &root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    assert_eq!(r.code, 1);
    assert_in("运行不了", &r.err);
    assert!(!(opt(&c, &svc).join(".env").exists() || opt(&c, &svc).join("app/cca-core").exists() || c.running(&svc)));
    assert_eq!(c.log("systemctl.log"), "", "没到起服务那一步");
    clean(&b);
}

/// 服务器上没有 CA 证书（没装 ca-certificates）：证书助手连不上 Let's Encrypt，说一声怎么办；Team 的同步服务器照样装好、在运行
/// （网页端的证书以后再说）。装上 CA 证书以后再运行就开上。两种运行方式都一样
fn no_ca_bundle(t: &mut Ctx) {
    let fx = setup(t);
    let (_a, _tid, root, svc) = fx.team("甲组");
    for opts in [any_ip(), docker_any_ip()] {
        let b = fx.hub(t, &opts);
        let ca = b.root.join(hubbox::CA_BUNDLE);
        let pem = fsx::read(&ca);
        std::fs::remove_file(&ca).expect("删掉 CA 证书");
        let (port, web) = two_ports();
        let r = fx.install(
            &b,
            &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string(), "--web-port", &web.to_string()],
        );
        assert_in("! 找不到 CA 证书（", &r.out);
        assert_in("）：证书助手连不上 Let's Encrypt", &r.out);
        assert_in(&format!("! 证书助手（{ASVC}）起不来"), &r.out);
        assert_in("✓ Team 的同步服务器在运行", &r.out);
        assert!(b.running(&svc) && !b.running(ASVC), "Team 的服务器在运行，证书助手没起");
        assert_eq!(health(port).get("web"), Some(&Value::from(format!("https://127.0.0.1:{web}/"))));
        fsx::write(&ca, &pem);
        let r = fx.install(&b, &["--dir", &format!("/opt/{svc}")]);
        assert_in(&format!("手机的网页端：https://127.0.0.1:{web}/（证书是"), &r.out);
        assert!(b.running(ASVC));
        clean(&b);
    }
}

/// Docker 方式的容器用的空镜像：这台服务器上的几个 Team 用同一个（只建一次）；卸载一个，另一个还用着：删不掉，留着；卸载
/// 最后一个，镜像一起删掉。systemd 方式装的卸载时没有这个镜像，不碍事
fn runtime_image(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &with_docker());
    let (a1, a2) = (fx.admin("a"), fx.admin("b"));
    let ((_tid1, root1, svc1), (_tid2, root2, svc2)) = (new_team(&a1, "甲组"), new_team(&a2, "乙组"));
    let image = format!("{}\n", hubbox::RUNTIME);
    for root in [&root1, &root2] {
        fx.install(&b, &["--team-root", root, "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    }
    let count = |what: &str| b.log("docker.log").lines().filter(|l| l.starts_with(what)).count();
    assert_eq!((count("import "), b.log("images")), (1, image.clone()), "两个 Team 用同一个镜像，只建一次");
    fx.install(&b, &["--uninstall", "--dir", &format!("/opt/{svc1}")]);
    assert_eq!((count("image rm "), b.log("images")), (1, image), "另一个 Team 还用着：删不掉，留着");
    assert!(b.running(&svc2) && !b.running(&svc1));
    fx.install(&b, &["--uninstall", "--dir", &format!("/opt/{svc2}")]);
    assert_eq!((count("image rm "), b.log("images")), (2, String::new()), "最后一个用它的卸载了：镜像也删掉");
    clean(&b);
    // systemd 方式（这台服务器上也有 Docker，只是没用它）：卸载时删镜像那一步不成，不碍事
    let c = fx.hub(t, &with_docker());
    fx.install(&c, &["--team-root", &root1, "--systemd", "--host", "127.0.0.1", "--port", &team_port().to_string()]);
    let r = fx.install(&c, &["--uninstall", "--dir", &format!("/opt/{svc1}")]);
    assert_in("卸载了 Team 的同步服务器", &r.out);
    assert_eq!(c.log("images"), "");
}

// ---- cca team deploy（team-deploy）：经假的 ssh 把同一个脚本交给假服务器，装好了接上 ----

/// 整个输出当成一串 sh 的 键=值 拆开（值里有换行——steps 是几行——所以整个一起拆，不能一行一行来）
fn kvs(text: &str) -> BTreeMap<String, String> {
    sandbox::shell_split(text)
        .into_iter()
        .map(|tok| match tok.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => panic!("不是 键=值：{tok:?}（整个输出：{text:?}）"),
        })
        .collect()
}

/// 经 SSH 装好（Docker）、接上，整条路通（发邀请、别的设备加入、同步到配置）；再部署是更新；地址是内网的：网页端不开；托管平台的部署文件
fn deploy(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &with_docker());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let port = team_port();
    let (ps, relay, host) = (port.to_string(), (port + 1).to_string(), format!("127.0.0.1:{port}"));
    assert_eq!((deploy_ok(&a, &tid, &b, &["--check"], &[]).code, d.exists()), (0, false), "--check 只看登录得上不");
    let r = deploy_ok(&a, &tid, &b, &["--port", &ps], &[]);
    assert_eq!(
        kv(&r.out),
        map(&[
            ("mode", "docker"),
            ("state", "new"),
            ("port", &ps),
            ("relay", &relay),
            ("dir", &format!("/opt/{svc}")),
            ("lan", "1"),
            ("host", &host),
            ("device", "studio"),
            ("members", "1"),
            ("stored", "1"),
            ("note", ""),
            ("svc", &svc),
            ("web", ""),
            ("web_cert", ""),
            ("web_off", "lan"),
            ("push", ""),
        ])
    );
    assert!(r.err.contains("Team 的同步服务器在运行"), "服务器那边的进度经 ssh 的 stderr 直接打出来");
    let team = team_json(&a, &tid);
    assert_eq!((&team["url"], &team["pins"]), (&Value::from(format!("https://127.0.0.1:{port}")), &list![pin(&d)]));
    assert_eq!(env_of(&b, &d, "CCA_HUB_HOST"), "127.0.0.1", "装之前就照 ssh -G 定了地址（网页端的证书要它）");
    let y = fsx::read_text(d.join("compose.yaml"));
    assert!(
        y.contains(&format!("container_name: {svc}\n"))
            && y.contains(r#""--mode", "team""#)
            && y.contains("CCA_TEAM_ROOT: ${CCA_TEAM_ROOT:?}"),
        "{y}"
    );
    assert!(!(y.contains(ASVC) || opt(&b, ASVC).exists()), "网页端没开：没有证书助手");
    assert_eq!(
        b.log("compose.projects").split_whitespace().collect::<BTreeSet<_>>(),
        BTreeSet::from([svc.as_str()]),
        "compose 的项目名照服务名（不照目录名）"
    );
    let argv = fsx::read_text(a.home.join("ssh.argv"));
    assert_not_in(p(&a.cca), &argv);
    assert_in("-o ConnectTimeout=15 -G root@box", &argv);
    assert!(argv.contains("-o ConnectTimeout=15 -T -o ServerAliveInterval=30 root@box sh -c"), "不要伪终端：脚本从 stdin 过去");
    // 发邀请、另一台设备加入：取到了配置
    let invite = a.ok(&["team-invite", &tid, "bob-mini"]);
    let link = invite.split('\n').next().unwrap_or_default();
    let bob = Device::new(&fx.dir.join(format!("bob-{}", crypto::token_hex(4))), "bob", &obj! {});
    assert_eq!(bob.ok_with(&["team-join", "--name", "bob-mini"], link, &[]).split('\n').nth(5), Some("ok"));
    // 再部署：更新，地址照原来接着的，设备不用动
    let r = deploy_ok(&a, &tid, &b, &[], &[]);
    assert_eq!(pick(&kv(&r.out), &["state", "port", "host", "members"]), ["update", ps.as_str(), host.as_str(), "2"]);
    assert_eq!(sync_state(&bob), "ok");
    // 这次 ssh 走的是内网地址（别名、VPN）：服务器上记着的地址不拿它盖掉（网页端的证书、各台设备连的都是记着的那个）
    let r = deploy_ok(&a, &tid, &b, &[], &[("HUB_SSH_HOST", "10.0.0.5")]);
    assert_eq!(pick(&kv(&r.out), &["state", "host"]), ["update", host.as_str()]);
    assert_eq!(env_of(&b, &d, "CCA_HUB_HOST"), "127.0.0.1");
    // 托管平台：生成部署文件（服务端程序 cca-core——Linux x86_64 的那一份、Dockerfile、README），里面只有公开的根公钥（后面那一段）
    let key = key_of(&root);
    let rw = a.home.join("rw");
    let out = kvs(&a.ok_with(&["team-hosted", &tid, "railway", "--dir", p(&rw)], "", &[("CCA_VERSION", "9.9.9")]));
    assert_eq!(pick(&out, &["dir", "tid", "name", "svc"]), [p(&rw), tid.as_str(), "甲组", svc.as_str()]);
    assert_eq!(names(&rw), ["Dockerfile", "README.md", "cca-core"]);
    let dockerfile = fsx::read_text(rw.join("Dockerfile"));
    assert_in(&format!("\nFROM scratch\nCOPY cca-core /cca-core\nENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n"), &dockerfile);
    assert!(dockerfile.ends_with("EXPOSE 8080\nCMD [\"/cca-core\", \"team-server\"]\n"), "{dockerfile}");
    assert_not_in("python", &dockerfile.to_lowercase());
    assert_in(&format!("railway init --name {svc}"), &out["steps"]);
    assert!(fsx::read(rw.join("cca-core")) == helper_bytes(), "部署文件里的服务端程序不是这份 cca 带着的那一份");
    assert_eq!(fsx::mode(rw.join("cca-core")) & 0o777, 0o755, "能运行（镜像里直接运行它）");
    assert_in("cca team attach 甲组 https://", &fsx::read_text(rw.join("README.md")));
    // 托管平台只有 Railway：不写平台就是它；没写目录：~/<服务名>-railway
    let auto = a.home.join(format!("{svc}-railway"));
    let out = kvs(&a.ok(&["team-hosted", &tid]));
    assert_eq!(out["dir"], p(&auto), "没写目录：~/<服务名>-railway");
    assert_in(&format!("railway init --name {svc}"), &out["steps"]);
    assert_in(&format!("ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n"), &fsx::read_text(auto.join("Dockerfile")));
    // Sealos 不再支持（它的表单只能把服务端程序当文本挂进去）：说换 Railway 或者自己的服务器，什么都不生成
    let se = a.home.join("se");
    let r = a.run(&["team-hosted", &tid, "sealos", "--dir", p(&se)]);
    assert_eq!(
        (r.code, r.out.as_str()),
        (1, "usage\t不再支持 Sealos：部署到 Railway，或者自己的服务器（cca team deploy）\n"),
        "{}",
        r.err
    );
    assert!(!se.exists());
    fsx::write(rw.join("other.txt"), "x");
    assert_eq!(code(&a, &["team-hosted", &tid, "railway", "--dir", p(&rw)], &[]), "dir");
    assert_eq!(code(&a, &["team-hosted", &tid, "heroku"], &[]), "usage");
    let files: String = [&rw, &auto]
        .into_iter()
        .flat_map(|d| names(d).into_iter().map(move |f| String::from_utf8_lossy(&fsx::read(d.join(f))).into_owned()))
        .collect();
    assert!(!files.contains(p(&a.cca)), "部署文件里没有这台设备上的东西");
    // 写错的：没有这个 Team、Team 的部署没有 --reset、成员不是管理员
    let env = [("HUB_ENTER", p(&b.enter)), ("CCA_VERSION", "9.9.9")];
    assert_eq!(code(&a, &["team-deploy", "nope", "root@box"], &env), "no_team");
    assert_eq!(code(&a, &["team-deploy", &tid, "root@box", "--reset"], &env), "usage");
    assert_eq!(code(&bob, &["team-deploy", &tid, "root@box"], &env), "not_admin");
    clean(&b);
}

/// 删除 Team 时卸载（team-undeploy：cca team leave 的管理员经它）：经 SSH 运行同一个脚本的 --uninstall，带着这个 Team 的根公钥——
/// 目录里装的是别的 Team 的不动（taken），没装的是 install；卸好了 stdout 是目录。部署时记下了装到哪（admin.json 的 deploy），删除时照它
fn undeploy(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, svc) = fx.team("甲组");
    let (a2, tid2, _root2, svc2) = fx.team("乙组");
    let (d, d2) = (opt(&b, &svc), opt(&b, &svc2));
    let rel = format!("no-such-{}/vps", crypto::token_hex(4)); // 相对的、不存在的（假 ssh 不看 -i）
    deploy_ok(&a, &tid, &b, &["--port", &team_port().to_string(), "--identity", &rel], &[]);
    let key2 = a2.home.join(".ssh/vps_key");
    std::fs::create_dir_all(a2.home.join(".ssh")).expect("建 ~/.ssh");
    fsx::write(&key2, "私钥\n");
    deploy_ok(&a2, &tid2, &b, &["--port", &team_port().to_string(), "--identity", "~/.ssh/vps_key"], &[]);
    // 辅助程序是在这个进程的当前目录里运行的：相对的路径照它补成绝对的（os.path.abspath）
    let abs = std::env::current_dir().expect("当前目录").join(&rel);
    let admin = fsx::read_json(a.cca.join("teams").join(&tid).join("admin.json"));
    assert_eq!(
        admin["deploy"],
        obj! { "target" => "root@box", "ssh_port" => "", "dir" => format!("/opt/{svc}"), "identity" => p(&abs) },
        "-i 记成绝对路径（换个目录运行、App 里删除时也找得到）"
    );
    assert_eq!(
        a.ok(&["team-leave", "plan", &tid]).trim_end_matches('\n').split('\t').skip(4).collect::<Vec<_>>(),
        ["root@box", "", &format!("/opt/{svc}"), "1"],
        "记下的私钥文件不在：不给，照 ssh 的配置登录"
    );
    assert_eq!(
        a2.ok(&["team-leave", "plan", &tid2]).trim_end_matches('\n').split('\t').skip(4).collect::<Vec<_>>(),
        ["root@box", "", &format!("/opt/{svc2}"), "1", p(&key2)],
        "~/ 换成主目录；文件在：最后一栏是它"
    );
    let env = [("CCA_VERSION", "9.9.9"), ("HUB_ENTER", p(&b.enter))];
    assert_eq!(
        code(&a, &["team-undeploy", &tid, "root@box", "--dir", &format!("/opt/{svc2}")], &env),
        "taken",
        "装的是别的 Team 的"
    );
    assert!(d2.join(".env").exists(), "别的 Team 的不动");
    assert_eq!(code(&a, &["team-undeploy", &tid, "root@box", "--dir", "/opt/nothing-here"], &env), "install", "目录里没装");
    let r = a.run_with(&["team-undeploy", &tid, "root@box", "--check"], "", &env);
    assert_eq!((r.code, r.out.as_str(), d.join(".env").exists()), (0, "", true), "--check 只看登录得上不");
    let r = a.run_with(&["team-undeploy", &tid, "root@box"], "", &env);
    assert_eq!((r.code, r.out.as_str()), (0, format!("/opt/{svc}\n").as_str()), "{}", r.err);
    assert!(r.err.contains("卸载了 Team 的同步服务器"), "服务器那边的进度经 ssh 的 stderr 直接打出来");
    assert!(!r.err.contains("cca team attach"), "删除 Team 时卸载：不再说换一台接上");
    assert!(!d.exists());
    assert!(d2.join(".env").exists(), "同一台服务器上别的 Team 的照旧");
    assert_eq!(code(&a2, &["team-undeploy", &tid, "root@box"], &env), "no_team");
}

/// 服务器上已经装着这个 Team（在服务器上直接运行安装脚本装的，记着地址），这台设备还没接上它：ssh 走的是内网地址时，
/// 各台设备连的也是安装脚本定下的（服务器上记着的）那个，和网页端的一致
fn recorded_host(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &with_docker());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let port = team_port();
    fx.install(&b, &["--team-root", &root, "--host", "127.0.0.1", "--port", &port.to_string()]);
    let r = deploy_ok(&a, &tid, &b, &[], &[("HUB_SSH_HOST", "10.0.0.5")]);
    assert_eq!(pick(&kv(&r.out), &["state", "host", "members"]), ["update", format!("127.0.0.1:{port}").as_str(), "1"]);
    assert_eq!(team_json(&a, &tid)["url"], Value::from(format!("https://127.0.0.1:{port}")));
    assert_eq!(env_of(&b, &d, "CCA_HUB_HOST"), "127.0.0.1");
}

/// 手机的网页端：装的时候就开上（证书助手要来了证书：web_cert ok；标题带上 Team 的名字：team-deploy 放进 --name）；--no-web 关掉；
/// 证书没要到是 web_cert wait
fn deploy_test_web(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &docker_any_ip());
    let (a, tid, _root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let (port, web) = two_ports();
    let url = format!("https://127.0.0.1:{web}/");
    let r = deploy_ok(&a, &tid, &b, &["--port", &port.to_string(), "--web-port", &web.to_string()], &[]);
    let o = kv(&r.out);
    assert_eq!(pick(&o, &["web", "web_cert", "web_off"]), [url.as_str(), "ok", ""]);
    assert!(b.running(ASVC));
    assert_eq!(fsx::read_text(d.join("app/name")), "甲组\n");
    assert_in(r#""--web-name-file", "/app/name"]"#, &fsx::read_text(d.join("compose.yaml")));
    let (st, body) = https(web, "/");
    assert_eq!(st, 200);
    assert_in("<title>cca 远程会话（甲组）</title>", &utf8(body));
    assert_in(r#""short_name": "cca 甲组""#, &utf8(https(web, "/manifest.webmanifest").1));
    assert_eq!(health(port).get("web"), Some(&Value::from(url.as_str())));
    let r = deploy_ok(&a, &tid, &b, &[], &[]);
    assert_eq!(kv(&r.out)["web"], url, "再部署：照旧开着");
    let r = deploy_ok(&a, &tid, &b, &["--no-web"], &[]);
    let o = kv(&r.out);
    assert_eq!(pick(&o, &["web", "web_off"]), ["", ""], "自己关的：不说原因");
    assert!(!b.running(ASVC));
    std::fs::remove_dir_all(opt(&b, ASVC).join("store")).expect("删掉存着的证书"); // 存着的那张也没了
    b.flag("acme_fail", true);
    let r = deploy_ok(&a, &tid, &b, &["--web-port", &web.to_string()], &[]);
    let o = kv(&r.out);
    assert_eq!(pick(&o, &["web", "web_cert"]), [url.as_str(), "wait"]);
}

/// 参数不对：辅助程序先查（usage），不连服务器
fn args(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, _svc) = fx.team("甲组");
    // 这里的 20000 只是参数里的数：先查出写错了，没有程序去绑它
    let cases: [&[&str]; 17] = [
        &["--port", "abc"],
        &["--port", "80"],
        &["--port", "+2000"],
        &["--dir", "//"],
        &["--dir", "/opt/../etc"],
        &["--dir", "/home/me/hub"],
        &["--host", "a b"],
        &["--ssh-port", "0"],
        &["--mode", "podman"],
        &["--bogus"],
        &["--port"],
        &["--web-port", "80"],
        &["--web-port", "20000", "--port", "20000"],
        &["--no-web", "--web-port", "20000"],
        &["--push", "https://192.0.2.10:18460"],
        &["--push", "ftp://192.0.2.10", "--push-pin", PUSH_PIN],
        &["--no-push", "--push", "https://192.0.2.10:18460", "--push-pin", PUSH_PIN],
    ];
    for case in cases {
        let r = try_deploy(&a, &tid, &b, case, &[]);
        assert_eq!((r.code, r.out.split('\t').next()), (1, Some("usage")), "{case:?}");
    }
    assert!(!a.home.join("ssh.argv").exists(), "不连服务器");
}

/// 推送网关（--push、--push-pin）：记在 .env 里，再运行照旧；开着中转时交给服务端——systemd 的经 EnvironmentFile，Docker 的经
/// compose 的 environment（假的 docker 查这两样对得上）——它起来时说转给哪个网关；没开中转时记着、不交；--no-push 去掉。写错了、
/// 和 --no-relay 一起写是 usage，什么都还没动。cca team deploy 照样传过去，结果里有 push，再部署照上次的
fn push(t: &mut Ctx) {
    let fx = setup(t);
    let (url, pin) = ("https://192.0.2.10:18460", PUSH_PIN);
    let said = "推送：手机登记了的，会话里的通知转给推送网关 192.0.2.10:18460";
    let both = [Some(url.to_string()), Some(pin.to_string())];
    let push_env = |b: &FakeBox, d: &Path| {
        let e = b.envfile(Some(d));
        [e.get("CCA_PUSH_URL").cloned(), e.get("CCA_PUSH_PIN").cloned()]
    };
    let started = |b: &FakeBox| b.log("server.log").matches(said).count();
    let b = fx.hub(t, &plain());
    let (a, tid, root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let ps = team_port().to_string();
    let base = ["--team-root", root.as_str(), "--host", "127.0.0.1", "--port", ps.as_str()];
    for (extra, why) in [
        (&["--push", url][..], "--push 和 --push-pin（推送网关证书的公钥）要一起写"),
        (&["--push-pin", pin], "--push 和 --push-pin（推送网关证书的公钥）要一起写"),
        (&["--push", "http://192.0.2.10", "--push-pin", pin], "--push 要写成 https://主机[:端口]"),
        (&["--push", url, "--push-pin", "sha256//abc="], "--push-pin 要写成 sha256//"),
        (&["--push", url, "--push-pin", pin, "--no-relay"], "推送是远程会话的"),
        (&["--no-push", "--push", url, "--push-pin", pin], "--no-push 和 --push、--push-pin 只能写一种"),
    ] {
        let r = fx.try_install(&b, &[&base[..], extra].concat());
        assert_eq!(r.code, 1, "{extra:?}：{}{}", r.out, r.err);
        assert_in(why, &r.err);
        assert!(!d.join(".env").exists(), "什么都还没动：{extra:?}");
    }
    let r = fx.install(&b, &[&base[..], &["--push", url, "--push-pin", pin]].concat());
    assert_in(&format!("推送：iPhone 上的 cca App 登记了的，会话里的通知经推送网关 {url} 推到手机上"), &r.out);
    assert_eq!(push_env(&b, &d), both);
    assert_eq!(started(&b), 1, "systemd：服务端经 EnvironmentFile 拿到了，起来时说了");
    fx.install(&b, &base);
    assert_eq!(push_env(&b, &d), both, "没写：照上次的");
    assert_eq!(started(&b), 2);
    let r = fx.install(&b, &[&base[..], &["--no-push"]].concat());
    assert_not_in("推送网关", &r.out);
    assert_eq!(push_env(&b, &d), [None, None]);
    assert_eq!(started(&b), 2, "这一次起来的不用推送");

    // Docker：开着中转时 compose 的 environment 里交给它；--no-relay 时记着、不交
    let bd = fx.hub(t, &Opts { docker: true, ..Opts::default() });
    let dd = opt(&bd, &svc);
    let ps = team_port().to_string();
    let base = ["--team-root", root.as_str(), "--host", "127.0.0.1", "--port", ps.as_str()];
    fx.install(&bd, &[&base[..], &["--push", url, "--push-pin", pin]].concat());
    let y = fsx::read_text(dd.join("compose.yaml"));
    assert_in("      CCA_PUSH_URL: ${CCA_PUSH_URL:?}\n      CCA_PUSH_PIN: ${CCA_PUSH_PIN:?}\n", &y);
    assert!(!y.contains('@'), "记号都换掉了");
    assert_eq!(started(&bd), 1, "Docker：服务端经 environment 拿到了");
    fx.install(&bd, &[&base[..], &["--no-relay"]].concat());
    assert_not_in("CCA_PUSH", &fsx::read_text(dd.join("compose.yaml")));
    assert_eq!(push_env(&bd, &dd), both, "没开中转：记着，再开中转时接着用");
    assert_eq!(started(&bd), 1);

    // cca team deploy：照样传过去，结果里有；再部署照上次的
    let bt = fx.hub(t, &plain());
    let pt = team_port().to_string();
    let r = deploy_ok(&a, &tid, &bt, &["--port", &pt, "--push", url, "--push-pin", pin], &[]);
    assert_eq!(kv(&r.out)["push"], url);
    let r = deploy_ok(&a, &tid, &bt, &[], &[]);
    assert_eq!(kv(&r.out)["push"], url, "再部署：服务器上照上次的");
    let r = deploy_ok(&a, &tid, &bt, &["--no-push"], &[]);
    assert_eq!(kv(&r.out)["push"], "");
    assert_eq!(started(&bt), 2);
}

/// 装好了却接不上（安全组没放行）：说要放行哪个端口；放行以后再部署（服务器上是更新），接上；服务器自己的防火墙经 SSH 装时也说
fn unreachable_then_again(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, _svc) = fx.team("甲组");
    let port = team_port();
    std::fs::create_dir(a.home.join("reply")).expect("建 reply");
    fsx::write(a.home.join("reply/team_enroll"), "000\n"); // 假 curl：连不上
    let r = try_deploy(&a, &tid, &b, &["--port", &port.to_string()], &[]);
    let (reason, why) = two_fields(&r.out);
    assert_eq!(reason, "unreachable");
    assert_in(&format!("服务器上装好了（127.0.0.1:{port}），但没接上"), why);
    assert_in(&format!("放行 TCP {port}，放行以后再运行一次"), why);
    assert!(!team_json(&a, &tid).get("url").is_some_and(py::truthy), "没接上：还是没接着服务器");
    std::fs::remove_file(a.home.join("reply/team_enroll")).expect("删掉 reply/team_enroll");
    b.fake("ufw", &format!("#!/bin/sh\n{UFW}"));
    let r = deploy_ok(&a, &tid, &b, &[], &[]);
    assert_eq!(pick(&kv(&r.out), &["mode", "state", "port"]), ["systemd", "update", port.to_string().as_str()]);
    assert_in(&format!("这台服务器开着 ufw：运行 ufw allow {port}/tcp 放行"), &r.err);
}

/// 用普通用户登录：免密码的 sudo 就经它装；sudo 要密码时什么都不运行，说要用 root 登录
fn not_root(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    fsx::write(b.d.join("uid"), "1000");
    let (a, tid, _root, svc) = fx.team("甲组");
    let r = try_deploy(&a, &tid, &b, &[], &[]);
    let (reason, why) = two_fields(&r.out);
    assert_eq!(reason, "root");
    assert_in("root@box 不是 root，sudo 又要密码", why);
    assert!(!opt(&b, &svc).exists());
    b.flag("sudo_ok", true);
    let r = deploy_ok(&a, &tid, &b, &["--port", &team_port().to_string()], &[]);
    assert_eq!(kv(&r.out)["state"], "new");
    assert_in("-n sh -s", &b.log("sudo.log"));
}

/// 服务已经用这一次的设置起来了、打结果时 SSH 断了（SIGPIPE）：服务器上不换回（换回的话跑着的和 .env 对不上），这台设备照旧接着
/// 原来的；再部署是更新，接上新的
fn dropped_after_start(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    let port = team_port();
    deploy_ok(&a, &tid, &b, &["--port", &port.to_string()], &[]);
    a.fake("ssh", &SSH.replace(r#"exec "$HUB_ENTER" "$cmd""#, r#""$HUB_ENTER" "$cmd" | true; exit 255"#));
    let p2 = team_port();
    let r = try_deploy(&a, &tid, &b, &["--port", &p2.to_string()], &[]);
    assert_eq!(r.out.split('\t').next(), Some("ssh"));
    assert_in("或者中途断了", &r.out);
    assert!(r.err.contains("Team 的同步服务器在运行"), "服务起来了");
    assert_eq!(env_of(&b, &d, "CCA_HUB_PORT"), p2.to_string(), "服务器上是这一次的");
    assert!(!d.join(".env.prev").exists());
    assert!(b.running(&svc));
    assert_eq!(team_json(&a, &tid)["url"], Value::from(format!("https://127.0.0.1:{port}")));
    a.fake("ssh", SSH);
    let r = deploy_ok(&a, &tid, &b, &[], &[]);
    assert_eq!(pick(&kv(&r.out), &["state", "port"]), ["update", p2.to_string().as_str()]);
    assert_eq!(team_json(&a, &tid)["url"], Value::from(format!("https://127.0.0.1:{p2}")));
}

/// App 里部署（bash 的 hub_deploy_ask）：--check 只看登录得上不（什么都不装、不起服务），ssh 说的分成几种原因（bash 照它重问、问密码、
/// 停下）；--askpass 经 SSH_ASKPASS 给密码（不用密钥、只问一次，有 --batch 也一样）；没有终端时不问要不要认这台主机（accept-new）
fn login_check_and_password(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, svc) = fx.team("甲组");
    let pw = format!("pw-{}", crypto::token_hex(8));
    let (pw_file, given_file, askpass, argv_file) =
        (a.home.join("server-pw"), a.home.join("given"), a.home.join("bin/askpass"), a.home.join("ssh.argv"));
    fsx::write(&pw_file, &pw);
    a.fake("ssh", SSH_PW);
    a.fake("askpass", &format!("cat \"{}\"\n", given_file.display())); // bash 的 askpass_run 那样：打出给它的密码
    let ask = [("HUB_PW", p(&pw_file)), ("SSH_ASKPASS", p(&askpass)), ("SSH_ASKPASS_REQUIRE", "force")];
    // 跑一次 team-deploy（成不成都给出来），连同这一次 ssh 的命令行；given 是假的 askpass 打出的密码
    let check = |args: &[&str], given: &str| -> (Ran, String) {
        fsx::write(&given_file, given);
        if argv_file.exists() {
            std::fs::remove_file(&argv_file).expect("删掉 ssh.argv");
        }
        let r = try_deploy(&a, &tid, &b, args, &ask);
        (r, fsx::try_read_text(&argv_file).unwrap_or_default())
    };

    let (r, argv) = check(&["--batch", "--check"], "");
    assert_eq!((r.code, r.out.split('\t').next()), (1, Some("ssh_auth")), "密钥登录不了、服务器认密码：bash 接着问密码");
    assert_in("要用密码登录", &r.out);
    for o in ["BatchMode=yes", "StrictHostKeyChecking=accept-new", "LogLevel=ERROR", "-T root@box true"] {
        assert_in(o, &argv);
    }
    let (r, argv) = check(&["--batch", "--askpass", "--check"], &format!("wrong-{pw}"));
    assert_eq!(r.out, "ssh_denied\t密码不对\n");
    for o in [
        "PubkeyAuthentication=no",
        "PreferredAuthentications=keyboard-interactive,password",
        "NumberOfPasswordPrompts=1",
        "StrictHostKeyChecking=accept-new",
    ] {
        assert_in(o, &argv);
    }
    assert!(!argv.contains("BatchMode"), "用密码时不能是 BatchMode（ssh 就不问 SSH_ASKPASS 了）");
    let (r, _argv) = check(&["--batch", "--askpass", "--check"], &pw);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""), "密码对了：什么都不打");
    assert!(!opt(&b, &svc).exists(), "--check 不装");
    std::fs::remove_file(a.home.join("askpass.n")).expect("删掉 askpass.n");
    let (r, argv) = check(&["--batch", "--askpass", "--port", &team_port().to_string()], &pw);
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(kv(&r.out)["state"], "new", "{}{}", r.out, r.err);
    assert_eq!(fsx::read_text(a.home.join("askpass.n")), "x\n", "部署只连一次（-G 不登录），只取一次密码");
    assert!(!format!("{argv}{}", b.files_text()).contains(&pw), "密码不在 ssh 的命令行和服务器上");
    assert!(!format!("{}{}", r.out, r.err).contains(&pw));
    a.fake("ssh", SSH); // 服务器认这台设备的密钥了；不认、也不让用密码：说怎么办
    b.flag("ssh_denied", true);
    let (r, _) = check(&["--batch", "--check"], "");
    assert_eq!(r.out.split('\t').next(), Some("ssh_keyonly"));
    assert_in("authorized_keys", &r.out);
    b.flag("ssh_denied", false);
    a.fake("ssh", SSH_SAYS);
    for (says, want, part) in [
        (
            "@@@@@@@@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nHost key verification failed.",
            "ssh_hostkey",
            "ssh-keygen -R",
        ),
        (
            "ssh: Could not resolve hostname box: nodename nor servname provided, or not known",
            "ssh",
            "SSH 连不上 root@box：ssh: Could not resolve hostname box",
        ),
        ("", "ssh", "SSH 连不上 root@box"),
        // ssh-agent 里的密钥太多：服务器只让试几把（MaxAuthTries），还没轮到密码就断了（LogLevel=ERROR 时最后一句就是这个）
        ("Received disconnect from 192.0.2.10 port 22:2: Too many authentication failures", "ssh_auth", "要用密码登录"),
    ] {
        let r = try_deploy(&a, &tid, &b, &["--batch", "--check"], &[("HUB_SSH_SAYS", says)]);
        assert_eq!((r.code, r.out.split('\t').next()), (1, Some(want)), "{says}");
        assert_in(part, &r.out);
    }
    let r = try_deploy(
        &a,
        &tid,
        &b,
        &["--batch", "--askpass", "--check"],
        &[("HUB_SSH_SAYS", "root@box: Permission denied (publickey).")],
    );
    assert_eq!((r.code, r.out.split('\t').next()), (1, Some("ssh_keyonly")), "用密码时服务器不让用密码：不说密码不对");
    assert_in("IdentitiesOnly", &r.out);
}

/// ssh 连不上、登录不了；服务器上的证书没了（重新生成）：照新的证书重新接上
fn ssh_and_cert(t: &mut Ctx) {
    let fx = setup(t);
    let b = fx.hub(t, &plain());
    let (a, tid, _root, svc) = fx.team("甲组");
    let d = opt(&b, &svc);
    b.flag("ssh_denied", true);
    let r = try_deploy(&a, &tid, &b, &["--batch"], &[]);
    let (reason, why) = two_fields(&r.out);
    assert_eq!(reason, "ssh");
    assert_in("在终端里运行 cca team deploy", why);
    let argv = fsx::read_text(a.home.join("ssh.argv"));
    assert_in("BatchMode=yes", &argv);
    assert!(!argv.contains("accept-new"), "命令行里没有终端：没见过的主机照旧不连（只有 App 里先查登录时认下）");
    b.flag("ssh_denied", false);
    deploy_ok(&a, &tid, &b, &["--port", &team_port().to_string()], &[]);
    let old = pin(&d);
    b.stop(None);
    for f in ["key.pem", "cert.pem"] {
        std::fs::remove_file(d.join("certs").join(f)).unwrap_or_else(|e| panic!("删不掉 certs/{f}：{e}"));
    }
    deploy_ok(&a, &tid, &b, &[], &[]);
    assert_ne!(pin(&d), old);
    assert_eq!(team_json(&a, &tid)["pins"], list![pin(&d)]);
    assert_eq!(sync_state(&a), "ok");
}
