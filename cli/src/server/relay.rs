//! 数据端口：两头（会话宿主、接入的设备或者网页端）各连进来，服务器把密文从一头搬到另一头，里面一个字节都不看。
//! Python 的 `RelayConn`、`Relay`（Py:1233-1569）。
//!
//! 一个线程：`libc::poll` 看着监听的套接字、所有连接和一个叫醒用的套接字（超时 1 秒，每秒清一次过期的）；套接字都不阻塞。
//! 网页端的 WebSocket 那一头（`ws::pump`）是 `UnixStream` 的一头：经 `Handle::adopt` 交进来，和数据端口上接进来的连接一样对待。
//! 线上来的字节（开场白那一行、控制帧）都经 `wire::Cur` 读；开场白的样子是 `wire::relay_line()`。
//!
//! 现在只有接口：里面的还没移植。

use super::remote::Shared;
use super::unfinished;
use crate::py::R;
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

/// 中转的线程在外面的把手
pub struct Handle {
    /// 等着交给中转的线程的连接（网页端的 WebSocket 那一头）和它的 IP
    adopted: Mutex<Vec<(UnixStream, String)>>,
}

impl Handle {
    /// 把一条字节流交给中转（Py:1497-1504）：放进队列、往叫醒用的套接字里写一个字节。中转那边连接太多时直接关掉它
    pub fn adopt(&self, sock: UnixStream, ip: String) {
        let _ = &self.adopted;
        drop((sock, ip)); // 还没移植：关掉（和中转满了一样）
    }
}

/// 起中转的线程（Py:1242-1259、1530-1569）。l 是已经绑好、在听的数据端口。线程起不来是 Err
pub fn start(l: TcpListener, sh: Arc<Shared>) -> R<Arc<Handle>> {
    let _ = (l, sh);
    unfinished("team-server 的中转（relay::start）")
}
