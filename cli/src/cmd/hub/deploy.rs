//! Team 的同步服务器的一键部署（docs/dev/team.md「部署」）：
//! - hub-installer：安装脚本。模板 server/hub-install.sh 里标着 CCA-HUB-EMBED 的那一行换成版本、服务端程序 server/cca-presence.py、
//!   证书助手 server/cca-acme.py 和网页端的文件 server/web/（都编进辅助程序里：改了要重新编）。`cca team deploy --script` 打出来的、
//!   release.sh 放到发布页上的 cca-hub-install.sh 都是它：在服务器上用 root 运行（加 --team-root 这个 Team 的根公钥）
//! - `team_install`（cca team deploy 经 cmd/team/admin.rs 调）：经 SSH 在服务器上运行同一个脚本（从 stdin 给过去，前面放进根公钥和参数）

use super::fail;
use crate::cmd::cost::version_ok;
use crate::cmd::presence::{PRESENCE_PIN_RE, PRESENCE_URL_RE};
use crate::core::base::HOST_RE;
use crate::py::base64::b64encode;
use crate::py::hash::sha256_hex;
use crate::py::proc::{getenv, Cmd as Proc, In, Out, ProcError};
use crate::py::re::Re;
use crate::py::{text, Fail, R};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

const TEMPLATE: &str = include_str!("../../../../server/hub-install.sh");
pub(super) const SERVER: &str = include_str!("../../../../server/cca-presence.py");
const ACME: &str = include_str!("../../../../server/cca-acme.py");
/// 网页端的文件（server/web/ 里服务端给出去的那几种，和 xterm.js 的许可）：安装脚本里每个是一段 base64，写进 app/web/。
/// 目录里新加了文件要加在这里（测试照目录核对）
const WEB: [(&str, &[u8]); 14] = [
    ("app.css", include_bytes!("../../../../server/web/app.css")),
    ("app.js", include_bytes!("../../../../server/web/app.js")),
    ("core.js", include_bytes!("../../../../server/web/core.js")),
    ("icon-180.png", include_bytes!("../../../../server/web/icon-180.png")),
    ("icon-192.png", include_bytes!("../../../../server/web/icon-192.png")),
    ("icon-512.png", include_bytes!("../../../../server/web/icon-512.png")),
    ("index.html", include_bytes!("../../../../server/web/index.html")),
    ("logo.svg", include_bytes!("../../../../server/web/logo.svg")),
    ("manifest.webmanifest", include_bytes!("../../../../server/web/manifest.webmanifest")),
    ("xterm-addon-fit.js", include_bytes!("../../../../server/web/xterm-addon-fit.js")),
    ("xterm-LICENSE.txt", include_bytes!("../../../../server/web/xterm-LICENSE.txt")),
    ("xterm.css", include_bytes!("../../../../server/web/xterm.css")),
    ("xterm.js", include_bytes!("../../../../server/web/xterm.js")),
    ("README.md", include_bytes!("../../../../server/web/README.md")),
];
/// 模板里换成版本、服务端程序这些的那一行
const EMBED: &str = "# @CCA-HUB-EMBED@\n";
/// 服务端程序、证书助手、网页端的文件放在 here-document 里，这是它们的结束行（程序里不能有这一行：测试查；release.sh、regress 照它们
/// 取出服务端程序、证书助手核对）
const SERVER_EOF: &str = "CCA_HUB_SERVER_PY_EOF";
const ACME_EOF: &str = "CCA_HUB_ACME_PY_EOF";
const WEB_EOF: &str = "CCA_HUB_WEB_EOF";
/// 在服务器上运行脚本的命令：root 直接运行，别的用户经免密码的 sudo（要密码时不运行，说一声：没有终端可问）。
/// 外面再包一层 sh -c：登录 shell 不一定是 sh（fish、csh）
const REMOTE: &str = r#"sh -c 'if [ "$(id -u)" = 0 ]; then exec sh -s; elif sudo -n true 2>/dev/null; then exec sudo -n sh -s; else echo "CCA-HUB fail sudo"; exit 1; fi'"#;
const USAGE: &str = "用法: py team-deploy Team [用户@]主机 [--ssh-port 端口] [--identity 私钥] [--port 端口] [--host 地址] \
                     [--web-port 端口 | --no-web] [--mode docker|systemd] [--dir 目录] [--batch] [--askpass] [--check]";

/// SSH 的目标（[用户@]主机；ssh 配置里的别名也行）：不能以 - 开头（会被当成 ssh 的选项）
static TARGET_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[A-Za-z0-9_][A-Za-z0-9._@:%\[\]-]*$"));

/// 安装脚本：模板里 EMBED 那一行换成版本、给 team_install 的几项（preamble；直接在服务器上运行的那一份是空的）、服务端程序、证书助手
/// 和网页端的文件
pub fn installer(preamble: &str) -> String {
    let version = getenv("CCA_VERSION").filter(|v| version_ok(v)).unwrap_or_default();
    let embed = format!(
        "CCA_HUB_VERSION='{version}'\n{preamble}hub_server_py() {{\n\tcat <<'{SERVER_EOF}'\n{SERVER}{SERVER_EOF}\n}}\n\
         hub_acme_py() {{\n\tcat <<'{ACME_EOF}'\n{ACME}{ACME_EOF}\n}}\n{}",
        web_files()
    );
    TEMPLATE.replacen(EMBED, &embed, 1)
}

/// 安装脚本里的 hub_web_files：网页端的文件写进目录 $1，每个文件一段 base64（一行 64 个字符：openssl base64 -d 认的样子），
/// 安装脚本的 web_file 解开、核对 SHA-256
fn web_files() -> String {
    let mut s = String::from("hub_web_files() {\n");
    for (name, raw) in WEB {
        s.push_str(&format!("\tweb_file \"$1\" {name} {} <<'{WEB_EOF}' || return 1\n", sha256_hex(raw)));
        let b = b64encode(raw);
        for line in b.as_bytes().chunks(64) {
            s.push_str(std::str::from_utf8(line).unwrap_or_default());
            s.push('\n');
        }
        s.push_str(WEB_EOF);
        s.push('\n');
    }
    s.push_str("}\n");
    s
}

/// py hub-installer：把安装脚本打到 stdout（cca team deploy --script；release.sh 的 cca-hub-install.sh）
pub fn cmd_hub_installer(args: &[String]) -> R<()> {
    if !args.is_empty() {
        return Err(Fail::Die("用法: py hub-installer".into()));
    }
    crate::out_raw!("{}", installer(""));
    Ok(())
}

/// team_install 的参数
#[derive(Default)]
struct Opts {
    target: String,
    ssh_port: Option<String>,
    identity: Option<String>,
    port: Option<i64>,
    host: Option<String>,
    /// 手机的网页端：端口（--web-port）；Some(0) 是不开（--no-web）
    web: Option<i64>,
    mode: Option<String>,
    dir: String,
    /// 没有终端：ssh 不问密码、不问要不要认这台主机
    batch: bool,
    /// 密码经 SSH_ASKPASS 给，只问一次（bash 的 askpass_run）
    askpass: bool,
    /// 只看登录得上不，不部署（ssh_check）
    check: bool,
}

/// 读参数、核对：不对的都是 usage，还没连服务器。def_dir：没写 --dir 时装在哪
fn opts(args: &[String], def_dir: &str) -> R<Opts> {
    let mut o = Opts { dir: def_dir.into(), ..Default::default() };
    let (mut port, mut web) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--batch" => o.batch = true,
            "--askpass" => o.askpass = true,
            "--check" => o.check = true,
            "--no-web" if web.is_none() => web = Some(String::new()),
            f @ ("--ssh-port" | "--identity" | "--port" | "--host" | "--web-port" | "--mode" | "--dir") => {
                let Some(v) = it.next().cloned() else { return fail("usage", USAGE) };
                match f {
                    "--ssh-port" => o.ssh_port = Some(v),
                    "--identity" => o.identity = Some(v),
                    "--port" => port = Some(v),
                    "--web-port" if web.is_none() => web = Some(v),
                    "--web-port" => return fail("usage", "--web-port 和 --no-web 只能写一个"),
                    "--host" => o.host = Some(v),
                    "--mode" => o.mode = Some(v),
                    _ => o.dir = v,
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
    match web.as_deref() {
        None => {}
        Some("") => o.web = Some(0),
        Some(p) => match p.parse::<i64>() {
            Ok(n @ 1024..=65535) if p.bytes().all(|b| b.is_ascii_digit()) && o.port != Some(n) => o.web = Some(n),
            _ => return fail("usage", "--web-port 要是 1024～65535 的端口（不能和 --port 一样）"),
        },
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
                "；要输密码的话，在终端里运行 cca team deploy（或者在 cca App 里部署）".to_string()
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
    /// 手机的网页端的地址（https://地址:端口/；没开是空的）、证书拿到了没有（ok | wait）、没开的原因（port80 | lan | host | web_port）
    web: String,
    web_cert: String,
    web_off: String,
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
            "web" => r.web = v.to_string(),
            "web_cert" => r.web_cert = v.to_string(),
            "web_off" => r.web_off = v.to_string(),
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
        "usage" => "usage",
        "start" => "start",
        "host" => "host",
        _ => "install",
    }
}

/// 给人看的原因：安装脚本说的就是全部（那个目录里装着别的：换 --dir）；sudo 要密码的再说怎么办
fn fail_why(code: &str, why: &str, o: &Opts) -> String {
    match code {
        "sudo" => format!("{} 不是 root，sudo 又要密码：用 root 登录（root@服务器），或者给这个用户免密码的 sudo", o.target),
        _ if why.is_empty() => "服务器上没装成".to_string(),
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
    /// 手机的网页端（安装脚本的 report_managed）：地址（没开是空的）、证书（ok | wait）、没开的原因
    pub web: String,
    pub web_cert: String,
    pub web_off: String,
    pub dir: String,
    /// 地址是内网的：别的网络里的设备连不上
    pub lan: bool,
}

/// Team 的同步服务器的一键部署（cca team deploy：cmd/team/admin.rs 的 cmd_team_deploy 调）。args 是 Opts 那一套参数；root 是 Team 的
/// 根公钥（ssh-ed25519 AAAA…），svc 是服务名（cca-team-<编号的前 8 位>：一台服务器上每个 Team
/// 一个服务，目录默认 /opt/<服务名>）；old 是这个 Team 现在接着的地址和证书公钥的指纹。--check 只看登录得上不（返回 None）。
/// 装好了返回各台设备该连的地址：--host；没写时，装好的那台用的证书就是现在接着的那张（还是那台服务器：只是更新）就照原来的地址，
/// 不然是 ssh -G 的 hostname。手机的网页端（证书机构要验证的地址）在装之前就要定：--host，没写时是 ssh -G 的 hostname
pub fn team_install(args: &[String], root: &str, svc: &str, old: Option<(&str, &[String])>) -> R<Option<TeamInstalled>> {
    let o = opts(args, &format!("/opt/{svc}"))?;
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
    match o.web {
        Some(0) => pre.push_str(" --no-web"),
        Some(p) => pre.push_str(&format!(" --web-port {p}")),
        None => {}
    }
    let web_host = o.host.clone().unwrap_or_else(|| ssh_host(&o));
    if HOST_RE.fullmatch(&web_host).is_some() {
        pre.push_str(&format!(" --host {}", sq(&web_host)));
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
        web: r.web,
        web_cert: r.web_cert,
        web_off: r.web_off,
        dir: o.dir.clone(),
        lan: lan_only(&host),
    }))
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
                     或者私钥有密码的话，在终端里运行 cca team deploy（在那里输私钥的密码）"
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

/// 服务器的地址（https://主机:端口）里的主机（IPv6 不带方括号）和端口（认不出时是空的、0）
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
        for (src, eof) in [(SERVER, SERVER_EOF), (ACME, ACME_EOF)] {
            assert!(src.ends_with('\n'));
            assert!(!src.lines().any(|l| l == eof || l == WEB_EOF), "程序里不能有 here-document 的结束行");
        }
        let s = installer("CCA_HUB_MANAGED=1\n");
        assert!(!s.contains(EMBED));
        assert_eq!((s.matches(SERVER).count(), s.matches(ACME).count()), (1, 1));
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("\nCCA_HUB_MANAGED=1\nhub_server_py() {\n"));
        assert!(s.trim_end().ends_with("main \"$@\""));
        // 网页端的文件：一个一段，base64 解开和原来的一字节不差
        for (name, raw) in WEB {
            let head = format!("\tweb_file \"$1\" {name} {} <<'{WEB_EOF}' || return 1\n", sha256_hex(raw));
            let body =
                s.split(&head).nth(1).and_then(|r| r.split(&format!("\n{WEB_EOF}\n")).next()).unwrap_or_else(|| panic!("{name}"));
            assert!(body.lines().all(|l| l.len() <= 64), "{name}");
            assert_eq!(crate::py::base64::b64decode(&body.replace('\n', ""), true).ok().as_deref(), Some(raw), "{name}");
        }
    }

    #[test]
    fn web_files_are_all_there() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../server/web");
        let mut have: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        have.sort();
        let mut want: Vec<String> = WEB.iter().map(|(n, _)| n.to_string()).collect();
        want.sort();
        assert_eq!(have, want, "server/web/ 里新加、删掉了文件：WEB 跟着改");
        assert!(WEB.iter().all(|(n, _)| n.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))));
    }

    #[test]
    fn reads_the_report() {
        let r = report("noise\nCCA-HUB mode docker\nCCA-HUB port 18445\nCCA-HUB pin sha256//abc=\nCCA-HUB state new\nCCA-HUB ok\n");
        assert_eq!(
            (r.mode.as_str(), r.port, r.pin.as_str(), r.state.as_str(), r.ok, r.web.as_str()),
            ("docker", 18445, "sha256//abc=", "new", true, "")
        );
        let r = report("CCA-HUB web https://192.0.2.10:18447/\nCCA-HUB web_cert wait\nCCA-HUB ok\n");
        assert_eq!((r.web.as_str(), r.web_cert.as_str(), r.web_off.as_str()), ("https://192.0.2.10:18447/", "wait", ""));
        assert_eq!(report("CCA-HUB web_off port80\n").web_off, "port80");
        let r = report("CCA-HUB fail port 端口 18445 被别的程序占着\n");
        assert_eq!(r.fail, Some(("port".into(), "端口 18445 被别的程序占着".into())));
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
    fn web_options() {
        let o = |a: &[&str]| opts(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "/opt/cca-team-0123abcd").map(|o| o.web);
        assert_eq!(o(&["root@192.0.2.10"]).ok(), Some(None), "没写：照安装脚本（上次的，第一次是开着）");
        assert_eq!(o(&["root@192.0.2.10", "--no-web"]).ok(), Some(Some(0)));
        assert_eq!(o(&["root@192.0.2.10", "--web-port", "18448"]).ok(), Some(Some(18448)));
        for bad in [
            &["root@192.0.2.10", "--web-port", "80"][..],
            &["root@192.0.2.10", "--web-port", "x"],
            &["root@192.0.2.10", "--port", "18448", "--web-port", "18448"],
            &["root@192.0.2.10", "--no-web", "--web-port", "18448"],
            &["root@192.0.2.10", "--web-port", "18448", "--no-web"],
        ] {
            assert!(o(bad).is_err(), "{bad:?}");
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
}
