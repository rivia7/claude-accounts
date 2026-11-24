//! tests/linux.sh 要的两个一直开着的小服务。原来各是一句 Python（python3 -m http.server、python3 -c 里的几行 socket），
//! 脚本里照旧在后台起它们、把进程号记进 $D/pids，退出时杀掉：
//!
//!   http-serve --dir 目录 --listen 地址:端口
//!       照一个目录给文件的 HTTP 服务（测试里当“外面的网站”：经账号的端口、内置代理、测试节点连到它，取回 ok.txt）。
//!       原来是 cd 目录 && python3 -m http.server 端口 --bind 地址。回的是 HTTP/1.0、回完就关（和 Python 的一样）；只认 GET、
//!       HEAD（别的 501），没有的 404。和 Python 不一样的：目录不列出来（有 index.html 给它，没有 404），Server 那个头不一样——
//!       测试都用不到。用的是 kit::httpd 的 files，规矩见那里
//!   accept-close 地址:端口
//!       在这个地址上听，连上来的马上关掉，一直这样。假路由器的 SSH 端口：cca 拿 nc 试这个端口连不连得上来认是不是在家，
//!       连上以后不说话。原来是 bind（SO_REUSEADDR）、listen、循环 accept 再 close 的几行；Rust 的 TcpListener::bind 在 Unix 上
//!       本来就设 SO_REUSEADDR
//!
//! 两个都是起来以后什么都不说、一直跑到被杀掉。起不来（地址不对、端口被占着）：stderr 上一句，退出码 1。用法不对：退出码 2，
//! 另在 $CCA_TEST_TRAP 里记一行（cca-test 的规矩）。

use crate::fx::{finish, Stop};
use kit::httpd;
use std::net::TcpListener;
use std::path::Path;

pub const CMDS: &[(&str, crate::Run)] = &[("http-serve", http_serve), ("accept-close", accept_close)];

/// --dir、--listen 后面的值（两个都要给，别的不认）
fn serve_args(args: &[String]) -> Result<(&str, &str), String> {
    let (mut dir, mut listen) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--dir" => &mut dir,
            "--listen" => &mut listen,
            other => return Err(format!("不认识 {other}")),
        };
        *slot = Some(it.next().ok_or_else(|| format!("{a} 后面要跟一个值"))?.as_str());
    }
    match (dir, listen) {
        (Some(dir), Some(listen)) => Ok((dir, listen)),
        _ => Err("--dir 目录 和 --listen 地址:端口 都要给".to_string()),
    }
}

/// 开始照 dir 给文件（HTTP/1.0）；丢掉返回的服务就停
fn serve_dir(dir: &Path, listen: &str) -> std::io::Result<httpd::Server> {
    if !dir.is_dir() {
        return Err(std::io::Error::other(format!("{} 不是目录", dir.display())));
    }
    httpd::Server::bind(listen)?.http10().start(httpd::files(dir))
}

fn http_serve(args: &[String]) -> i32 {
    let (dir, listen) = match serve_args(args) {
        Ok(got) => got,
        Err(why) => {
            let usage = format!("用法：cca-test http-serve --dir 目录 --listen 地址:端口（{why}）");
            return finish("http-serve", args, Err(Stop::Unsupported(usage)), "");
        }
    };
    match serve_dir(Path::new(dir), listen) {
        Ok(_server) => loop {
            std::thread::park(); // 一直开着，等着被杀掉（_server 活着服务就在）
        },
        Err(e) => {
            eprintln!("cca-test http-serve：在 {listen} 上起不来：{e}");
            1
        }
    }
}

/// 连上来的马上关掉，一直这样（监听的那个坏了才返回）
fn close_all(listener: &TcpListener) {
    for conn in listener.incoming() {
        drop(conn);
    }
}

fn accept_close(args: &[String]) -> i32 {
    let [addr] = args else {
        let usage = "用法：cca-test accept-close 地址:端口".to_string();
        return finish("accept-close", args, Err(Stop::Unsupported(usage)), "");
    };
    match TcpListener::bind(addr.as_str()) {
        Ok(listener) => {
            close_all(&listener);
            0
        }
        Err(e) => {
            eprintln!("cca-test accept-close：在 {addr} 上听不了：{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::fsx::{self, TempDir};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// 发一个请求，读到对方关上为止
    fn ask(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serve_args_need_both() {
        let a = strs(&["--listen", "127.0.0.1:18081", "--dir", "/w"]);
        assert_eq!(serve_args(&a), Ok(("/w", "127.0.0.1:18081")));
        for bad in [&[][..], &["--dir", "/w"], &["--dir"], &["--dir", "/w", "--listen"], &["/w", "127.0.0.1:1"], &["--port", "1"]] {
            assert!(serve_args(&strs(bad)).is_err(), "{bad:?}");
        }
    }

    // linux.sh 要的：GET 一个文件拿到它的内容（curl 经代理取 ok.txt），回完就关；没有的文件 404；POST 是 501
    // （python3 -m http.server 也是这三样：200 带内容、404、501）
    #[test]
    fn serves_files_like_http_server() {
        let dir = TempDir::new("cca-linux-www-");
        fsx::write(dir.join("ok.txt"), "cca-linux-ok\n");
        let srv = serve_dir(dir.path(), "127.0.0.1:0").unwrap();
        let got = ask(srv.port(), "GET /ok.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(got.starts_with("HTTP/1.0 200 OK\r\n"), "{got}");
        assert!(got.ends_with("\r\n\r\ncca-linux-ok\n"), "{got}");
        assert!(got.contains("\r\nContent-Length: 13\r\n"), "{got}");
        assert!(ask(srv.port(), "GET /v1/models HTTP/1.1\r\nHost: x\r\n\r\n").starts_with("HTTP/1.0 404 "));
        assert!(ask(srv.port(), "GET /../ok.txt HTTP/1.0\r\n\r\n").ends_with("cca-linux-ok\n"));
        assert!(
            ask(srv.port(), "POST /v1/messages HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}").starts_with("HTTP/1.0 501 ")
        );
    }

    #[test]
    fn serving_needs_a_directory_and_a_free_port() {
        let dir = TempDir::new("cca-linux-www-");
        assert!(serve_dir(&dir.join("missing"), "127.0.0.1:0").is_err());
        let srv = serve_dir(dir.path(), "127.0.0.1:0").unwrap();
        assert!(serve_dir(dir.path(), &format!("127.0.0.1:{}", srv.port())).is_err());
        assert!(serve_dir(dir.path(), "not-an-address").is_err());
    }

    #[test]
    fn accepted_connections_are_closed_at_once() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || close_all(&listener));
        for _ in 0..3 {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            // 对方什么都不说就关了：读到头（0 个字节）
            assert_eq!(s.read(&mut [0u8; 16]).unwrap(), 0);
        }
    }
}
