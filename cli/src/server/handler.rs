//! 一个请求怎么回：Python 的 `Handler.do_GET`、`static`、`websocket`、`client_ip`、`do_POST`、`team_op`（Py:1817-2029）。
//!
//! `handle` 不碰套接字：进来的是读好的请求，出去的是 `Outcome`（回什么），写出去、升级成 WebSocket 以后的事由 front.rs 做。
//! 这样每一种回复都能直接在测试里比。
//!
//! do_POST 的先后要和 Python 的一步不差（Py:1901-1997）：按 IP 限流 → Content-Length → 正文的上限（keys、ledger、team_put 是
//! `MAX_SYNC_BODY`，别的 `MAX_BODY`）→ 签名那个头 → 认不认这个 op（没开中转时不认中转的那几个）→ JSON → v、op、key →
//! 这把密钥是谁的、验签名（team_join 拿要加入的那把、管理员的 op 拿根密钥、访客拿 `Remote::guest_key` 给的）→ 按设备限流
//! （验签之后才扣）→ 时间 → nonce → 各个 op。
//!
//! 现在只有接口：里面的还没移植。

use super::http::Request;
use super::Ctx;
use crate::py::obj::V;
use std::net::IpAddr;

/// 一个请求的结果
pub enum Outcome {
    /// JSON 的回复：状态码、内容（写成紧凑的 UTF-8，带 `Cache-Control: no-store`，Py:1808-1815）
    Json(u16, V),
    /// 别的回复（网页端的文件、304）：状态码、要加的头（照先后）、正文
    Raw { code: u16, headers: Vec<(String, String)>, body: Vec<u8> },
    /// 升级成 WebSocket：accept 是 Sec-WebSocket-Accept 的值，ip 是限流用的那个地址（已经占了 `Web` 的一个名额：
    /// 这条连接完了要 `Web::free(ip)`）
    Upgrade { accept: String, ip: String },
    /// 不回，直接断开
    Close,
}

/// peer 是连过来的地址；proxied：在代理后面（--behind-proxy），限流照 X-Forwarded-For 的最后一项（Py:1890-1899）
pub fn handle(ctx: &Ctx, peer: IpAddr, proxied: bool, req: &Request) -> Outcome {
    let _ = (ctx, peer, proxied, req);
    Outcome::Close // 还没移植
}
