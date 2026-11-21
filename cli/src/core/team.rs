//! Team 在本机的状态（~/.cca/teams/）：加入了哪些 Team、各自的服务器、这台设备在里面是谁、拿到的配置。只读文件，不联网、不碰密钥
//! （加入、取配置、管理员的操作在 cmd/team.rs；线上格式在 cmd/team/doc.rs）。计划和信任怎么来见 docs/dev/team.md。
//!
//! ```text
//! teams/index.json         {"v": 1, "order": [编号…]}：加入的先后（同一个账号出现在两个 Team 里时先加入的算数）
//! teams/<编号>/team.json   这个 Team：名字、服务器、根公钥、这台设备的角色和编号（见 Team）
//! teams/<编号>/doc.json    {"v": 1, "seq": 序号, "at": 什么时候取到的, "doc": 验过签名、解开的配置}
//! teams/<编号>/sync.json   {"v": 1, "state": ok | removed | lost | error, "error": 原因, "at"}：最近一次取配置的结果（变了才写）
//! teams/<编号>/id_ed25519  这台设备在这个 Team 里的密钥；secret 是邀请里那串随机数（配置的密钥由它派生）
//! teams/<编号>/root_ed25519、admin.json  只有管理员的设备有：Team 的根密钥、管理员的状态（cmd/team/admin.rs）
//! ```
//! 整个目录在 CCA_DIR_PRIVATE 里（cca 启动的 Claude Code 读写不了），`cca restore` 的备份不带它。

use crate::core::base::CCA_HOME;
use crate::py::fs::load_quiet;
use crate::py::obj::{Dict, V};
use crate::py::path;
use std::sync::LazyLock;

pub static TEAMS_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "teams"));

/// Team 的编号：根公钥的 SHA-256 的前 32 个十六进制字符（只当名字用：认一个 Team 比的是整把根公钥）
pub fn tid_ok(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn team_dir(tid: &str) -> String {
    path::join(&TEAMS_DIR, tid)
}

pub fn team_path(tid: &str, name: &str) -> String {
    path::join(&team_dir(tid), name)
}

pub fn index_path() -> String {
    path::join(&TEAMS_DIR, "index.json")
}

/// 这台设备在 Team 里的角色
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// 拿着根密钥：管成员、发布配置
    Admin,
    Member,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

/// team.json：{"v": 1, "id", "name", "url": 服务器（管理员还没接上服务器时是空的）, "pins", "root": 根公钥（ssh-ed25519 AAAA…）,
/// "role": admin | member, "dev": 这台设备在 Team 里的编号, "device": 它在服务器上的名字, "joined_at", "gen": 服务器上这一份记录的编号}
#[derive(Clone, Debug)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub url: String,
    pub pins: Vec<String>,
    pub root: String,
    pub role: Role,
    pub dev: String,
    pub device: String,
    /// team.json 原样（别的项：joined_at、gen）
    pub raw: Dict,
}

impl Team {
    pub fn path(&self, name: &str) -> String {
        team_path(&self.id, name)
    }

    /// 接上服务器了没有（管理员刚建的 Team 还没有）
    pub fn attached(&self) -> bool {
        !self.url.is_empty()
    }
}

/// 读一个 Team；没有、读不了、要紧的几项不合格是 None
pub fn load_team(tid: &str) -> Option<Team> {
    if !tid_ok(tid) {
        return None;
    }
    let j = load_quiet(&team_path(tid, "team.json"))?.as_dict()?;
    let s = |k: &str| j.str(k).map(|v| v.to_string()).unwrap_or_default();
    let role = match s("role").as_str() {
        "admin" => Role::Admin,
        "member" => Role::Member,
        _ => return None,
    };
    let pins: Vec<String> = j.list("pins")?.items().iter().map(|p| p.as_str().map(String::from)).collect::<Option<_>>()?;
    let ok = j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && s("id") == tid && !s("name").is_empty() && !s("root").is_empty();
    ok.then(|| Team {
        id: tid.to_string(),
        name: s("name"),
        url: s("url"),
        pins,
        root: s("root"),
        role,
        dev: s("dev"),
        device: s("device"),
        raw: j.clone(),
    })
}

/// index.json 里的先后（不合格的项不算）
pub fn index_order() -> Vec<String> {
    let order = load_quiet(&index_path()).and_then(|j| j.as_dict()).and_then(|j| j.list("order"));
    let mut out: Vec<String> = Vec::new();
    for x in order.map(|o| o.items()).unwrap_or_default() {
        if let Some(t) = x.as_str().filter(|t| tid_ok(t) && !out.iter().any(|o| o == t)) {
            out.push(t.to_string());
        }
    }
    out
}

/// 这台设备在的 Team，照加入的先后；index.json 里没有、目录在的排在后面（建到一半、加入到一半被打断：目录有了，还没记进 index.json）。
/// 没有 teams/ 时不读别的文件（没用 Team 的机器上不多花时间）
pub fn teams() -> Vec<Team> {
    let mut ids = index_order();
    let mut extra: Vec<String> = match std::fs::read_dir(&*TEAMS_DIR) {
        Ok(entries) => entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => return Vec::new(),
    };
    extra.retain(|t| tid_ok(t) && !ids.contains(t));
    extra.sort();
    ids.extend(extra);
    ids.iter().filter_map(|t| load_team(t)).collect()
}

/// 拿到的配置：(序号, 配置)；还没取到、读不了是 None
pub fn team_doc(tid: &str) -> Option<(i64, Dict)> {
    let j = load_quiet(&team_path(tid, "doc.json"))?.as_dict()?;
    Some((j.get("seq")?.as_int().filter(|n| *n > 0)?, j.dict("doc")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tid_shape() {
        assert!(tid_ok(&"0a".repeat(16)));
        assert!(!tid_ok(&"0A".repeat(16)));
        assert!(!tid_ok(&"0a".repeat(8)));
        assert!(!tid_ok("../../../../../../../../etc/passwd"));
    }
}
