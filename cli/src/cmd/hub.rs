//! 同步服务器（hub）：设备统计、记下备查的 API key、折合金额经哪台服务器和这个账号的其他设备同步。两种：
//! - 维护者的（`HubKind::Maintainer`）：映射里的 presence 一节，拿映射的 deploy key 签名（`presence_load`）。只管维护者映射里的官方账号
//! - 自己的（`HubKind::Own`，界面上叫「自己的同步服务器」，命令是 `cca hub`）：用户自己部署、`cca hub join` 加入的，配置在 ~/.cca/hub/。
//!   管别的官方账号
//!
//! 账号走哪台只看 `route`（第三方 API 账号哪都不去）；`hub_of` 再看这台设备用不用得了那台。两台的状态文件分开（`HubKind::file`：
//! 自己的前面加 own.），盐、设备指纹也各是各的；服务器只看到账号摘要和密文。
//!
//! 加入链接：`cca-hub1.<base64url(JSON)>.<校验>`（格式见 server/README.md「自己的同步服务器」），里面有服务器的地址、证书公钥的指纹、
//! 入伙口令（admit）和盐。它是凭证：只经 stdin、提问会话传，不进命令行和环境变量。服务器只存口令的 SHA-256 和盐的校验值（`hub_check`，
//! 第一台设备加入时登记：拿着另一份盐的设备加入不了，免得同一台服务器上的设备互相解不开）。
//!
//! 子命令：hub-join（链接从 stdin 读）、hub-status、hub-devices、hub-remove、hub-leave、hub-invite（bash 的 `cca hub` 调它们）。

use super::cost::{cost_json, cost_kick_now, COST_DIR};
use super::keyshare::{is_num, keyshare_kick};
use super::presence::{
    agent_kick, key_at, presence_load, presence_off_reason, presence_post, presence_targets, presence_why, url_ok, FP_RE,
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
use crate::py::time::time;
use crate::py::{exit, json, path, text, Fail, R};
use crate::{dict, out};
use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

/// 这个模块的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("hub-join", cmd_hub_join),
    Cmd::all("hub-status", cmd_hub_status),
    Cmd::all("hub-devices", cmd_hub_devices),
    Cmd::all("hub-remove", cmd_hub_remove),
    Cmd::all("hub-leave", cmd_hub_leave),
    Cmd::all("hub-invite", cmd_hub_invite),
];

/// ~/.cca/hub：hub.json（加入的是哪台、入伙口令、盐；600）、id_ed25519（这台设备在它上面的密钥）、members.json（上次问到几台设备）
pub static HUB_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "hub"));

fn hub_path(name: &str) -> String {
    path::join(&HUB_DIR, name)
}

/// 同步服务器的两种
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HubKind {
    Maintainer,
    Own,
}

impl HubKind {
    /// 接口里的名字（_gui state 的 accounts[].hub、config.hubs[].kind）
    pub fn name(self) -> &'static str {
        match self {
            HubKind::Maintainer => "maintainer",
            HubKind::Own => "own",
        }
    }

    /// 这台服务器的状态文件叫什么：维护者的照旧，自己的前面加 own.（presence/、cost/ 里各一份，不用迁移）
    pub fn file(self, base: &str) -> String {
        match self {
            HubKind::Maintainer => base.to_string(),
            HubKind::Own => format!("own.{base}"),
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

/// 这台设备现在用得了的同步服务器，维护者的在前（config.json 里写着 "presence": false 时一台都没有）
pub fn hubs() -> R<Vec<Hub>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(Hub { kind: HubKind::Maintainer, cfg });
    }
    if let Some(cfg) = own_load()?.1 {
        out.push(Hub { kind: HubKind::Own, cfg });
    }
    Ok(out)
}

/// 这个账号（pm 是它那个工具的 policy_raw）的设备统计、记下的 key、折合金额该走哪一类服务器，不看那台配置了没有：第三方 API 账号、
/// 没有邮箱的哪都不去；维护者映射里的官方账号走维护者的（它的数据是共享这个账号的各台设备的，不上自己的服务器）；别的官方账号走自己的
pub fn route(pm: &Dict, key: &str, v: &V) -> Option<HubKind> {
    let e = parse_key(key).1.to_lowercase();
    if e.is_empty() || is_api(v) {
        return None;
    }
    Some(if pm.has(&e) { HubKind::Maintainer } else { HubKind::Own })
}

/// 这个账号是维护者映射里的官方账号吗（锁定的节点、关掉共享时收起来的就是它们）
pub fn mapped(pm: &Dict, key: &str, v: &V) -> bool {
    route(pm, key, v) == Some(HubKind::Maintainer)
}

/// 这个账号现在经哪台服务器同步：route 给的那一类，而且这台设备用得了它（hubs 是 hubs() 的结果）
pub fn hub_of<'a>(hubs: &'a [Hub], pm: &Dict, key: &str, v: &V) -> Option<&'a Hub> {
    let kind = route(pm, key, v)?;
    hubs.iter().find(|h| h.kind == kind)
}

// --- 加入链接 --------------------------------------------------------------------------------------------------------------

/// 加入链接的开头（格式不兼容时升到 cca-hub2.；只加字段不用升）
pub const LINK_PREFIX: &str = "cca-hub1.";

/// 加入链接里的：服务器的地址（https://地址:端口）、证书公钥的指纹（sha256//…，1～4 个）、入伙口令、盐（都是小写十六进制）
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub url: String,
    pub pins: Vec<String>,
    pub admit: String,
    pub salt: String,
}

/// 入伙口令：128 位的小写十六进制（服务器只存它的 SHA-256）
static ADMIT_RE: LazyLock<crate::py::re::Re> = LazyLock::new(|| crate::py::re::Re::new(r"^[0-9a-f]{32}$"));

/// 这几项合不合格；不合格是给人看的原因
fn link_problem(url: &str, pins: &[String], admit: &str, salt: &str) -> Option<&'static str> {
    if !url_ok(url) {
        Some("服务器的地址不对（应该是 https://地址:端口）")
    } else if !(1..=4).contains(&pins.len()) || !pins.iter().all(|p| PRESENCE_PIN_RE.fullmatch(p).is_some()) {
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
fn link_sum(head: &str) -> String {
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

/// hub.json：{"v": 1, "url", "pins", "admit", "salt", "name": 服务器上这台设备的名字, "joined_at": 第一次加入的时间}；
/// 没有、读不了、哪一项不合格都当作没加入
fn hub_doc() -> Option<Dict> {
    let j = load_quiet(&hub_path("hub.json"))?.as_dict()?;
    let s = |k: &str| j.str(k).map(|v| v.to_string()).unwrap_or_default();
    let pins: Vec<String> = j.list("pins")?.items().iter().map(|p| p.as_str().map(String::from)).collect::<Option<_>>()?;
    let ok = j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && link_problem(&s("url"), &pins, &s("admit"), &s("salt")).is_none();
    ok.then_some(j)
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
        return Ok(("还没加入自己的同步服务器（有加入链接时运行 cca hub join）".into(), None));
    };
    let Some((key, fp)) = key_at(&hub_path("id_ed25519"))? else {
        return Ok(("这台设备在自己的同步服务器上的密钥不见了（~/.cca/hub/id_ed25519）：重新运行 cca hub join".into(), None));
    };
    let cfg = own_cfg_of(&link_of(&j), key, fp);
    if let Ok(st) = std::fs::metadata(hub_path("hub.json")) {
        cfg.set("rev", st.mtime() as f64 + st.mtime_nsec() as f64 / 1e9);
    }
    Ok((String::new(), Some(cfg)))
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

/// 设备的名字：1～64 个字符，没有控制字符，前后没有空白（服务器也这么查）
fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.trim() == n && text::len(n) <= 64 && !n.chars().any(|c| c.is_control())
}

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

/// 生成这台设备在自己的服务器上的密钥（ed25519，没有口令）；旧的（坏了的）先删掉
fn hub_keygen(p: &str) -> R<()> {
    for f in [p.to_string(), format!("{p}.pub")] {
        match std::fs::remove_file(&f) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    let _ = Proc::new(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "cca-hub", "-f", p])
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

/// _gui state 的 config.hubs（PROTOCOL 2.2）：这台设备用得了的维护者的服务器，加入了的自己的服务器（own_entry）
pub fn hub_entries(count: impl Fn(HubKind) -> usize) -> R<Vec<V>> {
    let mut out = Vec::new();
    if let Some(cfg) = presence_load()?.1 {
        out.push(V::Dict(hub_view(HubKind::Maintainer, &cfg.at("url").py_str(), count(HubKind::Maintainer))));
    }
    out.extend(own_entry(count(HubKind::Own))?.map(V::Dict));
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
        return fail("name", "设备的名字要 1～64 个字符，不能有控制字符");
    }
    let link = match link_decode(&read_stdin_text()) {
        Ok(l) => l,
        Err(why) => return fail("link", &why),
    };
    let check = hub_check(&link.salt);
    let old = hub_doc();
    if let Some(o) = &old {
        if !(o.at("url").py_str() == link.url && hub_check(&o.at("salt").py_str()) == check) {
            let host = host_of(&o.at("url").py_str());
            return fail("other", &format!("这台设备已经加入了 {host} 上的同步服务器：要换一台，先运行 cca hub leave"));
        }
    }
    if old.is_none() {
        own_forget()?; // 以前加入过、退出时没删干净的（密钥、同步来的）不带到这一台上
    }
    let name = name.or_else(|| old.as_ref().and_then(|o| o.str("name")).map(|n| n.to_string())).unwrap_or_else(host_name);
    let posted = join_post(&link, &name, &check);
    if old.is_none() && !matches!(posted, Ok((200, _, _))) {
        own_forget()?; // 第一次加入没成：刚生成的密钥也不留
    }
    let (code, j, cfg) = posted?;
    if code != 200 {
        let err = j.str("error").map(|e| e.to_string()).unwrap_or_default();
        return match (code, err.as_str()) {
            (403, _) => fail("denied", "服务器不认这个加入链接（入伙口令不对：链接是旧的，或者不是这台服务器的）：向已经加入的设备要一个新的"),
            (409, "salt_mismatch") => fail(
                "salt",
                "这个加入链接和服务器上已经加入的设备用的不是同一份（服务器重新部署过？）：向已经加入的设备要它的加入链接（cca hub invite）",
            ),
            (507, _) => fail("full", "服务器上的设备已经满了（最多 64 台）：先在别的设备上移除不用的（cca hub remove）"),
            _ => {
                let (c, why) = fault(&cfg, code, &j)?;
                fail(c, &why)
            }
        };
    }
    let got = j.str("name").filter(|n| name_ok(n)).map(|n| n.to_string()).unwrap_or(name);
    let members = j.get("members").and_then(|m| m.as_int()).filter(|m| *m > 0);
    let joined = old.as_ref().and_then(|o| o.get("joined_at")).filter(is_num).unwrap_or_else(|| V::from(time().trunc() as i64));
    let doc = dict! {
        "v" => 1, "url" => link.url.as_str(), "pins" => cfg.at("pins"), "admit" => link.admit.as_str(), "salt" => link.salt.as_str(),
        "name" => got.as_str(), "joined_at" => joined,
    };
    save_json(&hub_path("hub.json"), &V::Dict(doc), Some(0o600), true)?;
    ensure_device_id()?;
    members_note(members);
    // 马上上报、同步一次：用着的账号的会话报给它，记下的 key、折合金额和别的设备交换
    agent_kick();
    keyshare_kick(true);
    cost_kick_now();
    let members = members.map_or(String::new(), |m| m.to_string());
    out!("{got}\n{}\n{members}\n{}", host_of(&link.url), presence_off_reason().unwrap_or_default());
    Ok(())
}

/// 签好名去加入：(状态码, 服务器回的, 这台设备在它上面的配置)。没有密钥就先生成一把（目录 700）；生成不了是 fail 的 sign
fn join_post(link: &Link, name: &str, check: &str) -> R<(i64, Dict, Dict)> {
    fs::makedirs_mode(&HUB_DIR, 0o700)?;
    let kp = hub_path("id_ed25519");
    if key_at(&kp)?.is_none() {
        hub_keygen(&kp)?;
    }
    let Some((key, fp)) = key_at(&kp)? else {
        return fail("sign", "生成不了这台设备的密钥（ssh-keygen）");
    };
    let pubkey = text::split_ws(&crate::py::fs::read_text(&format!("{kp}.pub"))?).into_iter().take(2).collect::<Vec<_>>().join(" ");
    let cfg = own_cfg_of(link, key, fp);
    let payload = dict! { "pub" => pubkey, "name" => name, "admit" => link.admit.as_str(), "check" => check };
    let (code, j) = presence_post(&cfg, "join", &payload, 15)?;
    Ok((code, j, cfg))
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

/// 问服务器现在有哪些设备：[{"fp", "name", "me", "joined_age", "seen_age"}…]；没成是原因
fn members(cfg: &Dict) -> R<Result<Vec<Dict>, String>> {
    let (code, j) = presence_post(cfg, "members", &Dict::new(), 10)?;
    let Some(rows) = j.list("members").filter(|_| code == 200) else {
        return Ok(Err(if code == 200 { "服务器回的内容看不懂".to_string() } else { fault(cfg, code, &j)?.1 }));
    };
    let out: Vec<Dict> = rows
        .items()
        .into_iter()
        .filter_map(|r| r.as_dict())
        .filter(|r| r.str("fp").is_some_and(|f| FP_RE.fullmatch(&f).is_some()) && r.str("name").is_some_and(|n| name_ok(&n)))
        .collect();
    members_note(Some(out.len() as i64));
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
        Err(why) => {
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
/// py hub-remove apply 指纹：从服务器上移除它（它现在的密钥作废，加入链接照旧；是这台设备自己就是退出，本机记着的也删掉），
/// stdout 是空的。退出码：0；1 服务器那边没成；2 没加入；3 认不出是哪一台（没成时 stdout 是原因）。关掉了同步也照样能移除
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
                Err(why) => {
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
            }
            Ok(())
        }
        _ => Err(Fail::Die("用法: py hub-remove plan 设备 | apply 指纹".into())),
    }
}

/// py hub-leave plan：stdout 是 "地址:端口\t服务器上这台设备的名字"（没加入：退出码 2，stdout 是原因）；
/// py hub-leave apply：退出自己的同步服务器：先告诉服务器移除这台（连不上、它已经不认这台了也接着退），再删掉本机记着的
/// （~/.cca/hub/、presence/ 和 cost/ 里 own. 开头的；别的设备同步来的折合金额跟着没了，记下的 API key 留在这台设备上）。
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
            if !said.is_empty() {
                out!("{said}");
            }
            Ok(())
        }
        _ => Err(Fail::Die("用法: py hub-leave plan | apply".into())),
    }
}

/// py hub-invite：打印加入链接（别的设备运行 cca hub join 时贴进去）。没加入：退出码 2，stdout 是原因
pub fn cmd_hub_invite(args: &[String]) -> R<()> {
    arity("cmd_hub_invite", args, 0, 0)?;
    let Some(j) = hub_doc() else {
        out!("还没加入自己的同步服务器（有加入链接时运行 cca hub join）");
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
        let pm = dict! { "alice@example.com" => "US-A" };
        let api = V::Dict(dict! { "base_url" => "https://api.example.com", "model" => "x" });
        assert_eq!(
            route(&pm, "alice(Alice@Example.com)", &V::from("US-A")),
            Some(HubKind::Maintainer),
            "映射里的（邮箱不分大小写）"
        );
        assert_eq!(route(&pm, "bob(bob@example.com)", &V::from("US-B")), Some(HubKind::Own), "自己的");
        assert_eq!(route(&Dict::new(), "alice(alice@example.com)", &V::from("US-A")), Some(HubKind::Own), "共享关着：都是自己的");
        assert_eq!(route(&pm, "relay(alice@example.com)", &api), None, "第三方 API 账号");
        assert_eq!(route(&pm, "noemail", &V::from("US-A")), None, "没有邮箱");
        assert!(mapped(&pm, "alice(alice@example.com)", &V::from("US-A")) && !mapped(&pm, "alice(alice@example.com)", &api));
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
            Link { pins: vec![], ..link() },
            Link { pins: vec!["sha1//x".into()], ..link() },
            Link { admit: "short".into(), ..link() },
            Link { admit: "0123456789ABCDEF0123456789ABCDEF".into(), ..link() },
            Link { salt: "xyz".into(), ..link() },
        ] {
            assert!(broken(&link_encode(&bad)).contains("加入链接里"), "{bad:?}");
        }
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
