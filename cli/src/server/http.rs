//! HTTP/1.0：读请求、写回复、带期限的连接。Python 那边是标准库的 `BaseHTTPRequestHandler`（Py:1799-1815），这里照它的行为自己写：
//! - 请求行最长 65536 个字节（超了回 414），一行请求头最长 65536 个字节、最多 100 个（超了回 431）
//! - 请求行要三段、第三段是 `HTTP/数字.数字`：2.0 以上回 505，只有两段回 400；GET、POST 以外的方法回 501
//! - 出错的回复是 `send_error` 的样子：`Connection: close`、`Content-Type: text/html;charset=utf-8`、短短的正文
//! - 回复的头一行是 `HTTP/1.0 状态码 说明`，带 `Server: cca-presence/1` 和 `Date`；一条连接只处理一个请求
//! - 连接从接进来起最多 15 秒（`CONN_DEADLINE`），握手时一次读写最多等 5 秒（`HANDSHAKE_TIMEOUT`）：`DeadlineStream` 包着
//!   `TcpStream` 记着到期的时间；`Conn` 是明文的或者 TLS 的那一条
//!
//! 现在只有 `Request`（`handler::handle` 的入参）：读请求、写回复、`DeadlineStream`、`Conn` 还没移植。
//!
//! 留给移植的人的一处先后：Python 的 do_POST 是先按 IP 限流（429）、看 Content-Length（411、413），然后才读正文（Py:1903-1913）。
//! `Request.body` 是调 `handle` 之前读进来的，所以读正文之前的那几步要先做掉（或者正文留到 `handle` 里再读），不然被限流的、
//! 报了超长正文的请求会先被读一遍。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// 读进来的一个请求
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    /// 请求行里的地址，原样（带着 `?` 后面的）
    pub target: String,
    /// 各个请求头，照来的先后；名字是原样的（查的时候不分大小写），值去掉了前后的空白
    pub headers: Vec<(String, String)>,
    /// 正文：照 Content-Length 读到的（对方提前断开时比它短）
    pub body: Vec<u8>,
}

impl Request {
    /// `self.headers.get(name)`：名字不分大小写，有几个同名的是头一个
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// `self.headers.get_all(name)`：同名的全部，照先后（X-Forwarded-For 可以有几个）
    pub fn header_all(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_case_insensitive_and_first_wins() {
        let req = Request {
            method: Method::Post,
            target: "/v1/report".into(),
            headers: vec![
                ("Content-Length".into(), "2".into()),
                ("X-Forwarded-For".into(), "198.51.100.7".into()),
                ("x-cca-sig".into(), "AAAA".into()),
                ("x-forwarded-for".into(), "192.0.2.1, 192.0.2.2".into()),
                ("X-Cca-Sig".into(), "BBBB".into()),
            ],
            body: b"{}".to_vec(),
        };
        assert_eq!((req.header("content-length"), req.header("X-Cca-Sig"), req.header("Origin")), (Some("2"), Some("AAAA"), None));
        assert_eq!(req.header_all("X-FORWARDED-FOR"), ["198.51.100.7", "192.0.2.1, 192.0.2.2"]);
        assert!(req.header_all("Upgrade").is_empty() && req.method == Method::Post && req.method != Method::Get);
    }
}
