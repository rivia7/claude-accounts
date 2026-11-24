//! plist get 文件 路径…：属性列表（XML 或二进制）里的几个值，制表符隔开、一行（假 launchctl 读 cca 写的 launchd 配置用）。
//! 路径用 . 分开：字典里按键、数组里按下标，@N 是字典的第 N 个键（按文件里的先后）。
//! 取不到、不是字符串：stdout 什么都不说，stderr 一行（照原来那句 Python 的异常的说法），退出码 1

use super::*;
use kit::plist::{self, Value as P};

pub fn run(a: &[String]) -> R<i32> {
    if a.first().map(String::as_str) != Some("get") || a.len() < 2 {
        eprintln!("用法：cca-test plist get 文件 路径…");
        return Ok(2);
    }
    let v = match plist::load_file(std::path::Path::new(&a[1])) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return fail(format!("plistlib.InvalidFileException: {e}")),
        Err(e) => return fail(os_text(&e, Some(&a[1]))),
    };
    let mut out = Vec::new();
    for (i, path) in a[2..].iter().enumerate() {
        out.push(lookup(&v, path, i)?);
    }
    print(&out.join("\t"));
    Ok(0)
}

/// 第 item 项（从 0 数）：照路径一段段取下去，最后得是字符串（"\t".join 只收字符串）
fn lookup(v: &P, path: &str, item: usize) -> R<String> {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for (i, seg) in segs.iter().enumerate() {
        if let (Some(n), P::Dict(d)) = (seg.strip_prefix('@'), cur) {
            // list(d)[N]：取到的是键（字符串），后面不能再往下取
            let Ok(n) = n.parse::<usize>() else {
                return fail(format!("ValueError: invalid literal for int() with base 10: {}", repr_str(n)));
            };
            if i + 1 != segs.len() {
                return fail("TypeError: string indices must be integers, not 'str'");
            }
            return d.get(n).map(|(k, _)| k.clone()).ok_or_else(|| Fail("IndexError: list index out of range".into()));
        }
        cur = match cur {
            P::Dict(d) => &d.iter().find(|(k, _)| k == seg).ok_or_else(|| Fail(format!("KeyError: {}", repr_str(seg))))?.1,
            P::Array(l) => {
                let Ok(n) = seg.parse::<usize>() else {
                    return fail("TypeError: list indices must be integers or slices, not str");
                };
                l.get(n).ok_or_else(|| Fail("IndexError: list index out of range".into()))?
            }
            P::Str(_) => return fail("TypeError: string indices must be integers, not 'str'"),
            P::Data(_) => return fail("TypeError: byte indices must be integers or slices, not str"),
            other => return fail(format!("TypeError: '{}' object is not subscriptable", kind(other))),
        };
    }
    match cur {
        P::Str(s) => Ok(s.clone()),
        other => fail(format!("TypeError: sequence item {item}: expected str instance, {} found", kind(other))),
    }
}

/// plistlib 读出来的值在 Python 里的类型名
fn kind(v: &P) -> &'static str {
    match v {
        P::Str(_) => "str",
        P::Int(_) => "int",
        P::Real(_) => "float",
        P::Bool(_) => "bool",
        P::Data(_) => "bytes",
        P::Date(_) => "datetime",
        P::Array(_) => "list",
        P::Dict(_) => "dict",
        P::Uid(_) => "UID",
    }
}
