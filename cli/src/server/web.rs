//! 网页端（手机浏览器里的那一页，server/README.md「网页端」）：页面的文件、页面的地址、每个回复都带的那几个头、WebSocket 的名额，
//! 和怎么给出一个文件。Python 的 `web_origin`、`web_name`、`web_named`、`Web`（Py:1572-1667）和 `Handler.static`（Py:1832-1851）。
//!
//! 页面的文件编在程序里（`FILES`，就是 server/web/ 里的那些）；--web-root 给了就改从那个目录读。两种都照文件名的先后过一遍
//! （Python 是 `sorted(os.listdir(root))`）：给出去的只有名字合 `wire::web_name_re()`、后缀在 `WEB_TYPES` 里的普通文件（说明、
//! 许可这些不是页面的一部分），一个最大 `WEB_FILE`，一定要有 index.html。启动时全读进内存：原样的、gzip 压过的（.png 不压，
//! 压了没变小的不留）、ETag（`"` + SHA-256 的前 32 个十六进制字符 + `"`）。Team 的名字给了（--web-name-file）：index.html 的标题、
//! manifest 里的名字带上它（`web_named`）。
//!
//! 和 Python 不一样的只有 gzip 压出来的字节（deflate 是 miniz_oxide 做的，见 py/gzip.rs）：解开是同一份，ETag 对原样的内容算，
//! 也一样。压出来的长短会差一点：「压了没变小的不留」这条线上，小文件用 `gzip::compress_shortest`（也试固定编码，和 zlib 一样挑短的），
//! 对照数据里的文件两边留不留都一样；正好压到和原样差不多长的文件仍可能一边给原样的、一边给压过的。
//!
//! 请求来了不碰文件系统：路径只拿来在内存里的表里查名字，`..`、编码过的路径都走不出去。

use super::handler::{error, Outcome};
use super::http::Request;
use super::limits::Slots;
use super::omap::OMap;
use super::wire::{app_title_re, title_re, web_name_re, web_url_re};
use super::{WEB_FILE, WEB_TYPES, WS_CONN, WS_CONN_IP};
use crate::py::fs::open_error_text;
use crate::py::json::{self, Fmt};
use crate::py::obj::V;
use crate::py::re::{Match, Re};
use crate::py::{gzip, hash, text, uni, Fail, R};
use std::io::{self, Read};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// 网页端的文件：server/web/ 里的全部（服务端给出去的那几种，加上说明和 xterm.js 的许可）。一键部署的安装脚本也用这张表
/// （cmd/hub/deploy.rs：每个是一段 base64，写进 app/web/），先后不能动——动了安装脚本就变样。目录里新加了文件要加在这里
/// （deploy.rs 的测试照目录核对）
pub const FILES: [(&str, &[u8]); 14] = [
    ("app.css", include_bytes!("../../../server/web/app.css")),
    ("app.js", include_bytes!("../../../server/web/app.js")),
    ("core.js", include_bytes!("../../../server/web/core.js")),
    ("icon-180.png", include_bytes!("../../../server/web/icon-180.png")),
    ("icon-192.png", include_bytes!("../../../server/web/icon-192.png")),
    ("icon-512.png", include_bytes!("../../../server/web/icon-512.png")),
    ("index.html", include_bytes!("../../../server/web/index.html")),
    ("logo.svg", include_bytes!("../../../server/web/logo.svg")),
    ("manifest.webmanifest", include_bytes!("../../../server/web/manifest.webmanifest")),
    ("xterm-addon-fit.js", include_bytes!("../../../server/web/xterm-addon-fit.js")),
    ("xterm-LICENSE.txt", include_bytes!("../../../server/web/xterm-LICENSE.txt")),
    ("xterm.css", include_bytes!("../../../server/web/xterm.css")),
    ("xterm.js", include_bytes!("../../../server/web/xterm.js")),
    ("README.md", include_bytes!("../../../server/web/README.md")),
];

/// 给出去的一个文件
pub struct WebFile {
    /// Content-Type（`WEB_TYPES` 里照后缀查的）
    pub ctype: &'static str,
    /// 原样的内容（Team 的名字给了的话是换过标题的）
    pub raw: Vec<u8>,
    /// gzip 压过的；.png、压了没变小的是 None
    pub gz: Option<Vec<u8>>,
    /// 带着引号的 ETag
    pub etag: String,
}

pub struct Web {
    /// 页面的地址，结尾带 `/`（/v1/health 里的 web、票据里回给设备的）
    pub url: String,
    /// 浏览器自己说的那个样子（WebSocket 的 Origin 拿它比）：url 去掉结尾的 `/`
    pub origin: String,
    /// 每个文件的回复都带的头（Content-Security-Policy 这些，Py:1628-1633），照先后
    pub headers: Vec<(&'static str, String)>,
    /// 文件名 → 文件，照文件名的先后
    pub files: OMap<WebFile>,
    /// 同时开着的 WebSocket：一共 `WS_CONN` 条、同一个 IP `WS_CONN_IP` 条
    slots: Mutex<Slots>,
}

/// Python 的文本文件一块一块地解码，一块 8192 个字节（`TextIOWrapper._CHUNK_SIZE`）：`web_name` 照它读头一块
const NAME_CHUNK: usize = 8192;
/// `f.readline(1024)`：第一行最多要这么多个字符
const NAME_LINE: usize = 1024;
/// Team 的名字最多几个字符（cca 里 Team 的名字也是这样）
const NAME_MAX: usize = 64;

impl Web {
    /// url 是 `web_origin` 写好的地址加 `/`；root 是 --web-root（None、空的：用编在程序里的那一份）；team 是 Team 的名字
    /// （`web_name` 给的，空的当没给）。文件读不了、太大、没有 index.html 是 Err，`str(e)` 和 Python 的一样：启动时说
    /// 「网页端的文件读不了：…」（Py:2229-2233）。读不了是 `Fail::Os`（带着文件名），别的是 `Fail::Value`
    pub fn open(url: &str, root: Option<&Path>, team: Option<&str>) -> R<Web> {
        let origin = url.trim_end_matches('/').to_string();
        let url = format!("{origin}/");
        let host = origin.split_once("://").map_or("", |(_, host)| host);
        // 页面只连自己这一个地址：脚本、样式、图片、请求、WebSocket 都只认同一个源（xterm.js 画界面要动态加样式，所以样式放行内联的）
        let csp = format!(
            "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; \
             manifest-src 'self'; connect-src 'self' {}://{host}; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            if origin.starts_with("https:") { "wss" } else { "ws" }
        );
        let headers = vec![
            ("Content-Security-Policy", csp),
            ("X-Content-Type-Options", "nosniff".to_string()),
            ("X-Frame-Options", "DENY".to_string()),
            ("Referrer-Policy", "no-referrer".to_string()),
            ("Cross-Origin-Opener-Policy", "same-origin".to_string()),
            ("Strict-Transport-Security", "max-age=31536000".to_string()),
        ];
        let team = team.filter(|t| !t.is_empty());
        let mut files = OMap::new();
        let place = match root.filter(|r| !r.as_os_str().is_empty()) {
            None => {
                let mut embedded: Vec<&(&str, &[u8])> = FILES.iter().collect();
                embedded.sort_by_key(|(name, _)| *name);
                for (name, raw) in embedded {
                    let Some(ctype) = served_type(name) else { continue };
                    if raw.len() > WEB_FILE {
                        return Err(Fail::Value(format!("{name} 太大")));
                    }
                    files.insert(*name, web_file(name, ctype, raw.to_vec(), team)?);
                }
                "编在程序里的页面".to_string()
            }
            Some(root) => {
                let place = root.to_string_lossy().into_owned();
                for name in listdir(root, &place)? {
                    let Some(ctype) = served_type(&name) else { continue };
                    let path = root.join(&name);
                    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                        continue; // 目录、断了的链接：不给（os.path.isfile 跟着链接看）
                    }
                    let raw = read_capped(&path)?;
                    let file = web_file(&name, ctype, raw, team)?;
                    files.insert(name, file);
                }
                place
            }
        };
        if !files.contains("index.html") {
            return Err(Fail::Value(format!("{place} 里没有 index.html")));
        }
        Ok(Web { url, origin, headers, files, slots: Web::new_slots() })
    }

    /// GET 的路径（请求行里的地址 `?` 前面那段）对应哪个文件（Py:1827）：`/` 是 index.html，别的去掉第一个字符照名字查。
    /// Python 去掉的就是第一个字符、不管它是不是 `/`（`GET xindex.html` 也给 index.html），照它。没有是 None：回 404 not_found。
    ///
    /// Python 的请求行是按 latin-1 解的，去掉的是一个字节。第一个字符不是 ASCII 的话，Python 剩下的开头是半个字符（或者别的
    /// 不是 ASCII 的字符），对不上任何文件名（都是 ASCII 的）：这里直接 None。这样请求行按 UTF-8 还是 latin-1 解都和 Python 一样
    pub fn file(&self, path: &str) -> Option<&WebFile> {
        if path == "/" {
            return self.files.get("index.html");
        }
        let mut rest = path.chars();
        match rest.next() {
            Some(first) if first.is_ascii() => self.files.get(rest.as_str()),
            _ => None,
        }
    }

    /// 给出一个文件（Python 的 `Handler.static`，Py:1832-1851）。bucket：按 IP 限流扣一下（`ip_bucket.take(client_ip)`），
    /// 扣不到回 429 rate。每次都让浏览器问一下变了没有（no-cache + ETag）：页面更新了手机上马上就是新的。
    /// headers 照 Python 的先后，写在 Server、Date 后面；带着 Content-Length（304 不带它和 Content-Type，也没有正文）
    pub fn reply(&self, f: &WebFile, req: &Request, bucket: impl FnOnce() -> bool) -> Outcome {
        if !bucket() {
            return error(429, "rate");
        }
        let same = req.header("If-None-Match") == Some(f.etag.as_str());
        // `'gzip' in Accept-Encoding`：只看有没有这几个字母（大小写要一样，`gzip;q=0`、`x-gzip` 也算），和 Python 一样
        let gz = f.gz.as_ref().filter(|_| !same && req.header("Accept-Encoding").unwrap_or("").contains("gzip"));
        let body = match (same, gz) {
            (true, _) => Vec::new(),
            (false, Some(gz)) => gz.clone(),
            (false, None) => f.raw.clone(),
        };
        let mut headers: Vec<(String, String)> = Vec::with_capacity(6 + self.headers.len());
        if !same {
            headers.push(("Content-Type".into(), f.ctype.into()));
            headers.push(("Content-Length".into(), body.len().to_string()));
            if gz.is_some() {
                headers.push(("Content-Encoding".into(), "gzip".into()));
            }
        }
        headers.push(("ETag".into(), f.etag.clone()));
        headers.push(("Cache-Control".into(), "no-cache".into()));
        headers.push(("Vary".into(), "Accept-Encoding".into()));
        headers.extend(self.headers.iter().map(|(k, v)| ((*k).to_string(), v.clone())));
        Outcome::Raw { code: if same { 304 } else { 200 }, headers, body }
    }

    /// 占一条 WebSocket 的名额；满了是 false（回 429 busy）（Py:1652-1658）
    pub fn take(&self, ip: &str) -> bool {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).take(ip, false)
    }

    /// 还一条（Py:1660-1667）
    pub fn free(&self, ip: &str) {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).free(ip);
    }

    /// 现在开着几条（Python 的 `web.conns`；测试看名额还了没有）
    #[cfg(test)]
    pub fn conns(&self) -> usize {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).used()
    }

    /// WebSocket 的名额：刚开始都空着
    pub(super) fn new_slots() -> Mutex<Slots> {
        Mutex::new(Slots::new(WS_CONN, WS_CONN_IP))
    }
}

/// 这个名字给不给、给的话 Content-Type 是什么（Py:1636-1639）：名字合 `web_name_re`，后缀（`os.path.splitext` 的那段）在
/// `WEB_TYPES` 里。合规矩的名字第一个字符不是 `.`，所以后缀就是从最后一个 `.` 起的那段（`x.tar.js` 是 `.js`）
fn served_type(name: &str) -> Option<&'static str> {
    if !web_name_re().matches(name) {
        return None;
    }
    let ext = name.rfind('.').and_then(|i| name.get(i..))?;
    WEB_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, ctype)| *ctype)
}

/// 读进来的一个文件变成给出去的样子（Py:1644-1647）
fn web_file(name: &str, ctype: &'static str, raw: Vec<u8>, team: Option<&str>) -> R<WebFile> {
    let raw = match team {
        Some(team) => web_named(name, &raw, team)?,
        None => raw,
    };
    // 小文件也要和 Python 挑得一样（压了比原样短才给），所以用另外试固定编码的那个（见 gzip::compress_shortest）
    let gz = if name.ends_with(".png") { None } else { Some(gzip::compress_shortest(&raw, 9)) };
    let gz = gz.filter(|gz| gz.len() < raw.len());
    let etag = format!("\"{}\"", hash::sha256_hex(&raw).get(..32).unwrap_or(""));
    Ok(WebFile { ctype, raw, gz, etag })
}

/// `sorted(os.listdir(root))`。名字不是 UTF-8 的直接跳过：合不上 `web_name_re`，Python 那边也是跳过
fn listdir(root: &Path, place: &str) -> R<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| os_fail(&e, place))? {
        let entry = entry.map_err(|e| os_fail(&e, place))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// `open(path, 'rb').read(WEB_FILE + 1)`，超过上限是「… 太大」（Py:1640-1643）
fn read_capped(path: &Path) -> R<Vec<u8>> {
    let place = path.to_string_lossy();
    let f = std::fs::File::open(path).map_err(|e| os_fail(&e, &place))?;
    let mut raw = Vec::new();
    f.take(WEB_FILE as u64 + 1).read_to_end(&mut raw)?;
    if raw.len() > WEB_FILE {
        return Err(Fail::Value(format!("{place} 太大")));
    }
    Ok(raw)
}

/// 带着文件名的 OSError：`str(e)` 是 `[Errno 2] No such file or directory: '/路径'`，还是 OSError 一类（`Fail::is_os`）
fn os_fail(e: &io::Error, path: &str) -> Fail {
    Fail::Os(io::Error::new(e.kind(), open_error_text(e, path)))
}

/// --web-url 写成浏览器自己说的那个样子（WebSocket 的 Origin 就是这么写的，要拿它比）：主机小写，默认的端口（https 的 443、
/// http 的 80）不写。样子不对、端口不在 1～65535 是 None（Py:1572-1584）。和 Python 一样先按原样比默认端口、再换成数：
/// `:0443` 不算默认的，写成 `:443`；端口里各种文字的数字（`\d` 认的）都按十进制数认
pub fn web_origin(url: &str) -> Option<String> {
    let m = web_url_re().match_(url)?;
    let (scheme, rest) = m.group(1)?.split_once("://")?;
    let (host, port) = rest.split_once(':').unwrap_or((rest, ""));
    if !port.is_empty() && !(1..=65535).contains(&text::to_int(port)?) {
        return None;
    }
    let port = if port == (if scheme == "https" { "443" } else { "80" }) { "" } else { port };
    let tail = if port.is_empty() { String::new() } else { format!(":{}", text::to_int(port)?) };
    Some(format!("{scheme}://{}{tail}", host.to_ascii_lowercase()))
}

/// 启动参数里和网页端有关的那几条检查（Py:2187-2203），先后和 Python 一样：cli.rs 在 --cert、--key 那两条之后、--team-root
/// 那条之前调。参数是 --web-url、--relay-listen、--web-listen、--web-cert、--web-key 的值（空的和没给一样：Python 看的是真假）
/// 和有没有 --behind-proxy。都对是 (写好的 --web-url：`web_origin` 的样子加 `/`，没给是 None；是不是本机试：http:// 的)；
/// 不对是 `ap.error` 的那句话（cli.rs 照 argparse 的样子打出来，退出码 2）
pub fn web_args(
    url: Option<&str>,
    relay_listen: Option<&str>,
    listen: Option<&str>,
    cert: Option<&str>,
    key: Option<&str>,
    behind_proxy: bool,
) -> Result<(Option<String>, bool), &'static str> {
    fn given(v: Option<&str>) -> Option<&str> {
        v.filter(|s| !s.is_empty())
    }
    let url = match given(url) {
        Some(u) => Some(format!(
            "{}/",
            web_origin(u)
                .ok_or("--web-url 要写成 https://域名[:端口]/（手机上打开的就是它；本机试的时候可以是 http://localhost:端口/）")?
        )),
        None => None,
    };
    if url.is_some() && given(relay_listen).is_none() {
        return Err("网页端是经中转接入会话的：--web-url 要和 --relay-listen 一起给");
    }
    // 本机试：http://localhost（浏览器把它当成安全的来源）
    let local = url.as_deref().is_some_and(|u| u.starts_with("http://"));
    let (listen, cert, key) = (given(listen), given(cert), given(key));
    if listen.is_some() || cert.is_some() || key.is_some() {
        if behind_proxy {
            return Err("--behind-proxy 时 HTTPS 由平台做：网页端就在同一个端口上，不用 --web-listen");
        }
        let (Some(_), Some(listen)) = (&url, listen) else { return Err("--web-listen 要和 --web-url 一起给") };
        if local {
            // `a.web_listen.rpartition(':')[0].strip('[]')`：没有冒号是空的
            let host = listen.rsplit_once(':').map_or("", |(host, _)| host).trim_matches(['[', ']']);
            if cert.is_some() || key.is_some() || !["127.0.0.1", "localhost", "::1"].contains(&host) {
                return Err("--web-url 是 http://localhost 时（本机试）：--web-listen 只能听本机的地址，不用证书");
            }
        } else if cert.is_none() || key.is_none() {
            return Err("--web-listen 要带浏览器信得过的证书：--web-cert、--web-key");
        }
    }
    Ok((url, local))
}

/// --web-name-file 里的 Team 的名字（安装脚本写的 app/name）：第一行去掉前后的空白，1～64 个字符、没有控制字符（cca 里 Team 的
/// 名字也是这样）。没给、读不了、不对是 None（页面照旧叫 cca）（Py:1587-1599）。
///
/// 照 Python 的 `open(path, encoding='utf-8').readline(1024)` 读：换行是 `\n`、`\r`、`\r\n`；开头的 BOM 不去掉（会留在名字里）；
/// 文件是一块一块解码的，头一块（8192 个字节）里哪里不是 UTF-8 都算读不了，哪怕在第一行后面，只有块末尾半个字符等下一块。
/// 第一行最多 1024 个字符，头一块一定装得下，用不着读第二块
pub fn web_name(path: Option<&Path>) -> Option<String> {
    let path = path.filter(|p| !p.as_os_str().is_empty())?;
    let mut chunk = Vec::with_capacity(NAME_CHUNK);
    std::fs::File::open(path).ok()?.take(NAME_CHUNK as u64).read_to_end(&mut chunk).ok()?;
    let (head, cut) = match std::str::from_utf8(&chunk) {
        Ok(head) => (head, false),
        Err(e) if e.error_len().is_none() => (std::str::from_utf8(chunk.get(..e.valid_up_to())?).ok()?, true),
        Err(_) => return None,
    };
    let mut line = String::new();
    let mut ended = false;
    for (n, c) in head.chars().enumerate() {
        if c == '\n' || c == '\r' || n == NAME_LINE {
            ended = true;
            break;
        }
        line.push(c);
    }
    if cut && !ended {
        return None; // 文件在半个字符处完了：最后那次解码报错
    }
    let name = line.trim_matches(uni::is_space);
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

/// 网页的标题、手机主屏幕上的名字带上 Team 的名字：index.html 的 `<title>` 和 apple-mobile-web-app-title（iPhone 加到主屏幕时
/// 用它），manifest 的 name、short_name（安卓用它）；别的文件原样（Py:1602-1616）。几个 Team 的页面加到同一台手机的主屏幕上分得清。
///
/// 不是 UTF-8、manifest 不是 JSON 是 `Fail::Value`（UnicodeDecodeError、JSONDecodeError 的那句话）。manifest 是 JSON 但不是对象时
/// Python 抛的是 TypeError，main 没接住、带着调用栈退出（退出码 1）；这里也给 `Fail::Value`（同样那句话），启动时当作文件读不了
pub fn web_named(fname: &str, raw: &[u8], name: &str) -> R<Vec<u8>> {
    let (title, short) = (format!("cca 远程会话（{name}）"), format!("cca {name}"));
    match fname {
        "index.html" => {
            let page = utf8(raw)?;
            let page = sub_once(title_re(), page, |_| format!("<title>{}</title>", escape_html(&title, false)));
            let page = sub_once(app_title_re(), &page, |m| format!("{}{}{}", m.g(1), escape_html(&short, true), m.g(2)));
            Ok(page.into_bytes())
        }
        "manifest.webmanifest" => {
            let doc = json::loads(utf8(raw)?)?;
            let V::Dict(d) = &doc else { return Err(Fail::Value(not_a_dict(&doc))) };
            d.set("name", V::str(&title));
            d.set("short_name", V::str(&short));
            Ok(format!("{}\n", json::dumps(&doc, Fmt::new().utf8().indent(2))).into_bytes())
        }
        _ => Ok(raw.to_vec()),
    }
}

/// `raw.decode('utf-8')`：不是 UTF-8 的是 ValueError（UnicodeDecodeError 的那句话）
fn utf8(raw: &[u8]) -> R<&str> {
    std::str::from_utf8(raw).map_err(|e| Fail::Value(json::utf8_error_text(raw, &e)))
}

/// `re.sub(rx, f, text, count=1)`：只换第一处
fn sub_once(rx: &Re, text: &str, f: impl FnOnce(&Match) -> String) -> String {
    match rx.search(text) {
        Some(m) => {
            let (start, end) = (m.start(0), m.end(0));
            format!("{}{}{}", text.get(..start).unwrap_or(""), f(&m), text.get(end..).unwrap_or(""))
        }
        None => text.to_string(),
    }
}

/// `html.escape(s, quote)`
fn escape_html(s: &str, quote: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote => out.push_str("&quot;"),
            '\'' if quote => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// manifest 不是 JSON 对象时 `doc['name'] = …` 抛的 TypeError 的那句话
fn not_a_dict(doc: &V) -> String {
    let kind = match doc {
        V::List(_) => return "list indices must be integers or slices, not str".to_string(),
        V::None => "NoneType",
        V::Bool(_) => "bool",
        V::Int(_) | V::Big(_) => "int",
        V::Float(_) => "float",
        V::Str(_) => "str",
        V::Dict(_) => "dict",
    };
    format!("'{kind}' object does not support item assignment")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::base64::b64decode;
    use crate::py::fs::TempDir;
    use crate::py::obj::{Dict, List};
    use crate::re;
    use crate::server::http::Method;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const URL: &str = "https://relay.example.com/";
    /// 真的 Python 服务器给 https://relay.example.com/ 回的 Content-Security-Policy（照抄，一个字符都不能差）
    const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; manifest-src 'self'; connect-src 'self' wss://relay.example.com; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
    /// 每个文件的回复最后都带的那几个头（Python 的 web.headers 去掉 CSP 以后的）
    const TAIL: [(&str, &str); 5] = [
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("Cross-Origin-Opener-Policy", "same-origin"),
        ("Strict-Transport-Security", "max-age=31536000"),
    ];

    /// 编在程序里的页面：调试版里 gzip 压一遍要一会儿，几个用例共用一份
    fn plain() -> &'static Web {
        static WEB: OnceLock<Web> = OnceLock::new();
        WEB.get_or_init(|| Web::open(URL, None, None).unwrap())
    }

    fn embedded(name: &str) -> &'static [u8] {
        FILES.iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn req(headers: &[(&str, &str)]) -> Request {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Request { method: Method::Get, target: "/".into(), headers, body: Vec::new() }
    }

    fn raw_reply(o: Outcome) -> (u16, Vec<(String, String)>, Vec<u8>) {
        match o {
            Outcome::Raw { code, headers, body } => (code, headers, body),
            _ => panic!("该是文件的回复"),
        }
    }

    fn json_reply(o: Outcome) -> (u16, String) {
        match o {
            Outcome::Json(code, v) => (code, json::dumps(&v, Fmt::new().utf8().compact())),
            _ => panic!("该是 JSON 的回复"),
        }
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// 对照数据（cli/tests/vectors/server_web.json）：真的 server/cca-presence.py 给的结果。生成它的是一份不进仓库的脚本：
    /// 进程里直接调 web_origin、web_name、web_named、Web，HTTP 的那些起一个真的服务器进程（--behind-proxy、--web-root）去请求
    fn vectors() -> Dict {
        json::loads(include_str!("../../tests/vectors/server_web.json")).unwrap().as_dict().unwrap()
    }

    fn b64(s: &str) -> Vec<u8> {
        b64decode(s, false).unwrap()
    }

    /// 对照数据里的一段字节：{"b64"}、{"text"}、{"fill": [字节, 个数]}、{"repeat": [b64, 次数]}
    fn payload(p: &Dict) -> Vec<u8> {
        if let Some(s) = p.str("b64") {
            return b64(&s);
        }
        if let Some(s) = p.str("text") {
            return s.as_bytes().to_vec();
        }
        if let Some(l) = p.list("fill") {
            return vec![l.at(0).as_int().unwrap() as u8; l.at(1).as_int().unwrap() as usize];
        }
        let l = p.list("repeat").unwrap();
        b64(l.at(0).as_str().unwrap()).repeat(l.at(1).as_int().unwrap() as usize)
    }

    /// 几段拼起来（web_name 的输入：长的一串同样的字节写成 fill）
    fn parts(l: &List) -> Vec<u8> {
        l.items().iter().flat_map(|p| payload(&p.as_dict().unwrap())).collect()
    }

    /// 和对照数据里记正文的写法一样：256 个字节以内是 {"b64"}，长的是 {"len", "sha256"}
    fn out_data(d: &[u8]) -> String {
        match d.len() {
            0..=256 => format!("{{\"b64\": \"{}\"}}", crate::py::base64::b64encode(d)),
            n => format!("{{\"len\": {n}, \"sha256\": \"{}\"}}", hash::sha256_hex(d)),
        }
    }

    fn recorded(v: &V) -> String {
        json::dumps(v, Fmt::new().sort_keys())
    }

    /// 照对照数据里的样子建一棵目录：{"b64"|"text"|"fill"|"repeat"}（文件）、{"link": 指向}、{"dir": {…}}，可以带 "mode"（八进制）
    fn build(tree: &Dict, root: &Path) {
        for (name, spec) in tree.items() {
            let spec = spec.as_dict().unwrap();
            let path = root.join(&*name);
            if let Some(sub) = spec.dict("dir") {
                std::fs::create_dir(&path).unwrap();
                build(&sub, &path);
            } else if let Some(to) = spec.str("link") {
                std::os::unix::fs::symlink(&*to, &path).unwrap();
            } else {
                std::fs::write(&path, payload(&spec)).unwrap();
            }
        }
        for (name, spec) in tree.items() {
            if let Some(mode) = spec.as_dict().unwrap().str("mode") {
                chmod(&root.join(&*name), u32::from_str_radix(&mode, 8).unwrap());
            }
        }
    }

    /// 测完把改过权限的改回来（不然临时目录删不掉）
    fn unbuild(tree: &Dict, root: &Path) {
        for (name, spec) in tree.items() {
            let spec = spec.as_dict().unwrap();
            if spec.has("mode") {
                chmod(&root.join(&*name), if spec.has("dir") { 0o700 } else { 0o600 });
            }
        }
    }

    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// root 读得了一切：权限的那几个用例看不出来，跳过
    fn as_root() -> bool {
        // SAFETY: geteuid 没有参数、不会失败
        unsafe { libc::geteuid() == 0 }
    }

    /// tests/web_relay.py 的 UrlTest：--web-url 要和浏览器发的 Origin 逐字相等才让开 WebSocket：主机小写、默认的端口不写
    #[test]
    fn the_web_url_is_written_the_way_browsers_say_it() {
        for (given, want) in [
            ("https://relay.example.com/", "https://relay.example.com"),
            ("https://Relay.Example.COM", "https://relay.example.com"),
            ("https://relay.example.com:443/", "https://relay.example.com"),
            ("https://relay.example.com:8443", "https://relay.example.com:8443"),
            ("https://relay.example.com:08443/", "https://relay.example.com:8443"),
            ("https://192.0.2.10:443/", "https://192.0.2.10"),
            ("http://localhost:80/", "http://localhost"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
        ] {
            assert_eq!(web_origin(given).as_deref(), Some(want), "{given}");
        }
        for bad in [
            "",
            "relay.example.com",
            "http://relay.example.com/",
            "https://relay.example.com/x",
            "https://relay.example.com:0/",
            "https://relay.example.com:99999/",
            "https://a b/",
            "https://relay.example.com/?x",
            "http://localhost.evil.example/",
        ] {
            assert_eq!(web_origin(bad), None, "{bad}");
        }
    }

    /// FilesTest.test_the_real_page：服务器读得进去、只发该发的；别人的代码（xterm.js）和说明里记的校验和一样；
    /// 页面自己不引别处的东西、没有内联的脚本（内容安全策略不许，写了也不会跑）
    #[test]
    fn the_real_page() {
        let web = plain();
        let mut names: Vec<&str> = web.files.keys().collect();
        names.sort_unstable();
        let mut need = vec![
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
        need.sort_unstable();
        assert_eq!(names, need, "多了、少了文件：页面目录里新加的东西要想清楚该不该发给浏览器");
        let total: usize = web.files.values().map(|f| f.gz.as_ref().unwrap_or(&f.raw).len()).sum();
        assert!(total < 400 * 1024, "压缩以后整页不到 400 KB：{total}");
        let row = re!(r"^\| `([^`]+)` \|[^|]+\|[^|]+\| `([0-9a-f]{64})` \|$");
        let readme = std::str::from_utf8(embedded("README.md")).unwrap();
        let table: Vec<(String, String)> =
            readme.lines().filter_map(|l| row.match_(l).map(|m| (m.g(1).to_string(), m.g(2).to_string()))).collect();
        let mut listed: Vec<&str> = table.iter().map(|(n, _)| n.as_str()).collect();
        listed.sort_unstable();
        assert_eq!(listed, ["xterm-LICENSE.txt", "xterm-addon-fit.js", "xterm.css", "xterm.js"]);
        for (name, want) in &table {
            assert_eq!(&hash::sha256_hex(embedded(name)), want, "{name}：和说明里记的不一样（被改过？）");
        }
        let html = std::str::from_utf8(embedded("index.html")).unwrap();
        for m in re!(r#"(?:src|href)="([^"]*)""#).finditer(html) {
            assert!(re!(r"[a-z0-9][a-z0-9._-]*").fullmatch(m.g(1)).is_some(), "只引同一个目录里的文件：{}", m.g(0));
        }
        for m in re!(r"<script").finditer(html) {
            let tag = html[m.end(0)..].split('>').next().unwrap();
            assert!(re!(r"\bsrc=").found(tag), "没有内联的脚本：{tag}");
        }
        assert!(!re!(r"\son[a-z]+\s*=|javascript:|<style").found(html), "没有内联的样式、事件");
        for name in ["app.js", "core.js"] {
            let src = std::str::from_utf8(embedded(name)).unwrap();
            assert!(!re!(r"\beval\(|new Function|innerHTML|outerHTML|insertAdjacentHTML|document\.write").found(src), "{name}");
            for m in re!(r"https?://").finditer(src) {
                assert!(src[m.end(0)..].starts_with("localhost"), "{name}：不连别处");
            }
        }
        let m = json::loads(std::str::from_utf8(embedded("manifest.webmanifest")).unwrap()).unwrap().as_dict().unwrap();
        let icons: Vec<String> =
            m.list("icons").unwrap().items().iter().map(|i| i.as_dict().unwrap().str("src").unwrap().to_string()).collect();
        assert_eq!((m.str("display").as_deref(), m.str("start_url").as_deref()), (Some("standalone"), Some("./")));
        assert_eq!(icons, ["icon-192.png", "icon-512.png"]);
        for n in [180u32, 192, 512] {
            let png = embedded(&format!("icon-{n}.png"));
            let size = (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()));
            assert_eq!((&png[..8], size), (&b"\x89PNG\r\n\x1a\n"[..], (n, n)));
        }
    }

    /// FilesTest.test_the_team_name：--web-name-file（安装脚本写的 app/name）：标题、手机主屏幕上的名字带上 Team 的名字（转义过）；
    /// 别的文件不动；没给、文件不在、名字不对（空的、太长、带控制字符）时照旧叫 cca
    #[test]
    fn the_team_name() {
        let dir = TempDir::new().unwrap();
        let f = Path::new(&dir.path).join("name");
        std::fs::write(&f, "甲组 <b>&\"x\"\n第二行不算\n").unwrap();
        let team = Web::open(URL, None, web_name(Some(&f)).as_deref()).unwrap();
        for bad in ["", " \n", &"x".repeat(65), "a\x07b"] {
            std::fs::write(&f, bad).unwrap();
            assert_eq!(web_name(Some(&f)), None, "{bad:?}");
        }
        assert_eq!(web_name(Some(&Path::new(&dir.path).join("nope"))), None);
        assert_eq!(web_name(None), None);
        let page = std::str::from_utf8(&team.files.get("index.html").unwrap().raw).unwrap();
        assert!(page.contains("<title>cca 远程会话（甲组 &lt;b&gt;&amp;\"x\"）</title>"), "{page}");
        assert!(page.contains("<meta name=\"apple-mobile-web-app-title\" content=\"cca 甲组 &lt;b&gt;&amp;&quot;x&quot;\">"));
        let plain_page = std::str::from_utf8(&plain().files.get("index.html").unwrap().raw).unwrap();
        let back = page.replace("（甲组 &lt;b&gt;&amp;\"x\"）", "").replace("cca 甲组 &lt;b&gt;&amp;&quot;x&quot;", "cca");
        assert_eq!(back, plain_page, "别处一个字不差");
        let manifest = |w: &Web| {
            json::loads(std::str::from_utf8(&w.files.get("manifest.webmanifest").unwrap().raw).unwrap()).unwrap().as_dict().unwrap()
        };
        let (m, m0) = (manifest(&team), manifest(plain()));
        assert_eq!(
            (m.str("name").as_deref(), m.str("short_name").as_deref()),
            (Some("cca 远程会话（甲组 <b>&\"x\"）"), Some("cca 甲组 <b>&\"x\""))
        );
        for d in [&m, &m0] {
            d.pop("name");
            d.pop("short_name");
        }
        assert!(V::Dict(m).py_eq(&V::Dict(m0)), "manifest 别的项不动");
        assert_ne!(team.files.get("index.html").unwrap().etag, plain().files.get("index.html").unwrap().etag, "ETag 跟着内容变");
        for (name, f) in plain().files.iter().filter(|(n, _)| !["index.html", "manifest.webmanifest"].contains(n)) {
            assert_eq!(team.files.get(name).unwrap().raw, f.raw, "{name}");
        }
    }

    /// 编进来的文件里，给得出去的那几个（名字合规矩、后缀认得）都不超过上限，有 index.html；说明和许可不给
    #[test]
    fn embedded_files_fit_the_serving_rules() {
        let served: Vec<&str> = FILES
            .iter()
            .filter(|(name, _)| web_name_re().matches(name) && WEB_TYPES.iter().any(|(ext, _)| name.ends_with(ext)))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(served.len(), 12, "{served:?}");
        assert!(served.contains(&"index.html") && !served.contains(&"README.md") && !served.contains(&"xterm-LICENSE.txt"));
        assert!(FILES.iter().all(|(_, raw)| !raw.is_empty() && raw.len() <= WEB_FILE));
        let mut names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FILES.len(), "没有重名的");
    }

    /// 每个文件：照后缀的类型、原样的和 gzip 的（.png 不压）、ETag 对上了回 304；头的先后和写法照 Python
    #[test]
    fn every_file_is_served_with_its_type() {
        let web = plain();
        assert_eq!(web.headers.first().map(|(k, v)| (*k, v.as_str())), Some(("Content-Security-Policy", CSP)));
        assert_eq!(web.headers[1..].iter().map(|(k, v)| (*k, v.as_str())).collect::<Vec<_>>(), TAIL);
        for (name, f) in web.files.iter() {
            let ctype = WEB_TYPES.iter().find(|(ext, _)| name.ends_with(ext)).unwrap().1;
            assert_eq!((f.ctype, f.gz.is_some()), (ctype, !name.ends_with(".png")), "{name}：类型、压没压");
            let tail = |mut h: Vec<(String, String)>| {
                h.extend(pairs(&[
                    ("ETag", f.etag.as_str()),
                    ("Cache-Control", "no-cache"),
                    ("Vary", "Accept-Encoding"),
                    ("Content-Security-Policy", CSP),
                ]));
                h.extend(pairs(&TAIL));
                h
            };
            let length = f.raw.len().to_string();
            for ae in [None, Some("identity"), Some("br")] {
                let r = req(&ae.map(|v| ("Accept-Encoding", v)).into_iter().collect::<Vec<_>>());
                let (code, headers, body) = raw_reply(web.reply(f, &r, || true));
                assert_eq!((code, &body), (200, &f.raw), "{name}");
                assert_eq!(headers, tail(pairs(&[("Content-Type", ctype), ("Content-Length", length.as_str())])), "{name}");
            }
            let (code, headers, body) = raw_reply(web.reply(f, &req(&[("Accept-Encoding", "gzip, deflate, br")]), || true));
            match &f.gz {
                Some(gz) => {
                    assert_eq!((code, &body), (200, gz));
                    assert_eq!(gzip::decompress(&body, WEB_FILE).unwrap(), f.raw, "{name}：解开是原样的");
                    let gz_length = gz.len().to_string();
                    let first =
                        pairs(&[("Content-Type", ctype), ("Content-Length", gz_length.as_str()), ("Content-Encoding", "gzip")]);
                    assert_eq!(headers, tail(first), "{name}");
                }
                None => assert_eq!(
                    (code, body, headers),
                    (200, f.raw.clone(), tail(pairs(&[("Content-Type", ctype), ("Content-Length", length.as_str())])))
                ),
            }
            for extra in [None, Some(("Accept-Encoding", "gzip"))] {
                let mut h = vec![("If-None-Match", f.etag.as_str())];
                h.extend(extra);
                let (code, headers, body) = raw_reply(web.reply(f, &req(&h), || true));
                assert_eq!((code, body, headers), (304, Vec::new(), tail(Vec::new())), "{name}：没变");
            }
            let (code, _, body) = raw_reply(web.reply(f, &req(&[("If-None-Match", "\"0\"")]), || true));
            assert_eq!((code, body), (200, f.raw.clone()), "{name}：ETag 不对照常给");
        }
        let f = web.file("/").unwrap();
        assert_eq!(json_reply(web.reply(f, &req(&[]), || false)), (429, r#"{"error":"rate"}"#.to_string()), "限流");
    }

    /// 路径只在表里查名字（Py:1827）：别的都没有（回 404），目录穿越、编码过的、大小写不同的都查不到
    #[test]
    fn paths_are_names_in_the_table() {
        let web = plain();
        let index = web.files.get("index.html").unwrap() as *const WebFile;
        for path in ["/", "/index.html", "xindex.html", "*index.html"] {
            assert_eq!(web.file(path).map(|f| f as *const WebFile), Some(index), "{path}");
        }
        assert!(web.file("/xterm.js").is_some_and(|f| f.raw == embedded("xterm.js")));
        for path in [
            "",
            "index.html",
            "//index.html",
            "/../index.html",
            "/./index.html",
            "/%2e%2e/index.html",
            "/index%2Ehtml",
            "/INDEX.HTML",
            "/index.html/",
            "/README.md",
            "/xterm-LICENSE.txt",
            "/web/index.html",
            "/index.html#top",
            "/v1/health",
            // 第一个字符不是 ASCII：Python 去掉一个字节，剩下的开头不是 ASCII（请求行按 UTF-8、latin-1 解的两种写法）
            "éindex.html",
            "\u{c3}\u{a9}index.html",
        ] {
            assert!(web.file(path).is_none(), "{path}");
        }
    }

    /// 启动参数的检查：先后和每一句话都照 Python（Py:2187-2203）
    #[test]
    fn web_args_like_python() {
        const BAD: &str = "--web-url 要写成 https://域名[:端口]/（手机上打开的就是它；本机试的时候可以是 http://localhost:端口/）";
        const RELAY: &str = "网页端是经中转接入会话的：--web-url 要和 --relay-listen 一起给";
        const PROXY: &str = "--behind-proxy 时 HTTPS 由平台做：网页端就在同一个端口上，不用 --web-listen";
        const PAIR: &str = "--web-listen 要和 --web-url 一起给";
        const LOCAL: &str = "--web-url 是 http://localhost 时（本机试）：--web-listen 只能听本机的地址，不用证书";
        const CERT: &str = "--web-listen 要带浏览器信得过的证书：--web-cert、--web-key";
        type Case<'a> = ([Option<&'a str>; 5], bool, Result<(Option<&'a str>, bool), &'a str>);
        let (r, l, c, k) = (Some("0.0.0.0:18444"), Some("0.0.0.0:443"), Some("/x/fullchain.pem"), Some("/x/privkey.pem"));
        let https = Some("https://Relay.Example.com:443/");
        let ok = Ok((Some("https://relay.example.com/"), false));
        let cases: Vec<Case> = vec![
            ([None; 5], false, Ok((None, false))),
            ([Some(""), None, Some(""), Some(""), Some("")], true, Ok((None, false))),
            ([https, r, None, None, None], false, ok),
            ([https, r, l, c, k], false, ok),
            ([Some("ftp://relay.example.com/"), r, None, None, None], false, Err(BAD)),
            ([Some("https://relay.example.com:0/"), r, None, None, None], false, Err(BAD)),
            ([Some("http://192.0.2.1/"), r, None, None, None], false, Err(BAD)),
            ([Some("bad"), None, l, None, None], true, Err(BAD)),
            ([https, None, None, None, None], false, Err(RELAY)),
            ([https, Some(""), l, c, k], false, Err(RELAY)),
            ([None, None, l, None, None], true, Err(PROXY)),
            ([https, r, None, None, k], true, Err(PROXY)),
            ([None, None, None, c, None], false, Err(PAIR)),
            ([https, r, None, c, k], false, Err(PAIR)),
            ([https, r, l, None, None], false, Err(CERT)),
            ([https, r, l, c, None], false, Err(CERT)),
            (
                [Some("http://localhost:8080"), r, Some("127.0.0.1:8080"), None, None],
                false,
                Ok((Some("http://localhost:8080/"), true)),
            ),
            ([Some("http://127.0.0.1:80/"), r, Some("[::1]:8080"), None, None], false, Ok((Some("http://127.0.0.1/"), true))),
            ([Some("http://localhost/"), r, Some("localhost:8080"), None, None], false, Ok((Some("http://localhost/"), true))),
            ([Some("http://localhost/"), r, None, None, None], false, Ok((Some("http://localhost/"), true))),
            ([Some("http://localhost/"), r, Some("0.0.0.0:8080"), None, None], false, Err(LOCAL)),
            ([Some("http://localhost/"), r, Some(":8080"), None, None], false, Err(LOCAL)),
            ([Some("http://localhost/"), r, Some("8080"), None, None], false, Err(LOCAL)),
            ([Some("http://localhost/"), r, Some("localhost:8080"), c, k], false, Err(LOCAL)),
        ];
        for ([url, relay, listen, cert, key], proxy, want) in cases {
            let got = web_args(url, relay, listen, cert, key, proxy);
            let want = want.map(|(u, local)| (u.map(str::to_string), local));
            assert_eq!(got, want, "{url:?} {relay:?} {listen:?} {cert:?} {key:?} {proxy}");
        }
    }

    #[test]
    fn vectors_origin() {
        for case in vectors().list("origin").unwrap().items() {
            let case = case.as_list().unwrap();
            let given = case.at(0).as_str().map(str::to_string);
            let want = case.at(1).as_str().map(str::to_string);
            assert_eq!(web_origin(given.as_deref().unwrap_or("")), want, "{given:?}");
        }
    }

    #[test]
    fn vectors_name() {
        let dir = TempDir::new().unwrap();
        let f = Path::new(&dir.path).join("name");
        let doc = vectors();
        for case in doc.list("name").unwrap().items() {
            let case = case.as_dict().unwrap();
            let raw = parts(&case.list("in").unwrap());
            std::fs::write(&f, &raw).unwrap();
            let want = case.str("want").map(|s| s.to_string());
            assert_eq!(web_name(Some(&f)), want, "{:?}", String::from_utf8_lossy(&raw[..raw.len().min(80)]));
        }
        let special = doc.dict("name_special").unwrap();
        assert!(special.items().iter().all(|(_, v)| v.is_none()));
        assert_eq!(web_name(Some(Path::new(&dir.path))), None, "目录");
        assert_eq!(web_name(Some(&Path::new(&dir.path).join("nope"))), None, "没有这个文件");
        assert_eq!((web_name(None), web_name(Some(Path::new("")))), (None, None));
    }

    #[test]
    fn vectors_named() {
        for case in vectors().list("named").unwrap().items() {
            let case = case.as_dict().unwrap();
            let (fname, raw) = (case.str("file").unwrap(), b64(&case.str("raw").unwrap()));
            for out in case.list("outs").unwrap().items() {
                let out = out.as_dict().unwrap();
                let team = out.str("team").unwrap();
                let got = web_named(&fname, &raw, &team);
                match (out.str("out"), out.str("error").or(out.str("type_error"))) {
                    (Some(want), _) => assert_eq!(got.unwrap(), b64(&want), "{fname} {team:?}"),
                    (None, Some(want)) => assert_eq!(got.err().unwrap().to_string(), &*want, "{fname} {team:?}"),
                    _ => panic!("对照数据不对"),
                }
            }
        }
    }

    /// 各种 --web-root：给哪些文件、类型、内容、ETag，读不了、太大、没有 index.html 时的那句话（带着路径）
    #[test]
    fn vectors_roots() {
        let doc = vectors();
        let typical = doc.dict("trees").unwrap().dict("typical").unwrap();
        let mut gz_differs = Vec::new();
        for case in doc.list("roots").unwrap().items() {
            let case = case.as_dict().unwrap();
            let about = case.str("about").unwrap().to_string();
            let dir = TempDir::new().unwrap();
            let root = Path::new(&dir.path).join("root");
            let (given, tree): (PathBuf, Option<Dict>) = if case.has("root_case") {
                let page: &[u8] = b"<!doctype html><title>cca</title>\n";
                match about.as_str() {
                    "missing root" => {}
                    "root is a file" => std::fs::write(&root, b"x").unwrap(),
                    "unreadable root" => {
                        std::fs::create_dir(&root).unwrap();
                        std::fs::write(root.join("index.html"), page).unwrap();
                        chmod(&root, 0);
                    }
                    "trailing slash, no index" => {
                        std::fs::create_dir(&root).unwrap();
                        std::fs::write(root.join("a.js"), b"x").unwrap();
                    }
                    "trailing slash, too big" => {
                        std::fs::create_dir(&root).unwrap();
                        std::fs::write(root.join("b.js"), vec![b'a'; WEB_FILE + 1]).unwrap();
                    }
                    other => panic!("认不得的用例 {other}"),
                }
                let given = case.str("given").unwrap().replace("{root}", &root.to_string_lossy());
                (PathBuf::from(given), None)
            } else {
                std::fs::create_dir(&root).unwrap();
                let tree = match case.get("tree").unwrap() {
                    V::Str(_) => typical.clone(),
                    t => t.as_dict().unwrap(),
                };
                build(&tree, &root);
                (root.clone(), Some(tree))
            };
            let moded = about.contains("unreadable");
            let got = Web::open(&case.str("url").unwrap_or_else(|| URL.into()), Some(&given), case.str("team").as_deref());
            if let Some(tree) = &tree {
                unbuild(tree, &root);
            }
            if about == "unreadable root" {
                chmod(&root, 0o700);
            }
            if moded && as_root() {
                continue;
            }
            match (got, case.str("error").or(case.str("type_error"))) {
                (Err(e), Some(want)) => {
                    assert_eq!(e.to_string(), want.replace("{root}", &root.to_string_lossy()), "{about}");
                    assert_eq!(e.is_os(), want.starts_with("[Errno"), "{about}：读不了的是 OSError 一类");
                }
                (Ok(web), None) => {
                    assert_eq!(
                        (web.url.as_str(), web.origin.as_str()),
                        (&*case.str("web_url").unwrap(), &*case.str("origin").unwrap()),
                        "{about}"
                    );
                    let headers: Vec<(String, String)> = web.headers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
                    let want: Vec<(String, String)> = case
                        .list("headers")
                        .unwrap()
                        .items()
                        .iter()
                        .map(|h| {
                            let h = h.as_list().unwrap();
                            (h.at(0).as_str().unwrap().to_string(), h.at(1).as_str().unwrap().to_string())
                        })
                        .collect();
                    assert_eq!(headers, want, "{about}");
                    let files = case.list("files").unwrap().items();
                    assert_eq!(
                        web.files.keys().collect::<Vec<_>>(),
                        files.iter().map(|f| f.as_dict().unwrap().str("name").unwrap().to_string()).collect::<Vec<_>>(),
                        "{about}"
                    );
                    for f in files {
                        let f = f.as_dict().unwrap();
                        let name = f.str("name").unwrap();
                        let mine = web.files.get(&name).unwrap();
                        let ctype = f.str("ctype").unwrap();
                        let want = (
                            &*ctype,
                            f.at("len").as_int().unwrap() as usize,
                            f.str("sha256").unwrap().to_string(),
                            f.str("etag").unwrap().to_string(),
                        );
                        assert_eq!(
                            (mine.ctype, mine.raw.len(), hash::sha256_hex(&mine.raw), mine.etag.clone()),
                            want,
                            "{about} {name}"
                        );
                        if mine.gz.is_some() != f.at("gz").as_bool().unwrap() {
                            gz_differs.push(format!("{about} {name}"));
                        }
                    }
                }
                (got, want) => panic!("{about}：得到 {:?}，该是 {want:?}", got.map(|w| w.files.len())),
            }
        }
        assert!(gz_differs.is_empty(), "压没压和 Python 不一样：{gz_differs:?}");
    }

    /// 对照数据里的 HTTP 请求：真的 Python 服务器（--behind-proxy、--web-root 是对照数据里那棵目录）怎么回的，这里照样回。
    /// 请求头的值照 Python 读到的样子给（去掉开头的空白，结尾的留着）；请求行里的地址开头的几个 `/` 并成一个（Python 的
    /// http.server 先这样做了才交给 do_GET）。HEAD、PUT 这些是 http.rs 的事（501），这里不比
    #[test]
    fn vectors_http() {
        let doc = vectors();
        let http = doc.dict("http").unwrap();
        let tree = doc.dict("trees").unwrap().dict(&http.str("tree").unwrap()).unwrap();
        let dir = TempDir::new().unwrap();
        let root = Path::new(&dir.path).join("root");
        std::fs::create_dir(&root).unwrap();
        build(&tree, &root);
        let web = Web::open(&http.str("url").unwrap(), Some(&root), None).unwrap();
        unbuild(&tree, &root);
        let mut seen = 0;
        for case in http.list("cases").unwrap().items() {
            let case = case.as_dict().unwrap();
            let about = case.str("about").unwrap().to_string();
            if &*case.str("layer").unwrap() != "web" && about != "the websocket path without upgrade" {
                continue;
            }
            let line = case.str("line").unwrap();
            let mut target = line.split(' ').nth(1).unwrap().to_string();
            if target.starts_with("//") {
                target = format!("/{}", target.trim_start_matches('/'));
            }
            let headers = case
                .list("headers")
                .unwrap()
                .items()
                .iter()
                .map(|h| {
                    let h = h.as_list().unwrap();
                    (h.at(0).as_str().unwrap().to_string(), h.at(1).as_str().unwrap().trim_start_matches([' ', '\t']).to_string())
                })
                .collect();
            let r = Request { method: Method::Get, target: target.clone(), headers, body: Vec::new() };
            let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
            let status: u16 = case.str("status_line").unwrap().split(' ').nth(1).unwrap().parse().unwrap();
            let want_body = recorded(&case.get("body").unwrap());
            let mut want_headers: Vec<(String, String)> = case
                .list("response_headers")
                .unwrap()
                .items()
                .iter()
                .map(|h| {
                    let h = h.as_list().unwrap();
                    (h.at(0).as_str().unwrap().to_string(), h.at(1).as_str().unwrap().to_string())
                })
                .filter(|(k, _)| k != "Server" && k != "Date")
                .collect();
            seen += 1;
            if path == "/v1/relay/ws" {
                let (code, body) = json_reply(crate::server::ws::websocket(&r, query, &web, "198.51.100.1", || true, |_| true));
                assert_eq!((code, out_data(body.as_bytes())), (status, want_body), "{about}");
                continue;
            }
            let Some(f) = web.file(path) else {
                assert_eq!((status, want_body), (404, out_data(br#"{"error":"not_found"}"#)), "{about}：该有这个文件");
                continue;
            };
            let (code, headers, body) = raw_reply(web.reply(f, &r, || true));
            assert_eq!(code, status, "{about}");
            if case.truthy("gzip") {
                // 压出来的字节和 Python 的不一样（长度也是）：比解开的，Content-Length 照这边的长度
                for h in want_headers.iter_mut().filter(|(k, _)| k == "Content-Length") {
                    h.1 = body.len().to_string();
                }
                assert_eq!(out_data(&gzip::decompress(&body, WEB_FILE).unwrap()), want_body, "{about}");
            } else {
                assert_eq!(out_data(&body), want_body, "{about}");
            }
            assert_eq!(headers, want_headers, "{about}");
        }
        assert!(seen > 100, "{seen}");
    }

    /// 同时开着的 WebSocket 的名额：一共 64 条、同一个 IP 8 条（Py:1647-1667）
    #[test]
    fn websocket_slots() {
        let web = Web {
            url: "https://example.com/".into(),
            origin: "https://example.com".into(),
            headers: Vec::new(),
            files: OMap::new(),
            slots: Web::new_slots(),
        };
        assert!((0..8).all(|_| web.take("192.0.2.1")) && !web.take("192.0.2.1"));
        web.free("192.0.2.1");
        assert!(web.take("192.0.2.1"));
        for i in 0..56 {
            assert!(web.take(&format!("198.51.100.{i}")), "{i}");
        }
        assert!(!web.take("203.0.113.1"), "一共 64 条");
        web.free("198.51.100.0");
        assert!(web.take("203.0.113.1"));
    }
}
