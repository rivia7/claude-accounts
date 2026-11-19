//! `.claude.json` 的三方合并（`claude_sync`）和共享目录的链接（`cmd_link` 用的那些）：会话里新建的、被原子保存替换掉链接的
//! 真实条目并回共享主目录再改回链接，冲突的配置按字段（JSON）或整份（TOML、文本）三方比较。
//! 对应 PY_SRC「.claude.json 三方合并」「共享目录链接」两节。
//!
//! 原来的 `MISSING`（“字典里没有这个键”，和 JSON 的 null 不是一回事）在这里是 `Option<V>` 的 `None`。

use super::device::prepare_device;
use crate::core::base::*;
use crate::core::model::{acct_id, cdir};
use crate::core::tool::{t, Kind, SHARED_KEYS};
use crate::err;
use crate::py::fs::{self, load_json, read_text, save_json};
use crate::py::hash::sha256_hex;
use crate::py::json::{self, JsonError};
use crate::py::num;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::rand;
use crate::py::shutil::*;
use crate::py::text;
use crate::py::time;
use crate::py::{die, R};
use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;

/// `a == b`，两边都可以是“没有”（原来的 MISSING）：没有只等于没有
pub fn eq(a: &Option<V>, b: &Option<V>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.py_eq(y),
        _ => false,
    }
}

/// `load_json(p, {}) or {}` 读出来的值当字典用：没有、算假的是空字典；别的不是字典的值，原来是用到时才出的
/// 没人接的 AttributeError
fn json_object(v: Option<V>, p: &str) -> Dict {
    match v {
        Some(V::Dict(d)) => d,
        Some(x) if x.truthy() => panic!("AttributeError: {} 里不是 JSON 对象（{}）", p, text::head(&x.py_repr(), 80)),
        _ => Dict::new(),
    }
}

/// `~/.claude.json` 里跨账号共享的那些键（SHARED_KEYS）和账号目录的 `.claude.json` 三方合并，基线是账号目录的
/// `.cca-sync-base.json`；别的键（登录、订阅、设备标识……）各管各的
pub fn claude_sync(key: &str) -> R<()> {
    let cd = cdir(key);
    let (aj, bj) = (path::join(&cd, ".claude.json"), path::join(&cd, ".cca-sync-base.json"));
    let _lock = Lock::acquire()?;
    prepare_device(key)?;
    let (m, a, b) = (load_json(&MASTER_JSON)?, load_json(&aj)?, load_json(&bj)?);
    let (b, a, m) = (json_object(b, &bj), json_object(a, &aj), json_object(m, &MASTER_JSON));
    let base = Dict::new();
    let (mut ch_a, mut ch_m, mut conflict) = (false, false, false);
    for k in SHARED_KEYS {
        let (v, collided) = merge_config_json(b.get(k), a.get(k), m.get(k));
        conflict |= collided;
        let Some(v) = v else {
            ch_a |= a.pop(k).is_some();
            ch_m |= m.pop(k).is_some();
            continue;
        };
        base.set(*k, &v);
        if !a.get(k).is_some_and(|x| x.py_eq(&v)) {
            a.set(*k, &v);
            ch_a = true;
        }
        if !m.get(k).is_some_and(|x| x.py_eq(&v)) {
            m.set(*k, &v);
            ch_m = true;
        }
    }
    if conflict {
        config_conflict(&aj, key)?;
    }
    if ch_a || !path::exists(&aj) {
        save_json(&aj, &V::Dict(a), Some(0o600), false)?;
    }
    if ch_m {
        save_json(&MASTER_JSON, &V::Dict(m), None, true)?;
    }
    save_json(&bj, &V::Dict(base), Some(0o600), true)
}

/// 账号目录和共享主目录里的同名条目类型对不上（一个是目录一个是文件；任务列表里还有一边是链接的）：
/// 账号这边的移到 conflicts 里，名字后面加上现在的秒数
pub fn aside(src: &str, key: &str) -> R<()> {
    let d = path::join_all(&[&t().sroot, "conflicts", &acct_id(key, t())]);
    fs::makedirs(&d)?;
    let dst = path::join(&d, &format!("{}.{}", path::basename(src), num::trunc(time::time())));
    shutil_move(src, &dst)?;
    err!("cca: {} 与 {} 中的同名条目类型不同，已移到 {}", path::basename(src), t().master.replacen(HOME.as_str(), "~", 1), dst);
    Ok(())
}

/// dst（是链接就是它指向的文件）原子地换成 src 的一份拷贝，修改时间跟着 src
pub fn copy_over(src: &str, dst: &str) -> R<()> {
    let real = path::realpath(dst);
    let tmp = format!("{}.cca-{}.tmp", real, std::process::id());
    copy2(src, &tmp)?;
    std::fs::rename(&tmp, &real)?;
    Ok(())
}

/// src 里 dst 没有的行接到 dst 后面（history.jsonl 这类一行一条的文件）
pub fn merge_lines(src: &str, dst: &str) -> R<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let have = std::fs::read(dst)?;
    let seen: HashSet<&[u8]> = splitlines_bytes(&have).into_iter().collect();
    let incoming = std::fs::read(src)?;
    // src 里重复的新行照样各接一遍（原来就不去重）
    let extra: Vec<&[u8]> = splitlines_bytes(&incoming).into_iter().filter(|l| !seen.contains(l)).collect();
    if !extra.is_empty() {
        let mut data: Vec<u8> = if !have.is_empty() && !have.ends_with(b"\n") { b"\n".to_vec() } else { Vec::new() };
        data.extend(extra.join(&b'\n'));
        data.push(b'\n');
        std::fs::OpenOptions::new().append(true).create(true).mode(0o666).open(path::realpath(dst))?.write_all(&data)?;
    }
    Ok(())
}

/// 两边都有的文件：.jsonl 并行；别的内容不一样时留修改时间较新的那份
pub fn merge_file(src: &str, dst: &str) -> R<()> {
    if src.ends_with(".jsonl") {
        merge_lines(src, dst)?;
    } else if !filecmp(src, dst)? && getmtime(src)? > getmtime(dst)? {
        copy_over(src, dst)?;
    }
    Ok(())
}

/// src 这棵目录并进 dst：dst 没有的拷过去（链接照样建成链接），都有的文件照 merge_file
pub fn merge_tree(src: &str, dst: &str) -> R<()> {
    walk(src, |root, dirs, files| {
        let rel = path::relpath(root, src);
        let troot = if rel == "." { dst.to_string() } else { path::join(dst, &rel) };
        fs::makedirs(&troot)?;
        let linked: Vec<&String> = dirs.iter().filter(|d| path::islink(&path::join(root, d))).collect();
        for n in linked.into_iter().chain(files) {
            let (sp, tp) = (path::join(root, n), path::join(&troot, n));
            if path::islink(&sp) {
                if !path::lexists(&tp) {
                    symlink(std::fs::read_link(&sp)?, &tp)?;
                }
            } else if !path::lexists(&tp) {
                copy2(&sp, &tp)?;
            } else if path::isfile(&tp) {
                merge_file(&sp, &tp)?;
            }
        }
        dirs.retain(|d| !path::islink(&path::join(root, d)));
        Ok(())
    })
}

/// 目录里的每一项（相对路径、内容摘要或链接目标）和最晚的修改时间（文件和目录都算，不跟随链接）
pub fn tree_state(p: &str) -> R<(Vec<(String, Vec<u8>)>, f64)> {
    let mut items = Vec::new();
    let mut newest = lstat_mtime(p)?;
    walk(p, |root, dirs, files| {
        dirs.sort();
        let mut sorted = files.to_vec();
        sorted.sort();
        for n in dirs.iter().chain(&sorted) {
            let q = path::join(root, n);
            newest = newest.max(lstat_mtime(&q)?);
            if path::islink(&q) {
                // 链接的目标照原样的字节比（不是 UTF-8 的也分得清）
                items.push((path::relpath(&q, p), [b"link:", std::fs::read_link(&q)?.as_os_str().as_bytes()].concat()));
            } else if files.contains(n) {
                items.push((path::relpath(&q, p), sha256_hex(std::fs::read(&q)?).into_bytes()));
            }
        }
        Ok(())
    })?;
    Ok((items, newest))
}

/// 账号自己的 tasks/ 并回共享的：任务列表（tasks/<列表 ID>）整份认。tasks 共享以前换账号续接过的会话，两边各有
/// 一份同一个列表，任务编号各自从 1 起，逐个文件拼会混进过时的任务：一样的丢掉这边的；不一样的留较新的那份，
/// 较旧的整份移进 conflicts
pub fn adopt_task_lists(src: &str, dst: &str, key: &str) -> R<()> {
    let mut names = listdir_strict(src)?;
    names.sort();
    for name in names {
        let (s, d) = (path::join(src, &name), path::join(dst, &name));
        if !path::lexists(&d) {
            shutil_move(&s, &d)?;
            continue;
        }
        if path::islink(&s) || path::islink(&d) || path::isdir(&s) != path::isdir(&d) {
            aside(&s, key)?;
            continue;
        }
        if !path::isdir(&s) {
            merge_file(&s, &d)?;
            std::fs::remove_file(&s)?;
            continue;
        }
        let ((mine, mine_at), (shared, shared_at)) = (tree_state(&s)?, tree_state(&d)?);
        if mine == shared {
            rmtree(&s)?;
            continue;
        }
        let older = if mine_at > shared_at { &d } else { &s };
        let conflicts = path::join_all(&[&t().sroot, "conflicts", &acct_id(key, t())]);
        makedirs_leaf_mode(&conflicts, 0o700)?;
        let kept = path::join(&conflicts, &format!("tasks-{}.{}", name, rand::uuid4().replace('-', "")));
        shutil_move(older, &kept)?;
        if older == &d {
            shutil_move(&s, &d)?;
        }
        err!(
            "cca: 任务列表 {} 在 {} 和这个账号里各有一份，留下较新的，较旧的那份移到 {}",
            name,
            dst.replacen(HOME.as_str(), "~", 1),
            kept
        );
    }
    Ok(())
}

/// 会话里新建的、或被原子写替换掉链接的真实条目：并回共享主目录（~/.claude、~/.codex）后改回链接。
pub fn adopt(src: &str, dst: &str, key: &str) -> R<()> {
    if !path::lexists(dst) {
        shutil_move(src, dst)?;
    } else if path::isdir(src) {
        if !path::isdir(dst) {
            return aside(src, key);
        }
        if t().kind == Kind::Claude && path::basename(src) == "tasks" {
            adopt_task_lists(src, dst, key)?;
        } else {
            merge_tree(src, dst)?;
        }
        rmtree(src)?;
    } else if path::isdir(dst) {
        return aside(src, key);
    } else {
        merge_file(src, dst)?;
        std::fs::remove_file(src)?;
    }
    Ok(())
}

/// 只拆旧版 cca 指向共享主目录同名项的链接，用户自己建的外部链接不动。
pub fn detach_legacy_shared(p: &str, shared: &str) -> R<()> {
    if !path::islink(p) {
        return Ok(());
    }
    let target = readlink(p)?;
    if path::abspath(&path::join(&path::dirname(p), &target)) != path::abspath(shared) {
        return Ok(());
    }
    if !path::exists(p) {
        std::fs::remove_file(p)?;
        return Ok(());
    }
    if path::isdir(p) {
        walk(p, |root, dirs, files| {
            if dirs.iter().chain(files).any(|n| path::islink(&path::join(root, n))) {
                return die(format!("旧共享目录包含内部符号链接，无法自动拆分，原目录未改动：{p}"));
            }
            Ok(())
        })?;
    }
    let work = TemporaryDirectory::new(".cca-unshare-", &path::dirname(p))?;
    let copied = path::join(&work.name, "copy");
    if path::isdir(p) {
        copytree(p, &copied)?;
    } else {
        copy2(p, &copied)?;
    }
    // 目录不能直接 rename 覆盖符号链接；复制成功后再移走旧链接，失败则恢复。
    let old = path::join(&work.name, "link");
    std::fs::rename(p, &old)?;
    if let Err(e) = std::fs::rename(&copied, p) {
        std::fs::rename(&old, p)?;
        return Err(e.into());
    }
    work.cleanup()?;
    Ok(())
}

/// 文件的文字；没有这个文件是 None（读不了、不是 UTF-8 照样是错）
pub fn shared_text(p: &str) -> R<Option<String>> {
    match read_text(p) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

/// 冲突副本完整保存；文件可能包含用户私有配置，不输出内容。
pub fn config_conflict(src: &str, key: &str) -> R<()> {
    let directory = path::join_all(&[&t().sroot, "conflicts", &acct_id(key, t())]);
    makedirs_leaf_mode(&directory, 0o700)?;
    let dst = path::join(&directory, &format!("{}.{}", path::basename(src), rand::uuid4().replace('-', "")));
    copy2(src, &dst)?;
    fs::chmod(&dst, 0o600)?;
    err!("cca: 共享配置 {} 有冲突或无法合并，冲突部分保留共享版本，账号副本已保存到 {}", path::basename(src), dst);
    Ok(())
}

/// 按基线逐字段合并；双方对同一字段作不同修改时保留共享值并报告冲突。
/// 三个参数和返回值里的 None 都是“没有这一项”
pub fn merge_config_json(base: Option<V>, ours: Option<V>, theirs: Option<V>) -> (Option<V>, bool) {
    if eq(&ours, &theirs) || eq(&ours, &base) {
        return (theirs, false);
    }
    if eq(&theirs, &base) {
        return (ours, false);
    }
    if let (Some(V::Dict(ours)), Some(V::Dict(theirs))) = (&ours, &theirs) {
        let before = match &base {
            Some(V::Dict(b)) => b.clone(),
            _ => Dict::new(),
        };
        let merged = Dict::new();
        let mut conflict = false;
        let mine: Vec<_> = ours.keys().into_iter().filter(|n| !theirs.has(n)).collect();
        for name in theirs.keys().into_iter().chain(mine) {
            let (value, changed) = merge_config_json(before.get(&name), ours.get(&name), theirs.get(&name));
            if let Some(value) = value {
                merged.set(name, value);
            }
            conflict |= changed;
        }
        return (Some(V::Dict(merged)), conflict);
    }
    (theirs, true)
}

/// 共享的 JSON 配置的内容：空文件当作空对象，不算坏的 JSON（agy 第一次运行时建的 config/mcp_config.json 就是 0 字节，
/// 以前每次退出都报冲突）
pub fn config_json(text: &str) -> Result<V, JsonError> {
    if text::strip(text).is_empty() {
        Ok(V::Dict(Dict::new()))
    } else {
        json::loads(text)
    }
}

/// 账号目录里被换成真实文件的共享配置 src 并回共享的 dst。base 是上次链接时记下的共享内容：
/// None 是基线里没有这一项，`V::None` 是当时共享主目录里还没有这个文件，字符串是当时的原文
pub fn merge_shared_config(src: &str, dst: &str, key: &str, base: Option<V>) -> R<()> {
    let (ours, theirs) = (shared_text(src)?, shared_text(dst)?);
    let same_as_base = |x: &Option<String>| match (&base, x) {
        (Some(V::None), None) => true,
        (Some(V::Str(b)), Some(x)) => **b == **x,
        _ => false,
    };
    if ours == theirs || same_as_base(&ours) {
        return Ok(());
    }
    let is_json = src.ends_with(".json");
    let mut parsed = None;
    if is_json {
        // 这时账号这份还在：前面刚确认过它是个文件（没了的话原来也是出错）
        match config_json(ours.as_deref().unwrap_or_else(|| panic!("AttributeError: {src} 读的时候已经不在了"))) {
            Ok(v) => parsed = Some(v),
            Err(_) => return config_conflict(src, key),
        }
    }
    if same_as_base(&theirs) || (base.is_none() && theirs.is_none()) {
        return copy_over(src, dst);
    }
    if let (Some(base), true) = (&base, is_json) {
        let merge = || -> Result<(Option<V>, Option<V>, bool), JsonError> {
            let now = match &theirs {
                Some(text) => Some(config_json(text)?),
                None => None,
            };
            let before = match base {
                V::None => None,
                b => Some(config_json(&b.py_str())?),
            };
            let (merged, conflict) = merge_config_json(before, parsed.clone(), now.clone());
            Ok((merged, now, conflict))
        };
        // 无法解析的配置保留原文，不能拿不完整 JSON 覆盖共享版本。
        if let Ok((merged, now, conflict)) = merge() {
            if conflict {
                config_conflict(src, key)?;
            }
            // 合出来和共享的一样（比如账号这份是空的）：共享的原样不动
            if let Some(merged) = merged.filter(|m| !eq(&Some(m.clone()), &now)) {
                save_json(dst, &merged, None, true)?;
            }
            return Ok(());
        }
    }
    // TOML/普通文本不按行拼接，避免合成重复的 key/table；无基线也不猜哪份更新。
    config_conflict(src, key)
}

/// 要链接的各层：顶层（''）和 T.nests 里的子目录，[(相对路径, 账号目录里的, 主目录里的)]。子目录在账号目录里建成真目录
/// （里面放指向主目录同一位置的链接）；账号目录里那一层是链接（不是 cca 建的）时这一层不动，免得经它写到别处
pub fn link_levels(cd: &str, master: &str) -> R<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for lvl in [""].iter().chain(t().nests) {
        let (lcd, lm) = (path::join(cd, lvl), path::join(master, lvl));
        if !lvl.is_empty() {
            if path::islink(&lcd) || (path::lexists(&lcd) && !path::isdir(&lcd)) {
                continue;
            }
            fs::makedirs(&lcd)?;
        }
        out.push((lvl.to_string(), lcd, lm));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dict, list};

    fn tmpdir(tag: &str) -> String {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-shared-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    fn j(text: &str) -> Option<V> {
        Some(json::loads(text).unwrap())
    }

    fn merged(base: Option<V>, ours: Option<V>, theirs: Option<V>) -> (String, bool) {
        let (v, conflict) = merge_config_json(base, ours, theirs);
        (v.map_or("MISSING".to_string(), |v| json::dumps(&v, json::LINE)), conflict)
    }

    #[test]
    fn eq_treats_missing_as_its_own_value() {
        assert!(eq(&None, &None) && !eq(&None, &Some(V::None)) && !eq(&Some(V::None), &None));
        assert!(
            eq(&Some(V::None), &Some(V::None))
                && eq(&Some(V::Int(1)), &Some(V::Bool(true)))
                && eq(&Some(V::Int(1)), &Some(V::Float(1.0)))
        );
        assert!(eq(&j(r#"{"a": 1, "b": [1, 2]}"#), &j(r#"{"b": [1, 2], "a": 1.0}"#)) && !eq(&j("[1, 2]"), &j("[2, 1]")));
    }

    #[test]
    fn three_way_merge_like_python() {
        // 右边都是原版 merge_config_json 的结果（MISSING 写成字面的 MISSING）
        let cases = [
            (None, None, None, "MISSING", false),
            (None, j("1"), None, "1", false),
            (None, None, j("1"), "1", false),
            (j("1"), None, j("1"), "MISSING", false),
            (j("1"), j("1"), None, "MISSING", false),
            (j("1"), j("2"), j("1"), "2", false),
            (j("1"), j("1"), j("3"), "3", false),
            (j("1"), j("2"), j("3"), "3", true),
            (j("1"), j("2"), None, "MISSING", true),
            (j("1"), None, j("3"), "3", true),
            (None, j("2"), j("3"), "3", true),
            (j("1"), j("2"), j("2"), "2", false),
            (j("null"), None, j("null"), "MISSING", false),
            (None, j("null"), j("1"), "1", true),
            (j("true"), j("1"), j("2"), "2", false),
            (j("[1]"), j("[1, 2]"), j("[1, 3]"), "[1, 3]", true),
            (j(r#"{"a": 1}"#), j("[1]"), j(r#"{"a": 2}"#), r#"{"a": 2}"#, true),
            (j("1"), j(r#"{"a": 1}"#), j(r#"{"b": 2}"#), r#"{"b": 2, "a": 1}"#, false),
            (None, j(r#"{"a": 1}"#), j(r#"{"a": 2}"#), r#"{"a": 2}"#, true),
            (
                j(r#"{"model": "base", "theme": "dark", "obsolete": true, "permissions": {"allow": ["Read"], "deny": ["Write"]}}"#),
                j(r#"{"model": "alice", "theme": "dark", "permissions": {"allow": ["Read", "Bash"], "deny": ["Write"]}}"#),
                j(r#"{"model": "base", "theme": "light", "obsolete": true, "permissions": {"allow": ["Read"], "deny": ["Edit"]}}"#),
                r#"{"model": "alice", "theme": "light", "permissions": {"allow": ["Read", "Bash"], "deny": ["Edit"]}}"#,
                false,
            ),
            (
                j(r#"{"a": 1, "b": 1, "c": {"x": 1}, "gone": 1}"#),
                j(r#"{"new": 0, "c": {"x": 2, "y": 1}, "b": 2, "a": 1}"#),
                j(r#"{"gone": 1, "b": 3, "a": 1, "c": {"x": 3}, "theirs": 5}"#),
                r#"{"b": 3, "a": 1, "c": {"x": 3, "y": 1}, "theirs": 5, "new": 0}"#,
                true,
            ),
        ];
        for (base, ours, theirs, want, conflict) in cases {
            let shown = format!("{base:?} {ours:?} {theirs:?}");
            assert_eq!(merged(base, ours, theirs), (want.to_string(), conflict), "{shown}");
        }
    }

    #[test]
    fn blank_json_is_an_empty_object() {
        for blank in ["", " \n\t", "\u{a0}\u{3000}", "\u{1c}"] {
            assert_eq!(config_json(blank).unwrap().py_repr(), "{}", "{blank:?}");
        }
        assert_eq!(config_json(" [1] ").unwrap().py_repr(), "[1]");
        assert_eq!(config_json("{\"a\":").unwrap_err().0, "Expecting value: line 1 column 6 (char 5)");
        assert_eq!(
            config_json("\u{feff}{}").unwrap_err().0,
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"
        );
        assert!(config_json("not json\n").is_err());
    }

    fn put(p: &str, data: &[u8], mtime: i64) {
        std::fs::create_dir_all(path::dirname(p)).unwrap();
        std::fs::write(p, data).unwrap();
        stamp(p, mtime, 0);
    }

    fn stamp(p: &str, sec: i64, nsec: i64) {
        let c = std::ffi::CString::new(p).unwrap();
        let ts = libc::timespec { tv_sec: sec, tv_nsec: nsec };
        // SAFETY: c 以 0 结尾，times 是两项的数组
        assert_eq!(unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [ts, ts].as_ptr(), libc::AT_SYMLINK_NOFOLLOW) }, 0);
    }

    fn read(p: &str) -> String {
        String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned()
    }

    fn mtime_ns(p: &str) -> (i64, i64) {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(p).unwrap();
        (m.mtime(), m.mtime_nsec())
    }

    #[test]
    fn lines_are_appended_once() {
        let d = tmpdir("lines");
        let (src, dst) = (format!("{d}/src.jsonl"), format!("{d}/dst.jsonl"));
        // 右边是原版 merge_lines 的结果：只认 \n、\r、\r\n；src 里重复的新行照样各加一遍；dst 结尾没换行先补
        std::fs::write(&dst, b"a\nb\r\nc").unwrap();
        std::fs::write(&src, b"b\nx\x0cy\rd\n\nd\nc\n\xff\n").unwrap();
        merge_lines(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"a\nb\r\nc\nx\x0cy\nd\n\nd\n\xff\n");
        // 没有新行：一个字节都不动
        std::fs::write(&src, b"a\r\nc\rb").unwrap();
        merge_lines(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"a\nb\r\nc\nx\x0cy\nd\n\nd\n\xff\n");
        // dst 是空的、是链接：写到它指向的文件里
        std::fs::write(format!("{d}/real"), b"").unwrap();
        std::os::unix::fs::symlink(format!("{d}/real"), format!("{d}/link.jsonl")).unwrap();
        merge_lines(&src, &format!("{d}/link.jsonl")).unwrap();
        assert!(path::islink(&format!("{d}/link.jsonl")));
        assert_eq!(std::fs::read(format!("{d}/real")).unwrap(), b"a\nc\nb\n");
        assert!(merge_lines(&src, &format!("{d}/none.jsonl")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn newer_file_wins_and_keeps_its_mtime() {
        let d = tmpdir("file");
        let (src, dst) = (format!("{d}/src.md"), format!("{d}/dst.md"));
        // 内容不一样、src 较新：换成 src 的，修改时间（到纳秒）、权限跟着 src
        put(&src, b"new", 200);
        stamp(&src, 200, 123456789);
        fs::chmod(&src, 0o640).unwrap();
        put(&dst, b"old", 100);
        merge_file(&src, &dst).unwrap();
        assert_eq!((read(&dst).as_str(), mtime_ns(&dst), fs::mode_of(&dst).unwrap()), ("new", (200, 123456789), 0o640));
        // src 较旧、一样新：不动
        put(&dst, b"keep", 200);
        stamp(&dst, 200, 123456789);
        merge_file(&src, &dst).unwrap();
        assert_eq!(read(&dst), "keep");
        stamp(&dst, 300, 0);
        merge_file(&src, &dst).unwrap();
        assert_eq!(read(&dst), "keep");
        // 内容一样：不管新旧都不动（修改时间也不动）
        put(&dst, b"new", 100);
        merge_file(&src, &dst).unwrap();
        assert_eq!(mtime_ns(&dst), (100, 0));
        // dst 是链接：换掉的是它指向的文件
        put(&format!("{d}/real.md"), b"old", 100);
        std::os::unix::fs::symlink(format!("{d}/real.md"), format!("{d}/link.md")).unwrap();
        merge_file(&src, &format!("{d}/link.md")).unwrap();
        assert!(path::islink(&format!("{d}/link.md")));
        assert_eq!(read(&format!("{d}/real.md")), "new");
        assert_eq!(fs::listdir_sorted(&d).unwrap(), ["dst.md", "link.md", "real.md", "src.md"]); // 临时文件没留下
                                                                                                 // 一个不存在：是错（原来是没人接的 FileNotFoundError）
        assert!(merge_file(&src, &format!("{d}/none.md")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn close_mtimes_compare_like_python_floats() {
        // Python 比的是 st_mtime（小数）：差不到一两百纳秒的两个时间换成小数以后一样大，不算较新
        let d = tmpdir("float");
        let (src, dst) = (format!("{d}/src"), format!("{d}/dst"));
        put(&src, b"new", 0);
        put(&dst, b"old", 0);
        stamp(&src, 1795058400, 100);
        stamp(&dst, 1795058400, 0);
        assert_eq!(getmtime(&src).unwrap(), getmtime(&dst).unwrap());
        merge_file(&src, &dst).unwrap();
        assert_eq!(read(&dst), "old");
        stamp(&src, 1795058400, 500);
        assert!(getmtime(&src).unwrap() > getmtime(&dst).unwrap());
        merge_file(&src, &dst).unwrap();
        assert_eq!(read(&dst), "new");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn trees_are_merged_entry_by_entry() {
        let d = tmpdir("tree");
        let (src, dst) = (format!("{d}/src"), format!("{d}/dst"));
        put(&format!("{src}/only-src.md"), b"s", 100);
        put(&format!("{src}/both.md"), b"src newer", 200);
        put(&format!("{src}/older.md"), b"src older", 100);
        put(&format!("{src}/log.jsonl"), b"b\nc\n", 100);
        put(&format!("{src}/sub/deep/x.md"), b"x", 100);
        put(&format!("{src}/sub/both.md"), b"same", 100);
        put(&format!("{src}/isdir-there/f"), b"f", 100);
        put(&format!("{src}/isfile-there"), b"file in src", 300);
        std::os::unix::fs::symlink("only-src.md", format!("{src}/link")).unwrap();
        std::os::unix::fs::symlink("sub", format!("{src}/dirlink")).unwrap();
        std::os::unix::fs::symlink("/nonexistent", format!("{src}/dangling")).unwrap();
        std::os::unix::fs::symlink("sub", format!("{src}/link-there")).unwrap();
        put(&format!("{dst}/both.md"), b"dst", 100);
        put(&format!("{dst}/older.md"), b"dst newer", 200);
        put(&format!("{dst}/log.jsonl"), b"a\nb\n", 100);
        put(&format!("{dst}/sub/both.md"), b"same", 50);
        put(&format!("{dst}/only-dst.md"), b"d", 100);
        put(&format!("{dst}/isdir-there"), b"a file", 100);
        put(&format!("{dst}/isfile-there/keep"), b"k", 100);
        put(&format!("{dst}/link-there"), b"real file in dst", 100);
        // dst 里同一个位置是文件、src 里是目录：原来是没人接的 FileExistsError（前面能并的已经并了）
        assert!(merge_tree(&src, &dst).is_err());
        std::fs::remove_file(format!("{dst}/isdir-there")).unwrap();
        merge_tree(&src, &dst).unwrap();
        let mut seen = Vec::new();
        walk(&dst, |root, dirs, files| {
            dirs.sort();
            for n in dirs.iter().chain(files) {
                let p = path::join(root, n);
                let what = if path::islink(&p) {
                    format!("-> {}", readlink(&p).unwrap())
                } else if path::isdir(&p) {
                    "/".to_string()
                } else {
                    read(&p)
                };
                seen.push(format!("{} {}", path::relpath(&p, &dst), what));
            }
            Ok(())
        })
        .unwrap();
        seen.sort();
        assert_eq!(
            seen,
            [
                "both.md src newer",
                "dangling -> /nonexistent",
                "dirlink -> sub",
                "isdir-there /",
                "isdir-there/f f",
                "isfile-there /",
                "isfile-there/keep k",
                "link -> only-src.md",
                "link-there real file in dst",
                "log.jsonl a\nb\nc\n",
                "older.md dst newer",
                "only-dst.md d",
                "only-src.md s",
                "sub /",
                "sub/both.md same",
                "sub/deep /",
                "sub/deep/x.md x",
            ]
        );
        assert_eq!(mtime_ns(&format!("{dst}/only-src.md")), (100, 0));
        assert_eq!(mtime_ns(&format!("{dst}/sub/both.md")), (50, 0));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn tree_state_lists_files_and_links() {
        let d = tmpdir("state");
        put(&format!("{d}/t/b.json"), b"b", 100);
        put(&format!("{d}/t/a/x"), b"x", 300);
        put(&format!("{d}/t/a/.hidden"), b"", 100);
        std::fs::create_dir(format!("{d}/t/empty")).unwrap();
        std::os::unix::fs::symlink("a", format!("{d}/t/dirlink")).unwrap();
        std::os::unix::fs::symlink("gone", format!("{d}/t/z")).unwrap();
        for p in ["t/a", "t/empty", "t/dirlink", "t/z", "t"] {
            stamp(&format!("{d}/{p}"), 100, 0);
        }
        let (items, newest) = tree_state(&format!("{d}/t")).unwrap();
        let shown: Vec<String> = items.iter().map(|(p, v)| format!("{} {}", p, String::from_utf8_lossy(v))).collect();
        // 这一层：目录（排过序）在前、文件在后；空目录不算一项；指向目录的链接算一项，不进去
        assert_eq!(
            shown,
            [
                "dirlink link:a",
                "b.json 3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d",
                "z link:gone",
                "a/.hidden e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "a/x 2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
            ]
        );
        assert_eq!(newest, 300.0);
        // 最晚的修改时间：目录自己的、链接自己的也算
        stamp(&format!("{d}/t/empty"), 400, 0);
        assert_eq!(tree_state(&format!("{d}/t")).unwrap().1, 400.0);
        stamp(&format!("{d}/t/z"), 500, 0);
        assert_eq!(tree_state(&format!("{d}/t")).unwrap().1, 500.0);
        stamp(&format!("{d}/t"), 600, 0);
        assert_eq!(tree_state(&format!("{d}/t")).unwrap().1, 600.0);
        assert_eq!(tree_state(&format!("{d}/t")).unwrap().0, items);
        assert!(tree_state(&format!("{d}/none")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn legacy_links_are_detached_into_copies() {
        let d = tmpdir("detach");
        let (master, client) = (format!("{d}/master"), format!("{d}/client"));
        std::fs::create_dir_all(&client).unwrap();
        put(&format!("{master}/state.json"), b"state", 100);
        fs::chmod(&format!("{master}/state.json"), 0o640).unwrap();
        put(&format!("{master}/runtime/sub/lease"), b"lease", 100);
        fs::chmod(&format!("{master}/runtime/sub"), 0o750).unwrap();
        for p in ["runtime/sub", "runtime"] {
            stamp(&format!("{master}/{p}"), 150, 0);
        }
        // 相对的、绝对的写法都认；拆开以后是独立的一份，权限、修改时间跟着原来的，共享源不动
        std::os::unix::fs::symlink("../master/state.json", format!("{client}/state.json")).unwrap();
        std::os::unix::fs::symlink(format!("{master}/runtime"), format!("{client}/runtime")).unwrap();
        detach_legacy_shared(&format!("{client}/state.json"), &format!("{master}/state.json")).unwrap();
        detach_legacy_shared(&format!("{client}/runtime"), &format!("{master}/runtime")).unwrap();
        assert!(!path::islink(&format!("{client}/state.json")) && !path::islink(&format!("{client}/runtime")));
        assert!(!path::samefile(&format!("{client}/state.json"), &format!("{master}/state.json")));
        assert_eq!(
            (read(&format!("{client}/state.json")).as_str(), fs::mode_of(&format!("{client}/state.json")).unwrap()),
            ("state", 0o640)
        );
        assert_eq!(mtime_ns(&format!("{client}/state.json")), (100, 0));
        assert_eq!(read(&format!("{client}/runtime/sub/lease")), "lease");
        assert_eq!(
            (
                fs::mode_of(&format!("{client}/runtime/sub")).unwrap(),
                mtime_ns(&format!("{client}/runtime/sub")),
                mtime_ns(&format!("{client}/runtime"))
            ),
            (0o750, (150, 0), (150, 0))
        );
        assert_eq!(fs::listdir_sorted(&client).unwrap(), ["runtime", "state.json"]); // 临时目录没留下
        assert_eq!(read(&format!("{master}/runtime/sub/lease")), "lease");
        // 不是链接、指向别处的链接、指向主目录里别的名字的链接：不动
        std::os::unix::fs::symlink(format!("{master}/state.json"), format!("{client}/other-name.json")).unwrap();
        std::os::unix::fs::symlink(format!("{d}/outside"), format!("{client}/mine")).unwrap();
        detach_legacy_shared(&format!("{client}/other-name.json"), &format!("{master}/other-name.json")).unwrap();
        detach_legacy_shared(&format!("{client}/mine"), &format!("{master}/mine")).unwrap();
        detach_legacy_shared(&format!("{client}/state.json"), &format!("{master}/state.json")).unwrap();
        assert!(path::islink(&format!("{client}/other-name.json")) && path::islink(&format!("{client}/mine")));
        // 指向主目录同名项、那一项已经不在了：链接删掉
        std::os::unix::fs::symlink(format!("{master}/old.json"), format!("{client}/old.json")).unwrap();
        detach_legacy_shared(&format!("{client}/old.json"), &format!("{master}/old.json")).unwrap();
        assert!(!path::lexists(&format!("{client}/old.json")) && !path::lexists(&format!("{master}/old.json")));
        // 目录里面有链接：不拆，什么都不动
        put(&format!("{master}/nested/a/f"), b"f", 100);
        std::os::unix::fs::symlink(format!("{d}/outside"), format!("{master}/nested/a/inner")).unwrap();
        std::os::unix::fs::symlink(format!("{master}/nested"), format!("{client}/nested")).unwrap();
        match detach_legacy_shared(&format!("{client}/nested"), &format!("{master}/nested")) {
            Err(crate::py::Fail::Die(m)) => {
                assert_eq!(m, format!("旧共享目录包含内部符号链接，无法自动拆分，原目录未改动：{client}/nested"))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(readlink(&format!("{client}/nested")).unwrap(), format!("{master}/nested"));
        assert_eq!(fs::listdir_sorted(&client).unwrap(), ["mine", "nested", "other-name.json", "runtime", "state.json"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn merge_helpers_handle_plain_values() {
        // list! / dict! 造出来的值和读进来的一样合并
        let (v, conflict) = merge_config_json(
            Some(V::Dict(dict! { "a" => list![1] })),
            Some(V::Dict(dict! { "a" => list![1], "b" => 2 })),
            Some(V::Dict(dict! { "a" => list![1, 2] })),
        );
        assert_eq!((json::dumps(&v.unwrap(), json::LINE).as_str(), conflict), (r#"{"a": [1, 2], "b": 2}"#, false));
    }
}
