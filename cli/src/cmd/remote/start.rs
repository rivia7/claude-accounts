//! 远程启动的这一头（`remote-launch`，`cca remote launch`）：配对过的另一台机器允许这台机器用它的模板新建会话（那台机器上
//! `cca remote allow 这台机器的名字 launch on`）时，挑一个模板让它起一个会话、接进去——和 iPhone 上的 cca App 做的一样
//! （docs/dev/launch.md）。那台机器上的启动器在服务器上是一个“会话”（`far::FarLauncher`）：照接入会话那样要票据、连数据端口、
//! 握手，头一帧说要起哪个（LAUNCH，不发 HELLO），等它回话（LAUNCHED），它回完就断开。起来的是一个普通的会话，接着照
//! `cca remote attach` 那样接进去；刚起的可能还没在服务器上登记好：头 45 秒里隔 1.5 秒再问一次（同 iOS 的 `Attachment` 的
//! `freshFor`、`freshGap`）。线上的格式见 server/README.md「启动器」。
//!
//! - `remote-launch`：列出配对过的机器上能用的模板，一行一个：`机器  编号  名字  工具  账号  文件夹  参数`
//! - `remote-launch [机器] 模板 [--line 开场白] [--size 列x行]`：起一个、接进去（要在终端里：先查，不在终端里什么都不起）。
//!   机器认配对的名字、配对编号的前几位（同 `cca remote allow`）；模板认编号、名字；只有一台机器上有这个模板时不用写机器
//! - 加 `--json`（界面 App，PROTOCOL 9.5）：起一个、等它在服务器上登记好，stdout 一份文档（新会话的编号、`remote list --json` 里
//!   它那一项），不接进去。`--digest 摘要`：App 列出来时看到的模板的摘要（那台机器上改过了就不起：`changed`）。失败时设了
//!   `CCA_EVENTS` 的话另记一条 `fail` 事件（原因的话和 stderr 的一样）
//!
//! 退出码：64 用法不对、对上了好几个，65 没有这台机器、这个模板，那台机器没让这台机器远程启动（或者它的启动器不在），66 经服务器没成
//! （连不上、那台机器不在线、没等到回话，起来的会话 45 秒还没登记上），67 那台机器不起（模板改过了、会话太多……原因写在 stderr），
//! 69 不在终端里；接进去以后同 `cca remote attach`。

use super::attach::{self, EXIT_NOT_FOUND, EXIT_NO_TTY, EXIT_RELAY, EXIT_USAGE};
use super::far::{self, clean, Far, FarLauncher, FarTemplate};
use super::local::{self, Decoder};
use super::manage::far_entry;
use super::peers::{self, Peer};
use super::pty::{self, Size};
use super::relay::{self, Server, Tunnel};
use crate::dict;
use crate::py::json;
use crate::py::obj::V;
use crate::py::time::{monotonic, sleep};
use crate::py::{exit, R};
use std::io::Write;

/// 那台机器不起（LAUNCHED 说了 `ok: false`）
pub const EXIT_REFUSED: i32 = 67;
/// 发了 LAUNCH 以后最多等这么久回话（启动器自己等 LAUNCH 20 秒、起会话最多 4 秒）
const ANSWER_WAIT: f64 = 25.0;
/// 起来的会话最多等这么久在服务器上登记好（iOS 的 `Attachment.freshFor`）
const FRESH_FOR: f64 = 45.0;
/// 等的时候隔这么久问一次（`freshGap`；服务器给每台设备限了流：每 2 秒 1 次、最多攒 30 次）
const FRESH_GAP: f64 = 1.5;
/// 新会话的大小的范围（和启动器的 `Ask::parse` 一样）；App 没说、问不到终端的大小时用 DEFAULT_SIZE
const COLS: std::ops::RangeInclusive<u16> = 20..=500;
const ROWS: std::ops::RangeInclusive<u16> = 5..=300;
const DEFAULT_SIZE: Size = Size { cols: 120, rows: 36 };
/// 开场白最多几个字（同 launch.rs 的 LINE_MAX）
const LINE_MAX: usize = 500;

const USAGE: &str = "用法：cca remote launch [机器] [模板] [--line 开场白]（不写模板是列出能用的；起好以后接进去）";

fn usage() -> R<()> {
    crate::err!("{USAGE}");
    exit(EXIT_USAGE)
}

/// 没成：原因写到 stderr；给界面 App 跑的（设了 CCA_EVENTS）另记一条 fail 事件（PROTOCOL 4.1）。返回退出码
fn fail(code: i32, why: &str) -> R<()> {
    crate::err!("{why}");
    if let Some(events) = crate::py::proc::getenv("CCA_EVENTS").filter(|s| !s.is_empty()) {
        let ev = dict! { "v" => 1, "ev" => "fail", "code" => "error", "message" => why };
        let line = json::dumps(&V::Dict(ev), json::Fmt::new().utf8().compact()) + "\n";
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(events) {
            let _ = f.write_all(line.as_bytes());
        }
    }
    exit(code)
}

/// 开场白能不能发：不是空白，不超过 500 个字，没有控制字符（换行、制表符也算）
fn line_ok(s: &str) -> bool {
    s.chars().count() <= LINE_MAX && !s.trim().is_empty() && !s.chars().any(char::is_control)
}

/// `列x行`
fn parse_size(s: &str) -> Option<Size> {
    let (c, r) = s.split_once('x')?;
    Some(Size { cols: c.parse().ok()?, rows: r.parse().ok()? })
}

fn clamp(s: Size) -> Size {
    Size { cols: s.cols.clamp(*COLS.start(), *COLS.end()), rows: s.rows.clamp(*ROWS.start(), *ROWS.end()) }
}

/// 列出来的一行：`机器  编号  名字  工具  账号  文件夹  参数`（没有参数的没有最后那一截）
fn row(l: &FarLauncher, t: &FarTemplate) -> String {
    let mut s = format!("{}  {}  {}  {}  {}  {}", l.peer, t.id, t.name, t.tool, t.account, t.dir);
    if !t.args.is_empty() {
        s.push_str("  ");
        s.push_str(&t.args);
    }
    s
}

/// 启动器说不起的原因（LAUNCHED 的 code）：照这一头说（那台机器写的 text 是给手机看的，不认识的 code 才用它）
fn refused_text(code: &str, text: &str) -> String {
    let s = match code {
        "denied" => "那台机器现在不让这台机器远程启动会话（在那台机器上运行 cca remote allow 这台机器的名字 launch on）",
        "no_template" => "那台机器上没有这个模板了（cca remote launch 看现在有哪些）",
        "changed" => "模板在那台机器上改过了：刷新一下再起",
        "no_account" => "模板的账号在那台机器的账号表里没有了",
        "no_dir" => "模板的文件夹在那台机器上没有了",
        "bad_args" => "模板的参数在那台机器上用不了（启动前的检查过不了）",
        "other_hub" => "模板的账号现在走别的 Team 的服务器：在那个 Team 的配对上起",
        "busy" => "那台机器上开着的会话太多了：先结束几个",
        "rate" => "起得太勤了：过一会儿再试",
        "spawn" => "会话没起来（在那台机器上运行 cca remote launcher log 看看）",
        "bad" => "那台机器看不懂这个请求（两边的 cca 版本差得太多：更新一下）",
        _ if !text.is_empty() => return text.to_string(),
        _ => return format!("那台机器没起（{code}）"),
    };
    s.to_string()
}

/// LAUNCHED 的内容：Ok(新会话的编号)，或者 Err((code, 那台机器写的原因))；样子不对是 None
fn launched(p: &[u8]) -> Option<Result<String, (String, String)>> {
    let d = json::loads(std::str::from_utf8(p).ok()?).ok()?.as_dict()?;
    if d.get("v").and_then(|v| v.as_int()) != Some(1) {
        return None;
    }
    if d.get("ok")?.as_bool()? {
        return Some(Ok(d.str("sid").filter(|s| peers::hex32(s))?.to_string()));
    }
    let code = d.str("code").filter(|c| (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))?;
    Some(Err((code.to_string(), d.str("text").map(|t| clean(&t, 200)).unwrap_or_default())))
}

/// 等启动器回话（LAUNCHED；别的帧不理），最多 ANSWER_WAIT 秒。Err：断了、没等到、看不懂（原因）
fn answer(tun: &mut Tunnel) -> Result<Result<String, (String, String)>, &'static str> {
    let end = monotonic() + ANSWER_WAIT;
    let mut dec = Decoder::default();
    loop {
        loop {
            match dec.next() {
                Ok(Some((local::LAUNCHED, p))) => return launched(&p).ok_or("那台机器的启动器回的话看不懂"),
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(()) => return Err("那台机器的启动器回的话看不懂"),
            }
        }
        let left = end - monotonic();
        if left <= 0.0 {
            return Err("等了一阵那台机器的启动器没回话（它不在线，或者网络不通）");
        }
        if tun.flush().is_err() {
            return Err("还没回话连接就断了");
        }
        let mut p = libc::pollfd { fd: tun.fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: p 是有效的一项
        unsafe { libc::poll(&mut p, 1, (left * 1000.0).clamp(1.0, 1000.0) as i32) };
        if p.revents != 0 {
            match tun.recv() {
                Ok(Some(data)) => dec.push(&data),
                _ => return Err("还没回话连接就断了"),
            }
        }
    }
}

/// 起来的会话等它在服务器上登记好（连着服务器）；FRESH_FOR 秒还没有是 None
fn fresh(srv: &Server, peer: &Peer, sid: &str) -> Option<Far> {
    let end = monotonic() + FRESH_FOR;
    loop {
        if let Ok(Ok(mut l)) = far::fetch(srv, std::slice::from_ref(peer), 8) {
            far::rename_here(peer.hub, &mut l.sessions, &mut []);
            if let Some(f) = l.sessions.into_iter().find(|f| f.sid == sid && f.online) {
                return Some(f);
            }
        }
        if monotonic() + FRESH_GAP >= end {
            return None;
        }
        sleep(FRESH_GAP);
    }
}

/// `remote-launch …`（见开头）
pub fn cmd_remote_launch(args: &[String]) -> R<()> {
    let mut pos: Vec<&str> = Vec::new();
    let (mut line, mut size, mut digest, mut as_json) = (None::<String>, None::<Size>, None::<String>, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" if !as_json => as_json = true,
            "--line" | "--size" | "--digest" => {
                let Some(v) = it.next() else { return usage() };
                match a.as_str() {
                    "--line" if line.is_none() => line = Some(v.clone()),
                    "--size" if size.is_none() => match parse_size(v) {
                        Some(s) => size = Some(s),
                        None => return usage(),
                    },
                    "--digest" if digest.is_none() && peers::lower_hex(v, 16) => digest = Some(v.clone()),
                    _ => return usage(),
                }
            }
            "--" => {
                pos.extend(it.by_ref().map(String::as_str));
                break;
            }
            s if s.starts_with("--") => return usage(),
            s => pos.push(s),
        }
    }
    if pos.len() > 2 || (pos.is_empty() && (as_json || line.is_some() || size.is_some() || digest.is_some())) {
        return usage();
    }
    let all = peers::load();
    if all.is_empty() {
        let why = "还没有配对过的机器：在要远程启动会话的那台上运行 cca remote pair，在这台上运行 cca remote join";
        if pos.is_empty() {
            crate::err!("{why}");
            return Ok(());
        }
        return fail(EXIT_NOT_FOUND, why);
    }
    let seen = far::fetch_all(&all, 8)?;
    for (_, why, asked) in &seen.errors {
        crate::err!("别的机器上的模板{}：{why}", if asked.is_some() { "没查到" } else { "没查" });
    }

    // 不写模板：列出能用的
    if pos.is_empty() {
        if seen.launchers.iter().all(|l| l.templates.is_empty()) {
            crate::err!("配对过的机器上现在都没有能用的模板（那台机器上要先 cca remote template add，再 cca remote allow 这台机器的名字 launch on）");
            return Ok(());
        }
        for l in &seen.launchers {
            for t in &l.templates {
                crate::out!("{}", row(l, t));
            }
            if !l.online {
                crate::err!("「{}」暂时没连着服务器：它上面的模板现在起不了", l.peer);
            }
            if l.more > 0 {
                crate::err!("「{}」上还有 {} 个模板没列出来（说明里放不下）", l.peer, l.more);
            }
        }
        return Ok(());
    }

    // 哪台机器上的哪个模板
    let (who, wanted) = match pos.as_slice() {
        [t] => (None, *t),
        [m, t] => (Some(*m), *t),
        _ => return usage(),
    };
    let launchers: Vec<&FarLauncher> = match who {
        None => seen.launchers.iter().collect(),
        Some(m) => {
            let peer = match peers::find(&all, m) {
                Ok(p) => p,
                Err(why) => return fail(EXIT_NOT_FOUND, &why),
            };
            let mine: Vec<&FarLauncher> = seen.launchers.iter().filter(|l| l.pid == peer.pid).collect();
            if mine.is_empty() && seen.asked.contains(&peer.pid) {
                let why = format!(
                    "「{}」现在没让这台机器远程启动会话（那台机器上要运行 cca remote allow 这台机器的名字 launch on，它的启动器也要在运行）",
                    peer.name
                );
                return fail(EXIT_NOT_FOUND, &why);
            }
            if mine.is_empty() {
                return fail(EXIT_RELAY, &format!("没问到「{}」上有哪些模板", peer.name));
            }
            mine
        }
    };
    let low = wanted.to_ascii_lowercase();
    let hits: Vec<(&FarLauncher, &FarTemplate)> = launchers
        .iter()
        .flat_map(|l| l.templates.iter().map(move |t| (*l, t)))
        .filter(|(_, t)| t.name == wanted || t.id == low)
        .collect();
    let (l, t) = match hits.as_slice() {
        [one] => *one,
        [] => return fail(EXIT_NOT_FOUND, &format!("没有叫「{wanted}」的模板（cca remote launch 看有哪些）")),
        _ => {
            crate::err!("有 {} 个模板对得上：说一下是哪台机器上的（cca remote launch 机器 模板）", hits.len());
            for (l, t) in &hits {
                crate::err!("  {}", row(l, t));
            }
            return fail(EXIT_USAGE, "对上了好几个模板");
        }
    };
    // 终端里的：起之前先看是不是在终端里（不然起了接不进去）
    if !as_json && (!pty::isatty(0) || !pty::isatty(1)) {
        return fail(EXIT_NO_TTY, "cca remote launch 要在终端里运行（起好以后接进去）");
    }
    if let Some(s) = &line {
        if !line_ok(s) {
            return fail(EXIT_USAGE, &format!("开场白不能是空白、不能超过 {LINE_MAX} 个字、不能有控制字符（换行也不行）"));
        }
        if !t.line {
            return fail(EXIT_USAGE, &format!("「{}」不能带开场白（参数里已经有一句话了，或者这个工具不认）", t.name));
        }
    }
    if !l.online {
        return fail(EXIT_RELAY, &format!("「{}」暂时没连着服务器：过一会儿再试", l.peer));
    }
    let size = clamp(size.or_else(|| if as_json { None } else { pty::get_size(0) }).unwrap_or(DEFAULT_SIZE));
    let (Some(peer), Some(secret)) = (all.iter().find(|p| p.pid == l.pid), peers::secret(&l.pid)) else {
        return fail(EXIT_RELAY, &format!("和「{}」配对的密钥读不到了：两边重新配对（cca remote pair / cca remote join）", l.peer));
    };
    let srv = match relay::server_of(peer.hub)? {
        Ok(s) => s,
        Err(why) => return fail(EXIT_RELAY, &why),
    };

    // 进启动器、说要起哪个、等回话
    crate::err!("正在「{}」上用模板「{}」新建会话…", l.peer, t.name);
    let mut tun = match attach::dial(&l.sid, peer, &secret, &srv)? {
        Ok(tun) => tun,
        Err(m) if m.code == EXIT_NOT_FOUND && !m.dialed => {
            return fail(EXIT_NOT_FOUND, &format!("「{}」的启动器现在不在（那边刚关掉了远程启动，或者那台机器不在线）", l.peer));
        }
        Err(m) => return fail(m.code, &format!("{}{}", if m.dialed { "没进得了启动器：" } else { "" }, m.why)),
    };
    let ask = dict! {
        "v" => 1, "t" => t.id.as_str(), "h" => digest.as_deref().unwrap_or(&t.digest),
        "c" => i64::from(size.cols), "r" => i64::from(size.rows)
    };
    if let Some(s) = &line {
        ask.set("l", s.as_str());
    }
    tun.send(&local::frame(local::LAUNCH, json::dumps(&V::Dict(ask), json::Fmt::new().utf8().compact()).as_bytes()));
    let got = answer(&mut tun);
    tun.close();
    let sid = match got {
        Ok(Ok(sid)) => sid,
        Ok(Err((code, text))) => return fail(EXIT_REFUSED, &format!("没起成：{}", refused_text(&code, &text))),
        Err(why) => return fail(EXIT_RELAY, why),
    };

    // 起来了：等它在服务器上登记好
    if !as_json {
        crate::err!("起好了会话 {}，等它在服务器上登记好…", local::short(&sid));
    }
    let found = fresh(&srv, peer, &sid);
    if as_json {
        let session = found.as_ref().map_or(V::None, |f| V::Dict(far_entry(f, &all)));
        let doc = dict! { "version" => 1, "kind" => "remote_launch", "id" => sid.as_str(), "session" => session };
        crate::out!("{}", json::dumps(&V::Dict(doc), json::LINE));
        return Ok(());
    }
    let Some(f) = found else {
        let short = local::short(&sid);
        let why = format!(
            "会话 {short} 起来了，但 {FRESH_FOR:.0} 秒里服务器上一直没有它（过一会儿 cca remote list 看看，再 cca remote attach {short}）"
        );
        return fail(EXIT_RELAY, &why);
    };
    attach::attach_launched(f, srv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_from_the_launcher() {
        let sid = "0123456789abcdef0123456789abcdef";
        assert_eq!(launched(format!(r#"{{"v":1,"ok":true,"sid":"{sid}"}}"#).as_bytes()), Some(Ok(sid.to_string())));
        assert_eq!(
            launched(r#"{"v":1,"ok":false,"code":"changed","text":"改过了\u001b[2J"}"#.as_bytes()),
            Some(Err(("changed".to_string(), "改过了[2J".to_string()))),
            "那台机器写的字去掉控制字符"
        );
        for bad in [
            r#"{"v":2,"ok":true,"sid":"0123456789abcdef0123456789abcdef"}"#,
            r#"{"v":1,"ok":true,"sid":"0123456789ABCDEF0123456789ABCDEF"}"#,
            r#"{"v":1,"ok":true}"#,
            r#"{"v":1,"ok":false,"code":"Changed"}"#,
            r#"{"v":1,"ok":false,"code":""}"#,
            r#"{"v":1,"ok":"yes","sid":"0123456789abcdef0123456789abcdef"}"#,
            r#"{"ok":true,"sid":"0123456789abcdef0123456789abcdef"}"#,
            "[]",
            "not json",
        ] {
            assert_eq!(launched(bad.as_bytes()), None, "{bad}");
        }
        // 认识的 code 照这一头说；不认识的用那台机器写的，什么都没写的说 code
        assert!(refused_text("changed", "模板在那台机器上改过了：刷新一下再选").contains("刷新一下再起"));
        assert_eq!(refused_text("new_code", "那边说的"), "那边说的");
        assert_eq!(refused_text("new_code", ""), "那台机器没起（new_code）");
        for code in ["denied", "no_template", "changed", "no_account", "no_dir", "bad_args", "other_hub", "busy", "rate", "spawn"] {
            assert_ne!(refused_text(code, ""), refused_text("bad", ""), "{code}");
        }
    }

    #[test]
    fn sizes_and_lines() {
        assert_eq!(parse_size("120x36"), Some(Size { cols: 120, rows: 36 }));
        for bad in ["120", "x36", "120x", "a x b", "-1x5", "120X36"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
        assert_eq!(clamp(Size { cols: 5, rows: 1000 }), Size { cols: 20, rows: 300 });
        assert_eq!(clamp(Size { cols: 900, rows: 2 }), Size { cols: 500, rows: 5 });
        assert!(line_ok("看看 README") && line_ok(&"字".repeat(LINE_MAX)));
        assert!(!line_ok(&"字".repeat(LINE_MAX + 1)) && !line_ok("  ") && !line_ok("a\nb") && !line_ok("a\tb") && !line_ok(""));
    }
}
