//! 启动器和图标（`launcher_names` 到 `cmd_keychain`）：launchers、keychain。
//!
//! 这一块的子命令都移植了。

use super::{arg, arity, Cmd};
use crate::core::base::CFG;
use crate::core::model::*;
use crate::core::tool::{t, Kind};
use crate::py::fs::{self, load_quiet, put, TempDir};
use crate::py::hash::{sha1_hex, sha256_hex};
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::plist::{self, P};
use crate::py::proc::{Cmd as Proc, In, Out};
use crate::py::text::{self, sh_quote};
use crate::py::uni;
use crate::py::R;
use crate::{dict, out, re};
use std::rc::Rc;
use unicode_normalization::UnicodeNormalization;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("launchers", cmd_launchers), Cmd::all("keychain", cmd_keychain)];

pub const LAUNCHER_MARK: &str = "cca-launcher.json";
pub const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// 起一个不关心结果的命令：不接输入输出，最多等 30 秒，起不来、超时都不管
pub fn subprocess_quiet(argv: &[&str]) {
    let _ = Proc::new(argv).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).timeout(30.0).run();
}

/// 启动器里的脚本。`%(名字)s` 是要填的，`%%` 是一个百分号（Python 的 `%` 格式）
const LAUNCHER_SH: &str = r#"#!/bin/bash
# cca 生成的 %(app)s 启动器：用账号 %(label)s 专属的资料目录和出口打开 %(app)s。
# 账号改名或删除时 cca 会自动更新或删掉它，不用手改。
export PATH="${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin"
CCA="$HOME/.local/bin/cca"
[ -x "$CCA" ] || CCA=%(cca)s
out=$(CCA_GUI=1 "$CCA" %(cmd)s %(key)s 2>&1 </dev/null) && exit 0
osascript - %(title)s "$(printf '%%s\n' "$out" | tail -n 8)" <<'OSA' >/dev/null 2>&1
on run argv
	display alert (item 1 of argv) message (item 2 of argv) as critical
end run
OSA
exit 1
"#;

/// `模板 % {名字: 值}`：填进去的值不再看里面的百分号
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 128);
    let mut rest = template;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(after) = rest.strip_prefix('%') {
            out.push('%');
            rest = after;
        } else {
            let end = rest.find(")s").expect("模板里的 %( 都有 )s");
            let name = &rest[1..end];
            out.push_str(values.iter().find(|(k, _)| *k == name).unwrap_or_else(|| panic!("KeyError: {name}")).1);
            rest = &rest[end + 2..];
        }
    }
    out.push_str(rest);
    out
}

/// 启动器的名字："Claude 别名"（Codex 账号是 "ChatGPT 别名"）；没有别名用邮箱 @ 前面的部分，和别的账号重名时改用完整邮箱
pub fn launcher_names(table: &Dict) -> Vec<(Rc<str>, String)> {
    let short: Vec<(Rc<str>, String)> = table
        .keys()
        .into_iter()
        .map(|k| {
            let (a, e) = parse_key(&k);
            let before_at = e.split('@').next().unwrap_or("");
            let n = if !a.is_empty() { a } else if !before_at.is_empty() { before_at.to_string() } else { k.to_string() };
            (k, n)
        })
        .collect();
    short
        .iter()
        .map(|(k, n)| {
            let (a, e) = parse_key(k);
            let n = if short.iter().filter(|(_, x)| x == n).count() > 1 && a.is_empty() { if e.is_empty() { k.to_string() } else { e } } else { n.clone() };
            (k.clone(), format!("{}{}", t().launcher_prefix, re!(r"[/:]").sub("-", &n)))
        })
        .collect()
}

pub const BADGE_VERSION: i64 = 2;
pub const BADGE_COLORS: [&str; 8] = ["#0A84FF", "#30A46C", "#8E4EC6", "#12A594", "#D6409F", "#3E63DD", "#E5484D", "#B2610B"];

/// 在 Claude 图标右下角画账号标识（AppKit，经 osascript 的 JavaScript 运行；输出 PNG）。
/// 标识必须整个落在原图标的圆角方形里：轮廓一变，macOS 26 起会把整个图标缩小、关进灰色底框
const BADGE_JS: &str = r#"ObjC.import('AppKit');
function run(argv) {
  var src = argv[0], out = argv[1], label = argv[2], hex = argv[3], S = 1024;
  var base = $.NSImage.alloc.initWithContentsOfFile(src);
  if (base.isNil()) throw new Error('no base icon');
  var img = $.NSImage.alloc.initWithSize($.NSMakeSize(S, S)), all = $.NSMakeRect(0, 0, S, S), zero = $.NSMakeRect(0, 0, 0, 0);
  img.lockFocus;
  base.drawInRectFromRectOperationFraction(all, zero, 2, 1.0);
  // 苹果图标网格里圆角方形离画布边 0.0977·S，标识再往里缩 0.022·S（e 是标识外沿离画布边的距离）
  var e = S * (0.0977 + 0.022), w = S * 0.02, d = S * 0.40 - w, x = S - e - d - w / 2, y = e + w / 2;
  var c = parseInt(hex.slice(1), 16);
  var o = $.NSBezierPath.bezierPathWithOvalInRect($.NSMakeRect(x, y, d, d));
  $.NSColor.colorWithSRGBRedGreenBlueAlpha((c >> 16 & 255) / 255, (c >> 8 & 255) / 255, (c & 255) / 255, 1).setFill;
  o.fill;
  o.setLineWidth(w);
  $.NSColor.whiteColor.setStroke;
  o.stroke;
  var font = $.NSFont.boldSystemFontOfSize(d * (label.length > 1 ? 0.36 : 0.52));
  var attrs = $.NSMutableDictionary.dictionary;
  attrs.setObjectForKey(font, 'NSFont');
  attrs.setObjectForKey($.NSColor.whiteColor, 'NSColor');
  var s = $.NSString.alloc.initWithUTF8String(label);
  var ts = s.sizeWithAttributes(attrs);
  s.drawAtPointWithAttributes($.NSMakePoint(x + (d - ts.width) / 2, y + (d - ts.height) / 2), attrs);
  base.drawInRectFromRectOperationFraction(all, zero, 7, 1.0);   // DestinationIn：按原图标的透明度再裁一遍，轮廓分毫不差
  img.unlockFocus;
  var rep = $.NSBitmapImageRep.imageRepWithData(img.TIFFRepresentation);
  var png = rep.representationUsingTypeProperties(4, $.NSDictionary.dictionary);
  if (!png.writeToFileAtomically(out, true)) throw new Error('write failed');
}"#;

/// 图标右下角的账号标识 (字, 颜色)：中文名取头两个字，其它取首字母大写；颜色按账号固定（按 base_order 分，调整账号表的顺序
/// 不会变色），尽量不和别的账号重复
pub fn launcher_badges(c: &Dict, table: &Dict, names: &[(Rc<str>, String)]) -> Vec<(Rc<str>, (String, &'static str))> {
    let mut out = Vec::new();
    let mut used: Vec<&'static str> = Vec::new();
    for k in base_order(c, table, t()) {
        let name = &names.iter().find(|(x, _)| *x == k).unwrap_or_else(|| panic!("KeyError: {k}")).1;
        let n = text::skip(name, text::len(t().launcher_prefix));
        let n = if n.is_empty() { "?" } else { n };
        let first = n.chars().next().expect("上面保证了不是空的");
        let label = if uni::is_wide(first) { n.chars().take(2).filter(|ch| uni::is_wide(*ch)).collect() } else { first.to_uppercase().collect::<String>() };
        // int(sha1 的十六进制, 16) % 颜色数
        let free: Vec<&'static str> = BADGE_COLORS.iter().copied().filter(|color| !used.contains(color)).collect();
        let free = if free.is_empty() { BADGE_COLORS.to_vec() } else { free };
        let i = sha1_hex(acct_id(&k, t())).chars().fold(0usize, |acc, d| (acc * 16 + d.to_digit(16).unwrap_or(0) as usize) % free.len());
        let color = free[i];
        used.push(color);
        out.push((k, (label, color)));
    }
    out
}

/// 返回画好标识的 .icns 内容：osascript 画 1024 的 PNG，sips 缩成各个尺寸，iconutil 打包。哪一步不成都算画不出来
fn render_icon(src: &str, label: &str, color: &str) -> Option<Vec<u8>> {
    let run = |argv: &[&str]| -> Option<()> {
        let done = Proc::new(argv).stdin(In::Null).stdout(Out::Null).stderr(Out::Null).timeout(60.0).run().ok()?;
        (done.rc == 0).then_some(())
    };
    let d = TempDir::new().ok()?;
    let (js, png, iset) = (path::join(&d.path, "badge.js"), path::join(&d.path, "icon.png"), path::join(&d.path, "AppIcon.iconset"));
    std::fs::write(&js, BADGE_JS).ok()?;
    run(&["osascript", "-l", "JavaScript", &js, src, &png, label, color])?;
    std::fs::create_dir(&iset).ok()?;
    for n in [16, 32, 128, 256, 512] {
        let (one, two) = (n.to_string(), (2 * n).to_string());
        run(&["sips", "-z", &one, &one, &png, "--out", &path::join(&iset, &format!("icon_{n}x{n}.png"))])?;
        run(&["sips", "-z", &two, &two, &png, "--out", &path::join(&iset, &format!("icon_{n}x{n}@2x.png"))])?;
    }
    let icns = path::join(&d.path, "AppIcon.icns");
    run(&["iconutil", "-c", "icns", &iset, "-o", &icns])?;
    std::fs::read(&icns).ok()
}

/// 启动器的图标：本机 App 的图标，右下角加账号标识；原图标或标识变了才重画。读不到原图标（OSError / ValueError）就不管图标
fn write_icon(b: &str, app: &str, badge: &(String, &'static str), mark: &Dict, mark_path: &str, icns: &str) -> R<()> {
    let info = plist::loads(&std::fs::read(path::join_all(&[app, "Contents", "Info.plist"]))?)?;
    let icon = match info {
        P::Dict(_) => match info.get("CFBundleIconFile").filter(|v| v.truthy()) {
            Some(P::Str(s)) => s.clone(),
            Some(other) => panic!("AttributeError: CFBundleIconFile 不是字符串（{other:?}）"),
            None => String::new(),
        },
        other => panic!("AttributeError: Info.plist 不是字典（{other:?}）"),
    };
    let icon = path::join_all(&[app, "Contents", "Resources", &if icon.ends_with(".icns") { icon } else { format!("{icon}.icns") }]);
    let src = std::fs::read(&icon)?;
    let mut seed = src.clone();
    seed.extend_from_slice(format!("|{}|{}|{}", badge.0, badge.1, BADGE_VERSION).as_bytes());
    let digest = sha1_hex(&seed);
    mark.set("icon", digest.as_str());
    let old = match std::fs::read(mark_path).map(|raw| json::loads_file_bytes(&raw)) {
        Ok(Ok(V::Dict(d))) => d,
        Ok(Ok(other)) => panic!("AttributeError: {mark_path} 不是对象（{}）", other.py_repr()),
        _ => Dict::new(),
    };
    if !old.get("icon").is_some_and(|i| i.as_str() == Some(digest.as_str())) || !path::exists(icns) {
        // 画不出来就用原图标，启动器照常能用
        let data = render_icon(&icon, &badge.0, badge.1).unwrap_or(src);
        if put(icns, &data, 0o644)? {
            fs::utime_now(b)?; // 让 Finder、启动台换掉缓存里的旧图标
            subprocess_quiet(&[LSREGISTER, "-f", b]);
        }
    }
    Ok(())
}

fn write_launcher(b: &str, key: &str, cca: &str, app: &str, name: &str, badge: &(String, &'static str)) -> R<()> {
    let tool = t();
    let res = path::join_all(&[b, "Contents", "Resources"]);
    fs::makedirs(&path::join_all(&[b, "Contents", "MacOS"]))?;
    fs::makedirs(&res)?;
    let info = P::dict([
        ("CFBundleName", P::from(name)),
        ("CFBundleDisplayName", P::from(name)),
        ("CFBundleIdentifier", P::from(format!("{}{}", tool.launcher_id, acct_id(key, tool)))),
        ("CFBundleExecutable", P::from("cca-launcher")),
        ("CFBundleIconFile", P::from("AppIcon")),
        ("CFBundlePackageType", P::from("APPL")),
        ("CFBundleShortVersionString", P::from("1.0")),
        ("CFBundleVersion", P::from("1")),
        ("LSUIElement", P::from(true)),
        ("NSHighResolutionCapable", P::from(true)),
    ]);
    put(&path::join_all(&[b, "Contents", "Info.plist"]), &plist::dumps_xml(&info)?, 0o644)?;
    let script = fill(
        LAUNCHER_SH,
        &[
            ("label", &label_of(key)),
            ("cca", &sh_quote(cca)),
            ("key", &sh_quote(key)),
            ("app", tool.app_label),
            ("cmd", tool.app_cmd),
            ("title", &sh_quote(&format!("{name} 没有打开"))),
        ],
    );
    put(&path::join_all(&[b, "Contents", "MacOS", "cca-launcher"]), script.as_bytes(), 0o755)?;
    let (mark_path, icns) = (path::join(&res, LAUNCHER_MARK), path::join(&res, "AppIcon.icns"));
    let mark = dict! { "id" => acct_id(key, tool), "key" => key, "badge" => crate::list![badge.0.as_str(), badge.1] };
    if tool.kind != Kind::Claude {
        // Claude 的启动器标记不带 tool（和以前一样），别的工具注明是谁的
        mark.set("tool", tool.name);
    }
    match write_icon(b, app, badge, &mark, &mark_path, &icns) {
        Err(e) if !e.is_os_or_value() => return Err(e),
        _ => {}
    }
    put(&mark_path, json::dumps(&V::Dict(mark), json::LINE).as_bytes(), 0o644)?;
    Ok(())
}

/// 这个 .app 是哪个工具的 cca 启动器（没有 cca 标记的返回 None；旧的标记没写 tool，是 Claude 的）
pub fn launcher_tool(p: &str) -> Option<V> {
    let mark = path::join_all(&[p, "Contents", "Resources", LAUNCHER_MARK]);
    if !path::isfile(&mark) {
        return None;
    }
    let tool = load_quiet(&mark).and_then(|m| m.as_dict()).and_then(|m| m.get("tool")).filter(|tool| tool.truthy());
    Some(tool.unwrap_or_else(|| V::from("claude")))
}

/// ~/Applications 里每个账号一个“Claude 别名”（Codex 账号是“ChatGPT 别名”）启动器：按账号表新建、改名、删除。
/// 只动带 cca 标记、并且是当前工具的 .app；没装对应的 App（app 为空）或 mode=remove-all 时只删不建。
/// 输出：+ 新建、- 删除、! 同名但不是 cca 生成的（跳过）。第三方 API 账号用不了 App，没有启动器
pub fn cmd_launchers(args: &[String]) -> R<()> {
    arity("cmd_launchers", args, 2, 4)?;
    let (ldir, cca, app, mode) = (args[0].as_str(), args[1].as_str(), arg(args, 2, ""), arg(args, 3, ""));
    let table = Dict::from_pairs(table_load(t())?.items().into_iter().filter(|(_, v)| !is_api(v)));
    let names = launcher_names(&table);
    let cfg = load_quiet(&CFG).filter(|c| c.truthy()).map_or_else(Dict::new, |c| c.as_dict().unwrap_or_else(|| panic!("AttributeError: config.json 不是对象")));
    let badges = launcher_badges(&cfg, &table, &names);
    // 路径 → 账号（两个账号的启动器同名时后一个算数，位置照前一个）
    let mut want: Vec<(String, Rc<str>)> = Vec::new();
    if mode != "remove-all" && !app.is_empty() {
        for (k, name) in &names {
            let p = path::join(ldir, &format!("{name}.app"));
            match want.iter_mut().find(|(x, _)| *x == p) {
                Some(slot) => slot.1 = k.clone(),
                None => want.push((p, k.clone())),
            }
        }
    }
    if path::isdir(ldir) {
        for n in fs::listdir_sorted(ldir)? {
            let p = path::join(ldir, &n);
            if n.ends_with(".app") && !want.iter().any(|(x, _)| *x == p) && launcher_tool(&p).is_some_and(|tool| tool.as_str() == Some(t().name)) {
                fs::rmtree(&p)?;
                out!("-\t{}", &n[..n.len() - 4]);
            }
        }
    }
    for (p, k) in &want {
        let new = !path::exists(p);
        let shown = path::basename(p);
        let shown = &shown[..shown.len() - 4];
        if !new && !path::isfile(&path::join_all(&[p, "Contents", "Resources", LAUNCHER_MARK])) {
            out!("!\t{shown}");
            continue;
        }
        fs::makedirs(ldir)?;
        let name = &names.iter().find(|(x, _)| x == k).expect("want 里的都在 names 里").1;
        let badge = &badges.iter().find(|(x, _)| x == k).unwrap_or_else(|| panic!("KeyError: {k}")).1;
        write_launcher(p, k, cca, app, name, badge)?;
        if new {
            out!("+\t{shown}");
        }
    }
    Ok(())
}

/// Claude Code 把这个账号的登录凭证存在钥匙串里用的服务名（配置目录的 NFC 写法的摘要）
pub fn cmd_keychain(args: &[String]) -> R<()> {
    arity("cmd_keychain", args, 1, 1)?;
    let p: String = cdir(&args[0]).nfc().collect();
    out!("Claude Code-credentials-{}", &sha256_hex(p)[..8]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_substitutes_once() {
        assert_eq!(fill("a %(x)s b %% %(y)s", &[("x", "%(y)s"), ("y", "100%")]), "a %(y)s b % 100%");
        assert!(LAUNCHER_SH.ends_with("exit 1\n") && LAUNCHER_SH.contains("printf '%%s\\n'"));
    }
}
