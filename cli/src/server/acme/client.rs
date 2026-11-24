//! 和证书机构说话（RFC 8555）：每个请求都是拿账户的私钥签的 JWS。Python 的 `Acme`（Acme:238-366）。
//!
//! - HTTPS：HTTP/1.1、`Connection: close`，每个请求带 `User-Agent: cca-acme/1` 和 `Content-Type: application/jose+json`；
//!   一次读写最多等 30 秒，回的内容最多读 1 MiB；Content-Length、chunked（Let's Encrypt 用的就是）、读到对方关为止三种都要认；
//!   不跟着重定向走，不看代理的环境变量。连不上、握不了手：`Problem("连不上证书机构（哪一类）")`
//! - 回的内容只有 Content-Type 里带 json 时才当 JSON 解析；哪个回复里有 Replay-Nonce 都记下来
//! - `post`（nonce 过期了照新给的再发，最多三次）、`why`（证书机构说的原因：只取类型和 detail，限制长度、去掉控制字符）、
//!   `account`、`wait`（最多等 `POLL` 秒；Retry-After 夹在 1 到 10 秒之间，没给是 2 秒）、`issue`（下单、应答 http-01、
//!   交证书请求、取证书；验证用的令牌最后一定收掉）一句一句照 Python 的来
//! - TLS 用 `tls::client_config`（证书机构的名字可以是 IP 地址：测试里的假证书机构就是）
//!
//! 还没移植。
