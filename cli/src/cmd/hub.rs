//! 同步服务器（hub）：设备统计、记下备查的 API key、折合金额经哪台服务器和这个账号的其他设备同步。只有一种：某个 Team 的
//! （`HubKind::Team`），Team 给这台设备的账号走它（docs/dev/team.md）。
//!
//! 账号走哪台只看 `route`（这台设备自己的官方账号、第三方 API 账号哪都不去）；`hub_of` 再看这台设备用不用得了那台。各台的状态文件
//! 分开（`HubKind::file`：前面加 team.<编号>.），盐、设备指纹也各是各的；服务器只看到账号摘要和密文。
//!
//! 原来还有维护者的服务器（映射里的 presence 一节，拿映射的 deploy key 签名；3.6.0 和映射一起去掉了）和「自己的同步服务器」
//! （`cca hub`、服务端的 `--mode own`，3.5.3 去掉了：自己的几台设备之间同步改成建一个 Team）。
//! 一键部署（安装脚本 `hub-installer`、`cca team deploy` 用的那一套）在 hub/deploy.rs，部署到托管平台在 hub/hosted.rs。

use super::cost::cost_json;
use super::keyshare::is_num;
use super::presence::{presence_off_reason, PRESENCE_DIR};
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
pub mod payload;

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

/// 同步服务器是哪一台：某个 Team 的（cca team）。原来还有维护者的那一台（3.6.0 去掉了）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HubKind {
    Team(TeamTag),
}

impl HubKind {
    /// 接口里的名字（_gui state 的 accounts[].hub、config.hubs[].kind）：只有 "team"
    pub fn name(self) -> &'static str {
        "team"
    }

    /// 那个 Team 的编号
    pub fn team(self) -> String {
        let HubKind::Team(t) = self;
        t.id()
    }

    /// 这台服务器的状态文件叫什么：前面加 team.<编号>.（presence/、cost/ 里各一份；没有前缀的是以前维护者的那一台的）
    pub fn file(self, base: &str) -> String {
        format!("team.{}.{base}", self.team())
    }
}

/// 一台用得了的同步服务器
pub struct Hub {
    pub kind: HubKind,
    /// presence_post 用的配置：{url, pins, salt, key（私钥的路径）, fp（这台设备在它上面的指纹）, "hub": "team",
    /// "name"（Team 的名字：presence_why 照它说原因）}
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

/// 这台设备现在用得了的同步服务器：各个 Team 的（照加入的先后；config.json 里写着 "presence": false 时一台都没有）
pub fn hubs() -> R<Vec<Hub>> {
    let mut out = Vec::new();
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

/// 这个账号（src 是它那个工具的 policy_sources：映射里的账号各是哪个 Team 给的）的设备统计、记下的 key、折合金额该走哪台服务器，
/// 不看那台配置了没有：某个 Team 给的走那个 Team 的；别的——这台设备自己的官方账号（`own_account`）、第三方 API 账号、没有邮箱的——
/// 哪都不去
pub fn route(src: &Dict, key: &str, v: &V) -> Option<HubKind> {
    let e = parse_key(key).1.to_lowercase();
    if e.is_empty() || is_api(v) {
        return None;
    }
    src.str(&e).and_then(|tid| TeamTag::new(&tid)).map(HubKind::Team)
}

/// 这台设备自己的官方账号吗：有邮箱、不是第三方 API 的，也不是哪个 Team 给的（route 给 None 的里面分出来的那一种）
pub fn own_account(src: &Dict, key: &str, v: &V) -> bool {
    let e = parse_key(key).1.to_lowercase();
    !e.is_empty() && !is_api(v) && !src.has(&e)
}

/// 这个账号是映射 pm（{邮箱: 节点}，policy_map）里的官方账号吗：节点被它固定的就是它们
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

/// https://地址 没写端口时补上 443：托管平台（Railway）给的公网地址只到主机名。写了端口的、带路径的原样（url_ok 再查）
pub fn with_https_port(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let Some(rest) = url.strip_prefix("https://") else { return url.to_string() };
    let has_port = if rest.starts_with('[') { rest.contains("]:") } else { rest.contains(':') };
    if has_port || rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        url.to_string()
    } else {
        format!("{url}:443")
    }
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
    if cost.str("state").as_deref() == Some("error") {
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

/// _gui state 的 config.hubs（PROTOCOL 2.2）：这台设备用得了的各个 Team 的服务器
pub fn hub_entries(count: impl Fn(HubKind) -> usize) -> R<Vec<V>> {
    let mut out = Vec::new();
    // 比 hub_view 多 "team"（编号）、"team_name"（Team 的名字）
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
    fn https_port_defaults_to_443() {
        assert_eq!(with_https_port("https://cca-team.up.railway.app/"), "https://cca-team.up.railway.app:443");
        assert_eq!(with_https_port("https://192.0.2.10:18445"), "https://192.0.2.10:18445");
        assert_eq!(with_https_port("https://[2001:db8::1]"), "https://[2001:db8::1]:443");
        assert_eq!(with_https_port("https://[2001:db8::1]:8443"), "https://[2001:db8::1]:8443");
        for keep in ["http://example.com", "https://example.com/x", "https://example.com?x", "example.com"] {
            assert_eq!(with_https_port(keep), keep, "不是 https、带路径的原样：url_ok 再查");
        }
    }

    #[test]
    fn route_table() {
        let (tid, other) = ("0a".repeat(16), "0b".repeat(16));
        let src = dict! { "alice@example.com" => other.as_str(), "carol@example.com" => tid.as_str(), "dave@example.com" => "x" };
        let api = V::Dict(dict! { "base_url" => "https://api.example.com", "model" => "x" });
        let tag = |t: &str| HubKind::Team(TeamTag::new(t).expect("32 个十六进制字符"));
        assert_eq!(route(&src, "alice(Alice@Example.com)", &V::from("US-A")), Some(tag(&other)), "邮箱不分大小写");
        assert_eq!(route(&src, "bob(bob@example.com)", &V::from("US-B")), None, "自己的：哪都不去");
        let team = route(&src, "carol@example.com", &V::from("US-A"));
        assert_eq!(team, Some(tag(&tid)), "Team 给的");
        assert_eq!(
            team.map(|k| (k.name(), k.team(), k.file("keys.json"))),
            Some(("team", tid.clone(), format!("team.{tid}.keys.json")))
        );
        assert_eq!(route(&src, "dave@example.com", &V::from("US-A")), None, "编号不对：哪都不去");
        assert_eq!(route(&Dict::new(), "alice(alice@example.com)", &V::from("US-A")), None, "不在 Team 里：都是自己的");
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
    fn names() {
        assert!(name_ok("studio-mac") && name_ok("家里的 Mac") && !name_ok("") && !name_ok(" a") && !name_ok("a\tb"));
        assert!(!name_ok(&"x".repeat(65)) && name_ok(&"中".repeat(64)));
    }
}
