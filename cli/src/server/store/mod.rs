//! 状态：名单里的设备、它们上报的会话、Team 的邀请，和存着的密文（API key、折合金额、各台设备的配置）。Python 的 `Store`（Py:387-899）。
//!
//! 四个文件，都在状态文件（--state）旁边：
//! - state.json：`{"v", "seq", "machines": {指纹: {"name", "key", "joined", "dev"}}, "beats": {指纹: {"seen", "end"}},
//!   "units": {指纹: {账号摘要: {"cli", "app", "used"}}}, "team": {"root", "gen", "invites", "spent"}}`
//! - keys.json：`{"acc": {账号摘要: {"blob", "fp", "name", "at"}}}`（心跳不用跟着重写它）
//! - cost-slots.json：`{"acc": {账号摘要: {设备指纹: {"blob", "name", "at"}}}}`
//! - team-docs.json：`{"docs": {设备编号: {"seq", "ct", "sig", "at"}}}`
//!
//! 写出去的要和 Python 写的逐字节一样（两种服务器可以换着跑同一个目录）：字典里各项的先后用 `OMap` 记着，数字用 `Num` 记着原来是
//! 整数还是小数，不认识的键原样留着；写法是 `save_doc` 的（Py:416-421：写到 `路径.进程号.tmp`，权限 600，再换过去）。
//!
//! 分在几个文件里：这里是读、筛、存和启动时的 `team_setup`；view.rs 是 report、prune、view；sync.rs 是 key_sync、cost_*；
//! team.rs 是 team_*。
//!
//! Python 里 `Store` 自己带锁、自己看时间。这里不带锁：整个放在 `Ctx.store` 的 `Mutex` 里；时间由调用方给（`now`），测试不用等。
//! 测试要换小的那几个上限在 `Limits` 里（Python 的测试是 `mock.patch` 模块里的常量）。
//!
//! 现在只有接口：里面的还没移植，`open` 回的是“未完成”。

pub mod sync;
pub mod team;
pub mod view;

use super::limits::Limits;
use super::unfinished;
use crate::py::obj::V;
use crate::py::R;
use std::path::PathBuf;

/// 启动时定下来的（命令行给的那几项，Py:2152-2158、2214）
#[derive(Clone, Debug)]
pub struct StoreCfg {
    /// --state：状态文件
    pub path: PathBuf,
    /// --ttl：开着会话却这么多秒没心跳就算离线
    pub ttl: f64,
    /// --idle：这么多秒没动静算闲置
    pub idle: f64,
    /// --retain-days 折成秒：上次使用的记录留多久
    pub retain: f64,
    /// --keys；没给就是状态文件旁边的 keys.json
    pub keys_path: Option<PathBuf>,
    /// --cost；没给就是状态文件旁边的 cost-slots.json
    pub cost_path: Option<PathBuf>,
    /// --cost-days 折成秒：折合金额的密文这么久没再传就删掉
    pub cost_keep: f64,
}

/// 加入用的邀请（请求里的 token）现在的样子（Py:715-725 的 'ok' | 'expired' | 'used' | None）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invite {
    /// 可以用
    Ok,
    /// 过期了
    Expired,
    /// 用过了
    Used,
    /// 没有这个邀请，或者不是令牌的样子：和签名不对回一样的 403，不留判断口
    Missing,
}

impl Invite {
    /// 回给设备的 error：签名对了、邀请过期了或者用过了才说是哪一种（Py:1943-1944）
    pub fn error(self) -> Option<&'static str> {
        match self {
            Invite::Expired => Some("expired"),
            Invite::Used => Some("used"),
            Invite::Ok | Invite::Missing => None,
        }
    }
}

pub struct Store {
    cfg: StoreCfg,
    lim: Limits,
    /// 配置的根公钥那一行（`ssh-ed25519 AAAA…`）
    root_key: String,
    /// 根公钥的指纹：管理员的请求的 key（它不是设备，登记、加入都不收）
    root_fp: String,
    /// 这一份记录的编号：设备拿它分清“被移除了”和“服务器的记录清掉了”
    gen: String,
}

impl Store {
    /// 读四个文件、筛掉样子不对的项（Py:433-453），再照根公钥做 `team_setup`（Py:468-494：根公钥和上次的不一样就从头开始，
    /// 以前的设备、密文都不要了，在日志里说一句）。root_line 是 `ssh-ed25519 AAAA…` 那一行；now 是服务器的时间。
    /// 存不了（状态文件所在的目录写不了……）是 Err：Python 里是没人接的异常，启动不了
    pub fn open(cfg: &StoreCfg, root_line: &str, lim: Limits, now: f64) -> R<Store> {
        let _ = (cfg, root_line, lim, now);
        unfinished("team-server 的状态（Store::open）")
    }

    /// GET /v1/health 的内容（Py:525-533）。mode 总是 team、seq 总是 0（这两项留着：以前的客户端、安装脚本照它们认是不是 Team 的
    /// 服务器）；relay_port 给了（开着中转）才列中转的那几个 op、带 relay 那一项
    pub fn health(&self, sig_ok: bool, relay_port: Option<u16>) -> V {
        let _ = (sig_ok, relay_port);
        V::None // 还没移植
    }

    /// 名单里这台设备的公钥那一行（验它的签名用）；不在名单里是 None（Py:499-501 的 machine(fp)['key']）
    pub fn machine_key(&self, fp: &str) -> Option<String> {
        let _ = fp;
        None // 还没移植
    }

    /// 名单里这台设备的名字；没有是 None（Py:516-519）
    pub fn name_of(&self, fp: &str) -> Option<String> {
        let _ = fp;
        None // 还没移植
    }

    /// 名单里各台设备的指纹（Py:521-523）：给 `Remote::keep_only` 的；先拷出来、放开这把锁，再去拿 Remote 的锁
    pub fn fps(&self) -> Vec<String> {
        Vec::new() // 还没移植
    }

    /// 根公钥的指纹（Py:512-514）
    pub fn root_fp(&self) -> &str {
        &self.root_fp
    }

    /// 配置的根公钥那一行（Py:508-510）
    pub fn root_key(&self) -> &str {
        &self.root_key
    }

    /// 这一份记录的编号（Py:503-506）
    pub fn gen(&self) -> &str {
        &self.gen
    }

    /// 启动时说的那一句里的“设备几台”（Py:2255）
    pub fn machines(&self) -> usize {
        0 // 还没移植
    }

    /// 请求里的 token（邀请里的令牌）现在的样子（Py:715-725）。token 是请求里原样的那个值：不是字符串、不是令牌的样子都是 Missing
    pub fn invite_state(&mut self, token: &V, now: f64) -> Invite {
        let _ = (token, now);
        Invite::Missing // 还没移植
    }

    /// 记下这台设备刚发过一个验过签的请求（只在内存里：不为它每个请求都重写状态文件）（Py:1969-1970）
    pub fn seen(&mut self, fp: &str, now: f64) {
        let _ = (fp, now); // 还没移植
    }

    /// 签名、时间、nonce 都验过了的请求里归这里管的那些 op：report、view、keys、ledger 和 Team 的各个 op（Py:1971-1997、
    /// 1999-2026）。返回 (状态码, 回的内容)；请求里的各项不合格是 (400, {"error": "bad"})。Err 只有存不了的时候：
    /// 调用方在日志里说一句 `cca-presence: 处理请求出错：…`、不回就断开（Py:2095-2096）
    pub fn op(&mut self, op: &str, fp: &str, j: &V, now: f64) -> R<(u16, V)> {
        let _ = (op, fp, j, now, &self.cfg, &self.lim);
        unfinished("team-server 的状态（Store::op）")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn invite_errors_name_only_the_two_safe_cases() {
        assert_eq!((Invite::Expired.error(), Invite::Used.error()), (Some("expired"), Some("used")));
        assert_eq!((Invite::Ok.error(), Invite::Missing.error()), (None, None));
    }
}
