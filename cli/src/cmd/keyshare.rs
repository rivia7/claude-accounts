//! 共享记下的 API key（`keyshare_on` 到 `cmd_key_sync`）：key-sync。
//!
//! 子命令还没移植。记下、删掉 key 时要用的这几样已经移植（secrets 模块调）：`keyshare_on`、`keyshare_edit`（keys.json 的
//! 读改写）、`keyshare_kick`。

use super::presence::{presence_load, presence_mapped, spawn_background, PRESENCE_DIR};
use super::Cmd;
use crate::core::model::{acct_id, is_api};
use crate::core::policy::policy_raw;
use crate::core::tool::{t, Kind};
use crate::dict;
use crate::py::fs::{load_quiet, save_json};
use crate::py::hash::sha256_hex;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::time::time;
use crate::py::R;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::io::AsRawFd;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

/// PRESENCE_DIR 里：{"v": 1, "acc": {工具:账号目录名: {"at": 时间, "sum": key 的摘要 | null}},
/// "synced": 上次同步, "retry": 旧的服务器什么时候再试, "error": 没同步成的原因}
pub const KEYSHARE_FILE: &str = "keys.json";
/// 后台最多隔这么久同步一次（在这台设备上记下、删掉时马上）
pub const KEYSHARE_EVERY: f64 = 600.0;

/// `isinstance(x, (int, float)) and not isinstance(x, bool)`
pub fn is_num(v: &V) -> bool {
    matches!(v, V::Int(_) | V::Big(_) | V::Float(_))
}

/// 这个账号（当前工具的）记下备查的 key 会不会同步：映射里的 Claude、Codex 官方账号，这台设备开着设备统计
pub fn keyshare_on(key: &str, v: &V) -> R<bool> {
    Ok(matches!(t().kind, Kind::Claude | Kind::Codex) && !is_api(v) && presence_mapped(&policy_raw(t()), key, v) && presence_load()?.1.is_some())
}

/// key 的摘要（没有 key、删掉了是 None）
pub fn keyshare_sum(s: Option<&str>) -> V {
    match s.filter(|s| !s.is_empty()) {
        Some(s) => V::from(&sha256_hex(s)[..16]),
        None => V::None,
    }
}

/// keys.json 里这个账号（当前工具的）的位置：按这台设备上的账号目录名，和盐无关（换了盐也认得出谁新）
pub fn keyshare_slot(key: &str) -> String {
    format!("{}:{}", t().name, acct_id(key, t()))
}

/// 建 PRESENCE_DIR，但不建 ~/.cca 本身：cca restore 刚删掉它时，后台还没跑完的同步不该把它建回来（没有时 OSError）
pub fn presence_dir() -> std::io::Result<()> {
    if !path::isdir(&PRESENCE_DIR) {
        match std::fs::DirBuilder::new().mode(0o700).create(&*PRESENCE_DIR) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// keys.json 的读改写（keys.lock：在这台设备上记下、删掉 key 和同步时照服务器上的改 key 互斥，拿着的时间很短）。
/// `enter` 拿锁、读出来；改完 `exit(true)` 写回去（出了错的 `exit(false)` 不写）；锁在这个值没了的时候放开
pub struct KeyshareFile {
    _lock: std::fs::File,
    pub j: Dict,
}

impl KeyshareFile {
    pub fn enter() -> R<KeyshareFile> {
        presence_dir()?;
        let lock = std::fs::OpenOptions::new().append(true).create(true).open(path::join(&PRESENCE_DIR, "keys.lock"))?;
        // SAFETY: lock 开着，fd 有效
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let j = load_quiet(&path::join(&PRESENCE_DIR, KEYSHARE_FILE)).and_then(|j| j.as_dict());
        let j = j.filter(|j| j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && j.dict("acc").is_some());
        Ok(KeyshareFile { _lock: lock, j: j.unwrap_or_else(|| dict! { "v" => 1, "acc" => Dict::new() }) })
    }

    pub fn exit(self, ok: bool) -> R<()> {
        if ok {
            save_json(&path::join(&PRESENCE_DIR, KEYSHARE_FILE), &V::Dict(self.j.clone()), Some(0o600), true)?;
        }
        Ok(())
    }
}

/// `with keyshare_edit(键, 时间) as ed`：在这台设备上记下（换掉）、删掉这个账号（当前工具的）记下备查的 key，改完
/// `ed.mark(key 或 None)` 记下时间（同步时按它比新旧）。拿着 keys.lock：同步不会在这中间拿服务器上旧的盖掉刚记下的。
/// 没开设备统计时不记（以后开了，记下的按 key_hints 里的时间算，删掉的不同步）；出错不影响记下、删掉本身
pub struct KeyshareEdit {
    key: String,
    at: f64,
    f: Option<KeyshareFile>,
}

/// `keyshare_edit(key, at).__enter__()`
pub fn keyshare_edit(key: &str, at: f64) -> KeyshareEdit {
    let open = || -> R<Option<KeyshareFile>> {
        if presence_load()?.1.is_some() {
            return Ok(Some(KeyshareFile::enter()?));
        }
        Ok(None)
    };
    let f = match open() {
        Ok(f) => f,
        Err(e) if e.is_os_or_value() => None,
        Err(e) => panic!("keyshare_edit：{e}"),
    };
    KeyshareEdit { key: key.to_string(), at, f }
}

impl KeyshareEdit {
    /// `ed.mark(s)`
    pub fn mark(&mut self, s: Option<&str>) {
        if let Some(f) = &self.f {
            f.j.dict("acc").expect("enter 保证了 acc 是字典").set(keyshare_slot(&self.key), dict! { "at" => self.at, "sum" => keyshare_sum(s) });
        }
    }

    /// `__exit__`；ok：with 里面没出异常（出了就不写 keys.json）
    pub fn exit(self, ok: bool) {
        if let Some(f) = self.f {
            match f.exit(ok) {
                Err(e) if !e.is_os_or_value() => panic!("keyshare_edit：{e}"),
                _ => {}
            }
        }
    }
}

/// 在后台跑一次 cca _key-sync（同步记下备查的 API key）；force 以外还不到时候就不起进程。静默
pub fn keyshare_kick(force: bool) {
    let kick = || -> R<()> {
        if presence_load()?.1.is_none() {
            return Ok(());
        }
        let j = load_quiet(&path::join(&PRESENCE_DIR, KEYSHARE_FILE)).and_then(|j| j.as_dict()).unwrap_or_default();
        let last = |k: &str| j.get(k).filter(is_num).and_then(|v| v.as_num()).unwrap_or(0.0);
        if !force && (time() - last("synced") < KEYSHARE_EVERY || time() < last("retry")) {
            return Ok(());
        }
        if force {
            spawn_background(&["_key-sync", "force"]);
        } else {
            spawn_background(&["_key-sync"]);
        }
        Ok(())
    };
    match kick() {
        Err(e) if !e.is_os_or_value() => panic!("keyshare_kick：{e}"),
        _ => {}
    }
}
