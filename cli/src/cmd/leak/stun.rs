//! WebRTC 那一半：用本机 UDP 探测 STUN（`stun_parse`、`stun_probe`、`stun_ips`），检查绕过账号代理的风险；
//! 不在浏览器里执行，不能据此断言浏览器是否泄露。
//!
//! 域名解析、建套接字直接用 C 库的 getaddrinfo / socket / connect / inet_ntop：原来是 Python 的 socket 模块薄薄包了一层，
//! 认哪些地址、先给哪个、地址怎么写成文字都是 C 库说了算（macOS 上按 IPv6 解析一个 IPv4 地址给的是 `::ffff:` 映射的）。

use super::join_until;
use crate::py::proc::env_or_empty;
use crate::py::rand::urandom;
use crate::py::text;
use crate::py::time::time;
use crate::{err, re};
use std::ffi::{CStr, CString};
use std::io::ErrorKind;
use std::net::UdpSocket;
use std::os::fd::FromRawFd;
use std::sync::mpsc;
use std::time::Duration;

/// 和检测页用的一样
pub const STUN_SERVERS: &[(&str, i64)] = &[("stun.cloudflare.com", 3478), ("stun.l.google.com", 19302)];

extern "C" {
    /// C 库的 inet_ntop（libc 这个库没导出它）。Python 的 `socket.inet_ntop` 就是它：IPv6 地址的写法各家 C 库有出入
    /// （`::1.2.3.4` 这类），照这台机器上的
    fn inet_ntop(af: libc::c_int, src: *const libc::c_void, dst: *mut libc::c_char, size: libc::socklen_t) -> *const libc::c_char;
}

/// `socket.inet_ntop(AF_INET 或 AF_INET6, a)`：a 是 4 个或 16 个字节
fn ntop(a: &[u8]) -> String {
    let af = if a.len() == 4 { libc::AF_INET } else { libc::AF_INET6 };
    let mut buf = [0 as libc::c_char; 64]; // 最长的 IPv6 写法是 45 个字符
                                           // SAFETY: a 的长度和 af 对得上（调用方只给 4 或 16 个字节）；buf 够长，写进去的是以 0 结尾的字符串
    unsafe {
        if inet_ntop(af, a.as_ptr().cast(), buf.as_mut_ptr(), buf.len() as libc::socklen_t).is_null() {
            return String::new();
        }
        CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
    }
}

/// STUN Binding 成功响应里的地址：XOR-MAPPED-ADDRESS 优先，旧服务器的 MAPPED-ADDRESS 也认
pub fn stun_parse(data: &[u8], tid: &[u8]) -> String {
    let u16_at = |i: usize| u16::from_be_bytes([data[i], data[i + 1]]) as usize;
    if data.len() < 20 || &data[8..20] != tid || u16_at(0) != 0x0101 || data[4..8] != [0x21, 0x12, 0xA4, 0x42] {
        return String::new();
    }
    // got[0x0020]、got[0x8020]、got[0x0001]：同一种出现几次的以后面的为准
    let (mut xor, mut xor_old, mut mapped) = (None, None, None);
    let end = data.len().min(20 + u16_at(2));
    let mut i = 20;
    while i + 4 <= end {
        let (kind, size) = (u16_at(i), u16_at(i + 2));
        // 属性的头在报文里就读；值按整个数据包切（原来是 data[i + 4:i + 4 + size]，不看报文头里说的长度）
        let v = &data[i + 4..data.len().min(i + 4 + size)];
        if matches!(kind, 0x0001 | 0x0020 | 0x8020) && v.len() >= 8 && matches!(v[1], 1 | 2) {
            let n = if v[1] == 1 { 4 } else { 16 };
            if let Some(a) = v.get(4..4 + n) {
                let mut a = a.to_vec();
                if kind != 0x0001 {
                    // 和魔数（IPv6 还要加上事务号）异或过
                    for (x, y) in a.iter_mut().zip(data[4..8].iter().chain(tid)) {
                        *x ^= y;
                    }
                }
                let ip = Some(ntop(&a));
                match kind {
                    0x0020 => xor = ip,
                    0x8020 => xor_old = ip,
                    _ => mapped = ip,
                }
            }
        }
        i += 4 + size + (4 - size % 4) % 4;
    }
    xor.or(xor_old).or(mapped).unwrap_or_default()
}

/// Python 的 getaddrinfo 先把主机名按 idna 编码，编不了是 UnicodeError（没人接，那个线程就此结束）：ASCII 的名字里
/// 有空的一段（最后一个点后面的不算），或者哪一段有 64 个字符以上。不是 ASCII 的名字 Python 会转成 `xn--` 的写法再查，
/// 这里没有那套转换，原样交给 C 库（CCA_STUN 只是测试用的开关，没有人这么写）
fn idna_problem(host: &str) -> Option<&'static str> {
    if !host.is_ascii() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels[..labels.len() - 1].iter().any(|l| l.is_empty()) {
        return Some("label empty");
    }
    labels.iter().any(|l| l.len() >= 64).then_some("label too long")
}

/// `socket.getaddrinfo(host, port, family, socket.SOCK_DGRAM)[0]`：第一个结果的地址族和地址；解析不了、没有结果是 None
fn first_addr(host: &str, port: i64, family: libc::c_int) -> Option<(libc::c_int, libc::sockaddr_storage, libc::socklen_t)> {
    let (host, port) = (CString::new(host).ok()?, CString::new(port.to_string()).ok()?);
    // SAFETY: addrinfo、sockaddr_storage 都是纯数据，全 0 是合法的值；getaddrinfo 给的链表只在 freeaddrinfo 之前读，
    // 地址拷进自己的 sockaddr_storage（放得下任何一种地址，长度取两者里小的）
    unsafe {
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_family = family;
        hints.ai_socktype = libc::SOCK_DGRAM;
        let mut res: *mut libc::addrinfo = std::ptr::null_mut();
        if libc::getaddrinfo(host.as_ptr(), port.as_ptr(), &hints, &mut res) != 0 || res.is_null() {
            return None;
        }
        let first = &*res;
        let mut addr: libc::sockaddr_storage = std::mem::zeroed();
        let len = (first.ai_addrlen as usize).min(std::mem::size_of::<libc::sockaddr_storage>());
        let found = if first.ai_addr.is_null() {
            None
        } else {
            std::ptr::copy_nonoverlapping(first.ai_addr.cast::<u8>(), (&raw mut addr).cast::<u8>(), len);
            Some((first.ai_family, addr, len as libc::socklen_t))
        };
        libc::freeaddrinfo(res);
        found
    }
}

/// 本机直连发 STUN Binding 请求；错误、超时、没有这种地址都返回空，不能当成确认没有泄露。
/// Err：原来这个线程会结束在没人接的异常上（主机名编不了码、建不了套接字）——结果一样是没有，只是 stderr 上多一段话
fn stun_probe(host: &str, port: Option<i64>, family: libc::c_int, timeout: f64) -> Result<String, String> {
    if let Some(why) = idna_problem(host) {
        return Err(format!("UnicodeEncodeError: 'idna' codec can't encode {host:?}: {why}"));
    }
    let Some(port) = port else {
        return Err("OverflowError: Python int too large to convert to C long".into());
    };
    let Some((af, addr, addr_len)) = first_addr(host, port, family) else {
        return Ok(String::new());
    };
    if af == libc::AF_INET6 {
        // SAFETY: 地址族是 AF_INET6，这块内存里就是一个 sockaddr_in6（sockaddr_storage 的对齐不比它松）
        let ip = unsafe { (*(&raw const addr).cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr };
        if ntop(&ip).to_lowercase().starts_with("::ffff:") {
            return Ok(String::new());
        }
    }
    let tid = urandom(12);
    // 和 Python 的套接字一样不留给子进程（同时在跑的线程会起 curl）：Linux 上建的时候就带上，别的系统紧跟着设
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let kind = libc::SOCK_DGRAM | libc::SOCK_CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let kind = libc::SOCK_DGRAM;
    // SAFETY: 只是建一个套接字；返回的描述符下面马上交给 UdpSocket 管（离开这个函数时关掉，原来的 finally: s.close()）
    let fd = unsafe { libc::socket(af, kind, 0) };
    if fd < 0 {
        return Err(format!("OSError: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: fd 是刚建的、没有别人用的套接字
    let s = unsafe {
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        UdpSocket::from_raw_fd(fd)
    };
    // connect 过的 UDP 套接字：端口不通时马上报错，不用干等
    // SAFETY: addr 里是 getaddrinfo 给的地址，addr_len 是它的长度
    if unsafe { libc::connect(fd, (&raw const addr).cast::<libc::sockaddr>(), addr_len) } != 0 {
        return Ok(String::new());
    }
    let mut request = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xA4, 0x42]; // struct.pack('!HHI', 1, 0, 0x2112A442)
    request.extend_from_slice(&tid);
    if s.send(&request).is_err() {
        return Ok(String::new());
    }
    let end = time() + timeout;
    let mut buf = [0u8; 2048];
    while time() < end {
        let wait = (end - time()).max(0.01);
        if s.set_read_timeout(Some(Duration::from_secs_f64(wait))).is_err() {
            return Ok(String::new());
        }
        match s.recv(&mut buf) {
            Ok(n) => {
                let ip = stun_parse(&buf[..n], &tid);
                if !ip.is_empty() {
                    return Ok(ip);
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Ok(String::new()), // 超时、对方端口不通（ICMP）……
        }
    }
    Ok(String::new())
}

/// 本机 UDP 的出口（非浏览器实测）：同时问几个 STUN 服务器，IPv4、IPv6 都试；空结果表示没测成。CCA_STUN 换服务器（测试用）
pub fn stun_ips(timeout: f64) -> Vec<String> {
    let env = env_or_empty("CCA_STUN");
    // 端口放不进 C 的 long 时，原来是线程里没人接的 OverflowError：留成 None 带进线程
    let servers: Vec<(String, Option<i64>)> = if env.is_empty() {
        STUN_SERVERS.iter().map(|(h, p)| (h.to_string(), Some(*p))).collect()
    } else {
        env.split(',')
            .filter(|x| re!(r"^[^:,]+:\d+$").matches(x))
            .filter_map(|x| x.rsplit_once(':'))
            .map(|(h, p)| (h.to_string(), text::to_int(p)))
            .collect()
    };
    let (tx, rx) = mpsc::channel::<String>();
    for (host, port) in &servers {
        for family in [libc::AF_INET, libc::AF_INET6] {
            let (host, port, tx) = (host.clone(), *port, tx.clone());
            std::thread::spawn(move || match stun_probe(&host, port, family, timeout) {
                Ok(ip) => {
                    let _ = tx.send(ip);
                }
                Err(why) => err!("cca-core: 线程里没处理的错误：{why}"),
            });
        }
    }
    drop(tx);
    let end = time() + timeout + 0.5; // STUN 服务器的域名解析卡住时也不等（getaddrinfo 自己没有超时）
    let mut ips: Vec<String> = join_until(&rx, end).into_iter().filter(|ip| !ip.is_empty()).collect();
    ips.sort();
    ips.dedup();
    ips
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::from_hex;

    // 每条：说明、数据包、事务号（都是十六进制）、Python 的 stun_parse 给的结果（在 macOS 上跑原来的函数得到的）
    const VECTORS: &[(&str, &str, &str, &str)] = &[
        (
            "xor-mapped v4",
            "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "xor-mapped v6",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212340113a9fa2502a2a3a4a52c89abd9d99f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "2001:db8:85a3::8a2e:370:7334",
        ),
        ("mapped v4", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0001000800011234c0000201", "a0a1a2a3a4a5a6a7a8a9aaab", "192.0.2.1"),
        (
            "mapped v6",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab000100140002123420010db8000000000000000000000001",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "2001:db8::1",
        ),
        ("8020 v4", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab8020000800011234ea12d54b", "a0a1a2a3a4a5a6a7a8a9aaab", "203.0.113.9"),
        (
            "8020 v6",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab8020001400021234df92a442a0a1a2a3a4a5a6a7a8a9aaaa",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "fe80::1",
        ),
        (
            "all three: 0020 wins",
            "010100242112a442a0a1a2a3a4a5a6a7a8a9aaab0001000800011234c00002018020000800011234e112a6400020000800011234e112a641",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.3",
        ),
        (
            "8020 beats 0001",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab0001000800011234c00002018020000800011234e112a640",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.2",
        ),
        (
            "duplicate 0020: last wins",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e112a6410020000800011234e112a646",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.4",
        ),
        ("wrong tid", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f", "000000000000000000000000", ""),
        ("error response type", "0111000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("request type", "0001000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("wrong magic", "0101000c2112a443a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("short", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aa", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("header only", "010100002112a442a0a1a2a3a4a5a6a7a8a9aaab", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("empty", "", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        (
            "length field 0 hides attr",
            "010100002112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "",
        ),
        (
            "length field 3: header not inside",
            "010100032112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "",
        ),
        (
            "length field 4: value past end still read",
            "010100042112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "length field too big",
            "010101f42112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "second attr past end ignored",
            "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0001000800011234c00002010020000800011234e112a641",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.1",
        ),
        ("value truncated", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c0", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        (
            "v6 value truncated",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212340113a9fa2502a2a3a4a52c89abd9d9",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "",
        ),
        ("family 3", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800031234e721c00f", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        ("family 0", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800001234e721c00f", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        (
            "family 2 with 4-byte address",
            "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800021234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "",
        ),
        (
            "family 1 with 16-byte address",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000112340113a9fa2502a2a3a4a52c89abd9d99f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "32.1.13.184",
        ),
        (
            "odd-sized attr before, padded",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab8022000561626364650000000020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "odd-sized attr before, not padded",
            "010100152112a442a0a1a2a3a4a5a6a7a8a9aaab8022000561626364650020000800011234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "",
        ),
        (
            "unknown attrs around",
            "0101001c2112a442a0a1a2a3a4a5a6a7a8a9aaab00080004010203040020000800011234e721c00f8028000400000000",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "v4 attr with extra bytes",
            "010100102112a442a0a1a2a3a4a5a6a7a8a9aaab0020000a00010001e721c00f7a7a0000",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        ("size 7", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000700010001e721c000", "a0a1a2a3a4a5a6a7a8a9aaab", ""),
        (
            "zero-size attr then addr",
            "010100102112a442a0a1a2a3a4a5a6a7a8a9aaab002000000001000800011234c0000201",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.1",
        ),
        (
            "trailing bytes after message",
            "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f6a756e6b",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "198.51.100.77",
        ),
        (
            "mapped then bad xor: mapped used",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab0001000800011234c00002010020000800091234e721c00f",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "192.0.2.1",
        ),
        (
            "xor v6 all zero tid",
            "010100182112a44200000000000000000000000000200014000212340113a9fa000000000000000000000002",
            "000000000000000000000000",
            "2001:db8::2",
        ),
        ("tid shorter than 12", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234e721c00f", "a0a1a2a3a4a5a6a7a8a9aa", ""),
        ("0.0.0.0", "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab000100080001123400000000", "a0a1a2a3a4a5a6a7a8a9aaab", "0.0.0.0"),
        (
            "255.255.255.255",
            "0101000c2112a442a0a1a2a3a4a5a6a7a8a9aaab0020000800011234deed5bbd",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "255.255.255.255",
        ),
        (
            "v6 zero run rules",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212340113a442a0a1a2a2a4a5a6a7a8a9aaaa",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "2001:0:0:1::1",
        ),
        (
            "v6 single zero group",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab000100140002123400010000000200000003000000040000",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "1:0:2:0:3:0:4:0",
        ),
        (
            "v6 loopback",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212342112a442a0a1a2a3a4a5a6a7a8a9aaaa",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "::1",
        ),
        (
            "v4-mapped v6",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212342112a442a0a1a2a3a4a5595868a9a8ac",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "::ffff:192.0.2.7",
        ),
    ];
    // 这两条的写法是 macOS 的 inet_ntop 的（别的 C 库不这么写）
    #[cfg(target_os = "macos")]
    const VECTORS_MACOS: &[(&str, &str, &str, &str)] = &[
        (
            "v4-compatible v6 (inet_ntop of this C library)",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab00200014000212342112a442a0a1a2a3a4a5a6a7a9aba9af",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "::1.2.3.4",
        ),
        (
            "::ffff:1",
            "010100182112a442a0a1a2a3a4a5a6a7a8a9aaab0001001400021234000000000000000000000000ffff0001",
            "a0a1a2a3a4a5a6a7a8a9aaab",
            "::255.255.0.1",
        ),
    ];

    fn check(vectors: &[(&str, &str, &str, &str)]) {
        for (what, data, tid, want) in vectors {
            assert_eq!(stun_parse(&from_hex(data).unwrap(), &from_hex(tid).unwrap()), *want, "{what}");
        }
    }

    #[test]
    fn stun_parse_matches_python() {
        assert_eq!(VECTORS.len(), 43);
        check(VECTORS);
        #[cfg(target_os = "macos")]
        check(VECTORS_MACOS);
    }

    #[test]
    fn hostnames_python_refuses_to_encode() {
        assert_eq!(idna_problem("stun.example.com"), None);
        assert_eq!(idna_problem("stun.example.com."), None); // 结尾的点可以
        assert_eq!(idna_problem("a..b"), Some("label empty"));
        assert_eq!(idna_problem(".a"), Some("label empty"));
        assert_eq!(idna_problem(&"x".repeat(63)), None);
        assert_eq!(idna_problem(&format!("a.{}", "x".repeat(64))), Some("label too long"));
        assert!(stun_probe("a..b", Some(9), libc::AF_INET, 0.05).is_err());
        assert!(stun_probe("127.0.0.1", None, libc::AF_INET, 0.05).is_err());
    }

    /// 只在本机回环上的假 STUN 服务器：收到一个请求就把 replies 里的几个回答依次发回去。返回它的端口和收到的请求；
    /// 这个地址绑不上（机器没有 IPv6）是 None
    fn responder(bind: &str, replies: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> Option<(i64, mpsc::Receiver<Vec<u8>>)> {
        let s = UdpSocket::bind(bind).ok()?;
        let port = s.local_addr().unwrap().port() as i64;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                for r in replies(&buf[..n]) {
                    let _ = s.send_to(&r, from);
                }
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Some((port, rx))
    }

    /// 给这个请求的成功响应：XOR-MAPPED-ADDRESS 是 ip（4 个或 16 个字节）
    fn answer(request: &[u8], ip: &[u8]) -> Vec<u8> {
        let mut r = vec![0x01, 0x01, 0x00, 8 + ip.len() as u8];
        r.extend_from_slice(&request[4..20]); // 魔数、事务号
        r.extend([0x00, 0x20, 0x00, 4 + ip.len() as u8, 0x00, if ip.len() == 4 { 1 } else { 2 }, 0x12, 0x34]);
        r.extend(ip.iter().zip(&request[4..20]).map(|(a, k)| a ^ k));
        r
    }

    const SEEN: [u8; 4] = [198, 51, 100, 77];

    #[test]
    fn probe_over_loopback() {
        // 请求的样子：Binding 请求、长度 0、魔数、12 个字节的事务号
        let (port, seen) = responder("127.0.0.1:0", |q| vec![answer(q, &SEEN)]).unwrap();
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET, 1.0).unwrap(), "198.51.100.77");
        let q = seen.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((q.len(), &q[..8]), (20, &from_hex("000100002112a442").unwrap()[..]));
        // 每次的事务号都不一样
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET, 1.0).unwrap(), "198.51.100.77");
        assert_ne!(seen.recv_timeout(Duration::from_secs(2)).unwrap()[8..], q[8..]);
        // 先来几个不算数的（事务号对不上、不是 STUN、出错的响应），接着等，等到对的那个
        let (port, _seen) = responder("127.0.0.1:0", |q| {
            let mut error = answer(q, &[192, 0, 2, 8]);
            error[1] = 0x11;
            vec![answer(&[0u8; 20], &[192, 0, 2, 9]), b"junk".to_vec(), error, answer(q, &SEEN)]
        })
        .unwrap();
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET, 1.0).unwrap(), "198.51.100.77");
    }

    #[test]
    fn probe_over_ipv6_loopback() {
        // 经 IPv6 发请求的那条路。对拍里没有它（CCA_STUN 里写不了 IPv6 地址，写 localhost 又要解析主机名：macOS 上几个线程
        // 同时解析时偶尔卡住），这里直接给 ::1。机器上没有 ::1 就不测
        let v6: std::net::Ipv6Addr = "2001:db8:85a3::8a2e:370:7334".parse().unwrap();
        let Some((port, seen)) = responder("[::1]:0", move |q| vec![answer(q, &v6.octets())]) else {
            return;
        };
        assert_eq!(stun_probe("::1", Some(port), libc::AF_INET6, 1.0).unwrap(), "2001:db8:85a3::8a2e:370:7334");
        let q = seen.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((q.len(), &q[..8]), (20, &from_hex("000100002112a442").unwrap()[..]));
        // IPv6 的服务器看到的也可能是个 IPv4 地址（照回答里写的）
        let (port, _seen) = responder("[::1]:0", |q| vec![answer(q, &SEEN)]).unwrap();
        assert_eq!(stun_probe("::1", Some(port), libc::AF_INET6, 1.0).unwrap(), "198.51.100.77");
        // 没人回答、按 IPv4 解析一个 IPv6 地址：空的
        let (port, _seen) = responder("[::1]:0", |_| Vec::new()).unwrap();
        assert_eq!(stun_probe("::1", Some(port), libc::AF_INET6, 0.2).unwrap(), "");
        assert_eq!(stun_probe("::1", Some(port), libc::AF_INET, 0.2).unwrap(), "");
    }

    #[test]
    fn failures_are_empty_never_an_address() {
        // 没人回答：等到超时，空的（不能当成没有泄露）
        let (port, _seen) = responder("127.0.0.1:0", |_| Vec::new()).unwrap();
        let t0 = time();
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET, 0.2).unwrap(), "");
        assert!((0.15..1.5).contains(&(time() - t0)));
        // 只回一个出错的响应：也是等到超时
        let (port, _seen) = responder("127.0.0.1:0", |q| {
            let mut r = answer(q, &SEEN);
            r[1] = 0x11;
            vec![r]
        })
        .unwrap();
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET, 0.2).unwrap(), "");
        // IPv4 地址按 IPv6 解析：macOS 给的是 ::ffff: 映射的地址（不算真的 IPv6），别的系统直接解析不了，都是空的，也不去问
        assert_eq!(stun_probe("127.0.0.1", Some(port), libc::AF_INET6, 0.2).unwrap(), "");
        // 端口没人听：connect 过的套接字马上知道，不用等到超时
        let t0 = time();
        assert_eq!(stun_probe("127.0.0.1", Some(9), libc::AF_INET, 3.0).unwrap(), "");
        assert!(time() - t0 < 2.5);
        // 解析不出这种地址（IPv6 的地址按 IPv4 解析：不用查 DNS 就知道不行）、端口不像话
        assert_eq!(stun_probe("::1", Some(9), libc::AF_INET, 0.2).unwrap(), "");
        assert_eq!(stun_probe("127.0.0.1", Some(-1), libc::AF_INET, 0.2).unwrap(), "");
    }
}
