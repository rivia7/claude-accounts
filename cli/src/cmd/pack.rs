//! `pack`：把四种机器的辅助程序接到 cca 后面（release.sh、回归的 `mkver` 和 10g、tests/linux.sh 用；原来是 cli/pack.py）：
//!
//! ```text
//! cca-core pack 仓库里的cca 输出文件 [--only] [目标=辅助程序的路径 …]
//! ```
//!
//! 没写「目标=路径」的目标用 cca 旁边的 cli/target/<目标>/release/cca-core（先把四个目标都编出来，见 release.sh）。
//! `--only`：只接点了名的那几个（回归测试用：只接这台机器的那一份；别的机器上运行会说没有这种机器用的辅助程序）。
//!
//! 做两件事：以 `HELPER_SUMS=""` 开头的那一行（正好一处，后面的注释随便改）整行换成各目标的 SHA-256（cca 第一次运行时按它
//! 校验解出来的文件，也按它给文件起名）；在文件最后（`exit` 的后面，bash 运行时读不到）接上各目标的辅助程序：gzip（不带时间戳，
//! 同样的输入出同样的字节）再 base64（一行 76 个字符），各自包在一个 `: <<'CCA-HELPER-END'` 的 here-document 里（`bash -n`
//! 检查发布的文件时这样最快：十几 MB 也只要几十毫秒）。解开它的是 cca 的 `helper_unpack`（`sed` 切出来、`base64` 解码、
//! `gunzip`）：格式和它一起改。校验值是对原样的程序算的，所以 gzip 出来的字节和原来 Python 压的不一样也没关系。

use super::Cmd;
use crate::out;
use crate::py::fail::os_error_text;
use crate::py::{base64, die, gzip, hash, path, R};
use std::io::Write;
use std::ops::Range;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const CMDS: &[Cmd] = &[Cmd::all("pack", cmd_pack)];

/// 四种机器，先后照这个排（`HELPER_SUMS` 和接在后面的各段）：和 cca 的 `helper_find`、release.sh 的 `TARGETS`、
/// install.sh 查的机器种类一起改
pub const TARGETS: [&str; 4] =
    ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"];
/// 要整行换掉的那一行的开头
const SUMS: &[u8] = b"HELPER_SUMS=\"\"";
/// 仓库里的 cca 的结尾：接过辅助程序的就不是这样了（不会接两次）
const ENDING: &[u8] = b"\nmain \"$@\"\nexit\n";
const USAGE: &str = "用法：cca-core pack 仓库里的cca 输出文件 [--only] [目标=辅助程序的路径 …]";

fn fail<T>(msg: impl std::fmt::Display) -> R<T> {
    die(format!("pack：{msg}"))
}

pub fn cmd_pack(args: &[String]) -> R<()> {
    let [src, out, rest @ ..] = args else {
        return fail(USAGE);
    };
    let only = rest.iter().any(|a| a == "--only");
    // 各目标的辅助程序在哪（先后照 TARGETS）
    let mut paths: [Option<String>; 4] = Default::default();
    if !only {
        let target = path::join_all(&[&path::dirname(&path::abspath(src)), "cli", "target"]);
        for (p, t) in paths.iter_mut().zip(TARGETS) {
            *p = Some(path::join_all(&[&target, t, "release", "cca-core"]));
        }
    }
    for spec in rest.iter().filter(|a| *a != "--only") {
        let (t, p) = spec.split_once('=').unwrap_or((spec, ""));
        match TARGETS.iter().position(|x| *x == t) {
            Some(i) if !p.is_empty() => paths[i] = Some(p.to_string()),
            _ => return fail(format!("参数写成 目标=路径，目标是 {} 之一", TARGETS.join("、"))),
        }
    }
    let script = match std::fs::read(src) {
        Ok(s) => s,
        Err(e) => return fail(format!("读不了 {src}（{}）", os_error_text(&e))),
    };
    let line = sums_line(src, &script).or_else(fail)?;
    if paths.iter().all(Option::is_none) {
        return fail("--only 后面要点名至少一个目标（目标=路径）");
    }
    let (mut sums, mut tail, mut sizes) = (Vec::new(), String::new(), Vec::new());
    for (t, p) in TARGETS.iter().zip(&paths) {
        let Some(p) = p else { continue };
        let raw = match std::fs::read(p) {
            Ok(raw) => raw,
            Err(e) => {
                return fail(format!(
                    "读不了 {t} 的辅助程序（{}）：先编出来，或者用 {t}=路径 指定",
                    crate::py::fs::open_error_text(&e, p)
                ))
            }
        };
        let (sum, text) = section(t, &raw).or_else(fail)?;
        sums.push(sum);
        tail.push_str(&text);
        // 给人看的大小：aarch64-mac 4.7 MB（照原来 Python 的 '%.1f'）
        let short = t.split('-').next().unwrap_or(t);
        sizes.push(format!("{short}-{} {:.1} MB", if t.contains("darwin") { "mac" } else { "linux" }, raw.len() as f64 / 1e6));
    }
    let packed = assemble(&script, line, &sums, &tail);
    if let Err(e) = write_out(out, &packed) {
        return fail(format!("写不了 {out}（{}）", os_error_text(&e)));
    }
    out!("pack：{out}（{} 字节，辅助程序 {}）", packed.len(), sizes.join("、"));
    Ok(())
}

/// 仓库里的 cca 里以 `HELPER_SUMS=""` 开头的那一行在哪（不带换行）：要正好一处，文件的结尾要是 `main "$@"` 和 `exit`
fn sums_line(name: &str, script: &[u8]) -> Result<Range<usize>, String> {
    let mut found = Vec::new();
    let mut at = 0;
    for l in script.split(|&b| b == b'\n') {
        if l.starts_with(SUMS) {
            found.push(at..at + l.len());
        }
        at += l.len() + 1;
    }
    let [line] = &found[..] else {
        return Err(format!("{name} 里没有（或者不止一处）以 HELPER_SUMS=\"\" 开头的那一行（已经接过辅助程序了？）"));
    };
    if !script.ends_with(ENDING) {
        return Err(format!("{name} 的结尾不是 main \"$@\" 和 exit（已经接过辅助程序了？）"));
    }
    Ok(line.clone())
}

/// 接在后面的一段：返回（`目标=SHA-256`，`#CCA-HELPER 目标` 开头、here-document 里是 gzip 再 base64 的程序的那一段）
fn section(t: &str, raw: &[u8]) -> Result<(String, String), String> {
    let gz = gzip::compress(raw, 9);
    // 接上之前自己先解一遍核对：压坏了的话，发出去的 cca 在用户那里解不开
    if gzip::decompress(&gz, raw.len()).ok().as_deref() != Some(raw) {
        return Err(format!("{t} 的辅助程序压缩以后解不回原样"));
    }
    let text = format!("#CCA-HELPER {t}\n: <<'CCA-HELPER-END'\n{}CCA-HELPER-END\n", base64::encodebytes(&gz));
    Ok((format!("{t}={}", hash::sha256_hex(raw)), text))
}

/// 接好的 cca：那一行整行换成 `HELPER_SUMS="目标=SHA-256 …"`，最后接上各段
fn assemble(script: &[u8], line: Range<usize>, sums: &[String], tail: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(script.len() + tail.len() + 400);
    out.extend_from_slice(&script[..line.start]);
    out.extend_from_slice(format!("HELPER_SUMS=\"{}\"", sums.join(" ")).as_bytes());
    out.extend_from_slice(&script[line.end..]);
    out.extend_from_slice(tail.as_bytes());
    out
}

/// 先写旁边的临时文件（`输出文件.进程号`）、改成 755，再改名换上：中途出错不留下半个文件，正在运行的那一份（同名的）也不受影响
fn write_out(out: &str, data: &[u8]) -> std::io::Result<()> {
    let tmp = format!("{out}.{}", std::process::id());
    let _ = std::fs::remove_file(&tmp);
    let done = write_tmp(&tmp, data).and_then(|()| std::fs::rename(&tmp, out));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

fn write_tmp(tmp: &str, data: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o755).open(tmp)?;
    f.write_all(data)?;
    drop(f);
    // 建文件时的权限还要过 umask：再明着改一次
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::fs::TempDir;
    use crate::py::Fail;
    use std::process::Command;

    fn repo_cca() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../cca")).unwrap()
    }

    /// 每次一样的「随机」字节
    fn noise(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    /// 四个假的辅助程序，各不一样：空的、一个字节、57 个随机的字节、几百 KB 一半能压一半随机的
    fn helpers() -> [Vec<u8>; 4] {
        [Vec::new(), vec![0x7f], noise(57, 1), [noise(150_000, 2), b"ELF ".repeat(40_000), noise(99_999, 3)].concat()]
    }

    fn run(args: &[&str]) -> Result<(), String> {
        cmd_pack(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).map_err(|e| match e {
            Fail::Die(m) => m,
            e => panic!("应该是 die：{e:?}"),
        })
    }

    /// 在临时目录里放好四个假的辅助程序，返回「目标=路径」
    fn put_helpers(dir: &str) -> Vec<String> {
        TARGETS
            .iter()
            .zip(helpers())
            .map(|(t, raw)| {
                let p = format!("{dir}/{t}");
                std::fs::write(&p, raw).unwrap();
                format!("{t}={p}")
            })
            .collect()
    }

    fn sh(script: &str, args: &[&str]) -> std::process::Output {
        Command::new("/bin/sh").arg("-c").arg(script).arg("sh").args(args).output().unwrap()
    }

    #[test]
    fn unpacks_with_the_released_pipeline() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let src = concat!(env!("CARGO_MANIFEST_DIR"), "/../cca");
        let out = format!("{d}/cca");
        let specs = put_helpers(d);
        let mut args = vec![src, out.as_str(), "--only"];
        args.extend(specs.iter().map(String::as_str));
        run(&args).unwrap();
        let packed = std::fs::read(&out).unwrap();
        // 权限 755，没留下临时文件
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(crate::py::fs::listdir_sorted(d).unwrap().len(), 5);
        // 还是语法正确的 bash，最后一行是最后一段的结束行
        let n = Command::new("bash").arg("-n").arg(&out).output().unwrap();
        assert!(n.status.success(), "bash -n：{}", String::from_utf8_lossy(&n.stderr));
        assert!(packed.ends_with(b"\nCCA-HELPER-END\n"));
        // HELPER_SUMS 那一行整行换掉了（后面的注释也去掉了），先后照 TARGETS；别的一个字节没变
        let sums: Vec<String> = TARGETS.iter().zip(helpers()).map(|(t, raw)| format!("{t}={}", hash::sha256_hex(raw))).collect();
        let want_line = format!("HELPER_SUMS=\"{}\"", sums.join(" "));
        let text = String::from_utf8(packed.clone()).unwrap();
        assert_eq!(text.lines().filter(|l| l.starts_with("HELPER_SUMS=")).collect::<Vec<_>>(), [want_line.as_str()]);
        let orig = repo_cca();
        let line = sums_line("cca", &orig).unwrap();
        assert_eq!(&packed[..line.start], &orig[..line.start]);
        let rest = &orig[line.end..];
        let after = &packed[line.start + want_line.len()..];
        assert_eq!(&after[..rest.len()], rest);
        // 接在后面的各段：一行最多 76 个字符，照 cca 的 helper_unpack 那一行（原样从 cca 里取出来）在 /bin/sh 里解开，
        // 和原来的程序一字节不差
        let tail = std::str::from_utf8(&after[rest.len()..]).unwrap();
        assert!(tail.lines().all(|l| l.len() <= 76));
        let orig_text = String::from_utf8(orig).unwrap();
        let pipe: Vec<&str> = orig_text.lines().map(str::trim).filter(|l| l.starts_with("sed -n \"/^#CCA-HELPER ")).collect();
        let [pipe] = pipe[..] else {
            panic!("cca 的 helper_unpack 里解辅助程序的那一行（sed | base64 | gunzip）找不到或者不止一处")
        };
        assert!(pipe.contains("\"$src\"") && pipe.contains("> \"$tmp\""), "{pipe}");
        // cca 在 macOS 上用系统自带的 /usr/bin/base64 -D，别处是 PATH 里的 base64 -d
        let (b64, c) = if cfg!(target_os = "macos") { ("/usr/bin/base64", "-D") } else { ("base64", "-d") };
        for (t, raw) in TARGETS.iter().zip(helpers()) {
            let got = format!("{d}/got-{t}");
            let r = sh(&format!("t=$1 src=$2 tmp=$3 b64=$4 c=$5; {pipe}"), &[t, &out, &got, b64, c]);
            assert!(r.status.success(), "{t}：{}", String::from_utf8_lossy(&r.stderr));
            assert!(std::fs::read(&got).unwrap() == raw, "{t}：解出来的和原来的不一样");
        }
    }

    #[test]
    fn same_input_same_bytes() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let src = concat!(env!("CARGO_MANIFEST_DIR"), "/../cca");
        let specs = put_helpers(d);
        let mut outs = Vec::new();
        for k in 0..2 {
            let out = format!("{d}/out{k}");
            let mut args = vec![src, out.as_str(), "--only"];
            args.extend(specs.iter().map(String::as_str));
            run(&args).unwrap();
            outs.push(std::fs::read(&out).unwrap());
        }
        assert!(outs[0] == outs[1]);
    }

    #[test]
    fn default_paths_and_only() {
        // 不写「目标=路径」：cca 旁边的 cli/target/<目标>/release/cca-core；写了的换掉那一个；--only 只接点了名的
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        std::fs::write(format!("{d}/cca"), repo_cca()).unwrap();
        for (t, raw) in TARGETS.iter().zip(helpers()) {
            std::fs::create_dir_all(format!("{d}/cli/target/{t}/release")).unwrap();
            std::fs::write(format!("{d}/cli/target/{t}/release/cca-core"), raw).unwrap();
        }
        std::fs::write(format!("{d}/other"), b"other").unwrap();
        let sums_of = |out: &str| -> String {
            let text = String::from_utf8(std::fs::read(out).unwrap()).unwrap();
            text.lines().find(|l| l.starts_with("HELPER_SUMS=")).unwrap().to_string()
        };
        let h = helpers();
        let s = |i: usize| format!("{}={}", TARGETS[i], hash::sha256_hex(&h[i]));
        let other = format!("x86_64-apple-darwin={}", hash::sha256_hex(b"other"));
        let (cca, out) = (format!("{d}/cca"), format!("{d}/out"));
        run(&[&cca, &out]).unwrap();
        assert_eq!(sums_of(&out), format!("HELPER_SUMS=\"{} {} {} {}\"", s(0), s(1), s(2), s(3)));
        let spec = format!("x86_64-apple-darwin={d}/other");
        run(&[&cca, &out, &spec]).unwrap();
        assert_eq!(sums_of(&out), format!("HELPER_SUMS=\"{} {other} {} {}\"", s(0), s(2), s(3)));
        // --only 写在哪都行；先后照 TARGETS，不照参数
        let spec0 = format!("aarch64-unknown-linux-musl={d}/cli/target/aarch64-unknown-linux-musl/release/cca-core");
        run(&[&cca, &out, &spec, &spec0, "--only"]).unwrap();
        assert_eq!(sums_of(&out), format!("HELPER_SUMS=\"{other} {}\"", s(3)));
        let text = String::from_utf8(std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(
            text.lines().filter(|l| l.starts_with("#CCA-HELPER ")).collect::<Vec<_>>(),
            ["#CCA-HELPER x86_64-apple-darwin", "#CCA-HELPER aarch64-unknown-linux-musl"]
        );
        // 少了一个：说是哪个目标、哪个文件
        std::fs::remove_file(format!("{d}/cli/target/x86_64-unknown-linux-musl/release/cca-core")).unwrap();
        let e = run(&[&cca, &format!("{d}/out2")]).unwrap_err();
        assert_eq!(
            e,
            format!(
                "pack：读不了 x86_64-unknown-linux-musl 的辅助程序（[Errno 2] No such file or directory: \
                 '{d}/cli/target/x86_64-unknown-linux-musl/release/cca-core'）：先编出来，或者用 x86_64-unknown-linux-musl=路径 指定"
            )
        );
        assert!(!std::path::Path::new(&format!("{d}/out2")).exists());
    }

    #[test]
    fn refuses() {
        let dir = TempDir::new().unwrap();
        let d = dir.path.as_str();
        let orig = repo_cca();
        let one = format!("aarch64-apple-darwin={d}/h");
        std::fs::write(format!("{d}/h"), b"helper").unwrap();
        let src = |name: &str, data: &[u8]| -> String {
            let p = format!("{d}/{name}");
            std::fs::write(&p, data).unwrap();
            p
        };
        let text = String::from_utf8(orig.clone()).unwrap();
        let none = src("none", text.replace("\nHELPER_SUMS=\"\"", "\nHELPER_SUMS_X=\"\"").as_bytes());
        let two = src("two", text.replace("\nhelper_find() {", "\nHELPER_SUMS=\"\"\nhelper_find() {").as_bytes());
        let ending = src("ending", &[&orig[..], "# 后面多了一行\n".as_bytes()].concat());
        let no_newline = src("no-newline", &orig[..orig.len() - 1]);
        let good = src("good", &orig);
        let out = format!("{d}/out");
        let usage = format!("pack：{USAGE}");
        let spec_err = "pack：参数写成 目标=路径，目标是 aarch64-apple-darwin、x86_64-apple-darwin、x86_64-unknown-linux-musl、\
                        aarch64-unknown-linux-musl 之一";
        let cases: Vec<(Vec<&str>, String)> = vec![
            (vec![], usage.clone()),
            (vec![&good], usage),
            (vec![&good, &out, "aarch64-apple-darwin"], spec_err.into()),
            (vec![&good, &out, "aarch64-apple-darwin="], spec_err.into()),
            (vec![&good, &out, "riscv64-unknown-linux-musl=/bin/sh"], spec_err.into()),
            (vec![&good, &out, "--foo"], spec_err.into()),
            (
                vec![&none, &out, &one],
                format!("pack：{none} 里没有（或者不止一处）以 HELPER_SUMS=\"\" 开头的那一行（已经接过辅助程序了？）"),
            ),
            (
                vec![&two, &out, &one],
                format!("pack：{two} 里没有（或者不止一处）以 HELPER_SUMS=\"\" 开头的那一行（已经接过辅助程序了？）"),
            ),
            (vec![&ending, &out, &one], format!("pack：{ending} 的结尾不是 main \"$@\" 和 exit（已经接过辅助程序了？）")),
            (vec![&no_newline, &out, &one], format!("pack：{no_newline} 的结尾不是 main \"$@\" 和 exit（已经接过辅助程序了？）")),
            (vec![&good, &out, "--only"], "pack：--only 后面要点名至少一个目标（目标=路径）".into()),
            (
                vec![&good, &out, "--only", "x86_64-apple-darwin=/nonexistent/cca-core"],
                "pack：读不了 x86_64-apple-darwin 的辅助程序（[Errno 2] No such file or directory: '/nonexistent/cca-core'）：\
                 先编出来，或者用 x86_64-apple-darwin=路径 指定"
                    .into(),
            ),
            (vec!["/nonexistent/cca", &out, &one], "pack：读不了 /nonexistent/cca（[Errno 2] No such file or directory）".into()),
            (
                vec![&good, "/nonexistent/out", "--only", &one],
                "pack：写不了 /nonexistent/out（[Errno 2] No such file or directory）".into(),
            ),
        ];
        for (args, want) in cases {
            assert_eq!(run(&args).unwrap_err(), want, "{args:?}");
            assert!(!std::path::Path::new(&out).exists(), "{args:?}：出错时不写输出文件");
        }
        // 已经接过的不再接（HELPER_SUMS 那一行换掉了）
        run(&[&good, &out, "--only", &one]).unwrap();
        let again = format!("{d}/again");
        assert!(run(&[&out, &again, "--only", &one]).unwrap_err().contains("已经接过辅助程序了？"));
        assert_eq!(crate::py::fs::listdir_sorted(d).unwrap(), ["ending", "good", "h", "no-newline", "none", "out", "two"]);
    }

    #[test]
    fn sizes_round_like_python() {
        // Python 的 '%.1f' % (n / 1e6)（正好在一半上的照「四舍六入五成双」）
        let want = [
            (0, "0.0"),
            (49_999, "0.0"),
            (50_000, "0.1"),
            (150_000, "0.1"),
            (250_000, "0.2"),
            (750_000, "0.8"),
            (1_250_000, "1.2"),
            (1_750_000, "1.8"),
            (2_250_000, "2.2"),
            (4_680_000, "4.7"),
            (4_650_000, "4.7"),
            (5_349_999, "5.3"),
            (5_350_000, "5.3"),
            (12_345_678, "12.3"),
        ];
        for (n, s) in want {
            assert_eq!(format!("{:.1}", n as f64 / 1e6), s, "{n}");
        }
    }
}
