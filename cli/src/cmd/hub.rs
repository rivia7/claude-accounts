//! 同步服务器（hub）：设备统计、记下备查的 API key、折合金额经哪台服务器和这个账号的其他设备同步。两种：
//! - 某个 Team 的（`HubKind::Team`）：Team 给这台设备的账号走它（docs/dev/team.md）
//! - 维护者的（`HubKind::Maintainer`，旧的通道）：映射里的 presence 一节，拿映射的 deploy key 签名（`presence_load`）。只管维护者映射里的
//!   官方账号；都迁成 Team 以后去掉
//!
//! 账号走哪台只看 `route`（这台设备自己的官方账号、第三方 API 账号哪都不去）；`hub_of` 再看这台设备用不用得了那台。各台的状态文件
//! 分开（`HubKind::file`：Team 的前面加 team.<编号>.），盐、设备指纹也各是各的；服务器只看到账号摘要和密文。
//!
//! 原来还有第三种「自己的同步服务器」（`cca hub`、服务端的 `--mode own`），3.5.3 去掉了：自己的几台设备之间同步改成建一个 Team。
//! 一键部署（安装脚本 `hub-installer`、`cca team deploy` 用的那一套）在 hub/deploy.rs，部署到托管平台在 hub/hosted.rs。

use super::cost::cost_json;
use super::keyshare::is_num;
use super::presence::{presence_load, presence_off_reason, PRESENCE_DIR};
use super::Cmd;
use crate::core::model::{is_api, parse_key};
use crate::py::fs::load_quiet;
use crate::py::hash::{from_hex, sha256_hex, to_hex};
use crate::py::obj::{Dict, V};
use crate::py::proc::{Cmd as Proc, In, Out};
use crate::py::{exit, path, text, R};
use crate::{dict, out};
use std::sync::LazyLock;

pub mod deploy;
pub mod hosted;

/// 这个模块的子命令：只剩打出安装脚本（cca team deploy --script、release.sh 的 cca-hub-install.sh）
pub const CMDS: &[Cmd] = &[Cmd::all("hub-installer", deploy::cmd_hub_installer)];

/// Team 的编号（32 个十六进制字符）压成 16 个字节：HubKind 要能随手拷
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TeamTag([u8; 16]);

impl TeamTag {
    pub fn new(tid: &str) -> Option<TeamTag> {
        from_hex(tid).and_then(|b| <[u8; 16]>::try_from(b).ok()).map(TeamTag)
    }

    /// 编号
    pub fn id(self) -> String {
        to_hex(&self.0)
    }
}

/// 同步服务器的几种：维护者的（旧的通道：映射里的 presence 一节）、某个 Team 的（cca team）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HubKind {
    Maintainer,
    Team(TeamTag),
}

impl HubKind {
    /// 接口里的名字（_gui state 的 accounts[].hub、config.hubs[].kind）
    pub fn name(self) -> &'static str {
        match self {
            HubKind::Maintainer => "maintainer",
            HubKind::Team(_) => "team",
        }
    }

    /// 是某个 Team 的服务器时：那个 Team 的编号
    pub fn team(self) -> Option<String> {
        match self {
            HubKind::Team(t) => Some(t.id()),
            _ => None,
        }
    }

    /// 这台服务器的状态文件叫什么：维护者的照旧，Team 的前面加 team.<编号>.（presence/、cost/ 里各一份）
    pub fn file(self, base: &str) -> String {
        match self {
            HubKind::Maintainer => base.to_string(),
            HubKind::Team(t) => format!("team.{}.{base}", t.id()),
        }
    }
}

/// 一台用得了的同步服务器
pub struct Hub {
    pub kind: HubKind,
    /// presence_post 用的配置：{url, pins, salt, key（私钥的路径）, fp（这台设备在它上面的指纹）}；Team 的还有 "hub": "team"、
    /// "name"（presence_why、is_maintainer 照它说原因）
    pub cfg: Dict,
}

impl Hub {
    pub fn file(&self, base: &str) -> String {
        self.kind.file(base)
    }

    pub fn salt(&self) -> String {
        self.cfg.at("salt").py_str()
    }
}

/// 这台设备现在用得了的同步服务器：维护者的，再是各个 Team 的（照加入的先后；config.json 里写着 "presence": false 时一台都没有）
pub fn hubs() -> R<Vec<Hub>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(Hub { kind: HubKind::Maintainer, cfg });
    }
    let teams = super::team::hub_cfgs();
    if !teams.is_empty() && presence_off_reason().is_none() {
        for (tid, cfg) in teams {
            if let Some(tag) = TeamTag::new(&tid) {
                out.push(Hub { kind: HubKind::Team(tag), cfg });
            }
        }
    }
    Ok(out)
}

/// 这个账号（src 是它那个工具的 policy_sources：映射里的账号各是谁定的）的设备统计、记下的 key、折合金额该走哪一类服务器，不看那台
/// 配置了没有：某个 Team 给的走那个 Team 的；维护者映射里的走维护者的（它的数据是共享这个账号的各台设备的）；别的——这台设备自己的
/// 官方账号（`own_account`）、第三方 API 账号、没有邮箱的——哪都不去
pub fn route(src: &Dict, key: &str, v: &V) -> Option<HubKind> {
    let e = parse_key(key).1.to_lowercase();
    if e.is_empty() || is_api(v) {
        return None;
    }
    match src.str(&e) {
        None => None,
        Some(who) if who.is_empty() => Some(HubKind::Maintainer),
        Some(tid) => TeamTag::new(&tid).map(HubKind::Team),
    }
}

/// 这台设备自己的官方账号吗：有邮箱、不是第三方 API 的，也不是哪个 Team 给的、维护者映射里的（route 给 None 的里面分出来的那一种）
pub fn own_account(src: &Dict, key: &str, v: &V) -> bool {
    let e = parse_key(key).1.to_lowercase();
    !e.is_empty() && !is_api(v) && !src.has(&e)
}

/// 这个账号是映射 pm（{邮箱: 节点}）里的官方账号吗：节点被它固定的就是它们（pm 是 policy_map 时）；关掉共享时收起来的是维护者
/// 映射里的那些（pm 是 legacy_raw 时）
pub fn mapped(pm: &Dict, key: &str, v: &V) -> bool {
    let e = parse_key(key).1.to_lowercase();
    !e.is_empty() && !is_api(v) && pm.has(&e)
}

/// 这个账号现在经哪台服务器同步：route 给的那一类，而且这台设备用得了它（hubs 是 hubs() 的结果）
pub fn hub_of<'a>(hubs: &'a [Hub], src: &Dict, key: &str, v: &V) -> Option<&'a Hub> {
    let kind = route(src, key, v)?;
    hubs.iter().find(|h| h.kind == kind)
}

/// 128 位的小写十六进制：Team 的邀请里那串随机数、加入用的令牌，服务器的这一份记录的编号（gen）都是这个样子
static ADMIT_RE: LazyLock<crate::py::re::Re> = LazyLock::new(|| crate::py::re::Re::new(r"^[0-9a-f]{32}$"));

pub fn hex32(s: &str) -> bool {
    ADMIT_RE.fullmatch(s).is_some()
}

/// 链接（Team 的邀请）最后的校验：前面那一段的 SHA-256 的前 4 位十六进制（看出复制时少了一截、抄错了）
pub fn link_sum(head: &str) -> String {
    sha256_hex(head)[..4].to_string()
}

/// 这台设备的主机名（第一段）；没有时是 Mac
pub fn host_name() -> String {
    // SAFETY: uts 是清零的可写内存；uname 成功时各项是以 0 结尾的字符串
    let node = unsafe {
        let mut uts: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut uts) == 0 {
            std::ffi::CStr::from_ptr(uts.nodename.as_ptr()).to_string_lossy().into_owned()
        } else {
            String::new()
        }
    };
    let short = node.split('.').next().unwrap_or("").to_string();
    if short.is_empty() {
        "Mac".to_string()
    } else {
        short
    }
}

/// 地址里给人看的那段（地址:端口）
pub fn host_of(url: &str) -> String {
    url.split_once("://").map_or(url, |x| x.1).trim_end_matches('/').to_string()
}

/// 设备的名字：1～64 个字符，没有控制字符，前后没有空白（服务器也这么查）；不合格时说 NAME_BAD
pub fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.trim() == n && text::len(n) <= 64 && !n.chars().any(|c| c.is_control())
}
pub const NAME_BAD: &str = "设备的名字要 1～64 个字符，不能有控制字符";

/// 生成一把 ed25519 密钥（没有口令；comment 是公钥文件里的备注）；旧的（坏了的）先删掉。Team 里设备的密钥和 Team 的根密钥都是它
/// 生成的
pub fn keygen(p: &str, comment: &str) -> R<()> {
    for f in [p.to_string(), format!("{p}.pub")] {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    let _ = Proc::new(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", comment, "-f", p])
        .stdin(In::Null)
        .stdout(Out::Null)
        .stderr(Out::Null)
        .timeout(30.0)
        .run();
    crate::py::signal::check()
}

// --- 一台服务器的样子（_gui state 的 config.hubs[]）-----------------------------------------------------------------

/// 只读本机记着的（三条管线最近一次的结果），不联网：state 是 ok（最近都成了）| error（最近有一条没成，error 是原因）|
/// pending（还没同步过）；checked_epoch 是最近一次成了的时候；accounts 是经它同步的账号数
pub fn hub_view(kind: HubKind, url: &str, accounts: usize) -> Dict {
    let read = |dir: &str, base: &str| load_quiet(&path::join(dir, &kind.file(base))).and_then(|j| j.as_dict()).unwrap_or_default();
    let num = |j: &Dict, k: &str| j.get(k).filter(is_num).and_then(|v| v.as_num());
    let mut errors: Vec<(f64, String)> = Vec::new();
    let mut oks: Vec<f64> = Vec::new();
    // 设备统计：{"error": 没报成的原因（报成了是空的）, "at": 变了的时候}
    let pres = read(&PRESENCE_DIR, "status.json");
    if let Some(at) = num(&pres, "at") {
        match pres.str("error").filter(|e| !e.is_empty()) {
            Some(e) => errors.push((at, e.to_string())),
            None => oks.push(at),
        }
    }
    // 记下的 API key：{"synced": 上次同步, "error": 没同步成的原因}
    let keys = read(&PRESENCE_DIR, "keys.json");
    if let Some(at) = num(&keys, "synced") {
        match keys.str("error").filter(|e| !e.is_empty()) {
            Some(e) => errors.push((at, e.to_string())),
            None => oks.push(at),
        }
    }
    // 折合金额：{"at": 上次同步, "ok_at": 上次成了, "state", "error"}
    let cost = cost_json(&kind.file("sync.json"));
    if let Some(ok) = num(&cost, "ok_at") {
        oks.push(ok);
    }
    if matches!(cost.str("state").as_deref(), Some("error") | Some("old_server")) {
        if let (Some(at), Some(e)) = (num(&cost, "at"), cost.str("error").filter(|e| !e.is_empty())) {
            errors.push((at, e.to_string()));
        }
    }
    let last_ok = oks.iter().copied().reduce(f64::max);
    let last_err = errors.iter().max_by(|a, b| a.0.total_cmp(&b.0));
    let (state, error) = match last_err {
        Some((_, e)) => ("error", V::from(e.as_str())),
        None if last_ok.is_some() => ("ok", V::None),
        None => ("pending", V::None),
    };
    // 服务器上这台设备的名字：上次同步折合金额时服务器说的
    let name = cost.get("me").filter(|m| m.is_str());
    dict! {
        "kind" => kind.name(), "url" => url, "host" => host_of(url), "state" => state, "error" => error,
        "checked_epoch" => last_ok, "accounts" => accounts, "name" => name.unwrap_or(V::None),
        // 原来只有自己的同步服务器才有值的几项：接口只加不删（PROTOCOL 2.2），照旧给空的
        "members" => V::None, "joined_epoch" => V::None, "can_invite" => false, "pinned" => V::None,
    }
}

/// _gui state 的 config.hubs（PROTOCOL 2.2）：这台设备用得了的维护者的服务器，各个 Team 的服务器
pub fn hub_entries(count: impl Fn(HubKind) -> usize) -> R<Vec<V>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(V::Dict(hub_view(HubKind::Maintainer, &cfg.at("url").py_str(), count(HubKind::Maintainer))));
    }
    // 各个 Team 的：多 "team"（编号）、"team_name"（Team 的名字）
    let teams = super::team::hub_cfgs();
    if !teams.is_empty() && presence_off_reason().is_none() {
        for (tid, cfg) in teams {
            let Some(kind) = TeamTag::new(&tid).map(HubKind::Team) else { continue };
            let v = hub_view(kind, &cfg.at("url").py_str(), count(kind));
            v.set("team", tid.as_str());
            v.set("team_name", cfg.at("name"));
            out.push(V::Dict(v));
        }
    }
    Ok(out)
}

// --- 失败 ------------------------------------------------------------------------------------------------------------------

/// 没成：stdout 是 "原因码\t给人看的原因"（bash 的 FAIL_CODE 是 hub_ 加上原因码；一键部署用），退出码 1
fn fail<T>(code: &str, why: &str) -> R<T> {
    out!("{code}\t{why}");
    exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table() {
        let tid = "0a".repeat(16);
        let src = dict! { "alice@example.com" => "", "carol@example.com" => tid.as_str() };
        let api = V::Dict(dict! { "base_url" => "https://api.example.com", "model" => "x" });
        assert_eq!(
            route(&src, "alice(Alice@Example.com)", &V::from("US-A")),
            Some(HubKind::Maintainer),
            "维护者映射里的（邮箱不分大小写）"
        );
        assert_eq!(route(&src, "bob(bob@example.com)", &V::from("US-B")), None, "自己的：哪都不去");
        let team = route(&src, "carol@example.com", &V::from("US-A"));
        assert_eq!(team, Some(HubKind::Team(TeamTag::new(&tid).expect("32 个十六进制字符"))), "Team 给的");
        assert_eq!(team.map(|k| (k.name(), k.file("keys.json"))), Some(("team", format!("team.{tid}.keys.json"))));
        assert_eq!(route(&Dict::new(), "alice(alice@example.com)", &V::from("US-A")), None, "共享关着：都是自己的");
        assert_eq!(route(&src, "relay(alice@example.com)", &api), None, "第三方 API 账号");
        assert_eq!(route(&src, "noemail", &V::from("US-A")), None, "没有邮箱");
        assert!(own_account(&src, "bob(bob@example.com)", &V::from("US-B")), "自己的官方账号");
        assert!(
            !own_account(&src, "alice(alice@example.com)", &V::from("US-A"))
                && !own_account(&src, "carol@example.com", &V::from("US-A"))
        );
        assert!(!own_account(&src, "relay(bob@example.com)", &api) && !own_account(&src, "noemail", &V::from("US-A")));
        let pm = dict! { "alice@example.com" => "US-A" };
        assert!(mapped(&pm, "alice(alice@example.com)", &V::from("US-A")) && !mapped(&pm, "alice(alice@example.com)", &api));
        assert!(!mapped(&pm, "bob@example.com", &V::from("US-A")) && !mapped(&pm, "noemail", &V::from("US-A")));
    }

    #[test]
    fn files_per_kind() {
        assert_eq!(HubKind::Maintainer.file("sync.json"), "sync.json");
        assert_eq!(HubKind::Maintainer.name(), "maintainer");
    }

    #[test]
    fn names() {
        assert!(name_ok("studio-mac") && name_ok("家里的 Mac") && !name_ok("") && !name_ok(" a") && !name_ok("a\tb"));
        assert!(!name_ok(&"x".repeat(65)) && name_ok(&"中".repeat(64)));
    }
}
