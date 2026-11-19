//! 出口检测的解析和 DNS / WebRTC 泄露检测（`coffee_parse` 到 `cmd_leak_verdict`）：coffee、leak-probe、leak-verdict。
//!
//! - `leak/coffee.rs`：`coffee_parse`、`cmd_coffee`（出口检测的一行结论和退出码）
//! - `leak/stun.rs`：`stun_parse`、`stun_probe`、`stun_ips`（本机 UDP 直连问 STUN）
//! - 这一份：`LEAK_*`、`leak_curl`、缓存和归属地（`leak_cache`、`ip_prefix`、`leak_geo`、`geo_fresh`、`leak_where`）、
//!   `leak_dns`、`cmd_leak_probe`、`cmd_leak_verdict`
//!
//! DNS / WebRTC 泄露检测（照 ip.net.coffee 的 DNS、WebRTC 检测页的做法）：和出口检测同时在后台跑，不拖慢启动。
//! DNS：经账号的端口访问两个一次性的子域名（`<token>-n.d.ip.net.coffee`），由账号出口那边去解析，ip.net.coffee 的权威 DNS
//! 记下是哪些解析器来查的（`/api/dns/result/<token>`）；有中国大陆的解析器、出口又不在中国大陆就是泄露。要几秒，
//! 结果按 线路|节点 缓存。WebRTC：用本机 UDP 探测 STUN，检查绕过账号代理的风险；不在浏览器里执行，不能据此断言浏览器是否泄露。
//!
//! 线程：原来的线程直接往共享的字典、列表里写。这里 `V` 不能跨线程：线程只带着 `String`、数字进去，结果经通道交回，
//! 由等它的那一方按“谁先做完”的先后记下来（和原来写进字典的先后一样）。到时间还没做完的不等了，进程退出时一起结束
//! （原来是 daemon 线程）；它起的 curl 照旧自己跑到超时。

mod coffee;
mod stun;

#[allow(unused_imports)] // coffee_parse：health-save 也用它，那个模块移植时从这里拿
pub use coffee::{cmd_coffee, coffee_parse};
pub use stun::stun_ips;

use super::{arg, arity, Cmd};
use crate::core::base::{str_or_empty, CCA_HOME, CFG};
use crate::core::tool::t;
use crate::py::fs::{load_quiet, save_json};
use crate::py::ipaddr;
use crate::py::json;
use crate::py::obj::{repr_str, str_list, Dict, List, V};
use crate::py::path;
use crate::py::proc::{Cmd as Proc, In, Out};
use crate::py::rand;
use crate::py::re::Re;
use crate::py::text;
use crate::py::time::{sleep, time};
use crate::py::{Fail, R};
use crate::{dict, out, re};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::LazyLock;
use std::time::Duration;

/// 缓存：DNS 的结果（按 线路|节点）、IP 归属地
pub static LEAK_FILE: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "leak.json"));
/// DNS 怎么解析取决于节点和路由器的设置，不常变
pub const LEAK_DNS_TTL: f64 = 6.0 * 3600.0;
/// 没测成的过 10 分钟再测
pub const LEAK_FAIL_TTL: f64 = 600.0;
pub const LEAK_GEO_TTL: f64 = 7.0 * 86400.0;
/// 后台检测最多这么多秒（和出口检测同时跑）
pub const LEAK_DEADLINE: f64 = 5.0;
/// cca check（用户就在等检测结果）：慢的线路上一次 HTTPS 往返要三四秒（2026-11-11 一台 Linux 服务器上实测），
/// 5 秒里问不到解析器，DNS 永远没测成。测成的缓存 6 小时，之后启动时也用得上
pub const LEAK_CHECK_DEADLINE: f64 = 20.0;
pub const COFFEE: &str = "https://ip.net.coffee";
pub static IP_TEXT: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[0-9A-Fa-f:.]{2,45}$"));

/// `type(x).__name__`（照 Python 说没人接的 TypeError / AttributeError 时用）
fn type_name(v: &V) -> &'static str {
    match v {
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::List(_) => "list",
        V::Dict(_) => "dict",
    }
}

/// `for t in ts: t.join(max(end - time.time(), 0))`：等线程做完，最晚等到 end。线程把结果发进通道，做完时放掉自己那份
/// 发送端；都放掉了就是都做完了。返回按做完的先后收到的结果，到时间还没来的不要了
fn join_until<T>(rx: &mpsc::Receiver<T>, end: f64) -> Vec<T> {
    let mut got = Vec::new();
    loop {
        let left = (end - time()).max(0.0);
        match rx.recv_timeout(Duration::from_secs_f64(left)) {
            Ok(x) => got.push(x),
            Err(_) => return got,
        }
    }
}

/// 经 curl 取网址（测试里的假 curl 照样管得到）：proxy 是账号的端口，空的是直连。拿不到返回 None
fn leak_curl(url: &str, proxy: &str, timeout: f64) -> Option<String> {
    let timeout = timeout.max(0.5);
    let limit = format!("{timeout:.1}");
    let mut argv = vec!["curl", "-q", "-s", "-m", &limit];
    if proxy.is_empty() {
        argv.extend(["--noproxy", "*"]);
    } else {
        argv.extend(["--noproxy", "", "-x", proxy]);
    }
    argv.push(url);
    // 起不来、超时（多给 2 秒，到时杀掉）、退出码不是 0 都算拿不到
    match Proc::new(argv).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).timeout(timeout + 2.0).run() {
        Ok(done) if done.rc == 0 => Some(done.out()),
        _ => None,
    }
}

fn leak_json(text: Option<String>) -> Option<V> {
    text.filter(|t| !t.is_empty()).and_then(|t| json::loads(&t).ok())
}

pub fn leak_cache() -> Dict {
    match load_quiet(&LEAK_FILE) {
        Some(V::Dict(j)) => j,
        _ => Dict::new(),
    }
}

/// 归属地按网段记：IPv4 的 /24、IPv6 的 /48（公共 DNS 的解析器常在几个网段里换着来，按 IP 记几乎每次都对不上）
pub fn ip_prefix(ip: &str) -> String {
    if ip.contains(':') {
        // ipaddress.ip_network(ip + '/48', strict=False)：认不出就原样。里面已经有斜杠的（多于一个斜杠）也认不出
        let parsed = if ip.contains('/') { None } else { ipaddr::ip_address(ip).ok().filter(|p| p.ip.version() == 6) };
        let Some(p) = parsed else {
            return ip.to_string();
        };
        let net = p.ip.network(48);
        // 带区域标识的：后 80 位本来就是 0 时网段地址还带着它，否则换成不带的
        let whole = ipaddr::Parsed { ip: p.ip, scope: None }.text();
        return match &p.scope {
            Some(zone) if net == format!("{whole}/48") => format!("{whole}%{zone}/48"),
            _ => net,
        };
    }
    let p: Vec<&str> = ip.split('.').collect();
    if p.len() == 4 {
        format!("{}.0/24", p[..3].join("."))
    } else {
        ip.to_string()
    }
}

/// 查到的一个网段的归属地（线程里查，带回来再写成字典）
struct GeoHit {
    t: f64,
    cc: String,
    country: String,
    isp: String,
    miss: bool,
}

impl GeoHit {
    fn dict(&self) -> Dict {
        if self.miss {
            dict! { "t" => self.t, "cc" => "", "country" => "", "isp" => "", "miss" => true }
        } else {
            dict! { "t" => self.t, "cc" => &self.cc, "country" => &self.country, "isp" => &self.isp }
        }
    }
}

/// `leak_geo` 里的 `one`：查一个 IP 的归属地
fn geo_one(ip: &str, proxy: &str, timeout: f64) -> GeoHit {
    match leak_json(leak_curl(&format!("{COFFEE}/api/geoip/{ip}"), proxy, timeout)) {
        Some(V::Dict(j)) => GeoHit {
            t: time(),
            cc: str_or_empty(&j.get("country_code")).to_lowercase(),
            country: str_or_empty(&j.get("country")),
            isp: str_or_empty(&j.get("isp")),
            miss: false,
        },
        // 查不到也记下（一小时）：ip.net.coffee 连不上时别每次启动都等它
        _ => GeoHit { t: time(), cc: String::new(), country: String::new(), isp: String::new(), miss: true },
    }
}

/// `leak_geo` 不碰 `V` 的那一半（可以在线程里跑）：这些 IP 各自的网段里，known 里有的（第一个返回值，照 IP 的先后），
/// 别的每个网段挑一个 IP 同时查（第二个返回值，照查完的先后）
fn geo_fetch(ips: &[String], proxy: &str, known: &dyn Fn(&str) -> bool, timeout: f64) -> (Vec<String>, Vec<(String, GeoHit)>) {
    let mut want: Vec<(String, String)> = Vec::new(); // want.setdefault(ip_prefix(ip), ip)
    for ip in ips {
        if IP_TEXT.matches(ip) {
            let k = ip_prefix(ip);
            if !want.iter().any(|(have, _)| *have == k) {
                want.push((k, ip.clone()));
            }
        }
    }
    let (have, todo): (Vec<_>, Vec<_>) = want.into_iter().partition(|(k, _)| known(k));
    let timeout = if timeout != 0.0 {
        timeout
    } else if proxy.is_empty() {
        2.0
    } else {
        4.0
    };
    let (tx, rx) = mpsc::channel();
    let ts: Vec<_> = todo
        .into_iter()
        .map(|(k, ip)| {
            let (tx, proxy) = (tx.clone(), proxy.to_string());
            std::thread::spawn(move || {
                let hit = geo_one(&ip, &proxy, timeout);
                let _ = tx.send((k, hit));
            })
        })
        .collect();
    drop(tx);
    for t in ts {
        let _ = t.join();
    }
    (have.into_iter().map(|(k, _)| k).collect(), rx.try_iter().collect())
}

/// 这些 IP 所在网段的归属地 {网段: {t, cc, country, isp}}：known 里有的不再查，别的每个网段挑一个 IP 同时查 ip.net.coffee 的
/// /api/geoip。timeout 不给（0）时经代理 4 秒、直连 2 秒
pub fn leak_geo(ips: &[String], proxy: &str, known: &Dict, timeout: f64) -> Dict {
    let (have, hits) = geo_fetch(ips, proxy, &|k| known.has(k), timeout);
    let out = Dict::new();
    for k in have {
        out.set(k.as_str(), known.at(&k));
    }
    for (k, hit) in hits {
        out.set(k, hit.dict());
    }
    out
}

/// `now - x`、`-x` 里的 x：数字（布尔值也算）；别的在 Python 里是没人接的 TypeError
fn number(v: &V) -> f64 {
    v.as_pynum().unwrap_or_else(|| panic!("TypeError: bad operand type for a number: '{}'", type_name(v)))
}

pub fn geo_fresh(g: &V, now: f64) -> bool {
    let V::Dict(g) = g else {
        return false;
    };
    now - g.get("t").map_or(0.0, |t| number(&t)) < if g.truthy("miss") { 3600.0 } else { LEAK_GEO_TTL }
}

/// 归属地写成一句：国家 · ISP
pub fn leak_where(g: Option<&V>) -> String {
    // (g or {}).get('country')、(g or {}).get('isp')
    let g = match g {
        Some(V::Dict(g)) => g,
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(v)),
        _ => return String::new(),
    };
    let mut parts = Vec::new();
    for k in ["country", "isp"] {
        match g.get(k).filter(|x| x.truthy()) {
            Some(V::Str(s)) => parts.push(s.to_string()),
            Some(v) => panic!("TypeError: sequence item {}: expected str instance, {} found", parts.len(), type_name(&v)),
            None => {}
        }
    }
    parts.join(" · ")
}

/// 经账号的端口解析两个一次性的子域名，再问 ip.net.coffee 是哪些解析器来查的：(ok, servers)。
/// 要的只是那边的解析器来查过：不等图片请求做完（HTTPS 往返要一两秒），最多一秒就开始问。patient（cca check）：每个请求多等一会儿
fn leak_dns(proxy: &str, deadline: f64, patient: bool) -> (bool, Vec<String>) {
    let tok = &rand::uuid4().replace('-', "")[..20]; // uuid.uuid4().hex[:20]
    // 两个请求各等多久在起线程之前就算好了（原来是建 Thread 对象时求的值）
    let limits: Vec<f64> = (0..2).map(|_| f64::min(if patient { 6.0 } else { 2.5 }, deadline - time())).collect();
    let (tx, rx) = mpsc::channel::<()>();
    for (i, limit) in limits.into_iter().enumerate() {
        let (url, proxy, tx) = (format!("https://{}-{}.d.ip.net.coffee/pixel.gif", tok, i + 1), proxy.to_string(), tx.clone());
        std::thread::spawn(move || {
            leak_curl(&url, &proxy, limit);
            drop(tx);
        });
    }
    drop(tx);
    join_until(&rx, f64::min(time() + 1.0, deadline));
    let (mut ok, mut servers) = (false, Vec::new());
    // 一般第一次就有；没有时稍等再问
    for n in 0..4 {
        if deadline - time() < 0.5 {
            break;
        }
        let url = format!("{COFFEE}/api/dns/result/{tok}");
        let j = leak_json(leak_curl(&url, proxy, f64::min(if patient { 8.0 } else { 3.0 }, deadline - time())));
        if let Some(list) = j.and_then(|j| j.as_dict()).and_then(|j| j.list("dns_servers")) {
            servers = list.strs().iter().filter(|s| IP_TEXT.matches(s)).map(|s| s.to_string()).collect();
            ok = !servers.is_empty();
            if ok {
                break;
            }
        }
        if n < 3 {
            sleep((deadline - time() - 0.5).clamp(0.0, 0.5)); // min(0.5, max(deadline - time.time() - 0.5, 0))
        }
    }
    // list(dict.fromkeys(servers))[:16]
    let mut uniq: Vec<String> = Vec::new();
    for s in servers {
        if !uniq.contains(&s) {
            uniq.push(s);
        }
    }
    uniq.truncate(16);
    (ok, uniq)
}

/// 缓存的 DNS 结果还能用：没测成的 10 分钟、测成的 6 小时；since（cca check 开始的时间）之前测的不算
fn leak_dns_fresh(d: Option<&V>, since: f64) -> bool {
    let Some(V::Dict(d)) = d else {
        return false;
    };
    let Some(at) = d.get("t").filter(|t| t.isinstance_num()).and_then(|t| t.as_pynum()) else {
        return false;
    };
    if at < since {
        return false;
    }
    time() - at < if d.truthy("ok") { LEAK_DNS_TTL } else { LEAK_FAIL_TTL }
}

/// `cache.get(k) or {}`：缓存里的一节。不是字典在 Python 里是没人接的 AttributeError（接下来就要 .items() / .get()）
fn section(cache: &Dict, k: &str) -> Dict {
    match cache.get(k) {
        Some(V::Dict(d)) => d,
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'items'", type_name(&v)),
        _ => Dict::new(),
    }
}

/// leak-probe 的两个后台任务交回来的结果
enum Probed {
    Stun(Vec<String>),
    Dns { ok: bool, servers: Vec<String>, have: Vec<String>, hits: Vec<(String, GeoHit)> },
}

/// 后台跑（和出口检测同时）：本机 STUN 直连检查潜在的 WebRTC 绕行风险；DNS 缓存不能用时经账号端口测一次。
/// 结果（JSON）打到 stdout，由 leak-verdict 汇总、写缓存。config.json 的 leak_check 设成 false 时什么都不测。
/// webrtc 是 0 时不测 WebRTC（服务器上没有账号专属的浏览器和桌面 App）：结果里写明 webrtc: off，leak-verdict 不说它
/// （只是没有 stun 的结果照旧算没测成：探测没做完、文件坏了都不能当作没问题）
pub fn cmd_leak_probe(args: &[String]) -> R<()> {
    arity("cmd_leak_probe", args, 2, 4)?;
    let (proxy, key, since, webrtc) = (args[0].as_str(), args[1].as_str(), arg(args, 2, ""), arg(args, 3, "1"));
    if let Some(V::Dict(c)) = load_quiet(&CFG) {
        if matches!(c.get("leak_check"), Some(V::Bool(false))) {
            out!("{}", json::dumps(&V::Dict(dict! { "off" => true }), json::Fmt::new()));
            return Ok(());
        }
    }
    let cache = leak_cache();
    let patient = !since.is_empty(); // cca check：给了开始时间
    let deadline = time() + if patient { LEAK_CHECK_DEADLINE } else { LEAK_DEADLINE };
    let res = if webrtc != "0" { dict! { "stun" => dict! { "ips" => List::new() } } } else { dict! { "webrtc" => "off" } };
    let known = Dict::from_pairs(section(&cache, "geo").items().into_iter().filter(|(_, g)| geo_fresh(g, time())));
    let cached = section(&cache, "dns").get(key);
    let since = match since {
        "" => 0.0,
        s => text::to_float(s).ok_or_else(|| Fail::Value(format!("could not convert string to float: {}", repr_str(s))))?,
    };
    let (tx, rx) = mpsc::channel();
    if res.has("stun") {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Probed::Stun(stun_ips(1.0)));
        });
    }
    if !leak_dns_fresh(cached.as_ref(), since) {
        // 线程里只要知道哪些网段已经有归属地（不用再查），归属地本身留在这边
        let fresh: Vec<String> = known.keys().iter().map(|k| k.to_string()).collect();
        let (tx, proxy) = (tx.clone(), proxy.to_string());
        std::thread::spawn(move || {
            let (ok, servers) = leak_dns(&proxy, deadline, patient);
            let (have, hits) = geo_fetch(&servers, &proxy, &|k| fresh.iter().any(|f| f == k), if patient { 8.0 } else { 0.0 });
            let _ = tx.send(Probed::Dns { ok, servers, have, hits });
        });
    }
    drop(tx);
    // 到时间还没做完的不要了；汇总时没有结果按未验证处理
    for done in join_until(&rx, deadline + 1.0) {
        match done {
            Probed::Stun(ips) => res.set("stun", dict! { "ips" => str_list(ips) }),
            Probed::Dns { ok, servers, have, hits } => {
                let geo = Dict::new();
                for k in have {
                    geo.set(k.as_str(), known.at(&k));
                }
                for (k, hit) in hits {
                    geo.set(k, hit.dict());
                }
                res.set("dns", dict! { "ok" => ok, "servers" => str_list(servers), "geo" => geo });
            }
        }
    }
    out!("{}", json::dumps(&V::Dict(res), json::Fmt::new()));
    Ok(())
}

/// `for x in (v or [])`：Python 里能这样遍历的 JSON 值——列表的项、字符串的一个个字符、字典的键；别的（数字、true）是没人接的 TypeError
fn py_iter(v: Option<V>) -> Vec<V> {
    match v {
        Some(V::List(l)) => l.items(),
        Some(V::Str(s)) => s.chars().map(|c| V::str(c.to_string())).collect(),
        Some(V::Dict(d)) => d.keys().into_iter().map(V::Str).collect(),
        Some(v) if v.truthy() => panic!("TypeError: '{}' object is not iterable", type_name(&v)),
        _ => Vec::new(),
    }
}

/// `d.update(v or {})`，v 是探测结果里读出来的、没验过的值。正常是字典。Python 的 update 还收“一串两项的东西”，
/// 所以 `[["网段", {…}]]`、`["us"]`（两个字符的文字：键 u、值 s）这样的列表也算数，照做；别的（数字、true、有哪一项不是两项的）
/// 是没人接的 TypeError / ValueError。键不是文字的（`[[5, {…}]]`）Python 收，这里的字典放不下，也按出错算——
/// 探测结果是 leak-probe 刚写的，只有手写的文件才会这样
fn dict_update(d: &Dict, v: Option<V>) {
    let pairs = match v {
        Some(V::Dict(other)) => return d.update(&other),
        Some(V::List(pairs)) => pairs.items(),
        Some(V::Str(s)) => s.chars().map(|c| V::str(c.to_string())).collect(),
        Some(v) if v.truthy() => panic!("TypeError: '{}' object is not iterable", type_name(&v)),
        _ => return,
    };
    for (i, pair) in pairs.iter().enumerate() {
        let two = match pair {
            V::None | V::Bool(_) | V::Int(_) | V::Big(_) | V::Float(_) => {
                panic!("TypeError: cannot convert dictionary update sequence element #{i} to a sequence")
            }
            other => py_iter(Some(other.clone())),
        };
        match &two[..] {
            [V::Str(k), v] => d.set(k.clone(), v),
            [_, _] => panic!("TypeError: 探测结果里 geo 的第 {i} 项的键不是文字"),
            _ => panic!("ValueError: dictionary update sequence element #{} has length {}; 2 is required", i, two.len()),
        }
    }
}

/// `(geo.get(k) or {}).get(field)`：这个网段记下的归属地里的一项。记的不是字典是没人接的 AttributeError
fn geo_field(geo: &Dict, k: &str, field: &str) -> Option<V> {
    match geo.get(k) {
        Some(V::Dict(g)) => g.get(field),
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(&v)),
        _ => None,
    }
}

/// `re.fullmatch('[a-z]{2}', cc or '')`：归属地的地区代码是两个小写字母（查到了）。不是文字是没人接的 TypeError
fn region_code(cc: Option<V>) -> bool {
    match cc.filter(|c| c.truthy()) {
        Some(V::Str(s)) => re!(r"[a-z]{2}").fullmatch(&s).is_some(),
        Some(v) => panic!("TypeError: expected string or bytes-like object, got '{}'", type_name(&v)),
        None => false,
    }
}

/// corenet 的 `udp_leak_hint`（WebRTC 直连风险的补充：这台 Mac 用内置代理时，怎么让它自己的 UDP 也经节点出去）。
/// corenet 还没移植。它开头的两条不靠 corenet——不是 macOS、没用内置代理都是空串——照原样写在这里，这样不用内置代理的
/// 机器上 WebRTC 直连风险那一句现在就是对的；用着内置代理时要看本机 UDP 走节点的状态，那一半等 corenet。
/// corenet 移植后，这个函数整个换成它的 `udp_leak_hint`
fn udp_leak_hint() -> String {
    if !cfg!(target_os = "macos") {
        return String::new();
    }
    let Some(V::Dict(c)) = load_quiet(&CFG) else {
        return String::new();
    };
    // (c.get('local') or {}).get('client') != 'builtin'
    let client = match c.get("local") {
        Some(V::Dict(local)) => local.get("client"),
        Some(v) if v.truthy() => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(&v)),
        _ => None,
    };
    if !matches!(client, Some(V::Str(s)) if &*s == "builtin") {
        return String::new();
    }
    udp_leak_hint_builtin()
}

/// 用着内置代理时的那一半：要 `udp_state`（本机 UDP 走节点设了没有、起没起作用），在 corenet 里
fn udp_leak_hint_builtin() -> String {
    unimplemented!("用着内置代理时 WebRTC 直连风险后面的那句补充（corenet：udp_leak_hint）")
}

/// 汇总 leak-probe 的结果（exit_ip / exit_cc 是这次出口检测拿到的）：第一行是给横幅的一句话，每个泄露再一行
/// W<TAB>dns|webrtc<TAB>说明，只作提醒的一行 N<TAB>webrtc<TAB>说明，最后是机器看的状态 S<TAB>dns|webrtc<TAB>ok|leak|note|failed
/// （出口检测结果里记下，见 health-save；leak_finish 只认 W、N、S 行）；把新结果写进缓存。detail（cca check）时一句话里多写点。
/// 本机 STUN IP 直连查归属地：别让账号的出口和它一起出现在别人那里
pub fn cmd_leak_verdict(args: &[String]) -> R<()> {
    arity("cmd_leak_verdict", args, 4, 5)?;
    let (path, key, exit_ip, exit_cc, detail) = (args[0].as_str(), args[1].as_str(), args[2].as_str(), args[3].as_str(), arg(args, 4, ""));
    let Some(V::Dict(p)) = load_quiet(path) else {
        return Ok(());
    };
    if p.truthy("off") {
        return Ok(());
    }
    let (now, cache) = (time(), leak_cache());
    let geo = Dict::from_pairs(section(&cache, "geo").items().into_iter().filter(|(_, g)| geo_fresh(g, now)));
    let dns = Dict::from_pairs(section(&cache, "dns").items().into_iter().filter(|(_, v)| leak_dns_fresh(Some(v), 0.0)));
    if let Some(found) = p.dict("dns") {
        dict_update(&geo, found.get("geo")); // geo.update(p['dns'].get('geo') or {})
        let servers: Vec<V> = py_iter(found.get("servers")).into_iter().filter(|s| s.is_str()).collect();
        dns.set(key, dict! { "t" => now, "ok" => found.truthy("ok"), "servers" => servers });
    }
    let rtc = !matches!(p.get("webrtc"), Some(V::Str(s)) if &*s == "off"); // 服务器上没测 WebRTC：不说它（没有 stun 的结果照旧算没测成）
    let st = p.dict("stun").unwrap_or_default();
    let ips: Vec<String> = py_iter(st.get("ips")).iter().filter_map(|ip| ip.as_str().map(String::from)).collect();
    let leaked: Vec<String> = ips.iter().filter(|ip| ip.as_str() != exit_ip).cloned().collect();
    geo.update(&leak_geo(&leaked, "", &geo, 0.0));
    let (mut parts, mut warns, mut notes, mut state) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if let Some(V::Dict(d)) = dns.get(key) {
        // sv = d.get('servers') or []：缓存里的没筛过，不是文字的项到 ip_prefix 那里是没人接的异常
        let sv: Vec<Rc<str>> = py_iter(d.get("servers"))
            .into_iter()
            .map(|s| s.rc_str().unwrap_or_else(|| panic!("TypeError: 解析器不是文字（{}）", type_name(&s))))
            .collect();
        let cn: Vec<&Rc<str>> = sv.iter().filter(|s| matches!(geo_field(&geo, &ip_prefix(s), "cc"), Some(V::Str(c)) if &*c == "cn")).collect();
        let unknown = sv.iter().filter(|s| !region_code(geo_field(&geo, &ip_prefix(s), "cc"))).count();
        if d.truthy("ok") && !cn.is_empty() && !exit_cc.is_empty() && exit_cc.to_uppercase() != "CN" {
            state.push(("dns", "leak"));
            parts.push(format!("DNS 泄露（{} 个解析器在中国大陆）", cn.len()));
            let listed: Vec<String> = cn
                .iter()
                .take(3)
                .map(|s| text::strip(&format!("{} {}", s, leak_where(geo.get(&ip_prefix(s)).as_ref()))).to_string())
                .collect();
            warns.push((
                "dns",
                format!(
                    "DNS 泄露：经这个账号的端口访问网站时，域名是中国大陆的 DNS 服务器解析的（{}），网站的 DNS 服务\
                     能看出查询来自中国大陆。检查路由器（在外面是本机 Clash）上的节点 {}：mihomo 把域名在本地解析好再交给节点时\
                     （比如直连、WireGuard 这类节点），用的是它自己的 DNS",
                    listed.join("、"),
                    key.split_once('|').map_or(key, |(_, node)| node)
                ),
            ));
        } else if !d.truthy("ok") || sv.is_empty() || unknown > 0 || exit_cc.is_empty() {
            state.push(("dns", "failed"));
            d.set("ok", false); // 未验证的旧缓存也只保留失败结果的短 TTL
            let reason = if !sv.is_empty() && unknown > 0 {
                "解析器归属地不完整"
            } else if sv.is_empty() {
                "未取得解析器"
            } else {
                "探测未完成"
            };
            parts.push(format!("DNS 没测成（{reason}）"));
        } else {
            state.push(("dns", "ok"));
            // cs = list(dict.fromkeys((geo.get(ip_prefix(s)) or {}).get('country') or '?' for s in sv))
            let mut cs: Vec<V> = Vec::new();
            for s in &sv {
                let c = geo_field(&geo, &ip_prefix(s), "country").filter(|c| c.truthy()).unwrap_or_else(|| "?".into());
                if c.is_list() || c.is_dict() {
                    panic!("TypeError: unhashable type: '{}'", type_name(&c));
                }
                if !cs.iter().any(|have| have.py_eq(&c)) {
                    cs.push(c);
                }
            }
            let more = if detail.is_empty() {
                String::new()
            } else {
                let shown: Vec<String> = cs
                    .iter()
                    .take(3)
                    .map(|c| match c {
                        V::Str(s) => s.to_string(),
                        other => panic!("TypeError: sequence item 0: expected str instance, {} found", type_name(other)),
                    })
                    .collect();
                format!("（{} 个解析器：{}{}；非浏览器实测）", sv.len(), shown.join("、"), if cs.len() > 3 { "等" } else { "" })
            };
            parts.push(format!("DNS 未发现地区异常{more}"));
        }
    } else {
        state.push(("dns", "failed"));
        parts.push("DNS 没测成（探测未完成）".to_string());
    }
    if !leaked.is_empty() {
        let cc_of = |ip: &str| geo_field(&geo, &ip_prefix(ip), "cc").filter(|cc| cc.truthy());
        let cn: Vec<&String> = leaked.iter().filter(|ip| matches!(cc_of(ip), Some(V::Str(c)) if &*c == "cn")).collect();
        let shown = cn.first().copied().unwrap_or(&leaked[0]);
        let w = leak_where(geo.get(&ip_prefix(shown)).as_ref());
        let at = if w.is_empty() { shown.clone() } else { format!("{shown}（{w}）") };
        if !cn.is_empty() {
            // 只有中国大陆的地址才算风险
            state.push(("webrtc", "leak"));
            parts.push(format!("WebRTC 直连风险：{at}"));
            warns.push((
                "webrtc",
                format!(
                    "WebRTC 直连风险：本机 UDP 的 STUN 探测看到中国大陆的地址 {}，与账号的出口不同。这是本机网络探测，\
                     尚未验证浏览器或 {} 是否会暴露该 IP；应在实际客户端验证 WebRTC 路由，并限制绕过账号代理的 UDP 流量{}",
                    at,
                    t().app_label,
                    udp_leak_hint()
                ),
            ));
        } else if leaked.iter().all(|ip| region_code(cc_of(ip))) {
            // 别的地区只作提醒
            state.push(("webrtc", "note"));
            parts.push(format!("WebRTC 备注：STUN 看到 {at}，与账号的出口不同"));
            notes.push((
                "webrtc",
                format!(
                    "本机 STUN 探测看到 {}，与账号的出口不同，但不是中国大陆的地址，只作提醒。尚未验证浏览器或 {} 是否会暴露该 IP",
                    at,
                    t().app_label
                ),
            ));
        } else {
            // 归属地查不到：说不清是不是中国大陆，也不能当作没问题
            state.push(("webrtc", "failed"));
            parts.push(format!("WebRTC 没测成（STUN 看到 {shown}，归属地没查到，浏览器状态未验证）"));
            notes.push(("webrtc", format!("本机 UDP 的 STUN 探测看到 {shown}，与账号的出口不同，但归属地没查到，判断不了是不是中国大陆的地址")));
        }
    } else if !ips.is_empty() {
        state.push(("webrtc", "ok"));
        parts.push("WebRTC：STUN 出口一致（浏览器状态未验证）".to_string());
    } else if rtc {
        state.push(("webrtc", "failed"));
        parts.push("WebRTC 没测成（STUN 探测未取得 IP，浏览器状态未验证）".to_string());
    }
    // keep = sorted(geo.items(), key=lambda x: -x[1].get('t', 0))[:300]：新的在前，一样新的保持原来的先后
    let mut keep: Vec<(f64, Rc<str>, V)> = geo
        .items()
        .into_iter()
        .map(|(k, g)| {
            let at = match &g {
                V::Dict(g) => g.get("t").map_or(0.0, |t| number(&t)),
                other => panic!("AttributeError: '{}' object has no attribute 'get'", type_name(other)),
            };
            (-at + 0.0, k, g) // 加 0.0：-0.0 和 0.0 算一样大（下面按全序比）
        })
        .collect();
    keep.sort_by(|a, b| a.0.total_cmp(&b.0));
    keep.truncate(300);
    cache.set("dns", dns);
    cache.set("geo", Dict::from_pairs(keep.into_iter().map(|(_, k, g)| (k, g))));
    match save_json(&LEAK_FILE, &V::Dict(cache), Some(0o600), true) {
        Err(e) if !e.is_os() => return Err(e),
        _ => {} // 写不了就算了（except OSError: pass）
    }
    if !parts.is_empty() {
        // 有横幅那一句才输出（它总在第一行）
        out!("{}", parts.join(" · "));
        for (kind, msg) in &warns {
            out!("W\t{kind}\t{msg}");
        }
        for (kind, msg) in &notes {
            out!("N\t{kind}\t{msg}");
        }
        for (kind, v) in &state {
            out!("S\t{kind}\t{v}");
        }
    }
    Ok(())
}

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("coffee", cmd_coffee), Cmd::all("leak-probe", cmd_leak_probe), Cmd::all("leak-verdict", cmd_leak_verdict)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_prefix_like_python() {
        // 右边是 Python 的 ip_prefix 给的
        for (ip, want) in [
            ("1.2.3.4", "1.2.3.0/24"),
            ("1.2.3", "1.2.3"),
            ("1.2.3.4.5", "1.2.3.4.5"),
            ("...", "...0/24"),
            ("1.2.3.4\n", "1.2.3.0/24"),
            ("2001:db8:1:2::1", "2001:db8:1::/48"),
            ("::ffff:1.2.3.4", "::/48"),
            ("fe80::1%eth0", "fe80::/48"),
            ("fe80::%eth0", "fe80::%eth0/48"),
            ("2001:db8:1::%eth0", "2001:db8:1::%eth0/48"),
            ("::1/64", "::1/64"),
            ("zz:1", "zz:1"),
            (":", ":"),
            ("2001:db8::1\n", "2001:db8::1\n"),
            ("2001:DB8:AB:1::", "2001:db8:ab::/48"),
            ("1:2:3:4:5:6:7:8", "1:2:3::/48"),
            ("::", "::/48"),
            ("a", "a"),
            ("", ""),
            ("2001:db8::1%", "2001:db8::1%"),
            ("1.2.3.4:80", "1.2.3.4:80"),
            ("::1.2.3.4", "::/48"),
            ("2001:db8:85a3::8a2e:370:7334", "2001:db8:85a3::/48"),
            ("2001:db8::%a%b", "2001:db8::%a%b"),
            (" ::1", " ::1"),
            ("٣::1", "٣::1"),
            ("2001:db8:1:0:0::%e/x", "2001:db8:1:0:0::%e/x"),
        ] {
            assert_eq!(ip_prefix(ip), want, "{ip:?}");
        }
    }

    #[test]
    fn curl_time_limit_is_written_like_python() {
        // '%.1f' % x：按这个小数真正的值取到一位，正好一半时取偶数
        for (x, want) in [(0.25, "0.2"), (0.35, "0.3"), (2.45, "2.5"), (2.5, "2.5"), (0.05, "0.1"), (3.0, "3.0"), (0.75, "0.8"), (4.96, "5.0")] {
            assert_eq!(format!("{x:.1}"), want);
        }
    }

    #[test]
    fn freshness() {
        let now = time();
        let geo = |age: f64, miss: bool| {
            let g = dict! { "t" => now - age, "cc" => "us" };
            if miss {
                g.set("miss", true);
            }
            V::Dict(g)
        };
        assert!(geo_fresh(&geo(60.0, false), now) && geo_fresh(&geo(LEAK_GEO_TTL - 5.0, false), now) && !geo_fresh(&geo(LEAK_GEO_TTL + 5.0, false), now));
        // 没查到的只记一小时
        assert!(geo_fresh(&geo(3590.0, true), now) && !geo_fresh(&geo(3610.0, true), now));
        // 没有 t 的当作很久以前；不是字典的不算
        assert!(!geo_fresh(&V::Dict(dict! { "cc" => "us" }), now) && !geo_fresh(&V::str("us"), now) && !geo_fresh(&V::None, now));
        assert!(geo_fresh(&V::Dict(dict! { "t" => true }), 5.0)); // 布尔值也是数
        let dns = |age: f64, ok: V| V::Dict(dict! { "t" => now - age, "ok" => ok, "servers" => List::new() });
        // 测成的 6 小时、没测成的 10 分钟（tests/isolation_leaks.py：空的 DNS 结果只短暂缓存）
        assert!(leak_dns_fresh(Some(&dns(LEAK_DNS_TTL - 5.0, true.into())), 0.0) && !leak_dns_fresh(Some(&dns(LEAK_DNS_TTL + 5.0, true.into())), 0.0));
        assert!(leak_dns_fresh(Some(&dns(LEAK_FAIL_TTL - 5.0, false.into())), 0.0) && !leak_dns_fresh(Some(&dns(LEAK_FAIL_TTL + 1.0, false.into())), 0.0));
        assert!(!leak_dns_fresh(Some(&dns(LEAK_FAIL_TTL + 1.0, V::None)), 0.0) && !leak_dns_fresh(Some(&dns(LEAK_FAIL_TTL + 1.0, 0.into())), 0.0));
        // since（cca check 开始的时间）之前测的不算
        assert!(!leak_dns_fresh(Some(&dns(60.0, true.into())), now - 30.0) && leak_dns_fresh(Some(&dns(60.0, true.into())), now - 90.0));
        // 没有 t、t 不是数字、不是字典
        assert!(!leak_dns_fresh(Some(&V::Dict(dict! { "ok" => true })), 0.0));
        assert!(!leak_dns_fresh(Some(&V::Dict(dict! { "t" => "now", "ok" => true })), 0.0));
        assert!(!leak_dns_fresh(Some(&V::str("x")), 0.0) && !leak_dns_fresh(None, 0.0));
    }

    #[test]
    fn where_and_iteration() {
        let g = V::Dict(dict! { "country" => "China", "isp" => "Example CN" });
        assert_eq!(leak_where(Some(&g)), "China · Example CN");
        assert_eq!(leak_where(Some(&V::Dict(dict! { "country" => "", "isp" => "ISP" }))), "ISP");
        assert_eq!(leak_where(Some(&V::Dict(dict! { "country" => "US", "isp" => V::None }))), "US");
        assert_eq!(leak_where(Some(&V::Dict(Dict::new()))), "");
        assert_eq!(leak_where(Some(&V::None)), "");
        assert_eq!(leak_where(None), "");
        let strs = |v: V| py_iter(Some(v)).iter().map(|x| x.py_str()).collect::<Vec<_>>();
        assert_eq!(strs(V::str("ab")), ["a", "b"]);
        assert_eq!(strs(V::Dict(dict! { "k1" => 1, "k2" => 2 })), ["k1", "k2"]);
        assert!(py_iter(Some(V::Int(0))).is_empty() && py_iter(Some(V::None)).is_empty() && py_iter(None).is_empty());
        assert!(region_code(Some("cn".into())) && !region_code(Some("CN".into())) && !region_code(Some("usa".into())));
        assert!(!region_code(Some("".into())) && !region_code(None) && !region_code(Some(V::None)) && !region_code(Some("us\n".into())));
    }

    #[test]
    fn join_until_takes_what_is_ready() {
        let (tx, rx) = mpsc::channel();
        let slow = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send("fast");
        });
        let hang = std::thread::spawn(move || {
            sleep(1.0);
            let _ = slow.send("slow");
        });
        let t0 = time();
        assert_eq!(join_until(&rx, t0 + 0.2), ["fast"]);
        assert!((0.15..0.9).contains(&(time() - t0)));
        // 都做完了就不等到时间
        hang.join().unwrap();
        let t0 = time();
        assert_eq!(join_until(&rx, t0 + 5.0), ["slow"]);
        assert!(time() - t0 < 1.0);
    }
}
