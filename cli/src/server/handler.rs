//! 一个请求怎么处理：Python 的 `Handler` 里的 do_GET、do_POST、team_op、static、websocket、client_ip（Py:1817-2029）。
//!
//! 这里不碰连接：请求读好了（`http::Request`）进来，出去的是要回什么（`Outcome`），由 front.rs 写出去。检查的先后、每一步回的
//! 状态码和 error 都照 Python（设备、界面 App、测试都是照着这些分情况的）：
//!
//! POST /v1/<op>（Py:1901-1997）：
//! 1. 按 IP 限流（429 rate）；Content-Length 不是数（411 bad）、是负的或者过了上限（413 bad；keys、ledger、team_put 是 1 MiB，
//!    别的 64 KiB）——到这里为止是 `admit`：这几步在读正文之前，过了才去读那么多个字节
//! 2. 正文没读全、X-Cca-Sig 不是 base64、是空的、解出来超过 4096 个字节（400 bad）
//! 3. 不认识的 op（404 not_found；中转的那几个没开中转时也不认）
//! 4. 正文不是 JSON、不是字典、v 不是 1、op 和地址里的不一样、key 不是指纹的样子（400 bad）
//! 5. 验签名，三种：team_join 拿请求里要加入的那把公钥验（没有这个邀请和签名不对回一样的 403 denied；签名对了才说邀请是过期了
//!    还是用过了）；管理员的 op 拿 Team 的根公钥或者管理员名单里的那一把验（403 denied；换名单的只认根公钥）；别的拿名单里这台设备的公钥验（不在名单里：403 unknown_device
//!    带着 gen；发的是中转的请求时可能是访客，问 `Remote::guest_key`）
//! 6. 按指纹限流（验了签名才扣；管理员的 op 另算）（429 rate）；ts 和服务器的时间差超过 300 秒（400 clock 带着 now）；
//!    nonce 不是 32 个十六进制字符（400 bad）、用过了（409 replay）
//! 7. 记下这台设备刚来过，然后才是各个 op 自己的事：Team 的和 report、keys、ledger、view 在 `Store::op`，中转的在
//!    `remote::handle_op`
//!
//! GET（Py:1817-1888）：/v1/health；开着网页端时 /v1/relay/ws（升级成 WebSocket：握手的检查在 `ws::websocket`）和页面的文件
//! （`Web::file`、`Web::reply`）；别的 404 not_found。
//!
//! 存不了这一类的错（`Store::op`、`remote::handle_op` 给的 Err）：Python 里是没人接的异常，在日志里说一句
//! `cca-presence: 处理请求出错：…`、不回就断开（Py:2095-2096）。这里也是（`Outcome::Close`）。
//!
//! 锁：Store、Remote、Guards 各一把，这里从不同时拿两把——除了 `handle_op` 拿着 Remote 的锁回头问设备的名字（先 Remote 后
//! Store，只有这一个方向）。验签名不拿着锁做。
//!
//! 时间：`handle`、`admit`、`answer` 自己看钟；测试用带 `_at` 的那几个，时间是给的。

use super::http::{Method, Request};
use super::store::{say, Invite};
use super::{remote, sshsig, wire, ws, Ctx};
use super::{
    API, MAX_BODY, MAX_SIG, MAX_SYNC_BODY, NS_REPORT, RELAY_OPS, SYNC_OPS, TEAM_ADMIN_OPS, TEAM_OPS, TEAM_ROOT_OPS, TS_WINDOW,
};
use crate::dict;
use crate::py::obj::V;
use crate::py::{base64, ipaddr, json, text, time, uni, Fail, R};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// 一个请求的结果：front.rs 照它写回复
pub enum Outcome {
    /// 状态码和一个 JSON（紧凑的 UTF-8，带 Cache-Control: no-store）
    Json(u16, V),
    /// 别的回复（网页端的文件、304）：状态码、要带的头（照先后）、正文
    Raw { code: u16, headers: Vec<(String, String)>, body: Vec<u8> },
    /// 升级成 WebSocket：回 101（accept 是 Sec-WebSocket-Accept 的值），这条连接从此交给 WebSocket 的泵。已经占了网页端的
    /// 一个名额（`Web::take(ip)`）：连接结束时调用方一定要 `Web::free(ip)`
    Upgrade { accept: String, ip: String },
    /// 不回，直接断开（处理时出了错：已经在日志里说过了）
    Close,
}

/// 拿锁。别的线程拿着它时 panic 了（发布版是 abort，到不了；测试里会）也接着用：里面的状态每一步都是完整的
pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// JSON 的错误回复 `{"error": what}`（Python 的 `self.send(code, {'error': what})`）
pub(super) fn error(code: u16, what: &str) -> Outcome {
    Outcome::Json(code, V::Dict(dict! { "error" => what }))
}

/// 处理时出了错（存不了……）：在日志里说一句，不回（Py:2095-2096）
fn failed(e: &Fail) -> Outcome {
    say(format!("cca-presence: 处理请求出错：{e}"));
    Outcome::Close
}

/// 读正文之前的那几步（Py:1903-1912）：过了是正文有多少个字节（GET 没有这几步，是 0），没过是要回的
pub fn admit(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request) -> Result<usize, Outcome> {
    admit_at(ctx, peer, proxied, req, time::monotonic())
}

/// 正文读好以后（req.body：读到的那些，对方提前断开时比 n 短）的那几步。n 是 `admit` 给的
pub fn answer(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request, n: usize) -> Outcome {
    answer_at(ctx, peer, proxied, req, n, time::time(), time::monotonic())
}

/// mono 是单调时钟（限流用的）
fn admit_at(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request, mono: f64) -> Result<usize, Outcome> {
    if req.method != Method::Post {
        return Ok(0);
    }
    if !lock(&ctx.guards).ip.take(&client_ip(peer, proxied, req), mono) {
        return Err(error(429, "rate"));
    }
    let op = op_of(req);
    // 交换密文的、管理员存各台设备的配置的、存几位管理员共用的状态的
    let big = SYNC_OPS.iter().any(|(name, _)| *name == op) || op == "team_put" || op == "team_astate_put";
    match content_length(req.header("Content-Length").unwrap_or_default()) {
        Length::Invalid => Err(error(411, "bad")),
        Length::Bytes(n) if n <= if big { MAX_SYNC_BODY } else { MAX_BODY } => Ok(n),
        Length::Negative | Length::Bytes(_) => Err(error(413, "bad")),
    }
}

/// now 是服务器的时间（验 ts、记时间用的），mono 是单调时钟（限流用的）
fn answer_at(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request, n: usize, now: f64, mono: f64) -> Outcome {
    match req.method {
        Method::Get => get(ctx, peer, proxied, req, now, mono),
        Method::Post => post(ctx, req, n, now, mono),
    }
}

/// 地址里的 op：`/v1/` 后面的全部（带着 `?` 的话连它一起：那就哪个 op 都不是）（Py:1909）
fn op_of(req: &Request) -> &str {
    req.target.strip_prefix("/v1/").unwrap_or_default()
}

/// 按哪个 IP 限流（Py:1890-1899）：--behind-proxy 时是 X-Forwarded-For 最后一项（平台的代理加的；前面的是客户端自己写的，
/// 能乱填），没有、不像 IP 时和平时一样是连过来的地址
pub fn client_ip(peer: IpAddr, proxied: bool, req: &Request) -> String {
    if proxied {
        let all = req.header_all("X-Forwarded-For").join(",");
        let last = text::strip(all.rsplit(',').next().unwrap_or_default());
        if let Ok(ip) = ipaddr::ip_address(last) {
            return ip.text();
        }
    }
    peer.to_canonical().to_string() // 听 IPv6 时连过来的 IPv4 地址（::ffff:a.b.c.d）照 IPv4 的写
}

/// Content-Length 那个头读出来的
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Length {
    /// 不是数（Python 的 int() 不认）：411
    Invalid,
    /// 负的：413
    Negative,
    /// 这么多个字节（大得放不下的记成最大的数：反正过了上限）
    Bytes(usize),
}

/// 照 Python 的 `int(值)` 读（Py:1906；和真的服务器对过，见 tests/vectors/server_http.json）：前后的空白不算（ASCII 里只有空格、
/// \t \n \v \f \r 这六个——\x1c 到 \x1f 不算——，别的是 Unicode 认的空白），可以带一个正负号，数字之间可以隔一个下划线，
/// 数字多于 4300 个不认（sys.get_int_max_str_digits）
fn content_length(value: &str) -> Length {
    const MAX_DIGITS: usize = 4300;
    let space = |c: char| if c.is_ascii() { matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r') } else { uni::is_space(c) };
    let body = value.trim_matches(space);
    let (negative, digits) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body.strip_prefix('+').unwrap_or(body)),
    };
    let (mut n, mut count, mut after_digit) = (0usize, 0usize, false);
    for c in digits.chars() {
        if c == '_' && after_digit {
            after_digit = false;
            continue;
        }
        let Some(d) = uni::decimal_value(c).and_then(|d| usize::try_from(d).ok()) else {
            return Length::Invalid;
        };
        n = n.saturating_mul(10).saturating_add(d);
        count = count.saturating_add(1);
        after_digit = true;
    }
    if !after_digit || count > MAX_DIGITS {
        return Length::Invalid; // 一个数字都没有、结尾是下划线、太长
    }
    if negative && n > 0 {
        Length::Negative
    } else {
        Length::Bytes(n)
    }
}

// ---- GET ----

/// GET（Py:1817-1830）。网页端的两样——页面的文件（`Web::file`、`Web::reply`）和 WebSocket 的握手（`ws::websocket`）——
/// 怎么查、怎么回在 web.rs、ws.rs；这里只管分给谁，和它们要的两样共用的东西：按 IP 限流的那个桶、中转记着的票据
fn get(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request, now: f64, mono: f64) -> Outcome {
    let (path, _, query) = text::partition(&req.target, "?");
    if path == "/v1/health" {
        let relay_port = ctx.remote.as_ref().map(|sh| lock(&sh.m).port());
        let health = lock(&ctx.store).health(ctx.sig_ok, relay_port);
        if let (Some(web), Some(d)) = (&ctx.web, health.as_dict()) {
            d.set("web", web.url.as_str());
        }
        return Outcome::Json(200, health);
    }
    if let Some(web) = &ctx.web {
        // 用到时才算是哪个 IP、才扣它的配额（Py:1834、1863-1864：都在别的检查后面）
        let ip = || client_ip(peer, proxied, req);
        let bucket = |ip: &str| lock(&ctx.guards).ip.take(ip, mono);
        if path == "/v1/relay/ws" {
            let ip = ip();
            // 开了网页端就一定开着中转（启动时查过）；万一没有，哪张票据都不认
            let claim = |tid: &str| ctx.remote.as_ref().is_some_and(|sh| lock(&sh.m).ws_claim(tid, now));
            return ws::websocket(req, query, web, &ip, || bucket(&ip), claim);
        }
        if let Some(f) = web.file(path) {
            return web.reply(f, req, || bucket(&ip()));
        }
    }
    error(404, "not_found")
}

// ---- POST ----

fn post(ctx: &Ctx, req: &Request, n: usize, now: f64, mono: f64) -> Outcome {
    let op = op_of(req);
    let body = req.body.get(..n).unwrap_or(&req.body);
    // base64.b64decode(…, validate=True)：不是 base64 的当作没有签名
    let sig = base64::b64decode(req.header("X-Cca-Sig").unwrap_or_default(), true).unwrap_or_default();
    if body.len() != n || sig.is_empty() || sig.len() > MAX_SIG {
        return error(400, "bad");
    }
    // 没开中转时和旧版服务器一样：不认中转的那几个 op。别的都不认：以前维护者的服务器的设备清单（/v1/machines）也是
    let relay = ctx.remote.as_deref().filter(|_| RELAY_OPS.contains(&op));
    let sync = SYNC_OPS.iter().any(|(name, _)| *name == op);
    if !(matches!(op, "report" | "view") || sync || relay.is_some() || TEAM_OPS.contains(&op)) {
        return error(404, "not_found");
    }
    let Some(jv) = std::str::from_utf8(body).ok().and_then(|text| json::loads(text).ok()) else {
        return error(400, "bad");
    };
    let Some(j) = jv.as_dict() else {
        return error(400, "bad");
    };
    // j.get('v') == API：照 Python 的相等（1.0、true 也算）
    if !(j.get("v").is_some_and(|v| v.py_eq(&V::Int(API))) && j.str("op").as_deref() == Some(op)) {
        return error(400, "bad");
    }
    let Some(fp) = j.str("key").filter(|fp| wire::fp_re().matches(fp)) else {
        return error(400, "bad");
    };
    let fp: &str = &fp;

    // 签名：公钥那一行（ssh-ed25519 AAAA…）→ 是不是它在 cca-presence 这个命名空间里对正文签的
    let signed_by = |line: &str| sshsig::key_blob(line).is_some_and(|blob| sshsig::ssh_verify(&blob, NS_REPORT, body, &sig));
    let admin = TEAM_ADMIN_OPS.contains(&op) || TEAM_ROOT_OPS.contains(&op);
    let mut guest: Option<String> = None;
    if op == "team_join" {
        // 拿邀请加入：拿要加入的这把密钥签名；没有这个邀请和签名不对回一样的 403（不留判断口）。签名对了、邀请过期了或者用过了
        // 才说是哪一种（拿着真的邀请的人才问得出来）（Py:1935-1944）
        let line = j.str("pub").and_then(|p| wire::key_re().match_(&p).map(|k| format!("ssh-ed25519 {}", k.g(1))));
        let own = line.as_deref().filter(|line| sshsig::key_blob(line).is_some_and(|blob| sshsig::key_fp(&blob) == fp));
        let state = match own {
            Some(_) => {
                let mut st = lock(&ctx.store);
                match st.root_fp() == fp {
                    true => Invite::Missing,
                    false => st.invite_state(&j.get("token").unwrap_or(V::None), now),
                }
            }
            None => Invite::Missing,
        };
        if state == Invite::Missing || !own.is_some_and(signed_by) {
            return error(403, "denied");
        }
        if let Some(why) = state.error() {
            return error(403, why);
        }
    } else if admin {
        // 管理员的：拿 Team 的根密钥签名（key 是根公钥的指纹）（Py:1945-1947）；3.6.15 起也认根密钥签过的管理员名单里的密钥
        // （换名单的 team_admins 除外：只认根密钥）
        let root = {
            let st = lock(&ctx.store);
            match st.root_fp() == fp {
                true => Some(st.root_key().to_string()),
                false if TEAM_ROOT_OPS.contains(&op) => None,
                false => st.admin_key(fp),
            }
        };
        if !root.is_some_and(|line| signed_by(&line)) {
            return error(403, "denied");
        }
    } else {
        let key = lock(&ctx.store).machine_key(fp);
        match key {
            Some(line) => {
                if !signed_by(&line) {
                    return error(403, "denied");
                }
            }
            None => {
                // 不在名单里、发的是中转的请求：可能是访客（手机上的网页端，和哪台设备配了对、或者正拿着配对码来配对）。
                // 不在名单里（被移除了、记录清掉了：设备看 gen 分清是哪种）；访客的签名不对也回这一个：从回的内容里分不出
                // “没有这个配对”和“有、但签名不对”（Py:1949-1955）
                guest = relay.and_then(|sh| lock(&sh.m).guest_key(fp, op, &jv, now)).filter(|line| !line.is_empty());
                if !guest.as_deref().is_some_and(signed_by) {
                    let gen = lock(&ctx.store).gen().to_string();
                    return Outcome::Json(403, V::Dict(dict! { "error" => "unknown_device", "gen" => gen }));
                }
            }
        }
    }

    {
        // 验签之后才扣：别人拿这台设备的指纹乱发，扣不到它的配额（Py:1958-1960）
        let mut guards = lock(&ctx.guards);
        let bucket = if admin { &mut guards.root } else { &mut guards.fp };
        if !bucket.take(fp, mono) {
            return error(429, "rate");
        }
    }
    // abs(ts - now) > TS_WINDOW：布尔值不算数；NaN 比什么都不大，算在窗口里（Python 也是）；放不进 i64 的整数离现在远得很
    let window = TS_WINDOW as f64;
    let stale = match j.get("ts") {
        Some(V::Int(ts)) => (ts as f64 - now).abs() > window,
        Some(V::Float(ts)) => (ts - now).abs() > window,
        _ => true,
    };
    if stale {
        return Outcome::Json(400, V::Dict(dict! { "error" => "clock", "now" => now }));
    }
    let Some(nonce) = j.str("nonce").filter(|nonce| wire::nonce_re().matches(nonce)) else {
        return error(400, "bad");
    };
    if !lock(&ctx.guards).nonces.add(&nonce, now) {
        return error(409, "replay");
    }
    // 设备的请求。加入的那一次在加入时记（没加入成的不记）；访客不是这里的设备（Py:1969-1970）
    if op != "team_join" && !admin && guest.is_none() {
        lock(&ctx.store).seen(fp, now);
    }

    let done = if TEAM_OPS.contains(&op) {
        team_op(ctx, op, fp, &jv, now)
    } else if let Some(sh) = relay {
        remote::handle_op(sh, op, fp, &jv, now, guest.as_deref(), &|fp| lock(&ctx.store).name_of(fp))
    } else {
        lock(&ctx.store).op(op, fp, &jv, now)
    };
    match done {
        Ok((code, reply)) => Outcome::Json(code, reply),
        Err(e) => failed(&e),
    }
}

/// Team 的 op（Py:1971-1977、1999-2029）。有设备的密钥不再认了（管理员重新登记、移除了设备，设备自己退出了）：它的配对、会话
/// 跟着清掉——先放开 Store 的锁再去拿 Remote 的
fn team_op(ctx: &Ctx, op: &str, fp: &str, j: &V, now: f64) -> R<(u16, V)> {
    let mut st = lock(&ctx.store);
    let (code, reply) = st.op(op, fp, j, now)?;
    if let Some(sh) = ctx.remote.as_ref().filter(|_| code == 200 && matches!(op, "team_enroll" | "team_revoke" | "team_leave")) {
        let fps = st.fps();
        drop(st);
        lock(&sh.m).keep_only(&fps, now)?;
    }
    Ok((code, reply))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(in crate::server) mod tests;
