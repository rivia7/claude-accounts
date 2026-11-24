//! 假的代理（原来的 Proxy）：一个端口同时认 SOCKS5（不带认证的）和 HTTP CONNECT，不管要连哪个主机都接到假 Gmail 上，
//! 日志里记下要连的目标。ctl.json 的 proxy：refuse 是不肯连，auth 是 SOCKS5 要认证（每个连接进来时读一次）。

use super::{Ctl, Shared};
use crate::json::Value;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

/// 跑着的假代理。端口在 port；close()（或者丢掉它）以后不再接新的连接，已经接上的照常转发到断开
pub struct Proxy {
    /// 监听的端口（127.0.0.1）
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Proxy {
    /// 在 127.0.0.1 随便一个空闲端口上起（接到这个假 Gmail 上）
    pub fn start(srv: &super::Server) -> io::Result<Proxy> {
        Proxy::start_on(srv, 0)
    }

    /// 在指定的端口上起（0 是随便一个空闲的）
    pub fn start_on(srv: &super::Server, port: u16) -> io::Result<Proxy> {
        let ls = TcpListener::bind(("127.0.0.1", port))?;
        let port = ls.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, flag) = (srv.shared(), stop.clone());
        thread::spawn(move || {
            for s in ls.incoming() {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(s) = s else { return };
                let sh = sh.clone();
                thread::spawn(move || serve(&sh, port, s));
            }
        });
        Ok(Proxy { port, stop })
    }

    /// 不再接新的连接
    pub fn close(&self) {
        if !self.stop.swap(true, Ordering::SeqCst) {
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.close();
    }
}

fn recv_n(s: &mut TcpStream, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

/// 一个方向：a 读到的都写给 b，读完、出错了把两边都关掉
fn pipe(mut a: TcpStream, mut b: TcpStream) {
    let mut buf = vec![0u8; 65536];
    loop {
        match a.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if b.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    for s in [&a, &b] {
        let _ = s.shutdown(Shutdown::Both);
    }
}

/// 不是 OSError 的异常（主机名不是 ASCII）：原来是这个线程直接出错退出，连接随之关掉
struct Died;

fn serve(sh: &Shared, my_port: u16, s: TcpStream) {
    let Ok(ctl) = sh.mb.ctl() else { return }; // ctl.json 不是对象：AttributeError
    let _ = handshake(sh, my_port, s, &ctl);
}

fn handshake(sh: &Shared, my_port: u16, mut s: TcpStream, ctl: &Ctl) -> Result<(), Died> {
    let mode = |m: &str| matches!(ctl.get("proxy"), Some(Value::Str(x)) if x == m);
    let r = (|| -> io::Result<Result<Option<TcpStream>, Died>> {
        let first = recv_n(&mut s, 1)?;
        if first[0] == 5 {
            let n = recv_n(&mut s, 1)?[0] as usize;
            recv_n(&mut s, n)?;
            if mode("auth") {
                s.write_all(b"\x05\xff")?;
                sh.log("PROXY socks5 auth-required")?;
                return Ok(Ok(None));
            }
            s.write_all(b"\x05\x00")?;
            let head = recv_n(&mut s, 4)?;
            let host = match head[3] {
                3 => {
                    let n = recv_n(&mut s, 1)?[0] as usize;
                    let b = recv_n(&mut s, n)?;
                    if !b.is_ascii() {
                        return Ok(Err(Died)); // .decode('ascii')：UnicodeDecodeError
                    }
                    String::from_utf8_lossy(&b).into_owned()
                }
                1 => {
                    let b = recv_n(&mut s, 4)?;
                    Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_string()
                }
                _ => {
                    let b = recv_n(&mut s, 16)?;
                    let mut a = [0u8; 16];
                    a.copy_from_slice(&b);
                    ntop6(a)
                }
            };
            let pb = recv_n(&mut s, 2)?;
            let port = u16::from_be_bytes([pb[0], pb[1]]);
            sh.log(&format!("PROXY socks5 {host}:{port}"))?;
            if mode("refuse") {
                s.write_all(b"\x05\x05\x00\x01\x00\x00\x00\x00\x00\x00")?;
                return Ok(Ok(None));
            }
            let up = TcpStream::connect(("127.0.0.1", sh.port))?;
            let mut reply = b"\x05\x00\x00\x01\x7f\x00\x00\x01".to_vec();
            reply.extend_from_slice(&my_port.to_be_bytes());
            s.write_all(&reply)?;
            Ok(Ok(Some(up)))
        } else {
            let mut buf = first;
            while !buf.ends_with(b"\r\n\r\n") {
                buf.extend(recv_n(&mut s, 1)?);
            }
            let m = re_bytes!(r"(?-u)\ACONNECT (\S+) HTTP/1\.[01]\r\n").captures(&buf);
            let target = m
                .as_ref()
                .map_or_else(|| "?".to_string(), |m| m[1].iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect());
            sh.log(&format!("PROXY http {target}"))?;
            if m.is_none() || mode("refuse") {
                s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")?;
                return Ok(Ok(None));
            }
            let up = TcpStream::connect(("127.0.0.1", sh.port))?;
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
            Ok(Ok(Some(up)))
        }
    })();
    let up = match r {
        Ok(Ok(Some(up))) => up,
        Ok(Err(died)) => return Err(died),
        Ok(Ok(None)) | Err(_) => return Ok(()),
    };
    // 两个方向各一个线程转发；客户端这边断了以后，另一个方向最多再等 5 秒（原来的 t.join(5)）
    let (Ok(s2), Ok(up2)) = (s.try_clone(), up.try_clone()) else { return Ok(()) };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        pipe(up2, s2);
        let _ = tx.send(());
    });
    pipe(s, up);
    let _ = rx.recv_timeout(Duration::from_secs(5));
    Ok(())
}

/// socket.inet_ntop(AF_INET6, …)：macOS 的写法（两组以上的 0 才缩成 ::，::a.b.c.d 和 ::ffff:a.b.c.d 写成点分）
fn ntop6(a: [u8; 16]) -> String {
    let ip = Ipv6Addr::from(a);
    let w = ip.segments();
    if w[..6].iter().all(|x| *x == 0) && w[6] != 0 {
        return format!("::{}.{}.{}.{}", a[12], a[13], a[14], a[15]);
    }
    ip.to_string()
}
