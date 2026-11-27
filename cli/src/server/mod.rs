//! Team 的同步服务器（`cca-core team-server`）和证书助手（`cca-core team-acme`）。3.6.x 及以前它们是两份 Python 程序
//! （`server/cca-presence.py`、`server/cca-acme.py`），这里是照着移过来的；那两份已经不在仓库里了。
//!
//! 和别的子命令不一样的地方：
//! - 它们是常驻的服务，跑在 Team 管理员的 Linux 服务器上（systemd 的专用用户、容器里），不是这台机器上 cca 的一部分：不认
//!   “当前是哪种工具”、不读账号表。所以 main 一开头就按名字把它们分出去（`standalone`），辅助程序平常开头做的那几步都不做。
//! - 一个连接一个线程，发布版是 `panic = "abort"`：哪个线程 panic 一次，整个服务就没了。所以这棵目录里不许 `unwrap`、
//!   `expect`、下标、`panic!`、`unreachable!`（下面那行 lint 守着；测试模块各自放开）。线上来的字节经游标（`wire::Cur`）
//!   一段一段取，长度用 `checked_add`，还没查过上限的长度不拿去分配内存。
//! - `py::obj::V` 里是 `Rc`，不能跨线程：请求的 JSON 在处理它的那个线程里解析、检查，取成普通的值再放进共用的状态。
//! - 正则只用 `re!`（整个进程一份）；`py::re::compile` 的缓存是每个线程各一份的，这里不用。
//! - 启动的路上不加东西：这里没有一上来就算的全局量，用到时才算。
//!
//! 移的时候 Python 那一份是规矩：行为、回的内容、日志里的每句话都照它（已经部署出去的服务器换成这一份，设备那头看不出来）。
//! 注释里的「Python 那一份」「Py:行号」指 server/cca-presence.py，「Acme:行号」指 server/cca-acme.py，都是它们最后的样子：
//! `git show v3.6.10:server/cca-presence.py`。对照数据（cli/tests/vectors/server_*.json）是那时候对着它们录的，现在是固定的
//! 数据：有意改了行为就直接改数据。往后这一份自己就是规矩，线上的格式以 server/README.md 为准。
//!
//! 各个文件：
//! - `limits`：各种上限（测试可以换小的）、限流的令牌桶、用过的 nonce、同时处理几个连接
//! - `omap`：记着先后的字典（状态文件里各项的先后要和 Python 写的一样）
//! - `wire`：线上来的东西怎么查——游标、数字、各个正则、请求里各项的检查
//! - `sshsig`：验 `ssh-keygen -Y sign` 的签名（SSHSIG，Ed25519）
//! - `x509`：读证书里的几项（有效期、给哪些地址的、公钥）
//! - `tls`：服务端的证书和私钥、网页端那张会换的证书、连证书机构用的客户端设置
//! - `store`：state.json、keys.json、cost-slots.json、team-docs.json 和它们的各个 op
//! - `remote`、`relay`：远程会话的中转——配对、票据、会话的记录，数据端口
//! - `http`、`front`、`handler`：读请求写回应、接连接起线程、一个请求怎么处理
//! - `ws`、`web`：网页端——WebSocket、页面的文件
//! - `cli`：`team-server` 的参数和启动
//! - `acme`：证书助手

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

pub mod acme;
pub mod cli;
pub mod disk;
pub mod front;
pub mod gateway;
pub mod handler;
pub mod http;
pub mod limits;
pub mod omap;
pub mod push;
pub mod relay;
pub mod remote;
pub mod sshsig;
pub mod store;
pub mod tls;
pub mod web;
pub mod wire;
pub mod ws;
pub mod x509;

use crate::cmd::Cmd;
use std::sync::{Arc, Mutex};

// ---- Python 那一份开头的常量：先后、注释照它（Py:67-110）。几个测试要换小的上限在 limits::Limits 里 ----

/// 接口的版本（请求和回的内容里的 v）（Py:67）
pub const API: i64 = 1;
/// 服务端的级别（GET /v1/version 回的 level）：服务器加了这一版 cca 要靠的东西时加一，cca 的 `team::SERVER_NEED` 跟着改（cca 同步配置时
/// 问一次，比它低的 Team 在界面里提示管理员更新服务器）。这个地址 3.7.1 起有；1：3.7.0 起（认 iPhone 上的 cca App：pair_open 的 guest）；
/// 2：3.7.5 起（手机上看账号的状态、收额度的通知：status_put、push_note，relay_list 的 have / status，push_set 的 usage）；
/// 3：3.7.10 起（一台设备同时 101 个可以接入的会话：100 个远程会话加上启动器；票据、数据端口的连接、限流跟着放宽）
pub const LEVEL: i64 = 3;
/// 设备给请求签名用的命名空间（Py:68）
pub const NS_REPORT: &str = "cca-presence";
/// 一个请求的内容最多这么大（Py:69）
pub const MAX_BODY: usize = 64 * 1024;
/// 签名（解开 base64 以后）最多这么大（Py:70）
pub const MAX_SIG: usize = 4096;
/// 请求里的时间和服务器差多少秒以内（Py:71）
pub const TS_WINDOW: i64 = 300;
/// 一次上报 / 查询最多几个账号（Py:72）
pub const MAX_ACCOUNTS: usize = 64;
/// 一台设备上一个账号最多几个会话（Py:73）
pub const MAX_UNITS: usize = 64;
/// 同时处理的连接（Py:74）
pub const MAX_CONN: usize = 64;
/// 同一个 IP 同时几个连接（Py:75）
pub const MAX_CONN_IP: usize = 8;
/// 一个连接从接进来起最多多少秒，到点就断（Py:76）
pub const CONN_DEADLINE: i64 = 15;
/// TLS 握手最多多少秒（Py:77）
pub const HANDSHAKE_TIMEOUT: i64 = 5;
/// 上报的“开了多久、闲置多久”最大多少秒（Py:78）
pub const MAX_AGE: i64 = 10 * 365 * 86400;
/// 交换密文的请求（/v1/keys、/v1/ledger）一次最多这么大（Py:79）
pub const MAX_SYNC_BODY: usize = 1 << 20;
/// 一份 API key 的密文（base64）最长多少个字符（Py:81）
pub const MAX_KEY_BLOB: usize = 8192;
/// 一份折合金额的密文（base64）最长多少个字符（Py:82）
pub const MAX_COST_BLOB: usize = 98304;
/// 交换密文的 op → 一份密文最长多少个字符（Py:87）
pub const SYNC_OPS: [(&str, usize); 2] = [("keys", MAX_KEY_BLOB), ("ledger", MAX_COST_BLOB)];
/// 邀请多久过期：最短、最长（秒）（Py:102）
pub const INVITE_TTL: (i64, i64) = (60, 30 * 86400);
/// 过期的、用掉的邀请记多久（Py:103）
pub const INVITE_KEEP: i64 = 30 * 86400;
/// 配置的序号比它小：各种语言的 JSON 都读得准的整数（Py:104）
pub const SEQ_MAX: i64 = 1 << 53;
/// 一台设备的配置（密文，base64）最长多少个字符（Py:105）
pub const MAX_TEAM_DOC: usize = 131072;
/// 配置的签名（armored 签名的 base64）最长多少个字符（Py:106）
pub const MAX_TEAM_SIG: usize = 1024;
/// team_enroll、team_put 一次最多几台设备（Py:107）
pub const MAX_TEAM_BATCH: usize = 64;
/// 所有设备的配置（密文，base64）加起来最多这么多个字符（Py:108）
pub const TEAM_DOCS_BYTES: usize = 16 << 20;
/// 管理员的 op：Team 的根密钥，或者根密钥签过的管理员名单里的一把密钥签名（Py:109；后两个是 3.6.15 起的）
pub const TEAM_ADMIN_OPS: [&str; 7] =
    ["team_enroll", "team_invite", "team_put", "team_members", "team_revoke", "team_astate_get", "team_astate_put"];
/// 只认根密钥签名的 op：换管理员名单（3.6.15 起）
pub const TEAM_ROOT_OPS: [&str; 1] = ["team_admins"];
/// 只有 Team 的服务器有的 op：管理员的那几个、只认根密钥的那一个，加上设备的三个（Py:110）
pub const TEAM_OPS: [&str; 11] = [
    "team_enroll",
    "team_invite",
    "team_put",
    "team_members",
    "team_revoke",
    "team_astate_get",
    "team_astate_put",
    "team_admins",
    "team_join",
    "team_get",
    "team_leave",
];
/// 管理员名单里最多几把密钥（根密钥不算）
pub const MAX_TEAM_ADMINS: usize = 16;
/// 管理员名单（紧凑的 JSON 文字）最长多少个字符
pub const MAX_ADMIN_LIST: usize = 8192;
/// 几位管理员共用的状态（密文，base64）最长多少个字符
pub const MAX_ASTATE: usize = 524_288;

// ---- 远程会话的中转、网页端（Py:901-941） ----

/// 中转的 op（开了中转才认）（Py:901）；最后三个是后加的（`RELAY_OPS_UNLISTED`）
pub const RELAY_OPS: [&str; 12] = [
    "pair_open",
    "pair_put",
    "pair_get",
    "pair_commit",
    "pair_close",
    "pair_drop",
    "relay_host",
    "relay_attach",
    "relay_list",
    "push_set",
    "status_put",
    "push_note",
];
/// 后加的中转的 op：/v1/health 不列（和以前逐字节一样；设备发了服务器不认的 op 回 404 not_found，照它知道服务器太旧）。
/// push_set：手机登记推送；status_put：宿主放上账号的状态、到点再推的通知；push_note：宿主推一条额度的通知
pub const RELAY_OPS_UNLISTED: [&str; 3] = ["push_set", "status_put", "push_note"];
/// 访客（手机：iOS 上的 cca App、网页端）能发的（Py:904）；push_set（登记推送）是后加的、只有访客能发
pub const GUEST_OPS: [&str; 8] =
    ["pair_put", "pair_get", "pair_commit", "pair_close", "pair_drop", "relay_attach", "relay_list", "push_set"];
/// 配对码多久过期（Py:905）
pub const PAIR_TTL: i64 = 600;
/// pair_get 最多等对方多久，要比 CONN_DEADLINE 短（Py:907）
pub const PAIR_WAIT: i64 = 10;
/// 同时等着的 pair_get，多了的不等、马上回（Py:908）
pub const PAIR_WAITERS: usize = 16;
/// 配对时交换的一份密文（base64）最长多少个字符（Py:909）
pub const PAIR_MSG: usize = 2048;
/// 这么久没用过的配对删掉（Py:912）
pub const EDGE_IDLE: i64 = 180 * 86400;
/// 票据多久过期，只能用一次（Py:913）
pub const TICKET_TTL: i64 = 30;
/// 会话的说明（密文，base64）最长多少个字符（Py:917）
pub const DIR_BLOB: usize = 4096;
/// 数据端口同时几个连接（Py:918 是 256；3.7.10 起一台设备能开 100 个会话：一个会话一条控制连接，接入时两头各一条）
pub const RELAY_CONN: usize = 1024;
/// 数据端口上同一个 IP 同时几个连接（Py:919 是 32；3.7.10 起：同一个地址后面的一台机器开着 100 个会话、另一台全接着也够）
pub const RELAY_CONN_IP: usize = 400;
/// 一头还没转出去的超过这么多，就先不从另一头读（Py:920）
pub const RELAY_QUEUE: usize = 256 * 1024;
/// 这么久一个字节都没有的连接断开（Py:921）
pub const RELAY_IDLE: i64 = 90;
/// 会话的控制连接断了以后，会话留这么久等它重连（Py:922）
pub const RELAY_GRACE: i64 = 60;
/// 连上以后这么久还没说完开场白就断开（Py:923）
pub const PREAMBLE_WAIT: i64 = 5;
/// 接入方等宿主那一头连上来最多这么久（Py:924）
pub const JOIN_WAIT: i64 = 10;
/// 控制连接上一帧最长（Py:925）
pub const CTL_FRAME: usize = 32 * 1024;
/// 控制连接上帧的类型（Py:926）
pub const F_JOIN: u8 = 0x02;
pub const F_PING: u8 = 0x06;
pub const F_PONG: u8 = 0x07;
pub const F_ANNOUNCE: u8 = 0x08;
pub const F_BYE: u8 = 0x09;
/// 宿主交来的一条推送的通知（push.rs）：Python 那一份没有，后加的
pub const F_PUSH: u8 = 0x0a;
/// 推送的通知（密文，base64）最长多少个字符：最长的一条（标题、副标题各 64 个字，正文 240 个字，都是 4 个字节的字）补齐以后是
/// 1792 个字节，base64 是 2392 个字符。推送服务一条最多 4 KB：网关照这个数留着余地
pub const PUSH_BLOB: usize = 3072;
/// 账号的状态（status_put 的密文，base64）最长多少个字符：一个请求最多 MAX_BODY，留出到点再推的那几条
pub const STATUS_BLOB: usize = 40960;
/// 一个配对最多几条到点再推的通知（status_put 的 later：额度恢复的那一刻）
pub const LATER_MAX: usize = 8;
/// 到点再推的通知最多推后多久（秒）：每周的额度最长 7 天才恢复
pub const LATER_SPAN: i64 = 8 * 86400;
/// 账号的状态留多久（秒）：宿主这么久没再放上来（多半关机了、不用了）就不给了
pub const STATUS_TTL: i64 = 14 * 86400;
/// 推送那条线程最多隔多久看一眼到点再推的通知（秒）
pub const LATER_POLL: f64 = 15.0;
/// 网页端同时几条 WebSocket（Py:927）
pub const WS_CONN: usize = 64;
/// 网页端同一个 IP 同时几条 WebSocket（Py:928）
pub const WS_CONN_IP: usize = 8;
/// WebSocket 一帧最长（Py:929）
pub const WS_FRAME: usize = 1 << 20;
/// 一头这么久写不动、一帧这么久收不齐：断开（Py:930）
pub const WS_STALL: i64 = 20;
/// 两头这么久都没有动静：断开（Py:931）
pub const WS_IDLE: i64 = 100;
/// 算 Sec-WebSocket-Accept 用的那一串（Py:932）
pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// 页面的文件：后缀 → Content-Type（Py:937-938）
pub const WEB_TYPES: [(&str, &str); 6] = [
    (".html", "text/html; charset=utf-8"),
    (".js", "text/javascript; charset=utf-8"),
    (".css", "text/css; charset=utf-8"),
    (".png", "image/png"),
    (".svg", "image/svg+xml"),
    (".webmanifest", "application/manifest+json"),
];
/// 网页的一个文件最长（Py:939）
pub const WEB_FILE: usize = 2 << 20;

/// 一个服务共用的东西：两个监听（原来的端口、网页端另开的端口）是同一个服务，状态、限流、用过的 nonce 都是同一份（Py:2037-2047）
pub struct Ctx {
    /// 状态文件那几份
    pub store: Mutex<store::Store>,
    /// 中转记着的（开了 --relay-listen 才有）
    pub remote: Option<Arc<remote::Shared>>,
    /// 数据端口的那条线程（网页端的 WebSocket 要交连接给它）
    pub relay: Option<Arc<relay::Handle>>,
    /// 网页端（给了 --web-url 才有）
    pub web: Option<web::Web>,
    /// 限流、用过的 nonce
    pub guards: Mutex<limits::Guards>,
    /// 启动时验签自检的结果（/v1/health 的 sig_ok）
    pub sig_ok: bool,
}

/// 这三条子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("team-server", cli::cmd_team_server),
    Cmd::all("team-acme", acme::cmd_team_acme),
    Cmd::all("team-push", gateway::cmd_team_push),
];

/// 是不是这里的子命令：是的话 main 直接运行它，不做辅助程序平常开头的那几步（认当前是哪种工具、读账号表里的模型名——
/// 那些都要读 HOME 下 cca 的文件，服务器上跑它的那个用户没有、也不该有）。只是比两次名字
pub fn standalone(name: &str) -> Option<&'static Cmd> {
    CMDS.iter().find(|c| c.name == name)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// 三条子命令都登记了，别的名字不归这里
    #[test]
    fn standalone_knows_only_its_three_commands() {
        assert_eq!(standalone("team-server").unwrap().name, "team-server");
        assert_eq!(standalone("team-acme").unwrap().name, "team-acme");
        assert_eq!(standalone("team-push").unwrap().name, "team-push");
        assert!(standalone("team-join").is_none() && standalone("env").is_none() && standalone("").is_none());
        // 平常的那张表里也查得到（`cca-core 不认识的名字` 的报错、名字不重复的检查都照它）
        assert!(["team-server", "team-acme", "team-push"].iter().all(|name| crate::cmd::find(name).is_some()));
    }

    /// 成组的常量和 Python 的一样：TEAM_OPS 是管理员的那几个加上设备的三个
    #[test]
    fn op_lists_match_python() {
        assert_eq!(&TEAM_OPS[..7], &TEAM_ADMIN_OPS[..]);
        assert_eq!(&TEAM_OPS[7..8], &TEAM_ROOT_OPS[..]);
        assert_eq!(&TEAM_OPS[8..], &["team_join", "team_get", "team_leave"]);
        assert!(GUEST_OPS.iter().all(|op| RELAY_OPS.contains(op)));
        assert!(!GUEST_OPS.contains(&"pair_open") && !GUEST_OPS.contains(&"relay_host"));
        assert_eq!((MAX_AGE, SEQ_MAX, INVITE_TTL), (315_360_000, 9_007_199_254_740_992, (60, 2_592_000)));
        // 客户端那边写死的同一个值（辅助程序传密文、分批时照它们）
        assert_eq!(MAX_COST_BLOB, crate::cmd::cost::COST_BLOB_MAX);
        assert_eq!(MAX_ACCOUNTS, crate::cmd::cost::COST_BATCH);
        assert_eq!((MAX_TEAM_DOC, SEQ_MAX), (crate::cmd::team::doc::DOC_MAX, crate::cmd::team::doc::SEQ_MAX));
    }
}
