//! 盯邮箱、打开过的登录邮件的记录、把链接交出去（`mail_used` 到 `mail_deliver`）。
//!
//! 打开过的登录邮件：MAIL_DIR/used.json（{摘要: {at, st: claim | done, pid}}，只记摘要，两天前的顺手清掉），先认领再打开：
//! 两个 cca 同时盯着同一个账号（两个终端都在登录它、cca mail open 和登录时的同时）只打开一次。
//! 链接是一次性的登录凭证：只经 stdin 交给 `cca _mail-open`，不打印、不记日志、不存盘、不进命令行和环境变量。

use super::imap::{Cand, Source};
use super::judge::{mail_digest, mail_extract, mail_judge, mail_judge_any};
use super::message::{mail_parse, Message};
use super::{check_interrupt, interrupted, keyboard_interrupt, mail_error, mail_knob, mail_why, py_int, MailRule};
use crate::cmd::presence::BG_ENV_DROP;
use crate::core::base::SELF_PATH;
use crate::dict;
use crate::py::fs;
use crate::py::json;
use crate::py::obj::{Dict, V};
use crate::py::path;
use crate::py::proc::{environ, Cmd, Out};
use crate::py::time;
use crate::py::{Fail, R};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

/// `mail_claim` 的三种结果
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Claim {
    /// 不该再打开（打开过、别的 cca 正在打开它）：原来的 False
    Taken,
    /// 认领到了，原来没有记录（没打开时把认领去掉）：原来的 None
    Fresh,
    /// 认领到了，原来是打开过的（--again；没打开时还原成 done）
    Done,
}

/// `now - at`，at 是 used.json 里记的时间：不是数是 None（布尔值也算数，Python 里就这样）；整数大得放不进 float 时
/// 原来是没人接的 OverflowError
fn age(now: f64, at: Option<&V>) -> R<Option<f64>> {
    match at {
        Some(big @ V::Big(_)) => match big.as_num() {
            Some(x) if x.is_finite() => Ok(Some(now - x)),
            _ => Err(Fail::Other("OverflowError", vec!["int too large to convert to float".into()])),
        },
        Some(n) if n.isinstance_num() => Ok(n.as_pynum().map(|x| now - x)),
        _ => Ok(None),
    }
}

/// 拿着 used.lock 的锁读 used.json（两天前的、不成样子的条目去掉），交给 f 改，改过了才存
pub fn mail_used<T>(dir: &str, f: impl FnOnce(&Dict, f64) -> R<T>) -> R<T> {
    fs::makedirs_mode(dir, 0o700)?;
    let lock = std::fs::OpenOptions::new().append(true).create(true).mode(0o666).open(path::join(dir, "used.lock"))?;
    loop {
        // SAFETY: lock 开着，fd 有效
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e.into());
        }
        check_interrupt()?;
    }
    let p = path::join(dir, "used.json");
    let old = fs::load_quiet(&p);
    let now = time::time();
    let d = Dict::new();
    if let Some(V::Dict(old)) = old {
        for (k, v) in old.items() {
            let at = v.as_dict().and_then(|e| e.get("at"));
            if age(now, at.as_ref())?.is_some_and(|a| a < 2.0 * 86400.0) {
                d.set(k, v);
            }
        }
    }
    let snapshot = |d: &Dict| json::dumps(&V::Dict(d.clone()), json::Fmt::new().sort_keys());
    let before = snapshot(&d);
    let r = f(&d, now)?;
    if snapshot(&d) != before {
        fs::save_json(&p, &V::Dict(d), Some(0o600), true)?;
    }
    Ok(r)
}

/// `os.kill(int(pid), 0)` 成不成：进程在不在（没权限发信号的也算不在，原来就这样）。pid 是 used.json 里别人写的值
pub fn mail_pid_alive(pid: Option<&V>) -> R<bool> {
    let overflow = || Fail::Other("OverflowError", vec!["Python int too large to convert to C int".into()]);
    let n: i64 = match pid {
        Some(V::Bool(b)) => i64::from(*b),
        Some(V::Int(i)) => *i,
        Some(V::Big(_)) => return Err(overflow()),
        Some(V::Float(f)) if f.is_nan() => return Ok(false),
        Some(V::Float(f)) if f.is_infinite() => {
            return Err(Fail::Other("OverflowError", vec!["cannot convert float infinity to integer".into()]))
        }
        Some(V::Float(f)) if f.abs() < 9.0e18 => f.trunc() as i64,
        Some(V::Float(_)) => return Err(overflow()),
        Some(V::Str(s)) => match py_int(s) {
            None => return Ok(false),
            Some((neg, digits)) => match digits.parse::<i64>() {
                Ok(n) if neg => -n,
                Ok(n) => n,
                Err(_) => return Err(overflow()),
            },
        },
        _ => return Ok(false),
    };
    let Ok(pid) = libc::pid_t::try_from(n) else {
        return Err(overflow());
    };
    // SAFETY: 信号 0 只是问这个进程在不在，不发信号
    Ok(unsafe { libc::kill(pid, 0) } == 0)
}

/// 认领这封邮件：不该再打开（打开过、别的 cca 正在打开它）是 Taken，否则给出原来的状态（没打开时照它还原）
pub fn mail_claim(dir: &str, dg: &str, again: bool, me: u32) -> R<Claim> {
    mail_used(dir, |d, now| {
        let v = d.dict(dg).filter(|v| !v.is_empty()).unwrap_or_default();
        let st = v.get("st");
        let is = |want: &str| st.as_ref().and_then(|s| s.as_str()) == Some(want);
        if is("done") && !again {
            return Ok(Claim::Taken);
        }
        if is("claim") {
            let recent = age(now, v.get("at").as_ref())?.is_some_and(|a| a < 120.0);
            let other = !v.get("pid").is_some_and(|p| p.py_eq(&V::Int(i64::from(me))));
            if recent && other && mail_pid_alive(v.get("pid").as_ref())? {
                return Ok(Claim::Taken);
            }
        }
        d.set(dg, dict! { "at" => now, "st" => "claim", "pid" => me });
        Ok(if is("done") { Claim::Done } else { Claim::Fresh })
    })
}

/// 记下这封邮件的状态（st 是 None：去掉记录）
pub fn mail_mark(dir: &str, dg: &str, st: Option<&str>) -> R<()> {
    mail_used(dir, |d, now| {
        match st.filter(|s| !s.is_empty()) {
            Some(st) => d.set(dg, dict! { "at" => now, "st" => st }),
            None => {
                d.pop(dg);
            }
        }
        Ok(())
    })
}

/// 盯邮箱时不变的那几样（原来是 mail_hunt 的参数；测试里换掉时钟和记录用的目录）
pub struct Hunt<'a> {
    /// used.json 所在的目录（MAIL_DIR；测试里是临时目录）
    pub dir: &'a str,
    pub rules: &'a [&'static MailRule],
    /// 账号的邮箱（小写）
    pub acct: &'a str,
    /// 这个账号信任的转发中转（mail::mail_forwarders；默认一个都没有）
    pub forwarders: &'a [String],
    /// 打开过的也再打开一次
    pub again: bool,
    /// 手动（cca mail open）：打开了、浏览器打不开就停，第一次就连不上马上停
    pub once: bool,
    pub now: &'a dyn Fn() -> f64,
    pub sleep: &'a dyn Fn(f64),
    /// 这个进程的进程号（认领时记下）
    pub pid: u32,
    /// 几秒看一次邮箱（平时 3 秒）、连不上 Gmail 和浏览器没打开时先等多久（平时 2 秒）：命令里照 `Hunt::knobs()` 取
    /// （测试用的开关 CCA_MAIL_POLL、CCA_MAIL_RETRY），单元测试直接给，不看进程的环境变量
    pub poll: f64,
    pub base: f64,
}

impl Hunt<'_> {
    /// (poll, base)：`mail_knob('CCA_MAIL_POLL', 3), mail_knob('CCA_MAIL_RETRY', 2)`
    pub fn knobs() -> (f64, f64) {
        (mail_knob("CCA_MAIL_POLL", 3.0), mail_knob("CCA_MAIL_RETRY", 2.0))
    }
}

/// 找到的、还没打开的那一封
pub struct Pend {
    pub cand: Cand,
    pub rule: &'static MailRule,
    /// 记在 used.json 里的摘要
    pub dg: String,
    /// 浏览器没打开，重试过几次
    tries: u32,
    /// 到这个时间再试
    at: f64,
}

impl Pend {
    pub fn new(cand: Cand, rule: &'static MailRule, dg: String) -> Pend {
        Pend { cand, rule, dg, tries: 0, at: 0.0 }
    }
}

/// 把链接交出去打开的办法（`mail_deliver` 给的；测试里换成假的）：返回 ok、refused、fail
pub type Deliver<'a> = &'a mut dyn FnMut(&str) -> R<String>;

/// `mail_hunt` 的结果
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Found {
    /// opened | none | fail | 最后连不上时 MailError 的 code
    pub code: String,
    pub text: String,
    pub opened: u32,
    pub used: u32,
    /// 没通过核对的 [(原因, 封数)]，照第一次出现的先后
    pub rejected: Vec<(String, u32)>,
}

/// 睡 secs 秒（到 end 为止），每半秒看一次 alive()
pub fn mail_nap(secs: f64, end: f64, alive: &dyn Fn() -> bool, now: &dyn Fn() -> f64, sleep: &dyn Fn(f64)) -> R<()> {
    let until = (now() + secs).min(end);
    while now() < until && alive() {
        check_interrupt()?;
        sleep((until - now()).clamp(0.01, 0.5));
    }
    check_interrupt()
}

/// 取来的全文照样再核对一遍、取出链接：(没通过的原因或 None, 链接或 None, 解析好的邮件)
pub fn mail_verdict(
    rule: &MailRule,
    raw: &[u8],
    got: f64,
    acct: &str,
    since: f64,
    forwarders: &[String],
) -> R<(Option<&'static str>, Option<String>, Message)> {
    let _frame = super::py_frame();
    let msg = mail_parse(raw)?;
    let why = mail_judge(rule, &msg, got, acct, since, forwarders)?;
    let link = match why {
        Some(_) => None,
        None => mail_extract(rule, &msg)?,
    };
    Ok((why, link, msg))
}

/// 认领、取全文、照全文再核对一遍、取出链接、打开：返回 ok、used（打开过了）、fail（浏览器没打开，链接还没用掉）、refused，
/// 或者没通过核对的原因。只有打开了才记下（别的照原来的状态还原：下次照样核对、照实说原因）；Gmail 那边出错时往外传（下一轮再来）
pub fn mail_take(h: &Hunt, src: &mut dyn Source, p: &Pend, since: f64, deliver: Deliver) -> R<String> {
    let _frame = super::py_frame();
    let prev = mail_claim(h.dir, &p.dg, h.again, h.pid)?;
    if prev == Claim::Taken {
        return Ok("used".to_string());
    }
    let run = (|| -> R<String> {
        let raw = src.full(&p.cand)?;
        let (why, link, _) = mail_verdict(p.rule, &raw, p.cand.got, h.acct, since, h.forwarders)?;
        Ok(match (why, link) {
            (Some(why), _) => why.to_string(),
            (None, Some(link)) if !link.is_empty() => deliver(&link)?,
            (None, _) => "nolink".to_string(),
        })
    })();
    // finally：只有打开了才记成 done，别的还原
    let opened = matches!(&run, Ok(st) if st == "ok");
    mail_mark(h.dir, &p.dg, if opened || prev == Claim::Done { Some("done") } else { None })?;
    run
}

/// 找 acct 的登录邮件，找到就交给 deliver(链接)（返回 ok、refused、fail）。once：手动（cca mail open），打开了、浏览器打不开就停，
/// 第一次就连不上马上停；否则一直盯到 end、alive() 为假（登录结束了）。来了好几封只开最新的一封（之前的多半已经失效）；浏览器
/// 没打开的过一会儿（CCA_MAIL_RETRY，默认 2 秒）再试，一共三次（到了 end 也试完）；这一轮从取头到打开都成了才算连上了（取不到全文的
/// 那封照连不上退避）。hello：第一次连上时调；idle：第一轮看完还没有能打开的时调（cca mail open 说要等）
pub fn mail_hunt(
    h: &Hunt,
    src: &mut dyn Source,
    since: f64,
    end: f64,
    deliver: Deliver,
    alive: &dyn Fn() -> bool,
    say: &dyn Fn(&str),
    mut hello: Option<&dyn Fn()>,
    mut idle: Option<&dyn Fn()>,
) -> R<Found> {
    let _frame = super::py_frame();
    let (poll, base) = (h.poll, h.base);
    let mut res = Found { code: "none".into(), ..Found::default() };
    let (mut wait, mut fails, mut polled) = (base, 0u32, false);
    let mut pend: Option<Pend> = None;
    let reject = |res: &mut Found, why: &str| {
        match res.rejected.iter_mut().find(|(w, _)| w == why) {
            Some(slot) => slot.1 += 1,
            None => res.rejected.push((why.to_string(), 1)),
        }
        // 别的账号的登录邮件（同时在登录几个账号）不说
        if !h.once && !matches!(why, "old" | "subject" | "to") {
            say(&format!(
                "! 邮箱助手：收到一封像是登录邮件的邮件，没通过核对（{}），没有打开：要用的话自己点里面的链接",
                mail_why(why).unwrap_or(why)
            ));
        }
    };
    while alive() {
        check_interrupt()?;
        // 这一轮：取头、筛、打开。连 Gmail 出的错在下面分两种处理，别的错往外传
        let round = (|| -> R<Option<Found>> {
            let new = src.poll(since)?;
            polled = true;
            if let Some(hello) = hello.take() {
                hello();
            }
            for c in new {
                let msg = mail_parse(&c.hdr)?;
                match mail_judge_any(h.rules, &msg, c.got, h.acct, since, h.forwarders)? {
                    (_, Some(why)) => reject(&mut res, why),
                    (Some(rule), None) => {
                        if pend.as_ref().is_none_or(|p| (c.got, c.uid) > (p.cand.got, p.cand.uid)) {
                            let dg = mail_digest(&msg, &c.folder, c.uid, c.got);
                            pend = Some(Pend::new(c, rule, dg));
                        }
                    }
                    (None, None) => {}
                }
            }
            if let Some(p) = pend.as_mut().filter(|p| (h.now)() >= p.at) {
                let st = mail_take(h, src, p, since, &mut *deliver)?;
                if st == "ok" {
                    res.opened += 1;
                    res.code = "opened".into();
                    pend = None;
                    if h.once {
                        return Ok(Some(res.clone()));
                    }
                } else if st == "fail" && p.tries < 2 {
                    p.tries += 1;
                    p.at = (h.now)() + base;
                } else {
                    pend = None;
                    if st == "used" {
                        res.used += 1;
                    } else if st == "fail" {
                        res.code = "fail".into();
                        if h.once {
                            return Ok(Some(res.clone()));
                        }
                        say(&format!(
                            "! 邮箱助手：收到登录邮件了，但没能打开浏览器（原因见上面）：链接没有用掉，处理好后运行 cca mail open {}",
                            h.acct
                        ));
                    } else if mail_why(&st).is_some() {
                        reject(&mut res, &st);
                    }
                }
            }
            if let Some(idle) = idle.take() {
                if pend.is_none() {
                    idle();
                }
            }
            Ok(None)
        })();
        match round {
            Ok(Some(done)) => return Ok(done),
            Ok(None) => {
                fails = 0;
                wait = base;
            }
            Err(e) if e.is("MailFatal") => {
                let (code, text) = mail_error(&e).unwrap_or(("", ""));
                res.code = code.to_string();
                res.text = text.to_string();
                return Ok(res);
            }
            Err(e) if e.is("MailRetry") => {
                let (code, text) = mail_error(&e).unwrap_or(("", ""));
                if h.once && !polled {
                    res.code = code.to_string();
                    res.text = text.to_string();
                    return Ok(res);
                }
                fails += 1;
                if fails == 3 && !h.once {
                    say(&format!("! 邮箱助手：连不上 Gmail（{text}），还在重试；登录照常进行，邮件到了也可以自己点里面的链接"));
                }
                if (h.now)() >= end {
                    res.code = code.to_string();
                    res.text = text.to_string();
                    break;
                }
                mail_nap(wait.min(30.0), end, alive, h.now, h.sleep)?;
                wait = (wait * 2.0).min(30.0);
                continue;
            }
            Err(e) => return Err(e),
        }
        match &pend {
            // 浏览器没打开的那一封：到点再试
            Some(p) => mail_nap(p.at - (h.now)(), p.at, alive, h.now, h.sleep)?,
            None if (h.now)() >= end => break,
            None => mail_nap(poll, end, alive, h.now, h.sleep)?,
        }
    }
    Ok(res)
}

/// 打开的办法：链接经 stdin 交给 cca _mail-open 账号（它核对后在账号专属的 Chrome 里打开，打开了那一句也是它说的）。
/// 不带 BG_ENV_DROP（图形界面的事件文件、答案文件这些），CCA_PROXY 留着
pub fn mail_deliver(key: &str) -> impl FnMut(&str) -> R<String> + '_ {
    move |link: &str| {
        if !link.is_ascii() {
            return Ok("fail".to_string()); // (link + '\n').encode('ascii') 出 UnicodeError
        }
        let env: Vec<(String, String)> = environ().into_iter().filter(|(k, _)| !BG_ENV_DROP.contains(&k.as_str())).collect();
        // 链接只经 stdin 给它：不进命令行、不进环境变量
        let cmd = Cmd::new([SELF_PATH.as_str(), "_mail-open", key]).input(Vec::new()).stdout(Out::Null).env(env);
        Ok(match run_opener(&cmd, format!("{link}\n").as_bytes(), 90.0)? {
            Some(0) => "ok",
            Some(2) => "refused",
            _ => "fail",
        }
        .to_string())
    }
}

/// `subprocess.run(…, input=…, timeout=…).returncode`：起不来、到时间还没完（杀掉）是 None。和 `py::proc` 的 `run` 不一样的
/// 只有一处：等它的时候按了 Ctrl-C（cca mail open），照 subprocess 的做法再等它四分之一秒、杀掉，KeyboardInterrupt 往上传
fn run_opener(cmd: &Cmd, input: &[u8], timeout: f64) -> R<Option<i32>> {
    use std::io::Write;
    use std::os::unix::process::ExitStatusExt;
    let Ok(mut child) = cmd.spawn() else { return Ok(None) };
    if let Some(mut stdin) = child.stdin.take() {
        // 一个链接加换行，比管道的缓冲小得多：不会卡住。它不读就退出了（管道断了）不算错；写完关上
        let _ = stdin.write_all(input);
    }
    let deadline = time::monotonic() + timeout;
    let mut delay = 0.0005;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status.code().unwrap_or_else(|| -status.signal().unwrap_or(0)))),
            Ok(None) => {}
            Err(_) => return Ok(None),
        }
        if interrupted() {
            let until = time::monotonic() + 0.25;
            while time::monotonic() < until && matches!(child.try_wait(), Ok(None)) {
                time::sleep(0.01);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(keyboard_interrupt());
        }
        let left = deadline - time::monotonic();
        if left <= 0.0 {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        delay = f64::min(delay * 2.0, 0.05).min(left);
        sleep_interruptible(delay);
    }
}

/// `time.sleep`，但 Ctrl-C 按下时马上回来（mail_nap 回来就看得到）
pub fn sleep_interruptible(secs: f64) {
    if secs <= 0.0 {
        return;
    }
    let ts = libc::timespec { tv_sec: secs.trunc() as _, tv_nsec: (secs.fract() * 1e9) as _ };
    // SAFETY: ts 是有效的内存；被信号打断就提前回来，不接着睡
    unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::testing::{bytes_of, vectors};
    use crate::cmd::mail::{mail_fatal, mail_retry, MAIL_RULES};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    const ALL: &str = "\"[Gmail]/All Mail\"";

    fn tmpdir(tag: &str) -> String {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("cca-mail-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.to_str().unwrap().to_string()
    }

    fn used(dir: &str) -> Dict {
        fs::load_quiet(&path::join(dir, "used.json")).and_then(|v| v.as_dict()).unwrap_or_default()
    }

    /// 一个链接只认领一次：别的进程认领着的不抢，认领的人不在了可以接手，--again 照样打开；只记摘要，链接不进文件
    #[test]
    fn claim_once_across_processes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("claim");
        let me = std::process::id();
        assert_eq!(mail_claim(&dir, "d1", false, me).unwrap(), Claim::Fresh);
        assert_eq!(mail_claim(&dir, "d1", false, me).unwrap(), Claim::Fresh, "自己认领过的可以再认领");
        // 另一个进程来认领（记录里是这个测试进程，它还在）：120 秒内不抢
        assert_eq!(mail_claim(&dir, "d1", false, 1).unwrap(), Claim::Taken);
        mail_mark(&dir, "d1", Some("done")).unwrap();
        assert_eq!(mail_claim(&dir, "d1", false, me).unwrap(), Claim::Taken);
        assert_eq!(mail_claim(&dir, "d1", true, me).unwrap(), Claim::Done, "--again：照样打开，失败时照原来的状态还原");
        mail_mark(&dir, "d1", Some("done")).unwrap();
        assert_eq!(mail_claim(&dir, "d3", false, me).unwrap(), Claim::Fresh);
        mail_mark(&dir, "d3", None).unwrap();
        assert_eq!(mail_claim(&dir, "d3", false, me).unwrap(), Claim::Fresh, "没打开时去掉认领，下次照样能开");
        let keys: Vec<String> = used(&dir).keys().iter().map(|k| k.to_string()).collect();
        assert_eq!(keys, ["d1", "d3"]);
        let mode = |p: &str| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&path::join(&dir, "used.json"))), (0o700, 0o600));
        assert!(!std::fs::read_to_string(path::join(&dir, "used.json")).unwrap().contains("magic"), "只记摘要，链接不进文件");
        // 认领的人不在了（进程号是编的）：可以接手
        let d = used(&dir);
        d.set("d4", dict! { "at" => time::time(), "st" => "claim", "pid" => 0x7fff_fff0i64 });
        fs::save_json(&path::join(&dir, "used.json"), &V::Dict(d), Some(0o600), true).unwrap();
        assert_eq!(mail_claim(&dir, "d4", false, me).unwrap(), Claim::Fresh);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 过了期的、样子不对的记录认领时清掉；没改动就不写文件
    #[test]
    fn old_records_are_dropped() {
        let dir = tmpdir("old");
        fs::makedirs_mode(&dir, 0o700).unwrap();
        let old = dict! { "old" => dict! { "at" => time::time() - 3.0 * 86400.0, "st" => "done" }, "junk" => 5, "noat" => dict! { "st" => "done" } };
        fs::save_json(&path::join(&dir, "used.json"), &V::Dict(old), Some(0o600), true).unwrap();
        assert_eq!(mail_claim(&dir, "new", false, std::process::id()).unwrap(), Claim::Fresh);
        assert_eq!(used(&dir).keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), ["new"]);
        // 没改动就不写：旧的条目留在文件里
        let stale = dict! { "old" => dict! { "at" => 1, "st" => "done" } };
        fs::save_json(&path::join(&dir, "used.json"), &V::Dict(stale), Some(0o600), true).unwrap();
        mail_mark(&dir, "absent", None).unwrap();
        assert!(used(&dir).has("old"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pid_values_like_python() {
        let me = V::Int(i64::from(std::process::id()));
        assert!(mail_pid_alive(Some(&me)).unwrap());
        assert!(mail_pid_alive(Some(&V::str(std::process::id().to_string()))).unwrap(), "int('123') 也认");
        assert!(mail_pid_alive(Some(&V::Float(f64::from(std::process::id())))).unwrap());
        for dead in [V::None, V::str("x"), V::Float(f64::NAN), V::Int(0x7fff_fff0), V::from(crate::list![1])] {
            assert!(!mail_pid_alive(Some(&dead)).unwrap(), "{dead:?}");
        }
        assert!(!mail_pid_alive(None).unwrap());
        assert!(mail_pid_alive(Some(&V::Int(1 << 40))).unwrap_err().is("OverflowError"));
        assert!(mail_pid_alive(Some(&V::Float(f64::INFINITY))).unwrap_err().is("OverflowError"));
    }

    // --- 对照数据的 hunt 一节：真的 mail_hunt 配假的邮件来源、假的时钟跑出来的 ----------------------------------------------------

    /// 照剧本给邮件的来源（移植时生成对照数据的脚本里的 HuntSource 是同一套剧本）。取全文时给哪一封（gone 是取不到）跟着这一封
    /// 候选走：记在 label 里（mail_hunt 不看它）
    struct Scripted {
        rounds: Vec<V>,
        emails: HashMap<String, Vec<u8>>,
        polls: i64,
    }

    impl Source for Scripted {
        fn poll(&mut self, _since: f64) -> R<Vec<Cand>> {
            self.polls += 1;
            if self.rounds.is_empty() {
                return Ok(Vec::new());
            }
            let r = self.rounds.remove(0).as_dict().unwrap();
            for (class, key) in [("MailRetry", "retry"), ("MailFatal", "fatal")] {
                if let Some(e) = r.list(key) {
                    return Err(Fail::Other(class, vec![e.at(0).py_str(), e.at(1).py_str()]));
                }
            }
            let mut out = Vec::new();
            for m in r.list("mails").unwrap().items() {
                let m = m.as_list().unwrap();
                let (uid, name) = (m.at(0).as_int().unwrap() as u64, m.at(1).py_str());
                let full = m.get(3).map_or_else(|| name.clone(), |f| f.py_str());
                out.push(Cand {
                    folder: ALL.into(),
                    label: full,
                    uid,
                    got: T0 - m.at(2).as_num().unwrap(),
                    hdr: self.emails[&name].clone(),
                    valid: Some(1),
                });
            }
            Ok(out)
        }

        fn full(&mut self, cand: &Cand) -> R<Vec<u8>> {
            match cand.label.as_str() {
                "gone" => Err(mail_retry("proto", "Gmail 没给出这封邮件")),
                name => Ok(self.emails[name].clone()),
            }
        }
    }

    fn ms(x: f64) -> f64 {
        (x * 1000.0).round() / 1000.0
    }

    /// 盯邮箱的整个过程——看几次邮箱、隔多久、开哪一封、试几次、说什么、最后记下什么——和 Python 的 mail_hunt 一样
    #[test]
    fn hunts_like_python() {
        let emails: HashMap<String, Vec<u8>> =
            vectors("judge").iter().map(|c| (c.at("name").py_str(), bytes_of(&c.at("raw")))).collect();
        let cases = vectors("hunt");
        assert!(cases.len() > 55);
        for c in cases {
            let name = c.at("name").py_str();
            let dir = tmpdir("vec");
            let who = |w: &V| match w.as_str() {
                Some("me") => Some(i64::from(std::process::id())),
                Some("parent") => Some(i64::from(std::os::unix::process::parent_id())),
                Some("dead") => Some(2147483600),
                _ => None,
            };
            let before = Dict::new();
            for u in c.list("used").unwrap().items() {
                let u = u.as_list().unwrap();
                let key = u.at(0).as_list().unwrap();
                let (mail, uid, ago) = (key.at(0).py_str(), key.at(1).as_int().unwrap() as u64, key.at(2).as_num().unwrap());
                let dg = mail_digest(&mail_parse(&emails[&mail]).unwrap(), ALL, uid, T0 - ago);
                let entry = dict! { "at" => time::time() - u.at(2).as_num().unwrap(), "st" => u.at(1) };
                if let Some(pid) = who(&u.at(3)) {
                    entry.set("pid", pid);
                }
                before.set(dg, entry);
            }
            if !before.is_empty() {
                fs::makedirs_mode(&dir, 0o700).unwrap();
                fs::save_json(&path::join(&dir, "used.json"), &V::Dict(before), Some(0o600), true).unwrap();
            }
            let clock = Cell::new(T0);
            let (said, opened) = (RefCell::new(Vec::<String>::new()), RefCell::new(Vec::<V>::new()));
            let (hello, idle, asked) = (Cell::new(0i64), Cell::new(0i64), Cell::new(0i64));
            let limit = c.at("alive").as_int();
            let alive = || {
                asked.set(asked.get() + 1);
                limit.is_none_or(|n| asked.get() <= n)
            };
            let mut results: Vec<String> = c.list("deliver").unwrap().strs().iter().map(|s| s.to_string()).collect();
            let mut deliver = |link: &str| -> R<String> {
                opened.borrow_mut().push(V::from(crate::list![ms(clock.get() - T0), link]));
                Ok(if results.is_empty() { "ok".to_string() } else { results.remove(0) })
            };
            let rules: Vec<&'static MailRule> = (0..c.at("rules").as_int().unwrap()).map(|_| &MAIL_RULES[0]).collect();
            let forwarders: Vec<String> = c.str("forwarder").map(|f| f.to_string()).into_iter().collect();
            let (now, sleep) = (|| clock.get(), |s: f64| clock.set(clock.get() + s));
            let h = Hunt {
                dir: &dir,
                rules: &rules,
                acct: "alice@example.com",
                forwarders: &forwarders,
                again: c.at("again").truthy(),
                once: c.at("once").truthy(),
                now: &now,
                sleep: &sleep,
                pid: std::process::id(),
                poll: 3.0,
                base: 2.0,
            };
            let mut src = Scripted { rounds: c.list("rounds").unwrap().items(), emails: emails.clone(), polls: 0 };
            let say = |s: &str| said.borrow_mut().push(s.to_string());
            let (since, end) = (T0 - c.at("since_ago").as_num().unwrap(), T0 + c.at("secs").as_num().unwrap());
            let res = mail_hunt(
                &h,
                &mut src,
                since,
                end,
                &mut deliver,
                &alive,
                &say,
                Some(&|| hello.set(hello.get() + 1)),
                Some(&|| idle.set(idle.get() + 1)),
            );
            let res = match res {
                Ok(f) => {
                    let rejected =
                        V::from(f.rejected.iter().map(|(w, n)| V::from(crate::list![w.as_str(), *n])).collect::<Vec<_>>());
                    dict! { "code" => f.code, "text" => f.text, "opened" => f.opened, "used" => f.used, "rejected" => rejected }
                }
                Err(Fail::Other(class, _)) => dict! { "error" => class },
                Err(e) => panic!("{name}：{e:?}"),
            };
            let mut after: Vec<(String, String)> =
                used(&dir).items().iter().map(|(k, v)| (k.to_string(), v.as_dict().unwrap().at("st").py_str())).collect();
            after.sort();
            let got = dict! {
                "res" => res, "said" => crate::py::obj::str_list(said.borrow().iter()), "opened" => V::from(opened.borrow().clone()), "polls" => src.polls,
                "clock" => ms(clock.get() - T0), "hello" => hello.get(), "idle" => idle.get(), "alive_calls" => asked.get(),
                "used_after" => V::from(after.iter().map(|(k, st)| V::from(crate::list![k.as_str(), st.as_str()])).collect::<Vec<_>>()),
            };
            for (k, v) in got.items() {
                assert!(v.py_eq(&c.at(&k)), "{name}：{k}\n  这里：{v:?}\n  Python：{:?}", c.at(&k));
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // --- 盯邮箱：假的邮件来源、假的时钟 ------------------------------------------------------------------

    enum Round {
        Mails(Vec<Cand>),
        Fail(Fail),
    }

    struct FakeSource {
        rounds: Vec<Round>,
        msgs: HashMap<u64, Vec<u8>>,
        polls: u32,
        /// 取全文时总是出这个错
        gone: bool,
    }

    impl Source for FakeSource {
        fn poll(&mut self, _since: f64) -> R<Vec<Cand>> {
            self.polls += 1;
            if self.rounds.is_empty() {
                return Ok(Vec::new());
            }
            match self.rounds.remove(0) {
                Round::Mails(m) => Ok(m),
                Round::Fail(e) => Err(e),
            }
        }
        fn full(&mut self, cand: &Cand) -> R<Vec<u8>> {
            if self.gone {
                return Err(mail_retry("proto", "Gmail 没给出这封邮件"));
            }
            Ok(self.msgs[&cand.uid].clone())
        }
    }

    /// 对照数据里的登录邮件（judge 一节里的 good、to bob……；当初是假 Gmail 那一份做的，现在是 kit::mailfake）
    fn eml(name: &str) -> Vec<u8> {
        let c =
            vectors("judge").into_iter().find(|c| c.at("name").py_str() == name).unwrap_or_else(|| panic!("对照数据里没有 {name}"));
        bytes_of(&c.at("raw"))
    }

    struct World {
        dir: String,
        clock: Cell<f64>,
        said: RefCell<Vec<String>>,
        opened: RefCell<Vec<(f64, String)>>,
    }

    const T0: f64 = 1794708000.0;

    impl World {
        fn new(tag: &str) -> World {
            World { dir: tmpdir(tag), clock: Cell::new(T0), said: RefCell::new(Vec::new()), opened: RefCell::new(Vec::new()) }
        }

        fn cand(&self, uid: u64, raw: &[u8], ago: f64) -> Cand {
            Cand { folder: ALL.into(), label: "所有邮件".into(), uid, got: T0 - ago, hdr: raw.to_vec(), valid: Some(1) }
        }

        fn hunt(
            &self,
            src: &mut FakeSource,
            results: &[&str],
            secs: f64,
            once: bool,
            again: bool,
            alive: &dyn Fn() -> bool,
            idle: Option<&dyn Fn()>,
        ) -> Found {
            let rules = [&MAIL_RULES[0]];
            let (now, sleep) = (|| self.clock.get(), |s: f64| self.clock.set(self.clock.get() + s));
            let h = Hunt {
                dir: &self.dir,
                rules: &rules,
                acct: "alice@example.com",
                forwarders: &[],
                again,
                once,
                now: &now,
                sleep: &sleep,
                pid: std::process::id(),
                poll: 3.0,
                base: 2.0,
            };
            let mut results: Vec<String> = results.iter().map(|s| s.to_string()).collect();
            let mut deliver = |link: &str| -> R<String> {
                self.opened.borrow_mut().push((((self.clock.get() - T0) * 10.0).round() / 10.0, link.to_string()));
                Ok(if results.is_empty() { "ok".to_string() } else { results.remove(0) })
            };
            let say = |s: &str| self.said.borrow_mut().push(s.to_string());
            let start = self.clock.get();
            mail_hunt(&h, src, start - 10.0, start + secs, &mut deliver, alive, &say, None, idle).unwrap()
        }

        fn run(&self, src: &mut FakeSource, results: &[&str], secs: f64, once: bool) -> Found {
            self.hunt(src, results, secs, once, false, &|| true, None)
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn source(rounds: Vec<Round>, msgs: &[(u64, &[u8])]) -> FakeSource {
        FakeSource { rounds, msgs: msgs.iter().map(|(u, m)| (*u, m.to_vec())).collect(), polls: 0, gone: false }
    }

    fn links(w: &World) -> Vec<String> {
        w.opened.borrow().iter().map(|(_, l)| l.clone()).collect()
    }

    #[test]
    fn opens_only_the_newest_and_keeps_watching() {
        let w = World::new("newest");
        let (a, b) = (eml("hunt old link"), eml("hunt new link"));
        let mut src = source(vec![Round::Mails(vec![w.cand(1, &a, 5.0), w.cand(2, &b, 1.0)])], &[(1, &a), (2, &b)]);
        let res = w.run(&mut src, &[], 30.0, false);
        assert_eq!(links(&w), ["https://claude.ai/magic-link#tok-new"]);
        assert_eq!((res.code.as_str(), res.opened), ("opened", 1));
        assert!(src.polls > 5, "打开以后接着看（登录结束才停）");
        assert!(w.said.borrow().is_empty());
    }

    #[test]
    fn browser_failure_retries_three_times_then_says_so() {
        let w = World::new("retry3");
        let a = eml("good");
        let res =
            w.run(&mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]), &["fail", "fail", "ok"], 30.0, false);
        assert_eq!(w.opened.borrow().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [0.0, 2.0, 4.0]);
        assert_eq!(res.code, "opened");
        w.opened.borrow_mut().clear();
        w.said.borrow_mut().clear();
        let b = eml("hunt second good");
        let res =
            w.run(&mut source(vec![Round::Mails(vec![w.cand(2, &b, 0.0)])], &[(2, &b)]), &["fail", "fail", "fail"], 30.0, false);
        assert_eq!((w.opened.borrow().len(), res.code.as_str()), (3, "fail"));
        assert_eq!(w.said.borrow().iter().filter(|s| s.contains("没能打开浏览器")).count(), 1);
        assert_eq!(
            w.said.borrow()[0],
            "! 邮箱助手：收到登录邮件了，但没能打开浏览器（原因见上面）：链接没有用掉，处理好后运行 cca mail open alice@example.com"
        );
    }

    #[test]
    fn retries_finish_even_past_the_end() {
        let w = World::new("pastend");
        let a = eml("good");
        let res = w.run(&mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]), &["fail", "fail", "ok"], 1.0, false);
        assert_eq!((w.opened.borrow().len(), res.code.as_str()), (3, "opened"));
    }

    #[test]
    fn once_stops_on_browser_failure() {
        let w = World::new("oncefail");
        let a = eml("good");
        let res =
            w.run(&mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]), &["fail", "fail", "fail"], 600.0, true);
        assert_eq!((res.code.as_str(), w.opened.borrow().len()), ("fail", 3));
        assert!(w.clock.get() - T0 < 10.0);
        assert!(w.said.borrow().is_empty(), "cca mail open 自己说结果");
    }

    #[test]
    fn connection_trouble_backs_off_and_says_once() {
        let w = World::new("backoff");
        let net = |t: &str| Round::Fail(mail_retry("net", t));
        let mut src = source(vec![net("x1"), net("x2"), net("x3"), net("x4"), Round::Mails(vec![])], &[]);
        let res = w.run(&mut src, &[], 60.0, false);
        assert_eq!(w.said.borrow().iter().filter(|s| s.contains("连不上 Gmail")).count(), 1);
        assert_eq!(w.said.borrow()[0], "! 邮箱助手：连不上 Gmail（x3），还在重试；登录照常进行，邮件到了也可以自己点里面的链接");
        assert_eq!(res.code, "none", "后来连上了");
        let mut down = source((0..50).map(|_| net("down")).collect(), &[]);
        let res = w.run(&mut down, &[], 20.0, false);
        assert_eq!((res.code.as_str(), res.text.as_str()), ("net", "down"), "到最后也没连上：说最后的原因");
    }

    #[test]
    fn once_fails_fast_only_before_the_first_poll() {
        let w = World::new("fast");
        let res = w.run(&mut source(vec![Round::Fail(mail_retry("proxy", "代理断开了连接"))], &[]), &[], 30.0, true);
        assert_eq!((res.code.as_str(), w.clock.get()), ("proxy", T0));
        let a = eml("good");
        let rounds = vec![Round::Mails(vec![]), Round::Fail(mail_retry("net", "blip")), Round::Mails(vec![w.cand(3, &a, 0.0)])];
        let res = w.run(&mut source(rounds, &[(3, &a)]), &[], 60.0, true);
        assert_eq!(res.code, "opened", "中途断一下接着等");
    }

    #[test]
    fn fatal_stops_at_once() {
        let w = World::new("fatal");
        let res = w.run(&mut source(vec![Round::Fail(mail_fatal("auth", "Gmail 不认这个密码"))], &[]), &[], 30.0, false);
        assert_eq!((res.code.as_str(), res.text.as_str()), ("auth", "Gmail 不认这个密码"));
    }

    #[test]
    fn idle_called_only_when_nothing_to_open() {
        let w = World::new("idle");
        let seen = RefCell::new(Vec::new());
        let a = eml("good");
        w.hunt(
            &mut source(vec![Round::Mails(vec![])], &[]),
            &[],
            10.0,
            true,
            false,
            &|| true,
            Some(&|| seen.borrow_mut().push("idle")),
        );
        w.hunt(
            &mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]),
            &[],
            10.0,
            true,
            false,
            &|| true,
            Some(&|| seen.borrow_mut().push("bad")),
        );
        assert_eq!(*seen.borrow(), ["idle"]);
    }

    #[test]
    fn used_mails_are_not_opened_again_unless_again() {
        let w = World::new("again");
        let a = eml("good");
        let mk = || source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]);
        w.run(&mut mk(), &[], 30.0, true);
        let res = w.run(&mut mk(), &[], 5.0, true);
        assert_eq!((w.opened.borrow().len(), res.used, res.code.as_str()), (1, 1, "none"));
        let res = w.hunt(&mut mk(), &[], 30.0, true, true, &|| true, None);
        assert_eq!((w.opened.borrow().len(), res.code.as_str()), (2, "opened"));
    }

    #[test]
    fn rejections_quiet_or_said() {
        let w = World::new("reject");
        let mails = [(1, eml("to bob")), (2, eml("hunt subject Receipt")), (3, eml("hunt from x@evil")), (4, eml("ar fail"))];
        let cands = mails.iter().map(|(u, r)| w.cand(*u, r, 0.0)).collect();
        let res = w.run(&mut source(vec![Round::Mails(cands)], &[]), &[], 5.0, false);
        let count = |why: &str| res.rejected.iter().find(|(w, _)| w == why).map(|(_, n)| *n);
        assert_eq!(
            (count("to"), count("subject"), count("from"), count("dkim"), res.rejected.len()),
            (Some(1), Some(1), Some(1), Some(1), 4)
        );
        assert_eq!(w.said.borrow().len(), 2, "别的账号的、标题不像的不说；发件人、签名不对的说");
        assert_eq!(
            w.said.borrow()[0],
            "! 邮箱助手：收到一封像是登录邮件的邮件，没通过核对（发件人不是 Anthropic），没有打开：要用的话自己点里面的链接"
        );
        assert!(w.opened.borrow().is_empty());
    }

    #[test]
    fn stops_when_login_is_over() {
        let w = World::new("over");
        let left = Cell::new(3);
        let alive = || {
            left.set(left.get() - 1);
            left.get() >= 0
        };
        let mut src = source(vec![], &[]);
        w.hunt(&mut src, &[], 600.0, false, false, &alive, None);
        assert!(src.polls <= 2);
    }

    #[test]
    fn mail_that_cannot_be_fetched_backs_off() {
        let w = World::new("gone");
        let a = eml("good");
        let mut src = source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[]);
        src.gone = true;
        let res = w.run(&mut src, &[], 120.0, false);
        assert_eq!(w.said.borrow().iter().filter(|s| s.contains("Gmail 没给出这封邮件")).count(), 1);
        assert!(src.polls < 15, "取不到全文的那封照连不上退避，不是每两秒取一次");
        assert_eq!(res.code, "proto");
    }

    #[test]
    fn mails_not_opened_are_checked_again_next_time() {
        let w = World::new("nolink");
        let a = eml("hunt help link only");
        for _ in 0..2 {
            let res = w.run(&mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]), &[], 5.0, true);
            assert_eq!(
                (res.rejected.clone(), res.used),
                (vec![("nolink".to_string(), 1)], 0),
                "没打开的不记下：下次照样核对、照实说原因"
            );
        }
    }

    #[test]
    fn refused_counts_as_rejected() {
        let w = World::new("refused");
        let a = eml("good");
        let res = w.run(&mut source(vec![Round::Mails(vec![w.cand(1, &a, 0.0)])], &[(1, &a)]), &["refused"], 5.0, false);
        assert_eq!((res.rejected, w.opened.borrow().len()), (vec![("refused".to_string(), 1)], 1));
        assert!(w.said.borrow().iter().any(|s| s.contains("打开之前再核对")));
    }

    #[test]
    fn nap_sleeps_in_half_second_steps() {
        let clock = Cell::new(100.0);
        let steps = RefCell::new(Vec::new());
        let (now, sleep) = (
            || clock.get(),
            |s: f64| {
                steps.borrow_mut().push(s);
                clock.set(clock.get() + s);
            },
        );
        mail_nap(1.2, 1000.0, &|| true, &now, &sleep).unwrap();
        assert_eq!(steps.borrow().len(), 3);
        assert!((clock.get() - 101.2).abs() < 1e-9);
        steps.borrow_mut().clear();
        mail_nap(5.0, clock.get() + 0.3, &|| true, &now, &sleep).unwrap();
        assert_eq!(steps.borrow().len(), 1, "到 end 为止");
        steps.borrow_mut().clear();
        mail_nap(5.0, 1e9, &|| false, &now, &sleep).unwrap();
        assert!(steps.borrow().is_empty(), "登录结束了：不睡");
    }
}
