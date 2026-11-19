//! 设备标识（`device_identity`、`prepare_device`）、`.claude.json` 的三方合并（`claude_sync`）、共享目录的链接和并回
//! （`adopt*`、`merge_*`、`cmd_link`）：子命令 device、sync、link。对应 PY_SRC 从「账号设备标识」到 `cmd_link` 那一段。
//!
//! 每次会话前后都要跑（`py link`、`py sync`、`py device`），三种工具都用：差别来自 `core::tool::Tool` 的
//! `shares`、`shared_config`、`precreate`、`nests`、`config_names`、`sub`、`master`，和这里的 `Tool::sync` / `Tool::after_link`。
//!
//! - `device.rs`：账号设备标识
//! - `shared.rs`：三方合并、并回、拆旧链接
//! - 文件操作（os.walk、shutil、filecmp……）在兼容层的 `py::shutil`（移植这个模块时写的）
//!
//! 收尾的两步在别的模块：`cost::cost_kick`（会话结束后在后台统计折合金额）、`codex::codex_fix_rollouts`（Codex 的会话库）。

mod device;
mod shared;

#[allow(unused_imports)]
pub use device::*;
#[allow(unused_imports)]
pub use shared::*;

use super::{arg, arity, Cmd};
use crate::core::base::Lock;
use crate::core::model::cdir;
use crate::core::tool::{t, Kind, Tool};
use crate::py::fs::{self, listdir_sorted, load_json, save_json};
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::shutil as pylib;
use crate::py::{die, R};
use std::collections::BTreeSet;

pub fn cmd_device(args: &[String]) -> R<()> {
    arity("cmd_device", args, 1, 1)?;
    let _lock = Lock::acquire()?;
    prepare_device(&args[0])
}

pub fn cmd_sync(args: &[String]) -> R<()> {
    arity("cmd_sync", args, 1, 1)?;
    t().sync(&args[0])
}

impl Tool {
    /// `T.sync(key)`：只有 Claude 有一份和 `~/.claude.json` 三方合并的 `.claude.json`
    pub fn sync(&self, key: &str) -> R<()> {
        match self.kind {
            Kind::Claude => claude_sync(key),
            Kind::Codex | Kind::Agy => Ok(()),
        }
    }

    /// `T.after_link(key)`：链接好以后各工具自己的收尾。Codex 把会话库里经账号目录的路径改到 ~/.codex
    pub fn after_link(&self, _key: &str) -> R<()> {
        match self.kind {
            Kind::Codex => super::codex::codex_fix_rollouts(),
            Kind::Claude | Kind::Agy => Ok(()),
        }
    }
}

/// py link 键 [cost]：链接共享目录；cost 是会话刚结束（在后台统计折合金额，最多 COST_EVERY 一次）
pub fn cmd_link(args: &[String]) -> R<()> {
    arity("cmd_link", args, 1, 2)?;
    let (key, after) = (args[0].as_str(), arg(args, 1, ""));
    let (cd, master) = (cdir(key), t().master.as_str());
    fs::makedirs(&cd)?;
    fs::makedirs(master)?;
    for d in t().precreate {
        fs::makedirs(&path::join(master, d))?;
    }
    {
        let _lock = Lock::acquire()?;
        prepare_device(key)?;
        let snapshot = path::join(&cd, ".cca-shared-base.json");
        let base = load_json(&snapshot)?.unwrap_or_else(|| V::Dict(Dict::new()));
        let Some(base) = base.as_dict().filter(|b| b.values().iter().all(|v| v.is_none() || v.is_str())) else {
            return die(format!("共享配置基线格式错误：{snapshot}"));
        };
        let levels = link_levels(&cd, master)?;
        // 名单、基线里的写法（子目录里的写成相对路径）
        let listed_as = |lvl: &str, name: &str| if lvl.is_empty() { name.to_string() } else { path::join(lvl, name) };
        for (lvl, lcd, lm) in &levels {
            for name in listdir_sorted(lcd)? {
                let n = listed_as(lvl, &name);
                let (p, to) = (path::join(lcd, &name), path::join(lm, &name));
                if !t().shares(&n) {
                    if t().kind == Kind::Claude {
                        detach_legacy_shared(&p, &to)?;
                    }
                    continue;
                }
                if path::islink(&p) {
                    if pylib::readlink(&p)? != to {
                        std::fs::remove_file(&p)?;
                    }
                    continue;
                }
                fs::makedirs(lm)?;
                if t().shared_config(&n) && path::isfile(&p) && (!path::lexists(&to) || path::isfile(&to)) {
                    merge_shared_config(&p, &to, key, base.get(&n))?;
                    std::fs::remove_file(&p)?;
                } else {
                    adopt(&p, &to, key)?;
                }
            }
        }
        let mut listed = Vec::new();
        for (lvl, lcd, lm) in &levels {
            let names = if path::isdir(lm) { listdir_sorted(lm)? } else { Vec::new() };
            for name in names {
                let n = listed_as(lvl, &name);
                listed.push(n.clone());
                let p = path::join(lcd, &name);
                if t().shares(&n) && !path::lexists(&p) {
                    pylib::symlink(path::join(lm, &name), &p)?;
                }
            }
        }
        let mut names: BTreeSet<String> = t().config_names.iter().map(|n| n.to_string()).collect();
        names.extend(base.keys().iter().map(|n| n.to_string()).chain(listed).filter(|n| t().shared_config(n)));
        let saved = Dict::new();
        for n in &names {
            let shared = path::join(master, n);
            if !path::isdir(&shared) {
                saved.set(n.as_str(), shared_text(&shared)?);
            }
        }
        if !V::Dict(saved.clone()).py_eq(&V::Dict(base)) || !path::exists(&snapshot) {
            save_json(&snapshot, &V::Dict(saved), Some(0o600), false)?;
        }
    }
    t().after_link(key)?;
    if after == "cost" && t().has_cost {
        super::cost::cost_kick();
    }
    Ok(())
}

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("link", cmd_link),
    Cmd::all("sync", cmd_sync),
    Cmd::all("device", cmd_device),
];
