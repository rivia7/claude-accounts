//! `cca-core team-server`：Team 的同步服务器的参数和启动。Python 的 `main`（Py:2135-2263）。
//!
//! 参数、环境变量、出错时说的话都照 Python 的 argparse：
//! - 参数：--listen、--cert、--key、--behind-proxy、--mode（只有 team）、--team-root、--state、--ttl、--idle、--retain-days、--keys、
//!   --cost、--cost-days、--port-file、--relay-listen、--relay-port、--relay-port-file、--remote、--web-url、--web-listen、
//!   --web-cert、--web-key、--web-root、--web-name-file、--web-port-file
//! - 环境变量：CCA_HUB_HOSTED=1（托管平台上的预设：等于 --behind-proxy --listen 0.0.0.0:8080 --state /data/state.json）、
//!   CCA_TEAM_ROOT、CCA_WEB_URL、CCA_WEB_LISTEN、CCA_WEB_CERT、CCA_WEB_KEY
//! - 参数不对：`usage: cca-core team-server [options]` 和 `cca-core team-server: error: 原因`，退出码 2
//! - 启动时出错（证书读不了、端口占着、端口文件写不了）：一句话，退出码 1
//! - 起来以后在 stderr 说一句（监听在哪、设备几台、签名自检过没过……，Py:2253-2258），然后主线程接原来那个端口的连接；
//!   网页端另开的端口、中转各有自己的线程
//!
//! 现在只有入口：参数还不认，说一句“未完成”、退出码 2。

use crate::py::R;

/// `cca-core team-server …`
pub fn cmd_team_server(args: &[String]) -> R<()> {
    let _ = args;
    crate::err!("cca-core team-server：未完成（Rust 版的 Team 同步服务器还在移植，现在用 server/cca-presence.py）");
    crate::py::exit(2)
}
