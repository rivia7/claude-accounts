//! 同步服务器（hub）：设备统计、记下备查的 API key、折合金额经哪台服务器和这个账号的其他设备同步。两种：
//! - 维护者的（`HubKind::Maintainer`）：映射里的 presence 一节，拿映射的 deploy key 签名（`presence_load`）。只管维护者映射里的官方账号
//! - 自己的（`HubKind::Own`，界面上叫「自己的同步服务器」，命令是 `cca hub`）：用户自己部署、`cca hub join` 加入的，配置在 ~/.cca/hub/。
//!   管别的官方账号
//!
//! 账号走哪台只看 `route`（第三方 API 账号哪都不去）；`hub_of` 再看这台设备用不用得了那台。两台的状态文件分开（`HubKind::file`：
//! 自己的前面加 own.），盐、设备指纹也各是各的；服务器只看到账号摘要和密文。
//!
//! 加入链接：`cca-hub1.<base64url(JSON)>.<校验>`（格式见 server/README.md「自己的同步服务器」），里面有服务器的地址、证书公钥的指纹
//! （没有的是托管平台、反向代理后面的：照证书机构验证书）、入伙口令（admit）和盐。它是凭证：只经 stdin、提问会话传，不进命令行和
//! 环境变量。服务器只存口令的 SHA-256 和盐的校验值（`hub_check`，第一台设备加入时登记：拿着另一份盐的设备加入不了，免得同一台
//! 服务器上的设备互相解不开）。
//!
//! 换加入链接（`relink`：移除别的设备以后、`cca hub invite --new`）：新的口令在这台设备上生成，服务器只拿到它的哈希和一份拿盐加密的
//! 新口令（`link_seal`），别的成员问设备时（`members`）拿到、解开、换上。已经加入的设备照旧，旧的链接不能再用来加入。
//! 服务器不认这台设备（403 unknown_device）时看它带的这一份记录的编号（gen）：和加入时记下的一样是被移除了（本机记着的删掉），
//! 不一样是服务器的记录没了，拿记着的口令自己重新加入（`own_recover`）。
//!
//! 子命令：hub-join（链接从 stdin 读）、hub-status、hub-devices、hub-remove、hub-leave、hub-invite，一键部署的 hub-installer、hub-deploy
//! 在 hub/deploy.rs，部署到托管平台的 hub-hosted 在 hub/hosted.rs（bash 的 `cca hub` 调它们）。

use super::cost::{cost_json, cost_kick_now, COST_DIR};
use super::keyshare::{blob_ad, blob_open, blob_seal, is_num, keyshare_kick, Keys};
use super::presence::{
    agent_kick, flock, key_at, presence_load, presence_off_reason, presence_post, presence_targets, presence_why, url_ok, FP_RE,
    PRESENCE_DIR, PRESENCE_PIN_RE, PRESENCE_SALT_RE,
};
use super::router::device_id;
use super::{arity, Cmd};
use crate::core::base::{Lock, CCA_HOME};
use crate::core::ctx::cfg_save;
use crate::core::model::{cfg_load, is_api, parse_key};
use crate::py::base64::{urlsafe_b64decode, urlsafe_b64encode};
use crate::py::fs::{self, load_quiet, save_json};
use crate::py::hash::{from_hex, hmac_sha256, sha256_hex, to_hex};
use crate::py::io::read_stdin_text;
use crate::py::obj::{Dict, List, V};
use crate::py::proc::{Cmd as Proc, In, Out};
use crate::py::rand::token_hex;
use crate::py::time::time;
use crate::py::{exit, json, path, text, Fail, R};
use crate::{dict, out};
use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

mod deploy;
mod hosted;

/// 这个模块的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("hub-installer", deploy::cmd_hub_installer),
    Cmd::all("hub-deploy", deploy::cmd_hub_deploy),
    Cmd::all("hub-hosted", hosted::cmd_hub_hosted),
    Cmd::all("hub-join", cmd_hub_join),
    Cmd::all("hub-status", cmd_hub_status),
    Cmd::all("hub-devices", cmd_hub_devices),
    Cmd::all("hub-remove", cmd_hub_remove),
    Cmd::all("hub-leave", cmd_hub_leave),
    Cmd::all("hub-invite", cmd_hub_invite),
];

/// ~/.cca/hub：hub.json（加入的是哪台、入伙口令、盐；600）、id_ed25519（这台设备在它上面的密钥）、members.json（上次问到几台设备）、
/// recover.lock。退出、被移除、加入没成时整个目录会删掉，所以要留下来的不放这里：被移除了的说明（~/.cca/hub-removed.json）、
/// 部署到托管平台还没加入时的口令和盐（~/.cca/hub-hosted.json，hosted.rs）
pub static HUB_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "hub"));

fn hub_path(name: &str) -> String {
    path::join(&HUB_DIR, name)
}

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

/// 同步服务器的几种：维护者的（旧的通道：映射里的 presence 一节）、自己的（cca hub）、某个 Team 的（cca team）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HubKind {
    Maintainer,
    Own,
    Team(TeamTag),
}

impl HubKind {
    /// 接口里的名字（_gui state 的 accounts[].hub、config.hubs[].kind）
    pub fn name(self) -> &'static str {
        match self {
            HubKind::Maintainer => "maintainer",
            HubKind::Own => "own",
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

    /// 这台服务器的状态文件叫什么：维护者的照旧，自己的前面加 own.，Team 的前面加 team.<编号>.（presence/、cost/ 里各一份）
    pub fn file(self, base: &str) -> String {
        match self {
            HubKind::Maintainer => base.to_string(),
            HubKind::Own => format!("own.{base}"),
            HubKind::Team(t) => format!("team.{}.{base}", t.id()),
        }
    }
}

/// 一台用得了的同步服务器
pub struct Hub {
    pub kind: HubKind,
    /// presence_post 用的配置：{url, pins, salt, key（私钥的路径）, fp（这台设备在它上面的指纹）}；自己的还有 "hub": "own"
    /// （presence_why 照它说原因）
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

/// 这台设备现在用得了的同步服务器：维护者的、自己的，再是各个 Team 的（照加入的先后；config.json 里写着 "presence": false 时一台都没有）
pub fn hubs() -> R<Vec<Hub>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(Hub { kind: HubKind::Maintainer, cfg });
    }
    if let Some(cfg) = own_load()?.1 {
        out.push(Hub { kind: HubKind::Own, cfg });
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
/// 配置了没有：第三方 API 账号、没有邮箱的哪都不去；某个 Team 给的走那个 Team 的；维护者映射里的走维护者的（它的数据是共享这个账号的
/// 各台设备的，不上自己的服务器）；别的官方账号走自己的
pub fn route(src: &Dict, key: &str, v: &V) -> Option<HubKind> {
    let e = parse_key(key).1.to_lowercase();
    if e.is_empty() || is_api(v) {
        return None;
    }
    Some(match src.str(&e) {
        None => HubKind::Own,
        Some(who) if who.is_empty() => HubKind::Maintainer,
        Some(tid) => HubKind::Team(TeamTag::new(&tid)?),
    })
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

// --- 加入链接 --------------------------------------------------------------------------------------------------------------

/// 加入链接的开头（格式不兼容时升到 cca-hub2.；只加字段不用升）
pub const LINK_PREFIX: &str = "cca-hub1.";

/// 加入链接里的：服务器的地址（https://地址:端口）、证书公钥的指纹（sha256//…，最多 4 个；没有是照证书机构验证书）、入伙口令、
/// 盐（都是小写十六进制）
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub url: String,
    pub pins: Vec<String>,
    pub admit: String,
    pub salt: String,
}

/// 入伙口令：128 位的小写十六进制（服务器只存它的 SHA-256）；服务器的这一份记录的编号（gen）也是这个样子
static ADMIT_RE: LazyLock<crate::py::re::Re> = LazyLock::new(|| crate::py::re::Re::new(r"^[0-9a-f]{32}$"));

pub fn hex32(s: &str) -> bool {
    ADMIT_RE.fullmatch(s).is_some()
}

/// 这几项合不合格；不合格是给人看的原因
fn link_problem(url: &str, pins: &[String], admit: &str, salt: &str) -> Option<&'static str> {
    if !url_ok(url) {
        Some("服务器的地址不对（应该是 https://地址:端口）")
    } else if pins.len() > 4 || !pins.iter().all(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()) {
        Some("服务器证书的指纹不对")
    } else if ADMIT_RE.fullmatch(admit).is_none() {
        Some("入伙口令不对")
    } else if PRESENCE_SALT_RE.fullmatch(salt).is_none() {
        Some("盐不对")
    } else {
        None
    }
}

/// 链接最后的校验：前面那一段的 SHA-256 的前 4 位十六进制（看出复制时少了一截、抄错了）
pub fn link_sum(head: &str) -> String {
    sha256_hex(head)[..4].to_string()
}

pub fn link_encode(l: &Link) -> String {
    let doc = dict! {
        "u" => l.url.as_str(), "p" => l.pins.iter().map(|p| V::from(p.as_str())).collect::<Vec<V>>(), "a" => l.admit.as_str(),
        "s" => l.salt.as_str(),
    };
    let raw = json::dumps(&V::Dict(doc), json::Fmt::new().compact());
    let head = format!("{LINK_PREFIX}{}", urlsafe_b64encode(raw.as_bytes()).trim_end_matches('='));
    let sum = link_sum(&head);
    format!("{head}.{sum}")
}

/// 读加入链接：中间的空白不算（聊天软件里贴过来可能折了行）；不对是给人看的原因
pub fn link_decode(raw: &str) -> Result<Link, String> {
    let s: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Err("没有输入加入链接".into());
    }
    let Some(rest) = s.strip_prefix(LINK_PREFIX) else {
        if s.starts_with("cca-hub") {
            return Err("这个加入链接是更新版本的 cca 生成的：先更新 cca（cca update）再加入".into());
        }
        return Err("这不是加入链接（加入链接以 cca-hub1. 开头）".into());
    };
    let broken = || "加入链接不完整或者抄错了（复制时少了一截？）：重新复制一次".to_string();
    let Some((body, sum)) = rest.split_once('.') else { return Err(broken()) };
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(broken());
    }
    if link_sum(&format!("{LINK_PREFIX}{body}")) != sum {
        return Err(broken());
    }
    let pad = "=".repeat((4 - body.len() % 4) % 4);
    let doc = urlsafe_b64decode(&format!("{body}{pad}"))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| json::loads(&t).ok())
        .and_then(|j| j.as_dict())
        .ok_or_else(broken)?;
    let s = |k: &str| doc.str(k).map(|v| v.to_string()).unwrap_or_default();
    let pins: Vec<String> = doc.list("p").map(|p| p.items().iter().map(|x| x.py_str()).collect()).unwrap_or_default();
    let l = Link { url: s("u").trim_end_matches('/').to_string(), pins, admit: s("a"), salt: s("s") };
    if let Some(why) = link_problem(&l.url, &l.pins, &l.admit, &l.salt) {
        return Err(format!("加入链接里{why}：向已经加入的设备要一个新的（cca hub invite）"));
    }
    Ok(l)
}

/// 盐的校验值（服务器登记、核对，看不出盐）：HMAC(盐, "cca-hub-check-v1") 的前 16 个字节，小写十六进制
pub fn hub_check(salt: &str) -> String {
    to_hex(&hmac_sha256(&from_hex(salt).unwrap_or_default(), b"cca-hub-check-v1"))[..32].to_string()
}

// --- 本机记着的（~/.cca/hub/）------------------------------------------------------------------------------------------------

/// hub.json：{"v": 1, "url", "pins", "admit": 现在的入伙口令, "salt", "name": 服务器上这台设备的名字, "joined_at": 第一次加入的时间,
/// "gen": 服务器上这一份记录的编号, "seed": 部署时配置的那个口令（服务器的记录没了以后认的是它；不知道时没有）,
/// "link": 服务器上换过几次口令（问设备时比，大了就换上新的）, "stale": 拿记着的口令重新加入、服务器不认的那一份记录的编号,
/// "hosted": 托管平台（railway | sealos：这台设备部署到托管平台的那一台；重新加入同一台时照旧）}；
/// 没有、读不了、前面几项哪一项不合格都当作没加入（后面几项不合格的当作没有）
fn hub_doc() -> Option<Dict> {
    let j = load_quiet(&hub_path("hub.json"))?.as_dict()?;
    let s = |k: &str| j.str(k).map(|v| v.to_string()).unwrap_or_default();
    let pins: Vec<String> = j.list("pins")?.items().iter().map(|p| p.as_str().map(String::from)).collect::<Option<_>>()?;
    let ok = j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && link_problem(&s("url"), &pins, &s("admit"), &s("salt")).is_none();
    ok.then_some(j)
}

/// hub.json 里的一项十六进制（gen、seed、stale）：不合格的当作没有
fn doc_hex(j: &Dict, k: &str) -> Option<String> {
    j.str(k).filter(|v| hex32(v)).map(|v| v.to_string())
}

fn save_doc(j: &Dict) -> R<()> {
    save_json(&hub_path("hub.json"), &V::Dict(j.copy()), Some(0o600), true)
}

fn link_of(j: &Dict) -> Link {
    Link {
        url: j.at("url").py_str(),
        pins: j.list("pins").map(|p| p.items().iter().map(|x| x.py_str()).collect()).unwrap_or_default(),
        admit: j.at("admit").py_str(),
        salt: j.at("salt").py_str(),
    }
}

/// 自己的服务器的配置（presence_load 的样子，多一项 "hub": "own"）
fn own_cfg_of(link: &Link, key: String, fp: String) -> Dict {
    let pins = List::from_vec(link.pins.iter().map(|p| V::from(p.as_str())).collect());
    dict! { "url" => link.url.as_str(), "pins" => pins, "salt" => link.salt.as_str(), "key" => key, "fp" => fp, "hub" => "own" }
}

/// 自己的服务器的配置，不看这台设备关没关同步（退出、看有哪些设备、移除设备也要找服务器）。
/// "rev" 是 hub.json 的修改时间：重新加入（换名字、被移除后再加入）以后上报进程据此认出是新的一次加入，从头报、不接着退避
pub fn own_cfg() -> R<(String, Option<Dict>)> {
    let Some(j) = hub_doc() else {
        return Ok((removed_note().unwrap_or_else(|| "还没加入自己的同步服务器（有加入链接时运行 cca hub join）".into()), None));
    };
    let Some((key, fp)) = key_at(&hub_path("id_ed25519"))? else {
        return Ok(("这台设备在自己的同步服务器上的密钥不见了（~/.cca/hub/id_ed25519）：重新运行 cca hub join".into(), None));
    };
    let cfg = own_cfg_of(&link_of(&j), key, fp);
    if let Some(rev) = hub_rev() {
        cfg.set("rev", rev);
    }
    Ok((String::new(), Some(cfg)))
}

/// hub.json 的修改时间（own_cfg 的 "rev"）
fn hub_rev() -> Option<f64> {
    std::fs::metadata(hub_path("hub.json")).ok().map(|st| st.mtime() as f64 + st.mtime_nsec() as f64 / 1e9)
}

/// (用不了的原因, 配置)：能用时原因是空的，配置的样子和 presence_load 的一样（多一项 "hub": "own"）
pub fn own_load() -> R<(String, Option<Dict>)> {
    if let Some(why) = presence_off_reason() {
        return Ok((why, None));
    }
    own_cfg()
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

/// 这台机器的 device_id（config.json；折合金额的密文里写着它，同一台机器在两台服务器上算一台设备）：没有就生成、写回去（拿着 Lock）
pub fn ensure_device_id() -> R<String> {
    let _lk = Lock::acquire()?;
    let c = cfg_load()?;
    let had = c.truthy("device_id");
    let id = device_id(&c).py_str();
    if !had {
        cfg_save(&c)?;
    }
    Ok(id)
}

/// 生成一把 ed25519 密钥（没有口令；comment 是公钥文件里的备注）；旧的（坏了的）先删掉。自己的同步服务器、Team 里设备的密钥和
/// Team 的根密钥都是它生成的
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

/// 上次问到服务器上有几台设备（hub-join、hub-devices、hub-remove 记下；_gui state、cca hub status 只读）。只在变了时写：
/// App 盯着这个文件，写一次就重读一次 state
fn members_note(n: Option<i64>) {
    if let Some(n) = n.filter(|n| members_known() != Some(*n)) {
        let _ = save_json(&hub_path("members.json"), &V::Dict(dict! { "v" => 1, "n" => n }), Some(0o600), true);
    }
}

fn members_known() -> Option<i64> {
    load_quiet(&hub_path("members.json"))?.as_dict()?.get("n")?.as_int()
}

/// 退出以后：本机记着的都删掉（~/.cca/hub/，presence/ 和 cost/ 里 own. 开头的）
fn own_forget() -> R<()> {
    if path::isdir(&HUB_DIR) {
        fs::rmtree(&HUB_DIR)?;
    }
    for dir in [&*PRESENCE_DIR, &*COST_DIR] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("own.") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

/// 被移除了的说明：~/.cca/hub-removed.json（{"v": 1, "url", "name": 移除前的名字, "at"}，600），再加入成了删掉
fn removed_path() -> String {
    path::join(&CCA_HOME, "hub-removed.json")
}

/// 被别的设备移除了（own_recover 发现的）：本机记着的和退出时一样删掉，留下哪台服务器、什么时候发现的给人看
fn own_removed(doc: &Dict) -> R<()> {
    own_forget()?;
    let note = dict! { "v" => 1, "url" => doc.at("url"), "name" => doc.at("name"), "at" => time().trunc() as i64 };
    save_json(&removed_path(), &V::Dict(note), Some(0o600), true)
}

/// 被移除了的说明（没有是 None）
pub fn removed_note() -> Option<String> {
    let j = load_quiet(&removed_path())?.as_dict()?;
    let host = host_of(&j.str("url")?);
    Some(format!(
        "这台设备被别的设备从 {host} 上的自己的同步服务器移除了：要再加入，向已经加入的设备要加入链接（cca hub invite），在这台设备上运行 cca hub join"
    ))
}

// --- 换加入链接（relink）、服务器的记录没了时重新加入 -------------------------------------------------------------------------------

/// 换口令时存在服务器上的那份密文的标签（认证里也带着它）
const LINK_LABEL: &str = "cca-hub-link-v1";

/// 那份密文的两把密钥（加密、认证）：HMAC(盐, 标签) 再分别 HMAC "enc"、"mac"。只有拿着加入链接的设备算得出来，服务器解不开
fn link_keys(salt: &str) -> Keys {
    let root = hmac_sha256(&from_hex(salt).unwrap_or_default(), LINK_LABEL.as_bytes());
    (hmac_sha256(&root, b"enc"), hmac_sha256(&root, b"mac"))
}

/// 新的口令（和部署时配置的那个，知道的话）→ 服务器存的密文（blob_seal）：明文是 {"a": 口令, "s": 部署时的口令 | null} 的紧凑 JSON
fn link_seal(salt: &str, admit: &str, seed: Option<&str>) -> R<Dict> {
    let pt = json::dumps(&V::Dict(dict! { "a" => admit, "s" => seed.map_or(V::None, V::from) }), json::Fmt::new().compact());
    Ok(blob_seal(&link_keys(salt), &blob_ad(&[LINK_LABEL])?, pt.as_bytes()))
}

/// link_seal 反过来：(口令, 部署时的口令)；认证不过、格式不对是 None
fn link_open(salt: &str, blob: &V) -> Option<(String, Option<String>)> {
    let pt = blob_open(&link_keys(salt), &blob_ad(&[LINK_LABEL]).ok()?, blob)?;
    let j = json::loads(std::str::from_utf8(&pt).ok()?).ok()?.as_dict()?;
    let admit = j.str("a").filter(|a| hex32(a))?.to_string();
    Some((admit, doc_hex(&j, "s")))
}

/// 问设备时服务器回的这一份记录的编号、换过的口令（换的那台设备拿盐加密的）：比本机记着的新就换上，这台设备的加入链接跟着换。只在变了时写
fn link_refresh(j: &Dict) -> R<()> {
    let Some(doc) = hub_doc() else { return Ok(()) };
    let mut changed = false;
    if let Some(gen) = doc_hex(j, "gen").filter(|g| doc_hex(&doc, "gen").as_ref() != Some(g)) {
        doc.set("gen", gen);
        doc.pop("link"); // 另一份记录：换过几次从头数
        changed = true;
    }
    let had = doc.get("link").and_then(|n| n.as_int()).unwrap_or(0);
    let link = j.get("link").and_then(|l| l.as_dict());
    if let Some((n, l)) = link.as_ref().and_then(|l| Some((l.get("n")?.as_int().filter(|n| *n > had)?, l))) {
        if let Some((admit, seed)) = link_open(&doc.at("salt").py_str(), &l.at("blob")) {
            doc.set("admit", admit);
            if let Some(s) = seed {
                doc.set("seed", s);
            }
            doc.set("link", n);
            changed = true;
        }
    }
    if changed {
        save_doc(&doc)?;
    }
    Ok(())
}

/// 服务器上配置的那个入伙口令（部署时的：seed；更新服务器时照它，不然服务器会从头开始）。没记下的先问一次服务器（换过加入链接的话，
/// 换的那台设备在密文里放了它：link_refresh 换上；和 devices、remove 一样不看关没关设备统计），还是不知道是 None
pub fn configured_admit(doc: &Dict) -> R<Option<String>> {
    if let Some(s) = doc_hex(doc, "seed") {
        return Ok(Some(s));
    }
    if let (_, Some(cfg)) = own_cfg()? {
        let _ = members(&cfg)?; // 连不上就算了
    }
    Ok(hub_doc().and_then(|j| doc_hex(&j, "seed")))
}

/// 一键部署更新了服务器以后、再加入之前：拿 pins（证书换了的话是新的指纹）问一次设备（members 不要口令）。别的设备换过加入链接的话，
/// 记着的口令已经加入不了：顺带换上服务器存的（link_refresh）；服务器的记录没了的话 own_recover 先重新加入。返回问过以后记着的口令
/// （没加入是 None；问不到照旧是记着的）
pub fn refresh_admit(pins: &[String]) -> R<Option<String>> {
    let (_, Some(cfg)) = own_cfg()? else { return Ok(None) };
    cfg.set("pins", List::from_vec(pins.iter().map(|p| V::from(p.as_str())).collect()));
    let _ = members(&cfg)?;
    Ok(hub_doc().map(|d| d.at("admit").py_str()))
}

/// 不知道部署时的口令（configured_admit 是 None）时说的
pub const SEED_UNKNOWN: &str =
    "这台设备不知道部署服务器时配置的入伙口令（它拿换过的加入链接加入，换链接的设备也不知道）：在部署服务器的那台设备上更新";

/// 换加入链接：新的口令在这台设备上生成，服务器只拿到它的哈希和一份拿盐加密的新口令（别的成员问设备时换上）。以后只认新的
/// （旧的链接不能再用来加入），已经加入的设备照旧。成了写进 hub.json（cca hub invite 给的就是新的）；没成是给人看的原因
fn relink(cfg: &Dict) -> R<Result<(), String>> {
    let Some(doc) = hub_doc() else { return Ok(Err("还没加入自己的同步服务器".into())) };
    let admit = token_hex(16);
    let blob = link_seal(&doc.at("salt").py_str(), &admit, doc_hex(&doc, "seed").as_deref())?;
    let (code, j) = presence_post(cfg, "relink", &dict! { "admit" => sha256_hex(&admit), "blob" => blob }, 10)?;
    if code != 200 {
        return Ok(Err(fault(cfg, code, &j)?.1));
    }
    // 发请求时别的进程可能写过（问设备时换上了别的设备更早的那一次）；这时退出了的不再写回去（别的设备问设备时照样换上）
    let Some(doc) = hub_doc() else { return Ok(Err("换链接的时候这台设备退出了自己的同步服务器".into())) };
    doc.set("admit", admit);
    if let Some(n) = j.get("n").and_then(|n| n.as_int()).filter(|n| *n > 0) {
        doc.set("link", n);
    }
    save_doc(&doc)?;
    Ok(Ok(()))
}

/// 自己的服务器不认这台设备了（403 unknown_device，j 是它回的；presence_post 调）：看它带的这一份记录的编号（gen）——
/// - 和加入时记下的一样：被别的设备移除了。本机记着的和退出时一样删掉（own_removed），不再自己加回去
/// - 不一样：服务器的记录没了（托管平台上没挂存储、数据目录删了……）。拿记着的口令重新加入：现在的那个，不认再试部署时配置的那个
///   （记录没了的服务器认的是它）。成了返回 true，调用方再发一次。都不认（从头部署过、换了加入链接）在 hub.json 里记下这个编号
///   （stale），不再为它试；连不上、别的错下次再试
/// - 加入时没记下编号（以前的版本）、服务器没说：什么都不做
///
/// 上报、同步 key、折合金额几个进程可能同时碰到：拿着 hub/recover.lock 一个个来；轮到时 hub.json 已经变了（别的进程处理过）就返回 true
pub fn own_recover(cfg: &Dict, j: &Dict) -> R<bool> {
    let Some(gen) = doc_hex(j, "gen") else { return Ok(false) };
    let Ok(lock) = std::fs::OpenOptions::new().append(true).create(true).open(hub_path("recover.lock")) else { return Ok(false) };
    if !flock(&lock, libc::LOCK_EX) {
        return Ok(false);
    }
    let Some(doc) = hub_doc() else { return Ok(false) };
    if hub_rev() != cfg.get("rev").and_then(|r| r.as_num()) {
        return Ok(true);
    }
    let Some(known) = doc_hex(&doc, "gen") else { return Ok(false) };
    if known == gen {
        own_removed(&doc)?;
        return Ok(false);
    }
    if doc_hex(&doc, "stale").as_deref() == Some(gen.as_str()) {
        return Ok(false);
    }
    let now = doc.at("admit").py_str();
    let seed = doc_hex(&doc, "seed").filter(|s| *s != now);
    for admit in std::iter::once(now).chain(seed) {
        match join_core(&Link { admit, ..link_of(&doc) }, None)? {
            Ok(_) => return Ok(true),
            Err(("denied", _)) => {}
            Err(_) => return Ok(false),
        }
    }
    doc.set("stale", gen);
    save_doc(&doc)?;
    Ok(false)
}

// --- 一台服务器的样子（_gui state 的 config.hubs[]、cca hub status）-----------------------------------------------------------------

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
    let own = (kind == HubKind::Own).then(hub_doc).flatten();
    // 服务器上这台设备的名字：自己的记在 hub.json；维护者的是上次同步折合金额时服务器说的
    let name = match &own {
        Some(j) => j.get("name"),
        None => cost.get("me").filter(|m| m.is_str()),
    };
    dict! {
        "kind" => kind.name(), "url" => url, "host" => host_of(url), "state" => state, "error" => error,
        "checked_epoch" => last_ok, "accounts" => accounts, "name" => name.unwrap_or(V::None),
        "members" => if kind == HubKind::Own { members_known().map_or(V::None, V::Int) } else { V::None },
        "joined_epoch" => own.as_ref().and_then(|j| j.get("joined_at")).filter(is_num).unwrap_or(V::None),
        "can_invite" => kind == HubKind::Own,
        // 自己的：加入链接里钉着证书的指纹（自己部署的，cca hub deploy 更新）；false 是照证书机构验（托管平台上的，cca hub hosted 更新）
        "pinned" => own.as_ref().map_or(V::None, |j| V::Bool(!link_of(j).pins.is_empty())),
    }
}

/// 加入了的自己的服务器（没加入是 None）：hub_view 的样子；这台设备用不了它时 state 是 off（关掉了设备统计）或者 error（error 是原因，
/// 比如密钥不见了）。count：经它同步的账号数
fn own_entry(count: usize) -> R<Option<Dict>> {
    let Some(j) = hub_doc() else { return Ok(None) };
    let v = hub_view(HubKind::Own, &j.at("url").py_str(), count);
    let (why, cfg) = own_load()?;
    if cfg.is_none() {
        v.set("state", if presence_off_reason().is_some() { "off" } else { "error" });
        v.set("error", why);
    }
    Ok(Some(v))
}

/// _gui state 的 config.hubs（PROTOCOL 2.2）：这台设备用得了的维护者的服务器，加入了的自己的服务器（own_entry），各个 Team 的服务器
pub fn hub_entries(count: impl Fn(HubKind) -> usize) -> R<Vec<V>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(V::Dict(hub_view(HubKind::Maintainer, &cfg.at("url").py_str(), count(HubKind::Maintainer))));
    }
    out.extend(own_entry(count(HubKind::Own))?.map(V::Dict));
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

// --- 子命令 ------------------------------------------------------------------------------------------------------------------

/// 没成：stdout 是 "原因码\t给人看的原因"（bash 的 FAIL_CODE 是 hub_ 加上原因码），退出码 1
fn fail<T>(code: &str, why: &str) -> R<T> {
    out!("{code}\t{why}");
    exit(1)
}

/// 请求没成的原因码和原因（加入以外的几个请求共用）
fn fault(cfg: &Dict, code: i64, j: &Dict) -> R<(&'static str, String)> {
    let err = j.str("error").map(|e| e.to_string()).unwrap_or_default();
    let c = match (code, err.as_str()) {
        (0, "pin") => "pin",
        (0, "cert") => "cert",
        (0, "sign") => "sign",
        (0, _) => "unreachable",
        (403, _) => "denied",
        (404, "not_found") => "old",
        (400, "clock") => "clock",
        (429, _) => "busy",
        _ => "server",
    };
    let why = if c == "old" {
        "这台服务器不是自己的同步服务器（没有用 --mode own 运行），或者它的版本太旧".to_string()
    } else {
        presence_why(cfg, code, j)?
    };
    Ok((c, why))
}

/// 加入成了：服务器上这台设备的名字、地址:端口、现在几台设备（服务器没说是 None）、这台设备关掉了同步时的原因（没关是空的）
struct Joined {
    name: String,
    host: String,
    members: Option<i64>,
    off: String,
}

impl Joined {
    /// hub-join、hub-hosted join 的 stdout：一行一项（见 cmd_hub_join）
    fn print(&self) {
        let members = self.members.map_or(String::new(), |m| m.to_string());
        out!("{}\n{}\n{members}\n{}", self.name, self.host, self.off);
    }
}

/// 没成：(原因码, 给人看的原因)；原因码见 cmd_hub_join
type JoinFail = (&'static str, String);

/// py hub-join [--name 名字]：加入自己的同步服务器。加入链接从 stdin 读（它是凭证：不进命令行）；已经加入了同一台（同一个地址、
/// 同一份盐）是重新加入（服务器丢了记录、换名字时用）。成了：stdout 一行一项：服务器上这台设备的名字、地址:端口、现在有几台设备
/// （服务器没说是空行）、这台设备关掉了同步时的原因（没关是空行），退出码 0，接着在后台上报、同步一次；没成见 fail（原因码：link
/// 链接不对、other 已经加入了别的、pin、sign、unreachable、denied 口令不对、salt 盐对不上、full 设备满了、old、clock、busy、
/// server、name 名字不对）。第一次加入没成时什么都不留（刚生成的密钥也删掉）
pub fn cmd_hub_join(args: &[String]) -> R<()> {
    let name = match args {
        [] => None,
        [flag, n] if flag == "--name" => Some(n.trim().to_string()),
        _ => return Err(Fail::Die("用法: py hub-join [--name 名字]（加入链接从 stdin 读）".into())),
    };
    if name.as_deref().is_some_and(|n| !name_ok(n)) {
        return fail("name", NAME_BAD);
    }
    let link = match link_decode(&read_stdin_text()) {
        Ok(l) => l,
        Err(why) => return fail("link", &why),
    };
    match join_with(&link, name)? {
        Ok(j) => {
            j.print();
            Ok(())
        }
        Err((code, why)) => fail(code, &why),
    }
}

/// 拿着加入链接加入（cmd_hub_join、一键部署、部署到托管平台共用）：成了写 hub.json、members.json、device_id，在后台上报、同步一次
fn join_with(link: &Link, name: Option<String>) -> R<Result<Joined, JoinFail>> {
    let r = join_core(link, name)?;
    if r.is_ok() {
        ensure_device_id()?;
        // 马上上报、同步一次：用着的账号的会话报给它，记下的 key、折合金额和别的设备交换
        agent_kick();
        keyshare_kick(true);
        cost_kick_now();
    }
    Ok(r)
}

/// 加入，成了写 hub.json、members.json（不碰 device_id、不在后台起什么：own_recover 在上报、同步的进程里调它）
fn join_core(link: &Link, name: Option<String>) -> R<Result<Joined, JoinFail>> {
    let check = hub_check(&link.salt);
    let old = hub_doc();
    if let Some(o) = &old {
        if !(o.at("url").py_str() == link.url && hub_check(&o.at("salt").py_str()) == check) {
            let host = host_of(&o.at("url").py_str());
            return Ok(Err(("other", format!("这台设备已经加入了 {host} 上的同步服务器：要换一台，先运行 cca hub leave"))));
        }
    }
    // 名字：没写时照上次的（被移除了的照移除前的）
    let was = old.clone().or_else(|| load_quiet(&removed_path()).and_then(|j| j.as_dict()));
    let name =
        name.or_else(|| was.and_then(|o| o.str("name").filter(|n| name_ok(n)).map(|n| n.to_string()))).unwrap_or_else(host_name);
    if old.is_none() {
        own_forget()?; // 以前加入过、退出时没删干净的（密钥、同步来的）不带到这一台上
    }
    let posted = join_post(link, &name, &check);
    if old.is_none() && !matches!(posted, Ok(Ok((200, _, _)))) {
        own_forget()?; // 第一次加入没成：刚生成的密钥也不留（被移除了的说明照旧留着）
    }
    let (code, j, cfg) = match posted? {
        Ok(p) => p,
        Err(f) => return Ok(Err(f)),
    };
    if code != 200 {
        let err = j.str("error").map(|e| e.to_string()).unwrap_or_default();
        let f: JoinFail = match (code, err.as_str()) {
            (403, _) => ("denied", "服务器不认这个加入链接（入伙口令不对：链接是旧的，或者不是这台服务器的）：向已经加入的设备要一个新的".into()),
            (409, "salt_mismatch") => (
                "salt",
                "这个加入链接和服务器上已经加入的设备用的不是同一份（服务器重新部署过？）：向已经加入的设备要它的加入链接（cca hub invite）"
                    .into(),
            ),
            (507, _) => ("full", "服务器上的设备已经满了（最多 64 台）：先在别的设备上移除不用的（cca hub remove）".into()),
            _ => fault(&cfg, code, &j)?,
        };
        return Ok(Err(f));
    }
    let got = j.str("name").filter(|n| name_ok(n)).map(|n| n.to_string()).unwrap_or(name);
    let members = j.get("members").and_then(|m| m.as_int()).filter(|m| *m > 0);
    let joined = old.as_ref().and_then(|o| o.get("joined_at")).filter(is_num).unwrap_or_else(|| V::from(time().trunc() as i64));
    let doc = dict! {
        "v" => 1, "url" => link.url.as_str(), "pins" => cfg.at("pins"), "admit" => link.admit.as_str(), "salt" => link.salt.as_str(),
        "name" => got.as_str(), "joined_at" => joined,
    };
    let gen = doc_hex(&j, "gen");
    let old_gen = old.as_ref().and_then(|o| doc_hex(o, "gen"));
    if let Some(g) = &gen {
        doc.set("gen", g.as_str());
    }
    // 部署时配置的那个口令：服务器说这次用的就是它，或者以前记下的（同一台服务器）
    let seed = if j.truthy("seed") { Some(link.admit.clone()) } else { old.as_ref().and_then(|o| doc_hex(o, "seed")) };
    if let Some(s) = seed {
        doc.set("seed", s);
    }
    // 换过几次口令：同一份记录才接着数（服务器的记录没了以后从头数）
    if let Some(n) = old.as_ref().filter(|_| gen.is_some() && gen == old_gen).and_then(|o| o.get("link")).and_then(|n| n.as_int()) {
        doc.set("link", n);
    }
    // 部署到托管平台的那一台：重新加入（服务器的记录没了、再贴一次链接）时照旧记着是哪个平台（cca hub hosted 更新时用）
    if let Some(p) = old.as_ref().and_then(|o| o.str("hosted")).filter(|p| matches!(&**p, "railway" | "sealos")) {
        doc.set("hosted", &*p);
    }
    save_doc(&doc)?;
    let _ = std::fs::remove_file(removed_path());
    members_note(members);
    Ok(Ok(Joined { name: got, host: host_of(&link.url), members, off: presence_off_reason().unwrap_or_default() }))
}

/// 签好名去加入：(状态码, 服务器回的, 这台设备在它上面的配置)。没有密钥就先生成一把（目录 700）；生成不了是 sign
fn join_post(link: &Link, name: &str, check: &str) -> R<Result<(i64, Dict, Dict), JoinFail>> {
    fs::makedirs_mode(&HUB_DIR, 0o700)?;
    let kp = hub_path("id_ed25519");
    if key_at(&kp)?.is_none() {
        keygen(&kp, "cca-hub")?;
    }
    let Some((key, fp)) = key_at(&kp)? else {
        return Ok(Err(("sign", "生成不了这台设备的密钥（ssh-keygen）".into())));
    };
    let pubkey = text::split_ws(&crate::py::fs::read_text(&format!("{kp}.pub"))?).into_iter().take(2).collect::<Vec<_>>().join(" ");
    let cfg = own_cfg_of(link, key, fp);
    let payload = dict! { "pub" => pubkey, "name" => name, "admit" => link.admit.as_str(), "check" => check };
    let (code, j) = presence_post(&cfg, "join", &payload, 15)?;
    Ok(Ok((code, j, cfg)))
}

/// 经自己的服务器同步的账号有几个
fn own_accounts() -> usize {
    presence_targets(HubKind::Own).map_or(0, |t| t.len())
}

/// py hub-status [--json]：自己的同步服务器（只看本机记着的，不联网）。--json：{"version": 1, "kind": "hub_status", "joined": 加入了没有,
/// "reason": 用不了的原因（用得了是 null）, "hub": own_entry 的样子（没加入是 null）}
pub fn cmd_hub_status(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return Err(Fail::Die("用法: cca hub [status] [--json]".into())),
    };
    let (why, cfg) = own_load()?;
    let view = own_entry(if cfg.is_some() { own_accounts() } else { 0 })?;
    if as_json {
        let doc = dict! {
            "version" => 1, "kind" => "hub_status", "joined" => view.is_some(),
            "reason" => if cfg.is_some() { V::None } else { V::from(why.as_str()) }, "hub" => view.map_or(V::None, V::Dict),
        };
        out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    let Some(v) = view.filter(|_| cfg.is_some()) else {
        out!("{why}");
        return Ok(());
    };
    let members = match v.get("members").and_then(|m| m.as_int()) {
        Some(n) => format!("，服务器上 {n} 台设备"),
        None => String::new(),
    };
    out!("自己的同步服务器：{}（这台设备叫 {}{}）", v.at("host").py_str(), v.at("name").py_str(), members);
    out!("  经它同步的账号：{} 个（维护者共享的账号照旧经维护者的服务器）", v.at("accounts").py_str());
    match v.at("state").as_str() {
        Some("error") => out!("  最近一次没同步成：{}", v.at("error").py_str()),
        Some("ok") => out!("  同步正常"),
        _ => out!("  还没同步过（有账号在用、或者统计折合金额时会同步）"),
    }
    Ok(())
}

/// 问服务器现在有哪些设备：[{"fp", "name", "me", "joined_age", "seen_age"}…]；没成是 (原因码, 原因)（fault 的：服务器不认这台设备是 denied）
fn members(cfg: &Dict) -> R<Result<Vec<Dict>, JoinFail>> {
    let (code, j) = presence_post(cfg, "members", &Dict::new(), 10)?;
    let Some(rows) = j.list("members").filter(|_| code == 200) else {
        return Ok(Err(if code == 200 { ("server", "服务器回的内容看不懂".to_string()) } else { fault(cfg, code, &j)? }));
    };
    let out: Vec<Dict> = rows
        .items()
        .into_iter()
        .filter_map(|r| r.as_dict())
        .filter(|r| r.str("fp").is_some_and(|f| FP_RE.fullmatch(&f).is_some()) && r.str("name").is_some_and(|n| name_ok(&n)))
        .collect();
    members_note(Some(out.len() as i64));
    link_refresh(&j)?;
    Ok(Ok(out))
}

/// py hub-devices [--json]：自己的同步服务器上有哪些设备（问服务器）。--json：{"version": 1, "kind": "hub_devices", "generated_epoch",
/// "devices": [{"id": 指纹, "name", "me": 是不是这台, "joined_epoch", "seen_epoch": 服务器最近一次收到它的请求}…]}（这台在前）。
/// 退出码：0；1 服务器那边没成；2 没加入（没成时 stdout 是原因）。这台设备关掉了同步（"presence": false）也照样问：管设备不是同步
pub fn cmd_hub_devices(args: &[String]) -> R<()> {
    let as_json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => return Err(Fail::Die("用法: cca hub devices [--json]".into())),
    };
    let (why, cfg) = own_cfg()?;
    let Some(cfg) = cfg else {
        out!("{why}");
        return exit(2);
    };
    let rows = match members(&cfg)? {
        Ok(rows) => rows,
        Err((_, why)) => {
            out!("{why}");
            return exit(1);
        }
    };
    let now = time();
    let me = cfg.at("fp").py_str();
    let epoch = |r: &Dict, k: &str| match r.get(k).filter(is_num).and_then(|a| a.as_num()) {
        Some(age) => V::from((now - age).round() as i64),
        None => V::None,
    };
    let mut devices: Vec<Dict> = rows
        .iter()
        .map(|r| {
            let fp = r.at("fp").py_str();
            dict! {
                "id" => fp.as_str(), "name" => r.at("name"), "me" => fp == me, "joined_epoch" => epoch(r, "joined_age"),
                "seen_epoch" => epoch(r, "seen_age"),
            }
        })
        .collect();
    devices.sort_by(|a, b| b.truthy("me").cmp(&a.truthy("me")).then_with(|| a.at("name").py_str().cmp(&b.at("name").py_str())));
    if as_json {
        let doc = dict! {
            "version" => 1, "kind" => "hub_devices", "generated_epoch" => now,
            "devices" => devices.into_iter().map(V::Dict).collect::<Vec<V>>(),
        };
        out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    out!("自己的同步服务器上的设备（{} 台）：", devices.len());
    for d in &devices {
        let seen = match d.get("seen_epoch").and_then(|s| s.as_num()) {
            Some(at) if now - at < 120.0 => "，刚刚还在同步".to_string(),
            Some(at) => format!("，最近一次同步在 {}", super::presence::past_time(at, now)?),
            None => String::new(),
        };
        out!(
            "  · {}{}  {}{}",
            d.at("name").py_str(),
            if d.truthy("me") { "（这台设备）" } else { "" },
            &d.at("id").py_str()[..15.min(d.at("id").py_str().len())],
            seen
        );
    }
    Ok(())
}

/// 用户写的「设备」对得上这一台吗：名字一样，或者写的是指纹（可以不带 SHA256:）的开头至少 6 位
fn device_matches(r: &Dict, want: &str) -> bool {
    let fp = r.at("fp").py_str();
    let bare = fp.trim_start_matches("SHA256:");
    let w = want.trim_start_matches("SHA256:");
    r.at("name").py_str() == want || (w.len() >= 6 && bare.starts_with(w))
}

/// py hub-remove plan 设备：问服务器现在有哪些设备，按名字或者指纹的开头认出要移除的那一台：stdout 是 "指纹\t名字\t1（是这台设备）| 0"；
/// py hub-remove apply 指纹：从服务器上移除它（它现在的密钥作废；是这台设备自己就是退出，本机记着的也删掉），移除的是别的设备时
/// 接着换一个加入链接（relink：它手里的链接不能再用来加入），stdout 是空的；链接没换成时 stdout 是原因（退出码照样是 0）。
/// 退出码：0；1 服务器那边没成；2 没加入；3 认不出是哪一台（没成时 stdout 是原因）。关掉了同步也照样能移除
pub fn cmd_hub_remove(args: &[String]) -> R<()> {
    arity("cmd_hub_remove", args, 2, 2)?;
    let (why, cfg) = own_cfg()?;
    let Some(cfg) = cfg else {
        out!("{why}");
        return exit(2);
    };
    let me = cfg.at("fp").py_str();
    match args[0].as_str() {
        "plan" => {
            let want = args[1].trim();
            let rows = match members(&cfg)? {
                Ok(rows) => rows,
                Err((_, why)) => {
                    out!("{why}");
                    return exit(1);
                }
            };
            let exact: Vec<&Dict> = rows.iter().filter(|r| r.at("name").py_str() == want).collect();
            let hits = if exact.is_empty() { rows.iter().filter(|r| device_matches(r, want)).collect() } else { exact };
            match hits.as_slice() {
                [r] => {
                    let fp = r.at("fp").py_str();
                    out!("{}\t{}\t{}", fp, r.at("name").py_str(), if fp == me { 1 } else { 0 });
                    Ok(())
                }
                [] => {
                    out!("服务器上没有叫 {want} 的设备（cca hub devices 看看有哪些）");
                    exit(3)
                }
                _ => {
                    out!("有好几台设备对得上 {want}：写全名字，或者指纹多写几位");
                    exit(3)
                }
            }
        }
        "apply" => {
            let target = args[1].as_str();
            let (code, j) = presence_post(&cfg, "remove", &dict! { "target" => target }, 10)?;
            let gone = code == 404 && j.str("error").as_deref() == Some("no_device"); // 已经不在了：一样
            if code != 200 && !gone {
                out!("{}", fault(&cfg, code, &j)?.1);
                return exit(1);
            }
            if target == me {
                own_forget()?;
            } else {
                members_note(j.get("members").and_then(|m| m.as_int()));
                if let Err(why) = relink(&cfg)? {
                    out!("{why}");
                }
            }
            Ok(())
        }
        _ => Err(Fail::Die("用法: py hub-remove plan 设备 | apply 指纹".into())),
    }
}

/// py hub-leave plan：stdout 是 "地址:端口\t服务器上这台设备的名字"（没加入：退出码 2，stdout 是原因）；
/// py hub-leave apply：退出自己的同步服务器：先告诉服务器移除这台（连不上、它已经不认这台了也接着退），再删掉本机记着的
/// （~/.cca/hub/、被移除了的说明、presence/ 和 cost/ 里 own. 开头的；别的设备同步来的折合金额跟着没了，记下的 API key 留在这台设备上）。
/// stdout：服务器那边没移除成时是原因（本机照样删了）。退出码 0；没加入是 2
pub fn cmd_hub_leave(args: &[String]) -> R<()> {
    arity("cmd_hub_leave", args, 1, 1)?;
    let (why, cfg) = own_cfg()?;
    match args[0].as_str() {
        "plan" => match hub_doc() {
            Some(j) => {
                out!("{}\t{}", host_of(&j.at("url").py_str()), j.at("name").py_str());
                Ok(())
            }
            None => {
                out!("{why}");
                exit(2)
            }
        },
        "apply" => {
            if cfg.is_none() && !path::exists(&HUB_DIR) {
                out!("{why}");
                return exit(2);
            }
            let mut said = String::new();
            if let Some(cfg) = cfg {
                let me = cfg.at("fp").py_str();
                let (code, j) = presence_post(&cfg, "remove", &dict! { "target" => me.as_str() }, 8)?;
                let err = j.str("error").map(|e| e.to_string()).unwrap_or_default();
                if code != 200 && !matches!((code, err.as_str()), (404, "no_device") | (403, "unknown_device")) {
                    said =
                        format!("服务器上没能移除这台设备（{}）：在别的设备上运行 cca hub remove 移除它", fault(&cfg, code, &j)?.1);
                }
            }
            own_forget()?;
            // 自己退出的：没有要说明的。后台上报恰好在服务器移除了这台、本机还没删的时候碰到 403，会当成被移除了写下说明
            let _ = std::fs::remove_file(removed_path());
            if !said.is_empty() {
                out!("{said}");
            }
            Ok(())
        }
        _ => Err(Fail::Die("用法: py hub-leave plan | apply".into())),
    }
}

/// py hub-invite [--new]：打印加入链接（别的设备运行 cca hub join 时贴进去）。先问一次服务器（members）：别的设备换过链接的话换上
/// 新的（连不上照本机记着的打）。--new：先换一个（relink：旧的不能再用来加入，已经加入的设备照旧；没成：退出码 1，stdout 是原因）。
/// 没加入：退出码 2，stdout 是原因
pub fn cmd_hub_invite(args: &[String]) -> R<()> {
    let new = match args {
        [] => false,
        [a] if a == "--new" => true,
        _ => return Err(Fail::Die("用法: py hub-invite [--new]".into())),
    };
    let (why, cfg) = own_cfg()?;
    match (&cfg, new) {
        (Some(cfg), true) => {
            if let Err(why) = relink(cfg)? {
                out!("{why}");
                return exit(1);
            }
        }
        (Some(cfg), false) => {
            let _ = members(cfg)?; // 被移除了的话 own_recover 删掉了本机记着的：下面说没加入
        }
        (None, true) => {
            out!("{why}");
            return exit(if hub_doc().is_some() { 1 } else { 2 });
        }
        (None, false) => {}
    }
    let Some(j) = hub_doc() else {
        out!("{}", own_cfg()?.0);
        return exit(2);
    };
    out!("{}", link_encode(&link_of(&j)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> Link {
        Link {
            url: "https://203.0.113.5:18443".into(),
            pins: vec![format!("sha256//{}=", "A".repeat(43))],
            admit: "0123456789abcdef0123456789abcdef".into(),
            salt: "ab".repeat(16),
        }
    }

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
        assert_eq!(route(&src, "bob(bob@example.com)", &V::from("US-B")), Some(HubKind::Own), "自己的");
        let team = route(&src, "carol@example.com", &V::from("US-A"));
        assert_eq!(team, Some(HubKind::Team(TeamTag::new(&tid).expect("32 个十六进制字符"))), "Team 给的");
        assert_eq!(team.map(|k| (k.name(), k.file("keys.json"))), Some(("team", format!("team.{tid}.keys.json"))));
        assert_eq!(route(&Dict::new(), "alice(alice@example.com)", &V::from("US-A")), Some(HubKind::Own), "共享关着：都是自己的");
        assert_eq!(route(&src, "relay(alice@example.com)", &api), None, "第三方 API 账号");
        assert_eq!(route(&src, "noemail", &V::from("US-A")), None, "没有邮箱");
        let pm = dict! { "alice@example.com" => "US-A" };
        assert!(mapped(&pm, "alice(alice@example.com)", &V::from("US-A")) && !mapped(&pm, "alice(alice@example.com)", &api));
        assert!(!mapped(&pm, "bob@example.com", &V::from("US-A")) && !mapped(&pm, "noemail", &V::from("US-A")));
    }

    #[test]
    fn files_per_kind() {
        assert_eq!(HubKind::Maintainer.file("sync.json"), "sync.json");
        assert_eq!(HubKind::Own.file("sync.json"), "own.sync.json");
        assert_eq!((HubKind::Maintainer.name(), HubKind::Own.name()), ("maintainer", "own"));
    }

    #[test]
    fn link_round_trip() {
        let l = link();
        let s = link_encode(&l);
        assert!(s.starts_with(LINK_PREFIX) && !s.contains('=') && !s.contains('+') && !s.contains('/'), "{s}");
        assert_eq!(link_decode(&s), Ok(l.clone()));
        // 折了行、前后有空白
        let (a, b) = s.split_at(20);
        assert_eq!(link_decode(&format!("  {a}\n  {b}\n")), Ok(l.clone()));
        // 末尾的 / 不算
        let slash = Link { url: format!("{}/", l.url), ..l.clone() };
        assert_eq!(link_decode(&link_encode(&slash)), Ok(l.clone()));
    }

    #[test]
    fn link_matches_the_spec() {
        // tests/spec_wire.py 的 link_encode（照 server/README.md「自己的同步服务器」另写的一份）算的
        assert_eq!(
            link_encode(&link()),
            "cca-hub1.eyJ1IjoiaHR0cHM6Ly8yMDMuMC4xMTMuNToxODQ0MyIsInAiOlsic2hhMjU2Ly9BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBPSJdLCJhIjoiMDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWYiLCJzIjoiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWIifQ.803a"
        );
    }

    #[test]
    fn link_damage_is_caught() {
        let s = link_encode(&link());
        let broken = |x: &str| link_decode(x).unwrap_err();
        assert!(broken(&s[..s.len() - 3]).contains("不完整"), "少了一截");
        assert!(broken(&s[..s.len() - 6]).contains("不完整"));
        let mut flipped: Vec<char> = s.chars().collect();
        let i = LINK_PREFIX.len() + 5;
        flipped[i] = if flipped[i] == 'A' { 'B' } else { 'A' };
        assert!(broken(&flipped.into_iter().collect::<String>()).contains("不完整"), "抄错了一个字");
        assert!(broken("").contains("没有输入"));
        assert!(broken("https://203.0.113.5:18443").contains("不是加入链接"));
        assert!(broken(&s.replace("cca-hub1.", "cca-hub2.")).contains("更新版本"));
        for bad in [
            Link { url: "http://203.0.113.5:18443".into(), ..link() },
            Link { url: "https://203.0.113.5".into(), ..link() },
            Link { pins: vec![link().pins[0].clone(); 5], ..link() },
            Link { pins: vec!["sha1//x".into()], ..link() },
            Link { admit: "short".into(), ..link() },
            Link { admit: "0123456789ABCDEF0123456789ABCDEF".into(), ..link() },
            Link { salt: "xyz".into(), ..link() },
        ] {
            assert!(broken(&link_encode(&bad)).contains("加入链接里"), "{bad:?}");
        }
    }

    #[test]
    fn link_without_pins_is_ca_mode() {
        // 托管平台、反向代理后面：没有指纹（照证书机构验证书），地址照样要写端口
        let l = Link { url: "https://hub.example.com:443".into(), pins: vec![], ..link() };
        assert_eq!(link_decode(&link_encode(&l)), Ok(l));
    }

    #[test]
    fn link_blob_round_trips_and_needs_the_salt() {
        let salt = "ab".repeat(16);
        let blob = V::Dict(link_seal(&salt, &"1".repeat(32), Some(&"2".repeat(32))).unwrap());
        assert_eq!(link_open(&salt, &blob), Some(("1".repeat(32), Some("2".repeat(32)))));
        assert_eq!(link_open(&"cd".repeat(16), &blob), None, "别的盐解不开");
        let no_seed = V::Dict(link_seal(&salt, &"1".repeat(32), None).unwrap());
        assert_eq!(link_open(&salt, &no_seed), Some(("1".repeat(32), None)), "不知道部署时的口令");
        let d = blob.as_dict().unwrap();
        d.set("c", d.at("t").py_str());
        assert_eq!(link_open(&salt, &blob), None, "改过的认证不过");
    }

    #[test]
    fn check_is_hmac_of_the_salt() {
        // 期望值用 Python 算的：hmac.new(bytes.fromhex('ab' * 16), b'cca-hub-check-v1', hashlib.sha256).hexdigest()[:32]
        assert_eq!(hub_check(&"ab".repeat(16)), "7ef14b91abca231d6cb0c2d677fcd616");
        assert_ne!(hub_check(&"ab".repeat(16)), hub_check(&"cd".repeat(16)));
    }

    #[test]
    fn names_and_device_matching() {
        assert!(name_ok("studio-mac") && name_ok("家里的 Mac") && !name_ok("") && !name_ok(" a") && !name_ok("a\tb"));
        assert!(!name_ok(&"x".repeat(65)) && name_ok(&"中".repeat(64)));
        let r = dict! { "fp" => "SHA256:abcdefGHIJKL0123456789abcdefGHIJKL012345678", "name" => "studio-mac" };
        assert!(device_matches(&r, "studio-mac") && device_matches(&r, "abcdef") && device_matches(&r, "SHA256:abcdefGH"));
        assert!(!device_matches(&r, "abcde") && !device_matches(&r, "studio") && !device_matches(&r, "bcdefG"));
    }
}
