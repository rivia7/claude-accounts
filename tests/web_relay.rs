//! 网页端（手机浏览器接入远程会话：docs/dev/remote.md「网页端」、server/README.md「网页端」）在服务器这一头：
//! 不在 Team 的名单里的访客拿配对码配对、它能发和不能发的请求、WebSocket 进中转、页面的文件、另开的那个带证书机构签的证书的端口。
//!
//! 真的服务器进程（Team「测试组」的同步服务器；用哪一份照 kit::server）；扮演手机的是这里照文档写的一份（Guest、WsSock：
//! ssh-keygen 签名、自己说 WebSocket），另一头是 Team 里的设备（照文档自己说话，或者真的 cca：见 pair_list_attach_type 那几条）。
//! 浏览器里跑的那一份（server/web/core.js）经 tests/web_client.mjs 在 Node 里测（要 Node 22 以上），真的那一页经
//! tests/web_page.mjs 在无界面的 Chrome 里走一遍（还要 Chrome：`CCA_CHROME` 换路径，`CCA_WEB_SHOTS=目录` 留下截图）；
//! 没有 Node、Chrome 时那几条自己跳过。不联网。夹具（Base、转发器、服务器）在 kit::relay 里。
//!
//! 和原来的 tests/web_relay.py 不一样的地方：
//! - --web-url 的各种写法怎么规整不在这里：在服务器自己的单元测试里（cli/src/server/web.rs 的
//!   the_web_url_is_written_the_way_browsers_say_it）；发哪些文件、压缩以后多大、--web-name-file 是问一台跑着的服务器
//!   （the_real_page、the_team_name），两份服务器都能对着测；名字不对时照旧叫 cca 的那几种（空的、太长、带控制字符）
//!   从外面一种要起一台服务器，也在服务器自己的单元测试里（cli/src/server/web.rs 的 the_team_name）
//! - signature 里页面签的名交给 ssh-keygen 来验（改过的内容、别的命名空间它不认）；服务器自己的验签认不认，
//!   pair_list_attach_type 那几条里真的服务器每个请求都在验
//! - WsSock 等到时限（20 秒）还读不到东西算出错（原来是当成对面关了）：该断开的地方服务器得真的断开

use kit::fleet::{self, Device, WAIT};
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module};
use kit::http::Request;
use kit::json::{self, Style, Value};
use kit::proc::{self, Child, Cmd, Session, Stdio};
use kit::py::truthy;
use kit::relay::{self, ctl, dial, read_ctl, read_exact, read_line, recv, sendall};
use kit::relay::{cols, has, matches, named_like, names, only, parsed, rows_of, secs, walk};
use kit::relay::{Base, Popen, RelayOpts, RelayServer, F_ANNOUNCE, F_JOIN};
use kit::spec::{self, Channel};
use kit::{case, crypto, list, obj, pki, tls};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MODULE: Module = Module {
    name: "web_relay",
    src: include_str!("web_relay.rs"),
    default: true,
    tests: &[
        case!(page_files),
        case!(guest_pairing_and_what_a_guest_may_do),
        case!(the_sponsor_is_revoked),
        case!(attach_over_websocket),
        case!(many_websockets),
        case!(the_web_side_is_switched_off),
        case!(the_certificate_arrives_later),
        case!(off),
        case!(the_same_numbers),
        case!(handshake_and_records),
        case!(signature),
        case!(pair_list_attach_type),
        case!(a_pairing_that_fails_is_closed_on_the_server),
        case!(view_only_unpair_and_no_session),
        case!(terminal),
        case!(in_the_app),
        case!(no_web),
        case!(the_real_page),
        case!(the_team_name),
        // 浏览器那一头自己最多走四分钟（原来 communicate(timeout=240)），再加上起服务器、起会话、收场
        case!(the_page_in_a_browser).timeout(400),
    ],
};

// ---- 几份用例共用的小东西 ----

/// `json.dumps(v)`
fn dumps(v: &Value) -> String {
    json::dumps(v, &Style::default())
}

/// `base64.b64decode(v)`
fn unb64(v: &Value) -> Vec<u8> {
    crypto::b64_decode(v.str()).unwrap_or_else(|e| panic!("不是 base64（{e}）：{v}"))
}

/// `bytes.fromhex(v)`
fn unhex(v: &Value) -> Vec<u8> {
    crypto::unhex(v.str()).unwrap_or_else(|e| panic!("不是十六进制（{e}）：{v}"))
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

/// CRC-32（gzip 末尾记的那一个）
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// `gzip.decompress(data)`：头（照 RFC 1952，带不带文件名、备注都行）、压缩的内容、末尾的校验和与长度都要对，不对就 panic
fn gunzip(data: &[u8]) -> Vec<u8> {
    assert!(data.len() >= 18 && data[..3] == [0x1f, 0x8b, 8], "不是 gzip：{:?}", &data[..data.len().min(16)]);
    let flags = data[3];
    let mut at = 10;
    if flags & 4 != 0 {
        at += 2 + usize::from(u16::from_le_bytes([data[at], data[at + 1]]));
    }
    for bit in [8, 16] {
        // 文件名、备注：到 0 字节为止
        if flags & bit != 0 {
            at += data[at..].iter().position(|b| *b == 0).expect("gzip 的头里少了结尾的 0") + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let (body, tail) = data[at..].split_at(data.len() - at - 8);
    let out = miniz_oxide::inflate::decompress_to_vec(body).unwrap_or_else(|e| panic!("gzip 里的内容解不开：{e}"));
    let (crc, size) = (u32::from_le_bytes(tail[..4].try_into().unwrap()), u32::from_le_bytes(tail[4..].try_into().unwrap()));
    assert_eq!((crc32(&out), out.len() as u32), (crc, size), "gzip 末尾的校验和、长度对不上");
    out
}

// ---- 请求 ----

/// 一次 GET 回来的三样（原来 get 返回的元组）：状态码、头（名字小写）、内容
struct Got {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Got {
    /// 头里的一项；没有就 panic（`h['etag']`）
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).unwrap_or_else(|| panic!("回应里没有 {name} 这个头：{:?}", self.headers))
    }

    /// 内容读成 JSON
    fn json(&self) -> Value {
        json::parse_bytes(&self.body).unwrap_or_else(|e| panic!("回的不是 JSON（{e}）：{:?}", String::from_utf8_lossy(&self.body)))
    }
}

/// GET url（https 的照 client 验证书；http 的不看它），带上 headers；连不上、握不成手、读不完是 Err
fn fetch(url: &str, client: tls::Client, headers: &[(&str, &str)], limit: Duration) -> io::Result<Got> {
    let mut request = Request::get(url).tls(client).timeout(limit);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let reply = request.send()?;
    let headers = reply.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    Ok(Got { status: reply.status, headers, body: reply.body })
}

/// 照测试的证书机构验证书（名字、链都要对；原来的 ca_ctx）
fn ca_client(ca: &Path) -> tls::Client {
    tls::Client::with_ca_file(ca).unwrap_or_else(|e| panic!("{e}"))
}

/// 照文档签一个请求发到 127.0.0.1:port（不验证书），只发一次、头里只有签名：(状态码, 回的 JSON)。
/// 关掉了网页端、没开网页端的服务器那两处用（那里没有网页端的端口，访客只能发到给设备的那个端口上）
fn signed_once(port: u16, keyfile: &Path, fp: &str, op: &str, extra: &Value) -> (u16, Value) {
    let raw = spec::post_body(extra, op, fp, fleet::now(), &crypto::token_hex(16));
    let sig = pki::sign(keyfile, "cca-presence", &raw);
    let request = Request::post(&format!("https://127.0.0.1:{port}/v1/{op}"))
        .header("X-Cca-Sig", &crypto::b64(&sig))
        .body(raw)
        .tls(tls::Client::insecure())
        .timeout(secs(10));
    let reply = request.send().unwrap_or_else(|e| panic!("{e}"));
    let j = reply.json().unwrap_or_else(|e| panic!("{op}：回的不是 JSON（{e}）：{}", reply.text()));
    (reply.status, j)
}

// ---- 照 RFC 6455 自己说的 WebSocket ----

/// 握手时给服务器算 Sec-WebSocket-Accept 用的那一串（RFC 6455）
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// 拼一帧：op 是帧的类型（2 二进制、1 文本、0 接着上一帧、9 ping），mask：加不加掩码（客户端发的都该加），fin：是不是最后一截
fn ws_frame(op: u8, data: &[u8], mask: bool, fin: bool) -> Vec<u8> {
    let n = data.len();
    let mut out = vec![if fin { 0x80 } else { 0 } | op];
    let m = if mask { 0x80 } else { 0 };
    if n < 126 {
        out.push(m | n as u8);
    } else if n < 65536 {
        out.push(m | 126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(m | 127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    if !mask {
        out.extend_from_slice(data);
        return out;
    }
    let k = crypto::random(4);
    out.extend_from_slice(&k);
    out.extend(data.iter().enumerate().map(|(i, b)| b ^ k[i % 4]));
    out
}

/// 照 RFC 6455 自己说的 WebSocket 客户端，用起来像一条连接（kit::relay 的 sendall / recv / read_line / read_exact 都认它）：
/// 发的装成加了掩码的二进制帧，收的把帧里的内容接成字节流。closed：服务器发了关闭帧、或者连接断了
struct WsSock {
    /// 底下的 TLS 连接（要自己拼帧发的时候直接写它）
    s: tls::Stream,
    /// 握手的回应的状态码（101 是开成了）；回应不成样子是 0
    status: u16,
    /// 握手的回应的头，原样
    head: String,
    /// 回应里的 Sec-WebSocket-Accept 对不对
    accept_ok: bool,
    buf: Vec<u8>,
    closed: bool,
    /// 收到的 pong 帧的内容
    pongs: Vec<Vec<u8>>,
}

impl WsSock {
    /// 连 127.0.0.1:port（照证书机构 ca 验证书）、发握手的请求、读回应的头。origin：带不带 Origin；headers：换掉、添上的头
    /// （值是 None 的那一项不发）
    fn open(port: u16, ca: &Path, origin: Option<&str>, path: &str, headers: &[(&str, Option<&str>)]) -> WsSock {
        let mut s = ca_client(ca).connect("127.0.0.1", port, secs(20)).unwrap_or_else(|e| panic!("连不上 127.0.0.1:{port}：{e}"));
        let key = crypto::b64(&crypto::random(16));
        let mut h: Vec<(String, Option<String>)> = vec![
            ("Host".to_string(), Some(format!("127.0.0.1:{port}"))),
            ("Upgrade".to_string(), Some("websocket".to_string())),
            ("Connection".to_string(), Some("Upgrade".to_string())),
            ("Sec-WebSocket-Key".to_string(), Some(key.clone())),
            ("Sec-WebSocket-Version".to_string(), Some("13".to_string())),
        ];
        if let Some(origin) = origin {
            h.push(("Origin".to_string(), Some(origin.to_string())));
        }
        for (name, value) in headers {
            let value = value.map(str::to_string);
            match h.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value,
                None => h.push((name.to_string(), value)),
            }
        }
        let mut request = format!("GET {path} HTTP/1.1\r\n");
        for (name, value) in &h {
            if let Some(value) = value {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        request.push_str("\r\n");
        sendall(&mut s, request.as_bytes());
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let c = recv(&mut s, 1);
            if c.is_empty() {
                break;
            }
            head.extend_from_slice(&c);
        }
        let head: String = head.iter().map(|b| *b as char).collect();
        let status = head
            .split(' ')
            .nth(1)
            .filter(|_| head.matches(' ').count() >= 2)
            .map_or(0, |code| code.parse().unwrap_or_else(|_| panic!("握手的回应里状态码不是数：{head:?}")));
        let accept = crypto::b64(&crypto::sha1(format!("{key}{WS_GUID}").as_bytes()));
        let accept_ok = head.contains(&format!("Sec-WebSocket-Accept: {accept}"));
        WsSock { s, status, head, accept_ok, buf: Vec::new(), closed: false, pongs: Vec::new() }
    }

    /// 原样写到底下的连接上（自己拼的帧）
    fn raw(&mut self, bytes: &[u8]) {
        sendall(&mut self.s, bytes);
    }

    /// 读一帧：(类型, 内容)
    fn next_frame(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        self.s.read_exact(&mut head)?;
        assert!(head[1] & 0x80 == 0, "服务器发的帧不该加掩码");
        let size = match head[1] & 0x7f {
            126 => {
                let mut n = [0u8; 2];
                self.s.read_exact(&mut n)?;
                usize::from(u16::from_be_bytes(n))
            }
            127 => {
                let mut n = [0u8; 8];
                self.s.read_exact(&mut n)?;
                u64::from_be_bytes(n) as usize
            }
            n => usize::from(n),
        };
        let mut data = vec![0u8; size];
        self.s.read_exact(&mut data)?;
        Ok((head[0] & 0x0f, data))
    }
}

impl Read for WsSock {
    /// 和连接的一样：有多少给多少（最多 out 那么长），那头关了是 0
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.buf.is_empty() && !self.closed {
            match self.next_frame() {
                Ok((8, _)) => self.closed = true,
                Ok((10, data)) => self.pongs.push(data),
                Ok((2, data)) => self.buf.extend_from_slice(&data),
                Ok(_) => {}
                // 等到时限还没有：出错（见这个文件开头）；别的（读到头、连接被断开）：那头关了
                Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => return Err(e),
                Err(_) => self.closed = true,
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

impl Write for WsSock {
    /// 一次写就是一帧二进制帧（加了掩码）
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.s.write_all(&ws_frame(2, data, true, true))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.s.flush()
    }
}

// ---- 网页端在哪、扮演手机的那一份 ----

/// 网页端在哪、怎么认它（原来 Guest 经 self.t 拿的 wport、ca、origin）
struct Web {
    /// 网页端另开的那个端口（重启以后会变）
    wport: u16,
    /// 测试的证书机构：照它验那个端口的证书
    ca: PathBuf,
    /// 浏览器开 WebSocket 时说的 Origin（服务器配置里页面的地址）；None：不带（Node 里的 WebSocket 不带）
    origin: Option<String>,
    /// 用例里开了、看完就不管的 WebSocket：留到用例完了才关（原来它们都记在 SOCKS 里、收尾时才关）
    held: RefCell<Vec<WsSock>>,
}

impl Web {
    /// GET 网页端的端口上的 path（照证书机构验证书）：状态码、头、内容
    fn get(&self, path: &str, headers: &[(&str, &str)]) -> Got {
        let url = format!("https://127.0.0.1:{}{path}", self.wport);
        fetch(&url, ca_client(&self.ca), headers, secs(20)).unwrap_or_else(|e| panic!("{e}"))
    }

    /// 开一条 WebSocket（握手做完；成没成看 status）
    fn ws(&self, origin: Option<&str>, path: &str, headers: &[(&str, Option<&str>)]) -> WsSock {
        WsSock::open(self.wport, &self.ca, origin, path, headers)
    }

    /// 握手的回应的状态码；连接留着不关
    fn status(&self, w: WsSock) -> u16 {
        let status = w.status;
        self.hold(w);
        status
    }

    /// 这条连接留到用例完了才关
    fn hold(&self, w: WsSock) {
        self.held.borrow_mut().push(w);
    }
}

/// 扮演手机上的那一页：一把自己生成的密钥（不在 Team 的名单里），请求照文档签名，发到网页端的那个端口（照证书机构验证书）
struct Guest {
    name: String,
    keyfile: PathBuf,
    /// 公钥那一行（ssh-ed25519 AAAA…）
    pubkey: String,
    fp: String,
    /// 配好对以后才有的：对面的指纹、那次配对的编号、两边之间的密钥和它的编号
    peer_fp: String,
    rid: String,
    p: [u8; 32],
    pid: String,
}

impl Guest {
    /// 在 d 里生成它的密钥（guest-随机的几个字符）
    fn new(d: &Path, name: &str) -> Guest {
        Guest::with_key(d.join(format!("guest-{}", crypto::token_hex(4))), name)
    }

    /// 密钥文件放在给的地方
    fn with_key(keyfile: PathBuf, name: &str) -> Guest {
        let pubkey = pki::keygen_as(&keyfile, "web");
        let fp = spec::fp_of(format!("{}.pub", text(&keyfile)));
        Guest {
            name: name.to_string(),
            keyfile,
            pubkey,
            fp,
            peer_fp: String::new(),
            rid: String::new(),
            p: [0; 32],
            pid: String::new(),
        }
    }

    /// 发一个请求（自己的密钥签名）：(状态码, 回的 JSON)。被限流（429 rate）时等 2 秒再发，一共 6 次
    fn post(&self, web: &Web, op: &str, extra: &Value) -> (u16, Value) {
        self.post_as(web, op, extra, &self.keyfile)
    }

    /// 同 [`Guest::post`]，签名用的是 key 这把密钥（body 里写的指纹还是自己的）
    fn post_as(&self, web: &Web, op: &str, extra: &Value, key: &Path) -> (u16, Value) {
        let raw = spec::post_body(extra, op, &self.fp, fleet::now(), &crypto::token_hex(16));
        let sig = pki::sign(key, "cca-presence", &raw);
        let request = Request::post(&format!("https://127.0.0.1:{}/v1/{op}", web.wport))
            .header("Content-Type", "application/json")
            .header("X-Cca-Sig", &crypto::b64(&sig))
            .body(raw)
            .tls(ca_client(&web.ca))
            .timeout(secs(20));
        let mut tries = 0;
        loop {
            let reply = request.send().unwrap_or_else(|e| panic!("{e}"));
            let body: &[u8] = if reply.body.is_empty() { b"{}" } else { &reply.body };
            let j = json::parse_bytes(body).unwrap_or_else(|e| panic!("{op}：回的不是 JSON（{e}）：{}", reply.text()));
            tries += 1;
            if reply.status != 429 || j.get("error") != Some(&Value::from("rate")) || tries == 6 {
                return (reply.status, j);
            }
            // 服务器给每把密钥限了流（两秒一个请求）：等够了再发
            std::thread::sleep(secs(2));
        }
    }

    /// 该成的请求：不是 200 就 panic，返回回的 JSON
    #[track_caller]
    fn good(&self, web: &Web, op: &str, extra: &Value) -> Value {
        let (st, j) = self.post(web, op, extra);
        assert_eq!(st, 200, "{op}：{j}");
        j
    }

    /// 拿配对码配对（对面先开了给网页端的配对、放了它那一份）。返回 (密钥, 配对编号)，记在自己身上
    fn join(&mut self, web: &Web, code: &[u8], owner_fp: Option<&str>) -> ([u8; 32], String) {
        let (kc, keys, rid) = spec::pair_keys(code);
        let nb = crypto::random(32);
        let m = spec::pair_msg(&keys, &rid, "b", &self.fp, &nb, &self.name);
        self.good(web, "pair_put", &obj! { "rid" => &rid, "pub" => &self.pubkey, "m" => m });
        let j = self.good(web, "pair_get", &obj! { "rid" => &rid, "wait" => 10 });
        let theirs = spec::pair_open(&keys, &rid, "a", &j["m"]).expect("对面放的那一份认证得过");
        assert_eq!(theirs["fp"], j["fp"]);
        if let Some(owner_fp) = owner_fp {
            assert_eq!(j["fp"].str(), owner_fp);
        }
        self.peer_fp = theirs["fp"].str().to_string();
        self.rid = rid;
        (self.p, self.pid) = spec::pair_secret(&kc, &unhex(&theirs["n"]), &nb, &self.peer_fp, &self.fp);
        self.good(web, "pair_commit", &obj! { "rid" => &self.rid, "pid" => &self.pid });
        (self.p, self.pid.clone())
    }

    /// 开一条 WebSocket；tid：网址里带的票据的编号（没有票据的进不来）
    fn ws(&self, web: &Web, tid: Option<&str>, headers: &[(&str, Option<&str>)]) -> WsSock {
        let path = format!("/v1/relay/ws{}", tid.map(|tid| format!("?t={tid}")).unwrap_or_default());
        web.ws(web.origin.as_deref(), &path, headers)
    }

    /// 换一张接入会话 sid 的票据
    fn ticket(&self, web: &Web, sid: &str) -> Value {
        self.good(web, "relay_attach", &obj! { "sid" => sid, "pid" => &self.pid })
    }

    /// 换一张票据、拿它开一条 WebSocket（还没说开场白）。返回 (连接, 票据)
    fn open_ws(&self, web: &Web, sid: &str) -> (WsSock, Value) {
        let t = self.ticket(web, sid);
        let s = self.ws(web, Some(t["tid"].str()), &[]);
        assert_eq!((s.status, s.accept_ok), (101, true), "{}", s.head);
        (s, t)
    }

    /// 换票据、经 WebSocket 进中转、说开场白。返回 (连接, 服务器该回的那一行)
    fn attach(&self, web: &Web, sid: &str) -> (WsSock, Vec<u8>) {
        let (mut s, t) = self.open_ws(web, sid);
        let (line, ok) = spec::relay_line("view", t["tid"].str(), &unb64(&t["tkey"]));
        sendall(&mut s, &line);
        (s, ok)
    }
}

// ---- 开着网页端的服务器（原来的 WebBase） ----

/// 临时目录里那一页的首页、脚本（和真的那一页无关）
const PAGE: &[u8] = b"<!doctype html><title>cca</title><script type=\"module\" src=\"app.js\"></script>\n";

fn script() -> Vec<u8> {
    [&b"// app\n"[..], &b"console.log(\"x\");\n".repeat(400)].concat()
}

/// 服务器配置里页面的地址（不带末尾的斜线：浏览器说的 Origin 就是这个样子）
const ORIGIN: &str = "https://relay.example.com:8443";

/// 开网页端的那几个参数：页面的地址、另开的端口（随便挑，写在 d/wport 里）、证书（d/web-tls）、页面的文件（d/web-root）
fn web_args(d: &Path) -> Vec<String> {
    let (tls, root) = (d.join("web-tls"), d.join("web-root"));
    let args = [
        ("--web-url", format!("{ORIGIN}/")),
        ("--web-listen", "127.0.0.1:0".to_string()),
        ("--web-port-file", text(&d.join("wport"))),
        ("--web-cert", text(&tls.join("cert.pem"))),
        ("--web-key", text(&tls.join("key.pem"))),
        ("--web-root", text(&root)),
    ];
    args.into_iter().flat_map(|(name, value)| [name.to_string(), value]).collect()
}

/// 原来 WebBase 的 server_extra：页面的文件在临时目录里（和真的那一页无关），另开一个端口、证书是测试的证书机构签的
fn web_extra(d: &Path) -> Vec<String> {
    let tls = d.join("web-tls");
    std::fs::create_dir(&tls).unwrap_or_else(|e| panic!("建 {}：{e}", tls.display()));
    pki::make_ca_cert(&tls);
    let root = d.join("web-root");
    fsx::write(root.join("index.html"), PAGE);
    fsx::write(root.join("app.js"), script());
    fsx::write(root.join("icon.png"), [&b"\x89PNG\r\n\x1a\n"[..], &crypto::random(64)].concat());
    fsx::write(root.join("README.md"), "不是页面的一部分\n");
    fsx::write(root.join(".secret.js"), "x");
    fsx::write(root.join("sub").join("x.js"), "x");
    web_args(d)
}

/// 网页端的端口（服务器起来以后写在 d/wport 里）
fn wport_in(d: &Path) -> u16 {
    let written = wait_for("网页端的端口起来", WAIT, || fsx::try_read_text(d.join("wport")).filter(|s| !s.is_empty()));
    written.trim().parse().unwrap_or_else(|_| panic!("端口文件里写的不是端口：{written:?}"))
}

/// 开着网页端的服务器，box、laptop 两台设备登记在上面。返回 (夹具, 网页端, box, laptop)。
/// origin：开 WebSocket 时带的 Origin（pair_list_attach_type 那几条不带）
fn web_fx_with(t: &mut Ctx, origin: Option<&str>) -> (Base, Web, Device, Device) {
    let fx = Base::with(t, true, web_extra);
    let ca = fx.d.join("web-tls").join("ca.pem");
    let web = Web { wport: wport_in(&fx.d), ca, origin: origin.map(str::to_string), held: RefCell::new(Vec::new()) };
    let (a, b) = (fx.device("box"), fx.device("laptop"));
    fx.machines(&[&a, &b]);
    (fx, web, a, b)
}

fn web_fx(t: &mut Ctx) -> (Base, Web, Device, Device) {
    web_fx_with(t, Some(ORIGIN))
}

/// 重启服务器（原来的端口、同一个目录：记下的东西都在），多带 extra 这几个参数
fn restart(fx: &mut Base, port: u16, extra: Vec<String>) {
    fx.srv.stop();
    let srv = RelayServer::start(&fx.d, &RelayOpts::new().sniff(&fx.sniff).port(port).extra(extra));
    fx.srv = srv;
}

/// dev 开一次给网页端的配对、放上它那一份。返回 (配对码的字节, rid, Kc, 它的随机数)
fn open_web(fx: &Base, dev: &Device) -> (Vec<u8>, String, [u8; 32], Vec<u8>) {
    let code = crypto::random(10);
    let (kc, keys, rid) = spec::pair_keys(&code);
    let na = crypto::random(32);
    let j = fx.good(dev, "pair_open", &obj! { "rid" => &rid, "web" => true });
    assert_eq!(j["web"].str(), format!("{ORIGIN}/"));
    fx.good(dev, "pair_put", &obj! { "rid" => &rid, "m" => spec::pair_msg(&keys, &rid, "a", dev.fp(), &na, &dev.name) });
    (code, rid, kc, na)
}

/// dev 和访客 g 配好对。返回 (密钥, 配对编号)
fn pair_guest(fx: &Base, web: &Web, dev: &Device, g: &mut Guest) -> ([u8; 32], String) {
    let (code, rid, _kc, _na) = open_web(fx, dev);
    let (p, pid) = g.join(web, &code, Some(dev.fp()));
    let j = fx.good(dev, "pair_get", &obj! { "rid" => &rid });
    assert_eq!((j["fp"].str(), &j["name"]), (g.fp.as_str(), &Value::Null), "访客不在 Team 的名单里：服务器不知道它叫什么");
    let theirs = spec::pair_open(&spec::pair_keys(&code).1, &rid, "b", &j["m"]).expect("访客放的那一份认证得过");
    assert_eq!(theirs["name"].str(), g.name);
    assert_eq!(fx.good(dev, "pair_commit", &obj! { "rid" => &rid, "pid" => &pid })["done"], Value::Bool(true));
    assert_eq!(g.good(web, "pair_get", &obj! { "rid" => &rid, "until" => "done" })["done"], Value::Bool(true));
    (p, pid)
}

/// 服务器记着的配对和访客（d/remote.json）；还没有这个文件就是都空着
fn remote(d: &Path) -> Value {
    let p = d.join("remote.json");
    if p.exists() {
        fsx::read_json(&p)
    } else {
        obj! { "edges" => obj! {}, "guests" => obj! {} }
    }
}

fn page_files(t: &mut Ctx) {
    let (fx, web, _a, _b) = web_fx(t);
    let r = web.get("/", &[]);
    assert_eq!((r.status, r.body.as_slice(), r.header("content-type")), (200, PAGE, "text/html; charset=utf-8"));
    assert_eq!(r.header("cache-control"), "no-cache");
    let csp = r.header("content-security-policy");
    let wanted =
        ["default-src 'none'", "script-src 'self'", "connect-src 'self' wss://relay.example.com:8443", "frame-ancestors 'none'"];
    for want in wanted {
        assert!(csp.contains(want), "{want}：{csp}");
    }
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    assert_eq!((r.header("x-content-type-options"), r.header("referrer-policy")), ("nosniff", "no-referrer"));
    assert_eq!(web.get("/index.html", &[]).body, PAGE);
    assert_eq!(web.get("/?from=home", &[]).body, PAGE);
    // 没变：304，不再发内容；压缩的那一份解出来一样
    let same = web.get("/", &[("If-None-Match", r.header("etag"))]);
    assert_eq!((same.status, same.body.as_slice()), (304, &b""[..]));
    let r = web.get("/app.js", &[("Accept-Encoding", "gzip, br")]);
    assert_eq!((r.status, r.header("content-encoding"), r.header("content-type")), (200, "gzip", "text/javascript; charset=utf-8"));
    assert!(r.body.len() < script().len() / 4, "{}", r.body.len());
    assert_eq!(gunzip(&r.body), script());
    assert_eq!(web.get("/app.js", &[]).body, script());
    let r = web.get("/icon.png", &[("Accept-Encoding", "gzip")]);
    assert_eq!(
        (r.status, r.header("content-type"), r.headers.contains_key("content-encoding"), r.body.len()),
        (200, "image/png", false, 72)
    );
    // 不是页面的文件、目录里别的东西、往外走的路径：都没有
    let outside = [
        "/README.md",
        "/.secret.js",
        "/sub/x.js",
        "/sub",
        "/../state.json",
        "/%2e%2e/state.json",
        "/app.js/",
        "/state.json",
        "/remote.json",
        "/nope.js",
    ];
    for path in outside {
        assert_eq!(web.get(path, &[]).status, 404, "{path}");
    }
    let j = web.get("/v1/health", &[]).json();
    assert_eq!((j["web"].str(), &j["ok"]), (format!("{ORIGIN}/").as_str(), &Value::Bool(true)));
    // 原来那个端口（自签的证书，给 cca 的设备）照旧；浏览器不认它的证书
    let health = format!("https://127.0.0.1:{}/v1/health", fx.srv.port);
    let loose = fetch(&health, tls::Client::insecure(), &[], secs(20)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(loose.json()["web"].str(), format!("{ORIGIN}/"));
    // 原来等的是 ssl.SSLError：握手里证书没验过（不是连不上、不是等过了时限）
    let Err(refused) = fetch(&health, ca_client(&web.ca), &[], secs(20)) else {
        panic!("照测试的证书机构验，给设备的那个端口（自签的证书）不该验得过")
    };
    assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{refused}");
    assert!(refused.to_string().contains("certificate"), "{refused}");
}

fn guest_pairing_and_what_a_guest_may_do(t: &mut Ctx) {
    let (mut fx, mut web, a, b) = web_fx(t);
    let d = fx.d.clone();
    let (mut g, other) = (Guest::new(&d, "iPhone"), Guest::new(&d, "iPad"));
    let (code, rid, _kc, _na) = open_web(&fx, &a);
    let keys = spec::pair_keys(&code).1;
    let m = spec::pair_msg(&keys, &rid, "b", &g.fp, &crypto::random(32), &g.name);
    // 不在名单里的：没带公钥、公钥和指纹对不上、签名不是这把密钥签的，都进不来；回的都一样（和不在名单里的设备收到的一样）
    let denied = fx.unknown();
    assert_eq!(g.post(&web, "pair_put", &obj! { "rid" => &rid, "m" => &m }), denied);
    assert_eq!(g.post(&web, "pair_put", &obj! { "rid" => &rid, "m" => &m, "pub" => &other.pubkey }), denied);
    assert_eq!(g.post_as(&web, "pair_put", &obj! { "rid" => &rid, "m" => &m, "pub" => &g.pubkey }, &other.keyfile), denied);
    assert_eq!(g.post(&web, "pair_get", &obj! { "rid" => &rid, "pub" => &g.pubkey }), denied, "还没放过密文的：别的请求不行");
    assert_eq!(
        g.post(&web, "pair_put", &obj! { "rid" => crypto::token_hex(16), "m" => &m, "pub" => &g.pubkey }),
        denied,
        "不知道配对码（rid 对不上）"
    );
    // 不是开给网页端的配对：访客进不去
    let code2 = crypto::random(10);
    let (_kc2, keys2, rid2) = spec::pair_keys(&code2);
    fx.good(&a, "pair_open", &obj! { "rid" => &rid2 });
    let m2 = spec::pair_msg(&keys2, &rid2, "b", &g.fp, &crypto::random(32), "x");
    assert_eq!(g.post(&web, "pair_put", &obj! { "rid" => &rid2, "pub" => &g.pubkey, "m" => m2 }), denied);
    fx.good(&a, "pair_close", &obj! { "rid" => &rid2 });
    assert_eq!(remote(&d)["guests"], obj! {});

    let (p, pid) = g.join(&web, &code, Some(a.fp()));
    assert_eq!(
        other.post(&web, "pair_put", &obj! { "rid" => &rid, "pub" => &other.pubkey, "m" => &m }),
        denied,
        "已经有人输入了：第二个访客进不来"
    );
    assert_eq!(remote(&d)["guests"], obj! {}, "还没配成：不记");
    assert_eq!(fx.good(&a, "pair_get", &obj! { "rid" => &rid })["fp"].str(), g.fp);
    assert_eq!(fx.good(&a, "pair_commit", &obj! { "rid" => &rid, "pid" => &pid })["done"], Value::Bool(true));
    let doc = remote(&d);
    assert_eq!(doc["guests"][g.fp.as_str()]["key"].str(), g.pubkey);
    let edge = &doc["edges"][pid.as_str()];
    assert_eq!((edge["a"].str(), edge["b"].str()), (a.fp(), g.fp.as_str()));
    assert!(!dumps(&doc).contains(&crypto::hex(&p)), "服务器记下的东西里不该有两边之间的密钥");

    // 访客能做的：看配对的设备上的会话、接入、取消配对。别的都不行
    assert_eq!(
        g.good(&web, "relay_list", &obj! { "pids" => list![&pid] }),
        obj! {
            "v" => 1,
            "ok" => true,
            "peers" => obj! { &pid => obj! { "fp" => a.fp(), "name" => "box" } },
            "sessions" => obj! { &pid => list![] },
        }
    );
    let no_session = (404, obj! { "error" => "no_session" });
    assert_eq!(g.post(&web, "relay_attach", &obj! { "sid" => "ab".repeat(16), "pid" => &pid }), no_session);
    let refused = [
        ("pair_open", obj! { "rid" => crypto::token_hex(16) }),
        ("pair_open", obj! { "rid" => crypto::token_hex(16), "web" => true }),
        ("relay_host", obj! { "sid" => crypto::token_hex(16), "pids" => list![&pid] }),
        ("report", obj! { "sessions" => list![] }),
        ("view", obj! { "accounts" => list![] }),
        ("keys", obj! {}),
        ("ledger", obj! {}),
    ];
    for (op, extra) in &refused {
        assert_eq!(g.post(&web, op, extra), denied, "{op}");
    }
    // 已经是访客了：再和别的设备配对，也只进得了开给网页端的（服务器认得这个访客：回的是 denied）
    let (_kc3, keys3, rid3) = spec::pair_keys(&crypto::random(10));
    fx.good(&b, "pair_open", &obj! { "rid" => &rid3 });
    let m3 = spec::pair_msg(&keys3, &rid3, "b", &g.fp, &crypto::random(32), "x");
    assert_eq!(g.post(&web, "pair_put", &obj! { "rid" => &rid3, "m" => m3 }), (403, obj! { "error" => "denied" }));
    fx.good(&b, "pair_close", &obj! { "rid" => &rid3 });
    let (_p2, pid2) = pair_guest(&fx, &web, &b, &mut g);
    let (mut edges, mut both) = (names(&remote(&d)["edges"]), vec![pid.as_str(), pid2.as_str()]);
    edges.sort();
    both.sort();
    assert_eq!(edges, both);
    // Team 里的设备也可以输入开给网页端的配对码（它不是访客）
    let (code4, rid4, _kc4, _na4) = open_web(&fx, &a);
    let m4 = spec::pair_msg(&spec::pair_keys(&code4).1, &rid4, "b", b.fp(), &crypto::random(32), "laptop");
    fx.good(&b, "pair_put", &obj! { "rid" => &rid4, "m" => m4 });
    fx.good(&a, "pair_close", &obj! { "rid" => &rid4 });
    assert_eq!(names(&remote(&d)["guests"]), [g.fp.as_str()]);

    // 重启以后访客还在；取消一个配对它还在，最后一个也取消了就删掉，之后它的请求不认
    let port = fx.srv.port;
    restart(&mut fx, port, web_args(&d));
    web.wport = wport_in(&d);
    let both = obj! { "pids" => list![&pid, &pid2] };
    assert_eq!(names(&g.good(&web, "relay_list", &both)["peers"]), [pid.as_str(), pid2.as_str()]);
    fx.good(&a, "pair_drop", &obj! { "pid" => &pid });
    assert_eq!(names(&remote(&d)["guests"]), [g.fp.as_str()]);
    assert_eq!(names(&g.good(&web, "relay_list", &both)["peers"]), [pid2.as_str()]);
    g.good(&web, "pair_drop", &obj! { "pid" => &pid2 });
    assert_eq!((&remote(&d)["guests"], &remote(&d)["edges"]), (&obj! {}, &obj! {}));
    assert_eq!(g.post(&web, "relay_list", &obj! { "pids" => list![&pid2] }), denied);
}

fn the_sponsor_is_revoked(t: &mut Ctx) {
    let (fx, web, a, b) = web_fx(t);
    let mut g = Guest::new(&fx.d, "iPhone");
    let (_p, pid) = pair_guest(&fx, &web, &a, &mut g);
    assert_eq!(names(&remote(&fx.d)["guests"]), [g.fp.as_str()]);
    fx.machines(&[&b]); // 领它进来的那台设备被移出了 Team：配对没了，访客也就没了
    let doc = remote(&fx.d);
    assert_eq!((&doc["edges"], &doc["guests"]), (&obj! {}, &obj! {}));
    assert_eq!(g.post(&web, "relay_list", &obj! { "pids" => list![&pid] }), fx.unknown());
}

/// 访客经配对 pid 看到的会话（relay_list 回的 sessions 里这个配对的那几行）
fn listed(g: &Guest, web: &Web, pid: &str) -> Value {
    g.good(web, "relay_list", &obj! { "pids" => list![pid] })["sessions"][pid].clone()
}

fn attach_over_websocket(t: &mut Ctx) {
    let (fx, web, a, _b) = web_fx(t);
    let mut g = Guest::new(&fx.d, "iPhone");
    let (p, pid) = pair_guest(&fx, &web, &a, &mut g);
    let sid = crypto::token_hex(16);
    let ht = fx.good(&a, "relay_host", &obj! { "sid" => &sid, "pids" => list![&pid] });
    let key = unb64(&ht["tkey"]);
    let (mut host, ok) = dial(fx.srv.data_port(), "host", ht["tid"].str(), &key);
    assert_eq!(read_line(&mut host), ok);
    let info = obj! { "v" => 1, "machine" => "box", "tool" => "claude", "name" => "工作", "cwd" => "~/项目", "since" => 1, "access" => "control" };
    let blob = spec::dir_seal(&p, &pid, &sid, &info);
    sendall(&mut host, &ctl(F_ANNOUNCE, dumps(&obj! { &pid => blob }).as_bytes()));
    let rows = wait_for("访客看到会话", WAIT, || Some(listed(&g, &web, &pid)).filter(truthy));
    let row = only(&rows);
    assert_eq!(spec::dir_open(&p, &pid, &sid, &row["blob"]).expect("会话的说明认证得过")["cwd"].str(), "~/项目");

    // 握手不对的、不是自己那一页发起的：进不来
    assert_eq!(web.status(g.ws(&web, None, &[("Sec-WebSocket-Version", Some("8"))])), 400);
    assert_eq!(web.status(g.ws(&web, None, &[("Upgrade", None)])), 400);
    assert_eq!(web.status(web.ws(Some("https://evil.example.com"), "/v1/relay/ws", &[])), 403);
    assert_eq!(web.get("/v1/relay/ws", &[]).status, 400);
    // 手里没有票据的开不了 WebSocket（没有凭证的人占不了名额）：不带、乱写的、宿主那种票据、同一张用第二次
    let host_ticket = format!("?t={}", ht["tid"].str());
    for query in ["", &format!("?t={}", "0".repeat(32)), "?t=xyz", "?x=1", &host_ticket] {
        let w = web.ws(web.origin.as_deref(), &format!("/v1/relay/ws{query}"), &[]);
        assert_eq!(web.status(w), 403, "{query}");
    }
    let tv = g.ticket(&web, &sid);
    assert_eq!(web.status(g.ws(&web, Some(tv["tid"].str()), &[])), 101);
    assert_eq!(web.status(g.ws(&web, Some(tv["tid"].str()), &[])), 403, "一张票据只开得了一条");

    let (mut view, vok) = g.attach(&web, &sid);
    let (kind, payload) = read_ctl(&mut host);
    assert_eq!(kind, Some(F_JOIN));
    let j = json::parse_bytes(&payload).unwrap_or_else(|e| panic!("有人要接入的那一帧不是 JSON：{e}"));
    let (mut join, jok) = dial(fx.srv.data_port(), "join", j["token"].str(), &key);
    assert_eq!((read_line(&mut join), read_line(&mut view)), (jok, vok));
    // 两头握手、收发（宿主那头是普通的 TCP，访客那头经 WebSocket）
    let mut hc = Channel::new("host", &p, &pid, &sid, a.fp(), &g.fp);
    let mut vc = Channel::new("viewer", &p, &pid, &sid, a.fp(), &g.fp);
    sendall(&mut view, &vc.hello());
    let reply = hc.reply(&read_exact(&mut join, 52)).expect("宿主认访客的开场");
    sendall(&mut join, &reply);
    let confirm = vc.confirm(&read_exact(&mut view, 68)).expect("访客认宿主的证明");
    sendall(&mut view, &confirm);
    assert!(hc.accept(&read_exact(&mut join, 35)));
    sendall(&mut view, &vc.seal(b"keys from the phone"));
    let mut buf = read_exact(&mut join, 8 + 2 + 32 + 16);
    assert_eq!(hc.open(&mut buf), Ok(Some(b"keys from the phone".to_vec())));
    // 大块的、切成碎帧的、几帧并成一次发的：一个字节不丢、不乱
    let big = crypto::random(700 * 1024);
    sendall(&mut join, &big);
    assert!(read_exact(&mut view, big.len()) == big, "宿主那头发的 700 KB 到访客这边不一样了");
    let up = crypto::random(300 * 1024);
    let frames = [
        ws_frame(2, &up[..10], true, false),
        ws_frame(0, &up[10..5000], true, false),
        ws_frame(0, &up[5000..70000], true, true),
        ws_frame(9, b"ping me", true, true),
        ws_frame(2, &up[70000..], true, true),
    ];
    view.raw(&frames.concat());
    assert!(read_exact(&mut join, up.len()) == up, "访客这边发的 300 KB 到宿主那头不一样了");
    sendall(&mut join, b"!");
    assert_eq!(recv(&mut view, 1), b"!");
    assert_eq!(view.pongs, [b"ping me".to_vec()]);
    assert_eq!(listed(&g, &web, &pid)[0]["viewers"], Value::from(1));
    // 宿主那头断了：访客这边收到关闭
    drop(join);
    assert_eq!(recv(&mut view, 1), b"");
    assert!(view.closed);
    wait_for("这一处接入算结束", WAIT, || listed(&g, &web, &pid)[0]["viewers"] == Value::from(0));

    // 不合规矩的帧（没加掩码、文本的）：断开这一条，服务器照常
    for bad in [ws_frame(2, b"x", false, true), ws_frame(1, b"text", true, true)] {
        let (mut w, _) = g.open_ws(&web, &sid);
        w.raw(&bad);
        assert_eq!(recv(&mut w, 1), b"");
        web.hold(w);
    }
    // 开场白不对（网址里的票据是真的，开场白里证明不了有它的密钥）：中转回一行原因就关，那张票据不算用掉
    let (mut w, tk) = g.open_ws(&web, &sid);
    sendall(&mut w, &spec::relay_line("view", tk["tid"].str(), &[b'k'; 32]).0);
    assert_eq!(read_line(&mut w), b"CCA-RELAY/1 ERR ticket\n");
    assert_eq!(recv(&mut w, 1), b"");
    // 连上不说话：5 秒后被中转断开（和数据端口上一样，先说一行原因）
    let (mut w, _) = g.open_ws(&web, &sid);
    let t0 = Instant::now();
    assert_eq!(read_line(&mut w), b"CCA-RELAY/1 ERR slow\n");
    assert_eq!(recv(&mut w, 1), b"");
    assert!(t0.elapsed() < secs(12), "{:?}", t0.elapsed());
    // 之后还接得上
    let (_view, _vok) = g.attach(&web, &sid);
    assert_eq!(read_ctl(&mut host).0, Some(F_JOIN));
}

/// 同一个地址开得了的 WebSocket 有上限；占着它们不影响普通请求
fn many_websockets(t: &mut Ctx) {
    let (fx, web, a, _b) = web_fx(t);
    let mut g = Guest::new(&fx.d, "iPhone");
    let (p, pid) = pair_guest(&fx, &web, &a, &mut g);
    let sid = crypto::token_hex(16);
    let ht = fx.good(&a, "relay_host", &obj! { "sid" => &sid, "pids" => list![&pid] });
    let (mut host, ok) = dial(fx.srv.data_port(), "host", ht["tid"].str(), &unb64(&ht["tkey"]));
    assert_eq!(read_line(&mut host), ok);
    let blob = spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1 });
    sendall(&mut host, &ctl(F_ANNOUNCE, dumps(&obj! { &pid => blob }).as_bytes()));
    wait_for("访客看到会话", WAIT, || truthy(&listed(&g, &web, &pid)));
    // 同一个 IP 最多 8 条（服务器的 WS_CONN_IP）
    let socks: Vec<WsSock> = (0..9).map(|_| g.ws(&web, Some(g.ticket(&web, &sid)["tid"].str()), &[])).collect();
    assert_eq!(socks.iter().map(|w| w.status).collect::<Vec<_>>(), [vec![101; 8], vec![429]].concat());
    assert_eq!(web.get("/", &[]).status, 200);
    assert_eq!(web.get("/v1/health", &[]).json()["ok"], Value::Bool(true));
    drop(socks);
    wait_for("名额还了", secs(20), || web.status(g.ws(&web, Some(g.ticket(&web, &sid)["tid"].str()), &[])) == 101);
}

/// 关掉网页端（重启时不带那几个参数）：记下的访客的请求一律不认；再打开，它还在
fn the_web_side_is_switched_off(t: &mut Ctx) {
    let (mut fx, mut web, a, _b) = web_fx(t);
    let d = fx.d.clone();
    let mut g = Guest::new(&d, "iPhone");
    let (_p, pid) = pair_guest(&fx, &web, &a, &mut g);
    let port = fx.srv.port;
    restart(&mut fx, port, Vec::new());
    let pids = obj! { "pids" => list![&pid] };
    assert_eq!(signed_once(port, &g.keyfile, &g.fp, "relay_list", &pids), fx.unknown());
    assert_eq!(names(&remote(&d)["guests"]), [g.fp.as_str()], "记录留着");
    assert_eq!(names(&fx.good(&a, "relay_list", &pids)["peers"]), [pid.as_str()], "电脑那一头照旧");
    restart(&mut fx, port, web_args(&d));
    web.wport = wport_in(&d);
    assert_eq!(names(&g.good(&web, "relay_list", &pids)["peers"]), [pid.as_str()]);
}

// ---- 证书后到的、没开网页端的服务器 ----

/// 网页端的证书启动时还没有（证书助手和服务端一起装上时，头一张要等它去要）：服务照样起来，那个端口上握不了手；
/// 证书出现以后几秒钟内自己用上，不用重启
fn the_certificate_arrives_later(t: &mut Ctx) {
    let fx = Base::with(t, true, |d| {
        let (tls, root) = (d.join("late-tls"), d.join("web-root"));
        std::fs::create_dir(&tls).unwrap_or_else(|e| panic!("建 {}：{e}", tls.display()));
        fsx::write(root.join("index.html"), b"<!doctype html><title>cca</title>");
        let args = [
            ("--web-url", "https://relay.example.com:8443/".to_string()),
            ("--web-listen", "127.0.0.1:0".to_string()),
            ("--web-port-file", text(&d.join("wport"))),
            ("--web-cert", text(&tls.join("cert.pem"))),
            ("--web-key", text(&tls.join("key.pem"))),
            ("--web-root", text(&root)),
        ];
        args.into_iter().flat_map(|(name, value)| [name.to_string(), value]).collect()
    });
    let certs = fx.d.join("late-tls");
    let wport = wport_in(&fx.d);
    // 首页的状态码；握不成手、连接被断开是 Err
    let page = |client: tls::Client| fetch(&format!("https://127.0.0.1:{wport}/"), client, &[], secs(10)).map(|r| r.status);
    let early = page(tls::Client::insecure());
    assert!(early.is_err(), "证书还没有：握手该握不成（连接被断开），却回了 {early:?}");
    // 给 cca 的设备用的那个端口不受影响
    let health = format!("https://127.0.0.1:{}/v1/health", fx.srv.port);
    let r = fetch(&health, tls::Client::insecure(), &[], secs(10)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.json()["ok"], Value::Bool(true));
    pki::make_ca_cert(&certs); // 证书来了（先有私钥、后有证书：中间那一下读不全，下一次再看）
    wait_for("证书出现以后用上了", secs(30), || {
        matches!(tls::Client::with_ca_file(certs.join("ca.pem")).and_then(&page), Ok(200))
    });
}

/// 没开网页端的服务器（和以前一样）：没有那一页、没有 WebSocket，设备开不了给网页端的配对，不在名单里的一律不认
fn off(t: &mut Ctx) {
    let fx = Base::new(t);
    let a = fx.device("box");
    fx.machines(&[&a]);
    for path in ["/", "/index.html", "/v1/relay/ws"] {
        let key = crypto::b64(&crypto::random(16));
        let upgrade = [
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Version", "13"),
            ("Sec-WebSocket-Key", key.as_str()),
        ];
        let url = format!("https://127.0.0.1:{}{path}", fx.srv.port);
        let r = fetch(&url, tls::Client::insecure(), &upgrade, secs(10)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.status, 404, "{path}");
    }
    let (_kc, keys, rid) = spec::pair_keys(&crypto::random(10));
    assert_eq!(fx.call(&a, "pair_open", &obj! { "rid" => &rid, "web" => true }), (404, obj! { "error" => "no_web" }));
    fx.good(&a, "pair_open", &obj! { "rid" => &rid });
    let g = Guest::with_key(fx.d.join("gk"), "x");
    let m = spec::pair_msg(&keys, &rid, "b", &g.fp, &crypto::random(32), "x");
    let put = obj! { "rid" => &rid, "pub" => &g.pubkey, "m" => m };
    assert_eq!(signed_once(fx.srv.port, &g.keyfile, &g.fp, "pair_put", &put), fx.unknown());
}

// ---- 浏览器里跑的那一份（server/web/core.js），在 Node 里 ----

const NO_NODE: &str = "没有 Node 22 以上：浏览器里跑的那一份（server/web/core.js）这次没测";

/// PATH 上的 Node，22 以上的才算（自带 WebSocket、WebCrypto 的 Ed25519）；没有是 None（回归不要求装 Node）
fn node_bin() -> Option<PathBuf> {
    static NODE: OnceLock<Option<PathBuf>> = OnceLock::new();
    NODE.get_or_init(|| {
        let node = proc::which("node")?;
        let out = Cmd::new(&node).arg("--version").pass_env(&["PATH"]).timeout(secs(20)).output().ok()?;
        let version = out.out();
        let major: u32 = version.trim().trim_start_matches('v').split('.').next()?.parse().ok()?;
        (major >= 22).then_some(node)
    })
    .clone()
}

/// 一次命令最多等 Node 那一头多久（最长的一条是等会话的输出，自己限了 40 秒；原来没有时限，卡住了要等到用例超时）
const NODE_WAIT: Duration = Duration::from_secs(120);

/// 跑着 tests/web_client.mjs 的一个 Node 进程：call(命令, 参数) 发一条、等它回一条。离开作用域时杀掉
struct Node {
    child: Child,
    stdin: File,
    /// 它在标准输出上回的，一行一条
    lines: Receiver<String>,
    /// 它在标准错误上说的（退出了的时候拿来说明原因）
    err: Arc<Mutex<Vec<u8>>>,
}

impl Node {
    /// ca：照这个证书机构认服务器的证书。环境里只有 PATH 和 Node 自己的两个开关（不带代理的那些：只连本机）
    fn start(node: &Path, ca: Option<&Path>) -> Node {
        let mut cmd = Cmd::new(node);
        cmd.arg(harness::repo().join("tests").join("web_client.mjs")).pass_env(&["PATH"]).env("NODE_NO_WARNINGS", "1");
        if let Some(ca) = ca {
            cmd.env("NODE_EXTRA_CA_CERTS", ca);
        }
        let mut child = cmd.stdin(Stdio::Piped).stdout(Stdio::Piped).stderr(Stdio::Piped).spawn().unwrap_or_else(|e| panic!("{e}"));
        let stdin = child.stdin.take().expect("标准输入接着管道");
        let stdout = BufReader::new(child.stdout.take().expect("标准输出接着管道"));
        let mut stderr = child.stderr.take().expect("标准错误接着管道");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let err = Arc::new(Mutex::new(Vec::new()));
        let sink = err.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = stderr.read(&mut buf) {
                sink.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
            }
        });
        Node { child, stdin, lines, err }
    }

    /// 它到现在为止在标准错误上说的，最后 2000 个字节
    fn said(&self) -> String {
        let err = self.err.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&err[err.len().saturating_sub(2000)..]).into_owned()
    }

    /// 发一条命令（args 是它的参数，一个对象）、等它回一条
    fn call(&mut self, cmd: &str, args: Value) -> Value {
        let mut doc = args.obj().clone();
        doc.insert("cmd", cmd);
        // 写不进去就是它已经退出了：下面等回话的地方说
        let _ = self.stdin.write_all(format!("{}\n", dumps(&Value::Obj(doc))).as_bytes());
        match self.lines.recv_timeout(NODE_WAIT) {
            Ok(line) => parsed(&line),
            Err(RecvTimeoutError::Disconnected) => {
                // 标准错误可能还差最后几个字节没读进来：等它退出再看
                let _ = self.child.wait_timeout(secs(2));
                std::thread::sleep(Duration::from_millis(100));
                panic!("Node 那一头退出了：{}", self.said())
            }
            Err(RecvTimeoutError::Timeout) => panic!("Node 那一头过了 {} 秒还没回 {cmd}：{}", NODE_WAIT.as_secs(), self.said()),
        }
    }

    /// 该成的命令：回的 ok 不是真就 panic
    #[track_caller]
    fn good(&mut self, cmd: &str, args: Value) -> Value {
        let j = self.call(cmd, args);
        assert!(j.get("ok").is_some_and(truthy), "{cmd} 没成：{j}");
        j
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 有 Node 才跑的用例开头调：没有就跳过
fn need_node(t: &Ctx) -> PathBuf {
    node_bin().unwrap_or_else(|| t.skip(NO_NODE))
}

// 浏览器里跑的那一份（server/web/core.js）和照文档独立实现的 kit::spec 对：同样的输入算出同样的东西，互相握得上手、
// 解得开对方的记录。不起服务器

fn the_same_numbers(t: &mut Ctx) {
    let mut node = Node::start(&need_node(t), None);
    let b = |x: &[u8]| crypto::b64(x);
    let code = crypto::random(10);
    let (kc, keys, rid) = spec::pair_keys(&code);
    let (na, nb) = (crypto::random(32), crypto::random(32));
    let fp = || format!("SHA256:{}", crypto::b64(&crypto::random(32)).trim_end_matches('='));
    let (fpa, fpb) = (fp(), fp());
    let (p, pid) = spec::pair_secret(&kc, &na, &nb, &fpa, &fpb);
    let sid = crypto::token_hex(16);
    let info = obj! {
        "v" => 1,
        "machine" => "书房的 Mac",
        "tool" => "claude",
        "name" => "工作\x1b[31m",
        "cwd" => format!("~/项目/{}", "x".repeat(300)),
        "since" => 1794000000,
        "access" => "view",
    };
    let (blob, key) = (crypto::random(1000), crypto::random(32));
    let text = spec::code_text(&code);
    let first: String = text.chars().take(1).collect();
    let bad_codes = list!["", &text[..text.len() - 1], format!("{text}A"), text.replace(&first, "U"), "u".repeat(16),];
    let j = node.good(
        "vectors",
        obj! {
            "code" => text.to_lowercase().replace('-', " ").replace('1', "l").replace('0', "o"),
            "na" => b(&na),
            "nb" => b(&nb),
            "fpa" => &fpa,
            "fpb" => &fpb,
            "msg_a" => spec::pair_msg(&keys, &rid, "a", &fpa, &na, "书房的 Mac"),
            "name_b" => "我的 iPhone",
            "sid" => &sid,
            "dir" => spec::dir_seal(&p, &pid, &sid, &info),
            "blob" => b(&blob),
            "key" => b(&key),
            "bad_codes" => bad_codes,
        },
    );
    assert_eq!(cols(&j, &["code_text", "rid", "p", "pid"]), list![&text, &rid, crypto::hex(&p), &pid]);
    assert_eq!(cols(&j, &["sha", "mac"]), list![crypto::sha256_hex(&blob), crypto::hex(&spec::h(&key, &blob))]);
    assert_eq!(j["opened"], obj! { "fp" => &fpa, "n" => crypto::hex(&na), "name" => "书房的 Mac" });
    assert_eq!(
        spec::pair_open(&keys, &rid, "b", &j["msg_b"]),
        Some(obj! { "v" => 1, "fp" => &fpb, "n" => crypto::hex(&nb), "name" => "我的 iPhone" })
    );
    assert_eq!(unb64(&j["msg_b"]["c"]).len(), 256, "补齐到 256 的整数倍（按字节算，不是按字）");
    // 会话的说明：控制字符去掉、太长的截掉；换一个会话编号就打不开
    assert_eq!(
        j["dir"],
        obj! {
            "machine" => "书房的 Mac",
            "tool" => "claude",
            "name" => "工作[31m",
            "cwd" => format!("~/项目/{}", "x".repeat(195)),
            "since" => 1794000000,
            "access" => "view",
        }
    );
    assert_eq!(j["dir_bad"], Value::Null);
    assert_eq!(j["bad_codes"], list![true, true, true, true, true]);
}

fn handshake_and_records(t: &mut Ctx) {
    let mut node = Node::start(&need_node(t), None);
    let b = |x: &[u8]| crypto::b64(x);
    let (p, pid, sid) = (crypto::random(32), crypto::token_hex(16), crypto::token_hex(16));
    let (fph, fpv) = ("SHA256:host", "SHA256:viewer");
    let channel = obj! { "p" => b(&p), "pid" => &pid, "sid" => &sid, "fph" => fph, "fpv" => fpv };
    let mut host = Channel::new("host", &p, &pid, &sid, fph, fpv);
    let hello = unb64(&node.good("channel", channel.clone())["hello"]);
    let reply = host.reply(&hello).expect("宿主认 Node 那一头的开场");
    let confirm = node.good("confirm", obj! { "reply" => b(&reply) })["confirm"].clone();
    assert!(host.accept(&unb64(&confirm)));
    // 两个方向各发几条：空的、一个字节、正好一条的上限
    let sizes = [0, 1, 30, 31, 1000, 16384];
    let data: Vec<Vec<u8>> = sizes.iter().map(|n| crypto::random(*n)).collect();
    let recs = node.good("seal", obj! { "data" => data.iter().map(|d| b(d)).collect::<Vec<_>>() })["records"].clone();
    let mut buf: Vec<u8> = recs.list().iter().flat_map(unb64).collect();
    let opened: Vec<Vec<u8>> = sizes.iter().map(|_| host.open(&mut buf).expect("记录认证得过").expect("一条完整的记录")).collect();
    assert!(opened == data, "Node 那一头封的几条，这边解出来不一样");
    assert_eq!(buf.len(), 0);
    let mut sent: Vec<u8> = data.iter().flat_map(|d| host.seal(d)).collect();
    sent.extend_from_slice(&host.seal(b"tail")[..20]);
    let j = node.good("open", obj! { "bytes" => b(&sent) });
    let got: Vec<Vec<u8>> = j["got"].list().iter().map(unb64).collect();
    assert!(got == data, "这边封的几条，Node 那一头解出来不一样");
    assert_eq!(j["left"], Value::from(20), "最后半条留着等后面的");
    // 改一个字节、漏一条（序号对不上）：认证不过
    let flipped = |r: Vec<u8>, _: &mut Channel| [&r[..12], &[r[12] ^ 1], &r[13..]].concat();
    let skipped = |_: Vec<u8>, host: &mut Channel| host.seal(b"next");
    let bads: [&dyn Fn(Vec<u8>, &mut Channel) -> Vec<u8>; 2] = [&flipped, &skipped];
    for bad in bads {
        let record = host.seal(b"skipped");
        let j = node.good("open", obj! { "bytes" => b(&bad(record, &mut host)) });
        assert_eq!((&j["got"], j.get("error")), (&list![], Some(&Value::from("record auth"))));
    }
    // 宿主的证明不对（它没有这把密钥）：不发确认
    let mut other = Channel::new("host", &crypto::random(32), &pid, &sid, fph, fpv);
    let hello = unb64(&node.good("channel", channel)["hello"]);
    let reply = other.reply(&hello).expect("开场的样子是对的");
    assert_eq!(node.good("confirm", obj! { "reply" => b(&reply) })["confirm"], Value::Null);
}

/// 页面签的请求：ssh-keygen 认（照文档：命名空间 cca-presence 的 SSH 签名）；内容改过、换一个命名空间就不认。
/// 原来另有三句直接调 Python 服务器的验签（srvmod.ssh_verify）：同样的三种情况这里交给 ssh-keygen 判
fn signature(t: &mut Ctx) {
    let mut node = Node::start(&need_node(t), None);
    let data = crypto::random(300);
    let j = node.good("sign", obj! { "data" => crypto::b64(&data) });
    let (public, sig) = (j["pub"].str(), j["sig"].str());
    assert!(matches(r"^ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI[A-Za-z0-9+/]{43}$", public), "{public:?}");
    // 指纹：公钥里 base64 那一段解出来的字节的 SHA-256，base64 不带 =（服务器的 key_fp、ssh-keygen -l 都是这个写法）
    assert_eq!(j["fp"].str(), spec::fp_of_line(public));
    assert!(spec::ssh_verify(public, "cca-presence", &data, sig.as_bytes()));
    assert!(!spec::ssh_verify(public, "cca-presence", &[&data[..], b"x"].concat(), sig.as_bytes()));
    assert!(!spec::ssh_verify(public, "cca-devices", &data, sig.as_bytes()));
    let d = t.tmp("cca-web-sig-");
    fsx::write(d.join("signers"), format!("web {public}\n"));
    fsx::write(d.join("sig"), sig);
    let mut verify = Cmd::new("ssh-keygen");
    verify.args(["-Y", "verify", "-f"]).arg(d.join("signers")).args(["-I", "web", "-n", "cca-presence", "-s"]).arg(d.join("sig"));
    let r = verify.pass_env(&["PATH"]).input(data).timeout(secs(60)).output().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.status.py(), 0, "{}", r.err());
}

// ---- 整条路：真的 cca 在一头，浏览器里跑的那一份（在 Node 里）在另一头 ----
// box 上是真的 cca（辅助程序：在宿主里运行的会话、cca remote pair --web），手机那一头是浏览器里跑的那一份
// （在 Node 里：自己生成密钥、输入配对码、列会话、经 WebSocket 接入、敲键、断了自己接上）

/// 这一组用例的准备：开着网页端的服务器、两台设备、一个 Node 进程（照测试的证书机构认服务器）。
/// 返回 (夹具, 网页端, box, laptop, Node, 页面连的地址：照实际的端口)。Node 里的 WebSocket 不带 Origin（浏览器才带）；
/// 真的浏览器里对不对由界面那一轮看
fn client_fx(t: &mut Ctx) -> (Base, Web, Device, Device, Node, String) {
    let node = need_node(t);
    let (fx, web, a, b) = web_fx_with(t, None);
    let node = Node::start(&node, Some(&web.ca));
    let base = format!("https://127.0.0.1:{}", web.wport);
    (fx, web, a, b, node, base)
}

/// dev 上 cca remote pair --web（辅助程序的那一步），手机输入配对码。返回 (那一头回的, 辅助程序说的对方的名字)
fn pair_web(node: &mut Node, dev: &Device) -> (Value, Option<String>) {
    let mut p = Popen::spawn(&mut relay::helper_cmd(dev, &["remote-pair-show", "--web"]));
    let (line, web) = (p.out_line(), p.out_line());
    assert!(matches(r"^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", &line), "{line:?}");
    assert_eq!(web, "web https://relay.example.com:8443/\n", "手机上打开的地址：服务器配置里写的那个");
    let code = line.split_whitespace().nth(1).expect("code 后面是配对码");
    let j = node.call("pair", obj! { "code" => code });
    if !truthy(&j["ok"]) {
        return (j, None);
    }
    let (rc, out, err) = p.communicate(secs(60));
    assert_eq!((rc, err.as_str()), (0, ""));
    assert!(matches(r"^peer .+\n$", &out), "{out:?}");
    (j, Some(out[5..out.len() - 1].to_string()))
}

/// 等手机那一头列出来的头一台设备上有会话、而且 want 认它的会话（原来 wait_for 里的那一串）。一次没等到先歇 1.2 秒再问：
/// 每问一次，页面都要向服务器发请求，服务器给每把密钥限了流
fn phone_rows(node: &mut Node, what: &str, want: impl Fn(&Value) -> bool) -> Value {
    wait_for(what, WAIT, || {
        let rows = node.good("list", obj! {})["rows"].clone();
        let sessions = &rows[0]["sessions"];
        if truthy(sessions) && want(sessions) {
            return Some(rows);
        }
        std::thread::sleep(Duration::from_millis(1200));
        None
    })
}

/// 文字的最后 n 个字（出错的说明里用）
fn tail(text: &str, n: usize) -> &str {
    let skip = text.chars().count().saturating_sub(n);
    text.char_indices().nth(skip).map_or("", |(at, _)| &text[at..])
}

fn pair_list_attach_type(t: &mut Ctx) {
    let (fx, _web, a, _b, mut node, base) = client_fx(t);
    let me = node.good("start", obj! { "base" => &base, "name" => "我的 iPhone" });
    assert_eq!(node.good("list", obj! {})["rows"], list![], "没配对过：不问服务器");
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    // 配对码输错了、输的是别的码：没配成，说得出是哪种
    assert_eq!(node.call("pair", obj! { "code" => "not a code" })["error"], Value::from("code"));
    let other = spec::code_text(&crypto::random(10));
    assert_eq!(node.call("pair", obj! { "code" => other })["error"], Value::from("no_pairing"));
    let (j, said) = pair_web(&mut node, &a);
    assert_eq!(
        cols(&j, &["fp", "name", "steps", "extractable"]),
        list![a.fp(), "box", list!["deriving", "asking", "confirming"], false]
    );
    assert_eq!(said.as_deref(), Some("我的 iPhone"));
    let pid = j["pid"].str().to_string();
    // box 的配对表：记着它是网页端、让它看和操作；密钥两边是同一把（box 存在密钥文件里，手机那头取不出来所以比不了，靠后面握手成功）
    let row = fsx::read_json(a.cca.join("remote").join("peers.json"))["peers"][pid.as_str()].clone();
    assert_eq!(cols(&row, &["fp", "name", "incoming", "shown", "web"]), list![&me["fp"], "我的 iPhone", "control", true, true]);
    let peers = a.ok(&["remote-peers"]);
    assert!(peers.contains("我的 iPhone（手机上的网页端）  对方可以看和操作这台机器的会话"), "{peers}");
    let doc = remote(&fx.d);
    let my_fp = me["fp"].str();
    assert_eq!((names(&doc["guests"]), &doc["guests"][my_fp]["key"]), (vec![my_fp.to_string()], &me["pub"]));

    // 列会话：说明是宿主封给这个配对的（工具、账号的短名字、文件夹；没有邮箱）
    let rows = phone_rows(&mut node, "手机看到会话", |_| true);
    assert_eq!(cols(&rows[0], &["pid", "name", "gone"]), list![&pid, "box", false]);
    let s = only(&rows[0]["sessions"]);
    assert_eq!(
        (cols(s, &["tool", "name", "access", "online"]), truthy(&s["machine"])),
        (list!["claude", "alice", "control", true], true)
    );
    assert!(!dumps(&rows).contains("example.com"), "{rows}");
    let sid = s["sid"].str().to_string();

    // 接入：先收到回放，敲的键到了会话里，会话的输出回到手机上；窗口大小跟着最小的那一处
    node.good("attach", obj! { "pid" => &pid, "sid" => &sid, "cols" => 60, "rows" => 20 });
    assert!(truthy(&node.good("wait", obj! { "state" => "open" })["ok"]));
    assert!(truthy(&node.good("wait", obj! { "text" => "<ready 24 80" })["ok"]), "回放里有接入之前的输出");
    h.expect("<size 20 60>");
    node.good("input", obj! { "text" => "say hello from the phone\r" });
    let j = node.good("wait", obj! { "text" => "<hello from the phone>" });
    assert!(truthy(&j["ok"]), "{}", j["tail"]);
    h.expect("<hello from the phone>");
    node.good("resize", obj! { "cols" => 50, "rows" => 18 });
    h.expect("<size 18 50>");
    // 一大段输出（几百 KB，切成很多条记录）：一个字节不丢
    node.good("clear", obj! {});
    node.good("input", obj! { "text" => "flood\r" });
    let j = node.good("wait", obj! { "text" => "<flooded>\r", "secs" => 40, "count" => " of the flood\r" });
    assert!(truthy(&j["ok"]), "{:?}", tail(j["tail"].str(), 200));
    assert_eq!(j["count"], Value::from(20000), "两万行一行不少");
    // 连接从底下断了：自己接回来，接着用
    node.good("clear", obj! {});
    node.good("cut", obj! {});
    let j = node.good("wait", obj! { "state" => "retrying" });
    assert!(truthy(&j["ok"]), "{}", j["events"]);
    node.good("input", obj! { "text" => "say typed while offline\r" }); // 断着的时候敲的：不送（不攒着等接上）
    wait_for("重新接上", secs(30), || {
        let events = node.good("wait", obj! { "state" => "open", "secs" => 1 })["events"].clone();
        events.list().last().expect("等状态的那一条回的事件是空的")[0] == Value::from("open")
    });
    node.good("input", obj! { "text" => "say back again\r" });
    assert!(truthy(&node.good("wait", obj! { "text" => "<back again>" })["ok"]));
    h.expect("<back again>");
    assert!(!has(h.out(), "typed while offline"));
    // 自己断开：会话接着运行，窗口大小回到原来的
    let ev = node.good("detach", obj! {})["events"].clone();
    assert_eq!(ev.list().last(), Some(&list!["closed", "detached"]));
    h.expect("<size 24 80>");
    // 会话结束：接着的那一处收到退出码
    node.good("attach", obj! { "pid" => &pid, "sid" => &sid, "cols" => 80, "rows" => 24 });
    assert!(truthy(&node.good("wait", obj! { "state" => "open" })["ok"]));
    h.send("exit 7\r");
    let j = node.good("wait", obj! { "state" => "closed", "why" => "ended" });
    assert!(truthy(&j["ok"]), "{}", j["events"]);
    assert!(j["events"].list().contains(&list!["exit", 7]), "{}", j["events"]);
}

/// 手机放了密文以后没配成（这里是对面那一份打不开）：告诉服务器这次配对作废，电脑那一头马上知道，配对码不留着
fn a_pairing_that_fails_is_closed_on_the_server(t: &mut Ctx) {
    let (fx, _web, a, _b, mut node, base) = client_fx(t);
    node.good("start", obj! { "base" => &base, "name" => "iPhone" });
    let code = crypto::random(10);
    let (_kc, keys, rid) = spec::pair_keys(&code);
    fx.good(&a, "pair_open", &obj! { "rid" => &rid, "web" => true });
    // 角色写错了：认证不过
    let wrong = spec::pair_msg(&keys, &rid, "b", a.fp(), &crypto::random(32), "box");
    fx.good(&a, "pair_put", &obj! { "rid" => &rid, "m" => wrong });
    let j = node.call("pair", obj! { "code" => spec::code_text(&code) });
    assert_eq!(cols(&j, &["ok", "error"]), list![false, "bad_peer"]);
    assert_eq!(fx.call(&a, "pair_get", &obj! { "rid" => &rid }), (404, obj! { "error" => "no_pairing" }));
    assert_eq!(node.good("list", obj! {})["rows"], list![], "这台设备上什么都没存");
    assert_eq!(remote(&fx.d)["guests"], obj! {});
}

fn view_only_unpair_and_no_session(t: &mut Ctx) {
    let (fx, _web, a, _b, mut node, base) = client_fx(t);
    node.good("start", obj! { "base" => &base, "name" => "iPad" });
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    let (j, _said) = pair_web(&mut node, &a);
    let pid = j["pid"].str().to_string();
    let rows = phone_rows(&mut node, "手机看到会话", |_| true);
    let sid = rows[0]["sessions"][0]["sid"].str().to_string();
    // 只让看：宿主那头把关，手机敲的键进不去（页面自己不发，发了也不转）
    a.ok(&["remote-allow", "iPad", "view"]);
    phone_rows(&mut node, "说明改成只看", |sessions| sessions[0]["access"] == Value::from("view"));
    node.good("attach", obj! { "pid" => &pid, "sid" => &sid, "cols" => 70, "rows" => 22 });
    assert!(truthy(&node.good("wait", obj! { "state" => "open" })["ok"]));
    node.good("input", obj! { "text" => "say must not arrive\r" });
    h.send("say typed on the box\r");
    assert!(truthy(&node.good("wait", obj! { "text" => "<typed on the box>" })["ok"]));
    h.expect("<typed on the box>");
    assert!(!has(h.out(), "must not arrive"));
    assert!(!has(h.out(), "<size 22 70>"), "只看的不算进窗口大小");
    // 那边不让接入了：正接着的被断开，再也接不上（过一阵说会话不在了）；没有这个会话的直接说
    a.ok(&["remote-allow", "iPad", "none"]);
    let j = node.good("wait", obj! { "state" => "retrying", "secs" => 30 });
    assert!(truthy(&j["ok"]), "{}", j["events"]);
    node.good("detach", obj! {});
    node.good("attach", obj! { "pid" => &pid, "sid" => "ab".repeat(16), "cols" => 80, "rows" => 24 });
    assert!(truthy(&node.good("wait", obj! { "state" => "closed", "why" => "gone" })["ok"]));
    // 手机上取消配对：服务器上的记录、访客都删掉；box 那边再看是“对方已经取消了”
    assert_eq!(node.good("unpair", obj! { "pid" => &pid })["told"], Value::Bool(true));
    assert_eq!(node.good("list", obj! {})["rows"], list![]);
    let doc = remote(&fx.d);
    assert_eq!((&doc["edges"], &doc["guests"]), (&obj! {}, &obj! {}));
    let checked = a.ok(&["remote-peers", "--check"]);
    assert!(checked.contains("对方已经取消了配对"), "{checked}");
}

// ---- bash 这一层：cca remote pair --web（真的 cca 脚本），终端里和界面 App 的提问会话里；手机那一头照文档自己说话（Guest） ----

fn terminal(t: &mut Ctx) {
    let (fx, web, a, _b) = web_fx(t);
    assert_eq!(parsed(&fx.cca(&a, &["_gui", "hello"]).out).get("remote_web"), Some(&Value::from(1)));
    let r = fx.cca(&a, &["remote", "pair", "--phone"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("用法：cca remote pair [--web]"), "{}", r.err);
    // 不同意就不出配对码；问之前说清楚：是手机、那一页是服务器给的
    let r = fx.cca_with(&a, &["remote", "pair", "--web"], Some(&["n"]), None);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("那台手机可以在浏览器里看和操作这台机器上开着远程会话的会话"), "{}", r.err);
    assert!(r.err.contains("管着服务器的人能换掉那一页的代码"), "{}", r.err);
    assert!(!r.err.contains("配对码："), "{}", r.err);
    assert_eq!(a.sent(None), Vec::<Value>::new(), "没同意：什么都没发给服务器");
    let mut p = Popen::spawn(&mut fx.cca_cmd(&a, &["remote", "pair", "--web"], &[("CCA_TTY", &fx.answers(&["y"]))]));
    let mut seen = String::new();
    let code = loop {
        let line = p.err_line();
        assert!(!line.is_empty(), "没等到配对码：{seen}");
        seen.push_str(&line);
        if line.contains("配对码：") {
            break line.trim().split('：').nth(1).expect("配对码：后面是配对码").to_string();
        }
    };
    assert!(seen.contains(&format!("手机上打开：{ORIGIN}/")), "先说地址，再说配对码：{seen}");
    assert!(seen.contains("添加到主屏幕"), "{seen}");
    let mut g = Guest::new(&fx.d, "我的 iPhone");
    g.join(&web, &spec::code_bytes(&code).expect("配对码的样子是对的"), Some(a.fp()));
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out.as_str()), (0, ""), "{said}");
    assert!(said.contains("已经和「我的 iPhone」配对：它可以接入这台机器上开着远程会话的会话"), "{said}");
    let left: Vec<PathBuf> = walk(&fx.d).into_iter().filter(|f| named_like(f, "cca-pair.")).collect();
    assert_eq!(left, Vec::<PathBuf>::new());
    let r = fx.cca(&a, &["remote", "peers"]);
    assert!(matches(r"^[0-9a-f]{8}  我的 iPhone（手机上的网页端）  对方可以看和操作这台机器的会话  ", &r.out), "{}", r.out);
    let r = fx.cca(&a, &["remote", "list", "--json"]);
    assert_eq!(
        rows_of(&parsed(&r.out)["peers"], &["name", "web", "incoming", "state"]),
        [list!["我的 iPhone", true, "control", "ok"]]
    );
    // 改成只看、取消配对照旧按名字；取消以后服务器上访客也没了
    let r = fx.cca(&a, &["remote", "allow", "我的 iPhone", "view"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let r = fx.cca(&a, &["remote", "unpair", "我的 iPhone"]);
    assert_eq!(r.code, 0, "{}", r.err);
    let doc = remote(&fx.d);
    assert_eq!((&doc["edges"], &doc["guests"]), (&obj! {}, &obj! {}));
}

fn in_the_app(t: &mut Ctx) {
    let (fx, web, a, _b) = web_fx(t);
    let no = obj! { "consent.remote_pair" => list![obj! { "v" => "n" }] };
    let (mut p, ev) = fx.app_session(&a, &no, &["remote", "pair", "--web"], &[]);
    assert_ne!(p.wait(secs(60)), 0);
    let es = relay::events(&ev);
    let asks: Vec<&Value> = es.iter().filter(|e| e["ev"].str() == "ask").collect();
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!((asks[0]["q"].str(), asks[0].get("scope").map_or("web", Value::str)), ("consent.remote_pair", "web"));
    // 显示的那一页：先是手机上打开的地址（App 画成二维码），再是配对码；都不用回答
    let yes = obj! { "consent.remote_pair" => list![obj! { "v" => "y" }], "remote.pair_web" => list![Value::Null] };
    let (mut p, ev) = fx.app_session(&a, &yes, &["remote", "pair", "--web"], &[]);
    let pages = wait_for("显示配对码的那一页", secs(30), || {
        let asked = Value::from("remote.pair_web");
        let pages: Vec<Value> = relay::events(&ev).into_iter().filter(|e| e.get("q") == Some(&asked)).collect();
        (!pages.is_empty()).then_some(pages)
    });
    assert_eq!(pages.len(), 1, "{pages:?}");
    let page = &pages[0];
    assert_eq!(rows_of(&page["fields"], &["id", "kind"]), [list!["url", "qr"], list!["code", "code"]]);
    assert_eq!((page["title"].str(), page["fields"][0]["value"].str()), ("和手机配对", format!("{ORIGIN}/").as_str()));
    let code = page["fields"][1]["value"].str();
    assert!(matches("^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", code), "{code:?}");
    Guest::new(&fx.d, "iPad").join(&web, &spec::code_bytes(code).expect("配对码的样子是对的"), Some(a.fp()));
    let rc = p.wait(secs(60));
    assert_eq!(rc, 0, "{}", p.err_rest());
    let es = relay::events(&ev);
    let results: Vec<&Value> = es.iter().filter(|e| e["ev"].str() == "result").collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!((results[0]["code"].str(), results[0]["message"].str().contains("「iPad」")), ("remote_paired", true));
}

/// 服务器没开网页端（或者是旧版）：cca remote pair --web 说清楚，不留下一个手机输了也没用的配对码
fn no_web(t: &mut Ctx) {
    let fx = Base::new(t);
    let a = fx.device("box");
    fx.machines(&[&a]);
    let r = fx.cca_with(&a, &["remote", "pair", "--web"], Some(&["y"]), None);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    let why = "「测试组」的服务器还没有开网页端，手机上接入不了（管理员要重新部署、把网页端打开：cca team deploy）";
    assert!(r.err.contains(why), "{}", r.err);
    assert!(!r.err.contains("配对码："), "{}", r.err);
    // 没开网页端的服务器照样配得了两台机器
    let r = a.run(&["remote-pair-show", "--web"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert_eq!(fx.call(&a, "pair_get", &obj! { "rid" => "0".repeat(32) }).0, 404);
}

// ---- 页面的文件（server/web/） ----
// 服务器读得进去、只发该发的；别人的代码（xterm.js）和说明里记的校验和一样；页面自己不引别处的东西、没有内联的脚本
// （内容安全策略不许，写了也不会跑）。原来这两条直接看 Python 服务器里读进去的文件表（srvmod.Web）；这里起一台真的服务器
// （--web-root 指着仓库里的 server/web/），从外面一个个文件去要

/// 仓库里页面的文件在哪
fn web_dir() -> PathBuf {
    harness::repo().join("server").join("web")
}

/// 服务器该发给浏览器的文件：正好这些
const NEED: [&str; 12] = [
    "index.html",
    "app.js",
    "app.css",
    "core.js",
    "xterm.js",
    "xterm.css",
    "xterm-addon-fit.js",
    "logo.svg",
    "manifest.webmanifest",
    "icon-180.png",
    "icon-192.png",
    "icon-512.png",
];

/// 一台发真的那一页的服务器（页面的地址和原来给 srvmod.Web 的一样：https://relay.example.com/）。
/// name：--web-name-file 指的文件里写什么（None：不带这个参数）。返回 (夹具, 网页端)
fn real_page_fx(t: &mut Ctx, name: Option<&str>) -> (Base, Web) {
    let fx = Base::with(t, true, |d| {
        let certs = d.join("web-tls");
        std::fs::create_dir(&certs).unwrap_or_else(|e| panic!("建 {}：{e}", certs.display()));
        pki::make_ca_cert(&certs);
        let mut args = vec![
            ("--web-url", "https://relay.example.com/".to_string()),
            ("--web-listen", "127.0.0.1:0".to_string()),
            ("--web-port-file", text(&d.join("wport"))),
            ("--web-cert", text(&certs.join("cert.pem"))),
            ("--web-key", text(&certs.join("key.pem"))),
            ("--web-root", text(&web_dir())),
        ];
        if let Some(name) = name {
            fsx::write(d.join("name"), name);
            args.push(("--web-name-file", text(&d.join("name"))));
        }
        args.into_iter().flat_map(|(flag, value)| [flag.to_string(), value]).collect()
    });
    let ca = fx.d.join("web-tls").join("ca.pem");
    let origin = Some("https://relay.example.com".to_string());
    let web = Web { wport: wport_in(&fx.d), ca, origin, held: RefCell::new(Vec::new()) };
    (fx, web)
}

/// 要一个该有的文件（不压缩）：不是 200 就 panic
fn page_file(web: &Web, name: &str) -> Got {
    let r = web.get(&format!("/{name}"), &[]);
    assert_eq!(r.status, 200, "{name}");
    r
}

fn the_real_page(t: &mut Ctx) {
    let dir = web_dir();
    let (_fx, web) = real_page_fx(t, None);
    // 原来比的是服务器读进去的文件表（set(web.files) == need）和表里压缩过的大小加起来；从外面看：该发的 12 个都要得到，
    // 目录里别的东西（README.md、xterm-LICENSE.txt……）一个都要不到；浏览器收得了压缩的时候，一整页收下来不到 400 KB
    let mut asked: Vec<String> = NEED.iter().map(|name| name.to_string()).collect();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("读不了 {}：{e}", dir.display())) {
        let name = entry.unwrap_or_else(|e| panic!("{e}")).file_name().to_string_lossy().into_owned();
        if !asked.contains(&name) {
            asked.push(name);
        }
    }
    assert!(asked.iter().any(|name| name == "README.md") && asked.iter().any(|name| name == "xterm-LICENSE.txt"), "{asked:?}");
    let (mut served, mut total) = (Vec::new(), 0);
    for name in &asked {
        let r = web.get(&format!("/{name}"), &[("Accept-Encoding", "gzip")]);
        if r.status == 200 {
            served.push(name.as_str());
            total += r.body.len();
        } else {
            assert_eq!(r.status, 404, "{name}");
        }
    }
    assert_eq!(served, NEED, "多了、少了文件：页面目录里新加的东西要想清楚该不该发给浏览器");
    assert!(total < 400 * 1024, "压缩以后整页不到 400 KB：{total}");
    let readme = fsx::read_text(dir.join("README.md"));
    let row = regex::Regex::new(r"(?m)^\| `([^`]+)` \|[^|]+\|[^|]+\| `([0-9a-f]{64})` \|$").expect("正则写得对");
    let table: BTreeMap<&str, &str> = row.captures_iter(&readme).map(|c| c.extract()).map(|(_, [name, sum])| (name, sum)).collect();
    assert_eq!(table.keys().copied().collect::<Vec<_>>(), ["xterm-LICENSE.txt", "xterm-addon-fit.js", "xterm.css", "xterm.js"]);
    for (name, want) in &table {
        assert_eq!(crypto::sha256_hex(&fsx::read(dir.join(name))), *want, "{name}：和说明里记的不一样（被改过？）");
    }
    let html = fsx::read_text(dir.join("index.html"));
    // 原来是 (?:src|href)="(?![a-z0-9][a-z0-9._-]*")[^"]*" 一个都找不到。regex 这个库没有“后面不是……”的写法：
    // 把每个 src、href 的值取出来，不是“同一个目录里的一个文件名”的就是引了别处
    let refs = regex::Regex::new(r#"(?:src|href)="([^"]*)""#).expect("正则写得对");
    let local = regex::Regex::new(r"^[a-z0-9][a-z0-9._-]*\z").expect("正则写得对");
    let elsewhere: Vec<&str> =
        refs.captures_iter(&html).map(|c| c.extract()).map(|(_, [value])| value).filter(|value| !local.is_match(value)).collect();
    assert_eq!(elsewhere, Vec::<&str>::new(), "只引同一个目录里的文件");
    // 原来是 <script(?![^>]*\bsrc=)|\son[a-z]+\s*=|javascript:|<style 找不到。头一种也是“后面不是……”：
    // 每个 <script 到它后面头一个 > 之间得有 src=（前面是词的边界：在整段文字里找，看得到 src 前面那个字）
    let src_attr = regex::Regex::new(r"\bsrc=").expect("正则写得对");
    let inline: Vec<usize> = html
        .match_indices("<script")
        .map(|(at, tag)| at + tag.len())
        .filter(|from| {
            let end = html[*from..].find('>').map_or(html.len(), |n| from + n);
            !src_attr.find_at(&html, *from).is_some_and(|m| m.end() <= end)
        })
        .collect();
    assert_eq!(inline, Vec::<usize>::new(), "没有内联的脚本、样式、事件");
    assert!(!matches(r"\son[a-z]+\s*=|javascript:|<style", &html), "没有内联的脚本、样式、事件");
    let urls = regex::Regex::new("https?://").expect("正则写得对");
    for name in ["app.js", "core.js"] {
        let src = fsx::read_text(dir.join(name));
        assert!(!matches(r"\beval\(|new Function|innerHTML|outerHTML|insertAdjacentHTML|document\.write", &src), "{name}");
        // 原来是 https?://(?!localhost) 找不到：每个 http:// 、https:// 后面都得是 localhost
        let away: Vec<&str> = urls
            .find_iter(&src)
            .filter(|m| !src[m.end()..].starts_with("localhost"))
            .map(|m| src[m.start()..].lines().next().unwrap_or_default())
            .collect();
        assert_eq!(away, Vec::<&str>::new(), "{name}：不连别处");
    }
    let m = parsed(&fsx::read_text(dir.join("manifest.webmanifest")));
    let icons: Vec<&str> = m["icons"].list().iter().map(|icon| icon["src"].str()).collect();
    assert_eq!((m["display"].str(), m["start_url"].str(), icons), ("standalone", "./", vec!["icon-192.png", "icon-512.png"]));
    for n in [180u32, 192, 512] {
        let png = fsx::read(dir.join(format!("icon-{n}.png")));
        let side = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().expect("四个字节"));
        assert_eq!((&png[..8], (side(16), side(20))), (&b"\x89PNG\r\n\x1a\n"[..], (n, n)));
    }
}

/// --web-name-file（安装脚本写的 app/name）：标题、手机主屏幕上的名字带上 Team 的名字（转义过）；别的文件不动；
/// 没给时照旧叫 cca。（文件不在、名字不对——空的、太长、带控制字符——时也照旧叫 cca：在服务器自己的单元测试里，
/// cli/src/server/web.rs 的 the_team_name）
fn the_team_name(t: &mut Ctx) {
    let (_plain_fx, plain) = real_page_fx(t, None);
    let (_team_fx, team) = real_page_fx(t, Some("甲组 <b>&\"x\"\n第二行不算\n"));
    let utf8 = |body: Vec<u8>| String::from_utf8(body).unwrap_or_else(|e| panic!("页面不是 UTF-8：{e}"));
    let (index, index0) = (page_file(&team, "index.html"), page_file(&plain, "index.html"));
    let (etag, etag0) = (index.header("etag").to_string(), index0.header("etag").to_string());
    let page = utf8(index.body);
    assert!(page.contains("<title>cca 远程会话（甲组 &lt;b&gt;&amp;\"x\"）</title>"), "{page}");
    let meta = "<meta name=\"apple-mobile-web-app-title\" content=\"cca 甲组 &lt;b&gt;&amp;&quot;x&quot;\">";
    assert!(page.contains(meta), "{page}");
    assert_eq!(
        page.replace("（甲组 &lt;b&gt;&amp;\"x\"）", "").replace("cca 甲组 &lt;b&gt;&amp;&quot;x&quot;", "cca"),
        utf8(index0.body),
        "别处一个字不差"
    );
    let (m, m0) = (page_file(&team, "manifest.webmanifest").json(), page_file(&plain, "manifest.webmanifest").json());
    assert_eq!((m["name"].str(), m["short_name"].str()), ("cca 远程会话（甲组 <b>&\"x\"）", "cca 甲组 <b>&\"x\""));
    let rest = |m: &Value| -> Vec<(String, Value)> {
        m.obj().iter().filter(|(k, _)| !["name", "short_name"].contains(k)).map(|(k, v)| (k.to_string(), v.clone())).collect()
    };
    assert_eq!(rest(&m), rest(&m0));
    assert_ne!(etag, etag0, "ETag 跟着内容变");
    for name in NEED {
        if !["index.html", "manifest.webmanifest"].contains(&name) {
            assert!(page_file(&team, name).body == page_file(&plain, name).body, "{name}");
        }
    }
}

// ---- 真的那一页在真的浏览器里 ----

const NO_BROWSER: &str = "没有 Node 22 以上或者没有 Chrome：页面这次没在浏览器里走";

/// Chrome 在哪（`CCA_CHROME` 换路径）
fn chrome() -> String {
    let given = std::env::var("CCA_CHROME").ok().filter(|path| !path.is_empty());
    given.unwrap_or_else(|| "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string())
}

/// 真的那一页在真的浏览器里（无界面的 Chrome，照 iPhone 的屏幕、触摸；tests/web_page.mjs）：对面是真的服务器、真的 cca 会话。
/// 网页端开在 http://localhost（浏览器把它当安全的来源，不用证书）。截图留在 CCA_WEB_SHOTS 指的目录里（没给就不留）
fn the_page_in_a_browser(t: &mut Ctx) {
    let chrome = chrome();
    let node = match node_bin() {
        Some(node) if Path::new(&chrome).exists() => node,
        _ => t.skip(NO_BROWSER),
    };
    // 先挑一个空着的端口：页面的地址（--web-url）里要写它
    let wport = fsx::free_port();
    let url = format!("http://localhost:{wport}/");
    let fx = Base::with(t, true, |_| {
        vec!["--web-url".to_string(), url.clone(), "--web-listen".to_string(), format!("127.0.0.1:{wport}")]
    });
    let b = fx.device("box");
    fx.machines(&[&b]);
    let _h = relay::session(&b, Device::KEY, "alice", &[]);
    let mut p = Popen::spawn(&mut relay::helper_cmd(&b, &["remote-pair-show", "--web"]));
    let (line, web) = (p.out_line(), p.out_line());
    let code = line.split_whitespace().nth(1).unwrap_or_else(|| panic!("头一行该是 code 配对码：{line:?}")).to_string();
    assert_eq!(web, format!("web {url}\n"));
    let kept = std::env::var_os("CCA_WEB_SHOTS").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let shots = kept.unwrap_or_else(|| t.tmp("cca-shots-"));
    // 环境里只有 PATH、Chrome 在哪、临时目录（不带代理的那些：只连本机）。临时目录指着用例自己的：Chrome 的资料目录建在
    // 里面，命令行里就带着这个用例的临时目录，没收场的找得到。HOME 不设：指到一个空目录的话 Chrome 打开页面那一步时快时慢
    // （实测 1 到 15 秒，有一次四分钟都没走完）；不设时它照系统里记的主目录来，资料目录还是上面那个临时的
    let scratch = t.tmp("cca-web-page-");
    let mut cmd = Cmd::new(&node);
    cmd.arg(harness::repo().join("tests").join("web_page.mjs")).args([url.as_str(), code.as_str()]).arg(&shots);
    cmd.pass_env(&["PATH"]).env("CCA_CHROME", &chrome).env("TMPDIR", &scratch);
    // 自己一个进程组：等不到时连同它起的 Chrome 一起停掉（只停 node 的话 Chrome 留在那里）
    let mut browser = Popen::spawn(cmd.session(Session::New));
    let end = Instant::now() + secs(240);
    while browser.poll().is_none() {
        if Instant::now() >= end {
            browser.stop();
            let (out, err) = (browser.out_rest(), browser.err_rest());
            panic!("浏览器那一头四分钟没走完：{} {}", tail(&out, 500), tail(&err, 1500));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (_rc, out, err) = browser.communicate(secs(10));
    let Ok(j) = json::parse(&out) else {
        panic!("浏览器那一头没给结果：{} {}", tail(&out, 500), tail(&err, 1500));
    };
    let shown = |key: &str| j.get(key).map_or("None".to_string(), Value::to_string);
    assert!(truthy(&j["ok"]), "{}\n页面上：{}\n控制台：{}", shown("error"), shown("dom"), shown("logs"));
    assert_eq!(
        j["steps"],
        list!["welcome", "paired", "typed", "clip", "keybar", "compose", "menu", "reattach", "ended", "settings"]
    );
    assert_eq!(j["logs"], list![], "页面的控制台里不该有报错（被内容安全策略拦下的也算）");
    assert_eq!(j["codeShown"].str(), code, "输入的配对码自动分成四个一组");
    assert!(j["home"].str().contains("alice"), "{}", j["home"]);
    assert!(j["ended"].str().contains("会话结束了（退出码 3）"), "{}", j["ended"]);
    assert!(j["sizeAfterFont"].i64() < j["size"][0].i64(), "字调大以后行数变少（终端跟着重新算了大小）：{j}");
    assert_eq!(p.wait(secs(30)), 0);
    assert_eq!(p.out_rest(), "peer iPhone\n");
}
