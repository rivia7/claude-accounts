//! 账号设备标识：先于登录准备，重新登录和改别名都不换。对应 PY_SRC「账号设备标识」一节。
//!
//! `device.json`（账号目录里，600）是账号的固定标识；客户端自己存的那份（Claude 的 `.claude.json` 里的 userID / machineID、
//! Codex 的 `installation_id`、agy 的 `installation_id` 和 `jetski_state.pbtxt`）每次照它修回来。`prepare_device` 要在 `Lock` 里调。

use crate::core::base::*;
use crate::core::model::{adir, cdir};
use crate::core::tool::{t, Kind};
use crate::py::fs::{self, load_json, load_quiet, read_text, save_json};
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::rand;
use crate::py::re::Re;
use crate::py::shutil::{listdir, uuid_str, write_text};
use crate::py::text;
use crate::py::{die, R};
use crate::{dict, re};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

/// 写成 UUID 文本的设备标识（Codex、agy）
pub const UUID_FIELDS: &[&str] = &["installation_id", "installation_uuid"];

pub fn device_fields() -> &'static [&'static str] {
    fields_of(t().kind)
}

/// 这种工具的设备标识有哪几个字段（单独出来：单元测试不用去换全局的当前工具）
fn fields_of(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Codex => &["installation_id"],
        Kind::Agy => UUID_FIELDS,
        Kind::Claude => &["userID", "machineID"],
    }
}

/// 合法的设备标识给出标准写法（UUID 一律小写带连字符；Claude 的是 64 位小写 hex），不合法是 None
pub fn device_value(name: &str, value: Option<&V>) -> Option<String> {
    let value = value?.as_str()?;
    if UUID_FIELDS.contains(&name) {
        return uuid_str(text::strip(value));
    }
    re!(r"[0-9a-f]{64}").fullmatch(value).map(|_| value.to_string())
}

/// jetski_state.pbtxt 顶层的那一项
pub static AGY_STATE_ID: LazyLock<Re> = LazyLock::new(|| Re::new(r"(?m)^installation_uuid:[ \t]*\x22([^\x22\n]*)\x22[ \t]*$"));

pub fn agy_device_files(cd: &str) -> (String, String) {
    (path::join_all(&[cd, "antigravity-cli", "installation_id"]), path::join_all(&[cd, "antigravity-cli", "jetski_state.pbtxt"]))
}

/// 客户端自己存的设备标识（cd 是它的配置目录）：读不了、没有的就不在返回的字典里
pub fn device_native(cd: &str) -> Dict {
    native_of(t().kind, cd)
}

fn native_of(kind: Kind, cd: &str) -> Dict {
    match kind {
        Kind::Claude => load_quiet(&path::join(cd, ".claude.json")).and_then(|j| j.as_dict()).unwrap_or_default(),
        Kind::Agy => {
            let out = Dict::new();
            let (ids, state) = agy_device_files(cd);
            // 原来两个文件在同一个 try 里读：installation_id 读不了，jetski_state.pbtxt 也就不读了
            if let Ok(s) = read_text(&ids) {
                out.set("installation_id", text::strip(&s));
                if let Ok(s) = read_text(&state) {
                    if let Some(m) = AGY_STATE_ID.search(&s) {
                        out.set("installation_uuid", m.g(1));
                    }
                }
            }
            out
        }
        Kind::Codex => match read_text(&path::join(cd, "installation_id")) {
            Ok(s) => dict! { "installation_id" => text::strip(&s) },
            Err(_) => Dict::new(),
        },
    }
}

/// 旧配置可能把设备文件链到共享目录；不能沿用共享值，更不能写穿链接。
pub fn private_file(p: &str) -> bool {
    !path::islink(p) && std::fs::metadata(p).map_or(true, |m| m.nlink() == 1)
}

/// 新的设备标识
fn fresh_device_value(name: &str) -> String {
    if UUID_FIELDS.contains(&name) {
        rand::uuid4()
    } else {
        rand::token_hex(32)
    }
}

/// 各字段见过的值（字段名 → 一组值）
type Seen = HashMap<&'static str, HashSet<String>>;

/// `device_identity` 里定下各字段取什么值的那一段（原来写在一起；单独出来是为了能测“撞了再换”这种靠随机数碰不到的路）。
/// own 是这个账号固定记录里的，native 是客户端自己存的；fresh 给一个新值
fn settle_identity(
    fields: &[&'static str],
    own: &Dict,
    native: &Dict,
    used: &Seen,
    legacy: &Seen,
    fresh: &mut dyn FnMut(&str) -> String,
) -> Dict {
    let identity = dict! { "version" => 1 };
    for n in fields {
        let fixed = device_value(n, own.get(n).as_ref());
        let mut value = fixed.clone().or_else(|| device_value(n, native.get(n).as_ref()));
        // 未迁移的副本不能迫使已有固定标识的账号换设备；轮到副本首次启动时再为它分配。
        let taken = |v: &String| used[n].contains(v) || legacy[n].contains(v);
        if value.as_ref().is_none_or(|v| used[n].contains(v) || (fixed.is_none() && legacy[n].contains(v))) {
            let mut new = fresh(n);
            while taken(&new) {
                new = fresh(n);
            }
            value = Some(new);
        }
        identity.set(*n, value.expect("上面刚定下来"));
    }
    identity
}

/// 调用方持有 Lock。device.json 是账号的固定标识；首次接管保留合法、独立的客户端旧值。
pub fn device_identity(key: &str, native: &Dict) -> R<Dict> {
    let account = adir(key);
    let p = path::join(&account, "device.json");
    let saved = load_json(&p)?.unwrap_or_else(|| V::Dict(Dict::new()));
    let Some(saved) = saved.as_dict().filter(|s| s.get("version").is_none_or(|v| v.py_eq(&V::Int(1)))) else {
        return die(format!("无法读取账号设备标识：{p}"));
    };
    let own = if private_file(&p) { saved.clone() } else { Dict::new() };
    let fields = device_fields();
    let mut used: Seen = fields.iter().map(|n| (*n, HashSet::new())).collect();
    let mut legacy: Seen = fields.iter().map(|n| (*n, HashSet::new())).collect();
    let master = if t().kind == Kind::Claude {
        load_quiet(&MASTER_JSON).and_then(|j| j.as_dict())
    } else {
        Some(device_native(&t().master))
    };
    if let Some(master) = master {
        for n in fields {
            if let Some(value) = device_value(n, master.get(n).as_ref()) {
                used.get_mut(n).expect("每个字段都有").insert(value);
            }
        }
    }
    let root = path::join(&t().sroot, "accounts");
    for name in listdir(&root)? {
        let other = path::join(&root, &name);
        if other == account || path::islink(&other) || !path::isdir(&other) {
            continue;
        }
        let old = device_native(&path::join(&other, t().sub));
        let record = path::join(&other, "device.json");
        let fixed = load_quiet(&record).and_then(|j| j.as_dict());
        let isolated = private_file(&record);
        // 已由 cca 接管的账号以固定记录为准，客户端暂时删改了字段也不影响其它账号。
        for n in fields {
            let value = fixed.as_ref().and_then(|f| device_value(n, f.get(n).as_ref()));
            match value {
                Some(value) if isolated => {
                    used.get_mut(n).expect("每个字段都有").insert(value);
                }
                value => {
                    let seen = legacy.get_mut(n).expect("每个字段都有");
                    // 共享记录断开硬链接后会变成普通文件，迁移时就要计入去重
                    seen.extend(value);
                    seen.extend(device_value(n, old.get(n).as_ref()));
                }
            }
        }
    }
    let identity = settle_identity(fields, &own, native, &used, &legacy, &mut fresh_device_value);
    if !V::Dict(identity.clone()).py_eq(&V::Dict(saved)) || !private_file(&p) {
        save_json(&p, &V::Dict(identity.clone()), Some(0o600), false)?;
    }
    Ok(identity)
}

/// 调用方持有 Lock；只修复设备字段，保留客户端的登录和其它配置。
pub fn prepare_device(key: &str) -> R<()> {
    let cd = cdir(key);
    if path::islink(&adir(key)) || path::islink(&cd) {
        return die(format!("账号目录不能是符号链接，无法隔离设备标识：{cd}"));
    }
    fs::makedirs(&cd)?;
    let (native, paths) = match t().kind {
        Kind::Claude => {
            let p = path::join(&cd, ".claude.json");
            let native = load_json(&p)?.unwrap_or_else(|| V::Dict(Dict::new()));
            let Some(native) = native.as_dict() else {
                return die(format!("账号配置必须是 JSON 对象：{p}"));
            };
            (native, vec![p])
        }
        Kind::Agy => {
            let (ids, state) = agy_device_files(&cd);
            (device_native(&cd), vec![ids, state])
        }
        Kind::Codex => (device_native(&cd), vec![path::join(&cd, "installation_id")]),
    };
    let private = paths.iter().all(|p| private_file(p));
    let identity = device_identity(key, &if private { native.clone() } else { Dict::new() })?;
    if !private || device_fields().iter().any(|n| !native.get(n).is_some_and(|v| v.py_eq(&identity.at(n)))) {
        match t().kind {
            Kind::Claude => {
                for n in device_fields() {
                    native.set(*n, identity.at(n));
                }
                save_json(&paths[0], &V::Dict(native), Some(0o600), false)?;
            }
            Kind::Agy => agy_device_write(&cd, &identity)?,
            // 原来是就地写的同样几步：临时文件、600、改名换上
            Kind::Codex => write_private(&paths[0], &format!("{}\n", identity.at("installation_id").py_str()))?,
        }
    }
    Ok(())
}

/// 原子替换成只有自己能读写的普通文件（原来是链接、硬链接时不写穿）
pub fn write_private(p: &str, text: &str) -> R<()> {
    let tmp = format!("{}.cca-{}.tmp", p, std::process::id());
    write_text(&tmp, text)?;
    fs::chmod(&tmp, 0o600)?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}

/// agy 的两份设备标识：installation_id 只能是 36 个字符（带换行它认不出，会换成新的，1.5.5 实测）；jetski_state.pbtxt
/// 只换（没有就加上）顶层的 installation_uuid 那一行，别的原样留着（没有这个文件时只写这一行，agy 照样读、补上别的）
pub fn agy_device_write(cd: &str, identity: &Dict) -> R<()> {
    let (ids, state) = agy_device_files(cd);
    fs::makedirs(&path::dirname(&ids))?;
    write_private(&ids, &identity.at("installation_id").py_str())?;
    let mut text = match read_text(&state) {
        Ok(s) => s,
        Err(e) if e.is_not_found() => String::new(),
        Err(e) => return Err(e),
    };
    let line = format!("installation_uuid:  \"{}\"", identity.at("installation_uuid").py_str());
    let found = AGY_STATE_ID.search(&text).map(|m| (m.start(0), m.end(0)));
    match found {
        Some((a, b)) => text.replace_range(a..b, &line),
        None => {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    write_private(&state, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 这里的测试只碰临时目录：凡是会用到 HOME、~/.cca 的函数（device_identity、prepare_device）都不在单元测试里调，
    // 它们经子命令在临时 HOME 里测（tests/*.py、回归）。也不换全局的当前工具（测试是并行跑的）

    fn tmpdir(tag: &str) -> String {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-device-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    fn seen(fields: &[&'static str], values: &[(&'static str, &str)]) -> Seen {
        let mut s: Seen = fields.iter().map(|n| (*n, HashSet::new())).collect();
        for (n, v) in values {
            s.get_mut(n).unwrap().insert(v.to_string());
        }
        s
    }

    #[test]
    fn values() {
        let hex = "0123456789abcdef".repeat(4);
        assert_eq!(device_value("userID", Some(&V::from(hex.as_str()))).as_deref(), Some(hex.as_str()));
        // fullmatch：结尾多一个换行、大写、长度不对都不算；不是字符串的不算
        for bad in [format!("{hex}\n"), hex.to_uppercase(), hex[1..].to_string(), format!("{hex}0"), String::new()] {
            assert_eq!(device_value("machineID", Some(&V::from(bad.as_str()))), None, "{bad:?}");
        }
        assert_eq!(device_value("userID", Some(&V::Int(1))), None);
        assert_eq!(device_value("userID", None), None);
        let v = |s: &str| device_value("installation_id", Some(&V::from(s)));
        assert_eq!(v(" ABCDEF01-2345-6789-ABCD-EF0123456789\n").as_deref(), Some("abcdef01-2345-6789-abcd-ef0123456789"));
        assert_eq!(
            device_value("installation_uuid", Some(&V::from("{abcdef0123456789abcdef0123456789}"))).as_deref(),
            Some("abcdef01-2345-6789-abcd-ef0123456789")
        );
        assert_eq!(v("not-a-uuid"), None);
        assert_eq!(v(&hex), None);
        assert_eq!(device_value("installation_id", Some(&V::None)), None);
    }

    #[test]
    fn keeps_valid_values_and_replaces_taken_ones() {
        let fields: &[&'static str] = &["userID", "machineID"];
        let (a, b, c, d, e) = ("a".repeat(64), "b".repeat(64), "c".repeat(64), "d".repeat(64), "e".repeat(64));
        let none = seen(fields, &[]);
        let mut never = |_: &str| -> String { panic!("不该要新值") };
        // 固定记录里合法的值照用，哪怕未迁移的副本里也有它
        let own = dict! { "version" => 1, "userID" => a.as_str(), "machineID" => b.as_str() };
        let native = dict! { "userID" => c.as_str(), "machineID" => d.as_str() };
        let got = settle_identity(fields, &own, &native, &none, &seen(fields, &[("userID", &a), ("machineID", &b)]), &mut never);
        assert_eq!(V::Dict(got).py_repr(), format!("{{'version': 1, 'userID': '{a}', 'machineID': '{b}'}}"));
        // 没有固定记录：客户端自己的旧值没和别人撞就留着
        let got = settle_identity(fields, &Dict::new(), &native, &seen(fields, &[("userID", &a)]), &none, &mut never);
        assert_eq!(V::Dict(got).py_repr(), format!("{{'version': 1, 'userID': '{c}', 'machineID': '{d}'}}"));
        // 固定值和别的账号的固定值撞了（共享记录拆开以后）、旧值在未迁移的副本里也有：都换新的；
        // 新值撞上这个字段已有的（用过的、未迁移的）就再要一个，别的字段用过的不碍事
        let own = dict! { "userID" => a.as_str(), "machineID" => 7 };
        let native = dict! { "machineID" => d.as_str() };
        let mut queue = vec![e.clone(), c.clone(), c.clone(), d.clone(), a.clone(), "7".repeat(64)];
        let mut asked = Vec::new();
        let mut scripted = |n: &str| {
            asked.push(n.to_string());
            queue.remove(0)
        };
        let used = seen(fields, &[("userID", &a), ("machineID", &c)]);
        let legacy = seen(fields, &[("userID", &e), ("machineID", &d)]);
        let got = settle_identity(fields, &own, &native, &used, &legacy, &mut scripted);
        assert_eq!(V::Dict(got).py_repr(), format!("{{'version': 1, 'userID': '{c}', 'machineID': '{a}'}}"));
        assert_eq!(asked, ["userID", "userID", "machineID", "machineID", "machineID"]);
        assert_eq!(queue, ["7".repeat(64)]);
        // 两边都没有合法的值：换新的
        let mut once = vec!["6".repeat(64), "5".repeat(64)];
        let got = settle_identity(
            fields,
            &dict! { "userID" => "short" },
            &dict! { "machineID" => V::None },
            &none,
            &none,
            &mut |_: &str| once.remove(0),
        );
        assert_eq!(
            V::Dict(got).py_repr(),
            format!("{{'version': 1, 'userID': '{}', 'machineID': '{}'}}", "6".repeat(64), "5".repeat(64))
        );
    }

    #[test]
    fn fresh_values_have_the_clients_shapes() {
        let (x, y) = (fresh_device_value("userID"), fresh_device_value("machineID"));
        assert!(
            x != y
                && device_value("userID", Some(&V::from(x.as_str()))).is_some()
                && device_value("machineID", Some(&V::from(y.as_str()))).is_some()
        );
        let (u, w) = (fresh_device_value("installation_id"), fresh_device_value("installation_uuid"));
        assert!(
            u != w && u.len() == 36 && device_value("installation_id", Some(&V::from(u.as_str()))).as_deref() == Some(u.as_str())
        );
        assert_eq!(device_value("installation_uuid", Some(&V::from(w.as_str()))).as_deref(), Some(w.as_str()));
    }

    #[test]
    fn private_means_not_linked() {
        let d = tmpdir("private");
        let f = format!("{d}/f");
        assert!(private_file(&f)); // 还没有也算
        std::fs::write(&f, "x").unwrap();
        assert!(private_file(&f));
        std::fs::hard_link(&f, format!("{d}/hard")).unwrap();
        assert!(!private_file(&f) && !private_file(&format!("{d}/hard")));
        std::fs::remove_file(format!("{d}/hard")).unwrap();
        std::os::unix::fs::symlink(&f, format!("{d}/soft")).unwrap();
        assert!(private_file(&f) && !private_file(&format!("{d}/soft")));
        std::os::unix::fs::symlink(format!("{d}/gone"), format!("{d}/dangling")).unwrap();
        assert!(!private_file(&format!("{d}/dangling")));
        // 原子替换：链接换成普通文件，原来指向的那份不动
        write_private(&format!("{d}/soft"), "y").unwrap();
        assert!(private_file(&format!("{d}/soft")));
        assert_eq!(
            (std::fs::read_to_string(&f).unwrap().as_str(), std::fs::read_to_string(format!("{d}/soft")).unwrap().as_str()),
            ("x", "y")
        );
        assert_eq!(fs::mode_of(&format!("{d}/soft")).unwrap(), 0o600);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn native_files_per_tool() {
        // 右边都是原版 device_native 在同样的文件上给出的结果
        let d = tmpdir("native");
        let shown = |kind: Kind| V::Dict(native_of(kind, &d)).py_repr();
        assert_eq!(fields_of(Kind::Codex), ["installation_id"]);
        assert_eq!(shown(Kind::Codex), "{}");
        std::fs::write(format!("{d}/installation_id"), " 11111111-1111-1111-1111-111111111111 \r\n").unwrap();
        assert_eq!(shown(Kind::Codex), "{'installation_id': '11111111-1111-1111-1111-111111111111'}");
        std::fs::write(format!("{d}/installation_id"), b"\xff").unwrap();
        assert_eq!(shown(Kind::Codex), "{}");

        assert_eq!(fields_of(Kind::Agy), ["installation_id", "installation_uuid"]);
        std::fs::create_dir_all(format!("{d}/antigravity-cli")).unwrap();
        let (ids, state) = agy_device_files(&d);
        // 只有 jetski_state.pbtxt：installation_id 读不了，两个都不给
        std::fs::write(&state, "installation_uuid: \"x\"\n").unwrap();
        assert_eq!(shown(Kind::Agy), "{}");
        std::fs::write(&ids, "abc").unwrap();
        assert_eq!(shown(Kind::Agy), "{'installation_id': 'abc', 'installation_uuid': 'x'}");
        // 只认顶层的那一行（行首、整行）
        std::fs::write(&state, "nested {\n  installation_uuid: \"inner\"\n}\ninstallation_uuid:\t\"top\" \t\r\nother: 1\n")
            .unwrap();
        assert_eq!(shown(Kind::Agy), "{'installation_id': 'abc', 'installation_uuid': 'top'}");
        std::fs::write(&state, "installation_uuid: \"a\" # 注释\n").unwrap();
        assert_eq!(shown(Kind::Agy), "{'installation_id': 'abc'}");
        std::fs::write(&state, b"installation_uuid: \"\xff\"\n").unwrap();
        assert_eq!(shown(Kind::Agy), "{'installation_id': 'abc'}");

        assert_eq!(fields_of(Kind::Claude), ["userID", "machineID"]);
        assert_eq!(shown(Kind::Claude), "{}");
        std::fs::write(format!("{d}/.claude.json"), "[1]").unwrap();
        assert_eq!(shown(Kind::Claude), "{}");
        std::fs::write(format!("{d}/.claude.json"), "{\"userID\": 1}").unwrap();
        assert_eq!(shown(Kind::Claude), "{'userID': 1}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn agy_state_line_is_replaced_in_place() {
        let d = tmpdir("agy");
        let identity = dict! { "version" => 1, "installation_id" => "11111111-1111-1111-1111-111111111111", "installation_uuid" => "22222222-2222-2222-2222-222222222222" };
        let (ids, state) = agy_device_files(&d);
        let line = "installation_uuid:  \"22222222-2222-2222-2222-222222222222\"";
        // 没有这两个文件：目录也建上；installation_id 不带换行
        agy_device_write(&d, &identity).unwrap();
        assert_eq!(std::fs::read_to_string(&ids).unwrap(), "11111111-1111-1111-1111-111111111111");
        assert_eq!(std::fs::read_to_string(&state).unwrap(), format!("{line}\n"));
        assert_eq!((fs::mode_of(&ids).unwrap(), fs::mode_of(&state).unwrap()), (0o600, 0o600));
        // 只换第一处顶层的那一行，别的原样；\r\n 读进来成了 \n
        std::fs::write(
            &state,
            "a: 1\r\ninstallation_uuid: \"old\"\nb {\n  installation_uuid: \"inner\"\n}\ninstallation_uuid: \"second\"\n",
        )
        .unwrap();
        agy_device_write(&d, &identity).unwrap();
        assert_eq!(
            std::fs::read_to_string(&state).unwrap(),
            format!("a: 1\n{line}\nb {{\n  installation_uuid: \"inner\"\n}}\ninstallation_uuid: \"second\"\n")
        );
        // 没有那一行：加在最后（原来结尾没有换行的先补一个）
        std::fs::write(&state, "a: 1").unwrap();
        agy_device_write(&d, &identity).unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap(), format!("a: 1\n{line}\n"));
        std::fs::write(&state, "a: 1\n").unwrap();
        agy_device_write(&d, &identity).unwrap();
        assert_eq!(std::fs::read_to_string(&state).unwrap(), format!("a: 1\n{line}\n"));
        // 读不了（不是 UTF-8）：installation_id 已经写了，这个文件原样不动，报错
        std::fs::write(&state, b"\xff\n").unwrap();
        std::fs::remove_file(&ids).unwrap();
        assert!(agy_device_write(&d, &identity).is_err());
        assert!(path::exists(&ids));
        assert_eq!(std::fs::read(&state).unwrap(), b"\xff\n");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
