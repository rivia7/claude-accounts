//! 官方版本列表：ChatGPT 的 Sparkle appcast（XML）、Claude 的 Squirrel 更新源（JSON），和取它们的 `cu_metadata`。

use super::pyv::*;
use super::{cu, cu_download, cu_host, cu_url, cu_url_v, cu_version_str, is, CU_CODEX, CU_FEED, CU_MAX_PACKAGE, CU_NS};
use crate::cmd::codex::json_loads_bytes;
use crate::dict;
use crate::py::base64::b64decode;
use crate::py::obj::{Dict, V};
use crate::py::rand::uuid4;
use crate::py::{path, text, xml, R};

/// 字节串里有没有这个词（不分 ASCII 的大小写）：`needle in data.upper()`
fn contains_upper(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// ChatGPT 的更新源（Sparkle 的 appcast）：适合这台 Mac 的完整安装包里构建号最大的那个，增量包不用
pub fn cu_parse_feed(data: &[u8], arch: &str, system: &str) -> R<Dict> {
    if data.len() > 2 * 1024 * 1024 || contains_upper(data, b"<!DOCTYPE") || contains_upper(data, b"<!ENTITY") {
        return cu_error("blocked", "官方版本列表格式不安全");
    }
    let Ok(tree) = xml::fromstring(data) else {
        return cu_error("error", "无法读取官方版本列表");
    };
    let root = tree.root();
    if root.tag() != "rss" {
        return cu_error("blocked", "官方版本列表格式无法识别");
    }
    let ns = |name: &str| format!("{CU_NS}{name}");
    let mut candidates: Vec<Dict> = Vec::new();
    for item in root.findall("channel").into_iter().flat_map(|channel| channel.findall("item")) {
        // item.findtext(…) or ''
        let field = |name: &str| item.findtext(&ns(name)).unwrap_or("").to_string();
        let (version, build, minimum, hardware) = (field("shortVersionString"), field("version"), field("minimumSystemVersion"), field("hardwareRequirements"));
        if hardware != arch || item.find(&ns("channel")).is_some() {
            continue;
        }
        if !digits_1_12(&build) || !dotted(&version, 1, 5) {
            continue;
        }
        if minimum.is_empty() || cu_version_str(&minimum)? > cu_version_str(system)? {
            continue;
        }
        let enclosures = item.findall("enclosure");
        if enclosures.len() != 1 {
            continue;
        }
        let enclosure = enclosures[0];
        if enclosure.get(&ns("deltaFrom")).is_some() {
            continue;
        }
        let url = cu_url(enclosure.get("url").unwrap_or(""), true)?;
        if !url.ends_with(&format!("/ChatGPT-darwin-{arch}-{version}.zip")) {
            return cu_error("blocked", "官方包地址和版本元数据不一致");
        }
        // int(enclosure.get('length', ''))：认不出、不在 (0, CU_MAX_PACKAGE] 里、签名解出来不是 64 个字节，都是「缺少有效长度或签名」
        let signature = enclosure.get(&ns("edSignature")).unwrap_or("");
        let length = PyInt::parse(enclosure.get("length").unwrap_or("")).ok().and_then(|n| n.to_i64()).filter(|n| 0 < *n && *n as u64 <= CU_MAX_PACKAGE);
        let (Some(length), Ok(64)) = (length, b64decode(signature, true).map(|raw| raw.len())) else {
            return cu_error("blocked", "官方包缺少有效长度或签名");
        };
        candidates.push(dict! {
            "version" => version, "build" => build, "minimum_system" => minimum, "arch" => arch, "url" => url, "length" => length, "signature" => signature,
        });
    }
    // max(candidates, key=lambda x: int(x['build']))：一样大的留前面那个
    let key = |c: &Dict| PyInt::parse(&c.at("build").py_str()).expect("构建号核对过是数字");
    let mut best: Option<Dict> = None;
    for c in candidates {
        if best.as_ref().is_none_or(|b| key(&c) > key(b)) {
            best = Some(c);
        }
    }
    match best {
        Some(best) => Ok(best),
        None => cu_error("unsupported", "官方列表没有适合此 Mac 的完整安装包"),
    }
}

/// Claude 的更新源（Squirrel 的 json 格式）：currentRelease 那一版的完整 zip，增量包不用
pub fn cu_parse_squirrel(data: &[u8], arch: &str) -> R<Dict> {
    if data.len() > 2 * 1024 * 1024 {
        return cu_error("blocked", "官方版本列表格式不安全");
    }
    let Some(doc) = json_loads_bytes(data) else {
        return cu_error("error", "无法读取官方版本列表");
    };
    let doc = doc.as_dict();
    let version = doc.as_ref().and_then(|d| d.get("currentRelease"));
    let releases = doc.as_ref().and_then(|d| d.get("releases"));
    let (Some(V::Str(version)), Some(V::List(releases))) = (&version, &releases) else {
        return cu_error("blocked", "官方版本列表格式无法识别");
    };
    if !dotted(version, 1, 5) {
        return cu_error("blocked", "官方版本列表格式无法识别");
    }
    let version_v = V::Str(version.clone());
    // rows = [r.get('updateTo') for r in releases if isinstance(r, dict) and r.get('version') == version]
    let rows: Vec<Option<V>> = releases.items().iter().filter_map(|r| r.as_dict()).filter(|r| r.get("version").is_some_and(|v| v.eq_py(&version_v))).map(|r| r.get("updateTo")).collect();
    let row = match rows.as_slice() {
        [Some(V::Dict(row))] if row.get("version").is_some_and(|v| v.eq_py(&version_v)) => row.clone(),
        _ => return cu_error("blocked", "官方版本列表格式无法识别"),
    };
    let (url, digest, size) = (row.get("url").unwrap_or_else(|| V::from("")), row.get("sha256"), row.get("size"));
    let url = cu_url_v(&url, true)?;
    if !url.contains(&format!("/releases/darwin/{arch}/{version}/")) {
        return cu_error("blocked", "官方包地址和版本元数据不一致");
    }
    // type(size) is int：布尔值不算
    let digest_ok = matches!(&digest, Some(V::Str(d)) if d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let size_ok = matches!(&size, Some(V::Int(n)) if 0 < *n && *n as u64 <= CU_MAX_PACKAGE);
    if !digest_ok || !size_ok {
        return cu_error("blocked", "官方包缺少有效长度或校验值");
    }
    Ok(dict! { "version" => version_v.clone(), "build" => version_v, "minimum_system" => "", "arch" => arch, "url" => url, "length" => size, "sha256" => digest })
}

/// 官方最新版本的元数据；None 表示更新源说没有新版本
pub fn cu_metadata(target: &Dict, folder: &str) -> R<Option<Dict>> {
    let (arch, system) = cu_host()?;
    let feed = path::join(folder, "feed");
    if is(cu(), &CU_CODEX) {
        cu_download(CU_FEED, &feed, folder, false, None, &[])?;
        return Ok(Some(cu_parse_feed(&std::fs::read(&feed)?, &arch, &system)?));
    }
    // device_id 是必填的：每次现生成一个，不用也不存任何账号实例的标识（Claude 的 ant-did 在各账号的资料目录里）
    for a in [arch.as_str(), "universal"] {
        let query = format!("device_id={}&version={}&os_version={}", uuid4(), target.at("version").py_str(), system);
        let code = cu_download(&format!("https://api.anthropic.com/api/desktop/darwin/{a}/squirrel/update?{query}"), &feed, folder, false, None, &["204", "404"])?;
        match code.as_deref() {
            Some("204") => return Ok(None),
            None => return Ok(Some(cu_parse_squirrel(&std::fs::read(&feed)?, a)?)),
            Some(_) => {}
        }
    }
    cu_error("unsupported", "官方列表没有适合此 Mac 的完整安装包")
}

/// 官方的最新版本：纯文本（Claude Code 的发布通道）或 JSON（npm、Homebrew 的 version，GitHub 的 tag_name）
pub fn cu_cli_latest(data: &[u8]) -> R<String> {
    let decoded = String::from_utf8_lossy(data);
    let mut text = text::strip(&decoded).to_string();
    if text.starts_with('{') {
        let Ok(j) = crate::py::json::loads(&text) else {
            return cu_error("error", "官方版本信息无法识别");
        };
        // str(j.get('version') or j.get('tag_name') or '')
        text = match j.as_dict() {
            Some(j) => j.get("version").filter(|v| v.truthy()).or_else(|| j.get("tag_name").filter(|v| v.truthy())).map_or(String::new(), |v| v.py_str()),
            None => String::new(),
        };
    }
    // (?:rust-v|v)?(\d+\.\d+\.\d+)
    let number = text.strip_prefix("rust-v").or_else(|| text.strip_prefix('v')).unwrap_or(&text);
    for candidate in [number, text.as_str()] {
        if dotted(candidate, 2, 2) {
            return Ok(candidate.to_string());
        }
    }
    cu_error("error", "官方版本信息无法识别")
}
