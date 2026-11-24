//! 扮演界面 App 的提问会话（原来的 tests/gui_session.py；macos/PROTOCOL.md 4.6）：读事件文件里的 ask、照答案表从 fd 3 的管道回答。
//!
//! 命令在新会话里运行，fd 3 是答案管道的读端（和 App 一样），stdin 是 /dev/null；边看它写的事件文件（CCA_EVENTS），边照答案表
//! 回答 ask 事件，每 20 毫秒看一次。回归 10h 的 sess（`cca-test gui-session`，tests/tool/gui_session.rs）用它，以后 remote_relay、
//! web_relay 改成 Rust 时也用它。
//!
//! 答案表：`{"题号": [第一次问时的答案, 第二次问时的答案…]}`，每次的答案是 `{"字段 id": "值"}`；写 null 的那一次不回答（cca 边等
//! 边干活、会自己发下一页的那种页，比如显示配对码、等对方输入的那一页）。问到答案表里没有的题号、这个题号的答案用完了，关掉管道
//! （cca 按取消处理）。每个字段回一行 `序号 字段id 值`（只显示的 code 不回）；值照 Python 的 str() 写（true 写成 True、数字照
//! Python 的写法……和原来一字不差）。选项见 [`Opts`]。
//!
//! ```ignore
//! let table = json::parse(r#"{"team.new":[{"name":"机房组"}]}"#).unwrap();
//! let mut cmd = Cmd::new(&cca);
//! cmd.args(["team", "new"]).envs(env).env("CCA_EVENTS", &events).env("CCA_PROMPT", "1");
//! let code = gui_session::run(&mut cmd, &events, &table, &Opts::default())?;   // 命令的退出码
//! ```

use crate::json::{self, Value};
use crate::proc::{Cmd, Session, Stdio};
use crate::py::{py_str, truthy};
use std::fs::File;
use std::io::{self, PipeWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

/// 怎么回答（原来的几个命令行选项）
#[derive(Clone, Debug, Default)]
pub struct Opts {
    /// --eof-at：问到这个题号时关掉管道（不回答）
    pub eof_at: Option<String>,
    /// --stale：每次回答前把上一页的答案再发一遍（像点了两下：序号对不上，cca 要丢掉）
    pub stale: bool,
    /// --wrong-field：回答这个题号时字段 id 一律写成 wrong
    pub wrong_field: Option<String>,
    /// --keep：答案里没写的字段按页面上现有的值（value，没有就 default）回答（字段多的表单）；不带时回空的
    pub keep: bool,
}

/// 运行 cmd（stdin、fd 3、新会话在这里设），照答案表回答，等它退出。返回它的退出码（收到信号时是 128 + 信号）。
/// 事件文件读不了、里面有读不懂的一行，答案表的样子不对：返回错误（命令这时还在运行的话停掉它）
pub fn run(cmd: &mut Cmd, events: &Path, table: &Value, opts: &Opts) -> io::Result<i32> {
    let (read, write) = io::pipe()?;
    let mut child = cmd.stdin(Stdio::Null).fd(3, read.into()).session(Session::New).spawn()?;
    let mut app = App { pipe: Some(write), last: String::new(), used: Vec::new() };
    let (mut offset, mut buf) = (0u64, Vec::new());
    loop {
        let chunk = read_from(events, offset)?;
        offset += chunk.len() as u64;
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let row: Vec<u8> = buf.drain(..=nl).collect();
            let row = &row[..nl];
            let e = json::parse_bytes(row)
                .map_err(|err| bad(format!("事件文件里有一行读不懂（{err}）：{}", String::from_utf8_lossy(row))))?;
            app.on_event(&e, table, opts)?;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(app.pipe.take());
    Ok(child.wait()?.shell())
}

/// 事件文件从 offset 起新写的
fn read_from(events: &Path, offset: u64) -> io::Result<Vec<u8>> {
    let what = |e: io::Error| io::Error::new(e.kind(), format!("读不了事件文件 {}：{e}", events.display()));
    let mut f = File::open(events).map_err(what)?;
    f.seek(SeekFrom::Start(offset)).map_err(what)?;
    let mut chunk = Vec::new();
    f.read_to_end(&mut chunk).map_err(what)?;
    Ok(chunk)
}

fn bad(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

/// 事件、字段里一定要有的一项
fn need<'a>(v: &'a Value, key: &str) -> io::Result<&'a Value> {
    v.get(key).ok_or_else(|| bad(format!("{} 里没有 {key}", v.compact())))
}

/// App 这一头
struct App {
    /// 答案管道的写端；关掉了是 None
    pipe: Option<PipeWriter>,
    /// 上一次回答的（--stale 再发一遍）
    last: String,
    /// 每个题号问过几次（键是题号的 JSON）
    used: Vec<(String, usize)>,
}

impl App {
    fn on_event(&mut self, e: &Value, table: &Value, opts: &Opts) -> io::Result<()> {
        if e.as_obj().is_none() {
            return Err(bad(format!("事件不是对象：{}", e.compact())));
        }
        if e.get("ev").and_then(Value::as_str) != Some("ask") || self.pipe.is_none() {
            return Ok(());
        }
        let q = need(e, "q")?;
        if matches!(q, Value::List(_) | Value::Obj(_)) {
            return Err(bad(format!("题号不是字符串：{}", q.compact())));
        }
        let i = self.count(q.compact());
        let table = table.as_obj().ok_or_else(|| bad(format!("答案表不是对象：{}", table.compact())))?;
        let qs = q.as_str();
        let rounds: &[Value] = match qs.and_then(|q| table.get(q)) {
            Some(Value::List(l)) => l,
            Some(v) if truthy(v) => return Err(bad(format!("答案表里 {} 的值不是列表：{}", q.compact(), v.compact()))),
            _ => &[],
        };
        if (opts.eof_at.is_some() && opts.eof_at.as_deref() == qs) || i >= rounds.len() {
            self.pipe = None;
            return Ok(());
        }
        let ans = &rounds[i];
        if ans.is_null() {
            return Ok(()); // 这一页不回答：等 cca 自己发下一页
        }
        if opts.stale && !self.last.is_empty() {
            let last = self.last.clone();
            self.say(&last)?; // 上一页的答案又发了一遍（比如点了两下）：序号对不上，要丢掉
        }
        let fields = need(e, "fields")?.as_list().ok_or_else(|| bad(format!("fields 不是列表：{}", e.compact())))?;
        let mut text = String::new();
        for field in fields {
            if need(field, "kind")?.as_str() == Some("code") {
                continue; // 只显示，不用回答
            }
            let fid = need(field, "id")?;
            let seq = py_d(need(e, "seq")?)?;
            let id =
                if opts.wrong_field.is_some() && opts.wrong_field.as_deref() == qs { "wrong".to_string() } else { py_str(fid) };
            let ans = ans.as_obj().ok_or_else(|| bad(format!("答案不是对象：{}", ans.compact())))?;
            let empty = Value::Str(String::new());
            let fallback = if opts.keep {
                [field.get("value"), field.get("default")].into_iter().flatten().find(|v| truthy(v)).unwrap_or(&empty)
            } else {
                &empty
            };
            let value = fid.as_str().and_then(|k| ans.get(k)).unwrap_or(fallback);
            text.push_str(&format!("{seq} {id} {}\n", py_str(value)));
        }
        self.say(&text)?;
        self.last = text;
        Ok(())
    }

    /// 这个题号是第几次问（从 0 起）
    fn count(&mut self, key: String) -> usize {
        match self.used.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => {
                e.1 += 1;
                e.1 - 1
            }
            None => {
                self.used.push((key, 1));
                0
            }
        }
    }

    /// 写进答案管道；cca 已经不读了（管道断了）不算错
    fn say(&mut self, text: &str) -> io::Result<()> {
        if let Some(w) = self.pipe.as_mut() {
            match w.write_all(text.as_bytes()) {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {}
                other => other?,
            }
        }
        Ok(())
    }
}

/// Python 的 '%d' % 序号：小数截掉小数部分，true 是 1；别的（字符串、null、无穷大）算错
fn py_d(v: &Value) -> io::Result<String> {
    match v {
        Value::Int(i) => Ok(i.to_string()),
        Value::Bool(b) => Ok(if *b { "1" } else { "0" }.to_string()),
        Value::Float(f) if f.is_finite() => {
            let s = format!("{:.0}", f.trunc());
            Ok(if s == "-0" { "0".to_string() } else { s })
        }
        _ => Err(bad(format!("序号不是数字：{}", v.compact()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_like_percent_d() {
        let d = |s: &str| py_d(&json::parse(s).unwrap()).ok();
        assert_eq!(d("12"), Some("12".to_string()));
        assert_eq!(d("3.9"), Some("3".to_string()));
        assert_eq!(d("-0.5"), Some("0".to_string()));
        assert_eq!(d("-2.5"), Some("-2".to_string()));
        assert_eq!(d("true"), Some("1".to_string()));
        assert_eq!(d(r#""1""#), None);
        assert_eq!(d("null"), None);
    }
}
