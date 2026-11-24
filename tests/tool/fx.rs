//! 现造的测试数据（`cca-test fx 名字 参数…`）：regress.sh 里原来用一小段 Python 现写的夹具当中，求值器（check / val / run / edit）
//! 写不了的那几样——要算摘要、排时间的格式、解 base64、一次改几个文件、照 shlex 拆命令行。求值器写得了的都留在 regress.sh 里
//! （数据就在用例旁边，看得见）；这里的每一个都照原来那段 Python 来：写出来的文件逐字节一样（JSON 的分隔、转不转义非 ASCII、
//! 结尾没有换行）、打印的一样、退出码一样、出错时停在同一步（前面已经写了的文件照样留着）。
//!
//! ```text
//! sha1 文字                      打出文字（UTF-8）的 SHA-1，40 位十六进制，一行。原来是 hashlib.sha1(…).hexdigest()：
//!                                账号目录名结尾的那 6 位就是邮箱的 SHA-1 的开头（1 那一节的 accounts.golden）
//! hookless ARGS行 账号目录名      假 claude 记下的 “ARGS: …” 那一行里，--settings 后面是不是正好只有记下会话的 hook
//!                                （{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "… _session-start 目录名"}]}]}}：
//!                                三层都没有别的键、别的项；command 照 shlex 拆开，第一段（cca 的路径）不看）。是：打出去掉
//!                                “ --settings {…}” 的那一行；不是、或者这一行根本没有 --settings：打出 BAD。退出码都是 0。
//!                                什么都不写。（10c 的 hookless）
//! claude-usage 文件 5小时% 本周% 订阅 额外的窗口
//!                                写假 claude 给一个账号的 get_usage 回应（10d 的 canned）：
//!                                {"subscription_type": 订阅, "rate_limits_available": true, "rate_limits": {"five_hour": {"utilization":
//!                                5小时%, "resets_at": 2 小时 13 分 30 秒以后}, "seven_day": {"utilization": 本周%, "resets_at": 3 天 5 小时
//!                                以后}, "seven_day_opus": null, "seven_day_sonnet": null}}，两个百分数写成小数（13 → 13.0），时刻是
//!                                UTC 的 “2026-11-24T05:56:51.123456+00:00” 这个样子；“额外的窗口”是一个 JSON 对象（可以是空串），
//!                                并进 rate_limits（同名的换掉、位置不动，新的接在后面）。五个参数都要给
//! text-prepend 文件 一行         像客户端“原子保存”那样在文件开头加一行：读出原文，“一行\n” + 原文 写进 文件.test-new，再改名换上
//!                                （文件是符号链接的话，链接换成普通文件；新文件的权限是新建文件默认的）。（10f 的 config.toml）
//! costlog 家目录 写法…           往共享的会话记录里写折合金额要读的那几份（10g 的 costlog）。先建 家目录/.claude/projects/-cost，
//!                                然后照写法（可以给几个，次序不论，总是 init → more → dave）：
//!                                init：s-cost-1.jsonl（session-owner 是 u-alice-0001 / org-1；一句 2 天前的提问 SECRET-PROMPT-TEXT；
//!                                      回复 mc1：2 天前 claude-opus-5-5 进 1000 出 100；mc2：1 天前 claude-sonnet-5-5[1m] 进 2000 出 1000，
//!                                      同一条写了两行；mc3：1 小时前 claude-new-9 进 10 出 10）、s-cost-0.jsonl（没有 session-owner，
//!                                      回复 mc4：3 小时前 claude-opus-5-5 进 1000 出 100）、
//!                                      家目录/.codex/sessions/2026/11/12/rollout-cost.jsonl（session_meta：th-cost / user-dave-0001 / openai，
//!                                      turn_context：gpt-6-astra，两条一样的 token_count：进 10000（缓存 8000）出 500）；都是整个重写
//!                                more：s-cost-1.jsonl 后面追加一条回复 mc5：10 分钟前 claude-opus-5-5 进 1000 出 100
//!                                dave：最后一个参数是 Codex 的 auth.json：tokens.id_token 中间那一段（base64url 的 JSON）里
//!                                      ["https://api.openai.com/auth"]["chatgpt_user_id"] 设成 user-dave-0001，写回去
//! router-occupy 账号表 owners 账号名 设备 节点 ck [custom]
//!                                多设备共用一个路由器：扮演另一台设备（owners 里的值是 设备），直接改路由器上的 accounts.yaml 和 owners，
//!                                让它的账号「账号名」占第一个空槽位（1 起），打出槽位号。先去掉账号表里原来叫这个名字的；新的一项是
//!                                {"slot", "name", "node"[, "ck"]}（ck 是空串就不带），slots 不够大就调到这个槽位号；owners 里这台设备
//!                                原来占的都去掉，再记上这一个。custom 给了的话：是 - 就去掉账号表的 custom，别的当一个节点的 JSON，
//!                                custom 换成 {"id": "other", "proxies": [它]}。（11b2c 的 mdslot）
//! ```
//!
//! 退出码：0 是做完了；1 是原来的 Python 也会出的异常（少了参数、文件读不了、JSON 写错了、缺键……）：stderr 上一行，和 Python
//! 的 traceback 最后那一行一个意思；2 是这里不敢担保的（没有这个名字、Python 认而这里不认的输入），stderr 上以 “cca-test：”
//! 开头，另在 $CCA_TEST_TRAP 里记一行——回归结尾查这个文件是空的。
//!
//! 环境变量 CCA_TEST_NOW=秒数（可以带小数）：把“现在”钉在这个时刻。只给对拍和单元测试用，回归里不设。
//!
//! lint.rs 也用这里的 Stop / finish / read_text 这几样。

use crate::fake::{self, repr_str, type_name};
use kit::json::{self, Obj, Style, Value};
use kit::obj;
use std::io::Write;

pub const CMDS: &[(&str, crate::Run)] = &[("fx", run)];

/// 一个 fx：收下名字后面的参数；要打印的攒在 out 里（Python 往管道上 print 也是攒到结束才写出去）
pub type Sub = fn(&[String], &mut String) -> R<i32>;

const SUBS: &[(&str, Sub)] = &[
    ("sha1", sha1),
    ("hookless", hookless),
    ("claude-usage", claude_usage),
    ("text-prepend", text_prepend),
    ("costlog", costlog),
    ("router-occupy", router_occupy),
];

fn run(args: &[String]) -> i32 {
    dispatch("fx", SUBS, args)
}

// ---- 和 lint.rs 共用的：怎么停下、怎么收尾 ----

/// 停下来的两种
#[derive(Debug)]
pub enum Stop {
    /// 原来的 Python 也会出的异常：stderr 上说这一行，退出码 1
    Raise(String),
    /// 这里不敢担保的（用法不对、Python 认而这里不认的输入）：退出码 2，记进 $CCA_TEST_TRAP
    Unsupported(String),
}
pub type R<T> = Result<T, Stop>;

impl From<fake::Fail> for Stop {
    fn from(f: fake::Fail) -> Stop {
        Stop::Raise(f.0)
    }
}

pub fn raise<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop::Raise(msg.into()))
}

pub fn unsupported<T>(msg: impl Into<String>) -> R<T> {
    Err(Stop::Unsupported(msg.into()))
}

/// 按名字找到那一个、跑完、收尾（group 是 fx 或者 lint）
pub fn dispatch(group: &str, subs: &[(&str, Sub)], args: &[String]) -> i32 {
    let mut out = String::new();
    let r = match args.split_first() {
        None => unsupported(format!("用法：cca-test {group} 名字 参数…")),
        Some((name, rest)) => match subs.iter().find(|(n, _)| n == name) {
            Some((_, sub)) => sub(rest, &mut out),
            None => unsupported(format!("没有 {name} 这个名字")),
        },
    };
    finish(group, args, r, &out)
}

/// 收尾：攒着的先写出去（出了异常，之前 print 的也照样出去，和 Python 一样），再定退出码。
/// 写不出去（读的那一头关了）：Python 这时的退出码是 120
pub fn finish(group: &str, args: &[String], r: R<i32>, out: &str) -> i32 {
    let mut so = std::io::stdout().lock();
    let wrote = out.is_empty() || so.write_all(out.as_bytes()).and_then(|()| so.flush()).is_ok();
    match r {
        Ok(code) if wrote => code,
        Ok(_) => 120,
        Err(Stop::Raise(msg)) => {
            eprintln!("{msg}");
            1
        }
        Err(Stop::Unsupported(msg)) => {
            eprintln!("cca-test：{group}：{msg}");
            write_trap(&format!("{group}\t{msg}\t{}", args.join(" ")));
            2
        }
    }
}

/// 往 $CCA_TEST_TRAP 追加一行（换行写成 \n，一次 write 写完：几个进程同时写也不会串行）；没设、写不进去都不管
fn write_trap(line: &str) {
    let Some(path) = std::env::var_os("CCA_TEST_TRAP").filter(|p| !p.is_empty()) else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let line = format!("{}\n", line.replace('\n', "\\n").replace('\r', "\\r"));
        let _ = f.write(line.as_bytes());
    }
}

/// sys.argv[i + 1]：没有就是 IndexError
pub fn arg(args: &[String], i: usize) -> R<&str> {
    match args.get(i) {
        Some(a) => Ok(a),
        None => raise("IndexError: list index out of range"),
    }
}

/// open(p, encoding='utf-8').read()：换行统一成 \n
pub fn read_text(path: &str) -> R<String> {
    Ok(fake::read_text(path)?)
}

// ---- 时间 ----

#[cfg(test)]
thread_local! {
    /// 单元测试里把这个线程的“现在”钉住（环境变量是整个进程的，几个测试同时跑时不能用）
    static PINNED: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
}

/// time.time()：CCA_TEST_NOW 设了就是它。平时照 Python 的算法从纳秒数除出来（两边在同一个纳秒上给出同一个小数）
fn now() -> R<f64> {
    #[cfg(test)]
    if let Some(t) = PINNED.with(std::cell::Cell::get) {
        return Ok(t);
    }
    match std::env::var("CCA_TEST_NOW") {
        Ok(v) if !v.is_empty() => match v.parse::<f64>() {
            Ok(t) if t.is_finite() => Ok(t),
            _ => unsupported(format!("CCA_TEST_NOW 不是秒数：{v}")),
        },
        _ => {
            let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            Ok(d.as_nanos() as f64 / 1e9)
        }
    }
}

/// time.strftime('%Y-%m-%dT%H:%M:%S', time.gmtime(t))：t 往下取整到秒
fn utc(t: f64) -> String {
    let d = kit::plist::date_from_unix(t.floor() as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute, d.second)
}

// ---- JSON（照 Python 的 json 和 dict） ----

/// json.loads：写错了的是 Python 也会报的 JSONDecodeError；Python 读得了、这里读不了的（整数太大、嵌套太深、单独的代理项）不敢担保
fn loads(text: &str, what: &str) -> R<Value> {
    json::parse(text).map_err(|e| {
        if e.unsupported() {
            Stop::Unsupported(format!("{what}：{e}"))
        } else {
            Stop::Raise(format!("json.decoder.JSONDecodeError: {e}"))
        }
    })
}

/// json.load(open(path))
fn load(path: &str) -> R<Value> {
    loads(&read_text(path)?, path)
}

/// json.dump(v, open(path, 'w'))：一行，", " 和 ": "，非 ASCII 转义，结尾没有换行
fn dump(path: &str, v: &Value) -> R<()> {
    Ok(fake::write_text(path, &json::dumps(v, &Style::default()))?)
}

/// d[k]
fn item<'a>(d: &'a Value, k: &str) -> R<&'a Value> {
    Ok(fake::item(d, k)?)
}

/// d[k]，拿来改
fn item_mut<'a>(d: &'a mut Value, k: &str) -> R<&'a mut Value> {
    match d {
        Value::Obj(o) => match o.get_mut(k) {
            Some(v) => Ok(v),
            None => raise(format!("KeyError: {}", repr_str(k))),
        },
        other => raise(format!("TypeError: '{}' object is not subscriptable", type_name(other))),
    }
}

/// float(s)：两头的空白不算，数字之间可以夹一个下划线；认不出是 ValueError。全角数字这些 Python 认、这里不认的：不敢担保
fn py_float(s: &str) -> R<f64> {
    if !s.is_ascii() {
        return unsupported(format!("float() 的参数里有非 ASCII 字符：{s}"));
    }
    let bad = || raise(format!("ValueError: could not convert string to float: {}", repr_str(s)));
    let t = s.trim_matches(|c: char| fake::is_py_space(c));
    let b = t.as_bytes();
    let mut plain = String::with_capacity(t.len());
    for (i, c) in t.char_indices() {
        if c != '_' {
            plain.push(c);
        } else if !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            return bad();
        }
    }
    match plain.parse::<f64>() {
        Ok(f) => Ok(f),
        Err(_) => bad(),
    }
}

/// d.update(other)，other 是 json.loads 出来的随便什么：字典并进去；别的照 Python 的“一对一对”来
/// （列表里一项是两个元素的列表、两个字符的字符串、两个键的字典），不成对的是 Python 也会出的异常
fn py_update(d: &mut Obj, other: &Value) -> R<()> {
    let not_iterable = |v: &Value| raise(format!("TypeError: '{}' object is not iterable", type_name(v)));
    let pairs: Vec<&Value> = match other {
        Value::Obj(o) => {
            for (k, v) in o.iter() {
                d.insert(k, v.clone());
            }
            return Ok(());
        }
        Value::List(l) => l.iter().collect(),
        Value::Str(s) if s.is_empty() => Vec::new(),
        Value::Str(_) => return raise("ValueError: dictionary update sequence element #0 has length 1; 2 is required"),
        v => return not_iterable(v),
    };
    for (i, pair) in pairs.into_iter().enumerate() {
        let wrong = |n: usize| raise(format!("ValueError: dictionary update sequence element #{i} has length {n}; 2 is required"));
        let (k, v) = match pair {
            Value::List(p) => match p.as_slice() {
                [k, v] => (k.clone(), v.clone()),
                _ => return wrong(p.len()),
            },
            Value::Str(s) => {
                let cs: Vec<char> = s.chars().collect();
                match cs.as_slice() {
                    [k, v] => (Value::Str(k.to_string()), Value::Str(v.to_string())),
                    _ => return wrong(cs.len()),
                }
            }
            Value::Obj(o) => {
                let ks: Vec<&str> = o.keys().collect();
                match ks.as_slice() {
                    [k, v] => (Value::from(*k), Value::from(*v)),
                    _ => return wrong(ks.len()),
                }
            }
            _ => return raise(format!("TypeError: cannot convert dictionary update sequence element #{i} to a sequence")),
        };
        match k {
            Value::Str(k) => {
                d.insert(k, v);
            }
            Value::List(_) | Value::Obj(_) => {
                return raise(format!(
                    "TypeError: cannot use '{}' as a dict key (unhashable type: '{}')",
                    type_name(&k),
                    type_name(&k)
                ))
            }
            // Python 里数字、True、None 也能当键（写成 JSON 时再变成字符串，1 和 True 还是同一个键）：这里的对象只有字符串的键
            _ => return unsupported("dict.update 的一对里，键不是字符串"),
        }
    }
    Ok(())
}

// ---- sha1 ----

fn sha1(a: &[String], out: &mut String) -> R<i32> {
    let text = arg(a, 0)?;
    out.push_str(&kit::crypto::sha1_hex(text.as_bytes()));
    out.push('\n');
    Ok(0)
}

// ---- hookless ----

/// 原来 try 里的那几步出的岔子，分三种
enum Hook {
    /// except (ValueError, KeyError, IndexError, TypeError) 接住的：算不是，打 BAD
    Caught,
    /// 没人接的异常（command 是数、列表、字典时 shlex 报的 AttributeError）：什么都不打，退出码 1
    Uncaught(String),
    /// 这里不敢担保的
    Unsupported(String),
}

/// v[k]（k 是字符串）：不是字典、没有这个键，Python 报的 TypeError / KeyError 都被接住
fn hook_key<'a>(v: &'a Value, k: &str) -> Result<&'a Value, Hook> {
    match v {
        Value::Obj(o) => o.get(k).ok_or(Hook::Caught),
        _ => Err(Hook::Caught),
    }
}

/// v[0]：列表的第一项、字符串的第一个字符；空的（IndexError）、字典（KeyError: 0）、别的（TypeError）都被接住
fn hook_first(v: &Value) -> Result<Value, Hook> {
    match v {
        Value::List(l) => l.first().cloned().ok_or(Hook::Caught),
        Value::Str(s) => s.chars().next().map(|c| Value::Str(c.to_string())).ok_or(Hook::Caught),
        _ => Err(Hook::Caught),
    }
}

/// shlex.split(s)（posix，只按空白拆，不认注释）：引号没合上、反斜杠后面没有字符是 ValueError
fn shlex_split(s: &str) -> Result<Vec<String>, &'static str> {
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Space,
        Word,
        Quote(char),
        Escape,
    }
    let mut tokens = Vec::new();
    let mut chars = s.chars();
    let mut state = Some(St::Space);
    // 一轮读出一段（read_token）；读到头、手里又没有东西就是结束
    loop {
        let mut token = String::new();
        let mut quoted = false;
        // 转义完了回到哪个状态（引号里的反斜杠回引号里，外面的回到词里）
        let mut back = St::Space;
        let done = loop {
            let c = chars.next();
            let Some(st) = state else { break true };
            match st {
                St::Space => match c {
                    None => {
                        state = None;
                        break false;
                    }
                    Some(' ' | '\t' | '\r' | '\n') => {
                        if !token.is_empty() || quoted {
                            break false;
                        }
                    }
                    Some('\\') => {
                        back = St::Word;
                        state = Some(St::Escape);
                    }
                    Some(q @ ('\'' | '"')) => state = Some(St::Quote(q)),
                    Some(c) => {
                        token.push(c);
                        state = Some(St::Word);
                    }
                },
                St::Quote(q) => {
                    quoted = true;
                    match c {
                        None => return Err("No closing quotation"),
                        Some(c) if c == q => state = Some(St::Word),
                        // 只有双引号里的反斜杠是转义
                        Some('\\') if q == '"' => {
                            back = st;
                            state = Some(St::Escape);
                        }
                        Some(c) => token.push(c),
                    }
                }
                St::Escape => {
                    let Some(c) = c else { return Err("No escaped character") };
                    // 双引号里只有引号自己和反斜杠能被转义：别的字符前面的反斜杠原样留着
                    if back == St::Quote('"') && c != '\\' && c != '"' {
                        token.push('\\');
                    }
                    token.push(c);
                    state = Some(back);
                }
                St::Word => match c {
                    None => {
                        state = None;
                        break false;
                    }
                    Some(' ' | '\t' | '\r' | '\n') => {
                        state = Some(St::Space);
                        if !token.is_empty() || quoted {
                            break false;
                        }
                    }
                    Some(q @ ('\'' | '"')) => state = Some(St::Quote(q)),
                    Some('\\') => {
                        back = St::Word;
                        state = Some(St::Escape);
                    }
                    Some(c) => token.push(c),
                },
            }
        };
        if done || (token.is_empty() && !quoted) {
            return Ok(tokens);
        }
        tokens.push(token);
    }
}

/// --settings 后面那一段 JSON 是不是正好只有记下会话的 hook
fn hook_only(settings: &str, name: &str) -> Result<bool, Hook> {
    let h = json::parse(settings).map_err(|e| if e.unsupported() { Hook::Unsupported(e.to_string()) } else { Hook::Caught })?;
    let hooks = hook_key(&h, "hooks")?;
    let starts = hook_key(hooks, "SessionStart")?;
    let c = hook_first(hook_key(&hook_first(starts)?, "hooks")?)?;
    let keys = |v: &Value| v.as_obj().map(|o| o.keys().map(str::to_string).collect::<Vec<_>>());
    // 到这里 h、h["hooks"] 一定是字典，h["hooks"]["SessionStart"] 一定是列表（不然上面那一串下标已经出了被接住的异常）
    if keys(&h) != Some(vec!["hooks".to_string()]) || keys(hooks) != Some(vec!["SessionStart".to_string()]) {
        return Ok(false);
    }
    if starts.as_list().map(<[Value]>::len) != Some(1) {
        return Ok(false);
    }
    // sorted(c) == ['command', 'timeout', 'type']：c 是正好这三个键的字典才往下走。c 是数、None：sorted 报 TypeError（被接住）；
    // 是字符串：排出来的是一个个字符，不相等；是列表：排不了（被接住）、不相等，或者正好是这三个字符串——那下一步 c['type'] 是 TypeError
    let Value::Obj(o) = &c else { return Err(Hook::Caught) };
    let mut ks: Vec<&str> = o.keys().collect();
    ks.sort_unstable();
    if ks != ["command", "timeout", "type"] {
        return Ok(false);
    }
    if c["type"] != Value::from("command") || c["timeout"] != Value::from(10) {
        return Ok(false);
    }
    let words = match &c["command"] {
        Value::Str(s) => shlex_split(s).map_err(|_| Hook::Caught)?,
        // shlex.split(None) 是 ValueError（被接住）；别的类型它当成文件去读：AttributeError，没人接
        Value::Null => return Err(Hook::Caught),
        other => return Err(Hook::Uncaught(format!("AttributeError: '{}' object has no attribute 'read'", type_name(other)))),
    };
    Ok(words.get(1..).unwrap_or(&[]) == ["_session-start", name])
}

fn hookless(a: &[String], out: &mut String) -> R<i32> {
    let (line, name) = (arg(a, 0)?, arg(a, 1)?);
    // 原来的正则：^(.*?) --settings (\{"hooks": .*?\}\]\}\]\}\})( .*)?$ 。Python 的 $ 在结尾、在结尾那个换行的前面都算：
    // 这里写成 \n?\z（. 两边都不认换行）
    let rx = regex::Regex::new(r#"^(.*?) --settings (\{"hooks": .*?\}\]\}\]\}\})( .*)?\n?\z"#).expect("写死的正则");
    let mut shown = "BAD".to_string();
    if let Some(m) = rx.captures(line) {
        match hook_only(&m[2], name) {
            Ok(true) => shown = format!("{}{}", &m[1], m.get(3).map_or("", |g| g.as_str())),
            Ok(false) | Err(Hook::Caught) => {}
            Err(Hook::Uncaught(msg)) => return raise(msg),
            Err(Hook::Unsupported(msg)) => return unsupported(format!("--settings 后面的 JSON：{msg}")),
        }
    }
    out.push_str(&shown);
    out.push('\n');
    Ok(0)
}

// ---- claude-usage ----

fn claude_usage(a: &[String], _out: &mut String) -> R<i32> {
    let path = arg(a, 0)?;
    let five = py_float(arg(a, 1)?)?;
    let week = py_float(arg(a, 2)?)?;
    let sub = arg(a, 3)?;
    let extra = arg(a, 4)?;
    let extra = loads(if extra.is_empty() { "{}" } else { extra }, "额外的窗口")?;
    // 原来的 at(秒数)：每算一个时刻各看一次钟
    let at = |secs: f64| -> R<String> { Ok(format!("{}.123456+00:00", utc(now()? + secs))) };
    let mut rl = Obj::new();
    rl.insert("five_hour", obj! { "utilization" => five, "resets_at" => at(8010.0)? }); // 2 小时 13 分 30 秒
    rl.insert("seven_day", obj! { "utilization" => week, "resets_at" => at(277_200.0)? }); // 3 天 5 小时
    rl.insert("seven_day_opus", Value::Null);
    rl.insert("seven_day_sonnet", Value::Null);
    py_update(&mut rl, &extra)?;
    dump(path, &obj! { "subscription_type" => sub, "rate_limits_available" => true, "rate_limits" => Value::Obj(rl) })?;
    Ok(0)
}

// ---- text-prepend ----

fn text_prepend(a: &[String], _out: &mut String) -> R<i32> {
    let (path, line) = (arg(a, 0)?, arg(a, 1)?);
    let data = read_text(path)?;
    let tmp = format!("{path}.test-new");
    fake::write_text(&tmp, &format!("{line}\n{data}"))?;
    fake::replace(&tmp, path)?;
    Ok(0)
}

// ---- costlog ----

/// time.strftime('%Y-%m-%dT%H:%M:%S.000Z', time.gmtime(t))
fn iso(t: f64) -> String {
    format!("{}.000Z", utc(t))
}

/// json.dumps(d, separators=(',', ':')) + '\n'
fn line(d: &Value) -> String {
    let mut s = json::dumps(d, &Style { compact: true, ..Style::default() });
    s.push('\n');
    s
}

/// Claude Code 的会话记录里的一条回复
fn reply(t: f64, mid: &str, model: &str, inp: i64, out: i64, sid: &str) -> String {
    line(&obj! {
        "type" => "assistant",
        "timestamp" => iso(t),
        "requestId" => format!("req_{mid}"),
        "sessionId" => sid,
        "message" => obj! {
            "id" => mid,
            "model" => model,
            "role" => "assistant",
            "usage" => obj! { "input_tokens" => inp, "output_tokens" => out },
        },
    })
}

/// base64.urlsafe_b64decode(c + '=' * (-len(c) % 4))，再 json.loads。Python 的解码是宽松的（字母表以外的字符跳过），这里只认
/// 规规矩矩的那一种（假 codex 写的就是）；不规矩的不敢担保
fn claims_of(c: &str) -> R<Value> {
    let Ok(raw) = kit::crypto::b64url_nopad_decode(c.trim_end_matches('=')) else {
        return unsupported("id_token 中间那一段不是规规矩矩的 base64url");
    };
    // json.loads(字节串) 认 UTF-8 / 16 / 32 和开头的 BOM：这里只认不带 BOM 的 UTF-8
    match String::from_utf8(raw) {
        Ok(text) if !text.starts_with('\u{feff}') && !text.as_bytes().iter().take(4).any(|b| *b == 0) => {
            loads(&text, "id_token 里的 claims")
        }
        _ => unsupported("id_token 中间那一段不是不带 BOM 的 UTF-8"),
    }
}

fn costlog(a: &[String], _out: &mut String) -> R<i32> {
    let home = arg(a, 0)?;
    let what = &a[1..];
    let has = |w: &str| what.iter().any(|x| x == w);
    let now = now()?;
    const DAY: f64 = 86400.0;
    let d = fake::join(&[home, ".claude", "projects", "-cost"]);
    fake::makedirs(&d)?;
    if has("init") {
        let owner = obj! { "type" => "session-owner", "accountUuid" => "u-alice-0001", "organizationUuid" => "org-1", "sessionId" => "s-cost-1" };
        let asked = obj! {
            "type" => "user",
            "timestamp" => iso(now - 2.0 * DAY),
            "message" => obj! { "role" => "user", "content" => "SECRET-PROMPT-TEXT" },
        };
        let twice = reply(now - DAY, "mc2", "claude-sonnet-5-5[1m]", 2000, 1000, "s-cost-1");
        fake::write_text(
            &fake::join(&[&d, "s-cost-1.jsonl"]),
            &[
                line(&owner),
                line(&asked),
                reply(now - 2.0 * DAY, "mc1", "claude-opus-5-5", 1000, 100, "s-cost-1"),
                twice.clone(),
                twice,
                reply(now - 3600.0, "mc3", "claude-new-9", 10, 10, "s-cost-1"),
            ]
            .concat(),
        )?;
        // 没有 session-owner（旧版本）
        fake::write_text(
            &fake::join(&[&d, "s-cost-0.jsonl"]),
            &reply(now - 3.0 * 3600.0, "mc4", "claude-opus-5-5", 1000, 100, "s-cost-0"),
        )?;
        let x = fake::join(&[home, ".codex", "sessions", "2026", "11", "12"]);
        fake::makedirs(&x)?;
        let ev = |t: f64, kind: &str, p: Value| line(&obj! { "timestamp" => iso(t), "type" => kind, "payload" => p });
        let u = obj! {
            "input_tokens" => 10000,
            "cached_input_tokens" => 8000,
            "output_tokens" => 500,
            "reasoning_output_tokens" => 0,
            "total_tokens" => 10500,
        };
        let tc = obj! { "type" => "token_count", "info" => obj! { "total_token_usage" => u.clone(), "last_token_usage" => u } };
        fake::write_text(
            &fake::join(&[&x, "rollout-cost.jsonl"]),
            &[
                ev(
                    now - 7200.0,
                    "session_meta",
                    obj! { "id" => "th-cost", "creator_user_id" => "user-dave-0001", "model_provider" => "openai" },
                ),
                ev(now - 7200.0, "turn_context", obj! { "model" => "gpt-6-astra" }),
                ev(now - 7100.0, "event_msg", tc.clone()),
                ev(now - 7099.0, "event_msg", tc),
            ]
            .concat(),
        )?;
    }
    if has("more") {
        fake::append_text(
            &fake::join(&[&d, "s-cost-1.jsonl"]),
            &reply(now - 600.0, "mc5", "claude-opus-5-5", 1000, 100, "s-cost-1"),
        )?;
    }
    if has("dave") {
        // dave 的 id_token 里补上用户 id（真的 id_token 有 chatgpt_user_id）。文件是最后一个参数（原来的 sys.argv[-1]）
        let p = a.last().map(String::as_str).unwrap_or_default();
        let mut j = load(p)?;
        let token = match item(item(&j, "tokens")?, "id_token")? {
            Value::Str(s) => s.clone(),
            other => return raise(format!("AttributeError: '{}' object has no attribute 'split'", type_name(other))),
        };
        let parts: Vec<&str> = token.split('.').collect();
        let [h, c, sg] = parts.as_slice() else {
            let n = parts.len();
            return raise(if n < 3 {
                format!("ValueError: not enough values to unpack (expected 3, got {n})")
            } else {
                format!("ValueError: too many values to unpack (expected 3, got {n})")
            });
        };
        let mut claims = claims_of(c)?;
        fake::set_item(item_mut(&mut claims, "https://api.openai.com/auth")?, "chatgpt_user_id", Value::from("user-dave-0001"))?;
        let c = kit::crypto::b64url_nopad(json::dumps(&claims, &Style::default()).as_bytes());
        fake::set_item(item_mut(&mut j, "tokens")?, "id_token", Value::from([*h, c.as_str(), *sg].join(".")))?;
        dump(p, &j)?;
    }
    Ok(0)
}

// ---- router-occupy ----

/// 槽位号 i 是不是和这个值相等（Python 的 ==：2.0、True 也算数）；列表、字典不能进集合（TypeError）
fn slot_is(v: &Value, i: i128) -> R<bool> {
    Ok(match v {
        Value::Int(n) => *n == i,
        Value::Float(f) => json::int_eq_float(i, *f),
        Value::Bool(b) => i128::from(*b) == i,
        Value::Null | Value::Str(_) => false,
        Value::List(_) | Value::Obj(_) => {
            return raise(format!(
                "TypeError: cannot use '{}' as a set element (unhashable type: '{}')",
                type_name(v),
                type_name(v)
            ))
        }
    })
}

/// max(原来的 slots, s)：原来的不比 s 小就留着它（连同它的类型）
fn max_slots(old: &Value, s: i128) -> R<Value> {
    let keep = match old {
        Value::Int(n) => *n >= s,
        Value::Float(f) => (s as f64).partial_cmp(f) != Some(std::cmp::Ordering::Greater),
        Value::Bool(b) => i128::from(*b) >= s,
        other => return raise(format!("TypeError: '>' not supported between instances of 'int' and '{}'", type_name(other))),
    };
    Ok(if keep { old.clone() } else { Value::Int(s) })
}

fn router_occupy(a: &[String], out: &mut String) -> R<i32> {
    let (acc_path, own_path) = (arg(a, 0)?, arg(a, 1)?);
    let mut acc = load(acc_path)?;
    let own = load(own_path)?;
    let (name, device) = (arg(a, 2)?, arg(a, 3)?);
    // acc['accounts'] = [a for a in acc['accounts'] if a['name'] != 账号名]
    let old = match item(&acc, "accounts")? {
        Value::List(l) => l.clone(),
        Value::Obj(o) if o.is_empty() => Vec::new(),
        Value::Str(s) if s.is_empty() => Vec::new(),
        Value::Obj(_) | Value::Str(_) => return raise("TypeError: string indices must be integers, not 'str'"),
        other => return raise(format!("TypeError: '{}' object is not iterable", type_name(other))),
    };
    let mut accounts = Vec::new();
    for x in old {
        if *item(&x, "name")? != Value::from(name) {
            accounts.push(x);
        }
    }
    // used = {a['slot'] for a in …}; s = next(i for i in range(1, 99) if i not in used)
    let mut s = None;
    for x in &accounts {
        slot_is(item(x, "slot")?, 0)?;
    }
    for i in 1..99 {
        let mut used = false;
        for x in &accounts {
            used = used || slot_is(item(x, "slot")?, i)?;
        }
        if !used {
            s = Some(i);
            break;
        }
    }
    let Some(s) = s else { return raise("StopIteration") };
    let mut entry = Obj::new();
    entry.insert("slot", s);
    entry.insert("name", name);
    entry.insert("node", arg(a, 4)?);
    let ck = arg(a, 5)?;
    if !ck.is_empty() {
        entry.insert("ck", ck);
    }
    accounts.push(Value::Obj(entry));
    let Value::Obj(top) = &mut acc else { unreachable!("取过 acc['accounts']：是字典") };
    top.insert("accounts", Value::List(accounts));
    let slots = max_slots(top.get("slots").unwrap_or(&Value::Int(0)), s)?;
    top.insert("slots", slots);
    if let Some(custom) = a.get(6) {
        if custom == "-" {
            top.remove("custom");
        } else {
            top.insert("custom", obj! { "id" => "other", "proxies" => vec![loads(custom, "custom 的节点")?] });
        }
    }
    // own = {k: v for k, v in own.items() if v != 设备}; own[str(s)] = 设备
    let Value::Obj(own) = own else {
        return raise(format!("AttributeError: '{}' object has no attribute 'items'", type_name(&own)));
    };
    let mut kept = Obj::new();
    for (k, v) in own.iter().filter(|(_, v)| **v != Value::from(device)) {
        kept.insert(k, v.clone());
    }
    kept.insert(s.to_string(), device);
    fake::write_text(acc_path, &json::dumps(&acc, &Style::default().unicode()))?;
    dump(own_path, &Value::Obj(kept))?;
    out.push_str(&format!("{s}\n"));
    Ok(0)
}

// ---- 测试：期望的值都是原来那几段 Python 在同样的输入上算出来的（生成它们的脚本不在仓库里） ----

#[cfg(test)]
mod tests {
    use super::*;
    use kit::fsx::{self, TempDir};

    /// 对拍时钉住的那个时刻
    const NOW: f64 = 1_795_500_000.25;

    fn call(sub: Sub, args: &[&str]) -> (R<i32>, String) {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = String::new();
        let r = sub(&args, &mut out);
        (r, out)
    }

    /// 做完了（退出码 0）：打印的
    fn done(sub: Sub, args: &[&str]) -> String {
        let (r, out) = call(sub, args);
        assert!(matches!(r, Ok(0)), "{args:?}：{r:?}");
        out
    }

    /// Python 也会出的异常：说的那一行
    fn raised(sub: Sub, args: &[&str]) -> String {
        match call(sub, args) {
            (Err(Stop::Raise(msg)), out) if out.is_empty() => msg,
            other => panic!("{args:?}：{other:?}"),
        }
    }

    /// 这里不敢担保的
    fn refused(sub: Sub, args: &[&str]) {
        let (r, _) = call(sub, args);
        assert!(matches!(r, Err(Stop::Unsupported(_))), "{args:?}：{r:?}");
    }

    fn at(dir: &TempDir, name: &str) -> String {
        dir.join(name).to_str().expect("临时目录的路径是 UTF-8").to_string()
    }

    #[test]
    fn sha1_is_hashlib() {
        assert_eq!(done(sha1, &["alice@example.com"]), "fc2398a73dd54d6237c4fdb58fd7d75347cf5af3\n");
        assert_eq!(raised(sha1, &[]), "IndexError: list index out of range");
    }

    #[test]
    fn shlex_split_is_pythons() {
        let cases: &[(&str, Result<&[&str], &str>)] = &[
            (r#"a b"#, Ok(&[r#"a"#, r#"b"#])),
            (r#"'a b' c"#, Ok(&[r#"a b"#, r#"c"#])),
            (r#""a\"b""#, Ok(&[r#"a"b"#])),
            (r#"a\ b"#, Ok(&[r#"a b"#])),
            (r#""a\b""#, Ok(&[r#"a\b"#])),
            (r#"'a\'"#, Ok(&[r#"a\"#])),
            (r#"a''b"#, Ok(&[r#"ab"#])),
            (r#"''"#, Ok(&[r#""#])),
            (r#"a """#, Ok(&[r#"a"#, r#""#])),
            (r#"  "#, Ok(&[])),
            (r#"a'b"#, Err(r#"No closing quotation"#)),
            (r#"a\"#, Err(r#"No escaped character"#)),
            (" a\tb\nc\r\nd ", Ok(&[r#"a"#, r#"b"#, r#"c"#, r#"d"#])),
            (r#""a'b""#, Ok(&[r#"a'b"#])),
            (r#"'a"b'"#, Ok(&[r#"a"b"#])),
            (r#"a"b c"d"#, Ok(&[r#"ab cd"#])),
            (r#""\\""#, Ok(&[r#"\"#])),
            (r#""\$x""#, Ok(&[r#"\$x"#])),
            (r#""#, Ok(&[])),
            (r#"/p/cca _session-start 'x y'"#, Ok(&[r#"/p/cca"#, r#"_session-start"#, r#"x y"#])),
            (r#"a #b"#, Ok(&[r#"a"#, r#"#b"#])),
            (r#""a" "" b"#, Ok(&[r#"a"#, r#""#, r#"b"#])),
        ];
        for (text, want) in cases {
            let got = shlex_split(text);
            let got: Result<Vec<&str>, &str> = got.as_ref().map(|v| v.iter().map(String::as_str).collect()).map_err(|e| *e);
            assert_eq!(got, want.map(<[&str]>::to_vec), "{text:?}");
        }
    }

    #[test]
    fn hookless_keeps_only_the_session_hook() {
        let name = "alice-example-com-fc2398";
        let cases: &[(&str, &str)] = &[
            (
                r#"ARGS: --add-dir=/a --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}} --version"#,
                "ARGS: --add-dir=/a --version\n",
            ),
            (
                r#"ARGS: --add-dir=/a --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}}"#,
                "ARGS: --add-dir=/a\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "'/p a/cca' _session-start 'alice-example-com-fc2398'"}]}]}} -p x"#,
                "ARGS: -p x\n",
            ),
            (r#"ARGS: --add-dir=/a --version"#, "BAD\n"),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start other"}]}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "x": 1, "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 11, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10.0, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}}"#,
                "ARGS:\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": null}]}]}}"#,
                "BAD\n",
            ),
            (r#"ARGS: --settings {"hooks": nope}]}]}}"#, "BAD\n"),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start 'alice-example-com-fc2398"}]}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"Stop": [], "SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}, {"hooks": []}]}}"#,
                "BAD\n",
            ),
            (
                r#"ARGS: 中文 --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": "/p/cca _session-start alice-example-com-fc2398"}]}]}} 尾巴"#,
                "ARGS: 中文 尾巴\n",
            ),
        ];
        for (line, want) in cases {
            assert_eq!(done(hookless, &[line, name]), *want, "{line}");
        }
        // command 是个数：shlex 把它当文件去读，这个异常原来没人接——什么都不打，退出码 1
        let line =
            r#"ARGS: --settings {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "timeout": 10, "command": 5}]}]}}"#;
        assert_eq!(raised(hookless, &[line, name]), "AttributeError: 'int' object has no attribute 'read'");
        assert_eq!(raised(hookless, &[line]), "IndexError: list index out of range");
    }

    #[test]
    fn claude_usage_writes_what_python_wrote() {
        let dir = TempDir::new("fx-usage-");
        let file = at(&dir, "u.json");
        PINNED.with(|p| p.set(Some(NOW)));
        done(
            claude_usage,
            &[
                &file,
                r#"13"#,
                r#"41.5"#,
                r#"max"#,
                r#"{"seven_day_opus": {"utilization": 5.0, "resets_at": null}, "five_hour": null}"#,
            ],
        );
        assert_eq!(
            fsx::read_text(&file),
            r#"{"subscription_type": "max", "rate_limits_available": true, "rate_limits": {"five_hour": null, "seven_day": {"utilization": 41.5, "resets_at": "2026-11-27T11:00:00.123456+00:00"}, "seven_day_opus": {"utilization": 5.0, "resets_at": null}, "seven_day_sonnet": null}}"#
        );
        done(claude_usage, &[&file, r#"0"#, r#"100"#, r#"pro"#, r#""#]);
        assert_eq!(
            fsx::read_text(&file),
            r#"{"subscription_type": "pro", "rate_limits_available": true, "rate_limits": {"five_hour": {"utilization": 0.0, "resets_at": "2026-11-24T08:13:30.123456+00:00"}, "seven_day": {"utilization": 100.0, "resets_at": "2026-11-27T11:00:00.123456+00:00"}, "seven_day_opus": null, "seven_day_sonnet": null}}"#
        );
        assert_eq!(
            raised(claude_usage, &[&file, r#"abc"#, r#"1"#, r#"max"#, r#""#]),
            r#"ValueError: could not convert string to float: 'abc'"#
        );
        done(claude_usage, &[&file, r#"1"#, r#"2"#, r#"max"#, r#"[["k", 1]]"#]);
        assert_eq!(
            fsx::read_text(&file),
            r#"{"subscription_type": "max", "rate_limits_available": true, "rate_limits": {"five_hour": {"utilization": 1.0, "resets_at": "2026-11-24T08:13:30.123456+00:00"}, "seven_day": {"utilization": 2.0, "resets_at": "2026-11-27T11:00:00.123456+00:00"}, "seven_day_opus": null, "seven_day_sonnet": null, "k": 1}}"#
        );
        assert_eq!(raised(claude_usage, &[&file, r#"1"#, r#"2"#, r#"max"#, r#"5"#]), r#"TypeError: 'int' object is not iterable"#);
        assert_eq!(raised(claude_usage, &[&file, r#"1"#]), r#"IndexError: list index out of range"#);
        // 出了异常的那几次没动文件（还是上一次写的）
        assert!(fsx::read_text(&file).ends_with(r#""seven_day_sonnet": null, "k": 1}}"#));
        // 键不是字符串的一对、全角数字：Python 认，这里不担保
        refused(claude_usage, &[&file, "1", "2", "max", "[[1, 2]]"]);
        refused(claude_usage, &[&file, "１", "2", "max", ""]);
    }

    #[test]
    fn text_prepend_replaces_the_file_like_an_atomic_save() {
        let dir = TempDir::new("fx-prepend-");
        fsx::write(dir.join("real.toml"), "a = 1\r\nb = 2\r");
        std::os::unix::fs::symlink("real.toml", dir.join("config.toml")).unwrap();
        let file = at(&dir, "config.toml");
        done(text_prepend, &[&file, r#"openai_base_url = "https://evil.invalid""#]);
        // 链接换成了普通文件，原来指着的那个没动；文本模式读写，换行都成了 \n；临时文件没留下
        assert!(!dir.join("config.toml").is_symlink());
        assert_eq!(fsx::read_text(&file), "openai_base_url = \"https://evil.invalid\"\na = 1\nb = 2\n");
        assert_eq!(fsx::read_text(dir.join("real.toml")), "a = 1\r\nb = 2\r");
        assert!(!dir.join("config.toml.test-new").exists());
        let missing = at(&dir, "nope.toml");
        assert_eq!(
            raised(text_prepend, &[&missing, "x"]),
            format!("FileNotFoundError: [Errno 2] No such file or directory: '{missing}'")
        );
        assert!(!dir.join("nope.toml.test-new").exists());
    }

    #[test]
    fn costlog_writes_what_python_wrote() {
        let dir = TempDir::new("fx-cost-");
        let home = at(&dir, "home");
        PINNED.with(|p| p.set(Some(NOW)));
        done(costlog, &[&home, "init"]);
        let cost = dir.join("home/.claude/projects/-cost");
        let first = concat!(
            r#"{"type":"session-owner","accountUuid":"u-alice-0001","organizationUuid":"org-1","sessionId":"s-cost-1"}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-11-22T06:00:00.000Z","message":{"role":"user","content":"SECRET-PROMPT-TEXT"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-22T06:00:00.000Z","requestId":"req_mc1","sessionId":"s-cost-1","message":{"id":"mc1","model":"claude-opus-5-5","role":"assistant","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-23T06:00:00.000Z","requestId":"req_mc2","sessionId":"s-cost-1","message":{"id":"mc2","model":"claude-sonnet-5-5[1m]","role":"assistant","usage":{"input_tokens":2000,"output_tokens":1000}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-23T06:00:00.000Z","requestId":"req_mc2","sessionId":"s-cost-1","message":{"id":"mc2","model":"claude-sonnet-5-5[1m]","role":"assistant","usage":{"input_tokens":2000,"output_tokens":1000}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-11-24T05:00:00.000Z","requestId":"req_mc3","sessionId":"s-cost-1","message":{"id":"mc3","model":"claude-new-9","role":"assistant","usage":{"input_tokens":10,"output_tokens":10}}}"#,
            "\n",
        );
        assert_eq!(fsx::read_text(cost.join("s-cost-1.jsonl")), first);
        assert_eq!(
            fsx::read_text(cost.join("s-cost-0.jsonl")),
            concat!(
                r#"{"type":"assistant","timestamp":"2026-11-24T03:00:00.000Z","requestId":"req_mc4","sessionId":"s-cost-0","message":{"id":"mc4","model":"claude-opus-5-5","role":"assistant","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
                "\n",
            )
        );
        assert_eq!(
            fsx::read_text(dir.join("home/.codex/sessions/2026/11/12/rollout-cost.jsonl")),
            concat!(
                r#"{"timestamp":"2026-11-24T04:00:00.000Z","type":"session_meta","payload":{"id":"th-cost","creator_user_id":"user-dave-0001","model_provider":"openai"}}"#,
                "\n",
                r#"{"timestamp":"2026-11-24T04:00:00.000Z","type":"turn_context","payload":{"model":"gpt-6-astra"}}"#,
                "\n",
                r#"{"timestamp":"2026-11-24T04:01:40.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10000,"cached_input_tokens":8000,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":10500},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":8000,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":10500}}}}"#,
                "\n",
                r#"{"timestamp":"2026-11-24T04:01:41.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10000,"cached_input_tokens":8000,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":10500},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":8000,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":10500}}}}"#,
                "\n",
            )
        );
        // more：接在后面；init 再来一次是整个重写
        done(costlog, &[&home, "more"]);
        let more = r#"{"type":"assistant","timestamp":"2026-11-24T05:50:00.000Z","requestId":"req_mc5","sessionId":"s-cost-1","message":{"id":"mc5","model":"claude-opus-5-5","role":"assistant","usage":{"input_tokens":1000,"output_tokens":100}}}"#;
        assert_eq!(fsx::read_text(cost.join("s-cost-1.jsonl")), format!("{first}{more}\n"));
        done(costlog, &[&home, "more", "init"]);
        assert_eq!(fsx::read_text(cost.join("s-cost-1.jsonl")), format!("{first}{more}\n"));
        done(costlog, &[&home, "init"]);
        assert_eq!(fsx::read_text(cost.join("s-cost-1.jsonl")), first);
        assert_eq!(raised(costlog, &[]), "IndexError: list index out of range");
    }

    #[test]
    fn costlog_dave_adds_the_user_id_to_the_id_token() {
        let dir = TempDir::new("fx-dave-");
        let (home, auth) = (at(&dir, "home"), at(&dir, "auth.json"));
        fsx::write(
            &auth,
            r#"{"OPENAI_API_KEY": null, "tokens": {"id_token": "aGVhZA.eyJlbWFpbCI6ICJkYXZlQGV4YW1wbGUuY29tIiwgImh0dHBzOi8vYXBpLm9wZW5haS5jb20vYXV0aCI6IHsiY2hhdGdwdF9wbGFuX3R5cGUiOiAicGx1cyJ9LCAidSI6ICJcdTRlMmQifQ.c2ln", "access_token": "a"}, "last_refresh": "x"}"#,
        );
        done(costlog, &[&home, "dave", &auth]);
        assert_eq!(
            fsx::read_text(&auth),
            r#"{"OPENAI_API_KEY": null, "tokens": {"id_token": "aGVhZA.eyJlbWFpbCI6ICJkYXZlQGV4YW1wbGUuY29tIiwgImh0dHBzOi8vYXBpLm9wZW5haS5jb20vYXV0aCI6IHsiY2hhdGdwdF9wbGFuX3R5cGUiOiAicGx1cyIsICJjaGF0Z3B0X3VzZXJfaWQiOiAidXNlci1kYXZlLTAwMDEifSwgInUiOiAiXHU0ZTJkIn0.c2ln", "access_token": "a"}, "last_refresh": "x"}"#
        );
        // 会话记录的目录照样先建好，别的什么都没写
        assert!(dir.join("home/.claude/projects/-cost").is_dir() && !dir.join("home/.codex").exists());
        for (token, want) in [
            (r#""a.b""#, r#"ValueError: not enough values to unpack (expected 3, got 2)"#),
            (r#""a.b.c.d""#, r#"ValueError: too many values to unpack (expected 3, got 4)"#),
            (r#"5"#, r#"AttributeError: 'int' object has no attribute 'split'"#),
        ] {
            let text = format!(r#"{{"tokens": {{"id_token": {token}}}}}"#);
            fsx::write(&auth, &text);
            assert_eq!(raised(costlog, &[&home, "dave", &auth]), want);
            assert_eq!(fsx::read_text(&auth), text);
        }
    }

    #[test]
    fn router_occupy_takes_the_first_free_slot() {
        let dir = TempDir::new("fx-occupy-");
        let (acc, own) = (at(&dir, "accounts.yaml"), at(&dir, "owners"));
        let acc_before = r#"{"base": 7900, "slots": 2, "nodes": ["US-A"], "accounts": [{"slot": 1, "name": "alice", "node": "US-A"}, {"slot": 3, "name": "dave-dev2", "node": "old", "ck": "x"}, {"slot": 2, "name": "bob", "node": "香港"}], "custom": {"id": "mine"}}"#;
        let own_before = r#"{"1": "aaaa", "3": "bbbb1111bbbb1111", "2": "aaaa"}"#;
        // (节点、ck、custom，打印的或者异常，之后的账号表，之后的 owners)
        let cases: &[(&[&str], Result<&str, &str>, &str, &str)] = &[
            (
                &[r#"Home IP"#, r#"0123"#],
                Ok("3\n"),
                r#"{"base": 7900, "slots": 3, "nodes": ["US-A"], "accounts": [{"slot": 1, "name": "alice", "node": "US-A"}, {"slot": 2, "name": "bob", "node": "香港"}, {"slot": 3, "name": "dave-dev2", "node": "Home IP", "ck": "0123"}], "custom": {"id": "mine"}}"#,
                r#"{"1": "aaaa", "2": "aaaa", "3": "bbbb1111bbbb1111"}"#,
            ),
            (
                &[r#"US-A"#, r#""#],
                Ok("3\n"),
                r#"{"base": 7900, "slots": 3, "nodes": ["US-A"], "accounts": [{"slot": 1, "name": "alice", "node": "US-A"}, {"slot": 2, "name": "bob", "node": "香港"}, {"slot": 3, "name": "dave-dev2", "node": "US-A"}], "custom": {"id": "mine"}}"#,
                r#"{"1": "aaaa", "2": "aaaa", "3": "bbbb1111bbbb1111"}"#,
            ),
            (
                &[r#"Home IP"#, r#"ck"#, r#"{"name": "Home IP", "type": "ss", "note": "中"}"#],
                Ok("3\n"),
                r#"{"base": 7900, "slots": 3, "nodes": ["US-A"], "accounts": [{"slot": 1, "name": "alice", "node": "US-A"}, {"slot": 2, "name": "bob", "node": "香港"}, {"slot": 3, "name": "dave-dev2", "node": "Home IP", "ck": "ck"}], "custom": {"id": "other", "proxies": [{"name": "Home IP", "type": "ss", "note": "中"}]}}"#,
                r#"{"1": "aaaa", "2": "aaaa", "3": "bbbb1111bbbb1111"}"#,
            ),
            (
                &[r#"Home IP"#, r#"ck"#, r#"-"#],
                Ok("3\n"),
                r#"{"base": 7900, "slots": 3, "nodes": ["US-A"], "accounts": [{"slot": 1, "name": "alice", "node": "US-A"}, {"slot": 2, "name": "bob", "node": "香港"}, {"slot": 3, "name": "dave-dev2", "node": "Home IP", "ck": "ck"}]}"#,
                r#"{"1": "aaaa", "2": "aaaa", "3": "bbbb1111bbbb1111"}"#,
            ),
            (&[r#"Home IP"#, r#"ck"#, r#"{"#], Err(r#"json.decoder.JSONDecodeError"#), acc_before, own_before),
            (&[r#"Home IP"#], Err(r#"IndexError: list index out of range"#), acc_before, own_before),
        ];
        for (args, want, acc_after, own_after) in cases {
            fsx::write(&acc, acc_before);
            fsx::write(&own, own_before);
            let mut all = vec![acc.as_str(), own.as_str(), "dave-dev2", "bbbb1111bbbb1111"];
            all.extend_from_slice(args);
            match want {
                Ok(printed) => assert_eq!(done(router_occupy, &all), *printed, "{args:?}"),
                Err(msg) => assert!(raised(router_occupy, &all).starts_with(msg), "{args:?}"),
            }
            assert_eq!(fsx::read_text(&acc), *acc_after, "{args:?}");
            assert_eq!(fsx::read_text(&own), *own_after, "{args:?}");
        }
        // 槽位号是小数、True 的也算占着（1.0 == 1 == True）；原来的 slots 比新槽位大就原样留着
        fsx::write(
            &acc,
            r#"{"accounts": [{"slot": 1.0, "name": "a"}, {"slot": true, "name": "b"}, {"slot": "2", "name": "c"}, {"slot": 2.5, "name": "d"}], "slots": 9.5}"#,
        );
        fsx::write(&own, r#"{"7": 1}"#);
        assert_eq!(done(router_occupy, &[&acc, &own, "dave-dev2", "bbbb1111bbbb1111", "US-A", ""]), "2\n");
        assert_eq!(
            fsx::read_text(&acc),
            r#"{"accounts": [{"slot": 1.0, "name": "a"}, {"slot": true, "name": "b"}, {"slot": "2", "name": "c"}, {"slot": 2.5, "name": "d"}, {"slot": 2, "name": "dave-dev2", "node": "US-A"}], "slots": 9.5}"#
        );
        assert_eq!(fsx::read_text(&own), r#"{"7": 1, "2": "bbbb1111bbbb1111"}"#);
        // 1 到 98 都占了：next() 没有东西可给
        let full: Vec<String> = (1..99).map(|i| format!(r#"{{"slot": {i}, "name": "n{i}"}}"#)).collect();
        let full = format!(r#"{{"accounts": [{}]}}"#, full.join(", "));
        fsx::write(&acc, &full);
        assert_eq!(raised(router_occupy, &[&acc, &own, "dave-dev2", "bbbb1111bbbb1111", "US-A", ""]), "StopIteration");
        assert_eq!(fsx::read_text(&acc), full);
    }
}
