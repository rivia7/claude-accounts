//! Team 在本机的状态（~/.cca/teams/）：加入了哪些 Team、各自的服务器、这台设备在里面是谁、拿到的配置。只读文件，不联网、不碰密钥
//! （加入、同步配置、管理员的操作在 cmd/team.rs；线上格式在 cmd/team/doc.rs）。计划和信任怎么来见 docs/dev/team.md。
//!
//! ```text
//! teams/index.json         {"v": 1, "order": [编号…]}：加入的先后（同一个账号出现在两个 Team 里时先加入的算数）
//! teams/<编号>/team.json   这个 Team：名字、服务器、根公钥、这台设备的角色和编号（见 Team）
//! teams/<编号>/doc.json    {"v": 1, "seq": 序号, "at": 什么时候同步到的, "doc": 验过签名、解开的配置}
//! teams/<编号>/sync.json   {"v": 1, "state": ok | removed | lost | error, "error": 原因, "at"}：最近一次同步配置的结果（变了才写）
//! teams/<编号>/id_ed25519  这台设备在这个 Team 里的密钥；secret 是邀请里那串随机数（配置的密钥由它派生）
//! teams/<编号>/root_ed25519、admin.json  只有管理员的设备有：Team 的根密钥、管理员的状态（cmd/team/admin.rs）
//! teams/<编号>/admins.json  验过根密钥签名的管理员名单（3.6.15 起）：{"v": 1, "list", "sig", "epoch", "floor": 根密钥签的配置里说过的最大纪元}
//! teams/<编号>/admin_key   别的管理员（team.json 的 coadmin 是 true：建 Team 的设备给的）才有：几位管理员共用的状态的密钥；
//!                          admin.json 是从服务器上取回来的共用的状态
//! ```
//! 整个目录在 CCA_DIR_PRIVATE 里（cca 启动的 Claude Code 读写不了），`cca restore` 的备份不带它。

use crate::core::base::CCA_HOME;
use crate::py::fs::load_quiet;
use crate::py::obj::{Dict, V};
use crate::py::path;
use std::cell::RefCell;
use std::rc::Rc;
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

    /// 是别的管理员（3.6.15 起）：这台设备是成员，建 Team 的设备经根密钥签的配置给了它管理员的密钥。没有根密钥：换不了管理员名单，
    /// 也删不了 Team、换不了服务器
    pub fn coadmin(&self) -> bool {
        self.role == Role::Member && self.raw.get("coadmin").is_some_and(|c| matches!(c, V::Bool(true)))
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

/// 拿到的配置：(序号, 配置)；还没同步下来、读不了是 None
pub fn team_doc(tid: &str) -> Option<(i64, Dict)> {
    let j = load_quiet(&team_path(tid, "doc.json"))?.as_dict()?;
    Some((j.get("seq")?.as_int().filter(|n| *n > 0)?, j.dict("doc")?))
}

/// 一个 Team 给这台设备的配置（验过签名、解开的那一份：账号 → 节点、别名、节点的定义……样子见 cmd/team/doc.rs 的 doc_problems）
pub struct Grant {
    pub id: String,
    pub name: String,
    pub doc: Dict,
    /// 这台设备是这个 Team 的管理员（它给的别名不往这台设备上已有的账号上改：policy_alias_fill、team_alias_follow）
    pub admin: bool,
}

thread_local! {
    static GRANTS: RefCell<Option<Rc<Vec<Grant>>>> = const { RefCell::new(None) };
}

/// 各个 Team 给这台设备的配置，照加入的先后（同一个账号出现在两个 Team 里，先加入的算数）。还没同步到配置的不在里面；被管理员移除了的
/// （sync.json 是 removed）也不在：手里那一份不再算数，它给的账号收起来。一个进程里只读一次（对账时每张账号表都要问）
pub fn grants() -> Rc<Vec<Grant>> {
    if let Some(g) = GRANTS.with(|g| g.borrow().clone()) {
        return g;
    }
    let mut out = Vec::new();
    for t in teams() {
        if let Some((_, doc)) = team_doc(&t.id).filter(|_| !removed(&t.id)) {
            out.push(Grant { id: t.id, name: t.name, doc, admin: t.role == Role::Admin });
        }
    }
    let out = Rc::new(out);
    GRANTS.with(|g| *g.borrow_mut() = Some(out.clone()));
    out
}

/// 忘掉记着的那一份：下一次 grants() 重读。常驻的进程（上报进程）每一圈调一次；刚写了 doc.json、sync.json、删了 Team 的命令也调
pub fn forget_grants() {
    GRANTS.with(|g| *g.borrow_mut() = None);
}

/// 被管理员移除了（最近一次同步配置时服务器说的）
fn removed(tid: &str) -> bool {
    load_quiet(&team_path(tid, "sync.json")).and_then(|j| j.as_dict()).is_some_and(|j| j.str("state").as_deref() == Some("removed"))
}

/// 这个 Team 现在给不给账号说得准吗：它的目录没了（退出了）、被管理员移除了，或者它的配置读得出来（那就照配置的）。目录还在、配置却
/// 读不出来（文件坏了、一时读不了）是说不准：这时不因为映射里没有，就把它加的账号收起来
pub fn grant_settled(tid: &str) -> bool {
    !path::isdir(&team_dir(tid)) || removed(tid) || grants().iter().any(|g| g.id == tid)
}

/// 这个 Team 叫什么（给人看的；不在了、还没同步到配置时是 None）
pub fn grant_name(tid: &str) -> Option<String> {
    grants().iter().find(|g| g.id == tid).map(|g| g.name.clone())
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
