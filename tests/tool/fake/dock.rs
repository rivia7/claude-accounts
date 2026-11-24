//! dockpl dock.plist set|get|add …：程序坞的设置（测试里是 plist 文件，cca 直接读写它），原来的 dockpl.py。
//! set 最近使用的 [固定的] [show-recents]（JSON 里写 App 名，A:nobid 是不带 bundle id 的图标），get 列出最近使用的，add App 加一个。
//! 读 XML 和二进制的（cca 写的是二进制的），写的是 XML（和 plistlib.dump 逐字节一样）

use super::claude::arg;
use super::*;
use kit::plist::{self, Value as P};

pub fn run(a: &[String]) -> R<i32> {
    let (dom, cmd) = (arg(a, 0)?, arg(a, 1)?);
    let r = dirname(dom);
    match cmd {
        "set" => {
            let mut d = vec![("recent-apps".to_string(), tiles(&r, arg(a, 2)?)?)];
            d.push(("persistent-apps".to_string(), if a.len() > 3 { tiles(&r, &a[3])? } else { P::Array(Vec::new()) }));
            if a.len() > 4 {
                d.push(("show-recents".to_string(), P::Bool(a[4] == "true")));
            }
            save(dom, &P::Dict(d))?;
        }
        "get" => {
            let d = load(dom)?;
            let mut labels = Vec::new();
            let recent = match dict_get(&d, "recent-apps")? {
                None => &[][..],
                Some(P::Array(l)) => &l[..],
                Some(_) => return fail("TypeError: string indices must be integers"),
            };
            for t in recent {
                let label =
                    t.get("tile-data").and_then(|x| x.get("file-label")).ok_or_else(|| Fail("KeyError: 'tile-data'".into()))?;
                labels.push(
                    label.as_str().ok_or_else(|| Fail("TypeError: sequence item: expected str instance".into()))?.to_string(),
                );
            }
            print(&labels.join(" "));
        }
        "add" => {
            let mut d = load(dom)?;
            let name = arg(a, 2)?;
            let t = tile(&r, name, &format!("{name}-back"), true)?;
            let P::Dict(items) = &mut d else { return fail("AttributeError: object has no attribute 'setdefault'") };
            match items.iter_mut().find(|(k, _)| k == "recent-apps") {
                Some((_, P::Array(list))) => list.push(t),
                Some(_) => return fail("AttributeError: object has no attribute 'append'"),
                None => items.push(("recent-apps".to_string(), P::Array(vec![t]))),
            }
            save(dom, &d)?;
        }
        _ => {}
    }
    Ok(0)
}

/// 读：文件不在（读不了）是空的字典；不是属性列表是异常
fn load(dom: &str) -> R<P> {
    match plist::load_file(std::path::Path::new(dom)) {
        Err(_) => Ok(P::Dict(Vec::new())),
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => fail(format!("plistlib.InvalidFileException: {e}")),
    }
}

/// d.get(k)：d 得是字典
fn dict_get<'a>(d: &'a P, k: &str) -> R<Option<&'a P>> {
    match d {
        P::Dict(_) => Ok(d.get(k)),
        _ => fail("AttributeError: object has no attribute 'get'"),
    }
}

/// 写：旁边的 .tmp 写好再改名
fn save(dom: &str, d: &P) -> R<()> {
    let tmp = format!("{dom}.tmp");
    let bytes = plist::dumps_xml(d).map_err(|e| Fail(format!("TypeError: {e}")))?;
    std::fs::write(&tmp, bytes).map_err(|e| Fail(os_text(&e, Some(&tmp))))?;
    replace(&tmp, dom)
}

/// 一个图标：App 在 dock.plist 旁边（<名字>.app），bid 时照它的 Info.plist 写上 bundle id
fn tile(r: &str, name: &str, label: &str, bid: bool) -> R<P> {
    let app = join(&[r, &format!("{name}.app")]);
    let mut d = vec![
        (
            "file-data".to_string(),
            P::Dict(vec![
                ("_CFURLString".to_string(), P::Str(format!("file://{app}/"))),
                ("_CFURLStringType".to_string(), P::Int(15)),
            ]),
        ),
        ("file-label".to_string(), P::Str(label.to_string())),
    ];
    if bid {
        let info = join(&[&app, "Contents", "Info.plist"]);
        let v = match plist::load_file(std::path::Path::new(&info)) {
            Err(e) => return Err(Fail(os_text(&e, Some(&info)))),
            Ok(Err(e)) => return fail(format!("plistlib.InvalidFileException: {e}")),
            Ok(Ok(v)) => v,
        };
        let id = v.get("CFBundleIdentifier").ok_or_else(|| Fail("KeyError: 'CFBundleIdentifier'".into()))?;
        d.push(("bundle-identifier".to_string(), id.clone()));
    }
    Ok(P::Dict(vec![("tile-data".to_string(), P::Dict(d)), ("tile-type".to_string(), P::Str("file-tile".into()))]))
}

/// set 的参数：JSON 数组里的 App 名（带 :nobid 的不写 bundle id），标签是原样的那一项
fn tiles(r: &str, spec: &str) -> R<P> {
    let v = json::parse(spec).map_err(|e| Fail(format!("json.decoder.JSONDecodeError: {e}")))?;
    let mut out = Vec::new();
    for s in super::claude::iter_list(&v)? {
        let s = super::claude::str_arg(&s)?.to_string();
        let name = s.split(':').next().unwrap_or_default();
        out.push(tile(r, name, &s, !s.contains(":nobid"))?);
    }
    Ok(P::Array(out))
}
