//! 一台假的 Linux 服务器（原来的 tests/hub_box.py）：给 Team 的同步服务器的一键部署用——hub_deploy 的那些用例，回归 11b3d 的
//! hubbox（`cca-test hub-box 目录`，tests/tool/hubbox.rs）。
//!
//! 目录下的 root/ 当作 /（安装脚本的 CCA_HUB_ROOT），bin/ 里是假的系统命令：id、useradd、userdel、chown、ip、journalctl、sudo、
//! 老的 docker-compose、python3 是几行 shell（下面的常量）；systemctl、docker 是这个小工具的角色（bin/ 里是 exec
//! `cca-test box-systemctl`、`cca-test box-docker` 的两行 sh）：照 unit 的 ExecStart、compose.yaml 里各个服务的 command、端口和
//! 挂载起真的服务端（监听 127.0.0.1）；证书助手不真的去找 Let's Encrypt，换成 `cca-test acme-stub`——几个 Team 共用的那一个照
//! --teams 里登记的地址一个地址写一张自签的证书、放进各个 Team 的目录，以前每个 Team 自己的那种照 --id 往 --out 里写一张；写过的
//! 记在 acme.issued。sh 是 /bin/dash（没有时 /bin/sh）。没有的 docker、systemctl 也放一个会失败的：不然会找到这台机器上真的。
//! 假命令的 #! 都是绝对路径（/bin/sh）：bin/ 排在 PATH 最前面，里面的 sh 是换过的。
//! enter 是假 ssh 的服务器那一头：在这台假服务器上运行一条命令（stdin 照样给它；目录下有 ssh_denied 时登录不了，它的内容是 once
//! 时只拒这一次）。
//!
//! 服务器上本来就有的：root/etc/ssl/certs/ca-certificates.crt（[`CA_BUNDLE`]：CA 证书，证书助手连 Let's Encrypt 要它；删掉它
//! 就是一台没装 ca-certificates 的服务器）。没有的：Python——bin/python3 谁调了就在 python3.log 里记一笔、失败（127），测试查
//! 这个文件一直是空的；镜像仓库——docker pull 一律失败（docker.log 里看得到有没有人拉过）。
//!
//! 目录下这些文件在时：uid（id -u 说的；root 是 0）、sudo_ok（免密码的 sudo）、ip（默认路由出去的源地址）、listening（ss 说这些
//! 端口有人监听）、import_fail（docker import 建不了镜像）、docker_down（docker info 不通）、no_compose（没有 compose 插件，只有
//! 老的 docker-compose 1.x）、compose_fail（docker compose up 起不来；--force-recreate 时原来的容器已经停了）、start_fail
//! （systemctl restart 起不来，原来的停了）、start_hang（systemctl restart 一直不回来）、acme_fail（证书助手要不到证书）。内容是 once
//! 的标志只管一次（[`Box::flag_once`]：出过一次状况就拿掉——换回原来的、重新起它的那一次照常）。安装脚本等证书最多
//! CCA_HUB_CERT_WAIT 秒（这里是 4）；any_ip 时内网地址也开网页端（CCA_HUB_ANY_IP）。
//!
//! 假命令记下的（测试照这些查）：systemctl.log、docker.log（每次的参数一行）、server.argv（起的每条命令一行）、server.log（起的
//! 程序的输出）、进程号文件 server.<服务名>.pid（以前的自己的同步服务器 cca-hub 是 server.pid；证书助手的服务名是 <服务名>-acme
//! 或者共用的 cca-team-acme；停的时候给整个进程组发 TERM）、images（docker import 建的镜像）、compose.projects（`compose -p` 的
//! 项目名）、compose.<项目名>.up（这个项目上次起的容器）、useradd.log、chown.log、sudo.log、python3.log；证书助手记 acme.argv、
//! acme.issued。
//!
//! 起的是哪个程序：只有一种——安装脚本放到服务器上的那一份 cca-core（`<目录>/app/cca-core`），原样运行，不换成别处的：
//! - unit 的 ExecStart 里 `/` 开头的词（连同第一个：要运行的程序）都接到 root/ 后面；compose 的 command 里每一个词（连同第一个）
//!   照 volumes 换成服务器上的路径。所以起的就是安装脚本解出来、核对过、换上去的那个文件。测试里它是这台机器上的辅助程序：生成
//!   安装脚本时用 CCA_HUB_CORE_X86_64、CCA_HUB_CORE_AARCH64 指的（真的 Linux 的那两份在 Mac 上运行不了）
//! - 第二个词是 team-server：同步服务器，照样运行；是 team-acme：证书助手，换成 `cca-test acme-stub 参数…`（先看那个程序在、
//!   能运行）。别的都起不了、算失败：以前的版本写的 `python3 -I …/cca-presence.py` 也一样，这台假服务器上没有 Python
//! - Docker 的容器用的镜像只能是安装脚本在本机建的那个空镜像（[`RUNTIME`]，和 server/hub-install.sh 的 RIMG 是约定）：
//!   `docker import - 名字` 读走 stdin（得是安装脚本给的那个空的 tar）、记下名字；`image inspect` 只认建过的；compose up 时服务
//!   写的不是它、它还没建，都起不来；`image rm` 在还有 compose 项目起着时不让删（真的 Docker 也是：有容器用着）
//! - 以前的版本装的、正在运行的服务（更新它、搬它的证书的用例要它在）由测试起一个顶替的：[`Box::stand_in`]
//!
//! 在测试里用：
//!
//! ```ignore
//! let b = Box::new(dir.join("box-1"), &Opts { docker: true, ..Opts::default() });   // Box(d, docker=True)
//! t.defer({ let b = b.clone(); move || b.stop(None) });                             // self.addCleanup(b.stop)
//! let r = b.install(&script, ["--team-root", root.as_str(), "--port", port.as_str()]); // b.install(script, …)
//! b.fake("ss", &format!("#!/bin/sh\n{SS}"));                                         // b.fake('ss', '#!/bin/sh\n' + SS)
//! b.flag("acme_fail", true);
//! assert_eq!(b.envfile(Some(&d))["CCA_HUB_MODE"], "docker");      // b.envfile(d)[…]
//! ```

use crate::json;
use crate::proc::{Cmd, Output, Session, Stdio};
use crate::py;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub use crate::fsx::free_port;

/// 编一次、记下来的正则（用到才编）
macro_rules! re {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::Regex::new($p).expect("正则写错了"))
    }};
}

/// 假服务器上的 sh：有 /bin/dash（Debian、Ubuntu 的 sh）就是它，没有时 /bin/sh
pub fn sh() -> &'static str {
    if Path::new("/bin/dash").exists() {
        "/bin/dash"
    } else {
        "/bin/sh"
    }
}

// ---- 假的系统命令（shell，前面接上 #!/bin/sh 放进 bin/）----

/// id：-u 说 uid 文件里的（没有是 0；经 sudo 的是 0），-g 说 useradd 记下的组，`id 用户` 看有没有这个用户
pub const ID: &str = r#"case "${1:-}" in
	-u) if [ -n "${HUB_SUDO:-}" ]; then echo 0; else cat "$HUB_BOX/uid" 2>/dev/null || echo 0; fi ;;
	-g) cat "$HUB_BOX/users/$2" 2>/dev/null ;;
	'') echo 'uid=0(root) gid=0(root) groups=0(root)' ;;
	*) [ -f "$HUB_BOX/users/$1" ] ;;
esac
"#;
/// useradd：系统用户的文件里记着它的组（id -g 说的；第几个建的，4201 起）
pub const USERADD: &str = r#"printf '%s\n' "$*" >> "$HUB_BOX/useradd.log"
for a in "$@"; do u=$a; done
echo $((4201 + $(ls "$HUB_BOX/users" | wc -l))) > "$HUB_BOX/users/$u"
"#;
/// userdel：删掉记着的系统用户
pub const USERDEL: &str = r#"rm -f "$HUB_BOX/users/$1"
"#;
/// sudo：sudo_ok 在时像免密码的 sudo（之后 id -u 是 0），不在时要密码（-n：不问，失败）。也免得测试里调到真的 sudo
pub const SUDO: &str = r#"printf '%s\n' "$*" >> "$HUB_BOX/sudo.log"
[ -f "$HUB_BOX/sudo_ok" ] || { echo 'sudo: a password is required' >&2; exit 1; }
[ "${1:-}" = -n ] && shift
HUB_SUDO=1 exec "$@"
"#;
/// ufw：开着、只放行了 22（`b.fake("ufw", …)` 才有）
pub const UFW: &str = r#"[ "${1:-}" = status ] && printf 'Status: active\n\nTo                         Action      From\n--                         ------      ----\n22/tcp                     ALLOW       Anywhere\n'
"#;
/// chown：只记一笔（假服务器上的用户、组都是假的）
pub const CHOWN: &str = r#"printf '%s\n' "$*" >> "$HUB_BOX/chown.log"
"#;
/// ip -4 route get 地址：默认路由出去的源地址
pub const IP: &str = r#"echo "192.0.2.1 via 10.9.0.1 dev eth0 src $(cat "$HUB_BOX/ip") uid 0"
"#;
/// journalctl：服务端输出的最后 20 行
pub const JOURNAL: &str = r#"tail -n 20 "$HUB_BOX/server.log" 2>/dev/null
"#;
/// ss -ltn：listening 里的端口有程序在监听（`b.fake("ss", …)` 才有）
pub const SS: &str = r#"echo 'State  Recv-Q Send-Q Local Address:Port Peer Address:Port'
while read -r p; do echo "LISTEN 0 4096 0.0.0.0:$p 0.0.0.0:*"; done < "$HUB_BOX/listening"
"#;
/// 老的 docker-compose 1.x：装着也不该用它（no_compose 的用例查它的记录是空的）
pub const COMPOSE_V1: &str = r#"printf '%s\n' "$*" >> "$HUB_BOX/docker-compose.log"
echo 'docker-compose version 1.29.2, build unknown'
"#;
/// python3：这台服务器上用不着它（服务端是安装脚本带着的 cca-core）。谁调了记一笔、失败：测试查 python3.log 一直是空的
pub const PYTHON3: &str = r#"printf '%s\n' "$*" >> "$HUB_BOX/python3.log"
exit 127
"#;
/// 这台假服务器上没有的命令（没有 systemd、Docker 时的 systemctl、docker）：整个脚本，带着 #!/bin/sh
pub const MISSING: &str = r#"#!/bin/sh
echo "$0: 这台假服务器上没有" >&2
exit 127
"#;

/// 这台服务器的 CA 证书在哪（root/ 下面；Debian、Ubuntu 放的地方，安装脚本的 ca_bundle 头一个看的就是它）。内容用不到：
/// 证书助手是假的，不连 Let's Encrypt
pub const CA_BUNDLE: &str = "etc/ssl/certs/ca-certificates.crt";
/// Docker 的容器用的镜像：安装脚本在本机建的空镜像（server/hub-install.sh 的 RIMG，compose.yaml 里写的也是它：一起改）
pub const RUNTIME: &str = "cca-team-runtime:1";
/// `docker import` 该收到的：一个空的 tar（结束块，10240 个 0 字节）
const EMPTY_TAR: usize = 10240;

/// 建假服务器时的选项（原来 `Box(d, docker=False, systemd=True, ip='203.0.113.7', any_ip=False)` 的那几个，默认值一样）
#[derive(Clone, Debug)]
pub struct Opts {
    /// 装着 Docker：假的 docker 照 compose.yaml 起服务；没装时 docker 是 [`MISSING`]
    pub docker: bool,
    /// 有 systemd：建 run/systemd/system、etc/systemd/system，假的 systemctl 照 unit 起服务；没有时 systemctl 是 [`MISSING`]
    pub systemd: bool,
    /// 默认路由出去的源地址（ip -4 route get 说的；安装脚本照它认这台服务器的地址）
    pub ip: String,
    /// 内网地址也开网页端（CCA_HUB_ANY_IP=1）
    pub any_ip: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { docker: false, systemd: true, ip: "203.0.113.7".to_string(), any_ip: false }
    }
}

/// 一台假的服务器（见模块开头）：root/ 当作 /，bin/ 里是假的系统命令（sh 是 [`sh()`]），enter 在它上面运行一条命令
#[derive(Clone, Debug)]
pub struct Box {
    /// 这台假服务器的目录：假命令的记录、标志文件、进程号文件都在这里
    pub d: PathBuf,
    /// 当作 / 的目录（CCA_HUB_ROOT）
    pub root: PathBuf,
    /// 假的系统命令（PATH 的第一项）
    pub bin: PathBuf,
    /// 以前的自己的同步服务器装在这里（root/opt/cca-hub；卸载它的用例）
    pub dir: PathBuf,
    /// 在这台假服务器上运行命令的环境，有先后（enter 照这个顺序写）
    pub env: Vec<(String, String)>,
    /// 假 ssh 的服务器那一头：`enter 命令`
    pub enter: PathBuf,
}

impl Box {
    /// 在 d 建一台假服务器（d/root、d/bin、d/users 不能已经在）；建不成就 panic（测试里用）
    pub fn new(d: impl AsRef<Path>, opts: &Opts) -> Box {
        let d = d.as_ref();
        Box::create(d, opts).unwrap_or_else(|e| panic!("建不了假服务器 {}：{e}", d.display()))
    }

    /// 同上，建不成返回错误（`cca-test hub-box` 用）。假的 systemctl、docker 是 exec cca-test 的两行 sh：cca-test 在哪照
    /// [`crate::proc::tool`]
    pub fn create(d: &Path, opts: &Opts) -> io::Result<Box> {
        let d = tidy(d);
        let (root, bin) = (d.join("root"), d.join("bin"));
        for p in [&root, &bin, &d.join("users")] {
            mkdir_new(p)?;
        }
        if opts.systemd {
            for p in ["run/systemd/system", "etc/systemd/system"] {
                mkdir_new(&root.join(p))?;
            }
        }
        let mut b = Box { d: d.clone(), dir: root.join("opt").join("cca-hub"), root, bin, env: Vec::new(), enter: d.join("enter") };
        for (name, script) in [
            ("id", ID),
            ("useradd", USERADD),
            ("userdel", USERDEL),
            ("chown", CHOWN),
            ("ip", IP),
            ("journalctl", JOURNAL),
            ("sudo", SUDO),
            ("docker-compose", COMPOSE_V1),
            ("python3", PYTHON3),
        ] {
            b.write_fake(name, &format!("#!/bin/sh\n{script}"))?;
        }
        // 这台服务器的 CA 证书（证书助手连 Let's Encrypt 要它：安装脚本照 ca_bundle 找，Docker 方式拷一份进容器）
        let ca = b.root.join(CA_BUNDLE);
        mkdir_new(ca.parent().unwrap_or(&b.root))?;
        fs::write(&ca, "# 假服务器的 CA 证书（内容用不到：证书助手是假的）\n")?;
        // 有的话是 exec cca-test 的角色（绝对路径：假服务器上的命令在别的目录里运行），没有的话是会失败的那一个
        let role = |on: bool, name: &str| -> io::Result<String> {
            if !on {
                return Ok(MISSING.to_string());
            }
            let tool = crate::proc::tool()?;
            let tool = fs::canonicalize(&tool).unwrap_or(tool);
            Ok(format!("#!/bin/sh\nexec {} {name} \"$@\"\n", quote(&tool.to_string_lossy())))
        };
        b.write_fake("systemctl", &role(opts.systemd, "box-systemctl")?)?;
        b.write_fake("docker", &role(opts.docker, "box-docker")?)?;
        std::os::unix::fs::symlink(sh(), b.bin.join("sh"))?;
        fs::write(d.join("ip"), &opts.ip)?;
        let text = |p: &Path| p.to_string_lossy().into_owned();
        b.env = vec![
            ("PATH".to_string(), format!("{}:/usr/bin:/bin", text(&b.bin))),
            ("CCA_HUB_ROOT".to_string(), text(&b.root)),
            ("HUB_BOX".to_string(), text(&d)),
            ("HOME".to_string(), text(&d)),
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ("CCA_HUB_CERT_WAIT".to_string(), "4".to_string()),
        ];
        if opts.any_ip {
            b.env.push(("CCA_HUB_ANY_IP".to_string(), "1".to_string()));
        }
        let assigns: Vec<String> = b.env.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
        let dd = text(&d);
        let enter = format!(
            r#"#!/bin/sh
if [ -f "{dd}/ssh_denied" ]; then
  [ "$(cat "{dd}/ssh_denied")" != once ] || rm -f "{dd}/ssh_denied"
  echo "Permission denied (publickey)." >&2; exit 255
fi
exec env -i {} /bin/sh -c "$1"
"#,
            assigns.join(" ")
        );
        fs::write(&b.enter, enter)?;
        fs::set_permissions(&b.enter, fs::Permissions::from_mode(0o755))?;
        Ok(b)
    }

    fn write_fake(&self, name: &str, text: &str) -> io::Result<()> {
        let p = self.bin.join(name);
        fs::write(&p, text)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755))
    }

    /// bin/ 里放（换掉）一个假命令：整个脚本，带着 #!
    pub fn fake(&self, name: &str, text: &str) {
        self.write_fake(name, text).unwrap_or_else(|e| panic!("写不了假命令 {name}：{e}"));
    }

    /// 放下一个只管一次的标志文件（内容是 once：出一次状况就自己拿掉；start_fail、start_hang、compose_fail 用得上）
    pub fn flag_once(&self, name: &str) {
        let p = self.d.join(name);
        fs::write(&p, "once\n").unwrap_or_else(|e| panic!("写不了标志文件 {}：{e}", p.display()));
    }

    /// 放下（on）或者拿掉目录下的一个标志文件（见模块开头的那几个）
    pub fn flag(&self, name: &str, on: bool) {
        let p = self.d.join(name);
        let r = if on {
            // Path.touch()：没有就建，有了改一下修改时间
            OpenOptions::new().create(true).append(true).open(&p).and_then(|f| f.set_modified(std::time::SystemTime::now()))
        } else if p.exists() {
            fs::remove_file(&p)
        } else {
            Ok(())
        };
        r.unwrap_or_else(|e| panic!("改不了标志文件 {}：{e}", p.display()));
    }

    /// 在这台假服务器上运行安装脚本：`sh 脚本 参数…`，sh 是 [`sh()`]，环境是 [`Box::env`]，stdin 是 /dev/null，新的会话
    /// （没有控制终端，像 cron、托管平台里那样：问不了公网地址），最多 180 秒。收齐输出返回；起不来、超时就 panic
    pub fn install<S: AsRef<OsStr>>(&self, script: impl AsRef<Path>, args: impl IntoIterator<Item = S>) -> Output {
        self.install_with(script, args, sh(), &[])
    }

    /// 同上，指定用哪个 sh，另加环境变量（cca team deploy 放进的那几项）
    pub fn install_with<S: AsRef<OsStr>>(
        &self,
        script: impl AsRef<Path>,
        args: impl IntoIterator<Item = S>,
        shell: &str,
        env: &[(&str, &str)],
    ) -> Output {
        let script = script.as_ref();
        Cmd::new(shell)
            .arg(script)
            .args(args)
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .envs(env.iter().copied())
            .stdin(Stdio::Null)
            .session(Session::New)
            .timeout(Duration::from_secs(180))
            .output()
            .unwrap_or_else(|e| panic!("在假服务器上运行 {} 不成：{e}", script.display()))
    }

    /// 目录下这个文件的内容（假命令的记录）；没有是空的
    pub fn log(&self, name: &str) -> String {
        fs::read(self.d.join(name)).map(|b| newlines(&String::from_utf8_lossy(&b))).unwrap_or_default()
    }

    /// 安装目录（None 是 [`Box::dir`]）下 .env 的各项（见 [`env_file`]）
    pub fn envfile(&self, d: Option<&Path>) -> BTreeMap<String, String> {
        let f = d.unwrap_or(&self.dir).join(".env");
        let text = fs::read(&f).unwrap_or_else(|e| panic!("读不了 {}：{e}", f.display()));
        env_file(&String::from_utf8_lossy(&text))
    }

    /// 这个服务的进程号文件（Team 的是 cca-team-<编号的前 8 位>，它的证书助手再加 -acme；以前的自己的同步服务器是 cca-hub）
    pub fn pidfile(&self, svc: &str) -> PathBuf {
        pid_file(&self.d, svc)
    }

    /// 这个服务在运行：进程号文件里的进程还在
    pub fn running(&self, svc: &str) -> bool {
        read_pid(&self.pidfile(svc)).is_some_and(alive)
    }

    /// 停掉这个服务（给它的进程组发 TERM，不等）；None 是目录下所有 server*.pid 的。测试收尾时调
    pub fn stop(&self, svc: Option<&str>) {
        let files = match svc {
            Some(svc) => vec![self.pidfile(svc)],
            None => fs::read_dir(&self.d)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| {
                            let n = e.file_name();
                            let n = n.to_string_lossy();
                            n.len() >= "server.pid".len() && n.starts_with("server") && n.ends_with(".pid")
                        })
                        .map(|e| e.path())
                        .collect()
                })
                .unwrap_or_default(),
        };
        for f in files {
            // 进程号不是正数的不发：killpg(0) 会发给自己这一组
            if let Some(pid) = read_pid(&f).filter(|pid| *pid > 0) {
                // SAFETY: 只是发一个信号；进程号是这台假服务器自己记下的、它起的服务
                unsafe { libc::killpg(pid, libc::SIGTERM) };
            }
        }
    }

    /// 服务器上（root/ 下）所有普通文件的内容接在一起（找口令、盐有没有落盘）；符号链接不算
    pub fn files_text(&self) -> String {
        let mut files = Vec::new();
        walk(&self.root, &mut files);
        files.sort();
        let texts: Vec<String> =
            files.iter().filter_map(|p| fs::read(p).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).collect();
        texts.join("\n")
    }

    /// 起一个进程顶替以前的版本装的、正在运行的服务（更新它、搬它的证书的用例要它在；那时候的程序是 Python 的，这台假服务器上
    /// 起不了）：argv 原样运行（第一个是这台机器上的程序），环境是这台假服务器的加上 env，输出进 server.log，进程号记在这个服务
    /// 的进程号文件里——之后 systemctl stop / restart / disable --now、docker compose up / down、[`Box::stop`] 照样停得掉它
    pub fn stand_in<S: AsRef<str>>(&self, svc: &str, argv: &[S], env: &[(&str, &str)]) {
        let argv: Vec<String> = argv.iter().map(|x| x.as_ref().to_string()).collect();
        let mut all = self.env.clone();
        all.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        stop_service(&self.d, svc)
            .and_then(|()| spawn_service(&self.d, &argv, false, &all, svc))
            .unwrap_or_else(|why| panic!("起不了顶替 {svc} 的进程：{why}"));
    }
}

/// 和 str(Path(d)) 一样：多余的斜杠、中间的 . 去掉
fn tidy(d: &Path) -> PathBuf {
    let parts: Vec<Component> = d.components().filter(|c| *c != Component::CurDir).collect();
    if parts.is_empty() {
        PathBuf::from(".")
    } else {
        parts.iter().collect()
    }
}

/// Path.mkdir(parents=True)：上面几层没有就建，它自己已经在了算错
fn mkdir_new(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} 已经在了", p.display())));
    }
    fs::create_dir_all(p)
}

/// 写进 sh 的一个值：单引号括起来（里面的单引号另写）
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// root/ 下的普通文件（不跟着符号链接走）
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&e.path(), out),
            Ok(t) if t.is_file() => out.push(e.path()),
            _ => {}
        }
    }
}

// ---- 几个假命令共用的（原来的 SERVE）----

/// 角色里出的错：说一行、退出码 1（原来的 Python 在这些地方抛异常或者 sys.exit(说明)，退出码也是 1）
type Fail = String;

fn finish(r: Result<i32, Fail>) -> i32 {
    match r {
        Ok(code) => code,
        Err(why) => {
            eprintln!("{why}");
            1
        }
    }
}

/// 环境里的一项；没有就失败（像 os.environ[...]）
fn need_env(key: &str) -> Result<String, Fail> {
    std::env::var(key).map_err(|_| format!("KeyError: '{key}'（假服务器上的命令要在它的环境里运行）"))
}

fn box_dir() -> Result<PathBuf, Fail> {
    need_env("HUB_BOX").map(PathBuf::from)
}

/// 标志文件在：这一回出这个状况。内容是 once 的只这一次（看到就拿掉）
fn flag_on(boxd: &Path, name: &str) -> bool {
    let p = boxd.join(name);
    match fs::read_to_string(&p) {
        Ok(s) if py::strip(&s) == "once" => {
            let _ = fs::remove_file(&p);
            true
        }
        Ok(_) => true,
        Err(_) => p.exists(),
    }
}

fn append(path: &Path, text: &str) -> Result<(), Fail> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("写不了 {}：{e}", path.display()))
}

/// 读成文本，换行照 Python 的文本模式（\r\n、\r 都当 \n）
fn read_text(path: &Path) -> Result<String, Fail> {
    fs::read(path).map(|b| newlines(&String::from_utf8_lossy(&b))).map_err(|e| format!("读不了 {}：{e}", path.display()))
}

fn newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// unit、.env 里 `名字=值` 的那几行（# 开头的、没有 = 的不算；同一个名字写了几次，算最后一次的）。原来的 env_file
pub fn env_file(text: &str) -> BTreeMap<String, String> {
    py::splitlines(text)
        .into_iter()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// 有先后的“字典”里放一项：已经有这个键就换掉值、不挪位置（和 Python 的 dict 一样）
fn put(map: &mut Vec<(String, String)>, key: &str, value: &str) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = value.to_string(),
        None => map.push((key.to_string(), value.to_string())),
    }
}

fn py_list(a: &[String]) -> String {
    format!("[{}]", a.iter().map(|s| py::repr_str(s)).collect::<Vec<_>>().join(", "))
}

fn pid_file(boxd: &Path, svc: &str) -> PathBuf {
    if svc == "cca-hub" {
        boxd.join("server.pid")
    } else {
        boxd.join(format!("server.{svc}.pid"))
    }
}

fn read_pid(f: &Path) -> Option<i32> {
    py::strip(&fs::read_to_string(f).ok()?).parse().ok()
}

/// 信号 0：这个进程还在（和 os.kill(pid, 0) 一样）
fn alive(pid: i32) -> bool {
    // SAFETY: 信号 0 不发信号，只问在不在
    unsafe { libc::kill(pid, 0) == 0 }
}

/// 原来的 running(svc)：进程号文件里的进程还在就是它的进程号
fn service_pid(boxd: &Path, svc: &str) -> Option<i32> {
    read_pid(&pid_file(boxd, svc)).filter(|pid| alive(*pid))
}

/// 在运行（进程号是 0 的不算：原来 Python 里 0 是假）
fn service_up(boxd: &Path, svc: &str) -> bool {
    service_pid(boxd, svc).is_some_and(|pid| pid != 0)
}

/// 停掉一个服务：给它的进程组发 TERM，等它没了（最多 4 秒），删掉进程号文件
fn stop_service(boxd: &Path, svc: &str) -> Result<(), Fail> {
    if let Some(pid) = service_pid(boxd, svc).filter(|pid| *pid != 0) {
        // SAFETY: 给这个服务的进程组发 TERM（起它的时候开了新会话，进程组号就是它的进程号）
        if unsafe { libc::killpg(pid, libc::SIGTERM) } != 0 {
            let e = io::Error::last_os_error();
            // 刚好自己退出了（信号 0 问得到、进程组却已经没了）：当它停了（原来的 Python 在这里会抛异常、这个假命令失败）
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("停不了 {svc}（进程组 {pid}）：{e}"));
            }
        }
        for _ in 0..200 {
            if service_pid(boxd, svc).is_none() {
                break;
            }
            let mut status = 0;
            // SAFETY: 不等（WNOHANG）；不是这个进程的子进程时返回 -1（ECHILD），不算错
            if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    match fs::remove_file(pid_file(boxd, svc)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(format!("删不了 {svc} 的进程号文件：{e}")),
        _ => Ok(()),
    }
}

/// 文件名是不是 cca-core（os.path.basename）
fn is_core(x: &str) -> bool {
    x.rsplit('/').next() == Some("cca-core")
}

/// `…/cca-core 子命令 …`（服务端、证书助手改成 Rust 以后的写法）：子命令
fn core_sub(argv: &[String]) -> Option<&str> {
    match argv {
        [first, sub, ..] if is_core(first) => Some(sub.as_str()),
        _ => None,
    }
}

fn tool_path() -> Result<String, Fail> {
    crate::proc::tool().map(|p| p.to_string_lossy().into_owned()).map_err(|e| e.to_string())
}

/// 这个文件在、能运行
fn runnable(p: &str) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// 要起的命令（路径已经换成这台假服务器上的）：只认安装脚本放上去的 cca-core（见模块开头「起的是哪个程序」）。team-server 照样
/// 运行；team-acme 换成假的证书助手，参数原样；别的不起
fn stage(svc: &str, argv: Vec<String>) -> Result<Vec<String>, Fail> {
    let cannot = |why: &str| format!("{svc}：{why}：{}", argv.join(" "));
    let Some(sub) = core_sub(&argv).map(str::to_string) else {
        return Err(cannot("这台假服务器只起安装脚本放上去的 app/cca-core（上面没有 Python）"));
    };
    if !runnable(&argv[0]) {
        return Err(cannot("要运行的程序不在，或者不能运行"));
    }
    match sub.as_str() {
        "team-server" => Ok(argv),
        // 证书助手：不真的去找 Let's Encrypt，换成假的
        "team-acme" => Ok([vec![tool_path()?, "acme-stub".to_string()], argv[2..].to_vec()].concat()),
        _ => Err(cannot("cca-core 的这条子命令不是服务")),
    }
}

/// 起一个服务（原来的 start）：先停掉同名的；argv 照 [`stage`] 看过、换过，环境是自己的（这台假服务器的）加上 overlay
fn start_service(boxd: &Path, argv: Vec<String>, overlay: &[(String, String)], svc: &str) -> Result<(), Fail> {
    stop_service(boxd, svc)?;
    let argv = stage(svc, argv)?;
    spawn_service(boxd, &argv, true, overlay, svc)
}

/// 起这条命令当这个服务：记进 server.argv；输出接到 server.log，stdin 是 /dev/null，新的会话（停的时候整组发 TERM）；环境是 env
/// （inherit 时先带上自己的）；进程号写进它的进程号文件
fn spawn_service(boxd: &Path, argv: &[String], inherit: bool, env: &[(String, String)], svc: &str) -> Result<(), Fail> {
    let Some((prog, args)) = argv.split_first() else { return Err(format!("{svc}：没有要运行的命令")) };
    let log = boxd.join("server.log");
    let out = OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("写不了 {}：{e}", log.display()))?;
    let err = out.try_clone().map_err(|e| format!("写不了 {}：{e}", log.display()))?;
    append(&boxd.join("server.argv"), &format!("{}\n", argv.join(" ")))?;
    let mut cmd = Cmd::new(prog);
    cmd.args(args);
    if inherit {
        cmd.env_inherit();
    }
    let child = cmd
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::Null)
        .stdout(out)
        .stderr(err)
        .session(Session::New)
        .spawn()
        .map_err(|e| format!("起不了 {svc}：{e}"))?;
    let pid = child.pid();
    // 它一直运行下去（像 systemd、docker 起的服务）：这个假命令退出时不停它，停它的是 stop_service（照进程号文件）
    std::mem::forget(child);
    fs::write(pid_file(boxd, svc), pid.to_string()).map_err(|e| format!("写不了 {svc} 的进程号文件：{e}"))
}

// ---- 假的 systemctl ----

/// 假的 systemctl（`cca-test box-systemctl 参数…`）：start / restart 照 unit 的 ExecStart 起服务（start_hang、start_fail 在时
/// 卡住、起不来），stop、disable --now 停掉，is-active --quiet 看在不在（退出码 0 / 3）；别的只记一笔
pub fn systemctl(args: &[String]) -> i32 {
    finish(systemctl_run(args))
}

fn systemctl_run(a: &[String]) -> Result<i32, Fail> {
    let boxd = box_dir()?;
    let root = need_env("CCA_HUB_ROOT")?;
    append(&boxd.join("systemctl.log"), &format!("{}\n", a.join(" ")))?;
    // 命令的最后一个参数是服务名（daemon-reload 没有：用不到）
    let svc = (if a.len() > 1 { a[a.len() - 1].as_str() } else { "cca-hub" }).replace(".service", "");
    let head: Vec<&str> = a.iter().take(2).map(String::as_str).collect();
    match head.as_slice() {
        ["restart" | "start", ..] => {
            if flag_on(&boxd, "start_hang") {
                std::thread::sleep(Duration::from_secs(60));
            }
            if flag_on(&boxd, "start_fail") {
                stop_service(&boxd, &svc)?;
                return Err(format!("Job for {svc}.service failed because the control process exited with error code."));
            }
            let kv = env_file(&read_text(Path::new(&format!("{root}/etc/systemd/system/{svc}.service")))?);
            let key = |k: &str| kv.get(k).cloned().ok_or_else(|| format!("KeyError: '{k}'（{svc}.service 里没有）"));
            let file_env = env_file(&read_text(Path::new(&format!("{root}{}", key("EnvironmentFile")?)))?);
            let mut argv: Vec<String> = py::split(&key("ExecStart")?).map(str::to_string).collect();
            // `/` 开头的都是服务器上的路径，连同第一个（要运行的程序：安装脚本放上去的那一份）：接到 root/ 后面
            for x in argv.iter_mut() {
                if x.starts_with('/') {
                    *x = format!("{root}{x}");
                }
            }
            let var = |name: &str| file_env.get(name).cloned().or_else(|| std::env::var(name).ok());
            let argv =
                argv.iter().map(|x| braces(x, &var).map(|x| x.replace("0.0.0.0", "127.0.0.1"))).collect::<Result<Vec<_>, _>>()?;
            let overlay: Vec<(String, String)> = file_env.into_iter().collect();
            start_service(&boxd, argv, &overlay, &svc)?;
        }
        ["disable", "--now"] | ["stop", ..] => stop_service(&boxd, &svc)?,
        ["is-active", "--quiet"] => return Ok(if service_up(&boxd, &svc) { 0 } else { 3 }),
        _ => {}
    }
    Ok(0)
}

/// ExecStart 里的 ${名字}：换成环境（.env 的、自己的）里的值，没有就失败
fn braces(x: &str, var: &dyn Fn(&str) -> Option<String>) -> Result<String, Fail> {
    let mut missing = None;
    let out = re!(r"\$\{(\w+)\}").replace_all(x, |c: &regex::Captures| {
        var(&c[1]).unwrap_or_else(|| {
            missing.get_or_insert_with(|| c[1].to_string());
            String::new()
        })
    });
    match missing {
        Some(name) => Err(format!("KeyError: '{name}'")),
        None => Ok(out.into_owned()),
    }
}

// ---- 假的 docker ----

/// compose.yaml 里各个服务的那一段：services: 下面缩进两格的名字开头，接着的缩进四格的那几行（原来的 services；和
/// server/hub-install.sh 写出来的样子是约定，改了两边一起看）
pub fn services(y: &str) -> Vec<String> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in py::splitlines(y) {
        if re!(r"^  [\w.-]+:$").is_match(line) {
            out.push(Vec::new());
        } else if let Some(block) = out.last_mut() {
            if line.starts_with("    ") {
                block.push(line);
            }
        }
    }
    out.into_iter().map(|b| b.join("\n")).collect()
}

/// compose 的 ${名字}、${名字:-默认}、${名字:?说明}，值从 env（.env 的各项）里取：:- 的没有值（或者是空的）用默认的，
/// 别的没有值就失败（原来的 interp）
pub fn interp(x: &str, env: &BTreeMap<String, String>) -> Result<String, String> {
    let mut missing = None;
    let out = re!(r"\$\{(\w+)(?::([-?])([^}]*))?\}").replace_all(x, |c: &regex::Captures| {
        let name = &c[1];
        if c.get(2).map(|m| m.as_str()) == Some("-") {
            return env
                .get(name)
                .filter(|v| !v.is_empty())
                .cloned()
                .unwrap_or_else(|| c.get(3).map_or("", |m| m.as_str()).to_string());
        }
        env.get(name).cloned().unwrap_or_else(|| {
            missing.get_or_insert_with(|| name.to_string());
            String::new()
        })
    });
    match missing {
        Some(name) => Err(format!("KeyError: '{name}'")),
        None => Ok(out.into_owned()),
    }
}

/// 一段服务里的 container_name
fn container_name(block: &str) -> Result<String, Fail> {
    re!(r"(?m)^    container_name: (\S+)$")
        .captures(block)
        .map(|c| c[1].to_string())
        .ok_or_else(|| format!("compose.yaml 里这个服务没写 container_name：\n{block}"))
}

/// 容器里监听的 0.0.0.0:端口 换成外面对着的 127.0.0.1:端口（ports 里没有这个端口是 0）。和 Python 的 re 一样，末尾的一个换行不碍事
fn to_local(x: &str, ports: &[(String, String)]) -> String {
    let (body, nl) = match x.strip_suffix('\n') {
        Some(body) => (body, "\n"),
        None => (x, ""),
    };
    match re!(r"^0\.0\.0\.0:(\d+)$").captures(body) {
        Some(c) => format!("127.0.0.1:{}{nl}", ports.iter().find(|(k, _)| *k == c[1]).map_or("0", |(_, v)| v.as_str())),
        None => x.to_string(),
    }
}

/// 假的 docker（`cca-test box-docker 参数…`）：compose up 照 compose.yaml 起各个服务、down 停掉；import 建镜像、image inspect /
/// image rm 看的是建过的那些（见模块开头），pull 一律失败；inspect、info、compose version、compose logs 照标志文件回答
pub fn docker(args: &[String]) -> i32 {
    finish(docker_run(args))
}

fn docker_run(args: &[String]) -> Result<i32, Fail> {
    let boxd = box_dir()?;
    append(&boxd.join("docker.log"), &format!("{}\n", args.join(" ")))?;
    let images = boxd.join("images");
    let cwd = std::env::current_dir().map_err(|e| format!("不知道当前目录：{e}"))?;
    // compose 的项目名：没写 -p 时是目录名
    let mut proj = cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut a = args.to_vec();
    let third = |a: &[String]| a.get(2).cloned().ok_or_else(|| "IndexError: list index out of range".to_string());
    if a.first().map(String::as_str) == Some("compose") {
        if flag_on(&boxd, "no_compose") {
            return Err("docker: 'compose' is not a docker command.".to_string());
        }
        if a.get(1).map(String::as_str) == Some("-p") {
            // Team 的：项目名照服务名（两个 Team 的目录同名时不互相当成孤儿删掉）。记下来给测试看
            let p = third(&a)?;
            append(&boxd.join("compose.projects"), &format!("{p}\n"))?;
            proj = p;
            a.drain(1..3);
        }
        if a.get(1).map(String::as_str) == Some("-f") {
            // 安装脚本总是写 -f compose.yaml（不认目录里别的 compose 文件）
            if third(&a)? != "compose.yaml" {
                return Err(format!("AssertionError: {}", py_list(&a)));
            }
            a.drain(1..3);
        }
    }
    let s: Vec<&str> = a.iter().map(String::as_str).collect();
    match s.as_slice() {
        ["compose", "version", ..] => println!("Docker Compose version v2.29.7"),
        ["info"] => return Ok(if flag_on(&boxd, "docker_down") { 1 } else { 0 }),
        // 这台假服务器连不上镜像仓库（安装脚本也不该去拉：docker.log 里有这一行就是拉过）
        ["pull", ..] => return Err(format!("Error response from daemon: pull access denied for {}", s[s.len() - 1])),
        // docker import - 名字：stdin 是一个 tar，建成镜像。安装脚本给的是空的 tar；读完再说成不成（不然前面的 dd 写不进来）
        ["import", "-", tag] => {
            let mut tar = Vec::new();
            io::stdin().read_to_end(&mut tar).map_err(|e| format!("读不了 stdin：{e}"))?;
            if flag_on(&boxd, "import_fail") {
                return Err(
                    "Error response from daemon: write /var/lib/docker/tmp/docker-import: no space left on device".to_string()
                );
            }
            if tar.len() != EMPTY_TAR || tar.iter().any(|b| *b != 0) {
                return Err(format!("AssertionError: docker import 收到的不是空的 tar（{} 字节）", tar.len()));
            }
            if !has_image(&images, tag) {
                append(&images, &format!("{tag}\n"))?;
            }
            println!("sha256:{}", "0".repeat(64));
        }
        ["image", "inspect", name] => return Ok(if has_image(&images, name) { 0 } else { 1 }),
        // 还有容器用着的镜像删不了（起着的 compose 项目用的都是 RUNTIME：compose_up 查过）
        ["image", "rm", name] => {
            if !has_image(&images, name) {
                return Err(format!("Error response from daemon: No such image: {name}"));
            }
            if *name == RUNTIME && !up_projects(&boxd).is_empty() {
                return Err(format!(
                    "Error response from daemon: conflict: unable to remove repository reference {name:?} (must force) - container is using its \
                     referenced image（{}）",
                    up_projects(&boxd).join("、")
                ));
            }
            let rest: String = py::split(&read_text(&images)?).filter(|i| i != name).map(|i| format!("{i}\n")).collect();
            fs::write(&images, rest).map_err(|e| format!("写不了 {}：{e}", images.display()))?;
            println!("Untagged: {name}");
        }
        // docker inspect -f 格式 容器名
        ["inspect", ..] => println!("{}", if service_up(&boxd, s[s.len() - 1]) { "true" } else { "false" }),
        ["compose", "up", ..] => compose_up(&boxd, &a, &proj, &cwd)?,
        ["compose", "down", ..] => {
            let y = if Path::new("compose.yaml").exists() { read_text(Path::new("compose.yaml"))? } else { String::new() };
            let mut blocks = services(&y);
            if blocks.is_empty() {
                blocks.push("    container_name: cca-hub".to_string());
            }
            for block in blocks {
                stop_service(&boxd, &container_name(&block)?)?;
            }
            let up = boxd.join(format!("compose.{proj}.up"));
            if up.exists() {
                fs::remove_file(&up).map_err(|e| format!("删不了 {}：{e}", up.display()))?;
            }
        }
        ["compose", "logs", ..] => {
            let log = boxd.join("server.log");
            let text = if log.exists() { read_text(&log)? } else { String::new() };
            io::stdout().write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        _ => {}
    }
    Ok(0)
}

/// 这个镜像在本机（docker import 建过）
fn has_image(images: &Path, name: &str) -> bool {
    fs::read(images).is_ok_and(|b| py::split(&String::from_utf8_lossy(&b)).any(|i| i == name))
}

/// 起着的 compose 项目（目录下的 compose.<项目名>.up）
fn up_projects(boxd: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(boxd) else { return Vec::new() };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| Some(e.file_name().to_string_lossy().strip_prefix("compose.")?.strip_suffix(".up")?.to_string()))
        .collect();
    out.sort();
    out
}

/// docker compose up：照 compose.yaml 起各个服务（命令、环境、挂载、端口照 compose 的规矩换成这台假服务器上的），
/// 查镜像是本机建的那个空镜像、命令和 ports、挂载对得上（和 server/hub-install.sh 写的 compose.yaml 是约定）；--remove-orphans 时
/// 这个项目上次起的、这次没有了的停掉（网页端关了，证书助手就没了）
fn compose_up(boxd: &Path, a: &[String], proj: &str, cwd: &Path) -> Result<(), Fail> {
    let y = read_text(Path::new("compose.yaml"))?;
    if flag_on(boxd, "compose_fail") {
        // 真的 compose 是先删掉原来的容器、建新的，起的时候才出错：--force-recreate 时原来的容器已经停了
        if a.iter().any(|x| x == "--force-recreate") {
            for block in services(&y) {
                stop_service(boxd, &container_name(&block)?)?;
            }
        }
        return Err("Error response from daemon: driver failed programming external connectivity on endpoint cca-team".to_string());
    }
    let env = env_file(&read_text(Path::new(".env"))?);
    let up = boxd.join(format!("compose.{proj}.up"));
    let before: Vec<String> = if up.exists() { py::split(&read_text(&up)?).map(str::to_string).collect() } else { Vec::new() };
    let mut names = Vec::new();
    for block in services(&y) {
        let svc = container_name(&block)?;
        names.push(svc.clone());
        // 镜像：只能是安装脚本在本机建的那个空镜像（写的是别的，真的 Docker 会去拉：这台服务器连不上镜像仓库）
        let image = re!(r"(?m)^    image: (\S+)$").captures(&block).ok_or_else(|| format!("compose.yaml 里 {svc} 没写 image"))?;
        if &image[1] != RUNTIME {
            return Err(format!("AssertionError: {svc} 的镜像是 {}，该是本机建的 {RUNTIME}", &image[1]));
        }
        if !has_image(&boxd.join("images"), RUNTIME) {
            return Err(format!(
                "Error response from daemon: pull access denied for {RUNTIME}, repository does not exist（本机还没建这个镜像）"
            ));
        }
        // 交给容器的环境变量（Team 的根公钥）：compose.yaml 里写着「名字: ${名字:?}」，值在 .env 里
        let mut given = Vec::new();
        for c in re!(r"(?m)^      (\w+): \$\{(\w+):\?\}$").captures_iter(&block) {
            let v = env.get(&c[2]).ok_or_else(|| format!("KeyError: '{}'", &c[2]))?;
            put(&mut given, &c[1], v);
        }
        let cmd =
            re!(r"(?m)^    command: (\[.*\])$").captures(&block).ok_or_else(|| format!("compose.yaml 里 {svc} 没写 command"))?;
        let cmd = json::parse(&cmd[1]).map_err(|e| format!("{svc} 的 command 不是 JSON：{e}"))?;
        let cmd = cmd.as_list().ok_or_else(|| format!("{svc} 的 command 不是列表"))?;
        // 挂载：./目录 是 compose.yaml 旁边的，/绝对路径 是这台假服务器上的（root/ 下面）
        let mut vols = Vec::new();
        for c in re!(r"(?m)^      - (\./\w+|/[\w./-]+):(/\w+)(?::|$)").captures_iter(&block) {
            let h = &c[1];
            let host = match h.strip_prefix("./") {
                Some(rest) if cwd == Path::new("/") => format!("/{rest}"),
                Some(rest) => format!("{}/{rest}", cwd.display()),
                None => format!("{}{h}", need_env("CCA_HUB_ROOT")?),
            };
            put(&mut vols, &c[2], &host);
        }
        // 每一个词（连同第一个：容器里要运行的程序）照挂载换成这台假服务器上的路径：起的就是安装脚本放在 app/ 里的那一份
        let mut argv = Vec::new();
        for x in cmd.iter() {
            let x = x.as_str().ok_or_else(|| format!("{svc} 的 command 里有不是字符串的：{}", x.compact()))?;
            let mapped = vols
                .iter()
                .find(|(c, _)| x == c.as_str() || x.starts_with(&format!("{c}/")))
                .map(|(c, h)| format!("{h}{}", &x[c.len()..]));
            argv.push(mapped.unwrap_or_else(|| x.to_string()));
        }
        // compose 做的两件事：${名字:?} 换成 .env 里的值；容器里的端口经 ports 那几行对到外面的
        let argv = argv.iter().map(|x| interp(x, &env)).collect::<Result<Vec<_>, _>>()?;
        let mut ports = Vec::new();
        for c in re!(r#"(?m)^      - "([^"]+):(\d+)"$"#).captures_iter(&block) {
            put(&mut ports, &c[2], &interp(&c[1], &env)?);
        }
        let argv: Vec<String> = argv.iter().map(|x| to_local(x, &ports)).collect();
        let joined = argv.join(" ");
        let has = |k: &str| argv.iter().any(|x| x == k);
        let port = |k: &str| ports.iter().any(|(c, _)| c == k);
        let same = |xs: &[bool]| xs.windows(2).all(|w| w[0] == w[1]);
        if core_sub(&argv) == Some("team-server") {
            let relay = [has("--relay-listen"), env.get("CCA_HUB_RELAY").map_or("off", String::as_str) != "off", port("18446")];
            if !same(&relay) {
                return Err("AssertionError: 开着中转：命令里有那两个参数、ports 里有数据端口；没开：都没有".to_string());
            }
            let web = [has("--web-listen"), env.contains_key("CCA_HUB_WEB_URL"), port("18447"), block.contains(":/webcert:ro,z")];
            if !same(&web) {
                return Err("AssertionError: 开着网页端：命令里有那几个参数、ports 里有它的端口、只读挂着共用的证书助手放给它的证书；没开：都没有"
                    .to_string());
            }
            // 推送：开着中转、.env 里有推送网关时 environment 里交给服务端那两个（systemd 的经 EnvironmentFile 自己就有）；不然都没有
            let push = [
                block.contains("CCA_PUSH_URL: ${CCA_PUSH_URL:?}") && block.contains("CCA_PUSH_PIN: ${CCA_PUSH_PIN:?}"),
                relay[1] && env.contains_key("CCA_PUSH_URL") && env.contains_key("CCA_PUSH_PIN"),
            ];
            if !same(&push) {
                return Err("AssertionError: 开着推送：environment 里有 CCA_PUSH_URL、CCA_PUSH_PIN；没开：都没有".to_string());
            }
        } else if core_sub(&argv) == Some("team-acme") {
            if !(ports.len() == 1 && ports[0].0 == "18080" && ports[0].1 == "80") {
                return Err("AssertionError: 证书助手：80 端口对到容器里的 18080".to_string());
            }
            // 容器是空的，里面没有 CA 证书：--ca 要指着挂进去的那一份（安装脚本拷进 app/ 的）
            let ca = argv.iter().position(|x| x == "--ca").and_then(|i| argv.get(i + 1));
            let app = vols.iter().find(|(c, _)| c == "/app").map(|(_, h)| format!("{h}/"));
            let mounted = |p: &String| app.as_ref().is_some_and(|app| p.starts_with(app.as_str()));
            if !ca.is_some_and(|p| mounted(p) && fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0)) {
                return Err(format!(
                    "AssertionError: 证书助手（Docker）：--ca 要指着挂进容器的 CA 证书（app/ca.pem），文件要在：{joined}"
                ));
            }
        } else {
            return Err(format!("AssertionError: {svc} 的 command 不是 app/cca-core team-server / team-acme：{joined}"));
        }
        start_service(boxd, argv, &given, &svc)?;
    }
    if a.iter().any(|x| x == "--remove-orphans") {
        for old in before.iter().filter(|old| !names.contains(old)) {
            stop_service(boxd, old)?;
        }
    }
    fs::write(&up, names.join(" ")).map_err(|e| format!("写不了 {}：{e}", up.display()))
}

// ---- 假的证书助手 ----

/// 假的证书助手（`cca-test acme-stub 参数…`，参数照真的 `cca-core team-acme`；多出来的 --ca、--listen 不看）：参数记进 acme.argv；不听端口、不联网，一直运行
/// （像真的那样等着续）。几个 Team 共用的（--teams、--store、--out）：照登记的地址一个地址一张自签的证书，存在
/// --store/<地址>/（手里没有、Team 的目录里有就接着用它：以前它自己的证书助手要的），放进 --out/<服务名>/（目录不在不放：
/// 安装脚本建）；登记的变了马上照新的。以前每个 Team 自己的（--id、--out）：照 --id 写一张（有了不再写）。写过的地址记在
/// acme.issued；acme_fail 在时什么都不写
pub fn acme_stub(args: &[String]) -> i32 {
    finish(acme_run(args))
}

const CERT_FILES: [&str; 2] = ["privkey.pem", "fullchain.pem"];

fn acme_run(a: &[String]) -> Result<i32, Fail> {
    let boxd = box_dir()?;
    append(&boxd.join("acme.argv"), &format!("{}\n", a.join(" ")))?;
    let opt = |k: &str| -> Result<PathBuf, Fail> {
        let i = a.iter().position(|x| x == k).ok_or_else(|| format!("ValueError: {} is not in list", py::repr_str(k)))?;
        a.get(i + 1).map(PathBuf::from).ok_or_else(|| "IndexError: list index out of range".to_string())
    };
    if !opt("--account")?.is_dir() {
        return Err(format!("AssertionError: {}", py_list(a)));
    }
    if a.iter().any(|x| x == "--id") {
        let out = opt("--out")?;
        if !out.join("fullchain.pem").exists() {
            issue(&boxd, &out, &opt("--id")?.to_string_lossy())?;
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    let (teams, store, out) = (opt("--teams")?, opt("--store")?, opt("--out")?);
    loop {
        let rd = fs::read_dir(&teams).map_err(|e| format!("读不了 {}：{e}", teams.display()))?;
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for t in names {
            if t.contains('.') {
                continue; // 安装脚本正写到一半的
            }
            let ident = fs::read_to_string(teams.join(&t)).map_err(|e| format!("读不了 {}：{e}", teams.join(&t).display()))?;
            let ident = py::strip(&ident);
            let (s, d) = (store.join(ident.replace(':', "_")), out.join(&t));
            fs::create_dir_all(&s).map_err(|e| format!("建不了 {}：{e}", s.display()))?;
            if !s.join("fullchain.pem").exists() {
                if d.join("fullchain.pem").exists() {
                    for f in CERT_FILES {
                        copy(&d.join(f), &s.join(f))?;
                    }
                } else {
                    issue(&boxd, &s, ident)?;
                }
            }
            if d.is_dir() && s.join("fullchain.pem").exists() {
                for f in CERT_FILES {
                    if !same_file(&s.join(f), &d.join(f)) {
                        let tmp = d.join(format!("{f}.tmp"));
                        copy(&s.join(f), &tmp)?;
                        fs::rename(&tmp, d.join(f)).map_err(|e| format!("换不了 {}：{e}", d.join(f).display()))?;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 写一张自签的证书（P-256，7 天）到 d/privkey.pem、d/fullchain.pem，记进 acme.issued
fn issue(boxd: &Path, d: &Path, cn: &str) -> Result<(), Fail> {
    if flag_on(boxd, "acme_fail") {
        return Ok(());
    }
    let key = d.join("privkey.pem").to_string_lossy().into_owned();
    let crt = d.join("fullchain.pem").to_string_lossy().into_owned();
    let (key_tmp, crt_tmp) = (format!("{key}.tmp"), format!("{crt}.tmp"));
    openssl(&["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", &key_tmp])?;
    openssl(&["req", "-new", "-x509", "-key", &key_tmp, "-out", &crt_tmp, "-days", "7", "-subj", &format!("/CN={cn}")])?;
    for (from, to) in [(&key_tmp, &key), (&crt_tmp, &crt)] {
        fs::rename(from, to).map_err(|e| format!("换不了 {to}：{e}"))?;
    }
    append(&boxd.join("acme.issued"), &format!("{cn}\n"))
}

/// 照这台假服务器的 PATH 找 openssl（和原来一样），失败了这个证书助手就退出
fn openssl(args: &[&str]) -> Result<(), Fail> {
    let what = format!("openssl {}", args.join(" "));
    let out = Cmd::new("openssl").args(args).env_inherit().output().map_err(|e| format!("{what}：{e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} 失败（{}）：{}", out.status, out.err()))
    }
}

/// shutil.copy：内容和权限（不带修改时间）
fn copy(src: &Path, dst: &Path) -> Result<(), Fail> {
    let data = fs::read(src).map_err(|e| format!("读不了 {}：{e}", src.display()))?;
    let mode = fs::metadata(src).map_err(|e| format!("读不了 {}：{e}", src.display()))?.permissions().mode() & 0o7777;
    fs::write(dst, data).map_err(|e| format!("写不了 {}：{e}", dst.display()))?;
    fs::set_permissions(dst, fs::Permissions::from_mode(mode)).map_err(|e| format!("改不了 {} 的权限：{e}", dst.display()))
}

/// 两个文件都在、内容一样
fn same_file(x: &Path, y: &Path) -> bool {
    x.exists() && y.exists() && fs::read(x).ok() == fs::read(y).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn services_cut_by_two_space_names() {
        let y = "services:\n  cca-team:\n    image: x\n    command: [\"a\"]\n\n  web.1-x:\n    container_name: w\n      - \"1:2\"\n  bad name:\n    no: 1\nvolumes:\n";
        assert_eq!(
            services(y),
            vec!["    image: x\n    command: [\"a\"]".to_string(), "    container_name: w\n      - \"1:2\"\n    no: 1".to_string()]
        );
        assert!(services("x:\n    a: 1\n").is_empty(), "顶格的不是服务");
        // splitlines 的分行：\r\n、\r 都算
        assert_eq!(services("  a:\r\n    x\r    y"), vec!["    x\n    y".to_string()]);
    }

    #[test]
    fn interp_like_compose() {
        let e = env(&[("A", "1"), ("E", "")]);
        assert_eq!(interp("x${A}y${A:?说明}", &e).unwrap(), "x1y1");
        assert_eq!(interp("${B:-0.0.0.0}:${E:-默认}:${A:-9}", &e).unwrap(), "0.0.0.0:默认:1");
        assert_eq!(interp("${B:-}|${B}x", &e).unwrap_err(), "KeyError: 'B'");
        assert_eq!(interp("${B:?要写}", &e).unwrap_err(), "KeyError: 'B'");
        assert_eq!(interp("$A ${ A} ${A:x}", &e).unwrap(), "$A ${ A} ${A:x}", "认不出的原样留着");
    }

    #[test]
    fn env_file_like_dict_of_pairs() {
        let e = env_file("# c=1\nA=1\nB==2\nnoeq\nA=3\r\n C= x \n");
        assert_eq!(e, env(&[("A", "3"), ("B", "=2"), (" C", " x ")]));
    }

    #[test]
    fn ports_to_local() {
        let ports = vec![("8080".to_string(), "18443".to_string())];
        assert_eq!(to_local("0.0.0.0:8080", &ports), "127.0.0.1:18443");
        assert_eq!(to_local("0.0.0.0:9", &ports), "127.0.0.1:0");
        assert_eq!(to_local("0.0.0.0:8080\n", &ports), "127.0.0.1:18443\n");
        assert_eq!(to_local("x0.0.0.0:8080", &ports), "x0.0.0.0:8080");
    }

    #[test]
    fn repr_for_messages() {
        assert_eq!(py_list(&["compose".to_string(), "it's".to_string(), "a\nb".to_string()]), r#"['compose', "it's", 'a\nb']"#);
        assert_eq!(quote("/a b/it's"), r"'/a b/it'\''s'");
        assert!(is_core("/opt/x/cca-core") && is_core("cca-core") && !is_core("/opt/cca-core/") && !is_core("cca-core.py"));
    }
}
