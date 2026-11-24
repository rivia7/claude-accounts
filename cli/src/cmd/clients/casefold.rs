//! `unicodedata.normalize('NFD', s).casefold()`：安装包里的名字按“不分大小写、不分 Unicode 写法”查重（macOS 的文件系统就是
//! 这样认名字的）。兼容层没有 casefold，先放在这里：表是拿 CPython 3.14 的 `str.casefold()` 生成的（`casefold_table.rs`）。
//!
//! NFD 用 `unicode-normalization` 库，它的 Unicode 版本（18.0）比 CPython 3.14 的（16.0）新。库认、CPython 还不认的字符——
//! 新收进来的带分解的字符、新的组合符号——两边结果会不一样（Python 原样留着、不参与重排）：这种字符记在 `NEWER` 里，照 Python 的
//! 来。单元测试拿 CPython 的整张分解表、整张组合类表（tests/vectors/clients.json 里记的是摘要）核对；对不上时（换了库的版本）
//! 照测试里写的办法补上 `NEWER`。

#[path = "casefold_table.rs"]
mod table;

use unicode_normalization::UnicodeNormalization;

/// Unicode 16.0 以后才有的、带分解或者组合类不是 0 的字符（`unicode-normalization` 认、CPython 3.14 不认）：照 Python 的原样留着。
/// 现在是 68 个新的组合符号（带分解的新字符一个也没有）。当初是拿 CPython 3.14 的 unicodedata 和库导出的表比出来的
/// （脚本 cli/tools/gen_clients_tables.py newer 在 git 历史里）；换库的版本时怎么补见下面的单元测试
const NEWER: &[(u32, u32)] = &[
    (0x5c8, 0x5c9),
    (0x1acf, 0x1af0),
    (0x10ecb, 0x10ecf),
    (0x10ef0, 0x10efb),
    (0x1d127, 0x1d128),
    (0x1d250, 0x1d252),
    (0x1d25b, 0x1d25c),
    (0x1d25f, 0x1d25f),
    (0x1d280, 0x1d281),
    (0x1e6e3, 0x1e6e3),
    (0x1e6e6, 0x1e6e6),
    (0x1e6ee, 0x1e6ef),
    (0x1e6f5, 0x1e6f5),
];

fn newer(c: char) -> bool {
    let x = c as u32;
    NEWER.iter().any(|&(a, b)| a <= x && x <= b)
}

/// `unicodedata.normalize('NFD', s)`
pub fn nfd(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    if !s.chars().any(newer) {
        return s.nfd().collect();
    }
    // 有库认、Python 不认的字符：它们当作不分解、不参与重排的字符，把文字从这些地方切开，各段分别规范化
    let mut out = String::with_capacity(s.len());
    let mut piece = String::new();
    for c in s.chars() {
        if newer(c) {
            out.extend(piece.nfd());
            piece.clear();
            out.push(c);
        } else {
            piece.push(c);
        }
    }
    out.extend(piece.nfd());
    out
}

/// `s.casefold()`
pub fn casefold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match table::CASEFOLD.binary_search_by_key(&(c as u32), |&(cp, _)| cp) {
            Ok(i) => out.push_str(table::CASEFOLD[i].1),
            Err(_) => out.push(c),
        }
    }
    out
}

/// `unicodedata.normalize('NFD', name).casefold()`
pub fn normalized(name: &str) -> String {
    casefold(&nfd(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::Sha256Stream;
    use crate::py::json;
    use std::fmt::Write;

    #[test]
    fn folds_like_python() {
        // 对照数据是真的 CPython 3.14 给的（移植时生成的，tests/vectors/clients.json 的 casefold 一节）
        let all = json::loads(include_str!("../../../tests/vectors/clients.json")).unwrap().as_dict().unwrap();
        let v = all.dict("casefold").unwrap();
        let words = v.list("words").unwrap().items();
        assert!(words.len() > 50);
        for row in words {
            let row = row.as_list().unwrap();
            let word = row.at(0).py_str();
            assert_eq!(nfd(&word), row.at(1).py_str(), "NFD {word:?}");
            assert_eq!(normalized(&word), row.at(2).py_str(), "NFD + casefold {word:?}");
        }
        assert_eq!(casefold("ChatGPT.app/Contents/MacOS/ChatGPT"), "chatgpt.app/contents/macos/chatgpt");
        // 表是排好的（查表用二分）
        assert!(table::CASEFOLD.windows(2).all(|w| w[0].0 < w[1].0));

        // 整张表：每个字符分解成什么、组合类是几（重排靠它），都要和 CPython 的一样。算上 NEWER 里的例外以后比摘要；
        // 设了 CCA_CLIENTS_UNICODE_DUMP=文件 时把库自己的（不算例外的）两张表写到那个文件里。换库的版本、摘要对不上时：
        // 换之前、换之后各导出一份，比一下（两份都按码位从小到大，一行一项），多出来的字符就是库新认的，加进 NEWER；
        // 原来就有的字符变了的话 NEWER 管不了，要看那个字符怎么处理
        let (mut nfd_sum, mut ccc_sum, mut dump) = (Sha256Stream::new(), Sha256Stream::new(), String::new());
        let dumping = std::env::var_os("CCA_CLIENTS_UNICODE_DUMP").is_some();
        let hex = |s: &str| s.chars().map(|x| format!("{:x}", x as u32)).collect::<Vec<_>>().join(",");
        let mut parts = Vec::new();
        for cp in 0..0x110000u32 {
            let Some(c) = char::from_u32(cp) else { continue };
            // 绝大多数字符不分解：先问库一声（不用建字符串），会分解的再走真的 nfd()
            parts.clear();
            unicode_normalization::char::decompose_canonical(c, |d| parts.push(d));
            let class = unicode_normalization::char::canonical_combining_class(c);
            if parts != [c] {
                if dumping {
                    writeln!(dump, "NFD {cp:x}:{}", hex(&std::iter::once(c).nfd().collect::<String>())).unwrap();
                }
                let ours = nfd(c.encode_utf8(&mut [0; 4]));
                if ours.chars().ne(std::iter::once(c)) {
                    nfd_sum.update(format!("{cp:x}:{};", hex(&ours)));
                }
            }
            if class != 0 {
                if dumping {
                    writeln!(dump, "CCC {cp:x}:{class}").unwrap();
                }
                if !newer(c) {
                    ccc_sum.update(format!("{cp:x}:{class};"));
                }
            }
        }
        if let Ok(file) = std::env::var("CCA_CLIENTS_UNICODE_DUMP") {
            std::fs::write(file, dump).unwrap();
        }
        let unicode = v.at("unicode").py_str();
        assert_eq!(nfd_sum.hexdigest(), v.at("nfd_table_sha256").py_str(), "分解表和 CPython（Unicode {unicode}）的不一样");
        assert_eq!(ccc_sum.hexdigest(), v.at("ccc_table_sha256").py_str(), "组合类的表和 CPython（Unicode {unicode}）的不一样");
    }
}
