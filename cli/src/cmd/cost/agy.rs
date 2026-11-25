//! 折合金额里 Antigravity（agy）的那一份。
//!
//! 用量：agy 把每个会话存成共享目录里的一个 SQLite 库（~/.gemini/antigravity-cli/conversations/<会话 id>.db，cca 的各个
//! agy 账号经链接共用这个目录）。steps 表里模型回复的那一步，metadata（protobuf，没有公开的定义：字段号是对着 1.5.16 的数据认的）
//! 带着时间（1：{1 秒, 2 纳秒}）和用量（9：1 模型的编号、2 没命中缓存的输入、3 输出（含思考）、5 缓存读、7 回复 id、9 思考、
//! 10 正文）；gen_metadata 表每次请求一行，有同一份用量和模型名（1.4.1 编号、1.19 名字）：模型编号 → 名字记在 scan.json 的 am。
//! 读库起系统的 sqlite3 命令（macOS 自带；Linux 上没装就不算 agy 的），只读：有 -wal 时 -readonly（读得到还没并回去的），没有时
//! immutable（-readonly 在没有 -shm 时开不了）。库的大小、修改时间（连同 -wal 的）没变就不读；读到哪按 steps 的 idx 记，
//! 回复按会话 id + 回复 id 去重（种类 a）。
//!
//! 是哪个账号的：会话库里没有账号的信息。cca agy 启动的每一次运行，agy 退出以后 cca 在账号目录的 agy-sessions.jsonl 记一行
//! {"at": 开始, "end": 结束, "pid": cca 的进程号, "convs": [这次运行用过的会话 id]}（agy-session-end）；会话 id 取这个账号自己的
//! 日志（账号目录里 gemini/antigravity-cli/log/cli-<本地时间>.log，各账号不共用；名字里的时间在这次运行里的那几份）里
//! Created conversation、Streaming conversation 那几句。没记上结束的（cca 被 kill -9、远程会话里关了终端）：下一次这个账号的
//! session-open 清掉旧登记时补记（结束取那几份日志最后的修改时间），统计时还在的登记（还在运行的、没补记的）照同样的办法现算。
//! 一条回复的时间落在某个账号的某次运行里（前后各 2 秒余量）、那次运行用过这个会话，就是那个账号的（身份 agy:<邮箱>）；不经 cca
//! 启动的 agy（日志在 ~/.gemini 里）对不上，记成认不出账号。agy 的日志隔一会儿才写盘：对不上的回复在 AGY_DEFER 以内先不算，
//! 下次再看。

use super::*;
use crate::core::tool::agy;
use crate::py::fs::listdir_sorted;
use crate::py::proc::{Cmd as Proc, Out};
use std::collections::HashSet;

/// 账号目录里记 agy 运行的文件（agy-session-end、session-open 写，统计时读）
pub const AGY_RUNS: &str = "agy-sessions.jsonl";
/// 日志的名字（agy 起来时的本地时间）在运行开始以前这么多秒以内也算这次的
const LOG_SLACK: f64 = 5.0;
/// 对不上账号的回复在这么久以内先不算（agy 的日志隔一会儿才写盘，正在运行的那次可能还没写下会话 id）
const AGY_DEFER: f64 = 600.0;
/// 一份日志最多读这么多（只找会话 id）
const AGY_LOG_MAX: u64 = 32 << 20;
/// 一次运行最多记这么多个会话 id
const AGY_CONVS_MAX: usize = 256;
/// 会话 id（会话库的文件名、日志里说的）
static CONV_RE: LazyLock<Re> = LazyLock::new(|| Re::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"));

/// Google 的 Gemini API 标价（美元 / 百万 token；Standard 档）：输入、输出（含思考）、缓存读。Gemini 4 只有隐式缓存（不收缓存写、
/// 存储），没有长上下文的分档。ai.google.dev/gemini-api/docs/pricing（现在跳到 aistudio.google.com/docs/pricing），2026-11-25 核对
pub(super) const GEMINI_PRICES: &[(&str, [f64; 3])] = &[
    ("gemini-4-neon", [3.0, 15.0, 0.15]),
    ("gemini-4-hydrogen", [1.50, 7.50, 0.075]),
    ("gemini-4-carbon", [2.0, 10.0, 0.10]),
    ("gemini-4-argon", [2.0, 10.0, 0.10]),
];
/// 按请求的时间分段的标价：(模型, 从这时起, 那以后的标价)。Carbon、Argon 2026-12-01 起涨一倍（官方没写时区：按太平洋时间零点）
pub(super) const GEMINI_FROM: &[(&str, f64, [f64; 3])] =
    &[("gemini-4-carbon", 1796112000.0, [4.0, 20.0, 0.20]), ("gemini-4-argon", 1796112000.0, [4.0, 20.0, 0.20])];
/// agy 里的模型名末尾带着思考的档（claude-opus-5-5-high）：算钱、按模型列时去掉
pub(super) const AGY_EFFORT: &str = r"-(?:minimal|low|medium|high|xhigh|max)$";

/// agy 的模型名 → 价格表里的写法（小写；去掉思考的档）
pub fn agy_model(name: &str) -> String {
    let m = cost_model(Some(&V::from(name)));
    if m.starts_with("claude-") || m.starts_with("gemini-") {
        re!(AGY_EFFORT).sub("", &m)
    } else {
        m
    }
}

/// 一条 agy 回复：(折合多少百万分之一美元：没有公开价格的模型是 None, (输入, 输出, 缓存读, 缓存写))。Claude 的模型按 Anthropic 的标价
/// （agy 不说缓存写了多少：没命中缓存的都按输入算）
pub fn cost_agy(model: &str, ts: f64, inp: i64, out: i64, cr: i64) -> (Option<f64>, [i64; 4]) {
    let tok = [inp, out, cr, 0];
    if let Some((_, p)) = GEMINI_PRICES.iter().find(|(n, _)| *n == model) {
        let p = GEMINI_FROM.iter().filter(|(n, from, _)| *n == model && ts >= *from).map(|(_, _, p)| p).next_back().unwrap_or(p);
        return (Some(inp as f64 * p[0] + out as f64 * p[1] + cr as f64 * p[2]), tok);
    }
    if model.starts_with("claude-") {
        let u = dict! { "input_tokens" => inp, "output_tokens" => out, "cache_read_input_tokens" => cr };
        return (cost_claude(model, &u).0, tok);
    }
    (None, tok)
}

// --- protobuf（只要这几样：变长整数和带长度的字段）-------------------------------------------------------------------------

enum Pb<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// 一层的字段 [(字段号, 值)]；格式不对是 None
fn pb_fields(b: &[u8]) -> Option<Vec<(u32, Pb<'_>)>> {
    fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let mut r = 0u64;
        for shift in (0..64).step_by(7) {
            let x = *b.get(*i)?;
            *i += 1;
            r |= u64::from(x & 0x7f) << shift;
            if x & 0x80 == 0 {
                return Some(r);
            }
        }
        None
    }
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        let k = varint(b, &mut i)?;
        let field = u32::try_from(k >> 3).ok()?;
        match k & 7 {
            0 => out.push((field, Pb::Int(varint(b, &mut i)?))),
            1 => i = i.checked_add(8)?,
            5 => i = i.checked_add(4)?,
            2 => {
                let n = usize::try_from(varint(b, &mut i)?).ok()?;
                let end = i.checked_add(n).filter(|e| *e <= b.len())?;
                out.push((field, Pb::Bytes(&b[i..end])));
                i = end;
            }
            _ => return None,
        }
        if i > b.len() {
            return None;
        }
    }
    Some(out)
}

fn pb_int(f: &[(u32, Pb)], n: u32) -> Option<u64> {
    f.iter().rev().find_map(|(k, v)| match v {
        Pb::Int(x) if *k == n => Some(*x),
        _ => None,
    })
}

fn pb_bytes<'a>(f: &[(u32, Pb<'a>)], n: u32) -> Option<&'a [u8]> {
    f.iter().rev().find_map(|(k, v)| match v {
        Pb::Bytes(x) if *k == n => Some(*x),
        _ => None,
    })
}

fn pb_str<'a>(f: &[(u32, Pb<'a>)], n: u32) -> Option<&'a str> {
    pb_bytes(f, n).and_then(|b| std::str::from_utf8(b).ok())
}

/// 一次回复（steps 的 metadata）
#[derive(Debug, PartialEq)]
pub struct Reply {
    pub ts: f64,
    pub model_id: u64,
    pub rid: String,
    pub inp: i64,
    pub out: i64,
    pub cr: i64,
}

fn count(x: Option<u64>) -> i64 {
    x.filter(|x| *x < 1_000_000_000_000).map_or(0, |x| x as i64)
}

/// 一步的时间（metadata 的 1：{1 秒, 2 纳秒}）
fn step_ts(top: &[(u32, Pb)]) -> Option<f64> {
    let at = pb_fields(pb_bytes(top, 1)?)?;
    Some(pb_int(&at, 1)? as f64 + pb_int(&at, 2).unwrap_or(0) as f64 / 1e9)
}

/// steps 的一行 metadata → (这一步的时间, 回复)：读不出时间是 None；不是模型回复的那一步（没有用量、回复 id）回复是 None
pub fn step_reply(meta: &[u8]) -> (Option<f64>, Option<Reply>) {
    let Some(top) = pb_fields(meta) else { return (None, None) };
    let ts = step_ts(&top);
    (ts, ts.and_then(|ts| reply_of(&top, ts)))
}

fn reply_of(top: &[(u32, Pb)], ts: f64) -> Option<Reply> {
    let u = pb_fields(pb_bytes(top, 9)?)?;
    let rid = pb_str(&u, 7).filter(|r| (8..=128).contains(&r.len()))?.to_string();
    let out = match count(pb_int(&u, 3)) {
        0 => count(pb_int(&u, 9)) + count(pb_int(&u, 10)),
        n => n,
    };
    Some(Reply { ts, model_id: pb_int(&u, 1)?, rid, inp: count(pb_int(&u, 2)), out, cr: count(pb_int(&u, 5)) })
}

/// gen_metadata 的一行 → (模型的编号, 模型名)
pub fn gen_model(data: &[u8]) -> Option<(u64, String)> {
    let top = pb_fields(data)?;
    let inner = pb_fields(pb_bytes(&top, 1)?)?;
    let u = pb_fields(pb_bytes(&inner, 4)?)?;
    let name = pb_str(&inner, 19).filter(|n| re!(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").matches(n))?;
    Some((pb_int(&u, 1)?, name.to_lowercase()))
}

// --- agy 的运行（账号目录的 agy-sessions.jsonl 和日志）----------------------------------------------------------------------

/// 账号的 agy 日志目录
fn log_dir(dir: &str) -> String {
    path::join_all(&[dir, agy().sub, "antigravity-cli", "log"])
}

/// cli-20261124_215602.log → 那个本地时间
fn log_time(name: &str) -> Option<f64> {
    let m = re!(r"^cli-([0-9]{4})([0-9]{2})([0-9]{2})_([0-9]{2})([0-9]{2})([0-9]{2})\.log$").match_(name)?;
    let n = |i: usize| m.g(i).parse::<i32>().ok();
    Some(mktime(n(1)?, n(2)?, n(3)?, n(4)?, n(5)?, n(6)?, -1))
}

/// 账号目录 dir 里在 [since, until] 起来的 agy 的日志：(用过的会话 id, 那几份日志最后的修改时间)
pub fn run_convs(dir: &str, since: f64, until: f64) -> (Vec<String>, Option<f64>) {
    let ld = log_dir(dir);
    let (mut convs, mut last): (Vec<String>, Option<f64>) = (Vec::new(), None);
    for name in listdir_sorted(&ld).unwrap_or_default() {
        let Some(at) = log_time(&name) else { continue };
        if at < since - LOG_SLACK || at > until + LOG_SLACK {
            continue;
        }
        let p = path::join(&ld, &name);
        let Ok(f) = std::fs::File::open(&p) else { continue };
        let Ok(st) = f.metadata() else { continue };
        if !st.is_file() {
            continue;
        }
        let mt = shutil::st_mtime(&st);
        last = Some(last.map_or(mt, |l: f64| l.max(mt)));
        let mut raw = Vec::new();
        if f.take(AGY_LOG_MAX).read_to_end(&mut raw).is_err() {
            continue;
        }
        let body = String::from_utf8_lossy(&raw);
        for m in
            re!(r"(?:Created conversation|Streaming conversation|conversation update stream for) ([0-9a-f-]{36})").finditer(&body)
        {
            let c = m.g(1);
            if CONV_RE.matches(c) && convs.len() < AGY_CONVS_MAX && !convs.iter().any(|x| x == c) {
                convs.push(c.to_string());
            }
        }
    }
    (convs, last)
}

/// 记一次运行（agy 退出以后；没记上的由下一次 session-open 补）：会话 id 从这次的日志里取
pub fn record_run(dir: &str, pid: i64, since: f64, end: Option<f64>) {
    let until = end.unwrap_or_else(time);
    let (convs, last) = run_convs(dir, since, until);
    // 没记上结束的：结束取日志最后的修改时间（一份都没有：这次没真正运行起来，没有要记的）
    let Some(end) = end.or(last) else { return };
    let rec = dict! { "at" => since, "end" => end.max(since), "pid" => pid, "convs" => convs.into_iter().map(V::from).collect::<Vec<V>>() };
    let line = json::dumps(&V::Dict(rec), json::Fmt::new().compact());
    let _ = sessions_append(&path::join(dir, AGY_RUNS), format!("{line}\n").as_bytes());
}

/// cca agy-session-end 账号 cca的进程号：run_account 在 agy 退出以后、删掉登记以前调（CCA_TOOL=agy）。登记（cli-<进程号>.json）
/// 不在、不对就不记。出什么错都不说
pub fn cmd_agy_session_end(args: &[String]) -> R<()> {
    arity("cmd_agy_session_end", args, 2, 2)?;
    if crate::core::tool::t().kind != Kind::Agy {
        return Ok(());
    }
    let Some(pid) = text::to_int(&args[1]) else { return Ok(()) };
    let key = args[0].as_str();
    let since = super::super::session::cli_marks(key)
        .into_iter()
        .find(|m| m.at("pid").as_int() == Some(pid))
        .and_then(|m| m.at("since").as_num());
    if let Some(since) = since {
        record_run(&adir(key), pid, since, Some(time()));
    }
    Ok(())
}

/// 一次运行：[开始, 结束] 里、用过这些会话的回复是 ident 的
struct Run {
    ident: String,
    at: f64,
    end: f64,
    convs: HashSet<String>,
}

/// 账号表里各 agy 账号的运行（记下的，和还在的登记现算的）；账号认不出邮箱的不要
fn agy_runs(now: f64) -> Vec<Run> {
    let tool = agy();
    let table = if path::exists(&tool.table) { load_quiet(&tool.table).and_then(|t| t.as_dict()) } else { None };
    let _using = Using::tool(tool);
    let mut runs = Vec::new();
    for (k, _) in table.map(|t| t.items()).unwrap_or_default() {
        let email = parse_key(&k).1.to_lowercase();
        if email.is_empty() {
            continue;
        }
        let ident = format!("agy:{email}");
        let dir = adir(&k);
        for d in jsonl_locked(&path::join(&dir, AGY_RUNS)) {
            let (Some(at), Some(end)) = (d.get("at").and_then(|a| a.as_num()), d.get("end").and_then(|e| e.as_num())) else {
                continue;
            };
            let convs = d.list("convs").map(|l| l.items().iter().filter_map(|c| c.as_str().map(|c| c.to_string())).collect());
            runs.push(Run { ident: ident.clone(), at, end, convs: convs.unwrap_or_default() });
        }
        // 还在的登记：在运行的（结束算现在）、cca 没来得及记的（结束取日志最后的修改时间）
        let marks = super::super::session::cli_marks(&k);
        let pids: Vec<i64> = marks.iter().filter_map(|m| m.at("pid").as_int()).collect();
        let starts = proc_starts(&pids);
        for m in marks {
            let (Some(pid), Some(started), Some(since)) = (m.at("pid").as_int(), m.at("started").as_int(), m.at("since").as_num())
            else {
                continue;
            };
            let (convs, last) = run_convs(&dir, since, now);
            let end = if cli_alive(pid, started, &starts) { Some(now) } else { last };
            if let Some(end) = end {
                runs.push(Run { ident: ident.clone(), at: since, end, convs: convs.into_iter().collect() });
            }
        }
    }
    runs
}

/// 会话 conv 在 ts 的回复是谁的：用过它、时间对得上的运行里最晚开始的那次
fn run_who<'a>(runs: &'a [Run], conv: &str, ts: f64) -> Option<&'a str> {
    runs.iter()
        .filter(|r| r.at - MARK_SLACK <= ts && ts <= r.end + MARK_SLACK && r.convs.contains(conv))
        .max_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal))
        .map(|r| r.ident.as_str())
}

// --- 读会话库 --------------------------------------------------------------------------------------------------------------

/// 共享的会话库：[(路径, 会话 id)]
pub(super) fn agy_files() -> Vec<(String, String)> {
    let dir = path::join_all(&[&agy().master, "antigravity-cli", "conversations"]);
    let mut out: Vec<(String, String)> = visible(&dir)
        .into_iter()
        .filter_map(|n| {
            let id = n.strip_suffix(".db")?;
            CONV_RE.matches(id).then(|| (path::join(&dir, &n), id.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// 库和它的 -wal 的大小、修改时间（没变就不读）
fn stamp(p: &str) -> Option<String> {
    let st = std::fs::metadata(p).ok()?;
    let wal = std::fs::metadata(format!("{p}-wal")).ok();
    Some(format!(
        "{}:{}:{}:{}:{}",
        st.size(),
        st.mtime(),
        st.mtime_nsec(),
        wal.as_ref().map_or(0, |w| w.size()),
        wal.as_ref().map_or(0, |w| w.mtime_nsec() + w.mtime() * 1_000_000_000)
    ))
}

/// URI 里的路径：% ? # 要转义
fn uri_path(p: &str) -> String {
    p.replace('%', "%25").replace('?', "%3f").replace('#', "%23")
}

/// 只读地问一个会话库：有 -wal（可能有还没并回去的）时 -readonly，开不了（没有 -shm）或者没有 -wal 时 immutable。起不来、出错是 None
fn query(db: &str, script: &str) -> Option<String> {
    let run = |target: &str| -> Option<String> {
        let done = Proc::new(["sqlite3", "-readonly", "-batch", "-bail", target])
            .input(script.as_bytes())
            .stdout(Out::Capture)
            .stderr(Out::Null)
            .timeout(30.0)
            .run()
            .ok()?;
        (done.rc == 0).then(|| done.out())
    };
    let wal = std::fs::metadata(format!("{db}-wal")).is_ok_and(|m| m.len() > 0);
    if wal {
        if let Some(out) = run(db) {
            return Some(out);
        }
    }
    run(&format!("file:{}?immutable=1", uri_path(db)))
}

fn unhex(h: &str) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok()).collect()
}

impl CostScan {
    /// 读共享的会话库里新增的回复，并进 j；live 记下读过的路径（scan.json 的 files 里不在的清掉）
    pub(super) fn agy(&mut self, live: &mut HashSet<String>, checkpoint: &mut dyn FnMut(&Seen) -> R<()>) -> R<()> {
        let files = agy_files();
        if files.is_empty() {
            return Ok(());
        }
        let mut runs: Option<Vec<Run>> = None;
        let models = self.j.dict("am").expect("scan.json 核对过");
        for (p, conv) in files {
            live.insert(p.clone());
            let Some(now_stamp) = stamp(&p) else { continue };
            let Ok(st) = std::fs::metadata(&p) else { continue };
            let mtime = shutil::st_mtime(&st);
            match self.oldest.iter_mut().find(|(t, _)| *t == "agy") {
                Some((_, m)) => *m = m.min(mtime),
                None => self.oldest.push(("agy", mtime)),
            }
            let all = self.j.dict("files").expect("scan.json 核对过");
            let same_file = |c: &Dict| {
                c.get("ino").is_some_and(|v| v.py_eq(&V::Int(st.ino() as i64)))
                    && c.get("dev").is_some_and(|v| v.py_eq(&V::Int(st.dev() as i64)))
            };
            let cur = match all.dict(&p).filter(|c| same_file(c) && c.get("s").is_some_and(|s| s.is_int())) {
                Some(c) => c,
                None => dict! { "ino" => st.ino() as i64, "dev" => st.dev() as i64, "s" => -1, "g" => -1 },
            };
            all.set(p.as_str(), &cur);
            if cur.get("st").is_some_and(|s| s.as_str() == Some(now_stamp.as_str())) {
                continue;
            }
            let (s0, g0) = (cur.at("s").as_int().unwrap_or(-1), cur.at("g").as_int().unwrap_or(-1));
            let script = format!(
                ".mode list\nselect 'g', idx, hex(data) from gen_metadata where idx > {g0} order by idx;\n\
                 select 's', idx, hex(metadata) from steps where idx > {s0} order by idx;\n"
            );
            let Some(out) = query(&p, &script) else {
                cur.set("st", now_stamp); // 读不了（不是 agy 的库、没装 sqlite3）：库变了再试
                continue;
            };
            self.files += 1;
            self.bytes += out.len() as i64;
            // hold：第一个还拿不准的那一步（新写的、还认不出账号或模型的）：后面的照样算（同一个回复去重），进度只推到它前面
            let (mut g, mut s, mut hold) = (g0, s0, None::<i64>);
            for line in out.lines() {
                let mut parts = line.splitn(3, '|');
                let (Some(kind), Some(idx), Some(h)) = (parts.next(), parts.next().and_then(text::to_int), parts.next()) else {
                    continue;
                };
                let Some(data) = unhex(h) else { continue };
                if kind == "g" {
                    if let Some((id, name)) = gen_model(&data) {
                        models.set(id.to_string(), name);
                    }
                    g = g.max(idx);
                    continue;
                }
                let (ts, r) = step_reply(&data);
                let Some(r) = r else {
                    // 不是模型回复的那一步（或者还没写完的；读不出时间的按库的修改时间）：新的先不跳过
                    if self.now - ts.unwrap_or(mtime) < AGY_DEFER {
                        hold.get_or_insert(idx);
                    } else {
                        s = s.max(idx);
                    }
                    continue;
                };
                let model = models.get(&r.model_id.to_string()).and_then(|m| m.as_str().map(agy_model));
                let runs = runs.get_or_insert_with(|| agy_runs(self.now));
                let who = run_who(runs, &conv, r.ts).map(|w| w.to_string());
                if (who.is_none() || model.is_none()) && self.now - r.ts < AGY_DEFER {
                    hold.get_or_insert(idx); // 日志、模型名可能还没写下：下次再看
                    continue;
                }
                let model = model.unwrap_or_default();
                let (micro, tok) = cost_agy(&model, r.ts, r.inp, r.out, r.cr);
                let key = (b'a', sha1_head(format!("{conv}\0{}", r.rid).as_bytes()));
                let who = who.map(|w| V::from(text::partition(&w, ":").2)).unwrap_or(V::None);
                self.add("agy", &who, Some(r.ts), key, micro, tok, &model);
                s = s.max(idx);
            }
            if let Some(h) = hold {
                s = s.min(h - 1);
            }
            cur.set("s", s);
            cur.set("g", g);
            if hold.is_none() {
                cur.set("st", now_stamp);
            } else {
                cur.pop("st");
            }
            checkpoint(&self.seen)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vint(mut x: u64, out: &mut Vec<u8>) {
        loop {
            let b = (x & 0x7f) as u8;
            x >>= 7;
            if x == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    fn fint(n: u32, x: u64, out: &mut Vec<u8>) {
        vint(u64::from(n) << 3, out);
        vint(x, out);
    }
    fn fbytes(n: u32, b: &[u8], out: &mut Vec<u8>) {
        vint((u64::from(n) << 3) | 2, out);
        vint(b.len() as u64, out);
        out.extend_from_slice(b);
    }

    pub fn usage(model: u64, rid: &str) -> Vec<u8> {
        let mut u = Vec::new();
        fint(1, model, &mut u);
        fint(2, 12080, &mut u);
        fint(3, 220, &mut u);
        fint(5, 11025, &mut u);
        fint(6, 24, &mut u);
        fbytes(7, rid.as_bytes(), &mut u);
        fint(9, 112, &mut u);
        fint(10, 108, &mut u);
        u
    }

    #[test]
    fn decodes_steps_and_gens() {
        let mut at = Vec::new();
        fint(1, 1795529662, &mut at);
        fint(2, 500_000_000, &mut at);
        let mut meta = Vec::new();
        fbytes(1, &at, &mut meta);
        fint(3, 2, &mut meta);
        fbytes(9, &usage(1526, "vpsFa5PHLISRur8P"), &mut meta);
        fint(11, 1526, &mut meta);
        let (ts, r) = step_reply(&meta);
        assert_eq!(ts, Some(1795529662.5));
        let r = r.unwrap();
        assert_eq!(r, Reply { ts: 1795529662.5, model_id: 1526, rid: "vpsFa5PHLISRur8P".into(), inp: 12080, out: 220, cr: 11025 });
        // 别的步：没有用量
        let mut other = Vec::new();
        fbytes(1, &at, &mut other);
        assert_eq!(step_reply(&other), (Some(1795529662.5), None));
        assert_eq!(step_reply(b"\xff\xff"), (None, None));

        let mut inner = Vec::new();
        fbytes(4, &usage(1526, "vpsFa5PHLISRur8P"), &mut inner);
        fbytes(19, b"gemini-4-hydrogen", &mut inner);
        fbytes(21, "Gemini 4 Hydrogen (Medium)".as_bytes(), &mut inner);
        let mut gen = Vec::new();
        fbytes(1, &inner, &mut gen);
        assert_eq!(gen_model(&gen), Some((1526, "gemini-4-hydrogen".into())));
    }

    #[test]
    fn prices() {
        assert_eq!(agy_model("claude-opus-5-5-high"), "claude-opus-5-5");
        assert_eq!(agy_model("Gemini-4-Neon"), "gemini-4-neon");
        // 1M 输入 + 1M 输出 + 1M 缓存读
        let m = 1_000_000;
        let usd = |model: &str, ts: f64| (cost_agy(model, ts, m, m, m).0.unwrap() / 1e3).round() / 1000.0;
        assert_eq!(usd("gemini-4-neon", 0.0), 18.15);
        assert_eq!(usd("gemini-4-hydrogen", 0.0), 9.075);
        assert_eq!((usd("gemini-4-carbon", 1796111999.0), usd("gemini-4-carbon", 1796112000.0)), (12.1, 24.2));
        assert_eq!((usd("gemini-4-argon", 1796111999.0), usd("gemini-4-argon", 1796112000.0)), (12.1, 24.2));
        // Claude 的模型按 Anthropic 的标价：opus 5.5 输入 4、输出 20、缓存读 0.20
        assert_eq!(usd("claude-opus-5-5", 0.0), 24.2);
        assert_eq!(cost_agy("gemini-9-unknown", 0.0, m, m, m), (None, [m, m, m, 0]));
    }

    #[test]
    fn who_is_the_run_that_used_the_conversation() {
        let run = |ident: &str, at: f64, end: f64, c: &[&str]| Run {
            ident: ident.into(),
            at,
            end,
            convs: c.iter().map(|x| x.to_string()).collect(),
        };
        let runs = vec![run("agy:a@example.com", 100.0, 200.0, &["c1"]), run("agy:b@example.com", 150.0, 300.0, &["c2", "c1"])];
        assert_eq!(run_who(&runs, "c1", 120.0), Some("agy:a@example.com"));
        assert_eq!(run_who(&runs, "c1", 160.0), Some("agy:b@example.com")); // 两次都对得上：最晚开始的
        assert_eq!(run_who(&runs, "c2", 120.0), None);
        assert_eq!(run_who(&runs, "c1", 302.0), Some("agy:b@example.com"));
        assert_eq!(run_who(&runs, "c1", 303.0), None);
    }

    #[test]
    fn log_names() {
        let t = log_time("cli-20261124_215602.log").unwrap();
        assert_eq!(t, mktime(2026, 11, 24, 21, 56, 2, -1));
        assert!(log_time("acp-login-20261112_172531-86873.log").is_none());
        assert!(log_time("cli.log").is_none());
    }
}
