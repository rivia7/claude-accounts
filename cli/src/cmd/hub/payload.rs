//! Team 的同步服务器要传到 Linux 服务器上的程序从哪来：就是 cca-core 自己（musl 静态链接的 x86_64、aarch64 两份）。一键部署的
//! 安装脚本两份都带上，服务器上照 `uname -m` 挑一份（先问服务器是什么机器的话要多登录一次）；部署到托管平台用 x86_64 的那一份。
//!
//! - **发布的 cca**（`HELPER_SUMS` 有值）：文件最后本来就接着四种机器的辅助程序（`cca-core pack`，见 cmd/pack.rs），Linux 的
//!   那两份就是要传的，不另外再带一份。拿那两段原样的 gzip，解开核对 `HELPER_SUMS` 里记的 SHA-256。认的是和正在运行的 cca
//!   同一个版本的那个文件：这一份 cca 已经被新版本换掉了（这个进程是更新之前起的）就从更新留下的上一个版本（`update/cca.prev`）里拿，
//!   和 cca 的 `helper_unpack` 一样；两个都对不上版本时让用户重新运行一次。环境变量指定的一律不认：一个环境变量不能让发布的 cca
//!   把别的程序装到服务器上去（和 `CCA_HELPER` 一个道理）。
//! - **仓库里的 cca**（开发、测试时）：`CCA_HUB_CORE_X86_64`、`CCA_HUB_CORE_AARCH64` 写了哪个就只用写了的（测试拿这台 Mac 的
//!   辅助程序冒充 Linux 的）；都没写时用仓库里交叉编译出来的 `cli/target/<目标>/release/cca-core`，编出来几个用几个，一个都没有时
//!   说怎么编。
//!
//! 只有三处用它：安装脚本（`hub-installer`）、`cca team deploy`（deploy.rs 的 `team_install`）、`cca team hosted`（hosted.rs 的
//! `team_files`）。别让启动的路上碰它：读一遍十几 MB 的 cca、解两份程序要几十毫秒。

use crate::core::base::CCA_HOME;
use crate::py::base64::a2b_base64;
use crate::py::fail::os_error_text;
use crate::py::hash::sha256_hex;
use crate::py::proc::getenv;
use crate::py::text::sh_quote;
use crate::py::{gzip, path};
use std::path::{Path, PathBuf};

/// 安装脚本里的机器名 ↔ 辅助程序的目标，先后就是安装脚本里两段的先后。和 cmd/pack.rs 的 `TARGETS` 里 Linux 的那两个、
/// 安装脚本照 `uname -m` 认机器的那几行一起改
pub const LINUX: [(&str, &str); 2] = [("X86_64", "x86_64-unknown-linux-musl"), ("AARCH64", "aarch64-unknown-linux-musl")];

/// 解开以后最多这么大：现在一份是 5～7 MB；文件坏了、被人改过，也不至于把内存吃光
const LIMIT: usize = 64 << 20;
/// cmd/pack.rs 接在后面的每一段：`#CCA-HELPER 目标`、下面这一行、base64 的几行、结束行
const OPEN: &[u8] = b": <<'CCA-HELPER-END'";
const END: &[u8] = b"CCA-HELPER-END";

/// 一份要传到服务器上的 cca-core
pub struct Core {
    /// 机器名：`LINUX` 里的 X86_64、AARCH64
    pub arch: &'static str,
    /// gzip 过的程序：发布的 cca 里接着的那一段原样（gzip 的字节）；开发时是现压的
    pub gz: Vec<u8>,
    /// 原样的程序的 SHA-256（十六进制）
    pub sha256: String,
}

/// 到哪里找：`linux_cores` 照环境变量填好，测试直接给
pub struct Source {
    /// 正在运行的这一份 cca（`CCA_SELF`）
    pub self_path: Option<PathBuf>,
    /// 正在运行的 cca 的版本（`CCA_VERSION`）
    pub version: String,
    /// 更新留下的上一个版本（`$CCA_HOME/update/cca.prev`）
    pub prev: Option<PathBuf>,
    /// 开发时指定的（机器名, 程序）：`CCA_HUB_CORE_X86_64`、`CCA_HUB_CORE_AARCH64` 里写了的
    pub overrides: Vec<(&'static str, PathBuf)>,
}

/// 照这次运行的环境找：cca 的 `py` 给每条子命令都带着 `CCA_SELF`、`CCA_VERSION`
pub fn linux_cores() -> Result<Vec<Core>, String> {
    let var = |k: &str| getenv(k).filter(|v| !v.is_empty());
    linux_cores_from(&Source {
        self_path: var("CCA_SELF").map(PathBuf::from),
        version: var("CCA_VERSION").unwrap_or_default(),
        prev: Some(PathBuf::from(path::join_all(&[&CCA_HOME, "update", "cca.prev"]))),
        overrides: LINUX
            .iter()
            .filter_map(|(arch, _)| Some((*arch, PathBuf::from(var(&format!("CCA_HUB_CORE_{arch}"))?))))
            .collect(),
    })
}

/// 找出来的几份，先后照 `LINUX`。只看给的这几项，不碰环境变量（单元测试直接调它）。不成的是给人看的一句话
pub fn linux_cores_from(src: &Source) -> Result<Vec<Core>, String> {
    if let Some(me) = &src.self_path {
        let text = read(me)?;
        if !sums_of(&text).is_empty() {
            return released(src, me, text);
        }
    }
    dev(src)
}

/// 解开一份，再核对一遍 SHA-256（部署到托管平台时要原样的程序：它放进 Docker 镜像里）
pub fn core_raw(c: &Core) -> Result<Vec<u8>, String> {
    let name = LINUX.iter().find(|(a, _)| *a == c.arch).map_or(c.arch, |(_, t)| *t);
    let raw = gzip::decompress(&c.gz, LIMIT).map_err(|why| format!("{name} 的服务端程序解不开（{why}）"))?;
    if sha256_hex(&raw) != c.sha256 {
        return Err(format!("{name} 的服务端程序和记着的校验值对不上"));
    }
    Ok(raw)
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("读不了 {}（{}）", p.display(), os_error_text(&e)))
}

fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split(|&b| b == b'\n')
}

/// 文件里的版本号：第一个 `VERSION="…"` 的行（照 cca 的 `ver_of`：数字开头，只有数字、小写字母、点和横线）
fn version_of(text: &[u8]) -> Option<&str> {
    lines(text).find_map(|l| {
        let v = l.strip_prefix(b"VERSION=\"")?.strip_suffix(b"\"")?;
        let ok = v.first().is_some_and(u8::is_ascii_digit)
            && v.iter().all(|&c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'.' || c == b'-');
        if ok {
            std::str::from_utf8(v).ok()
        } else {
            None
        }
    })
}

/// `HELPER_SUMS="目标=SHA-256 …"` 那一行记着的（目标, SHA-256）；仓库里的 cca 这一行是空的
fn sums_of(text: &[u8]) -> Vec<(String, String)> {
    let Some(line) = lines(text).find(|l| l.starts_with(b"HELPER_SUMS=\"")) else {
        return Vec::new();
    };
    let value = line[b"HELPER_SUMS=\"".len()..].split(|&b| b == b'"').next().unwrap_or_default();
    String::from_utf8_lossy(value)
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(t, sum)| (t.to_string(), sum.to_string()))
        .collect()
}

/// 接在后面的、这个目标的那一段里的 base64（去掉了换行）；没有这一段是 None，有开头、样子不对是 Err（为什么坏了）
fn section(text: &[u8], triple: &str) -> Result<Option<Vec<u8>>, String> {
    let head = format!("#CCA-HELPER {triple}");
    let mut it = lines(text).skip_while(|l| *l != head.as_bytes());
    if it.next().is_none() {
        return Ok(None);
    }
    if it.next() != Some(OPEN) {
        return Err("开头的第二行不对".into());
    }
    let mut b64 = Vec::new();
    for l in it {
        if l == END {
            return Ok(Some(b64));
        }
        b64.extend_from_slice(l);
    }
    Err("没有结束的那一行（文件不完整）".into())
}

/// 发布的 cca：和正在运行的同一个版本的那个文件里，Linux 的那几段
fn released(src: &Source, me: &Path, mine: Vec<u8>) -> Result<Vec<Core>, String> {
    let (file, text) = if version_of(&mine) == Some(src.version.as_str()) {
        (me.to_path_buf(), mine)
    } else {
        // 这一份 cca 正在运行时被更新换掉了：更新留下的上一个版本才是这个进程的
        match src.prev.as_ref().and_then(|p| Some((p.clone(), std::fs::read(p).ok()?))) {
            Some((p, t)) if version_of(&t) == Some(src.version.as_str()) => (p, t),
            _ => return Err("cca 刚更新过：重新运行一次".into()),
        }
    };
    let name = file.display();
    let sums = sums_of(&text);
    let mut cores = Vec::new();
    for (arch, triple) in LINUX {
        // 两样都有的才算：只接了几种机器的（回归里 mkver 做的假发布版）少的那几种不带
        let Some((_, sum)) = sums.iter().find(|(t, _)| t == triple) else { continue };
        let broken = |why: &str| format!("{name} 里 {triple} 的服务端程序坏了（{why}）：重新安装 cca");
        let Some(b64) = section(&text, triple).map_err(|why| broken(&why))? else { continue };
        let gz = a2b_base64(&b64, true).map_err(|_| broken("base64 解不开"))?;
        let raw = gzip::decompress(&gz, LIMIT).map_err(|why| broken(&why))?;
        if sha256_hex(&raw) != *sum {
            return Err(format!("{name} 里 {triple} 的服务端程序和 HELPER_SUMS 记的校验值对不上：重新安装 cca"));
        }
        cores.push(Core { arch, gz, sha256: sum.clone() });
    }
    if cores.is_empty() {
        return Err(format!("{name} 不完整（没带 Linux 服务器上运行的服务端程序）：重新安装 cca"));
    }
    Ok(cores)
}

/// 仓库里的 cca：指定了的只用指定的；没指定时用 cli/target 里交叉编译出来的
fn dev(src: &Source) -> Result<Vec<Core>, String> {
    let fresh = |arch: &'static str, raw: Vec<u8>| Core { arch, gz: gzip::compress(&raw, 1), sha256: sha256_hex(&raw) };
    let repo = src.self_path.as_deref().and_then(Path::parent);
    let mut cores = Vec::new();
    if !src.overrides.is_empty() {
        for (arch, _) in LINUX {
            if let Some((_, p)) = src.overrides.iter().find(|(a, _)| *a == arch) {
                let raw = std::fs::read(p)
                    .map_err(|e| format!("读不了 CCA_HUB_CORE_{arch} 指的 {}（{}）", p.display(), os_error_text(&e)))?;
                cores.push(fresh(arch, raw));
            }
        }
    } else if let Some(repo) = repo {
        for (arch, triple) in LINUX {
            let p = repo.join("cli/target").join(triple).join("release/cca-core");
            if p.exists() {
                cores.push(fresh(arch, read(&p)?));
            }
        }
    }
    if cores.is_empty() {
        let cli = repo.map_or_else(|| "cli".to_string(), |r| sh_quote(&r.join("cli").to_string_lossy()));
        return Err(format!(
            "这一份 cca 是仓库里的源码，没带 Linux 服务器上运行的服务端程序：先交叉编译出来 (cd {cli} && . ./musl-env.sh && \
             cargo build --release --locked --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl)，\
             或者用 CCA_HUB_CORE_X86_64、CCA_HUB_CORE_AARCH64 指定"
        ));
    }
    Ok(cores)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::pack::{cmd_pack, TARGETS};
    use crate::py::base64::encodebytes;
    use crate::py::fs::TempDir;

    const X86: &str = "x86_64-unknown-linux-musl";
    const ARM: &str = "aarch64-unknown-linux-musl";
    const MAC: &str = "aarch64-apple-darwin";

    fn repo_cca() -> String {
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../cca")).unwrap()
    }

    /// 冒充 Linux 的程序：每份不一样，一半能压、一半压不动
    fn fake(tag: &str, n: usize) -> Vec<u8> {
        let mut v = format!("\x7fELF {tag} ").repeat(n).into_bytes();
        v.extend((0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
        v
    }

    /// 发布版的样子：仓库里的 cca 换成版本 ver，用 cmd/pack.rs 接上给的几段（目标, 程序），写成 dir/name
    fn packed(dir: &str, name: &str, ver: &str, parts: &[(&str, &[u8])]) -> PathBuf {
        let cca = repo_cca();
        let old = format!("\nVERSION=\"{}\"\n", version_of(cca.as_bytes()).unwrap());
        assert!(cca.contains(&old));
        let src = format!("{dir}/{name}.src");
        std::fs::write(&src, cca.replacen(&old, &format!("\nVERSION=\"{ver}\"\n"), 1)).unwrap();
        let out = format!("{dir}/{name}");
        let mut args = vec![src, out.clone(), "--only".to_string()];
        for (i, (t, raw)) in parts.iter().enumerate() {
            let p = format!("{dir}/{name}.part{i}");
            std::fs::write(&p, raw).unwrap();
            args.push(format!("{t}={p}"));
        }
        cmd_pack(&args).unwrap();
        PathBuf::from(out)
    }

    fn released_src(me: &Path, ver: &str, prev: Option<&Path>) -> Source {
        Source { self_path: Some(me.to_path_buf()), version: ver.into(), prev: prev.map(Path::to_path_buf), overrides: Vec::new() }
    }

    fn arches(cores: &[Core]) -> Vec<&str> {
        cores.iter().map(|c| c.arch).collect()
    }

    fn raws(cores: &[Core]) -> Vec<Vec<u8>> {
        cores.iter().map(|c| core_raw(c).unwrap()).collect()
    }

    fn err(src: &Source) -> String {
        match linux_cores_from(src) {
            Ok(c) => panic!("应该不成，却拿到了 {:?}", arches(&c)),
            Err(e) => e,
        }
    }

    #[test]
    fn linux_targets_are_packed() {
        // 发布的 cca 里接着的就是它们：cmd/pack.rs 的 TARGETS 里要有
        for (_, t) in LINUX {
            assert!(TARGETS.contains(&t), "{t}");
        }
    }

    #[test]
    fn released_uses_the_packed_sections() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let (x, a) = (fake("x86 2", 3000), fake("arm 2", 2000));
        let me = packed(d, "cca", "9.9.2", &[(MAC, b"mac"), (X86, &x), (ARM, &a)]);
        let cores = linux_cores_from(&released_src(&me, "9.9.2", None)).unwrap();
        assert_eq!(arches(&cores), ["X86_64", "AARCH64"]);
        assert!(raws(&cores) == [x.clone(), a.clone()]);
        assert_eq!((cores[0].sha256.as_str(), cores[1].sha256.as_str()), (sha256_hex(&x).as_str(), sha256_hex(&a).as_str()));
        // gzip 的字节就是文件里那一段原样（不重新压）
        let text = std::fs::read_to_string(&me).unwrap();
        for (c, t) in cores.iter().zip([X86, ARM]) {
            assert!(text.contains(&format!("#CCA-HELPER {t}\n: <<'CCA-HELPER-END'\n{}CCA-HELPER-END\n", encodebytes(&c.gz))));
        }
        // 发布的 cca 不认环境变量指定的
        std::fs::write(format!("{d}/other"), b"other").unwrap();
        let mut src = released_src(&me, "9.9.2", None);
        src.overrides = vec![("X86_64", format!("{d}/other").into()), ("AARCH64", format!("{d}/nothing").into())];
        assert!(raws(&linux_cores_from(&src).unwrap()) == [x, a]);
        // 只接了一种 Linux 的（回归里的假发布版就是这样）：只有那一份
        let arm = fake("arm only", 10);
        let one = packed(d, "one", "9.9.2", &[(MAC, b"mac"), (ARM, &arm)]);
        let cores = linux_cores_from(&released_src(&one, "9.9.2", None)).unwrap();
        assert_eq!(arches(&cores), ["AARCH64"]);
        assert!(raws(&cores) == [arm]);
    }

    #[test]
    fn released_falls_back_to_the_previous_version() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let (x2, a2, x3, a3) = (fake("x86 2", 100), fake("arm 2", 100), fake("x86 3", 100), fake("arm 3", 100));
        let prev = packed(d, "cca.prev", "9.9.2", &[(X86, &x2), (ARM, &a2)]);
        let me = packed(d, "cca", "9.9.3", &[(X86, &x3), (ARM, &a3)]);
        // 正在运行的就是这个文件的版本：用它自己的
        assert!(raws(&linux_cores_from(&released_src(&me, "9.9.3", Some(&prev))).unwrap()) == [x3, a3]);
        // 更新之前起的进程（9.9.2）：文件已经换成了 9.9.3，用更新留下的上一个版本
        assert!(raws(&linux_cores_from(&released_src(&me, "9.9.2", Some(&prev))).unwrap()) == [x2, a2]);
        // 两个都对不上：又更新过一次、没有上一个版本、不知道自己的版本
        let again = "cca 刚更新过：重新运行一次";
        assert_eq!(err(&released_src(&me, "9.9.1", Some(&prev))), again);
        assert_eq!(err(&released_src(&me, "9.9.2", None)), again);
        assert_eq!(err(&released_src(&me, "9.9.2", Some(Path::new(&format!("{d}/nothing"))))), again);
        assert_eq!(err(&released_src(&me, "", Some(&prev))), again);
        // 上一个版本没带 Linux 的：说的是它
        let old = packed(d, "old.prev", "9.9.0", &[(MAC, b"mac")]);
        assert_eq!(
            err(&released_src(&me, "9.9.0", Some(&old))),
            format!("{} 不完整（没带 Linux 服务器上运行的服务端程序）：重新安装 cca", old.display())
        );
    }

    #[test]
    fn released_refuses_incomplete_and_broken_files() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let try_ = |p: &Path| err(&released_src(p, "9.9.2", None));
        // 只接了 Mac 的
        let mac = packed(d, "mac", "9.9.2", &[(MAC, b"mac"), ("x86_64-apple-darwin", b"intel")]);
        assert_eq!(try_(&mac), format!("{} 不完整（没带 Linux 服务器上运行的服务端程序）：重新安装 cca", mac.display()));
        // 读不了
        assert_eq!(try_(Path::new(&format!("{d}/nope"))), format!("读不了 {d}/nope（[Errno 2] No such file or directory）"));

        let (x, a) = (fake("x86", 5000), fake("arm", 100));
        let me = packed(d, "cca", "9.9.2", &[(X86, &x), (ARM, &a)]);
        let text = std::fs::read_to_string(&me).unwrap();
        let head = format!("#CCA-HELPER {X86}\n: <<'CCA-HELPER-END'\n");
        let body = text.find(&head).unwrap() + head.len();
        let end = body + text[body..].find("\nCCA-HELPER-END\n").unwrap() + "\nCCA-HELPER-END\n".len();
        let variant = |name: &str, s: &str| -> PathBuf {
            let p = PathBuf::from(format!("{d}/{name}"));
            std::fs::write(&p, s).unwrap();
            p
        };
        let bad = |p: &Path, why: &str| format!("{} 里 {X86} 的服务端程序坏了（{why}）：重新安装 cca", p.display());
        let with = |at: usize, c: &str| format!("{}{c}{}", &text[..at], &text[at + 1..]);
        // base64 里混进了别的字符
        let p = variant("star", &with(body + 100, "*"));
        assert_eq!(try_(&p), bad(&p, "base64 解不开"));
        // 换了一个字符、还是 base64：gzip 解出来不对
        let p = variant("flip", &with(body + 200, if &text[body + 200..body + 201] == "A" { "B" } else { "A" }));
        let e = try_(&p);
        assert!(e.starts_with(&format!("{} 里 {X86} 的服务端程序坏了（", p.display())) && e.ends_with("）：重新安装 cca"), "{e}");
        // 开头的第二行不对
        let p = variant("open", &text.replacen(&head, &format!("#CCA-HELPER {X86}\n: <<'EOF'\n"), 1));
        assert_eq!(try_(&p), bad(&p, "开头的第二行不对"));
        // 截断在这一段中间
        let p = variant("cut", &text[..body + 300]);
        assert_eq!(try_(&p), bad(&p, "没有结束的那一行（文件不完整）"));
        // 和 HELPER_SUMS 记的对不上
        let sum = format!("{X86}={}", sha256_hex(&x));
        assert!(text.contains(&sum));
        let p = variant("sum", &text.replacen(&sum, &format!("{X86}={}", sha256_hex(b"other")), 1));
        assert_eq!(try_(&p), format!("{} 里 {X86} 的服务端程序和 HELPER_SUMS 记的校验值对不上：重新安装 cca", p.display()));
        // 记了校验值、后面没有这一段的（截断在两段之间），没记校验值的段：都只是不带它
        let p = variant("first", &text[..end]);
        assert_eq!(arches(&linux_cores_from(&released_src(&p, "9.9.2", None)).unwrap()), ["X86_64"]);
        let p = variant("unsummed", &text.replacen(&format!(" {ARM}={}", sha256_hex(&a)), "", 1));
        assert_eq!(arches(&linux_cores_from(&released_src(&p, "9.9.2", None)).unwrap()), ["X86_64"]);
    }

    #[test]
    fn dev_uses_overrides_or_the_built_targets() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        // 仓库里的 cca（HELPER_SUMS 空着），cli/target 里两个 Linux 的目标都编出来了
        std::fs::write(format!("{d}/cca"), repo_cca()).unwrap();
        let (tx, ta) = (fake("target x86", 100), fake("target arm", 100));
        for (t, raw) in [(X86, &tx), (ARM, &ta)] {
            std::fs::create_dir_all(format!("{d}/cli/target/{t}/release")).unwrap();
            std::fs::write(format!("{d}/cli/target/{t}/release/cca-core"), raw).unwrap();
        }
        let me = PathBuf::from(format!("{d}/cca"));
        let src = |ov: Vec<(&'static str, PathBuf)>| Source {
            self_path: Some(me.clone()),
            version: "9.9.2".into(),
            prev: Some(PathBuf::from(format!("{d}/cca"))),
            overrides: ov,
        };
        // 没指定：cli/target 里的（现压的）
        let cores = linux_cores_from(&src(vec![])).unwrap();
        assert_eq!(arches(&cores), ["X86_64", "AARCH64"]);
        assert!(raws(&cores) == [tx.clone(), ta.clone()]);
        assert_eq!(cores[1].sha256, sha256_hex(&ta));
        // 指定了一个：只用它，cli/target 里的不管
        let (ox, oa) = (fake("override x86", 50), fake("override arm", 50));
        std::fs::write(format!("{d}/ox"), &ox).unwrap();
        std::fs::write(format!("{d}/oa"), &oa).unwrap();
        let cores = linux_cores_from(&src(vec![("X86_64", format!("{d}/ox").into())])).unwrap();
        assert_eq!(arches(&cores), ["X86_64"]);
        assert!(raws(&cores) == [ox.clone()]);
        // 两个都指定：出来的先后照 LINUX，不照写的先后
        let cores =
            linux_cores_from(&src(vec![("AARCH64", format!("{d}/oa").into()), ("X86_64", format!("{d}/ox").into())])).unwrap();
        assert_eq!(arches(&cores), ["X86_64", "AARCH64"]);
        assert!(raws(&cores) == [ox, oa.clone()]);
        // 指定的读不了：说是哪个变量
        assert_eq!(
            err(&src(vec![("AARCH64", format!("{d}/missing").into())])),
            format!("读不了 CCA_HUB_CORE_AARCH64 指的 {d}/missing（[Errno 2] No such file or directory）")
        );
        // 没有 CCA_SELF（直接运行辅助程序）也认指定的
        let bare =
            Source { self_path: None, version: String::new(), prev: None, overrides: vec![("AARCH64", format!("{d}/oa").into())] };
        assert!(raws(&linux_cores_from(&bare).unwrap()) == [oa]);
        // cli/target 里只编了一个：就用这一个
        std::fs::remove_file(format!("{d}/cli/target/{X86}/release/cca-core")).unwrap();
        let cores = linux_cores_from(&src(vec![])).unwrap();
        assert_eq!(arches(&cores), ["AARCH64"]);
        assert!(raws(&cores) == [ta]);
    }

    #[test]
    fn dev_without_anything_says_how_to_build() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        std::fs::write(format!("{d}/cca"), repo_cca()).unwrap();
        let want = |cli: &str| {
            format!(
                "这一份 cca 是仓库里的源码，没带 Linux 服务器上运行的服务端程序：先交叉编译出来 (cd {cli} && . ./musl-env.sh && \
                 cargo build --release --locked --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl)，\
                 或者用 CCA_HUB_CORE_X86_64、CCA_HUB_CORE_AARCH64 指定"
            )
        };
        let src = Source { self_path: Some(format!("{d}/cca").into()), version: "9.9.2".into(), prev: None, overrides: vec![] };
        assert_eq!(err(&src), want(&sh_quote(&format!("{d}/cli"))));
        // 不知道仓库在哪
        let src = Source { self_path: None, version: String::new(), prev: None, overrides: vec![] };
        assert_eq!(err(&src), want("cli"));
        // 路径里有空格：命令照样能复制了运行
        let spaced = format!("{d}/my repo");
        std::fs::create_dir(&spaced).unwrap();
        std::fs::write(format!("{spaced}/cca"), repo_cca()).unwrap();
        let src = Source { self_path: Some(format!("{spaced}/cca").into()), version: String::new(), prev: None, overrides: vec![] };
        assert_eq!(err(&src), want(&format!("'{spaced}/cli'")));
    }

    #[test]
    fn core_raw_checks_the_sum() {
        let raw = fake("x86", 100);
        let c = Core { arch: "X86_64", gz: gzip::compress(&raw, 9), sha256: sha256_hex(&raw) };
        assert!(core_raw(&c).unwrap() == raw);
        let c = Core { sha256: sha256_hex(b"other"), ..c };
        assert_eq!(core_raw(&c).unwrap_err(), format!("{X86} 的服务端程序和记着的校验值对不上"));
        let c = Core { arch: "AARCH64", gz: b"not gzip".to_vec(), sha256: String::new() };
        assert_eq!(core_raw(&c).unwrap_err(), format!("{ARM} 的服务端程序解不开（不是 gzip 格式）"));
    }
}
