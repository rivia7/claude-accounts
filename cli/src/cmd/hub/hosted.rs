//! Team 的同步服务器部署到托管平台（cca team hosted，docs/dev/team.md「托管平台」）。只有 Railway：它给的是 https 的公网地址，证书是
//! 平台的，设备照系统的证书机构验它（接上时没有指纹）；服务端程序照 CCA_HUB_HOSTED=1 运行（--behind-proxy，监听 8080，数据在挂在
//! /data 的存储里），有 CCA_TEAM_ROOT（Team 的根公钥）就是这个 Team 的服务器。这里只生成部署文件和要做的步骤（`team_files`）。
//! 以前还有一家国内的平台，为什么去掉了见 docs/dev/team.md「托管平台」

use super::deploy::SERVER;
use crate::cmd::cost::version_ok;
use crate::core::base::HOME;
use crate::py::fs;
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{path, R};

/// 生成的部署文件：目录里只能有这几个和以 . 开头的（访达的 .DS_Store、git……），别的不碰
const FILES: &[&str] = &["Dockerfile", "cca-presence.py", "README.md"];

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

/// Team 的同步服务器部署到 Railway 的文件（cca team hosted：cmd/team/admin.rs 的 cmd_team_hosted 调）。目录（没写是 ~/<服务名>-railway）
/// 里生成服务端程序、Dockerfile 和 README.md。平台上只有 Team 的根公钥（公开的）：Dockerfile 的环境变量 CCA_TEAM_ROOT 写它
/// ssh-ed25519 后面那一段 key（服务端有了它、又是 CCA_HUB_HOSTED=1，就是 Team 的服务器）。update：这个 Team 已经接着托管平台上的
/// 那一台，这一次只是换上新版的服务端程序。返回（目录, 这一次要做的）；目录里有别的东西是（原因码, 原因）
pub fn team_files(
    dir: Option<&str>,
    key: &str,
    svc: &str,
    name: &str,
    update: bool,
) -> R<Result<(String, String), (String, String)>> {
    let dir = dir.map_or_else(|| path::join(&HOME, &format!("{svc}-railway")), |d| path::abspath(&path::expanduser(d)));
    if let Err(why) = prepare_dir(&dir) {
        return Ok(Err(("dir".into(), why)));
    }
    let ver = getenv("CCA_VERSION").filter(|v| version_ok(v)).map_or(String::new(), |v| format!(" {v}"));
    let first = team_steps(&dir, svc, false);
    let again = team_steps(&dir, svc, true);
    let readme = format!(
        "# Team「{name}」的同步服务器：部署到 Railway\n\n\
         这个目录是 cca{ver} 的 `cca team hosted` 生成的：`cca-presence.py` 是服务端程序（只用 Python 标准库），\
         `Dockerfile` 照 python:3.13-slim 运行它。\
         平台上只有这个 Team 的根公钥（环境变量 `CCA_TEAM_ROOT`：它是公开的，不用保密）；设备、邀请、每台设备的配置都由管理员的设备管，\
         服务器上存的是密文。\n\n\
         ## 第一次部署\n\n```\n{first}\n```\n\n\
         没挂存储（/data）的话服务端程序不启动：不然重启一次数据就全没了。界面、命令以平台现在的为准。\n\n\
         部署好以后，在管理员的设备上把平台给的公网地址（https://…）接上：\n\n```sh\ncca team attach {name} https://平台给的地址\n```\n\n\
         ## 更新\n\n\
         新版 cca 带着新版的服务端程序。在管理员的设备上再运行一次 `cca team hosted`，然后：\n\n```\n{again}\n```\n\n\
         数据在 /data 的存储里，更新以后照旧，设备不用重新加入。\n"
    );
    let dockerfile = format!(
        "# cca{ver} 的 cca team hosted railway 生成的：Team 的同步服务器（部署的步骤在 README.md）\n\
         FROM python:3.13-slim\n\
         COPY cca-presence.py /app/cca-presence.py\n\
         ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n\
         EXPOSE 8080\n\
         CMD [\"python3\", \"-I\", \"/app/cca-presence.py\"]\n"
    );
    for (file, body) in [("cca-presence.py", SERVER.to_string()), ("README.md", readme), ("Dockerfile", dockerfile)] {
        fs::put(&path::join(&dir, file), body.as_bytes(), 0o644)?;
    }
    Ok(Ok((dir.clone(), team_steps(&dir, svc, update))))
}

/// 要做的：一行一条的命令（第一次部署；更新只要重新上传）
fn team_steps(dir: &str, svc: &str, update: bool) -> String {
    let cd = format!("cd {}", sh_quote(dir));
    if update {
        format!("{cd}\nrailway up --detach")
    } else {
        format!(
            "{cd}\nrailway login\nrailway init --name {svc}\nrailway add --service {svc}\nrailway volume add --mount-path /data\n\
             railway up --detach\nrailway domain --port 8080"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;

    #[test]
    fn steps_have_no_shell_comments() {
        let rw = team_steps("/tmp/it's here", "cca-team-0a0a0a0a", false);
        assert!(rw.lines().all(|l| !l.contains('#')), "zsh 交互时不认注释");
        assert!(rw.starts_with("cd '/tmp/it'\"'\"'s here'\n"), "{rw}");
        assert!(rw.contains("\nrailway init --name cca-team-0a0a0a0a\n") && rw.ends_with("\nrailway domain --port 8080"), "{rw}");
        assert_eq!(team_steps("/d", "cca-team-0a0a0a0a", true), "cd /d\nrailway up --detach");
    }

    #[test]
    fn files_carry_only_the_public_key() {
        let tmp = TempDir::new().unwrap();
        let d = format!("{}/rw", tmp.path);
        let key = "AAAAC3NzaC1lZDI1NTE5AAAA";
        let (dir, steps) = team_files(Some(&d), key, "cca-team-0a0a0a0a", "甲组", false).unwrap().unwrap();
        assert_eq!((dir.as_str(), steps), (d.as_str(), team_steps(&d, "cca-team-0a0a0a0a", false)));
        assert_eq!(fs::listdir_sorted(&d).unwrap(), ["Dockerfile", "README.md", "cca-presence.py"]);
        let docker = std::fs::read_to_string(format!("{d}/Dockerfile")).unwrap();
        assert!(docker.contains(&format!("\nENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n")), "{docker}");
        let readme = std::fs::read_to_string(format!("{d}/README.md")).unwrap();
        assert!(
            readme.contains("cca team attach 甲组 https://") && readme.contains("railway volume add --mount-path /data"),
            "{readme}"
        );
        assert_eq!(std::fs::read_to_string(format!("{d}/cca-presence.py")).unwrap(), SERVER);
        // 再运行（更新）照样覆盖，只要重新上传
        let (_, steps) = team_files(Some(&d), key, "cca-team-0a0a0a0a", "甲组", true).unwrap().unwrap();
        assert_eq!(steps, format!("cd {d}\nrailway up --detach"));
        // 目录里有别的文件：不碰
        std::fs::write(format!("{d}/other.txt"), "x").unwrap();
        let (code, why) = team_files(Some(&d), key, "cca-team-0a0a0a0a", "甲组", true).unwrap().unwrap_err();
        assert_eq!((code.as_str(), why), ("dir", format!("{d} 里有别的文件（other.txt）：换一个目录（--dir），或者先把它挪走")));
    }

    #[test]
    fn the_server_has_the_hosted_preset() {
        // 平台上照 CCA_HUB_HOSTED=1 运行：生成的步骤、Dockerfile 靠的是服务端程序里的这一套（server/cca-presence.py）
        for want in ["CCA_HUB_HOSTED", "CCA_TEAM_ROOT", "0.0.0.0:8080", "/data/state.json", "--behind-proxy"] {
            assert!(SERVER.contains(want), "{want}");
        }
    }
}
