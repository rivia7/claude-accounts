//! 回归脚本（tests/regress.sh）里用的小工具：原来各是一行 `python3 -c`，求值器（tests/tool/ev）不该管的那些——端口、时间、
//! 文件的修改时间、进程、整段文字的替换、几样校验和换算。打出来的、退出码、对文件做的事都和原来那一行一样。
//!
//! ```text
//! split-and 断言            把一条断言按顶层的 " && " 拆开（引号里面的不拆），每段去掉两头的空白、后面跟一个 NUL（ok_why 用）
//! port                      本机一个现在空着的端口，一行
//! hold-port 端口 就绪文件   在 127.0.0.1 上占住这个端口（只听不接），占上以后建出就绪文件，然后一直占着：到被停掉为止，
//!                           最多 600 秒
//! maxw                      stdin 的文字里最宽的一行占几格（东亚宽字符算两格），一行；一行都没有：退出码 1
//! time --utc [--add 秒] --fmt 格式
//!                           现在（加上这么多秒，可以是负的）的 UTC 时间照格式写出来，一行；格式里认 %Y %m %d %H %M %S %%
//! touch-ago 文件 秒         把文件的访问时间、修改时间拨到这么多秒以前
//! pstart 进程号             进程的启动时间戳，一行：LC_ALL=C ps -o lstart= 说的那个时刻按本地时间换算（和 cca 一样的算法）
//! utf8                      stdin 是合法的 UTF-8：退出码 0；不是：1
//! text-replace 文件 [--once] 原文 新文 [[--once] 原文 新文]…
//!                           文件里的「原文」都换成「新文」（一处都没有也行）；写了几对就依次换，最后写一次文件。
//!                           前面带 --once 的那一对：原文得正好出现一次，不然退出码 1、文件不动
//! text-sub 文件 正则 新文   文件里头一处匹配正则的换成「新文」（原样放进去，不认 $1 这些）；换完和原来一样：退出码 1、
//!                           文件不动。正则是 regex 的写法，另认 \Q…\E：夹在中间的原样匹配
//! ckof 节点的JSON           自定义节点的身份摘要 ck，一行（辅助程序的 custom_ck、路由器端的 fnv(ckey(…)) 是同一个算法，
//!                           这里另写一份核对）
//! b64                       stdin 的字节的 base64，一行
//! plist-json 文件           属性列表（XML、二进制的都认）转成 JSON，一行；里面有日期、二进制数据的转不了
//! ```
//!
//! 退出码：0 是做完了；1 是原来那一行 Python 会抛异常的地方（文件不在、断言不成立、不是 UTF-8、端口占不上……），stderr 上
//! 说一行，要改的文件不动；2 是用法不对（少了参数、不认识的选项、认不了的正则），另在 $CCA_TEST_TRAP 里记一行——回归最后
//! 一条用例查这个文件是空的。
//! 读文字文件照 Python 的 open(p).read()：得是 UTF-8，\r\n、\r 读成 \n；写回去是整个覆盖（文件是符号链接时改的是它指着的）。

use crate::trap::usage;
use kit::json::{self, Obj, Style, Value};
use kit::py;
use std::io::{Read, Write};

pub const CMDS: &[(&str, crate::Run)] = &[
    ("split-and", split_and_cmd),
    ("port", port),
    ("hold-port", hold_port),
    ("maxw", maxw),
    ("time", time),
    ("touch-ago", touch_ago),
    ("pstart", pstart),
    ("utf8", utf8),
    ("text-replace", text_replace),
    ("text-sub", text_sub),
    ("ckof", ckof_cmd),
    ("b64", b64),
    ("plist-json", plist_json),
];

/// 原来那一行 Python 在这里会抛异常：说一行，退出码 1
fn raised(role: &str, why: impl std::fmt::Display) -> i32 {
    eprintln!("cca-test {role}：{why}");
    1
}

/// 写到 stdout；对方不收了（EPIPE）就算了。退出码 0
fn put(bytes: &[u8]) -> i32 {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes).and_then(|()| out.flush());
    0
}

/// stdin 的全部字节
fn stdin_bytes() -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::stdin().lock().read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Python 读文字时的换行：\r\n、单独的 \r 都读成 \n
fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// open(p).read()：整个文件当文字读进来；读不了、不是 UTF-8 时是要说的那一行
fn read_text(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读不了 {path}：{e}"))?;
    let text = String::from_utf8(bytes).map_err(|e| format!("{path} 不是 UTF-8：{e}"))?;
    Ok(universal_newlines(&text))
}

/// 现在的时间戳（time.time()）
fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ---- split-and ----

/// 一条断言按顶层的 " && " 拆开：单引号、双引号里面的不拆（双引号里反斜杠后面那个字符跟着走），每段去掉两头的空白
fn split_and(text: &str) -> Vec<String> {
    let s: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if let Some(q) = quote {
            cur.push(c);
            if c == '\\' && q == '"' && i + 1 < s.len() {
                cur.push(s[i + 1]);
                i += 1;
            } else if c == q {
                quote = None;
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            cur.push(c);
        } else if s[i..].starts_with(&[' ', '&', '&', ' ']) {
            parts.push(std::mem::take(&mut cur));
            i += 3;
        } else {
            cur.push(c);
        }
        i += 1;
    }
    parts.push(cur);
    parts.iter().map(|p| py::strip(p).to_string()).collect()
}

fn split_and_cmd(args: &[String]) -> i32 {
    let [text] = args else { return usage("split-and", args, "用法：cca-test split-and 断言") };
    let mut out = Vec::new();
    for part in split_and(text) {
        out.extend_from_slice(part.as_bytes());
        out.push(0);
    }
    put(&out)
}

// ---- 端口 ----

fn port(args: &[String]) -> i32 {
    if !args.is_empty() {
        return usage("port", args, "不要参数");
    }
    put(format!("{}\n", kit::fsx::free_port()).as_bytes())
}

/// 占着端口的时间：原来那一行是 time.sleep(600)。用它的地方占上就不管了，等回归 kill 它；万一没人 kill，到时候自己退出
const HOLD: std::time::Duration = std::time::Duration::from_secs(600);

/// 占住 127.0.0.1 上的这个端口，建出就绪文件，占够这么久再放开；占不上、建不了文件是要说的那一行
fn hold(port: u16, ready: &str, how_long: std::time::Duration) -> Result<(), String> {
    // 标准库在 Unix 上绑之前就设了 SO_REUSEADDR，和原来一样
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("占不上 127.0.0.1:{port}：{e}"))?;
    std::fs::File::create(ready).map_err(|e| format!("建不了 {ready}：{e}"))?;
    std::thread::sleep(how_long);
    drop(listener);
    Ok(())
}

fn hold_port(args: &[String]) -> i32 {
    let [port, ready] = args else { return usage("hold-port", args, "用法：cca-test hold-port 端口 就绪文件") };
    let Ok(port) = port.parse::<u16>() else { return usage("hold-port", args, &format!("{port} 不是端口")) };
    match hold(port, ready, HOLD) {
        Ok(()) => 0,
        Err(why) => raised("hold-port", why),
    }
}

// ---- maxw ----

/// 最宽的一行占几格；一行都没有是 None
fn max_width(text: &str) -> Option<usize> {
    py::splitlines(text).into_iter().map(kit::width::of).max()
}

fn maxw(args: &[String]) -> i32 {
    if !args.is_empty() {
        return usage("maxw", args, "不要参数（文字从 stdin 读）");
    }
    let text = match stdin_bytes().map(String::from_utf8) {
        Ok(Ok(text)) => universal_newlines(&text),
        Ok(Err(e)) => return raised("maxw", format!("stdin 不是 UTF-8：{e}")),
        Err(e) => return raised("maxw", format!("读不了 stdin：{e}")),
    };
    match max_width(&text) {
        Some(width) => put(format!("{width}\n").as_bytes()),
        None => raised("maxw", "一行都没有（max() 的参数是空的）"),
    }
}

// ---- 时间 ----

/// 时间戳（UTC）→ 年、月、日、时、分、秒
fn civil(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // 从 0000-03-01 起算的天数 → 年月日（400 年一轮，一轮 146097 天；三月当一年的头一个月，闰日就落在年尾）
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// time.strftime(格式, time.gmtime(时间戳))，只认 %Y %m %d %H %M %S %%；别的 % 写法是 Err
fn strftime_utc(format: &str, secs: i64) -> Result<String, String> {
    let (year, month, day, hour, minute, second) = civil(secs);
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{year:04}")),
            Some('m') => out.push_str(&format!("{month:02}")),
            Some('d') => out.push_str(&format!("{day:02}")),
            Some('H') => out.push_str(&format!("{hour:02}")),
            Some('M') => out.push_str(&format!("{minute:02}")),
            Some('S') => out.push_str(&format!("{second:02}")),
            Some('%') => out.push('%'),
            Some(other) => return Err(format!("格式里的 %{other} 这里不认（只认 %Y %m %d %H %M %S %%）")),
            None => return Err("格式不能以单独的 % 结尾".to_string()),
        }
    }
    Ok(out)
}

fn time(args: &[String]) -> i32 {
    const USAGE: &str = "用法：cca-test time --utc [--add 秒] --fmt 格式";
    let (mut utc, mut add, mut format) = (false, 0.0f64, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match (arg.as_str(), it.as_slice().first()) {
            ("--utc", _) => utc = true,
            ("--add", Some(value)) => match value.parse::<f64>() {
                Ok(n) if n.is_finite() => (add, _) = (n, it.next()),
                _ => return usage("time", args, &format!("--add 后面要秒数，却是 {value}")),
            },
            ("--fmt", Some(value)) => (format, _) = (Some(value), it.next()),
            _ => return usage("time", args, USAGE),
        }
    }
    let (true, Some(format)) = (utc, format) else { return usage("time", args, USAGE) };
    // time.gmtime(小数)：往下取整到秒
    match strftime_utc(format, (now() + add).floor() as i64) {
        Ok(text) => put(format!("{text}\n").as_bytes()),
        Err(why) => usage("time", args, &why),
    }
}

fn touch_ago(args: &[String]) -> i32 {
    let [path, ago] = args else { return usage("touch-ago", args, "用法：cca-test touch-ago 文件 秒") };
    let Some(ago) = ago.parse::<f64>().ok().filter(|n| n.is_finite()) else {
        return usage("touch-ago", args, &format!("{ago} 不是秒数"));
    };
    let Ok(cpath) = std::ffi::CString::new(path.as_bytes()) else {
        return usage("touch-ago", args, "文件名里不能有 NUL");
    };
    // os.utime(p, (t, t))：小数的时间戳拆成秒和纳秒（往下取整）
    let t = now() - ago;
    let nanos = ((t - t.floor()) * 1e9).floor().min(999_999_999.0);
    // SAFETY: timespec 全是数字，清零就是有效的值
    let mut stamp: libc::timespec = unsafe { std::mem::zeroed() };
    stamp.tv_sec = t.floor() as _;
    stamp.tv_nsec = nanos as libc::c_long;
    let times = [stamp, stamp];
    // SAFETY: cpath 是以 NUL 结尾的字符串，times 是两个 timespec（访问时间、修改时间），调用期间都活着
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), times.as_ptr(), 0) };
    if rc != 0 {
        return raised("touch-ago", format!("改不了 {path} 的时间：{}", std::io::Error::last_os_error()));
    }
    0
}

// ---- 进程的启动时间 ----

const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// 一两位的数字，在 lo..=hi 里
fn small(text: &str, lo: i32, hi: i32) -> Option<i32> {
    let digits = !text.is_empty() && text.len() <= 2 && text.bytes().all(|b| b.is_ascii_digit());
    text.parse::<i32>().ok().filter(|n| digits && (lo..=hi).contains(n))
}

/// ps -o lstart= 的那一行（C locale："Mon Nov 24 19:05:12 2026"，中间的空白多少不论）→ [年, 月, 日, 时, 分, 秒]；
/// 照 time.strptime(" ".join(s.split()), "%a %b %d %H:%M:%S %Y") 的认法，认不出是 None
fn lstart_fields(text: &str) -> Option<[i32; 6]> {
    let fields: Vec<&str> = py::split(text).collect();
    let [weekday, month, day, clock, year] = fields[..] else { return None };
    WEEKDAYS.iter().position(|w| w.eq_ignore_ascii_case(weekday))?;
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month))? as i32 + 1;
    let hms: Vec<&str> = clock.split(':').collect();
    let [hour, minute, second] = hms[..] else { return None };
    if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year = year.parse::<i32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    Some([year, month, small(day, 1, last)?, small(hour, 0, 23)?, small(minute, 0, 59)?, small(second, 0, 61)?])
}

/// time.mktime：本地时间的年月日时分秒 → 时间戳（夏令时让系统自己判断）；换不了是 None
fn local_secs(fields: [i32; 6]) -> Option<i64> {
    let [year, month, day, hour, minute, second] = fields;
    // SAFETY: tm 里是数字和一个不去读的指针（tm_zone），清零就是有效的值
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    (tm.tm_year, tm.tm_mon, tm.tm_mday) = (year - 1900, month - 1, day);
    (tm.tm_hour, tm.tm_min, tm.tm_sec) = (hour, minute, second);
    // -1：夏令时不知道；星期先放 -1，mktime 成了会把它填上（返回 -1 时靠它分清是出错还是正好是 1969 年最后一秒）
    (tm.tm_isdst, tm.tm_wday) = (-1, -1);
    // SAFETY: tm 是这里的局部变量，mktime 只在调用期间读写它
    let secs = unsafe { libc::mktime(&mut tm) };
    (secs != -1 || tm.tm_wday != -1).then_some(secs as i64)
}

fn pstart(args: &[String]) -> i32 {
    let [pid] = args else { return usage("pstart", args, "用法：cca-test pstart 进程号") };
    // 和原来一样：环境照传，只把 LC_ALL 换成 C（月份、星期是英文的）；ps 按名字找
    let out = kit::proc::Cmd::new("ps").env_inherit().env("LC_ALL", "C").args(["-o", "lstart=", "-p", pid.as_str()]).output();
    let text = match out {
        Ok(out) => out.out(),
        Err(e) => return raised("pstart", format!("起不了 ps：{e}")),
    };
    match lstart_fields(&text).and_then(local_secs) {
        Some(secs) => put(format!("{secs}\n").as_bytes()),
        None => raised("pstart", format!("认不出 ps 说的启动时间（没有这个进程？）：{:?}", text.trim())),
    }
}

// ---- 校验、换算 ----

fn utf8(args: &[String]) -> i32 {
    if !args.is_empty() {
        return usage("utf8", args, "不要参数（从 stdin 读）");
    }
    match stdin_bytes() {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(_) => 0,
            Err(e) => raised("utf8", format!("stdin 不是 UTF-8：{e}")),
        },
        Err(e) => raised("utf8", format!("读不了 stdin：{e}")),
    }
}

fn b64(args: &[String]) -> i32 {
    if !args.is_empty() {
        return usage("b64", args, "不要参数（从 stdin 读）");
    }
    match stdin_bytes() {
        Ok(bytes) => put(format!("{}\n", kit::crypto::b64(&bytes)).as_bytes()),
        Err(e) => raised("b64", format!("读不了 stdin：{e}")),
    }
}

/// 属性列表里的值 → JSON 的；日期、二进制数据、UID 在 JSON 里没有对应的写法，是 Err
fn plist_to_json(v: &kit::plist::Value) -> Result<Value, String> {
    use kit::plist::Value as P;
    Ok(match v {
        P::Str(s) => Value::Str(s.clone()),
        P::Int(n) => Value::Int(*n),
        P::Real(f) => Value::Float(*f),
        P::Bool(b) => Value::Bool(*b),
        P::Array(items) => Value::List(items.iter().map(plist_to_json).collect::<Result<_, _>>()?),
        P::Dict(pairs) => {
            let mut obj = Obj::new();
            for (key, value) in pairs {
                obj.insert(key.as_str(), plist_to_json(value)?);
            }
            Value::Obj(obj)
        }
        P::Data(_) => return Err("里面有二进制数据（<data>）".to_string()),
        P::Date(_) => return Err("里面有日期（<date>）".to_string()),
        P::Uid(_) => return Err("里面有 UID".to_string()),
    })
}

fn plist_json(args: &[String]) -> i32 {
    let [path] = args else { return usage("plist-json", args, "用法：cca-test plist-json 文件") };
    let doc = match kit::plist::load_file(std::path::Path::new(path)) {
        Ok(Ok(doc)) => doc,
        Ok(Err(e)) => return raised("plist-json", format!("{path} 不是属性列表：{e}")),
        Err(e) => return raised("plist-json", format!("读不了 {path}：{e}")),
    };
    match plist_to_json(&doc) {
        Ok(value) => put(format!("{}\n", json::dumps(&value, &Style::default())).as_bytes()),
        Err(why) => usage("plist-json", args, &format!("{path} 转不成 JSON：{why}")),
    }
}

// ---- 整段文字的替换 ----

/// text-replace 的一对：原文、新文、原文是不是得正好出现一次
struct Swap<'a> {
    old: &'a str,
    new: &'a str,
    once: bool,
}

/// 依次换；带 once 的那一对原文不是正好出现一次时是 Err（说出现了几次）
fn replace_all(mut text: String, swaps: &[Swap]) -> Result<String, String> {
    for swap in swaps {
        let seen = text.matches(swap.old).count();
        if swap.once && seen != 1 {
            return Err(format!("要正好出现一次的 {:?} 出现了 {seen} 次", swap.old));
        }
        text = text.replace(swap.old, swap.new);
    }
    Ok(text)
}

fn text_replace(args: &[String]) -> i32 {
    const USAGE: &str = "用法：cca-test text-replace 文件 [--once] 原文 新文 [[--once] 原文 新文]…";
    let Some((path, mut rest)) = args.split_first() else { return usage("text-replace", args, USAGE) };
    let mut swaps = Vec::new();
    while !rest.is_empty() {
        let once = rest[0] == "--once";
        let [old, new, more @ ..] = &rest[usize::from(once)..] else { return usage("text-replace", args, USAGE) };
        swaps.push(Swap { old, new, once });
        rest = more;
    }
    if swaps.is_empty() {
        return usage("text-replace", args, USAGE);
    }
    let changed = match read_text(path).and_then(|text| replace_all(text, &swaps)) {
        Ok(changed) => changed,
        Err(why) => return raised("text-replace", why),
    };
    match std::fs::write(path, changed) {
        Ok(()) => 0,
        Err(e) => raised("text-replace", format!("写不了 {path}：{e}")),
    }
}

/// 正则里 \Q…\E 之间的换成转义好的（原样匹配）；没有 \E 就一直到末尾。别的反斜杠连同它后面那个字符原样留着
fn quote_literals(pattern: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(literal) = after.strip_prefix('Q') {
            let (literal, tail) = literal.split_once("\\E").unwrap_or((literal, ""));
            out.push_str(&regex::escape(literal));
            rest = tail;
        } else {
            let escaped = after.chars().next().map_or(0, char::len_utf8);
            out.push_str(&rest[at..at + 1 + escaped]);
            rest = &after[escaped..];
        }
    }
    out.push_str(rest);
    out
}

/// 头一处匹配的换成 new（原样）；正则认不了是 Err(Err(说明))，换完没变是 Err(Ok(()))
fn sub_first(text: &str, pattern: &str, new: &str) -> Result<String, Result<(), String>> {
    let rx = regex::Regex::new(&quote_literals(pattern)).map_err(|e| Err(format!("正则认不了：{e}")))?;
    let changed = rx.replacen(text, 1, regex::NoExpand(new));
    if changed == text {
        return Err(Ok(()));
    }
    Ok(changed.into_owned())
}

fn text_sub(args: &[String]) -> i32 {
    let [path, pattern, new] = args else { return usage("text-sub", args, "用法：cca-test text-sub 文件 正则 新文") };
    let text = match read_text(path) {
        Ok(text) => text,
        Err(why) => return raised("text-sub", why),
    };
    let changed = match sub_first(&text, pattern, new) {
        Ok(changed) => changed,
        Err(Ok(())) => return raised("text-sub", format!("{path} 换完和原来一样（没有匹配的，或者本来就是这样）")),
        Err(Err(why)) => return usage("text-sub", args, &why),
    };
    match std::fs::write(path, changed) {
        Ok(()) => 0,
        Err(e) => raised("text-sub", format!("写不了 {path}：{e}")),
    }
}

// ---- ckof ----

/// 节点的一个字段写成文字：没有、null 是空的，true / false 是小写的，数照 Python 的 str()；数组、对象这里不认（None）
fn ck_text(v: Option<&Value>) -> Option<String> {
    Some(match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Str(s)) => s.clone(),
        Some(Value::Int(n)) => n.to_string(),
        Some(Value::Float(f)) => json::float_repr(*f),
        Some(Value::List(_) | Value::Obj(_)) => return None,
    })
}

/// 自定义节点的身份摘要：类型、地址、端口、SNI（servername，没有就是 sni）、用户名、密码、uuid 用 | 连起来，
/// 做 64 位的 FNV-1a，写成 16 位十六进制。类型、地址、SNI 不分大小写，地址和 SNI 末尾的点不算。
/// 节点不是对象、哪个字段是数组或者对象：None
fn ckof(node: &Value) -> Option<String> {
    let node = node.as_obj()?;
    let host = |v: Option<&Value>| Some(py::strip(&ck_text(v)?).to_lowercase().trim_end_matches('.').to_string());
    let sni = match node.get("servername") {
        None | Some(Value::Null | Value::Bool(false)) => node.get("sni"),
        named => named,
    };
    let mut key = vec![
        py::strip(&ck_text(node.get("type"))?).to_lowercase(),
        host(node.get("server"))?,
        py::strip(&ck_text(node.get("port"))?).to_string(),
        host(sni)?,
    ];
    for field in ["username", "password", "uuid"] {
        key.push(ck_text(node.get(field))?);
    }
    let hash = key.join("|").bytes().fold(0xcbf2_9ce4_8422_2325u64, |x, b| (x ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    Some(format!("{hash:016x}"))
}

fn ckof_cmd(args: &[String]) -> i32 {
    let [text] = args else { return usage("ckof", args, "用法：cca-test ckof 节点的JSON") };
    let node = match json::parse(text) {
        Ok(node) => node,
        Err(e) => return raised("ckof", format!("不是 JSON：{e}")),
    };
    match ckof(&node) {
        Some(ck) => put(format!("{ck}\n").as_bytes()),
        None => usage("ckof", args, "节点得是一个对象，字段不能是数组、对象"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kit::fsx::TempDir;

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 在临时目录里放一个文件，返回目录（拿着它文件才在）和文件的路径
    fn file_with(text: &str) -> (TempDir, String) {
        let dir = TempDir::new("util-unit-");
        let path = dir.join("f.txt").to_str().unwrap().to_string();
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn split_and_keeps_quoted_parts_whole() {
        assert_eq!(split_and("a && b"), ["a", "b"]);
        assert_eq!(
            split_and("[ $rc -eq 0 ] && grep -q 'x && y' f &&  cfgok \"a \\\" && b\"  "),
            ["[ $rc -eq 0 ]", "grep -q 'x && y' f", "cfgok \"a \\\" && b\""]
        );
        // 没有空格的 && 不拆；空的段也算一段；引号没关上就一直到末尾
        assert_eq!(split_and("a&&b && c"), ["a&&b", "c"]);
        assert_eq!(split_and("x &&  && y"), ["x", "", "y"]);
        assert_eq!(split_and(" && "), ["", ""]);
        assert_eq!(split_and("'open && x"), ["'open && x"]);
        assert_eq!(split_and(""), [""]);
        // 单引号里的反斜杠不带着后面那个字符走
        assert_eq!(split_and("'a\\' && b"), ["'a\\'", "b"]);
        // 两头的空白照 Python 的 strip()：全角空格、U+001C 也去掉
        assert_eq!(split_and("\u{3000}a\u{1c} && \u{1f}b "), ["a", "b"]);
    }

    #[test]
    fn width_like_python() {
        assert_eq!(max_width(""), None);
        assert_eq!(max_width("\n"), Some(0));
        assert_eq!(max_width("abc\n中文ab\nabcde"), Some(6));
        assert_eq!(max_width("😀😀\nabc"), Some(4));
        assert_eq!(universal_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn utc_time_like_strftime_gmtime() {
        // 期望值是 time.strftime("%Y-%m-%dT%H:%M:%SZ %%", time.gmtime(t)) 算的
        let cases: &[(i64, &str)] = &[
            (0, "1970-01-01T00:00:00Z %"),
            (1_764_000_000, "2025-11-24T16:00:00Z %"),
            (1_709_251_199, "2024-02-29T23:59:59Z %"),
            (951_782_400, "2000-02-29T00:00:00Z %"),
            (-1, "1969-12-31T23:59:59Z %"),
            (4_102_444_800, "2100-01-01T00:00:00Z %"),
            (1_740_787_199, "2025-02-28T23:59:59Z %"),
        ];
        for (secs, want) in cases {
            assert_eq!(strftime_utc("%Y-%m-%dT%H:%M:%SZ %%", *secs).as_deref(), Ok(*want), "{secs}");
        }
        assert!(strftime_utc("%Z", 0).is_err());
        assert!(strftime_utc("100%", 0).is_err());
        assert_eq!(time(&strs(&["--fmt", "%Y"])), 2, "不带 --utc 不认");
        assert_eq!(time(&strs(&["--utc", "--add", "x", "--fmt", "%Y"])), 2);
        assert_eq!(time(&strs(&["--utc", "--fmt", "%j"])), 2);
    }

    #[test]
    fn lstart_line_like_strptime() {
        assert_eq!(lstart_fields("Mon Nov 24 19:05:12 2026\n"), Some([2026, 11, 24, 19, 5, 12]));
        assert_eq!(lstart_fields("  tue  FEB  9 03:04:05 2024 "), Some([2024, 2, 9, 3, 4, 5]));
        assert_eq!(lstart_fields("Thu Feb 29 23:59:60 2024"), Some([2024, 2, 29, 23, 59, 60]));
        for bad in [
            "",
            "Nov 24 19:05:12 2026",
            "Mon Nov 24 19:05:12 2026 x",
            "Xyz Nov 24 19:05:12 2026",
            "Mon Nvo 24 19:05:12 2026",
            "Mon Nov 31 19:05:12 2026",
            "Mon Feb 29 19:05:12 2026",
            "Mon Nov 24 24:05:12 2026",
            "Mon Nov 24 19:60:12 2026",
            "Mon Nov 24 19:05 2026",
            "Mon Nov 24 19:05:12 26",
            "Mon Nov 024 19:05:12 2026",
            "Mon Nov 0 19:05:12 2026",
        ] {
            assert_eq!(lstart_fields(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn local_time_round_trips_through_mktime() {
        // 现在这一刻照本地时间拆开，再换回来还是它（夏令时交界的那一小时除外，这里用的是现在）
        // time_t 在这里要编的几种机器上都是 64 位的
        let stamp = now().floor() as i64;
        // SAFETY: tm 清零就是有效的值；localtime_r 只在调用期间读 stamp、写 tm
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            assert!(!libc::localtime_r(&stamp, &mut tm).is_null());
            tm
        };
        let fields = [tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec];
        assert_eq!(local_secs(fields), Some(stamp));
    }

    #[test]
    fn pstart_of_this_process_is_recent_past() {
        let pid = std::process::id().to_string();
        let out = kit::proc::Cmd::new("ps")
            .env_inherit()
            .env("LC_ALL", "C")
            .args(["-o", "lstart=", "-p", pid.as_str()])
            .output()
            .unwrap();
        let started = lstart_fields(&out.out()).and_then(local_secs).expect("ps 说的启动时间认得出");
        let age = now() - started as f64;
        assert!((-1.0..3600.0).contains(&age), "启动时间离现在 {age} 秒");
        assert_eq!(pstart(&strs(&["999999"])), 1, "没有这个进程");
        assert_eq!(pstart(&[]), 2);
    }

    #[test]
    fn touch_ago_moves_both_times_back() {
        let (_dir, path) = file_with("x");
        assert_eq!(touch_ago(&strs(&[&path, "600"])), 0);
        let meta = std::fs::metadata(&path).unwrap();
        for stamp in [meta.modified().unwrap(), meta.accessed().unwrap()] {
            let ago = stamp.elapsed().unwrap().as_secs_f64();
            assert!((599.0..660.0).contains(&ago), "拨到了 {ago} 秒以前");
        }
        assert_eq!(touch_ago(&strs(&[&format!("{path}.nope"), "60"])), 1, "文件不在");
        assert_eq!(touch_ago(&strs(&[&path, "soon"])), 2);
        assert_eq!(touch_ago(&strs(&[&path])), 2);
    }

    #[test]
    fn hold_keeps_the_port_until_done() {
        let dir = TempDir::new("util-unit-");
        let ready = dir.join("ready").to_str().unwrap().to_string();
        let port = kit::fsx::free_port();
        let holder = {
            let ready = ready.clone();
            std::thread::spawn(move || hold(port, &ready, std::time::Duration::from_millis(400)))
        };
        kit::fsx::wait_for("占上端口", std::time::Duration::from_secs(10), || std::path::Path::new(&ready).exists());
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err(), "占着的时候别人绑不上");
        assert!(hold(port, &format!("{ready}.2"), std::time::Duration::ZERO).is_err(), "再占一次占不上");
        assert!(!std::path::Path::new(&format!("{ready}.2")).exists(), "没占上就不建就绪文件");
        assert_eq!(holder.join().unwrap(), Ok(()));
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "放开以后绑得上");
        assert_eq!(hold_port(&strs(&["x", &ready])), 2);
    }

    #[test]
    fn replace_pairs_in_order_and_once_means_exactly_once() {
        let once = |old, new| Swap { old, new, once: true };
        let all = |old, new| Swap { old, new, once: false };
        assert_eq!(replace_all("a b a".to_string(), &[all("a", "c")]).as_deref(), Ok("c b c"));
        assert_eq!(replace_all("a b".to_string(), &[all("x", "y")]).as_deref(), Ok("a b"));
        assert_eq!(replace_all("a b".to_string(), &[once("a", "b"), all("b", "c")]).as_deref(), Ok("c c"));
        assert!(replace_all("a a".to_string(), &[once("a", "b")]).is_err());
        assert!(replace_all("b".to_string(), &[once("a", "b")]).is_err());
        // 不重叠地数（和 str.count 一样）
        assert_eq!(replace_all("aaa".to_string(), &[once("aa", "b")]).as_deref(), Ok("ba"));
    }

    #[test]
    fn text_replace_rewrites_the_file_or_leaves_it() {
        let block = "  - name: OpenAi\n    type: select\n    proxies:\n      - US-A\n";
        let flow = "  - {name: OpenAi, type: select, proxies: [US-A]}\n";
        let (_dir, path) = file_with(&format!("proxy-groups:\r\n{block}rules: []\n"));
        assert_eq!(text_replace(&strs(&[&path, "--once", block, flow])), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("proxy-groups:\n{flow}rules: []\n"), "\\r\\n 读成 \\n");
        // 再换一次：原文已经不在了，退出码 1，文件不动
        assert_eq!(text_replace(&strs(&[&path, "--once", block, flow])), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("proxy-groups:\n{flow}rules: []\n"));
        // 两对：前一对正好一次，后一对有几处换几处
        assert_eq!(text_replace(&strs(&[&path, "--once", "rules", "RULES", "o", "0"])), 0);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "pr0xy-gr0ups:\n  - {name: OpenAi, type: select, pr0xies: [US-A]}\nRULES: []\n"
        );
        assert_eq!(text_replace(&strs(&[&format!("{path}.nope"), "a", "b"])), 1);
        assert_eq!(text_replace(&strs(&[&path])), 2);
        assert_eq!(text_replace(&strs(&[&path, "a"])), 2);
        assert_eq!(text_replace(&strs(&[&path, "--once", "a"])), 2);
        std::fs::write(&path, b"\xff").unwrap();
        assert_eq!(text_replace(&strs(&[&path, "a", "b"])), 1, "不是 UTF-8");
    }

    #[test]
    fn literal_spans_are_escaped() {
        assert_eq!(quote_literals(r"a\Qb.c\Ed"), r"ab\.cd");
        assert_eq!(quote_literals(r"\Q(x)"), r"\(x\)");
        assert_eq!(quote_literals(r"a\\Qb\nc"), r"a\\Qb\nc", "转义了的反斜杠后面的 Q 不算");
        assert_eq!(quote_literals(r"\Q\E\Q+\E+"), r"\++");
        assert_eq!(quote_literals("no escapes"), "no escapes");
        assert_eq!(quote_literals("tail\\"), "tail\\");
    }

    #[test]
    fn sub_replaces_the_first_match_literally() {
        // 回归的 one_line：块写法的节点换回一行的写法
        let pattern = |indent: &str, name: &str| format!("(?m)^{indent}- name: \\Q{name}\\E # cca\\n(?:{indent}  .*\\n)*");
        let text = "proxies:\n  - name: A.B # cca\n    type: ss\n    port: 1\n  - name: AxB # cca\n    type: ss\n";
        assert_eq!(
            sub_first(text, &pattern("  ", "A.B"), "  - {name: A.B, note: '$1 \\1'} # cca\n").as_deref(),
            Ok("proxies:\n  - {name: A.B, note: '$1 \\1'} # cca\n  - name: AxB # cca\n    type: ss\n")
        );
        // 只换头一处
        assert_eq!(sub_first("a a a", "a", "b").as_deref(), Ok("b a a"));
        assert_eq!(sub_first(text, &pattern("  ", "Nope"), "x"), Err(Ok(())), "没有匹配的");
        assert_eq!(sub_first("same", "same", "same"), Err(Ok(())), "换完和原来一样");
        assert!(matches!(sub_first("x", "(", "y"), Err(Err(_))), "正则认不了");
    }

    #[test]
    fn text_sub_rewrites_the_file_or_leaves_it() {
        let (_dir, path) = file_with("- name: N # cca\n  a: 1\n  b: 2\n- name: M\n");
        assert_eq!(text_sub(&strs(&[&path, "(?m)^- name: \\QN\\E # cca\\n(?:  .*\\n)*", "- {name: N} # cca\n"])), 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- {name: N} # cca\n- name: M\n");
        assert_eq!(text_sub(&strs(&[&path, "(?m)^- name: \\QN\\E # cca\\n(?:  .*\\n)*", "- {name: N} # cca\n"])), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- {name: N} # cca\n- name: M\n");
        assert_eq!(text_sub(&strs(&[&path, "(", "x"])), 2);
        assert_eq!(text_sub(&strs(&[&path, "x"])), 2);
        assert_eq!(text_sub(&strs(&[&format!("{path}.nope"), "a", "b"])), 1);
    }

    #[test]
    fn ck_like_the_python_it_replaces() {
        // 期望值是回归里原来那段 Python（ckof）算的
        let cases: &[(&str, &str)] = &[
            (
                r#"{"type": "socks5", "server": "198.51.100.7", "port": 1080, "username": "u@x", "password": "p:w"}"#,
                "ec3098e50d14aa14",
            ),
            (r#"{"type": " SS ", "server": "Example.COM.", "port": "8388 ", "password": "x"}"#, "cc72a3f6c5a0a729"),
            (
                r#"{"type": "vless", "server": "192.0.2.22", "port": 443, "uuid": "00000000-0000-4000-8000-000000000001", "servername": "WWW.Example.com.."}"#,
                "10c3293bb3508174",
            ),
            (
                r#"{"type": "trojan", "server": "h", "port": 443, "servername": false, "sni": "Sni.Example.Com."}"#,
                "e1d1236203500c0d",
            ),
            (r#"{"type": "trojan", "server": "h", "port": 443, "servername": null, "sni": "a"}"#, "ed550e963943e28d"),
            (r#"{"type": "trojan", "server": "h", "port": 443, "servername": "", "sni": "a"}"#, "f4ed1f87045878fc"),
            (r#"{"type": "http", "server": "h", "port": 8080.5, "username": true, "password": false}"#, "956301deadd44bf3"),
            ("{}", "263177021cd327a5"),
        ];
        for (node, want) in cases {
            assert_eq!(ckof(&json::parse(node).unwrap()).as_deref(), Some(*want), "{node}");
        }
        assert_eq!(ckof(&json::parse("[1]").unwrap()), None);
        assert_eq!(ckof(&json::parse(r#"{"server": ["h"]}"#).unwrap()), None);
        assert_eq!(ckof_cmd(&strs(&["not json"])), 1);
        assert_eq!(ckof_cmd(&strs(&["[1]"])), 2);
        assert_eq!(ckof_cmd(&[]), 2);
    }

    #[test]
    fn plist_values_become_json() {
        use kit::plist::Value as P;
        let doc = P::Dict(vec![
            (
                "KeepAlive".to_string(),
                P::Dict(vec![("PathState".to_string(), P::Dict(vec![("/tmp/x y".to_string(), P::Bool(true))]))]),
            ),
            ("ProgramArguments".to_string(), P::Array(vec![P::Str("/a/cca-udp".to_string())])),
            ("N".to_string(), P::Int(5)),
            ("F".to_string(), P::Real(1.5)),
        ]);
        assert_eq!(
            json::dumps(&plist_to_json(&doc).unwrap(), &Style::default()),
            r#"{"KeepAlive": {"PathState": {"/tmp/x y": true}}, "ProgramArguments": ["/a/cca-udp"], "N": 5, "F": 1.5}"#
        );
        assert!(plist_to_json(&P::Array(vec![P::Data(vec![1])])).is_err());
        assert!(plist_to_json(&P::Uid(1)).is_err());
        // 文件：XML 的读得出；不在、不是属性列表是 1
        let xml = kit::plist::dumps_xml(&doc).unwrap();
        let dir = TempDir::new("util-unit-");
        let path = dir.join("p.plist").to_str().unwrap().to_string();
        std::fs::write(&path, xml).unwrap();
        assert_eq!(plist_json(&strs(&[&path])), 0);
        assert_eq!(plist_json(&strs(&[&format!("{path}.nope")])), 1);
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(plist_json(&strs(&[&path])), 1);
        assert_eq!(plist_json(&[]), 2);
    }

    #[test]
    fn usage_errors_are_recorded() {
        // 不设 CCA_TEST_TRAP 也照样是退出码 2（记不记由环境定，这里只看退出码）
        assert_eq!(split_and_cmd(&[]), 2);
        assert_eq!(port(&strs(&["x"])), 2);
        assert_eq!(maxw(&strs(&["x"])), 2);
        assert_eq!(utf8(&strs(&["x"])), 2);
        assert_eq!(b64(&strs(&["x"])), 2);
    }
}
