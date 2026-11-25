//! 状态：名单里的设备、它们上报的会话、Team 的邀请，和存着的密文（API key、折合金额、各台设备的配置）。Python 的 `Store`（Py:387-899）。
//!
//! 四个文件，都在状态文件（--state）旁边：
//! - state.json：`{"v", "seq", "machines": {指纹: {"name", "key", "joined", "dev"}}, "beats": {指纹: {"seen", "end"}},
//!   "units": {指纹: {账号摘要: {"cli", "app", "used"}}}, "team": {"root", "gen", "invites", "spent"}}`
//! - keys.json：`{"acc": {账号摘要: {"blob", "fp", "name", "at"}}}`（心跳不用跟着重写它）
//! - cost-slots.json：`{"acc": {账号摘要: {设备指纹: {"blob", "name", "at"}}}}`
//! - team-docs.json：`{"docs": {设备编号: {"seq", "ct", "sig", "at"}}}`
//!
//! 写出去的要和 Python 写的逐字节一样（两种服务器可以换着跑同一个目录）：
//! - 字典里各项的先后用 `OMap` 记着；每一项里面几个键的先后是 Python 造这一项时的先后（上面写的就是）
//! - 数字用 `Num` 记着原来是整数还是小数（读进来是 `1101` 的不写成 `1101.0`）；服务器自己新记的时间是小数
//! - 文件最外面那一层、`team` 那一项里各个键的先后照读进来的，不认识的键原样留着（值记成 JSON 文字：`V` 里是 `Rc`，
//!   不能放进跨线程的状态），存的时候放回原来的位置
//! - 写法是 `save_doc` 的（Py:416-421：写到 `路径.进程号.tmp`，权限 600，再换过去），存的时机也照 Python：哪个 op 在哪一步
//!   存哪个文件，这里就在哪一步存（没改动的文件不重写）
//!
//! 和 Python 不一样的两处。存的时候换过去之前 fsync、没存成不留下临时文件（`disk`）。再就是：Python 只在启动时筛邀请、用掉的令牌、名单（`team_setup`），别的项（心跳、会话、API key 的密文）读进来
//! 是什么就是什么，手改坏了的要到用到它的请求才出错（以后每个这样的请求都出错）。这里读的时候都取成有类型的值：样子不对的
//! 那一项当它没有，一项里多出来的键不留。Python 自己写的文件里没有这样的项，所以它写的文件读进来再写出去一个字节都不变。
//!
//! 分在几个文件里：这里是读、筛、存、启动时的 `team_setup` 和各个 op 的入口；view.rs 是 report、prune、view；sync.rs 是
//! key_sync、cost_*；team.rs 是 team_*。
//!
//! Python 里 `Store` 自己带锁、自己看时间。这里不带锁：整个放在 `Ctx.store` 的 `Mutex` 里；时间由调用方给（`now`），测试不用等。
//! 测试要换小的那几个上限在 `Limits` 里（Python 的测试是 `mock.patch` 模块里的常量）。

pub mod sync;
pub mod team;
pub mod view;

use super::disk::{self, Step};
use super::limits::Limits;
use super::omap::OMap;
use super::sshsig;
use super::wire::{self, Blob, Num};
use super::{API, MAX_ACCOUNTS, MAX_COST_BLOB, MAX_TEAM_DOC, RELAY_OPS, SYNC_OPS, TEAM_DOCS_BYTES, TEAM_OPS, TEAM_ROOT_OPS};
use crate::dict;
use crate::py::json::{self, Fmt};
use crate::py::obj::{Dict, List, V};
use crate::py::{fs as pyfs, hash, path, rand, Fail, R};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

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

// ---------------------------------------------------------------- 状态里的各项（键的先后是 Python 造这一项时的先后）

/// 名单里的一台设备：machines[指纹]（Py:747-749、786-788）
#[derive(Clone, Debug, PartialEq)]
struct Machine {
    name: String,
    /// 公钥那一行（`ssh-ed25519 AAAA…`）
    key: String,
    /// 什么时候加入的；用的时候要是正数才算（`is_time`）
    joined: Option<Num>,
    /// 它在 Team 里的编号
    dev: String,
    /// 加入时带的认证码（服务器不懂，原样存着、回给管理员）
    proof: Option<String>,
}

/// 一台设备最近一次的报告：beats[指纹]（Py:537）
#[derive(Clone, Debug, PartialEq)]
struct Beat {
    seen: Option<Num>,
    /// 报告里的 end：没有、`none`、`down`
    end: Option<String>,
}

/// 一台设备上一个账号的会话：units[指纹][账号摘要]（Py:543-546）。每个会话是 [开始, 最后动静]（服务器的时间）
#[derive(Clone, Debug, PartialEq)]
struct Unit {
    cli: Vec<[Num; 2]>,
    app: Option<[Num; 2]>,
    /// 上次使用。新造的是整数 0（Py:543），之后是会话里最晚的那个时间
    used: Option<Num>,
}

/// 一个邀请：team.invites[令牌的 SHA-256]（Py:767）
#[derive(Clone, Debug, PartialEq)]
struct Inv {
    dev: String,
    /// 什么时候过期
    exp: Num,
    at: Option<Num>,
}

/// 一个账号记下的 API key 的密文：keys.json 的 acc[账号摘要]（Py:576；移除了传它的设备以后没有 fp、name，Py:683-686）
#[derive(Clone, Debug, PartialEq)]
struct KeyEntry {
    blob: Blob,
    fp: Option<String>,
    name: Option<String>,
    at: Option<Num>,
}

/// 一台设备传的一个账号的折合金额：cost-slots.json 的 acc[账号摘要][指纹]（Py:605）
#[derive(Clone, Debug, PartialEq)]
struct CostEntry {
    blob: Blob,
    name: Option<String>,
    at: Option<Num>,
}

/// 管理员给一台设备的配置：team-docs.json 的 docs[设备编号]（Py:815）
#[derive(Clone, Debug, PartialEq)]
struct DocEntry {
    seq: i64,
    ct: Blob,
    sig: String,
    /// 不是根密钥签的：签名的那把公钥那一行（3.6.15 起）
    by: Option<String>,
    at: Option<Num>,
}

/// state.json 最外面那一层里，v、seq、machines、beats、units 之后的项，照文件里的先后（Python 是 `data.update(读进来的)`，Py:441-442）
#[derive(Clone, Debug, PartialEq)]
enum Rest {
    /// "team" 占着的位置（内容在 Store 的 gen、invites、spent 里）
    Team,
    /// 不认识的键：原样留着（值是紧凑的 JSON 文字）
    Other(String, String),
}

/// state.json 的 team 那一项里的各个键，照文件里的先后（接着用时 Python 是在读进来的那个字典上改的，Py:473-478）
#[derive(Clone, Debug, PartialEq)]
enum TeamKey {
    Root,
    Gen,
    Invites,
    Spent,
    /// 根密钥签过的管理员名单（3.6.15 起；没有时不写）
    Admins,
    /// 几位管理员共用的状态（3.6.15 起；没有时不写）
    AState,
    /// 不认识的键：原样留着（值是紧凑的 JSON 文字）
    Other(String, String),
}

impl TeamKey {
    /// 从头开始时新造的那一项：root、gen、invites、spent（Py:494）
    fn fresh() -> Vec<TeamKey> {
        vec![TeamKey::Root, TeamKey::Gen, TeamKey::Invites, TeamKey::Spent]
    }
}

/// keys.json、cost-slots.json、team-docs.json 这样的一个文件：`{"v": 1, 那一个字典, 不认识的键…}`
#[derive(Clone, Debug)]
struct Side<T> {
    /// "v" 的值（JSON 文字；平常是 1）
    v: String,
    items: OMap<T>,
    /// 不认识的键和它的值（紧凑的 JSON 文字），照文件里的先后
    extra: Vec<(String, String)>,
}

impl<T> Side<T> {
    /// `{'v': 1, 那一个字典: {}}`
    fn new() -> Side<T> {
        Side { v: "1".into(), items: OMap::new(), extra: Vec::new() }
    }

    /// 读进来的文件最外面那一层：v 和不认识的键记下来（name 是那一个字典的键，它的内容由调用方取）
    fn from_file(d: &Dict, name: &str) -> Side<T> {
        let mut side = Side::new();
        for (k, v) in d.items() {
            if &*k == "v" {
                side.v = text_of(&v);
            } else if &*k != name {
                side.extra.push((k.to_string(), text_of(&v)));
            }
        }
        side
    }

    /// 写成 JSON 的值：v、那一个字典、不认识的键
    fn to_v(&self, name: &str, f: impl Fn(&T) -> V) -> V {
        let d = dict! { "v" => value_of(&self.v), name => self.items.to_v(f) };
        for (k, text) in &self.extra {
            d.set(k.as_str(), value_of(text));
        }
        V::Dict(d)
    }
}

pub struct Store {
    cfg: StoreCfg,
    lim: Limits,
    /// 所有设备的配置加起来最多这么多个字符（TEAM_DOCS_BYTES；测试里换小的）
    docs_bytes: usize,
    /// 回给设备的 ttl、idle 写成什么样（`reply_times`）
    ttl_out: Num,
    idle_out: Num,
    /// 四个文件的路径，照 Python 的写法拼的（Py:437-439：状态文件旁边就是 `os.path.dirname(path) or '.'`）
    path: String,
    keys_path: String,
    cost_path: String,
    docs_path: String,
    /// 配置的根公钥那一行（`ssh-ed25519 AAAA…`）
    root_key: String,
    /// 根公钥的指纹：管理员的请求的 key（它不是设备，登记、加入都不收）
    root_fp: String,
    /// Team 的编号（/v1/health 的 team）
    team_id: String,
    /// 这一份记录的编号：设备拿它分清“被移除了”和“服务器的记录清掉了”
    gen: String,

    // ---- state.json
    /// "v"、"seq" 的值（JSON 文字）：读进来是什么就是什么，平常是 1 和 0
    v: String,
    seq: String,
    machines: OMap<Machine>,
    beats: OMap<Beat>,
    units: OMap<OMap<Unit>>,
    rest: Vec<Rest>,
    /// team.invites、team.spent
    invites: OMap<Inv>,
    spent: OMap<Num>,
    /// team 那一项里各个键的先后（不认识的键也在里面）
    team_keys: Vec<TeamKey>,

    // ---- 旁边的三个文件
    keys: Side<KeyEntry>,
    cost: Side<OMap<CostEntry>>,
    docs: Side<DocEntry>,

    /// team.admins：根密钥签过的管理员名单；team.astate：几位管理员共用的状态（版本号, 密文）
    admins: Option<wire::Admins>,
    astate: Option<(i64, Blob)>,

    // ---- 只在内存里的
    /// 各台设备最近一次验过签的请求（不为它每个请求都重写状态文件）
    seen: HashMap<String, f64>,
    /// 各台设备同步配置时说的手里那一份的序号
    have: HashMap<String, i64>,
}

/// `team_setup` 要知道的、读文件时看到的几样
struct Loaded {
    /// team 那一项是同一个根公钥的、gen 的样子也对：接着用
    kept: bool,
    /// 接着用时筛掉了样子不对的邀请、用掉的令牌、设备：状态文件要重写（Py:476-482）
    changed: bool,
    had_own: bool,
    had_team: bool,
    /// 文件里 machines、keys.json 的 acc 各有几项（筛之前的：从头开始时说“以前的几台设备”用，Py:486-493）
    machines: usize,
    keys: usize,
}

const COMPACT: Fmt = Fmt::new().utf8().compact();

/// 一个 JSON 的值写成紧凑的文字
fn text_of(v: &V) -> String {
    json::dumps(v, COMPACT)
}

/// `text_of` 写的再读回来（一定读得回来；万一不行当 null）
fn value_of(text: &str) -> V {
    json::loads(text).unwrap_or(V::None)
}

/// 回给设备的出错：`(状态码, {"error": 哪一种})`
fn error(code: u16, what: &str) -> (u16, V) {
    (code, V::Dict(dict! { "error" => what }))
}

/// 请求里的各项不合格
fn bad() -> (u16, V) {
    error(400, "bad")
}

/// 往日志（stderr）里说一句。测试里收进这个线程的一张单子（Python 的测试是把 sys.stderr 换掉）
pub(in crate::server) fn say(line: String) {
    #[cfg(test)]
    tests::SAID.with(|s| s.borrow_mut().push(line));
    #[cfg(not(test))]
    crate::err!("{line}");
}

/// `load_doc`（Py:402-413）：没有这个文件、不是字典、keys 那几项不是字典都是 None；读不了、格式不对的在日志里说一声
fn load_doc(path: &str, keys: &[&str]) -> Option<Dict> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            say(format!("cca-presence: {path} 读不了（{}），从空的开始", pyfs::open_error_text(&e, path)));
            return None;
        }
    };
    match json::loads_file_bytes(&raw) {
        Ok(V::Dict(d)) if keys.iter().all(|k| d.dict(k).is_some()) => Some(d),
        Ok(_) => None,
        Err(e) => {
            say(format!("cca-presence: {path} 读不了（{e}），从空的开始"));
            None
        }
    }
}

/// OSError 带着文件名时 `str(e)` 的样子（“处理请求出错：…”那一句里看得到是哪个文件）
fn os_fail(e: &io::Error, what: String) -> Fail {
    Fail::Os(io::Error::new(e.kind(), format!("{}: {what}", crate::py::fail::os_error_text(e))))
}

/// `save_doc`（Py:416-421）：写到 `路径.进程号.tmp`（新建的话权限 600），再换过去。和 Python 不一样的：换过去之前 fsync，
/// 没存成时不留下临时文件（见 `disk`）
fn save_doc(path: &str, data: &V) -> R<()> {
    let tmp = format!("{path}.{}.tmp", std::process::id());
    let quoted = crate::py::obj::repr_str;
    disk::replace(Path::new(&tmp), Path::new(path), text_of(data).as_bytes()).map_err(|(step, e)| match step {
        Step::Open => os_fail(&e, quoted(&tmp)),
        Step::Write => e.into(),
        Step::Rename => os_fail(&e, format!("{} -> {}", quoted(&tmp), quoted(path))),
    })
}

/// 数字是不是真的（Python 的 `if x`）：0、0.0 是假
fn truthy(n: Num) -> bool {
    n.f() != 0.0
}

/// `x or 0`：没有、是 0 的都当整数 0
fn or_zero(x: Option<Num>) -> Num {
    x.filter(|n| truthy(*n)).unwrap_or(Num::Int(0))
}

/// 两个数的列表：`[开始, 最后动静]`
fn pair_v(p: &[Num; 2]) -> V {
    let [a, b] = *p;
    V::List(List::from_vec(vec![a.v(), b.v()]))
}

/// 读进来的 `[开始, 最后动静]`：正好两个数
fn pair_of(v: &V) -> Option<[Num; 2]> {
    let l = v.as_list()?;
    if l.len() != 2 {
        return None;
    }
    Some([Num::of(&l.get(0)?)?, Num::of(&l.get(1)?)?])
}

impl Machine {
    /// 读进来的一项。筛法是 Python 的（Py:479-480）：编号、名字、公钥的样子都要对
    fn of(m: &V) -> Option<Machine> {
        let m = m.as_dict()?;
        let dev = m.str("dev").filter(|d| wire::dev_re().matches(d))?;
        let name = m.str("name").filter(|n| wire::name_text_ok(n))?;
        let key = m.str("key").filter(|k| wire::key_re().matches(k))?;
        Some(Machine {
            name: name.to_string(),
            key: key.to_string(),
            joined: m.get("joined").as_ref().and_then(Num::of),
            dev: dev.to_string(),
            proof: m.str("proof").map(|p| p.to_string()),
        })
    }

    fn v(&self) -> V {
        let d = dict! { "name" => &self.name, "key" => &self.key };
        if let Some(j) = self.joined {
            d.set("joined", j.v());
        }
        d.set("dev", &self.dev);
        if let Some(p) = &self.proof {
            d.set("proof", p);
        }
        V::Dict(d)
    }

    /// 加入的时间：是正数才算（`is_time(m.get('joined'))`）
    fn joined_time(&self) -> Option<Num> {
        self.joined.filter(|j| j.f() > 0.0)
    }
}

impl Beat {
    fn of(b: &V) -> Option<Beat> {
        let b = b.as_dict()?;
        Some(Beat { seen: b.get("seen").as_ref().and_then(Num::of), end: b.str("end").map(|e| e.to_string()) })
    }

    fn v(&self) -> V {
        let d = Dict::new();
        if let Some(s) = self.seen {
            d.set("seen", s.v());
        }
        d.set("end", self.end.as_deref());
        V::Dict(d)
    }
}

impl Unit {
    /// 读进来的一项：cli 是 [[开始, 最后动静]…]（没有、空的都是没有会话），app 是 [开始, 最后动静] 或者没有
    fn of(e: &V) -> Option<Unit> {
        let e = e.as_dict()?;
        let cli = match e.get("cli").filter(V::truthy) {
            Some(c) => c.as_list()?.items().iter().map(pair_of).collect::<Option<Vec<_>>>()?,
            None => Vec::new(),
        };
        let app = match e.get("app").filter(V::truthy) {
            Some(a) => Some(pair_of(&a)?),
            None => None,
        };
        Some(Unit { cli, app, used: e.get("used").as_ref().and_then(Num::of) })
    }

    fn v(&self) -> V {
        let d = dict! {
            "cli" => List::from_vec(self.cli.iter().map(pair_v).collect()),
            "app" => self.app.as_ref().map(pair_v),
        };
        if let Some(u) = self.used {
            d.set("used", u.v());
        }
        V::Dict(d)
    }
}

impl Inv {
    /// 筛法是 Python 的（Py:477）：过期的时间是正数、编号的样子对
    fn of(i: &V) -> Option<Inv> {
        let i = i.as_dict()?;
        let exp = i.get("exp").filter(wire::is_time).as_ref().and_then(Num::of)?;
        let dev = i.str("dev").filter(|d| wire::dev_re().matches(d))?;
        Some(Inv { dev: dev.to_string(), exp, at: i.get("at").as_ref().and_then(Num::of) })
    }

    fn v(&self) -> V {
        let d = dict! { "dev" => &self.dev, "exp" => self.exp.v() };
        if let Some(at) = self.at {
            d.set("at", at.v());
        }
        V::Dict(d)
    }
}

/// 读进来的一份密文，不查长短（Python 读 keys.json 时不筛）：正好 n、c、t 三项，都是字符串
fn blob_of(b: &V) -> Option<Blob> {
    let b = b.as_dict()?;
    if b.len() != 3 {
        return None;
    }
    Some(Blob { n: b.str("n")?.to_string(), c: b.str("c")?.to_string(), t: b.str("t")?.to_string() })
}

impl KeyEntry {
    fn of(e: &V) -> Option<KeyEntry> {
        let e = e.as_dict()?;
        Some(KeyEntry {
            blob: blob_of(&e.get("blob")?)?,
            fp: e.str("fp").map(|s| s.to_string()),
            name: e.str("name").map(|s| s.to_string()),
            at: e.get("at").as_ref().and_then(Num::of),
        })
    }

    fn v(&self) -> V {
        let d = dict! { "blob" => self.blob.to_v() };
        if let Some(fp) = &self.fp {
            d.set("fp", fp);
        }
        if let Some(name) = &self.name {
            d.set("name", name);
        }
        if let Some(at) = self.at {
            d.set("at", at.v());
        }
        V::Dict(d)
    }
}

impl CostEntry {
    /// 筛法是 Python 的（Py:447）：密文要过 `parse_blob`
    fn of(e: &V) -> Option<CostEntry> {
        let e = e.as_dict()?;
        Some(CostEntry {
            blob: wire::parse_blob(&e.get("blob")?, MAX_COST_BLOB)?,
            name: e.str("name").map(|s| s.to_string()),
            at: e.get("at").as_ref().and_then(Num::of),
        })
    }

    fn v(&self) -> V {
        let d = dict! { "blob" => self.blob.to_v() };
        if let Some(name) = &self.name {
            d.set("name", name);
        }
        if let Some(at) = self.at {
            d.set("at", at.v());
        }
        V::Dict(d)
    }

    /// 密文多长（`len(e['blob']['c'])`：base64，一个字符一个字节）
    fn size(&self) -> usize {
        self.blob.c.len()
    }

    /// `e.get('at') or 0`
    fn at(&self) -> f64 {
        or_zero(self.at).f()
    }
}

impl DocEntry {
    /// 筛法是 Python 的（Py:451-452）：序号是正整数、密文过 `parse_blob`、签名是字符串。放不进 i64 的序号不认
    /// （Python 存的都比 2^53 小）
    fn of(e: &V) -> Option<DocEntry> {
        let e = e.as_dict()?;
        Some(DocEntry {
            seq: e.get("seq").filter(wire::is_count)?.as_int()?,
            ct: wire::parse_blob(&e.get("ct")?, MAX_TEAM_DOC)?,
            sig: e.str("sig")?.to_string(),
            by: e.str("by").filter(|b| wire::key_re().matches(b)).map(|b| b.to_string()),
            at: e.get("at").as_ref().and_then(Num::of),
        })
    }

    fn v(&self) -> V {
        let d = dict! { "seq" => self.seq, "ct" => self.ct.to_v(), "sig" => &self.sig };
        if let Some(by) = &self.by {
            d.set("by", by.as_str());
        }
        if let Some(at) = self.at {
            d.set("at", at.v());
        }
        V::Dict(d)
    }
}

impl Store {
    /// 读四个文件、筛掉样子不对的项（Py:433-453），再照根公钥做 `team_setup`（Py:468-494：根公钥和上次的不一样就从头开始，
    /// 以前的设备、密文都不要了，在日志里说一句）。root_line 是 `ssh-ed25519 AAAA…` 那一行；now 是服务器的时间。
    /// 存不了（状态文件所在的目录写不了……）是 Err：Python 里是没人接的异常，启动不了
    pub fn open(cfg: &StoreCfg, root_line: &str, lim: Limits, now: f64) -> R<Store> {
        let _ = now; // Python 启动时不看时间（过期的邀请、很久没传的密文都是之后的请求顺带清的）
        let Some(root_blob) = sshsig::key_blob(root_line) else {
            return Err(Fail::Value("根公钥不是 ssh-ed25519 AAAA… 那一行".into()));
        };
        let path = cfg.path.to_string_lossy().into_owned();
        let dir = match path::dirname(&path) {
            d if d.is_empty() => ".".to_string(),
            d => d,
        };
        let beside = |given: &Option<PathBuf>, name: &str| match given.as_ref().filter(|p| !p.as_os_str().is_empty()) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => path::join(&dir, name),
        };
        let mut st = Store {
            cfg: cfg.clone(),
            lim,
            docs_bytes: TEAM_DOCS_BYTES,
            ttl_out: Num::Float(cfg.ttl),
            idle_out: Num::Float(cfg.idle),
            keys_path: beside(&cfg.keys_path, "keys.json"),
            cost_path: beside(&cfg.cost_path, "cost-slots.json"),
            docs_path: path::join(&dir, "team-docs.json"),
            path,
            root_key: root_line.to_string(),
            root_fp: sshsig::key_fp(&root_blob),
            team_id: sshsig::team_id(&root_blob),
            gen: String::new(),
            v: "1".into(),
            seq: "0".into(),
            machines: OMap::new(),
            beats: OMap::new(),
            units: OMap::new(),
            rest: Vec::new(),
            invites: OMap::new(),
            spent: OMap::new(),
            team_keys: TeamKey::fresh(),
            keys: Side::new(),
            cost: Side::new(),
            docs: Side::new(),
            admins: None,
            astate: None,
            seen: HashMap::new(),
            have: HashMap::new(),
        };
        let loaded = st.load();
        st.team_setup(&loaded)?;
        Ok(st)
    }

    /// 读四个文件（Py:441-452）。state.json 里的名单、邀请只有接着用时才取（从头开始的话都不要了）
    fn load(&mut self) -> Loaded {
        let state = load_doc(&self.path, &["machines", "beats", "units"]).unwrap_or_default();
        let keys = load_doc(&self.keys_path, &["acc"]).unwrap_or_default();
        let cost = load_doc(&self.cost_path, &["acc"]).unwrap_or_default();
        let docs = load_doc(&self.docs_path, &["docs"]).unwrap_or_default();

        for (k, v) in state.items() {
            match &*k {
                "v" => self.v = text_of(&v),
                "seq" => self.seq = text_of(&v),
                "machines" | "beats" | "units" => {}
                "team" => self.rest.push(Rest::Team),
                _ => self.rest.push(Rest::Other(k.to_string(), text_of(&v))),
            }
        }
        let machines = state.dict("machines").unwrap_or_default();
        let mut out = Loaded {
            kept: false,
            changed: false,
            had_own: state.has("own"),
            had_team: state.has("team"),
            machines: machines.len(),
            keys: 0,
        };
        // 接着用：team 是字典、根公钥和配置的一样、gen 的样子对（Py:474）。`str(gen)` 对得上的除了字符串只有 32 位的十进制整数
        let team = state.dict("team");
        let gen = team.as_ref().and_then(|t| match t.get("gen") {
            Some(V::Str(s)) | Some(V::Big(s)) => Some(s.to_string()),
            _ => None,
        });
        if let (Some(team), Some(gen)) = (team, gen) {
            if team.str("root").is_some_and(|r| *r == *self.root_key) && wire::gen_re().matches(&gen) {
                out.kept = true;
                self.gen = gen;
                out.changed = self.load_team(&team, &machines);
                for (fp, b) in state.dict("beats").unwrap_or_default().items() {
                    if let Some(b) = Beat::of(&b) {
                        self.beats.insert(&*fp, b);
                    }
                }
                for (fp, accts) in state.dict("units").unwrap_or_default().items() {
                    if let Some(accts) = accts.as_dict() {
                        self.units.insert(&*fp, OMap::from_dict(&accts, |_, e| Unit::of(e)));
                    }
                }
            }
        }

        let acc = keys.dict("acc").unwrap_or_default();
        out.keys = acc.len();
        self.keys = Side::from_file(&keys, "acc");
        self.keys.items = OMap::from_dict(&acc, |_, e| KeyEntry::of(e));

        // 折合金额：样子不对的那一份不要；账号下面一份都不剩的先留着空的（Py:447-448），之后 cost_drop 清
        self.cost = Side::from_file(&cost, "acc");
        for (aid, slots) in cost.dict("acc").unwrap_or_default().items() {
            if let Some(slots) = slots.as_dict() {
                self.cost.items.insert(&*aid, OMap::from_dict(&slots, |_, e| CostEntry::of(e)));
            }
        }

        self.docs = Side::from_file(&docs, "docs");
        self.docs.items = OMap::from_dict(&docs.dict("docs").unwrap_or_default(), |_, e| DocEntry::of(e));
        out
    }

    /// 接着用时：team 里的邀请、用掉的令牌和名单，样子不对的不要（手改坏了的：留着的话以后每个请求都出错）（Py:475-480）。
    /// 返回有没有筛掉什么（invites、spent 不是字典也算：Python 会把它换成空的字典）
    fn load_team(&mut self, team: &Dict, machines: &Dict) -> bool {
        let mut changed = false;
        match team.dict("invites") {
            Some(d) => {
                for (h, i) in d.items() {
                    match Inv::of(&i) {
                        Some(i) => {
                            self.invites.insert(&*h, i);
                        }
                        None => changed = true,
                    }
                }
            }
            None => changed = true,
        }
        match team.dict("spent") {
            Some(d) => {
                for (h, t) in d.items() {
                    match Some(&t).filter(|t| wire::is_time(t)).and_then(Num::of) {
                        Some(t) => {
                            self.spent.insert(&*h, t);
                        }
                        None => changed = true,
                    }
                }
            }
            None => changed = true,
        }
        for (fp, m) in machines.items() {
            match Machine::of(&m) {
                Some(m) => {
                    self.machines.insert(&*fp, m);
                }
                None => changed = true,
            }
        }
        // 各个键的先后照文件里的；原来没有 invites、spent 的，Python 赋值时加在最后（先 invites 后 spent）
        self.team_keys = team
            .items()
            .iter()
            .map(|(k, v)| match &**k {
                "root" => TeamKey::Root,
                "gen" => TeamKey::Gen,
                "invites" => TeamKey::Invites,
                "spent" => TeamKey::Spent,
                "admins" => TeamKey::Admins,
                "astate" => TeamKey::AState,
                _ => TeamKey::Other(k.to_string(), text_of(v)),
            })
            .collect();
        // 管理员名单照根公钥再验一遍（手改过、换了根公钥的不认），共用的状态的样子要对：不对的丢掉
        if let Some(a) = team.dict("admins") {
            let blob = sshsig::key_blob(&self.root_key).unwrap_or_default();
            self.admins =
                wire::parse_admins(&a.get("list").unwrap_or(V::None), &a.get("sig").unwrap_or(V::None), &blob, &self.team_id);
            changed |= self.admins.is_none();
        }
        if let Some(s) = team.dict("astate") {
            let rev = s.get("rev").filter(wire::seq_ok).and_then(|r| r.as_int());
            let ct = s.get("ct").and_then(|c| wire::parse_blob(&c, super::MAX_ASTATE));
            self.astate = rev.zip(ct);
            changed |= self.astate.is_none();
        }
        for key in [TeamKey::Invites, TeamKey::Spent] {
            if !self.team_keys.contains(&key) {
                self.team_keys.push(key);
            }
        }
        changed
    }

    /// 配置的根公钥和上次的不一样（第一次启动、换了一个 Team、以前是维护者的服务器或者自己的同步服务器）就从头开始：以前的设备、
    /// 邀请、记录、密文都不要了（另一个 Team 的盐算出来的账号摘要和密钥都不一样，旧的谁也用不上）。接着用时，不在名单里的设备的
    /// 记录不留（forget_gone：以前的版本移除设备时留下的）（Py:468-494）
    fn team_setup(&mut self, loaded: &Loaded) -> R<()> {
        if loaded.kept {
            if loaded.changed {
                self.save()?;
            }
            self.docs_prune()?;
            return self.forget_gone();
        }
        if loaded.machines > 0 || loaded.keys > 0 || !self.cost.items.is_empty() || !self.docs.items.is_empty() {
            let was = if loaded.had_own {
                "以前是自己的同步服务器（3.5.3 以前的 --mode own）"
            } else if !loaded.had_team {
                "以前是维护者的服务器（3.6.0 以前的 --mode maintainer）"
            } else {
                "换了一个 Team（根公钥不一样）"
            };
            say(format!("cca-presence: {was}：以前的 {} 台设备和存的密文都不要了", loaded.machines));
        }
        self.reset(rand::token_hex(16))
    }

    /// 从头开始：以前的设备、上报的记录、存的密文（API key、折合金额）、各台设备的配置都不要了。gen 是这一份记录新的编号。
    /// 'own' 是 3.5.3 以前的自己的同步服务器留下的：一起去掉（Py:455-466）
    fn reset(&mut self, gen: String) -> R<()> {
        self.rest.retain(|r| !matches!(r, Rest::Other(k, _) if k == "own"));
        if !self.rest.contains(&Rest::Team) {
            self.rest.push(Rest::Team);
        }
        self.seq = "0".into();
        self.machines.clear();
        self.beats.clear();
        self.units.clear();
        self.gen = gen;
        self.invites.clear();
        self.spent.clear();
        self.team_keys = TeamKey::fresh();
        self.admins = None;
        self.astate = None;
        self.keys = Side::new();
        self.cost = Side::new();
        self.save_keys()?;
        self.save_cost()?;
        self.save()?;
        // 没有就不写（第一次启动）：不留一个空的 team-docs.json
        if !self.docs.items.is_empty() || path::exists(&self.docs_path) {
            self.docs = Side::new();
            self.save_docs()?;
        }
        Ok(())
    }

    /// 存 state.json（Py:496-497）
    fn save(&self) -> R<()> {
        let d = dict! {
            "v" => value_of(&self.v),
            "seq" => value_of(&self.seq),
            "machines" => self.machines.to_v(Machine::v),
            "beats" => self.beats.to_v(Beat::v),
            "units" => self.units.to_v(|accts| accts.to_v(Unit::v)),
        };
        for r in &self.rest {
            match r {
                Rest::Team => {
                    let team = Dict::new();
                    for key in &self.team_keys {
                        match key {
                            TeamKey::Root => team.set("root", &self.root_key),
                            TeamKey::Gen => team.set("gen", &self.gen),
                            TeamKey::Invites => team.set("invites", self.invites.to_v(Inv::v)),
                            TeamKey::Spent => team.set("spent", self.spent.to_v(|t| t.v())),
                            TeamKey::Admins => {
                                if let Some(a) = &self.admins {
                                    team.set("admins", a.to_v());
                                }
                            }
                            TeamKey::AState => {
                                if let Some((rev, ct)) = &self.astate {
                                    team.set("astate", dict! { "rev" => *rev, "ct" => ct.to_v() });
                                }
                            }
                            TeamKey::Other(k, text) => team.set(k.as_str(), value_of(text)),
                        }
                    }
                    d.set("team", team);
                }
                Rest::Other(k, text) => d.set(k.as_str(), value_of(text)),
            }
        }
        save_doc(&self.path, &V::Dict(d))
    }

    fn save_keys(&self) -> R<()> {
        save_doc(&self.keys_path, &self.keys.to_v("acc", KeyEntry::v))
    }

    fn save_cost(&self) -> R<()> {
        save_doc(&self.cost_path, &self.cost.to_v("acc", |slots| slots.to_v(CostEntry::v)))
    }

    fn save_docs(&self) -> R<()> {
        save_doc(&self.docs_path, &self.docs.to_v("docs", DocEntry::v))
    }

    /// GET /v1/health 的内容（Py:525-533）。mode 总是 team、seq 总是 0（这两项留着：以前的客户端、安装脚本照它们认是不是 Team 的
    /// 服务器）；relay_port 给了（开着中转）才列中转的那几个 op、带 relay 那一项
    pub fn health(&self, sig_ok: bool, relay_port: Option<u16>) -> V {
        let ops = List::new();
        for op in ["report", "view"] {
            ops.push(op);
        }
        for (op, _) in SYNC_OPS {
            ops.push(op);
        }
        // 3.6.15 起的几位管理员的 op 不列：/v1/health 和以前逐字节一样（客户端发了这台服务器不认的 op 会回 404 not_found，照它说服务器太旧）
        for op in TEAM_OPS.iter().filter(|op| !op.starts_with("team_astate") && !TEAM_ROOT_OPS.contains(op)) {
            ops.push(*op);
        }
        if relay_port.is_some() {
            for op in RELAY_OPS {
                ops.push(op);
            }
        }
        let out = dict! {
            "v" => API,
            "ok" => true,
            "sig_ok" => sig_ok,
            "machines" => self.machines.len(),
            "seq" => value_of(&self.seq),
            "mode" => "team",
            "ops" => ops,
            "team" => &self.team_id,
        };
        if let Some(port) = relay_port {
            out.set("relay", dict! { "port" => port });
        }
        V::Dict(out)
    }

    /// 名单里这台设备的公钥那一行（验它的签名用）；不在名单里是 None（Py:499-501 的 machine(fp)['key']）
    pub fn machine_key(&self, fp: &str) -> Option<String> {
        self.machines.get(fp).map(|m| m.key.clone())
    }

    /// 管理员名单里这把密钥（指纹）的公钥那一行；不在名单里是 None
    pub fn admin_key(&self, fp: &str) -> Option<String> {
        self.admins.as_ref()?.keys.iter().find(|(f, _)| f == fp).map(|(_, line)| line.clone())
    }

    /// 名单里这台设备的名字；没有是 None（Py:516-519）
    pub fn name_of(&self, fp: &str) -> Option<String> {
        self.machines.get(fp).map(|m| m.name.clone())
    }

    /// 名单里各台设备的指纹（Py:521-523）：给 `Remote::keep_only` 的；先拷出来、放开这把锁，再去拿 Remote 的锁
    pub fn fps(&self) -> Vec<String> {
        self.machines.keys().map(str::to_string).collect()
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

    /// 回给设备的 ttl、idle 写成什么样（Py:1985、897）。命令行里给了 --ttl、--idle 的是小数（`150.0`）；没给的是 Python 的默认值：
    /// 整数 150、600，写出来不带小数点。`open` 以后先都当小数，cli.rs 照命令行里给没给再定（算在不在线、闲没闲置照旧用 cfg 里的）
    pub fn reply_times(&mut self, ttl: Num, idle: Num) {
        (self.ttl_out, self.idle_out) = (ttl, idle);
    }

    /// 启动时说的那一句里的“设备几台”（Py:2255）
    pub fn machines(&self) -> usize {
        self.machines.len()
    }

    /// 请求里的 token（邀请里的令牌）现在的样子（Py:715-725）。token 是请求里原样的那个值：不是字符串、不是令牌的样子都是 Missing
    pub fn invite_state(&mut self, token: &V, now: f64) -> Invite {
        let Some(token) = token.as_str().filter(|t| wire::admit_re().matches(t)) else {
            return Invite::Missing;
        };
        let h = hash::sha256_hex(token);
        match self.invites.get(&h) {
            Some(inv) if now < inv.exp.f() => Invite::Ok,
            Some(_) => Invite::Expired,
            None if self.spent.contains(&h) => Invite::Used,
            None => Invite::Missing,
        }
    }

    /// 记下这台设备刚发过一个验过签的请求（只在内存里：不为它每个请求都重写状态文件）（Py:1969-1970）
    pub fn seen(&mut self, fp: &str, now: f64) {
        self.seen.insert(fp.to_string(), now);
    }

    /// 签名、时间、nonce 都验过了的请求里归这里管的那些 op：report、view、keys、ledger 和 Team 的各个 op（Py:1971-1997、
    /// 1999-2026）。返回 (状态码, 回的内容)；请求里的各项不合格是 (400, {"error": "bad"})。Err 只有存不了的时候：
    /// 调用方在日志里说一句 `cca-presence: 处理请求出错：…`、不回就断开（Py:2095-2096）
    pub fn op(&mut self, op: &str, fp: &str, j: &V, now: f64) -> R<(u16, V)> {
        let Some(j) = j.as_dict() else {
            return Ok(bad());
        };
        if op == "team_join" {
            return self.join_op(fp, &j, now);
        }
        if TEAM_OPS.contains(&op) {
            return self.team_op(op, fp, &j, now);
        }
        if op == "report" {
            // end 只能是没有、none（正常结束：这时不该还报着会话）、down（Py:1981-1983）
            let end = match j.get("end") {
                None | Some(V::None) => None,
                Some(V::Str(s)) if matches!(&*s, "none" | "down") => Some(s),
                Some(_) => return Ok(bad()),
            };
            let Some(units) = j.get("units").as_ref().and_then(wire::parse_units) else {
                return Ok(bad());
            };
            if end.as_deref() == Some("none") && !units.is_empty() {
                return Ok(bad());
            }
            self.report(fp, &units, end.as_deref(), now)?;
            let (ttl, idle) = (self.ttl_out.v(), self.idle_out.v());
            return Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "ttl" => ttl, "idle" => idle })));
        }
        if let Some((_, limit)) = SYNC_OPS.iter().find(|(name, _)| *name == op) {
            let Some(sync) = wire::parse_sync(&j, *limit) else {
                return Ok(bad());
            };
            return if op == "keys" { self.key_sync(fp, &sync.put, &sync.get, now) } else { self.cost_sync(fp, &sync, now) };
        }
        if op != "view" {
            return Ok(bad()); // 到不了：别的 op 调用方已经回了 404，中转的不归这里
        }
        // view：要看哪些账号（重复的只留头一个）（Py:1994-1997）
        let Some(asked) = j.list("accounts").filter(|l| l.len() <= MAX_ACCOUNTS) else {
            return Ok(bad());
        };
        let mut aids: Vec<String> = Vec::new();
        for a in asked.items() {
            let Some(a) = a.as_str().filter(|a| wire::aid_re().matches(a)) else {
                return Ok(bad());
            };
            if !aids.iter().any(|x| x == a) {
                aids.push(a.to_string());
            }
        }
        Ok((200, self.view(fp, &aids, now)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(in crate::server) mod tests;
