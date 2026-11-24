//! 监听：接连接、限名额、每条连接一个线程（握手、读请求、调 `handler::handle`、写回复）；升级成 WebSocket 的交给 `ws::pump`。
//! Python 的 `Server`（Py:2032-2129）。
//!
//! - 名额是每个监听自己的（`limits::Slots`：一共 64 个、同一个 IP 8 个，--behind-proxy 时只看总数）；满了直接断开，不排队
//! - 限流、用过的 nonce（`Ctx.guards`）是两个监听共用的
//! - 线程只用 `thread::Builder::spawn` 起：起不来就还掉名额、断开、在日志里说一句
//! - 接连接出错（比如文件描述符用完了）：等 50 毫秒接着接
//! - TLS 握手在处理这条连接的线程里做：握手时一次读写最多 5 秒，整条连接从接进来起最多 15 秒
//! - 成了 WebSocket 的连接要开很久：处理请求的名额现在就还，不再受 15 秒的期限管（Py:2053-2057）
//!
//! 现在只有接口：里面的还没移植。

use super::tls::CertCtx;
use super::Ctx;
use rustls::ServerConfig;
use std::net::TcpListener;
use std::sync::Arc;

/// 这个监听说什么
pub enum Tls {
    /// 只说 HTTP：--behind-proxy（HTTPS 由前面的代理做），或者本机试网页端的那个监听（http://localhost）
    Plain,
    /// 一直是这一对证书、私钥（--cert、--key）
    Fixed(Arc<ServerConfig>),
    /// 网页端另开的端口：证书会换，每条连接问一次现在用哪一份；还没有证书时不握手、直接断开
    Reload(Arc<CertCtx>),
}

/// 在这个监听上一直接连接、处理，不返回（Python 的 `serve_forever`）。proxied：在代理后面
pub fn serve(l: TcpListener, tls: Tls, proxied: bool, ctx: Arc<Ctx>) -> ! {
    let _ = (l, tls, proxied, ctx);
    crate::err!("cca-core team-server：未完成（Rust 版还在移植：front::serve）");
    crate::py::io::finish();
    std::process::exit(2)
}
