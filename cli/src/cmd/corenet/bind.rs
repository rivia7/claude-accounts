//! 别的程序开着 TUN 时（Clash 客户端的虚拟网卡模式、VPN 之类），内置代理的出站绑定到物理网卡（只在 macOS 上，见 `core_bind`）：
//! `sys_cmd_out`、`virtual_iface`、`route_iface`、`phys_iface`、`core_dns`、`core_bind`、`core_bind_apply`。
//!
//! 探测网卡、路由照旧按名字起系统命令（`route`、`netstat`、`ipconfig`），只查不改；回归把它们换成 PATH 上的假命令。

use crate::py::ipaddr::{self, Ip};
use crate::py::obj::{str_list, Dict, V};
use crate::py::proc::{env_set, environ, getenv, Cmd as Proc, In, Out};
use crate::py::text;
use crate::{dict, re};
use std::cell::RefCell;

/// 虚拟网卡：VPN、ZeroTier、Tailscale、WireGuard、Clash 客户端的 TUN 这类隧道和虚拟局域网。bash 的 VIRTUAL_IFACE_RE 是同一份名单
/// （只经它们连得到的路由器不算在家），要一起改（回归里核对）
pub const VIRTUAL_IFACE_RE: &str = r"^(utun|ipsec|ppp|tun|tap|gif|stf|feth|zt|wg|tailscale|nebula|vti|gre|ipip|ip6|sit|Meta|Mihomo|singbox)";
/// 节点的服务器都写成域名时，拿这个公网地址看公网的流量走哪块网卡（只查路由表，不发包）
pub const CORE_ROUTE_PROBE: &str = "1.1.1.1";

/// `core_bind` 的结果：出站绑定的物理网卡、去节点的流量现在走的虚拟网卡、另配的 DNS（空的是不另配）
#[derive(Clone, Debug, PartialEq)]
pub struct Bind {
    pub iface: String,
    pub via: String,
    pub dns: Vec<String>,
}

thread_local! {
    /// `CORE_BIND`：core_bind 的结果，一个进程只查一次（外面一层是查过没有）
    static CORE_BIND: RefCell<Option<Option<Bind>>> = const { RefCell::new(None) };
}

/// 系统命令（route、netstat、ipconfig）的输出；没有这个命令、失败、超时时是空的。PATH 后面补上 /usr/sbin、/sbin
fn sys_cmd_out(argv: &[&str]) -> String {
    let mut env = environ();
    let path = format!("{}:/usr/sbin:/sbin", getenv("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()));
    env_set(&mut env, "PATH", &path);
    match Proc::new(argv).stdin(In::Null).stdout(Out::Capture).stderr(Out::Null).timeout(5.0).env(env).run() {
        Ok(done) if done.rc == 0 => done.out(),
        _ => String::new(),
    }
}

pub fn virtual_iface(name: &str) -> bool {
    !name.is_empty() && re!(VIRTUAL_IFACE_RE).matches(name)
}

/// `route -n get` 的输出里的网卡（`interface: en0` 那一行）；没有是空的
fn route_iface_in(output: &str) -> String {
    for line in text::splitlines(output) {
        let f = text::split_ws(line);
        if f.len() == 2 && f[0] == "interface:" {
            return f[1].to_string();
        }
    }
    String::new()
}

/// 连这个 IP 走哪块网卡（macOS 的 route get：只查路由表）；查不到是空的。IPv6 要带 -inet6（不带时 route 当它是主机名去解析）
fn route_iface(addr: &str) -> String {
    let mut argv = vec!["route", "-n", "get"];
    if addr.contains(':') {
        argv.push("-inet6");
    }
    argv.push(addr);
    route_iface_in(&sys_cmd_out(&argv))
}

/// `netstat -rn -f inet` 的输出里，默认路由里第一块不是虚拟网卡的；没有是空的
fn phys_iface_in(output: &str) -> String {
    for line in text::splitlines(output) {
        let f = text::split_ws(line);
        if f.len() >= 4 && f[0] == "default" && !virtual_iface(f[3]) {
            return f[3].to_string();
        }
    }
    String::new()
}

/// 物理网卡：默认路由（netstat 里也列着各网卡自己的默认路由）里第一块不是虚拟网卡的；没有是空的
fn phys_iface() -> String {
    phys_iface_in(&sys_cmd_out(&["netstat", "-rn", "-f", "inet"]))
}

/// `ip.is_unspecified`（CPython 3.14：映射的 IPv4 地址照 IPv4 看）
fn is_unspecified(ip: &Ip) -> bool {
    match ip {
        Ip::V4(a) => a.is_unspecified(),
        Ip::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => v4.is_unspecified(),
            None => a.is_unspecified(),
        },
    }
}

/// `core_dns` 里的 `usable`：这个 DNS 经物理网卡到得了吗。via 是「连这个地址走哪块网卡」（route_iface）
fn usable(a: &str, via: &dyn Fn(&str) -> String) -> bool {
    let (a, _, scope) = text::partition(a, "%");
    let Ok(parsed) = ipaddr::ip_address(a) else {
        return false;
    };
    let ip = parsed.ip;
    if ip.is_loopback() || is_unspecified(&ip) {
        return false;
    }
    ip.is_global() || !virtual_iface(&if scope.is_empty() { via(a) } else { scope.to_string() })
}

/// resolv.conf 里的 nameserver（照文本模式读：不是 UTF-8 的字节换掉，`\r\n`、单独的 `\r` 也算换行）
fn resolv_nameservers(raw: &[u8]) -> Vec<String> {
    let content = String::from_utf8_lossy(raw).replace("\r\n", "\n").replace('\r', "\n");
    content
        .split('\n')
        .map(text::split_ws)
        .filter(|w| w.len() > 1 && w[0] == "nameserver")
        .map(|w| w[1].to_string())
        .collect()
}

/// `ipconfig getpacket 网卡` 的输出里 DHCP 给的 DNS（`domain_name_server (ip_mult): {a, b}`），原样的几段
fn dhcp_dns(output: &str) -> Vec<String> {
    match re!(r"domain_name_server \(ip_mult\): \{([^}]*)\}").search(output) {
        Some(m) => m.g(1).split(',').map(|a| text::strip(a).to_string()).collect(),
        None => Vec::new(),
    }
}

/// 绑定物理网卡时节点的域名用哪些 DNS：内核自己照系统配的 DNS 查（查询也走绑定的网卡，不会被 TUN 劫持成假 IP），系统的 DNS
/// 经物理网卡到得了就不另配（空的）。系统的 DNS 是 TUN 自己的地址（不是公网地址、又走虚拟网卡：198.18.0.2 这类；还有本机的
/// 127.0.0.1）时到不了，改用物理网卡从 DHCP 拿到的；DHCP 也没给就不配（服务器写成 IP 的节点照样能用）
fn core_dns(phys: &str) -> Vec<String> {
    let conf = getenv("CCA_RESOLV_CONF").filter(|p| !p.is_empty()).unwrap_or_else(|| "/etc/resolv.conf".into());
    let mine = std::fs::read(&conf).map(|raw| resolv_nameservers(&raw)).unwrap_or_default();
    if mine.iter().any(|a| usable(a, &route_iface)) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for a in dhcp_dns(&sys_cmd_out(&["ipconfig", "getpacket", phys])) {
        if usable(&a, &route_iface) && !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// 内置代理的出站要不要绑定网卡：`Bind { iface: 物理网卡, via: 虚拟网卡, dns: [DNS…] }`，不用时是 None。只在 macOS 上：别的程序
/// 开着 TUN（Clash 客户端的虚拟网卡模式、VPN 之类）把去节点的流量路由到虚拟网卡上时，内核连节点的连接会被它截走，照它自己的规则走
/// （可能再套一层代理，就连不上了）。这时把出站绑定到物理网卡（macOS 照这块网卡自己的路由走，绕开 TUN；mihomo 的 interface-name）。
/// 看节点服务器里第一个写成公网 IPv4 的走哪块网卡（常常没有 IPv6 的路由），没有时看 CORE_ROUTE_PROBE。开关 TUN、换网络都会变：
/// 生成配置、算摘要时现查。config.json 的 core_bind: false 关掉（要经 VPN 才连得到节点的网络）
pub fn core_bind(c: &Dict, defs: &[Dict]) -> Option<Bind> {
    if let Some(done) = CORE_BIND.with(|b| b.borrow().clone()) {
        return done;
    }
    let mut b = None;
    if cfg!(target_os = "macos") && !matches!(c.get("core_bind"), Some(V::Bool(false))) {
        let mut probe = CORE_ROUTE_PROBE.to_string();
        for p in defs {
            let server = p.get("server").unwrap_or(V::None).py_str();
            if ipaddr::ip_address(&server).is_ok_and(|a| a.ip.version() == 4 && a.ip.is_global()) {
                probe = server;
                break;
            }
        }
        let via = route_iface(&probe);
        let phys = if virtual_iface(&via) { phys_iface() } else { String::new() };
        if !phys.is_empty() {
            let dns = core_dns(&phys);
            b = Some(Bind { iface: phys, via, dns });
        }
    }
    CORE_BIND.with(|done| *done.borrow_mut() = Some(b.clone()));
    b
}

/// 开着 TUN 时给内核的配置加上出站绑定（系统的 DNS 到不了时再加 DNS），见 core_bind；没开时配置不变
pub fn core_bind_apply(cfg: Dict, c: &Dict, defs: &[Dict]) -> Dict {
    if let Some(b) = core_bind(c, defs) {
        cfg.set("interface-name", b.iface);
        if !b.dns.is_empty() {
            cfg.set("dns", dict! { "enable" => true, "ipv6" => true, "default-nameserver" => str_list(&b.dns), "nameserver" => str_list(&b.dns) });
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_interfaces() {
        // 右边是 Python 的 virtual_iface
        for (name, want) in [
            ("utun4", true), ("en0", false), ("", false), ("ipsec0", true), ("ppp0", true), ("tun0", true), ("tap1", true), ("gif0", true),
            ("stf0", true), ("feth123", true), ("zt7nnig26", true), ("wg0", true), ("tailscale0", true), ("nebula1", true), ("vti0", true),
            ("gre0", true), ("ipip0", true), ("ip6tnl0", true), ("sit0", true), ("Meta", true), ("Mihomo", true), ("singbox_tun", true),
            ("meta", false), ("mihomo", false), ("bridge0", false), ("lo0", false), ("awdl0", false), ("eth0", false), ("xutun0", false),
            ("UTUN0", false), ("en0utun", false), (" utun0", false), ("utun0\n", true),
        ] {
            assert_eq!(virtual_iface(name), want, "{name:?}");
        }
    }

    #[test]
    fn route_and_netstat_output() {
        let route = "   route to: 1.1.1.1\ndestination: default\n       mask: default\n    gateway: 192.0.2.1\n  interface: en0\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n";
        assert_eq!(route_iface_in(route), "en0");
        assert_eq!(route_iface_in("interface: utun4 extra\ninterface:\n\tinterface:\ten1\r\n"), "en1");
        assert_eq!(route_iface_in("Interface: en0\ninterface:en0\n"), "");
        assert_eq!(route_iface_in(""), "");
        // 断行照 str.splitlines：\x0b、\x0c、\x1c 这些也算
        assert_eq!(route_iface_in("junk\u{b}interface: en7\u{1c}more"), "en7");
        let netstat = "Routing tables\n\nInternet:\nDestination        Gateway            Flags               Netif Expire\n\
                       default            link#22            UCSg                utun4\ndefault            192.0.2.1          UGScIg                en0\n\
                       default            192.0.2.9          UGScIg                en5\n127                127.0.0.1          UCS                   lo0\n";
        assert_eq!(phys_iface_in(netstat), "en0");
        assert_eq!(phys_iface_in("default 198.18.0.1 UGSc utun4\ndefault link#9 UCSIg tailscale0\n"), "");
        assert_eq!(phys_iface_in("default 192.0.2.1 UGSc\nDefault 192.0.2.1 UGSc en0\ndefault a b en9 extra columns\n"), "en9");
        assert_eq!(phys_iface_in(""), "");
    }

    #[test]
    fn resolv_conf_and_dhcp() {
        assert_eq!(resolv_nameservers(b"# comment\nnameserver 198.18.0.2\nnameserver\t192.0.2.53  # x\nsearch example.com\nnameserver\n"), ["198.18.0.2", "192.0.2.53"]);
        assert_eq!(resolv_nameservers(b"nameserver 192.0.2.1\r\nnameserver 192.0.2.2\rnameserver fe80::1%en0"), ["192.0.2.1", "192.0.2.2", "fe80::1%en0"]);
        assert_eq!(resolv_nameservers(b" nameserver 192.0.2.3\nNameserver 192.0.2.4\nnameserver\xff 192.0.2.5\n"), ["192.0.2.3"]);
        assert!(resolv_nameservers(b"").is_empty());
        assert_eq!(dhcp_dns("op = BOOTREPLY\ndomain_name_server (ip_mult): {192.0.2.53, 192.0.2.54}\nend (none):\n"), ["192.0.2.53", "192.0.2.54"]);
        assert_eq!(dhcp_dns("domain_name_server (ip_mult): {}\n"), [""]);
        assert_eq!(dhcp_dns("domain_name_server (ip_mult): { 192.0.2.53 ,\n\t198.18.0.2,, }"), ["192.0.2.53", "198.18.0.2", "", ""]);
        assert!(dhcp_dns("domain_name_server (ip): 192.0.2.53\n").is_empty() && dhcp_dns("").is_empty());
        // 两处都有时用前一处
        assert_eq!(dhcp_dns("domain_name_server (ip_mult): {192.0.2.1} domain_name_server (ip_mult): {192.0.2.2}"), ["192.0.2.1"]);
    }

    #[test]
    fn which_dns_is_reachable() {
        // 右边是原来的 core_dns 实跑出来的（系统没配 DNS、DHCP 只给这一个地址时结果空不空；route_iface 换成：198.18.x、fd00: 走 utun4，别的走 en0）
        let via = |a: &str| if a.starts_with("198.18.") || a.starts_with("fd00:") { "utun4".to_string() } else { "en0".to_string() };
        for (a, want) in [
            ("8.8.8.8", true), ("192.0.2.53", true), ("198.18.0.2", false), ("127.0.0.1", false), ("0.0.0.0", false), ("::1", false), ("::", false),
            ("::ffff:127.0.0.1", false), ("::ffff:0.0.0.0", false), ("2606:4700:4700::1111", true), ("fd00::53", false), ("fe80::1%en0", true),
            ("fe80::1%utun4", false), ("fe80::1", true), ("10.1.2.3", true), ("10.0.0.1%utun4", false), ("8.8.8.8%utun4", true), ("not-an-ip", false),
            ("", false), ("198.18.0.2%en0", true), ("10.1.2.3%", true), ("fe80::1%en0%x", true), ("1.2.3", false),
        ] {
            assert_eq!(usable(a, &via), want, "{a:?}");
        }
    }
}
