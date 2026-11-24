//! 远程会话的中转记着的东西，和设备发来的中转的请求（RELAY_OPS）。Python 的 `Remote`（Py:952-1230）。
//!
//! - edges（remote.json，移除设备、180 天没用过才删）：`{配对编号: {"a", "b", "at", "used"}}`，服务器只知道是哪两台设备配了对
//! - guests（也在 remote.json 里）：访客（手机上的网页端，不在名单里）的公钥
//! - 只在内存里的：slots（正在进行的配对）、tickets（进数据端口的票据）、sessions（开着的会话）
//!
//! Python 里是一把可重入的锁加一个条件变量；这里是 `Shared`：`Mutex<Remote>` + `Condvar`。pair_get 要等对方放密文进来：
//! 等的时候放开锁（`cv.wait_timeout`），同时等着的最多 `PAIR_WAITERS` 个（多了的不等，照现在的回）；配对的状态每变一次
//! `notify_all`。锁的先后：拿着 Remote 的锁可以再拿 Store 的（`name_of` 就是），反过来不行。
//!
//! 写出去的 remote.json 要和 Python 写的逐字节一样（两种服务器可以换着跑同一个目录）。Python 把读进来的字典原样留着、原样写回去，
//! 所以这里各项的先后（`OMap`、`Edge::fields`）、数字原来是整数还是小数（`Num`）、不认识的键（`Val::Json`）都记着。
//!
//! 分在几个文件里：这里是记着的东西、读和存、名单变了怎么清、访客的公钥、票据；ops.rs 是各个 op（`handle_op`）。
//! 数据端口那条线程（relay.rs）经 `Shared` 看会话、用票据：它用得着的几项是 `pub(super)` 的。
//!
//! 和 Python 不一样的两处（都只在手改坏了的 remote.json 上碰得到）：一个配对的 at、used 不是数字（是字符串、列表……）时，Python 每个
//! 中转的请求都出错、不回就断开，这里当它是 0（很久没用过：下一个请求就把这个配对删掉）；嵌套一千层以上的文件 Python 起不来，这里
//! 当作读不了、从空的开始。

mod ops;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod replay;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests;

use super::limits::Limits;
use super::omap::OMap;
use super::wire::{self, Blob, Num};
use super::{sshsig, API, EDGE_IDLE, GUEST_OPS, TICKET_TTL};
use crate::py::fs::open_error_text;
use crate::py::json::{self, Fmt};
use crate::py::obj::{Dict, V};
use crate::py::{base64, Fail, R};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// 状态文件的写法：`ensure_ascii=False, separators=(',', ':')`（Py:420）
const COMPACT: Fmt = Fmt::new().utf8().compact();

/// remote.json 里照原样留着的一个值：读进来什么样，写回去还是什么样
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Str(String),
    /// 数字：记着原来是整数还是小数（服务器自己记的时间都是小数）
    Num(Num),
    /// 别的（null、布尔值、列表、字典、放不进 i64 的整数）：紧凑的 JSON 文字，写回去时再解析
    Json(String),
}

impl Val {
    fn of(v: &V) -> Val {
        match v {
            V::Str(s) => Val::Str(s.to_string()),
            V::Int(i) => Val::Num(Num::Int(*i)),
            V::Float(f) => Val::Num(Num::Float(*f)),
            other => Val::Json(json::dumps(other, COMPACT)),
        }
    }

    fn null() -> Val {
        Val::Json("null".into())
    }

    fn v(&self) -> V {
        match self {
            Val::Str(s) => V::str(s),
            Val::Num(n) => n.v(),
            Val::Json(text) => json::loads(text).unwrap_or(V::None), // 是 dumps 写出来的，读得回来
        }
    }

    /// `x or 0` 当数字用（Py:1059、1212）。不是数字的当 0：见文件开头说的不一样的地方
    fn secs(&self) -> f64 {
        match self {
            Val::Num(n) => n.f(),
            Val::Str(_) => 0.0,
            // true 在 Python 里是 1；放不进 i64 的整数照它的值；null、false、列表、字典是 0
            Val::Json(text) if text == "true" => 1.0,
            Val::Json(text) => match json::loads(text) {
                Ok(v @ V::Big(_)) => v.as_num().unwrap_or(0.0),
                _ => 0.0,
            },
        }
    }
}

/// 一个配对：哪两台设备（Py:954-955）
#[derive(Clone, Debug)]
pub(super) struct Edge {
    /// 显示配对码的设备的指纹
    pub(super) a: String,
    /// 输入配对码的设备的指纹
    pub(super) b: String,
    /// 这一项在文件里的各个键（a、b 也在里面），照先后。服务器自己记的是 a、b、at、used；读进来的还带着别的键也留着
    fields: Vec<(String, Val)>,
}

impl Edge {
    /// 刚配好的（Py:1174）
    fn new(a: &str, b: &str, now: f64) -> Edge {
        let at = || Val::Num(Num::Float(now));
        Edge {
            a: a.to_string(),
            b: b.to_string(),
            fields: vec![
                ("a".into(), Val::Str(a.to_string())),
                ("b".into(), Val::Str(b.to_string())),
                ("at".into(), at()),
                ("used".into(), at()),
            ],
        }
    }

    /// 文件里读进来的：a、b 都是指纹的样子才要（Py:965-967）
    fn loaded(d: &Dict) -> Option<Edge> {
        let (a, b) = (d.str("a")?, d.str("b")?);
        if !(wire::fp_re().matches(&a) && wire::fp_re().matches(&b)) {
            return None;
        }
        let fields = d.items().iter().map(|(k, v)| (k.to_string(), Val::of(v))).collect();
        Some(Edge { a: a.to_string(), b: b.to_string(), fields })
    }

    /// `e.get(k) or 0`
    fn secs(&self, k: &str) -> f64 {
        self.fields.iter().find(|(name, _)| name == k).map_or(0.0, |(_, v)| v.secs())
    }

    /// 最后一次有动静：`max(e.get('used') or 0, e.get('at') or 0)`（Py:1059；写成 Python 的 max 那样比，不是数的时候结果也一样）
    fn last(&self) -> f64 {
        let (used, at) = (self.secs("used"), self.secs("at"));
        if at > used {
            at
        } else {
            used
        }
    }

    /// `e['used'] = now`：已有的键留在原来的位置，没有就加在最后
    fn set_used(&mut self, now: f64) {
        let v = Val::Num(Num::Float(now));
        match self.fields.iter_mut().find(|(name, _)| name == "used") {
            Some(slot) => slot.1 = v,
            None => self.fields.push(("used".into(), v)),
        }
    }

    /// `fp in (e['a'], e['b'])`
    pub(super) fn has(&self, fp: &str) -> bool {
        self.a == fp || self.b == fp
    }

    /// `{e['a'], e['b']} == {x, y}`（x、y 是两台不同的设备）
    fn joins(&self, x: &str, y: &str) -> bool {
        (self.a == x && self.b == y) || (self.a == y && self.b == x)
    }

    fn to_v(&self) -> V {
        let d = Dict::new();
        for (k, v) in &self.fields {
            d.set(k.as_str(), v.v());
        }
        V::Dict(d)
    }
}

/// 访客（Py:980-983）：手机上的网页端不是 Team 里的设备、不在名单里；它拿着一台设备显示的配对码配上对，服务器才记下它的公钥
#[derive(Clone, Debug)]
struct Guest {
    /// 公钥那一行（验它的签名用）
    key: String,
    /// 什么时候记下的（没人用它，原样留着）
    at: Val,
}

/// 正在进行的一次配对（Py:1110-1111）
#[derive(Clone, Debug)]
struct Slot {
    /// 这一次配对的号：同一个 rid 过期以后可以再开一次，等着的 pair_get 靠它认出“已经不是我等的那一次了”
    serial: u64,
    /// 显示配对码的设备
    owner: String,
    /// 输入配对码的设备：头一个放密文进来的别的设备
    joiner: Option<String>,
    /// 两边各放的一份密文：指纹 → 密文
    msgs: HashMap<String, Blob>,
    /// 两边各报的配对编号：指纹 → 配对编号
    commit: HashMap<String, String>,
    /// 配对记下了
    done: bool,
    exp: f64,
    /// 开给网页端的：输入配对码的可以是不在名单里的
    web: bool,
    /// 输入配对码的是访客时它的公钥那一行（配成了才记进 guests）
    gkey: Option<String>,
}

impl Slot {
    /// `fp in (s['owner'], s['joiner'])`
    fn party(&self, fp: &str) -> bool {
        self.owner == fp || self.joiner.as_deref() == Some(fp)
    }

    /// 对方是谁、它放的那一份密文（Py:1139-1141）
    fn theirs(&self, fp: &str) -> (Option<&str>, Option<&Blob>) {
        let peer = if fp == self.owner { self.joiner.as_deref() } else { Some(self.owner.as_str()) };
        (peer, peer.and_then(|p| self.msgs.get(p)))
    }
}

/// 票据是给谁的：会话的控制连接，还是接入方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Host,
    View,
}

#[cfg(test)]
impl Role {
    /// 开场白里的写法（对照 Python 的记录时用）
    pub(super) fn name(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::View => "view",
        }
    }
}

/// 进数据端口的票据（Py:1065-1070）：30 秒内有效、只能用一次
#[derive(Clone, Debug)]
pub(super) struct Ticket {
    pub(super) role: Role,
    /// 换这张票据的设备
    pub(super) fp: String,
    pub(super) sid: String,
    /// 宿主的票据：这个会话给哪几个配对看
    pub(super) pids: Vec<String>,
    /// 接入方的票据：是哪个配对的
    pub(super) pid: Option<String>,
    /// 票据的密钥：开场白里要证明有它
    pub(super) key: Vec<u8>,
    exp: f64,
    /// 已经有一条 WebSocket 拿它进来过了
    ws: bool,
}

/// 开着的会话（Py:1359-1360）
#[derive(Clone, Debug)]
pub(super) struct Session {
    /// 宿主那台设备
    pub(super) fp: String,
    /// 给哪几个配对看
    pub(super) pids: Vec<String>,
    /// 各个配对一份说明的密文
    pub(super) blobs: OMap<Blob>,
    /// 控制连接（数据端口那边连接的编号）；断了是 None
    pub(super) conn: Option<u64>,
    /// 控制连接什么时候断的（单调时钟）
    pub(super) lost: Option<f64>,
    /// 正接着几处
    pub(super) viewers: usize,
    /// 什么时候开的（服务器的时间）
    pub(super) since: f64,
}

pub struct Remote {
    /// remote.json
    path: PathBuf,
    /// 客户端连数据端口用的端口（--relay-port，默认是监听的那个）：票据里回给设备、/v1/health 里也有
    port: u16,
    /// 网页端的地址（开着网页端时）：给了才让设备开“给网页端的配对”、才有访客
    web: Option<String>,
    pub(super) lim: Limits,
    /// remote.json 的 v：读进来什么样留什么样（没有就是 1）
    v: Val,
    edges: OMap<Edge>,
    guests: OMap<Guest>,
    /// remote.json 里别的不认识的键，照先后（写回去时排在 v、edges、guests 后面）
    extra: Vec<(String, Val)>,
    /// 正在进行的配对：rid → 配对
    slots: HashMap<String, Slot>,
    /// 下一次配对的号
    serial: u64,
    /// pair_get 正等着的那几次配对的号（一个等着的一项）
    waiting: Vec<u64>,
    /// 有 pair_get 等着的时候被拿掉的配对最后的样子：Python 里等的人手里还拿着那个字典，醒来照它回（Py:1150-1152）
    closed: Vec<Slot>,
    /// 进数据端口的票据：tid → 票据
    tickets: HashMap<String, Ticket>,
    /// 开着的会话：sid → 会话，照开的先后（relay_list 照它列）
    pub(super) sessions: OMap<Session>,
}

/// 几个线程共用的那一份：处理请求的线程、中转的线程都经它
pub struct Shared {
    pub m: Mutex<Remote>,
    /// 配对的状态变了（对方放了密文、报了配对编号、不做了）
    pub cv: Condvar,
    /// 正等在 pair_get 里的请求有几个
    pub waiters: AtomicUsize,
}

impl Shared {
    pub fn new(remote: Remote) -> Shared {
        Shared { m: Mutex::new(remote), cv: Condvar::new(), waiters: AtomicUsize::new(0) }
    }

    /// 拿锁。哪个线程拿着锁 panic 过也照样拿（发布版里 panic 就是整个进程退出，到不了这里）
    pub(super) fn lock(&self) -> MutexGuard<'_, Remote> {
        self.m.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 读状态文件（Py:402-413）：没有、读不了、格式不对、keys 里的哪一项不是字典都是 None；读不了的交给 say 说一声
fn load_doc(path: &Path, keys: &[&str], say: &mut dyn FnMut(String)) -> Option<Dict> {
    let shown = path.to_string_lossy();
    let mut cannot = |why: String| say(format!("cca-presence: {shown} 读不了（{why}），从空的开始"));
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            cannot(open_error_text(&e, &shown));
            return None;
        }
    };
    match json::loads_file_bytes(&raw) {
        Ok(v) => v.as_dict().filter(|d| keys.iter().all(|k| d.dict(k).is_some())),
        Err(e) => {
            cannot(e.to_string());
            None
        }
    }
}

/// 存状态文件（Py:416-421）：写到 `路径.进程号.tmp`（新建的是 600），再换过去
fn save_doc(path: &Path, data: &V) -> R<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let text = json::dumps(data, COMPACT);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Remote {
    /// 读 remote.json、筛掉样子不对的配对和访客（Py:958-973）；没有、读不了从空的开始（读不了的在日志里说一声）
    pub fn open(path: &Path, port: u16, web: Option<String>, lim: Limits) -> Remote {
        Remote::open_saying(path, port, web, lim, &mut |line| crate::err!("{line}"))
    }

    /// say：日志里要说的那一句交给谁（测试里收起来比）
    fn open_saying(path: &Path, port: u16, web: Option<String>, lim: Limits, say: &mut dyn FnMut(String)) -> Remote {
        let mut r = Remote {
            path: path.to_path_buf(),
            port,
            web,
            lim,
            v: Val::Num(Num::Int(API)),
            edges: OMap::new(),
            guests: OMap::new(),
            extra: Vec::new(),
            slots: HashMap::new(),
            serial: 0,
            waiting: Vec::new(),
            closed: Vec::new(),
            tickets: HashMap::new(),
            sessions: OMap::new(),
        };
        let Some(doc) = load_doc(path, &["edges"], say) else { return r };
        for (k, v) in doc.items() {
            match (&*k, &v) {
                ("v", _) => r.v = Val::of(&v),
                // 配对编号是 32 个十六进制字符、两头都是指纹的样子的才要
                ("edges", V::Dict(d)) => {
                    r.edges = OMap::from_dict(d, |pid, e| Edge::loaded(&e.as_dict()?).filter(|_| wire::aid_re().matches(pid)));
                }
                // 指纹和公钥对得上的才要；别的键不留（Py:969-971）
                ("guests", g) => {
                    let known = |fp: &str, x: &V| {
                        let x = x.as_dict()?;
                        let key = x.str("key")?;
                        let blob = base64::b64decode(wire::key_re().match_(&key)?.g(1), false).ok()?;
                        (wire::fp_re().matches(fp) && sshsig::key_fp(&blob) == fp)
                            .then(|| Guest { key: key.to_string(), at: x.get("at").map_or_else(Val::null, |at| Val::of(&at)) })
                    };
                    r.guests = g.as_dict().map_or_else(OMap::new, |g| OMap::from_dict(&g, known));
                }
                ("edges", _) => {} // load_doc 查过是字典
                _ => r.extra.push((k.to_string(), Val::of(&v))),
            }
        }
        r.prune_guests();
        r
    }

    /// 客户端连数据端口用的端口
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 记着几个配对（启动时说的那一句里用，Py:2257）
    pub fn edges(&self) -> usize {
        self.edges.len()
    }

    /// 网页端的地址；没开（没给、是空的）是 None
    fn web(&self) -> Option<&str> {
        self.web.as_deref().filter(|w| !w.is_empty())
    }

    /// 存 remote.json（Py:1019-1020）：v、edges、guests，然后是别的不认识的键
    fn save(&self) -> R<()> {
        let doc = Dict::new();
        doc.set("v", self.v.v());
        doc.set("edges", self.edges.to_v(Edge::to_v));
        doc.set("guests", self.guests.to_v(|g| V::Dict(crate::dict! { "key" => &g.key, "at" => g.at.v() })));
        for (k, v) in &self.extra {
            doc.set(k.as_str(), v.v());
        }
        save_doc(&self.path, &V::Dict(doc))
    }

    /// 一个配对都不剩的访客不留（它的票据也作废）（Py:985-995）
    fn prune_guests(&mut self) {
        let edges = &self.edges;
        let gone: Vec<String> = self.guests.keys().filter(|fp| !edges.values().any(|e| e.has(fp))).map(str::to_string).collect();
        for fp in &gone {
            self.guests.remove(fp);
        }
        if !gone.is_empty() {
            self.tickets.retain(|_, k| !gone.contains(&k.fp));
        }
    }

    /// 不在名单里的这把密钥能不能发这个请求：能的话是验它签名用的公钥那一行，不能是 None（Py:997-1017）。两种：已经配了对的访客；
    /// 正拿着配对码来的（那次配对是开给网页端的、还没有别的设备放过密文，公钥在请求的 pub 里、指纹要对得上）。
    /// 知道 rid 就是知道配对码：不知道的人到不了验签名那一步
    pub fn guest_key(&self, fp: &str, op: &str, j: &V, now: f64) -> Option<String> {
        // 没开网页端（关掉了）：访客的请求一律不认；记录留着，再打开时还在
        if !GUEST_OPS.contains(&op) || self.web().is_none() {
            return None;
        }
        if let Some(g) = self.guests.get(fp) {
            return Some(g.key.clone());
        }
        let j = j.as_dict()?;
        let s = self.slots.get(&*j.str("rid")?).filter(|s| s.web && s.exp >= now)?;
        if s.joiner.as_deref() == Some(fp) {
            return s.gkey.clone();
        }
        let b64 = j.str("pub").and_then(|line| Some(wire::key_re().match_(&line)?.g(1).to_string()))?;
        let blob = base64::b64decode(&b64, false).ok()?;
        (op == "pair_put" && s.joiner.is_none() && sshsig::key_fp(&blob) == fp).then(|| format!("ssh-ed25519 {b64}"))
    }

    /// `pid` 这个配对里有没有这台设备（Py:1022-1024）
    fn member(&self, pid: &str, fp: &str) -> bool {
        self.edges.get(pid).is_some_and(|e| e.has(fp))
    }

    /// 配对的另一头（Py:1026-1028）
    fn other(&self, pid: &str, fp: &str) -> Option<&str> {
        let e = self.edges.get(pid)?;
        Some(if e.a == fp { e.b.as_str() } else { e.a.as_str() })
    }

    /// 还有没有这个配对（数据端口那边看：配对没了，正接着的断开）
    pub(super) fn has_edge(&self, pid: &str) -> bool {
        self.edges.contains(pid)
    }

    /// 名单变了（管理员登记、移除了设备，设备退出了）：不在名单里的设备的配对、票据、会话都删掉（它的控制连接由数据端口那边发现会话
    /// 没了时断开）；访客不在名单里，领它进来的设备没了，它的配对跟着没有，它也就删掉了（Py:1030-1042）。有删掉的配对要存，存不了是 Err。
    /// now 现在用不着（Python 的这一个不看时间）
    pub fn keep_only(&mut self, fps: &[String], now: f64) -> R<()> {
        let _ = now;
        let listed = |fp: &str| fps.iter().any(|f| f == fp);
        let guests = &self.guests;
        let ok = |fp: &str| listed(fp) || guests.contains(fp);
        let gone: Vec<String> = self.edges.iter().filter(|(_, e)| !ok(&e.a) || !ok(&e.b)).map(|(p, _)| p.to_string()).collect();
        for p in &gone {
            self.drop_edge(p);
        }
        let guests = &self.guests;
        self.tickets.retain(|_, k| listed(&k.fp) || guests.contains(&k.fp));
        self.sessions.retain(|_, x| listed(&x.fp));
        if !gone.is_empty() {
            self.save()?;
        }
        Ok(())
    }

    /// 删一个配对：各个会话不再给它看，访客一个配对都不剩的也删掉（Py:1044-1051）
    fn drop_edge(&mut self, pid: &str) {
        self.edges.remove(pid);
        for x in self.sessions.values_mut() {
            if x.pids.iter().any(|p| p == pid) {
                x.pids.retain(|p| p != pid);
                x.blobs.remove(pid);
            }
        }
        self.prune_guests();
    }

    /// 拿掉一次正在进行的配对。有 pair_get 正等着它的话留下它最后的样子：等的人醒来照它回
    fn slot_pop(&mut self, rid: &str) {
        if let Some(s) = self.slots.remove(rid) {
            if self.waiting.contains(&s.serial) {
                self.closed.push(s);
            }
        }
    }

    /// 过期的配对、票据，很久没用的配对（Py:1053-1063）
    fn sweep(&mut self, now: f64) -> R<()> {
        let stale: Vec<String> = self.slots.iter().filter(|(_, s)| s.exp < now).map(|(rid, _)| rid.clone()).collect();
        for rid in &stale {
            self.slot_pop(rid);
        }
        self.tickets.retain(|_, k| k.exp >= now);
        let old: Vec<String> =
            self.edges.iter().filter(|(_, e)| e.last() < now - EDGE_IDLE as f64).map(|(p, _)| p.to_string()).collect();
        for p in &old {
            self.drop_edge(p);
        }
        if !old.is_empty() {
            self.save()?;
        }
        Ok(())
    }

    /// 给这台设备一张票据（Py:1065-1070）；它手里没用掉的太多了是 429。取不到随机数是 Err
    fn ticket(&mut self, now: f64, role: Role, fp: &str, sid: String, pids: Vec<String>, pid: Option<String>) -> R<(u16, V)> {
        if self.tickets.values().filter(|k| k.fp == fp).count() >= self.lim.tickets_dev {
            return Ok((429, V::Dict(crate::dict! { "error" => "rate" })));
        }
        let (tid, key) = (random(16)?, random(32)?);
        let tid = crate::py::hash::to_hex(&tid);
        let reply = crate::dict! {
            "v" => API, "ok" => true, "tid" => &tid, "tkey" => base64::b64encode(&key), "port" => self.port, "ttl" => TICKET_TTL,
        };
        let exp = now + TICKET_TTL as f64;
        self.tickets.insert(tid, Ticket { role, fp: fp.to_string(), sid, pids, pid, key, exp, ws: false });
        Ok((200, V::Dict(reply)))
    }

    /// 网页端要开一条 WebSocket：它说的票据在不在（接入用的、没过期、还没有别的 WebSocket 拿它进来过）；在就记下已经有人拿它进来了
    /// （Py:1072-1080）。没有凭证的人拿不到票据，也就占不了 WebSocket 的名额；票据本身照旧要在开场白里证明有它的密钥才算用掉
    pub fn ws_claim(&mut self, tid: &str, now: f64) -> bool {
        match self.tickets.get_mut(tid) {
            Some(k) if k.role == Role::View && k.exp >= now && !k.ws => {
                k.ws = true;
                true
            }
            _ => false,
        }
    }

    /// 用掉一张票据（Py:1082-1088）。proof(票据的密钥) 核对来的人确实有这把密钥：对不上的不算用掉（光知道编号的人废不了别人的票据）
    pub(super) fn take_ticket(&mut self, tid: &str, role: Role, now: f64, proof: impl FnOnce(&[u8]) -> bool) -> Option<Ticket> {
        let k = self.tickets.get(tid)?;
        if k.role != role || k.exp < now || !proof(&k.key) {
            return None;
        }
        self.tickets.remove(tid)
    }
}

#[cfg(test)]
impl Remote {
    /// 测试里放一个开着的会话进来（平常是宿主在数据端口上连上来时建的）：控制连接算连着，还没有说明
    pub(super) fn test_session(&mut self, sid: &str, fp: &str, pids: &[&str], since: f64) {
        let pids = pids.iter().map(|p| p.to_string()).collect();
        self.sessions
            .insert(sid, Session { fp: fp.to_string(), pids, blobs: OMap::new(), conn: Some(0), lost: None, viewers: 0, since });
    }

    /// 测试里放一个配对进来（不经配对的那几步）
    pub(super) fn test_edge(&mut self, pid: &str, a: &str, b: &str, now: f64) {
        self.edges.insert(pid, Edge::new(a, b, now));
    }

    /// 测试里放一张编号、密钥都定好的宿主的票据进来（平常两样都是随机的）
    pub(super) fn test_ticket(&mut self, tid: &str, key: &[u8], fp: &str, sid: &str, pids: &[&str], exp: f64) {
        let pids = pids.iter().map(|p| p.to_string()).collect();
        let k = Ticket {
            role: Role::Host,
            fp: fp.to_string(),
            sid: sid.to_string(),
            pids,
            pid: None,
            key: key.to_vec(),
            exp,
            ws: false,
        };
        self.tickets.insert(tid.to_string(), k);
    }
}

/// 一个中转的请求（op 是 RELAY_OPS 里的；不认识的回 404 not_found）：状态码和回的内容（Py:1090-1230）。签名、时间、nonce 由处理请求的
/// 那一头验过了才到这里。fp：发请求的设备的指纹；j：请求的内容；now：服务器的时间；gkey：发请求的是访客（不在名单里）时，验它签名
/// 用的公钥那一行（`guest_key` 给的），名单里的设备是 None；name_of：名单里这台设备的名字（没有是 None；拿着 Remote 的锁调它）。
/// 要等的只有 pair_get（最多 PAIR_WAIT 秒，等的时候不占着锁）。Err：状态文件存不了、取不到随机数——Python 那边是没人接的异常，
/// 这个请求不回就断开
pub fn handle_op(
    sh: &Shared,
    op: &str,
    fp: &str,
    j: &V,
    now: f64,
    gkey: Option<&str>,
    name_of: &dyn Fn(&str) -> Option<String>,
) -> R<(u16, V)> {
    ops::handle(sh, op, fp, j, now, gkey, name_of)
}

/// `os.urandom(n)`。取不到（系统的随机源用不了）是 Err：Python 那边是没人接的 OSError，这个请求不回就断开
pub(super) fn random(n: usize) -> R<Vec<u8>> {
    let mut buf = vec![0u8; n];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut buf)
        .map_err(|_| Fail::Os(std::io::Error::other("取不到系统的随机数")))?;
    Ok(buf)
}
