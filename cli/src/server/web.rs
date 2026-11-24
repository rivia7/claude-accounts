//! 网页端（手机浏览器里的那一页，server/README.md「网页端」）：页面的文件、页面的地址、每个回复都带的那几个头、WebSocket 的名额。
//! Python 的 `web_origin`、`web_name`、`web_named`、`Web`（Py:1572-1667）。
//!
//! 页面的文件编在程序里（`FILES`，就是 server/web/ 里的那些）；--web-root 给了就改从那个目录读（照文件名的先后）。给出去的只有
//! 名字合 `wire::web_name_re()`、后缀在 `WEB_TYPES` 里的（说明、许可这些不是页面的一部分），一个最大 `WEB_FILE`，一定要有 index.html。
//! 启动时读进内存：原样的、gzip 压过的（.png 不压，压了没变小的不留）、ETag（`"` + SHA-256 的前 32 个十六进制字符 + `"`）。
//! Team 的名字给了（--web-name-file）：index.html 的标题、manifest 里的名字带上它（`web_named`）。
//!
//! 现在只有文件表、WebSocket 的名额和接口：别的还没移植。

use super::limits::Slots;
use super::omap::OMap;
use super::{unfinished, WS_CONN, WS_CONN_IP};
use crate::py::R;
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

impl Web {
    /// url 是 `web_origin` 写好的地址加 `/`；root 是 --web-root（None：用编在程序里的那一份）；team 是 Team 的名字（`web_name` 给的）。
    /// 文件读不了、太大、没有 index.html 是 Err（启动时说“网页端的文件读不了：…”，Py:2232-2233）
    pub fn open(url: &str, root: Option<&Path>, team: Option<&str>) -> R<Web> {
        let _ = (url, root, team, gzip9(&[]));
        unfinished("team-server 的网页端（Web::open）")
    }

    /// 占一条 WebSocket 的名额；满了是 false（回 429 busy）（Py:1652-1658）
    pub fn take(&self, ip: &str) -> bool {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).take(ip, false)
    }

    /// 还一条（Py:1660-1667）
    pub fn free(&self, ip: &str) {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner).free(ip);
    }

    /// WebSocket 的名额：刚开始都空着
    pub(super) fn new_slots() -> Mutex<Slots> {
        Mutex::new(Slots::new(WS_CONN, WS_CONN_IP))
    }
}

/// 【占位】gzip 压缩（level 9）。等 `cli/src/py/gzip.rs` 有了（别的人在写：`compress(raw, level)` / `decompress(gz, max)`），
/// 这里换成 `crate::py::gzip::compress(raw, 9)`、删掉这个函数。现在一律是 None：等于每个文件都没有压过的那一份
fn gzip9(raw: &[u8]) -> Option<Vec<u8>> {
    let _ = raw;
    None
}

/// --web-url 写成浏览器自己说的那个样子：主机小写，默认的端口（https 的 443、http 的 80）不写。样子不对、端口不在范围里是 None
/// （Py:1572-1584）。还没移植
pub fn web_origin(url: &str) -> Option<String> {
    let _ = url;
    None
}

/// --web-name-file 里的 Team 的名字：第一行，1～64 个字符、没有控制字符。没给、读不了、不对是 None（页面照旧叫 cca）
/// （Py:1587-1599）。还没移植
pub fn web_name(path: Option<&Path>) -> Option<String> {
    let _ = path;
    None
}

/// 网页的标题、手机主屏幕上的名字带上 Team 的名字：index.html 的 `<title>` 和 apple-mobile-web-app-title，manifest 的 name、
/// short_name；别的文件原样（Py:1602-1616）。还没移植：现在都是原样
pub fn web_named(fname: &str, raw: &[u8], name: &str) -> Vec<u8> {
    let _ = (fname, name);
    raw.to_vec()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::server::wire::web_name_re;
    use crate::server::{WEB_FILE, WEB_TYPES};

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

    /// WebSocket 的名额：同一个 IP 8 条、一共 64 条
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
