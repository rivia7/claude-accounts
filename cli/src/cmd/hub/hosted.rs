//! Team 的同步服务器部署到托管平台（cca team hosted，docs/dev/team.md「托管平台」）。只有 Railway：它给的是 https 的公网地址，证书是
//! 平台的，设备照系统的证书机构验它（接上时没有指纹）；服务端程序（Linux x86_64 的 cca-core，`team-server`）照 CCA_HUB_HOSTED=1
//! 运行（--behind-proxy，监听 8080，数据在挂在 /data 的存储里），有 CCA_TEAM_ROOT（Team 的根公钥）就是这个 Team 的服务器。
//! 这里只生成部署文件和要做的步骤（`team_files`）：程序放进一个空的镜像（FROM scratch），平台上不用 Python。
//! 以前还有一家国内的平台，为什么去掉了见 docs/dev/team.md「托管平台」

use super::payload::{self, Core};
use crate::cmd::cost::version_ok;
use crate::core::base::HOME;
use crate::py::fs;
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{path, R};

/// 生成的部署文件：目录里只能有这几个和以 . 开头的（访达的 .DS_Store、git……），别的不碰
const FILES: &[&str] = &["Dockerfile", "cca-core", "README.md"];
/// 以前的版本生成的服务端程序（Python 的）：目录里有它不算别的文件，这一次生成好以后删掉（不然 railway up 还把它传上去）
const LEGACY: &str = "cca-presence.py";
/// 托管平台上运行的是哪种机器的程序（payload.rs 的机器名）：Railway 是 x86_64 的
const ARCH: &str = "X86_64";

/// 放部署文件的目录：没有就建；有的话里面只能是我们生成的（FILES、LEGACY）和以 . 开头的
fn prepare_dir(dir: &str) -> Result<(), String> {
    if path::lexists(dir) && !path::isdir(dir) {
        return Err(format!("{dir} 不是目录：换一个（--dir）"));
    }
    if path::isdir(dir) {
        let names = fs::listdir_sorted(dir).map_err(|e| format!("读不了 {dir}（{e}）"))?;
        if let Some(other) = names.iter().find(|n| !n.starts_with('.') && !FILES.contains(&n.as_str()) && n.as_str() != LEGACY) {
            return Err(format!("{dir} 里有别的文件（{other}）：换一个目录（--dir），或者先把它挪走"));
        }
    }
    fs::makedirs(dir).map_err(|e| format!("建不了 {dir}（{e}）"))
}

/// Team 的同步服务器部署到 Railway 的文件（cca team hosted：cmd/team/admin.rs 的 cmd_team_hosted 调）。目录（没写是 ~/<服务名>-railway）
/// 里生成服务端程序（cca-core：这份 cca 带着的 Linux x86_64 的那一份，见 payload.rs）、Dockerfile 和 README.md。平台上只有 Team 的
/// 根公钥（公开的）：Dockerfile 的环境变量 CCA_TEAM_ROOT 写它 ssh-ed25519 后面那一段 key（服务端有了它、又是 CCA_HUB_HOSTED=1，
/// 就是 Team 的服务器）。update：这个 Team 已经接着托管平台上的那一台，这一次只是换上新版的服务端程序。返回（目录, 这一次要做的）；
/// 没成是（原因码, 原因）：目录里有别的东西是 dir，这份 cca 没带服务端程序（不完整、刚更新过）是 install
pub fn team_files(
    dir: Option<&str>,
    key: &str,
    svc: &str,
    name: &str,
    update: bool,
) -> R<Result<(String, String), (String, String)>> {
    let dir = dir.map_or_else(|| path::join(&HOME, &format!("{svc}-railway")), |d| path::abspath(&path::expanduser(d)));
    // 先看这份 cca 带着服务端程序没有：没带就什么都不建
    let core = match payload::linux_cores() {
        Ok(cores) => cores.into_iter().find(|c| c.arch == ARCH),
        Err(why) => return Ok(Err(("install".into(), why))),
    };
    let Some(core) = core else {
        return Ok(Err((
            "install".into(),
            "这一份 cca 没带 Linux x86_64 的服务端程序（托管平台上运行的是它）：重新安装 cca".into(),
        )));
    };
    team_files_with(&dir, key, svc, name, update, &core)
}

/// team_files 找到服务端程序以后的那一半（测试直接给一份）
fn team_files_with(
    dir: &str,
    key: &str,
    svc: &str,
    name: &str,
    update: bool,
    core: &Core,
) -> R<Result<(String, String), (String, String)>> {
    let dir = dir.to_string();
    let raw = match payload::core_raw(core) {
        Ok(raw) => raw,
        Err(why) => return Ok(Err(("install".into(), why))),
    };
    if let Err(why) = prepare_dir(&dir) {
        return Ok(Err(("dir".into(), why)));
    }
    let ver = getenv("CCA_VERSION").filter(|v| version_ok(v)).map_or(String::new(), |v| format!(" {v}"));
    let first = team_steps(&dir, svc, false);
    let again = team_steps(&dir, svc, true);
    let readme = format!(
        "# Team「{name}」的同步服务器：部署到 Railway\n\n\
         这个目录是 cca{ver} 的 `cca team hosted` 生成的：`cca-core` 是服务端程序（Linux x86_64 上的一个静态链接的程序，\
         就是 cca 自己的辅助程序，照 `team-server` 运行），`Dockerfile` 把它放进一个空的镜像（FROM scratch）运行，不用别的。\
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
         FROM scratch\n\
         COPY cca-core /cca-core\n\
         ENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\n\
         EXPOSE 8080\n\
         CMD [\"/cca-core\", \"team-server\"]\n"
    );
    put_core(&dir, &raw)?;
    for (file, body) in [("README.md", readme), ("Dockerfile", dockerfile)] {
        fs::put(&path::join(&dir, file), body.as_bytes(), 0o644)?;
    }
    // 以前的版本生成的那一份 Python 的服务端程序：用不着了
    match std::fs::remove_file(path::join(&dir, LEGACY)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    Ok(Ok((dir.clone(), team_steps(&dir, svc, update))))
}

/// 服务端程序写成 目录/cca-core：先写到旁边（. 开头的：生成到一半停了，下次这个目录照样认）再换上；权限 755——镜像里的权限照
/// 这个文件的（COPY 不改），不能运行的话平台上起不来
fn put_core(dir: &str, raw: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path::join(dir, ".cca-core.new");
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o755).open(&tmp)?.write_all(raw)?;
    fs::chmod(&tmp, 0o755)?;
    std::fs::rename(&tmp, path::join(dir, "cca-core"))
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

    /// 冒充 Linux x86_64 上的服务端程序
    fn fake_core(tag: &str) -> (Core, Vec<u8>) {
        let raw = format!("\x7fELF {tag} ").repeat(500).into_bytes();
        (Core { arch: ARCH, gz: crate::py::gzip::compress(&raw, 9), sha256: crate::py::hash::sha256_hex(&raw) }, raw)
    }

    fn mode(p: &str) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn files_carry_only_the_public_key() {
        let tmp = TempDir::new().unwrap();
        let d = format!("{}/rw", tmp.path);
        let key = "AAAAC3NzaC1lZDI1NTE5AAAA";
        let (core, raw) = fake_core("一");
        let (dir, steps) = team_files_with(&d, key, "cca-team-0a0a0a0a", "甲组", false, &core).unwrap().unwrap();
        assert_eq!((dir.as_str(), steps), (d.as_str(), team_steps(&d, "cca-team-0a0a0a0a", false)));
        assert_eq!(fs::listdir_sorted(&d).unwrap(), ["Dockerfile", "README.md", "cca-core"]);
        let docker = std::fs::read_to_string(format!("{d}/Dockerfile")).unwrap();
        assert!(
            docker.ends_with(&format!(
                "\nFROM scratch\nCOPY cca-core /cca-core\nENV CCA_HUB_HOSTED=1 CCA_TEAM_ROOT={key}\nEXPOSE 8080\n\
                 CMD [\"/cca-core\", \"team-server\"]\n"
            )),
            "{docker}"
        );
        assert_eq!(docker.lines().filter(|l| !l.starts_with('#')).count(), 5, "空的镜像里只有这一个程序：{docker}");
        let readme = std::fs::read_to_string(format!("{d}/README.md")).unwrap();
        assert!(
            readme.contains("cca team attach 甲组 https://") && readme.contains("railway volume add --mount-path /data"),
            "{readme}"
        );
        assert!(!format!("{docker}{readme}").to_lowercase().contains("python"), "平台上不用 Python");
        // 服务端程序：原样的那一份，能运行（755）
        assert!(std::fs::read(format!("{d}/cca-core")).unwrap() == raw);
        assert_eq!((mode(&format!("{d}/cca-core")), mode(&format!("{d}/Dockerfile"))), (0o755, 0o644));
        // 再运行（更新）照样覆盖，只要重新上传；以前的版本生成的 Python 的服务端程序在目录里不碍事，这一次删掉；程序换成新的，
        // 权限被改过也改回来
        std::fs::write(format!("{d}/cca-presence.py"), "# 以前的\n").unwrap();
        std::fs::set_permissions(format!("{d}/cca-core"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let (core2, raw2) = fake_core("二");
        let (_, steps) = team_files_with(&d, key, "cca-team-0a0a0a0a", "甲组", true, &core2).unwrap().unwrap();
        assert_eq!(steps, format!("cd {d}\nrailway up --detach"));
        assert_eq!(fs::listdir_sorted(&d).unwrap(), ["Dockerfile", "README.md", "cca-core"]);
        assert!(std::fs::read(format!("{d}/cca-core")).unwrap() == raw2);
        assert_eq!(mode(&format!("{d}/cca-core")), 0o755);
        // 目录里有别的文件：不碰
        std::fs::write(format!("{d}/other.txt"), "x").unwrap();
        let (code, why) = team_files_with(&d, key, "cca-team-0a0a0a0a", "甲组", true, &core).unwrap().unwrap_err();
        assert_eq!((code.as_str(), why), ("dir", format!("{d} 里有别的文件（other.txt）：换一个目录（--dir），或者先把它挪走")));
        assert!(std::fs::read(format!("{d}/cca-core")).unwrap() == raw2, "没成：原来的不动");
        // 服务端程序对不上校验值（这份 cca 坏了）：什么都不建
        let bad = Core { sha256: crate::py::hash::sha256_hex(b"other"), ..fake_core("三").0 };
        let d2 = format!("{}/rw2", tmp.path);
        let (code, why) = team_files_with(&d2, key, "cca-team-0a0a0a0a", "甲组", false, &bad).unwrap().unwrap_err();
        assert_eq!((code.as_str(), why.as_str()), ("install", "x86_64-unknown-linux-musl 的服务端程序和记着的校验值对不上"));
        assert!(!std::path::Path::new(&d2).exists());
    }
}
