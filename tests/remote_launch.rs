//! 远程启动（docs/dev/launch.md）：配对过的手机照这台机器上的模板新建会话、接进去。
//!
//! 真的服务器（一个 Team 的同步服务器，开着中转）、真的辅助程序：这台机器（box）上跑 `remote-launcher serve`（LaunchAgent、
//! crontab 起的就是它），手机那一头是 kit::spec 照文档独立实现的那一份（SpecPeer）。手机配对本身（网页端、App 的访客）在
//! web_relay.rs、ios_kit.rs 里测：这里的“手机”是一台照文档说话的设备，box 的配对表里那一行标成手机（web）——启动器只照配对表认手机。
//! 启动器起的会话里的“cca”是假客户端（启动器的 CCA_SELF 指着它）：它一启动报一行 `<ready 行 列 … args=[…]>`，看得到起它的参数；
//! 环境、当前目录让它 `run` 一下 env、pwd 看。

use kit::fleet::Device;
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
    tests: &[case!(a_phone_launches_a_session), case!(what_the_launcher_refuses)],
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
    let mut argv = vec!["remote-template", "add", name, "claude", "alice", dir, "--"];
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
    // 打开：只有手机、只有能操作的
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
    for code in ["changed", "no_template", "bad", "no_dir"] {
        assert!(log.contains(&format!("没起（{code}）")), "{code}：{log}");
    }
    // 一个会话都没起
    assert!(peer.sessions().is_empty());
    assert_eq!(a.ok(&["remote-list", "--local"]), "");
    l.signal(libc::SIGTERM);
    assert_eq!(l.wait(secs(15)), 0);
}
