//! P-256 的私钥：证书助手的账户的那一把（account.key）、证书的那一把（privkey.pem）。Python 的 `Key`（Acme:131-179）。
//!
//! - `Key::open(path, make)`：没有就生成（make 是 false 时不生成：没有就是 `Problem("没有 路径")`）。生成用 ring 的
//!   `EcdsaKeyPair::generate_pkcs8`，写成 `BEGIN PRIVATE KEY` 的 PEM（64 个字符一行），权限 600，先写旁边再换过去
//! - 以前 openssl 写的 account.key、privkey.pem 要照样读得了：续证书时账户和证书的私钥都不换
//! - 公钥（SPKI）：固定的前面 26 个字节 `3059301306072a8648ce3d020106082a8648ce3d030107034200` 加 65 个字节的点（04 X Y）
//! - JWK：`{"crv": "P-256", "kty": "EC", "x", "y"}`；摘要（RFC 7638）是它按键名排好、不带空白的 JSON 的 SHA-256
//! - 两种签名：JWS 要的 r、s 各 32 个字节接在一起（`ECDSA_P256_SHA256_FIXED_SIGNING`），证书请求要的 DER（`…_ASN1_SIGNING`）
//!
//! 还没移植。
