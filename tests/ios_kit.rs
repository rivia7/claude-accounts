//! iOS App 的协议核心（ios/CcaRemoteKit，Swift）：没有 Xcode 也测得了——用命令行工具的 swiftc 编成 macOS 上的程序跑
//! （docs/dev/ios.md「测试」）。
//! - against_the_spec：kit::spec（照 server/README.md 独立写的那一份）出数据，Swift 那边逐项核对（ios/Tests/KitTests）；
//!   Swift 封的配对消息、发的记录、签名再拿回这边核对（签名用 ssh-keygen -Y verify）
//! - pair_list_attach_type、wrong_pin_and_used_code、session_ends：整条路。box 上是真的 cca（辅助程序：在宿主里运行的会话、
//!   remote-pair-show --web），手机那一头是 ios/tools/ccaguest（和 App 用的是同一份 CcaRemoteKit：钉住证书的 HTTPS、TCP 进数据
//!   端口）：扫二维码（链接）配对、列会话、接入、敲键、只看、取消配对
//! - in_the_simulator：模拟器里真的 App（要 Xcode；先跑 `bash ios/test.sh`，再带 CCA_IOS_SIM=1）
//!
//! 没有 swiftc（不是 macOS）时跳过。编出来的程序放在 ios/build/test/（源码没变时用上次的）。夹具（Base、服务器）在 kit::relay 里。
//! 原来是 tests/ios_kit.py：数据由 spec_wire.py 出，这里换成 kit::spec，核对的一样。

use kit::fleet::Device;
use kit::fsx::{self, wait_for};
use kit::harness::{self, Ctx, Module, Need};
use kit::json::{self, Style, Value};
use kit::proc::{Child, Cmd, Stdio};
use kit::relay::{self, has, secs, Base, Popen};
use kit::spec::{self, Channel};
use kit::{case, crypto, list, obj, pki};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SWIFT: &[Need] = &[Need::MacOs, Need::Bin("swiftc")];

pub const MODULE: Module = Module {
    name: "ios_kit",
    src: include_str!("ios_kit.rs"),
    default: true,
    tests: &[
        // 头一次要编（几十秒）
        case!(against_the_spec).needs(SWIFT).timeout(600),
        case!(pair_list_attach_type).needs(SWIFT).timeout(600),
        case!(wrong_pin_and_used_code).needs(SWIFT).timeout(600),
        case!(session_ends).needs(SWIFT).timeout(600),
        case!(in_the_simulator).needs(&[Need::MacOs]).timeout(600),
    ],
};

// ---- 编 Swift 的程序 ----

/// 路径当参数用（临时目录都是 UTF-8 的）
fn text(path: &Path) -> String {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display())).to_string()
}

/// 目录里的 .swift，按名字排
fn swift_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读 {}：{e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "swift"))
        .collect();
    files.sort();
    files
}

/// CcaRemoteKit 的源码加上 main（一个目录）编成一个程序；源码没变时用上次的（ios/build/test/<名字>-<源码的摘要>）。回程序的路径
fn build(name: &str, main: &Path) -> PathBuf {
    let ios = harness::repo().join("ios");
    let kit = ios.join("CcaRemoteKit").join("Sources").join("CcaRemoteKit");
    let srcs: Vec<PathBuf> = swift_files(&kit).into_iter().chain(swift_files(main)).collect();
    let mut all = Vec::new();
    for p in &srcs {
        all.extend_from_slice(p.file_name().and_then(|n| n.to_str()).expect("文件名").as_bytes());
        all.push(0);
        all.extend_from_slice(&fsx::read(p));
    }
    let cache = ios.join("build").join("test");
    let out = cache.join(format!("{name}-{}", &crypto::hex(&crypto::sha256(&all))[..16]));
    if out.exists() {
        return out;
    }
    std::fs::create_dir_all(&cache).unwrap_or_else(|e| panic!("建 {}：{e}", cache.display()));
    for old in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
        if old.file_name().to_str().is_some_and(|n| n.starts_with(&format!("{name}-"))) {
            let _ = std::fs::remove_file(old.path());
        }
    }
    let tmp = cache.join(format!("{name}.tmp{}", std::process::id()));
    let arch = if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x86_64" };
    let mut cmd = Cmd::new("swiftc");
    cmd.args(["-Onone", "-swift-version", "5", "-module-name", &name.replace('-', "_"), "-target"]);
    cmd.arg(format!("{arch}-apple-macos14.0")).arg("-o").arg(&tmp).args(&srcs);
    cmd.env_inherit().timeout(secs(500));
    let r = cmd.output().unwrap_or_else(|e| panic!("起不了 swiftc：{e}"));
    if !r.status.success() {
        let err = r.err();
        panic!("编不过：\n{}", &err[err.len().saturating_sub(4000)..]);
    }
    std::fs::rename(&tmp, &out).unwrap_or_else(|e| panic!("{e}"));
    out
}

// ---- 对照 kit::spec ----

/// 0、7、14……（每个字节是下标乘 7 的低 8 位）
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7) as u8).collect()
}

/// 证书（PEM）换成 DER
fn der_of(cert: &Path) -> Vec<u8> {
    let mut cmd = Cmd::new("openssl");
    cmd.args(["x509", "-in"]).arg(cert).args(["-outform", "der"]).pass_env(&["PATH"]);
    cmd.output().unwrap_or_else(|e| panic!("{e}")).check().stdout
}

/// 网址里的一项：字母、数字和 `-._~` 原样，别的（UTF-8 的每个字节）写成 %XX（Python 的 `quote(s, safe='-._~')`）
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn against_the_spec(t: &mut Ctx) {
    let exe = build("kit-tests", &harness::repo().join("ios").join("Tests").join("KitTests"));
    let d = t.tmp("cca-kit-");
    let hx = |b: &[u8]| crypto::hex(b);
    let code_row = |c: &[u8]| obj! { "hex" => hx(c), "text" => spec::code_text(c), "rid" => spec::pair_keys(c).2 };
    let codes: Vec<Vec<u8>> = vec![crypto::random(10), crypto::random(10), crypto::random(10), vec![0; 10], vec![0xff; 10]];
    let mut v = json::Obj::new();
    let rows: Vec<Value> = codes[..2].iter().chain(&codes[3..]).map(|c| code_row(c)).collect();
    v.insert("codes", Value::List(rows));
    v.insert(
        "bad_codes",
        list![
            "",
            "ABCD-EFGH-JKMN-PQR",
            "ABCD-EFGH-JKMN-PQRST",
            "ABCD-EFGH-JKMN-PQRU",
            "ABCD-EFGH-JKMN-PQR*",
            "ＡBCD-EFGH-JKMN-PQRS"
        ],
    );
    // 配对
    let code = &codes[2];
    let (kc, keys, rid) = spec::pair_keys(code);
    let (na, nb) = (crypto::random(32), crypto::random(32));
    let (fpa, fpb) = (format!("SHA256:{}xyz", "a/b+".repeat(10)), format!("SHA256:{}", "Q".repeat(43)));
    let (p, pid) = spec::pair_secret(&kc, &na, &nb, &fpa, &fpb);
    v.insert(
        "pair",
        obj! {
            "code" => hx(code), "other_code" => crypto::token_hex(10),
            "msg_a" => spec::pair_msg(&keys, &rid, "a", &fpa, &na, "Mac 上的 cca"),
            "fp_a" => &fpa, "n_a" => hx(&na), "name_a" => "Mac 上的 cca", "nb" => hx(&nb), "fp_b" => &fpb, "p" => hx(&p), "pid" => &pid
        },
    );
    // 会话的说明
    let sid = crypto::token_hex(16);
    let cwd = format!("~/srv/{}", "x".repeat(300));
    let doc = obj! {
        "v" => 1, "machine" => "box\u{202e}\u{1b}", "tool" => "claude", "name" => "账号 1", "cwd" => &cwd, "since" => 1763800000.5,
        "access" => "view"
    };
    v.insert(
        "dir",
        obj! {
            "p" => hx(&p), "pid" => &pid, "sid" => &sid, "blob" => spec::dir_seal(&p, &pid, &sid, &doc),
            "forged" => spec::dir_seal(&crypto::random(32), &pid, &sid, &doc),
            "control" => spec::dir_seal(&p, &pid, &sid, &obj! { "v" => 1, "machine" => "m", "since" => true }),
            "want" => obj! {
                "machine" => "box", "tool" => "claude", "name" => "账号 1", "cwd" => &cwd[..200], "since" => 1763800000.5, "view" => true
            }
        },
    );
    // 握手和记录：这边是宿主
    let csid = crypto::token_hex(16);
    let mut viewer = Channel::new("viewer", &p, &pid, &csid, &fpa, &fpb);
    let mut host = Channel::new("host", &p, &pid, &csid, &fpa, &fpb);
    let nv = crypto::random(32);
    let hello = viewer.hello_with(&nv);
    let reply = host.reply(&hello).expect("宿主认 hello");
    let confirm = viewer.confirm(&reply).expect("接入方认宿主的证明");
    assert!(host.accept(&confirm));
    let lens = [0usize, 1, 30, 31, 1000, 16384];
    let records: Vec<Value> = lens.iter().map(|n| Value::from(hx(&host.seal(&pattern(*n))))).collect();
    let tail = hx(&host.seal(b"\x09"));
    v.insert(
        "chan",
        obj! {
            "p" => hx(&p), "pid" => &pid, "sid" => &csid, "fph" => &fpa, "fpv" => &fpb, "nv" => hx(&nv), "hello" => hx(&hello),
            "reply" => hx(&reply), "confirm" => hx(&confirm), "records" => Value::List(records),
            "lens" => Value::List(lens.iter().map(|n| Value::from(*n)).collect()), "tail" => tail
        },
    );
    // 开场白
    let (tid, tkey, cn) = (crypto::token_hex(16), crypto::random(32), crypto::token_hex(8));
    let (line, ok) = spec::relay_line_with("view", &tid, &tkey, &cn);
    let utf = |b: Vec<u8>| String::from_utf8(b).expect("开场白是 ASCII");
    v.insert("relay", obj! { "tid" => &tid, "tkey" => hx(&tkey), "cn" => &cn, "line" => utf(line), "expect" => utf(ok) });
    // 证书：EC（cca 的服务器都是这种）、RSA、Ed25519
    pki::make_cert(&d);
    let mut spki = vec![
        obj! { "what" => "EC P-256", "der" => hx(&der_of(&d.join("cert.pem"))), "pin" => pki::openssl_pin(d.join("cert.pem")) },
    ];
    for (what, newkey) in [("RSA", "rsa:2048"), ("Ed25519", "ed25519")] {
        let c = d.join(format!("{what}.pem"));
        let mut cmd = Cmd::new("openssl");
        cmd.args(["req", "-x509", "-newkey", newkey, "-nodes", "-keyout"]).arg(d.join("k.pem"));
        cmd.args(["-subj", "/CN=x", "-days", "2", "-out"]).arg(&c).pass_env(&["PATH"]).stdout(Stdio::Null).stderr(Stdio::Null);
        cmd.output().unwrap_or_else(|e| panic!("{e}")).check();
        spki.push(obj! { "what" => what, "der" => hx(&der_of(&c)), "pin" => pki::openssl_pin(&c) });
    }
    v.insert("spki", Value::List(spki));
    // 二维码里的链接（cli/src/cmd/remote/pair.rs 的 app_link 出的那种）
    let pin = pki::openssl_pin(d.join("cert.pem"));
    let ctext = spec::code_text(code);
    let bare = ctext.replace('-', "");
    let url = "https://192.0.2.10:18443";
    let base = format!("cca-remote://pair?v=1&u={}&p={}&c={bare}&n={}", enc(url), enc(&pin), enc("我的 Mac"));
    let link = |link: String, pins: Value, team: Value| {
        obj! { "link" => link, "url" => url, "pins" => pins, "code" => &ctext, "name" => "我的 Mac", "team" => team }
    };
    v.insert(
        "links",
        list![
            link(base.clone(), list![&pin], Value::Null),
            link(format!("{base}&t={}", enc("Team A")), list![&pin], Value::from("Team A")),
            link(format!(" {base}&p={}\n", enc(&pin)), list![&pin, &pin], Value::Null),
            link(base.replace(&format!("&p={}", enc(&pin)), ""), list![], Value::Null)
        ],
    );
    let many_pins: String = (0..4).map(|_| format!("&p={}", enc(&pin))).collect();
    v.insert(
        "bad_links",
        list![
            base.replace("v=1", "v=2"),
            base.replace("cca-remote://pair", "cca-remote://join"),
            base.replace("cca-remote", "https"),
            base.replace(&enc("https://"), &enc("http://")),
            base.replace(&bare, &"U".repeat(16)),
            format!("{base}&c={ctext}"),
            base.replace(&enc(&pin), &enc("sha256//abc=")),
            format!("{base}{many_pins}"),
            base.replace(&enc(url), &enc("https://u@192.0.2.10/")),
            "https://example.com/",
            ""
        ],
    );
    let data = b"{\"op\":\"relay_list\",\"pids\":[]}";
    v.insert("sign", obj! { "raw" => crypto::token_hex(32), "data" => hx(data) });
    fsx::write(d.join("in.json"), json::dumps(&Value::Obj(v), &Style::default()));

    let mut cmd = Cmd::new(&exe);
    cmd.arg(d.join("in.json")).arg(d.join("out.json")).timeout(secs(120));
    let r = cmd.output().unwrap_or_else(|e| panic!("{e}"));
    let said = format!("{}{}", r.out(), r.err());
    assert!(r.status.success(), "{said}");
    assert!(relay::matches(r"通过 \d+，失败 0", &r.out()), "{said}");
    let out = fsx::read_json(d.join("out.json"));
    // 这一份封的配对消息：kit::spec 解得开、内容对
    let theirs = spec::pair_open(&keys, &rid, "b", &out["msg_b"]).expect("Swift 封的配对消息认证得过");
    assert_eq!(relay::cols(&theirs, &["v", "fp", "n", "name"]), list![1, &fpb, hx(&nb), "我的 iPhone"]);
    let c = crypto::b64_decode(out["msg_b"]["c"].str()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.len() % 256, 0, "补齐到 256 的整数倍");
    // 这一份发的记录：宿主收得到
    let mut buf: Vec<u8> = out["records"].list().iter().flat_map(|x| crypto::unhex(x.str()).expect("十六进制")).collect();
    let got: Vec<Vec<u8>> = lens.iter().map(|_| host.open(&mut buf).expect("记录认证得过").expect("够一条")).collect();
    assert_eq!(got, lens.iter().map(|n| pattern(*n)).collect::<Vec<_>>());
    assert!(buf.is_empty(), "多出来 {} 个字节", buf.len());
    // 签名：ssh-keygen 认
    let s = &out["sign"];
    assert!(s["pub"].str().starts_with("ssh-ed25519 "), "{s}");
    fsx::write(d.join("pub"), format!("{}\n", s["pub"].str()));
    assert_eq!(s["fp"].str(), spec::fp_of(d.join("pub")));
    assert!(spec::ssh_verify(s["pub"].str(), "cca-presence", data, s["sig"].str().as_bytes()), "ssh-keygen 不认 Swift 的签名：{s}");
}

// ---- 手机那一头：ios/tools/ccaguest ----

/// 手机那一头：ios/tools/ccaguest，配对表、密钥都在自己的目录里
struct Guest {
    exe: PathBuf,
    dir: PathBuf,
    name: String,
}

/// ccaguest 跑完的结果
struct Done {
    code: i32,
    out: String,
    err: String,
}

impl Guest {
    fn new(fx: &Base, name: &str) -> Guest {
        let exe = build("ccaguest", &harness::repo().join("ios").join("tools").join("ccaguest"));
        let dir = fx.d.join(format!("guest-{}", crypto::token_hex(3)));
        std::fs::create_dir(&dir).unwrap_or_else(|e| panic!("{e}"));
        Guest { exe, dir, name: name.to_string() }
    }

    fn cmd(&self, args: &[&str]) -> Cmd {
        let mut cmd = Cmd::new(&self.exe);
        cmd.arg("--store").arg(&self.dir).args(["--name", self.name.as_str()]).args(args);
        cmd
    }

    /// 跑一条命令（标准输入是 input），等它跑完（最多 90 秒）
    fn run_with(&self, args: &[&str], input: &str) -> Done {
        let r = self.cmd(args).input(input).timeout(secs(90)).output().unwrap_or_else(|e| panic!("{e}"));
        Done { code: r.status.py(), out: r.out(), err: r.err() }
    }

    fn run(&self, args: &[&str]) -> Done {
        self.run_with(args, "")
    }

    /// 该成的：退出码不是 0 就 panic；给出标准输出
    #[track_caller]
    fn ok(&self, args: &[&str]) -> String {
        let r = self.run(args);
        assert_eq!(r.code, 0, "{} {}", r.out, r.err);
        r.out
    }

    /// 列出来的（一行一个 JSON）
    fn rows(&self) -> Vec<Value> {
        self.ok(&["list"]).lines().map(relay::parsed).collect()
    }

    /// 接入（不等）：敲的键写到它的标准输入，终端的字节、它说的状态（state open……）一直有线在收
    fn attach(&self, pid: &str, sid: &str, flags: &[&str]) -> Attached {
        let mut cmd = self.cmd(&[&["attach", pid, sid], flags].concat());
        cmd.stdin(Stdio::Piped).stdout(Stdio::Piped).stderr(Stdio::Piped);
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{e}"));
        let (out, err) = (collect(child.stdout.take().expect("管道")), collect(child.stderr.take().expect("管道")));
        let stdin = child.stdin.take();
        Attached { child, stdin, out, err }
    }
}

/// 另起一条线一直读，读到的攒起来
fn collect(mut from: File) -> Arc<Mutex<Vec<u8>>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&buf[..n]);
        }
    });
    got
}

/// 接着会话的 ccaguest
struct Attached {
    child: Child,
    stdin: Option<File>,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
}

impl Attached {
    fn out(&self) -> Vec<u8> {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn err(&self) -> Vec<u8> {
        self.err.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 敲键
    fn type_in(&mut self, keys: &[u8]) {
        let stdin = self.stdin.as_mut().expect("标准输入还开着");
        stdin.write_all(keys).and_then(|_| stdin.flush()).unwrap_or_else(|e| panic!("{e}"));
    }

    /// 等它说 state open
    fn wait_open(&self) {
        wait_for("接上了", secs(30), || has(&self.err(), "state open"));
    }

    /// 关上标准输入（读到头：断开），等它退出：退出码
    fn finish(&mut self) -> i32 {
        drop(self.stdin.take());
        self.wait()
    }

    fn wait(&mut self) -> i32 {
        match self.child.wait_timeout(secs(30)).unwrap_or_else(|e| panic!("{e}")) {
            Some(exit) => exit.py(),
            None => panic!("ccaguest 30 秒还没退出：{}", String::from_utf8_lossy(&self.err())),
        }
    }
}

/// 开着中转的服务器，box 登记在上面
fn guest_fx(t: &mut Ctx) -> (Base, Device) {
    let fx = Base::new(t);
    let a = fx.device("box");
    fx.machines(&[&a]);
    (fx, a)
}

/// box 上 remote-pair-show --web（不等它配完）；给出进程和 link 那一行里的链接
fn show(a: &Device) -> (Popen, String) {
    let mut p = Popen::spawn(&mut relay::helper_cmd(a, &["remote-pair-show", "--web"]));
    let first = p.out_line();
    assert!(first.starts_with("code "), "{first:?}");
    let line = p.out_line();
    let link = line.strip_prefix("link cca-remote://").map(|_| line[5..].trim().to_string());
    (p, link.unwrap_or_else(|| panic!("配对码后面该是给 App 的链接：{line:?}")))
}

/// box 上 remote-pair-show --web，手机拿 link 那一行配对。回 (手机说的那一行, box 说的对方的名字)
fn pair(a: &Device, g: &Guest) -> (String, String) {
    let (mut p, link) = show(a);
    let out = g.ok(&["pair", &link]);
    let (rc, said, err) = p.communicate(secs(60));
    assert_eq!((rc, err.as_str()), (0, ""));
    (out, said.trim().trim_start_matches("peer ").to_string())
}

/// 服务器记着的配对和访客（d/remote.json）
fn remote(d: &Path) -> Value {
    fsx::read_json(d.join("remote.json"))
}

/// 手机列出来的会话（有 sid 的那几行）：等到正好一个
fn one_session(g: &Guest) -> Value {
    wait_for("手机看到会话", secs(30), || {
        let rows: Vec<Value> = g.rows().into_iter().filter(|r| r.get("sid").is_some()).collect();
        (rows.len() == 1).then(|| rows[0].clone())
    })
}

fn pair_list_attach_type(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    assert_eq!(g.ok(&["list"]), "", "没配对过：什么都没有");
    let (out, said) = pair(&a, &g);
    assert_eq!(said, "我的 iPhone");
    let words: Vec<&str> = out.splitn(3, ' ').collect();
    assert_eq!((words.len(), words[0], words.get(2).map(|n| n.trim())), (3, "peer", Some("box")), "{out:?}");
    let pid = words[1].to_string();
    let row = relay::peers(&a)[pid.as_str()].clone();
    assert_eq!(relay::cols(&row, &["incoming", "web"]), list!["control", true]);
    let peers = a.ok(&["remote-peers"]);
    assert!(peers.contains("我的 iPhone（手机）"), "{peers}");
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    let r = one_session(&g);
    assert_eq!(relay::cols(&r, &["pid", "tool", "online", "view"]), list![&pid, "claude", true, false], "{r}");
    assert!(!r["machine"].str().is_empty(), "{r}");
    let sid = r["sid"].str().to_string();
    let mut p = g.attach(&pid, &sid, &[]);
    p.wait_open();
    p.type_in(b"say hello-from-the-phone\r");
    h.expect("<hello-from-the-phone>"); // 会话程序收到了（宿主那边的终端里看得到它的回答）
    wait_for("手机收到会话的输出", secs(20), || has(&p.out(), "<hello-from-the-phone>"));
    assert_eq!(p.finish(), 0, "{}", String::from_utf8_lossy(&p.err())); // 读到头：断开（会话接着运行）
    assert!(has(&p.err(), "closed detached"), "{}", String::from_utf8_lossy(&p.err()));
    h.send(b"say still-here\r");
    h.expect("<still-here>");
    // 只看：敲的不送
    let mut q = g.attach(&pid, &sid, &["--view"]);
    q.wait_open();
    q.type_in(b"say not-sent\r");
    h.send(b"say from-the-mac\r");
    wait_for("只看的也收到输出", secs(20), || has(&q.out(), "<from-the-mac>"));
    assert_eq!(q.finish(), 0);
    assert!(!has(&q.out(), "<not-sent>"), "只看：敲的不送");
    // 取消配对：服务器上的记录没了
    assert_eq!(g.ok(&["unpair", &pid]).trim(), "unpaired");
    assert_eq!(g.ok(&["list"]), "");
    let doc = remote(&fx.d);
    assert_eq!((&doc["edges"], &doc["guests"]), (&obj! {}, &obj! {}));
}

fn wrong_pin_and_used_code(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    let (mut p, link) = show(&a);
    // 证书和二维码里的指纹对不上（服务器被换掉了、路上有人）：一个请求都不发成
    let other = format!("sha256//{}", crypto::b64(&crypto::random(32)));
    let bad = link.replace(&enc(&fx.pin), &enc(&other));
    assert_ne!(bad, link);
    let r = g.run(&["pair", &bad]);
    assert_eq!((r.code, r.out.trim()), (1, "fail pin"), "{}", r.err);
    assert_eq!(g.run(&["pair", "cca-remote://pair?v=1"]).out.trim(), "fail link");
    g.ok(&["pair", &link]);
    assert_eq!(p.wait(secs(60)), 0);
    // 同一个配对码只能用一次
    let r = Guest::new(&fx, "iPad").run(&["pair", &link]);
    assert_eq!((r.code, r.out.trim()), (1, "fail no_pairing"), "{}", r.err);
}

fn session_ends(t: &mut Ctx) {
    let (fx, a) = guest_fx(t);
    let g = Guest::new(&fx, "我的 iPhone");
    let (out, _) = pair(&a, &g);
    let pid = out.split_whitespace().nth(1).expect("peer 后面是配对编号").to_string();
    let _h = relay::session(&a, Device::KEY, "alice", &[]);
    let r = one_session(&g);
    let sid = r["sid"].str().to_string();
    let mut p = g.attach(&pid, &sid, &[]);
    p.wait_open();
    p.type_in(b"exit 3\r"); // 会话程序退出（退出码 3）
    let rc = p.wait();
    assert!(has(&p.err(), "exit 3\nclosed ended"), "{}", String::from_utf8_lossy(&p.err()));
    assert_eq!(rc, 0);
    // 会话没了：再接是 gone
    let r2 = g.run_with(&["attach", &pid, &sid], "");
    assert!(r2.err.contains("closed gone"), "{}", r2.err);
}

// ---- 模拟器里真的 App ----

const BUNDLE: &str = "com.cca.remote";

/// ios/test.sh 建的那台模拟器（开着的）：有就回它的 UDID
fn simulator() -> Option<String> {
    let f = harness::repo().join("ios").join("build").join("simulator");
    let udid = fsx::try_read_text(&f)?.trim().to_string();
    let mut cmd = Cmd::new("xcrun");
    cmd.args(["simctl", "list", "devices", "booted", "-j"]).env_inherit().stderr(Stdio::Null);
    let r = cmd.output().ok()?;
    let doc = json::parse(&r.out()).ok()?;
    let booted =
        doc.get("devices")?.obj().values().flat_map(|ds| ds.list().iter().map(|d| d["udid"].str().to_string())).collect::<Vec<_>>();
    booted.contains(&udid).then_some(udid)
}

/// xcrun simctl 的一条命令（不管成不成）
fn simctl(args: &[&str]) -> bool {
    let mut cmd = Cmd::new("xcrun");
    cmd.arg("simctl").args(args).env_inherit().stdout(Stdio::Null).stderr(Stdio::Null).timeout(secs(120));
    cmd.output().is_ok_and(|r| r.status.success())
}

/// 模拟器里真的 App（调试版，ios/test.sh 编、装的那一份）：扫 cca remote pair --web 给的链接配对（启动参数代替点界面，
/// 见 ios/App/TestDriver.swift）、列出 box 上的会话、接入、敲键，box 上的会话收到；最后截一张接着真会话的终端
fn in_the_simulator(t: &mut Ctx) {
    if std::env::var("CCA_IOS_SIM").ok().as_deref() != Some("1") {
        t.skip("模拟器里的端到端要先跑 bash ios/test.sh（开好模拟器、装上 App），再带 CCA_IOS_SIM=1");
    }
    let Some(udid) = simulator() else { t.skip("ios/test.sh 建的模拟器没开着") };
    let (fx, a) = guest_fx(t);
    let ios = harness::repo().join("ios").join("build");
    let app = ios.join("dd").join("Build").join("Products").join("Debug-iphonesimulator").join("cca.app");
    // 每次重新装：上一次的配对表、钥匙串都不留（App 第一次打开时清钥匙串）
    simctl(&["terminate", &udid, BUNDLE]);
    simctl(&["uninstall", &udid, BUNDLE]);
    assert!(simctl(&["install", &udid, &text(&app)]), "装不上 {}", app.display());
    let stop = udid.clone();
    t.defer(move || {
        simctl(&["terminate", &stop, BUNDLE]);
    });
    let mut h = relay::session(&a, Device::KEY, "alice", &[]);
    let (mut p, link) = show(&a);
    let mut cmd = Cmd::new("xcrun");
    cmd.args(["simctl", "launch", "--console-pty", "--terminate-running-process", &udid, BUNDLE]);
    cmd.args(["-test-pair", &link, "-test-attach", "-test-type", "say from-the-iphone"]);
    cmd.env_inherit().stdin(Stdio::Null).stdout(Stdio::Piped).stderr(Stdio::Null);
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("{e}"));
    let lines = app_lines(child.stdout.take().expect("管道"));
    let mut seen = Vec::new();
    let mut said = |word: &str| {
        let end = Instant::now() + secs(90);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match lines.recv_timeout(left) {
                Ok(line) => {
                    seen.push(line.clone());
                    if line.starts_with(word) {
                        return;
                    }
                    if format!(" {line}").contains(" fail") || line.starts_with("state closed") {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        panic!("App 没说「{word}」：{seen:?}");
    };
    said("pair ok");
    assert_eq!(p.wait(secs(60)), 0);
    let peers = relay::peers(&a);
    let rows: Vec<Value> = peers.obj().values().map(|r| relay::cols(r, &["name", "web"])).collect();
    assert_eq!(rows, [list!["我的 iPhone", true]]);
    said("state open");
    said("typed");
    h.expect("<from-the-iphone>"); // box 上的会话收到了手机上敲的
    std::thread::sleep(Duration::from_millis(1500));
    let shots = ios.join("shots");
    let _ = std::fs::create_dir_all(&shots);
    simctl(&["io", &udid, "screenshot", &text(&shots.join("e2e-terminal.png"))]);
    let _ = child.stop();
    drop(fx);
}

/// App 在控制台上说的、带 `cca-test: ` 的那几行（去掉前缀），一行一条送过来
fn app_lines(mut from: File) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut acc = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
            while let Some(nl) = acc.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&acc[..nl]).into_owned();
                acc.drain(..=nl);
                if let Some((_, said)) = line.split_once("cca-test: ") {
                    if tx.send(said.trim().to_string()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}
