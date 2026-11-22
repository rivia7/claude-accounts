//! Team 的同步服务器部署到托管平台（cca team hosted，docs/dev/team.md）。Railway、Sealos 上的服务给的是 https 的公网地址：证书是
//! 平台的，设备照系统的证书机构验它（接上时没有指纹）；服务端程序照 CCA_HUB_HOSTED=1 运行（--behind-proxy，监听 8080，数据在挂在
//! /data 的存储里），有 CCA_TEAM_ROOT（Team 的根公钥）就是这个 Team 的服务器。这里只生成部署文件和要做的步骤（`team_files`）

use super::deploy::SERVER;
use crate::cmd::cost::version_ok;
use crate::core::base::HOME;
use crate::py::fs;
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{path, R};

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

/// Team 的同步服务器部署到托管平台的文件（cca team hosted：cmd/team/admin.rs 的 cmd_team_hosted 调）。目录（没写是 ~/<服务名>-<平台>）
/// 里生成服务端程序和 README.md，Railway 另有 Dockerfile。平台上只有 Team 的根公钥（公开的）：环境变量 CCA_TEAM_ROOT 写它
/// ssh-ed25519 后面那一段 key（服务端有了它、又是 CCA_HUB_HOSTED=1，就是 Team 的服务器）。update：这个 Team 已经接着托管平台上的
/// 那一台，这一次只是换上新版的服务端程序。返回（目录, 这一次要做的）；平台不认识、目录里有别的东西是（原因码, 原因）
pub fn team_files(
    platform: &str,
    dir: Option<&str>,
    key: &str,
    svc: &str,
    name: &str,
    update: bool,
) -> R<Result<(String, String), (String, String)>> {
    let Some(p) = Platform::parse(platform) else {
        return Ok(Err(("usage".into(), "平台写 railway 或者 sealos".into())));
    };
    let dir = dir.map_or_else(|| path::join(&HOME, &format!("{svc}-{}", p.name())), |d| path::abspath(&path::expanduser(d)));
    if let Err(why) = prepare_dir(&dir) {
        return Ok(Err(("dir".into(), why)));
    }
    let ver = getenv("CCA_VERSION").filter(|v| version_ok(v)).map_or(String::new(), |v| format!(" {v}"));
    let first = team_steps(p, &dir, key, svc, false);
    let again = team_steps(p, &dir, key, svc, true);
    let readme = format!(
        "# Team「{name}」的同步服务器：部署到 {}\n\n\
         这个目录是 cca{ver} 的 `cca team hosted` 生成的：`cca-presence.py` 是服务端程序（只用 Python 标准库）{}。\
         平台上只有这个 Team 的根公钥（环境变量 `CCA_TEAM_ROOT`：它是公开的，不用保密）；设备、邀请、每台设备的配置都由管理员的设备管，\
         服务器上存的是密文。\n\n\
         ## 第一次部署\n\n```\n{first}\n```\n\n\
         没挂存储（/data）的话服务端程序不启动：不然重启一次数据就全没了。界面、命令以平台现在的为准。\n\n\
         部署好以后，在管理员的设备上把平台给的公网地址（https://…）接上：\n\n```sh\ncca team attach {name} https://平台给的地址\n```\n\n\
         ## 更新\n\n\
         新版 cca 带着新版的服务端程序。在管理员的设备上再运行一次 `cca team hosted`，然后：\n\n```\n{again}\n```\n\n\
         数据在 /data 的存储里，更新以后照旧，设备不用重新加入。\n",
        if p == Platform::Railway { "Railway" } else { "Sealos" },
        if p == Platform::Railway { "，`Dockerfile` 照 python:3.13-slim 运行它" } else { "，在 Sealos 上作为配置文件挂进容器" },
    );
    let mut files = vec![("cca-presence.py", SERVER.to_string()), ("README.md", readme)];
    if p == Platform::Railway {
        files.push((
            "Dockerfile",
            format!(
                "# cca{ver} 的 cca team hosted railway 生成的：Team 的同步服务器（部署的步骤在 README.md）\n\
                 FROM python:3.13-slim\n\
                 COPY cca-presence.py /app/cca-presence.py\n\
                 ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n\
                 EXPOSE 8080\n\
                 CMD [\"python3\", \"-I\", \"/app/cca-presence.py\"]\n"
            ),
        ));
    }
    for (file, body) in &files {
        fs::put(&path::join(&dir, file), body.as_bytes(), 0o644)?;
    }
    Ok(Ok((dir.clone(), team_steps(p, &dir, key, svc, update))))
}

/// Team 的那一份要做的：Railway 是一行一条的命令，Sealos 是表单里填什么（和 steps 一个样子，口令的哈希换成 Team 的根公钥）
fn team_steps(p: Platform, dir: &str, key: &str, svc: &str, update: bool) -> String {
    let cd = format!("cd {}", sh_quote(dir));
    let file = path::join(dir, "cca-presence.py");
    match (p, update) {
        (Platform::Railway, false) => format!(
            "{cd}\nrailway login\nrailway init --name {svc}\nrailway add --service {svc}\nrailway volume add --mount-path /data\n\
             railway up --detach\nrailway domain --port 8080"
        ),
        (Platform::Railway, true) => format!("{cd}\nrailway up --detach"),
        (Platform::Sealos, false) => format!(
            "镜像名称：python:3.13-slim\n\
             部署模式：固定实例，1 个\n\
             计算资源：最小的就够（0.1 核、128 MB 内存）\n\
             网络配置：容器端口 8080，打开公网访问（https）\n\
             启动命令：python3 -I /app/cca-presence.py\n\
             环境变量：CCA_HUB_HOSTED=1\n\
             环境变量：CCA_TEAM_ROOT={key}\n\
             存储卷：挂载路径 /data，容量 1 Gi\n\
             配置文件：挂载路径 /app/cca-presence.py，内容是 {file} 的全部内容"
        ),
        (Platform::Sealos, true) => format!(
            "在「应用管理」里打开这个应用，点「变更」\n\
             配置文件 /app/cca-presence.py 的内容换成 {file} 的全部内容（新版的服务端程序）\n\
             再点「变更」：应用重启，数据在 /data 的存储卷里，设备不用重新加入"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_have_no_shell_comments_and_carry_the_key() {
        let key = "AAAAC3NzaC1lZDI1NTE5AAAA";
        let rw = team_steps(Platform::Railway, "/tmp/it's here", key, "cca-team-0a0a0a0a", false);
        assert!(rw.lines().all(|l| !l.contains('#')), "zsh 交互时不认注释");
        assert!(rw.starts_with("cd '/tmp/it'\"'\"'s here'\n"), "{rw}");
        assert!(team_steps(Platform::Sealos, "/d", key, "cca-team-0a0a0a0a", false).contains(&format!("CCA_TEAM_ROOT={key}")));
    }

    #[test]
    fn the_server_has_the_hosted_preset() {
        // 平台上照 CCA_HUB_HOSTED=1 运行：生成的步骤、Dockerfile 靠的是服务端程序里的这一套（server/cca-presence.py）
        for want in ["CCA_HUB_HOSTED", "CCA_TEAM_ROOT", "0.0.0.0:8080", "/data/state.json", "--behind-proxy"] {
            assert!(SERVER.contains(want), "{want}");
        }
    }
}
