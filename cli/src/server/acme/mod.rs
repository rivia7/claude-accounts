//! 证书助手（`cca-core team-acme`）：给远程会话的网页端申请、续上浏览器信得过的证书。照原来 Python 的 server/cca-acme.py
//! 移过来的（那一份不在仓库里了：见 server/mod.rs 开头）。
//!
//! 照 ACME（RFC 8555）向 Let's Encrypt 要证书——给 IP 地址签的（RFC 8738，只有约 6 天的短期证书这一种）或者给域名签的——放到一个
//! 目录里（fullchain.pem、privkey.pem），到期之前自己续上；服务端发现文件换了会自己重新读（`tls::CertCtx`）。验证只用 http-01：
//! 证书机构从公网连这台机器的 80 端口取 `/.well-known/acme-challenge/<令牌>`。
//!
//! Python 那一份靠系统的 openssl 命令生成密钥、签名、做证书请求、看证书；Rust 版不起外部命令：密钥和签名用 ring（key.rs），
//! 证书请求自己拼 DER（csr.rs），看证书用 `x509::cert_info`。
//!
//! - `identifier`：命令行、--teams 目录里写的一个地址 → 要签的是哪一种（这里）
//! - key.rs：P-256 的私钥（账户的、证书的），JWK 和它的摘要，两种签名
//! - csr.rs：证书请求
//! - client.rs：和证书机构说话——HTTPS、JWS、nonce、账户、下单、等验证、取证书
//! - respond.rs：http-01 的应答——自己听一个端口，或者把文件写进别的网站服务的目录
//! - fetch.rs、lines.rs、roots.rs：连证书机构用的那一点 HTTP 客户端、从连接上有上限地读、信哪些根证书
//! - der.rs、files.rs：拼 DER、读写文件的那几样
//! - args.rs：命令行参数
//! - 这里还有：手里的证书用不用得了、什么时候该续（`cert_state`、`usable`），要一张换上（`renew`），两种运行的方式——
//!   `serve_one`（--id：一张证书）和 `serve_teams`（--teams：几个 Team 共用，一个地址一张）——和入口（Acme:369-611）
//!
//! 和 Python 不一样的地方各个文件开头写着；这里的：
//! - 证书的起止时间按 UTC 算准（Python 用 `mktime`，夏令时的地方会差一个小时）
//! - --listen 的端口不在 0 到 65535 里、端口文件写不了：说“起不来”、退出码 2（Python 是没人接的异常，退出码 1）
//! - 参数不对时的用法那一行、-h 的帮助不是 argparse 的原样；不认缩写（args.rs）
//! - Ctrl-C（SIGINT）照系统默认的办法结束（Python 是接住、退出码 130；在 shell 里看到的退出码一样是 130）

pub mod args;
pub mod client;
pub mod csr;
pub mod der;
pub mod fetch;
pub mod files;
pub mod key;
pub mod lines;
pub mod respond;
pub mod roots;

use self::args::{Args, Parsed};
use self::client::Acme;
use self::files::{makedirs, put_file, read, read_capped, strerror};
use self::key::Key;
use self::respond::{Challenges, Responder, Webroot};
use super::x509::{cert_info_pem, San};
use crate::py::fs::open_error_text;
use crate::py::ipaddr::ip_address;
use crate::py::obj::repr_str;
use crate::py::re::Re;
use crate::py::{json, path, text, time, R};
use crate::re;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::os::unix::fs::MetadataExt;

/// 证书机构的 ACME 目录默认是 Let's Encrypt（Acme:50）
pub const LETSENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// 请求里的 User-Agent（Acme:51）
pub const AGENT: &str = "cca-acme/1";
/// 没成以后隔多少秒再试（最后一个一直用）（Acme:55）
pub const RETRY: [u64; 4] = [60, 600, 3600, 6 * 3600];
/// 等证书机构验证、签发最多多少秒（Acme:56）
pub const POLL: u64 = 90;
/// --teams：等着的时候隔几秒看一眼目录变了没有（Acme:57）
pub const LOOK: u64 = 5;

/// http-01 的令牌（Acme:52）。`\Z`：`$` 会放过结尾的换行（令牌要当文件名用：--webroot）
pub fn token_re() -> &'static Re {
    re!(r"^[A-Za-z0-9_-]{1,256}\Z")
}

/// 域名（Acme:53）。原来开头还有一个 `(?=.{1,253}$)`（整个名字最长 253 个字符）：这里的正则没有这种写法，长度在 `identifier` 里另查
pub fn name_re() -> &'static Re {
    re!(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$")
}

/// --teams 里的文件名：Team 的服务名（`cca-team-<编号的前 8 位>`）（Acme:54）
pub fn team_re() -> &'static Re {
    re!(r"^[a-z0-9][a-z0-9-]{0,62}\Z")
}

/// 没办成：原因是给人看的一句话（Python 的 `Problem`，Acme:60-61）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem(pub String);

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 没办成的两类。Python 里是两种异常，接它们的地方不一样，所以要分开带着：
/// - `Problem`：一句给人看的原因。要证书的那一轮接住它、记一次没成、过一阵再试（`except Problem`）
/// - `Os`：读写文件、监听端口这些没成（Python 的 OSError），里面是 `str(e)` 那句话（`[Errno 13] Permission denied: '/路径'`）。
///   --id 那种方式下它没人接、一路到入口：说一句“起不来”、退出码 2；--teams 那种方式下说一句“放不好”、一分钟后再来
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    Problem(String),
    Os(String),
}

impl From<Problem> for Trouble {
    fn from(p: Problem) -> Trouble {
        Trouble::Problem(p.0)
    }
}

impl std::fmt::Display for Trouble {
    /// `str(e)`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Trouble::Problem(why) | Trouble::Os(why) => f.write_str(why),
        }
    }
}

/// 命令行里写的一个地址 → 要签的是什么（ACME 的 identifier）：IP 地址写成规范的样子，别的当域名（小写、去掉结尾的点）（Acme:84-97）。
/// 证书机构只给公网的地址签；any_ip（--any-ip，测试时对着本机的假证书机构要签 127.0.0.1）放开这一条
pub fn identifier(given: &str, any_ip: bool) -> Result<San, Problem> {
    let Ok(ip) = ip_address(text::strip_chars(given, "[]")) else {
        let name = given.to_lowercase();
        let name = text::rstrip_chars(&name, ".");
        // 原来那个 (?=.{1,253}$)：$ 也认结尾那个换行之前，所以量的是去掉它以后的长度
        let len = text::len(name.strip_suffix('\n').unwrap_or(name));
        if !(name_re().matches(name) && (1..=253).contains(&len)) {
            return Err(Problem(format!("{given} 不是 IP 地址，也不是域名")));
        }
        return Ok(San::Dns(name.to_string()));
    };
    let a = &ip.ip;
    if (a.is_private() || a.is_loopback() || a.is_link_local() || a.is_multicast() || a.is_unspecified() || a.is_reserved())
        && !any_ip
    {
        return Err(Problem(format!("{} 不是公网的地址：证书机构只给从公网连得上的地址签证书", ip.text())));
    }
    Ok(San::Ip(ip.text()))
}

/// 一个目录里证书、私钥的文件名（服务端的 --web-cert / --web-key 指的就是它们）
const CHAIN: &str = "fullchain.pem";
const PRIVKEY: &str = "privkey.pem";
/// --teams 目录里的一个文件最多读开头这么多个字（Acme:466 的 `f.read(512)`）
const ADDRESS_MAX: usize = 512;

/// 现在几点、怎么等、话说到哪：平常是真的，测试里换掉
pub struct World {
    /// `time.time()`
    pub now: Box<dyn Fn() -> f64>,
    /// `time.sleep(秒)`
    pub sleep: Box<dyn Fn(f64)>,
    /// Python 的 `log`（Acme:64-66）：一行，写到 stderr，前面带 `cca-acme: `
    pub log: Box<dyn Fn(&str)>,
}

impl World {
    pub fn real() -> World {
        World { now: Box::new(time::time), sleep: Box::new(time::sleep), log: Box::new(|said| crate::err!("cca-acme: {said}")) }
    }
}

/// 时间写成给人看的（Acme:127-128）
fn when(t: f64) -> String {
    time::strftime_utc("%Y-%m-%d %H:%M UTC", t)
}

/// 几个地址写成一串
fn names_of(ids: &[San]) -> String {
    ids.iter().map(San::value).collect::<Vec<_>>().join("、")
}

/// 没成以后隔多少秒再试：第 fails 次没成（从 1 数起），照 `RETRY` 一级级加长，最后一个一直用
fn retry_after(fails: usize) -> u64 {
    RETRY.get(fails.min(RETRY.len()).saturating_sub(1)).copied().unwrap_or(6 * 3600)
}

/// 手里的证书：(什么时候该续, 到期时间)；没有、看不懂、不是给这几个地址的、不是这把私钥的，是 None（Acme:369-391）。
/// fraction：过了有效期的这么多就续（10 天以内的证书 Let's Encrypt 建议的是一半）。文件里有几张证书时看的是头一张
pub fn cert_state(path: &str, ids: &[San], key: &Key, fraction: f64) -> Option<(f64, f64)> {
    let info = cert_info_pem(&read(path)?)?;
    // 比的是集合：先后、重复不管
    if info.san != ids.iter().cloned().collect::<BTreeSet<San>>() || info.spki != key.spki() {
        return None;
    }
    let (start, end) = (info.not_before as f64, info.not_after as f64);
    Some((start + (end - start) * fraction, end))
}

/// 这一对证书、私钥是给这几个地址的、还没过期：(什么时候该续, 到期时间)；不是（或者不在）是 None。私钥不在时不生成（Acme:397-403）
pub fn usable(chain: &str, keyfile: &str, ids: &[San], fraction: f64, now: f64) -> Option<(f64, f64)> {
    let key = Key::open(keyfile, false).ok()?;
    cert_state(chain, ids, &key, fraction).filter(|(_, end)| now < *end)
}

/// 一次运行里不变的那些
struct Job<'a> {
    a: &'a Args,
    /// 过了有效期的这么多就续
    fraction: f64,
    account_key: &'a Key,
    responder: &'a dyn Challenges,
    world: &'a World,
}

impl<'a> Job<'a> {
    fn log(&self, said: &str) {
        (self.world.log)(said);
    }

    fn now(&self) -> f64 {
        (self.world.now)()
    }

    /// `acme = acme or Acme(a.directory, account_key, a.ca)`：手里没有和证书机构的对话就开一段
    fn acme<'s>(&self, held: &'s mut Option<Acme<'a>>) -> Result<&'s mut Acme<'a>, Trouble> {
        if held.is_none() {
            *held = Some(Acme::new(&self.a.directory, self.account_key, self.a.ca.as_deref())?);
        }
        held.as_mut().ok_or_else(|| Trouble::Problem("连不上证书机构".to_string()))
    }

    /// 要一张新的、换上（fullchain.pem 别人读得了）：返回它的 (什么时候该续, 到期时间)（Acme:406-415）
    fn renew(&self, acme: &mut Acme<'_>, ids: &[San], chain: &str, cert_key: &Key) -> Result<(f64, f64), Trouble> {
        let pem = acme.issue(ids, &self.a.profile, cert_key, self.responder)?;
        put_file(chain, &pem, 0o644)?;
        let names = names_of(ids);
        let Some(got) = cert_state(chain, ids, cert_key, self.fraction) else {
            return Err(Problem(format!("证书机构给的证书不是给 {names} 的，或者不是这把私钥的")).into());
        };
        self.log(&format!("拿到 {names} 的证书，{} 到期", when(got.1)));
        Ok(got)
    }
}

/// --id：一张证书（上面是这几个地址），放在 --out 里（Acme:418-448）。返回退出码（只有 --once 才会返回）；
/// 读写文件没成（`Trouble::Os`）、证书的私钥读不了是 Err：入口说“起不来”
fn serve_one(job: &Job<'_>, ids: &[San]) -> Result<i32, Trouble> {
    let a = job.a;
    makedirs(&a.out, 0o700)?;
    let cert_key = Key::open(&path::join(&a.out, PRIVKEY), true)?;
    let chain = path::join(&a.out, CHAIN);
    let names = names_of(ids);
    let mut fails = 0usize;
    let mut acme: Option<Acme<'_>> = None;
    loop {
        let state = cert_state(&chain, ids, &cert_key, job.fraction);
        let now = job.now();
        if let Some((due, _)) = state.filter(|(due, _)| now < *due) {
            if a.once {
                return Ok(0);
            }
            (job.world.sleep)((due - now).clamp(60.0, 3600.0));
            continue;
        }
        let tried = job.acme(&mut acme).and_then(|acme| job.renew(acme, ids, &chain, &cert_key));
        match tried {
            Ok(_) => {
                fails = 0;
                if a.once {
                    return Ok(0);
                }
            }
            Err(Trouble::Problem(why)) => {
                fails = fails.saturating_add(1);
                let pause = retry_after(fails);
                let left = match state.filter(|(_, end)| now < *end) {
                    Some((_, end)) => format!("，手里的那张 {} 到期", when(end)),
                    None => "，手里没有用得了的证书".to_string(),
                };
                job.log(&format!("{names} 的证书没要到：{why}{left}。{} 分钟后再试", pause / 60));
                acme = None; // 账户、nonce 从头来
                if a.once {
                    return Ok(1);
                }
                (job.world.sleep)(pause as f64);
            }
            Err(os) => return Err(os),
        }
    }
}

/// --teams 时的一个地址：它的证书、私钥存在 --store/<种类>-<地址>/（IPv6 的冒号写成下划线），没成了几次、什么时候再试（Acme:451-457）
struct Slot {
    ident: San,
    dir: String,
    chain: String,
    key: String,
    fails: usize,
    retry: f64,
}

impl Slot {
    fn new(store: &str, ident: San) -> Slot {
        let dir = path::join(store, &format!("{}-{}", ident.kind(), ident.value().replace(':', "_")));
        let (chain, key) = (path::join(&dir, CHAIN), path::join(&dir, PRIVKEY));
        Slot { ident, dir, chain, key, fails: 0, retry: 0.0 }
    }
}

/// --teams 目录里一个文件登记的地址：`identifier(open(p, encoding='utf-8').read(512).strip())`。没成是那句原因（`str(e)`）
fn registered(p: &str, any_ip: bool) -> Result<San, String> {
    // 文本模式一次读 8192 个字节来解码：这么多里有不是 UTF-8 的就是错，哪怕在前 512 个字以后
    let raw = read_capped(p, 8192).map_err(|e| open_error_text(&e, p))?;
    let content = match std::str::from_utf8(&raw) {
        Ok(content) => content,
        Err(e) => {
            // 只是最后一个字被切成了半截、前面已经够 512 个字：不算错
            let whole = raw.get(..e.valid_up_to()).and_then(|good| std::str::from_utf8(good).ok()).unwrap_or("");
            if e.error_len().is_some() || text::len(whole) < ADDRESS_MAX {
                return Err(json::utf8_error_text(&raw, &e));
            }
            whole
        }
    };
    // 文本模式的通用换行
    let content = if content.contains('\r') { content.replace("\r\n", "\n").replace('\r', "\n") } else { content.to_string() };
    identifier(text::strip(text::head(&content, ADDRESS_MAX)), any_ip).map_err(|p| p.0)
}

/// --teams 目录里登记的：照名字排好的 (Team, 地址)。看不懂的文件说一次（said 记着说过的）、跳过（Acme:460-473）。
/// 目录读不了是 Err
fn registry(
    dir: &str,
    said: &mut HashMap<String, String>,
    any_ip: bool,
    log: &dyn Fn(&str),
) -> std::io::Result<Vec<(String, San)>> {
    let mut teams = Vec::new();
    for name in crate::py::fs::listdir_sorted(dir)? {
        let p = path::join(dir, &name);
        if !team_re().matches(&name) || !path::isfile(&p) {
            continue; // 安装脚本正写到一半的（.new.<进程号>）这些
        }
        match registered(&p, any_ip) {
            Ok(id) => {
                said.remove(&name);
                teams.push((name, id));
            }
            Err(why) => {
                if said.get(&name) != Some(&why) {
                    log(&format!("{name} 登记的地址用不了：{why}"));
                    said.insert(name, why);
                }
            }
        }
    }
    Ok(teams)
}

/// --teams 目录现在的样子（文件名、修改时间、大小）：变了就照新的来（Acme:476-481）。读不了（或者里面有哪一项看不了）是 None
fn stamp(dir: &str) -> Option<Vec<(String, i128, u64)>> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).ok()? {
        let name = entry.ok()?.file_name().to_string_lossy().into_owned();
        let meta = std::fs::metadata(path::join(dir, &name)).ok()?;
        let mtime_ns = i128::from(meta.mtime()).saturating_mul(1_000_000_000).saturating_add(i128::from(meta.mtime_nsec()));
        all.push((name, mtime_ns, meta.size()));
    }
    all.sort();
    Some(all)
}

/// 这个地址的证书、私钥放进一个 Team 的目录：不一样才换，私钥先换（服务端看到两个都换了才重新读）。目录不在（安装脚本还没建，
/// 或者已经去掉了这个 Team）返回 false（Acme:484-493）
fn give(dir: &str, slot: &Slot) -> Result<bool, Trouble> {
    if !path::isdir(dir) {
        return Ok(false);
    }
    for (name, mode) in [(PRIVKEY, 0o640), (CHAIN, 0o644)] {
        let Some(want) = read(&path::join(&slot.dir, name)) else { continue };
        let there = path::join(dir, name);
        if read(&there).as_ref() != Some(&want) {
            put_file(&there, &want, mode)?;
        }
    }
    Ok(true)
}

/// --teams 那种方式下一轮轮之间记着的
#[derive(Default)]
struct Teams<'a> {
    /// (种类, 地址) → 这个地址的那一份
    slots: HashMap<(String, String), Slot>,
    /// 哪个 Team 登记的地址看不懂、说过什么
    said: HashMap<String, String>,
    /// 说过“目录不在”的 Team
    missing: HashSet<String>,
    acme: Option<Acme<'a>>,
}

impl<'a> Teams<'a> {
    /// 看一轮：照目录里登记的 Team，每个地址一张证书，放进用它的那几个 Team 的目录（Acme:499-555 循环里的一遍）。
    /// 返回（是不是每个地址都有证书、都放进去了, 什么时候再看）
    fn round(&mut self, job: &Job<'a>) -> (bool, f64) {
        let dir = job.a.teams.as_deref().unwrap_or("");
        let teams = registry(dir, &mut self.said, job.a.any_ip, &*job.world.log).unwrap_or_else(|e| {
            job.log(&format!("读不了 {dir}（{}）", strerror(&e)));
            Vec::new()
        });
        // 地址 → 用它的那几个 Team；照（种类, 地址）排好了一个个来
        let mut users: BTreeMap<(String, String), (San, Vec<String>)> = BTreeMap::new();
        for (team, id) in teams {
            let user = users.entry((id.kind().to_string(), id.value().to_string())).or_insert_with(|| (id, Vec::new()));
            user.1.push(team);
        }
        let (mut wake, mut ready) = (job.now() + 3600.0, true);
        for (k, (id, names)) in users {
            let value = id.value().to_string();
            if let Err(e) = self.address(job, k, id, &names, &mut wake, &mut ready) {
                ready = false;
                wake = wake.min(job.now() + 60.0);
                job.log(&format!("{value} 的证书放不好：{e}"));
            }
        }
        (ready, wake)
    }

    /// 一轮里的一个地址。读写文件没成、连证书机构用的根证书读不了是 Err（那一轮说“放不好”，一分钟后再来）
    fn address(
        &mut self,
        job: &Job<'a>,
        k: (String, String),
        id: San,
        names: &[String],
        wake: &mut f64,
        ready: &mut bool,
    ) -> Result<(), Trouble> {
        let a = job.a;
        let store = a.store.as_deref().unwrap_or("");
        let s = self.slots.entry(k).or_insert_with(|| Slot::new(store, id));
        let ids = [s.ident.clone()];
        let value = s.ident.value().to_string();
        makedirs(&s.dir, 0o700)?;
        let mut got = usable(&s.chain, &s.key, &ids, job.fraction, job.now());
        if got.is_none() {
            // 手里没有用得了的：这几个 Team 的目录里有（以前它自己的证书助手要的，安装脚本搬过来的）就接着用，不再要一张
            for team in names {
                let d = path::join(&a.out, team);
                let (chain, key) = (path::join(&d, CHAIN), path::join(&d, PRIVKEY));
                let Some(found) = usable(&chain, &key, &ids, job.fraction, job.now()) else { continue };
                // 刚看过用得了、这一下又读不了了：当它没有
                let (Some(key), Some(chain)) = (read(&key), read(&chain)) else { continue };
                put_file(&s.key, &key, 0o600)?;
                put_file(&s.chain, &chain, 0o644)?;
                job.log(&format!("{value} 接着用 {team} 原来的证书（{} 到期）", when(found.1)));
                got = Some(found);
                break;
            }
        }
        let now = job.now();
        if got.is_none_or(|(due, _)| now >= due) && now >= s.retry {
            let tried = job.acme(&mut self.acme).and_then(|acme| {
                let cert_key = Key::open(&s.key, true)?;
                job.renew(acme, &ids, &s.chain, &cert_key)
            });
            match tried {
                Ok(state) => {
                    got = Some(state);
                    s.fails = 0;
                }
                Err(Trouble::Problem(why)) => {
                    s.fails = s.fails.saturating_add(1);
                    let pause = retry_after(s.fails);
                    s.retry = job.now() + pause as f64;
                    let left = match got {
                        Some((_, end)) => format!("，手里的那张 {} 到期", when(end)),
                        None => "，手里没有用得了的证书".to_string(),
                    };
                    job.log(&format!("{value} 的证书没要到（{} 用）：{why}{left}。{} 分钟后再试", names.join("、"), pause / 60));
                    self.acme = None; // 账户、nonce 从头来
                }
                Err(os) => return Err(os),
            }
        }
        let due = match got {
            Some((due, _)) if job.now() < due => due,
            _ => s.retry,
        };
        *wake = wake.min(due.max(job.now() + 1.0));
        if got.is_none() {
            *ready = false;
            return Ok(());
        }
        for team in names {
            let d = path::join(&a.out, team);
            if give(&d, s)? {
                self.missing.remove(team);
                continue;
            }
            *ready = false;
            if self.missing.insert(team.clone()) {
                job.log(&format!("{team} 的目录 {d} 不在：证书放不进去（安装脚本建它）"));
            }
        }
        Ok(())
    }
}

/// --teams：照目录里登记的 Team，每个地址一张证书，放进用它的那几个 Team 的目录；目录变了过几秒就照新的来（Acme:496-562）
fn serve_teams(job: &Job<'_>) -> Result<i32, Trouble> {
    let dir = job.a.teams.as_deref().unwrap_or("");
    makedirs(job.a.store.as_deref().unwrap_or(""), 0o700)?;
    let mut held = Teams::default();
    loop {
        let seen = stamp(dir);
        let (ready, wake) = held.round(job);
        if job.a.once {
            return Ok(if ready { 0 } else { 1 });
        }
        // 睡到该续、该再试的时候（最多一小时）；登记的变了（安装脚本加了、去掉了一个 Team）过几秒就醒
        while job.now() < wake && stamp(dir) == seen {
            (job.world.sleep)(f64::min(LOOK as f64, f64::max(0.2, wake - job.now())));
        }
    }
}

/// http-01 的内容放在哪：自己听的端口，或者别的网站服务的目录
enum Place {
    Listen(Responder),
    Dir(Webroot),
}

/// --listen 写的 `地址:端口` 拆开（Acme:598-599）：地址可以带方括号，没写是 0.0.0.0
fn listen_at(listen: &str) -> Result<(String, u16), Trouble> {
    let (host, _, port) = text::rpartition(listen, ":");
    let host = text::strip_chars(host, "[]");
    // Python 的 int(port)：看不懂时说的就是这句
    let number =
        text::to_int(port).ok_or_else(|| Trouble::Os(format!("invalid literal for int() with base 10: {}", repr_str(port))))?;
    let port = u16::try_from(number).map_err(|_| Trouble::Os("bind(): port must be 0-65535.".to_string()))?;
    Ok((if host.is_empty() { "0.0.0.0" } else { host }.to_string(), port))
}

/// 实际监听的端口写到文件里（测试用）：先写到旁边再换过去（Acme:605-607）
fn write_port_file(port_file: &str, port: u16) -> Result<(), Trouble> {
    let tmp = format!("{port_file}.tmp");
    std::fs::write(&tmp, format!("{port}\n")).map_err(|e| Trouble::Os(open_error_text(&e, &tmp)))?;
    std::fs::rename(&tmp, port_file)
        .map_err(|e| Trouble::Os(format!("{}: {} -> {}", crate::py::fail::os_error_text(&e), repr_str(&tmp), repr_str(port_file))))
}

/// 参数都对了以后的那一段（Acme:585-611）：返回退出码
fn run(a: &Args, world: &World) -> i32 {
    let nonempty = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
    let failed = |e: Trouble| {
        (world.log)(&format!("起不来：{e}"));
        2
    };
    let setup = || -> Result<(Vec<San>, Key, Place), Trouble> {
        // dict.fromkeys(a.id)：写了几次一样的只算一次（比的是写法，不是规范以后的样子）
        let mut given: Vec<&String> = Vec::new();
        for id in &a.id {
            if !given.contains(&id) {
                given.push(id);
            }
        }
        let ids = given.iter().map(|id| identifier(id, a.any_ip)).collect::<Result<Vec<San>, Problem>>()?;
        makedirs(&a.account, 0o700)?;
        let account_key = Key::open(&path::join(&a.account, "account.key"), true)?;
        if let Some(teams) = nonempty(&a.teams).filter(|teams| !path::isdir(teams)) {
            return Err(Problem(format!("没有 {teams}")).into());
        }
        let place = match nonempty(&a.webroot) {
            Some(root) => Place::Dir(Webroot::new(&root)?),
            None => {
                let (host, port) = listen_at(&a.listen)?;
                Place::Listen(Responder::bind(&host, port)?)
            }
        };
        Ok((ids, account_key, place))
    };
    let (ids, account_key, place) = match setup() {
        Ok(ready) => ready,
        Err(e) => return failed(e),
    };
    let responder: &dyn Challenges = match &place {
        Place::Dir(webroot) => webroot,
        Place::Listen(responder) => {
            // 应答的端口在头一次去要之前就听着（--once、什么都不用做时也是）
            let port_file = nonempty(&a.port_file).map_or(Ok(()), |port_file| write_port_file(&port_file, responder.port()));
            if let Err(e) = port_file.and_then(|()| responder.start()) {
                return failed(e);
            }
            responder
        }
    };
    let job = Job { a, fraction: a.fraction(), account_key: &account_key, responder, world };
    let done = if nonempty(&a.teams).is_some() { serve_teams(&job) } else { serve_one(&job, &ids) };
    done.unwrap_or_else(failed)
}

/// `cca-core team-acme …`
pub fn cmd_team_acme(argv: &[String]) -> R<()> {
    let a = match args::parse(argv) {
        Parsed::Run(a) => a,
        Parsed::Help => {
            crate::out!("{}\n\n{}", args::USAGE, args::HELP);
            return Ok(());
        }
        Parsed::Error(why) => {
            crate::err!("{}", args::USAGE);
            crate::err!("cca-core team-acme: error: {why}");
            return crate::py::exit(2);
        }
    };
    match run(&a, &World::real()) {
        0 => Ok(()),
        code => crate::py::exit(code),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::json;

    /// 这几个正则都编得出来（写错了的要到头一次用才 panic），宽严和 Python 的一样
    #[test]
    fn regexes_compile_and_match_like_python() {
        assert!(token_re().matches("abc_DEF-123") && token_re().matches(&"a".repeat(256)));
        assert!(
            !token_re().matches("")
                && !token_re().matches(&"a".repeat(257))
                && !token_re().matches("a/b")
                && !token_re().matches("a.b")
        );
        assert!(!token_re().matches("abc\n"), "\\Z 不放过结尾的换行");
        assert!(team_re().matches("cca-team-0a1b2c3d") && team_re().matches("a") && team_re().matches(&"a".repeat(63)));
        assert!(
            !team_re().matches("") && !team_re().matches("-a") && !team_re().matches("A") && !team_re().matches(&"a".repeat(64))
        );
        assert!(!team_re().matches("cca-team-0a1b2c3d\n") && !team_re().matches("cca-team.new.123"));
        assert!(name_re().matches("example.com") && name_re().matches("a.b") && name_re().matches("xn--fiq228c.example.com"));
        assert!(!name_re().matches("localhost") && !name_re().matches("-a.example.com") && !name_re().matches("a..example.com"));
        assert!(!name_re().found("EXAMPLE.com") && name_re().fullmatch("example.com").is_some());
    }

    fn ok(given: &str, any_ip: bool) -> (String, String) {
        let id = identifier(given, any_ip).unwrap_or_else(|e| panic!("{given}: {e}"));
        (id.kind().to_string(), id.value().to_string())
    }

    fn why(given: &str, any_ip: bool) -> String {
        identifier(given, any_ip).err().unwrap_or_else(|| panic!("{given} 不该收")).to_string()
    }

    fn pair(kind: &str, value: &str) -> (String, String) {
        (kind.to_string(), value.to_string())
    }

    /// 对照数据（cli/tests/vectors/server_acme.json）：每一项是 server/cca-acme.py 的 identifier 给的结果（CPython 3.14.0）——各种写法的
    /// IP 地址、域名和两样都不是的，开着、没开 --any-ip 的各一遍。两边要一项不差，连没收时说的那句话
    #[test]
    fn identifier_like_python() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme.json")).unwrap().as_dict().unwrap();
        let (mut ips, mut names, mut not_public, mut neither) = (0, 0, 0, 0);
        for c in doc.list("identifier").unwrap().items() {
            let c = c.as_dict().unwrap();
            let (given, any_ip) = (c.str("given").unwrap().to_string(), c.get("any_ip").unwrap().as_bool().unwrap());
            let want = match c.str("err") {
                Some(why) => Err(why.to_string()),
                None => Ok(pair(&c.str("type").unwrap(), &c.str("value").unwrap())),
            };
            let got = identifier(&given, any_ip).map(|id| pair(id.kind(), id.value())).map_err(|p| p.0);
            assert_eq!(got, want, "{given:?}（--any-ip：{any_ip}）");
            match want {
                Ok((kind, _)) if kind == "ip" => ips += 1,
                Ok(_) => names += 1,
                Err(why) if why.ends_with("不是公网的地址：证书机构只给从公网连得上的地址签证书") => {
                    not_public += 1
                }
                Err(_) => neither += 1,
            }
        }
        assert!(ips >= 60 && names >= 30 && not_public >= 25 && neither >= 60, "{ips} {names} {not_public} {neither}");

        // 其中值得写明的几条。公网的 IP 地址：写成规范的样子，方括号去掉
        assert_eq!(ok("[2606:4700:4700::1111]", false), pair("ip", "2606:4700:4700::1111"));
        assert_eq!(ok("2620:FE::FE", false), pair("ip", "2620:fe::fe"));
        // 不是公网的不签，--any-ip 放开；说的是规范的写法
        assert_eq!(why("[fe80::1]", false), "fe80::1 不是公网的地址：证书机构只给从公网连得上的地址签证书");
        assert_eq!(ok("192.0.2.10", true), pair("ip", "192.0.2.10"));
        // 100.64.0.0/10（运营商的 NAT）在 Python 里不算 is_private：放过去了（证书机构自己会拒）
        assert_eq!(ok("100.64.0.1", false), pair("ip", "100.64.0.1"));
        // 域名：小写、去掉结尾的点。不是 IP 地址的写法、但各段合规矩的也当域名（结尾带点的地址就是这样）
        assert_eq!(ok("Team.Example.COM.", false), pair("dns", "team.example.com"));
        assert_eq!(ok("8.8.8.8.", false), pair("dns", "8.8.8.8"));
        // 整个名字最长 253 个字符
        let long = format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61));
        assert_eq!((long.len(), ok(&long, false)), (253, pair("dns", &long)));
        assert_eq!(why(&format!("{long}d"), true), format!("{long}d 不是 IP 地址，也不是域名"), "说的是原样的那个写法");
        // Python 的 $ 放过结尾的一个换行（--teams 的文件内容是先 strip 过的，命令行里一般带不进来）：照样
        assert_eq!(ok("example.com\n", false), pair("dns", "example.com\n"));
        assert_eq!(Problem("原因".into()).to_string(), "原因");
    }

    // ---- 手里的证书、两种运行的方式、入口 ----

    use super::client::testkit::{Ca, Knobs, Shelf};
    use super::key::pem;
    use super::key::tests::{fixture, fixture_key};
    use crate::py::fs::{chmod, listdir_sorted, mode_of, TempDir};
    use crate::py::time::timegm;
    use crate::server::x509::testkit as certs;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// 测试里的“现在”：2026-11-24 12:00 UTC
    const NOW: f64 = 1795521600.0;
    /// `certs::self_signed` 签的证书到什么时候
    const ENDS: &str = "2099-12-31 23:59 UTC";

    /// 假的世界：钟是拨好的（睡多久过多久），说的话、睡过的记下来
    struct Fake {
        now: Rc<Cell<f64>>,
        said: Rc<RefCell<Vec<String>>>,
        naps: Rc<RefCell<Vec<f64>>>,
        /// 睡满这么多次就不往下跑了（不带 --once 的那两个循环自己不会停：测试里从 sleep 里跳出来）
        limit: Rc<Cell<usize>>,
        /// 每睡一次调一下（这是第几次）
        on_nap: NapHook,
        world: World,
    }

    type NapHook = Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>;

    impl Fake {
        fn new() -> Fake {
            let (now, said, naps) = (Rc::new(Cell::new(NOW)), Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(Vec::new())));
            let (limit, on_nap): (_, NapHook) = (Rc::new(Cell::new(usize::MAX)), Rc::new(RefCell::new(None)));
            let (read, moved, noted, heard) = (now.clone(), now.clone(), naps.clone(), said.clone());
            let (cap, hooked) = (limit.clone(), on_nap.clone());
            let sleep = move |secs: f64| {
                noted.borrow_mut().push(secs);
                moved.set(moved.get() + secs);
                let n = noted.borrow().len();
                if let Some(hook) = hooked.borrow_mut().as_mut() {
                    hook(n);
                }
                if n >= cap.get() {
                    std::panic::resume_unwind(Box::new("睡够了"));
                }
            };
            let world = World {
                now: Box::new(move || read.get()),
                sleep: Box::new(sleep),
                log: Box::new(move |text| heard.borrow_mut().push(text.to_string())),
            };
            Fake { now, said, naps, limit, on_nap, world }
        }

        /// 跑一个自己不会停的循环，再睡满 more 次为止
        fn until_naps<T>(&self, more: usize, run: impl FnOnce() -> T) {
            self.limit.set(self.naps.borrow().len() + more);
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            self.limit.set(usize::MAX);
            assert!(stopped.is_err(), "该一直跑下去的");
        }

        /// 上次问过以后说的话
        fn said(&self) -> Vec<String> {
            std::mem::take(&mut *self.said.borrow_mut())
        }
    }

    fn v4() -> Vec<San> {
        vec![San::Ip("192.0.2.10".to_string())]
    }

    /// 在 dir 里放一对用得了的证书和私钥（给 ids 的，2025 年初到 2099 年底）
    fn put_pair(dir: &str, ids: &[San]) {
        std::fs::create_dir_all(dir).unwrap();
        let pkcs8 = certs::new_key();
        std::fs::write(path::join(dir, PRIVKEY), pem("PRIVATE KEY", &pkcs8)).unwrap();
        std::fs::write(path::join(dir, CHAIN), certs::pem("CERTIFICATE", &certs::self_signed(&pkcs8, Some(ids)))).unwrap();
    }

    /// 都在 d 下面、对着一个连不上的证书机构的一套参数
    fn args_in(d: &str) -> Args {
        Args {
            id: vec!["192.0.2.10".to_string()],
            teams: None,
            store: None,
            out: format!("{d}/out"),
            account: format!("{d}/account"),
            listen: "127.0.0.1:0".to_string(),
            webroot: None,
            directory: "http://127.0.0.1:1/dir".to_string(),
            profile: "shortlived".to_string(),
            once: true,
            ca: None,
            any_ip: true,
            port_file: None,
            renew_at: 0.5,
        }
    }

    /// 对照数据：Python 的 cert_state 对 openssl 造的这些证书怎么说（哪把私钥、要哪几个地址、过了多少就续），这里就怎么说——
    /// IPv4、IPv6、域名、几个地址的，SEC1 私钥的，2050 年以后的（时间是另一种写法），带中间证书的，前面有别的字的，没有地址的，
    /// LibreSSL 签的 v1 的，不是证书的，空的，不在的
    #[test]
    fn cert_state_like_python() {
        let d = TempDir::new().unwrap();
        let (mut usable_cases, mut not_usable) = (0, 0);
        let certs = fixture().list("certs").unwrap().items();
        let names: Vec<String> = certs.iter().map(|c| c.as_dict().unwrap().str("name").unwrap().to_string()).collect();
        for want in
            ["cert-ipv4", "cert-ipv6", "cert-dns", "cert-mixed", "cert-sec1", "cert-2050", "chain-ipv4", "chain-leading-text"]
        {
            assert!(names.iter().any(|n| n == want), "对照数据里要有 {want}");
        }
        for want in ["chain-ca-first", "cert-no-san", "cert-v1-libressl", "cert-text", "cert-empty", "missing", "cert-dst"] {
            assert!(names.iter().any(|n| n == want), "对照数据里要有 {want}");
        }
        for cert in certs {
            let cert = cert.as_dict().unwrap();
            let name = cert.str("name").unwrap();
            let p = path::join(&d.path, &format!("{name}.pem"));
            if let Some(pem) = cert.str("pem") {
                std::fs::write(&p, pem.as_bytes()).unwrap();
            }
            for case in cert.list("cases").unwrap().items() {
                let case = case.as_dict().unwrap();
                let key = fixture_key(&d.path, &case.str("key").unwrap());
                let ids: Vec<San> = case
                    .list("ids")
                    .unwrap()
                    .items()
                    .iter()
                    .map(|id| {
                        let id = id.as_list().unwrap().strs();
                        if &*id[0] == "ip" {
                            San::Ip(id[1].to_string())
                        } else {
                            San::Dns(id[1].to_string())
                        }
                    })
                    .collect();
                let fraction = case.get("fraction").unwrap().as_num().unwrap();
                let want = case.list("state").map(|s| (s.at(0).as_num().unwrap(), s.at(1).as_num().unwrap()));
                assert_eq!(cert_state(&p, &ids, &key, fraction), want, "{name}：{ids:?}，{fraction}");
                if want.is_some() {
                    usable_cases += 1
                } else {
                    not_usable += 1
                }
            }
        }
        assert!(usable_cases >= 18 && not_usable >= 18, "{usable_cases} {not_usable}");
    }

    /// 用得了：证书和私钥是一对、是给这几个地址的、还没过期；私钥不在时不生成
    #[test]
    fn usable_needs_the_pair_and_time_left() {
        let d = TempDir::new().unwrap();
        let (mine, other) = (path::join(&d.path, "mine"), path::join(&d.path, "other"));
        put_pair(&mine, &v4());
        put_pair(&other, &v4());
        let at = |dir: &str, name: &str| path::join(dir, name);
        let (start, end) = (timegm(2025, 1, 1, 0, 0, 0) as f64, timegm(2099, 12, 31, 23, 59, 59) as f64);
        let state = |fraction: f64, now: f64| usable(&at(&mine, CHAIN), &at(&mine, PRIVKEY), &v4(), fraction, now);
        assert_eq!(state(0.5, NOW), Some((start + (end - start) * 0.5, end)));
        assert_eq!(state(0.0, NOW), Some((start, end)));
        assert_eq!(state(0.5, end - 1.0), Some((start + (end - start) * 0.5, end)), "过了该续的时候、还没到期：还用得了");
        assert_eq!(state(0.5, end), None, "到期了");
        assert_eq!(usable(&at(&mine, CHAIN), &at(&other, PRIVKEY), &v4(), 0.5, NOW), None, "不是这把私钥的");
        assert_eq!(usable(&at(&mine, CHAIN), &at(&mine, PRIVKEY), &[San::Ip("192.0.2.11".to_string())], 0.5, NOW), None);
        assert_eq!(usable(&at(&mine, CHAIN), &at(&mine, PRIVKEY), &[], 0.5, NOW), None);
        let nowhere = path::join(&d.path, "nowhere");
        assert_eq!(usable(&at(&mine, CHAIN), &at(&nowhere, PRIVKEY), &v4(), 0.5, NOW), None);
        assert!(!path::exists(&nowhere), "私钥不在时不生成");
        assert_eq!(usable(&at(&nowhere, CHAIN), &at(&mine, PRIVKEY), &v4(), 0.5, NOW), None);
        assert_eq!(when(end), ENDS);
        assert_eq!(when(0.0), "1970-01-01 00:00 UTC");
    }

    /// 没成以后隔多久再试：1 分钟、10 分钟、1 小时、6 小时，之后一直 6 小时
    #[test]
    fn retry_ladder() {
        assert_eq!([1, 2, 3, 4, 5, 100].map(retry_after), [60, 600, 3600, 21600, 21600, 21600]);
        assert_eq!(
            names_of(&[San::Ip("192.0.2.10".to_string()), San::Dns("relay.example.com".to_string())]),
            "192.0.2.10、relay.example.com"
        );
        let slot = Slot::new("/store", San::Ip("2001:db8::1".to_string()));
        assert_eq!((slot.dir.as_str(), slot.chain.as_str()), ("/store/ip-2001_db8__1", "/store/ip-2001_db8__1/fullchain.pem"));
        assert_eq!(Slot::new("/store/", San::Dns("relay.example.com".to_string())).key, "/store/dns-relay.example.com/privkey.pem");
    }

    /// --teams 目录里登记的：文件名是 Team 的服务名、内容是地址。不是这种名字的、不是文件的跳过；地址看不懂的说一次
    #[test]
    fn registry_reads_what_the_installer_wrote() {
        let d = TempDir::new().unwrap();
        let file = |name: &str, content: &[u8]| std::fs::write(path::join(&d.path, name), content).unwrap();
        file("cca-team-aaaaaaaa", b"192.0.2.10\n");
        file("cca-team-bbbbbbbb", b"  Relay.Example.COM. \r\n");
        file("cca-team-cccccccc", b"not a name\n");
        file("cca-team-dddddddd", b"\xff\xfe");
        file("cca-team-eeeeeeee", b"[2001:db8::1]");
        file("cca-team-ffffffff", "a.b\r\nc".as_bytes());
        file("cca-team-gggggggg", format!("{}.example.com", "a".repeat(600)).as_bytes());
        file("cca-team-aaaaaaaa.new.123", b"192.0.2.99\n");
        file("Team", b"192.0.2.99\n");
        file("-team", b"192.0.2.99\n");
        std::fs::create_dir(path::join(&d.path, "cca-team-directory")).unwrap();
        let said = RefCell::new(Vec::new());
        let log = |text: &str| said.borrow_mut().push(text.to_string());
        let mut told = HashMap::new();
        let got = registry(&d.path, &mut told, true, &log).unwrap();
        let want = [
            ("cca-team-aaaaaaaa", San::Ip("192.0.2.10".to_string())),
            ("cca-team-bbbbbbbb", San::Dns("relay.example.com".to_string())),
            ("cca-team-eeeeeeee", San::Ip("2001:db8::1".to_string())),
        ];
        assert_eq!(got, want.map(|(team, id)| (team.to_string(), id)));
        assert_eq!(
            std::mem::take(&mut *said.borrow_mut()),
            [
                "cca-team-cccccccc 登记的地址用不了：not a name 不是 IP 地址，也不是域名".to_string(),
                "cca-team-dddddddd 登记的地址用不了：'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
                    .to_string(),
                "cca-team-ffffffff 登记的地址用不了：a.b\nc 不是 IP 地址，也不是域名".to_string(),
                format!("cca-team-gggggggg 登记的地址用不了：{} 不是 IP 地址，也不是域名", "a".repeat(512)),
            ]
        );
        // 说过的不再说；换了一种不对法再说；改对了就收，以后再坏还说
        assert_eq!(registry(&d.path, &mut told, true, &log).unwrap().len(), 3);
        assert!(said.borrow().is_empty());
        file("cca-team-cccccccc", b"still not a name\n");
        file("cca-team-dddddddd", b"198.51.100.7\n");
        assert_eq!(registry(&d.path, &mut told, true, &log).unwrap().len(), 4);
        assert_eq!(
            std::mem::take(&mut *said.borrow_mut()),
            ["cca-team-cccccccc 登记的地址用不了：still not a name 不是 IP 地址，也不是域名"]
        );
        file("cca-team-dddddddd", b"\xff\xfe");
        registry(&d.path, &mut told, true, &log).unwrap();
        assert_eq!(said.borrow().len(), 1);
        // 不是公网的地址：没开 --any-ip 时不收
        said.borrow_mut().clear();
        let public = registry(&d.path, &mut HashMap::new(), false, &log).unwrap();
        assert_eq!(public.iter().map(|(team, _)| team.as_str()).collect::<Vec<_>>(), ["cca-team-bbbbbbbb"]);
        assert!(said.borrow().contains(
            &"cca-team-aaaaaaaa 登记的地址用不了：192.0.2.10 不是公网的地址：证书机构只给从公网连得上的地址签证书".to_string()
        ));
        // 读不了的文件（root 不受权限管：那样这一条测不了）
        let locked = path::join(&d.path, "cca-team-aaaaaaaa");
        chmod(&locked, 0o000).unwrap();
        if std::fs::read(&locked).is_err() {
            said.borrow_mut().clear();
            registry(&d.path, &mut HashMap::new(), true, &log).unwrap();
            assert_eq!(said.borrow()[0], format!("cca-team-aaaaaaaa 登记的地址用不了：[Errno 13] Permission denied: '{locked}'"));
        }
        chmod(&locked, 0o644).unwrap();
        // 目录不在
        let e = registry(&path::join(&d.path, "nowhere"), &mut told, true, &log).unwrap_err();
        assert_eq!(strerror(&e), "No such file or directory");
    }

    /// 目录的样子：里面加了、去掉了、改了文件就不一样；看不了是 None
    #[test]
    fn stamp_follows_the_directory() {
        let d = TempDir::new().unwrap();
        let at = |name: &str| path::join(&d.path, name);
        assert_eq!(stamp(&d.path), Some(Vec::new()));
        std::fs::write(at("cca-team-bbbbbbbb"), "192.0.2.10\n").unwrap();
        std::fs::write(at("cca-team-aaaaaaaa"), "192.0.2.10\n").unwrap();
        let two = stamp(&d.path).unwrap();
        assert_eq!(
            two.iter().map(|(name, _, size)| (name.as_str(), *size)).collect::<Vec<_>>(),
            [("cca-team-aaaaaaaa", 11), ("cca-team-bbbbbbbb", 11)]
        );
        assert_eq!(stamp(&d.path), Some(two.clone()), "没动过就一样");
        std::fs::write(at("cca-team-aaaaaaaa"), "192.0.2.110\n").unwrap();
        let changed = stamp(&d.path).unwrap();
        assert_ne!(changed, two);
        std::fs::remove_file(at("cca-team-bbbbbbbb")).unwrap();
        assert_eq!(stamp(&d.path).unwrap().len(), 1);
        // 修改时间细到纳秒：大小一样、只是时间变了也看得出来
        let old = std::fs::File::options().write(true).open(at("cca-team-aaaaaaaa")).unwrap();
        old.set_modified(std::time::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 123_456_789)).unwrap();
        let moved = stamp(&d.path).unwrap();
        assert_eq!(moved[0].1, 1_700_000_000_123_456_789);
        assert_eq!(stamp(&at("nowhere")), None);
        std::os::unix::fs::symlink(at("gone"), at("dangling")).unwrap();
        assert_eq!(stamp(&d.path), None, "有一项看不了：整个算看不了（Python 的 os.stat 抛了异常）");
    }

    /// 放进一个 Team 的目录：私钥 640、证书 644；一样的不换；目录不在是 false
    #[test]
    fn give_replaces_only_what_differs() {
        use std::os::unix::fs::MetadataExt;
        let d = TempDir::new().unwrap();
        let slot = Slot::new(&path::join(&d.path, "store"), San::Ip("192.0.2.10".to_string()));
        put_pair(&slot.dir, &v4());
        let team = path::join(&d.path, "out/cca-team-aaaaaaaa");
        assert_eq!(give(&team, &slot), Ok(false));
        std::fs::create_dir_all(&team).unwrap();
        assert_eq!(give(&team, &slot), Ok(true));
        let (key, chain) = (path::join(&team, PRIVKEY), path::join(&team, CHAIN));
        assert_eq!((mode_of(&key).unwrap(), mode_of(&chain).unwrap()), (0o640, 0o644));
        assert_eq!(std::fs::read(&key).unwrap(), std::fs::read(&slot.key).unwrap());
        assert_eq!(std::fs::read(&chain).unwrap(), std::fs::read(&slot.chain).unwrap());
        assert_eq!(listdir_sorted(&team).unwrap(), [CHAIN, PRIVKEY], "没有留下半截的文件");
        // 再放一次：一样的不换（还是原来那个文件）
        let inode = |p: &str| std::fs::metadata(p).unwrap().ino();
        let before = (inode(&key), inode(&chain));
        assert_eq!(give(&team, &slot), Ok(true));
        assert_eq!((inode(&key), inode(&chain)), before);
        // 证书续上了：只换证书
        std::fs::write(&slot.chain, "new chain").unwrap();
        assert_eq!(give(&team, &slot), Ok(true));
        assert_eq!((inode(&key), std::fs::read(&chain).unwrap()), (before.0, b"new chain".to_vec()));
        // 存的那一份里少一个文件：有的那个照放
        std::fs::remove_file(&slot.chain).unwrap();
        assert_eq!(give(&team, &slot), Ok(true));
        // 放不进去（那个位置是个有东西的目录）：是错，不是 false
        std::fs::remove_file(&key).unwrap();
        std::fs::create_dir_all(path::join(&key, "x")).unwrap();
        std::fs::write(&slot.key, "changed").unwrap();
        assert!(matches!(give(&team, &slot), Err(Trouble::Os(_))));
    }

    /// 几个 Team 共用：一个地址一份，存的那份没有、Team 的目录里有用得了的就接着用（不去要）；放进各个 Team 的目录；
    /// 目录还没建的说一次；登记的地址看不懂的说一次
    #[test]
    fn teams_share_one_certificate_per_address() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let mut a = args_in(&d.path);
        (a.id, a.teams, a.store) = (Vec::new(), Some(format!("{}/teams", d.path)), Some(format!("{}/store", d.path)));
        let (teams, store) = (a.teams.clone().unwrap(), a.store.clone().unwrap());
        std::fs::create_dir_all(&teams).unwrap();
        let register = |team: &str, address: &str| std::fs::write(path::join(&teams, team), format!("{address}\n")).unwrap();
        let out = |team: &str| format!("{}/out/{team}", d.path);
        let (ta, tb, tc, td) = ("cca-team-aaaaaaaa", "cca-team-bbbbbbbb", "cca-team-cccccccc", "cca-team-dddddddd");
        for team in [ta, tb, tc] {
            register(team, "192.0.2.10");
        }
        register(td, "not a name");
        std::fs::create_dir_all(out(ta)).unwrap();
        put_pair(&out(tb), &v4()); // 以前 b 自己的证书助手要的
        let account = Key::open(&format!("{}/account.key", d.path), true).unwrap();
        let shelf = Shelf::default();
        let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        makedirs(&store, 0o700).unwrap();
        let mut held = Teams::default();
        assert_eq!(held.round(&job), (false, NOW + 3600.0));
        assert_eq!(
            fake.said(),
            [
                format!("{td} 登记的地址用不了：not a name 不是 IP 地址，也不是域名"),
                format!("192.0.2.10 接着用 {tb} 原来的证书（{ENDS} 到期）"),
                format!("{tc} 的目录 {} 不在：证书放不进去（安装脚本建它）", out(tc)),
            ]
        );
        let slot = format!("{store}/ip-192.0.2.10");
        let modes = |dir: &str| {
            (mode_of(dir).unwrap(), mode_of(&path::join(dir, PRIVKEY)).unwrap(), mode_of(&path::join(dir, CHAIN)).unwrap())
        };
        assert_eq!(modes(&slot), (0o700, 0o600, 0o644));
        assert_eq!((modes(&out(ta)).1, modes(&out(ta)).2), (0o640, 0o644));
        for name in [PRIVKEY, CHAIN] {
            let kept = std::fs::read(path::join(&out(tb), name)).unwrap();
            assert_eq!(std::fs::read(path::join(&slot, name)).unwrap(), kept, "{name}");
            assert_eq!(std::fs::read(path::join(&out(ta), name)).unwrap(), kept, "{name}");
        }
        assert_eq!(listdir_sorted(&store).unwrap(), ["ip-192.0.2.10"]);
        assert_eq!(listdir_sorted(&slot).unwrap(), [CHAIN, PRIVKEY]);
        // 再看一轮：说过的不再说
        assert_eq!(held.round(&job), (false, NOW + 3600.0));
        assert!(fake.said().is_empty());
        // c 的目录建好了：放进去，都齐了。之后又没了：再说一次
        std::fs::create_dir_all(out(tc)).unwrap();
        assert_eq!(
            held.round(&job),
            (true, NOW + 3600.0),
            "登记的地址看不懂的那个 Team 不算在里面（Python 也是）：别的都齐了就算齐了"
        );
        std::fs::remove_file(path::join(&teams, td)).unwrap();
        assert_eq!(held.round(&job), (true, NOW + 3600.0));
        assert!(fake.said().is_empty());
        assert_eq!(modes(&out(tc)), (mode_of(&out(tc)).unwrap(), 0o640, 0o644));
        std::fs::remove_dir_all(out(tc)).unwrap();
        assert_eq!(held.round(&job), (false, NOW + 3600.0));
        assert_eq!(fake.said(), [format!("{tc} 的目录 {} 不在：证书放不进去（安装脚本建它）", out(tc))]);
        // --once：照这一轮的结果定退出码
        assert_eq!(serve_teams(&job), Ok(1));
        std::fs::remove_file(path::join(&teams, tc)).unwrap();
        assert_eq!(serve_teams(&job), Ok(0));
        assert!(shelf.history().is_empty() && fake.naps.borrow().is_empty());
        // 登记的目录没了
        std::fs::remove_dir_all(&teams).unwrap();
        fake.said();
        assert_eq!(held.round(&job), (true, NOW + 3600.0));
        assert_eq!(fake.said(), [format!("读不了 {teams}（No such file or directory）")]);
    }

    /// 几个 Team 共用时要不到证书：记一次、过一阵再试（1 分钟、10 分钟……），没到时候不去要；别的错（文件放不了）一分钟后再来
    #[test]
    fn teams_retry_later() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let mut a = args_in(&d.path);
        (a.id, a.teams, a.store) = (Vec::new(), Some(format!("{}/teams", d.path)), Some(format!("{}/store", d.path)));
        let (teams, store) = (a.teams.clone().unwrap(), a.store.clone().unwrap());
        std::fs::create_dir_all(&teams).unwrap();
        let (te, tf) = ("cca-team-eeeeeeee", "cca-team-ffffffff");
        for team in [te, tf] {
            std::fs::write(path::join(&teams, team), "198.51.100.7\n").unwrap();
            std::fs::create_dir_all(format!("{}/out/{team}", d.path)).unwrap();
        }
        let account = Key::open(&format!("{}/account.key", d.path), true).unwrap();
        let shelf = Shelf::default();
        let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        let mut held = Teams::default();
        let failed = |minutes: u32| {
            format!("198.51.100.7 的证书没要到（{te}、{tf} 用）：连不上证书机构（ConnectionRefused），手里没有用得了的证书。{minutes} 分钟后再试")
        };
        assert_eq!(held.round(&job), (false, NOW + 60.0));
        assert_eq!(fake.said(), [failed(1)]);
        assert_eq!(listdir_sorted(&format!("{store}/ip-198.51.100.7")).unwrap(), Vec::<String>::new(), "连不上时私钥也不生成");
        // 还没到再试的时候
        fake.now.set(NOW + 59.0);
        assert_eq!(held.round(&job), (false, NOW + 60.0));
        assert!(fake.said().is_empty());
        let mut at = NOW;
        for (waited, minutes, next) in [(60.0, 10, 600.0), (600.0, 60, 3600.0), (3600.0, 360, 21600.0), (21600.0, 360, 21600.0)] {
            at += waited;
            fake.now.set(at);
            assert_eq!(held.round(&job), (false, at + f64::min(next, 3600.0)), "{minutes}");
            assert_eq!(fake.said(), [failed(minutes)]);
        }
        // 手里有一张、到了该续的时候要不到：说它什么时候到期
        put_pair(&format!("{store}/ip-198.51.100.7"), &[San::Ip("198.51.100.7".to_string())]);
        let renewing =
            Job { fraction: 0.0, ..Job { a: &a, fraction: 0.0, account_key: &account, responder: &shelf, world: &fake.world } };
        let mut fresh = Teams::default();
        assert_eq!(fresh.round(&renewing), (true, at + 60.0), "手里那张还用得了：照样放进各个 Team 的目录");
        assert_eq!(
            fake.said(),
            [format!("198.51.100.7 的证书没要到（{te}、{tf} 用）：连不上证书机构（ConnectionRefused），手里的那张 {ENDS} 到期。1 分钟后再试")]
        );
        assert!(path::isfile(&format!("{}/out/{te}/{PRIVKEY}", d.path)));
        // --ca 指的文件读不了：不算“没要到”，说放不好、一分钟后再来
        let mut broken = a.clone();
        broken.ca = Some(format!("{}/nowhere.pem", d.path));
        let job = Job { a: &broken, fraction: 0.0, account_key: &account, responder: &shelf, world: &fake.world };
        assert_eq!(Teams::default().round(&job), (false, at + 60.0));
        assert_eq!(fake.said(), ["198.51.100.7 的证书放不好：[Errno 2] No such file or directory"]);
        // 存证书的地方建不了
        let mut blocked = a.clone();
        blocked.store = Some(format!("{}/account.key", account.path.trim_end_matches("/account.key")));
        let job = Job { a: &blocked, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        assert_eq!(Teams::default().round(&job), (false, at + 60.0));
        assert_eq!(
            fake.said(),
            [format!("198.51.100.7 的证书放不好：[Errno 20] Not a directory: '{}/ip-198.51.100.7'", account.path)]
        );
    }

    /// --id 那种方式、带 --once：手里有用得了的、没到该续的时候什么都不做；该要时要一张、换上；要不到是退出码 1、说清楚
    #[test]
    fn serve_one_once() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let mut a = args_in(&d.path);
        let account = Key::open(&format!("{}/account.key", d.path), true).unwrap();
        let shelf = Shelf::default();
        let (key, chain) = (path::join(&a.out, PRIVKEY), path::join(&a.out, CHAIN));
        // 什么都没有、证书机构连不上：目录、私钥建好了（权限对），退出码 1
        {
            let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
            assert_eq!(serve_one(&job, &v4()), Ok(1));
            assert_eq!(
                fake.said(),
                ["192.0.2.10 的证书没要到：连不上证书机构（ConnectionRefused），手里没有用得了的证书。1 分钟后再试"]
            );
            assert_eq!(
                (mode_of(&a.out).unwrap(), mode_of(&key).unwrap(), listdir_sorted(&a.out).unwrap()),
                (0o700, 0o600, vec![PRIVKEY.to_string()])
            );
        }
        // 假证书机构签给这把私钥的证书
        put_pair(&a.out, &v4());
        let signed = std::fs::read(&chain).unwrap();
        std::fs::remove_file(&chain).unwrap();
        let ca = Ca::start(Knobs { profiles: vec!["shortlived"], cert: signed.clone(), ..Knobs::default() }, &shelf);
        a.directory = ca.directory();
        let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        assert_eq!(serve_one(&job, &v4()), Ok(0));
        assert_eq!(fake.said(), [format!("拿到 192.0.2.10 的证书，{ENDS} 到期")]);
        assert_eq!((std::fs::read(&chain).unwrap(), mode_of(&chain).unwrap()), (signed.clone(), 0o644));
        assert_eq!(listdir_sorted(&a.out).unwrap(), [CHAIN, PRIVKEY], "没有留下半截的文件");
        assert_eq!(ca.log().iter().filter(|l| *l == "issued").count(), 1);
        // 还没到续的时候：什么都不向证书机构要，什么都不说
        let asked = ca.lines().len();
        assert_eq!(serve_one(&job, &v4()), Ok(0));
        assert_eq!((ca.lines().len(), fake.said().len()), (asked, 0));
        // 到时候了（一过有效期的开头就续）：要一张新的
        let job = Job { fraction: 0.0, ..job };
        assert_eq!(serve_one(&job, &v4()), Ok(0));
        assert_eq!(ca.log().iter().filter(|l| *l == "issued").count(), 2);
        assert_eq!(fake.said().len(), 1);
        // 证书机构给的不是这把私钥的：换是换上了（Python 也是先换上再看），算没要到
        let other = TempDir::new().unwrap();
        put_pair(&other.path, &v4());
        ca.set(|k| k.cert = std::fs::read(path::join(&other.path, CHAIN)).unwrap());
        assert_eq!(serve_one(&job, &v4()), Ok(1));
        assert_eq!(
            fake.said(),
            [format!("192.0.2.10 的证书没要到：证书机构给的证书不是给 192.0.2.10 的，或者不是这把私钥的，手里的那张 {ENDS} 到期。1 分钟后再试")]
        );
        // 证书的私钥读不了：不是“没要到”，是起不来
        std::fs::write(&key, "not a key").unwrap();
        assert_eq!(serve_one(&job, &v4()), Err(Trouble::Problem(format!("{key} 不是 P-256 的私钥"))));
        assert!(fake.naps.borrow().is_empty(), "带 --once 的不等");
    }

    /// --listen 怎么拆
    #[test]
    fn listen_like_python() {
        let ok = |host: &str, port: u16| Ok((host.to_string(), port));
        assert_eq!(listen_at("0.0.0.0:18080"), ok("0.0.0.0", 18080));
        assert_eq!(listen_at("[::1]:80"), ok("::1", 80));
        assert_eq!(listen_at("::1:80"), ok("::1", 80));
        assert_eq!(listen_at(":8080"), ok("0.0.0.0", 8080));
        assert_eq!(listen_at("8080"), ok("0.0.0.0", 8080));
        assert_eq!(listen_at("localhost: 0 "), ok("localhost", 0));
        let bad = |listen: &str| listen_at(listen).unwrap_err().to_string();
        assert_eq!(bad(""), "invalid literal for int() with base 10: ''");
        assert_eq!(bad("127.0.0.1:"), "invalid literal for int() with base 10: ''");
        assert_eq!(bad("127.0.0.1:http"), "invalid literal for int() with base 10: 'http'");
        assert_eq!(bad("127.0.0.1"), "invalid literal for int() with base 10: '127.0.0.1'");
        assert_eq!(bad("127.0.0.1:70000"), "bind(): port must be 0-65535.");
        assert_eq!(bad("127.0.0.1:-1"), "bind(): port must be 0-65535.");
    }

    /// 入口：起不来的各种情况说一句、退出码 2；起得来时应答的端口先听上、端口写进文件
    #[test]
    fn run_says_why_it_cannot_start() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let base = args_in(&d.path);
        let run_with = |change: &dyn Fn(&mut Args)| {
            let mut a = base.clone();
            change(&mut a);
            (run(&a, &fake.world), fake.said())
        };
        let said = |why: &str| (2, vec![format!("起不来：{why}")]);
        assert_eq!(
            run_with(&|a| (a.id, a.any_ip) = (vec!["192.168.1.5".to_string()], false)),
            said("192.168.1.5 不是公网的地址：证书机构只给从公网连得上的地址签证书")
        );
        assert!(!path::exists(&base.account), "地址不对时什么都不建");
        assert_eq!(run_with(&|a| a.id = vec!["not a name".to_string()]), said("not a name 不是 IP 地址，也不是域名"));
        assert_eq!(run_with(&|a| a.listen = "127.0.0.1:http".to_string()), said("invalid literal for int() with base 10: 'http'"));
        let nowhere = format!("{}/nowhere", d.path);
        assert_eq!(
            run_with(&|a| (a.id, a.teams, a.store) = (Vec::new(), Some(nowhere.clone()), Some(format!("{}/store", d.path)))),
            said(&format!("没有 {nowhere}"))
        );
        let taken = format!("{}/account/account.key", d.path);
        assert_eq!(run_with(&|a| a.account = taken.clone()), said(&format!("[Errno 17] File exists: '{taken}'")));
        assert_eq!(run_with(&|a| a.out = taken.clone()), said(&format!("[Errno 17] File exists: '{taken}'")));
        assert_eq!(
            run_with(&|a| a.port_file = Some(format!("{nowhere}/port"))),
            said(&format!("[Errno 2] No such file or directory: '{nowhere}/port.tmp'"))
        );
        // 端口占着
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (code, why) = run_with(&|a| a.listen = format!("127.0.0.1:{}", busy.local_addr().unwrap().port()));
        assert!(code == 2 && why[0].starts_with("起不来：[Errno ") && why[0].ends_with("Address already in use"), "{why:?}");
        // 起得来：手里有用得了的证书，带 --once 什么都不做、退出码 0；端口文件里是实际听的端口
        put_pair(&base.out, &v4());
        let port_file = format!("{}/port", d.path);
        assert_eq!(run_with(&|a| a.port_file = Some(port_file.clone())), (0, Vec::new()));
        let port: u16 = std::fs::read_to_string(&port_file).unwrap().strip_suffix('\n').unwrap().parse().unwrap();
        assert!(port > 0 && listdir_sorted(&d.path).unwrap().iter().all(|name| !name.ends_with(".tmp")));
        // --webroot：不听端口，目录建好
        let site = format!("{}/site", d.path);
        assert_eq!(run_with(&|a| (a.webroot, a.listen) = (Some(site.clone()), "nonsense".to_string())), (0, Vec::new()));
        assert!(path::isdir(&format!("{site}/.well-known/acme-challenge")));
        // 同一个地址写了两遍只算一遍；写法不一样的各算各的（证书上还是那一个地址：比的是集合）
        assert_eq!(
            run_with(&|a| a.id = vec!["192.0.2.10".to_string(), "192.0.2.10".to_string(), "[192.0.2.10]".to_string()]),
            (0, Vec::new())
        );
    }

    /// --id 那种方式、不带 --once：没到该续的时候睡到那时候（最少一分钟、最多一小时）再看，什么都不说；要不到就隔 1 分钟、10 分钟、
    /// 1 小时、6 小时再试；要到了接着等
    #[test]
    fn serve_one_keeps_running() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let mut a = args_in(&d.path);
        a.once = false;
        let account = Key::open(&format!("{}/account.key", d.path), true).unwrap();
        let shelf = Shelf::default();
        put_pair(&a.out, &v4());
        let (start, end) = (timegm(2025, 1, 1, 0, 0, 0) as f64, timegm(2099, 12, 31, 23, 59, 59) as f64);
        // 离该续还远：一小时看一次
        let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        fake.until_naps(3, || serve_one(&job, &v4()));
        assert_eq!((std::mem::take(&mut *fake.naps.borrow_mut()), fake.said()), (vec![3600.0; 3], Vec::new()));
        // 再过 10 秒就该续：最少睡一分钟。到时候了证书机构连不上：一级级隔得更久，手里那张还在
        fake.now.set(NOW);
        let job = Job { fraction: (NOW + 10.0 - start) / (end - start), ..job };
        fake.until_naps(6, || serve_one(&job, &v4()));
        assert_eq!(std::mem::take(&mut *fake.naps.borrow_mut()), [60.0, 60.0, 600.0, 3600.0, 21600.0, 21600.0]);
        let failed = |minutes: u32| {
            format!("192.0.2.10 的证书没要到：连不上证书机构（ConnectionRefused），手里的那张 {ENDS} 到期。{minutes} 分钟后再试")
        };
        assert_eq!(fake.said(), [1, 10, 60, 360, 360].map(failed));
        // 证书机构连得上：要到一张，接着等
        let chain = path::join(&a.out, CHAIN);
        let signed = std::fs::read(&chain).unwrap();
        std::fs::remove_file(&chain).unwrap();
        let ca = Ca::start(Knobs { profiles: vec!["shortlived"], cert: signed.clone(), ..Knobs::default() }, &shelf);
        let mut reachable = a.clone();
        reachable.directory = ca.directory();
        let job = Job { a: &reachable, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        fake.until_naps(2, || serve_one(&job, &v4()));
        assert_eq!(fake.said(), [format!("拿到 192.0.2.10 的证书，{ENDS} 到期")]);
        assert_eq!((fake.naps.borrow().clone(), ca.log().iter().filter(|l| *l == "issued").count()), (vec![3600.0; 2], 1));
        assert_eq!(std::fs::read(&chain).unwrap(), signed);
    }

    /// 几个 Team 共用、不带 --once：睡到该续、该再试的时候，一次最多 5 秒，醒了看一眼登记的变了没有——安装脚本登记了新的 Team
    /// 不用等到一小时；要不到的照 1 分钟、10 分钟、1 小时、6 小时再试，一小时总要看一轮
    #[test]
    fn teams_keep_running() {
        let d = TempDir::new().unwrap();
        let fake = Fake::new();
        let mut a = args_in(&d.path);
        (a.id, a.teams, a.store, a.once) =
            (Vec::new(), Some(format!("{}/teams", d.path)), Some(format!("{}/store", d.path)), false);
        let teams = a.teams.clone().unwrap();
        std::fs::create_dir_all(&teams).unwrap();
        let out = |team: &str| format!("{}/out/{team}", d.path);
        let (ta, tb, te) = ("cca-team-aaaaaaaa", "cca-team-bbbbbbbb", "cca-team-eeeeeeee");
        std::fs::write(path::join(&teams, ta), "192.0.2.10\n").unwrap();
        put_pair(&out(ta), &v4());
        let account = Key::open(&format!("{}/account.key", d.path), true).unwrap();
        let shelf = Shelf::default();
        let job = Job { a: &a, fraction: 0.5, account_key: &account, responder: &shelf, world: &fake.world };
        // 睡到第 3 次时登记了第二个 Team（同一个地址）：下一眼就放进它的目录
        let (registry_dir, second) = (teams.clone(), out(tb));
        *fake.on_nap.borrow_mut() = Some(Box::new(move |n| {
            if n == 3 {
                std::fs::create_dir_all(&second).unwrap();
                std::fs::write(path::join(&registry_dir, tb), "192.0.2.10\n").unwrap();
            }
        }));
        fake.until_naps(4, || serve_teams(&job));
        *fake.on_nap.borrow_mut() = None;
        assert_eq!(std::mem::take(&mut *fake.naps.borrow_mut()), [5.0; 4]);
        assert_eq!(fake.said(), [format!("192.0.2.10 接着用 {ta} 原来的证书（{ENDS} 到期）")]);
        assert_eq!(std::fs::read(path::join(&out(tb), CHAIN)).unwrap(), std::fs::read(path::join(&out(ta), CHAIN)).unwrap());
        assert_eq!(mode_of(&path::join(&out(tb), PRIVKEY)).unwrap(), 0o640);
        // 多了一个地址、证书机构连不上：1 分钟（12 次 5 秒）、10 分钟（120 次）、1 小时（720 次）以后各再试一次，每次说一句；
        // 之后要隔 6 小时，中间那几轮（一小时一轮）不去要、不说
        std::fs::write(path::join(&teams, te), "198.51.100.7\n").unwrap();
        fake.until_naps(12 + 120 + 720 + 720 + 1, || serve_teams(&job));
        assert!(fake.naps.borrow().iter().all(|nap| *nap == 5.0));
        let failed = |minutes: u32| {
            format!("198.51.100.7 的证书没要到（{te} 用）：连不上证书机构（ConnectionRefused），手里没有用得了的证书。{minutes} 分钟后再试")
        };
        assert_eq!(fake.said(), [1, 10, 60, 360].map(failed));
        assert!(shelf.history().is_empty());
        // 再过 3 秒就该续的：睡 3 秒（不是 5 秒）就去要；没要到，手里那张照样放着
        std::fs::remove_file(path::join(&teams, te)).unwrap();
        fake.naps.borrow_mut().clear();
        let (start, end) = (timegm(2025, 1, 1, 0, 0, 0) as f64, timegm(2099, 12, 31, 23, 59, 59) as f64);
        let job = Job { fraction: (fake.now.get() + 3.0 - start) / (end - start), ..job };
        fake.until_naps(16, || serve_teams(&job));
        let naps = fake.naps.borrow().clone();
        assert!((naps[0] - 3.0).abs() < 0.001 && naps[1..].iter().all(|nap| (0.2..=5.0).contains(nap)), "{naps:?}");
        let failed = |minutes: u32| {
            format!("192.0.2.10 的证书没要到（{ta}、{tb} 用）：连不上证书机构（ConnectionRefused），手里的那张 {ENDS} 到期。{minutes} 分钟后再试")
        };
        assert_eq!(fake.said(), [1, 10].map(failed));
    }
}
