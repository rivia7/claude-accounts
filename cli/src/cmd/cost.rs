//! 折合金额（`cost_model` 到 `cmd_cost_stop`）：cost-worker、cost-sync、cost-stop。视图（`cost_local_day` 到 `CostView`，
//! _gui state 用）还没移植。
//!
//! 这台设备上 Claude Code、Codex 的会话记录（共享的 ~/.claude/projects、~/.codex/sessions）里每一次请求的用量，按官方 API 的
//! 标价折成美元，按登录的账号身份（Claude 的 accountUuid、Codex 的 chatgpt_user_id）、按小时累计。不是实际花费（订阅不按这个
//! 收费）。只读用量的数字（token 数、模型、时间、账号身份），不读对话的内容。
//! 会话记录里带账号信息才算得进来：Claude Code 在会话里写 session-owner（2.1.317 起；换了账号续接时它把会话复制成一个新的，
//! 所以按位置认：这一行以后的回复都是这个账号的），子代理的记录按它所在的会话认；Codex 的 session_meta 写 creator_user_id
//! （0.157 起；一个文件里第一个 session_meta 是它自己的，后面的是复制进来的历史），用量取 token_count 事件。更早的、Claude
//! 桌面 App 里的会话算不进来；Codex 续接别的账号建的会话时，用量记在建它的账号上。
//! 增量地读：每个文件记着读到哪（只读完整的行；文件被截短、换了、读过的末尾对不上就从头读），回复按消息 id 去重，Codex 的
//! 按时间和累计的用量去重。
//! 映射里的官方账号、开着设备统计时，各台设备算的经维护者的设备统计服务器交换（op ledger，见 server/README.md）：一台设备
//! 一份、只写自己的那一份，服务器只存密文（密钥和共享 API key 一样由盐、邮箱、登录身份派生，标签不同）。
//! 统计在后台进程里做（cca _cost-sync：会话结束、cca usage、cca devices、上报进程里，最多 COST_EVERY 一次；App 定时跑
//! cca cost sync --json），_gui state 只读统计好的（cost/local.json、remote.json、sync.json）。config.json 的 "cost": false 关掉

use super::keyshare::{blob_ad, blob_open, blob_seal, is_num, keyshare_keys, share_ident, Keys};
use super::presence::{presence_aid, presence_load, presence_post, presence_targets, presence_why, spawn_background};
use super::session::{cli_alive, proc_starts};
use super::usage::parse_ts;
use super::{arity, Cmd};
use crate::core::base::{CCA_HOME, CFG, EMAIL_RE, MASTER_JSON};
use crate::core::model::{cdir, is_api, parse_key};
use crate::core::tool::{claude, codex, Kind, Tool, Using};
use crate::py::fs::{load_quiet, utime_now};
use crate::py::hash::{sha1_hex, sha256_hex};
use crate::py::obj::{Dict, List, V};
use crate::py::proc::getenv;
use crate::py::re::Re;
use crate::py::time::{monotonic, sleep, time, timegm};
use crate::py::{exit, json, num, path, shutil, text, zlib, Fail, R};
use crate::{dict, list, out};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[Cmd::all("cost-worker", cmd_cost_worker), Cmd::all("cost-sync", cmd_cost_sync), Cmd::all("cost-stop", cmd_cost_stop)];

pub static COST_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "cost"));
/// 价格表核实的日期（platform.claude.com/docs/en/about-claude/pricing、developers.openai.com/api/docs/pricing）
pub const COST_PRICES_DATE: &str = "2026-11-13";
/// 价格表、读会话记录的规则变了要改：scan.json 的 build 对不上就从头重算（COST_PRICES_DATE + "/1"）
pub const COST_BUILD: &str = "2026-11-13/1";
/// 后台统计最多隔这么久起一次
pub const COST_EVERY: f64 = 300.0;
/// 和别的设备同步最多隔这么久一次（cca cost sync --now 不等）
pub const COST_SYNC_EVERY: f64 = 600.0;
const WEEK: f64 = 7.0 * 86400.0;
/// scan.json 里的小时数据留这么久（WEEKS_KEEP 周）：从头重算时会话记录可能已经被 Claude Code 删掉了一部分，以前算过的靠它接上
pub const COST_KEEP: f64 = 60.0 * WEEK;
/// local.json 里、传给别的设备的留这么久（App 看最近 12 周和这一周）：只传显示得到的，密文小
pub const COST_PUBLISH: f64 = 14.0 * WEEK;
/// 去重的记录留这么久；之后才读到的更早的回复（改了名、复制出来的文件）不算
pub const COST_SEEN: f64 = 35.0 * 86400.0;
/// 别的设备超过这么久没传上来：之后用的可能还没算进来
pub const COST_STALE: f64 = 36.0 * 3600.0;
/// 内容没变也隔这么久重传一次（别的设备按它看这台设备是不是还在同步）
pub const COST_BEAT: f64 = 12.0 * 3600.0;
/// 旧的服务器（404：还不收）隔这么久再试
pub const COST_RETRY: f64 = 6.0 * 3600.0;
/// 按天看最近这么多天
pub const COST_DAYS: i64 = 30;
/// 密钥、认证的标签（和共享 API key 的分开）
pub const COST_LABEL: &str = "cca-cost-v1";
/// 明文补齐到这么长的整数倍
pub const COST_PAD: usize = 1024;
/// 服务器收的一份密文最长多少个字符（base64）
pub const COST_BLOB_MAX: usize = 98304;
/// 一次请求最多传这么多密文（多了分几次）
pub const COST_PUT_MAX: usize = 256 * 1024;
/// cca cost sync 最多等正在跑的统计这么久
pub const COST_WAIT: i64 = 240;
pub const COST_OLD: &str = "维护者的设备统计服务器还不支持同步折合金额（维护者要更新服务器：./policy.sh presence deploy）";
pub static COST_HOUR_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[0-9]{9,11}$"));
static COST_DATE_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"-(20[0-9]{6}|20[0-9]{2}-[0-9]{2}-[0-9]{2})$"));

/// Anthropic 的标价（美元 / 百万 token）：输入、5 分钟缓存写、1 小时缓存写、缓存读、输出
const CLAUDE_PRICES: &[(&str, [f64; 5])] = &[
    ("claude-fable-5-2", [8.0, 10.0, 16.0, 0.20, 40.0]),
    ("claude-mythos-5-2", [8.0, 10.0, 16.0, 0.20, 40.0]),
    ("claude-fable-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-mythos-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-fable-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-mythos-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-opus-5-5", [4.0, 5.0, 8.0, 0.20, 20.0]),
    ("claude-opus-5", [5.0, 6.25, 10.0, 0.50, 25.0]),
    ("claude-opus-4-8", [5.0, 6.25, 10.0, 0.50, 25.0]),
    ("claude-opus-4-7", [5.0, 6.25, 10.0, 0.50, 25.0]),
    ("claude-opus-4-6", [5.0, 6.25, 10.0, 0.50, 25.0]),
    ("claude-opus-4-5", [5.0, 6.25, 10.0, 0.50, 25.0]),
    ("claude-opus-4-1", [15.0, 18.75, 30.0, 1.50, 75.0]),
    ("claude-opus-4", [15.0, 18.75, 30.0, 1.50, 75.0]),
    ("claude-sonnet-5-5", [2.0, 2.5, 4.0, 0.10, 10.0]),
    ("claude-sonnet-5", [2.0, 2.5, 4.0, 0.20, 10.0]),
    ("claude-sonnet-4-6", [3.0, 3.75, 6.0, 0.30, 15.0]),
    ("claude-sonnet-4-5", [3.0, 3.75, 6.0, 0.30, 15.0]),
    ("claude-sonnet-4", [3.0, 3.75, 6.0, 0.30, 15.0]),
    ("claude-haiku-5-5", [0.10, 0.125, 0.20, 0.01, 0.50]),
    ("claude-haiku-4-5", [1.0, 1.25, 2.0, 0.10, 5.0]),
];
/// 一次请求的输入（含缓存）超过这么多 token 时的标价
const CLAUDE_LONG: &[(&str, i64, [f64; 5])] = &[("claude-haiku-5-5", 100000, [0.50, 0.625, 1.0, 0.05, 2.50])];
/// 快速模式（usage.speed 是 fast）的输入、输出；缓存跟着输入翻倍
const CLAUDE_FAST: &[(&str, f64, f64)] = &[
    ("claude-fable-5-2", 16.0, 80.0),
    ("claude-mythos-5-2", 16.0, 80.0),
    ("claude-opus-5-5", 8.0, 40.0),
    ("claude-opus-5", 10.0, 50.0),
    ("claude-opus-4-8", 10.0, 50.0),
];
/// 4.6 以前的模型：只在美国推理（inference_geo 是 us）不加价，别的加 10%
const CLAUDE_NO_GEO: &[&str] = &["claude-opus-4-5", "claude-opus-4-1", "claude-opus-4", "claude-sonnet-4-5", "claude-sonnet-4", "claude-haiku-4-5"];
/// 网页搜索一次 0.01 美元（单位：百万分之一美元）
const CLAUDE_WEB_SEARCH: f64 = 10000.0;

/// OpenAI 的一组标价（美元 / 百万 token）：输入、缓存读、缓存写（30 分钟；没有单独标价的按输入算）、输出
type OpenAiPrice = (f64, f64, Option<f64>, f64);
/// 后一组是一次请求的输入超过 OPENAI_LONG 时的。
/// gpt-5.6-sol 是促销价（官方写着到 2026-11-30，没写之后的价）：官方改价以后要按请求的时间分段、以前的照这个价，
/// 不能直接换掉（换了价要改 COST_BUILD，从头重算时以前的用量也会按新价算）
const OPENAI_PRICES: &[(&str, OpenAiPrice, Option<OpenAiPrice>)] = &[
    ("gpt-6.1-sol", (2.0, 0.10, Some(2.50), 10.0), Some((4.0, 0.20, Some(5.0), 15.0))),
    ("gpt-6-astra", (10.0, 1.0, Some(12.50), 50.0), Some((20.0, 2.0, Some(25.0), 75.0))),
    ("gpt-6.1-terra", (1.0, 0.05, Some(1.25), 5.0), Some((2.0, 0.10, Some(2.50), 7.50))),
    ("gpt-6-sol", (2.0, 0.20, Some(2.50), 10.0), Some((4.0, 0.40, Some(5.0), 15.0))),
    ("gpt-6-luna", (0.10, 0.01, Some(0.125), 0.50), Some((0.20, 0.02, Some(0.25), 0.75))),
    ("gpt-5.6-sol", (4.0, 0.40, Some(5.0), 20.0), Some((8.0, 0.80, Some(10.0), 30.0))),
    ("gpt-5.6-terra", (2.0, 0.20, Some(2.50), 12.0), Some((4.0, 0.40, Some(5.0), 18.0))),
    ("gpt-5.6-luna", (0.20, 0.02, Some(0.25), 1.20), Some((0.40, 0.04, Some(0.50), 1.80))),
    ("gpt-5.3-codex", (1.75, 0.175, None, 14.0), None),
];
const OPENAI_LONG: i64 = 272000;
/// Codex 的 service_tier → 标价的倍数（Priority 改名叫 Fast；Flex 半价）
const OPENAI_TIERS: &[(&str, f64)] = &[("priority", 2.0), ("fast", 2.0), ("flex", 0.5)];
/// Ultrafast 只有这两个模型有标价（6 倍），别的按 Fast 算
const OPENAI_ULTRA: &[(&str, f64)] = &[("gpt-6.1-sol", 6.0), ("gpt-6-astra", 6.0)];

/// 会话记录里的模型名 → 价格表里的写法：小写，去掉 [1m] 这类后缀和末尾的日期
pub fn cost_model(m: Option<&V>) -> String {
    let Some(m) = m.and_then(|m| m.as_str()) else { return String::new() };
    let lower = text::strip(m).to_lowercase();
    let base = lower.split('[').next().unwrap_or("");
    text::head(&COST_DATE_RE.sub("", base), 64).to_string()
}

/// token 数这类计数：非负整数（bool、小数、大得离谱的都当 0）
fn cost_int(x: Option<V>) -> i64 {
    match x {
        Some(V::Int(n)) if (0..1_000_000_000_000).contains(&n) => n,
        _ => 0,
    }
}

/// `u.get(k)`；u 不是对象的地方原来是 AttributeError（那一行跳过），这里由调用的地方先核对
fn tokens(u: &Dict, k: &str) -> i64 {
    cost_int(u.get(k))
}

/// 一条 Claude 回复的用量（message.usage）：(折合多少百万分之一美元：没有公开价格的模型是 None, (输入, 输出, 缓存读, 缓存写))。
/// 金额照原来的先后用小数算（原来整数的标价乘出来是整数，这里的小数在 2^53 以内一样精确）
pub fn cost_claude(model: &str, u: &Dict) -> (Option<f64>, [i64; 4]) {
    let (inp, out, cr) = (tokens(u, "input_tokens"), tokens(u, "output_tokens"), tokens(u, "cache_read_input_tokens"));
    let cc = u.dict("cache_creation").unwrap_or_default();
    let (mut w5, w1) = (tokens(&cc, "ephemeral_5m_input_tokens"), tokens(&cc, "ephemeral_1h_input_tokens"));
    if w5 == 0 && w1 == 0 {
        w5 = tokens(u, "cache_creation_input_tokens");
    }
    let tok = [inp, out, cr, w5 + w1];
    let Some((_, mut p)) = CLAUDE_PRICES.iter().find(|(name, _)| *name == model).copied() else { return (None, tok) };
    if let Some((_, over, long)) = CLAUDE_LONG.iter().find(|(name, _, _)| *name == model) {
        if inp + cr + w5 + w1 > *over {
            p = *long;
        }
    }
    let [mut pin, mut p5, mut p1, mut pr, mut pout] = p;
    let speed = u.get("speed");
    if speed.as_ref().and_then(|s| s.as_str()) == Some("fast") {
        if let Some((_, fin, fout)) = CLAUDE_FAST.iter().find(|(name, _, _)| *name == model) {
            let k = fin / pin;
            (pin, p5, p1, pr, pout) = (*fin, p5 * k, p1 * k, pr * k, *fout);
        }
    }
    // 美元 / 百万 token × token 数 = 百万分之一美元
    let mut micro = inp as f64 * pin + w5 as f64 * p5 + w1 as f64 * p1 + cr as f64 * pr + out as f64 * pout;
    let geo = u.get("inference_geo");
    if geo.as_ref().and_then(|g| g.as_str()) == Some("us") && !CLAUDE_NO_GEO.contains(&model) {
        micro *= 1.1;
    }
    let searches = cost_int(u.dict("server_tool_use").and_then(|s| s.get("web_search_requests")));
    (Some(micro + searches as f64 * CLAUDE_WEB_SEARCH), tok)
}

/// 一次 Codex 请求（token_count 的 last_token_usage：input_tokens 含缓存读、缓存写，output_tokens 含推理）：同 cost_claude
pub fn cost_codex(model: &str, tier: Option<&str>, u: &Dict) -> (Option<f64>, [i64; 4]) {
    let (inp, cached, write, out) = (tokens(u, "input_tokens"), tokens(u, "cached_input_tokens"), tokens(u, "cache_write_input_tokens"), tokens(u, "output_tokens"));
    let cached = cached.min(inp);
    let write = write.min(inp - cached);
    let plain = inp - cached - write;
    let tok = [plain, out, cached, write];
    let Some((_, base, long)) = OPENAI_PRICES.iter().find(|(name, _, _)| *name == model) else { return (None, tok) };
    let (pin, pc, pw, pout) = match long {
        Some(long) if inp > OPENAI_LONG => *long,
        _ => *base,
    };
    let k = if tier == Some("ultrafast") {
        OPENAI_ULTRA.iter().find(|(name, _)| *name == model).map_or(2.0, |(_, k)| *k)
    } else {
        OPENAI_TIERS.iter().find(|(name, _)| Some(*name) == tier).map_or(1.0, |(_, k)| *k)
    };
    (Some((plain as f64 * pin + cached as f64 * pc + write as f64 * pw.unwrap_or(pin) + out as f64 * pout) * k), tok)
}

/// 会话记录里的时间（2026-11-13T08:30:00.123Z）→ 秒；别的写法交给 parse_ts
pub fn cost_ts(s: Option<&V>) -> R<Option<f64>> {
    let Some(s) = s else { return Ok(None) };
    if let Some(t) = s.as_str() {
        let c: Vec<char> = t.chars().collect();
        if c.len() >= 20 && c[4] == '-' && c[10] == 'T' && c[c.len() - 1] == 'Z' {
            let int = |a: usize, b: usize| text::to_int(&c[a..b].iter().collect::<String>());
            if let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (int(0, 4), int(5, 7), int(8, 10), int(11, 13), int(14, 16), int(17, 19)) {
                // calendar.timegm 只核对年、月（datetime.date(年, 月, 1)）：不对的是 ValueError，落到 parse_ts
                if (1..=9999).contains(&y) && (1..=12).contains(&mo) {
                    return Ok(Some(timegm(y, mo, d, h, mi, sec) as f64));
                }
            }
        }
    }
    parse_ts(s)
}

/// 读到的位置前面 64 个字节的摘要：下次对得上才接着读（文件被换掉、改写过就从头读）
fn cost_tail(f: &mut std::fs::File, off: i64) -> std::io::Result<String> {
    if off <= 0 {
        return Ok(String::new());
    }
    f.seek(SeekFrom::Start((off - 64).max(0) as u64))?;
    let mut buf = vec![0u8; off.min(64) as usize];
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(sha1_hex(&buf[..got])[..12].to_string())
}

/// 系统调用出的错带上路径，写成 Python 的 `str(OSError)` 那样（cca cost sync 把它打给人看）
fn at_path(e: std::io::Error, p: &str) -> Fail {
    Fail::Os(std::io::Error::new(e.kind(), crate::py::fs::open_error_text(&e, p)))
}

/// 折合金额开着吗：config.json 里没写 "cost": false
pub fn cost_on() -> bool {
    !load_quiet(&CFG).and_then(|c| c.as_dict()).is_some_and(|c| matches!(c.get("cost"), Some(V::Bool(false))))
}

/// 建 COST_DIR，但不建 ~/.cca 本身（和 presence_dir 一样：cca restore 刚删掉它时，后台还没跑完的统计不该把它建回来）
pub fn cost_dir() -> std::io::Result<()> {
    if !path::isdir(&COST_DIR) {
        match std::fs::DirBuilder::new().mode(0o700).create(&*COST_DIR) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// cost/ 里的文件：紧凑的 JSON（scan.json 有几 MB）、600、原子替换。config.json 没了（cca restore 刚删掉 ~/.cca）不写：OSError
pub fn cost_save(name: &str, data: &V) -> R<()> {
    use std::io::Write;
    if !path::isfile(&CFG) {
        return Err(Fail::Os(std::io::Error::other(format!("{} 不在了", *CFG))));
    }
    let dest = path::join(&COST_DIR, name);
    let tmp = format!("{}.cca-{}.tmp", dest, std::process::id());
    let write = || -> R<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| at_path(e, &tmp))?;
        f.write_all(json::dumps(data, json::Fmt::new().utf8().compact()).as_bytes())?;
        drop(f);
        std::fs::rename(&tmp, &dest).map_err(|e| {
            let text = format!("{} -> {}", crate::py::fs::open_error_text(&e, &tmp), crate::py::obj::repr_str(&dest));
            Fail::Os(std::io::Error::new(e.kind(), text))
        })
    };
    let done = write();
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// cost/ 里的一个文件（v 是 1 的对象）；没有、读不了、格式不对是 {}
pub fn cost_json(name: &str) -> Dict {
    load_quiet(&path::join(&COST_DIR, name)).and_then(|j| j.as_dict()).filter(|j| j.get("v").is_some_and(|v| v.py_eq(&V::Int(1)))).unwrap_or_default()
}

/// 7 个计数：都是整数（不是布尔值），不小于 0、不超过 10^15
fn counts_ok(r: &V) -> bool {
    r.as_list().is_some_and(|r| r.len() == 7 && r.items().iter().all(|x| matches!(x, V::Int(n) if (0..=1_000_000_000_000_000).contains(n))))
}

/// {小时: [7 个计数]}：金额（百万分之一美元）、请求数、输入、输出、缓存读、缓存写、其中没有标价的请求数
pub fn cost_rows_ok(rows: Option<&V>) -> bool {
    rows.and_then(|r| r.as_dict()).is_some_and(|rows| rows.items().iter().all(|(k, r)| COST_HOUR_RE.matches(k) && counts_ok(r)))
}

fn cost_fresh() -> Dict {
    dict! {
        "v" => 1, "build" => COST_BUILD, "done" => false, "files" => Dict::new(), "seen" => Dict::new(), "owners" => Dict::new(),
        "h" => Dict::new(), "orphan" => Dict::new(), "um" => Dict::new(), "who" => Dict::new(),
    }
}

/// 身份的写法：claude:<accountUuid>、codex:<chatgpt_user_id>
pub fn cost_ident_ok(k: &str) -> bool {
    let (tool, _, ident) = text::partition(k, ":");
    (tool == "claude" || tool == "codex") && (8..=64).contains(&text::len(ident))
}

/// docs（scan.json、local.json 的内容）里记下的身份 → 邮箱，合在一起（后面的优先）；格式不对的项不要
pub fn cost_who_kept(docs: &[&Dict]) -> Dict {
    let out = Dict::new();
    for d in docs {
        for (k, e) in d.dict("who").map(|w| w.items()).unwrap_or_default() {
            if let Some(e) = e.as_str().filter(|e| cost_ident_ok(&k) && EMAIL_RE.matches(e)) {
                out.set(k, e.to_lowercase());
            }
        }
    }
    out
}

/// `(h.items() if isinstance(h, dict) else [])` 里合格的那些
fn rows_of(h: Option<V>) -> Vec<(std::rc::Rc<str>, V)> {
    h.and_then(|h| h.as_dict()).map(|h| h.items()).unwrap_or_default()
}

/// 从头重算时先留着以前算好的小时数据（重算完由 cost_keep_old 挑着用）：旧的 scan.json 里的（最多 COST_KEEP；上一次重算
/// 没算完又要从头来时是它留着的那份），scan.json 没了（restore 的备份不带它）就用 local.json 里的（最近 COST_PUBLISH）
fn cost_rows_kept(j: &Dict, loc: &Dict) -> Dict {
    let src = j.dict("old").unwrap_or_else(|| j.clone());
    let (mut h, mut orphan) = (src.get("h"), src.get("orphan"));
    if !h.as_ref().and_then(|h| h.as_dict()).is_some_and(|h| !h.is_empty()) {
        (h, orphan) = (loc.get("acc"), loc.get("orphan"));
    }
    let kept = Dict::new();
    for (i, r) in rows_of(h) {
        if cost_ident_ok(&i) && cost_rows_ok(Some(&r)) {
            kept.set(i, r);
        }
    }
    dict! { "h" => kept, "orphan" => orphan.filter(|o| cost_rows_ok(Some(o))).unwrap_or_else(|| V::Dict(Dict::new())) }
}

/// `int(k)`：小时的写法核对过是 9～11 位数字
fn hour_of(k: &str) -> f64 {
    text::to_int(k).unwrap_or(0) as f64
}

/// 从头重算完（s 是读完这一遍的 CostScan）：会话记录可能已经删掉了一部分的小时，以前算过的沿用以前的，别的照重算的。
/// Claude Code 按修改时间删会话记录（默认 30 天），删掉的那些里的请求都早于现在还在的最早一个的修改时间（s.oldest）：
/// 这个时间以后的小时会话记录是全的；以前的可能不全，用以前的价格、规则算的也比丢掉好
fn cost_keep_old(j: &Dict, s: &CostScan) {
    let Some(old) = j.pop("old").and_then(|o| o.as_dict()) else { return };
    let far = s.now + 86400.0; // 这个工具的会话记录一个都没了：以前算的全留着
    let h = j.dict("h").expect("scan.json 核对过");
    // (old.get('h') or {}).items()
    let old_h = match old.get("h").filter(|h| h.truthy()) {
        None => Vec::new(),
        Some(V::Dict(h)) => h.items(),
        Some(other) => panic!("AttributeError: 留着的小时数据是 {}", other.py_repr()),
    };
    for (ident, rows) in old_h {
        if cost_ident_ok(&ident) && cost_rows_ok(Some(&rows)) {
            let tool = text::partition(&ident, ":").0;
            let lo = s.oldest.iter().find(|(t, _)| *t == tool).map_or(far, |(_, m)| *m);
            for (k, r) in rows.as_dict().expect("cost_rows_ok 核对过").items() {
                if hour_of(&k) < lo {
                    h.setdefault(&ident, Dict::new()).as_dict().expect("scan.json 核对过").set(k, r);
                }
            }
        }
    }
    // 认不出是谁的不分工具：几个工具的会话记录都全了以后才照重算的
    let lo = s.oldest.iter().map(|(_, m)| *m).reduce(f64::max).unwrap_or(far);
    let orphan = j.dict("orphan").expect("scan.json 核对过");
    if let Some(rows) = old.get("orphan").filter(|r| cost_rows_ok(Some(r))) {
        for (k, r) in rows.as_dict().expect("cost_rows_ok 核对过").items() {
            if hour_of(&k) < lo {
                orphan.set(k, r);
            }
        }
    }
}

/// `type(x) is int`
fn is_int(x: &V) -> bool {
    matches!(x, V::Int(_) | V::Big(_))
}

/// scan.json（读到哪、去重、小时数据）；没有、价格表或规则变了、格式不对：从头来。从头来时以前记下的身份 → 邮箱照样留着
/// （scan.json、local.json 里的：退出登录了的账号，重算出来的用量照样认得出是谁的；restore 的备份不带 scan.json，带着 local.json）
fn cost_load_scan() -> Dict {
    let j = cost_json("scan.json");
    let base = cost_fresh();
    let dicts = ["files", "seen", "owners", "h", "orphan", "um", "who"];
    let shape = j.get("build").is_some_and(|b| b.as_str() == Some(COST_BUILD))
        && j.get("v").is_some_and(|v| v.isinstance_int())
        && j.get("done").is_some_and(|d| d.is_bool())
        && dicts.iter().all(|k| j.dict(k).is_some());
    let all = |k: &str, ok: &dyn Fn(&V) -> bool| j.dict(k).is_some_and(|d| d.values().iter().all(ok));
    if shape
        && all("files", &|x| x.is_dict())
        && all("seen", &is_int)
        && all("owners", &|x| x.is_list())
        && cost_rows_ok(j.get("orphan").as_ref())
        && all("h", &|r| cost_rows_ok(Some(r)))
        && all("um", &|m| m.as_dict().is_some_and(|m| m.values().iter().all(is_int)))
    {
        j.set("who", cost_who_kept(&[&j]));
        return j;
    }
    let loc = cost_json("local.json");
    base.set("who", cost_who_kept(&[&loc, &j]));
    base.set("old", cost_rows_kept(&j, &loc));
    base
}

/// 会话记录的种类：Claude 的主会话、子代理，Codex 的
#[derive(Clone, Copy, PartialEq)]
enum FileKind {
    Main,
    Sub,
    Rollout,
}

/// 读一遍会话记录里新增的部分，并进 j（scan.json 的内容，原地改）。第一次全读（j 的 done 还是 False）时按全部记录去重、
/// 不管多早；之后比 COST_SEEN 更早的回复不算（去重的记录已经清掉了：多半是改了名、复制出来的文件里读过的）
struct CostScan {
    j: Dict,
    now: f64,
    first: bool,
    cut: f64,
    records: i64,
    bytes: i64,
    files: i64,
    mains: std::collections::HashSet<String>,
    /// 这一遍看到的会话记录里，各工具（claude、codex）最早的修改时间（cost_keep_old 用）
    oldest: Vec<(&'static str, f64)>,
}

/// 一行里的东西对不上（原来是 TypeError / AttributeError / KeyError / IndexError，和 ValueError 一起被接住）：跳过这一行
fn bad_line<T>() -> R<T> {
    Err(Fail::Value("这一行的格式不对".into()))
}

/// `json.loads(raw)`：一行（字节）解析成对象；不是对象的后面 `.get` 是 AttributeError（跳过这一行）
fn line_doc(raw: &[u8]) -> R<Dict> {
    let raw = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
    let Ok(text_) = std::str::from_utf8(raw) else { return bad_line() };
    match json::loads(text_)? {
        V::Dict(d) => Ok(d),
        _ => bad_line(),
    }
}

/// 字节串里有没有这一段
fn has(raw: &[u8], needle: &[u8]) -> bool {
    raw.windows(needle.len()).any(|w| w == needle)
}

/// `8 <= len(u) <= 64` 的字符串
fn ident_str(u: Option<V>) -> Option<V> {
    u.filter(|u| u.as_str().is_some_and(|u| (8..=64).contains(&text::len(u))))
}

impl CostScan {
    fn new(j: &Dict, now: f64) -> CostScan {
        let first = !j.at("done").truthy();
        CostScan { j: j.clone(), now, first, cut: now - COST_SEEN, records: 0, bytes: 0, files: 0, mains: Default::default(), oldest: Vec::new() }
    }

    fn add(&mut self, tool: &str, who: &V, ts: Option<f64>, key: &str, micro: Option<f64>, tok: [i64; 4], model: &str) {
        let seen = self.j.dict("seen").expect("scan.json 核对过");
        let Some(ts) = ts else { return };
        if !(1e9..=self.now + 2.0 * 86400.0).contains(&ts) || (!self.first && ts < self.cut) || seen.has(key) {
            return;
        }
        let hour = ((ts / 3600.0).floor() * 3600.0) as i64;
        seen.set(key, hour);
        let ident = if who.truthy() { format!("{}:{}", tool, who.py_str()) } else { String::new() };
        let rows = if ident.is_empty() {
            self.j.dict("orphan").expect("scan.json 核对过")
        } else {
            self.j.dict("h").expect("scan.json 核对过").setdefault(&ident, Dict::new()).as_dict().expect("scan.json 核对过")
        };
        let b = rows.setdefault(&hour.to_string(), list![0, 0, 0, 0, 0, 0, 0]).as_list().expect("scan.json 核对过");
        let bump = |i: usize, by: i64| b.set(i, b.at(i as isize).as_int().unwrap_or(0) + by);
        bump(1, 1);
        match micro {
            None => {
                bump(6, 1);
                if !ident.is_empty() && !model.is_empty() {
                    // 没有标价的模型：记最后一次见到它的小时（过了 COST_KEEP 就不列）
                    let ms = self.j.dict("um").expect("scan.json 核对过").setdefault(&ident, Dict::new()).as_dict().expect("scan.json 核对过");
                    let last = ms.get(model).and_then(|x| x.as_int()).unwrap_or(hour);
                    ms.set(model, last.max(hour));
                }
            }
            Some(micro) => bump(0, num::trunc(micro + 0.5)),
        }
        for (i, n) in tok.iter().enumerate() {
            bump(2 + i, *n);
        }
        self.records += 1;
    }

    /// 一个文件新增的完整的行
    fn file(&mut self, kind: FileKind, path_: &str, sid: &str) -> R<()> {
        if kind == FileKind::Main {
            self.mains.insert(sid.to_string());
        }
        // 读的时候被删掉、读不了（OSError）：下次再说
        let Ok(mut f) = std::fs::File::open(path_) else { return Ok(()) };
        // Python 的 open() 不开目录（IsADirectoryError）；系统的 open 开得了，读的时候才出错
        if f.metadata().map_or(true, |m| m.is_dir()) {
            return Ok(());
        }
        match self.read_file(&mut f, kind, path_, sid) {
            Err(e) if e.is_os() => Ok(()),
            other => other,
        }
    }

    fn read_file(&mut self, f: &mut std::fs::File, kind: FileKind, path_: &str, sid: &str) -> R<()> {
        let st = f.metadata()?;
        self.files += 1;
        let tool = if kind == FileKind::Rollout { "codex" } else { "claude" };
        let mtime = shutil::st_mtime(&st);
        match self.oldest.iter_mut().find(|(t, _)| *t == tool) {
            Some((_, m)) => *m = m.min(mtime),
            None => self.oldest.push((tool, mtime)),
        }
        let (ino, dev, size) = (st.ino() as i64, st.dev() as i64, st.size() as i64);
        let files = self.j.dict("files").expect("scan.json 核对过");
        let mut cur = None;
        let mut failed = None;
        let mut tail_at = |f: &mut std::fs::File, off: i64| match cost_tail(f, off) {
            Ok(t) => t,
            Err(e) => {
                failed = Some(e);
                String::new()
            }
        };
        if let Some(c) = files.dict(path_) {
            let same = |k: &str, n: i64| c.get(k).is_some_and(|v| v.py_eq(&V::Int(n)));
            if let Some(V::Int(off)) = c.get("off").filter(|_| same("ino", ino) && same("dev", dev)) {
                // 读不了（OSError）：这个文件这一遍不读了，记的位置照旧
                if 0 < off && off <= size && c.get("fp").is_some_and(|fp| fp.as_str() == Some(tail_at(f, off).as_str())) {
                    cur = Some(c);
                }
            }
        }
        if let Some(e) = failed {
            return Err(e.into());
        }
        // 新的、换了、改写过：从头读（读过的靠去重不重复计）
        let cur = cur.unwrap_or_else(|| dict! { "ino" => ino, "dev" => dev, "off" => 0 });
        files.set(path_, &cur);
        let start = cur.at("off").as_int().unwrap_or(0);
        if start == size {
            return Ok(());
        }
        f.seek(SeekFrom::Start(start as u64))?;
        let mut pos = start;
        let mut reader = std::io::BufReader::with_capacity(256 * 1024, &mut *f);
        let mut raw = Vec::new();
        loop {
            raw.clear();
            if reader.read_until(b'\n', &mut raw)? == 0 || raw.last() != Some(&b'\n') {
                break; // 还没写完的一行：下次再读
            }
            pos += raw.len() as i64;
            let done = if kind == FileKind::Rollout { self.codex_line(&cur, &raw) } else { self.claude_line(kind, &cur, sid, &raw) };
            match done {
                // 坏的一行：跳过
                Err(e) if e.is_value() => {}
                other => other?,
            }
        }
        drop(reader);
        self.bytes += pos - start;
        cur.set("off", pos);
        cur.set("fp", cost_tail(f, pos)?);
        Ok(())
    }

    fn claude_line(&mut self, kind: FileKind, cur: &Dict, sid: &str, raw: &[u8]) -> R<()> {
        if raw.starts_with(b"{\"type\":\"session-owner\"") && raw.len() < 600 {
            let d = line_doc(raw)?;
            let owner = d.get("type").is_some_and(|t| t.as_str() == Some("session-owner"));
            let u = ident_str(d.get("accountUuid").filter(|_| owner));
            cur.set("own", u.clone());
            if kind != FileKind::Main {
                return Ok(());
            }
            // 主会话：owners 里先有它这一项（没认出账号也有，是空的）
            let tl = self.j.dict("owners").expect("scan.json 核对过").setdefault(sid, List::new()).as_list().expect("scan.json 核对过");
            if let Some(u) = u {
                // tl[-1][1] != u：上一项不是 [时间, 账号] 的样子原来是 IndexError / TypeError（跳过这一行）
                let changed = match tl.items().last() {
                    None => true,
                    Some(V::List(last)) if last.len() >= 2 => !last.at(1).py_eq(&u),
                    Some(V::Str(last)) if text::len(last) >= 2 => text::slice(last, 1, 2) != u.as_str().unwrap_or(""),
                    Some(_) => return bad_line(),
                };
                if changed {
                    // 从这条回复以后（子代理的记录按时间认）
                    tl.push(list![cur.get("ts").filter(|t| t.truthy()).unwrap_or(V::Int(0)), u]);
                }
            }
            return Ok(());
        }
        if !has(raw, b"\"usage\"") || !has(raw, b"\"assistant\"") {
            return Ok(());
        }
        let d = line_doc(raw)?;
        let m = if d.get("type").is_some_and(|t| t.as_str() == Some("assistant")) { d.get("message") } else { None };
        let Some(m) = m.and_then(|m| m.as_dict()) else { return Ok(()) };
        let Some(u) = m.dict("usage") else { return Ok(()) };
        if m.get("model").is_some_and(|x| x.as_str() == Some("<synthetic>")) {
            return Ok(()); // <synthetic>：Claude Code 自己写的（出错提示之类），没有请求
        }
        let Some(ts) = cost_ts(d.get("timestamp").as_ref())? else { return Ok(()) };
        let key = match m.get("id") {
            Some(V::Str(mid)) => format!("c{}", &sha1_hex(format!("{}\0{}", mid, d.get("requestId").unwrap_or(V::None).py_str()))[..16]),
            _ => format!("c{}", &sha1_hex(raw)[..16]),
        };
        let mut who = cur.get("own").unwrap_or(V::None);
        if kind == FileKind::Main {
            cur.set("ts", ts);
        } else if !who.truthy() {
            // for at, x in self.j['owners'].get(sid) or []：项不是两样东西、时间比不了原来是 ValueError / TypeError（跳过这一行）
            let tl = match self.j.dict("owners").expect("scan.json 核对过").get(sid).filter(|t| t.truthy()) {
                None => Vec::new(),
                Some(V::List(tl)) => tl.items(),
                Some(_) => return bad_line(),
            };
            for entry in tl {
                let Some(pair) = entry.as_list().filter(|p| p.len() == 2) else { return bad_line() };
                let Some(at) = pair.at(0).as_pynum() else { return bad_line() };
                if at <= ts {
                    who = pair.at(1);
                }
            }
        }
        let model = cost_model(m.get("model").as_ref());
        let (micro, tok) = cost_claude(&model, &u);
        self.add("claude", &who, Some(ts), &key, micro, tok, &model);
        Ok(())
    }

    fn codex_line(&mut self, cur: &Dict, raw: &[u8]) -> R<()> {
        let head = &raw[..raw.len().min(160)];
        if has(head, b"\"token_count\"") {
            if cur.truthy("skip") {
                return Ok(());
            }
            let d = line_doc(raw)?;
            let p = d.dict("payload");
            let info = p.filter(|p| p.get("type").is_some_and(|t| t.as_str() == Some("token_count"))).and_then(|p| p.dict("info"));
            let Some(info) = info else { return Ok(()) };
            let tot = json::dumps(&info.get("total_token_usage").unwrap_or(V::None), json::Fmt::new().compact().sort_keys());
            if cur.get("tot").is_some_and(|t| t.as_str() == Some(tot.as_str())) {
                return Ok(()); // 累计的用量没变：同一次请求又报了一遍（只是额度更新了）
            }
            cur.set("tot", tot.as_str());
            let stamp = d.get("timestamp");
            let (last, ts) = (info.dict("last_token_usage"), cost_ts(stamp.as_ref())?);
            let (Some(last), Some(ts)) = (last, ts) else { return Ok(()) };
            let key = format!("x{}", &sha1_hex(format!("{}\0{}", stamp.unwrap_or(V::None).py_str(), tot))[..16]);
            let model = cur.get("model").filter(|m| m.truthy()).map_or(String::new(), |m| m.py_str());
            let tier = cur.get("tier");
            let (micro, tok) = cost_codex(&model, tier.as_ref().and_then(|t| t.as_str()), &last);
            self.add("codex", &cur.get("who").unwrap_or(V::None), Some(ts), &key, micro, tok, &model);
        } else if has(head, b"\"session_meta\"") {
            if cur.has("meta") {
                return Ok(()); // 后面的 session_meta 是复制进来的历史（续接、分叉）
            }
            let d = line_doc(raw)?;
            let p = if d.get("type").is_some_and(|t| t.as_str() == Some("session_meta")) { d.dict("payload") } else { None };
            let Some(p) = p else { return Ok(()) };
            let prov = p.get("model_provider");
            cur.set("meta", 1);
            cur.set("who", ident_str(p.get("creator_user_id")));
            // 第三方的接口（cca 的 API 账号也是）：不是官方账号的用量
            cur.set("skip", prov.as_ref().and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty() && x != "openai"));
        } else if has(head, b"\"turn_context\"") {
            let d = line_doc(raw)?;
            let p = d.dict("payload");
            if let Some(p) = p.filter(|_| d.get("type").is_some_and(|t| t.as_str() == Some("turn_context"))) {
                if let Some(model) = p.get("model").filter(|m| m.is_str()) {
                    cur.set("model", cost_model(Some(&model)));
                }
            }
        } else if has(head, b"\"thread_settings_applied\"") {
            let d = line_doc(raw)?;
            let p = d.dict("payload");
            let s = p.filter(|p| p.get("type").is_some_and(|t| t.as_str() == Some("thread_settings_applied"))).and_then(|p| p.dict("thread_settings"));
            if let Some(s) = s {
                if let Some(model) = s.get("model").filter(|m| m.is_str()) {
                    cur.set("model", cost_model(Some(&model)));
                }
                cur.set("tier", s.get("service_tier").filter(|t| t.is_str()));
                if let Some(prov) = s.get("model_provider_id").filter(|p| p.as_str().is_some_and(|p| !p.is_empty())) {
                    cur.set("skip", prov.as_str() != Some("openai"));
                }
            }
        }
        Ok(())
    }
}

/// `glob` 的 `*`：目录里不以点开头的名字（名字不是 UTF-8 的跳过）；读不了是空的
fn visible(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| !n.starts_with('.')).collect()
}

/// `*.jsonl`
fn is_jsonl(name: &str) -> bool {
    name.ends_with(".jsonl")
}

/// `glob.glob(root/**/*.jsonl, recursive=True)`：root 和它下面各层（不以点开头的）目录里的 *.jsonl。跟着链接进目录；
/// 链接绕成圈的时候原来一直绕到路径太长为止，这里最多 64 层
fn rglob_jsonl(dir: &str, depth: usize, out: &mut Vec<String>) {
    for name in visible(dir) {
        let p = path::join(dir, &name);
        if is_jsonl(&name) {
            out.push(p.clone());
        }
        if depth < 64 && path::isdir(&p) {
            rglob_jsonl(&p, depth + 1, out);
        }
    }
}

/// 建立的先后（macOS 上是建立时间，没有的系统用修改时间；读不了的排在最前面），一样时按路径
fn born(p: &str) -> f64 {
    let Ok(st) = std::fs::metadata(p) else { return 0.0 };
    #[cfg(target_os = "macos")]
    let birth = {
        use std::os::macos::fs::MetadataExt as _;
        shutil::stat_time(st.st_birthtime(), st.st_birthtime_nsec())
    };
    #[cfg(not(target_os = "macos"))]
    let birth = 0.0;
    if birth != 0.0 {
        birth
    } else {
        shutil::st_mtime(&st)
    }
}

fn by_birth(mut paths: Vec<String>) -> Vec<String> {
    let mut keyed: Vec<(f64, String)> = paths.drain(..).map(|p| (born(&p), p)).collect();
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, p)| p).collect()
}

/// 要读的会话记录：[(种类, 路径, 会话 id)]。Claude 的主会话在前（子代理的记录按所在会话的账号认），各按建立的先后
fn cost_files() -> Vec<(FileKind, String, String)> {
    let root = path::join(&claude().master, "projects");
    let (mut mains, mut subs) = (Vec::new(), Vec::new());
    for project in visible(&root) {
        let pdir = path::join(&root, &project);
        if !path::isdir(&pdir) {
            continue;
        }
        for name in visible(&pdir) {
            let p = path::join(&pdir, &name);
            if is_jsonl(&name) {
                mains.push(p.clone());
            }
            if path::isdir(&p) {
                let sdir = path::join(&p, "subagents");
                subs.extend(visible(&sdir).into_iter().filter(|n| is_jsonl(n)).map(|n| path::join(&sdir, &n)));
            }
        }
    }
    let mut out = Vec::new();
    for p in by_birth(mains) {
        let name = path::basename(&p);
        let sid = text::head(name, text::len(name).saturating_sub(6)).to_string();
        out.push((FileKind::Main, p, sid));
    }
    for p in by_birth(subs) {
        let sid = path::basename(&path::dirname(&path::dirname(&p))).to_string();
        out.push((FileKind::Sub, p, sid));
    }
    // 归档是把文件挪过去：路径变了，重读的靠去重不重复计
    for d in ["sessions", "archived_sessions"] {
        let mut found = Vec::new();
        rglob_jsonl(&path::join(&codex().master, d), 0, &mut found);
        found.sort();
        out.extend(found.into_iter().map(|p| (FileKind::Rollout, p, String::new())));
    }
    out
}

/// 读新增的会话记录、并进 j；checkpoint 隔一会儿调一次（第一次全读可能要好一会儿：先存一次，被打断了下次接着读）
fn cost_scan(j: &Dict, now: f64, checkpoint: &mut dyn FnMut() -> R<()>) -> R<CostScan> {
    let mut s = CostScan::new(j, now);
    let mut live = std::collections::HashSet::new();
    for (kind, p, sid) in cost_files() {
        s.file(kind, &p, &sid)?;
        live.insert(p);
        checkpoint()?;
    }
    let files = j.dict("files").expect("scan.json 核对过");
    for p in files.keys() {
        if !live.contains(&*p) {
            files.pop(&p);
        }
    }
    let owners = j.dict("owners").expect("scan.json 核对过");
    for sid in owners.keys() {
        if !s.mains.contains(&*sid) {
            owners.pop(&sid);
        }
    }
    cost_keep_old(j, &s);
    j.set("done", true);
    Ok(s)
}

/// scan.json 只留 COST_KEEP 的小时数据、COST_SEEN 的去重记录
fn cost_prune(j: &Dict, now: f64) {
    let lo = now - COST_KEEP;
    let h = j.dict("h").expect("scan.json 核对过");
    let mut tables: Vec<Dict> = h.values().iter().filter_map(|r| r.as_dict()).collect();
    tables.push(j.dict("orphan").expect("scan.json 核对过"));
    for rows in tables {
        for k in rows.keys() {
            if hour_of(&k) < lo {
                rows.pop(&k);
            }
        }
    }
    let kept = Dict::new();
    for (i, r) in h.items() {
        if r.truthy() {
            kept.set(i, r);
        }
    }
    j.set("h", kept);
    let lo_seen = now - COST_SEEN - 3600.0;
    let seen = Dict::new();
    for (k, v) in j.dict("seen").expect("scan.json 核对过").items() {
        if v.as_num().is_some_and(|v| v >= lo_seen) {
            seen.set(k, v);
        }
    }
    j.set("seen", seen);
    let um = Dict::new();
    for (i, ms) in j.dict("um").expect("scan.json 核对过").items() {
        let recent = Dict::new();
        for (m, x) in ms.as_dict().expect("scan.json 核对过").items() {
            if x.as_num().is_some_and(|x| x >= lo) {
                recent.set(m, x);
            }
        }
        if !recent.is_empty() {
            um.set(i, recent);
        }
    }
    j.set("um", um);
}

/// Claude Code 的 .claude.json 里登录的账号：(accountUuid, 邮箱)；读不出是 (None, None)
fn oauth_ident(p: &str) -> (Option<V>, Option<V>) {
    let oa = load_quiet(p).and_then(|j| j.as_dict()).and_then(|j| j.dict("oauthAccount")).unwrap_or_default();
    (oa.get("accountUuid"), oa.get("emailAddress"))
}

/// `codex_claims(key, path)` 里的 (user, email)。codex_claims 在 usage+codex 那一批里移植，合进来以后换成它
fn codex_claims_pair(_key: Option<&str>, _path: Option<&str>) -> (Option<V>, Option<V>) {
    unimplemented!("codex_claims 还没合进来（usage+codex 那一批）：有 Codex 账号、或者 ~/.codex/auth.json 时折合金额先统计不了")
}

/// 这台设备上登录过的账号身份 → 邮箱（小写），记进 j 的 who：cca 的账号目录里的，和不经 cca 直接登录的（~/.claude.json、
/// ~/.codex/auth.json；只读 id_token 里的邮箱和用户 id）。以前记下的留着（退出登录了，以前的用量照样认得出是谁的）
fn cost_who(j: &Dict) {
    let mut found: Vec<(&str, Option<V>, Option<V>)> = Vec::new();
    for tool in [claude(), codex()] {
        let table = if path::exists(&tool.table) { load_quiet(&tool.table).and_then(|t| t.as_dict()) } else { None };
        let _using = Using::tool(tool);
        for (k, v) in table.map(|t| t.items()).unwrap_or_default() {
            if is_api(&v) {
                continue;
            }
            if tool.kind == Kind::Claude {
                let (u, e) = oauth_ident(&path::join(&cdir(&k), ".claude.json"));
                found.push(("claude", u, e));
            } else {
                let (u, e) = codex_claims_pair(Some(&k), None);
                found.push(("codex", u, e));
            }
        }
    }
    let (u, e) = oauth_ident(&MASTER_JSON);
    found.push(("claude", u, e));
    let auth = path::join(&codex().master, "auth.json");
    // 没有这个文件时 codex_claims 给的是空的：不用等它合进来
    let (u, e) = if path::exists(&auth) { codex_claims_pair(None, Some(&auth)) } else { (None, None) };
    found.push(("codex", u, e));
    let who = j.dict("who").expect("scan.json 核对过");
    for (tool, ident, email) in found {
        let ident = ident.as_ref().and_then(|i| i.as_str()).filter(|i| (8..=64).contains(&text::len(i)));
        let email = email.as_ref().and_then(|e| e.as_str()).filter(|e| EMAIL_RE.matches(e));
        if let (Some(ident), Some(email)) = (ident, email) {
            who.set(format!("{tool}:{ident}"), email.to_lowercase());
        }
    }
}

/// local.json（_gui state、同步读它）：最近 COST_PUBLISH 的小时数据、身份 → 邮箱、没有标价的模型、认不出是谁的最近 31 天。
/// 内容变了才写（App 看它变了就重读 state）
fn cost_publish(j: &Dict, now: f64) -> R<()> {
    let lo = now - COST_PUBLISH;
    let recent = |rows: &V, lo: f64| -> Dict {
        let out = Dict::new();
        for (k, x) in rows.as_dict().expect("scan.json 核对过").items() {
            if hour_of(&k) >= lo {
                out.set(k, x);
            }
        }
        out
    };
    let acc = Dict::new();
    for (i, rows) in j.dict("h").expect("scan.json 核对过").items() {
        let r = recent(&rows, lo);
        if !r.is_empty() {
            acc.set(i, r);
        }
    }
    let um = Dict::new();
    for (i, ms) in j.dict("um").expect("scan.json 核对过").items() {
        let mut models: Vec<std::rc::Rc<str>> =
            ms.as_dict().expect("scan.json 核对过").items().into_iter().filter(|(_, x)| x.as_num().is_some_and(|x| x >= lo)).map(|(m, _)| m).collect();
        models.sort();
        if !models.is_empty() {
            um.set(i, V::List(List::from_vec(models.into_iter().map(V::from).collect())));
        }
    }
    let doc = dict! {
        "v" => 1, "build" => COST_BUILD, "who" => j.at("who"), "acc" => acc, "um" => um,
        "orphan" => recent(&j.at("orphan"), now - (COST_DAYS + 1) as f64 * 86400.0),
    };
    if !V::Dict(cost_json("local.json")).py_eq(&V::Dict(doc.clone())) {
        cost_save("local.json", &V::Dict(doc))?;
    }
    Ok(())
}

/// cost/ 里的一个空文件，修改时间改成现在（没有就建）
pub fn cost_touch(name: &str) -> std::io::Result<()> {
    let p = path::join(&COST_DIR, name);
    std::fs::OpenOptions::new().append(true).create(true).open(&p)?;
    utime_now(&p)
}

/// cost/ 里这个文件的修改时间；没有、读不了是 None
pub fn cost_mtime(name: &str) -> Option<f64> {
    shutil::getmtime(&path::join(&COST_DIR, name)).ok()
}

/// 另一个统计正在跑、等不到（原来的 CostBusy 异常）
fn cost_busy() -> Fail {
    Fail::Other("CostBusy", Vec::new())
}

/// 统计一次：读新增的会话记录、发布 local.json，再和别的设备同步（到时候了才同步；sync_now 马上）。
/// 同时只有一个（cost.lock）：wait 是最多等正在跑的那个多久，等不到是 CostBusy。返回 (统计的结果, 同步的结果)（PROTOCOL 3.5）
fn cost_run(sync_now: bool, wait: f64) -> R<(Dict, Dict)> {
    cost_dir().map_err(|e| at_path(e, &COST_DIR))?;
    let lock_path = path::join(&COST_DIR, "cost.lock");
    let lk = std::fs::OpenOptions::new().append(true).create(true).open(&lock_path).map_err(|e| at_path(e, &lock_path))?;
    let until = monotonic() + wait;
    // SAFETY: lk 开着，fd 有效
    while unsafe { libc::flock(lk.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if monotonic() >= until {
            return Err(cost_busy());
        }
        sleep(0.5);
    }
    let me = std::process::id() as i64;
    cost_save("worker.json", &V::Dict(dict! { "v" => 1, "pid" => me, "started" => proc_starts(&[me]).get(&me).copied().unwrap_or(0) }))?;
    let run = || -> R<(Dict, Dict)> {
        let now = time();
        let j = cost_load_scan();
        let first = !j.at("done").truthy();
        let mut last = monotonic();
        let mut checkpoint = || -> R<()> {
            if monotonic() - last >= 20.0 {
                cost_save("scan.json", &V::Dict(j.clone()))?;
                last = monotonic();
            }
            Ok(())
        };
        let s = cost_scan(&j, now, &mut checkpoint)?;
        cost_who(&j);
        cost_prune(&j, now);
        cost_save("scan.json", &V::Dict(j.clone()))?;
        cost_publish(&j, now)?;
        let scanned = path::join(&COST_DIR, "scanned");
        cost_touch("scanned").map_err(|e| at_path(e, &scanned))?;
        let scan = dict! {
            "state" => "ready", "updated_epoch" => cost_mtime("scanned"), "files" => s.files, "read_bytes" => s.bytes, "records" => s.records,
            "first" => first,
        };
        Ok((scan, cost_sync(sync_now)?))
    };
    let done = run();
    let _ = std::fs::remove_file(path::join(&COST_DIR, "worker.json"));
    done
}

/// 这台设备这个账号的数据 → 服务器存的那一份 {"n": nonce, "c": 密文, "t": 认证}（base64）。明文是压缩的 JSON，前面 4 个字节
/// 是长度，用零字节补齐到 COST_PAD 的整数倍；认证里带着账号摘要和设备指纹（挪不到别的账号、别的设备上）。长度不省：zlib 虽然
/// 认得出自己在哪结束，有了它读的一方不靠解压器判断补齐从哪开始，照 server/README.md 另写一份也简单
fn cost_seal(keys: &Keys, aid: &str, fp: &str, doc: &Dict) -> R<Dict> {
    let z = zlib::compress(json::dumps(&V::Dict(doc.clone()), json::Fmt::new().compact()).as_bytes(), 9);
    let mut pt = (z.len() as u32).to_be_bytes().to_vec();
    pt.extend_from_slice(&z);
    pt.resize(pt.len().div_ceil(COST_PAD) * COST_PAD, 0);
    Ok(blob_seal(keys, &blob_ad(&[COST_LABEL, aid, fp])?, &pt))
}

/// 别的设备传来的明文：{"v": 1, "at": 时间, "h": {小时: [7 个计数]}, "um": [没有标价的模型…]}；格式不对是 None
fn cost_doc(d: &V, now: f64) -> Option<Dict> {
    let d = d.as_dict()?;
    let at = d.get("at").filter(is_num)?.as_num()?;
    if !(d.get("v").is_some_and(|v| v.py_eq(&V::Int(1))) && 0.0 < at && at <= now + 86400.0) {
        return None;
    }
    let (h, um) = (d.get("h"), d.list("um")?);
    if !(cost_rows_ok(h.as_ref()) && h.as_ref()?.as_dict()?.len() <= 24 * 7 * 20) {
        return None;
    }
    let models: Vec<V> = um.items().into_iter().filter(|m| m.as_str().is_some_and(|m| text::isprintable(m) && text::len(m) <= 64)).take(32).collect();
    Some(dict! { "at" => at, "h" => h, "um" => models })
}

/// 服务器上别的设备的那一份解开；认证不过、格式不对（伪造的、别的账号的、坏了）是 None
fn cost_open(keys: &Keys, aid: &str, fp: &str, blob: &V, now: f64) -> R<Option<Dict>> {
    let Some(pt) = blob_open(keys, &blob_ad(&[COST_LABEL, aid, fp])?, blob) else { return Ok(None) };
    let n = pt.get(..4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let body = pt.get(4..).unwrap_or(&[]);
    let Some((raw, tail)) = zlib::decompress_limited(&body[..n.min(body.len())], 4 << 20) else { return Ok(None) };
    let Some(doc) = std::str::from_utf8(&raw).ok().and_then(|t| json::loads(t).ok()) else { return Ok(None) };
    Ok(if n <= body.len() && !tail { cost_doc(&doc, now) } else { None })
}

/// 这台设备上这个账号的小时数据：这个工具的、登录的邮箱是它的身份加起来（新的列表，可以改）；和没有标价的模型
pub fn cost_content(local: &Dict, tool: &Tool, email: &str) -> (Dict, Vec<String>) {
    let who = cost_who_kept(&[local]);
    let h = Dict::new();
    let mut um: Vec<String> = Vec::new();
    let prefix = format!("{}:", tool.name);
    // (local.get('acc') or {}).items()
    let acc = match local.get("acc").filter(|a| a.truthy()) {
        None => Vec::new(),
        Some(V::Dict(acc)) => acc.items(),
        Some(other) => panic!("AttributeError: local.json 的 acc 是 {}", other.py_repr()),
    };
    for (ident, rows) in acc {
        if ident.starts_with(&prefix) && who.get(&ident).is_some_and(|e| e.as_str() == Some(email)) && cost_rows_ok(Some(&rows)) {
            for (k, r) in rows.as_dict().expect("cost_rows_ok 核对过").items() {
                let r = r.as_list().expect("cost_rows_ok 核对过");
                let b = h.setdefault(&k, list![0, 0, 0, 0, 0, 0, 0]).as_list().expect("刚放进去的");
                for i in 0..7 {
                    b.set(i, b.at(i as isize).as_int().unwrap_or(0) + r.at(i as isize).as_int().unwrap_or(0));
                }
            }
            // (local.get('um') or {}).get(ident) or []
            let models = match local.get("um").filter(|u| u.truthy()) {
                None => None,
                Some(V::Dict(u)) => u.get(&ident).filter(|m| m.truthy()),
                Some(other) => panic!("AttributeError: local.json 的 um 是 {}", other.py_repr()),
            };
            let models = match models {
                None => Vec::new(),
                Some(V::List(m)) => m.items(),
                Some(V::Dict(m)) => m.keys().into_iter().map(V::from).collect(),
                Some(V::Str(m)) => m.chars().map(|c| V::str(c.to_string())).collect(),
                Some(other) => panic!("TypeError: local.json 的 um 里是 {}", other.py_repr()),
            };
            for m in models {
                if let Some(m) = m.as_str().filter(|m| !um.iter().any(|x| x == m)) {
                    um.push(m.to_string());
                }
            }
        }
    }
    um.sort();
    (h, um)
}

/// 要同步的账号：(工具, 邮箱小写, slot（工具名:邮箱，sync.json、remote.json 里的键）, 账号摘要, 密钥)
struct Item {
    tool: &'static Tool,
    email: String,
    slot: String,
    aid: String,
    keys: Keys,
}

/// 要同步的账号：映射里的 Claude、Codex 官方账号，这台设备登录着（没登录的解不开，也不传）。账号表读不了是 None
fn cost_items(cfg: &Dict) -> R<Option<Vec<Item>>> {
    let Some(targets) = presence_targets() else { return Ok(None) };
    let salt = cfg.at("salt").py_str();
    let mut out = Vec::new();
    for (tool, key, _) in targets {
        if !matches!(tool.kind, Kind::Claude | Kind::Codex) {
            continue;
        }
        let ident = {
            let _using = Using::tool(tool);
            share_ident(tool, &key)
        };
        let email = parse_key(&key).1.to_lowercase();
        if let Some(ident) = ident.filter(|_| !email.is_empty()) {
            out.push(Item {
                tool,
                slot: format!("{}:{}", tool.name, email),
                aid: presence_aid(&salt, tool.name, &email)?,
                keys: keyshare_keys(&salt, tool.name, &email, &ident, COST_LABEL)?,
                email,
            });
        }
    }
    Ok(Some(out))
}

/// 和别的设备交换（见上面这一节的说明）。force 以外最多 COST_SYNC_EVERY 一次、旧的服务器 COST_RETRY 以后再试。
/// 返回 {"state", "message", "checked_epoch"}（PROTOCOL 3.5 的 sync）
fn cost_sync(force: bool) -> R<Dict> {
    let cfg = presence_load()?.1;
    let st = Some(cost_json("sync.json")).filter(|s| !s.is_empty()).unwrap_or_else(|| dict! { "v" => 1 });
    let res = |state: &str, msg: Option<V>| -> Dict { dict! { "state" => state, "message" => msg, "checked_epoch" => st.get("ok_at").filter(is_num) } };
    let Some(cfg) = cfg else { return Ok(res("skipped", None)) };
    let now = time();
    let num_ = |k: &str| st.get(k).filter(is_num).and_then(|v| v.as_num()).unwrap_or(0.0);
    // 在未来的（时钟快过）不当真
    if !force && ((0.0..COST_SYNC_EVERY).contains(&(now - num_("at"))) || (0.0 < num_("retry") - now && num_("retry") - now <= COST_RETRY)) {
        let bad = st.get("state").is_some_and(|s| matches!(s.as_str(), Some("error") | Some("old_server")));
        let state = if bad {
            "waiting"
        } else if st.truthy("ok_at") {
            "ok"
        } else {
            "skipped"
        };
        return Ok(res(state, if bad { st.get("error") } else { None }));
    }
    let Some(items) = cost_items(&cfg)? else { return Ok(res("error", Some(V::from("读不了账号表")))) };
    if items.is_empty() {
        return Ok(res("skipped", None));
    }
    let rem = Some(cost_json("remote.json")).filter(|r| !r.is_empty()).unwrap_or_else(|| dict! { "v" => 1, "acc" => Dict::new() });
    if rem.dict("acc").is_none() {
        rem.set("acc", Dict::new());
    }
    let old_rem = V::Dict(rem.clone()).deepcopy();
    st.set("at", now);
    let err = cost_round(&cfg, &items, &cost_json("local.json"), &st, &rem, now)?;
    st.set("state", if err == COST_OLD { "old_server" } else if !err.is_empty() { "error" } else { "ok" });
    st.set("error", if err.is_empty() { V::None } else { V::from(err.as_str()) });
    if err == COST_OLD {
        st.set("retry", now + COST_RETRY);
    } else {
        st.pop("retry");
    }
    if err.is_empty() {
        st.set("ok_at", now);
    }
    if !V::Dict(rem.clone()).py_eq(&old_rem) {
        cost_save("remote.json", &V::Dict(rem.clone()))?;
    }
    cost_save("sync.json", &V::Dict(st.clone()))?;
    Ok(res(&st.at("state").py_str(), Some(st.at("error"))))
}

/// `d.setdefault(k, {})`，里面不是对象的原来后面是 AttributeError / TypeError
fn sub_dict(d: &Dict, k: &str) -> Dict {
    match d.setdefault(k, Dict::new()) {
        V::Dict(x) => x,
        other => panic!("AttributeError: {k} 那一项是 {}，不是对象", other.py_repr()),
    }
}

/// 一轮：问服务器这些账号各台设备的（带着这台设备已经有的认证码：没变的不回），解开、存进 rem（remote.json 的内容）；这台设备的
/// 变了、服务器上没有或者不是上次传的、隔了 COST_BEAT 就传上去。st（sync.json 的内容）原地改。返回没成的原因（成了是空的）
fn cost_round(cfg: &Dict, items: &[Item], local: &Dict, st: &Dict, rem: &Dict, now: f64) -> R<String> {
    let (accs, racc) = (sub_dict(st, "acc"), rem.dict("acc").expect("cost_sync 保证了是字典"));
    let get: Vec<V> = items.iter().map(|x| V::from(x.aid.as_str())).collect();
    let (code, r) = presence_post(cfg, "ledger", &dict! { "get" => get, "have" => cost_have(cfg, items, &accs, &racc) }, 20)?;
    if code == 404 || code == 413 {
        // 旧的服务器：没有 /v1/ledger（body 超过它的 64 KB 上限时先回 413；早先试过的 /v1/cost 是另一种格式，不用它）
        return Ok(COST_OLD.to_string());
    }
    let Some(cost) = r.dict("cost").filter(|_| code == 200) else {
        return Ok(if code != 200 { presence_why(cfg, code, &r)? } else { "设备统计的服务器回的内容看不懂".to_string() });
    };
    if let Some(me) = r.str("me_name").filter(|n| text::isprintable(n)) {
        st.set("me", text::head(&me, 64));
    }
    let missing = r.dict("missing").unwrap_or_default();
    let fp = cfg.at("fp").py_str();
    let puts = Dict::new();
    let mut sent: Vec<(String, V, String)> = Vec::new();
    for x in items {
        let a = sub_dict(&accs, &x.slot);
        let old = match racc.get(&x.slot).filter(|o| o.truthy()) {
            None => Dict::new(),
            Some(V::Dict(o)) => o,
            Some(other) => panic!("AttributeError: remote.json 里 {} 那一项是 {}", x.slot, other.py_repr()),
        };
        let (new, mine) = cost_take(cfg, cost.get(&x.aid), &x.keys, &x.aid, &old, now)?;
        if new.is_empty() {
            racc.pop(&x.slot);
        } else {
            racc.set(x.slot.as_str(), new);
        }
        a.set("missing", cost_missing_view(missing.get(&x.aid), now));
        let (h, um) = cost_content(local, x.tool, &x.email);
        let recent = Dict::new();
        for (k, v) in h.items() {
            if hour_of(&k) >= now - COST_PUBLISH {
                recent.set(k, v);
            }
        }
        let doc = dict! { "h" => recent, "um" => V::List(List::from_vec(um.iter().map(|m| V::from(m.as_str())).collect())) };
        let s = sha256_hex(json::dumps(&V::Dict(doc.clone()), json::Fmt::new().compact().sort_keys()))[..16].to_string();
        let up = a.get("up_at").filter(is_num).and_then(|u| u.as_num()).unwrap_or(0.0);
        let same_tag = |mine: &Dict| match (mine.get("tag"), a.get("tag")) {
            (None, None) => true,
            (Some(m), Some(t)) => m.py_eq(&t),
            (Some(v), None) | (None, Some(v)) => v.is_none(),
        };
        let unchanged = mine.as_ref().is_some_and(same_tag) && a.get("sum").is_some_and(|x| x.as_str() == Some(s.as_str())) && (0.0..COST_BEAT).contains(&(now - up));
        if !unchanged {
            let blob = cost_seal_fit(&x.keys, &x.aid, &fp, &doc, now)?;
            sent.push((x.slot.clone(), blob.at("t"), s));
            puts.set(x.aid.as_str(), blob);
        }
    }
    let err = cost_put(cfg, &puts)?;
    if !err.is_empty() {
        return Ok(err);
    }
    for (slot, tag, s) in sent {
        let a = sub_dict(&accs, &slot);
        a.set("tag", tag);
        a.set("sum", s);
        a.set("up_at", now);
    }
    for x in items {
        sub_dict(&accs, &x.slot).set("checked", now);
    }
    Ok(String::new())
}

/// 问服务器时带上的：每个账号这台设备已经有的各台设备的认证码（没变的服务器就不回密文），和这台设备自己上次传的
fn cost_have(cfg: &Dict, items: &[Item], accs: &Dict, racc: &Dict) -> Dict {
    let have = Dict::new();
    for x in items {
        let tags = Dict::new();
        let known = match racc.get(&x.slot).filter(|o| o.truthy()) {
            None => Vec::new(),
            Some(V::Dict(o)) => o.items(),
            Some(other) => panic!("AttributeError: remote.json 里 {} 那一项是 {}", x.slot, other.py_repr()),
        };
        for (fp, e) in known {
            if let Some(tag) = e.as_dict().and_then(|e| e.get("tag")).filter(|t| t.is_str()) {
                tags.set(fp, tag);
            }
        }
        if let Some(tag) = accs.dict(&x.slot).and_then(|m| m.get("tag")).filter(|t| t.is_str()) {
            tags.set(cfg.at("fp").py_str(), tag);
        }
        have.set(x.aid.as_str(), tags);
    }
    have
}

/// 服务器回的这个账号各台设备的 → ({设备指纹: remote.json 里的一项}, 这台设备自己的那一行 | None)。带着密文的解开
/// （认证不过的不要），没带的（没变，或者这次回得太多没给）用上次的（old）
fn cost_take(cfg: &Dict, rows: Option<V>, keys: &Keys, aid: &str, old: &Dict, now: f64) -> R<(Dict, Option<Dict>)> {
    let (new, mut mine) = (Dict::new(), None);
    let my_fp = cfg.at("fp").py_str();
    for row in rows.and_then(|r| r.as_list()).map(|r| r.items()).unwrap_or_default() {
        let Some(row) = row.as_dict() else { continue };
        let (Some(fp), Some(tag)) = (row.str("fp"), row.str("tag")) else { continue };
        if matches!(row.get("me"), Some(V::Bool(true))) || *fp == *my_fp {
            mine = Some(row);
            continue;
        }
        let prev = old.dict(&fp);
        let e = if let Some(blob) = row.get("blob").filter(|b| b.is_dict()) {
            match cost_open(keys, aid, &fp, &blob, now)? {
                None => continue, // 认证不过（伪造的、坏了）：不要
                Some(doc) => {
                    doc.set("tag", &*tag);
                    doc
                }
            }
        } else if let Some(prev) = prev.filter(|p| !p.is_empty()) {
            prev.copy() // 认证码照旧：没给的下次再取
        } else {
            continue;
        };
        let name = row.str("name").filter(|n| !n.is_empty() && text::isprintable(n));
        e.set("name", name.map_or("?".to_string(), |n| text::head(&n, 64).to_string()));
        e.set("revoked", matches!(row.get("revoked"), Some(V::Bool(true))));
        new.set(fp, e);
    }
    Ok((new, mine))
}

/// 服务器说 30 天内用过这个账号、却没传过折合金额的设备 → [{name, used_epoch}]（最多 32 台）
fn cost_missing_view(ms: Option<V>, now: f64) -> V {
    let mut out = Vec::new();
    for m in ms.and_then(|m| m.as_list()).map(|m| m.items()).unwrap_or_default() {
        let Some(m) = m.as_dict() else { continue };
        let (Some(name), Some(age)) = (m.str("name").filter(|n| text::isprintable(n)), m.get("used_age").filter(is_num).and_then(|a| a.as_num())) else { continue };
        out.push(V::Dict(dict! { "name" => text::head(&name, 64), "used_epoch" => num::round(now - age) }));
        if out.len() == 32 {
            break;
        }
    }
    V::List(List::from_vec(out))
}

/// 封好这台设备这个账号的数据（cost_seal）；太大了（很少见：14 周里小时很多、又压缩不了）就去掉最早的四分之一的小时，
/// 直到放得下 COST_BLOB_MAX
fn cost_seal_fit(keys: &Keys, aid: &str, fp: &str, doc: &Dict, now: f64) -> R<Dict> {
    let doc = doc.copy();
    let seal = |doc: &Dict| -> R<Dict> {
        let full = doc.copy();
        full.set("v", 1);
        full.set("at", now);
        cost_seal(keys, aid, fp, &full)
    };
    let mut blob = seal(&doc)?;
    loop {
        let h = doc.dict("h").expect("调用的地方给的");
        if text::len(&blob.at("c").py_str()) <= COST_BLOB_MAX || h.is_empty() {
            return Ok(blob);
        }
        let mut ks = h.keys();
        ks.sort_by_key(|k| text::to_int(k).unwrap_or(0));
        let kept = Dict::new();
        for k in &ks[ks.len() / 4 + 1..] {
            kept.set(k.clone(), h.at(k));
        }
        doc.set("h", kept);
        blob = seal(&doc)?;
    }
}

/// 把这几份（{账号摘要: 密文}）传上去：一次最多 COST_PUT_MAX，多了分几次。返回没成的原因（成了是空的）
fn cost_put(cfg: &Dict, puts: &Dict) -> R<String> {
    let mut batches = vec![Dict::new()];
    let mut size = 0;
    for (aid, blob) in puts.items() {
        let len = text::len(&blob.as_dict().expect("cost_seal_fit 给的").at("c").py_str());
        if !batches.last().expect("至少有一份").is_empty() && size + len > COST_PUT_MAX {
            batches.push(Dict::new());
            size = 0;
        }
        batches.last().expect("至少有一份").set(aid, blob);
        size += len;
    }
    for batch in batches {
        if batch.is_empty() {
            continue;
        }
        let (code, r) = presence_post(cfg, "ledger", &dict! { "put" => batch }, 30)?;
        if code == 404 {
            return Ok(COST_OLD.to_string());
        }
        if code != 200 {
            return presence_why(cfg, code, &r);
        }
    }
    Ok(String::new())
}

/// 在后台跑一次 cca _cost-sync（读新增的会话记录、和别的设备同步）；上次起过还不到 COST_EVERY 就不起。静默。
/// CCA_COST=off（回归测试）不起
pub fn cost_kick() {
    if getenv("CCA_COST").as_deref() == Some("off") || !path::isfile(&CFG) || !cost_on() {
        return;
    }
    if let Some(stamp) = cost_mtime("last").filter(|s| *s != 0.0) {
        let age = time() - stamp;
        if (0.0..COST_EVERY).contains(&age) {
            return;
        }
    }
    // 建不了目录、碰不了文件（OSError）就算了
    if cost_dir().is_err() || cost_touch("last").is_err() {
        return;
    }
    spawn_background(&["_cost-sync"]);
}

/// cca _cost-sync：cost_kick 在后台起的；有别的统计在跑就走。出什么错都安静地退出（下次再来）
pub fn cmd_cost_worker(_args: &[String]) -> R<()> {
    if !(path::isfile(&CFG) && cost_on()) {
        return Ok(());
    }
    match cost_run(false, 0.0) {
        // except (CostBusy, OSError, ValueError, TypeError, KeyError, SystemExit)
        Err(e) if e.is("CostBusy") || e.is_os_or_value() || e.is_exit() || e.is("TypeError") || e.is("KeyError") => {
            e.swallowed();
            Ok(())
        }
        Err(e) => Err(e),
        Ok(_) => Ok(()),
    }
}

/// 这台设备上 Claude、Codex 的官方账号有几个（读不了的表不算）
fn cost_accounts() -> usize {
    let mut n = 0;
    for tool in [claude(), codex()] {
        if !path::exists(&tool.table) {
            continue;
        }
        if let Some(table) = load_quiet(&tool.table).and_then(|t| t.as_dict()) {
            n += table.values().iter().filter(|v| !is_api(v)).count();
        }
    }
    n
}

/// cca cost sync [--now] [--json]（PROTOCOL 3.5）。退出码：0 统计好了（同步没成也是 0）；1 没统计成；2 用不了。没成时 stdout 是原因
pub fn cmd_cost_sync(args: &[String]) -> R<()> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    if !cost_on() {
        out!("折合金额关掉了（config.json 里写着 \"cost\": false）");
        return exit(2);
    }
    if cost_accounts() == 0 {
        out!("没有 Claude、Codex 的官方账号：没有可统计的");
        return exit(2);
    }
    let (scan, sync) = match cost_run(flag("--now"), COST_WAIT as f64) {
        Ok(done) => done,
        Err(e) if e.is("CostBusy") => {
            out!("另一个统计一直没结束（等了 {COST_WAIT} 秒）");
            return exit(1);
        }
        Err(e) if e.is_os_or_value() || e.is("TypeError") || e.is("KeyError") => {
            out!("统计折合金额没成：{e}");
            return exit(1);
        }
        Err(e) => return Err(e),
    };
    if flag("--json") {
        let doc = dict! { "version" => 1, "kind" => "cost_sync", "generated_epoch" => time(), "scan" => scan, "sync" => sync };
        out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    let tail = match sync.at("state").as_str() {
        Some("ok") => "；和其他设备同步好了".to_string(),
        Some("old_server") => format!("；{COST_OLD}"),
        Some("error") => format!("；和其他设备同步没成：{}", sync.at("message").py_str()),
        _ => String::new(),
    };
    out!("统计好了：看了 {} 个会话记录，新算进来 {} 条{}", scan.at("files").py_str(), scan.at("records").py_str(), tail);
    Ok(())
}

/// cca restore 之前：停掉正在统计的后台进程（它在 ~/.cca 删掉以后也不会写回来，这里只是不让它白跑）
pub fn cmd_cost_stop(args: &[String]) -> R<()> {
    arity("cmd_cost_stop", args, 0, 0)?;
    let j = cost_json("worker.json");
    // isinstance(pid, int)、type(started) is int、started 不是 0
    let Some(V::Int(started)) = j.get("started").filter(|s| s.truthy()) else { return Ok(()) };
    let pid = match j.get("pid") {
        Some(V::Int(pid)) => pid,
        Some(V::Bool(true)) => {
            // 布尔值在 Python 里也算 int：True 当成进程号去问（ps 的参数写成 True，问不到），等于不在
            if !cfg!(target_os = "linux") {
                super::session::ps_lstart("True");
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let alive = || cli_alive(pid, started, &proc_starts(&[pid]));
    if !alive() {
        return Ok(());
    }
    // SAFETY: 只是给一个进程号发信号
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        return Ok(());
    }
    for _ in 0..30 {
        if !alive() {
            return Ok(());
        }
        sleep(0.1);
    }
    Ok(())
}
