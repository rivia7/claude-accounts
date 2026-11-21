//! 部署到托管平台（docs/dev/hub.md「托管平台」）。Railway、Sealos 上的服务给的是 https 的公网地址：证书是平台的，设备照系统的证书机构
//! 验它（加入链接里没有指纹）；服务端程序照 CCA_HUB_HOSTED=1 运行（--behind-proxy，监听 8080，数据在挂在 /data 的存储里）。
//! - hub-hosted files railway|sealos [--dir 目录]：生成部署文件（默认 ~/cca-hub-<平台>）。入伙口令和盐在这台设备上生成，先记在
//!   ~/.cca/hub-hosted.json（600；加入成了删掉。再运行一次照旧用它：已经部署了的照样认）；部署文件里只有口令的 SHA-256。
//!   加入着托管平台上的那一台时是更新：哈希是部署时配置的那个口令的（seed，换过加入链接也不变）。哈希不对的话服务器会当成从头部署、
//!   丢掉所有设备，所以不知道 seed 时不生成
//! - hub-hosted join 地址 [--name 名字]：部署好以后平台给的地址，拿记着的口令和盐加入（join_with）
//! - hub-hosted platform：记着的平台（加入着的那一台、还没加入的那一份；都没有是空的）

use super::deploy::SERVER;
use super::{
    doc_hex, fail, host_of, hub_doc, join_with, link_of, members, name_ok, own_cfg, save_doc, Link, NAME_BAD, SEED_UNKNOWN,
};
use crate::cmd::cost::version_ok;
use crate::cmd::presence::url_ok;
use crate::core::base::{sh, CCA_HOME, HOME};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::sha256_hex;
use crate::py::obj::V;
use crate::py::proc::getenv;
use crate::py::rand::token_hex;
use crate::py::text::sh_quote;
use crate::py::time::time;
use crate::py::{path, Fail, R};
use crate::{dict, out};

const USAGE: &str = "用法: py hub-hosted files railway|sealos [--dir 目录] | join 地址 [--name 名字] | platform";

/// 生成的部署文件：目录里只能有这几个和以 . 开头的（访达的 .DS_Store、git……），别的不碰
const FILES: &[&str] = &["Dockerfile", "cca-presence.py", "README.md"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Platform {
    Railway,
    Sealos,
}

impl Platform {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "railway" => Some(Self::Railway),
            "sealos" => Some(Self::Sealos),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Railway => "railway",
            Self::Sealos => "sealos",
        }
    }
}

/// py hub-hosted …（见模块的说明）
pub fn cmd_hub_hosted(args: &[String]) -> R<()> {
    match args.split_first() {
        Some((sub, rest)) if sub == "files" => cmd_files(rest),
        Some((sub, rest)) if sub == "join" => cmd_join(rest),
        Some((sub, [])) if sub == "platform" => {
            let p = hub_doc()
                .and_then(|j| j.str("hosted").and_then(|p| Platform::parse(&p)))
                .or_else(|| pending().and_then(|p| p.platform));
            out!("{}", p.map_or("", Platform::name));
            Ok(())
        }
        _ => Err(Fail::Die(USAGE.into())),
    }
}

fn pending_path() -> String {
    path::join(&CCA_HOME, "hub-hosted.json")
}

/// 还没加入的那一份（~/.cca/hub-hosted.json）
struct Pending {
    admit: String,
    salt: String,
    platform: Option<Platform>,
}

fn pending() -> Option<Pending> {
    let j = load_quiet(&pending_path())?.as_dict()?;
    Some(Pending {
        admit: doc_hex(&j, "admit")?,
        salt: doc_hex(&j, "salt")?,
        platform: j.str("platform").and_then(|p| Platform::parse(&p)),
    })
}

/// files：部署文件写进目录，stdout 是给 bash eval 的几行：platform、dir、hash（口令的 SHA-256）、update（1：加入着，是更新）、
/// steps（这一次要做的：Railway 是一行一条的命令，Sealos 是表单里填什么）。没成见 fail（not_hosted：加入的是自己部署的那种；seed；dir）
fn cmd_files(args: &[String]) -> R<()> {
    let (platform, dir) = match args {
        [p] => (p, None),
        [p, flag, d] if flag == "--dir" && !d.is_empty() => (p, Some(d)),
        _ => return Err(Fail::Die(USAGE.into())),
    };
    let Some(platform) = Platform::parse(platform) else { return Err(Fail::Die(USAGE.into())) };
    let dir =
        dir.map_or_else(|| path::join(&HOME, &format!("cca-hub-{}", platform.name())), |d| path::abspath(&path::expanduser(d)));
    let (admit, update) = match hub_doc() {
        Some(j) => {
            let l = link_of(&j);
            if !l.pins.is_empty() {
                return fail(
                    "not_hosted",
                    &format!(
                        "这台设备加入的是 {} 上自己部署的同步服务器（更新它用 cca hub deploy）：要改用托管平台，先退出（cca hub leave）",
                        host_of(&l.url)
                    ),
                );
            }
            (update_seed()?, true)
        }
        None => (pending_admit(platform)?, false),
    };
    if let Err(why) = prepare_dir(&dir) {
        return fail("dir", &why);
    }
    let hash = sha256_hex(&admit);
    let ver = getenv("CCA_VERSION").filter(|v| version_ok(v)).map_or(String::new(), |v| format!(" {v}"));
    let mut files = vec![("cca-presence.py", SERVER.to_string()), ("README.md", readme(platform, &dir, &hash, &ver))];
    if platform == Platform::Railway {
        files.push(("Dockerfile", dockerfile(&hash, &ver)));
    }
    for (name, body) in &files {
        fs::put(&path::join(&dir, name), body.as_bytes(), 0o644)?;
    }
    let steps = steps(platform, &dir, &hash, update);
    sh(&[
        ("platform", platform.name()),
        ("dir", dir.as_str()),
        ("hash", hash.as_str()),
        ("update", if update { "1" } else { "" }),
        ("steps", steps.as_str()),
    ]);
    Ok(())
}

/// 更新时的口令：部署时配置的那个（哈希不对的话服务器会从头开始，所有设备都要重新加入）。平台上的配置看不到（经 SSH 的有安装脚本核对），
/// 所以先问一次服务器：别的设备换过链接的换上 seed，服务器的记录没了的先重新加入（own_recover）；服务器不认这台设备（被移除了、
/// 服务器从头开始过）时不生成——照这台设备记着的口令生成会让服务器又从头开始。连不上就照记着的；不知道部署时的口令也不生成（seed）
fn update_seed() -> R<String> {
    let asked = match own_cfg()? {
        (_, Some(cfg)) => members(&cfg)?,
        _ => Ok(vec![]),
    };
    let Some(j) = hub_doc() else { return fail("not_joined", &own_cfg()?.0) };
    if let Err(("denied", why)) = asked {
        return fail("denied", &format!("{why}。照这台设备记着的入伙口令生成部署文件会让服务器从头开始，所以先不生成"));
    }
    doc_hex(&j, "seed").map_or_else(|| fail("seed", SEED_UNKNOWN), Ok)
}

/// 还没加入：记着的那一份口令（没有就生成一份口令和盐，记下来）；平台照这一次的
fn pending_admit(platform: Platform) -> R<String> {
    let (admit, salt) = pending().map_or_else(|| (token_hex(16), token_hex(16)), |p| (p.admit, p.salt));
    let doc = dict! {
        "v" => 1, "admit" => admit.as_str(), "salt" => salt.as_str(), "platform" => platform.name(),
        "at" => time().trunc() as i64,
    };
    fs::makedirs_mode(&CCA_HOME, 0o700)?;
    save_json(&pending_path(), &V::Dict(doc), Some(0o600), true)?;
    Ok(admit)
}

/// 放部署文件的目录：没有就建；有的话里面只能是我们生成的（FILES）和以 . 开头的
fn prepare_dir(dir: &str) -> Result<(), String> {
    if path::lexists(dir) && !path::isdir(dir) {
        return Err(format!("{dir} 不是目录：换一个（--dir）"));
    }
    if path::isdir(dir) {
        let names = fs::listdir_sorted(dir).map_err(|e| format!("读不了 {dir}（{e}）"))?;
        if let Some(other) = names.iter().find(|n| !n.starts_with('.') && !FILES.contains(&n.as_str())) {
            return Err(format!("{dir} 里有别的文件（{other}）：换一个目录（--dir），或者先把它挪走"));
        }
    }
    fs::makedirs(dir).map_err(|e| format!("建不了 {dir}（{e}）"))
}

/// Railway 照它构建：服务端程序只用 Python 标准库；口令的哈希写在 ENV 里（只是哈希，拿它加入不了）
fn dockerfile(hash: &str, ver: &str) -> String {
    format!(
        "# cca{ver} 的 cca hub hosted railway 生成的：cca 自己的同步服务器（部署的步骤在 README.md）\n\
         FROM python:3.13-slim\n\
         COPY cca-presence.py /app/cca-presence.py\n\
         ENV CCA_HUB_HOSTED=1 CCA_HUB_ADMIT_HASH={hash}\n\
         EXPOSE 8080\n\
         CMD [\"python3\", \"-I\", \"/app/cca-presence.py\"]\n"
    )
}

/// 这一次要做的（bash 打出来、App 里显示）：Railway 是一行一条的命令（不带注释：zsh 交互时不认 #），Sealos 是表单里填什么
fn steps(p: Platform, dir: &str, hash: &str, update: bool) -> String {
    let cd = format!("cd {}", sh_quote(dir));
    match (p, update) {
        (Platform::Railway, false) => [
            cd.as_str(),
            "railway login",
            "railway init --name cca-hub",
            "railway add --service cca-hub",
            "railway volume add --mount-path /data",
            "railway up --detach",
            "railway domain --port 8080",
        ]
        .join("\n"),
        (Platform::Railway, true) => format!("{cd}\nrailway up --detach"),
        (Platform::Sealos, false) => format!(
            "镜像名称：python:3.13-slim\n\
             部署模式：固定实例，1 个\n\
             计算资源：最小的就够（0.1 核、128 MB 内存）\n\
             网络配置：容器端口 8080，打开公网访问（https）\n\
             启动命令：python3 -I /app/cca-presence.py\n\
             环境变量：CCA_HUB_HOSTED=1\n\
             环境变量：CCA_HUB_ADMIT_HASH={hash}\n\
             存储卷：挂载路径 /data，容量 1 Gi\n\
             配置文件：挂载路径 /app/cca-presence.py，内容是 {} 的全部内容",
            path::join(dir, "cca-presence.py")
        ),
        (Platform::Sealos, true) => format!(
            "在「应用管理」里打开这个应用，点「变更」\n\
             配置文件 /app/cca-presence.py 的内容换成 {} 的全部内容（新版的服务端程序）\n\
             再点「变更」：应用重启，数据在 /data 的存储卷里，设备不用重新加入",
            path::join(dir, "cca-presence.py")
        ),
    }
}

/// 部署文件里的 README.md：第一次部署、更新的步骤都在里面
fn readme(p: Platform, dir: &str, hash: &str, ver: &str) -> String {
    let first = steps(p, dir, hash, false);
    let again = steps(p, dir, hash, true);
    let name = p.name();
    match p {
        Platform::Railway => format!(
            "# cca 自己的同步服务器：部署到 Railway\n\n\
             这个目录是 cca{ver} 的 `cca hub hosted railway` 生成的：\n\n\
             - `Dockerfile`：照 python:3.13-slim 运行服务端程序。里面写着入伙口令的 SHA-256（`CCA_HUB_ADMIT_HASH`）：只是哈希，\
             拿它加入不了；加入链接只在生成这个目录的那台设备上\n\
             - `cca-presence.py`：服务端程序（只用 Python 标准库）\n\n\
             ## 第一次部署\n\n\
             要 Railway 的账号和命令行工具（`brew install railway`，或者 `npm i -g @railway/cli`）。在终端里一行一行地运行\
             （有的会打开浏览器、问问题）：\n\n```sh\n{first}\n```\n\n\
             - `railway add --service cca-hub` 新建一个空的服务，连上这个目录\n\
             - `railway volume add --mount-path /data` 给它挂存储：设备、记录、密文都存在 /data（没挂的话服务端程序不启动，\
             日志里说没有 /data）\n\
             - `railway up --detach` 照 Dockerfile 构建、部署，要一两分钟（`railway logs` 看日志）\n\
             - `railway domain --port 8080` 生成公网地址（https://….up.railway.app），指向服务端程序监听的 8080\n\n\
             最后把这个地址交给 cca：`cca hub hosted` 还在等的话填给它；关掉了的话再运行一次 `cca hub hosted {name}`\
             （它记得这一份口令，已经部署好的照样认）。\n\n\
             Railway 的命令行和网页时常改版，对不上的地方以 Railway 现在的文档为准（`railway <命令> --help`）。同样的设置在网页上也能做：\
             服务的存储（Volume，挂载路径 /data）、Settings → Networking → Generate Domain（端口 8080）。\n\n\
             ## 更新\n\n\
             新版 cca 带着新版的服务端程序。在加入着的设备上再运行一次 `cca hub hosted {name}`（口令的哈希照旧），然后：\n\n\
             ```sh\n{again}\n```\n\n\
             不是在部署它的那台设备上的话，先运行 `railway link`，选部署时的项目和 cca-hub 服务。数据在 /data 的存储里，\
             更新以后照旧，设备不用重新加入。\n\n\
             ## 费用\n\n按用量算（这个服务很小：几十 MB 内存），见 https://railway.com/pricing 。\n"
        ),
        Platform::Sealos => format!(
            "# cca 自己的同步服务器：部署到 Sealos\n\n\
             这个目录是 cca{ver} 的 `cca hub hosted sealos` 生成的：`cca-presence.py` 是服务端程序（只用 Python 标准库），\
             在 Sealos 上作为配置文件挂进容器。\n\n\
             ## 第一次部署\n\n\
             在 Sealos 的「应用管理」里「创建应用」，照下面填（界面以 Sealos 现在的为准；启动命令分成运行命令和命令参数两栏时，\
             分别填 `python3` 和 `-I /app/cca-presence.py`）：\n\n```\n{first}\n```\n\n\
             - 配置文件的内容：复制 `cca-presence.py` 的全部内容（Mac 上 `pbcopy < {}`），粘贴进去\n\
             - `CCA_HUB_ADMIT_HASH` 是入伙口令的 SHA-256：只是哈希，拿它加入不了；加入链接只在生成这个目录的那台设备上\n\
             - 没挂存储卷的话服务端程序不启动（日志里说没有 /data）：不然重启一次数据就全没了\n\n\
             部署好以后，把公网地址（https://…）交给 cca：`cca hub hosted` 还在等的话填给它；关掉了的话再运行一次 \
             `cca hub hosted {name}`（它记得这一份口令，已经部署好的照样认）。\n\n\
             ## 更新\n\n\
             在加入着的设备上再运行一次 `cca hub hosted {name}`（生成新版的 cca-presence.py，口令的哈希照旧），然后：\n\n\
             ```\n{again}\n```\n\n\
             ## 费用\n\n按用量算，见 Sealos 的价格页。\n",
            sh_quote(&path::join(dir, "cca-presence.py"))
        ),
    }
}

/// 平台给的地址 → 加入链接里的地址（https://主机:端口，没写端口是 443）。只认 https（平台的公网地址都是）、不带路径
fn hosted_url(raw: &str) -> Result<String, String> {
    const LIKE: &str = "平台给的公网地址只到主机名，像 Railway 的 https://名字.up.railway.app";
    let s = raw.trim();
    if s.is_empty() {
        return Err("没有输入地址".into());
    }
    let rest = match s.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
        Some(_) => return Err(format!("要 https 的地址：{LIKE}")),
        None => s,
    };
    let rest = rest.trim_end_matches('/');
    if rest.contains(['/', '?', '#', '@']) || rest.is_empty() {
        return Err(format!("只要地址，不带路径：{LIKE}"));
    }
    let has_port = if rest.starts_with('[') { rest.contains("]:") } else { rest.contains(':') };
    let url = if has_port { format!("https://{rest}") } else { format!("https://{rest}:443") };
    if !url_ok(&url) {
        return Err(format!("地址不对：{LIKE}"));
    }
    Ok(url)
}

/// join 地址 [--name 名字]：拿记着的口令和盐加入（链接里没有指纹：照证书机构验证书）。成了删掉记着的那一份，在 hub.json 里记下平台；
/// stdout 和 hub-join 一样。没成见 fail（url：地址不对；pending：没有记着的；别的同 hub-join）
fn cmd_join(args: &[String]) -> R<()> {
    let (raw, name) = match args {
        [u] => (u, None),
        [u, flag, n] if flag == "--name" => (u, Some(n.trim().to_string())),
        _ => return Err(Fail::Die(USAGE.into())),
    };
    if name.as_deref().is_some_and(|n| !name_ok(n)) {
        return fail("name", NAME_BAD);
    }
    let url = match hosted_url(raw) {
        Ok(u) => u,
        Err(why) => return fail("url", &why),
    };
    let Some(p) = pending() else {
        return fail("pending", "没有记着的部署：先生成部署文件（cca hub hosted railway 或者 sealos）");
    };
    match join_with(&Link { url, pins: vec![], admit: p.admit, salt: p.salt }, name)? {
        Ok(j) => {
            if let (Some(doc), Some(platform)) = (hub_doc(), p.platform) {
                doc.set("hosted", platform.name());
                save_doc(&doc)?;
            }
            let _ = std::fs::remove_file(pending_path());
            j.print();
            Ok(())
        }
        Err((code, why)) => fail(code, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_url_adds_the_port_and_refuses_paths() {
        let ok = |s: &str| hosted_url(s).unwrap();
        assert_eq!(ok("https://hub.example.com"), "https://hub.example.com:443");
        assert_eq!(ok(" hub.example.com/ "), "https://hub.example.com:443", "没写 https:// 也行");
        assert_eq!(ok("HTTPS://example.com:8443/"), "https://example.com:8443");
        assert_eq!(ok("https://[2001:db8::1]"), "https://[2001:db8::1]:443");
        for bad in [
            "",
            "http://example.com",
            "ftp://example.com",
            "https://example.com/v1/health",
            "https://u@example.com",
            "https://",
            "https://example.com:0",
            "https://example.com:70000",
        ] {
            assert!(hosted_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn steps_have_no_shell_comments_and_carry_the_hash() {
        let hash = "ab".repeat(32);
        let rw = steps(Platform::Railway, "/tmp/it's here", &hash, false);
        assert!(rw.lines().all(|l| !l.contains('#')), "zsh 交互时不认注释");
        assert!(rw.starts_with("cd '/tmp/it'\"'\"'s here'\n"), "{rw}");
        assert!(steps(Platform::Sealos, "/d", &hash, false).contains(&format!("CCA_HUB_ADMIT_HASH={hash}")));
        assert!(dockerfile(&hash, "").contains(&format!("CCA_HUB_ADMIT_HASH={hash}")));
    }

    #[test]
    fn the_server_has_the_hosted_preset() {
        // 平台上照 CCA_HUB_HOSTED=1 运行：生成的步骤、Dockerfile 靠的是服务端程序里的这一套（server/cca-presence.py）
        for want in ["CCA_HUB_HOSTED", "CCA_HUB_ADMIT_HASH", "0.0.0.0:8080", "/data/state.json", "--behind-proxy"] {
            assert!(SERVER.contains(want), "{want}");
        }
    }
}
