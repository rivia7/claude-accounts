//! 折合金额（`cost_claude` 到 `CostView`）：cost-worker、cost-sync、cost-stop。
//!
//! 子命令还没移植。会话结束时在后台起统计要用的这几样已经移植（link 模块调）：`cost_on`、`cost_dir`、`cost_touch`、
//! `cost_mtime`、`cost_kick`。

use super::presence::spawn_background;
use super::Cmd;
use crate::core::base::{CCA_HOME, CFG};
use crate::py::fs::{load_quiet, utime_now};
use crate::py::obj::V;
use crate::py::path;
use crate::py::proc::getenv;
use crate::py::time::time;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[];

pub static COST_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "cost"));
/// 后台统计最多隔这么久起一次
pub const COST_EVERY: f64 = 300.0;

/// 折合金额开着吗：config.json 里没写 "cost": false
pub fn cost_on() -> bool {
    !load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("cost"), Some(V::Bool(false))))
}

/// 建 COST_DIR，但不建 ~/.cca 本身（和 presence_dir 一样：cca restore 刚删掉它时，后台还没跑完的统计不该把它建回来）
pub fn cost_dir() -> std::io::Result<()> {
    if !path::isdir(&COST_DIR) {
        match std::fs::DirBuilder::new().mode(0o700).create(&*COST_DIR) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// cost/ 里的一个空文件，修改时间改成现在（没有就建）
pub fn cost_touch(name: &str) -> std::io::Result<()> {
    let p = path::join(&COST_DIR, name);
    std::fs::OpenOptions::new().append(true).create(true).open(&p)?;
    utime_now(&p)
}

/// cost/ 里这个文件的修改时间；没有、读不了是 None
pub fn cost_mtime(name: &str) -> Option<f64> {
    let m = std::fs::metadata(path::join(&COST_DIR, name)).ok()?;
    Some(m.mtime() as f64 + m.mtime_nsec() as f64 / 1e9)
}

/// 在后台跑一次 cca _cost-sync（读新增的会话记录、和别的设备同步）；上次起过还不到 COST_EVERY 就不起。静默。
/// CCA_COST=off（回归测试）不起
pub fn cost_kick() {
    if getenv("CCA_COST").as_deref() == Some("off") || !path::isfile(&CFG) || !cost_on() {
        return;
    }
    if let Some(stamp) = cost_mtime("last").filter(|s| *s != 0.0) {
        let age = time() - stamp;
        if (0.0..COST_EVERY).contains(&age) {
            return;
        }
    }
    // 建不了目录、碰不了文件（OSError）就算了
    if cost_dir().is_err() || cost_touch("last").is_err() {
        return;
    }
    spawn_background(&["_cost-sync"]);
}
