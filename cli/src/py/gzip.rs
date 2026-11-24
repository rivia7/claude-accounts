//! `gzip`：一个 gzip 成员（RFC 1952）的压缩和限长的解压。发布的 cca 把四种机器的辅助程序 gzip 再 base64 接在文件最后
//! （`cca-core pack`，见 cmd/pack.rs），第一次运行时用系统的 `gunzip` 解开（cca 的 `helper_unpack`）。
//!
//! 压缩出来的字节和 CPython 的 `gzip.compress` 不一样（deflate 用的是 miniz_oxide，和 zlib 找重复的办法不同，长短也会差一点）：
//! 要求的只是 `gunzip`、Python 的 gzip 模块解得开，同样的输入出同样的字节（不带时间戳、文件名）。
//! 要逐字节一样的地方（校验值、摘要）对原样的数据算，不对压缩过的算。

use super::zip::crc32;
use miniz_oxide::inflate::core::{decompress as inflate, inflate_flags, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

const MAGIC: [u8; 2] = [0x1f, 0x8b];
/// 压缩方法：只有 deflate 一种（8）
const DEFLATE: u8 = 8;
// 头里的标志位（FTEXT 0x01 只是提示，不用管）
const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
/// 保留的三位：RFC 1952 要求解的一方见到就报错（gzip、zlib 都是）
const RESERVED: u8 = 0xe0;
/// 固定长度的头：标识两字节、方法、标志、修改时间四字节、XFL、系统
const HEAD: usize = 10;
/// 结尾：原样数据的 CRC32、长度（除以 2^32 的余数），都是小端
const TAIL: usize = 8;

/// `gzip.compress(raw, level, mtime=0)`：一个成员，修改时间是 0、不带文件名，XFL 照 zlib（9 级是 2，0、1 级是 4，别的是 0），
/// 系统那一字节是 255（不说是哪种系统：CPython 也是这样，同样的输入在哪台机器上都出同样的字节）。level 只有 0～9，大的当 9
pub fn compress(raw: &[u8], level: u8) -> Vec<u8> {
    let level = level.min(9);
    let xfl = match level {
        9 => 2,
        0 | 1 => 4,
        _ => 0,
    };
    let body = miniz_oxide::deflate::compress_to_vec(raw, level);
    let mut gz = Vec::with_capacity(HEAD + body.len() + TAIL);
    gz.extend_from_slice(&[MAGIC[0], MAGIC[1], DEFLATE, 0, 0, 0, 0, 0, xfl, 255]);
    gz.extend_from_slice(&body);
    gz.extend_from_slice(&crc32(raw, 0).to_le_bytes());
    gz.extend_from_slice(&(raw.len() as u32).to_le_bytes()); // 截成 32 位就是除以 2^32 的余数
    gz
}

/// 解开一个 gzip 成员：头里的附加字段、文件名、注释、头的校验跳过不看（和 Python 的 gzip 模块一样），解出来的数据的
/// CRC32 和长度要和结尾记的对得上；后面不许再有别的（只认一个成员）。
///
/// 解出来超过 max 个字节就不往下解、算错，而且不会先要这么大的内存：缓冲区跟着解出来的长度一点点变大，最多到 max；
/// 结尾记的长度不能信，不拿它定大小。数据坏了、不完整、不是 gzip 都是 Err（一句给人看的说明），不会 panic
/// （发布版是 panic = "abort"，panic 就是整个进程没了）
pub fn decompress(gz: &[u8], max: usize) -> Result<Vec<u8>, String> {
    let start = header_len(gz)?;
    let (raw, used) = inflate_raw(&gz[start..], max)?;
    let tail = &gz[start + used..];
    if tail.len() < TAIL {
        return Err("数据不完整：缺了结尾的校验值和长度".into());
    }
    if le32(&tail[..4]) != crc32(&raw, 0) {
        return Err("校验值（CRC32）对不上：数据坏了".into());
    }
    if le32(&tail[4..TAIL]) != raw.len() as u32 {
        return Err("长度和结尾记的对不上：数据坏了".into());
    }
    if tail.len() > TAIL {
        return Err(format!("结尾后面还有 {} 个字节：只认一个 gzip 成员", tail.len() - TAIL));
    }
    Ok(raw)
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// 头有多长（压缩的数据从哪开始）
fn header_len(gz: &[u8]) -> Result<usize, String> {
    let short = || "数据不完整：gzip 的头都不全".to_string();
    if !gz.starts_with(&MAGIC) {
        // 只有一两个字节、对得上开头的：是不完整，不是认错了格式
        return Err(if !gz.is_empty() && MAGIC.starts_with(gz) { short() } else { "不是 gzip 格式".into() });
    }
    if gz.len() < HEAD {
        return Err(short());
    }
    if gz[2] != DEFLATE {
        return Err(format!("压缩方法是 {}，不是 deflate（8）", gz[2]));
    }
    let flags = gz[3];
    if flags & RESERVED != 0 {
        return Err(format!("头里有保留的标志位（{flags:#04x}）：不是这一版 gzip 的格式"));
    }
    let mut i = HEAD;
    if flags & FEXTRA != 0 {
        let n = gz.get(i..i + 2).ok_or_else(short)?;
        i += 2 + u16::from_le_bytes([n[0], n[1]]) as usize;
        if i > gz.len() {
            return Err(short());
        }
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            // 以 0 结尾的一串（文件名、注释）
            i += gz[i..].iter().position(|&b| b == 0).ok_or_else(short)? + 1;
        }
    }
    if flags & FHCRC != 0 {
        i += 2;
        if i > gz.len() {
            return Err(short());
        }
    }
    Ok(i)
}

/// 解开不带头尾的 deflate 数据：返回解出来的和用掉了多少输入（后面紧接着就是结尾的 8 个字节）
fn inflate_raw(data: &[u8], max: usize) -> Result<(Vec<u8>, usize), String> {
    // 输出放在一整块缓冲区里（往回引用的数据都在里面），不够了翻倍，最多到 max。先按压缩数据的 4 倍要、至少 64 KB
    const MIN: usize = 64 << 10;
    let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut state = Box::<DecompressorOxide>::default();
    let mut out = vec![0u8; data.len().saturating_mul(4).max(MIN).min(max)];
    let (mut used, mut pos) = (0, 0);
    loop {
        let (status, n_in, n_out) = inflate(&mut state, &data[used..], &mut out, pos, flags);
        used = (used + n_in).min(data.len());
        pos = (pos + n_out).min(out.len());
        match status {
            TINFLStatus::Done => {
                out.truncate(pos);
                return Ok((out, used));
            }
            TINFLStatus::HasMoreOutput if out.len() >= max => return Err(format!("解出来超过 {max} 个字节：不解了")),
            TINFLStatus::HasMoreOutput => {
                let bigger = out.len().saturating_mul(2).max(MIN).min(max);
                out.resize(bigger, 0);
            }
            // 输入用完了，压缩的数据还没到头（没带 HAS_MORE_INPUT 时是后一种）
            TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => {
                return Err("数据不完整：压缩的数据中途断了".into())
            }
            _ => return Err("压缩的数据坏了".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系统的 gzip（macOS 自带的 /usr/bin/gzip -9）压的：头里带文件名（fixture.txt）、修改时间，系统那一字节是 3（Unix）
    const SYSTEM_GZIP: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x08, 0x00, 0x63, 0x04, 0x6b, 0x02, 0x03, 0x66, 0x69, 0x78, 0x74, 0x75, 0x72, 0x65, 0x2e, 0x74, 0x78,
        0x74, 0x00, 0x4b, 0x4e, 0x4e, 0xd4, 0x4d, 0xce, 0x2f, 0x4a, 0x55, 0x48, 0xaf, 0xca, 0x2c, 0x50, 0x48, 0xcb, 0xac, 0x28,
        0x29, 0x2d, 0x4a, 0xe5, 0xca, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc0, 0x41, 0x72, 0x01, 0x00, 0xc7, 0xdd, 0x24, 0xd2, 0x34,
        0x00, 0x00, 0x00,
    ];
    /// Python 的 `gzip.compress(FIXTURE, 9, mtime=0)`（CPython 3.14）
    const PYTHON_GZIP: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x4b, 0x4e, 0x4e, 0xd4, 0x4d, 0xce, 0x2f, 0x4a, 0x55, 0x48,
        0xaf, 0xca, 0x2c, 0x50, 0x48, 0xcb, 0xac, 0x28, 0x29, 0x2d, 0x4a, 0xe5, 0xca, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc0, 0x41,
        0x72, 0x01, 0x00, 0xc7, 0xdd, 0x24, 0xd2, 0x34, 0x00, 0x00, 0x00,
    ];
    const FIXTURE: &[u8] = b"cca-core gzip fixture\nhello hello hello hello hello\n";

    /// 看着像程序的数据：一段段随机的字节夹着重复的片段（xorshift，每次一样）
    fn random_ish(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut v = Vec::with_capacity(len);
        while v.len() < len {
            let r = next();
            if r % 4 == 0 && v.len() > 64 {
                // 往回抄一段（像程序里重复出现的指令、字符串）
                let n = (r >> 8) as usize % 48 + 3;
                let from = v.len() - 1 - (r >> 16) as usize % (v.len().min(4096) - 1);
                for k in 0..n {
                    v.push(v[from + k % (v.len() - from)]);
                }
            } else {
                v.extend_from_slice(&r.to_le_bytes()[..(r >> 60) as usize % 8 + 1]);
            }
        }
        v.truncate(len);
        v
    }

    #[test]
    fn round_trips() {
        let cases: [(&str, Vec<u8>); 5] = [
            ("空的", Vec::new()),
            ("一个字节", b"a".to_vec()),
            ("短的", FIXTURE.to_vec()),
            ("1 MiB 像程序的", random_ish(1 << 20, 7)),
            ("很能压的", b"cca ".repeat(300_000)),
        ];
        for (name, raw) in &cases {
            for level in [1, 6, 9] {
                let gz = compress(raw, level);
                assert_eq!(decompress(&gz, raw.len()).as_ref(), Ok(raw), "{name}（{level} 级）");
            }
        }
        // 很能压的压得下去；随机的也不会胀很多
        assert!(compress(&cases[4].1, 9).len() < 5_000);
        assert!(compress(&cases[3].1, 9).len() < (1 << 20) + 1_000);
    }

    #[test]
    fn header_like_python() {
        // 同样的输入出同样的字节；头照 CPython：修改时间 0，XFL 照级别，系统 255；结尾是 CRC32 和长度
        let gz = compress(FIXTURE, 9);
        assert_eq!(gz, compress(FIXTURE, 9));
        assert_eq!(gz[..HEAD], PYTHON_GZIP[..HEAD]);
        assert_eq!(gz[gz.len() - TAIL..], PYTHON_GZIP[PYTHON_GZIP.len() - TAIL..]);
        assert_eq!([compress(b"x", 0)[8], compress(b"x", 1)[8], compress(b"x", 6)[8], compress(b"x", 9)[8]], [4, 4, 0, 2]);
        assert_eq!(compress(b"x", 12), compress(b"x", 9));
    }

    #[test]
    fn reads_other_compressors() {
        assert_eq!(decompress(SYSTEM_GZIP, 1 << 20).as_deref(), Ok(FIXTURE));
        assert_eq!(decompress(PYTHON_GZIP, 1 << 20).as_deref(), Ok(FIXTURE));
        // 头里别的几样（附加字段、注释、头的校验）也跳得过去：拿 Python 那一份改出来
        let mut gz = PYTHON_GZIP[..HEAD].to_vec();
        gz[3] = FHCRC | FEXTRA | FNAME | FCOMMENT | 0x01;
        gz.extend_from_slice(&[5, 0, b'A', b'B', 3, 0, 0]);
        gz.extend_from_slice(b"name.txt\0");
        gz.extend_from_slice("注释\n".as_bytes());
        gz.push(0);
        gz.extend_from_slice(&[0x12, 0x34]);
        gz.extend_from_slice(&PYTHON_GZIP[HEAD..]);
        assert_eq!(decompress(&gz, 1 << 20).as_deref(), Ok(FIXTURE));
        // 大一些、分好几块的：现场用系统的 gzip -9 压
        let raw = random_ish(300_000, 11);
        let gz = run_gzip(&["-9", "-c"], &raw);
        assert_eq!(decompress(&gz, raw.len()), Ok(raw));
    }

    /// 系统的 gzip：在一个临时目录里，输入写成文件（不经管道：两头都大的时候会卡住）
    fn run_gzip(args: &[&str], input: &[u8]) -> Vec<u8> {
        let dir = crate::py::fs::TempDir::new().unwrap();
        let f = format!("{}/in", dir.path);
        std::fs::write(&f, input).unwrap();
        let bin = if std::path::Path::new("/usr/bin/gzip").exists() { "/usr/bin/gzip" } else { "gzip" };
        let out = std::process::Command::new(bin).args(args).arg(&f).output().unwrap();
        assert!(out.status.success(), "gzip {args:?}：{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }

    #[test]
    fn system_gzip_reads_ours() {
        for raw in [Vec::new(), FIXTURE.to_vec(), random_ish(1 << 20, 3), vec![0u8; 3 << 20]] {
            for level in [1, 9] {
                assert!(run_gzip(&["-dc"], &compress(&raw, level)) == raw, "{} 字节，{level} 级", raw.len());
            }
        }
    }

    #[test]
    fn refuses_bad_input() {
        let good = compress(FIXTURE, 9);
        let with = |at: usize, b: u8| {
            let mut g = good.clone();
            g[at] = b;
            g
        };
        let head = |flags: u8, rest: &[u8]| {
            let mut g = vec![0x1f, 0x8b, 8, flags, 0, 0, 0, 0, 0, 255];
            g.extend_from_slice(rest);
            g
        };
        let n = good.len();
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("空的", Vec::new(), "不是 gzip"),
            ("只有标识", vec![0x1f, 0x8b], "头都不全"),
            ("头不全", good[..9].to_vec(), "头都不全"),
            ("标识不对", with(1, 0x8c), "不是 gzip"),
            ("不是 gzip", b"#!/bin/bash\necho hi\n".to_vec(), "不是 gzip"),
            ("压缩方法不对", with(2, 7), "不是 deflate"),
            ("保留的标志位", with(3, 0x20), "保留的标志位"),
            ("附加字段的长度不全", head(FEXTRA, &[5]), "头都不全"),
            ("附加字段超出", head(FEXTRA, &[200, 0, 1, 2, 3]), "头都不全"),
            ("文件名没有结尾", head(FNAME, b"abc"), "头都不全"),
            ("注释没有结尾", head(FCOMMENT, b"abc"), "头都不全"),
            ("头的校验不全", head(FHCRC, &[1]), "头都不全"),
            ("只有头", good[..HEAD].to_vec(), "中途断了"),
            ("数据坏了", head(0, &[0x07, 0, 0, 0, 0, 0, 0, 0, 0]), "数据坏了"),
            ("数据断了", good[..HEAD + 5].to_vec(), "中途断了"),
            ("没有结尾", good[..n - TAIL].to_vec(), "缺了结尾"),
            ("结尾不全", good[..n - 3].to_vec(), "缺了结尾"),
            ("校验值不对", with(n - TAIL, good[n - TAIL] ^ 1), "CRC32"),
            ("长度不对", with(n - 1, 1), "长度"),
            ("后面还有", [good.clone(), vec![0]].concat(), "只认一个"),
            ("两个成员", [good.clone(), good.clone()].concat(), "只认一个"),
        ];
        for (name, gz, want) in cases {
            match decompress(&gz, 1 << 20) {
                Err(e) => assert!(e.contains(want), "{name}：{e}"),
                Ok(_) => panic!("{name}：应该解不开"),
            }
        }
        // 截到哪都是错，不会 panic；改掉哪一位都不会 panic，解得开的（改的是修改时间、XFL、系统、FTEXT，或者最后一字节里
        // 用不到的那几位）一定是原来的数据
        for k in 0..n {
            assert!(decompress(&good[..k], 1 << 20).is_err(), "截到 {k}");
            for bit in [0x01, 0x10, 0x80] {
                if let Ok(raw) = decompress(&with(k, good[k] ^ bit), 1 << 20) {
                    assert_eq!(raw, FIXTURE, "改了第 {k} 个字节");
                }
            }
        }
        // 随便什么字节都不会 panic
        for len in [0, 1, 10, 11, 100, 5000] {
            let mut junk = random_ish(len, len as u64 + 5);
            let _ = decompress(&junk, 1 << 20);
            if len >= HEAD {
                junk[..4].copy_from_slice(&[0x1f, 0x8b, 8, 0]);
                assert!(decompress(&junk, 1 << 20).is_err());
            }
        }
    }

    #[test]
    fn size_cap() {
        let raw = random_ish(200_000, 9);
        let gz = compress(&raw, 9);
        assert_eq!(decompress(&gz, raw.len()).as_ref(), Ok(&raw));
        assert!(decompress(&gz, raw.len() - 1).unwrap_err().contains("超过"));
        assert!(decompress(&compress(b"x", 9), 0).unwrap_err().contains("超过"));
        assert_eq!(decompress(&compress(b"", 9), 0), Ok(Vec::new()));
        // 压得很小、解开很大的（16 MiB 的 0，压完几十 KB）：到了上限就停下，不先要 16 MiB
        let bomb = compress(&vec![0u8; 16 << 20], 1);
        assert!(bomb.len() < 100_000);
        assert!(decompress(&bomb, 1 << 20).unwrap_err().contains("超过 1048576 个字节"));
        // 结尾记的长度是假的（说有 4 GB）：不拿它要内存，照样按实际的数据核对
        let mut lie = compress(FIXTURE, 9);
        let n = lie.len();
        lie[n - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decompress(&lie, usize::MAX).unwrap_err().contains("长度"));
    }
}
