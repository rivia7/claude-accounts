//! 证书助手（`cca-core team-acme`）：给远程会话的网页端申请、续上浏览器信得过的证书。Python 的 server/cca-acme.py。
//!
//! 照 ACME（RFC 8555）向 Let's Encrypt 要证书——给 IP 地址签的（RFC 8738，只有约 6 天的短期证书这一种）或者给域名签的——放到一个
//! 目录里（fullchain.pem、privkey.pem），到期之前自己续上；服务端发现文件换了会自己重新读（`tls::CertCtx`）。验证只用 http-01：
//! 证书机构从公网连这台机器的 80 端口取 `/.well-known/acme-challenge/<令牌>`。
//!
//! Python 那一份靠系统的 openssl 命令生成密钥、签名、做证书请求、看证书；Rust 版不起外部命令：密钥和签名用 ring（key.rs），
//! 证书请求自己拼 DER（csr.rs），看证书用 `x509::cert_info`。
//!
//! - `identifier`：命令行、--teams 目录里写的一个地址 → 要签的是哪一种（这里）
//! - key.rs：P-256 的私钥（账户的、证书的），JWK 和它的摘要，两种签名
//! - csr.rs：证书请求
//! - client.rs：和证书机构说话——HTTPS、JWS、nonce、账户、下单、等验证、取证书
//! - respond.rs：http-01 的应答——自己听一个端口，或者把文件写进别的网站服务的目录
//! - 这里还要有：`cert_state`、`usable`、`renew`、`serve_one`、`Slot`、`registry`、`stamp`、`give`、`serve_teams` 和参数（Acme:369-611）
//!
//! 现在移植好的只有 `identifier`：入口说一句“未完成”、退出码 2。

pub mod client;
pub mod csr;
pub mod key;
pub mod respond;

use super::x509::San;
use crate::py::ipaddr::ip_address;
use crate::py::re::Re;
use crate::py::{text, R};
use crate::re;

/// 证书机构的 ACME 目录默认是 Let's Encrypt（Acme:50）
pub const LETSENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// 请求里的 User-Agent（Acme:51）
pub const AGENT: &str = "cca-acme/1";
/// 没成以后隔多少秒再试（最后一个一直用）（Acme:55）
pub const RETRY: [u64; 4] = [60, 600, 3600, 6 * 3600];
/// 等证书机构验证、签发最多多少秒（Acme:56）
pub const POLL: u64 = 90;
/// --teams：等着的时候隔几秒看一眼目录变了没有（Acme:57）
pub const LOOK: u64 = 5;

/// http-01 的令牌（Acme:52）。`\Z`：`$` 会放过结尾的换行（令牌要当文件名用：--webroot）
pub fn token_re() -> &'static Re {
    re!(r"^[A-Za-z0-9_-]{1,256}\Z")
}

/// 域名（Acme:53）。原来开头还有一个 `(?=.{1,253}$)`（整个名字最长 253 个字符）：这里的正则没有这种写法，长度在 `identifier` 里另查
pub fn name_re() -> &'static Re {
    re!(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)+$")
}

/// --teams 里的文件名：Team 的服务名（`cca-team-<编号的前 8 位>`）（Acme:54）
pub fn team_re() -> &'static Re {
    re!(r"^[a-z0-9][a-z0-9-]{0,62}\Z")
}

/// 没办成：原因是给人看的一句话（Python 的 `Problem`，Acme:60-61）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem(pub String);

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 命令行里写的一个地址 → 要签的是什么（ACME 的 identifier）：IP 地址写成规范的样子，别的当域名（小写、去掉结尾的点）（Acme:84-97）。
/// 证书机构只给公网的地址签；any_ip（--any-ip，测试时对着本机的假证书机构要签 127.0.0.1）放开这一条
pub fn identifier(given: &str, any_ip: bool) -> Result<San, Problem> {
    let Ok(ip) = ip_address(text::strip_chars(given, "[]")) else {
        let name = given.to_lowercase();
        let name = text::rstrip_chars(&name, ".");
        // 原来那个 (?=.{1,253}$)：$ 也认结尾那个换行之前，所以量的是去掉它以后的长度
        let len = text::len(name.strip_suffix('\n').unwrap_or(name));
        if !(name_re().matches(name) && (1..=253).contains(&len)) {
            return Err(Problem(format!("{given} 不是 IP 地址，也不是域名")));
        }
        return Ok(San::Dns(name.to_string()));
    };
    let a = &ip.ip;
    if (a.is_private() || a.is_loopback() || a.is_link_local() || a.is_multicast() || a.is_unspecified() || a.is_reserved())
        && !any_ip
    {
        return Err(Problem(format!("{} 不是公网的地址：证书机构只给从公网连得上的地址签证书", ip.text())));
    }
    Ok(San::Ip(ip.text()))
}

/// `cca-core team-acme …`
pub fn cmd_team_acme(args: &[String]) -> R<()> {
    let _ = args;
    crate::err!("cca-core team-acme：未完成（Rust 版的证书助手还在移植，现在用 server/cca-acme.py）");
    crate::py::exit(2)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::json;

    /// 这几个正则都编得出来（写错了的要到头一次用才 panic），宽严和 Python 的一样
    #[test]
    fn regexes_compile_and_match_like_python() {
        assert!(token_re().matches("abc_DEF-123") && token_re().matches(&"a".repeat(256)));
        assert!(
            !token_re().matches("")
                && !token_re().matches(&"a".repeat(257))
                && !token_re().matches("a/b")
                && !token_re().matches("a.b")
        );
        assert!(!token_re().matches("abc\n"), "\\Z 不放过结尾的换行");
        assert!(team_re().matches("cca-team-0a1b2c3d") && team_re().matches("a") && team_re().matches(&"a".repeat(63)));
        assert!(
            !team_re().matches("") && !team_re().matches("-a") && !team_re().matches("A") && !team_re().matches(&"a".repeat(64))
        );
        assert!(!team_re().matches("cca-team-0a1b2c3d\n") && !team_re().matches("cca-team.new.123"));
        assert!(name_re().matches("example.com") && name_re().matches("a.b") && name_re().matches("xn--fiq228c.example.com"));
        assert!(!name_re().matches("localhost") && !name_re().matches("-a.example.com") && !name_re().matches("a..example.com"));
        assert!(!name_re().found("EXAMPLE.com") && name_re().fullmatch("example.com").is_some());
    }

    fn ok(given: &str, any_ip: bool) -> (String, String) {
        let id = identifier(given, any_ip).unwrap_or_else(|e| panic!("{given}: {e}"));
        (id.kind().to_string(), id.value().to_string())
    }

    fn why(given: &str, any_ip: bool) -> String {
        identifier(given, any_ip).err().unwrap_or_else(|| panic!("{given} 不该收")).to_string()
    }

    fn pair(kind: &str, value: &str) -> (String, String) {
        (kind.to_string(), value.to_string())
    }

    /// 对照数据（cli/tests/vectors/server_acme.json）：每一项是 server/cca-acme.py 的 identifier 给的结果（CPython 3.14.0）——各种写法的
    /// IP 地址、域名和两样都不是的，开着、没开 --any-ip 的各一遍。两边要一项不差，连没收时说的那句话
    #[test]
    fn identifier_like_python() {
        let doc = json::loads(include_str!("../../../tests/vectors/server_acme.json")).unwrap().as_dict().unwrap();
        let (mut ips, mut names, mut not_public, mut neither) = (0, 0, 0, 0);
        for c in doc.list("identifier").unwrap().items() {
            let c = c.as_dict().unwrap();
            let (given, any_ip) = (c.str("given").unwrap().to_string(), c.get("any_ip").unwrap().as_bool().unwrap());
            let want = match c.str("err") {
                Some(why) => Err(why.to_string()),
                None => Ok(pair(&c.str("type").unwrap(), &c.str("value").unwrap())),
            };
            let got = identifier(&given, any_ip).map(|id| pair(id.kind(), id.value())).map_err(|p| p.0);
            assert_eq!(got, want, "{given:?}（--any-ip：{any_ip}）");
            match want {
                Ok((kind, _)) if kind == "ip" => ips += 1,
                Ok(_) => names += 1,
                Err(why) if why.ends_with("不是公网的地址：证书机构只给从公网连得上的地址签证书") => {
                    not_public += 1
                }
                Err(_) => neither += 1,
            }
        }
        assert!(ips >= 60 && names >= 30 && not_public >= 25 && neither >= 60, "{ips} {names} {not_public} {neither}");

        // 其中值得写明的几条。公网的 IP 地址：写成规范的样子，方括号去掉
        assert_eq!(ok("[2606:4700:4700::1111]", false), pair("ip", "2606:4700:4700::1111"));
        assert_eq!(ok("2620:FE::FE", false), pair("ip", "2620:fe::fe"));
        // 不是公网的不签，--any-ip 放开；说的是规范的写法
        assert_eq!(why("[fe80::1]", false), "fe80::1 不是公网的地址：证书机构只给从公网连得上的地址签证书");
        assert_eq!(ok("192.0.2.10", true), pair("ip", "192.0.2.10"));
        // 100.64.0.0/10（运营商的 NAT）在 Python 里不算 is_private：放过去了（证书机构自己会拒）
        assert_eq!(ok("100.64.0.1", false), pair("ip", "100.64.0.1"));
        // 域名：小写、去掉结尾的点。不是 IP 地址的写法、但各段合规矩的也当域名（结尾带点的地址就是这样）
        assert_eq!(ok("Team.Example.COM.", false), pair("dns", "team.example.com"));
        assert_eq!(ok("8.8.8.8.", false), pair("dns", "8.8.8.8"));
        // 整个名字最长 253 个字符
        let long = format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61));
        assert_eq!((long.len(), ok(&long, false)), (253, pair("dns", &long)));
        assert_eq!(why(&format!("{long}d"), true), format!("{long}d 不是 IP 地址，也不是域名"), "说的是原样的那个写法");
        // Python 的 $ 放过结尾的一个换行（--teams 的文件内容是先 strip 过的，命令行里一般带不进来）：照样
        assert_eq!(ok("example.com\n", false), pair("dns", "example.com\n"));
        assert_eq!(Problem("原因".into()).to_string(), "原因");
    }
}
