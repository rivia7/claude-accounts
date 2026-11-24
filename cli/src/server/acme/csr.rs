//! 证书请求（PKCS#10）。Python 那一份是写一个 openssl 的配置文件、起 `openssl req -new`（Acme:350-355）；这里自己拼 DER：
//! 版本 0，名字是空的（`30 00`：给 IP 签的证书不许有 Common Name，地址只写在 subjectAltName 里），公钥，
//! 一个 extensionRequest（1.2.840.113549.1.9.14）里放 subjectAltName（不是 critical：openssl 出来的就不是）——IP 是 `[7]` 加
//! 4 个或者 16 个字节，域名是 `[2]`；拿证书的私钥签（ecdsa-with-SHA256）。
//!
//! 还没移植。
