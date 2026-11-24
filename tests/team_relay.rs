//! 每个 Team 的会话走这个 Team 自己的服务器中转（docs/dev/remote.md「每个 Team 各用各的中转服务器」）。
//!
//! 两台真的服务器进程，各是一个 Team 的同步服务器、都开着中转：「研发组」的（管理员是一台真的 cca：建 Team、接上服务器、发邀请，设备拿
//! 邀请加入）和「测试组」的（kit::relay 的 Base 起的：设备直接写好加入了的样子；它的数据端口前面隔着转发器，经过的字节都记下来）。
//! 两台设备先加入研发组、再加入测试组，跑的是 cca 的辅助程序。box 上三个会话：研发组给的账号的（alice）、这台设备自己的账号的（dave：
//! 走这台机器的「家」，加入的第一个 Team 的那一台）、测试组给的账号的（bob：走测试组的那一台）。看：配对记在哪台服务器上、经它看得到的
//! 是走那台服务器的会话；没说在哪台上配对时在家里那一台上配；输入配对码的那一头自己找得到是哪台。不联网。

use kit::fleet::{Device, ServerProc, WAIT};
use kit::fsx::{self, wait_for};
use kit::harness::{Ctx, Module};
use kit::json::{self, Style, Value};
use kit::relay::{self, Base, Popen};
use kit::term::{self, Term};
use kit::{case, list, obj, pki};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "team_relay",
    src: include_str!("team_relay.rs"),
    default: true,
    tests: &[
        case!(each_teams_sessions_go_through_its_own_server),
        case!(terminal),
        case!(pairing_on_a_team_server_without_relay),
        case!(need),
    ],
};

/// 账号表里的三个键：研发组给的账号、这台设备自己的账号、测试组给的账号
const TEAM_KEY: &str = "alice(alice@example.com)";
const OWN_KEY: &str = "mine(dave@example.com)";
const OTHER_KEY: &str = "other(bob@example.com)";

// ---- 小东西 ----

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

/// 对象的键，照里面的先后（`list(d)`）
fn names(v: &Value) -> Vec<String> {
    v.obj().keys().map(str::to_string).collect()
}

/// 字节里有没有这一截（`needle in out`）
fn has(hay: &[u8], needle: impl AsRef<[u8]>) -> bool {
    term::find(hay, needle).is_some()
}

/// Python 的 re.search(pattern, text) 找不找得到。正则照原来的写；只有末尾的 $ 换一种写法：Python 里它在最后那个换行的
/// 前面也算数，regex 这个库的 $ 只认真正的末尾
fn matches(pattern: &str, text: &str) -> bool {
    let pattern = match pattern.strip_suffix('$') {
        Some(head) => format!("{head}\n?\\z"),
        None => pattern.to_string(),
    };
    regex::Regex::new(&pattern).unwrap_or_else(|e| panic!("正则 {pattern:?} 写得不对：{e}")).is_match(text)
}

/// 对象里这几个键的值，照给的先后排成一个数组（原来拿元组比的地方）
fn cols(row: &Value, keys: &[&str]) -> Value {
    Value::List(keys.iter().map(|k| row[*k].clone()).collect())
}

/// 数组里每一项的这几个键（`[(p['a'], p['b']) for p in rows]`）
fn rows_of(rows: &Value, keys: &[&str]) -> Vec<Value> {
    rows.list().iter().map(|row| cols(row, keys)).collect()
}

/// 数组里每一项的这两个键（都是字符串），排好序（`sorted((s[a], s[b]) for s in rows)`）
fn sorted_pairs(rows: &Value, a: &str, b: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> =
        rows.list().iter().map(|row| (row[a].str().to_string(), row[b].str().to_string())).collect();
    pairs.sort();
    pairs
}

/// 排好序的一份
fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}

/// 输出的各行（`out.split('\n')[:-1]`：最后一个换行后面的不算）
fn split_lines(out: &str) -> Vec<String> {
    let mut lines: Vec<String> = out.split('\n').map(str::to_string).collect();
    lines.pop();
    lines
}

/// dev 上一条该成的辅助程序的命令，输出的各行
fn lines(dev: &Device, argv: &[&str]) -> Vec<String> {
    split_lines(&dev.ok(argv))
}

/// 这台服务器写下的端口
fn port_in(written: &str) -> u16 {
    written.trim().parse().unwrap_or_else(|_| panic!("端口文件里写的不是端口：{written:?}"))
}

/// 起服务器的那几个参数：监听 127.0.0.1 的随便一个端口，证书、状态文件都在 d 里，根公钥是 root
fn server_args(d: &Path, root: &str) -> Vec<String> {
    let mut args = vec!["--listen".to_string(), "127.0.0.1:0".to_string(), "--port-file".to_string(), text(&d.join("port"))];
    args.extend(["--cert".to_string(), text(&d.join("cert.pem")), "--key".to_string(), text(&d.join("key.pem"))]);
    args.extend(["--team-root".to_string(), root.to_string(), "--state".to_string(), text(&d.join("state.json"))]);
    args
}

// ---- 两个 Team、两台服务器（原来的 TeamServer、TeamRelayTest） ----

/// 「研发组」的同步服务器（自己的目录、证书），开着中转。离开作用域时杀掉
struct TeamServer {
    d: PathBuf,
    proc: ServerProc,
    url: String,
    pin: String,
}

impl TeamServer {
    fn start(d: &Path, root: &str) -> TeamServer {
        std::fs::create_dir(d).unwrap_or_else(|e| panic!("建 {}：{e}", d.display()));
        pki::make_cert(d);
        let mut args = server_args(d, root);
        args.extend(["--ttl", "60", "--idle", "600", "--relay-listen", "127.0.0.1:0", "--relay-port-file"].map(String::from));
        args.push(text(&d.join("rport")));
        // 端口文件没写下、服务器就退出了的话马上算失败（带上它说的话），不干等
        let mut proc = ServerProc::spawn(&args, &[]);
        let port = port_in(&proc.wait_file(&d.join("port"), "Team 的服务器起来", WAIT));
        proc.wait_file(&d.join("rport"), "它的数据端口起来", WAIT);
        TeamServer { d: d.to_path_buf(), proc, url: format!("https://127.0.0.1:{port}"), pin: pki::openssl_pin(d.join("cert.pem")) }
    }

    /// 这台服务器记着的配对
    fn edges(&self) -> Value {
        relay::edges_in(&self.d)
    }
}

impl Drop for TeamServer {
    fn drop(&mut self) {
        self.proc.kill();
    }
}

/// 原来 TeamRelayTest 的 setUp 备下的：研发组（管理员 admin 建的，编号 tid，服务器 rsrv）、测试组（fx）、两台设备 box 和 laptop。
/// 字段照收场的先后排：设备先、研发组的服务器其次、测试组的服务器最后
struct TeamFx {
    tid: String,
    a: Device,
    b: Device,
    _admin: Device,
    rsrv: TeamServer,
    fx: Base,
}

/// 一台设备：先拿邀请加入研发组（它给 alice），再加入测试组（它给 bob）；账号表里还有一个这台设备自己的（dave）
fn member(fx: &Base, name: &str, admin: &Device, tid: &str) -> Device {
    let mut dev = fx.device_unjoined(name);
    let accounts = dev.cca.join("accounts.json");
    let mut table = fsx::read_json(&accounts);
    let rows = table.as_obj_mut().expect("账号表是对象");
    rows.insert(OWN_KEY, "US-B");
    rows.insert(OTHER_KEY, "US-A");
    fsx::write(&accounts, json::dumps(&table, &Style::default()));
    dev.config_edit(|c| {
        c.insert("nodes", list!["US-A", "US-B"]);
    });
    let invite = lines(admin, &["team-invite", tid, name, "--accounts", "claude:alice@example.com"]);
    assert_eq!(invite.len(), 3, "{invite:?}");
    let (link, problem) = (&invite[0], &invite[2]);
    assert_eq!(problem, "");
    assert_eq!(split_lines(&dev.ok_with(&["team-join", "--name", name], link, &[]))[5..], ["ok", "1"]);
    let (url, pins) = (fx.url(), [fx.pin.as_str()]);
    let doc = fx.team.doc(&url, &pins, &obj! { "accounts" => obj! { "claude" => obj! { "bob@example.com" => "US-A" } } });
    dev.join_with(&fx.team, &url, &pins, Some(&doc), None);
    let r = dev.run(&["team-apply"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert_eq!(
        sorted(names(&fsx::read_json(&accounts))),
        sorted(vec![TEAM_KEY.to_string(), OWN_KEY.to_string(), OTHER_KEY.to_string()])
    );
    dev
}

fn team_fx(t: &mut Ctx) -> TeamFx {
    let fx = Base::new(t);
    let admin = Device::new(&fx.d.join("admin"), "admin", &obj! {});
    let made = lines(&admin, &["team-new", "研发组", "--device", "studio"]);
    assert_eq!(made.len(), 3, "{made:?}");
    let (tid, root) = (made[0].as_str(), made[1].as_str());
    let rsrv = TeamServer::start(&fx.d.join("team-srv"), root);
    lines(&admin, &["team-attach", tid, &rsrv.url, &rsrv.pin]);
    lines(&admin, &["team-map", tid, "claude", "alice@example.com", "US-A"]);
    lines(&admin, &["team-alias", tid, "alice@example.com", "alice"]);
    let (a, b) = (member(&fx, "box", &admin, tid), member(&fx, "laptop", &admin, tid));
    fx.machines(&[&a, &b]); // 登记到测试组的服务器上
    TeamFx { tid: tid.to_string(), a, b, _admin: admin, rsrv, fx }
}

/// dev 上起一个在宿主里运行的会话：账号是 key，名字是 name（假客户端也拿这个名字当参数）
fn session(dev: &Device, key: &str, name: &str) -> Term {
    relay::session(dev, key, name, &[name])
}

/// shower 显示配对码（hub：--hub 服务器），joiner 输入（join_hub：它的 --hub）。返回这次配对在 joiner 的配对表里的编号
fn pair(shower: &Device, joiner: &Device, hub: &[&str], join_hub: &[&str]) -> String {
    let before: BTreeSet<String> = names(&relay::peers(joiner)).into_iter().collect();
    let mut p = Popen::spawn(&mut relay::helper_cmd(shower, &[&["remote-pair-show"], hub].concat()));
    let line = p.out_line();
    if !matches("^code ", &line) {
        panic!("没给配对码：{line:?} {}", if line.is_empty() { p.err_rest() } else { String::new() });
    }
    let code = format!("{}\n", line.split_whitespace().nth(1).expect("配对码"));
    let r = joiner.run_with(&[&["remote-pair-join"], join_hub].concat(), &code, &[]);
    assert_eq!((r.code, r.err.as_str()), (0, ""), "{}", r.out);
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, said.as_str()), (0, ""), "{out}");
    let added: Vec<String> = names(&relay::peers(joiner)).into_iter().filter(|pid| !before.contains(pid)).collect();
    assert_eq!(added.len(), 1, "{added:?}");
    added[0].clone()
}

/// dev 看到的别的机器上的会话（remote list --json）；两次之间隔开一点（服务器按设备限流）
fn far(dev: &Device) -> Value {
    std::thread::sleep(Duration::from_millis(1100));
    let r = dev.run(&["remote-list", "--far", "--json"]);
    assert_eq!(r.code, 0, "{}", r.err);
    json::parse(&r.out).unwrap_or_else(|e| panic!("remote-list --json 给的不是 JSON（{e}）：{}", r.out))
}

fn each_teams_sessions_go_through_its_own_server(t: &mut Ctx) {
    let f = team_fx(t);
    let (fx, rsrv, a, b, tid, other) = (&f.fx, &f.rsrv, &f.a, &f.b, f.tid.as_str(), f.fx.team.id.as_str());
    // 用得了两台：没说在哪台上配对时用的那一台（家）是先加入的那个 Team 的
    assert_eq!(lines(a, &["remote-hubs"]), [format!("{tid}\t「研发组」的服务器\thome"), format!("{other}\t「测试组」的服务器")]);
    let (mut ha, ho, hm) = (session(a, TEAM_KEY, "alice"), session(a, OWN_KEY, "mine"), session(a, OTHER_KEY, "other"));
    assert_eq!(a.run(&["remote-pair-show", "--hub", "没有这个组"]).code, 1);
    assert_eq!((rsrv.edges(), fx.edges()), (obj! {}, obj! {}));

    // 没说在哪台上配对：在家里那一台（研发组的服务器）上配，记在它上面，测试组的那一台不知道；输入配对码的那一头没说在哪台，自己找到了
    let team_pid = pair(a, b, &[], &[]);
    let team_pid = team_pid.as_str();
    assert_eq!((names(&rsrv.edges()), fx.edges()), (vec![team_pid.to_string()], obj! {}));
    for (dev, shown) in [(a, true), (b, false)] {
        let table = relay::peers(dev);
        assert_eq!(cols(&table[team_pid], &["hub", "shown"]), list![tid, shown]);
    }
    assert!(a.ok(&["remote-peers"]).contains("经「研发组」的服务器"));
    // 经它看得到的：研发组给的账号的会话，和这台设备自己的账号的会话（走家里那一台）；测试组给的账号的会话走测试组的那一台，
    // 那边还没配对：看不到
    let doc = wait_for("laptop 看到会话", WAIT, || Some(far(b)).filter(|d| d["sessions"].list().len() == 2));
    let both = |pid: &str| vec![("alice".to_string(), pid.to_string()), ("mine".to_string(), pid.to_string())];
    assert_eq!(sorted_pairs(&doc["sessions"], "name", "peer"), both(team_pid));
    assert_eq!(rows_of(&doc["peers"], &["id", "team", "state"]), [list![team_pid, tid, "ok"]]);
    assert_eq!(
        doc["relay"]["hubs"],
        list![obj! { "team" => tid, "name" => "「研发组」的服务器" }, obj! { "team" => other, "name" => "「测试组」的服务器" }]
    );
    assert_eq!(doc["errors"], list![]);
    // 在研发组的服务器上配对，名字、编号都认
    assert_eq!(rsrv.edges().obj().len(), 1);

    // 再在测试组的那一台上配一次（同两台设备，各记各的、各有一把密钥）：这才看得到测试组给的那个账号的会话，走的是测试组的服务器
    let other_pid = pair(a, b, &["--hub", "测试组"], &["--hub", other]);
    let other_pid = other_pid.as_str();
    assert_eq!((names(&rsrv.edges()), names(&fx.edges())), (vec![team_pid.to_string()], vec![other_pid.to_string()]));
    assert_eq!(relay::peers(b)[other_pid]["hub"].str(), other, "配对表里记着是哪个 Team 的服务器");
    assert_ne!(relay::secret(b, team_pid), relay::secret(b, other_pid));
    let doc = wait_for("laptop 看到三个会话", WAIT, || Some(far(b)).filter(|d| d["sessions"].list().len() == 3));
    let mut three = both(team_pid);
    three.push(("other".to_string(), other_pid.to_string()));
    assert_eq!(sorted_pairs(&doc["sessions"], "name", "peer"), three);
    assert_eq!(
        sorted_pairs(&doc["peers"], "id", "team"),
        sorted(vec![(team_pid.to_string(), tid.to_string()), (other_pid.to_string(), other.to_string())])
    );

    // 接入研发组的那个会话：经研发组的服务器握手、收发（测试组那一台的转发器上一个字节都没有经过它的内容）
    let alice = doc["sessions"].list().iter().find(|s| s["name"].str() == "alice").expect("alice 的会话");
    let sid = alice["id"].str();
    let seen = fx.sniff.snapshot().len();
    let mut v = relay::term(b, &["remote-attach", &sid[..10]]);
    v.expect("<ready 24 80");
    v.send("say via the team server\r");
    v.expect("<via the team server>");
    ha.expect("<via the team server>");
    assert!(!has(ho.out(), "via the team server") && !has(hm.out(), "via the team server"));
    assert_eq!(fx.sniff.snapshot().len(), seen, "这个会话的数据不经测试组的服务器");

    // 取消研发组上的那个配对：告诉的是研发组的服务器，测试组那一台上的照旧
    assert_eq!(lines(b, &["remote-unpair", &team_pid[..8]]), ["box"]);
    assert_eq!((rsrv.edges(), names(&fx.edges())), (obj! {}, vec![other_pid.to_string()]));
    let doc = wait_for("只剩测试组那一台上的", WAIT, || {
        Some(far(b)).filter(|d| rows_of(&d["sessions"], &["name"]) == [list!["other"]])
    });
    assert_eq!(rows_of(&doc["peers"], &["id", "team"]), [list![other_pid, other]]);
}

// ---- bash 这一层（真的 cca 脚本）：cca remote pair --team 名字，cca remote join 自己找到是哪台服务器（原来的 ShellTest） ----

/// dev 上 cca remote pair 参数…（同意）：返回 (进程, 配对码, 到配对码为止 stderr 上说的)
fn shown(fx: &Base, dev: &Device, argv: &[&str]) -> (Popen, String, String) {
    let mut p = Popen::spawn(&mut fx.cca_cmd(dev, &[&["remote", "pair"], argv].concat(), &[("CCA_TTY", &fx.answers(&["y"]))]));
    let mut seen = String::new();
    let code = loop {
        let line = p.err_line();
        assert!(!line.is_empty(), "没等到配对码：{seen}");
        seen.push_str(&line);
        if line.contains("配对码：") {
            break line.trim().split('：').nth(1).expect("配对码：后面是配对码").to_string();
        }
    };
    (p, code, seen)
}

fn terminal(t: &mut Ctx) {
    let f = team_fx(t);
    let (fx, rsrv, a, b, tid) = (&f.fx, &f.rsrv, &f.a, &f.b, f.tid.as_str());
    for bad in ["--team", "--nope"] {
        let r = fx.cca(a, &["remote", "pair", bad]);
        assert_eq!((r.code, r.out.as_str()), (1, ""), "{bad}");
        assert!(r.err.contains("用法：cca remote pair [--web] [--team 名字"), "{}", r.err);
    }
    assert_eq!(fx.cca(b, &["remote", "join", "--web"]).code, 1);
    // 用得了两台、没说在哪台上：在家里那一台（先加入的研发组的）上配，问之前说清楚记在哪台上、对方看得到的是哪些会话、
    // 要在别的上面配怎么写；另一台输入时不用说在哪台
    let (mut p, code, seen) = shown(fx, a, &[]);
    assert!(seen.contains("这次配对记在「研发组」的服务器上：对方经它看得到的，是走这台服务器的会话"), "{seen}");
    assert!(seen.contains("要在别的 Team 的服务器上配对：--team 名字"), "{seen}");
    let r = fx.cca_with(b, &["remote", "join"], Some(&[&code]), None);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert!(r.err.contains("已经和「box」配对"), "{}", r.err);
    assert_eq!(p.wait(secs(60)), 0, "{}", p.err_rest());
    let on_team = names(&rsrv.edges());
    assert_eq!(on_team.len(), 1, "{on_team:?}");
    let team_pid = on_team[0].as_str();
    assert_eq!((relay::peers(b)[team_pid]["hub"].str(), fx.edges()), (tid, obj! {}));
    // 写明了在研发组的服务器上（名字）：不再说要在别的上面配怎么写
    let (mut p, _code, seen) = shown(fx, a, &["--team", "研发组"]);
    assert!(seen.contains("这次配对记在「研发组」的服务器上"), "{seen}");
    assert!(!seen.contains("要在别的 Team 的服务器上配对"), "{seen}");
    p.signal(libc::SIGTERM); // 不配了：cca 退出时停掉后台的辅助程序（SIGKILL 的话 EXIT 的 trap 不跑，它会一直等着）
    assert_eq!(p.wait(secs(30)), 130);
    // 测试组的那一台：输入的那一头指定了研发组（写编号）的话，在研发组的服务器上找不到这个配对码
    let (mut p, code, seen) = shown(fx, a, &["--team", "测试组"]);
    assert!(seen.contains("这次配对记在「测试组」的服务器上"), "{seen}");
    let r = fx.cca_with(b, &["remote", "join", "--team", tid], Some(&[&code]), None);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("没有这个配对码"), "{}", r.err);
    let r = fx.cca_with(b, &["remote", "join", "--team", "测试组"], Some(&[&code]), None);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    assert_eq!(p.wait(secs(60)), 0, "{}", p.err_rest());
    assert_eq!((fx.edges().obj().len(), names(&rsrv.edges())), (1, vec![team_pid.to_string()]));
    let r = fx.cca(b, &["remote", "peers"]);
    let via: Vec<&str> =
        kit::sandbox::splitlines(&r.out).into_iter().map(|line| line.rsplit_once("  ").map_or(line, |(_, last)| last)).collect();
    assert_eq!(sorted(via), ["经「测试组」的服务器", "经「研发组」的服务器"]);
}

// ---- 一个 Team 的服务器没开中转（原来的 NoRelayTest；测试组的那一台开着） ----

fn pairing_on_a_team_server_without_relay(t: &mut Ctx) {
    let fx = Base::new(t);
    let admin = Device::new(&fx.d.join("admin"), "admin", &obj! {});
    let made = admin.ok(&["team-new", "研发组", "--device", "studio"]);
    let made: Vec<&str> = made.split('\n').collect();
    let (tid, root) = (made[0], made[1]);
    let d = fx.d.join("team-srv");
    std::fs::create_dir(&d).unwrap_or_else(|e| panic!("建 {}：{e}", d.display()));
    pki::make_cert(&d);
    // 端口文件没写下、服务器就退出了的话马上算失败，不干等
    let mut proc = ServerProc::spawn(&server_args(&d, root), &[]);
    let port = port_in(&proc.wait_file(&d.join("port"), "Team 的服务器起来", WAIT));
    admin.ok(&["team-attach", tid, &format!("https://127.0.0.1:{port}"), &pki::openssl_pin(d.join("cert.pem"))]);
    admin.ok(&["team-map", tid, "claude", "alice@example.com", "US-A"]);
    let dev = fx.device("box"); // 在测试组里（它的服务器开着中转）
    dev.config_edit(|c| {
        c.insert("nodes", list!["US-A", "US-B"]);
    });
    let invite = admin.ok(&["team-invite", tid, "box", "--accounts", "claude:alice@example.com"]);
    let link = invite.split('\n').next().expect("邀请链接");
    dev.ok_with(&["team-join", "--name", "box"], link, &[]);
    fx.machines(&[&dev]);
    let r = dev.run(&["remote-pair-show", "--hub", tid]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("「研发组」的服务器还没开远程会话的中转（管理员要重新部署、把中转打开：cca team deploy）"), "{}", r.err);
    // 测试组的那一台照常配得了
    let mut p = Popen::spawn(&mut relay::helper_cmd(&dev, &["remote-pair-show", "--hub", &fx.team.id]));
    let line = p.out_line();
    assert!(matches(r"^code [0-9A-Z]{4}(-[0-9A-Z]{4}){3}$", &line), "{line:?}");
}

/// cca remote pair / join 问同意之前先看一眼（remote-hubs --need，bash 的 remote_need_hub）：一台都用不了时说的原因和配对时
/// 一样（不在哪个 Team 里、Team 还没接上服务器……），用得了时什么都不说
fn need(t: &mut Ctx) {
    let fx = Base::new(t);
    let admin = Device::new(&fx.d.join("admin"), "admin", &obj! {});
    let r = admin.run(&["remote-hubs", "--need"]);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (1, "", "先加入或者新建一个 Team：远程会话经 Team 的服务器中转\n"));
    admin.ok(&["team-new", "研发组", "--device", "studio"]);
    let r = admin.run(&["remote-hubs", "--need"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("「研发组」还没有接上同步服务器"), "{}", r.err);
    let dev = fx.device("box"); // 在测试组里，它的服务器用得了
    let r = dev.run(&["remote-hubs", "--need"]);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
    let r = dev.run(&["remote-hubs", "--nope"]);
    assert_eq!((r.code, r.out.as_str()), (1, ""));
    assert!(r.err.contains("用法"), "{}", r.err);
}
