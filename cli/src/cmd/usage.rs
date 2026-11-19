//! 用量（`cmd_usage_query` 到 `cmd_reset_done`）：usage-query、login-plan-forget、usage-render、reset-plan、reset-done，
//! 以及时间的写法、套餐的叫法和档位、用量缓存、每周用量。
//!
//! 这一块的子命令都移植了。`Tool` 里按工具分派、和用量 / 套餐有关的几个方法（who、plan、plan_tier、outdated_login_plan、
//! usage_query、usage_caches）也在这里；Codex 那一半（app-server、登录身份）在 codex.rs。

use super::codex::{agy_claims, codex_claims, codex_plan_label, codex_plan_tier, codex_usage_query, json_loads_bytes, temp_file, Got, Piped};
use super::cost::cost_kick;
use super::keyshare::{is_num, keyshare_kick};
use super::{arg, arity, Cmd};
use crate::core::base::{sh, str_or_empty, CCA_HOME};
use crate::core::ctx::{acct_json, ctx, oauth_of};
use crate::core::model::{adir, is_api, name_of, parse_key};
use crate::core::tool::{t, Kind, Tool, AGY_TIERS};
use crate::py::fs::{load_quiet, read_text, save_json};
use crate::py::io;
use crate::py::json;
use crate::py::num::{float_repr, half_up, round_to};
use crate::py::obj::{repr_str, Dict, V};
use crate::py::path;
use crate::py::proc::env_or_empty;
use crate::py::text::{self, fit};
use crate::py::time::{self, timegm, Tm};
use crate::py::uni;
use crate::py::{die, exit, Fail, R};
use crate::{dict, out, re};

/// 这个模块移植好的子命令
pub const CMDS: &[Cmd] = &[
    Cmd::all("usage-query", cmd_usage_query),
    Cmd::all("usage-render", cmd_usage_render),
    Cmd::all("login-plan-forget", cmd_login_plan_forget),
    Cmd::all("reset-plan", cmd_reset_plan),
    Cmd::all("reset-done", cmd_reset_done),
];

// --- 订阅用量（cca usage、菜单）：让 Claude Code 自己查（get_usage 控制请求），cca 不碰登录凭证 ---------------
// （Codex 账号：让 codex 的 app-server 查，account/rateLimits/read，见 codex.rs 的 codex_usage_query）
pub const USAGE_REQ: &str = "cca-usage";
pub const WEEKDAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
/// 查不了的原因 → 给人看的说明（花括号里的由 usage_why 填）
const USAGE_WHY: &[(&str, &str)] = &[
    ("login", "还没登录（{cmd} @{name} 登录）"),
    ("mismatch", "登录的是 {msg}，不是这个账号（{cmd} @{name} 改回）"),
    ("route", "账号出口未能核实，未发起查询"),
    ("port", "出口端口 {msg} 不通"),
    ("trace", "出口连不上 {host}"),
    ("region", "{msg}，没有查询"),
    ("unsupported", "这个版本的 {product} 查不了用量（{update} 升级后再试）"),
    ("timeout", "查询超时"),
    ("nolimits", "没有订阅用量（按量计费的账号，或者登录已失效：{cmd} login {name} 重新登录）"),
    ("auth", "登录已失效（比如改过密码）：{cmd} login {name} 重新登录"),
    ("error", "查询失败：{msg}"),
];

// --- 原来靠 Python 的数字、文字的几样行为 --------------------------------------------------------------------

/// 没人接的 TypeError
pub(super) fn type_error(msg: impl Into<String>) -> Fail {
    Fail::Other("TypeError", vec![msg.into()])
}

/// 没人接的 OverflowError
fn overflow_error(msg: impl Into<String>) -> Fail {
    Fail::Other("OverflowError", vec![msg.into()])
}

/// 拿 JSON 里的值当键去查表（`表.get(值)`、`值 in 表`）：列表、字典不能当键，原来是没人接的 TypeError
pub(super) fn hashable(v: &V) -> R<()> {
    match v {
        V::List(_) => Err(type_error("unhashable type: 'list'")),
        V::Dict(_) => Err(type_error("unhashable type: 'dict'")),
        _ => Ok(()),
    }
}

/// `float(s)`：认不出是没人接的 ValueError
pub(super) fn float_arg(s: &str) -> R<f64> {
    text::to_float(s).ok_or_else(|| Fail::Value(format!("could not convert string to float: {}", repr_str(s))))
}

/// `max(a, b)`：b 大才取 b（有 NaN 时比不出大小，留着 a；`f64::max` 不是这样）
pub(super) fn py_max(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

/// `int(x)`，x 是小数：NaN 是 ValueError，无穷大是 OverflowError。放不进 i64 的（Python 里是任意长的整数）这里也按
/// OverflowError 出错：用它的地方（取到用量是多少秒以前）走到这里之前，这么离谱的时间已经在别处出错了
fn py_int(x: f64) -> R<i64> {
    if x.is_nan() {
        return Err(Fail::Value("cannot convert float NaN to integer".into()));
    }
    if x.is_infinite() {
        return Err(overflow_error("cannot convert float infinity to integer"));
    }
    let n = x.trunc();
    if n.abs() >= 9.2e18 {
        return Err(overflow_error(format!("整数太大，放不下：{}", float_repr(x))));
    }
    Ok(n as i64)
}

/// 十进制写的非负整数除以一个小的数：(商的十进制写法, 余数)。时长大到放不进 i64 时用——Python 的整数没有上限，
/// 重置时间写成 1e30 这样的数它照样算得出「多少天多少小时」
pub(super) fn div_small(digits: &str, d: u64) -> (String, u64) {
    let (mut q, mut r) = (String::with_capacity(digits.len()), 0u64);
    for c in digits.bytes() {
        r = r * 10 + (c - b'0') as u64;
        if !(q.is_empty() && r < d) {
            q.push((b'0' + (r / d) as u8) as char);
        }
        r %= d;
    }
    (if q.is_empty() { "0".into() } else { q }, r)
}

/// `int(max(secs, 0) // 60)`：整分钟数
enum Minutes {
    Small(i64),
    /// 放不进 i64 的：十进制的写法
    Huge(String),
}

fn minutes(secs: f64) -> R<Minutes> {
    let m = floordiv_f(py_max(secs, 0.0), 60.0);
    if m.is_nan() {
        return Err(Fail::Value("cannot convert float NaN to integer".into())); // 无穷大整除 60 也是 NaN
    }
    Ok(if m < 9.0e18 { Minutes::Small(m as i64) } else { Minutes::Huge(format!("{m:.0}")) })
}

/// `x // y`，两个小数、y 大于 0：照 CPython 的 float_floor_div（先 fmod 再除，商贴到最近的整数）
fn floordiv_f(x: f64, y: f64) -> f64 {
    let m = x % y;
    let mut div = (x - m) / y;
    if m != 0.0 && (y < 0.0) != (m < 0.0) {
        div -= 1.0; // 余数要和除数同号
    }
    if div != 0.0 {
        let floor = div.floor();
        if div - floor > 0.5 {
            floor + 1.0
        } else {
            floor
        }
    } else {
        0.0_f64.copysign(x / y)
    }
}

/// `round(x)` 之后要的两样：`%d` 的写法（多大的数都写得出来）和它的值（比大小用）
struct Rounded {
    text: String,
    val: f64,
}

/// `round(x)`，x 是 JSON 里的数：整数原样，小数四舍六入五成双；NaN、无穷大是没人接的 ValueError / OverflowError
fn py_round(x: &V) -> R<Rounded> {
    match x {
        V::Int(i) => Ok(Rounded { text: i.to_string(), val: *i as f64 }),
        V::Big(s) => Ok(Rounded { text: s.to_string(), val: s.parse().unwrap_or(f64::INFINITY) }),
        V::Float(f) if f.is_nan() => Err(Fail::Value("cannot convert float NaN to integer".into())),
        V::Float(f) if f.is_infinite() => Err(overflow_error("cannot convert float infinity to integer")),
        V::Float(f) => {
            let r = f.round_ties_even();
            // 9e18 以内直接转整数（-0.0 写成 0）；更大的小数本来就是整数，照它的十进制写法
            let text = if r.abs() < 9.0e18 { (r as i64).to_string() } else { format!("{r:.0}") };
            Ok(Rounded { text, val: r })
        }
        other => Err(type_error(format!("type {} doesn't define __round__ method", other.py_repr()))),
    }
}

/// JSON 里的数当小数用（`float(x)`）；不是数的不该走到这里
fn num(v: &V) -> f64 {
    v.as_num().unwrap_or_else(|| panic!("TypeError: 要的是数字，拿到的是 {}", v.py_repr()))
}

/// `a + b`：两个都是整数还是整数，有一个是小数就是小数
fn n_add(a: &V, b: &V) -> V {
    match (a, b) {
        (V::Int(x), V::Int(y)) => x.checked_add(*y).map_or_else(|| V::Float(*x as f64 + *y as f64), V::Int),
        _ => V::Float(num(a) + num(b)),
    }
}

/// `a - b`
fn n_sub(a: &V, b: &V) -> V {
    match (a, b) {
        (V::Int(x), V::Int(y)) => x.checked_sub(*y).map_or_else(|| V::Float(*x as f64 - *y as f64), V::Int),
        _ => V::Float(num(a) - num(b)),
    }
}

/// `a < b`：两个整数照整数比（很大的整数换成小数会并到一起）
fn n_lt(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Int(x), V::Int(y)) => x < y,
        _ => num(a) < num(b),
    }
}

/// `n / d`，两个整数的真除法（d 大于 0）：结果是离真值最近的那个小数，一半时取偶——CPython 的整数除法就是这样算的，
/// 不是先把 n 换成小数再除（n 超过 2^53 时那样会多舍入一次）
fn true_div(n: i128, d: u32) -> f64 {
    let (neg, a, d) = (n < 0, n.unsigned_abs(), d as u128);
    let sign = if neg { -1.0 } else { 1.0 };
    if a < (1u128 << 53) {
        return sign * (a as f64 / d as f64);
    }
    let bits = |x: u128| 128 - x.leading_zeros();
    let s = 56u32.saturating_sub(bits(a / d)); // 左移到商至少有 56 位：留出舍入要看的那几位
    let (t, rem) = ((a << s) / d, (a << s) % d);
    let drop = bits(t) - 53;
    let (mut mant, low, half) = (t >> drop, t & ((1u128 << drop) - 1), 1u128 << (drop - 1));
    if low > half || (low == half && (rem != 0 || mant & 1 == 1)) {
        mant += 1;
    }
    sign * mant as f64 * 2f64.powi(drop as i32 - s as i32)
}

/// `列表.sort(key=…)`：稳定的；lt(a, b) 是 a 的键比 b 的小。键里有 NaN（比不出大小）也不会出错——标准库的排序遇到
/// 不成全序的比较可能 panic
fn sort_stable<T>(v: &mut [T], lt: impl Fn(&T, &T) -> bool) {
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && lt(&v[j], &v[j - 1]) {
            v.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// 第一个字符的标题写法（`str.capitalize()` 用的）：绝大多数就是大写，下面这几类不一样（照 CPython 3.14 / Unicode 16 列的）
fn titlecase(c: char) -> String {
    let u = c as u32;
    let one = |x: u32| char::from_u32(x).map_or_else(String::new, |ch| ch.to_string());
    match u {
        0xdf => "Ss".into(),
        0x1c4..=0x1c6 => one(0x1c5),
        0x1c7..=0x1c9 => one(0x1c8),
        0x1ca..=0x1cc => one(0x1cb),
        0x1f1..=0x1f3 => one(0x1f2),
        0x587 => "\u{535}\u{582}".into(),
        // 格鲁吉亚文：大写是另一套字母（Mtavruli），标题写法是它自己
        0x10d0..=0x10fa | 0x10fd..=0x10ff => c.to_string(),
        // 带下加小 ι 的希腊字母：标题写法是一个字符，大写是拆开的两个
        0x1f80..=0x1faf => one(u | 8),
        0x1fb2 => "\u{1fba}\u{345}".into(),
        0x1fb3 | 0x1fbc => one(0x1fbc),
        0x1fb4 => "\u{386}\u{345}".into(),
        0x1fb7 => "\u{391}\u{342}\u{345}".into(),
        0x1fc2 => "\u{1fca}\u{345}".into(),
        0x1fc3 | 0x1fcc => one(0x1fcc),
        0x1fc4 => "\u{389}\u{345}".into(),
        0x1fc7 => "\u{397}\u{342}\u{345}".into(),
        0x1ff2 => "\u{1ffa}\u{345}".into(),
        0x1ff3 | 0x1ffc => one(0x1ffc),
        0x1ff4 => "\u{38f}\u{345}".into(),
        0x1ff7 => "\u{3a9}\u{342}\u{345}".into(),
        0xfb00 => "Ff".into(),
        0xfb01 => "Fi".into(),
        0xfb02 => "Fl".into(),
        0xfb03 => "Ffi".into(),
        0xfb04 => "Ffl".into(),
        0xfb05 | 0xfb06 => "St".into(),
        0xfb13 => "\u{544}\u{576}".into(),
        0xfb14 => "\u{544}\u{565}".into(),
        0xfb15 => "\u{544}\u{56b}".into(),
        0xfb16 => "\u{54e}\u{576}".into(),
        0xfb17 => "\u{544}\u{56d}".into(),
        _ => c.to_uppercase().collect(),
    }
}

/// `s.capitalize()`：第一个字符换成标题写法，其余小写
pub(super) fn capitalize(s: &str) -> String {
    let Some(first) = s.chars().next() else {
        return String::new();
    };
    // 整串一起转小写（词尾的 Σ 要看前面的字符），再把第一个字符的那一段换掉
    let lower = s.to_lowercase();
    let rest: String = lower.chars().skip(first.to_lowercase().count()).collect();
    titlecase(first) + &rest
}

// --- 查用量 -------------------------------------------------------------------------------------------------

pub fn cmd_usage_query(args: &[String]) -> R<()> {
    arity("cmd_usage_query", args, 1, 2)?;
    t().usage_query(&args[0], arg(args, 1, "25"))
}

/// 在账号的环境里（bash 的 child_env 已经设好 CLAUDE_CONFIG_DIR 和出口）无界面启动 claude，发 get_usage 控制
/// 请求取用量。--safe-mode 不加载 hooks、MCP、插件；不发消息、不耗额度；token 由 Claude Code 自己读取和刷新。
/// 拿到回应之前 stdin 一直开着（太早关掉请求可能被取消）。stdout 打印一行 JSON：
/// {ok: true, usage: 回应} 或 {ok: false, why: unsupported|timeout|error, msg: 说明}
pub fn claude_usage_query(claude: &str, timeout: &str) -> R<()> {
    let cd = env_or_empty("CLAUDE_CONFIG_DIR");
    let err = temp_file()?;
    let cwd = if path::isdir(&cd) { path::dirname(&cd) } else { CCA_HOME.clone() };
    let argv = [
        claude, "-p", "--safe-mode", "--no-session-persistence", "--strict-mcp-config",
        "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
    ];
    let mut p = match Piped::spawn(&argv, &cwd, err) {
        Ok(p) => p,
        Err(e) if e.is_os() || e.is("OSError") => {
            out!("{}", json::dumps(&V::Dict(dict! { "ok" => false, "why" => "error", "msg" => e.to_string() }), json::LINE));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let req = dict! { "type" => "control_request", "request_id" => USAGE_REQ, "request" => dict! { "subtype" => "get_usage", "skip_behaviors" => true } };
    p.write(format!("{}\n", json::dumps(&V::Dict(req), json::Fmt::new())).as_bytes()); // 写不进去（它已经退出了）不管，下面读到头再说
    let mut out = dict! { "ok" => false, "why" => "timeout", "msg" => "" };
    let end = time::time() + float_arg(timeout)?;
    loop {
        let line = match p.get(end, false)? {
            Got::Empty => break,
            // 没回应就退出了：多半是太老的版本（不认 --safe-mode 等参数）
            Got::Eof => {
                p.wait(3.0);
                let said = p.err_tail();
                let tail = text::splitlines(text::strip(&said));
                let old = re!(r"unknown option|nsupported").found(&tail.join(" "));
                let msg = match tail.last() {
                    Some(last) => text::head(last, 200).to_string(),
                    None => format!("没有回应就退出了（退出码 {}）", p.returncode().map_or_else(|| "None".to_string(), |rc| rc.to_string())),
                };
                out = dict! { "ok" => false, "why" => if old { "unsupported" } else { "error" }, "msg" => msg };
                break;
            }
            Got::Line(line) => line,
        };
        let Some(V::Dict(m)) = json_loads_bytes(&line) else { continue };
        if !m.get("type").is_some_and(|x| x.as_str() == Some("control_response")) {
            continue;
        }
        let Some(r) = m.dict("response") else { continue };
        if !r.get("request_id").is_some_and(|x| x.as_str() == Some(USAGE_REQ)) {
            continue;
        }
        if r.get("subtype").is_some_and(|x| x.as_str() == Some("success")) {
            out = dict! { "ok" => true, "usage" => r.dict("response").unwrap_or_default() };
        } else {
            let e = str_or_empty(&r.get("error"));
            out = dict! { "ok" => false, "why" => if e.contains("nsupported") { "unsupported" } else { "error" }, "msg" => text::head(&e, 200) };
        }
        break;
    }
    if out.get("why").is_some_and(|w| w.as_str() == Some("timeout")) {
        p.kill();
    }
    p.close_stdin();
    if !p.wait(5.0) {
        p.kill();
    }
    out!("{}", json::dumps(&V::Dict(out), json::LINE));
    Ok(())
}

/// ISO 时间（小数秒、Z、时区都行）或秒/毫秒数 → 秒；看不懂返回 None。月份不在 1–12、年份是 0 时 Python 的 calendar.timegm
/// 报 ValueError（没人接），这里也往上传
pub fn parse_ts(s: &V) -> R<Option<f64>> {
    let s = match s {
        V::Bool(_) => return Ok(None),
        V::Int(_) | V::Big(_) | V::Float(_) => {
            let n = s.as_num().unwrap_or(f64::INFINITY);
            return Ok(Some(if n > 1e11 { n / 1000.0 } else { n }));
        }
        V::Str(s) => s,
        _ => return Ok(None),
    };
    let Some(m) = re!(r"^(\d{4})-(\d\d)-(\d\d)[T ](\d\d):(\d\d)(?::(\d\d)(\.\d+)?)?\s*(Z|[+-]\d\d:?\d\d)?$").match_(text::strip(s)) else {
        return Ok(None);
    };
    let int = |i: usize| text::to_int(m.g(i)).unwrap_or(0);
    let (y, mo) = (int(1), int(2));
    if !(1..=9999).contains(&y) {
        return Err(Fail::Value(format!("year must be in 1..9999, not {y}")));
    }
    if !(1..=12).contains(&mo) {
        return Err(Fail::Value(format!("month must be in 1..12, not {mo}")));
    }
    let frac = m.group(7).and_then(text::to_float).unwrap_or(0.0);
    let mut ts = timegm(y, mo, int(3), int(4), int(5), m.group(6).map_or(0, |_| int(6))) as f64 + frac;
    if let Some(tz) = m.group(8).filter(|tz| *tz != "Z") {
        let z = tz.replace(':', "");
        let sign = if z.starts_with('+') { 1.0 } else { -1.0 };
        let digits: Vec<i64> = z.chars().skip(1).map(|c| crate::py::uni::decimal_value(c).unwrap_or(0) as i64).collect();
        ts -= sign * ((digits[0] * 10 + digits[1]) * 3600 + (digits[2] * 10 + digits[3]) * 60) as f64;
    }
    Ok(Some(ts))
}

/// `time.localtime(t)` / `time.gmtime(t)` 连同它们出错的几种情况（原来都是没人接的异常）：NaN 是 ValueError，放不进
/// time_t 是 OverflowError，年份放不进 C 的 int（C 库返回空指针）是 OSError
#[allow(deprecated)] // musl 上 libc::time_t 标着“以后要改成 64 位”：只是提醒
fn tm_checked(t: f64, local: bool) -> R<Tm> {
    if t.is_nan() {
        return Err(Fail::Value("Invalid value NaN (not a number)".into()));
    }
    let secs = t.floor();
    if !(-9.223372036854776e18..9.223372036854776e18).contains(&secs) {
        return Err(overflow_error("timestamp out of range for platform time_t"));
    }
    let tm = if local { time::localtime(t) } else { time::gmtime(t) }; // 换算经 py::time（时区的事它管）；下面只是再问一遍看 C 库认不认
    let secs = secs as libc::time_t;
    // SAFETY: 两个指针都指向这里的局部变量
    let known = unsafe {
        let mut probe: libc::tm = std::mem::zeroed();
        !(if local { libc::localtime_r(&secs, &mut probe) } else { libc::gmtime_r(&secs, &mut probe) }).is_null()
    };
    if !known {
        return Err(Fail::Os(std::io::Error::from_raw_os_error(libc::EOVERFLOW)));
    }
    Ok(tm)
}

pub fn iso(ts: f64) -> R<String> {
    tm_checked(ts, false)?;
    Ok(time::strftime_utc("%Y-%m-%dT%H:%M:%SZ", ts))
}

/// 一个用量窗口：pct 是百分比（JSON 里的数，原样留着），at 是重置时间（秒），reset 表示重置时间已经过了（缓存里的数不算数了）
pub struct Win {
    pub pct: V,
    pub at: Option<f64>,
    pub reset: bool,
}

/// 一个用量窗口 → Win；不是窗口（不是字典、没有百分比）是 None
pub fn win(w: &V, now: f64) -> R<Option<Win>> {
    let Some(w) = w.as_dict() else { return Ok(None) };
    let Some(pct) = w.get("utilization").filter(is_num) else { return Ok(None) };
    let at = parse_ts(&w.get("resets_at").unwrap_or(V::None))?;
    Ok(Some(Win { pct, at, reset: at.is_some_and(|at| at <= now) }))
}

/// 一段时长：不到 1 分钟 / 25 分钟 / 2 小时 13 分 / 3 天 4 小时
pub fn span(secs: f64) -> R<String> {
    let m = match minutes(secs)? {
        Minutes::Small(m) => m,
        Minutes::Huge(m) => {
            let (h, _) = div_small(&m, 60);
            let (d, h) = div_small(&h, 24);
            return Ok(if h != 0 { format!("{d} 天 {h} 小时") } else { format!("{d} 天") });
        }
    };
    if m < 1 {
        return Ok("不到 1 分钟".into());
    }
    if m < 60 {
        return Ok(format!("{m} 分钟"));
    }
    let (h, m) = (m / 60, m % 60);
    if h < 24 {
        return Ok(if m != 0 { format!("{h} 小时 {m} 分") } else { format!("{h} 小时") });
    }
    let (d, h) = (h / 24, h % 24);
    Ok(if h != 0 { format!("{d} 天 {h} 小时") } else { format!("{d} 天") })
}

/// “周二 09:00”（lt 是 localtime 的结果）
pub fn weekday_clock(lt: &Tm) -> String {
    format!("周{} {:02}:{:02}", WEEKDAYS[lt.wday as usize], lt.hour, lt.min)
}

/// 紧凑的时长（cca usage 的表格里，不加空格）：不到1分钟 / 25分钟 / 2小时13分 / 3天4小时
pub fn span_short(secs: f64) -> R<String> {
    Ok(span(secs)?.replace(' ', ""))
}

/// b 那天比 a 那天晚几天（本地时间的日历日；跨夏令时也对）。年份不在 1–9999 里原来是没人接的 ValueError（date 装不下）
pub fn day_gap(a: f64, b: f64) -> R<i64> {
    let day = |ts: f64| -> R<i64> {
        let tm = tm_checked(ts, true)?;
        if !(1..=9999).contains(&tm.year) {
            return Err(Fail::Value(format!("year {} is out of range", tm.year)));
        }
        Ok(timegm(tm.year as i64, tm.mon as i64, tm.mday as i64, 0, 0, 0) / 86400)
    };
    let (later, earlier) = (day(b)?, day(a)?);
    Ok(later - earlier)
}

/// 表格里的重置时间（本地时间）：5 小时的窗口写还有多久（2小时13分后）；本周的一小时以内也写还有多久，
/// 再远写今天 / 明天 / 星期几和几点（周四 09:00）
pub fn reset_short(at: f64, now: f64, week: bool) -> R<String> {
    if !week || at - now < 3600.0 {
        return Ok(format!("{}后", span_short(at - now)?));
    }
    let lt = tm_checked(at, true)?;
    let day = match day_gap(now, at)? {
        0 => "今天".to_string(),
        1 => "明天".to_string(),
        _ => format!("周{}", WEEKDAYS[lt.wday as usize]),
    };
    Ok(format!("{} {:02}:{:02}", day, lt.hour, lt.min))
}

pub fn ago(secs: f64) -> R<String> {
    let m = match minutes(secs)? {
        Minutes::Small(m) => m,
        Minutes::Huge(m) => return Ok(format!("{} 天", div_small(&m, 1440).0)),
    };
    Ok(if m < 60 {
        format!("{} 分钟", m.max(1))
    } else if m < 1440 {
        format!("{} 小时", m / 60)
    } else {
        format!("{} 天", m / 1440)
    })
}

// --- 套餐的叫法和档位 -----------------------------------------------------------------------------------------

/// Claude Code 收 profile 接口给的套餐名（planDisplayName）的规矩：字母数字开头，中间可以有空格和 .+-，最长 40 个字
fn plan_name_ok(n: &str) -> bool {
    re!(r"^[^\W_](?:(?:[^\W_]|[ .+-]){0,38}(?:[^\W_]|\+))?$").matches(n)
}

/// Claude 套餐种类（organizationType 去掉 claude_）的叫法，和 Claude Code 自己显示的一样（它写成 Claude Pro）
fn claude_kind_name(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "pro" => "Pro",
        "max" => "Max",
        "team" => "Team",
        "enterprise" => "Enterprise",
        "free" => "Free",
        _ => return None,
    })
}

/// 套餐，叫法和官网一样：Free、Pro、Max、Ultra、Team、Team Premium、Enterprise。
/// 先用 oauthAccount 里的 planDisplayName：profile 接口给的官方名字，新版 Claude Code 才存，它自己也显示这个，官方再改名
/// 也跟得上。没有的话按 organizationType 和额度档位推：原来的 Max 20x 现在叫 Ultra，Max 5x 就叫 Max；Team 的 5x 档位是
/// Premium 席位（Claude Code 的 isTeamPremiumSubscriber）。都没有时看查询结果里的 subscription_type（分不出档位）
pub fn plan_label(oa: &Dict, sub: &V) -> String {
    if let Some(n) = oa.str("planDisplayName") {
        if plan_name_ok(&n) {
            return n.to_string();
        }
    }
    let (kind, x) = claude_plan_parts(oa, sub);
    if kind == "max" && x == 20 {
        return "Ultra".into();
    }
    if kind == "team" && x == 5 {
        return "Team Premium".into();
    }
    let name = claude_kind_name(&kind).map_or_else(|| capitalize(&kind), str::to_string);
    if name.is_empty() {
        "—".into()
    } else {
        name
    }
}

/// (种类, 额度倍数)：种类是 organizationType 去掉 claude_（没有时用查询结果里的 subscription_type），倍数是
/// organizationRateLimitTier / userRateLimitTier 里的 Nx（没有是 0）。plan_label 和 claude_plan_tier 共用，免得各认各的
pub fn claude_plan_parts(oa: &Dict, sub: &V) -> (String, i64) {
    let org = str_or_empty(&oa.get("organizationType")).to_lowercase();
    let kind = match org.strip_prefix("claude_") {
        Some(kind) => kind.to_string(),
        None => str_or_empty(&Some(sub.clone())).to_lowercase(),
    };
    let tiers = format!("{} {}", str_or_empty(&oa.get("organizationRateLimitTier")), str_or_empty(&oa.get("userRateLimitTier")));
    // 位数多到放不下的倍数（Python 里照样是个整数）：当作很大
    let x = re!(r"(\d+)x\b").search(&tiers).map_or(0, |m| text::to_int(m.g(1)).unwrap_or(i64::MAX));
    (kind, x)
}

/// Claude 套餐档位的名字表（planDisplayName 小写、合并空格、去掉开头的 claude 之后查）：只在种类和倍数不够判断时兜底
fn claude_tier_name(name: &str) -> &'static str {
    match name {
        "free" => "entry",
        "pro" | "team" | "enterprise" => "standard",
        "max" | "max 5x" | "team premium" => "advanced",
        "ultra" | "max 20x" => "premium",
        _ => "",
    }
}

/// 套餐档位（_gui state 的 plan_tier）：entry / standard / advanced / premium，认不出是 ''（不猜）。先看种类和额度倍数
/// （官方改叫法不受影响，Claude Code 自己认 Max 20x、Team Premium 也靠它们），倍数缺失时才看 planDisplayName；
/// 两边冲突时种类和倍数赢。倍数用 >=：以后出了更高的倍数也不会掉档
pub fn claude_plan_tier(oa: &Dict, sub: &V) -> &'static str {
    let (kind, x) = claude_plan_parts(oa, sub);
    let shown = text::split_ws(&str_or_empty(&oa.get("planDisplayName")).to_lowercase()).join(" ");
    let named = claude_tier_name(shown.strip_prefix("claude ").unwrap_or(&shown));
    if kind == "free" {
        return "entry";
    }
    if kind == "pro" || kind == "enterprise" || kind.starts_with("edu") {
        return "standard";
    }
    if kind == "max" {
        return if if x != 0 { x >= 20 } else { named == "premium" } { "premium" } else { "advanced" };
    }
    if kind == "team" {
        return if if x != 0 { x >= 5 } else { named == "advanced" } { "advanced" } else { "standard" };
    }
    named
}

// Claude Code 登录时把当时的套餐种类记进登录凭证（get_usage、claude auth status 回的 subscription_type 就是它），之后从官方读到的
// 新资料只更新 .claude.json 的 oauthAccount，凭证里这一项不改（2.1.322 核实：登录时记下的不拿资料覆盖，刷新 token 时资料齐全也不再读），
// 所以升降级以后它还按旧套餐显示（启动横幅、/status 写 Claude Pro）、按旧套餐处理，重新登录才更新。额度由服务器按实际的套餐算，不受影响。
// cca 查用量时把它记进 usage.json（login_plan），和账号资料对不上就提示重新登录
/// Claude Code 只记这几种，别的记成 null
const CLAUDE_LOGIN_KINDS: [&str; 4] = ["pro", "max", "team", "enterprise"];

/// Claude Code 登录时记下的套餐种类（recorded）和账号资料里的（organizationType）不一样：记下的那个的叫法（Pro 等）；
/// 一样、哪边没有、认不出都是 ''。额度倍数（Max 5x / 20x）不比：查用量时不回
pub fn claude_login_plan_outdated(oa: &Dict, recorded: &str) -> &'static str {
    let org = str_or_empty(&oa.get("organizationType")).to_lowercase();
    let kind = org.strip_prefix("claude_").unwrap_or("");
    let r = recorded.to_lowercase();
    if CLAUDE_LOGIN_KINDS.contains(&kind) && CLAUDE_LOGIN_KINDS.contains(&r.as_str()) && r != kind {
        claude_kind_name(&r).unwrap_or("")
    } else {
        ""
    }
}

/// Claude Code 记的套餐过时了：给人看的一句（cca usage 的表格下面、启动横幅里；App 自己按 outdated_login_plan 说）。
/// plan 是账号现在的套餐（表格里和套餐那一列一样）
pub fn login_plan_note(key: &str, old: &str, plan: &str) -> String {
    format!(
        "{} 里记的套餐还是 {}（登录时记下的，不会自己更新），账号现在是 {}：重新登录一次就会更新（{} login {}）",
        t().product,
        old,
        plan,
        t().cmd,
        name_of(key)
    )
}

// --- 用量缓存 -------------------------------------------------------------------------------------------------

pub fn usage_file(key: &str) -> String {
    path::join(&adir(key), "usage.json")
}

/// 上次 cca 查用量时 Claude Code 登录时记的套餐种类（usage_save 存的 login_plan）；登录的已经不是那个账号时不算，
/// 经 cca 重新登录前清掉（login-plan-forget）
pub fn usage_login_plan(key: &str) -> R<String> {
    let Some(V::Dict(j)) = load_quiet(&usage_file(key)) else { return Ok(String::new()) };
    let Some(p) = j.str("login_plan") else { return Ok(String::new()) };
    let uuid = t().who(key)?;
    let other = j.get("accountUuid").filter(|o| o.truthy());
    Ok(if uuid.truthy() && other.is_some_and(|o| !o.py_eq(&uuid)) { String::new() } else { p.to_string() })
}

/// 登录之前（claude_login）：usage.json 里记的 Claude Code 登录时的套餐作废，免得重新登录以后还提示；下一次查用量再记
pub fn cmd_login_plan_forget(args: &[String]) -> R<()> {
    arity("cmd_login_plan_forget", args, 1, 1)?;
    let p = usage_file(&args[0]);
    if let Some(V::Dict(j)) = load_quiet(&p) {
        if j.has("login_plan") {
            j.pop("login_plan");
            quiet_os(save_json(&p, &V::Dict(j), Some(0o600), true))?;
        }
    }
    Ok(())
}

/// `try: … except OSError: pass`
fn quiet_os(r: R<()>) -> R<()> {
    match r {
        Err(e) if e.is_os() => Ok(()),
        other => other,
    }
}

/// 最近一次的用量：at 是取到它的时间（秒），rl 是用量窗口
#[derive(Clone)]
pub struct Cached {
    pub at: f64,
    pub rl: Dict,
}

/// 最近一次的用量：cca 存的 usage.json 和 Claude Code 的 cachedUsageUtilization 取新的那份（一样新时取 usage.json）；
/// 登录的已经不是存它的那个账号时不算。
/// 新的那份没有分模型的每周额度（model_scoped）时照另一份补上（usage_scoped_fill）：Claude Code 的缓存是 /api/oauth/usage 的原样，
/// 没有这一项（get_usage 才从 limits 里挑出来），每次查用量时它都比 usage.json 早写几秒，这几秒里读到的用量少了 Fable 那几行
pub fn usage_cached(key: &str) -> R<Option<Cached>> {
    let uuid = t().who(key)?;
    usage_pick(&uuid, t().usage_caches(key)?)
}

/// usage_cached 挑的那一步：uuid 是现在登录的账号，caches 是各处找来的缓存（先 usage.json）
fn usage_pick(uuid: &V, caches: Vec<Option<V>>) -> R<Option<Cached>> {
    let mut found: Vec<Cached> = Vec::new();
    for x in caches {
        let Some(V::Dict(x)) = x else { continue };
        let Some(rl) = x.dict("utilization") else { continue };
        let Some(at) = x.get("fetchedAtMs").filter(is_num) else { continue };
        if uuid.truthy() && x.get("accountUuid").filter(|o| o.truthy()).is_some_and(|o| !o.py_eq(uuid)) {
            continue;
        }
        found.push(Cached { at: num(&at) / 1000.0, rl });
    }
    if found.is_empty() {
        return Ok(None);
    }
    sort_stable(&mut found, |a, b| -a.at < -b.at); // 稳定的：一样新时还是 usage.json 在前
    let mut best = found[0].clone();
    if !best.rl.has("model_scoped") {
        if let Some(prev) = found[1..].iter().find(|x| x.rl.list("model_scoped").is_some()) {
            best.rl = usage_scoped_fill(&best.rl, &prev.rl);
        }
    }
    Ok(Some(best))
}

/// rl 没有分模型的每周额度（model_scoped）时，照 prev 的 model_scoped 有哪几个模型补上：从 rl 的 limits 里取（kind 是 weekly_scoped、
/// scope.model 是这个模型的那一项：percent、resets_at，和 get_usage 给的一样），取不到（limits 被拿掉了、没有这一项）照搬 prev 的那一项，
/// 哪一行都不会少。prev 里没有的模型不加：get_usage 只给一部分模型（Claude Code 照自己的名单挑），cca 不猜
pub fn usage_scoped_fill(rl: &Dict, prev: &Dict) -> Dict {
    let got = Dict::new();
    for lim in dicts(rl, "limits") {
        if !lim.get("kind").is_some_and(|k| k.as_str() == Some("weekly_scoped")) {
            continue;
        }
        let Some(name) = lim.dict("scope").and_then(|s| s.dict("model")).and_then(|m| m.str("display_name")) else { continue };
        let k = name_key(&Some(V::Str(name.clone())));
        if !k.is_empty() && !got.has(&k) && lim.get("percent").is_some_and(|p| is_num(&p)) {
            got.set(k, dict! { "display_name" => name, "utilization" => lim.at("percent"), "resets_at" => lim.get("resets_at").unwrap_or(V::None) });
        }
    }
    let mut out: Vec<V> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for m in dicts(prev, "model_scoped") {
        let k = name_key(&m.get("display_name"));
        if !seen.contains(&k) {
            out.push(if k.is_empty() { V::Dict(m) } else { got.get(&k).unwrap_or(V::Dict(m)) });
            seen.push(k);
        }
    }
    let filled = rl.copy();
    filled.set("model_scoped", out);
    filled
}

/// 查到的用量存进 usage.json，本周的用量顺带记进每周用量（weeks_record；先记它：App 看到 usage.json 变了就重读 state）。
/// Claude 账号顺带记下查询回的 subscription_type（Claude Code 登录时记的套餐，login_plan）：
/// 放在顶层、不进用量窗口，用量可能取 Claude Code 自己那份更新的缓存，这一项只有 cca 查的时候才拿得到
pub fn usage_save(key: &str, rl: &Dict, sub: &V) -> R<()> {
    let ms = (time::time() * 1000.0) as i64;
    weeks_record(key, rl, ms as f64 / 1000.0)?;
    let j = dict! { "fetchedAtMs" => ms, "accountUuid" => t().who(key)?, "utilization" => rl };
    if t().kind == Kind::Claude && sub.as_str().is_some_and(|s| !s.is_empty()) {
        j.set("login_plan", sub);
    }
    quiet_os(save_json(&usage_file(key), &V::Dict(j), Some(0o600), true))
}

// 每周用量（App 账号页的「每周用量」、「用量统计」页；macos/PROTOCOL.md 2.2 的 usage_weeks）：Claude、OpenAI 都不给过去几周的额度，
// 所以 cca 每次查到用量（usage_save）时把本周的窗口记进账号目录的 usage-weeks.json（600）。按窗口的重置时间认是哪一周，记这一周
// 查到的最高百分比；同一周里比最高的低了一大截是中间重置过（重置额度、官方重置），之前的最高值累计下来。只给图形界面，终端不显示
pub const WEEK: i64 = 7 * 86400;
/// 最多记这么多周（一年多）
pub const WEEKS_KEEP: usize = 60;
/// App 看最近这么多周，统计也按它
pub const WEEKS_SHOW: i64 = 12;
/// 重置时间差不到 1 小时的算同一周（接口给的时间有零点几秒的出入）
pub const WEEK_SAME: f64 = 3600.0;
/// 同一周里比最高的低了超过这么多个百分点：中间重置过
pub const WEEK_DROP: f64 = 5.0;
/// 结束前一天里一次也没查到：那一周后来的用量没记下，实际可能更多
pub const WEEK_PARTIAL: f64 = 86400.0;

pub fn weeks_file(key: &str) -> String {
    path::join(&adir(key), "usage-weeks.json")
}

/// 有限的数字：NaN、无穷大不算（App 解码不了，算平均时 int() 会出错）
pub fn week_num(x: &V) -> bool {
    match x {
        V::Int(i) => i.unsigned_abs() < 1_000_000_000_000_000_000, // 整数照整数比：999999999999999999 换成小数就是 1e18 了
        V::Float(f) => f.abs() < 1e18,
        _ => false, // 放不进 i64 的整数都比 1e18 大
    }
}

/// 记下的一周：end 重置时间，peak 上次重置以来查到的最高百分比，carried 这一周里之前重置掉的（累计），resets 中间重置过几次，
/// first / last 第一次、最后一次查到的时间。数字照文件里写的留着（整数还是小数），写回去、给 App 时写法不变
#[derive(Clone)]
pub struct Week {
    pub end: V,
    pub peak: V,
    pub first: V,
    pub last: V,
    pub carried: V,
    pub resets: i64,
}

impl Week {
    fn to_dict(&self) -> Dict {
        dict! { "end" => &self.end, "peak" => &self.peak, "first" => &self.first, "last" => &self.last, "carried" => &self.carried, "resets" => self.resets }
    }
}

/// 记下的每周用量，按结束时间排好；格式不对的项不要
pub fn weeks_load(key: &str) -> Vec<Week> {
    weeks_parse(load_quiet(&weeks_file(key)))
}

/// weeks_load 读进来以后的那一步：j 是 usage-weeks.json 的内容
fn weeks_parse(j: Option<V>) -> Vec<Week> {
    let mut out = Vec::new();
    if let Some(V::Dict(j)) = j {
        for e in dicts(&j, "weeks") {
            let f = |k: &str| e.get(k).filter(week_num);
            if let (Some(end), Some(peak), Some(first), Some(last)) = (f("end"), f("peak"), f("first"), f("last")) {
                out.push(Week { end, peak, first, last, carried: f("carried").unwrap_or(V::Int(0)), resets: f("resets").map_or(0, |r| num(&r).trunc() as i64) });
            }
        }
    }
    sort_stable(&mut out, |a, b| n_lt(&a.end, &b.end));
    out
}

/// at 时查到的用量（rl）并进 weeks（原地改，按结束时间排好、最多 WEEKS_KEEP 周）：本周的窗口还没重置时记下，
/// 返回记了没有。不比已经记下的新（Claude Code 的缓存、同一次查询）就不改
pub fn weeks_add(weeks: &mut Vec<Week>, rl: &Dict, at: f64) -> R<bool> {
    let Some(w) = win(&rl.get("seven_day").unwrap_or(V::None), at)? else { return Ok(false) };
    let Some(end) = w.at.filter(|end| end.abs() < 1e18) else { return Ok(false) };
    if w.reset || !week_num(&w.pct) {
        return Ok(false);
    }
    let pct = num(&w.pct);
    let Some(i) = weeks.iter().position(|x| (num(&x.end) - end).abs() <= WEEK_SAME) else {
        weeks.push(Week { end: V::Float(end), peak: V::Float(pct), first: V::Float(at), last: V::Float(at), carried: V::Int(0), resets: 0 });
        sort_stable(weeks, |a, b| n_lt(&a.end, &b.end));
        if weeks.len() > WEEKS_KEEP {
            weeks.drain(..weeks.len() - WEEKS_KEEP);
        }
        return Ok(true);
    };
    let e = &mut weeks[i];
    if at <= num(&e.last) {
        return Ok(false);
    }
    if pct < num(&e.peak) - WEEK_DROP {
        e.carried = n_add(&e.carried, &e.peak);
        e.resets += 1;
        e.peak = V::Float(pct);
    } else if pct > num(&e.peak) {
        e.peak = V::Float(pct); // max(e['peak'], pct)：pct 大才换
    }
    e.end = V::Float(end);
    e.last = V::Float(at);
    Ok(true)
}

/// usage_save 里：查到的本周用量记进 usage-weeks.json
fn weeks_record(key: &str, rl: &Dict, at: f64) -> R<()> {
    weeks_record_at(&weeks_file(key), rl, at)
}

/// weeks_record 的做法；p 是这个账号的 usage-weeks.json（weeks_file(key)；测试里是临时目录里的）。没记下新东西就不写
fn weeks_record_at(p: &str, rl: &Dict, at: f64) -> R<()> {
    let mut weeks = weeks_parse(load_quiet(p));
    if weeks_add(&mut weeks, rl, at)? {
        let doc = dict! { "version" => 1, "weeks" => weeks.iter().map(|w| V::Dict(w.to_dict())).collect::<Vec<V>>() };
        quiet_os(save_json(p, &V::Dict(doc), Some(0o600), true))?;
    }
    Ok(())
}

/// _gui state 的 usage_weeks（macos/PROTOCOL.md 2.2）：最近 WEEKS_SHOW 周每周用到这一周额度的百分之几，和这几周的统计；
/// 没有记录是 None。cached 是 usage_cached 的结果（Claude Code 的缓存可能比 cca 记的新）：只在内存里并进来，不写文件（CCA_RO）。
/// 下一周在这一周结束前就开始了（中间重置后重新计时）：这一周到那时为止，不算还没过完
pub fn usage_weeks_view(key: &str, now: f64, cached: Option<&Cached>) -> R<Option<Dict>> {
    weeks_view(weeks_load(key), now, cached)
}

/// usage_weeks_view 读进来以后的那一步：weeks 是记下的每周用量
fn weeks_view(mut weeks: Vec<Week>, now: f64, cached: Option<&Cached>) -> R<Option<Dict>> {
    if let Some(c) = cached {
        weeks_add(&mut weeks, &c.rl, c.at)?;
    }
    if weeks.is_empty() {
        return Ok(None);
    }
    let week = V::Int(WEEK);
    let mut out: Vec<V> = Vec::new();
    let mut done: Vec<i64> = Vec::new(); // 过完的几周，和显示的一样四舍五入（round 是银行家舍入）
    for (i, e) in weeks.iter().enumerate() {
        let mut end = e.end.clone();
        if let Some(nxt) = weeks.get(i + 1) {
            if n_lt(&nxt.first, &n_sub(&end, &V::Int(WEEK_SAME as i64))) {
                // max(min(nxt['end'] - WEEK, end), e['last'])：min / max 都是后一个更小 / 更大才取它
                let early = n_sub(&nxt.end, &week);
                let cut = if n_lt(&end, &early) { end.clone() } else { early };
                end = if n_lt(&cut, &e.last) { e.last.clone() } else { cut };
            }
        }
        if num(&end) <= now - (WEEKS_SHOW * WEEK) as f64 {
            continue;
        }
        let current = num(&end) > now;
        let used = match n_add(&e.carried, &e.peak) {
            V::Float(f) => V::Float(round_to(f, 1)),
            whole => whole,
        };
        if !current {
            done.push(half_up(num(&used)));
        }
        out.push(V::Dict(dict! {
            "start_epoch" => n_sub(&e.end, &week), "end_epoch" => &end, "used_percent" => used, "resets" => e.resets,
            "last_epoch" => &e.last, "current" => current,
            "partial" => !current && num(&end) - num(&e.last) > WEEK_PARTIAL && num(&e.peak) < 100.0,
        }));
    }
    let mut since = &weeks[0].first;
    for e in &weeks[1..] {
        if n_lt(&e.first, since) {
            since = &e.first;
        }
    }
    let average = if done.is_empty() { V::None } else { V::Int(half_up(true_div(done.iter().map(|p| *p as i128).sum(), done.len() as u32))) };
    Ok(Some(dict! {
        "since_epoch" => since, "weeks" => out,
        "summary" => dict! {
            "weeks" => done.len(), "average_percent" => average,
            "maxed" => done.iter().filter(|p| **p >= 100).count(), "under_half" => done.iter().filter(|p| **p < 50).count(),
        },
    }))
}

/// 用量超过 10 分钟算旧：菜单里注明多久前，图形界面里变暗
pub const USAGE_STALE: f64 = 600.0;

/// 菜单里的一格（只读缓存，不联网；只给登录着的账号）：5h 13% · 周 41%；数据旧了注明多久前，
/// 重置时间过了的窗口写已重置
pub fn usage_cell(key: &str, now: f64) -> R<String> {
    cell_text(usage_cached(key)?.as_ref(), now)
}

/// usage_cell 拿到缓存以后的那一步：u 是 usage_cached 的结果
fn cell_text(u: Option<&Cached>, now: f64) -> R<String> {
    let Some(u) = u else { return Ok(String::new()) };
    let mut cells = Vec::new();
    for (label, k) in [("5h", "five_hour"), ("周", "seven_day")] {
        if let Some(w) = win(&u.rl.get(k).unwrap_or(V::None), now)? {
            cells.push(format!("{} {}", label, if w.reset { "已重置".to_string() } else { format!("{}%", py_round(&w.pct)?.text) }));
        }
    }
    if cells.is_empty() {
        return Ok(String::new());
    }
    let old = if now - u.at >= USAGE_STALE { format!("（{}前）", ago(now - u.at)?) } else { String::new() };
    Ok(format!("{}{}", cells.join(" · "), old))
}

// --- 用量表格、JSON -------------------------------------------------------------------------------------------

/// 后台查询的结果：{ok: true, usage} 或 {ok: false, why, msg}（bash 跳过时把原因写在 i.skip）
pub fn usage_result(tmp: &str, i: usize) -> R<Dict> {
    match read_text(&path::join(tmp, &format!("{i}.skip"))) {
        Ok(said) => {
            let (why, _, msg) = text::partition(said.trim_end_matches('\n'), "\t");
            return Ok(dict! { "ok" => false, "why" => why, "msg" => msg });
        }
        Err(e) if e.is_os() => {}
        Err(e) => return Err(e), // 不是 UTF-8：原来是没人接的 UnicodeDecodeError
    }
    if let Ok(said) = read_text(&path::join(tmp, &format!("{i}.json"))) {
        if let Some(last) = text::splitlines(text::strip(&said)).last() {
            if let Ok(V::Dict(r)) = json::loads(last) {
                return Ok(r);
            }
        }
    }
    Ok(dict! { "ok" => false, "why" => "error", "msg" => "没有拿到结果" })
}

/// 查不了的原因换成说明时查哪张表：查用量的；重置额度的（RESET_WHY）；已经请求重置之后出的错、不知道用掉了没有的（RESET_UNSURE）
#[derive(Clone, Copy)]
pub enum WhyTable {
    Usage,
    Reset,
    Unsure,
}

impl WhyTable {
    fn text(self, why: &str) -> Option<&'static str> {
        match (self, why) {
            (WhyTable::Reset | WhyTable::Unsure, "unsupported") => Some("这个版本的 {product} 不能重置额度（{update} 升级后再试）"),
            (WhyTable::Unsure, "timeout") => Some("没等到重置的结果，可能已经用掉了一次：刷新用量看看还剩几次"),
            (WhyTable::Unsure, "error") => Some("重置出错了（{msg}），可能已经用掉了一次：刷新用量看看还剩几次"),
            _ => USAGE_WHY.iter().find(|(k, _)| *k == why).map(|(_, said)| *said),
        }
    }
}

/// 查不了的原因（r.why，USAGE_WHY 的键）→ 给人看的说明；重置额度另有自己的说法
pub fn usage_why(r: &Row, table: WhyTable) -> R<String> {
    hashable(&r.why)?;
    let template = r.why.as_str().and_then(|why| table.text(why)).or_else(|| table.text("error")).unwrap_or("");
    let msg = if r.msg.truthy() { r.msg.py_str() } else { "原因不明".to_string() };
    let fill = [("name", r.name.as_str()), ("msg", msg.as_str()), ("cmd", t().cmd), ("host", t().trace_host), ("product", t().product), ("update", t().update_cmd)];
    // `模板.format(名字=值, …)`：一遍扫过去换，填进去的字不再当模板看
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map_or(rest.len(), |i| open + i);
        let name = &rest[open + 1..close];
        out.push_str(fill.iter().find(|(k, _)| *k == name).map_or("", |(_, v)| v));
        rest = rest.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    Ok(out)
}

/// rl[k] 这个列表里的字典（不是列表、不是字典的项都不算）
pub fn dicts(rl: &Dict, k: &str) -> Vec<Dict> {
    rl.list(k).map(|l| l.items().iter().filter_map(|x| x.as_dict()).collect()).unwrap_or_default()
}

/// 按名字认分模型的额度：去掉两头的空白，不分大小写
pub fn name_key(s: &Option<V>) -> String {
    text::strip(&str_or_empty(s)).to_lowercase()
}

/// 还能重置几次：`%d` 的写法（多大都写得出来），是不是 0，是不是多于 1
pub struct Count {
    pub text: String,
    pub zero: bool,
    pub many: bool,
}

/// Codex 还能重置几次额度（codex 给的 availableCount）；旧版本的 codex 不给，是 None
pub fn reset_count(rl: &Dict) -> R<Option<Count>> {
    let Some(n) = rl.dict("reset_credits").and_then(|rc| rc.get("availableCount")).filter(is_num) else { return Ok(None) };
    let small = |i: i64| Count { text: i.max(0).to_string(), zero: i <= 0, many: i > 1 };
    // max(int(n), 0)
    Ok(Some(match &n {
        V::Int(i) => small(*i),
        V::Big(s) if s.starts_with('-') => small(0),
        V::Big(s) => Count { text: s.to_string(), zero: false, many: true },
        _ => {
            let f = num(&n);
            if f.is_nan() {
                return Err(Fail::Value("cannot convert float NaN to integer".into()));
            }
            if f.is_infinite() {
                return Err(overflow_error("cannot convert float infinity to integer"));
            }
            let whole = f.trunc();
            if whole.abs() < 9.0e18 {
                small(whole as i64)
            } else if whole < 0.0 {
                small(0)
            } else {
                Count { text: format!("{whole:.0}"), zero: false, many: true }
            }
        }
    }))
}

/// 表格的备注：Codex 还能重置几次额度，分模型的每周额度、额外用量，Codex 的 credits
pub fn usage_notes(rl: &Dict, now: f64) -> R<Vec<String>> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    // Codex：0 次也写；codex 没给（旧版本）就不写
    if let Some(n) = reset_count(rl)? {
        let e = if n.zero { None } else { reset_credits_view(rl.get("reset_credits"))?.and_then(|v| v.get("next_expires_epoch")) };
        let lt = match e.filter(|e| is_num(e) && num(e) > now) {
            Some(e) => Some(tm_checked(num(&e), true)?),
            None => None,
        };
        let expires = lt.map_or_else(String::new, |lt| format!("（{}{} 月 {} 日过期）", if n.many { "最早 " } else { "" }, lt.mon, lt.mday));
        out.push(format!("剩余重置 {} 次{}", n.text, expires));
    }
    let mut pairs: Vec<(V, V)> = vec![(V::from("Opus"), rl.get("seven_day_opus").unwrap_or(V::None)), (V::from("Sonnet"), rl.get("seven_day_sonnet").unwrap_or(V::None))];
    pairs.extend(dicts(rl, "model_scoped").into_iter().map(|m| (m.get("display_name").unwrap_or(V::None), V::Dict(m))));
    // 没用过的（0%）不写，免得一排 0%；JSON 里照样都有
    for (label, w) in pairs {
        let Some(x) = win(&w, now)? else { continue };
        let k = name_key(&Some(label.clone()));
        if label.truthy() && !x.reset && num(&x.pct) > 0.0 && !seen.contains(&k) {
            seen.push(k);
            out.push(format!("{} 本周 {}%", label.py_str(), py_round(&x.pct)?.text));
        }
    }
    if let Some(e) = rl.dict("extra_usage").filter(|e| e.truthy("is_enabled")) {
        out.push(match e.get("utilization").filter(is_num) {
            Some(p) => format!("额外用量 {}%", py_round(&p)?.text),
            None => "额外用量已开启".to_string(),
        });
    }
    // Codex 的 credits
    if let Some(cr) = rl.dict("credits") {
        if cr.truthy("unlimited") {
            out.push("credits 不限".to_string());
        } else if cr.truthy("hasCredits") && cr.truthy("balance") {
            out.push(format!("credits 余额 {}", cr.at("balance").py_str()));
        }
    }
    Ok(out)
}

/// cmd_usage_render / _gui state 里一个账号的用量行（usage_entry 读它；原来 usage_row 造的字典）
#[derive(Clone)]
pub struct Row {
    pub key: String,
    pub name: String,
    pub alias: String,
    pub email: String,
    pub plan: String,
    /// live | cache | none
    pub source: &'static str,
    /// 取到数据的时间
    pub at: Option<f64>,
    /// 用量窗口
    pub rl: Dict,
    /// 失败原因（USAGE_WHY 的键），没有是 None。后台带回来的 JSON 里写的是什么就是什么
    pub why: V,
    /// 补充说明
    pub msg: V,
    /// Claude Code 记的过时的套餐（T.outdated_login_plan）
    pub outdated_login_plan: String,
}

/// 一个账号的用量行，别的项先是空的（source 是 none）：调用方再填
pub fn usage_row(key: &str, plan: impl Into<String>) -> R<Row> {
    let (alias, e) = parse_key(key);
    let email = if e.is_empty() { t().email(key)? } else { e };
    Ok(Row {
        key: key.to_string(),
        name: name_of(key),
        alias,
        email,
        plan: plan.into(),
        source: "none",
        at: None,
        rl: Dict::new(),
        why: V::None,
        msg: V::from(""),
        outdated_login_plan: String::new(),
    })
}

/// `limits[int(i)] if is_num(i) and int(i) == i and 0 <= i < len(limits) else None` 里的下标
fn limit_index(i: &V, len: usize) -> R<Option<usize>> {
    Ok(match i {
        V::Int(n) => usize::try_from(*n).ok().filter(|n| *n < len),
        V::Float(f) if f.is_nan() => return Err(Fail::Value("cannot convert float NaN to integer".into())),
        V::Float(f) if f.is_infinite() => return Err(overflow_error("cannot convert float infinity to integer")),
        V::Float(f) => (f.trunc() == *f && 0.0 <= *f && *f < len as f64).then_some(*f as usize),
        _ => None,
    })
}

/// Claude 分模型的每周额度（model_scoped，例如 Fable）各占本周总额度多少：{name_key(模型名): {used_percent_of_weekly（已用掉
/// 本周额度的百分之几）, allowance_percent_of_weekly（最多能用本周额度的百分之几）}}。weekly_scoped_shares 的 limit_index
/// 指 limits 里的那一项（kind 是 weekly_scoped，scope.model 是哪个模型）；没有 limits（Claude Code 存的缓存里没有）而
/// 分模型的额度、份额都只有一项时就是它俩，别的情况认不出就不写
pub fn weekly_shares(rl: &Dict) -> R<Dict> {
    let (shares, ms) = (dicts(rl, "weekly_scoped_shares"), dicts(rl, "model_scoped"));
    let limits = rl.list("limits").map(|l| l.items()).unwrap_or_default();
    let out = Dict::new();
    for s in &shares {
        let mut name = None;
        if !limits.is_empty() {
            let lim = match s.get("limit_index") {
                Some(i) => limit_index(&i, limits.len())?.map(|i| limits[i].clone()),
                None => None,
            };
            name = lim.and_then(|l| l.as_dict()).and_then(|l| l.dict("scope")).and_then(|s| s.dict("model")).and_then(|m| m.get("display_name"));
        } else if shares.len() == 1 && ms.len() == 1 {
            name = ms[0].get("display_name");
        }
        if let Some(V::Str(name)) = name {
            let k = name_key(&Some(V::Str(name)));
            if !k.is_empty() {
                let fields = Dict::new();
                for f in ["used_percent_of_weekly", "allowance_percent_of_weekly"] {
                    if let Some(v) = s.get(f).filter(is_num) {
                        fields.set(f, v);
                    }
                }
                out.set(k, fields);
            }
        }
    }
    Ok(out)
}

/// Codex 还能重置几次额度（codex 原样给的 rateLimitResetCredits）。cca 另加（macos/PROTOCOL.md 3.1）：每一次的明细上
/// Unix 秒的 expires_epoch（codex 给的 expiresAt 原样留着，不过期为 null），还能用的里面最早过期的那次的 next_expires_epoch
pub fn reset_credits_view(rc: Option<V>) -> R<Option<Dict>> {
    let Some(V::Dict(mut rc)) = rc else { return Ok(None) };
    if let Some(credits) = rc.list("credits") {
        let mut seen = Vec::new();
        for c in credits.items() {
            seen.push(match c {
                V::Dict(c) => {
                    let c2 = c.copy();
                    c2.set("expires_epoch", parse_ts(&c.get("expiresAt").unwrap_or(V::None))?);
                    V::Dict(c2)
                }
                other => other,
            });
        }
        rc = rc.copy();
        rc.set("credits", seen);
    }
    let opens = reset_credits_open(&rc);
    let out = rc.copy();
    out.set("next_expires_epoch", opens.first().map_or(V::None, |c| c.at("expires_epoch")));
    Ok(Some(out))
}

/// 还能用的重置机会（reset_credits_view 看过的），最早过期的排前面（不过期的排最后；和 codex 里 /usage 的 Redeem reset 一样）
pub fn reset_credits_open(rc: &Dict) -> Vec<Dict> {
    let mut cs: Vec<Dict> = dicts(rc, "credits")
        .into_iter()
        .filter(|c| c.get("status").is_some_and(|s| s.as_str() == Some("available")) && c.str("id").is_some_and(|id| !id.is_empty()))
        .collect();
    let expires = |c: &Dict| c.get("expires_epoch").filter(is_num).map_or(f64::INFINITY, |e| num(&e));
    sort_stable(&mut cs, |a, b| expires(a) < expires(b));
    cs
}

/// 本周额度 24 小时内重置、还剩至少 20%：App 里建议先用这个账号
const USE_FIRST_HOURS: f64 = 24.0;
const USE_FIRST_LEFT: i64 = 20;
/// 这次查询说明账号现在用不了
const USE_FIRST_BAD: [&str; 8] = ["login", "mismatch", "route", "port", "trace", "region", "nolimits", "auth"];

/// 本周额度快重置了、还剩不少（macos/PROTOCOL.md 3.1）：{left_percent, from_epoch, until_epoch}，App 拿当前时间和
/// [from_epoch, until_epoch) 比（快照可能放了好几分钟）；不该提示时是 None。剩余按四舍五入后的百分比算（和显示的一致；
/// Python 的 round 是银行家舍入，所以不用它）。登录、线路、出口、5 小时的额度由 App 按各自的状态挡
pub fn use_first(rl: &Dict, now: f64, why: &V) -> R<Option<Dict>> {
    let w = win(&rl.get("seven_day").unwrap_or(V::None), now)?;
    if why.as_str().is_some_and(|why| USE_FIRST_BAD.contains(&why)) {
        return Ok(None);
    }
    let Some(w) = w else { return Ok(None) };
    let Some(at) = w.at.filter(|_| !w.reset) else { return Ok(None) };
    // int(pct + 0.5)，再夹在 0 到 100 之间
    let shown = num(&w.pct) + 0.5;
    if shown.is_nan() {
        return Err(Fail::Value("cannot convert float NaN to integer".into()));
    }
    if shown.is_infinite() {
        return Err(overflow_error("cannot convert float infinity to integer"));
    }
    let left = 100 - shown.trunc().clamp(0.0, 100.0) as i64;
    if left < USE_FIRST_LEFT {
        return Ok(None);
    }
    Ok(Some(dict! { "left_percent" => left, "from_epoch" => at - USE_FIRST_HOURS * 3600.0, "until_epoch" => at }))
}

/// cca usage --json 里一个账号的条目（_gui state 里的 usage 也是它；格式见 macos/PROTOCOL.md，字段只加不删）。
/// r 是 usage_row 造的一行
pub fn usage_entry(r: &Row, now: f64) -> R<Dict> {
    let w = |x: Option<V>| -> R<V> {
        let x = x.unwrap_or(V::None);
        Ok(match win(&x, now)? {
            Some(v) => V::Dict(dict! {
                "utilization" => &v.pct, "resets_at" => x.as_dict().and_then(|d| d.get("resets_at")).unwrap_or(V::None),
                "resets_epoch" => v.at, "reset" => v.reset,
            }),
            None => V::None,
        })
    };
    let rl = &r.rl;
    let at = r.at.filter(|at| *at != 0.0); // `if at`：0 和没有一样
    let failed = r.why.truthy();
    let a = Dict::new();
    a.set("account", &r.name);
    a.set("key", &r.key);
    a.set("email", if r.email.is_empty() { V::None } else { V::from(&r.email) });
    a.set("plan", &r.plan);
    a.set("source", r.source);
    a.set("fetched_at", match at {
        Some(at) => V::from(iso(at)?),
        None => V::None,
    });
    a.set("fetched_epoch", at);
    a.set("age_seconds", match at {
        Some(at) => V::Int(py_int(now - at)?),
        None => V::None,
    });
    a.set("stale", at.is_some_and(|at| now - at >= USAGE_STALE));
    a.set("error", if failed { V::from(usage_why(r, WhyTable::Usage)?) } else { V::None });
    a.set("error_code", if failed { r.why.clone() } else { V::None });
    a.set("error_detail", if failed && r.msg.truthy() { r.msg.clone() } else { V::None });
    // 窗口原样给：没有 5 小时限制的套餐接口不给 five_hour，就是 null。别拿 limits 里的 is_active 判断（docs/dev/usage.md）
    for k in t().usage_windows {
        a.set(*k, w(rl.get(k))?);
    }
    let shares = weekly_shares(rl)?;
    let scoped = if rl.list("model_scoped").is_some() {
        let mut items = Vec::new();
        for m in dicts(rl, "model_scoped") {
            let name = m.get("display_name");
            let e = match w(Some(V::Dict(m)))? {
                V::Dict(e) => e,
                _ => Dict::new(),
            };
            e.set("name", name.clone().unwrap_or(V::None));
            if let Some(share) = shares.dict(&name_key(&name)) {
                e.update(&share);
            }
            items.push(V::Dict(e));
        }
        V::from(items)
    } else {
        V::None
    };
    a.set("model_scoped", scoped);
    a.set("extra_usage", rl.dict("extra_usage"));
    a.set("use_first", use_first(rl, now, &r.why)?);
    if t().kind == Kind::Claude {
        // Claude Code 登录时记的套餐和账号现在的对不上（重新登录才会更新）：记的那个的叫法；对得上、不知道是 null
        a.set("outdated_login_plan", if r.outdated_login_plan.is_empty() { V::None } else { V::from(&r.outdated_login_plan) });
    } else {
        // codex 原样给的：credits 余额；还能重置几次额度（availableCount）和每一次的明细
        a.set("credits", rl.dict("credits"));
        a.set("reset_credits", reset_credits_view(rl.get("reset_credits"))?);
    }
    Ok(a)
}

/// 查到的用量里至少有一个认得出的窗口：都没有的（接口的格式变了）不算查到，免得把好好的缓存换成空的
pub fn usage_has_windows(rl: &V) -> R<bool> {
    let Some(rl) = rl.as_dict() else { return Ok(false) };
    for k in t().usage_windows {
        if win(&rl.get(k).unwrap_or(V::None), 0.0)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 后台带回来的用量（usage_result 里查用量的格式）：至少认得出一个窗口的才算查到，返回它；没有、认不出返回 None
pub fn usage_take(r: &Dict) -> R<Option<Dict>> {
    let Some(u) = r.dict("usage").filter(|_| r.truthy("ok")) else { return Ok(None) };
    Ok(if u.truthy("rate_limits_available") && usage_has_windows(&u.get("rate_limits").unwrap_or(V::None))? { Some(u) } else { None })
}

/// 查到的那份用量里的窗口（usage_take 核对过它是字典）
fn rate_limits(u: &Dict) -> Dict {
    u.dict("rate_limits").unwrap_or_default()
}

pub fn usage_json(rows: &[Row], now: f64, api: &[String]) -> R<Dict> {
    let mut accounts = Vec::new();
    for r in rows {
        accounts.push(V::Dict(usage_entry(r, now)?));
    }
    let j = dict! {
        "version" => 1, "kind" => "usage", "generated_at" => iso(now)?, "generated_epoch" => now, "accounts" => accounts,
        "skipped_api_accounts" => crate::py::obj::str_list(api),
    };
    if t().kind != Kind::Claude {
        j.set("tool", t().name); // Claude 的输出和以前一样，不加这个字段
    }
    Ok(j)
}

/// cca usage 的表格里备注至少留这么宽（放得下一项原因或额度）；不够时先收起别的列
const USAGE_NOTE_FLOOR: i64 = 20;

/// 表格的备注（按 ； 分项）放进 room 格：放得下几项放几项，后面的写成 ；…；第一项就放不下时先去掉括号里的补充，
/// 再不行按字截
pub fn fit_note(parts: &[String], room: usize) -> String {
    for n in (1..=parts.len()).rev() {
        let s = format!("{}{}", parts[..n].join("；"), if n < parts.len() { "；…" } else { "" });
        if uni::width(&s) <= room {
            return s;
        }
    }
    let head = parts[0].split('（').next().unwrap_or("");
    if head != parts[0] && uni::width(head) < room {
        format!("{head}…")
    } else {
        fit(&parts[0], room, false)
    }
}

/// `max(int(cols) if str(cols).isdigit() else 80, 40)` 里的那个数
fn cols_of(cols: &str) -> R<i64> {
    if !text::isdigit(cols) {
        return Ok(80);
    }
    match text::to_int(cols) {
        Some(n) => Ok(n),
        // 位数多到放不下：当作很宽
        None if cols.chars().all(uni::is_decimal) => Ok(i64::MAX / 4),
        // isdigit 认、int() 不认的（² 这一类）：原来是没人接的 ValueError
        None => Err(Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(cols)))),
    }
}

/// 一个用量窗口在表格里的样子：(百分比, 重置时间, 颜色)
type Cell = (String, String, &'static str);

/// 表格里收起了哪几样
#[derive(Clone, Copy)]
struct Show {
    email: bool,
    plan: bool,
    r5: bool,
    r7: bool,
}

/// cca usage 的表格：一个账号一行，整行不超过终端的宽度。放不下时依次收起有别名的账号的邮箱、套餐那一列、
/// 两个重置时间；备注至少留 USAGE_NOTE_FLOOR 格，按项取舍（fit_note）；查不到的行从 5 小时那一列起写原因
pub fn usage_table(rows: &[Row], now: f64, cols: &str, color: bool, api: &[String]) -> R<()> {
    for line in usage_table_lines(rows, now, cols, color, api)? {
        out!("{line}");
    }
    Ok(())
}

/// usage_table 要打印的各行（表格是算完了才开始打印的，中途出错时一行也没有）
fn usage_table_lines(rows: &[Row], now: f64, cols: &str, color: bool, api: &[String]) -> R<Vec<String>> {
    let total = cols_of(cols)?.max(40) - 2; // 一行最多这么宽（留 2 格：… 和 — 在有的终端里占 2 格）
    let wide = total as usize;
    let paint = |s: &str, code: &str| -> String {
        if color && !code.is_empty() && !text::strip(s).is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    let cell = |w: Option<Win>, week: bool| -> R<Cell> {
        let Some(w) = w else { return Ok(("—".into(), String::new(), "")) };
        if w.reset {
            return Ok(("已重置".into(), String::new(), ""));
        }
        let p = py_round(&w.pct)?;
        let resets = match w.at.filter(|at| *at != 0.0) {
            Some(at) => reset_short(at, now, week)?,
            None => String::new(),
        };
        Ok((format!("{:>3}%", p.text), resets, if p.val >= 90.0 { "31" } else if p.val >= 70.0 { "33" } else { "" }))
    };
    struct Line<'a> {
        r: &'a Row,
        five: Option<Cell>,
        week: Option<Cell>,
        notes: Vec<String>,
    }
    let mut data: Vec<Line> = Vec::new();
    for r in rows {
        if r.source == "none" {
            data.push(Line { r, five: None, week: None, notes: vec![usage_why(r, WhyTable::Usage)?] });
            continue;
        }
        let mut notes = usage_notes(&r.rl, now)?;
        if r.source == "cache" {
            // 原因在前（窄的终端里先保住它），再说是多久以前的数
            let mut first = if r.why.truthy() { vec![usage_why(r, WhyTable::Usage)?] } else { Vec::new() };
            let at = r.at.unwrap_or_else(|| panic!("TypeError: 缓存的用量行没有取到数据的时间"));
            first.push(format!("缓存（{}前）", ago(now - at)?));
            first.append(&mut notes);
            notes = first;
        }
        let five = cell(win(&r.rl.get("five_hour").unwrap_or(V::None), now)?, false)?;
        let week = cell(win(&r.rl.get("seven_day").unwrap_or(V::None), now)?, true)?;
        data.push(Line { r, five: Some(five), week: Some(week), notes });
    }
    if rows.is_empty() {
        return Err(Fail::Value("max() iterable argument is empty".into()));
    }
    // 账号一格：别名、邮箱各占一小列，和菜单一样（都没有别名就只写邮箱）
    let aw = rows.iter().map(|r| uni::width(&r.alias)).max().unwrap_or(0).min(16);
    let ew = rows.iter().map(|r| uni::width(&r.email)).max().unwrap_or(0).min(32);
    let acct = |r: &Row, show: Show| -> String {
        if show.email {
            let mut parts = Vec::new();
            if aw > 0 {
                parts.push(fit(&r.alias, aw, true));
            }
            if ew > 0 {
                parts.push(fit(&r.email, ew, true));
            }
            return parts.join("  ");
        }
        fit(if r.alias.is_empty() { &r.email } else { &r.alias }, 32, false)
    };
    // 一个窗口那一列：(表头, 宽)。重置时间收起来以后（on 是 false）只写百分比
    let win_col = |five: bool, title: &str, on: bool| -> (String, i64) {
        let cells: Vec<&Cell> = data.iter().filter_map(|d| if five { d.five.as_ref() } else { d.week.as_ref() }).collect();
        let rw = if on { cells.iter().map(|c| uni::width(&c.1)).max().unwrap_or(0) } else { 0 };
        let head = if rw > 0 { format!("{title}/重置") } else { title.to_string() };
        let w = cells.iter().map(|c| uni::width(&c.0) + if rw > 0 && !c.1.is_empty() { 1 + rw } else { 0 }).chain([uni::width(&head)]).max().unwrap_or(0);
        (head, w as i64)
    };
    // 各列的宽：账号、套餐（收起来是 0）、5 小时、本周
    let layout = |show: Show| -> (i64, i64, i64, i64) {
        let a = rows.iter().map(|r| uni::width(&acct(r, show))).chain([uni::width("账号")]).max().unwrap_or(0) as i64;
        let p = if show.plan { rows.iter().map(|r| uni::width(&r.plan)).chain([uni::width("套餐")]).max().unwrap_or(0) as i64 } else { 0 };
        (a, p, win_col(true, "5 小时", show.r5).1, win_col(false, "本周", show.r7).1)
    };
    // 这样排时一行剩下给备注的宽（可能是负数）
    let slack = |a: i64, p: i64, f: i64, k: i64| total - (2 + a + (if p != 0 { 2 + p } else { 0 }) + 2 + f + 2 + k);
    // 查到了的行的第一项备注
    let firsts: Vec<i64> = data.iter().filter(|d| d.five.is_some() && !d.notes.is_empty()).map(|d| uni::width(&d.notes[0]) as i64).collect();
    // 备注那一列（连同前面的两格）想要的宽
    let want = firsts.iter().max().map_or(0, |m| USAGE_NOTE_FLOOR.min(*m) + 2);
    let mut show = Show { email: true, plan: true, r5: true, r7: true };
    // 重置时间只在数据都放不下时才收
    for (k, need) in [("email", want), ("plan", want), ("r5", 0), ("r7", 0)] {
        let (a, p, f, kk) = layout(show);
        if slack(a, p, f, kk) < need && (k != "email" || rows.iter().any(|r| !r.alias.is_empty() && !r.email.is_empty())) {
            match k {
                "email" => show.email = false,
                "plan" => show.plan = false,
                "r5" => show.r5 = false,
                _ => show.r7 = false,
            }
        }
    }
    let (mut a, p, f, k) = layout(show);
    if slack(a, p, f, k) < 0 {
        a = (a + slack(a, p, f, k)).max(10); // 还是放不下：账号那一列截短（最少 10 格），再不行整行截断
    }
    let left = slack(a, p, f, k) - 2;
    let room = if left >= 8 { left as usize } else { 0 }; // 备注的宽；太窄就不写备注
    let (a, p, f, k) = (a as usize, p as usize, f as usize, k as usize);
    let mut head = vec![fit("账号", a, true)];
    if p != 0 {
        head.push(fit("套餐", p, true));
    }
    head.push(fit(&win_col(true, "5 小时", show.r5).0, f, true));
    head.push(fit(&win_col(false, "本周", show.r7).0, k, true));
    let noted = room != 0 && !firsts.is_empty();
    let mut lines = Vec::new();
    lines.push(paint(&fit(text::rstrip(&format!("  {}{}", head.join("  "), if noted { "  备注" } else { "" })), wide, false), "2"));
    let painted = |cells: &[(String, &str)]| cells.iter().map(|(s, code)| paint(s, code)).collect::<Vec<_>>().join("  ");
    let mut lost = false;
    for d in &data {
        let dim = if d.r.source == "live" { "" } else { "2" };
        let mut cells: Vec<(String, &str)> = vec![(fit(&acct(d.r, show), a, true), dim)];
        if p != 0 {
            cells.push((fit(&d.r.plan, p, true), dim));
        }
        // 查不到：从 5 小时那一列起写原因
        let (Some(five), Some(week)) = (&d.five, &d.week) else {
            let start = 2 + a + 2 + if p != 0 { p + 2 } else { 0 };
            lines.push(format!("  {}  {}", painted(&cells), paint(&fit(&d.notes[0], wide.saturating_sub(start).max(10), false), "2")));
            continue;
        };
        for ((pct, resets, code), width, on) in [(five, f, show.r5), (week, k, show.r7)] {
            let said = if on && !resets.is_empty() { format!("{pct} {resets}") } else { pct.clone() };
            cells.push((fit(&said, width, true), if code.is_empty() { dim } else { code }));
        }
        let line = format!("  {}", painted(&cells));
        let note = if !d.notes.is_empty() && room != 0 { fit_note(&d.notes, room) } else { String::new() };
        lost = lost || (!d.notes.is_empty() && room == 0);
        lines.push(if note.is_empty() { text::rstrip(&line).to_string() } else { format!("{}  {}", line, paint(&note, if dim.is_empty() { "" } else { "2" })) });
    }
    if lost {
        lines.push(paint(&fit(&format!("  （终端太窄，备注没显示：拉宽一点，或者用 {} usage --json 看全部）", t().cmd), wide, false), "2"));
    }
    // Claude Code 记的套餐过时了：表格下面单独一行（太长就换短一点的说法，整行不超过终端的宽度）
    for r in rows {
        let old = &r.outdated_login_plan;
        if !old.is_empty() {
            let fix = format!("{} login {}", t().cmd, r.name);
            let says = [
                format!("  ! {}：{}", r.name, login_plan_note(&r.key, old, &r.plan)),
                format!("  ! {}：{} 里记的套餐还是 {}，现在是 {}，重新登录：{}", r.name, t().product, old, r.plan, fix),
                format!("  ! 套餐过时（记的是 {}，现在是 {}）：{}", old, r.plan, fix),
            ];
            let said = says.iter().find(|s| uni::width(s) <= wide).unwrap_or(&says[2]);
            lines.push(paint(&fit(said, wide, false), "33"));
        }
    }
    if !api.is_empty() {
        let names: Vec<String> = api.iter().map(|k| name_of(k)).collect();
        lines.push(paint(&fit(&format!("  第三方 API 账号没有订阅用量，不查：{}", names.join("、")), wide, false), "2"));
    }
    Ok(lines)
}

/// 把后台查询的结果整理成表格或 JSON（stdout）；查到的存进 usage.json，查不到的用缓存顶上（注明多久前）
pub fn cmd_usage_render(args: &[String]) -> R<()> {
    arity("cmd_usage_render", args, 4, usize::MAX)?;
    let (tmp, mode, cols, color, keys) = (args[0].as_str(), args[1].as_str(), args[2].as_str(), args[3].as_str(), &args[4..]);
    let (_, table, _) = ctx()?;
    let now = time::time();
    let mut rows = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let mut r = usage_result(tmp, i)?;
        let u = usage_take(&r)?;
        if r.truthy("ok") && u.is_none() {
            r = dict! { "ok" => false, "why" => "nolimits", "msg" => "" };
        }
        let sub = u.as_ref().and_then(|u| u.get("subscription_type")).unwrap_or(V::None);
        let plan = t().plan(k, &sub)?;
        let mut row = usage_row(k, plan)?;
        row.why = if u.is_some() { V::None } else { r.get("why").filter(|w| w.truthy()).unwrap_or_else(|| V::from("error")) };
        row.msg = r.get("msg").filter(|m| m.truthy()).unwrap_or_else(|| V::from(""));
        let logged_in = !matches!(row.why.as_str(), Some("login" | "mismatch"));
        if let Some(u) = &u {
            row.source = "live";
            row.at = Some(now);
            row.rl = rate_limits(u);
            usage_save(k, &row.rl, &sub)?;
        } else if logged_in {
            if let Some(x) = usage_cached(k)? {
                row.source = "cache";
                row.at = Some(x.at);
                if x.rl.truthy("plan") {
                    row.plan = t().plan(k, &x.rl.at("plan"))?;
                }
                row.rl = x.rl;
            }
        }
        // 没登录、登录的不是这个账号时不说
        if logged_in {
            row.outdated_login_plan = t().outdated_login_plan(k)?;
        }
        rows.push(row);
    }
    // 查的正好是全部官方账号（set(keys) == official）时，顺带说一句第三方 API 账号不查
    let items = table.items();
    let official: Vec<&str> = items.iter().filter(|(_, v)| !is_api(v)).map(|(k, _)| &**k).collect();
    let all_official = keys.iter().all(|k| official.contains(&k.as_str())) && official.iter().all(|o| keys.iter().any(|k| k == o));
    let api: Vec<String> = if all_official { items.iter().filter(|(_, v)| is_api(v)).map(|(k, _)| k.to_string()).collect() } else { Vec::new() };
    if mode == "json" {
        out!("{}", json::dumps(&V::Dict(usage_json(&rows, now, &api)?), json::FILE));
    } else {
        usage_table(&rows, now, cols, color == "1", &api)?;
    }
    io::flush_out();
    keyshare_kick(false); // 开着设备统计时：记下备查的 API key 和登录了同一个账号的其他设备同步（后台，最多 KEYSHARE_EVERY 一次）
    if t().has_cost {
        cost_kick(); // 折合金额：在后台统计、和别的设备同步（最多 COST_EVERY 一次）
    }
    exit(if rows.iter().any(|r| r.source != "none") { 0 } else { 1 })
}

// --- 重置额度（cca codex reset）：查不了、重置不了的原因 → 说明，和图形界面的失败原因（fail 事件的 code，macos/PROTOCOL.md 4.5）---

/// 查不了、重置不了的原因 → fail 事件的 code
fn reset_fail_code(why: &str) -> &'static str {
    match why {
        "login" | "auth" => "not_logged_in",
        "mismatch" => "login_mismatch",
        "route" => "local_unavailable",
        "port" => "port_closed",
        "region" => "region_restricted",
        "trace" => "no_exit_ip",
        "unsupported" => "reset_unsupported",
        _ => "error",
    }
}

/// 重置的结果 → 说的那句话（%s 是账号的名字）
fn reset_said(outcome: &str) -> Option<&'static str> {
    Some(match outcome {
        "reset" => "已重置 %s 的额度",
        "already_redeemed" => "%s 的这次重置已经完成过了",
        "nothing_to_reset" => "%s 现在的用量还不需要重置，没有用掉重置机会",
        _ => return None,
    })
}

/// 后台没成（usage_result 的 {ok: false, why, msg}）→ (fail 事件的 code, 说明)
pub fn reset_fail(key: &str, r: &Dict, table: WhyTable) -> R<(String, String)> {
    let mut row = usage_row(key, "")?;
    row.why = r.get("why").filter(|w| w.truthy()).unwrap_or_else(|| V::from("error"));
    row.msg = r.get("msg").filter(|m| m.truthy()).unwrap_or_else(|| V::from(""));
    hashable(&row.why)?;
    Ok((row.why.as_str().map_or("error", reset_fail_code).to_string(), usage_why(&row, table)?))
}

/// 一次重置机会：名字（OpenAI 没给就叫完整重置）和什么时候过期（本地时间）
pub fn credit_label(c: &Dict) -> R<String> {
    let when = match c.get("expires_epoch").filter(is_num) {
        Some(e) => {
            let lt = tm_checked(num(&e), true)?;
            format!("{} 月 {} 日 {:02}:{:02} 过期", lt.mon, lt.mday, lt.hour, lt.min)
        }
        None => "不过期".to_string(),
    };
    let title = str_or_empty(&c.get("title"));
    let title = text::strip(&title);
    Ok(format!("{}，{}", if title.is_empty() { "完整重置" } else { title }, when))
}

/// cca codex reset 的第一步：后台经账号的出口查到的用量（tmp/0）存进 usage.json，给 bash：RC_CODE / RC_MSG（查不了时
/// fail 事件的 code 和说明，查到了是空的）；RC_COUNT 还能重置几次；RC_ID 要用掉的那一次（最早过期的；没有明细时是空的，
/// 由 OpenAI 挑）、RC_NEXT 它的说明；RC_LIST 每一次的说明（一行一个，明细可能比次数少）
pub fn cmd_reset_plan(args: &[String]) -> R<()> {
    arity("cmd_reset_plan", args, 2, 2)?;
    let (tmp, key) = (args[0].as_str(), args[1].as_str());
    let stop = |(code, msg): (String, String)| sh(&[("RC_CODE", code.as_str()), ("RC_MSG", msg.as_str()), ("RC_COUNT", ""), ("RC_ID", ""), ("RC_NEXT", ""), ("RC_LIST", "")]);
    let r = usage_result(tmp, 0)?;
    let Some(u) = usage_take(&r)? else {
        stop(reset_fail(key, &if r.truthy("ok") { dict! { "why" => "nolimits" } } else { r }, WhyTable::Reset)?);
        return Ok(());
    };
    let rl = rate_limits(&u);
    usage_save(key, &rl, &V::None)?;
    // 旧版本的 codex：回应里没有重置次数
    let Some(n) = reset_count(&rl)? else {
        stop(reset_fail(key, &dict! { "why" => "unsupported" }, WhyTable::Reset)?);
        return Ok(());
    };
    let opens = reset_credits_view(rl.get("reset_credits"))?.map(|view| reset_credits_open(&view)).unwrap_or_default();
    let mut labels = Vec::new();
    for c in &opens {
        labels.push(credit_label(c)?);
    }
    let id = opens.first().and_then(|c| c.str("id")).map_or_else(String::new, |id| id.to_string());
    sh(&[
        ("RC_CODE", ""),
        ("RC_MSG", ""),
        ("RC_COUNT", n.text.as_str()),
        ("RC_ID", id.as_str()),
        ("RC_NEXT", labels.first().map_or("", |l| l.as_str())),
        ("RC_LIST", labels.join("\n").as_str()),
    ]);
    Ok(())
}

/// cca codex reset 的最后一步：用掉一次重置机会的结果（tmp/1）→ 重置后的用量存进 usage.json，给 bash RC_OUTCOME、
/// RC_CODE（重置不了时 fail 事件的 code，否则是空的）、RC_MSG（说明）、RC_DOC（mode=json 时 kind=reset 的文档）
pub fn cmd_reset_done(args: &[String]) -> R<()> {
    arity("cmd_reset_done", args, 2, 4)?;
    let (tmp, key, mode, credit) = (args[0].as_str(), args[1].as_str(), arg(args, 2, ""), arg(args, 3, ""));
    let (now, name) = (time::time(), name_of(key));
    let r = usage_result(tmp, 1)?;
    let outcome = if r.truthy("ok") { r.get("outcome").unwrap_or(V::None) } else { V::None };
    if outcome.as_str() == Some("no_credit") {
        let msg = if credit.is_empty() { format!("{name} 没有能用的重置机会了") } else { format!("{name} 的这次重置机会已经不能用了（可能刚在别处用掉了），刷新用量看看还剩几次") };
        sh(&[("RC_OUTCOME", "no_credit"), ("RC_CODE", "no_reset_credit"), ("RC_DOC", ""), ("RC_MSG", msg.as_str())]);
        return Ok(());
    }
    hashable(&outcome)?;
    // 已经请求重置了，结果不知道（超时、出错）
    let Some((outcome, said)) = outcome.as_str().and_then(|o| Some((o, reset_said(o)?))) else {
        let r = if r.truthy("ok") { dict! { "why" => "error", "msg" => "看不懂 codex 回的结果" } } else { r };
        let (code, msg) = reset_fail(key, &r, WhyTable::Unsure)?;
        sh(&[("RC_OUTCOME", ""), ("RC_CODE", code.as_str()), ("RC_MSG", msg.as_str()), ("RC_DOC", "")]);
        return Ok(());
    };
    let u = usage_take(&r)?;
    if let Some(u) = &u {
        usage_save(key, &rate_limits(u), &V::None)?;
    }
    let mut msg = said.replace("%s", &name);
    let cell = if u.is_some() && outcome != "nothing_to_reset" { usage_cell(key, now)? } else { String::new() };
    if !cell.is_empty() {
        msg.push_str(&format!("（现在 {cell}）"));
    }
    let usage = match &u {
        Some(u) => {
            let mut row = usage_row(key, t().plan(key, &u.get("subscription_type").unwrap_or(V::None))?)?;
            row.source = "live";
            row.at = Some(now);
            row.rl = rate_limits(u);
            V::Dict(usage_entry(&row, now)?)
        }
        None => V::None,
    };
    let doc = dict! {
        "version" => 1, "kind" => "reset", "tool" => t().name, "generated_epoch" => now, "key" => key, "outcome" => outcome,
        "message" => &msg, "usage" => usage,
    };
    let doc = if mode == "json" { json::dumps(&V::Dict(doc), json::FILE) } else { String::new() };
    sh(&[("RC_OUTCOME", outcome), ("RC_CODE", ""), ("RC_MSG", msg.as_str()), ("RC_DOC", doc.as_str())]);
    Ok(())
}

// --- 各工具和用量、套餐有关的做法（原来 Tool / CodexTool / AgyTool 的方法）-------------------------------------------

/// agy 的档位 ID → (叫法, 档位)；认不出两样都是空的。sub 是查询带回来的，没有就用登录身份里的
fn agy_tier(key: &str, sub: &V) -> R<(&'static str, &'static str)> {
    let id = if sub.truthy() { sub.clone() } else { agy_claims(key).get("tier").unwrap_or(V::None) };
    hashable(&id)?;
    Ok(id.as_str().and_then(|id| AGY_TIERS.iter().find(|(x, _, _)| *x == id)).map_or(("", ""), |(_, name, tier)| (*name, *tier)))
}

impl Tool {
    /// `T.who(key)`：登录的是哪个账号——用量缓存按它认，换了账号旧缓存就不算。没登录是 None
    pub fn who(&self, key: &str) -> R<V> {
        Ok(match self.kind {
            Kind::Claude => oauth_of(key)?.get("accountUuid").unwrap_or(V::None),
            Kind::Codex => {
                let x = codex_claims(key, None);
                if x.is_empty() {
                    V::None
                } else {
                    V::from(format!("{}|{}", str_or_empty(&x.get("email")), str_or_empty(&x.get("account"))))
                }
            }
            Kind::Agy => agy_claims(key).get("email").filter(|e| e.truthy()).unwrap_or(V::None),
        })
    }

    /// `T.plan(key, sub)`：套餐的叫法。sub 是查用量带回来的套餐（没有是 None）
    pub fn plan(&self, key: &str, sub: &V) -> R<String> {
        Ok(match self.kind {
            Kind::Claude => plan_label(&oauth_of(key)?, sub),
            Kind::Codex => codex_plan_label(&if sub.truthy() { sub.clone() } else { codex_claims(key, None).get("plan").unwrap_or(V::None) }),
            Kind::Agy => agy_tier(key, sub)?.0.to_string(),
        })
    }

    /// `T.plan_tier(key, sub)`：套餐的档位（entry / standard / advanced / premium，认不出是 ''）
    pub fn plan_tier(&self, key: &str, sub: &V) -> R<&'static str> {
        Ok(match self.kind {
            Kind::Claude => claude_plan_tier(&oauth_of(key)?, sub),
            Kind::Codex => codex_plan_tier(&if sub.truthy() { sub.clone() } else { codex_claims(key, None).get("plan").unwrap_or(V::None) }),
            Kind::Agy => agy_tier(key, sub)?.1,
        })
    }

    /// `T.outdated_login_plan(key)`：Claude Code 登录时记下的套餐和账号现在的对不上——记下的那个的叫法（Pro 等），要重新登录
    /// 才会更新；对得上、不知道时是 ''。Codex 不比（app-server 查用量时回的就是服务器现在的套餐，没有一份登录时记下的可比）
    pub fn outdated_login_plan(&self, key: &str) -> R<String> {
        Ok(match self.kind {
            Kind::Claude => claude_login_plan_outdated(&oauth_of(key)?, &usage_login_plan(key)?).to_string(),
            Kind::Codex | Kind::Agy => String::new(),
        })
    }

    /// `T.usage_query(bin_, timeout)`
    pub fn usage_query(&self, bin_: &str, timeout: &str) -> R<()> {
        match self.kind {
            Kind::Claude => claude_usage_query(bin_, timeout),
            Kind::Codex => codex_usage_query(bin_, timeout),
            Kind::Agy => die(format!("{} 没有查用量的接口", self.label)),
        }
    }

    /// `T.usage_caches(key)`：最近一次的用量从哪找——cca 存的 usage.json，Claude 还有 Claude Code 自己缓存的那份
    pub fn usage_caches(&self, key: &str) -> R<Vec<Option<V>>> {
        Ok(match self.kind {
            Kind::Claude => vec![load_quiet(&usage_file(key)), acct_json(key)?.get("cachedUsageUtilization")],
            Kind::Codex => vec![load_quiet(&usage_file(key))],
            Kind::Agy => Vec::new(),
        })
    }
}

/// 单元测试里两个文件共用的：对照数据（tests/vectors/usage.json，tools/gen_usage_vectors.py 从 PY_SRC 生成）和读它的几样
#[cfg(test)]
pub(super) mod vectors {
    use crate::py::json;
    use crate::py::obj::{Dict, V};
    use crate::py::text;
    use crate::py::{Fail, R};

    pub fn load() -> Dict {
        json::loads(include_str!("../../tests/vectors/usage.json")).expect("对照数据是 JSON").as_dict().expect("是个对象")
    }

    /// 一节对照数据的各行（每行是一个列表）
    pub fn rows(section: &str) -> Vec<Vec<V>> {
        load().list(section).unwrap_or_else(|| panic!("对照数据里没有 {section}")).items().iter().map(|r| r.as_list().expect("一行是个列表").items()).collect()
    }

    /// Python 的 repr 写的数（nan、inf、-inf 也认）
    pub fn number(v: &V) -> f64 {
        text::to_float(v.as_str().expect("数字写成了文字")).expect("是个数")
    }

    /// JSON 文字 → 值（NaN、Infinity 也认）
    pub fn parse(v: &V) -> V {
        json::loads(v.as_str().expect("JSON 写成了文字")).expect("是 JSON")
    }

    /// 出错时 Python 那边是哪一类异常
    pub fn raised(e: &Fail) -> &'static str {
        match e {
            Fail::Value(_) => "ValueError",
            Fail::Os(_) => "OSError",
            Fail::Other(class, _) => class,
            Fail::Die(_) | Fail::Exit(_) => "SystemExit",
        }
    }

    /// 结果对得上：want 是那边的返回值，或者 {"raises": 异常的类名}
    pub fn check<T: Into<V>>(got: R<T>, want: &V, what: &str) {
        let want_class = want.as_dict().and_then(|d| d.str("raises"));
        match (got, want_class) {
            (Ok(got), None) => {
                let got: V = got.into();
                assert_eq!(json::dumps(&got, json::LINE), json::dumps(want, json::LINE), "{what}");
            }
            (Err(e), Some(class)) => assert_eq!(raised(&e), &*class, "{what}"),
            (Ok(got), Some(class)) => panic!("{what}：那边是 {class}，这边得到了 {}", got.into().py_repr()),
            (Err(e), None) => panic!("{what}：那边是 {}，这边出错了：{e}", want.py_repr()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vectors::{check, number, parse, rows};
    use super::*;
    use crate::py::re::Re;

    fn j(text: &str) -> V {
        json::loads(text).unwrap_or_else(|e| panic!("{text}：{e}"))
    }

    fn d(text: &str) -> Dict {
        j(text).as_dict().expect("是个对象")
    }

    #[test]
    fn parse_ts_matches_python() {
        // 期望值是 CPython 3.14 算的
        let p = |s: &str| parse_ts(&V::from(s)).unwrap();
        assert_eq!(p("2026-11-19T08:00:00Z"), Some(1795075200.0));
        assert_eq!(p("2026-11-19 08:00"), Some(1795075200.0));
        assert_eq!(p(" 2026-11-19T08:00:00.25+08:00 "), Some(1795046400.25));
        assert_eq!(p("2026-11-19T08:00:00-0530"), Some(1795095000.0));
        assert_eq!(p("2026-02-31T25:61:00Z"), Some(1772589660.0)); // 日、时、分不核对，照样往后算
        assert_eq!(p("2026-11-19"), None);
        assert_eq!(p("2026-11-19T08:00:00 UTC"), None);
        assert!(parse_ts(&V::from("2026-13-01T00:00:00Z")).is_err() && parse_ts(&V::from("0000-01-01T00:00:00Z")).is_err());
        assert_eq!(parse_ts(&V::Int(1795075200)).unwrap(), Some(1795075200.0));
        assert_eq!(parse_ts(&V::Int(1795075200123)).unwrap(), Some(1795075200.123));
        assert_eq!(parse_ts(&V::Float(1.5)).unwrap(), Some(1.5));
        assert_eq!(parse_ts(&V::Bool(true)).unwrap(), None);
        assert_eq!(parse_ts(&V::None).unwrap(), None);
    }

    #[test]
    fn spans_match_python() {
        for r in rows("span") {
            let secs = number(&r[0]);
            check(span(secs), &r[1], &format!("span({})", r[0].py_str()));
            check(span_short(secs), &r[2], &format!("span_short({})", r[0].py_str()));
            check(ago(secs), &r[3], &format!("ago({})", r[0].py_str()));
        }
        for r in rows("iso") {
            check(iso(number(&r[0])), &r[1], &format!("iso({})", r[0].py_str()));
        }
        for r in rows("floordiv") {
            assert_eq!(float_repr(floordiv_f(number(&r[0]), 60.0)), r[1].py_str(), "{} // 60", r[0].py_str());
        }
        for r in rows("true_div") {
            let n: i128 = r[0].as_str().unwrap().parse().unwrap();
            assert_eq!(float_repr(true_div(n, r[1].as_int().unwrap() as u32)), r[2].py_str(), "{} / {}", r[0].py_str(), r[1].py_str());
        }
        for r in rows("weekday_clock") {
            let int = |i: usize| r[i].as_int().unwrap() as i32;
            let lt = Tm { year: 2026, mon: 11, mday: 19, hour: int(1), min: int(2), sec: 0, wday: int(0), yday: 323, isdst: 0 };
            assert_eq!(weekday_clock(&lt), r[3].py_str());
        }
        // 5 小时的窗口（week 是 false）只写还有多久：和时区无关
        for r in rows("reset_short") {
            check(reset_short(number(&r[0]), number(&r[1]), false), &r[2], &format!("reset_short({}, {})", r[0].py_str(), r[1].py_str()));
        }
    }

    #[test]
    fn reset_short_like_python_test() {
        // tests/usage_view.py 的 test_reset_short
        let now = time::time();
        assert_eq!(reset_short(now + 2.0 * 3600.0 + 13.0 * 60.0 + 5.0, now, false).unwrap(), "2小时13分后");
        assert_eq!(reset_short(now + 30.0, now, false).unwrap(), "不到1分钟后");
        assert_eq!(reset_short(now + 25.0 * 60.0 + 5.0, now, true).unwrap(), "25分钟后"); // 本周的一小时以内也写还有多久
        let lt = time::localtime(now + 3.0 * 86400.0);
        assert_eq!(reset_short(now + 3.0 * 86400.0, now, true).unwrap(), format!("周{} {:02}:{:02}", WEEKDAYS[lt.wday as usize], lt.hour, lt.min));
        // 今天、明天按本地的日历日算
        let lt = time::localtime(now);
        let midnight = time::mktime(lt.year, lt.mon, lt.mday + 1, 0, 0, 0, -1);
        if midnight - now > 2.0 * 3600.0 + 60.0 {
            assert!(reset_short(now + 2.0 * 3600.0, now, true).unwrap().starts_with("今天 "));
        }
        assert!(reset_short(midnight + 3600.0 * 5.0, now, true).unwrap().starts_with(if midnight + 3600.0 * 5.0 - now < 3600.0 { "5" } else { "明天 " }));
        assert_eq!(day_gap(now, midnight + 86400.0 + 60.0).unwrap(), 2);
        assert_eq!(day_gap(midnight + 60.0, now).unwrap(), -1);
        // 年份装不进 date 的：原来是没人接的 ValueError
        assert!(matches!(day_gap(now, 253402300800.0 + 86400.0 * 400.0), Err(Fail::Value(_))));
        assert!(matches!(reset_short(f64::NAN, now, true), Err(Fail::Value(_))));
    }

    #[test]
    fn plans_match_python() {
        for r in rows("plans") {
            let (oa, sub) = (r[0].as_dict().unwrap(), &r[1]);
            let what = format!("{} / {}", r[0].py_repr(), sub.py_repr());
            assert_eq!(plan_label(&oa, sub), r[2].py_str(), "plan_label {what}");
            let (kind, x) = claude_plan_parts(&oa, sub);
            assert_eq!((kind.as_str(), x), (r[3].as_str().unwrap(), r[4].as_int().unwrap()), "claude_plan_parts {what}");
            assert_eq!(claude_plan_tier(&oa, sub), r[5].py_str(), "claude_plan_tier {what}");
        }
        for r in rows("outdated") {
            assert_eq!(claude_login_plan_outdated(&r[0].as_dict().unwrap(), r[1].as_str().unwrap()), r[2].py_str(), "{} / {}", r[0].py_repr(), r[1].py_repr());
        }
        // agy 的档位表和 PY_SRC 的 AGY_TIERS 一样
        let tiers = vectors::load().dict("agy_tiers").unwrap();
        assert_eq!(tiers.len(), AGY_TIERS.len());
        for (id, name, tier) in AGY_TIERS {
            assert_eq!(tiers.list(id).unwrap().strs().iter().map(|s| s.to_string()).collect::<Vec<_>>(), [*name, *tier]);
        }
    }

    #[test]
    fn capitalize_matches_python() {
        // 标题写法和大写不一样的那些字符，一个不落
        let special = rows("capitalize");
        assert_eq!(special.len(), 135);
        for r in special.iter().chain(rows("capitalize_words").iter()) {
            assert_eq!(capitalize(r[0].as_str().unwrap()), r[1].py_str(), "{}", r[0].py_repr());
        }
    }

    #[test]
    fn rounding_matches_python() {
        for r in rows("round") {
            let p = py_round(&parse(&r[0])).unwrap();
            assert_eq!(p.text, r[1].py_str(), "round({})", r[0].py_str());
            assert_eq!(format!("{:>3}%", p.text), r[2].py_str(), "round({})", r[0].py_str());
            assert_eq!((p.val >= 90.0, p.val >= 70.0), (r[3].truthy(), r[4].truthy()), "round({})", r[0].py_str());
        }
        assert!(matches!(py_round(&V::Float(f64::NAN)), Err(Fail::Value(_))));
        assert!(py_round(&V::Float(f64::INFINITY)).is_err_and(|e| e.is("OverflowError")));
        for r in rows("reset_count") {
            let rl = dict! { "reset_credits" => dict! { "availableCount" => parse(&r[0]) } };
            let got = reset_count(&rl).unwrap().map(|n| (n.text, n.zero, n.many));
            let want = r[1].as_list().map(|l| (l.at(0).py_str(), l.at(1).truthy(), l.at(2).truthy()));
            assert_eq!(got, want, "reset_count({})", r[0].py_str());
        }
        assert!(reset_count(&dict! { "reset_credits" => dict! { "availableCount" => f64::NAN } }).is_err());
        assert!(reset_count(&dict! { "reset_credits" => "x" }).unwrap().is_none());
    }

    #[test]
    fn windows_match_python() {
        for r in rows("win") {
            let now = r[1].as_num().unwrap();
            let got = win(&r[0], now).unwrap().map(|w| dict! { "pct" => w.pct, "at" => w.at, "reset" => w.reset });
            check(Ok(got), &r[2], &format!("win({}, {})", r[0].py_repr(), now));
        }
        assert!(win(&j(r#"{"utilization": 1, "resets_at": "2026-13-01T00:00:00Z"}"#), 0.0).is_err());
    }

    #[test]
    fn weeks_view_matches_python() {
        for r in rows("weeks_view") {
            let cached = r[2].as_dict().map(|c| Cached { at: c.at("at").as_num().unwrap(), rl: c.dict("rl").unwrap() });
            let got = weeks_view(weeks_parse(Some(r[0].clone()).filter(|doc| !doc.is_none())), number(&r[1]), cached.as_ref());
            let what = format!("usage_weeks_view：{} / {} / {}", r[0].py_repr(), r[1].py_str(), r[2].py_repr());
            match (&r[3], got) {
                // 那边记的是 json.dumps 写出来的文字：数字是整数还是小数也要一样
                (V::Str(want), Ok(got)) => assert_eq!(json::dumps(&V::from(got), json::Fmt::new()), **want, "{what}"),
                (want, got) => check(got, want, &what),
            }
        }
    }

    #[test]
    fn usage_cell_matches_python() {
        for r in rows("usage_cell") {
            let cached = parse(&r[0]).as_dict().map(|c| Cached { at: c.at("at").as_num().unwrap(), rl: c.dict("rl").unwrap() });
            check(cell_text(cached.as_ref(), number(&r[1])), &r[2], &format!("usage_cell：{} / {}", r[0].py_str(), r[1].py_str()));
        }
    }

    #[test]
    fn fit_note_like_python_test() {
        // tests/usage_view.py 的 test_fit_note
        let parts: Vec<String> = ["剩余重置 2 次（最早 12 月 5 日过期）", "GPT-5-Pro 本周 30%", "credits 余额 12.5"].iter().map(|s| s.to_string()).collect();
        assert_eq!(fit_note(&parts, 200), parts.join("；"));
        assert_eq!(fit_note(&parts, 60), "剩余重置 2 次（最早 12 月 5 日过期）；GPT-5-Pro 本周 30%；…");
        assert_eq!(fit_note(&parts, 20), "剩余重置 2 次…"); // 第一项都放不下：先去掉括号里的补充
        assert_eq!(fit_note(&["出口端口 7902 不通".to_string()], 10), "出口端口 …");
    }

    #[test]
    fn use_first_like_python_test() {
        // tests/usage_view.py 的 test_use_first
        let now = time::time();
        let at = |secs: f64| (now + secs).floor();
        let rl = |pct: V, secs: f64| dict! { "seven_day" => dict! { "utilization" => pct, "resets_at" => iso(now + secs).unwrap() } };
        let left = |pct: V, secs: f64, why: &V| use_first(&rl(pct, secs), now, why).unwrap();
        let got = left(V::Int(41), 36000.0, &V::None).unwrap();
        assert!(V::Dict(got).py_eq(&V::Dict(dict! { "left_percent" => 59, "from_epoch" => at(36000.0) - 86400.0, "until_epoch" => at(36000.0) })));
        assert_eq!(left(V::Float(80.4), 36000.0, &V::None).unwrap().at("left_percent").as_int(), Some(20)); // 按四舍五入后显示的百分比（80%）
        assert!(left(V::Float(80.5), 36000.0, &V::None).is_none()); // 显示 81%：还剩 19%
        assert!(left(V::Int(41), -60.0, &V::None).is_none()); // 重置时间已经过了：缓存里的数不算数
        assert!(use_first(&d(r#"{"seven_day": {"utilization": 41}}"#), now, &V::None).unwrap().is_none()); // 不知道什么时候重置
        assert!(use_first(&Dict::new(), now, &V::None).unwrap().is_none());
        assert!(left(V::Int(41), 36000.0, &V::from("port")).is_none()); // 这次查询说明账号现在用不了
        assert!(left(V::Int(41), 36000.0, &V::from("timeout")).is_some()); // 只是没查成：照缓存里的数
        assert!(use_first(&rl(V::Float(f64::NAN), 36000.0), now, &V::None).is_err());
    }

    #[test]
    fn weeks_add_like_python_test() {
        // tests/usage_view.py 的 test_weeks_add：按重置时间认是哪一周，记最高的百分比；掉下去一大截是中间重置过，之前的累计下来
        let w = WEEK as f64;
        let t = 1_800_000_000.0;
        let end = t + 3.0 * 86400.0;
        let rl = |pct: f64, e: f64| dict! { "seven_day" => dict! { "utilization" => pct, "resets_at" => e } };
        let mut weeks: Vec<Week> = Vec::new();
        let add = |weeks: &mut Vec<Week>, rl: Dict, at: f64| weeks_add(weeks, &rl, at).unwrap();
        assert!(add(&mut weeks, rl(10.0, end), t));
        assert!(add(&mut weeks, rl(30.0, end), t + 60.0));
        assert!(add(&mut weeks, rl(27.0, end + 0.4), t + 120.0)); // 低了不到 5 个百分点；重置时间差零点几秒：还是同一周
        assert_eq!(weeks.len(), 1);
        let seen = |e: &Week| (num(&e.peak), num(&e.carried), e.resets, num(&e.end), num(&e.first), num(&e.last));
        assert_eq!(seen(&weeks[0]), (30.0, 0.0, 0, end + 0.4, t, t + 120.0));
        assert!(!add(&mut weeks, rl(50.0, end), t + 30.0)); // 比已经记下的旧（Claude Code 的缓存）：不改
        assert!(!add(&mut weeks, rl(50.0, end), t + 120.0)); // 同一次查询
        assert!(add(&mut weeks, rl(1.0, end), t + 180.0)); // 掉到 1%：中间重置过
        assert!(add(&mut weeks, rl(40.0, end), t + 240.0));
        assert_eq!(seen(&weeks[0]), (40.0, 30.0, 1, end, t, t + 240.0));
        assert!(!add(&mut weeks, rl(60.0, t), t + 300.0)); // 窗口已经重置了：缓存里的数不算数
        assert!(!add(&mut weeks, d(r#"{"seven_day": {"utilization": 5}}"#), t)); // 不知道什么时候重置（这一周还没开始用）
        assert!(!add(&mut weeks, Dict::new(), t));
        assert!(!add(&mut weeks, rl(f64::NAN, end), t + 400.0)); // NaN、无穷大不记（App 解码不了，算平均会出错）
        assert!(!add(&mut weeks, rl(f64::INFINITY, end), t + 400.0));
        assert!(!add(&mut weeks, rl(10.0, f64::INFINITY), t + 400.0));
        assert_eq!((num(&weeks[0].peak), num(&weeks[0].last)), (40.0, t + 240.0));
        for i in 1..70 {
            add(&mut weeks, rl(i as f64, end + i as f64 * w), t + i as f64 * w); // 新的一周接在后面，最多留 WEEKS_KEEP 周
        }
        assert_eq!(weeks.len(), WEEKS_KEEP);
        assert_eq!(num(&weeks[weeks.len() - 1].end), end + 69.0 * w);
        assert!(weeks.windows(2).all(|p| num(&p[0].end) <= num(&p[1].end)));
        // 文件里写的是整数的，没改到的项照样是整数
        let mut weeks = weeks_parse(Some(j(r#"{"weeks": [{"end": 1800259200, "peak": 41, "first": 5, "last": 6, "carried": 3, "resets": 2.9}]}"#)));
        assert!(add(&mut weeks, rl(20.0, 1800259200.0), 7.0));
        assert_eq!(json::dumps(&V::Dict(weeks[0].to_dict()), json::Fmt::new()), r#"{"end": 1800259200.0, "peak": 20.0, "first": 5, "last": 7.0, "carried": 44, "resets": 3}"#);
        assert!(week_num(&V::Int(999_999_999_999_999_999)) && !week_num(&V::Int(1_000_000_000_000_000_000)) && !week_num(&V::Float(1e18)) && !week_num(&V::Bool(true)));
    }

    #[test]
    fn weeks_record_file_like_python_test() {
        // tests/usage_view.py 的 test_weeks_record_file：查到用量时记进账号目录的 usage-weeks.json（600）；同一次查询不重写，
        // 坏的项读的时候不要
        use std::os::unix::fs::MetadataExt;
        let dir = crate::py::fs::TempDir::new().unwrap();
        let p = path::join(&dir.path, "usage-weeks.json");
        let load = || weeks_parse(load_quiet(&p));
        let dump = |weeks: Vec<Week>| json::dumps(&V::from(weeks.iter().map(|w| V::Dict(w.to_dict())).collect::<Vec<V>>()), json::Fmt::new());
        let now = time::time();
        let rl = dict! { "seven_day" => dict! { "utilization" => 12.5, "resets_at" => now + 86400.0 } };
        weeks_record_at(&p, &rl, now).unwrap();
        let st = std::fs::metadata(&p).unwrap();
        assert_eq!(st.mode() & 0o777, 0o600);
        assert_eq!(load().iter().map(|w| (num(&w.peak), num(&w.first))).collect::<Vec<_>>(), [(12.5, now)]);
        weeks_record_at(&p, &rl, now).unwrap();
        let again = std::fs::metadata(&p).unwrap();
        assert_eq!((again.ino(), again.mtime(), again.mtime_nsec()), (st.ino(), st.mtime(), st.mtime_nsec()), "同一次查询不重写");
        std::fs::write(&p, r#"{"version": 1, "weeks": [{"end": "x"}, "bad", {"end": 1, "peak": 2, "first": 3, "last": 4}]}"#).unwrap();
        assert_eq!(dump(load()), r#"[{"end": 1, "peak": 2, "first": 3, "last": 4, "carried": 0, "resets": 0}]"#);
        std::fs::write(
            &p,
            r#"{"version": 1, "weeks": [{"end": 1, "peak": NaN, "first": 3, "last": 4}, {"end": 5, "peak": 6, "first": 7, "last": 8, "carried": Infinity, "resets": NaN}]}"#,
        )
        .unwrap();
        assert_eq!(dump(load()), r#"[{"end": 5, "peak": 6, "first": 7, "last": 8, "carried": 0, "resets": 0}]"#);
        assert!(weeks_parse(load_quiet(&path::join(&dir.path, "nobody.json"))).is_empty(), "没有记录的账号");
    }

    #[test]
    fn usage_cached_keeps_model_scoped() {
        // tests/usage_view.py 的 test_usage_cached_keeps_model_scoped：Claude Code 的用量缓存比 usage.json 新时，它没有 model_scoped，
        // 照 usage.json 有哪几个模型从它的 limits 补上；usage.json 里没有的模型不加，账号对不上的不算
        let now = time::time();
        let ends = "2099-01-01T00:00:00Z";
        let mine = |at: f64, rl: &Dict, uuid: &str| Some(V::Dict(dict! { "fetchedAtMs" => at * 1000.0, "accountUuid" => uuid, "utilization" => rl }));
        let weekly = d(r#"{"kind": "weekly_all", "percent": 61, "resets_at": "2099-01-01T00:00:00Z", "scope": null}"#);
        let fable = d(r#"{"kind": "weekly_scoped", "percent": 44, "resets_at": "2099-01-01T00:00:00Z", "scope": {"model": {"id": null, "display_name": "Fable"}}}"#);
        let with = |base: &Dict, k: &str, v: V| {
            let c = base.copy();
            c.set(k, v);
            c
        };
        let scoped = j(r#"[{"display_name": "Fable", "utilization": 43, "resets_at": "2099-01-01T00:00:00Z"}]"#);
        let ours = dict! {
            "five_hour" => V::None, "seven_day" => dict! { "utilization" => 60, "resets_at" => ends },
            "limits" => crate::list![with(&weekly, "percent", V::Int(60)), with(&fable, "percent", V::Int(43))], "model_scoped" => &scoped,
            "weekly_scoped_shares" => j(r#"[{"limit_index": 1, "used_percent_of_weekly": 21, "allowance_percent_of_weekly": 50}]"#),
        };
        let theirs = dict! {
            "five_hour" => V::None, "seven_day" => dict! { "utilization" => 61, "resets_at" => ends }, "limits" => crate::list![&weekly, &fable],
            "weekly_scoped_shares" => j(r#"[{"limit_index": 1, "used_percent_of_weekly": 22, "allowance_percent_of_weekly": 50}]"#),
        };
        let pick = |caches: Vec<Option<V>>| usage_pick(&V::from("u-1"), caches).unwrap();
        let dump = |v: Option<V>| json::dumps(&v.unwrap_or(V::None), json::Fmt::new());
        let x = pick(vec![mine(now - 10.0, &ours, "u-1"), mine(now - 2.0, &theirs, "u-1")]).unwrap();
        assert_eq!(x.at, (now - 2.0) * 1000.0 / 1000.0);
        assert_eq!(x.rl.dict("seven_day").unwrap().at("utilization").as_int(), Some(61));
        assert_eq!(dump(x.rl.get("model_scoped")), r#"[{"display_name": "Fable", "utilization": 44, "resets_at": "2099-01-01T00:00:00Z"}]"#);
        assert!(!theirs.has("model_scoped")); // 只在返回的那份里补，读到的不改
        let mut row = Row {
            key: "bob@example.com".into(), name: "bob@example.com".into(), alias: String::new(), email: "bob@example.com".into(), plan: "Ultra".into(),
            source: "cache", at: Some(x.at), rl: x.rl.clone(), why: V::None, msg: V::from(""), outdated_login_plan: String::new(),
        };
        let e = usage_entry(&row, now).unwrap();
        assert!(e.at("five_hour").is_none());
        assert_eq!(
            dump(e.get("model_scoped")),
            r#"[{"utilization": 44, "resets_at": "2099-01-01T00:00:00Z", "resets_epoch": 4070908800.0, "reset": false, "name": "Fable", "used_percent_of_weekly": 22, "allowance_percent_of_weekly": 50}]"#
        );
        // usage.json 是新的那份：原样
        assert!(pick(vec![mine(now - 1.0, &ours, "u-1"), mine(now - 2.0, &theirs, "u-1")]).unwrap().rl.same(&ours));
        // 一样新：取 usage.json
        assert!(pick(vec![mine(now - 2.0, &ours, "u-1"), mine(now - 2.0, &theirs, "u-1")]).unwrap().rl.same(&ours));
        // limits 被拿掉了（Claude Code 查不到时拿缓存顶上的那种）：照搬 usage.json 的那一项
        let bare = Dict::from_pairs(theirs.items().into_iter().filter(|(k, _)| &**k != "limits"));
        assert_eq!(dump(pick(vec![mine(now - 10.0, &ours, "u-1"), mine(now - 2.0, &bare, "u-1")]).unwrap().rl.get("model_scoped")), dump(Some(scoped.clone())));
        // limits 里没有这个模型了：也照搬，哪一行都不少（下一次查到时以查到的为准）
        let gone = with(&theirs, "limits", V::from(crate::list![&weekly]));
        assert_eq!(dump(pick(vec![mine(now - 10.0, &ours, "u-1"), mine(now - 2.0, &gone, "u-1")]).unwrap().rl.get("model_scoped")), dump(Some(scoped.clone())));
        // usage.json 里没有分模型的额度（[]）：不照 limits 加，cca 不猜 get_usage 给哪几个模型
        let none = with(&ours, "model_scoped", j("[]"));
        assert_eq!(dump(pick(vec![mine(now - 10.0, &none, "u-1"), mine(now - 2.0, &theirs, "u-1")]).unwrap().rl.get("model_scoped")), "[]");
        // 只有 Claude Code 的缓存（cca 还没查过）：不补
        assert!(!pick(vec![None, mine(now - 2.0, &theirs, "u-1")]).unwrap().rl.has("model_scoped"));
        // usage.json 是别的账号的（换过登录）：不拿它补
        assert!(!pick(vec![mine(now - 10.0, &ours, "u-2"), mine(now - 2.0, &theirs, "u-1")]).unwrap().rl.has("model_scoped"));
        // 名字重复的只留一个；limits 里的不是数字时照搬旧的
        let twice = with(&ours, "model_scoped", V::from(crate::list![scoped.as_list().unwrap().at(0), scoped.as_list().unwrap().at(0)]));
        let odd = with(&theirs, "limits", V::from(crate::list![&weekly, with(&fable, "percent", V::None)]));
        assert_eq!(dump(pick(vec![mine(now - 10.0, &twice, "u-1"), mine(now - 2.0, &odd, "u-1")]).unwrap().rl.get("model_scoped")), dump(Some(scoped.clone())));
        assert!(pick(vec![None, None]).is_none());
        // 数据旧了、取到的时间：JSON 里的 age_seconds 朝 0 截断
        row.at = Some(now - 1410.9);
        let e = usage_entry(&row, now).unwrap();
        assert_eq!((e.at("age_seconds").as_int(), e.at("stale").as_bool()), (Some(1410), Some(true)));
        row.at = Some(now + 30.9);
        assert_eq!(usage_entry(&row, now).unwrap().at("age_seconds").as_int(), Some(-30));
    }

    /// tests/usage_view.py 的那四行：查到了的（备注两项）、Claude Code 记的套餐过时的、端口不通用缓存顶上的、没登录的
    fn table_rows(now: f64) -> Vec<Row> {
        let row = |alias: &str, email: &str, plan: &str, five: i64, week: i64, source: &'static str, why: V, msg: &str, extra: Option<Dict>, old: &str| {
            let rl = dict! {
                "five_hour" => dict! { "utilization" => five, "resets_at" => iso(now + 2.0 * 3600.0 + 13.0 * 60.0 + 30.0).unwrap() },
                "seven_day" => dict! { "utilization" => week, "resets_at" => iso(now + 3.0 * 86400.0 + 5.0 * 3600.0).unwrap() },
            };
            if let Some(extra) = extra {
                rl.update(&extra);
            }
            Row {
                key: if alias.is_empty() { email.to_string() } else { format!("{alias}({email})") },
                name: if alias.is_empty() { email } else { alias }.to_string(),
                alias: alias.to_string(),
                email: email.to_string(),
                plan: plan.to_string(),
                source,
                at: Some(now - if source == "cache" { 23.0 * 60.0 } else { 0.0 }),
                rl: if source == "none" { Dict::new() } else { rl },
                why,
                msg: V::from(msg),
                outdated_login_plan: old.to_string(),
            }
        };
        let weekly = d(r#"{"seven_day_sonnet": {"utilization": 60, "resets_at": "2099-01-01T00:00:00Z"},
                           "model_scoped": [{"display_name": "Fable", "utilization": 34, "resets_at": "2099-01-01T00:00:00Z"}]}"#);
        vec![
            row("工作", "bob@example.com", "Ultra", 13, 41, "live", V::None, "", Some(weekly), ""),
            row("", "alice@example.com", "Max", 98, 72, "live", V::None, "", None, "Pro"),
            row("副号", "carol@example.com", "Pro", 7, 33, "cache", V::from("port"), "7902", None, ""),
            row("", "dave@example.com", "", 0, 0, "none", V::from("login"), "", None, ""),
        ]
    }

    #[test]
    fn table_never_wider_than_terminal() {
        // tests/usage_view.py 的 test_one_line_per_account_never_wider_than_terminal
        let now = time::time();
        let rows = table_rows(now);
        for cols in 40..=200 {
            let lines = usage_table_lines(&rows, now, &cols.to_string(), false, &["kimi".to_string()]).unwrap();
            assert!(lines.iter().map(|l| uni::width(l)).max().unwrap() <= cols - 2, "{cols} 列：{lines:#?}");
            // 每个账号正好一行（别名、没有别名的邮箱总有一个看得到）
            for r in &rows {
                let shown = if r.alias.is_empty() { &r.email } else { &r.alias };
                assert_eq!(lines.iter().filter(|l| l.contains(shown.as_str()) && !l.trim_start().starts_with('!')).count(), 1, "{cols} 列：{shown}");
            }
        }
    }

    #[test]
    fn table_folds_columns_in_order() {
        // tests/usage_view.py 的 test_wide_terminal_keeps_everything、test_narrow_terminal_folds_columns_in_order
        let now = time::time();
        let table = |cols: &str| usage_table_lines(&table_rows(now), now, cols, false, &["kimi".to_string()]).unwrap();
        let like = |line: &str, pattern: &str| assert!(Re::new(pattern).found(line), "{line:?} 对不上 {pattern}");
        let lines = table("160");
        like(&lines[0], r"^  账号 +套餐 +5 小时/重置 +本周/重置 +备注$");
        like(&lines[1], r"^  工作  bob@example.com +Ultra +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；Fable 本周 34%$");
        like(&lines[3], r"出口端口 7902 不通；缓存（23 分钟前）$");
        like(&lines[4], r"^ {8}dave@example.com +还没登录（cca @dave@example.com 登录）$");
        assert_eq!(lines[5], "  ! alice@example.com：Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）");
        assert_eq!(lines[6], "  第三方 API 账号没有订阅用量，不查：kimi");
        let lines = table("80");
        like(&lines[0], r"^  账号 +5 小时/重置 +本周/重置 +备注$"); // 先收起有别名的账号的邮箱，再收套餐
        like(&lines[1], r"^  工作 +13% 2小时13分后 +41% 周. \d\d:\d\d  Sonnet 本周 60%；…$");
        like(&lines[3], r"出口端口 7902 不通；…$"); // 原因在前：窄了先保住它
        assert_eq!(lines[5], "  ! 套餐过时（记的是 Pro，现在是 Max）：cca login alice@example.com");
        let lines = table("40");
        like(&lines[0], r"^  账号 +5 小时 +本周$"); // 最窄时连重置时间也收起，不写备注
        assert!(lines.iter().any(|l| l.contains("终端太窄，备注没显示")));
        // 颜色：表头、不是这次查到的行变暗，90% 以上红、70% 以上黄
        let lines = usage_table_lines(&table_rows(now), now, "160", true, &[]).unwrap();
        assert!(lines[0].starts_with("\x1b[2m  账号") && lines[0].ends_with("备注\x1b[0m"));
        assert!(lines[2].contains("\x1b[31m 98% 2小时13分后\x1b[0m") && lines[2].contains("\x1b[33m 72% "));
        assert!(lines[5].starts_with("\x1b[33m  ! alice@example.com"));
        // 一行也没有：原来是没人接的 ValueError（max() 的参数是空的）
        assert!(matches!(usage_table_lines(&[], now, "80", false, &[]), Err(Fail::Value(_))));
        assert_eq!((cols_of("").unwrap(), cols_of("x").unwrap(), cols_of("１２０").unwrap(), cols_of("-5").unwrap()), (80, 80, 120, 80));
        assert!(cols_of("²").is_err() && cols_of(&"9".repeat(30)).unwrap() > 1 << 40);
    }

    #[test]
    fn why_texts() {
        let row = |why: V, msg: V| Row {
            key: "工作(bob@example.com)".into(), name: "工作".into(), alias: "工作".into(), email: "bob@example.com".into(), plan: String::new(), source: "none",
            at: None, rl: Dict::new(), why, msg, outdated_login_plan: String::new(),
        };
        let said = |why: &str, msg: &str, table: WhyTable| usage_why(&row(V::from(why), V::from(msg)), table).unwrap();
        assert_eq!(said("login", "", WhyTable::Usage), "还没登录（cca @工作 登录）");
        assert_eq!(said("mismatch", "x@example.com", WhyTable::Usage), "登录的是 x@example.com，不是这个账号（cca @工作 改回）");
        assert_eq!(said("trace", "", WhyTable::Usage), "出口连不上 claude.ai");
        assert_eq!(said("unsupported", "", WhyTable::Usage), "这个版本的 Claude Code 查不了用量（claude update 升级后再试）");
        assert_eq!(said("unsupported", "", WhyTable::Reset), "这个版本的 Claude Code 不能重置额度（claude update 升级后再试）");
        assert_eq!(said("timeout", "", WhyTable::Reset), "查询超时");
        assert_eq!(said("timeout", "", WhyTable::Unsure), "没等到重置的结果，可能已经用掉了一次：刷新用量看看还剩几次");
        assert_eq!(said("error", "", WhyTable::Unsure), "重置出错了（原因不明），可能已经用掉了一次：刷新用量看看还剩几次");
        assert_eq!(said("no-such-code", "{name} {", WhyTable::Usage), "查询失败：{name} {"); // 填进去的字不再当模板看
        assert_eq!(usage_why(&row(V::Int(5), V::Int(7)), WhyTable::Usage).unwrap(), "查询失败：7");
        assert!(usage_why(&row(j("[1]"), V::None), WhyTable::Usage).is_err_and(|e| e.is("TypeError")));
        assert_eq!(reset_fail_code("auth"), "not_logged_in");
        assert_eq!(reset_fail_code("timeout"), "error");
        assert_eq!(login_plan_note("工作(bob@example.com)", "Pro", "Max"), "Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），账号现在是 Max：重新登录一次就会更新（cca login 工作）");
    }
}
