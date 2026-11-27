//! 远程启动（docs/dev/launch.md）：配对过的手机照这台机器上的模板新建会话、接进去；配对过的电脑也行（`remote-launch`，Mac 上的
//! cca App 用它的 `--json`：a_machine_launches_a_session）。
//!
//! 真的服务器（一个 Team 的同步服务器，开着中转）、真的辅助程序：这台机器（box）上跑 `remote-launcher serve`（LaunchAgent、
//! crontab 起的就是它），手机那一头是 kit::spec 照文档独立实现的那一份（SpecPeer）。手机配对本身（网页端、App 的访客）在
//! web_relay.rs、ios_kit.rs 里测：这里的“手机”是一台照文档说话的设备，box 的配对表里那一行标成手机（web）——启动器只照配对表认手机。
//! 启动器起的会话里的“cca”是假客户端（启动器的 CCA_SELF 指着它）：它一启动报一行 `<ready 行 列 … args=[…]>`，看得到起它的参数；
//! 环境、当前目录让它 `run` 一下 env、pwd 看。

use kit::fleet::{Device, Team};
use kit::fsx::{self, wait_for};
use kit::harness::{Ctx, Module};
use kit::json::Value;
use kit::relay::{self, cols, frame, has, parsed, secs, Base, Popen, Reader, SpecPeer, EXIT, HELLO, INPUT, LAUNCH, OUTPUT};
use kit::{case, list, obj};
use std::collections::BTreeSet;
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "remote_launch",
    src: include_str!("remote_launch.rs"),
    default: true,
    tests: &[
        case!(a_phone_launches_a_session),
        case!(what_the_launcher_refuses),
        case!(what_changed_on_the_host),
        case!(a_machine_launches_a_session),
    ],
};

/// 同一台手机两次接入启动器之间至少隔这么久（launch.rs 的 JOIN_GAP 是 3 秒：太勤的不理）
const GAP: Duration = Duration::from_millis(3200);

/// a 显示配对码、phone 照文档输入，配好以后 a 的配对表里把这一行标成手机（真的手机经 remote pair --web 配上就是这样）
fn phone<'a>(fx: &'a Base, a: &Device, b: &'a Device) -> SpecPeer<'a> {
    let mut peer = SpecPeer::new(fx, b);
    let mut p = Popen::spawn(&mut relay::helper_cmd(a, &["remote-pair-show"]));
    let line = p.out_line();
    peer.join(line.split_whitespace().nth(1).expect("配对码"));
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out), (0, format!("peer {}\n", b.name)), "{said}");
    let f = a.cca.join("remote").join("peers.json");
    let mut j = fsx::read_json(&f);
    let row = j.as_obj_mut().and_then(|o| o.get_mut("peers")).and_then(|p| p.as_obj_mut()).and_then(|p| p.get_mut(&peer.pid));
    row.and_then(|r| r.as_obj_mut()).expect("配对表里有这一行").insert("web", true);
    fsx::write_json(&f, &j);
    peer
}

/// a 上加一个模板（alice 的 Claude 账号，在 dir 里，带着 args）。返回 (编号, 摘要)
fn template(a: &Device, name: &str, dir: &str, args: &[&str]) -> (String, String) {
    template_for(a, name, "alice", dir, args)
}

/// 同 [`template`]，账号是 account（账号表里的别名或者邮箱）
fn template_for(a: &Device, name: &str, account: &str, dir: &str, args: &[&str]) -> (String, String) {
    let mut argv = vec!["remote-template", "add", name, "claude", account, dir, "--"];
    argv.extend(args);
    let out = a.ok(&argv);
    let (id, got) = out.trim_end().split_once('\t').expect("编号<TAB>名字");
    assert_eq!(got, name);
    let doc = parsed(&a.ok(&["remote-template", "list", "--json"]));
    let row = doc["templates"].list().iter().find(|t| t["id"].str() == id).expect("列出来了").clone();
    (id.to_string(), row["digest"].str().to_string())
}

/// a 上起启动器（前台的 serve）：它起的会话里的 cca 换成假客户端；环境里多一个不该带进会话的变量
fn launcher(a: &Device) -> Popen {
    let mut cmd = relay::helper_cmd(a, &["remote-launcher", "serve"]);
    cmd.env("CCA_SELF", relay::client(a)).env("CCA_TEST_CANARY", "do-not-pass");
    Popen::spawn(&mut cmd)
}

/// LAUNCH 的内容：200 列（假客户端报到的那一行比 100 列长：接入时它已经说完了的话，宿主回放的是画好的屏幕，100 列会折行）
fn ask(id: &str, digest: &str, line: Option<&str>) -> Value {
    let mut v = obj! { "v" => 1, "t" => id, "h" => digest, "c" => 200, "r" => 30 };
    if let Some(l) = line {
        v.as_obj_mut().expect("对象").insert("l", l);
    }
    v
}

/// 没起成：(code, 说明里有字)
fn refused(got: &Value) -> (String, bool) {
    assert_eq!((got["v"].i64(), got["ok"].as_bool()), (1, Some(false)), "{got}");
    (got["code"].str().to_string(), !got["text"].str().is_empty())
}

fn log_of(a: &Device) -> String {
    fsx::try_read_text(a.cca.join("remote").join("launch.log")).unwrap_or_default()
}

fn a_phone_launches_a_session(t: &mut Ctx) {
    let fx = Base::new(t);
    let (a, b) = (fx.device("box"), fx.device("phone"));
    fx.machines(&[&a, &b]);
    let peer = phone(&fx, &a, &b);
    let pid = peer.pid.clone();
    let work = relay::work(&a);
    let (id, digest) = template(&a, "修 bug", work.to_str().expect("UTF-8"), &["--model", "opus"]);

    // 还没有哪台手机能远程启动：启动器什么都不做就退出（退出码 0：launchd 不再起它）
    assert_eq!(a.run(&["remote-launcher", "wanted"]).code, 1);
    assert_eq!(launcher(&a).wait(secs(15)), 0);
    // 打开：只有能操作的（手机、电脑都行：电脑的见 a_machine_launches_a_session）
    a.ok(&["remote-allow", &pid, "view"]);
    let r = a.run(&["remote-allow", &pid, "launch", "on"]);
    assert!(r.code != 0 && r.err.contains("先让它能看和操作"), "{r:?}");
    a.ok(&["remote-allow", &pid, "control"]);
    assert_eq!(a.ok(&["remote-allow", &pid, "launch", "on"]), "phone\n");
    assert_eq!(a.run(&["remote-launcher", "wanted"]).code, 0);
    let show = a.ok(&["remote-allow", "--show", "phone"]);
    assert_eq!(show, format!("{pid}\tphone\t1\tcontrol\t1\n"));

    let mut l = launcher(&a);
    let (lsid, doc) = wait_for("启动器的说明", secs(30), || peer.launcher());
    // 说明：机器的名字、模板（编号、摘要、名字、工具、账号的短名字、~ 开头的文件夹、参数、能带开场白）；账号的邮箱不给
    assert_eq!((doc["v"].i64(), doc["machine"].str() != "", doc["more"].i64()), (1, true, 0));
    assert_eq!(doc["t"].list().len(), 1, "{doc}");
    let row = &doc["t"].list()[0];
    assert_eq!(
        cols(row, &["i", "h", "n", "k", "a", "d", "x", "l"]),
        list![id.as_str(), digest.as_str(), "修 bug", "claude", "alice", "~/work", "--model opus", true]
    );
    assert_eq!(row["aid"].str().len(), 32, "Team 给的账号带账号摘要：{row}");
    assert!(!doc.compact().contains("example.com"));
    assert!(peer.sessions().is_empty(), "启动器不算会话");
    // 第二个启动器起不来（锁着）：说一句；--if-down（crontab 每 10 分钟那一下）什么都不说
    let r = a.run(&["remote-launcher", "serve"]);
    assert!(r.code == 0 && r.err.contains("已经在运行"), "{r:?}");
    assert_eq!(a.run(&["remote-launcher", "serve", "--if-down"]).err, "");
    let list_doc = parsed(&a.ok(&["remote-list", "--local", "--json"]));
    assert_eq!(list_doc["launcher"], obj! { "running" => true, "templates" => 1, "peers" => 1 });
    assert_eq!(list_doc["peers"].list()[0]["launch"], Value::Bool(true));

    // 起一个：带开场白。回的是新会话的编号；它在列表里，接得上
    let got = peer.launch(&lsid, &ask(&id, &digest, Some("你好 world")));
    assert_eq!((got["v"].i64(), got["ok"].as_bool()), (1, Some(true)), "{got}");
    let sid = got["sid"].str().to_string();
    assert!(sid.len() == 32 && sid.bytes().all(|c| c.is_ascii_hexdigit()), "{sid}");
    let found = wait_for("新会话", secs(30), || peer.sessions().into_iter().find(|s| s["sid"].str() == sid));
    assert_eq!(cols(&found, &["tool", "name", "cwd", "access"]), list!["claude", "alice", "~/work", "control"]);
    let (s, ch) = peer.attach(&sid);
    let mut r = Reader::new(s, ch.expect("握手：宿主的证明对得上"));
    r.send(&frame(HELLO, &[1, 0, 0, 200, 0, 30]));
    // 客户端（cca）收到的：账号、--、模板的参数、--、开场白；窗口是手机说的大小；没有终端也是会话的头、有控制终端（伪终端）
    let want =
        format!("200 leader=True ctty=True tty=True args=['@{}', '--', '--model', 'opus', '--', '你好 world']>", Device::KEY);
    r.until(|out| has(out, &want));
    // （刚起的客户端可能赶上接入时让它重画的那一下：报到时少一行，接着回到 30 行）
    r.until(|out| has(out, "<ready 30 200 ") || has(out, "<size 30 200>"));
    // 环境只带白名单里的，加上 CCA_REMOTE、CCA_REMOTE_HOSTED（bash 的 cca 照它不再套一层宿主）；当前目录是模板的文件夹
    // （run 起的程序的输出不到终端上：让它写进当前目录里的文件）
    r.send(&frame(INPUT, b"run /bin/sh -c env>env.txt;pwd>pwd.txt\n"));
    r.until(|out| has(out, "<ran 0>"));
    let env = fsx::read_text(work.join("env.txt"));
    for line in ["CCA_REMOTE=1", &format!("CCA_REMOTE_HOSTED={sid}"), "TERM=xterm-256color", "CCA_KEYSTORE=file"] {
        assert!(env.lines().any(|l| l == line), "{line}：{env}");
    }
    for word in ["CCA_TEST_CANARY", "CCA_SELF=", "CCA_TOOL="] {
        assert!(!env.contains(word), "{word} 不该带进会话：{env}");
    }
    assert_eq!(fsx::read_text(work.join("pwd.txt")).trim_end(), work.to_str().expect("UTF-8"));
    // 会话的通道上发 LAUNCH：转接进程不转（只有启动器认它），什么都不起
    let before = log_of(&a);
    r.send(&frame(LAUNCH, ask(&id, &digest, None).compact().as_bytes()));
    r.send(&frame(INPUT, b"say still-here\n"));
    r.until(|out| has(out, "<still-here>"));
    assert_eq!(r.kinds, BTreeSet::from([OUTPUT]));
    // 记下了：什么时候、哪台手机、哪个模板、起了哪个会话（不记开场白）
    let log = log_of(&a);
    assert_eq!(log, before);
    assert!(log.contains("phone") && log.contains("修 bug") && log.contains(&format!("起了会话 {}", &sid[..8])), "{log}");
    assert!(!log.contains("你好"));
    // 会话结束：手机收到退出码；接入过的无头会话不再留着
    r.send(&frame(INPUT, b"exit 3\n"));
    assert!(has(r.until(|out| has(out, "<EXIT")), "<EXIT 3>"));
    assert!(r.kinds.contains(&EXIT));
    wait_for("会话从列表里没了", secs(30), || !a.ok(&["remote-list", "--local"]).contains(&sid[..8]));

    // 启动器重启：还是原来那个编号（手机上不用重新找）
    l.signal(libc::SIGTERM);
    assert_eq!(l.wait(secs(15)), 0);
    let state = fsx::read_json(a.cca.join("remote").join("launcher.json"));
    assert_eq!(state["sids"].obj().values().map(|v| v.str().to_string()).collect::<Vec<_>>(), vec![lsid.clone()]);
    let mut l = launcher(&a);
    wait_for("重启以后的启动器", secs(30), || peer.launcher().filter(|(s, _)| *s == lsid));
    // 关掉：启动器自己退出（没有手机要了），服务器上它的说明跟着没了
    assert_eq!(a.ok(&["remote-allow", &pid, "launch", "off"]), "phone\n");
    assert_eq!(l.wait(secs(20)), 0);
    wait_for("启动器的说明没了", secs(30), || peer.launcher().is_none());
    assert_eq!(a.run(&["remote-launcher", "wanted"]).code, 1);
    // 改成只看也关掉远程启动（再改回能操作时不自己打开）
    a.ok(&["remote-allow", &pid, "launch", "on"]);
    a.ok(&["remote-allow", &pid, "view"]);
    a.ok(&["remote-allow", &pid, "control"]);
    assert_eq!(a.run(&["remote-launcher", "wanted"]).code, 1);
}

fn what_the_launcher_refuses(t: &mut Ctx) {
    let fx = Base::new(t);
    let (a, b) = (fx.device("box"), fx.device("phone"));
    fx.machines(&[&a, &b]);
    let peer = phone(&fx, &a, &b);
    let pid = peer.pid.clone();
    let work = relay::work(&a);
    let gone = work.join("gone");
    std::fs::create_dir(&gone).expect("建文件夹");
    let (id, digest) = template(&a, "工作", work.to_str().expect("UTF-8"), &[]);
    let (gid, gdigest) = template(&a, "要删掉的文件夹", gone.to_str().expect("UTF-8"), &[]);
    a.ok(&["remote-allow", &pid, "launch", "on"]);
    let mut l = launcher(&a);
    let (lsid, doc) = wait_for("启动器的说明", secs(30), || peer.launcher().filter(|(_, d)| d["t"].list().len() == 2));
    assert_eq!(doc["t"].list().iter().map(|r| r["n"].str()).collect::<Vec<_>>(), ["工作", "要删掉的文件夹"]);

    // 模板在这台机器上改过了（手机拿的是改之前的摘要）：不起，说改过了
    let (_, changed) = {
        let out = a.ok(&[
            "remote-template",
            "add",
            "工作",
            "claude",
            "alice",
            work.to_str().expect("UTF-8"),
            "--replace",
            &id,
            "--",
            "--model",
            "fable",
        ]);
        assert_eq!(out, format!("{id}\t工作\n"));
        let doc = parsed(&a.ok(&["remote-template", "list", "--json"]));
        (id.clone(), doc["templates"].list()[0]["digest"].str().to_string())
    };
    assert_ne!(changed, digest);
    assert_eq!(refused(&peer.launch(&lsid, &ask(&id, &digest, None))), ("changed".to_string(), true));
    // 没有这个模板；请求不对（窗口太小、开场白是空白、有控制字符）
    std::thread::sleep(GAP);
    assert_eq!(refused(&peer.launch(&lsid, &ask("0badc0de", &digest, None))), ("no_template".to_string(), true));
    // 同一台手机接得太勤（上一次接进来不到 3 秒）：照样接上、回它 rate（不理的话手机要等到服务器说启动器不在，说的原因就错了）
    assert_eq!(refused(&peer.launch(&lsid, &ask(&id, &changed, None))), ("rate".to_string(), true));
    // LAUNCH 缺了一项：看不懂（bad），不能崩（崩了手机收不到回话）
    std::thread::sleep(GAP);
    let mut short = ask(&id, &changed, None);
    short.as_obj_mut().expect("对象").remove("r");
    assert_eq!(refused(&peer.launch(&lsid, &short)).0, "bad");
    std::thread::sleep(GAP);
    let mut small = ask(&id, &changed, None);
    small.as_obj_mut().expect("对象").insert("c", 10);
    assert_eq!(refused(&peer.launch(&lsid, &small)).0, "bad");
    std::thread::sleep(GAP);
    assert_eq!(refused(&peer.launch(&lsid, &ask(&id, &changed, Some("a\u{1b}[31m")))).0, "bad");
    // 模板的文件夹没了
    std::fs::remove_dir(&gone).expect("删文件夹");
    std::thread::sleep(GAP);
    assert_eq!(refused(&peer.launch(&lsid, &ask(&gid, &gdigest, None))).0, "no_dir");
    // 说明里也不再有它（下一次发说明的时候：模板的文件变了就重发）
    a.ok(&["remote-template", "rm", "要删掉的文件夹"]);
    wait_for("说明里只剩一个模板", secs(30), || peer.launcher().filter(|(_, d)| d["t"].list().len() == 1));
    let log = log_of(&a);
    for code in ["changed", "no_template", "rate", "bad", "no_dir"] {
        assert!(log.contains(&format!("没起（{code}）")), "{code}：{log}");
    }
    // 一个会话都没起
    assert!(peer.sessions().is_empty());
    assert_eq!(a.ok(&["remote-list", "--local"]), "");
    l.signal(libc::SIGTERM);
    assert_eq!(l.wait(secs(15)), 0);
}

/// 模板加好以后这台机器上变了的：账号走别的服务器、参数现在过不了检查、账号删了、会话已经太多；参数里已经有一句话的模板不能带开场白；
/// 关掉一台手机的远程启动（另一台还开着、启动器接着跑）以后它接不进启动器
fn what_changed_on_the_host(t: &mut Ctx) {
    let fx = Base::new(t);
    let (mut a, b, c) = (fx.device("box"), fx.device("phone"), fx.device("phone2"));
    fx.machines(&[&a, &b, &c]);
    // 这台机器自己的账号 dave（走加入的第一个 Team 的服务器，就是这一台）、另一个 Team 给的 bob（走那个 Team 的服务器：没起，
    // 启动器也不连它——没有在那台上配的手机）
    let accounts = a.cca.join("accounts.json");
    let mut table = fsx::read_json(&accounts);
    let rows = table.as_obj_mut().expect("账号表是对象");
    rows.insert("dave(dave@example.com)", "US-A");
    rows.insert("bob(bob@example.com)", "US-A");
    fsx::write_json(&accounts, &table);
    let other = Team::named(&fx.d, "另一个组", "root2", &"11".repeat(16));
    let (url, pins) = ("https://127.0.0.1:9", [fx.pin.as_str()]);
    let doc = other.doc(url, &pins, &obj! { "accounts" => obj! { "claude" => obj! { "bob@example.com" => "US-A" } } });
    a.join_with(&other, url, &pins, Some(&doc), None);
    let r = a.run(&["team-apply"]);
    assert_eq!((r.code, r.out.as_str()), (0, ""), "{}", r.err);
    let peer = phone(&fx, &a, &b);
    let peer2 = phone(&fx, &a, &c);
    let work = relay::work(&a);
    let dir = work.to_str().expect("UTF-8");
    let extra = work.join("extra.json");
    fsx::write(&extra, "{}");
    let (wid, wdigest) = template(&a, "工作", dir, &[]);
    let (did, ddigest) = template_for(&a, "dave 的", "dave", dir, &[]);
    let (oid, odigest) = template_for(&a, "bob 的", "bob", dir, &[]);
    let (sid, sdigest) = template(&a, "带设置", dir, &["--settings", "extra.json"]);
    let (lid, ldigest) = template(&a, "带话", dir, &["--model", "opus", "说中文"]);
    for p in [&peer.pid, &peer2.pid] {
        a.ok(&["remote-allow", p, "launch", "on"]);
    }
    let mut l = launcher(&a);
    // 说明里没有账号走别的服务器的；参数里已经有一句话的不能带开场白（再接一句：claude 不理它，codex 起不来）
    let (lsid, doc) = wait_for("启动器的说明", secs(30), || peer.launcher().filter(|(_, d)| d["t"].list().len() == 4));
    let rows: Vec<(String, bool)> =
        doc["t"].list().iter().map(|r| (r["n"].str().to_string(), r["l"].as_bool() == Some(true))).collect();
    let want = [("工作", true), ("dave 的", true), ("带设置", true), ("带话", false)];
    assert_eq!(rows, want.map(|(n, l)| (n.to_string(), l)));
    // 两台手机轮流来（同一台两次接进来要隔 3 秒）
    let tries: [(&SpecPeer, &str, &str, Option<&str>, &str); 2] = [
        (&peer, &lid, &ldigest, Some("再说一句"), "bad"),
        (&peer2, &oid, &odigest, None, "other_hub"), // 手机是在这台服务器上配的
    ];
    for (p, id, digest, line, code) in tries {
        assert_eq!(refused(&p.launch(&lsid, &ask(id, digest, line))), (code.to_string(), true));
    }
    // 参数现在过不了启动前的检查（--settings 的那个文件后来写进了 apiKeyHelper）；账号表里没有这个账号了
    fsx::write(&extra, r#"{"apiKeyHelper": "echo test"}"#);
    let mut table = fsx::read_json(&accounts);
    table.as_obj_mut().expect("账号表是对象").remove("dave(dave@example.com)");
    fsx::write_json(&accounts, &table);
    std::thread::sleep(GAP);
    assert_eq!(refused(&peer.launch(&lsid, &ask(&sid, &sdigest, None))), ("bad_args".to_string(), true));
    assert_eq!(refused(&peer2.launch(&lsid, &ask(&did, &ddigest, None))), ("no_account".to_string(), true));
    // 这台机器走这台服务器的会话已经有 7 个（服务器给一台设备 8 个，启动器自己占一个）：不再起
    let names7: Vec<String> = (1..=7).map(|i| format!("s{i}")).collect();
    let busy: Vec<_> = names7.iter().map(|n| relay::session(&a, Device::KEY, n, &[n.as_str()])).collect();
    std::thread::sleep(GAP);
    assert_eq!(refused(&peer.launch(&lsid, &ask(&wid, &wdigest, None))), ("busy".to_string(), true));
    drop(busy);
    let log = log_of(&a);
    for code in ["bad", "other_hub", "bad_args", "no_account", "busy"] {
        assert!(log.contains(&format!("没起（{code}）")), "{code}：{log}");
    }
    // 关掉一台手机的远程启动：它不再收到说明、要不来接入启动器的票据；另一台照旧（启动器接着跑）
    a.ok(&["remote-allow", &peer.pid, "launch", "off"]);
    wait_for("关掉的那台手机收不到说明了", secs(30), || peer.launcher().is_none().then_some(()));
    wait_for("另一台照旧收得到", secs(30), || peer2.launcher().filter(|(s, _)| *s == lsid));
    let (st, j) = fx.call(&b, "relay_attach", &obj! { "sid" => lsid.as_str(), "pid" => peer.pid.as_str() });
    assert_eq!((st, j["error"].str()), (404, "no_session"), "{j}");
    assert!(l.poll().is_none(), "启动器还在跑");
    assert!(!log_of(&a).contains("起了会话"), "一个会话都没起：{}", log_of(&a));
    l.signal(libc::SIGTERM);
    assert_eq!(l.wait(secs(15)), 0);
}

/// a 显示配对码、b（真的辅助程序，另一台电脑）输入：a 的配对表里 b 能看和操作（显示配对码的这一头默认这样）。返回配对编号
fn machine(a: &Device, b: &Device) -> String {
    let mut p = Popen::spawn(&mut relay::helper_cmd(a, &["remote-pair-show"]));
    let line = p.out_line();
    let code = line.split_whitespace().nth(1).expect("配对码").to_lowercase().replace('-', " ");
    let r = b.run_with(&["remote-pair-join"], &format!("{code}\n"), &[]);
    assert_eq!((r.code, r.out.trim_end()), (0, a.name.as_str()), "{}", r.err);
    let (rc, out, said) = p.communicate(secs(60));
    assert_eq!((rc, out), (0, format!("peer {}\n", b.name)), "{said}");
    let show = a.ok(&["remote-allow", "--show", &b.name]);
    show.split('\t').next().expect("配对编号").to_string()
}

/// b 上问一次配对过的机器上的会话和启动器（remote-list --json）；服务器按设备限流（每 2 秒 1 次、最多攒 30 次）：等的时候隔 1 秒问一次
fn far_doc(b: &Device, ready: impl Fn(&Value) -> bool) -> Option<Value> {
    let doc = parsed(&b.ok(&["remote-list", "--json"]));
    if ready(&doc) {
        return Some(doc);
    }
    std::thread::sleep(Duration::from_secs(1));
    None
}

/// 配对过的电脑（cca remote launch；Mac 上的 cca App 用 --json）照这台机器上的模板新建会话：2026-11-27 起不只手机（3.7.6 只给手机）。
/// 列出能用的模板、不在终端里不起、给 App 的 --json（起好了等它登记上，说是哪个会话；窗口大小、开场白照它说的）、终端里起好接进去、
/// 那台机器不起时说原因（另记一条 fail 事件）、关掉以后起不了
fn a_machine_launches_a_session(t: &mut Ctx) {
    let fx = Base::new(t);
    let (a, b) = (fx.device("box"), fx.device("mac"));
    fx.machines(&[&a, &b]);
    let pid = machine(&a, &b);
    let work = relay::work(&a);
    let (id, digest) = template(&a, "修 bug", work.to_str().expect("UTF-8"), &["--model", "opus"]);
    // 不是手机也能打开（要能看和操作：显示配对码的这一头默认就是）
    assert_eq!(a.ok(&["remote-allow", &pid, "launch", "on"]), "mac\n");
    assert_eq!(a.ok(&["remote-allow", "--show", "mac"]), format!("{pid}\tmac\t0\tcontrol\t1\n"));
    let mut l = launcher(&a);

    // mac 上列出来的：box 的启动器（不算会话）和它的模板；账号是 mac 上的名字
    let doc = wait_for("mac 看得到 box 的启动器", secs(30), || {
        far_doc(&b, |d| d["peer_launchers"].list().iter().any(|l| !l["templates"].list().is_empty()))
    });
    let ls = doc["peer_launchers"].list();
    assert_eq!(ls.len(), 1, "{doc}");
    assert_eq!(cols(&ls[0], &["peer", "machine", "state", "more"]), list![pid.as_str(), "box", "ok", 0]);
    assert_eq!(ls[0]["id"].str().len(), 32, "{doc}");
    let row = &ls[0]["templates"].list()[0];
    assert_eq!(
        cols(row, &["id", "digest", "name", "tool", "account", "host_account", "dir", "args", "line"]),
        list![id.as_str(), digest.as_str(), "修 bug", "claude", "alice", "alice", "~/work", "--model opus", true]
    );
    assert!(doc["sessions"].list().is_empty(), "启动器不算会话：{doc}");
    let r = b.run(&["remote-launch"]);
    assert_eq!(
        (r.code, r.out.as_str()),
        (0, format!("box  {id}  修 bug  claude  alice  ~/work  --model opus\n").as_str()),
        "{}",
        r.err
    );

    // 起之前先查的：不在终端里（起了接不进去）、开场白不对、没有这台机器、没有这个模板：什么都不起
    let r = b.run(&["remote-launch", "box", "修 bug"]);
    assert!(r.code == 69 && r.err.contains("要在终端里运行"), "{r:?}");
    let r = b.run(&["remote-launch", "box", "修 bug", "--json", "--line", "a\nb"]);
    assert!(r.code == 64 && r.err.contains("控制字符"), "{r:?}");
    let r = b.run(&["remote-launch", "nobox", "修 bug", "--json"]);
    assert!(r.code == 65 && r.err.contains("没有叫「nobox」的配对"), "{r:?}");
    let r = b.run(&["remote-launch", "没有这个", "--json"]);
    assert!(r.code == 65 && r.err.contains("没有叫「没有这个」的模板"), "{r:?}");
    assert_eq!(log_of(&a), "", "一个都没起");

    // 界面 App 的：--json，窗口大小、开场白、看到的摘要照它说的；起好了等它登记上，stdout 是新会话（remote list --json 里它那一项）
    let events = b.home.join("ev.jsonl");
    let ev = events.to_str().expect("UTF-8");
    let argv = ["remote-launch", "box", &id, "--json", "--size", "200x30", "--line", "你好 world", "--digest", &digest];
    let r = b.run_with(&argv, "", &[("CCA_EVENTS", ev)]);
    assert_eq!(r.code, 0, "{r:?}");
    let got = parsed(&r.out);
    assert_eq!(cols(&got, &["version", "kind"]), list![1, "remote_launch"]);
    let sid = got["id"].str().to_string();
    assert!(sid.len() == 32 && sid.bytes().all(|c| c.is_ascii_hexdigit()), "{got}");
    assert_eq!(
        cols(&got["session"], &["id", "where", "peer", "machine", "tool", "name", "cwd", "state", "access"]),
        list![sid.as_str(), "peer", pid.as_str(), "box", "claude", "alice", "~/work", "ok", "control"]
    );
    assert_eq!(fsx::try_read_text(&events).unwrap_or_default(), "", "成了不记 fail 事件");
    let log = log_of(&a);
    assert!(log.contains("mac") && log.contains("修 bug") && log.contains(&format!("起了会话 {}", &sid[..8])), "{log}");
    assert!(!log.contains("你好"), "{log}");
    // 接进去看：客户端拿到的参数里有开场白，窗口是 App 说的大小
    let mut v = relay::term_sized(&b, &["remote-attach", &sid[..8]], 30, 200);
    let want =
        format!("200 leader=True ctty=True tty=True args=['@{}', '--', '--model', 'opus', '--', '你好 world']>", Device::KEY);
    v.expect(&regex::escape(&want));
    v.send("exit 5\n");
    assert_eq!(v.wait(), 5);

    // 终端里：起好了接进去（大小是这个终端的），会话结束时退出码是它的
    std::thread::sleep(GAP); // 同一台接进启动器要隔 3 秒
    let mut v = relay::term(&b, &["remote-launch", "box", "修 bug"]);
    v.expect("正在「box」上用模板「修 bug」新建会话");
    v.expect("<ready 24 80 ");
    v.send("say from-the-mac\n");
    v.expect("<from-the-mac>");
    v.send("exit 4\n");
    assert_eq!(v.wait(), 4);
    v.expect("会话已经结束（退出码 4）");

    // 那台机器不起（App 看到的模板后来改过了）：退出码 67，说原因；App 的另记一条 fail 事件
    std::thread::sleep(GAP);
    let r = b.run_with(&["remote-launch", "box", &id, "--json", "--digest", "0000000000000000"], "", &[("CCA_EVENTS", ev)]);
    assert!(r.code == 67 && r.err.contains("没起成：模板在那台机器上改过了"), "{r:?}");
    assert_eq!(r.out, "");
    let evs = fsx::read_text(&events);
    let fail = parsed(evs.lines().last().expect("记了一条"));
    assert_eq!(cols(&fail, &["v", "ev", "code"]), list![1, "fail", "error"], "{evs}");
    assert!(fail["message"].str().contains("模板在那台机器上改过了"), "{fail}");
    assert!(log_of(&a).contains("没起（changed）"), "{}", log_of(&a));

    // 关掉：启动器自己退出（没有允许的了），mac 上列不出来、起不了
    assert_eq!(a.ok(&["remote-allow", &pid, "launch", "off"]), "mac\n");
    assert_eq!(l.wait(secs(20)), 0);
    wait_for("mac 上没有 box 的启动器了", secs(30), || far_doc(&b, |d| d["peer_launchers"].list().is_empty()));
    let r = b.run(&["remote-launch", "box", "修 bug", "--json"]);
    assert!(r.code == 65 && r.err.contains("「box」现在没让这台机器远程启动会话"), "{r:?}");
}
