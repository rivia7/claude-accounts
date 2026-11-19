//! `zlib`：压缩和限长的解压（折合金额和别的设备交换的那一份是压缩的 JSON）。解压的结果照 CPython 核对过
//! （tests/vectors/zlib.json，tools/gen_vectors.py 生成）。
//!
//! 压缩出来的字节和 CPython 的 zlib 不逐字节一样（两个实现找重复的办法不同，长短也会差一点），解出来的内容一样：
//! 用在只要求“对方解得开”的地方；要逐字节一样的地方（摘要的输入）不能用压缩过的。

use miniz_oxide::inflate::TINFLStatus;

/// `zlib.compress(data, level)`
pub fn compress(data: &[u8], level: u8) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, level)
}

/// `z = zlib.decompressobj(); raw = z.decompress(data, max_length)`：返回 (raw, `bool(z.unconsumed_tail)`)。
/// 最多解出 max_length 个字节，还有输入没解完时第二项是 true；流在中间断了（输入不全）照 Python 的样子给出已经解出来的、
/// 不算错；流结束以后多出来的输入不管（Python 里进 `z.unused_data`）。数据坏了（头不对、校验和不对）是 None（`zlib.error`）
pub fn decompress_limited(data: &[u8], max_length: usize) -> Option<(Vec<u8>, bool)> {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    // 自己看着解到哪了：miniz_oxide 现成的 decompress_to_vec_zlib_with_limit 出错时交回来的缓冲区没有截到实际解出来的长度
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut state = Box::<DecompressorOxide>::default();
    let mut out = vec![0u8; data.len().saturating_mul(2).min(max_length)];
    let (mut input, mut pos) = (data, 0);
    loop {
        let (status, used, wrote) = decompress(&mut state, input, &mut out, pos, flags);
        pos += wrote;
        input = &input[used.min(input.len())..];
        match status {
            TINFLStatus::Done => break,
            TINFLStatus::HasMoreOutput if out.len() >= max_length => {
                out.truncate(pos);
                return Some((out, true));
            }
            TINFLStatus::HasMoreOutput => {
                let bigger = out.len().saturating_mul(2).clamp(64, max_length);
                out.resize(bigger, 0);
            }
            TINFLStatus::NeedsMoreInput | TINFLStatus::FailedCannotMakeProgress => break,
            _ => return None,
        }
    }
    out.truncate(pos);
    Some((out, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::{from_hex, sha256_hex};
    use crate::py::json;

    #[test]
    fn decompress_like_cpython() {
        let doc = json::loads(include_str!("../../tests/vectors/zlib.json")).unwrap();
        let cases = doc.as_dict().unwrap().list("cases").unwrap().items();
        assert!(cases.len() > 60);
        for c in cases {
            let c = c.as_dict().unwrap();
            let (name, data) = (c.at("name").py_str(), from_hex(&c.at("hex").py_str()).unwrap());
            let got = decompress_limited(&data, c.at("limit").as_int().unwrap() as usize);
            if c.truthy("error") {
                assert!(got.is_none(), "{name}：CPython 说数据坏了");
                continue;
            }
            let (raw, tail) = got.unwrap_or_else(|| panic!("{name}：CPython 解得出来"));
            assert_eq!((raw.len() as i64, sha256_hex(&raw), tail), (c.at("len").as_int().unwrap(), c.at("sha256").py_str(), c.truthy("tail")), "{name}");
        }
    }

    #[test]
    fn compress_round_trips() {
        for plain in [&b""[..], b"a", b"hello hello hello hello hello", &[7u8; 100_000]] {
            let z = compress(plain, 9);
            assert_eq!(decompress_limited(&z, 4 << 20), Some((plain.to_vec(), false)));
        }
        // 能压的压得下去
        assert!(compress(&[7u8; 100_000], 9).len() < 500);
    }
}
