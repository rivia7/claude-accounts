//! 一键部署自己的同步服务器（docs/dev/hub.md「部署」）：
//! - hub-installer：安装脚本。模板 server/hub-install.sh 里标着 CCA-HUB-EMBED 的那一行换成版本和服务端程序 server/cca-presence.py
//!   （两个都编进辅助程序里：改了要重新编）。`cca hub deploy --script` 打出来的、release.sh 放到发布页上的 cca-hub-install.sh 都是它：
//!   在服务器上用 root 运行，自己生成入伙口令和盐、打出加入链接（盐只在服务器的内存里过一遍，不落盘）
//! - hub-deploy：经 SSH 在服务器上运行同一个脚本（从 stdin 给过去），入伙口令和盐在这台设备上生成，服务器只拿到口令的 SHA-256；
//!   装好以后这台设备跟着加入（`join_with`）。加入链接不出这个进程：不进命令行、环境变量，也不打出来（别的设备要的用 cca hub invite 看）

use super::{
    configured_admit, fail, host_of, hub_doc, join_with, link_of, link_problem, name_ok, own_forget, refresh_admit, Link, NAME_BAD,
    SEED_UNKNOWN,
};
use crate::cmd::cost::version_ok;
use crate::cmd::presence::{PRESENCE_PIN_RE, PRESENCE_URL_RE};
use crate::core::base::{sh, HOST_RE};
use crate::py::hash::sha256_hex;
use crate::py::proc::{getenv, Cmd as Proc, In, Out, ProcError};
use crate::py::rand::token_hex;
use crate::py::re::Re;
use crate::py::{text, Fail, R};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

const TEMPLATE: &str = include_str!("../../../../server/hub-install.sh");
pub(super) const SERVER: &str = include_str!("../../../../server/cca-presence.py");
/// 模板里换成版本、服务端程序的那一行
const EMBED: &str = "# @CCA-HUB-EMBED@\n";
/// 服务端程序放在 here-document 里，这是它的结束行（服务端程序里不能有这一行：测试查；release.sh、regress 照它取出服务端程序核对）
const SERVER_EOF: &str = "CCA_HUB_SERVER_PY_EOF";
const DEF_DIR: &str = "/opt/cca-hub";
/// 在服务器上运行脚本的命令：root 直接运行，别的用户经免密码的 sudo（要密码时不运行，说一声：没有终端可问）。
/// 外面再包一层 sh -c：登录 shell 不一定是 sh（fish、csh）
const REMOTE: &str = r#"sh -c 'if [ "$(id -u)" = 0 ]; then exec sh -s; elif sudo -n true 2>/dev/null; then exec sudo -n sh -s; else echo "CCA-HUB fail sudo"; exit 1; fi'"#;
const USAGE: &str = "用法: py hub-deploy [用户@]主机 [--ssh-port 端口] [--identity 私钥] [--port 端口] [--host 地址] \
                     [--mode docker|systemd] [--dir 目录] [--name 名字] [--reset] [--batch | --askpass] [--check]";

/// SSH 的目标（[用户@]主机；ssh 配置里的别名也行）：不能以 - 开头（会被当成 ssh 的选项）
static TARGET_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[A-Za-z0-9_][A-Za-z0-9._@:%\[\]-]*$"));

/// 安装脚本：模板里 EMBED 那一行换成版本、给 hub-deploy 的几项（preamble；直接在服务器上运行的那一份是空的）和服务端程序
pub fn installer(preamble: &str) -> String {
    let version = getenv("CCA_VERSION").filter(|v| version_ok(v)).unwrap_or_default();
    let embed =
        format!("CCA_HUB_VERSION='{version}'\n{preamble}hub_server_py() {{\n\tcat <<'{SERVER_EOF}'\n{SERVER}{SERVER_EOF}\n}}\n");
    TEMPLATE.replacen(EMBED, &embed, 1)
}

/// py hub-installer：把安装脚本打到 stdout（cca hub deploy --script；release.sh 的 cca-hub-install.sh）
pub fn cmd_hub_installer(args: &[String]) -> R<()> {
    if !args.is_empty() {
        return Err(Fail::Die("用法: py hub-installer".into()));
    }
    crate::out_raw!("{}", installer(""));
    Ok(())
}

/// hub-deploy 的参数
#[derive(Default)]
struct Opts {
    target: String,
    ssh_port: Option<String>,
    identity: Option<String>,
    port: Option<i64>,
    host: Option<String>,
    mode: Option<String>,
    dir: String,
    name: Option<String>,
    reset: bool,
    /// 没有终端：ssh 不问密码、不问要不要认这台主机
    batch: bool,
    /// 密码经 SSH_ASKPASS 给，只问一次（bash 的 askpass_run）
    askpass: bool,
    /// 只看登录得上不，不部署（ssh_check）
    check: bool,
    /// 装的是 Team 的同步服务器（cca team deploy）：说话时的命令名、怎么办的建议照它
    team: bool,
}

impl Opts {
    /// 给人看的命令名
    fn cmd(&self) -> &'static str {
        if self.team {
            "cca team deploy"
        } else {
            "cca hub deploy"
        }
    }
}

/// 读参数、核对：不对的都是 usage（bash 的 hub_usage；名字不对是 name），还没连服务器
fn opts(args: &[String]) -> R<Opts> {
    let mut o = Opts { dir: DEF_DIR.into(), ..Default::default() };
    let mut port = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--reset" => o.reset = true,
            "--batch" => o.batch = true,
            "--askpass" => o.askpass = true,
            "--check" => o.check = true,
            f @ ("--ssh-port" | "--identity" | "--port" | "--host" | "--mode" | "--dir" | "--name") => {
                let Some(v) = it.next().cloned() else { return fail("usage", USAGE) };
                match f {
                    "--ssh-port" => o.ssh_port = Some(v),
                    "--identity" => o.identity = Some(v),
                    "--port" => port = Some(v),
                    "--host" => o.host = Some(v),
                    "--mode" => o.mode = Some(v),
                    "--dir" => o.dir = v,
                    _ => o.name = Some(v.trim().to_string()),
                }
            }
            t if o.target.is_empty() && !t.starts_with('-') => o.target = t.to_string(),
            _ => return fail("usage", USAGE),
        }
    }
    if o.target.is_empty() {
        return fail("usage", USAGE);
    }
    if TARGET_RE.fullmatch(&o.target).is_none() {
        return fail("usage", &format!("服务器写错了：{}", o.target));
    }
    if o.ssh_port.as_deref().is_some_and(|p| !p.bytes().all(|b| b.is_ascii_digit()) || !matches!(p.parse::<u32>(), Ok(1..=65535))) {
        return fail("usage", "SSH 端口要是 1～65535 的数字");
    }
    if let Some(p) = port {
        match p.parse::<i64>() {
            Ok(n @ 1024..=65535) if p.bytes().all(|b| b.is_ascii_digit()) => o.port = Some(n),
            _ => return fail("usage", "--port 要是 1024～65535 的端口"),
        }
    }
    if o.mode.as_deref().is_some_and(|m| m != "docker" && m != "systemd") {
        return fail("usage", "--mode 是 docker 或者 systemd");
    }
    o.dir = match dir_of(&o.dir) {
        Ok(d) => d,
        Err(why) => return fail("usage", why),
    };
    if o.host.as_deref().is_some_and(|h| HOST_RE.fullmatch(h).is_none()) {
        return fail("usage", "--host 要是 IP 或者域名");
    }
    if o.name.as_deref().is_some_and(|n| !name_ok(n)) {
        return fail("name", NAME_BAD);
    }
    Ok(o)
}

/// systemd 方式的服务看不到、写不了，或者重启就清掉的地方（unit 的 ProtectSystem=full、ProtectHome、PrivateTmp、PrivateDevices；
/// /run 是 tmpfs）：--dir 不能在它们下面。和安装脚本的 dir_check 一样
const DIR_HIDDEN: [&str; 17] = [
    "/usr", "/boot", "/efi", "/etc", "/home", "/root", "/run", "/var/run", "/tmp", "/var/tmp", "/dev", "/proc", "/sys", "/bin",
    "/sbin", "/lib", "/lib64",
];

/// --dir：/ 下面的一个目录（不是 /，没有空的、.、.. 这样的一节），只用字母、数字和 . _ - /，不在 DIR_HIDDEN 下面；末尾的 / 去掉。
/// 和安装脚本的 dir_check 一样
fn dir_of(d: &str) -> Result<String, &'static str> {
    let d = d.strip_suffix('/').unwrap_or(d);
    let ok = d.strip_prefix('/').is_some_and(|rest| rest.split('/').all(|p| !p.is_empty() && p != "." && p != ".."))
        && d.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b));
    if !ok {
        return Err("--dir 要是服务器上 / 下面的一个目录（绝对路径，只用字母、数字和 . _ - /）");
    }
    if DIR_HIDDEN.iter().any(|h| d.strip_prefix(h).is_some_and(|r| r.is_empty() || r.starts_with('/'))) {
        return Err("--dir 不能在 /usr、/etc、/home、/root、/run、/tmp 这些下面（systemd 方式的服务看不到、写不了那里）");
    }
    Ok(d.to_string())
}

/// py hub-deploy [用户@]主机 [选项]：经 SSH 把自己的同步服务器装到这台服务器上（或者更新），这台设备跟着加入。
/// 没加入时：新的入伙口令和盐；服务器上已经装了、有设备加入过的不动（taken），--reset 从头来（服务器忘掉所有设备）。
/// 已经加入了：只能是它加入的那一台（EXPECT：服务器上现在的是别的就 not_this），只更新，再重新加入一次（核对连得上）；
/// --reset 换一个新的加入链接重新加入（名字照旧）。服务器那边的进度直接打到 stderr（ssh 转过来的）。
/// 成了：stdout 是 `sh` 的 键=值（mode、state、port、dir、lan 地址只在内网、name 服务器上这台设备的名字、host 地址:端口、
/// members 几台设备、off 关掉了同步时的原因、newpin 证书换了；是的是 1，不是的是空的），bash eval 它；没成见 fail（原因码：usage、name、
/// ssh、root、deps、image、port、taken、not_this、moved、host、install、start；platform 加入着的是托管平台上的（--reset、--check 也不碰它）、
/// seed 不知道部署时配置的口令，以及加入的那些）
pub fn cmd_hub_deploy(args: &[String]) -> R<()> {
    let o = opts(args)?;
    let joined = hub_doc();
    if let Some(l) = joined.as_ref().map(link_of).filter(|l| l.pins.is_empty()) {
        return fail(
            "platform",
            &format!(
                "这台设备加入的是 {} 上托管平台的同步服务器（更新它用 cca hub hosted）：要部署到自己的服务器，先退出（cca hub leave）",
                host_of(&l.url)
            ),
        );
    }
    if o.check {
        return ssh_check(&o);
    }
    if let Some(l) = joined.as_ref().map(link_of).filter(|_| !o.reset) {
        let (host, port) = url_parts(&l.url);
        if o.host.as_deref().is_some_and(|h| h != host) || o.port.is_some_and(|p| p != port) {
            let at = host_of(&l.url);
            return fail(
                "moved",
                &format!("这台设备加入的是 {at}：地址、端口照加入链接里的。要换地址、端口，加 --reset（每台设备都要重新加入）"),
            );
        }
    }
    // 入伙口令和盐：加入着（不 --reset）就是那一份，只更新。服务器上配置的是部署时的那个口令（seed：换过加入链接的话服务器现在认的是
    // 另一个，配置照旧；安装脚本拿它核对是不是这一台：not_this），不知道就不部署；别的在这台设备上生成新的
    let seed = match &joined {
        Some(j) => match configured_admit(j)? {
            Some(s) => Some(s),
            None => return fail("seed", &format!("{SEED_UNKNOWN}；要从头部署，先退出（cca hub leave），再加 --reset 部署")),
        },
        None => None,
    };
    let joined = joined.and_then(|_| hub_doc()); // 问服务器时可能换上了别的设备换的口令
    let old = joined.as_ref().map(link_of);
    let (admit, salt) = match (&old, &seed) {
        (Some(l), Some(s)) if !o.reset => (s.clone(), l.salt.clone()),
        _ => (token_hex(16), token_hex(16)),
    };
    let r = run_installer(&o, &preamble(&o, &admit, seed.as_deref()))?;

    // 加入：加入着的照原来的链接重新加入（证书换了时换上新的指纹）；别的用新的链接
    let mut newpin = false;
    let link = match old {
        Some(mut l) if !o.reset => {
            if !l.pins.contains(&r.pin) {
                l.pins = vec![r.pin.clone()];
                newpin = true;
            }
            // 别的设备换过加入链接的话，记着的口令已经加入不了：先问一次设备，照换上的那个加入
            if let Some(a) = refresh_admit(&l.pins)? {
                l.admit = a;
            }
            l
        }
        old => new_link(&o, old.as_ref(), &r, admit, salt)?,
    };
    let name = o.name.clone().or_else(|| joined.as_ref().and_then(|j| j.str("name")).map(|n| n.to_string()));
    if o.reset && joined.is_some() {
        own_forget()?; // --reset：服务器已经忘掉了这台设备，本机记着的旧的也不要了（名字照旧用）
    }
    let lan = lan_only(&url_parts(&link.url).0);
    let mut joined = join_with(&link, name.clone())?;
    if let (Err(("denied", _)), Some(s), false) = (&joined, &seed, o.reset) {
        if *s != link.admit {
            // 服务器上的记录没了（数据目录删了……）：认的又是部署时的那个口令
            joined = join_with(&Link { admit: s.clone(), ..link.clone() }, name)?;
        }
    }
    let j = match joined {
        Ok(j) => j,
        Err((code, why)) => {
            let fw = if code == "unreachable" {
                format!(
                    "。云服务商的安全组（或者轻量应用服务器的防火墙）里要放行 TCP {}，放行以后再运行一次 cca hub deploy {}",
                    r.port, o.target
                )
            } else {
                String::new()
            };
            return fail(code, &format!("服务器上装好了（{}），但这台设备没加入成：{why}{fw}", host_of(&link.url)));
        }
    };
    let flag = |b: bool| if b { "1" } else { "" };
    let (port, members) = (r.port.to_string(), j.members.map_or(String::new(), |m| m.to_string()));
    sh(&[
        ("mode", r.mode.as_str()),
        ("state", r.state.as_str()),
        ("port", port.as_str()),
        ("dir", o.dir.as_str()),
        ("lan", flag(lan)),
        ("name", j.name.as_str()),
        ("host", j.host.as_str()),
        ("members", members.as_str()),
        ("off", j.off.as_str()),
        ("newpin", flag(newpin)),
    ]);
    Ok(())
}

/// 安装脚本前面放进的几项（经 ssh 的 stdin 给过去，不进命令行）：这一次的口令的哈希（WANT）、加入着的那一份部署时配置的口令的
/// 哈希（EXPECT：服务器的 .env 里的就是它）和参数
fn preamble(o: &Opts, admit: &str, seed: Option<&str>) -> String {
    let mut s = format!("CCA_HUB_MANAGED=1\nCCA_HUB_WANT={}\n", sq(&sha256_hex(admit)));
    if let Some(seed) = seed {
        s.push_str(&format!("CCA_HUB_EXPECT={}\n", sq(&sha256_hex(seed))));
    }
    s.push_str(&format!("set -- --dir {}", sq(&o.dir)));
    if let Some(p) = o.port {
        s.push_str(&format!(" --port {p}"));
    }
    if let Some(m) = &o.mode {
        s.push_str(&format!(" --{m}"));
    }
    if o.reset {
        s.push_str(" --reset");
    }
    s.push('\n');
    s
}

/// 单引号包起来（只给已经核对过样子的值用：里面没有单引号）
fn sq(v: &str) -> String {
    format!("'{v}'")
}

/// ssh 的那几个选项（-p、-i）。没有终端（--batch）：BatchMode，不问密码；--askpass：只用密码，经 SSH_ASKPASS 问一次（bash 的 askpass_run
/// 只给一次，问第二次读不到东西）。App 里（先 --check，再 --askpass）没人能答要不要认这台主机：第一次见的认下（accept-new，和在终端里
/// 答 yes 一样；要 OpenSSH 7.6），和记下的不一样时照旧不连（ssh_check 说是 ssh_hostkey），ssh 自己的提醒（记下了新主机）不打出来。
/// 命令行里没有终端（只有 --batch）的照旧：没见过的主机不连
fn ssh_argv(o: &Opts) -> Vec<String> {
    let mut v: Vec<String> = vec!["ssh".into(), "-o".into(), "ConnectTimeout=15".into()];
    let opt = |v: &mut Vec<String>, kv: &str| v.extend(["-o".into(), kv.into()]);
    if o.askpass {
        for kv in ["PubkeyAuthentication=no", "PreferredAuthentications=keyboard-interactive,password", "NumberOfPasswordPrompts=1"]
        {
            opt(&mut v, kv);
        }
    } else if o.batch {
        opt(&mut v, "BatchMode=yes");
    }
    if o.askpass || o.check {
        opt(&mut v, "StrictHostKeyChecking=accept-new");
        opt(&mut v, "LogLevel=ERROR");
    }
    if let Some(p) = &o.ssh_port {
        v.extend(["-p".into(), p.clone()]);
    }
    if let Some(k) = &o.identity {
        v.extend(["-i".into(), k.clone(), "-o".into(), "IdentitiesOnly=yes".into()]);
    }
    v
}

/// 经 SSH 在服务器上运行安装脚本，读它的结果。-T：不要伪终端（ssh 配置里写了 RequestTTY 也不要：脚本从 stdin 过去）；
/// ServerAliveInterval：拉镜像时好几分钟没有输出，别让路上的 NAT 把连接断了。整个最多等半个小时
fn run_installer(o: &Opts, preamble: &str) -> R<Report> {
    let mut argv = ssh_argv(o);
    argv.extend(["-T".into(), "-o".into(), "ServerAliveInterval=30".into(), o.target.clone(), REMOTE.into()]);
    let ran = Proc::new(argv).input(installer(preamble)).capture().stderr(Out::Inherit).timeout(1800.0).run();
    crate::py::signal::check()?;
    let done = match ran {
        Ok(d) => d,
        Err(ProcError::Timeout { .. }) => {
            return fail("install", "服务器上半个小时还没装完：停下了（在服务器上看看 docker、systemd 在做什么）")
        }
        Err(_) => return fail("ssh", "这台设备上没有 ssh"),
    };
    let r = report(&done.out());
    if let Some((code, why)) = &r.fail {
        return fail(fail_code(code), &fail_why(code, why, o));
    }
    if !r.ok {
        if done.rc == 255 {
            let hint = if o.batch && !o.askpass {
                format!("；要输密码的话，在终端里运行 {}（或者在 cca App 里部署）", o.cmd())
            } else {
                String::new()
            };
            // 部署到一半断了也是 255：服务起来以后才断的，服务器上已经是这一次的了（安装脚本不再换回），再运行时照它判断
            return fail("ssh", &format!("SSH 连不上 {}、登录不了，或者中途断了（上面是 ssh 说的）{hint}", o.target));
        }
        return fail("install", "服务器上没装完（上面是服务器那边的输出）");
    }
    if PRESENCE_PIN_RE.fullmatch(&r.pin).is_none() || !(1..=65535).contains(&r.port) {
        return fail("install", "服务器没说清楚装好的服务器用哪个端口、哪张证书");
    }
    Ok(r)
}

/// 安装脚本回来的结果（stdout 里 CCA-HUB 开头的那些行：安装脚本的 report_managed、die）
#[derive(Default, Debug, PartialEq)]
struct Report {
    mode: String,
    port: i64,
    /// Team 的服务器：远程会话的中转的数据端口（没开是 0；旧的安装脚本不说这一项）
    relay: i64,
    pin: String,
    state: String,
    ok: bool,
    fail: Option<(String, String)>,
}

fn report(stdout: &str) -> Report {
    let mut r = Report::default();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("CCA-HUB ") else { continue };
        let (k, v) = rest.split_once(' ').unwrap_or((rest, ""));
        match k {
            "mode" => r.mode = v.to_string(),
            "port" => r.port = v.parse().unwrap_or(0),
            "relay" => r.relay = v.parse().unwrap_or(0),
            "pin" => r.pin = v.to_string(),
            "state" => r.state = v.to_string(),
            "ok" => r.ok = true,
            "fail" => {
                let (c, why) = v.split_once(' ').unwrap_or((v, ""));
                r.fail = Some((c.to_string(), why.to_string()));
            }
            _ => {}
        }
    }
    r
}

/// 安装脚本说的原因码 → bash 的（认不出的当作 install）
fn fail_code(code: &str) -> &'static str {
    match code {
        "root" | "sudo" => "root",
        "deps" => "deps",
        "image" => "image",
        "port" => "port",
        "taken" => "taken",
        "not_this" | "none" => "not_this",
        "usage" => "usage",
        "start" => "start",
        "host" => "host",
        _ => "install",
    }
}

/// 给人看的原因：安装脚本说的，taken、not_this 再说怎么办（none：服务器上没装，这台设备加入的是别的）
fn fail_why(code: &str, why: &str, o: &Opts) -> String {
    let why = if why.is_empty() { "服务器上没装成" } else { why };
    let t = &o.target;
    if o.team && code != "sudo" {
        return why.to_string(); // Team 的：安装脚本说的就是全部（那个目录里装着别的：换 --dir）
    }
    match code {
        "taken" => format!(
            "{why}：在加入了的设备上运行 cca hub invite 拿加入链接，在这台设备上 cca hub join；要从头来（服务器忘掉所有设备，每台都要重新加入）：cca hub deploy {t} --reset"
        ),
        "none" => format!("{why}，这台设备加入的是别的服务器：要部署到这一台，先 cca hub leave"),
        "not_this" => format!(
            "{why}（这是另一台服务器，或者它被别的设备、在服务器上用 --reset 从头部署过）：这台设备先 cca hub leave，再 cca hub join 贴它现在的加入链接，或者 cca hub deploy {t} --reset 从头来"
        ),
        "sudo" => format!("{t} 不是 root，sudo 又要密码：用 root 登录（root@服务器），或者给这个用户免密码的 sudo"),
        _ => why.to_string(),
    }
}

/// Team 的同步服务器装好了（team_install）：各台设备该连的地址、证书公钥的指纹、怎么装的
pub struct TeamInstalled {
    pub url: String,
    pub pin: String,
    pub mode: String,
    pub state: String,
    pub port: i64,
    /// 远程会话的中转的数据端口（Team 的服务器给这个 Team 的会话做中转）；0 是没开
    pub relay: i64,
    pub dir: String,
    /// 地址是内网的：别的网络里的设备连不上
    pub lan: bool,
}

const TEAM_USAGE: &str = "用法: py team-deploy Team [用户@]主机 [--ssh-port 端口] [--identity 私钥] [--port 端口] [--host 地址] \
                          [--mode docker|systemd] [--dir 目录] [--batch] [--askpass] [--check]";

/// Team 的同步服务器的一键部署（cca team deploy：cmd/team/admin.rs 的 cmd_team_deploy 调）。args 是 hub-deploy 那一套参数（没有
/// --reset、--name）；root 是 Team 的根公钥（ssh-ed25519 AAAA…），svc 是服务名（cca-team-<编号的前 8 位>：一台服务器上每个 Team
/// 一个服务，目录默认 /opt/<服务名>）；old 是这个 Team 现在接着的地址和证书公钥的指纹。--check 只看登录得上不（返回 None）。
/// 装好了返回各台设备该连的地址：--host；没写时，装好的那台用的证书就是现在接着的那张（还是那台服务器：只是更新）就照原来的地址，
/// 不然是 ssh -G 的 hostname
pub fn team_install(args: &[String], root: &str, svc: &str, old: Option<(&str, &[String])>) -> R<Option<TeamInstalled>> {
    let given_dir = args.iter().any(|a| a == "--dir");
    let mut o = opts(args)?;
    if o.reset || o.name.is_some() {
        return fail("usage", TEAM_USAGE);
    }
    o.team = true;
    if !given_dir {
        o.dir = format!("/opt/{svc}");
    }
    if o.check {
        ssh_check(&o)?;
        return Ok(None);
    }
    let Some(key) = root.split(' ').nth(1).filter(|k| k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'))
    else {
        return fail("key", "这个 Team 的根公钥读不出来");
    };
    let mut pre = format!("CCA_HUB_MANAGED=1\nCCA_HUB_TEAM_ROOT={}\nset -- --dir {}", sq(key), sq(&o.dir));
    if let Some(p) = o.port {
        pre.push_str(&format!(" --port {p}"));
    }
    if let Some(m) = &o.mode {
        pre.push_str(&format!(" --{m}"));
    }
    pre.push('\n');
    let r = run_installer(&o, &pre)?;
    let host = match (&o.host, old) {
        (Some(h), _) => h.clone(),
        (None, Some((u, pins))) if pins.contains(&r.pin) => url_parts(u).0,
        _ => ssh_host(&o),
    };
    if HOST_RE.fullmatch(&host).is_none() {
        return fail("host", &format!("服务器装好了，但认不出各台设备该连的地址（{host}）：加 --host 公网IP或域名，再运行一次"));
    }
    let hp = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    Ok(Some(TeamInstalled {
        url: format!("https://{hp}:{}", r.port),
        pin: r.pin,
        mode: r.mode,
        state: r.state,
        port: r.port,
        relay: r.relay,
        dir: o.dir.clone(),
        lan: lan_only(&host),
    }))
}

/// 新的加入链接：地址是 --host，没写时 --reset 的照原来链接里的（安装脚本核对过就是那一台：EXPECT），再是 ssh -G 的 hostname
fn new_link(o: &Opts, old: Option<&Link>, r: &Report, admit: String, salt: String) -> R<Link> {
    let host = match (&o.host, old) {
        (Some(h), _) => h.clone(),
        (None, Some(l)) => url_parts(&l.url).0,
        (None, None) => ssh_host(o),
    };
    let hp = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let l = Link { url: format!("https://{hp}:{}", r.port), pins: vec![r.pin.clone()], admit, salt };
    if link_problem(&l.url, &l.pins, &l.admit, &l.salt).is_some() {
        return fail("host", &format!("服务器装好了，但认不出各台设备该连的地址（{host}）：加 --host 公网IP或域名，再运行一次"));
    }
    Ok(l)
}

/// --check：只看登录得上不（`ssh … true`，不运行安装脚本，什么都不动）。App 里部署前先查：成了退出码 0，没成见 ssh_trouble
fn ssh_check(o: &Opts) -> R<()> {
    let mut argv = ssh_argv(o);
    argv.extend(["-T".into(), o.target.clone(), "true".into()]);
    let ran = Proc::new(argv).stdin(In::Null).capture().stderr(Out::Capture).timeout(60.0).run();
    crate::py::signal::check()?;
    match ran {
        Ok(d) if d.rc == 0 => Ok(()),
        Ok(d) => {
            let (code, why) = ssh_trouble(&d.err(), o);
            fail(code, &why)
        }
        Err(ProcError::Timeout { .. }) => fail("ssh", &format!("SSH 一分钟还没登录上 {}", o.target)),
        Err(_) => fail("ssh", "这台设备上没有 ssh"),
    }
}

/// ssh 没登录上时说的（stderr）→ 原因码和给人看的原因：ssh_auth 这台设备的密钥登录不了、服务器认密码（bash 接着问密码）|
/// ssh_denied 密码不对（--askpass；不让 root 用密码登录的服务器，密码对了也是这样）| ssh_keyonly 服务器不让用密码、只认密钥（不认
/// 这台设备的）| ssh_hostkey 主机密钥和以前记下的不一样 | ssh 连不上（地址、端口不对，服务器没开机……：ssh 说的最后一句）。
/// Too many authentication failures：服务器只让试几次（MaxAuthTries），ssh-agent 里的密钥太多、还没轮到密码就断了：当作密钥登录不了
fn ssh_trouble(err: &str, o: &Opts) -> (&'static str, String) {
    let t = &o.target;
    if err.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") || err.contains("Host key verification failed") {
        return (
            "ssh_hostkey",
            format!(
                "{t} 的 SSH 主机密钥和这台设备以前记下的不一样（服务器重装过系统，或者有人在中间冒充它）：确认是重装过的话，\
                 在终端里运行 ssh-keygen -R 服务器地址 删掉记下的那一把，再部署"
            ),
        );
    }
    let methods = err.split_once("Permission denied (").map(|(_, rest)| rest.split(')').next().unwrap_or(""));
    let many = err.contains("Too many authentication failures");
    if methods.is_some() || many {
        let password = many || methods.is_some_and(|m| m.split(',').any(|m| m == "password" || m == "keyboard-interactive"));
        return match (o.askpass, password) {
            (true, true) => ("ssh_denied", "密码不对".into()),
            (false, true) => ("ssh_auth", format!("这台设备的 SSH 密钥登录不了 {t}，要用密码登录")),
            (true, false) => (
                "ssh_keyonly",
                format!(
                    "{t} 不让用密码登录，只认 SSH 密钥：把这台设备的公钥加进服务器的 ~/.ssh/authorized_keys；加过了的话，可能是这台设备的\
                     密钥太多、服务器只让试几把，在 ~/.ssh/config 里给它写上 IdentityFile 和 IdentitiesOnly yes"
                ),
            ),
            (false, false) => (
                "ssh_keyonly",
                format!(
                    "{t} 不认这台设备的 SSH 密钥，也不让用密码登录：把这台设备的公钥加进服务器的 ~/.ssh/authorized_keys，\
                     或者私钥有密码的话，在终端里运行 cca hub deploy（在那里输私钥的密码）"
                ),
            ),
        };
    }
    match err.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
        Some(last) => ("ssh", format!("SSH 连不上 {t}：{last}")),
        None => ("ssh", format!("SSH 连不上 {t}")),
    }
}

/// 服务器的地址（IPv6 不带方括号）：ssh 照它的配置连的那台（`ssh -G` 的 hostname：目标是 ssh 配置里的别名时是真的地址）；
/// ssh 太旧没有 -G 时是 @ 后面那段
fn ssh_host(o: &Opts) -> String {
    let mut argv = ssh_argv(o);
    argv.extend(["-G".into(), o.target.clone()]);
    let got = Proc::new(argv).stdin(In::Null).capture().stderr(Out::Null).timeout(15.0).run().ok().filter(|d| d.rc == 0);
    let host = got.and_then(|d| d.out().lines().find_map(|l| l.strip_prefix("hostname ").map(|h| h.trim().to_string())));
    let host = host.filter(|h| !h.is_empty()).unwrap_or_else(|| o.target.rsplit('@').next().unwrap_or("").to_string());
    host.trim_matches(['[', ']']).to_string()
}

/// 加入链接里的地址（IPv6 不带方括号）和端口（链接都核对过：认不出时是空的、0）
fn url_parts(url: &str) -> (String, i64) {
    let m = PRESENCE_URL_RE.match_(url);
    let host = m.as_ref().and_then(|m| m.name("host")).unwrap_or("").trim_matches(['[', ']']).to_string();
    (host, m.as_ref().and_then(|m| m.name("port")).and_then(text::to_int).unwrap_or(0))
}

/// 这个地址只在内网里连得上（私有地址、本机、链路本地、运营商级 NAT 的 100.64/10，IPv6 的 fc00::/7、fe80::/10）：别的设备出门在外就连不上
fn lan_only(host: &str) -> bool {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        let [a, b, ..] = ip.octets();
        ip.is_private() || ip.is_loopback() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
    } else if let Ok(ip) = host.parse::<Ipv6Addr>() {
        let s = ip.segments()[0];
        ip.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_embeds_server_once() {
        assert!(TEMPLATE.contains(EMBED), "模板里要有 {EMBED}");
        assert!(SERVER.ends_with('\n'));
        assert!(!SERVER.lines().any(|l| l == SERVER_EOF), "服务端程序里不能有 here-document 的结束行");
        let s = installer("CCA_HUB_MANAGED=1\n");
        assert!(!s.contains(EMBED));
        assert_eq!(s.matches(SERVER).count(), 1);
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("\nCCA_HUB_MANAGED=1\nhub_server_py() {\n"));
        assert!(s.trim_end().ends_with("main \"$@\""));
    }

    #[test]
    fn reads_the_report() {
        let r = report("noise\nCCA-HUB mode docker\nCCA-HUB port 18445\nCCA-HUB pin sha256//abc=\nCCA-HUB state new\nCCA-HUB ok\n");
        assert_eq!(
            (r.mode.as_str(), r.port, r.pin.as_str(), r.state.as_str(), r.ok),
            ("docker", 18445, "sha256//abc=", "new", true)
        );
        let r = report("CCA-HUB fail taken 这台服务器上已经有 自己的\n");
        assert_eq!(r.fail, Some(("taken".into(), "这台服务器上已经有 自己的".into())));
        assert!(!r.ok);
    }

    #[test]
    fn lan_addresses() {
        for h in [
            "10.1.2.3",
            "192.168.1.20",
            "172.16.0.1",
            "172.31.255.255",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(lan_only(h), "{h}");
        }
        for h in ["192.0.2.10", "172.32.0.1", "100.128.0.1", "example.com", "2001:db8::1", "1.2.3"] {
            assert!(!lan_only(h), "{h}");
        }
    }

    #[test]
    fn dirs() {
        for (d, want) in [
            ("/opt/cca-hub", "/opt/cca-hub"),
            ("/opt/cca-hub/", "/opt/cca-hub"),
            ("/srv/.hub", "/srv/.hub"),
            ("/a", "/a"),
            ("/var/lib/cca-hub", "/var/lib/cca-hub"),
            ("/homework", "/homework"),
            ("/usr2/hub", "/usr2/hub"),
        ] {
            assert_eq!(dir_of(d).as_deref(), Ok(want), "{d}");
        }
        for d in ["/", "//", "/.", "/./", "/a/..", "/a/../b", "/a//b", "/a/./b", "relative", "", "/a b", "/a/b//", "/opt/x'"] {
            assert!(dir_of(d).is_err_and(|why| why.contains("下面的一个目录")), "{d}");
        }
        for d in [
            "/home/me/hub",
            "/home",
            "/root/hub",
            "/etc/cca-hub",
            "/usr/local/hub",
            "/tmp/hub",
            "/var/tmp/hub",
            "/run/hub",
            "/var/run/x",
        ] {
            assert!(dir_of(d).is_err_and(|why| why.contains("systemd 方式的服务看不到")), "{d}");
        }
    }

    #[test]
    fn ssh_options() {
        let o = |batch, askpass| Opts {
            target: "root@192.0.2.10".into(),
            ssh_port: Some("2222".into()),
            batch,
            askpass,
            ..Default::default()
        };
        let s = |o: &Opts| ssh_argv(o).join(" ");
        assert_eq!(s(&o(false, false)), "ssh -o ConnectTimeout=15 -p 2222", "终端里：ssh 自己问密码、问要不要认这台主机");
        assert_eq!(s(&o(true, false)), "ssh -o ConnectTimeout=15 -o BatchMode=yes -p 2222", "命令行里没有终端：没见过的主机不连");
        assert_eq!(
            s(&Opts { check: true, ..o(true, false) }),
            "ssh -o ConnectTimeout=15 -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o LogLevel=ERROR -p 2222",
            "App 里先查登录：第一次见的主机认下"
        );
        let a = s(&o(true, true));
        assert!(!a.contains("BatchMode"), "BatchMode 连 SSH_ASKPASS 也不问");
        assert!(a.contains(
            "-o PubkeyAuthentication=no -o PreferredAuthentications=keyboard-interactive,password -o NumberOfPasswordPrompts=1"
        ));
        assert!(a.contains("StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn ssh_troubles() {
        let o = |askpass| Opts { target: "root@192.0.2.10".into(), batch: true, askpass, ..Default::default() };
        let code = |err: &str, askpass| ssh_trouble(err, &o(askpass)).0;
        let pw = "root@192.0.2.10: Permission denied (publickey,gssapi-keyex,gssapi-with-mic,password).\n";
        assert_eq!(code(pw, false), "ssh_auth");
        assert_eq!(code("root@192.0.2.10: Permission denied (publickey,keyboard-interactive).\n", false), "ssh_auth");
        assert_eq!(code("root@192.0.2.10: Permission denied (publickey).\n", false), "ssh_keyonly");
        assert_eq!(code(pw, true), "ssh_denied", "用了密码还不让进：密码不对");
        assert_eq!(code("root@192.0.2.10: Permission denied (publickey).\n", true), "ssh_keyonly", "没问密码：服务器不让用密码");
        let many = "Received disconnect from 192.0.2.10 port 22:2: Too many authentication failures\n";
        assert_eq!(code(many, false), "ssh_auth", "ssh-agent 里的密钥太多：还没轮到密码就断了，接着问密码（那时不用密钥）");
        assert_eq!(code(many, true), "ssh_denied");
        assert_eq!(code("Host key verification failed.\n", false), "ssh_hostkey");
        let (c, why) = ssh_trouble("\nssh: connect to host 192.0.2.10 port 22: Connection refused\n\n", &o(false));
        assert_eq!(
            (c, why.as_str()),
            ("ssh", "SSH 连不上 root@192.0.2.10：ssh: connect to host 192.0.2.10 port 22: Connection refused")
        );
        assert_eq!(ssh_trouble("", &o(false)), ("ssh", "SSH 连不上 root@192.0.2.10".into()));
    }

    #[test]
    fn targets_and_urls() {
        for t in ["root@192.0.2.10", "vps", "me@host.example.com", "root@[2001:db8::1]"] {
            assert!(TARGET_RE.fullmatch(t).is_some(), "{t}");
        }
        for t in ["-oProxyCommand=x", "root@host;rm", "a b", ""] {
            assert!(TARGET_RE.fullmatch(t).is_none(), "{t}");
        }
        assert_eq!(url_parts("https://192.0.2.10:18445"), ("192.0.2.10".into(), 18445));
        assert_eq!(url_parts("https://[2001:db8::1]:20000"), ("2001:db8::1".into(), 20000));
    }

    #[test]
    fn preamble_has_the_hashes_not_the_secrets() {
        let o = Opts { dir: "/srv/hub".into(), port: Some(20000), mode: Some("docker".into()), reset: true, ..Default::default() };
        let seed = "a".repeat(32);
        let p = preamble(&o, &"c".repeat(32), Some(&seed));
        assert_eq!(
            p,
            format!(
                "CCA_HUB_MANAGED=1\nCCA_HUB_WANT='{}'\nCCA_HUB_EXPECT='{}'\nset -- --dir '/srv/hub' --port 20000 --docker --reset\n",
                sha256_hex("c".repeat(32)),
                sha256_hex("a".repeat(32))
            )
        );
        assert!(!p.contains(&"a".repeat(32)) && !p.contains(&"c".repeat(32)));
    }
}
