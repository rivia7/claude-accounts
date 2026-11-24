//! http-01 的应答：证书机构来取 `/.well-known/acme-challenge/<令牌>` 时回“令牌.账户公钥的摘要”。
//! Python 的 `Responder`、`Challenge`、`Webroot`（Acme:182-235）。
//!
//! - 自己听一个端口（--listen，部署时把公网的 80 端口指到它）：一条连接一个线程，读写最多等 10 秒；路径对、令牌合
//!   `acme::token_re()`、手里有这个令牌才回 200，别的一律 404，不记日志。头一次去要证书之前就听上（--once、没什么要做时也是）
//! - 80 端口上已经有别的网站服务（--webroot 目录）：文件写进 `目录/.well-known/acme-challenge/`（644，先写旁边再换过去），验完就删
//!
//! 还没移植。
