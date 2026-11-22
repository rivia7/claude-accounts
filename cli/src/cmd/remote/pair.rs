//! 配对：两台机器各存一把一样的密钥，之后凭它互相认、给会话的内容加密（`crypto.rs`）。
//!
//! 一台机器显示配对码（`remote-pair-show`，`cca remote pair`），另一台输入（`remote-pair-join`，`cca remote join`；
//! 配对码从 stdin 来，不进命令行）。两边各往服务器上放一份用配对码封起来的密文（自己的设备指纹、一个随机数、机器名），
//! 取到对方的那一份就算出同一把密钥，各自报一次它的编号，服务器看两边报的一样才记下这两台设备配了对。服务器见不到
//! 配对码和密钥；两边还各自核对“密文里写的指纹”和“服务器验过签名的指纹”是同一个。
//!
//! 默认是单向的：显示配对码的那台让对方看和操作自己的会话，输入的那台不让对方接入自己（`remote-allow` 改）。
//! `remote-peers` 列出来，`remote-unpair` 取消，`remote-forget` 是 `cca restore` 用的（全部取消、密钥都删掉）。

use super::crypto::{self, PairKeys};
use super::far::{self, clean};
use super::peers::{self, Incoming, Peer};
use super::relay::{self, Server, NO_WEB};
use crate::cmd::arity;
use crate::cmd::secrets::keystore_name;
use crate::core::base::Lock;
use crate::dict;
use crate::py::io::read_stdin_line;
use crate::py::obj::{Dict, V};
use crate::py::rand::urandom;
use crate::py::time::{monotonic, strftime_local, time};
use crate::py::{die, signal, R};

/// 配对码多久过期（和服务器的 PAIR_TTL 一样）
const CODE_TTL: f64 = 600.0;

pub fn need_server() -> R<Server> {
    match relay::server()? {
        Ok(s) => Ok(s),
        Err(why) => die(format!("远程会话要经维护者的服务器中转，现在用不了：{why}")),
    }
}

fn ask(srv: &Server, op: &str, payload: &Dict, timeout: i64) -> R<Dict> {
    match srv.post(op, payload, timeout)? {
        Ok(j) => Ok(j),
        Err(f) => die(f.text),
    }
}

/// 对方放上来的那一份，和服务器说的：它验过签名的对方的指纹、维护者的设备清单里对方的名字（没有是空的）
struct Theirs {
    m: V,
    fp: String,
    listed: String,
}

/// 等对方放上来的那一份；到 until 还没等到是 None。服务器一次最多等 10 秒
fn wait_theirs(srv: &Server, rid: &str, until: f64) -> R<Option<Theirs>> {
    loop {
        let left = until - monotonic();
        if left <= 0.0 {
            return Ok(None);
        }
        let asked = monotonic();
        let mut pause = 2.0;
        match srv.post("pair_get", &dict! { "rid" => rid, "wait" => left.min(10.0) }, 15)? {
            Ok(j) => {
                if let (Some(m), Some(fp)) = (j.get("m").filter(|m| !m.is_none()), j.str("fp")) {
                    let listed = j.str("name").map(|n| n.to_string()).unwrap_or_default();
                    return Ok(Some(Theirs { m, fp: fp.to_string(), listed }));
                }
            }
            // 网络断了一下、问得太勤：歇一会儿接着等
            Err(f) if f.code == 0 && f.error == "unreachable" => {}
            Err(f) if f.code == 429 => pause = 5.0,
            Err(f) => return die(f.text),
        }
        if monotonic() - asked < pause {
            signal::sleep(pause)?; // 服务器没替我们等（等着的太多了）：自己等，别一个接一个地问
        }
    }
}

/// 两边的密文都有了：算出密钥、存好、报给服务器、等它记下，再写进配对表。shown：配对码是这台机器显示的；
/// web：这次配对是开给网页端的（输入配对码的那一头不在设备清单里时，配对表里记成网页端）
fn conclude(srv: &Server, k: &PairKeys, shown: bool, web: bool, mine: &[u8], theirs: Theirs) -> R<Peer> {
    let Theirs { m, fp: server_fp, listed } = theirs;
    let web = web && listed.is_empty();
    let Some((fp, n, name)) = crypto::pair_open(&k.keys, &k.rid, if shown { "b" } else { "a" }, &m) else {
        return die("对方放上来的内容打不开（不是用这个配对码封的）：没有配对，重新来一次");
    };
    if fp != server_fp || fp == srv.fp {
        return die("服务器说的那台设备和对方自己说的不一样：没有配对");
    }
    let p =
        if shown { crypto::pair_secret(&k.kc, mine, &n, &srv.fp, &fp) } else { crypto::pair_secret(&k.kc, &n, mine, &fp, &srv.fp) };
    let pid = crypto::pid_of(&p);
    if !peers::put_secret(&pid, &p)? {
        return die(format!("配对的密钥存不进{}：没有配对", keystore_name()));
    }
    let committed = (|| -> R<bool> {
        let mut j = ask(srv, "pair_commit", &dict! { "rid" => k.rid.as_str(), "pid" => pid.as_str() }, 10)?;
        let end = monotonic() + 60.0;
        while j.at("done").as_bool() != Some(true) {
            if monotonic() > end {
                return Ok(false);
            }
            let asked = monotonic();
            match srv.post("pair_get", &dict! { "rid" => k.rid.as_str(), "wait" => 10, "until" => "done" }, 15)? {
                Ok(got) => j = got,
                // 网络断了一下、问得太勤：歇一会儿接着等（和等对方输入时一样）
                Err(f) if (f.code == 0 && f.error == "unreachable") || f.code == 429 => {}
                Err(f) => return die(f.text),
            }
            if j.at("done").as_bool() != Some(true) && monotonic() - asked < 2.0 {
                signal::sleep(2.0)?;
            }
        }
        Ok(true)
    })();
    if !matches!(committed, Ok(true)) {
        peers::del_secret(&pid)?; // 没配成：密钥不留
        committed?;
        return die("对方没有确认（它那边可能取消了）：没有配对，重新来一次");
    }
    // 写进配对表。先写表、再删以前那一把：表没写成的话把新的这一把删掉，不留下表里没有的密钥，也不让表指着删掉了的
    let written = (|| -> R<(Peer, Vec<String>)> {
        let _lock = Lock::acquire()?;
        let mut all = peers::load();
        let (peer, old) = entry(&mut all, &pid, &fp, [listed, name], shown, web);
        peers::save(&all)?;
        Ok((peer, old))
    })();
    match written {
        Ok((peer, old)) => {
            for pid in old {
                let _ = peers::del_secret(&pid); // 以前那一把：表里已经没有它了，删不掉也只是多留一条没人用的
            }
            Ok(peer)
        }
        Err(e) => {
            let _ = peers::del_secret(&pid);
            Err(e)
        }
    }
}

/// 配对表里换上这一次配对的那一项。返回它，和这台设备以前配过的那些配对编号（它们的密钥该删了；服务器上也是换掉）
fn entry(all: &mut Vec<Peer>, pid: &str, fp: &str, names: [String; 2], shown: bool, web: bool) -> (Peer, Vec<String>) {
    // 输入配对码不改“让不让它接入”：以前让的照旧
    let before = all.iter().find(|x| x.fp == fp).map(|x| x.incoming);
    let old = all.iter().filter(|x| x.fp == fp && x.pid != pid).map(|x| x.pid.clone()).collect();
    all.retain(|x| x.fp != fp && x.pid != pid);
    // 名字：维护者的设备清单里那台设备叫什么（cca devices 里看到的也是它）；清单里没写就用对方自己说的机器名。
    // 只是给人看的：认设备靠的是上面核对过的指纹
    let name = names
        .iter()
        .map(|n| clean(n, 48).trim().to_string())
        .find(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{} {}", if web { "手机" } else { "机器" }, &pid[..6]));
    let incoming = if shown { Incoming::Control } else { before.unwrap_or(Incoming::None) };
    let peer = Peer { pid: pid.to_string(), fp: fp.to_string(), name, incoming, shown, at: time(), web };
    all.push(peer.clone());
    (peer, old)
}

/// `remote-pair-show [--web]`：出一个配对码、等另一台机器输入。stdout：先是 `code 配对码`（马上写出去：调用方——bash——
/// 把它显示给用户，终端里打印、界面 App 里是一页），配好以后 `peer 对方的名字`。
/// --web：这次配对开给手机上的网页端（输入配对码的可以是不在设备清单里的设备）；`code` 那一行后面紧跟一行 `web 网页端的地址`
/// （一起写出去：手机上打开它、在那一页里输入配对码）
pub fn cmd_remote_pair_show(args: &[String]) -> R<()> {
    let web = match args {
        [] => false,
        [a] if a == "--web" => true,
        _ => return die("用法：remote-pair-show [--web]"),
    };
    let srv = need_server()?;
    let code = crypto::code_new();
    let k = crypto::pair_keys(&code);
    let open = dict! { "rid" => k.rid.as_str() };
    if web {
        open.set("web", true);
    }
    let opened = ask(&srv, "pair_open", &open, 10)?;
    // 旧版的服务器不认 web 这一项、照普通的配对开了：手机输入配对码会被拒，所以这里就停下
    let page = opened.str("web").map(|u| u.to_string()).filter(|u| web_url_ok(u));
    if web && page.is_none() {
        let _ = srv.post("pair_close", &dict! { "rid" => k.rid.as_str() }, 5);
        return die(NO_WEB);
    }
    let na = urandom(32);
    let m = crypto::pair_msg(&k.keys, &k.rid, "a", &srv.fp, &na, &peers::machine_name())?;
    ask(&srv, "pair_put", &dict! { "rid" => k.rid.as_str(), "m" => m }, 10)?;
    crate::out!("code {}", crypto::code_text(&code));
    if let Some(page) = &page {
        crate::out!("web {page}");
    }
    crate::py::io::flush_out();
    // 等的时候被叫停（Ctrl-C、调用方不等了：界面 App 里点了取消）要能收场，所以这几个信号不直接把进程带走
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        signal::raise_on(sig, "Stop");
    }
    let paired = (|| -> R<Peer> {
        let Some(theirs) = wait_theirs(&srv, &k.rid, monotonic() + CODE_TTL)? else {
            return die("配对码过期了，没有配对：要配对的话重新运行 cca remote pair");
        };
        conclude(&srv, &k, true, web, &na, theirs)
    })();
    match paired {
        Ok(peer) => {
            crate::out!("peer {}", peer.name);
            Ok(())
        }
        Err(e) => {
            // 没配成（叫停了、过期了、出错了）：告诉服务器这次配对作废，另一台这时输入配对码马上知道，不用干等对方确认
            let _ = signal::check();
            let _ = srv.post("pair_close", &dict! { "rid" => k.rid.as_str() }, 5);
            match e {
                crate::py::Fail::Other("Stop", _) => crate::py::exit(130),
                e => Err(e),
            }
        }
    }
}

/// `remote-pair-join`：stdin 一行是另一台机器显示的配对码。配好以后 stdout 一行：对方的名字
pub fn cmd_remote_pair_join(args: &[String]) -> R<()> {
    arity("cmd_remote_pair_join", args, 0, 0)?;
    let Some(code) = crypto::code_bytes(&read_stdin_line()) else {
        return die("配对码不对：是 16 个字母和数字，照另一台机器上显示的输入（大小写、横线随意）");
    };
    let srv = need_server()?;
    let k = crypto::pair_keys(&code);
    let nb = urandom(32);
    let m = crypto::pair_msg(&k.keys, &k.rid, "b", &srv.fp, &nb, &peers::machine_name())?;
    match srv.post("pair_put", &dict! { "rid" => k.rid.as_str(), "m" => m }, 10)? {
        Ok(_) => {}
        Err(f) if f.code == 404 && f.error == "no_pairing" => return die("没有这个配对码：输错了，或者过期了（10 分钟内有效）"),
        Err(f) if f.code == 400 => {
            return die("这个配对码用不了：它是这台机器自己显示的（要在另一台机器上输入），或者已经输入过一次了")
        }
        Err(f) => return die(f.text),
    }
    let Some(theirs) = wait_theirs(&srv, &k.rid, monotonic() + 30.0)? else {
        return die("没等到对方那一份：显示配对码的那台机器还开着 cca remote pair 吗？");
    };
    let peer = conclude(&srv, &k, false, false, &nb, theirs)?;
    crate::out!("{}", peer.name);
    Ok(())
}

fn row(p: &Peer, note: &str) -> String {
    let kind = if p.web { "（手机上的网页端）" } else { "" };
    format!("{}  {}{}  对方{}  {} 配对{}", &p.pid[..8], p.name, kind, p.incoming.text(), strftime_local("%Y-%m-%d", p.at), note)
}

/// 服务器给的网页端地址：要拿去显示、做成二维码，所以只认 https://主机[:端口]/ 这一种样子（本机试的时候是 http://localhost）
fn web_url_ok(u: &str) -> bool {
    let rest = match u.strip_prefix("https://") {
        Some(r) => r,
        None => match u.strip_prefix("http://") {
            Some(r) if r.starts_with("localhost") || r.starts_with("127.0.0.1") => r,
            _ => return false,
        },
    };
    let host = rest.strip_suffix('/').unwrap_or(rest);
    !host.is_empty() && host.len() <= 200 && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// `remote-peers [--check]`：一行一台配对过的机器；一台都没有时什么都不打印。--check：问一下服务器，对方取消了的标出来
pub fn cmd_remote_peers(args: &[String]) -> R<()> {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => return die("用法：remote-peers [--check]"),
    };
    let all = peers::load();
    let mut gone = std::collections::HashSet::new();
    if check && !all.is_empty() {
        match relay::server()? {
            Ok(srv) => match far::fetch(&srv, &all, 8)? {
                Ok(l) => gone = all.iter().filter(|p| !l.known.contains(&p.pid)).map(|p| p.pid.clone()).collect(),
                Err(f) => crate::err!("没问到服务器，对方那边取消了没有不知道：{}", f.text),
            },
            Err(why) => crate::err!("{why}"),
        }
    }
    for p in &all {
        crate::out!("{}", row(p, if gone.contains(&p.pid) { "（对方已经取消了配对：cca remote unpair 把它删掉）" } else { "" }));
    }
    Ok(())
}

fn pick<'a>(all: &'a [Peer], who: &str) -> R<&'a Peer> {
    match peers::find(all, who) {
        Ok(p) => Ok(p),
        Err(why) => die(why),
    }
}

/// `remote-allow 名字 control|view|none`：对方能对这台机器的会话做什么。开着的会话几秒钟内照新的来
/// （不让了的、改成只看的，正接着的那一处会被断开）。stdout 一行：对方的名字
pub fn cmd_remote_allow(args: &[String]) -> R<()> {
    arity("cmd_remote_allow", args, 2, 2)?;
    let Some(incoming) = Incoming::parse(&args[1]) else { return die("用法：remote-allow 名字 control|view|none") };
    let _lock = Lock::acquire()?;
    let mut all = peers::load();
    let pid = pick(&all, &args[0])?.pid.clone();
    let p = all.iter_mut().find(|p| p.pid == pid).expect("刚找到的");
    p.incoming = incoming;
    crate::out!("{}", p.name);
    peers::save(&all)
}

/// 从这台机器上删掉一个配对（密钥也删），再告诉服务器。服务器没告诉成也算取消了：这边没有密钥，对方已经接不进来
fn drop_peer(pid: &str) -> R<Option<String>> {
    {
        let _lock = Lock::acquire()?;
        let mut all = peers::load();
        all.retain(|p| p.pid != pid);
        peers::save(&all)?;
        peers::del_secret(pid)?;
    }
    let srv = match relay::server()? {
        Ok(srv) => srv,
        Err(why) => return Ok(Some(why)),
    };
    for attempt in 0..3 {
        match srv.post("pair_drop", &dict! { "pid" => pid }, 6)? {
            Ok(_) => break,
            // 问得太勤被挡了：歇一下再说一次（这一句没说到的话，对方那边一直列着这台机器）
            Err(f) if f.code == 429 && attempt < 2 => signal::sleep(2.0)?,
            Err(f) => return Ok(Some(f.text)),
        }
    }
    Ok(None)
}

/// `remote-unpair 名字`：取消一个配对。stdout 第一行是对方的名字；这台机器上取消了、服务器上的记录却没删掉时，第二行说明
pub fn cmd_remote_unpair(args: &[String]) -> R<()> {
    arity("cmd_remote_unpair", args, 1, 1)?;
    let all = peers::load();
    let p = pick(&all, &args[0])?;
    let why = drop_peer(&p.pid)?;
    crate::out!("{}", p.name);
    if let Some(why) = why {
        crate::out!("这台机器上已经取消了；服务器上的记录没删掉（{why}），对方那边还列着这台机器，但接不进来了");
    }
    Ok(())
}

/// `remote-forget`：`cca restore` 用。所有配对都取消、密钥都删掉；什么都不说
pub fn cmd_remote_forget(args: &[String]) -> R<()> {
    arity("cmd_remote_forget", args, 0, 0)?;
    for p in peers::load() {
        drop_peer(&p.pid)?;
    }
    Ok(())
}
