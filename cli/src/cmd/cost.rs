//! 折合金额（`cost_model` 到 `CostView`）：cost-worker、cost-sync、cost-stop，和 _gui state 用的视图（`CostView`）。
//!
//! 这台设备上 Claude Code、Codex 的会话记录（共享的 ~/.claude/projects、~/.codex/sessions）里每一次请求的用量，按官方 API 的
//! 标价折成美元，按登录的账号身份（Claude 的 accountUuid、Codex 的 chatgpt_user_id）、按小时累计。不是实际花费（订阅不按这个
//! 收费）。只读用量的数字（token 数、模型、时间、账号身份），不读对话的内容。
//! 会话记录里带账号信息才算得进来：Claude Code 在会话里写 session-owner（2.1.317 起；按位置认：这一行以后的回复都是这个账号的。
//! 换了账号续接时 2.1.330 照旧写原来的账号：cca 记的会话开始比它新时按 cca 记的认，见 Marks），子代理的记录按它所在的会话认；
//! Codex 的 session_meta 写 creator_user_id（0.157 起；一个文件里第一个 session_meta 是它自己的，后面的是复制进来的历史），用量取
//! token_count 事件。更早的 Claude Code 里不经 cca 启动的会话算不进来，Claude 桌面 App 里的会话要开着 cca cost hook（见
//! cmd_cost_hook）；Codex 续接别的账号建的会话时，用量记在建它的账号上。
//! 增量地读：每个文件记着读到哪（只读完整的行；文件被截短、换了、读过的末尾对不上就从头读），回复按消息 id 去重，Codex 的
//! 按时间和累计的用量去重。
//! Team 给的官方账号、开着设备统计时，各台设备算的经那个 Team 的同步服务器交换（op ledger，见 server/README.md）：一台设备
//! 一份、只写自己的那一份，服务器只存密文。密钥整个 Team 共用（3.6.6 起只由 Team 的盐派生，不用登录：Team 给了这个账号的设备
//! 都看得到各台设备的；服务器没有盐，解不开）；3.6.5 及以前的还带着登录身份（和共享 API key 一样），这台设备登录着时照样解得开。
//! 从 Team 里移除了的设备传的，服务器删掉（以前的服务器还回来、标着 revoked 的不要）。
//! 统计在后台进程里做（cca _cost-sync：会话结束、cca usage、cca devices、上报进程里，最多 COST_EVERY 一次；App 定时跑
//! cca cost sync --json），_gui state 只读统计好的（cost/local.json、remote.json、sync.json）。config.json 的 "cost": false 关掉

use super::codex::codex_claims;
use super::hub::{host_name, hubs, Hub, HubKind, TeamTag};
use super::keyshare::{blob_ad, blob_open, blob_seal, is_num, keyshare_keys, share_ident, team_keys, Keys};
use super::presence::{presence_aid, presence_post, presence_targets, presence_why, spawn_background};
use super::session::{cli_alive, proc_starts};
use super::settings::home_tilde;
use super::usage::{parse_ts, WEEK, WEEKS_KEEP, WEEKS_SHOW};
use super::{arity, Cmd};
use crate::core::base::{Lock, CCA_HOME, CFG, EMAIL_RE, MASTER_JSON, SELF_PATH};
use crate::core::model::{adir, cdir, is_api, parse_key};
use crate::core::tool::{claude, codex, Kind, Tool, Using};
use crate::py::fail::{errno_text, os_error_text};
use crate::py::fs::{load_quiet, save_json, utime_now};
use crate::py::hash::{sha1_hex, sha256_hex};
use crate::py::obj::{Dict, List, V};
use crate::py::proc::getenv;
use crate::py::re::Re;
use crate::py::time::{localtime, mktime, monotonic, sleep, strftime_local, time, timegm};
use crate::py::{exit, json, num, path, shutil, text, zlib, Fail, R};
use crate::{dict, list, out, re};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::sync::LazyLock;

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("cost-worker", cmd_cost_worker),
    Cmd::all("cost-sync", cmd_cost_sync),
    Cmd::all("cost-stop", cmd_cost_stop),
    Cmd::all("_cost-periods", cmd_cost_periods),
    Cmd::all("_cost-view", cmd_cost_view),
    Cmd::all("session-start", cmd_session_start),
    Cmd::all("cost-hook", cmd_cost_hook),
];

pub static COST_DIR: LazyLock<String> = LazyLock::new(|| path::join(&CCA_HOME, "cost"));
/// 价格表核实的日期（platform.claude.com/docs/en/about-claude/pricing、developers.openai.com/api/docs/pricing）
pub const COST_PRICES_DATE: &str = "2026-11-24";
/// 价格表、读会话记录的规则变了要改：scan.json 的 build 对不上就从头重算（COST_PRICES_DATE + "/第几版"；2026-11-13/2：按会话开始的
/// 记录认账号、记下诊断；2026-11-24/1：补上 GPT-6.2 的标价，以前记成没有标价的请求按新价重算；2026-11-24/2：会话开始的记录比
/// session-owner 新时按记录认（换了账号续接），认 Claude App 的记录；2026-11-24/3：价格表里还没有的新版本按同系列的标价估算
/// （priced_by），以前记成没有标价的请求重算）。price_tables_pinned_to_build 守着：价格表、估算的规则变了它就不过
pub const COST_BUILD: &str = "2026-11-24/3";
/// 后台统计最多隔这么久起一次
pub const COST_EVERY: f64 = 300.0;
/// 和别的设备同步最多隔这么久一次（cca cost sync --now 不等）
pub const COST_SYNC_EVERY: f64 = 600.0;
/// scan.json 里的小时数据留这么久（WEEKS_KEEP 周）：从头重算时会话记录可能已经被 Claude Code 删掉了一部分，以前算过的靠它接上
pub const COST_KEEP: f64 = (WEEKS_KEEP as i64 * WEEK) as f64;
/// local.json 里、传给别的设备的留这么久（App 看最近 12 周和这一周）：只传显示得到的，密文小
pub const COST_PUBLISH: f64 = (14 * WEEK) as f64;
/// 去重的记录留这么久；之后才读到的更早的回复（改了名、复制出来的文件）不算
pub const COST_SEEN: f64 = 35.0 * 86400.0;
/// 别的设备超过这么久没传上来：之后用的可能还没算进来
pub const COST_STALE: f64 = 36.0 * 3600.0;
/// 内容没变也隔这么久重传一次（别的设备按它看这台设备是不是还在同步）
pub const COST_BEAT: f64 = 12.0 * 3600.0;
/// 按天看最近这么多天
pub const COST_DAYS: i64 = 30;
/// 密钥、认证的标签（和共享 API key 的分开）。3.6.6 起密钥整个 Team 共用：只由 Team 的盐和它派生（team_keys）
pub const COST_LABEL: &str = "cca-cost-v2";
/// 3.6.5 及以前的标签：密钥还由登录身份派生（keyshare_keys），登录着那个账号的设备才解得开。只用来读还没升级的设备传的
/// （Item::keys_v1）；各台设备都升级到 3.6.6 以后连同它们一起去掉
pub const COST_LABEL_V1: &str = "cca-cost-v1";
/// 认不出是哪个账号的用量（会话记录里没有账号信息：Claude App 里的会话、旧版 Claude Code）每台设备也传一份：每个 Team 里一个
/// 单独的账号摘要（presence_aid(盐, COST_ORPHAN, "")，和账号的碰不上），密钥、明文的格式和账号的一样。sync.json、remote.json
/// 里的键是 COST_ORPHAN_SLOT
pub const COST_ORPHAN: &str = "cca-cost-orphan";
const COST_ORPHAN_SLOT: &str = "orphan";
/// 别的设备传来的按模型分的小时数据：一个小时最多这么多个模型（多了这一项整个不要）
const COST_MODELS_HOUR: usize = 64;
/// 明文补齐到这么长的整数倍
pub const COST_PAD: usize = 1024;
/// 服务器收的一份密文最长多少个字符（base64）
pub const COST_BLOB_MAX: usize = 98304;
/// 一次请求最多传这么多密文（多了分几次）
pub const COST_PUT_MAX: usize = 256 * 1024;
/// 一次请求最多问、传这么多个账号（服务器的 MAX_ACCOUNTS）：多了分几次
pub const COST_BATCH: usize = 64;
/// cca cost sync 最多等正在跑的统计这么久
pub const COST_WAIT: i64 = 240;
/// 账号目录里记会话开始的文件（cmd_session_start 写，Marks 读）
pub const SESSIONS_FILE: &str = "sessions.jsonl";
/// Claude 桌面 App 里的 Code 会话开始的记录（cca cost hook 加的 hook 回调 cca _session-start --desktop 写，Marks 读）：在 COST_DIR 里，
/// 和 sessions.jsonl 一样一行一个，多一项 uuid（App 登录的账号的 accountUuid）
pub const DESKTOP_SESSIONS: &str = "desktop-sessions.jsonl";
/// cca cost hook 加在共享的 ~/.claude/settings.json 里的 SessionStart hook：命令里有这一段的就是它（cca 的路径会变）
const DESKTOP_HOOK_MARK: &str = " _session-start --desktop";
/// sessions.jsonl 超过这么大时只留最后 SESSIONS_KEEP 字节（一行六七十字节：一天开两百次会话也留得下一个多月）
const SESSIONS_MAX: u64 = 1 << 20;
const SESSIONS_KEEP: usize = 512 << 10;
/// 会话开始的记录晚于回复这么多秒以内也算（回复的时间只到秒）
const MARK_SLACK: f64 = 2.0;
/// 从这个版本起 Claude Code 在会话里写 session-owner
const CLAUDE_OWNER_SINCE: &str = "2.1.317";
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
const CLAUDE_NO_GEO: &[&str] =
    &["claude-opus-4-5", "claude-opus-4-1", "claude-opus-4", "claude-sonnet-4-5", "claude-sonnet-4", "claude-haiku-4-5"];
/// 网页搜索一次 0.01 美元（单位：百万分之一美元）
const CLAUDE_WEB_SEARCH: f64 = 10000.0;

/// OpenAI 的一组标价（美元 / 百万 token）：输入、缓存读、缓存写（30 分钟；没有单独标价的按输入算）、输出
type OpenAiPrice = (f64, f64, Option<f64>, f64);
/// 后一组是一次请求的输入超过 OPENAI_LONG 时的。
/// gpt-5.6-sol 是促销价（官方写着到 2026-11-30，没写之后的价）：官方改价以后要按请求的时间分段、以前的照这个价，
/// 不能直接换掉（换了价要改 COST_BUILD，从头重算时以前的用量也会按新价算）
const OPENAI_PRICES: &[(&str, OpenAiPrice, Option<OpenAiPrice>)] = &[
    ("gpt-6.2-astra", (8.0, 0.40, Some(10.0), 40.0), Some((16.0, 0.80, Some(20.0), 60.0))),
    ("gpt-6.2-sol", (2.0, 0.10, Some(2.50), 10.0), Some((4.0, 0.20, Some(5.0), 15.0))),
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
/// Ultrafast 有标价的模型：(模型, ULTRA_PROMO_END 以前的倍数, 以后的倍数)，别的按 Fast 算。GPT-6.2 的两个是促销价：官方写着
/// 到 2027-02-28（太平洋时间）是 3 倍，2027-03-01 起 6 倍——按请求的时间分段，以前的照以前的倍数
const OPENAI_ULTRA: &[(&str, f64, f64)] =
    &[("gpt-6.1-sol", 6.0, 6.0), ("gpt-6-astra", 6.0, 6.0), ("gpt-6.2-astra", 3.0, 6.0), ("gpt-6.2-sol", 3.0, 6.0)];
/// 2027-03-01 00:00 太平洋时间（PST）：GPT-6.2 的 Ultrafast 从这时起是 6 倍
const ULTRA_PROMO_END: f64 = 1803888000.0;

/// 价格表还没收录的新版本（新模型出来了、cca 还没发新版）：按同一系列里不比它新、最新的那个的标价估算，不至于漏算（PROTOCOL 2.2
/// 的 models[].priced_as 说按的是哪一个）。系列按名字认：Claude 的 claude-<系列>-<主版本>[-<小版本>]，OpenAI 的
/// gpt-<主版本>[.<小版本>]-<档>。认不出系列的（claude-new-9、codex-auto-review 这类）、比价格表里同系列的都旧的照旧没有公开价格
/// （只数 token）。改了这几项要改 COST_BUILD
const CLAUDE_FAMILIES: &[&str] = &["opus", "sonnet", "haiku", "fable", "mythos"];
const OPENAI_FAMILIES: &[&str] = &["astra", "sol", "terra", "luna", "codex"];
const CLAUDE_NAME: &str = r"^claude-([a-z]+)-([0-9]{1,3})(?:-([0-9]{1,3}))?$";
const OPENAI_NAME: &str = r"^gpt-([0-9]{1,3})(?:\.([0-9]{1,3}))?-([a-z]+)$";

/// 系列和版本：(系列, (主版本, 小版本))，认不出的是 None
type Family<'a> = Option<(&'a str, (u32, u32))>;

fn family_version(major: Option<&str>, minor: Option<&str>) -> Option<(u32, u32)> {
    Some((major?.parse().ok()?, minor.map_or(Some(0), |m| m.parse().ok())?))
}

fn claude_family(m: &str) -> Family<'_> {
    let c = re!(CLAUDE_NAME).fullmatch(m)?;
    let fam = c.group(1).filter(|f| CLAUDE_FAMILIES.contains(f))?;
    Some((fam, family_version(c.group(2), c.group(3))?))
}

fn openai_family(m: &str) -> Family<'_> {
    let c = re!(OPENAI_NAME).fullmatch(m)?;
    let fam = c.group(3).filter(|f| OPENAI_FAMILIES.contains(f))?;
    Some((fam, family_version(c.group(1), c.group(2))?))
}

/// 按价格表里哪个模型的标价算：有它自己的就是它（第二项 false）；新版本按同系列里不比它新、最新的那个估算（true）；都没有是 None。
/// 平常的模型第一步就找到了，只有价格表里没有的才去认系列
fn priced_by(
    model: &str,
    names: impl Iterator<Item = &'static str> + Clone,
    family: for<'a> fn(&'a str) -> Family<'a>,
) -> Option<(&'static str, bool)> {
    if let Some(n) = names.clone().find(|n| *n == model) {
        return Some((n, false));
    }
    let (fam, ver) = family(model)?;
    names.filter_map(|n| family(n).filter(|(f, v)| *f == fam && *v <= ver).map(|(_, v)| (v, n))).max().map(|(_, n)| (n, true))
}

fn claude_priced_by(model: &str) -> Option<(&'static str, bool)> {
    priced_by(model, CLAUDE_PRICES.iter().map(|(n, _)| *n), claude_family)
}

fn openai_priced_by(model: &str) -> Option<(&'static str, bool)> {
    priced_by(model, OPENAI_PRICES.iter().map(|(n, _, _)| *n), openai_family)
}

/// PROTOCOL 2.2 的 models[].priced_as：价格表里还没有、按同系列别的模型的标价估算的，是那个模型；别的（有自己的标价、没有公开价格）是 None
pub fn priced_as(model: &str) -> Option<&'static str> {
    claude_priced_by(model).or_else(|| openai_priced_by(model)).filter(|(_, estimated)| *estimated).map(|(n, _)| n)
}

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
/// 金额照原来的先后用小数算（原来整数的标价乘出来是整数，这里的小数在 2^53 以内一样精确）。价格表里还没有的新版本照估算它的那个
/// 模型的一套算（长上下文、快速模式、只在美国推理都是那个模型的）
pub fn cost_claude(model: &str, u: &Dict) -> (Option<f64>, [i64; 4]) {
    let (inp, out, cr) = (tokens(u, "input_tokens"), tokens(u, "output_tokens"), tokens(u, "cache_read_input_tokens"));
    let cc = u.dict("cache_creation").unwrap_or_default();
    let (mut w5, w1) = (tokens(&cc, "ephemeral_5m_input_tokens"), tokens(&cc, "ephemeral_1h_input_tokens"));
    if w5 == 0 && w1 == 0 {
        w5 = tokens(u, "cache_creation_input_tokens");
    }
    let tok = [inp, out, cr, w5 + w1];
    let Some((key, _)) = claude_priced_by(model) else { return (None, tok) };
    let mut p = CLAUDE_PRICES.iter().find(|(name, _)| *name == key).map(|(_, p)| *p).expect("priced_by 给的是价格表里的名字");
    if let Some((_, over, long)) = CLAUDE_LONG.iter().find(|(name, _, _)| *name == key) {
        if inp + cr + w5 + w1 > *over {
            p = *long;
        }
    }
    let [mut pin, mut p5, mut p1, mut pr, mut pout] = p;
    let speed = u.get("speed");
    if speed.as_ref().and_then(|s| s.as_str()) == Some("fast") {
        if let Some((_, fin, fout)) = CLAUDE_FAST.iter().find(|(name, _, _)| *name == key) {
            let k = fin / pin;
            (pin, p5, p1, pr, pout) = (*fin, p5 * k, p1 * k, pr * k, *fout);
        }
    }
    // 美元 / 百万 token × token 数 = 百万分之一美元
    let mut micro = inp as f64 * pin + w5 as f64 * p5 + w1 as f64 * p1 + cr as f64 * pr + out as f64 * pout;
    let geo = u.get("inference_geo");
    if geo.as_ref().and_then(|g| g.as_str()) == Some("us") && !CLAUDE_NO_GEO.contains(&key) {
        micro *= 1.1;
    }
    let searches = cost_int(u.dict("server_tool_use").and_then(|s| s.get("web_search_requests")));
    (Some(micro + searches as f64 * CLAUDE_WEB_SEARCH), tok)
}

/// 一次 Codex 请求（token_count 的 last_token_usage：input_tokens 含缓存读、缓存写，output_tokens 含推理）：同 cost_claude。
/// ts 是请求的时间（有的标价按时间分段：OPENAI_ULTRA）。价格表里还没有的新版本同 cost_claude
pub fn cost_codex(model: &str, tier: Option<&str>, ts: f64, u: &Dict) -> (Option<f64>, [i64; 4]) {
    let (inp, cached, write, out) = (
        tokens(u, "input_tokens"),
        tokens(u, "cached_input_tokens"),
        tokens(u, "cache_write_input_tokens"),
        tokens(u, "output_tokens"),
    );
    let cached = cached.min(inp);
    let write = write.min(inp - cached);
    let plain = inp - cached - write;
    let tok = [plain, out, cached, write];
    let Some((key, _)) = openai_priced_by(model) else { return (None, tok) };
    let (_, base, long) = OPENAI_PRICES.iter().find(|(name, _, _)| *name == key).expect("priced_by 给的是价格表里的名字");
    let (pin, pc, pw, pout) = match long {
        Some(long) if inp > OPENAI_LONG => *long,
        _ => *base,
    };
    let k = if tier == Some("ultrafast") {
        OPENAI_ULTRA
            .iter()
            .find(|(name, _, _)| *name == key)
            .map_or(2.0, |(_, before, after)| if ts < ULTRA_PROMO_END { *before } else { *after })
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
            if let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) =
                (int(0, 4), int(5, 7), int(8, 10), int(11, 13), int(14, 16), int(17, 19))
            {
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
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| at_path(e, &tmp))?;
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
    load_quiet(&path::join(&COST_DIR, name))
        .and_then(|j| j.as_dict())
        .filter(|j| j.get("v").is_some_and(|v| v.py_eq(&V::Int(1))))
        .unwrap_or_default()
}

/// 7 个计数：都是整数（不是布尔值），不小于 0、不超过 10^15
fn counts_ok(r: &V) -> bool {
    r.as_list().is_some_and(|r| {
        r.len() == 7 && r.items().iter().all(|x| matches!(x, V::Int(n) if (0..=1_000_000_000_000_000).contains(n)))
    })
}

/// {小时: [7 个计数]}：金额（百万分之一美元）、请求数、输入、输出、缓存读、缓存写、其中没有标价的请求数
pub fn cost_rows_ok(rows: Option<&V>) -> bool {
    rows.and_then(|r| r.as_dict()).is_some_and(|rows| rows.items().iter().all(|(k, r)| COST_HOUR_RE.matches(k) && counts_ok(r)))
}

/// 模型名：会话记录里的写法（cost_model 给的：小写，不超过 64 个字符），可以是空的（会话记录里没写）
fn model_name_ok(m: &str) -> bool {
    text::len(m) <= 64 && text::isprintable(m)
}

/// 按模型分的小时数据 {小时: {模型: [7 个计数]}}（scan.json 的 hm 每个身份一份、om 是认不出是谁的；local.json 同样；传来的明文的 hm）。
/// 一个小时最多 cap 个模型：这台设备自己的不限（限了的话，多出来时从头重算会一直重算下去）
pub fn cost_mrows_ok(rows: Option<&V>, cap: usize) -> bool {
    rows.and_then(|r| r.as_dict()).is_some_and(|rows| {
        rows.items().iter().all(|(k, ms)| {
            COST_HOUR_RE.matches(k)
                && ms
                    .as_dict()
                    .is_some_and(|ms| ms.len() <= cap && ms.items().iter().all(|(m, r)| model_name_ok(m) && counts_ok(r)))
        })
    })
}

fn cost_fresh() -> Dict {
    dict! {
        "v" => 1, "build" => COST_BUILD, "done" => false, "files" => Dict::new(), "seen" => Dict::new(), "owners" => Dict::new(),
        "h" => Dict::new(), "orphan" => Dict::new(), "um" => Dict::new(), "who" => Dict::new(), "hm" => Dict::new(), "om" => Dict::new(),
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
/// 没算完又要从头来时是它留着的那份），scan.json 没了（restore 的备份不带它）就用 local.json 里的（最近 COST_PUBLISH）。
/// 按模型分的（hm、om）一起留着（以前的版本没有）
fn cost_rows_kept(j: &Dict, loc: &Dict) -> Dict {
    let src = j.dict("old").unwrap_or_else(|| j.clone());
    let (mut h, mut orphan, mut hm, mut om) = (src.get("h"), src.get("orphan"), src.get("hm"), src.get("om"));
    if !h.as_ref().and_then(|h| h.as_dict()).is_some_and(|h| !h.is_empty()) {
        (h, orphan, hm, om) = (loc.get("acc"), loc.get("orphan"), loc.get("hm"), loc.get("om"));
    }
    let (kept, kept_m) = (Dict::new(), Dict::new());
    for (i, r) in rows_of(h) {
        if cost_ident_ok(&i) && cost_rows_ok(Some(&r)) {
            kept.set(i, r);
        }
    }
    for (i, r) in rows_of(hm) {
        if cost_ident_ok(&i) && cost_mrows_ok(Some(&r), usize::MAX) {
            kept_m.set(i, r);
        }
    }
    let empty = || V::Dict(Dict::new());
    dict! {
        "h" => kept, "orphan" => orphan.filter(|o| cost_rows_ok(Some(o))).unwrap_or_else(empty),
        "hm" => kept_m, "om" => om.filter(|o| cost_mrows_ok(Some(o), usize::MAX)).unwrap_or_else(empty),
    }
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
    // 按模型分的照样（以前的版本没有 hm、om）
    let hm = j.dict("hm").expect("scan.json 核对过");
    for (ident, rows) in rows_of(old.get("hm")) {
        if cost_ident_ok(&ident) && cost_mrows_ok(Some(&rows), usize::MAX) {
            let tool = text::partition(&ident, ":").0;
            let lo = s.oldest.iter().find(|(t, _)| *t == tool).map_or(far, |(_, m)| *m);
            for (k, r) in rows.as_dict().expect("cost_mrows_ok 核对过").items() {
                if hour_of(&k) < lo {
                    sub_dict(&hm, &ident).set(k, r);
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
    let om = j.dict("om").expect("scan.json 核对过");
    if let Some(rows) = old.get("om").filter(|r| cost_mrows_ok(Some(r), usize::MAX)) {
        for (k, r) in rows.as_dict().expect("cost_mrows_ok 核对过").items() {
            if hour_of(&k) < lo {
                om.set(k, r);
            }
        }
    }
}

// --- 去重的记录（scan.json 的 seen：{消息的摘要: 那一小时}）-------------------------------------------------------------
// 35 天里每条回复一项，用得多的人有几十万项。原来是一个普通的字典（Python 里每项一百多字节；照搬成通用的 Dict 也是，10 万项
// 约 30 MB）。键都是 c / x 加 16 位十六进制（摘要的前 8 个字节），值都是整点的时间戳：存成 16 个字节一项的表，从文件里
// 直接读出来、直接写回去，不经通用的 JSON 值。先后照旧（写出来的文件和原来逐字节一样）。文件里有不是这个样子的项（手改过）
// 时退回通用的字典，行为和原来一样。

/// 一项：键是 kind（c：Claude 的回复，x：Codex 的请求）加 id 的十六进制，值是 hour × 3600
#[derive(Clone, Copy)]
struct SeenEntry {
    id: u64,
    hour: u32,
    kind: u8,
}

/// 摘要本身就是均匀的：拿它的低 64 位当哈希
#[derive(Default, Clone, Copy)]
struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = self.0.rotate_left(8) ^ *b as u64;
        }
    }

    fn write_u128(&mut self, n: u128) {
        self.0 = (n as u64) ^ ((n >> 64) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

type IdSet = std::collections::HashSet<u128, std::hash::BuildHasherDefault<IdHasher>>;

#[derive(Default)]
struct CompactSeen {
    entries: Vec<SeenEntry>,
    index: IdSet,
}

fn seen_id(kind: u8, id: u64) -> u128 {
    ((kind as u128) << 64) | id as u128
}

impl CompactSeen {
    /// `{"c0123456789abcdef":1794024000,…}` 这样的一段 JSON：每一项都是规矩的样子才读得出来（不是的由调用方退回通用的读法）
    fn parse(text: &str) -> Option<CompactSeen> {
        let b = text.as_bytes();
        let ws = |mut i: usize| {
            while matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                i += 1;
            }
            i
        };
        let mut seen = CompactSeen::default();
        let mut i = ws(0);
        if b.get(i) != Some(&b'{') {
            return None;
        }
        i = ws(i + 1);
        if b.get(i) == Some(&b'}') {
            return (ws(i + 1) == b.len()).then_some(seen);
        }
        loop {
            // "c0123456789abcdef"
            let key = b.get(i..i + 19)?;
            if key[0] != b'"' || key[18] != b'"' || !matches!(key[1], b'c' | b'x') {
                return None;
            }
            let mut id = 0u64;
            for h in &key[2..18] {
                let d = match h {
                    b'0'..=b'9' => h - b'0',
                    b'a'..=b'f' => h - b'a' + 10,
                    _ => return None,
                };
                id = (id << 4) | d as u64;
            }
            i = ws(i + 19);
            if b.get(i) != Some(&b':') {
                return None;
            }
            i = ws(i + 1);
            // 整点的时间戳：不带正负号、不以 0 开头的十进制数（单独一个 0 可以）
            let start = i;
            let mut value = 0u64;
            while let Some(d) = b.get(i).filter(|d| d.is_ascii_digit()) {
                value = value.checked_mul(10)?.checked_add((d - b'0') as u64)?;
                i += 1;
            }
            if i == start || (b[start] == b'0' && i - start > 1) || !value.is_multiple_of(3600) {
                return None;
            }
            let hour = u32::try_from(value / 3600).ok()?;
            if !seen.index.insert(seen_id(key[1], id)) {
                return None; // 同一个键写了两次
            }
            seen.entries.push(SeenEntry { id, hour, kind: key[1] });
            i = ws(i);
            match b.get(i) {
                Some(b',') => i = ws(i + 1),
                Some(b'}') => return (ws(i + 1) == b.len()).then_some(seen),
                _ => return None,
            }
        }
    }
}

/// scan.json 的 seen
enum Seen {
    Compact(CompactSeen),
    /// 文件里有不规矩的项：照原来的样子用通用的字典
    Generic(Dict),
}

impl Seen {
    fn new() -> Seen {
        Seen::Compact(CompactSeen::default())
    }

    fn key_text(kind: u8, id: u64) -> String {
        format!("{}{:016x}", kind as char, id)
    }

    /// `key in seen`
    fn has(&self, kind: u8, id: u64) -> bool {
        match self {
            Seen::Compact(s) => s.index.contains(&seen_id(kind, id)),
            Seen::Generic(d) => d.has(&Seen::key_text(kind, id)),
        }
    }

    /// `seen[key] = hour`（key 还不在里面；hour 是整点的时间戳）
    fn add(&mut self, kind: u8, id: u64, hour: i64) {
        if let Seen::Compact(s) = self {
            match u32::try_from(hour / 3600) {
                Ok(h) if hour % 3600 == 0 && hour >= 0 => {
                    s.index.insert(seen_id(kind, id));
                    s.entries.push(SeenEntry { id, hour: h, kind });
                    return;
                }
                // 放不进紧凑的表（不会有：小时是从 10 亿到后天之间的时间戳算的）：换成通用的
                _ => *self = Seen::Generic(self.to_dict()),
            }
        }
        if let Seen::Generic(d) = self {
            d.set(Seen::key_text(kind, id), hour);
        }
    }

    /// `{k: v for k, v in seen.items() if v >= lo}`
    fn retain(&mut self, lo: f64) {
        match self {
            Seen::Compact(s) => {
                let before = s.entries.len();
                s.entries.retain(|e| (e.hour as u64 * 3600) as f64 >= lo);
                if s.entries.len() != before {
                    s.index = s.entries.iter().map(|e| seen_id(e.kind, e.id)).collect();
                }
            }
            Seen::Generic(d) => {
                let kept = Dict::new();
                for (k, v) in d.items() {
                    if v.as_num().is_some_and(|v| v >= lo) {
                        kept.set(k, v);
                    }
                }
                *d = kept;
            }
        }
    }

    fn to_dict(&self) -> Dict {
        match self {
            Seen::Compact(s) => {
                let d = Dict::new();
                for e in &s.entries {
                    d.set(Seen::key_text(e.kind, e.id), e.hour as i64 * 3600);
                }
                d
            }
            Seen::Generic(d) => d.clone(),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            Seen::Compact(s) => s.entries.len(),
            Seen::Generic(d) => d.len(),
        }
    }
}

/// scan.json 的文字拆成两段：把顶层 "seen" 的值换成 {} 的那一份，和 seen 的值原来的那一段。只按引号、括号配对着找；
/// 找不到正好一个值是对象的 "seen"、顶层不是一个对象、键里有转义，都是 None（调用方照通用的读法读整份）
fn split_seen(text: &str) -> Option<(String, &str)> {
    let b = text.as_bytes();
    let ws = |mut i: usize| {
        while matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            i += 1;
        }
        i
    };
    // 从开头的引号后面起，找到字符串结尾的引号后面
    let string_end = |mut i: usize| -> Option<usize> {
        loop {
            match b.get(i)? {
                b'"' => return Some(i + 1),
                b'\\' => i += 2,
                _ => i += 1,
            }
        }
    };
    let value_end = |i: usize| -> Option<usize> {
        match b.get(i)? {
            b'"' => string_end(i + 1),
            b'{' | b'[' => {
                let (mut depth, mut j) = (0usize, i);
                loop {
                    match b.get(j)? {
                        b'"' => {
                            j = string_end(j + 1)?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(j + 1);
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
            }
            _ => {
                let mut j = i;
                while !matches!(b.get(j), None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')) {
                    j += 1;
                }
                (j > i).then_some(j)
            }
        }
    };
    let mut i = ws(0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i = ws(i + 1);
    let mut found: Option<(usize, usize)> = None;
    if b.get(i) != Some(&b'}') {
        loop {
            if b.get(i) != Some(&b'"') {
                return None;
            }
            let key_end = string_end(i + 1)?;
            let key = &b[i + 1..key_end - 1];
            if key.contains(&b'\\') {
                return None;
            }
            i = ws(key_end);
            if b.get(i) != Some(&b':') {
                return None;
            }
            i = ws(i + 1);
            let end = value_end(i)?;
            if key == b"seen" {
                if found.is_some() || b[i] != b'{' {
                    return None;
                }
                found = Some((i, end));
            }
            i = ws(end);
            match b.get(i)? {
                b',' => i = ws(i + 1),
                b'}' => break,
                _ => return None,
            }
        }
    }
    if ws(i + 1) != b.len() {
        return None;
    }
    let (start, end) = found?;
    Some((format!("{}{{}}{}", &text[..start], &text[end..]), &text[start..end]))
}

/// 读 scan.json：(内容, seen)。内容里 seen 那一项是空的占位（位置照旧），真的在第二个返回值里。
/// 没有、读不了、不是 v 为 1 的对象：({}, None)（和 cost_json 一样）；seen 不是对象：(内容, None)
fn scan_load() -> (Dict, Option<Seen>) {
    let none = || (Dict::new(), None);
    let Ok(raw) = std::fs::read(path::join(&COST_DIR, "scan.json")) else { return none() };
    let v1 = |j: &Dict| j.get("v").is_some_and(|v| v.py_eq(&V::Int(1)));
    if let Ok(text) = std::str::from_utf8(&raw) {
        if let Some((rest, seen_text)) = split_seen(text) {
            if let (Ok(V::Dict(j)), Some(seen)) = (json::loads(&rest), CompactSeen::parse(seen_text)) {
                return if v1(&j) { (j, Some(Seen::Compact(seen))) } else { none() };
            }
        }
    }
    // 通用的读法（文件里有不规矩的地方；写坏了的也在这里认出来）
    match json::loads_file_bytes(&raw) {
        Ok(V::Dict(j)) if v1(&j) => {
            let seen = j.dict("seen").map(Seen::Generic);
            (j, seen)
        }
        _ => none(),
    }
}

/// 写 scan.json：和 `cost_save("scan.json", j)` 写出来的逐字节一样，只是 seen 直接从紧凑的表写，不先拼成一个大字典
fn scan_save(j: &Dict, seen: &Seen) -> R<()> {
    use std::io::Write;
    let s = match seen {
        Seen::Generic(d) => {
            j.set("seen", d);
            return cost_save("scan.json", &V::Dict(j.clone()));
        }
        Seen::Compact(s) => s,
    };
    if !path::isfile(&CFG) {
        return Err(Fail::Os(std::io::Error::other(format!("{} 不在了", *CFG))));
    }
    // seen 的位置上先放一个记号，写的时候在那里换成表里的内容
    let marker = format!("cca-seen-{}", crate::py::rand::token_hex(16));
    j.set("seen", marker.as_str());
    let body = json::dumps(&V::Dict(j.clone()), json::Fmt::new().utf8().compact());
    j.set("seen", Dict::new());
    let quoted = format!("\"{marker}\"");
    let at = body.find(&quoted).expect("刚放进去的记号");
    let dest = path::join(&COST_DIR, "scan.json");
    let tmp = format!("{}.cca-{}.tmp", dest, std::process::id());
    let write = || -> R<()> {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| at_path(e, &tmp))?;
        let mut w = std::io::BufWriter::with_capacity(256 * 1024, f);
        w.write_all(&body.as_bytes()[..at])?;
        w.write_all(b"{")?;
        for (n, e) in s.entries.iter().enumerate() {
            if n > 0 {
                w.write_all(b",")?;
            }
            write!(w, "\"{}{:016x}\":{}", e.kind as char, e.id, e.hour as u64 * 3600)?;
        }
        w.write_all(b"}")?;
        w.write_all(&body.as_bytes()[at + quoted.len()..])?;
        w.flush()?;
        drop(w);
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

/// `type(x) is int`
fn is_int(x: &V) -> bool {
    matches!(x, V::Int(_) | V::Big(_))
}

/// scan.json（读到哪、去重、小时数据）；没有、价格表或规则变了、格式不对：从头来。从头来时以前记下的身份 → 邮箱照样留着
/// （scan.json、local.json 里的：退出登录了的账号，重算出来的用量照样认得出是谁的；restore 的备份不带 scan.json，带着 local.json）
fn cost_load_scan() -> (Dict, Seen) {
    let (j, seen) = scan_load();
    let base = cost_fresh();
    let dicts = ["files", "owners", "h", "orphan", "um", "who", "hm", "om"];
    let shape = j.get("build").is_some_and(|b| b.as_str() == Some(COST_BUILD))
        && j.get("v").is_some_and(|v| v.isinstance_int())
        && j.get("done").is_some_and(|d| d.is_bool())
        && dicts.iter().all(|k| j.dict(k).is_some());
    let all = |k: &str, ok: &dyn Fn(&V) -> bool| j.dict(k).is_some_and(|d| d.values().iter().all(ok));
    // seen 是对象、值都是整数（紧凑的表读得出来就是）
    let seen = seen.filter(|s| match s {
        Seen::Compact(_) => true,
        Seen::Generic(d) => d.values().iter().all(is_int),
    });
    if let Some(seen) = seen.filter(|_| {
        shape
            && all("files", &|x| x.is_dict())
            && all("owners", &|x| x.is_list())
            && cost_rows_ok(j.get("orphan").as_ref())
            && all("h", &|r| cost_rows_ok(Some(r)))
            && all("hm", &|r| cost_mrows_ok(Some(r), usize::MAX))
            && cost_mrows_ok(j.get("om").as_ref(), usize::MAX)
            && all("um", &|m| m.as_dict().is_some_and(|m| m.values().iter().all(is_int)))
    }) {
        j.set("who", cost_who_kept(&[&j]));
        // 诊断（以前的版本没有）不对就不要了，不为它从头重算
        if j.get("diag").is_some_and(|d| !diag_ok(&d)) {
            j.pop("diag");
        }
        return (j, seen);
    }
    let loc = cost_json("local.json");
    base.set("who", cost_who_kept(&[&loc, &j]));
    base.set("old", cost_rows_kept(&j, &loc));
    (base, Seen::new())
}

// --- 会话开始的记录（账号目录的 sessions.jsonl、Claude App 的 desktop-sessions.jsonl）---------------------------------------
// Claude Code 2.1.316 及以前不在会话里写 session-owner，Claude 桌面 App 里的会话也不写：这些回复认不出是哪个账号的；换了账号
// 续接的会话里 Claude Code（2.1.330 实测）照旧写原来的账号。cca 启动 Claude Code 时经 --settings 带一个 SessionStart hook
// （accounts.rs 的 session_hook），会话开始、续接、/clear、压缩以后回调 cca _session-start 账号目录名，在这个账号目录的
// sessions.jsonl 末尾记一行 {"sid": 会话 id, "at": 时间, "source": 怎么开始的}。Claude App 自己起 Claude Code，带不上
// --settings：cca cost hook 问过以后在共享的 ~/.claude/settings.json 里加一条 SessionStart hook，只在 App 里的会话
// （CLAUDE_CODE_ENTRYPOINT=claude-desktop）回调 cca _session-start --desktop，记进 COST_DIR 的 desktop-sessions.jsonl，多一项
// uuid（App 在 CLAUDE_CODE_ACCOUNT_UUID 里给的、它登录的账号）。
// 统计时按它认（Marks）：这个会话在回复以前最后一次是在哪个账号下开始的——没有 session-owner 的回复，和记录比 session-owner
// 起作用的时候新的（换了账号续接）；第三方 API 账号下开始的不算（不是订阅的用量）。不经 cca 启动的（直接运行 claude、IDE，
// 没开 cca cost hook 时的桌面 App）没有记录，照旧按 session-owner 认，没有就认不出。

/// cca _session-start 账号目录名 | --desktop（Claude Code 的 SessionStart hook）：stdin 是 hook 的输入（JSON：session_id、source……），
/// 会话 id 记进 ~/.cca/accounts/<账号目录名>/sessions.jsonl；--desktop（cca cost hook 加的，Claude App 里的会话）记进 COST_DIR 的
/// desktop-sessions.jsonl，带上 App 登录的账号（CLAUDE_CODE_ACCOUNT_UUID；不是 App 里的会话、没有它、关了折合金额都不记）。
/// 600，追加，flock 排队；超过 SESSIONS_MAX 先只留最后 SESSIONS_KEEP。
/// 出什么错都不说（bash 把 stdout、stderr 都丢掉、总是成功：hook 的 stdout 会进 Claude 的上下文）
pub fn cmd_session_start(args: &[String]) -> R<()> {
    arity("cmd_session_start", args, 1, 1)?;
    let id = args[0].as_str();
    // SAFETY: 只是问 fd 0 是不是终端（手动运行时不等输入）
    if unsafe { libc::isatty(0) } == 1 {
        return Ok(());
    }
    let (file, uuid) = if id == "--desktop" {
        let app = getenv("CLAUDE_CODE_ENTRYPOINT").as_deref() == Some("claude-desktop");
        match getenv("CLAUDE_CODE_ACCOUNT_UUID").filter(|u| re!(r"^[0-9A-Za-z][0-9A-Za-z_-]{7,63}$").matches(u)) {
            Some(u) if app && cost_on() && cost_dir().is_ok() => (path::join(&COST_DIR, DESKTOP_SESSIONS), Some(u)),
            _ => return Ok(()),
        }
    } else {
        let dir = path::join_all(&[&claude().sroot, "accounts", id]);
        if !re!(r"^[a-z0-9][a-z0-9-]{0,63}$").matches(id) || !path::isdir(&dir) {
            return Ok(());
        }
        (path::join(&dir, SESSIONS_FILE), None)
    };
    let mut raw = Vec::new();
    if std::io::stdin().lock().take(64 << 10).read_to_end(&mut raw).is_err() {
        return Ok(());
    }
    let Ok(d) = line_doc(&raw) else { return Ok(()) };
    let Some(sid) = d.str("session_id").filter(|s| re!(r"^[0-9A-Za-z][0-9A-Za-z_-]{7,63}$").matches(s)) else { return Ok(()) };
    let source = d.str("source").filter(|s| re!(r"^[a-z_]{1,16}$").matches(s)).map_or(String::new(), |s| s.to_string());
    let rec = dict! { "sid" => &*sid, "at" => time(), "source" => source };
    if let Some(u) = uuid {
        rec.set("uuid", u);
    }
    let line = json::dumps(&V::Dict(rec), json::Fmt::new().compact());
    let _ = sessions_append(&file, format!("{line}\n").as_bytes());
    Ok(())
}

/// 在 sessions.jsonl 末尾加一行（拿着 flock：和别的会话开始、统计时读它的排队）；太大了先只留最后 SESSIONS_KEEP 字节（从整行开始）
fn sessions_append(p: &str, line: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().read(true).append(true).create(true).mode(0o600).open(p)?;
    // SAFETY: f 开着，fd 有效；锁在 f 关上时放开
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if f.metadata()?.len() > SESSIONS_MAX {
        let mut all = Vec::new();
        f.seek(SeekFrom::Start(0))?;
        f.read_to_end(&mut all)?;
        let cut = all.len().saturating_sub(SESSIONS_KEEP);
        let start = all[cut..].iter().position(|b| *b == b'\n').map_or(all.len(), |i| cut + i + 1);
        f.set_len(0)?; // 追加着写：截短以后从头写
        f.write_all(&all[start..])?;
    }
    f.write_all(line)
}

/// 这条 hook 的命令：只在 Claude App 里的 Code 会话里（CLAUDE_CODE_ENTRYPOINT=claude-desktop）回调 cca；命令行里的会话、
/// cca 已经不在了，都什么也不做。不出声（SessionStart hook 的 stdout 会进 Claude 的上下文）
fn desktop_hook_command() -> String {
    let cca = text::sh_quote(&SELF_PATH);
    format!(
        "[ \"$CLAUDE_CODE_ENTRYPOINT\" = claude-desktop ] && [ -x {cca} ] && exec {cca}{DESKTOP_HOOK_MARK} >/dev/null 2>&1; exit 0"
    )
}

/// 共享的 Claude Code 设置（各账号目录里的 settings.json 链到它）
fn desktop_hook_file() -> String {
    path::join(&claude().master, "settings.json")
}

/// hooks.SessionStart 里这条 hook 的命令（每一组的 hooks 里命令带着 DESKTOP_HOOK_MARK 的）
fn desktop_hook_commands(groups: &List) -> Vec<String> {
    let mut found = Vec::new();
    for g in groups.items() {
        for h in g.as_dict().and_then(|g| g.list("hooks")).map(|l| l.items()).unwrap_or_default() {
            if let Some(c) = h.as_dict().and_then(|h| h.str("command")).filter(|c| c.contains(DESKTOP_HOOK_MARK)) {
                found.push(c.to_string());
            }
        }
    }
    found
}

/// 共享设置里有没有这条 hook（cost-hook status、App 的 unattributed.app_hook）：读不了、不是 JSON 算没有
fn desktop_hook_on() -> bool {
    let data = load_quiet(&desktop_hook_file()).and_then(|d| d.as_dict());
    data.and_then(|d| d.dict("hooks")).and_then(|h| h.list("SessionStart")).is_some_and(|g| !desktop_hook_commands(&g).is_empty())
}

/// 去掉 groups（hooks.SessionStart）里这条 hook；一组里只有它的，整组去掉（别的组原样留着，连同本来就空着的）
fn desktop_hook_strip(groups: &List) {
    let mine = |h: &V| h.as_dict().and_then(|h| h.str("command")).is_some_and(|c| c.contains(DESKTOP_HOOK_MARK));
    let kept: Vec<V> = groups
        .items()
        .into_iter()
        .filter(|g| {
            let Some(list) = g.as_dict().and_then(|g| g.list("hooks")) else { return true };
            let items = list.items();
            if !items.iter().any(mine) {
                return true;
            }
            list.clear();
            for h in items.into_iter().filter(|h| !mine(h)) {
                list.push(h);
            }
            !list.is_empty()
        })
        .collect();
    groups.clear();
    for g in kept {
        groups.push(g);
    }
}

/// 加上（on）、去掉这条 hook：Ok(改没改)；共享设置读不了、不是 JSON、hooks 的样子不对、写不了时 Ok(Err(说明))，什么都不改。
/// 拿着 sync.lock（py link 也在锁里改共享的配置）；是链接的照旧写到它指向的文件，原来没有这个文件时新建（600）。别的设置不动
/// （照 JSON 重新写一遍：缩进两格，和 Claude Code 自己写的一样）
fn desktop_hook_set(on: bool) -> R<Result<bool, String>> {
    let file = desktop_hook_file();
    let _lock = Lock::acquire()?;
    let raw = match std::fs::read(&file) {
        Ok(raw) => Some(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if path::lexists(&file) {
                return Ok(Err(format!("{} 是个指向不存在的文件的链接，cca 不改它", home_tilde(&file))));
            }
            None
        }
        Err(_) => return Ok(Err(format!("{} 读不了，cca 不改它", home_tilde(&file)))),
    };
    let created = raw.is_none();
    let data = match raw {
        None if !on => return Ok(Ok(false)),
        None => Dict::new(),
        Some(raw) => match std::str::from_utf8(&raw).ok().and_then(|t| json::loads(t).ok()).and_then(|d| d.as_dict()) {
            Some(d) => d,
            None => {
                return Ok(Err(format!("{} 不是合法的 JSON 设置，cca 不改它：先把它改对（或者挪走）再试", home_tilde(&file))));
            }
        },
    };
    let shape = |what: &str| Ok(Err(format!("{} 里的 {what}，cca 不改它", home_tilde(&file))));
    let hooks = match data.get("hooks") {
        None => None,
        Some(V::Dict(h)) => Some(h),
        Some(_) => return shape("hooks 不是对象"),
    };
    let groups = match hooks.as_ref().and_then(|h| h.get("SessionStart")) {
        None => None,
        Some(V::List(g)) => Some(g),
        Some(_) => return shape("hooks.SessionStart 不是列表"),
    };
    let want = desktop_hook_command();
    let mine = groups.as_ref().map(desktop_hook_commands).unwrap_or_default();
    if (on && mine.len() == 1 && mine[0] == want) || (!on && mine.is_empty()) {
        return Ok(Ok(false)); // 本来就是这样
    }
    // 先都去掉（cca 换了路径的、多出来的），要的话再在最后加一条
    if let (Some(hooks), Some(groups)) = (&hooks, &groups) {
        desktop_hook_strip(groups);
        if !on && groups.is_empty() {
            hooks.pop("SessionStart");
            if hooks.is_empty() {
                data.pop("hooks");
            }
        }
    }
    if on {
        let hooks = hooks.unwrap_or_else(|| {
            let h = Dict::new();
            data.set("hooks", h.clone());
            h
        });
        let groups = groups.unwrap_or_else(|| {
            let g = List::new();
            hooks.set("SessionStart", g.clone());
            g
        });
        groups.push(dict! { "hooks" => list![dict! { "type" => "command", "command" => want, "timeout" => 10 }] });
    }
    match save_json(&file, &V::Dict(data), None, true) {
        Ok(()) => {}
        Err(Fail::Os(e)) => {
            let why = e.raw_os_error().map_or_else(|| os_error_text(&e), errno_text);
            return Ok(Err(format!("{} 改不了（{}），什么都没改", home_tilde(&file), why)));
        }
        Err(e) => return Err(e),
    }
    if created {
        desktop_hook_link(&file);
    }
    Ok(Ok(true))
}

/// 共享的 settings.json 是刚新建的：各个 Claude 账号目录里还没链到它，链上（py link 下次启动这个账号时也会，那就晚了：开着的
/// Claude App 新开的会话就要用上）
fn desktop_hook_link(shared: &str) {
    let tool = claude();
    let Some(table) = load_quiet(&tool.table).and_then(|t| t.as_dict()) else { return };
    tool.set_ids(&table);
    let _using = Using::tool(tool);
    for k in table.keys() {
        let dir = cdir(&k);
        let p = path::join(&dir, "settings.json");
        if path::isdir(&dir) && !path::lexists(&p) {
            let _ = shutil::symlink(shared, &p);
        }
    }
}

/// py cost-hook status | on | off（cca cost hook）：Claude App 里的 Code 会话开始时记下 App 登录的账号——在共享的
/// ~/.claude/settings.json 里加上、去掉一条 SessionStart hook（desktop_hook_command）。status 打印 on 或 off；on、off 改了打印
/// changed，本来就是这样打印 same；on 时折合金额关着退出码 2；改不了（读不了、不是 JSON、hooks 的样子不对、写不了）退出码 4，
/// stdout 是说明
pub fn cmd_cost_hook(args: &[String]) -> R<()> {
    arity("cmd_cost_hook", args, 1, 1)?;
    let on = match args[0].as_str() {
        "status" => {
            out!("{}", if desktop_hook_on() { "on" } else { "off" });
            return Ok(());
        }
        "on" => true,
        "off" => false,
        other => {
            out!("不认识的操作: {other}（status、on、off）");
            return exit(1);
        }
    };
    if on && !cost_on() {
        out!("折合金额关掉了（config.json 里写着 \"cost\": false）：先打开它");
        return exit(2);
    }
    match desktop_hook_set(on)? {
        Ok(changed) => out!("{}", if changed { "changed" } else { "same" }),
        Err(why) => {
            out!("{why}");
            return exit(4);
        }
    }
    Ok(())
}

/// 这台设备记不记 Claude App 里 Code 会话的账号（PROTOCOL 2.2 的 unattributed.app_hook）：on | off；不是 macOS（没有 Claude App）为 null
fn app_hook_view() -> V {
    if cfg!(target_os = "macos") {
        V::from(if desktop_hook_on() { "on" } else { "off" })
    } else {
        V::None
    }
}

/// 一次会话开始是在谁下面
enum MarkWho {
    /// 官方账号：它现在登录的 accountUuid（读不出是 None：认不出）
    Official(Option<V>),
    /// 第三方 API 账号：它的会话不算
    Api,
}

/// Claude 各账号目录的 sessions.jsonl 和 Claude App 的 desktop-sessions.jsonl：会话 id → [(时间, 是谁)]（按时间排好）
struct Marks {
    /// (路径, 是谁（None：Claude App 的记录，每一行带着 App 登录的账号）, 上次读时的大小和修改时间)
    files: Vec<(String, Option<std::rc::Rc<MarkWho>>, Option<(u64, i64, i64)>)>,
    /// 账号表里官方账号的身份 → 它的 MarkWho。Claude App 的记录只认这些：别的身份（不在这台设备的账号表里）算进去也没有
    /// 哪个账号显示它，不如照旧留在「认不出账号」里
    known: std::collections::HashMap<String, std::rc::Rc<MarkWho>>,
    by_sid: std::collections::HashMap<String, Vec<(f64, std::rc::Rc<MarkWho>)>>,
}

impl Marks {
    /// 账号表里各 Claude 账号的（官方账号的身份读它配置目录的 .claude.json；读不了账号表是空的），和 Claude App 的
    fn load() -> Marks {
        let tool = claude();
        let table = if path::exists(&tool.table) { load_quiet(&tool.table).and_then(|t| t.as_dict()) } else { None };
        if let Some(t) = &table {
            tool.set_ids(t); // 第三方 API 账号的目录按它的 id 起名
        }
        let _using = Using::tool(tool);
        let (mut files, mut known) = (Vec::new(), std::collections::HashMap::new());
        for (k, v) in table.map(|t| t.items()).unwrap_or_default() {
            let who = std::rc::Rc::new(if is_api(&v) {
                MarkWho::Api
            } else {
                MarkWho::Official(ident_str(oauth_ident(&path::join(&cdir(&k), ".claude.json")).0))
            });
            if let MarkWho::Official(Some(u)) = &*who {
                known.insert(u.py_str(), who.clone());
            }
            files.push((path::join(&adir(&k), SESSIONS_FILE), Some(who), None));
        }
        files.push((path::join(&COST_DIR, DESKTOP_SESSIONS), None, None));
        let mut m = Marks { files, known, by_sid: Default::default() };
        m.refresh();
        m
    }

    /// 有文件变了（大小、修改时间）就全部重读；返回有没有变
    fn refresh(&mut self) -> bool {
        let mut changed = false;
        for (p, _, stamp) in &mut self.files {
            let now = std::fs::metadata(&*p).ok().map(|st| (st.size(), st.mtime(), st.mtime_nsec()));
            if now != *stamp {
                *stamp = now;
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        let Marks { files, known, by_sid } = self;
        by_sid.clear();
        for (p, who, _) in files.iter() {
            let Ok(mut f) = std::fs::File::open(p) else { continue };
            // SAFETY: f 开着，fd 有效；锁在 f 关上时放开（等正在写、正在截短的写完）
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH) };
            let mut raw = Vec::new();
            if (&mut f).take(4 << 20).read_to_end(&mut raw).is_err() {
                continue;
            }
            drop(f);
            for line in raw.split(|b| *b == b'\n') {
                let Ok(d) = line_doc(line) else { continue };
                let (Some(sid), Some(at)) =
                    (d.str("sid").filter(|s| (8..=64).contains(&s.len())), d.get("at").and_then(|a| a.as_num()))
                else {
                    continue;
                };
                // Claude App 的记录：照 App 登录的账号认（不是这台设备账号表里的官方账号的不认）
                let Some(who) = who.clone().or_else(|| d.str("uuid").and_then(|u| known.get(&*u).cloned())) else { continue };
                by_sid.entry(sid.to_string()).or_default().push((at, who));
            }
        }
        for v in by_sid.values_mut() {
            v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
        true
    }

    /// 会话 sid 在 ts（回复的时间）以前最后一次是在谁下面开始的，和那一次的时间
    fn at(&self, sid: &str, ts: f64) -> Option<(f64, std::rc::Rc<MarkWho>)> {
        self.by_sid.get(sid)?.iter().rev().find(|(at, _)| *at <= ts + MARK_SLACK).map(|(at, w)| (*at, w.clone()))
    }
}

/// 版本号的样子（cca 的、Claude Code 的）：最多 32 个字母、数字和 . _ + -
pub fn version_ok(v: &str) -> bool {
    (1..=32).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// scan.json 的 diag（CostScan::note 记的）：{"cc" 或认不出的原因: [时间, 版本]}
fn diag_ok(d: &V) -> bool {
    d.as_dict().is_some_and(|d| {
        d.items().iter().all(|(k, x)| {
            matches!(&**k, "cc" | "old_claude_code" | "claude_app" | "unknown")
                && x.as_list().is_some_and(|l| l.len() == 2 && is_num(&l.at(0)) && l.at(1).is_str())
        })
    })
}

/// 诊断（local.json 的 diag、别的设备传来的 dg）核对过、只留认得的样子：{"cca": 版本（只有传来的有）, "claude_code": 命令行
/// 最近用的 Claude Code 版本, "why": [{"code": 为什么有回复认不出账号, "last_epoch": 最近一次, "claude_code": 那时的版本}…]}。
/// 没有、不是对象是 None
fn cost_diag_ok(x: Option<V>, now: f64) -> Option<Dict> {
    let x = x?.as_dict()?;
    let out = Dict::new();
    for k in ["cca", "claude_code"] {
        if let Some(v) = x.str(k).filter(|v| version_ok(v)) {
            out.set(k, &*v);
        }
    }
    let mut why = Vec::new();
    for e in x.list("why").map(|l| l.items()).unwrap_or_default().into_iter().take(8) {
        let Some(e) = e.as_dict() else { continue };
        let code = e.str("code").filter(|c| re!(r"^[a-z][a-z_]{0,31}$").matches(c));
        let at = e.get("last_epoch").filter(is_num).and_then(|a| a.as_num()).filter(|a| 0.0 < *a && *a <= now + 86400.0);
        let (Some(code), Some(at)) = (code, at) else { continue };
        let w = dict! { "code" => &*code, "last_epoch" => at };
        if let Some(v) = e.str("claude_code").filter(|v| version_ok(v)) {
            w.set("claude_code", &*v);
        }
        why.push(V::Dict(w));
    }
    out.set("why", why);
    Some(out)
}

/// scan.json 的 diag → local.json 的 diag（cost_diag_ok 的样子，没有 cca）：最近 COST_DAYS+1 天的，原因按名字排；什么都没有是 None
fn cost_diag_view(j: &Dict, now: f64) -> Option<Dict> {
    let lo = now - (COST_DAYS + 1) as f64 * 86400.0;
    let mut items = j.dict("diag")?.items();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let out = Dict::new();
    let mut why = Vec::new();
    for (code, x) in items {
        let l = x.as_list().expect("cost_load_scan 核对过");
        let (at, v) = (l.at(0).as_num().unwrap_or(0.0), l.at(1).py_str());
        if at < lo {
            continue;
        }
        if &*code == "cc" {
            out.set("claude_code", v);
            continue;
        }
        let w = dict! { "code" => &*code, "last_epoch" => at };
        if !v.is_empty() {
            w.set("claude_code", v);
        }
        why.push(V::Dict(w));
    }
    if out.is_empty() && why.is_empty() {
        return None;
    }
    out.set("why", why);
    Some(out)
}

/// 传给别的设备的诊断（这台设备一份，传的每个账号都带着；见 cost_diag_ok）：cca 的版本，local.json 的 diag，最近一次只到天（UTC）。
/// 不传条数
fn cost_dg(local: &Dict, now: f64) -> Dict {
    let dg = cost_diag_ok(local.get("diag"), now).unwrap_or_else(|| dict! { "why" => List::new() });
    if let Some(v) = getenv("CCA_VERSION").filter(|v| version_ok(v)) {
        dg.set("cca", v);
    }
    for w in dg.list("why").expect("刚放进去的").items() {
        let w = w.as_dict().expect("cost_diag_ok 给的");
        w.set("last_epoch", (w.at("last_epoch").as_num().unwrap_or(0.0) / 86400.0).floor() * 86400.0);
    }
    dg
}

/// Claude Code 的这个版本还不写 session-owner
fn claude_code_old(v: &str) -> bool {
    match (super::gui::ver_key(v), super::gui::ver_key(CLAUDE_OWNER_SINCE)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
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
    /// 去重的记录（scan.json 的 seen）
    seen: Seen,
    now: f64,
    first: bool,
    cut: f64,
    records: i64,
    bytes: i64,
    files: i64,
    mains: std::collections::HashSet<String>,
    /// 这一遍看到的会话记录里，各工具（claude、codex）最早的修改时间（cost_keep_old 用）
    oldest: Vec<(&'static str, f64)>,
    /// 会话开始的记录：碰到第一条没有 session-owner 的回复时才读（多数回复用不着它）
    marks: Option<Marks>,
    /// 读这个文件时已经重读过会话开始的记录（新开的会话可能是读完记录以后才开始的：每个文件最多重读一次）
    marks_fresh: bool,
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

/// `hashlib.sha1(data).hexdigest()[:16]` 那 16 位十六进制代表的数（摘要的前 8 个字节）
fn sha1_head(data: &[u8]) -> u64 {
    let d = crate::py::hash::sha1(data);
    u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

/// 字节串里有没有这一段（`needle in raw`）。实测过换成“先找少见的字节”的找法，第一次全读的 CPU 没有变化（0.71 → 0.69 秒 /
/// 416 MB）：时间在解析那些要算的行上，不在这里
fn has(raw: &[u8], needle: &[u8]) -> bool {
    raw.windows(needle.len()).any(|w| w == needle)
}

/// `8 <= len(u) <= 64` 的字符串
fn ident_str(u: Option<V>) -> Option<V> {
    u.filter(|u| u.as_str().is_some_and(|u| (8..=64).contains(&text::len(u))))
}

impl CostScan {
    fn new(j: &Dict, seen: Seen, now: f64) -> CostScan {
        let first = !j.at("done").truthy();
        CostScan {
            j: j.clone(),
            seen,
            now,
            first,
            cut: now - COST_SEEN,
            records: 0,
            bytes: 0,
            files: 0,
            mains: Default::default(),
            oldest: Vec::new(),
            marks: None,
            marks_fresh: false,
        }
    }

    /// 会话 sid 在 ts 以前最后一次是在谁下面开始的、是什么时候（会话开始的记录）；这个文件里还没重读过记录时，找不到就重读一次再找
    fn mark_who(&mut self, sid: &str, ts: f64) -> Option<(f64, std::rc::Rc<MarkWho>)> {
        if self.marks.is_none() {
            self.marks = Some(Marks::load());
            self.marks_fresh = true;
        }
        let marks = self.marks.as_mut().expect("刚读的");
        if let Some(w) = marks.at(sid, ts) {
            return Some(w);
        }
        if self.marks_fresh {
            return None;
        }
        self.marks_fresh = true;
        if marks.refresh() {
            marks.at(sid, ts)
        } else {
            None
        }
    }

    /// 算进来的一条 Claude 回复：记下这台设备命令行里最近用的 Claude Code 版本（scan.json 的 diag 里的 cc），认不出账号的
    /// 再记下为什么认不出（diag 里的 old_claude_code、claude_app、unknown）。都是 [最近一次的时间, Claude Code 的版本]，不记条数；
    /// 同一个版本隔 10 分钟以上才改（每条回复都写一遍太费）
    fn note(&self, d: &Dict, ts: f64, orphan: bool) {
        let version = d.str("version").filter(|v| version_ok(v));
        let desktop = d.get("entrypoint").is_some_and(|e| e.as_str() == Some("claude-desktop"));
        let why = if !orphan {
            None
        } else if desktop {
            Some("claude_app")
        } else if version.as_ref().is_some_and(|v| claude_code_old(v)) {
            Some("old_claude_code")
        } else {
            Some("unknown")
        };
        let cc = if desktop || version.is_none() { None } else { Some("cc") };
        let v = version.as_deref().unwrap_or("");
        for code in [cc, why].into_iter().flatten() {
            let diag = sub_dict(&self.j, "diag");
            let (last, last_v) = match diag.list(code) {
                Some(l) => (l.at(0).as_num().unwrap_or(0.0), l.at(1).py_str()),
                None => (0.0, String::new()),
            };
            if ts > last && (ts - last >= 600.0 || last_v != v) {
                diag.set(code, list![ts, v]);
            }
        }
    }

    /// key 是 (c 或 x, 摘要的前 8 个字节)：原来写成一个字符加 16 位十六进制的字符串。算进来了返回 true
    fn add(
        &mut self,
        tool: &str,
        who: &V,
        ts: Option<f64>,
        key: (u8, u64),
        micro: Option<f64>,
        tok: [i64; 4],
        model: &str,
    ) -> bool {
        let Some(ts) = ts else { return false };
        if !(1e9..=self.now + 2.0 * 86400.0).contains(&ts) || (!self.first && ts < self.cut) || self.seen.has(key.0, key.1) {
            return false;
        }
        let hour = ((ts / 3600.0).floor() * 3600.0) as i64;
        self.seen.add(key.0, key.1, hour);
        let ident = if who.truthy() { format!("{}:{}", tool, who.py_str()) } else { String::new() };
        let rows = if ident.is_empty() {
            self.j.dict("orphan").expect("scan.json 核对过")
        } else {
            self.j.dict("h").expect("scan.json 核对过").setdefault(&ident, Dict::new()).as_dict().expect("scan.json 核对过")
        };
        let key = hour.to_string();
        let b = rows.setdefault(&key, list![0, 0, 0, 0, 0, 0, 0]).as_list().expect("scan.json 核对过");
        // 按模型分的同一份（hm 里每个身份一份，认不出是谁的在 om）
        let mrows = if ident.is_empty() {
            self.j.dict("om").expect("scan.json 核对过")
        } else {
            sub_dict(&self.j.dict("hm").expect("scan.json 核对过"), &ident)
        };
        let mb = sub_dict(&mrows, &key).setdefault(model, list![0, 0, 0, 0, 0, 0, 0]).as_list().expect("scan.json 核对过");
        let bump = |i: usize, by: i64| {
            for r in [&b, &mb] {
                r.set(i, r.at(i as isize).as_int().unwrap_or(0) + by);
            }
        };
        bump(1, 1);
        match micro {
            None => {
                bump(6, 1);
                if !ident.is_empty() && !model.is_empty() {
                    // 没有标价的模型：记最后一次见到它的小时（过了 COST_KEEP 就不列）
                    let ms = self
                        .j
                        .dict("um")
                        .expect("scan.json 核对过")
                        .setdefault(&ident, Dict::new())
                        .as_dict()
                        .expect("scan.json 核对过");
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
        true
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
        self.marks_fresh = false;
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
            let done =
                if kind == FileKind::Rollout { self.codex_line(&cur, &raw) } else { self.claude_line(kind, &cur, sid, &raw) };
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
            // 换了（这个文件里头一回有、和上一行的不一样）：从这以后的第一条回复起作用，那时再记下时间（own_at）
            if !cur.get("own").unwrap_or(V::None).py_eq(u.as_ref().unwrap_or(&V::None)) {
                cur.pop("own_at");
            }
            cur.set("own", u.clone());
            if kind != FileKind::Main {
                return Ok(());
            }
            // 主会话：owners 里先有它这一项（没认出账号也有，是空的）
            let tl =
                self.j.dict("owners").expect("scan.json 核对过").setdefault(sid, List::new()).as_list().expect("scan.json 核对过");
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
        // 'c' + sha1(…).hexdigest()[:16]
        let key = match m.get("id") {
            Some(V::Str(mid)) => {
                (b'c', sha1_head(format!("{}\0{}", mid, d.get("requestId").unwrap_or(V::None).py_str()).as_bytes()))
            }
            _ => (b'c', sha1_head(raw)),
        };
        let mut who = cur.get("own").unwrap_or(V::None);
        // who 从什么时候起是它：这个文件里的 session-owner 换了以后的第一条回复的时间；子代理的记录按会话的 owners 里的时间
        let mut since = 0.0;
        if kind == FileKind::Main {
            cur.set("ts", ts);
        }
        if who.truthy() {
            since = match cur.get("own_at").and_then(|a| a.as_pynum()) {
                Some(at) => at,
                None => {
                    cur.set("own_at", ts);
                    ts
                }
            };
        } else if kind != FileKind::Main {
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
                    since = at;
                }
            }
        }
        // 会话开始的记录（见 Marks）：没有 session-owner，或者记录比 session-owner 起作用的时候新（换了账号续接以后 Claude Code
        // 照旧写原来的账号），按记录认
        if let Some((at, m)) = self.mark_who(sid, ts) {
            if !who.truthy() || at > since {
                match &*m {
                    MarkWho::Api => return Ok(()), // 第三方 API 账号的会话：不是订阅的用量，不算
                    MarkWho::Official(Some(u)) => who = u.clone(),
                    MarkWho::Official(None) => {} // 读不出是谁：照 session-owner（没有就认不出）
                }
            }
        }
        let model = cost_model(m.get("model").as_ref());
        let (micro, tok) = cost_claude(&model, &u);
        if self.add("claude", &who, Some(ts), key, micro, tok, &model) {
            self.note(&d, ts, !who.truthy());
        }
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
            let key = (b'x', sha1_head(format!("{}\0{}", stamp.unwrap_or(V::None).py_str(), tot).as_bytes()));
            let model = cur.get("model").filter(|m| m.truthy()).map_or(String::new(), |m| m.py_str());
            let tier = cur.get("tier");
            let (micro, tok) = cost_codex(&model, tier.as_ref().and_then(|t| t.as_str()), ts, &last);
            self.add("codex", &cur.get("who").unwrap_or(V::None), Some(ts), key, micro, tok, &model);
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
            let s = p
                .filter(|p| p.get("type").is_some_and(|t| t.as_str() == Some("thread_settings_applied")))
                .and_then(|p| p.dict("thread_settings"));
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
fn cost_scan(j: &Dict, seen: Seen, now: f64, checkpoint: &mut dyn FnMut(&Seen) -> R<()>) -> R<CostScan> {
    let mut s = CostScan::new(j, seen, now);
    let mut live = std::collections::HashSet::new();
    for (kind, p, sid) in cost_files() {
        s.file(kind, &p, &sid)?;
        live.insert(p);
        checkpoint(&s.seen)?;
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
fn cost_prune(j: &Dict, seen: &mut Seen, now: f64) {
    let lo = now - COST_KEEP;
    let (h, hm) = (j.dict("h").expect("scan.json 核对过"), j.dict("hm").expect("scan.json 核对过"));
    let mut tables: Vec<Dict> = h.values().iter().filter_map(|r| r.as_dict()).collect();
    tables.push(j.dict("orphan").expect("scan.json 核对过"));
    tables.extend(hm.values().iter().filter_map(|r| r.as_dict()));
    tables.push(j.dict("om").expect("scan.json 核对过"));
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
    let kept_m = Dict::new();
    for (i, r) in hm.items() {
        if r.truthy() {
            kept_m.set(i, r);
        }
    }
    j.set("hm", kept_m);
    seen.retain(now - COST_SEEN - 3600.0);
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
                let x = codex_claims(&k, None);
                found.push(("codex", x.get("user"), x.get("email")));
            }
        }
    }
    let (u, e) = oauth_ident(&MASTER_JSON);
    found.push(("claude", u, e));
    let x = codex_claims("", Some(&path::join(&codex().master, "auth.json")));
    found.push(("codex", x.get("user"), x.get("email")));
    let who = j.dict("who").expect("scan.json 核对过");
    for (tool, ident, email) in found {
        let ident = ident.as_ref().and_then(|i| i.as_str()).filter(|i| (8..=64).contains(&text::len(i)));
        let email = email.as_ref().and_then(|e| e.as_str()).filter(|e| EMAIL_RE.matches(e));
        if let (Some(ident), Some(email)) = (ident, email) {
            who.set(format!("{tool}:{ident}"), email.to_lowercase());
        }
    }
}

/// local.json（_gui state、同步读它）：最近 COST_PUBLISH 的小时数据（acc；按模型分的另一份在 hm）、身份 → 邮箱、没有标价的模型、
/// 认不出是谁的（orphan，按模型分的在 om；3.6.7 及以前只留最近 31 天：只给这台设备说一句，现在也传给别的设备）。
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
        let mut models: Vec<std::rc::Rc<str>> = ms
            .as_dict()
            .expect("scan.json 核对过")
            .items()
            .into_iter()
            .filter(|(_, x)| x.as_num().is_some_and(|x| x >= lo))
            .map(|(m, _)| m)
            .collect();
        models.sort();
        if !models.is_empty() {
            um.set(i, V::List(List::from_vec(models.into_iter().map(V::from).collect())));
        }
    }
    let hm = Dict::new();
    for (i, rows) in j.dict("hm").expect("scan.json 核对过").items() {
        let r = recent(&rows, lo);
        if !r.is_empty() {
            hm.set(i, r);
        }
    }
    let doc = dict! {
        "v" => 1, "build" => COST_BUILD, "who" => j.at("who"), "acc" => acc, "um" => um, "orphan" => recent(&j.at("orphan"), lo),
        "hm" => hm, "om" => recent(&j.at("om"), lo),
    };
    if let Some(diag) = cost_diag_view(j, now) {
        doc.set("diag", diag);
    }
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

/// 拿 cost.lock（lock_path）：同时只有一个统计在跑。最多等正在跑的那个 wait 秒，等不到是 CostBusy。锁在返回的文件关上时放开
fn cost_lock(lock_path: &str, wait: f64) -> R<std::fs::File> {
    let lk = std::fs::OpenOptions::new().append(true).create(true).open(lock_path).map_err(|e| at_path(e, lock_path))?;
    let until = monotonic() + wait;
    // SAFETY: lk 开着，fd 有效
    while unsafe { libc::flock(lk.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if monotonic() >= until {
            return Err(cost_busy());
        }
        sleep(0.5);
    }
    Ok(lk)
}

/// 统计一次：读新增的会话记录、发布 local.json，再和别的设备同步（到时候了才同步；sync_now 马上）。
/// 同时只有一个（cost.lock）：wait 是最多等正在跑的那个多久，等不到是 CostBusy。返回 (统计的结果, 同步的结果)（PROTOCOL 3.5）
fn cost_run(sync_now: bool, wait: f64) -> R<(Dict, Dict)> {
    cost_dir().map_err(|e| at_path(e, &COST_DIR))?;
    let _lk = cost_lock(&path::join(&COST_DIR, "cost.lock"), wait)?;
    let me = std::process::id() as i64;
    cost_save(
        "worker.json",
        &V::Dict(dict! { "v" => 1, "pid" => me, "started" => proc_starts(&[me]).get(&me).copied().unwrap_or(0) }),
    )?;
    let run = || -> R<(Dict, Dict)> {
        let now = time();
        let (j, seen) = cost_load_scan();
        let first = !j.at("done").truthy();
        let mut last = monotonic();
        let mut checkpoint = |seen: &Seen| -> R<()> {
            if monotonic() - last >= 20.0 {
                scan_save(&j, seen)?;
                last = monotonic();
            }
            Ok(())
        };
        let mut s = cost_scan(&j, seen, now, &mut checkpoint)?;
        cost_who(&j);
        cost_prune(&j, &mut s.seen, now);
        scan_save(&j, &s.seen)?;
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

/// 别的设备传来的明文：{"v": 1, "at": 时间, "h": {小时: [7 个计数]}, "um": [没有标价的模型…], "dg": 诊断（cost_dg；以前的版本没有）,
/// "dv": 那台机器的 device_id（以前的版本、没有 device_id 的设备没有）, "hm": 按模型分的 {小时: {模型: [7 个计数]}}（3.6.7 及以前没有）}；
/// 格式不对是 None（dg、dv、hm 不对只是不要它）
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
    let models: Vec<V> = um
        .items()
        .into_iter()
        .filter(|m| m.as_str().is_some_and(|m| text::isprintable(m) && text::len(m) <= 64))
        .take(32)
        .collect();
    let out = dict! { "at" => at, "h" => h, "um" => models };
    if let Some(dg) = cost_diag_ok(d.get("dg"), now) {
        out.set("dg", dg);
    }
    if let Some(dv) = d.str("dv").filter(|v| DV_RE.fullmatch(v).is_some()) {
        out.set("dv", &*dv);
    }
    let hm =
        d.get("hm").filter(|x| cost_mrows_ok(Some(x), COST_MODELS_HOUR) && x.as_dict().is_some_and(|x| x.len() <= 24 * 7 * 20));
    if let Some(hm) = hm {
        out.set("hm", hm);
    }
    Some(out)
}

/// 机器的标识（config.json 的 device_id，router.rs 的 device_id 生成的：16 位小写十六进制）
static DV_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[0-9a-f]{16,64}$"));

/// 这台机器的 device_id（config.json 里有、格式对才算；同步路由器时生成）：折合金额的密文里写着它（dv）
pub fn my_dv() -> Option<String> {
    let c = load_quiet(&CFG)?.as_dict()?;
    c.str("device_id").filter(|d| DV_RE.fullmatch(d).is_some()).map(|d| d.to_string())
}

/// 服务器上别的设备的那一份解开（label 是封它用的标签）；认证不过、格式不对（伪造的、别的账号的、坏了）是 None
fn cost_open(keys: &Keys, label: &str, aid: &str, fp: &str, blob: &V, now: f64) -> R<Option<Dict>> {
    let Some(pt) = blob_open(keys, &blob_ad(&[label, aid, fp])?, blob) else { return Ok(None) };
    let n = pt.get(..4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let body = pt.get(4..).unwrap_or(&[]);
    let Some((raw, tail)) = zlib::decompress_limited(&body[..n.min(body.len())], 4 << 20) else { return Ok(None) };
    let Some(doc) = std::str::from_utf8(&raw).ok().and_then(|t| json::loads(t).ok()) else { return Ok(None) };
    Ok(if n <= body.len() && !tail { cost_doc(&doc, now) } else { None })
}

/// 把一个小时的 7 个计数（核对过的）加进 into 的 k 那一项
fn add_counts(into: &Dict, k: &str, r: &V) {
    let r = r.as_list().expect("核对过");
    let b = into.setdefault(k, list![0, 0, 0, 0, 0, 0, 0]).as_list().expect("刚放进去的");
    for i in 0..7 {
        b.set(i, b.at(i as isize).as_int().unwrap_or(0) + r.at(i as isize).as_int().unwrap_or(0));
    }
}

/// 这台设备上这个账号的小时数据：这个工具的、登录的邮箱是它的身份加起来（新的，可以改）；按模型分的同样加起来
/// （local.json 的 hm；以前的版本写的 local.json 没有，是空的）；和没有标价的模型
pub fn cost_content(local: &Dict, tool: &Tool, email: &str) -> (Dict, Dict, Vec<String>) {
    let who = cost_who_kept(&[local]);
    let (h, hm) = (Dict::new(), Dict::new());
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
                add_counts(&h, &k, &r);
            }
            let mrows = local.dict("hm").and_then(|x| x.get(&ident)).filter(|r| cost_mrows_ok(Some(r), usize::MAX));
            for (k, ms) in mrows.and_then(|r| r.as_dict()).map(|r| r.items()).unwrap_or_default() {
                let dst = sub_dict(&hm, &k);
                for (m, r) in ms.as_dict().expect("cost_mrows_ok 核对过").items() {
                    add_counts(&dst, &m, &r);
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
    (h, hm, um)
}

/// 这台设备认不出是哪个账号的小时数据：local.json 的 orphan、按模型分的 om（格式不对的当没有）；和其中没有标价的模型
fn orphan_content(local: &Dict) -> (Dict, Dict, Vec<String>) {
    let pick = |k: &str, ok: &dyn Fn(&V) -> bool| local.get(k).filter(|x| ok(x)).and_then(|x| x.as_dict()).unwrap_or_default();
    let h = pick("orphan", &|x| cost_rows_ok(Some(x)));
    let hm = pick("om", &|x| cost_mrows_ok(Some(x), usize::MAX));
    let mut um: Vec<String> = Vec::new();
    for (_, ms) in hm.items() {
        for (m, r) in ms.as_dict().expect("cost_mrows_ok 核对过").items() {
            let unpriced = r.as_list().is_some_and(|r| r.at(6).as_int().unwrap_or(0) > 0);
            if unpriced && !m.is_empty() && !um.iter().any(|x| **x == *m) {
                um.push(m.to_string());
            }
        }
    }
    um.sort();
    (h, hm, um)
}

/// 要同步的账号：(工具, 邮箱小写, slot（工具名:邮箱，sync.json、remote.json 里的键）, 账号摘要, 密钥（Team 共用的），
/// 3.6.5 及以前的密钥（这台设备登录着这个账号时才有：还没升级的设备传的靠它解；有它也说明登录着）)。
/// 认不出是哪个账号的那一份（cost_orphan_item）也是一项：工具是 None，邮箱是空的，slot 是 COST_ORPHAN_SLOT
struct Item {
    tool: Option<&'static Tool>,
    email: String,
    slot: String,
    aid: String,
    keys: Keys,
    keys_v1: Option<Keys>,
}

impl Item {
    /// 解开 fp 那台设备传的：先用 Team 共用的密钥，再用以前的；解不开、内容不对是 None
    fn open(&self, fp: &str, blob: &V, now: f64) -> R<Option<Dict>> {
        if let Some(doc) = cost_open(&self.keys, COST_LABEL, &self.aid, fp, blob, now)? {
            return Ok(Some(doc));
        }
        match &self.keys_v1 {
            Some(keys) => cost_open(keys, COST_LABEL_V1, &self.aid, fp, blob, now),
            None => Ok(None),
        }
    }
}

/// 和这台服务器同步的账号：走它的 Claude、Codex 官方账号（Team 给了这台设备的；3.6.6 起登录没登录都取，自己的那一份只传用过的、
/// 登录着的：cost_round）。账号表读不了是 None
fn cost_items(hub: &Hub) -> R<Option<Vec<Item>>> {
    let Some(targets) = presence_targets(hub.kind) else { return Ok(None) };
    let salt = hub.salt();
    let mut out = Vec::new();
    for (tool, key, _) in targets {
        let email = parse_key(&key).1.to_lowercase();
        if !matches!(tool.kind, Kind::Claude | Kind::Codex) || email.is_empty() {
            continue;
        }
        let ident = {
            let _using = Using::tool(tool);
            share_ident(tool, &key)
        };
        out.push(Item {
            tool: Some(tool),
            slot: format!("{}:{}", tool.name, email),
            aid: presence_aid(&salt, tool.name, &email)?,
            keys: team_keys(&salt, COST_LABEL)?,
            keys_v1: ident.map(|i| keyshare_keys(&salt, tool.name, &email, &i, COST_LABEL_V1)).transpose()?,
            email,
        });
    }
    Ok(Some(out))
}

/// 认不出是哪个账号的那一份（COST_ORPHAN）：跟着这台服务器的账号一起同步，密钥是 Team 共用的那一把
fn cost_orphan_item(hub: &Hub) -> R<Item> {
    let salt = hub.salt();
    Ok(Item {
        tool: None,
        email: String::new(),
        slot: COST_ORPHAN_SLOT.to_string(),
        aid: presence_aid(&salt, COST_ORPHAN, "")?,
        keys: team_keys(&salt, COST_LABEL)?,
        keys_v1: None,
    })
}

/// 和别的设备交换：用得了的每一台同步服务器各一轮（cost_hub）。返回 PROTOCOL 3.5 的 sync：{"state", "message", "checked_epoch"} 是
/// 各台里最要紧的那一台的（error、waiting、ok、skipped 依次；一台都没有是 skipped，checked_epoch 是 null；old_server 只有以前维护者的
/// 那一台给过，3.6.0 起不会有），"hubs" 是各台的（多一项 "kind"、"team"）
fn cost_sync(force: bool) -> R<Dict> {
    let mut per: Vec<(HubKind, Dict)> = Vec::new();
    for hub in hubs()? {
        per.push((hub.kind, cost_hub(&hub, force)?));
    }
    let rank = |r: &Dict| ["error", "waiting", "ok"].iter().position(|x| r.str("state").as_deref() == Some(*x)).unwrap_or(3);
    let out = match per.iter().min_by_key(|(_, r)| rank(r)) {
        Some((_, r)) => r.copy(),
        None => dict! { "state" => "skipped", "message" => V::None, "checked_epoch" => V::None },
    };
    let each: Vec<V> = per
        .iter()
        .map(|(kind, r)| {
            let d = dict! { "kind" => kind.name(), "team" => kind.team() };
            d.update(r);
            V::Dict(d)
        })
        .collect();
    out.set("hubs", each);
    Ok(out)
}

/// 和一台服务器交换走它的那些账号的（见上面这一节的说明；状态在这台的 sync.json、remote.json 里）。force 以外最多 COST_SYNC_EVERY
/// 一次。返回 {"state", "message", "checked_epoch"}
fn cost_hub(hub: &Hub, force: bool) -> R<Dict> {
    let (sync_file, remote_file) = (hub.file("sync.json"), hub.file("remote.json"));
    let st = Some(cost_json(&sync_file)).filter(|s| !s.is_empty()).unwrap_or_else(|| dict! { "v" => 1 });
    let res = |state: &str, msg: Option<V>| -> Dict {
        dict! { "state" => state, "message" => msg, "checked_epoch" => st.get("ok_at").filter(is_num) }
    };
    let now = time();
    let num_ = |k: &str| st.get(k).filter(is_num).and_then(|v| v.as_num()).unwrap_or(0.0);
    // 在未来的（时钟快过）不当真
    if !force && (0.0..COST_SYNC_EVERY).contains(&(now - num_("at"))) {
        let bad = st.str("state").as_deref() == Some("error");
        let state = if bad {
            "waiting"
        } else if st.truthy("ok_at") {
            "ok"
        } else {
            "skipped"
        };
        return Ok(res(state, if bad { st.get("error") } else { None }));
    }
    let Some(mut items) = cost_items(hub)? else { return Ok(res("error", Some(V::from("读不了账号表")))) };
    if items.is_empty() {
        // 没有要同步的账号了（Team 不再给、移走了）：上次没同步成的原因不再算数（不然 config.hubs 一直说没同步成）
        if st.str("state").as_deref() == Some("error") {
            st.set("state", "skipped");
            st.set("error", V::None);
            cost_save(&sync_file, &V::Dict(st.clone()))?;
        }
        return Ok(res("skipped", None));
    }
    // 认不出是哪个账号的那一份：Team 给了这台设备账号时才传（各个这样的 Team 各传一份，别的设备按机器的标识认成一台）
    items.push(cost_orphan_item(hub)?);
    let rem = Some(cost_json(&remote_file)).filter(|r| !r.is_empty()).unwrap_or_else(|| dict! { "v" => 1, "acc" => Dict::new() });
    if rem.dict("acc").is_none() {
        rem.set("acc", Dict::new());
    }
    let old_rem = V::Dict(rem.clone()).deepcopy();
    st.set("at", now);
    let err = cost_round(&hub.cfg, &items, &cost_json("local.json"), &st, &rem, now)?;
    st.set("state", if err.is_empty() { "ok" } else { "error" });
    st.set("error", if err.is_empty() { V::None } else { V::from(err.as_str()) });
    if err.is_empty() {
        st.set("ok_at", now);
    }
    if !V::Dict(rem.clone()).py_eq(&old_rem) {
        cost_save(&remote_file, &V::Dict(rem.clone()))?;
    }
    cost_save(&sync_file, &V::Dict(st.clone()))?;
    Ok(res(&st.at("state").py_str(), Some(st.at("error"))))
}

/// `d.setdefault(k, {})`，里面不是对象的原来后面是 AttributeError / TypeError
fn sub_dict(d: &Dict, k: &str) -> Dict {
    match d.setdefault(k, Dict::new()) {
        V::Dict(x) => x,
        other => panic!("AttributeError: {k} 那一项是 {}，不是对象", other.py_repr()),
    }
}

/// 一轮：问服务器这些账号各台设备的（带着这台设备已经有的认证码：没变的不回；一次最多 COST_BATCH 个账号），解开、存进 rem
/// （remote.json 的内容）；这台设备的变了、服务器上没有或者不是上次传的、隔了 COST_BEAT 就传上去。st（sync.json 的内容）原地改。
/// 返回没成的原因（成了是空的）
fn cost_round(cfg: &Dict, items: &[Item], local: &Dict, st: &Dict, rem: &Dict, now: f64) -> R<String> {
    let (accs, racc) = (sub_dict(st, "acc"), rem.dict("acc").expect("cost_sync 保证了是字典"));
    let (cost, missing) = (Dict::new(), Dict::new());
    for batch in items.chunks(COST_BATCH) {
        let get: Vec<V> = batch.iter().map(|x| V::from(x.aid.as_str())).collect();
        let (code, r) = presence_post(cfg, "ledger", &dict! { "get" => get, "have" => cost_have(cfg, batch, &accs, &racc) }, 20)?;
        let Some(got) = r.dict("cost").filter(|_| code == 200) else {
            return Ok(if code != 200 {
                presence_why(cfg, code, &r)?
            } else {
                "设备统计的服务器回的内容看不懂".to_string()
            });
        };
        cost.update(&got);
        missing.update(&r.dict("missing").unwrap_or_default());
        if let Some(me) = r.str("me_name").filter(|n| text::isprintable(n)) {
            st.set("me", text::head(&me, 64));
        }
    }
    let fp = cfg.at("fp").py_str();
    let dv = my_dv();
    let puts = Dict::new();
    let mut sent: Vec<(String, V, String)> = Vec::new();
    for x in items {
        let a = sub_dict(&accs, &x.slot);
        let last = match racc.get(&x.slot).filter(|o| o.truthy()) {
            None => Dict::new(),
            Some(V::Dict(o)) => o,
            Some(other) => panic!("AttributeError: remote.json 里 {} 那一项是 {}", x.slot, other.py_repr()),
        };
        let (new, mine) = cost_take(cfg, cost.get(&x.aid), x, &last, now)?;
        if new.is_empty() {
            racc.pop(&x.slot);
        } else {
            racc.set(x.slot.as_str(), new);
        }
        a.set("missing", cost_missing_view(missing.get(&x.aid), now));
        let (h, hm, um) = match x.tool {
            Some(tool) => cost_content(local, tool, &x.email),
            None => orphan_content(local),
        };
        if x.tool.is_some() && h.is_empty() && x.keys_v1.is_none() {
            // 自己的那一份只传用过（有记录）或者登录着的账号：没用过的传上去是空的，别的设备的账号页里会多出一台金额是 0 的
            // （带着这台设备的诊断）；服务器给每台设备存的账号也有上限（COST_SLOTS）。认不出是谁的那一份空的也传：
            // 别的设备据此分得清这台设备没有认不出的用量，还是 cca 太旧、不传
            continue;
        }
        let recent = |rows: &Dict| -> Dict {
            let out = Dict::new();
            for (k, v) in rows.items() {
                if hour_of(&k) >= now - COST_PUBLISH {
                    out.set(k, v);
                }
            }
            out
        };
        let doc = dict! {
            "h" => recent(&h), "hm" => recent(&hm), "um" => V::List(List::from_vec(um.iter().map(|m| V::from(m.as_str())).collect())),
            "dg" => cost_dg(local, now),
        };
        if let Some(dv) = &dv {
            doc.set("dv", dv.as_str()); // 这台机器（别的设备据此把它在两台服务器上的认成一台）
        }
        // 内容的摘要（没变就不重传）带着标签：换了密钥以后（3.6.6 起是 Team 共用的）重传一次，没登录的设备也解得开了
        let body = json::dumps(&V::Dict(doc.clone()), json::Fmt::new().compact().sort_keys());
        let s = sha256_hex(format!("{COST_LABEL}\0{body}"))[..16].to_string();
        let up = a.get("up_at").filter(is_num).and_then(|u| u.as_num()).unwrap_or(0.0);
        let same_tag = |mine: &Dict| match (mine.get("tag"), a.get("tag")) {
            (None, None) => true,
            (Some(m), Some(t)) => m.py_eq(&t),
            (Some(v), None) | (None, Some(v)) => v.is_none(),
        };
        let unchanged = mine.as_ref().is_some_and(same_tag)
            && a.get("sum").is_some_and(|x| x.as_str() == Some(s.as_str()))
            && (0.0..COST_BEAT).contains(&(now - up));
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
            let Some(e) = e.as_dict() else { continue };
            if x.keys_v1.is_some() && e.truthy("no_v1") {
                continue; // 上次没有以前的密钥、这一轮有了：不带认证码，让服务器重发，再解一次（cost_take）
            }
            if let Some(tag) = e.get("tag").filter(|t| t.is_str()) {
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

/// 服务器回的这个账号各台设备的 → ({设备指纹: remote.json 里的一项}, 这台设备自己的那一行 | None)。带着密文的解开（Item::open）；
/// 解不开的只记下认证码和名字，没变就不再取（cost_have），视图里没有它：两把密钥都试过了的（伪造的、别的 Team 的、坏了）就这样；
/// 这一轮没有以前的密钥的（没登录这个账号，或者 .claude.json 正写到一半：可能是还没升级的设备封的）另记一笔 no_v1，以前解开过的
/// 先留着，有了以前的密钥时 cost_have 不带它的认证码、让服务器重发。没带密文的（没变，或者这次回得太多没给）用上次的（last）。
/// 从 Team 里移除了的设备（以前的服务器还回来，标着 revoked）不要
fn cost_take(cfg: &Dict, rows: Option<V>, x: &Item, last: &Dict, now: f64) -> R<(Dict, Option<Dict>)> {
    let (new, mut mine) = (Dict::new(), None);
    let my_fp = cfg.at("fp").py_str();
    for row in rows.and_then(|r| r.as_list()).map(|r| r.items()).unwrap_or_default() {
        let Some(row) = row.as_dict() else { continue };
        let (Some(fp), Some(tag)) = (row.str("fp"), row.str("tag")) else { continue };
        if matches!(row.get("me"), Some(V::Bool(true))) || *fp == *my_fp {
            mine = Some(row);
            continue;
        }
        if matches!(row.get("revoked"), Some(V::Bool(true))) {
            continue;
        }
        let e = if let Some(blob) = row.get("blob").filter(|b| b.is_dict()) {
            let e = match x.open(&fp, &blob, now)? {
                Some(doc) => doc,
                None if x.keys_v1.is_none() && x.tool.is_some() => {
                    let e = last.dict(&fp).filter(|p| p.has("h")).map(|p| p.copy()).unwrap_or_default();
                    e.set("no_v1", true);
                    e
                }
                None => Dict::new(),
            };
            e.set("tag", &*tag);
            e
        } else if let Some(prev) = last.dict(&fp).filter(|p| !p.is_empty()) {
            prev.copy() // 认证码照旧：没给的下次再取
        } else {
            continue;
        };
        let name = row.str("name").filter(|n| !n.is_empty() && text::isprintable(n));
        e.set("name", name.map_or("?".to_string(), |n| text::head(&n, 64).to_string()));
        new.set(fp, e);
    }
    Ok((new, mine))
}

/// 服务器说 30 天内用过这个账号、却没传过折合金额的设备 → [{name, used_epoch}]（最多 32 台）
fn cost_missing_view(ms: Option<V>, now: f64) -> V {
    let mut out = Vec::new();
    for m in ms.and_then(|m| m.as_list()).map(|m| m.items()).unwrap_or_default() {
        let Some(m) = m.as_dict() else { continue };
        let (Some(name), Some(age)) =
            (m.str("name").filter(|n| text::isprintable(n)), m.get("used_age").filter(is_num).and_then(|a| a.as_num()))
        else {
            continue;
        };
        out.push(V::Dict(dict! { "name" => text::head(&name, 64), "used_epoch" => num::round(now - age) }));
        if out.len() == 32 {
            break;
        }
    }
    V::List(List::from_vec(out))
}

/// 封好这台设备这个账号的数据（cost_seal）；太大了（很少见：14 周里小时很多、又压缩不了）就去掉最早的四分之一的小时，
/// 直到放得下 COST_BLOB_MAX。先去掉按模型分的（hm：只是明细，合计在 h 里；别的设备那里，少了的那一部分算作没有按模型记下的），
/// 都去掉了还放不下再去掉 h 的
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
        let (h, hm) = (doc.dict("h").expect("调用的地方给的"), doc.dict("hm").unwrap_or_default());
        if text::len(&blob.at("c").py_str()) <= COST_BLOB_MAX || (h.is_empty() && hm.is_empty()) {
            return Ok(blob);
        }
        let (which, rows) = if hm.is_empty() { ("h", h) } else { ("hm", hm) };
        let mut ks = rows.keys();
        ks.sort_by_key(|k| text::to_int(k).unwrap_or(0));
        let kept = Dict::new();
        for k in &ks[ks.len() / 4 + 1..] {
            kept.set(k.clone(), rows.at(k));
        }
        doc.set(which, kept);
        blob = seal(&doc)?;
    }
}

/// 把这几份（{账号摘要: 密文}）传上去：一次最多 COST_PUT_MAX、COST_BATCH 份，多了分几次。返回没成的原因（成了是空的）
fn cost_put(cfg: &Dict, puts: &Dict) -> R<String> {
    let mut batches = vec![Dict::new()];
    let mut size = 0;
    for (aid, blob) in puts.items() {
        let len = text::len(&blob.as_dict().expect("cost_seal_fit 给的").at("c").py_str());
        let last = batches.last().expect("至少有一份");
        if !last.is_empty() && (size + len > COST_PUT_MAX || last.len() >= COST_BATCH) {
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
        if code != 200 {
            return presence_why(cfg, code, &r);
        }
    }
    Ok(String::new())
}

/// 在后台跑一次 cca _cost-sync（读新增的会话记录、和别的设备同步）；上次起过还不到 COST_EVERY 就不起。静默。
/// CCA_COST=off（回归测试）不起。关掉了折合金额时不统计，只在 cca cost hook 加的 hook 还在时起它去掉 hook（关掉以后 App 和
/// 这里都不再统计，不然要等手动 cost sync；调这里的可能拿着 sync.lock，所以在后台改）
pub fn cost_kick() {
    if getenv("CCA_COST").as_deref() == Some("off") || !path::isfile(&CFG) {
        return;
    }
    if !cost_on() && !(cfg!(target_os = "macos") && desktop_hook_on()) {
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

/// cca _cost-sync：cost_kick 在后台起的；有别的统计在跑就走。出什么错都安静地退出（下次再来）。关掉了折合金额时只去掉
/// cca cost hook 加的 hook（见 cost_kick）
pub fn cmd_cost_worker(_args: &[String]) -> R<()> {
    if !path::isfile(&CFG) {
        return Ok(());
    }
    if !cost_on() {
        if desktop_hook_on() {
            let _ = desktop_hook_set(false);
        }
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
        // cca cost hook 加的 hook 也去掉（关掉折合金额时去掉；改不了就算了，cca cost hook off 会说为什么）
        if desktop_hook_on() {
            let _ = desktop_hook_set(false);
        }
        out!("折合金额关掉了（config.json 里写着 \"cost\": false）");
        return exit(2);
    }
    if cost_accounts() == 0 {
        out!("没有 Claude、Codex 的官方账号：没有可统计的");
        return exit(2);
    }
    let (scan, sync) = match cost_run(flag("--now"), COST_WAIT as f64) {
        Ok(done) => done,
        Err(e) => return cost_sync_failed(e),
    };
    if flag("--json") {
        let doc = dict! { "version" => 1, "kind" => "cost_sync", "generated_epoch" => time(), "scan" => scan, "sync" => sync };
        out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    let tail = match sync.at("state").as_str() {
        Some("ok") => "；和其他设备同步好了".to_string(),
        Some("error") => format!("；和其他设备同步没成：{}", sync.at("message").py_str()),
        _ => String::new(),
    };
    out!("统计好了：看了 {} 个会话记录，新算进来 {} 条{}", scan.at("files").py_str(), scan.at("records").py_str(), tail);
    Ok(())
}

/// cca cost sync 没统计成（cost_run 出的错）：另一个统计一直没结束、读写出了错的说原因（stdout）、退出码 1；别的错照旧往上传
fn cost_sync_failed(e: Fail) -> R<()> {
    if e.is("CostBusy") {
        out!("另一个统计一直没结束（等了 {COST_WAIT} 秒）");
        return exit(1);
    }
    if e.is_os_or_value() || e.is("TypeError") || e.is("KeyError") {
        out!("统计折合金额没成：{e}");
        return exit(1);
    }
    Err(e)
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

// --- 折合金额的视图（_gui state 的 cost、cost_total，PROTOCOL 2.2）：只读 cost/ 里统计好的，按时段切、加起来 ---------------

/// 7 个计数：金额（百万分之一美元）、请求数、输入、输出、缓存读、缓存写、其中没有标价的请求数。原来是 Python 的整数（不会溢出）：
/// 一项最多 10^15，加起来用 i128
type Counts = [i128; 7];
/// {小时: 计数}
type Hours = std::collections::BTreeMap<i64, Counts>;

/// 一个时段：开始、结束（原来是什么数就是什么数：额度窗口给的整数、本地零点算出来的小数，输出时写法不一样）
#[derive(Clone)]
pub struct Period {
    pub start: V,
    pub end: V,
    /// 按周的：calendar / quota；按天的：日期（2026-11-13）
    pub tag: String,
}

impl Period {
    fn s(&self) -> f64 {
        self.start.as_pynum().unwrap_or(0.0)
    }

    fn e(&self) -> f64 {
        self.end.as_pynum().unwrap_or(0.0)
    }
}

/// t 那一天（本地时间）往后数 add 天的零点
pub fn cost_local_day(t_: f64, add: i32) -> f64 {
    let lt = localtime(t_);
    mktime(lt.year, lt.mon, lt.mday + add, 0, 0, 0, -1)
}

/// 最近 COST_DAYS 个本地日历日：[(开始, 结束, 日期)]，最后一项是今天（夏令时的那天 23 或 25 小时）
pub fn cost_days(now: f64) -> Vec<Period> {
    let mut out = Vec::new();
    for i in (0..COST_DAYS as i32).rev() {
        let s = cost_local_day(now, -i);
        out.push(Period { start: V::from(s), end: V::from(cost_local_day(now, -i + 1)), tag: strftime_local("%Y-%m-%d", s) });
    }
    out
}

/// 按周的时段：[(开始, 结束, basis)]，首尾相接，最后一项包含 now。有本周额度窗口（usage_weeks 的 weeks）的地方用它，
/// 没有的时段按本地时间周一零点切；不到一天的零头并进前一项（没有前一项就并进后一项）。只留结束在最近 WEEKS_SHOW 周里的，最多 13 项
pub fn cost_weeks(quota: Option<&V>, now: f64) -> Vec<Period> {
    let lo = now - (WEEKS_SHOW * WEEK) as f64;
    let mut t_ = V::from(cost_local_day(lo, -localtime(lo).wday));
    let num_ = |v: &V| v.as_pynum().unwrap_or_else(|| panic!("TypeError: 时段的边界是 {}，不是数", v.py_repr()));
    let mut out: Vec<Period> = Vec::new();
    // 从 t 起按日历周补到 until：每项到下一个周一零点，cap 时不超过 until
    let fill = |out: &mut Vec<Period>, t_: &mut V, until: &V, cap: bool| {
        while num_(t_) < num_(until) {
            let from = num_(t_);
            let e = V::from(cost_local_day(from, 7 - localtime(from).wday)); // 下一个周一零点
            let end = if cap && num_(until) < num_(&e) { until.clone() } else { e };
            out.push(Period { start: t_.clone(), end: end.clone(), tag: "calendar".into() });
            *t_ = end;
        }
    };
    // for w in quota or []
    let quota = match quota.filter(|q| q.truthy()) {
        None => Vec::new(),
        Some(V::List(q)) => q.items(),
        Some(other) => panic!("TypeError: 额度窗口是 {}，不是列表", other.py_repr()),
    };
    for w in quota {
        let w = w.as_dict().unwrap_or_else(|| panic!("TypeError: 额度窗口里的一项不是对象"));
        let (ws, e) = (w.at("start_epoch"), w.at("end_epoch"));
        // max(w['start_epoch'], t)：一样大时是前一个
        let s = if num_(&t_) > num_(&ws) { t_.clone() } else { ws };
        if num_(&e) - num_(&s) <= 0.0 {
            continue;
        }
        fill(&mut out, &mut t_, &s, true);
        out.push(Period { start: s, end: e.clone(), tag: "quota".into() });
        t_ = e;
    }
    if num_(&t_) <= now {
        fill(&mut out, &mut t_, &V::from(now + 1.0), false); // 最后一项是这一周（到下周一零点）
    }
    let mut i = 0;
    while i < out.len() {
        if out[i].e() - out[i].s() < 86400.0 && out.len() > 1 {
            if i > 0 {
                out[i - 1].end = out[i].end.clone();
            } else {
                out[1].start = out[0].start.clone();
            }
            out.remove(i);
            continue;
        }
        i += 1;
    }
    let kept: Vec<Period> = out.into_iter().filter(|p| p.e() > lo).collect();
    let skip = kept.len().saturating_sub(WEEKS_SHOW as usize + 1);
    kept.into_iter().skip(skip).collect()
}

/// 小时数据按时段加起来：[[7 个计数]…]（和 periods 一一对应）；跨过边界的那个小时整个算在它的中点所在的时段
fn cost_bin(periods: &[Period], h: &Hours) -> Vec<Counts> {
    let mut out = vec![[0i128; 7]; periods.len()];
    for (k, r) in h {
        let mid = (*k + 1800) as f64;
        // bisect.bisect_right(starts, mid) - 1：最后一个开始不晚于 mid 的时段
        let after = periods.partition_point(|p| p.s() <= mid);
        if after > 0 && mid < periods[after - 1].e() {
            for x in 0..7 {
                out[after - 1][x] += r[x];
            }
        }
    }
    out
}

/// 百万分之一美元 → 美元（PROTOCOL 里的小数）
fn usd(m: i128) -> f64 {
    num::round_to(m as f64 / 1e6, 4)
}

fn usd_opt(m: Option<i128>) -> V {
    m.map_or(V::None, |m| V::from(usd(m)))
}

/// 一方（整个账号或者一台设备）的几项（百万分之一美元）
#[derive(Clone)]
struct Money {
    weeks: i128,
    current: i128,
    days: i128,
    today: i128,
    average: Option<i128>,
    average_day: Option<i128>,
    day_list: Vec<i128>,
    /// 按模型分的两段（view 算好以后放进来）
    models: Models,
}

/// 一方（整个账号或者一台设备）加起来：(百万分之一美元的几项, PROTOCOL 2.2 的 summary)。wr、dr 是按周、按天的各项；good、
/// good_days 是算平均的那几周（过完的、不是 partial、至少 6 天）、那几天（过完的、不是 partial）。一台设备的平均也按整个
/// 账号的这几项算：各台设备的平均加起来就是整个账号的
fn cost_summary(wr: &[Counts], dr: &[Counts], good: &[usize], good_days: &[usize]) -> (Money, Dict) {
    let avg = |rows: &[Counts], sel: &[usize]| -> Option<i128> {
        if sel.is_empty() {
            return None;
        }
        Some(sel.iter().map(|n| rows[*n][0]).sum::<i128>().div_euclid(sel.len() as i128))
    };
    let m = Money {
        weeks: wr.iter().map(|r| r[0]).sum(),
        current: wr.last().expect("至少有这一周")[0],
        days: dr.iter().map(|r| r[0]).sum(),
        today: dr.last().expect("至少有今天")[0],
        average: avg(wr, good),
        average_day: avg(dr, good_days),
        day_list: dr.iter().map(|r| r[0]).collect(),
        models: Models::new(),
    };
    let summary = dict! {
        "weeks_usd" => usd(m.weeks), "current_usd" => usd(m.current), "average_usd" => usd_opt(m.average), "average_weeks" => good.len(),
        "days_usd" => usd(m.days), "today_usd" => usd(m.today), "average_day_usd" => usd_opt(m.average_day), "average_days" => good_days.len(),
    };
    (m, summary)
}

/// 计数写进 JSON：放得进 i64 的是整数（实际都是）
fn count_v(n: i128) -> V {
    match i64::try_from(n) {
        Ok(n) => V::Int(n),
        Err(_) => V::Big(n.to_string().into()),
    }
}

/// PROTOCOL 2.2 的一项（weeks[]、days[]）：p 是时段，t 是这一项的 7 个计数，last 是还没过完的那一项
fn cost_period_view(p: &Period, t_: &Counts, last: bool, since: f64) -> Dict {
    dict! {
        "start_epoch" => &p.start, "end_epoch" => &p.end, "current" => last, "partial" => since > p.s(),
        "usd" => usd(t_[0]), "requests" => count_v(t_[1]), "unpriced_requests" => count_v(t_[6]),
        "tokens" => dict! { "input" => count_v(t_[2]), "output" => count_v(t_[3]), "cache_read" => count_v(t_[4]), "cache_write" => count_v(t_[5]) },
    }
}

/// 核对过的 [7 个计数] → 计数
fn counts_of(r: &V) -> Counts {
    let r = r.as_list().expect("核对过");
    let mut c = [0i128; 7];
    for (x, slot) in c.iter_mut().enumerate() {
        *slot = r.at(x as isize).as_int().unwrap_or(0) as i128;
    }
    c
}

/// 核对过的 {小时: [7 个计数]} → 按小时查的表
fn hours_of(rows: &Dict) -> Hours {
    let mut h = Hours::new();
    for (k, r) in rows.items() {
        h.insert(text::to_int(&k).unwrap_or(0), counts_of(&r));
    }
    h
}

/// {小时: [(模型, 7 个计数)…]}：按模型分的小时数据
type ModelHours = std::collections::BTreeMap<i64, Vec<(String, Counts)>>;

/// 核对过的 {小时: {模型: [7 个计数]}} → 按小时查的表
fn mhours_of(rows: &Dict) -> ModelHours {
    let mut h = ModelHours::new();
    for (k, ms) in rows.items() {
        let ms = ms.as_dict().expect("cost_mrows_ok 核对过");
        h.insert(text::to_int(&k).unwrap_or(0), ms.items().into_iter().map(|(m, r)| (m.to_string(), counts_of(&r))).collect());
    }
    h
}

/// 按模型加起来的两段（列出的周期、按天的那 30 天）：模型 → 两段各 7 个计数。None 是没有按模型记下的那一部分（合计减去各个模型的：
/// 旧版 cca 的设备、以前的版本算的、传的时候为了放得下去掉了的）
type Models = std::collections::BTreeMap<Option<String>, [Counts; 2]>;

/// 按模型分的小时数据在这几个首尾相接的时段里加起来：跨过边界的小时和 cost_bin 一样整个算在中点所在的那一项
fn cost_bin_models(periods: &[Period], hm: &ModelHours) -> std::collections::BTreeMap<String, Counts> {
    let mut out: std::collections::BTreeMap<String, Counts> = std::collections::BTreeMap::new();
    for (k, ms) in hm {
        let mid = (*k + 1800) as f64;
        let after = periods.partition_point(|p| p.s() <= mid);
        if after > 0 && mid < periods[after - 1].e() {
            for (m, r) in ms {
                let b = out.entry(m.clone()).or_insert([0; 7]);
                for x in 0..7 {
                    b[x] += r[x];
                }
            }
        }
    }
    out
}

/// 一台设备按模型分的两段：各个模型的，和这一段的合计（wk、dy：它按周、按天的各项）减去各个模型的那一部分（None；不会是负的）
fn device_models(weeks: &[Period], days: &[Period], hm: &ModelHours, wk: &[Counts], dy: &[Counts]) -> Models {
    let mut out = Models::new();
    for (n, (periods, rows)) in [(weeks, wk), (days, dy)].into_iter().enumerate() {
        let mut rest = [0i128; 7];
        for r in rows {
            for x in 0..7 {
                rest[x] += r[x];
            }
        }
        for (m, c) in cost_bin_models(periods, hm) {
            for x in 0..7 {
                rest[x] -= c[x];
            }
            out.entry(Some(m)).or_insert([[0; 7]; 2])[n] = c;
        }
        if rest.iter().any(|v| *v > 0) {
            out.entry(None).or_insert([[0; 7]; 2])[n] = rest.map(|v| v.max(0));
        }
    }
    out
}

/// 把 m 加进 into
fn add_models(into: &mut Models, m: &Models) {
    for (k, v) in m {
        let b = into.entry(k.clone()).or_insert([[0; 7]; 2]);
        for n in 0..2 {
            for x in 0..7 {
                b[n][x] += v[n][x];
            }
        }
    }
}

/// PROTOCOL 2.2 的 models[]：每个模型（没有按模型记下的那一部分 model 是 null）在列出的周期（weeks）、按天的那 30 天（days）里
/// 折成多少美元、几次请求、其中没有标价的几次；tokens 只给整个账号的；价格表里还没有、按同系列的标价估算的带 priced_as。
/// 按列出的周期的金额、请求数从多到少，再按 30 天的金额、模型名排，没有按模型记下的在最后；两段里都没有请求、没有金额的不列
fn models_view(ms: &Models, tokens: bool) -> Vec<V> {
    let mut rows: Vec<(&Option<String>, &[Counts; 2])> =
        ms.iter().filter(|(_, c)| c.iter().any(|c| c[1] != 0 || c[0] != 0)).collect();
    rows.sort_by(|a, b| {
        a.0.is_none()
            .cmp(&b.0.is_none())
            .then(b.1[0][0].cmp(&a.1[0][0]))
            .then(b.1[0][1].cmp(&a.1[0][1]))
            .then(b.1[1][0].cmp(&a.1[1][0]))
            .then(a.0.cmp(b.0))
    });
    let range = |c: &Counts| -> Dict {
        let d = dict! { "usd" => usd(c[0]), "requests" => count_v(c[1]), "unpriced_requests" => count_v(c[6]) };
        if tokens {
            d.set(
                "tokens",
                dict! { "input" => count_v(c[2]), "output" => count_v(c[3]), "cache_read" => count_v(c[4]), "cache_write" => count_v(c[5]) },
            );
        }
        d
    };
    rows.into_iter()
        .map(|(m, c)| {
            let model = m.as_deref().map_or(V::None, V::from);
            let row = dict! { "model" => model, "weeks" => range(&c[0]), "days" => range(&c[1]) };
            if let Some(base) = m.as_deref().and_then(priced_as) {
                row.set("priced_as", base);
            }
            V::Dict(row)
        })
        .collect()
}

/// 有数据的一台设备
struct Device {
    id: V,
    name: String,
    me: bool,
    synced_epoch: V,
    stale: bool,
    h: Hours,
    /// 按模型分的（旧版 cca 的设备没传：空的，都算作没有按模型记下的）
    hm: ModelHours,
    /// 它的诊断（cost_diag_ok 的样子；这台设备的是 local.json 的 diag 加上 cca 的版本）
    diag: Option<Dict>,
}

/// 一个账号里一台用过它的设备的那一份（百万分之一美元）：cost_total 的 me、devices 按设备加起来
struct DevMoney {
    id: String,
    name: String,
    me: bool,
    stale: bool,
    money: Money,
}

/// 一台用得了的同步服务器那边的折合金额：同步的状态（它的 sync.json）、别的设备经它同步来的（它的 remote.json）
struct HubCost {
    kind: HubKind,
    sync: Dict,
    remote: Dict,
}

/// _gui state 里折合金额要的（一次读好）：这台设备统计的、别的设备经各台服务器同步来的、同步的状态，按天的时段
pub struct CostView {
    pub on: bool,
    now: f64,
    presence_off: bool,
    local: Dict,
    hubs: Vec<HubCost>,
    days: Vec<Period>,
    updated: Option<f64>,
    me_name: String,
    /// 这台机器的 id：config.json 的 device_id（别的设备传来的 dv 和它一样就是这台机器以前传的）；没有是 local
    me_id: String,
    /// [(工具名, 整个账号的各项, 用过它的各台设备的那一份)]（百万分之一美元）：cost_total 加起来
    accounts: Vec<(&'static str, Option<Money>, Vec<DevMoney>)>,
}

impl CostView {
    /// hubs 是这台设备用得了的同步服务器（hub.rs 的 hubs()）
    pub fn new(c: &Dict, now: f64, hubs: &[HubKind]) -> CostView {
        let is_false = |k: &str| matches!(c.get(k), Some(V::Bool(false)));
        let hubs: Vec<HubCost> = hubs
            .iter()
            .map(|k| HubCost { kind: *k, sync: cost_json(&k.file("sync.json")), remote: cost_json(&k.file("remote.json")) })
            .collect();
        // 这台设备的名字：各个 Team 的服务器上叫什么（照加入的先后），都没有是主机名
        let me_name = hubs.iter().find_map(|h| h.sync.str("me")).map_or_else(host_name, |m| m.to_string());
        let me_id =
            c.str("device_id").filter(|d| DV_RE.fullmatch(d).is_some()).map_or_else(|| "local".to_string(), |d| d.to_string());
        CostView {
            on: !is_false("cost"),
            now,
            presence_off: is_false("presence"),
            local: cost_json("local.json"),
            hubs,
            days: cost_days(now),
            updated: cost_mtime("scanned"),
            me_name,
            me_id,
            accounts: Vec::new(),
        }
    }

    fn hub(&self, kind: Option<HubKind>) -> Option<&HubCost> {
        kind.and_then(|k| self.hubs.iter().find(|h| h.kind == k))
    }

    /// 给 App 的诊断（PROTOCOL 2.2 的 devices[].diag）：{"cca", "claude_code", "claude_code_old": claude_code 还是不写账号信息的
    /// 旧版本（App 据此提醒更新，不自己比版本）, "why": 按天看的这一段里（往前多一天：传来的只到天）有回复认不出账号的原因}；
    /// 没有诊断（旧版 cca 的设备）是 None
    fn diag_view(&self, d: &Option<Dict>) -> Option<Dict> {
        let d = d.as_ref()?;
        let lo = self.days[0].s() - 86400.0;
        let mut why = Vec::new();
        for w in d.list("why").map(|l| l.items()).unwrap_or_default() {
            let w = w.as_dict().expect("cost_diag_ok 给的");
            if w.at("last_epoch").as_num().is_some_and(|a| a >= lo) {
                why.push(V::Dict(dict! {
                    "code" => w.at("code"), "last_epoch" => w.at("last_epoch"), "claude_code" => w.get("claude_code").unwrap_or(V::None),
                }));
            }
        }
        let opt = |k: &str| d.get(k).unwrap_or(V::None);
        let old = d.str("claude_code").is_some_and(|v| claude_code_old(&v));
        Some(dict! { "cca" => opt("cca"), "claude_code" => opt("claude_code"), "claude_code_old" => old, "why" => why })
    }

    /// 这台设备最近有回复认不出账号（要提醒）
    fn warns(&self, d: &Option<Dict>) -> bool {
        self.diag_view(d).is_some_and(|v| v.list("why").is_some_and(|l| !l.is_empty()))
    }

    fn scan_state(&self) -> &'static str {
        if self.updated.is_some_and(|u| u != 0.0) {
            return "ready";
        }
        let j = cost_json("worker.json");
        // isinstance(pid, int) and type(started) is int and cli_alive(…)
        if let (Some(pid), Some(V::Int(started))) = (j.get("pid"), j.get("started")) {
            match pid {
                V::Int(pid) if cli_alive(pid, started, &proc_starts(&[pid])) => return "scanning",
                V::Bool(true) if !cfg!(target_os = "linux") => {
                    // 布尔值在 Python 里也算 int：True 当成进程号去问（ps 的参数写成 True，问不到）
                    super::session::ps_lstart("True");
                }
                _ => {}
            }
        }
        "never"
    }

    /// 账号的 sync（PROTOCOL 2.2）；kind 是这个账号经哪台服务器同步（hub.rs 的 hub_of：这台设备用得了的），不经服务器是 None
    fn sync_view(&self, slot: &str, kind: Option<HubKind>) -> Dict {
        let hub = self.hub(kind);
        let empty = Dict::new();
        let sync = hub.map_or(&empty, |h| &h.sync);
        // (sync.get('acc') or {}).get(slot)
        let a = match sync.get("acc").filter(|a| a.truthy()) {
            None => None,
            Some(V::Dict(acc)) => acc.dict(slot),
            Some(other) => panic!("AttributeError: sync.json 的 acc 是 {}", other.py_repr()),
        };
        let a = a.unwrap_or_default();
        let checked = a.get("checked").filter(is_num);
        let gst = sync.get("state");
        let gst = gst.as_ref().and_then(|g| g.as_str());
        let mut msg = V::None;
        let state = if self.presence_off {
            "off"
        } else if hub.is_none() {
            "local_only"
        } else if gst == Some("error")
            && (!checked.as_ref().is_some_and(|c| c.truthy())
                || sync.get("at").filter(is_num).is_some_and(|at| at.as_num() > checked.as_ref().and_then(|c| c.as_num())))
        {
            msg = sync.get("error").filter(|e| e.is_str()).unwrap_or(V::None);
            "error"
        } else if checked.as_ref().is_some_and(|c| c.truthy()) {
            "ok"
        } else {
            "pending"
        };
        let mut missing = Vec::new();
        if state != "off" && state != "local_only" {
            for m in a.list("missing").map(|m| m.items()).unwrap_or_default() {
                let Some(m) = m.as_dict() else { continue };
                let (Some(name), Some(used)) = (m.get("name").filter(|n| n.is_str()), m.get("used_epoch").filter(is_num)) else {
                    continue;
                };
                if used.as_num().is_some_and(|u| u > self.now - (COST_DAYS * 86400) as f64) {
                    missing.push(V::Dict(dict! { "name" => name, "used_epoch" => used }));
                }
            }
        }
        // off、local_only 以外 hub 一定有（上面的判断）
        let hub = match hub {
            Some(h) if !matches!(state, "off" | "local_only") => V::from(h.kind.name()),
            _ => V::None,
        };
        dict! { "state" => state, "message" => msg, "checked_epoch" => checked, "missing" => missing, "hub" => hub }
    }

    /// 这台设备的 id（cost_total.devices[]、账号的 devices[] 里这台的那一项）
    fn me_id(&self) -> V {
        V::from(self.me_id.as_str())
    }

    /// 有数据的设备，这台设备在最前；别的设备的 id 是它传的 dv（机器的标识），以前的版本没有 dv 时是它在服务器上的指纹。
    /// 同一台机器（id 一样）的几份并成一份（它有数据以前的小时才用另一份，不重复计）：这台机器自己以前传的（dv 就是这台的
    /// device_id）并进这台。从 Team 里移除了的设备（以前同步来的，标着 revoked）不算：服务器已经删了它传的。
    /// 第二个返回值：没有标价的模型（排好序）
    fn devices(&self, tool: &Tool, email: &str, slot: &str, hub: Option<&HubCost>) -> (Vec<Device>, Vec<String>) {
        let (h, hm, ums) = cost_content(&self.local, tool, email);
        let entries = hub.map(|hub| self.remote_entries(hub, slot)).unwrap_or_default();
        self.devices_of(&h, &hm, ums, entries)
    }

    /// 别的设备经这台服务器传来的这一份（remote.json 的 acc 里 slot 那一项）：[(设备指纹, 那一项)]
    fn remote_entries(&self, hub: &HubCost, slot: &str) -> Vec<(std::rc::Rc<str>, V)> {
        // ((remote.get('acc') or {}).get(slot) or {}).items()
        let acc = match hub.remote.get("acc").filter(|a| a.truthy()) {
            None => None,
            Some(V::Dict(acc)) => acc.get(slot).filter(|s| s.truthy()),
            Some(other) => panic!("AttributeError: remote.json 的 acc 是 {}", other.py_repr()),
        };
        match acc {
            None => Vec::new(),
            Some(V::Dict(s)) => s.items(),
            Some(other) => panic!("AttributeError: remote.json 里 {slot} 那一项是 {}", other.py_repr()),
        }
    }

    /// devices 的后一半：h、hm、ums 是这台设备的，entries 是别的设备传来的（remote_entries；认不出是谁的那一份来自几台服务器）
    fn devices_of(
        &self,
        h: &Dict,
        hm: &Dict,
        mut ums: Vec<String>,
        entries: Vec<(std::rc::Rc<str>, V)>,
    ) -> (Vec<Device>, Vec<String>) {
        let me_id = self.me_id();
        let me_diag = cost_diag_ok(self.local.get("diag"), self.now).unwrap_or_else(|| dict! { "why" => List::new() });
        if let Some(v) = getenv("CCA_VERSION").filter(|v| version_ok(v)) {
            me_diag.set("cca", v);
        }
        let mut devs = vec![Device {
            id: me_id,
            name: self.me_name.clone(),
            me: true,
            synced_epoch: V::from(self.updated),
            stale: false,
            h: hours_of(h),
            hm: mhours_of(hm),
            diag: Some(me_diag),
        }];
        {
            for (fp, e) in entries {
                let Some(e) = e.as_dict().filter(|e| !matches!(e.get("revoked"), Some(V::Bool(true)))) else { continue };
                let (Some(rows), Some(at), Some(name)) =
                    (e.get("h").filter(|h| cost_rows_ok(Some(h))), e.get("at").filter(is_num), e.str("name"))
                else {
                    continue;
                };
                devs.push(Device {
                    id: V::from(e.str("dv").filter(|d| DV_RE.fullmatch(d).is_some()).unwrap_or(fp)),
                    name: name.to_string(),
                    me: false,
                    stale: self.now - at.as_num().unwrap_or(0.0) > COST_STALE,
                    synced_epoch: at,
                    h: hours_of(&rows.as_dict().expect("cost_rows_ok 核对过")),
                    hm: e
                        .get("hm")
                        .filter(|x| cost_mrows_ok(Some(x), COST_MODELS_HOUR))
                        .and_then(|x| x.as_dict())
                        .map_or_else(ModelHours::new, |x| mhours_of(&x)),
                    diag: cost_diag_ok(e.get("dg"), self.now),
                });
                // e.get('um') or []
                let models = match e.get("um").filter(|u| u.truthy()) {
                    None => Vec::new(),
                    Some(V::List(u)) => u.items(),
                    Some(V::Dict(u)) => u.keys().into_iter().map(V::from).collect(),
                    Some(V::Str(u)) => u.chars().map(|ch| V::str(ch.to_string())).collect(),
                    Some(other) => panic!("TypeError: remote.json 里的 um 是 {}", other.py_repr()),
                };
                for m in models {
                    if let Some(m) = m.as_str().filter(|m| !ums.iter().any(|x| x == m)) {
                        ums.push(m.to_string());
                    }
                }
            }
        }
        // 并进哪一台（into）：同一台机器（id 一样）的只留一份：这台设备，不然是最近同步过的。都直接并进留下的那一份，不会一层套一层
        let n = devs.len();
        let rank = |d: &Device| (d.me, d.synced_epoch.as_num().unwrap_or(0.0));
        let mut into: Vec<Option<usize>> = vec![None; n];
        for i in 1..n {
            let best = (0..n)
                .filter(|&j| devs[j].id.py_eq(&devs[i].id))
                .max_by(|&a, &b| rank(&devs[a]).partial_cmp(&rank(&devs[b])).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
            into[i] = best.filter(|&b| b != i);
        }
        // 只用它有数据以前的小时（不重复计）；几份并进同一台时新的先并，依次往前接
        let mut order: Vec<usize> = (1..n).filter(|&i| into[i].is_some()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(devs[i].h.keys().next_back().copied()));
        for i in order {
            let a = into[i].expect("上面筛过");
            let first = devs[a].h.keys().next().copied();
            let extra: Vec<(i64, Counts)> =
                devs[i].h.iter().filter(|(k, _)| first.is_none_or(|f| **k < f)).map(|(k, r)| (*k, *r)).collect();
            for (k, r) in extra {
                let b = devs[a].h.entry(k).or_insert([0; 7]);
                for x in 0..7 {
                    b[x] += r[x];
                }
            }
            let extra_m: Vec<(i64, Vec<(String, Counts)>)> =
                devs[i].hm.iter().filter(|(k, _)| first.is_none_or(|f| **k < f)).map(|(k, r)| (*k, r.clone())).collect();
            for (k, ms) in extra_m {
                let b = devs[a].hm.entry(k).or_default();
                for (m, r) in ms {
                    match b.iter_mut().find(|(x, _)| *x == m) {
                        Some((_, c)) => {
                            for x in 0..7 {
                                c[x] += r[x];
                            }
                        }
                        None => b.push((m, r)),
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (i, d) in devs.into_iter().enumerate() {
            if into[i].is_none() {
                out.push(d);
            }
        }
        ums.sort();
        (out, ums)
    }

    /// 账号的 cost（PROTOCOL 2.2）；没有任何记录是 None。kind：这个账号经哪台服务器同步（不经服务器是 None）。
    /// 各项的百万分之一美元另记进 self.accounts（cost_total 用）
    pub fn account(&mut self, tool: &'static Tool, k: &str, weeks_view: Option<&Dict>, kind: Option<HubKind>) -> V {
        let email = parse_key(k).1.to_lowercase();
        if email.is_empty() {
            return V::None;
        }
        let slot = format!("{}:{}", tool.name, email);
        let sync = self.sync_view(&slot, kind);
        let share = !matches!(sync.at("state").as_str(), Some("off") | Some("local_only"));
        let (devs, ums) = self.devices(tool, &email, &slot, if share { self.hub(kind) } else { None });
        let quota = weeks_view.filter(|w| !w.is_empty()).map(|w| w.at("weeks"));
        let weeks = cost_weeks(quota.as_ref(), self.now);
        match self.view(&devs, &ums, &weeks, sync, true) {
            Some((out, m, shares)) => {
                self.accounts.push((tool.name, Some(m), shares));
                V::Dict(out)
            }
            None => {
                self.accounts.push((tool.name, None, Vec::new()));
                V::None
            }
        }
    }

    /// 一个账号（或者认不出是哪个账号的那些）的 cost，PROTOCOL 2.2 的样子：devs 是有数据的设备（devices 给的），ums 是没有标价的模型，
    /// weeks 是按周的时段。还没有任何记录是 None；不然还给整个的各项、用过它的各台设备的那一份（cost_total 按设备加起来）。
    /// diag_rows：没有数据、最近却有回复认不出账号的别的设备也列出来（账号的：它用这个账号的可能漏了；认不出是谁的那一份不用）
    fn view(
        &self,
        devs: &[Device],
        ums: &[String],
        weeks: &[Period],
        sync: Dict,
        diag_rows: bool,
    ) -> Option<(Dict, Money, Vec<DevMoney>)> {
        let wk: Vec<Vec<Counts>> = devs.iter().map(|d| cost_bin(weeks, &d.h)).collect();
        let dy: Vec<Vec<Counts>> = devs.iter().map(|d| cost_bin(&self.days, &d.h)).collect();
        let requests = |rows: &[Counts]| rows.iter().map(|r| r[1]).sum::<i128>();
        let data: Vec<usize> = (0..devs.len()).filter(|i| requests(&wk[*i]) != 0 || requests(&dy[*i]) != 0).collect();
        if data.is_empty() {
            return None;
        }
        // 这个账号没有数据、最近却有回复认不出账号的别的设备也列出来（金额是 0，带着诊断：它用这个账号的可能漏了）
        let keep: Vec<usize> =
            (0..devs.len()).filter(|i| data.contains(i) || (diag_rows && !devs[*i].me && self.warns(&devs[*i].diag))).collect();
        // data 里的设备都有数据
        let since = data.iter().filter_map(|i| devs[*i].h.keys().next()).min().copied().expect("data 里的设备都有数据") as f64;
        let good: Vec<usize> = weeks
            .iter()
            .enumerate()
            .filter(|(_, p)| p.e() <= self.now && since <= p.s() && p.e() - p.s() >= 6.0 * 86400.0)
            .map(|(n, _)| n)
            .collect();
        let good_days: Vec<usize> =
            self.days.iter().enumerate().filter(|(_, p)| p.e() <= self.now && since <= p.s()).map(|(n, _)| n).collect();
        let total = |rows: &[Vec<Counts>], count: usize| -> Vec<Counts> {
            (0..count)
                .map(|n| {
                    let mut c = [0i128; 7];
                    for i in &keep {
                        for x in 0..7 {
                            c[x] += rows[*i][n][x];
                        }
                    }
                    c
                })
                .collect()
        };
        let (wsum, dsum) = (total(&wk, weeks.len()), total(&dy, self.days.len()));
        let (mut m, summary) = cost_summary(&wsum, &dsum, &good, &good_days);
        summary.set("requests", count_v(wsum.iter().map(|r| r[1]).sum()));
        summary.set("unpriced_requests", count_v(wsum.iter().map(|r| r[6]).sum()));
        let mut parts: Vec<(usize, Money, Dict)> = keep
            .iter()
            .map(|i| {
                let (pm, ps) = cost_summary(&wk[*i], &dy[*i], &good, &good_days);
                (*i, pm, ps)
            })
            .collect();
        // 按模型分的：各台设备的，加起来是整个的
        for (i, pm, _) in parts.iter_mut() {
            pm.models = device_models(weeks, &self.days, &devs[*i].hm, &wk[*i], &dy[*i]);
            add_models(&mut m.models, &pm.models);
        }
        // 用过它的各台设备的那一份（cost_total 的 me、devices 按设备加起来；只为诊断列出来的不算）。
        // 这台设备没有记录时没有它的
        let shares: Vec<DevMoney> = parts
            .iter()
            .filter(|(i, _, _)| data.contains(i))
            .map(|(i, pm, _)| {
                let d = &devs[*i];
                DevMoney { id: d.id.py_str(), name: d.name.clone(), me: d.me, stale: d.stale, money: pm.clone() }
            })
            .collect();
        let mut devices: Vec<Dict> = parts
            .iter()
            .map(|(i, pm, ps)| {
                let d = &devs[*i];
                dict! {
                    // revoked：3.6.6 起总是 false（移除了的设备不列；接口只加不删）
                    "id" => &d.id, "name" => d.name.as_str(), "me" => d.me, "revoked" => false, "synced_epoch" => &d.synced_epoch, "stale" => d.stale,
                    "used" => data.contains(i),
                    "weeks_usd" => wk[*i].iter().map(|r| V::from(usd(r[0]))).collect::<Vec<V>>(),
                    "days_usd" => dy[*i].iter().map(|r| V::from(usd(r[0]))).collect::<Vec<V>>(),
                    "summary" => ps, "diag" => self.diag_view(&d.diag), "models" => models_view(&pm.models, false),
                }
            })
            .collect();
        // 这台设备在前，别的按最近 30 天的多少
        devices.sort_by(|a, b| {
            let key = |d: &Dict| (!d.truthy("me"), -d.dict("summary").expect("刚建的").at("days_usd").as_num().unwrap_or(0.0));
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0)
                .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.at("name").py_str().cmp(&b.at("name").py_str()))
        });
        let weeks_out: Vec<V> = weeks
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let d = cost_period_view(p, &wsum[n], n == weeks.len() - 1, since);
                d.set("basis", p.tag.as_str());
                V::Dict(d)
            })
            .collect();
        let days_out: Vec<V> = self
            .days
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let d = cost_period_view(p, &dsum[n], n == self.days.len() - 1, since);
                d.set("date", p.tag.as_str());
                V::Dict(d)
            })
            .collect();
        let out = dict! {
            "currency" => "USD", "prices_date" => COST_PRICES_DATE, "since_epoch" => since, "weeks" => weeks_out, "days" => days_out,
            "summary" => summary, "devices" => devices.into_iter().map(V::Dict).collect::<Vec<V>>(),
            "unpriced_models" => ums.iter().map(|m| V::from(m.as_str())).collect::<Vec<V>>(), "sync" => sync,
            "models" => models_view(&m.models, true),
        };
        Some((out, m, shares))
    }

    /// 认不出是哪个账号的那些（这台设备的，和别的设备经各个 Team 的服务器传来的 COST_ORPHAN）：(PROTOCOL 2.2 的 unattributed.cost，
    /// 哪台设备都没有记录时是 None；各台设备的那一份；传过这一份的别的设备的 id)。同一台机器经几台服务器传的按机器的标识认成一台
    /// （按周的时段是本地时间的日历周：没有额度窗口）
    fn orphan(&self) -> (Option<Dict>, Vec<DevMoney>, Vec<String>) {
        let (h, hm, ums) = orphan_content(&self.local);
        let mut entries = Vec::new();
        if !self.presence_off {
            for hub in &self.hubs {
                entries.extend(self.remote_entries(hub, COST_ORPHAN_SLOT));
            }
        }
        let (devs, ums) = self.devices_of(&h, &hm, ums, entries);
        let synced = devs.iter().filter(|d| !d.me).map(|d| d.id.py_str()).collect();
        match self.view(&devs, &ums, &cost_weeks(None, self.now), self.orphan_sync(), false) {
            Some((out, _, shares)) => (Some(out), shares, synced),
            None => (None, Vec::new(), synced),
        }
    }

    /// 认不出是哪个账号的那一份的 sync：经最近同步过它的那台服务器的（都还没同步过时是第一台；一台都没有时只有这台设备）
    fn orphan_sync(&self) -> Dict {
        let checked = |h: &HubCost| -> f64 {
            let a = h.sync.dict("acc").and_then(|a| a.dict(COST_ORPHAN_SLOT));
            a.and_then(|a| a.get("checked")).filter(is_num).and_then(|c| c.as_num()).unwrap_or(0.0)
        };
        let mut best: Option<&HubCost> = None;
        for h in &self.hubs {
            if best.is_none_or(|b| checked(h) > checked(b)) {
                best = Some(h);
            }
        }
        self.sync_view(COST_ORPHAN_SLOT, best.map(|h| h.kind))
    }

    /// 顶层的 cost_total（PROTOCOL 2.2）；tools 是启用了的工具名。折合金额关掉了、没有 Claude / Codex 的官方账号时是 None。
    /// 顶层是各账号（所有设备）加起来的，me 是各账号里这台设备的那一份加起来的，devices 是按设备分开加起来的（App 的设备菜单）
    pub fn total(&self, tools: &[&str]) -> V {
        if !self.on || self.accounts.is_empty() {
            return V::None;
        }
        let agg = |sel: &[&Money]| -> Dict {
            let avg: Vec<i128> = sel.iter().filter_map(|m| m.average).collect();
            let avd: Vec<i128> = sel.iter().filter_map(|m| m.average_day).collect();
            dict! {
                "current_usd" => usd(sel.iter().map(|m| m.current).sum()), "weeks_usd" => usd(sel.iter().map(|m| m.weeks).sum()),
                "average_usd" => if avg.is_empty() { V::None } else { V::from(usd(avg.iter().sum())) },
                "average_day_usd" => if avd.is_empty() { V::None } else { V::from(usd(avd.iter().sum())) },
                "days_usd" => usd(sel.iter().map(|m| m.days).sum()), "today_usd" => usd(sel.iter().map(|m| m.today).sum()),
            }
        };
        // ms 是 [(工具名, 各项)]：几项统计、30 天每天、按工具分开
        let part = |ms: &[(&'static str, &Money)]| -> Dict {
            let all: Vec<&Money> = ms.iter().map(|(_, m)| *m).collect();
            let out = agg(&all);
            let days: Vec<V> = self
                .days
                .iter()
                .enumerate()
                .map(|(n, p)| {
                    V::Dict(dict! {
                        "start_epoch" => &p.start, "end_epoch" => &p.end, "date" => p.tag.as_str(), "current" => n == self.days.len() - 1,
                        "usd" => usd(ms.iter().map(|(_, m)| m.day_list[n]).sum()),
                    })
                })
                .collect();
            out.set("days", days);
            let by_tool = Dict::new();
            for tool in tools.iter().filter(|x| **x == "claude" || **x == "codex") {
                let sel: Vec<&Money> = ms.iter().filter(|(x, _)| x == tool).map(|(_, m)| *m).collect();
                by_tool.set(*tool, agg(&sel));
            }
            out.set("by_tool", by_tool);
            let mut models = Models::new();
            for (_, m) in ms {
                add_models(&mut models, &m.models);
            }
            out.set("models", models_view(&models, false));
            out
        };
        let lo = self.days[0].s();
        // (self.local.get('orphan') or {}).items()
        let orphan_rows = match self.local.get("orphan").filter(|o| o.truthy()) {
            None => Vec::new(),
            Some(V::Dict(o)) => o.items(),
            Some(other) => panic!("AttributeError: local.json 的 orphan 是 {}", other.py_repr()),
        };
        let (mut orphan_micro, mut orphan_requests) = (0i128, 0i128);
        for (k, r) in orphan_rows {
            let ints = r.as_list().filter(|r| r.len() == 7 && r.items().iter().all(is_int));
            let Some(r) = ints.filter(|_| COST_HOUR_RE.matches(&k) && hour_of(&k) + 1800.0 >= lo) else { continue };
            orphan_micro += r.at(0).as_int().unwrap_or(0) as i128;
            orphan_requests += r.at(1).as_int().unwrap_or(0) as i128;
        }
        let whole: Vec<(&'static str, &Money)> =
            self.accounts.iter().filter_map(|(t_, m, _)| m.as_ref().map(|m| (*t_, m))).collect();
        let mine: Vec<(&'static str, &Money)> =
            self.accounts.iter().flat_map(|(t_, _, ds)| ds.iter().filter(|d| d.me).map(move |d| (*t_, &d.money))).collect();
        // 别的设备按 id 归到一起（同一台在几个账号里的那几份），按 30 天的金额从多到少（一样多按名字，再按 id）
        let mut by_id: std::collections::BTreeMap<&str, Vec<(&'static str, &DevMoney)>> = Default::default();
        for (t_, _, ds) in &self.accounts {
            for d in ds.iter().filter(|d| !d.me) {
                by_id.entry(d.id.as_str()).or_default().push((*t_, d));
            }
        }
        let mut others: Vec<Vec<(&'static str, &DevMoney)>> = by_id.into_values().collect();
        let days_of = |g: &[(&'static str, &DevMoney)]| g.iter().map(|(_, d)| d.money.days).sum::<i128>();
        others.sort_by(|a, b| days_of(b).cmp(&days_of(a)).then_with(|| a[0].1.name.cmp(&b[0].1.name)));
        // 一台设备：是哪台（head），再加上几项统计、30 天每天、按工具分开（和 me 一样）
        let device = |head: Dict, ms: &[(&'static str, &Money)]| -> V {
            head.update(&part(ms));
            V::Dict(head)
        };
        let me_head =
            dict! { "id" => self.me_id(), "name" => self.me_name.as_str(), "me" => true, "revoked" => false, "stale" => false };
        let mut devices = vec![device(me_head, &mine)];
        for g in &others {
            let first = g[0].1;
            let ms: Vec<(&'static str, &Money)> = g.iter().map(|(t_, d)| (*t_, &d.money)).collect();
            // 很久没同步：每个账号里都很久没同步上来（revoked 3.6.6 起总是 false：移除了的设备不列）
            let head = dict! {
                "id" => first.id.as_str(), "name" => first.name.as_str(), "me" => false, "revoked" => false,
                "stale" => g.iter().all(|(_, d)| d.stale),
            };
            devices.push(device(head, &ms));
        }
        // 只有认不出是哪个账号的用量的设备也列出来（账号的几项都是 0；按认不出的那 30 天的多少排在后面）：看得到它用了多少
        let (orphan, oshares, synced) = self.orphan();
        let mut extra: Vec<&DevMoney> = oshares.iter().filter(|d| !d.me && !others.iter().any(|g| g[0].1.id == d.id)).collect();
        extra.sort_by(|a, b| b.money.days.cmp(&a.money.days).then_with(|| a.name.cmp(&b.name)));
        for d in extra {
            let head =
                dict! { "id" => d.id.as_str(), "name" => d.name.as_str(), "me" => false, "revoked" => false, "stale" => d.stale };
            devices.push(device(head, &[]));
        }
        // 设备菜单里列着、却没传过认不出是哪个账号的那一份的别的设备（cca 太旧，或者还没同步过）
        let unsynced: Vec<V> = others
            .iter()
            .filter(|g| !synced.contains(&g[0].1.id))
            .map(|g| V::Dict(dict! { "id" => g[0].1.id.as_str(), "name" => g[0].1.name.as_str() }))
            .collect();
        let out = part(&whole);
        out.set("currency", "USD");
        out.set("prices_date", COST_PRICES_DATE);
        out.set("me", part(&mine));
        out.set("devices", devices);
        out.set("scan", dict! { "state" => self.scan_state(), "updated_epoch" => self.updated });
        // 为什么认不出（这台设备的诊断）、这台设备命令行里最近用的 Claude Code 版本
        let diag = self.diag_view(&cost_diag_ok(self.local.get("diag"), self.now)).unwrap_or_default();
        out.set(
            "unattributed",
            dict! {
                "days_usd" => usd(orphan_micro), "requests" => count_v(orphan_requests),
                "why" => diag.get("why").unwrap_or_else(|| V::List(List::new())), "claude_code" => diag.get("claude_code").unwrap_or(V::None),
                "claude_code_old" => diag.get("claude_code_old").unwrap_or(V::Bool(false)),
                "cost" => orphan.map_or(V::None, V::Dict), "unsynced" => unsynced, "app_hook" => app_hook_view(),
            },
        );
        V::Dict(out)
    }
}

/// 只给测试用（`_cost-periods <现在> [额度窗口的 JSON]`）：打印 cost_days、cost_weeks 切出来的时段。这两个函数靠本地时区、
/// 夏令时，tests/cost_view.rs 拿固定的“现在”在几个时区下看
pub fn cmd_cost_periods(args: &[String]) -> R<()> {
    arity("cmd_cost_periods", args, 1, 2)?;
    let now = text::to_float(&args[0]).ok_or_else(|| Fail::Value("现在要是一个数".into()))?;
    let quota = match args.get(1) {
        Some(q) => Some(json::loads(q)?),
        None => None,
    };
    let rows = |ps: Vec<Period>| -> Vec<V> {
        ps.into_iter().map(|p| V::List(List::from_vec(vec![p.start, p.end, V::from(p.tag)]))).collect()
    };
    out!(
        "{}",
        json::dumps(
            &V::Dict(dict! { "days" => rows(cost_days(now)), "weeks" => rows(cost_weeks(quota.as_ref(), now)) }),
            json::LINE
        )
    );
    Ok(())
}

/// 只给测试用（`_cost-view <现在> <要看什么的 JSON>`；tests/cost_view.rs、tests/presence.rs）：照 cost/ 里现在的文件给出 CostView 的结果。
/// CostView 靠“现在”和本地时区切时段，_gui state 里的“现在”定不住：测试拿固定的时刻、固定的时区（TZ）核对。要看什么：
/// {"config": config.json 里它看的那几项（cost、presence、device_id）, "team": 用得了的同步服务器——一个 Team 的编号，或者几个编号的
///  列表（照列表的先后），写 "load" 是这台设备现在用得了的那几台（hubs()），没有是一台都用不了,
///  "accounts": [[工具, 账号表的键, usage_weeks 或 null, 谁给的]…], "sync": [[slot, 谁给的]…],
///  "tools": [启用了的工具名…]}。谁给的：Team 的编号是那个 Team 给的（经它的服务器同步；那一台用不了时只算这台设备的），
///  false 或 null 是这台设备自己的（只算这台设备的）。3.6.0 起没有维护者的服务器：以前的 "presence" 一项不看了，谁给的写 true 是错的
/// 打印一行 {"accounts": [各账号的 cost…], "sync": [各 slot 的 sync…], "total": cost_total, "scan": 统计到哪一步了}
pub fn cmd_cost_view(args: &[String]) -> R<()> {
    arity("cmd_cost_view", args, 2, 2)?;
    let bad = |what: &str| Fail::Value(format!("_cost-view：{what}"));
    let now = text::to_float(&args[0]).ok_or_else(|| bad("现在要是一个数"))?;
    let spec = json::loads(&args[1])?.as_dict().ok_or_else(|| bad("要看什么要是一个对象"))?;
    let team = |v: &V| v.as_str().and_then(TeamTag::new).map(HubKind::Team);
    let kinds: Vec<HubKind> = match spec.get("team") {
        Some(V::Str(s)) if &*s == "load" => hubs()?.iter().map(|h| h.kind).collect(),
        Some(V::List(l)) => l.items().iter().filter_map(team).collect(),
        Some(v) => team(&v).into_iter().collect(),
        None => Vec::new(),
    };
    let via = |who: &V| -> R<Option<HubKind>> {
        match who {
            V::Str(_) => Ok(team(who).filter(|k| kinds.contains(k))),
            V::None | V::Bool(false) => Ok(None),
            _ => Err(bad("谁给的要写 Team 的编号，或者 false（这台设备自己的）")),
        }
    };
    let mut cv = CostView::new(&spec.dict("config").unwrap_or_default(), now, &kinds);
    let rows = |k: &str| spec.list(k).map(|l| l.items()).unwrap_or_default();
    let mut accounts = Vec::new();
    for a in rows("accounts") {
        let a = a.as_list().filter(|a| a.len() == 4).ok_or_else(|| bad("账号要写成 [工具, 键, usage_weeks, 谁给的]"))?;
        let name = a.at(0).py_str();
        let tool = crate::core::tool::tools().iter().find(|t| t.name == name).ok_or_else(|| bad("没有这个工具"))?;
        let _using = Using::tool(tool);
        accounts.push(cv.account(tool, &a.at(1).py_str(), a.at(2).as_dict().as_ref(), via(&a.at(3))?));
    }
    let mut sync = Vec::new();
    for x in rows("sync") {
        let x = x.as_list().filter(|x| x.len() == 2).ok_or_else(|| bad("sync 要写成 [slot, 谁给的]"))?;
        sync.push(V::Dict(cv.sync_view(&x.at(0).py_str(), via(&x.at(1))?)));
    }
    let names: Vec<String> = rows("tools").iter().map(|n| n.py_str()).collect();
    let tools: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
    out!(
        "{}",
        json::dumps(
            &V::Dict(dict! { "accounts" => accounts, "sync" => sync, "total" => cv.total(&tools), "scan" => cv.scan_state() }),
            json::LINE
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text_: &str) -> Dict {
        json::loads(text_).unwrap_or_else(|e| panic!("{text_}：{e}")).as_dict().expect("是个对象")
    }

    /// `assertAlmostEqual`（差不到小数点后 7 位）
    fn near(got: Option<f64>, want: f64, what: &str) {
        let got = got.unwrap_or_else(|| panic!("{what}：没有金额"));
        assert!((got - want).abs() < 5e-8, "{what}：{got} != {want}");
    }

    #[test]
    fn new_versions_priced_by_family() {
        // 价格表里还没有的新版本：按同系列里不比它新、最新的那个估算；认不出系列的、比同系列的都旧的照旧没有公开价格
        for (m, want) in [
            ("claude-opus-5-5", Some(("claude-opus-5-5", false))),
            ("claude-opus-5-6", Some(("claude-opus-5-5", true))),
            ("claude-opus-6", Some(("claude-opus-5-5", true))),
            ("claude-opus-5-2", Some(("claude-opus-5", true))),
            ("claude-opus-5-05", Some(("claude-opus-5-5", true))),
            ("claude-sonnet-6-1", Some(("claude-sonnet-5-5", true))),
            ("claude-haiku-6", Some(("claude-haiku-5-5", true))),
            ("claude-fable-5-3", Some(("claude-fable-5-2", true))),
            ("claude-mythos-6", Some(("claude-mythos-5-2", true))),
            ("claude-opus-3", None),
            ("claude-new-9", None),
            ("claude-unknown-1", None),
            ("claude-opus-5-5-preview", None),
            ("claude-opus", None),
            ("gpt-6.3-sol", None),
        ] {
            assert_eq!(claude_priced_by(m), want, "{m}");
        }
        for (m, want) in [
            ("gpt-6.2-sol", Some(("gpt-6.2-sol", false))),
            ("gpt-6.3-sol", Some(("gpt-6.2-sol", true))),
            ("gpt-7-astra", Some(("gpt-6.2-astra", true))),
            ("gpt-6.2-luna", Some(("gpt-6-luna", true))),
            ("gpt-6.2-terra", Some(("gpt-6.1-terra", true))),
            ("gpt-5.4-codex", Some(("gpt-5.3-codex", true))),
            ("gpt-5-codex", None),
            ("codex-auto-review", None),
            ("gpt-6.2-nova", None),
            ("gpt-6.2.1-sol", None),
            ("claude-opus-5-6", None),
        ] {
            assert_eq!(openai_priced_by(m), want, "{m}");
        }
        assert_eq!(priced_as("claude-opus-5-6"), Some("claude-opus-5-5"));
        assert_eq!(priced_as("gpt-6.3-sol"), Some("gpt-6.2-sol"));
        assert_eq!(priced_as("claude-opus-5-5"), None, "有自己的标价：不是估算的");
        assert_eq!(priced_as("codex-auto-review"), None, "没有可估的");
        assert_eq!(priced_as(""), None);
        // 估算的照那个模型的一套算：快速模式、只在美国推理、长上下文、Ultrafast
        let u = d(r#"{"input_tokens": 1000, "output_tokens": 100, "speed": "fast", "inference_geo": "us"}"#);
        assert_eq!(cost_claude("claude-opus-5-6", &u), cost_claude("claude-opus-5-5", &u));
        let long = d(r#"{"input_tokens": 120000, "output_tokens": 100}"#);
        assert_eq!(cost_claude("claude-haiku-5-6", &long), cost_claude("claude-haiku-5-5", &long));
        let cu = d(r#"{"input_tokens": 300000, "cached_input_tokens": 1000, "output_tokens": 100}"#);
        for tier in [None, Some("ultrafast"), Some("flex")] {
            assert_eq!(cost_codex("gpt-6.3-sol", tier, 0.0, &cu), cost_codex("gpt-6.2-sol", tier, 0.0, &cu), "{tier:?}");
        }
        assert_eq!(cost_codex("codex-auto-review", None, 0.0, &cu).0, None);
    }

    #[test]
    fn price_tables_pinned_to_build() {
        // 价格表、估算的规则变了要同时改 COST_BUILD（和这里记的摘要）：不然各台设备不从头重算，以前的用量还按旧价、旧规则
        let tables = format!(
            "{CLAUDE_PRICES:?}{CLAUDE_LONG:?}{CLAUDE_FAST:?}{CLAUDE_NO_GEO:?}{CLAUDE_WEB_SEARCH:?}{OPENAI_PRICES:?}{OPENAI_LONG:?}\
             {OPENAI_TIERS:?}{OPENAI_ULTRA:?}{ULTRA_PROMO_END:?}{CLAUDE_FAMILIES:?}{OPENAI_FAMILIES:?}{CLAUDE_NAME:?}{OPENAI_NAME:?}"
        );
        assert_eq!(
            (COST_BUILD, &sha256_hex(tables.as_bytes())[..16]),
            ("2026-11-24/3", "bafda80d22537243"),
            "价格表变了：改 COST_BUILD，再把这里的两项换成新的"
        );
    }

    #[test]
    fn model_names_like_python_test() {
        // 会话记录里的模型名 → 价格表里的写法
        let long = "m".repeat(100);
        for (raw, want) in [
            (V::from("claude-opus-5-5[1m]"), "claude-opus-5-5"),
            (V::from("claude-sonnet-4-5-20250929"), "claude-sonnet-4-5"),
            (V::from("Claude-Haiku-4-5"), "claude-haiku-4-5"),
            (V::from("claude-x-2026-01-15"), "claude-x"),
            (V::from("gpt-6-astra"), "gpt-6-astra"),
            (V::from(" gpt-6.1-sol "), "gpt-6.1-sol"),
            (V::None, ""),
            (V::Int(5), ""),
            (V::from(long.as_str()), &long[..64]),
        ] {
            assert_eq!(cost_model(Some(&raw)), want, "{raw:?}");
        }
        assert_eq!(cost_model(None), "");
    }

    #[test]
    fn claude_prices_like_python_test() {
        // Anthropic 的标价：缓存、快速模式、只在美国推理、长上下文、网页搜索、没有公开价格的
        let f = |model: &str, u: &str| cost_claude(model, &d(u));
        let with = |more: &str| {
            format!(
                r#"{{"input_tokens": 1000, "output_tokens": 100, "cache_read_input_tokens": 10000, "cache_creation": {{"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 500}}{more}}}"#
            )
        };
        let u = with("");
        let (usd, tok) = f("claude-opus-5-5", &u);
        near(usd, 1000.0 * 4.0 + 2000.0 * 5.0 + 500.0 * 8.0 + 10000.0 * 0.2 + 100.0 * 20.0, "opus 5.5");
        assert_eq!(tok, [1000, 100, 10000, 2500], "(输入, 输出, 缓存读, 缓存写)");
        near(
            f("claude-opus-5-5", &with(r#", "speed": "fast""#)).0,
            1000.0 * 8.0 + 2000.0 * 10.0 + 500.0 * 16.0 + 10000.0 * 0.4 + 100.0 * 40.0,
            "快速模式：输入、输出按快速模式的标价，缓存跟着输入翻倍",
        );
        near(
            f("claude-sonnet-5-5", &with(r#", "speed": "fast""#)).0,
            f("claude-sonnet-5-5", &u).0.unwrap(),
            "没有快速模式标价的模型照常",
        );
        near(
            f("claude-opus-5-5", &with(r#", "inference_geo": "us""#)).0,
            f("claude-opus-5-5", &u).0.unwrap() * 1.1,
            "只在美国推理：加 10%",
        );
        near(
            f("claude-sonnet-4-5", &with(r#", "inference_geo": "us""#)).0,
            1000.0 * 3.0 + 2000.0 * 3.75 + 500.0 * 6.0 + 10000.0 * 0.3 + 100.0 * 15.0,
            "4.6 以前的模型不加价",
        );
        near(
            f("claude-opus-5-5", r#"{"cache_creation_input_tokens": 1000}"#).0,
            1000.0 * 5.0,
            "没有分 5 分钟、1 小时的：按 5 分钟算",
        );
        near(
            f("claude-opus-5-5", r#"{"output_tokens": 10, "server_tool_use": {"web_search_requests": 3}}"#).0,
            10.0 * 20.0 + 3.0 * 10000.0,
            "网页搜索一次 0.01 美元",
        );
        near(
            f("claude-haiku-5-5", r#"{"input_tokens": 50000, "output_tokens": 1000}"#).0,
            50000.0 * 0.1 + 1000.0 * 0.5,
            "haiku 5.5",
        );
        near(
            f("claude-haiku-5-5", r#"{"input_tokens": 90000, "cache_read_input_tokens": 20000, "output_tokens": 1000}"#).0,
            90000.0 * 0.5 + 20000.0 * 0.05 + 1000.0 * 2.5,
            "输入（含缓存）超过 10 万 token：长上下文的标价",
        );
        assert_eq!(f("claude-unknown-9", &u), (None, [1000, 100, 10000, 2500]), "没有公开价格的：只数 token");
        assert_eq!(f("claude-opus-5-6", &u), f("claude-opus-5-5", &u), "价格表里还没有的新版本：按同系列最新的估算");
        for junk in ["true", "-5", "1.5", r#""100""#, "10000000000000", "null"] {
            assert_eq!(
                f("claude-opus-5-5", &format!(r#"{{"input_tokens": {junk}, "output_tokens": 1}}"#)).1,
                [0, 1, 0, 0],
                "{junk}"
            );
        }
    }

    #[test]
    fn codex_prices_like_python_test() {
        // OpenAI 的标价：档位、长上下文、缓存写
        let now = 1795500000.0; // 2026-11-24：GPT-6.2 的 Ultrafast 还在促销
        let f = |model: &str, tier: Option<&str>, u: &str| cost_codex(model, tier, now, &d(u));
        let u = r#"{"input_tokens": 10000, "cached_input_tokens": 8000, "cache_write_input_tokens": 0, "output_tokens": 500, "reasoning_output_tokens": 300}"#;
        let base = 2000.0 * 10.0 + 8000.0 * 1.0 + 500.0 * 50.0;
        assert_eq!(f("gpt-6-astra", Some("default"), u), (Some(base), [2000, 500, 8000, 0]), "输入含缓存读；推理算在输出里");
        for (tier, k) in [(None, 1.0), (Some("priority"), 2.0), (Some("fast"), 2.0), (Some("flex"), 0.5), (Some("ultrafast"), 6.0)]
        {
            near(f("gpt-6-astra", tier, u).0, base * k, &format!("{tier:?}"));
        }
        near(
            f("gpt-6-sol", Some("ultrafast"), u).0,
            (2000.0 * 2.0 + 8000.0 * 0.2 + 500.0 * 10.0) * 2.0,
            "没有 Ultrafast 标价的模型按 Fast 算",
        );
        near(f("gpt-6-astra", None, r#"{"input_tokens": 300000}"#).0, 300000.0 * 20.0, "输入超过 272K：长上下文的标价");
        let write = r#"{"input_tokens": 5000, "cached_input_tokens": 1000, "cache_write_input_tokens": 2000}"#;
        near(f("gpt-6-astra", None, write).0, 2000.0 * 10.0 + 1000.0 * 1.0 + 2000.0 * 12.5, "缓存写");
        near(f("gpt-5.3-codex", None, write).0, 2000.0 * 1.75 + 1000.0 * 0.175 + 2000.0 * 1.75, "没有缓存写标价的按输入算");
        near(f("gpt-5.3-codex", None, r#"{"input_tokens": 300000}"#).0, 300000.0 * 1.75, "没有长上下文标价的照常");
        assert_eq!(
            f("gpt-6-astra", None, r#"{"input_tokens": 100, "cached_input_tokens": 500}"#).1,
            [0, 0, 100, 0],
            "缓存不超过输入"
        );
        assert_eq!(f("codex-auto-review", None, u), (None, [2000, 500, 8000, 0]));
        // GPT-6.2（2026-11-17 起）：标价、长上下文、缓存写；Ultrafast 到 2027-02-28 是 3 倍，2027-03-01（太平洋时间）起 6 倍
        let astra = 2000.0 * 8.0 + 8000.0 * 0.40 + 500.0 * 40.0;
        near(f("gpt-6.2-astra", None, u).0, astra, "gpt-6.2-astra");
        near(f("gpt-6.2-sol", None, u).0, 2000.0 * 2.0 + 8000.0 * 0.10 + 500.0 * 10.0, "gpt-6.2-sol");
        near(f("gpt-6.2-astra", None, r#"{"input_tokens": 300000}"#).0, 300000.0 * 16.0, "gpt-6.2-astra 长上下文");
        near(f("gpt-6.2-sol", None, r#"{"input_tokens": 300000}"#).0, 300000.0 * 4.0, "gpt-6.2-sol 长上下文");
        near(f("gpt-6.2-astra", None, write).0, 2000.0 * 8.0 + 1000.0 * 0.40 + 2000.0 * 10.0, "gpt-6.2-astra 缓存写");
        near(f("gpt-6.2-astra", Some("fast"), u).0, astra * 2.0, "gpt-6.2-astra Fast");
        near(f("gpt-6.2-astra", Some("ultrafast"), u).0, astra * 3.0, "促销期 Ultrafast 3 倍");
        near(
            cost_codex("gpt-6.2-astra", Some("ultrafast"), ULTRA_PROMO_END - 1.0, &d(u)).0,
            astra * 3.0,
            "2027-02-28 最后一秒还是 3 倍",
        );
        near(cost_codex("gpt-6.2-astra", Some("ultrafast"), ULTRA_PROMO_END, &d(u)).0, astra * 6.0, "2027-03-01 起 6 倍");
        near(cost_codex("gpt-6-astra", Some("ultrafast"), ULTRA_PROMO_END, &d(u)).0, base * 6.0, "gpt-6-astra 一直 6 倍");
    }

    #[test]
    fn bin_by_midpoint_like_python_test() {
        // 小时数据按时段加起来：跨过边界的小时按中点算；时段以外的不算
        let period = |s: i64, e: i64, tag: &str| Period { start: V::Int(s), end: V::Int(e), tag: tag.to_string() };
        let periods = [period(1000 * 3600, 1000 * 3600 + 5400, "x"), period(1000 * 3600 + 5400, 1003 * 3600, "y")];
        let h: Hours = [(1000, 1), (1001, 2), (1002, 4), (999, 8), (1003, 16)]
            .into_iter()
            .map(|(hour, usd)| (hour * 3600, [usd, 1, 0, 0, 0, 0, 0]))
            .collect();
        assert_eq!(cost_bin(&periods, &h).iter().map(|r| r[0]).collect::<Vec<_>>(), [1, 6]);
    }

    #[test]
    fn content_like_python_test() {
        // tests/cost_view.rs 的 run_and_publish 里看 cost_content 的那几处：这台设备上这个账号的小时数据——
        // 这个工具的、登录的邮箱是它的身份加起来，工具分开（local.json 是那个用例统计出来的样子）
        let local = d(r#"{"v": 1, "build": "2026-11-13/1",
            "who": {"claude:aaaaaaaa-0000-4000-8000-000000000001": "alice@example.com", "claude:aaaaaaaa-0000-4000-8000-000000000002": "bob@example.com",
                    "codex:user-cccccccccccccccccccccccc": "carol@example.com"},
            "acc": {"claude:aaaaaaaa-0000-4000-8000-000000000001": {"1794900000": [12000, 2, 2000, 200, 0, 0, 0], "1794903600": [0, 1, 1000, 100, 0, 0, 1]},
                    "claude:aaaaaaaa-0000-4000-8000-000000000002": {"1794907200": [12000, 2, 2000, 200, 0, 0, 0]},
                    "codex:user-cccccccccccccccccccccccc": {"1794900000": [140000, 3, 5000, 600, 8000, 0, 0]}},
            "um": {"claude:aaaaaaaa-0000-4000-8000-000000000001": ["claude-unknown-1"]}, "orphan": {"1794900000": [6000, 1, 1000, 100, 0, 0, 0]}}"#);
        let (h, _, um) = cost_content(&local, claude(), "alice@example.com");
        let usd: i64 = h.values().iter().map(|r| r.as_list().unwrap().at(0).as_int().unwrap()).sum();
        assert_eq!((usd, um), (12000, vec!["claude-unknown-1".to_string()]));
        let (h, _, um) = cost_content(&local, codex(), "alice@example.com");
        assert!(h.is_empty() && um.is_empty(), "工具分开");
        assert_eq!(cost_content(&local, codex(), "carol@example.com").0.len(), 1);
    }

    #[test]
    fn models_split_and_remainder() {
        // 按模型分的两段（列出的周期、30 天）：各个模型的照中点归到时段里（时段以外的不算）；合计减去各个模型的是没有按模型记下的
        // （None），不会是负的；排序：列出的周期的金额、请求数，再按 30 天的金额、模型名，None 在最后；两段都没有的不列
        let period = |s: i64, e: i64| Period { start: V::Int(s * 3600), end: V::Int(e * 3600), tag: String::new() };
        let (weeks, days) = ([period(0, 10), period(10, 20)], [period(10, 20)]);
        let c = |usd: i128, req: i128| [usd, req, 10 * req, req, 0, 0, 0];
        let hm: ModelHours = [
            (2 * 3600, vec![("a".to_string(), c(5, 1)), ("b".to_string(), c(5, 2)), ("y".to_string(), c(5, 1))]),
            (12 * 3600, vec![("a".to_string(), c(1, 1)), ("z".to_string(), c(0, 0))]),
            (30 * 3600, vec![("c".to_string(), c(9, 9))]),
        ]
        .into_iter()
        .collect();
        let ms = device_models(&weeks, &days, &hm, &[c(17, 4), c(1, 1)], &[c(1, 1)]);
        assert_eq!(ms.get(&None), Some(&[c(2, 0), [0; 7]]), "列出的周期剩下 2 美元没有按模型记下；30 天里都记下了");
        assert!(!ms.contains_key(&Some("c".to_string())), "时段以外的不算");
        let names = |rows: &[V]| -> Vec<Option<String>> {
            rows.iter().map(|r| r.as_dict().unwrap().str("model").map(|m| m.to_string())).collect()
        };
        let rows = models_view(&ms, false);
        assert_eq!(
            names(&rows),
            [Some("a".to_string()), Some("b".to_string()), Some("y".to_string()), None],
            "金额一样时请求多的在前（b 比 y）；z 两段都没有：不列"
        );
        let a = rows[0].as_dict().unwrap();
        assert_eq!(
            json::dumps(&a.get("days").unwrap(), json::Fmt::new()),
            r#"{"usd": 0.0, "requests": 1, "unpriced_requests": 0}"#,
            "设备的不带 tokens"
        );
        let with = models_view(&ms, true);
        let wk = with[0].as_dict().unwrap().get("weeks").unwrap();
        assert_eq!(
            json::dumps(&wk.as_dict().unwrap().get("tokens").unwrap(), json::Fmt::new()),
            r#"{"input": 20, "output": 2, "cache_read": 0, "cache_write": 0}"#
        );
        let over = device_models(&weeks, &days, &hm, &[c(3, 9), c(0, 0)], &[c(0, 0)]);
        assert_eq!(
            over.get(&None).map(|x| x[0]),
            Some([0, 4, 40, 4, 0, 0, 0]),
            "各个模型的比合计多（传来的不一致）：剩下的按 0 算，不是负的"
        );
        assert!(!device_models(&weeks, &days, &hm, &[c(16, 5), c(0, 0)], &[c(1, 1)]).contains_key(&None), "都记下了：没有 None");
    }

    #[test]
    fn lock_waits_then_gives_up() {
        // tests/cost_view.rs 的 sync_command 的最后一段（那里把 COST_WAIT 换成 0.2 秒）：另一个统计拿着 cost.lock 一直没结束，
        // 等够了时间是 CostBusy（cca cost sync 说一句、退出码 1，见下面的 sync_failures_exit_1）；它放了锁就拿得到
        let dir = crate::py::fs::TempDir::new().unwrap();
        let p = path::join(&dir.path, "cost.lock");
        let held = cost_lock(&p, 0.0).unwrap();
        let t0 = monotonic();
        assert!(cost_lock(&p, 0.2).is_err_and(|e| e.is("CostBusy")));
        assert!(monotonic() - t0 >= 0.2, "等了给的那么久");
        assert!(cost_lock(&p, 0.0).is_err_and(|e| e.is("CostBusy")), "后台的那种不等");
        drop(held);
        assert!(cost_lock(&p, 0.0).is_ok());
        assert!(cost_lock(&path::join(&dir.path, "nowhere/cost.lock"), 0.0).is_err_and(|e| e.is_os()));
    }

    #[test]
    fn sync_failures_exit_1() {
        // cca cost sync：另一个统计一直没结束、读写出了错都是退出码 1（原因说在 stdout）；别的错照旧往上传
        assert!(matches!(cost_sync_failed(cost_busy()), Err(Fail::Exit(1))));
        assert!(matches!(cost_sync_failed(Fail::Os(std::io::Error::other("磁盘满了"))), Err(Fail::Exit(1))));
        assert!(matches!(cost_sync_failed(Fail::Value("写坏了".into())), Err(Fail::Exit(1))));
        assert!(matches!(cost_sync_failed(Fail::Die("config.json 读不了".into())), Err(Fail::Die(_))));
        assert_eq!(COST_WAIT, 240);
    }

    #[test]
    fn compact_seen_reads_only_well_formed_entries() {
        let s =
            CompactSeen::parse(r#"{"c0123456789abcdef":1794024000, "x00000000000000ff" : 0 ,"cffffffffffffffff":15461882254800}"#)
                .unwrap();
        assert_eq!(s.entries.len(), 3);
        assert_eq!((s.entries[0].kind, s.entries[0].id, s.entries[0].hour), (b'c', 0x0123456789abcdef, 498340));
        assert_eq!((s.entries[1].kind, s.entries[1].id, s.entries[1].hour), (b'x', 0xff, 0));
        assert_eq!(s.entries[2].hour, u32::MAX - 2); // 15461882254800 / 3600
        assert!(CompactSeen::parse("{}").unwrap().entries.is_empty());
        assert!(CompactSeen::parse(" { } ").unwrap().entries.is_empty());
        for bad in [
            "",
            "[]",
            "{",
            r#"{"c0123456789abcdef":1794024000"#,
            r#"{"c0123456789abcdef":1794024000,}"#,
            r#"{"c0123456789abcdef":1794024000} x"#,
            r#"{"c0123456789abcdef":1794024001}"#,  // 不是整点
            r#"{"c0123456789abcdef":01794024000}"#, // JSON 里数不能以 0 开头
            r#"{"c0123456789abcdef":-3600}"#,
            r#"{"c0123456789abcdef":3600.0}"#,
            r#"{"c0123456789abcdef":"3600"}"#,
            r#"{"c0123456789abcdef":true}"#,
            r#"{"c0123456789abcdef":1e30}"#,
            r#"{"c0123456789abcdef":15461882265600}"#, // 小时数放不进 32 位
            r#"{"c0123456789abcdef":99999999999999999999999999999999993600}"#,
            r#"{"C0123456789abcdef":3600}"#,
            r#"{"c0123456789ABCDEF":3600}"#,
            r#"{"c0123456789abcde":3600}"#,
            r#"{"c0123456789abcdef0":3600}"#,
            r#"{"y0123456789abcdef":3600}"#,
            r#"{"odd key":3600}"#,
            r#"{"c0123456789abcdef":3600,"c0123456789abcdef":7200}"#, // 同一个键两次：Python 留后一个的值、位置是前一个的
        ] {
            assert!(CompactSeen::parse(bad).is_none(), "{bad}");
        }
        // 键里有转义（反斜杠 u0036 就是 6）：不认
        let escaped = r#"{"c012345BSu0036789abcdef":3600}"#.replace("BS", "\\");
        assert!(CompactSeen::parse(&escaped).is_none());
    }

    #[test]
    fn split_seen_finds_the_top_level_member() {
        let doc = r#" {"v":1,"files":{"/a\"b}":{"seen":{"x":1}}},"seen" : {"c0123456789abcdef":3600} ,"h":{"seen":[1,{"}":"]"}]},"s":"a\\","n":-1.5e3} "#;
        let (rest, seen) = split_seen(doc).unwrap();
        assert_eq!(seen, r#"{"c0123456789abcdef":3600}"#);
        assert_eq!(
            rest,
            r#" {"v":1,"files":{"/a\"b}":{"seen":{"x":1}}},"seen" : {} ,"h":{"seen":[1,{"}":"]"}]},"s":"a\\","n":-1.5e3} "#
        );
        assert!(json::loads(&rest).is_ok());
        assert_eq!(split_seen(r#"{"seen":{}}"#).unwrap(), (r#"{"seen":{}}"#.to_string(), "{}"));
        // 键里有转义（反斜杠 u0065 就是 e）：不认，交给通用的读法
        let escaped = r#"{"seBSu0065n":{}}"#.replace("BS", "\\");
        assert!(escaped.contains('\\') && json::loads(&escaped).is_ok() && split_seen(&escaped).is_none());
        for none in [
            "",
            "[]",
            r#"{"v":1}"#,               // 没有 seen
            r#"{"seen":[1]}"#,          // 不是对象
            r#"{"seen":{},"seen":{}}"#, // 两个
            r#"{"seen":{}} x"#,
            r#"{"seen":{}"#,
            r#"{"seen":{"a":"unterminated}}"#,
            r#"{"seen":{},}"#,
            r#"{"seen" {}}"#,
            r#"{seen:{}}"#,
        ] {
            assert!(split_seen(none).is_none(), "{none}");
        }
    }

    #[test]
    fn seen_behaves_like_the_dict() {
        let mut compact = Seen::new();
        let mut generic = Seen::Generic(Dict::new());
        for (i, hour) in [1794024000i64, 1794027600, 1790000000 / 3600 * 3600, 1794024000].into_iter().enumerate() {
            for s in [&mut compact, &mut generic] {
                assert!(!s.has(b'c', i as u64));
                s.add(b'c', i as u64, hour);
                assert!(s.has(b'c', i as u64) && !s.has(b'x', i as u64));
            }
        }
        let dump = |s: &Seen| json::dumps(&V::Dict(s.to_dict()), json::Fmt::new().compact());
        assert_eq!(dump(&compact), dump(&generic));
        for s in [&mut compact, &mut generic] {
            s.retain(1794024000.0);
            assert_eq!(s.len(), 3);
            assert!(!s.has(b'c', 2) && s.has(b'c', 3));
        }
        assert_eq!(dump(&compact), dump(&generic));
        assert_eq!(
            dump(&compact),
            r#"{"c0000000000000000":1794024000,"c0000000000000001":1794027600,"c0000000000000003":1794024000}"#
        );
        // 放不进紧凑的表的值：整张表换成通用的，内容不变
        compact.add(b'x', 9, 1794024001);
        assert!(matches!(compact, Seen::Generic(_)) && compact.has(b'c', 0) && compact.has(b'x', 9) && compact.len() == 4);
        // hashlib.sha1(b"abc").hexdigest()[:16] == "a9993e364706816a"
        assert_eq!(sha1_head(b"abc"), 0xa9993e364706816a);
    }
}
