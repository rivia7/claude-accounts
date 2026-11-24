//! stun 沙箱目录：假的 STUN 服务器（只在 127.0.0.1 上，泄露检测那一节才开）。原来的 fake_stun.py。
//! 回答 Binding 请求，说对方的公网 IP 是 $T/stun_ip 里写的（IPv4），端口写进 $T/stun.port；每次回答记在 $T/stun.log

use super::claude::arg;
use super::*;
use std::net::UdpSocket;

pub fn run(a: &[String]) -> R<i32> {
    let t = arg(a, 0)?;
    let s = UdpSocket::bind("127.0.0.1:0")?;
    let tmp = join(&[t, "stun.port.tmp"]);
    write_text(&tmp, &s.local_addr()?.port().to_string())?;
    replace(&tmp, &join(&[t, "stun.port"]))?;
    let mut buf = [0u8; 2048];
    loop {
        let (n, peer) = s.recv_from(&mut buf)?;
        let d = &buf[..n];
        if d.len() < 20 || d[..2] != [0, 1] {
            continue;
        }
        // 读不到 stun_ip 就不回答（原来的 except OSError: continue）
        let Ok(raw) = std::fs::read(join(&[t, "stun_ip"])) else { continue };
        let ip = String::from_utf8(raw).map_err(|_| Fail("UnicodeDecodeError: 'utf-8' codec can't decode stun_ip".into()))?;
        let ip = py_strip(&ip.replace("\r\n", "\n").replace('\r', "\n")).to_string();
        append_text(&join(&[t, "stun.log"]), &format!("STUN {ip}\n"))?;
        let addr = inet_aton(&ip)?;
        let port = match peer {
            std::net::SocketAddr::V4(v4) => v4.port(),
            std::net::SocketAddr::V6(v6) => v6.port(),
        };
        let mut attr = Vec::with_capacity(12);
        attr.extend_from_slice(&0x0020u16.to_be_bytes());
        attr.extend_from_slice(&8u16.to_be_bytes());
        attr.extend_from_slice(&[0, 1]);
        attr.extend_from_slice(&(port ^ 0x2112).to_be_bytes());
        attr.extend(addr.iter().zip(&d[4..8]).map(|(p, q)| p ^ q));
        let mut out = Vec::with_capacity(20 + attr.len());
        out.extend_from_slice(&0x0101u16.to_be_bytes());
        out.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        out.extend_from_slice(&d[4..20]);
        out.extend_from_slice(&attr);
        s.send_to(&out, peer)?;
    }
}

extern "C" {
    // libc 这个 crate 在 macOS 上没有列出它（C 库里有）
    #[link_name = "inet_aton"]
    fn c_inet_aton(cp: *const libc::c_char, inp: *mut libc::in_addr) -> libc::c_int;
}

/// socket.inet_aton：C 库的 inet_aton（认 1.2.3.4，也认 127.1、0x7f.1 这类写法）
fn inet_aton(ip: &str) -> R<[u8; 4]> {
    let bad = || Fail("OSError: illegal IP address string passed to inet_aton".into());
    let c = std::ffi::CString::new(ip).map_err(|_| Fail("ValueError: embedded null character".into()))?;
    let mut a = libc::in_addr { s_addr: 0 };
    // SAFETY: c 是以 0 结尾的字符串，a 是可写的 in_addr；inet_aton 只读前者、只写后者
    let ok = unsafe { c_inet_aton(c.as_ptr(), &mut a) };
    if ok == 0 {
        return Err(bad());
    }
    Ok(a.s_addr.to_ne_bytes())
}
