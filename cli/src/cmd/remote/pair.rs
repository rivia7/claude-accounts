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
use super::relay::{self, Server};
use crate::cmd::arity;
use crate::cmd::hub::{HubKind, TeamTag};
use crate::cmd::secrets::keystore_name;
use crate::core::base::Lock;
use crate::core::team::teams;
use crate::dict;
use crate::py::io::read_stdin_line;
use crate::py::obj::{Dict, V};
use crate::py::rand::urandom;
use crate::py::time::{monotonic, strftime_local, time};
use crate::py::{die, signal, R};

/// 配对码多久过期（和服务器的 PAIR_TTL 一样）
const CODE_TTL: f64 = 600.0;

/// `--hub` 的值是哪台服务器：Team 的编号或者名字。`maintainer`（以前是维护者的那一台）3.6.0 起是用法错误
fn hub_named(want: &str) -> R<HubKind> {
    if want == "maintainer" {
        return die("没有维护者的服务器了（远程会话只经 Team 的服务器中转）：--hub 后面写 Team 的名字或者编号");
    }
    let all = teams();
    let hit: Vec<_> = all.iter().filter(|t| t.id == want || t.name == want).collect();
    match hit.as_slice() {
        [t] => match TeamTag::new(&t.id) {
            Some(tag) => Ok(HubKind::Team(tag)),
            None => die(format!("「{want}」这个 Team 的编号不对")),
        },
        [] => die(format!("这台机器不在叫「{want}」的 Team 里（cca team 看在哪些 Team 里）")),
        _ => die(format!("有好几个 Team 都叫「{want}」：写它的编号（cca team 看得到）")),
    }
}

/// 在哪台服务器上配对。说了（`--hub`）就是它；没说是这台机器的家（`relay::home_hub`：加入的第一个 Team 的）——
/// 配对记在哪台服务器上，经它看得到的就是走那台服务器的会话（哪个 Team 的账号）。没有家（不在哪个 Team 里、一台都用不了）时配不了
pub fn need_server(hub: Option<&str>) -> R<Server> {
    let kind = match hub {
        Some(want) => hub_named(want)?,
        None => match relay::home_hub() {
            Some(kind) => kind,
            None => return die(relay::no_home_why()),
        },
    };
    match relay::server_of(kind)? {
        Ok(s) => Ok(s),
        Err(why) => die(format!("远程会话要经服务器中转，现在用不了：{why}")),
    }
}

/// 参数里的 `--hub 服务器`（没有是 None），和剩下的参数
fn hub_arg(args: &[String]) -> R<(Option<String>, Vec<String>)> {
    let mut rest = Vec::new();
    let mut hub = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--hub" {
            match it.next() {
                Some(v) if hub.is_none() && !v.is_empty() => hub = Some(v.clone()),
                _ => return die("--hub 后面要跟一台服务器（Team 的名字或者编号），只能写一次"),
            }
        } else {
            rest.push(a.clone());
        }
    }
    Ok((hub, rest))
}

fn ask(srv: &Server, op: &str, payload: &Dict, timeout: i64) -> R<Dict> {
    match srv.post(op, payload, timeout)? {
        Ok(j) => Ok(j),
        Err(f) => die(f.text),
    }
}

/// 对方放上来的那一份，和服务器说的：它验过签名的对方的指纹、服务器的设备清单（Team 的）里对方的名字（没有是空的）
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
        let (peer, old) = entry(&mut all, &pid, &fp, [listed, name], shown, web, srv.kind);
        peers::save(&all)?;
        Ok((peer, old))
    })();
    match written {
        Ok((peer, old)) => {
            for pid in old {
                let _ = peers::del_secret(&pid); // 以前那一把：表里已经没有它了，删不掉也只是多留一条没人用的
            }
            if peer.web {
                super::status::status_kick(); // 手机马上看得到这台机器上的账号（remote/status.rs）
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
///
/// 同一台设备在不同的服务器上各配各的（各是一项、各有一把密钥）：换掉的只是同一台服务器上的那一个
fn entry(
    all: &mut Vec<Peer>,
    pid: &str,
    fp: &str,
    names: [String; 2],
    shown: bool,
    web: bool,
    hub: HubKind,
) -> (Peer, Vec<String>) {
    // 输入配对码不改“让不让它接入”：以前让的照旧
    let same = |x: &Peer| x.fp == fp && x.hub == hub;
    let before = all.iter().find(|x| same(x)).map(|x| x.incoming);
    let old = all.iter().filter(|x| same(x) && x.pid != pid).map(|x| x.pid.clone()).collect();
    all.retain(|x| !same(x) && x.pid != pid);
    // 名字：服务器的设备清单（Team 的）里那台设备叫什么（cca team devices 里看到的也是它）；清单里没写就用对方自己说的机器名。
    // 只是给人看的：认设备靠的是上面核对过的指纹
    let name = names
        .iter()
        .map(|n| clean(n, 48).trim().to_string())
        .find(|n| !n.is_empty())
        .unwrap_or_else(|| format!("{} {}", if web { "手机" } else { "机器" }, &pid[..6]));
    let incoming = if shown { Incoming::Control } else { before.unwrap_or(Incoming::None) };
    let peer = Peer { pid: pid.to_string(), fp: fp.to_string(), name, incoming, shown, at: time(), web, hub, launch: false };
    all.push(peer.clone());
    (peer, old)
}

/// `remote-pair-show [--web] [--hub 服务器]`：出一个配对码、等另一台机器输入。在哪台服务器上配见 `need_server`。
/// stdout：先是 `code 配对码`（马上写出去：调用方——bash——
/// 把它显示给用户，终端里打印、界面 App 里是一页），配好以后 `peer 对方的名字`。
/// --web：这次配对开给手机（iOS 上的 cca App；输入配对码的可以是不在设备清单里的设备）；`code` 那一行后面紧跟一行
/// `link cca-remote://pair?…`（一起写出去：画成二维码，App 扫了就配对，格式见 `app_link`）
pub fn cmd_remote_pair_show(args: &[String]) -> R<()> {
    let (hub, rest) = hub_arg(args)?;
    let web = match rest.as_slice() {
        [] => false,
        [a] if a == "--web" => true,
        _ => return die("用法：remote-pair-show [--web] [--hub 服务器]"),
    };
    let srv = need_server(hub.as_deref())?;
    let code = crypto::code_new();
    let k = crypto::pair_keys(&code);
    let open = dict! { "rid" => k.rid.as_str() };
    if web {
        open.set("guest", true);
    }
    let opened = ask(&srv, "pair_open", &open, 10)?;
    // 旧版的服务器不认 guest 这一项、照普通的配对开了：手机输入配对码会被拒，所以这里就停下
    let link = if web {
        match (opened.at("guest").as_bool(), app_link(&srv, &crypto::code_text(&code))) {
            (Some(true), Some(link)) => Some(link),
            (Some(true), None) => {
                let _ = srv.post("pair_close", &dict! { "rid" => k.rid.as_str() }, 5);
                return die(format!("{}的地址、证书指纹做不成给手机扫的二维码", relay::label(srv.kind)));
            }
            _ => {
                let _ = srv.post("pair_close", &dict! { "rid" => k.rid.as_str() }, 5);
                return die(srv.no_guest());
            }
        }
    } else {
        None
    };
    let na = urandom(32);
    let m = crypto::pair_msg(&k.keys, &k.rid, "a", &srv.fp, &na, &peers::machine_name())?;
    ask(&srv, "pair_put", &dict! { "rid" => k.rid.as_str(), "m" => m }, 10)?;
    crate::out!("code {}", crypto::code_text(&code));
    if let Some(link) = &link {
        crate::out!("link {link}");
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

/// `remote-pair-join [--hub 服务器]`：stdin 一行是另一台机器显示的配对码。配好以后 stdout 一行：对方的名字。
/// 没说在哪台服务器上：这台机器用得了的几台挨个试——配对码是在哪台上开的，哪台才认它（别的回“没有这个配对”，马上就回）
pub fn cmd_remote_pair_join(args: &[String]) -> R<()> {
    let (hub, rest) = hub_arg(args)?;
    arity("cmd_remote_pair_join", &rest, 0, 0)?;
    let Some(code) = crypto::code_bytes(&read_stdin_line()) else {
        return die("配对码不对：是 16 个字母和数字，照另一台机器上显示的输入（大小写、横线随意）");
    };
    let candidates = match hub.as_deref() {
        Some(_) => vec![need_server(hub.as_deref())?],
        None => {
            let all = relay::servers()?;
            if all.is_empty() {
                vec![need_server(None)?] // 一台都用不了：照它说原因
            } else {
                all
            }
        }
    };
    let k = crypto::pair_keys(&code);
    let nb = urandom(32);
    let mut found = None;
    let mut trouble: Option<String> = None;
    for srv in candidates {
        let m = crypto::pair_msg(&k.keys, &k.rid, "b", &srv.fp, &nb, &peers::machine_name())?;
        match srv.post("pair_put", &dict! { "rid" => k.rid.as_str(), "m" => m }, 10)? {
            Ok(_) => {
                found = Some(srv);
                break;
            }
            Err(f) if f.code == 404 && f.error == "no_pairing" => {}
            Err(f) if f.code == 400 => {
                return die("这个配对码用不了：它是这台机器自己显示的（要在另一台机器上输入），或者已经输入过一次了")
            }
            // 这一台问不了（连不上、没开中转）：别的照试，都没成再说它
            Err(f) => trouble = trouble.or(Some(f.text)),
        }
    }
    let Some(srv) = found else {
        return die(match trouble {
            Some(why) => format!("没找到这个配对码，而且有一台服务器没问到（{why}）：配对码要是在那台上开的，修好了再输一次"),
            None => "没有这个配对码：输错了，或者过期了（10 分钟内有效）".into(),
        });
    };
    let Some(theirs) = wait_theirs(&srv, &k.rid, monotonic() + 30.0)? else {
        return die("没等到对方那一份：显示配对码的那台机器还开着 cca remote pair 吗？");
    };
    let peer = conclude(&srv, &k, false, false, &nb, theirs)?;
    crate::out!("{}", peer.name);
    Ok(())
}

fn row(p: &Peer, note: &str) -> String {
    let kind = if p.web { "（手机）" } else { "" };
    let launch = if p.launch { "、能用模板新建会话" } else { "" };
    // 记在哪个 Team 的服务器上（经它看得到的是这个 Team 的账号的会话）
    let via = format!("  经{}", relay::label(p.hub));
    format!(
        "{}  {}{}  对方{}{}  {} 配对{}{}",
        &p.pid[..8],
        p.name,
        kind,
        p.incoming.text(),
        launch,
        strftime_local("%Y-%m-%d", p.at),
        via,
        note
    )
}

/// `remote-hubs`：这台机器现在用得了的中转服务器，一行一台：`编号<TAB>给人看的名字`（编号是 Team 的编号；3.6.0 起没有
/// maintainer 那一行了），没说在哪台上配对时用的那一台（home_hub）后面再有 `<TAB>home`。bash 拿它给人看配对记在哪台服务器上。
/// `--need`：配对之前（还没问同意，bash 的 remote_need_hub）看一眼：用得了的话什么都不输出；一台都用不了时和配对一样说原因
/// （need_server：不在哪个 Team 里、Team 还没接上服务器、配置还没同步下来……；stderr，退出码 1）
pub fn cmd_remote_hubs(args: &[String]) -> R<()> {
    arity("cmd_remote_hubs", args, 0, 1)?;
    let servers = relay::servers()?;
    match args.first().map(String::as_str) {
        None => {}
        Some("--need") if servers.is_empty() => {
            need_server(None)?;
            return die("远程会话经 Team 的服务器中转，这台机器现在一台都用不了");
        }
        Some("--need") => return Ok(()),
        Some(_) => return die("用法: remote-hubs [--need]"),
    }
    let home = relay::home_hub();
    for s in servers {
        let mark = if Some(s.kind) == home { "\thome" } else { "" };
        crate::out!("{}\t{}{mark}", s.kind.team(), relay::label(s.kind));
    }
    Ok(())
}

/// 给手机扫的二维码（iOS 上的 cca App 认它）：
/// `cca-remote://pair?v=1&u=服务器的地址&p=证书公钥的指纹（0～4 个）&c=配对码&n=这台机器的名字[&t=Team 的名字]`，
/// 每一项都百分号转义过。配对码本来就显示在同一页上：扫二维码和照着输入是一回事（服务器见不到它：只在手机和这台机器之间）。
/// 没有 p 时（托管平台：证书是证书机构签的）App 照系统的证书机构验。地址、指纹不像样（要画进二维码）时是 None
fn app_link(srv: &Server, code: &str) -> Option<String> {
    let cfg = srv.cfg();
    let url = cfg.at("url").py_str();
    let url = url.trim_end_matches('/');
    if !web_url_ok(&format!("{url}/")) || url.starts_with("http://") {
        return None;
    }
    let pins: Vec<String> = cfg.list("pins").map(|p| p.items().iter().map(|x| x.py_str()).collect()).unwrap_or_default();
    if pins.len() > 4 || !pins.iter().all(|p| pin_ok(p)) {
        return None;
    }
    let mut q = format!("v=1&u={}", pct(url));
    for p in &pins {
        q += &format!("&p={}", pct(p));
    }
    q += &format!("&c={}&n={}", code.replace('-', ""), pct(&clean(&peers::machine_name(), 48)));
    // Team 的名字（App 里给这台电脑标上是哪个 Team 的）
    q += &format!("&t={}", pct(&clean(&relay::team_name(srv.kind), 64)));
    Some(format!("cca-remote://pair?{q}"))
}

/// curl 的 `--pinnedpubkey` 那种写法：`sha256//` 加 44 个字符的 base64（32 个字节）
fn pin_ok(p: &str) -> bool {
    p.strip_prefix("sha256//").is_some_and(|b| {
        b.len() == 44 && b.ends_with('=') && b[..43].bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
    })
}

/// 网址里的一项：字母、数字和 `-._~` 原样，别的（UTF-8 的每个字节）写成 %XX
fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// 服务器的地址：要画进二维码，所以只认 `https://主机[:端口]/` 这一种样子（本机试的时候是
/// `http://localhost[:端口]/`、`http://127.0.0.1[:端口]/`——主机要整个相等，`localhost.别的域名` 不算）
fn web_url_ok(u: &str) -> bool {
    let (local, rest) = match (u.strip_prefix("https://"), u.strip_prefix("http://")) {
        (Some(r), _) => (false, r),
        (None, Some(r)) => (true, r),
        _ => return false,
    };
    let Some(hostport) = rest.strip_suffix('/') else { return false };
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    let host_ok = !host.is_empty()
        && host.len() <= 200
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        && (!local || host == "localhost" || host == "127.0.0.1");
    host_ok && port_ok
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
        // 配对记在哪台服务器上就问哪台；没问到的那一台上的配对，这次不知道对方取消了没有
        let seen = far::fetch_all(&all, 8)?;
        gone = all.iter().filter(|p| seen.asked.contains(&p.pid) && !seen.known.contains(&p.pid)).map(|p| p.pid.clone()).collect();
        for (_, why, asked) in &seen.errors {
            match asked {
                Some(_) => crate::err!("没问到服务器，对方那边取消了没有不知道：{why}"),
                None => crate::err!("{why}"),
            }
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

const ALLOW_USAGE: &str = "用法：remote-allow 名字 control|view|none | remote-allow 名字 launch on|off | remote-allow --show 名字";

/// `remote-allow 名字 control|view|none`：对方能对这台机器的会话做什么。开着的会话几秒钟内照新的来
/// （不让了的、改成只看的，正接着的那一处会被断开；不再是看和操作都让的，远程启动也一起关上）。stdout 一行：对方的名字。
/// `remote-allow 名字 launch on|off`：能不能用这台机器上的模板新建会话（launch.rs）：只给看和操作都让的，手机、配对过的电脑都行（同意 bash 问过）。
/// `remote-allow --show 名字`：只是找出是哪一个（bash 照它问同意）：一行 `配对编号<TAB>名字<TAB>手机 1|0<TAB>control|view|none<TAB>远程启动 1|0`
pub fn cmd_remote_allow(args: &[String]) -> R<()> {
    arity("cmd_remote_allow", args, 2, 3)?;
    if args[0] == "--show" {
        if args.len() != 2 {
            return die(ALLOW_USAGE);
        }
        let all = peers::load();
        let p = pick(&all, &args[1])?;
        crate::out!("{}\t{}\t{}\t{}\t{}", p.pid, p.name, u8::from(p.web), p.incoming.word(), u8::from(p.launch));
        return Ok(());
    }
    let _lock = Lock::acquire()?;
    let mut all = peers::load();
    let pid = pick(&all, &args[0])?.pid.clone();
    let p = all.iter_mut().find(|p| p.pid == pid).expect("刚找到的");
    match (args[1].as_str(), args.get(2).map(String::as_str)) {
        ("launch", Some(word @ ("on" | "off"))) => {
            let on = word == "on";
            if on && p.incoming != Incoming::Control {
                return die(format!("「{}」现在{}：先让它能看和操作（cca remote allow 名字 control）", p.name, p.incoming.text()));
            }
            p.launch = on;
        }
        (word, None) => {
            let Some(incoming) = Incoming::parse(word) else { return die(ALLOW_USAGE) };
            p.incoming = incoming;
            if incoming != Incoming::Control {
                p.launch = false;
            }
        }
        _ => return die(ALLOW_USAGE),
    }
    crate::out!("{}", p.name);
    let web = p.web;
    peers::save(&all)?;
    if web {
        super::status::status_kick(); // 手机上的账号页照新的来：不让接入了就盖掉服务器上的那一份（remote/status.rs）
    }
    Ok(())
}

/// 从这台机器上删掉一个配对（密钥也删），再告诉服务器。服务器没告诉成也算取消了：这边没有密钥，对方已经接不进来
fn drop_peer(pid: &str) -> R<Option<String>> {
    let hub;
    {
        let _lock = Lock::acquire()?;
        let mut all = peers::load();
        hub = all.iter().find(|p| p.pid == pid).map(|p| p.hub);
        all.retain(|p| p.pid != pid);
        peers::save(&all)?;
        peers::del_secret(pid)?;
    }
    // 告诉记着这个配对的那台服务器（刚才找到的配对，这期间被别的 cca 删掉了的话不用再说）
    let Some(hub) = hub else { return Ok(None) };
    let srv = match relay::server_of(hub)? {
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
    // 以前在维护者那一台上配的（load 不认了，那台服务器也没了）：写回配对表时只删这台机器上的密钥（peers::save）
    if peers::has_old() {
        let _lock = Lock::acquire()?;
        peers::save(&peers::load())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_parts() {
        assert_eq!(pct("https://192.0.2.10:18443"), "https%3A%2F%2F192.0.2.10%3A18443");
        assert_eq!(pct("我的 Mac"), "%E6%88%91%E7%9A%84%20Mac");
        assert_eq!(pct("a-b._~"), "a-b._~");
        assert!(pin_ok("sha256//47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="));
        for bad in [
            "",
            "sha256//",
            "sha256//47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU",
            "sha1//47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
            "sha256//47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSu&U=",
        ] {
            assert!(!pin_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn web_urls() {
        for ok in [
            "https://relay.example.com/",
            "https://relay.example.com:18445/",
            "https://192.0.2.10:8443/",
            "http://localhost:8080/",
            "http://127.0.0.1:8080/",
            "http://localhost/",
        ] {
            assert!(web_url_ok(ok), "{ok}");
        }
        // 没有结尾的斜杠、带路径 / 账号 / 空白 / 控制字符的、不是本机的明文地址、别的协议：都不认（要显示出来、画成二维码）
        for bad in [
            "",
            "https://",
            "https:///",
            "https://relay.example.com",
            "https://relay.example.com/x",
            "https://relay.example.com/?a=1",
            "https://user@relay.example.com/",
            "https://relay.example.com:/",
            "https://relay.example.com:123456/",
            "https://relay.example.com:80a/",
            "https://relay.example.com:1:2/",
            "https://relay example.com/",
            "https://relay.example.com/\nq=x",
            "https://relay.example.com\u{1b}[31m/",
            "http://relay.example.com/",
            "http://localhost.evil.example/",
            "http://127.0.0.1.evil.example/",
            "http://localhostx/",
            "javascript:alert(1)/",
            "ftp://relay.example.com/",
        ] {
            assert!(!web_url_ok(bad), "{bad:?}");
        }
    }
}
