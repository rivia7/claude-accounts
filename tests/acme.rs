//! 证书助手（server/cca-acme.py、`cca-core team-acme`：给网页端申请、续上浏览器信得过的证书）对着假的证书机构（kit::acmefake：
//! 照 ACME 的规范逐项核对请求，http-01 是真的来取文件）。看：给 IP 地址要的是短期的那一种、nonce 不对时自己再发、证书和私钥放对地方
//! 而且权限对、没到时候不重复要、到时候续上而私钥不换、各种没成的情况说得清楚而且不留下半截的东西；几个 Team 共用的那种（--teams、
//! --store）：同一个地址只要一张、登记的地址不对的跳过、各个 Team 的那一份权限对、手里已经有的（以前它自己的证书助手要的）接着用、
//! 运行着时登记了新的马上去要。不联网（都在 127.0.0.1 上）。
//!
//! 原来是 tests/acme.py。证书助手用哪一份跑照 CCA_TEST_SERVER（kit::server::acme_argv：平常是 server/cca-acme.py，rust 是辅助程序的
//! team-acme）；假的证书机构、“别的网站”在这个进程里跑（kit::acmefake、kit::httpd::files）。

use kit::acmefake::{self, FakeCa, Knobs};
use kit::case;
use kit::fsx::{self, wait_for};
use kit::harness::{Ctx, Module};
use kit::httpd;
use kit::json::Value;
use kit::proc::{Child, Cmd, Output, Stdio};
use kit::server::acme_argv;
use kit::{list, obj};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODULE: Module = Module {
    name: "acme",
    src: include_str!("acme.rs"),
    default: true,
    tests: &[
        case!(an_ip_certificate_and_its_renewal),
        case!(a_domain),
        case!(what_goes_wrong),
        case!(no_short_lived_profile),
        case!(webroot),
        case!(several_teams),
        case!(several_teams_running),
        case!(the_responder_and_running_forever),
    ],
};

/// 等的时限（原来 wait_for 默认的 60 秒）：机器再忙也该等得到
const WAIT: Duration = Duration::from_secs(60);
/// 跑一次证书助手最多等多久（原来 subprocess.run 的 timeout）
const RUN_LIMIT: Duration = Duration::from_secs(180);
/// 平常要签的地址（原来 acme() 的 ids 默认值）
const IP: &[&str] = &["127.0.0.1"];

/// 每个用例的准备：临时目录，里面 ca/ 是测试的证书机构和假证书机构自己的 HTTPS 证书；证书助手应答的端口
struct Fx {
    d: PathBuf,
    cad: PathBuf,
    http_port: u16,
}

fn setup(t: &mut Ctx) -> Fx {
    let d = t.tmp("cca-acme-");
    let cad = d.join("ca");
    // 测试的证书机构（带着“能签证书”的用途：新的 Python 默认连这个也查），和假证书机构自己的 HTTPS 证书
    acmefake::make_pki(&cad).unwrap_or_else(|e| panic!("测试的证书做不出来：{e}"));
    fsx::write(d.join("tmp/.keep"), "");
    Fx { d, cad, http_port: fsx::free_port() }
}

/// 路径当参数用（临时目录都是 UTF-8 的）
fn p(path: &Path) -> &str {
    path.to_str().unwrap_or_else(|| panic!("路径不是 UTF-8：{}", path.display()))
}

/// openssl 的一条命令（原来的 run：退出码不是 0 就失败），给出标准输出
fn openssl(args: &[&str]) -> Vec<u8> {
    Cmd::new("openssl")
        .args(args)
        .pass_env(&["PATH", "OPENSSL_CONF"])
        .output()
        .unwrap_or_else(|e| panic!("openssl 运行不了：{e}"))
        .check()
        .stdout
}

/// haystack 里有 needle（assertIn）
fn assert_in(needle: &str, haystack: &str) {
    assert!(haystack.contains(needle), "没有 {needle:?}：\n{haystack}");
}

/// 记录里带着这一项的（`[e for e in log if key in e]`）
fn with_key<'a>(log: &'a [Value], key: &str) -> Vec<&'a Value> {
    log.iter().filter(|e| e.get(key).is_some()).collect()
}

/// 这几条记录里这一项的值（`[e[key] for e in …]`）
fn values(entries: &[&Value], key: &str) -> Vec<Value> {
    entries.iter().map(|e| e[key].clone()).collect()
}

/// 目录里的文件名，排好（`sorted(p.name for p in d.iterdir())`）
fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读不了 {}：{e}", dir.display()))
        .map(|e| e.expect("目录项").file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// 证书里的序列号（`text.split('serial=')[1].split()[0]`）
fn serial_of(text: &str) -> String {
    let after = text.split("serial=").nth(1).unwrap_or_else(|| panic!("没有 serial=：{text}"));
    after.split_whitespace().next().unwrap_or_default().to_string()
}

/// 回复里的一行（去掉行尾的 \r\n）
fn read_line(rd: &mut BufReader<TcpStream>) -> String {
    let mut buf = Vec::new();
    rd.read_until(b'\n', &mut buf).expect("读回复");
    String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string()
}

/// GET 一个路径（原来的 http.client：Host 是 127.0.0.1:端口，时限 10 秒），给出状态码和全部内容
fn http_get(port: u16, path: &str) -> (u16, Vec<u8>) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).unwrap_or_else(|e| panic!("连不上 {addr}：{e}"));
    s.set_read_timeout(Some(Duration::from_secs(10))).expect("设时限");
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept-Encoding: identity\r\n\r\n").expect("发请求");
    let mut rd = BufReader::new(s);
    let status_line = read_line(&mut rd);
    let status = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok());
    let status: u16 = status.unwrap_or_else(|| panic!("状态行看不懂：{status_line:?}"));
    let (mut length, mut chunked) = (None, false);
    loop {
        let h = read_line(&mut rd);
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        if k.trim().eq_ignore_ascii_case("content-length") {
            length = v.trim().parse::<usize>().ok();
        } else if k.trim().eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let size =
                usize::from_str_radix(read_line(&mut rd).split(';').next().unwrap_or("").trim(), 16).expect("chunked 的长度");
            if size == 0 {
                while !read_line(&mut rd).is_empty() {}
                break;
            }
            let mut part = vec![0u8; size];
            rd.read_exact(&mut part).expect("chunked 的一段");
            body.extend(part);
            read_line(&mut rd);
        }
    } else if let Some(n) = length {
        body.resize(n, 0);
        rd.read_exact(&mut body).expect("读内容");
    } else {
        rd.read_to_end(&mut body).expect("读内容");
    }
    (status, body)
}

impl Fx {
    /// 假证书机构的开关：照这个用例的应答端口取 http-01 的文件，别的默认
    fn knobs(&self) -> Knobs {
        Knobs::new(self.http_port)
    }

    /// 起假的证书机构（原来的 authority()）；丢掉就停
    fn authority(&self, knobs: Knobs) -> FakeCa {
        let _ = std::fs::remove_file(self.cad.join("port"));
        FakeCa::start(&self.cad, knobs).unwrap_or_else(|e| panic!("假的证书机构起不来：{e}"))
    }

    /// 起证书助手的命令：环境只带要的（PATH 找 openssl，语言设置，HOME、TMPDIR 指到这个用例的临时目录）
    fn cmd(&self, argv: &[String]) -> Cmd {
        let mut cmd = Cmd::new(&argv[0]);
        cmd.args(&argv[1..])
            .pass_env(&["PATH", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME"])
            .env("HOME", &self.d)
            .env("TMPDIR", self.d.join("tmp"));
        cmd
    }

    /// 跑完一次证书助手（--once），收回输出
    fn run(&self, argv: &[String]) -> Output {
        self.cmd(argv).timeout(RUN_LIMIT).output().unwrap_or_else(|e| panic!("{} 运行不了：{e}", argv.join(" ")))
    }

    /// 起一个一直运行的证书助手：标准错误写进临时目录里的 err（等不到东西时拿它说明）
    fn spawn(&self, argv: &[String], err: &str) -> (Child, PathBuf) {
        let path = self.d.join(err);
        let file = std::fs::File::create(&path).unwrap_or_else(|e| panic!("建不了 {}：{e}", path.display()));
        let child =
            self.cmd(argv).stdout(Stdio::Null).stderr(file).spawn().unwrap_or_else(|e| panic!("{} 起不来：{e}", argv.join(" ")));
        (child, path)
    }

    /// 等一个文件出现（原来的 wait_for）；证书助手先退出了就别干等，带上它说的话失败
    fn wait_file(&self, what: &str, path: &Path, limit: Duration, child: &mut Child, err: &Path) {
        wait_for(what, limit, || {
            if path.exists() {
                return true;
            }
            if let Ok(Some(exit)) = child.try_wait() {
                panic!("等{what}的时候证书助手退出了（{exit}）：{}", fsx::try_read_text(err).unwrap_or_default());
            }
            false
        });
    }

    /// 一张证书的证书助手（--id）：ids 是要签的地址，any_ip 时带 --any-ip；都带 --once（原来的 acme()）
    fn acme(&self, directory: &str, args: &[&str], ids: &[&str], any_ip: bool) -> Output {
        let mut argv = acme_argv();
        argv.extend(
            [
                "--out",
                p(&self.d.join("cert")),
                "--account",
                p(&self.d.join("account")),
                "--listen",
                &format!("127.0.0.1:{}", self.http_port),
                "--directory",
                directory,
                "--ca",
                p(&self.cad.join("ca.pem")),
            ]
            .map(String::from),
        );
        argv.extend(args.iter().map(|a| a.to_string()));
        for i in ids {
            argv.extend(["--id".to_string(), i.to_string()]);
        }
        if any_ip {
            argv.push("--any-ip".to_string());
        }
        argv.push("--once".to_string());
        self.run(&argv)
    }

    /// 几个 Team 共用的证书助手（--teams）：登记在 teams/，证书存在 store/，放进 out/<服务名>/
    fn teams_argv(&self, directory: &str, once: bool) -> Vec<String> {
        let mut argv = acme_argv();
        argv.extend(
            [
                "--teams",
                p(&self.d.join("teams")),
                "--store",
                p(&self.d.join("store")),
                "--out",
                p(&self.d.join("out")),
                "--account",
                p(&self.d.join("account")),
                "--listen",
                &format!("127.0.0.1:{}", self.http_port),
                "--directory",
                directory,
                "--ca",
                p(&self.cad.join("ca.pem")),
                "--any-ip",
            ]
            .map(String::from),
        );
        if once {
            argv.push("--once".to_string());
        }
        argv
    }

    /// 跑完一次几个 Team 共用的（--once）
    fn teams(&self, directory: &str) -> Output {
        self.run(&self.teams_argv(directory, true))
    }

    /// 登记一个 Team（安装脚本做的）：teams/<服务名> 里写它的地址；outdir 时建好它的 out/<服务名>/
    fn register(&self, team: &str, address: &str, outdir: bool) {
        fsx::write(self.d.join("teams").join(team), format!("{address}\n"));
        if outdir {
            std::fs::create_dir_all(self.d.join("out").join(team)).expect("建 Team 的目录");
        }
    }

    /// 假证书机构记下的（原来的 log()）
    fn log(&self) -> Vec<Value> {
        acmefake::read_log(&self.cad)
    }

    /// cert/fullchain.pem 里头一张证书的主题、地址、序列号、公钥（openssl x509 的输出）
    fn cert(&self) -> String {
        let out = openssl(&[
            "x509",
            "-in",
            p(&self.d.join("cert/fullchain.pem")),
            "-noout",
            "-subject",
            "-ext",
            "subjectAltName",
            "-serial",
            "-pubkey",
        ]);
        String::from_utf8(out).expect("openssl 输出的不是 UTF-8")
    }

    /// 临时目录里一个文件的权限位
    fn mode(&self, rel: &str) -> u32 {
        fsx::mode(self.d.join(rel))
    }
}

fn an_ip_certificate_and_its_renewal(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(Knobs { bad_nonce: 3, ..fx.knobs() });
    let directory = ca.directory();
    let r = fx.acme(&directory, &[], IP, true);
    assert_eq!((r.status.py(), r.out()), (0, String::new()), "{}", r.err());
    assert_in("拿到 127.0.0.1 的证书", &r.err());
    let log = fx.log();
    let orders = with_key(&log, "order");
    assert_eq!(orders.len(), 1, "{orders:?}");
    let want_ids = list![obj! { "type" => "ip", "value" => "127.0.0.1" }];
    assert_eq!((&orders[0]["ids"], &orders[0]["profile"]), (&want_ids, &Value::from("shortlived")), "给 IP 签的要的是短期的那一种");
    let validated = with_key(&log, "validated");
    let pairs: Vec<(Value, Value)> = validated.iter().map(|e| (e["validated"].clone(), e["ok"].clone())).collect();
    assert_eq!(pairs, vec![(Value::from("127.0.0.1"), Value::from(true))]);
    assert!(log.iter().any(|e| e.get("refused") == Some(&Value::from("badNonce"))), "说 nonce 不对的那几次，它自己再发了");
    let other: Vec<&Value> = log.iter().filter(|e| e.get("refused").is_some_and(|k| *k != Value::from("badNonce"))).collect();
    assert_eq!(other, Vec::<&Value>::new());
    // 证书：给 127.0.0.1 的、没有 Common Name、连着签它的那一张；照测试的机构验得过；私钥和它是一对
    let text = fx.cert();
    assert_in("IP Address:127.0.0.1", &text);
    let first = text.split('\n').next().unwrap_or_default();
    assert!(!first.contains("CN"), "{first}");
    let chain = fsx::read_text(fx.d.join("cert/fullchain.pem"));
    assert_eq!(chain.matches("BEGIN CERTIFICATE").count(), 2);
    let end = chain.find("-----END CERTIFICATE-----").expect("有一张证书") + 26;
    fsx::write(fx.d.join("leaf.pem"), &chain[..end.min(chain.len())]);
    openssl(&["verify", "-CAfile", p(&fx.cad.join("ca.pem")), p(&fx.d.join("leaf.pem"))]);
    let public = || openssl(&["pkey", "-in", p(&fx.d.join("cert/privkey.pem")), "-pubout"]);
    assert_in(String::from_utf8(public()).expect("公钥是文字").trim(), &text);
    // 权限：两个目录只有自己进得去，两把私钥只有自己读得了，证书别人读得了
    assert_eq!(
        [
            fx.mode("cert"),
            fx.mode("account"),
            fx.mode("cert/privkey.pem"),
            fx.mode("account/account.key"),
            fx.mode("cert/fullchain.pem")
        ],
        [0o700, 0o700, 0o600, 0o600, 0o644]
    );
    assert_eq!(names(&fx.d.join("cert")), ["fullchain.pem", "privkey.pem"], "没有留下半截的文件");
    // 还没到续的时候：再运行一次什么都不向证书机构要
    let (before, serial, key) = (fx.log().len(), serial_of(&text), public());
    let r = fx.acme(&directory, &[], IP, true);
    assert_eq!((r.status.py(), r.err()), (0, String::new()));
    assert_eq!(fx.log().len(), before);
    // 到时候了（这里说“一过有效期的开头就续”）：要一张新的，用的还是同一个账户、同一把证书私钥
    let r = fx.acme(&directory, &["--renew-at", "0"], IP, true);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    let log = fx.log();
    assert_eq!(values(&with_key(&log, "account"), "new"), [Value::from(true), Value::from(false)], "续的时候是原来的账户");
    assert_eq!(with_key(&log, "issued").len(), 2);
    assert_ne!(serial_of(&fx.cert()), serial);
    assert_eq!(public(), key, "证书的私钥没换");
}

fn a_domain(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(fx.knobs());
    let r = fx.acme(&ca.directory(), &[], &["Relay.Example.com"], false);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    let log = fx.log();
    let orders = with_key(&log, "order");
    assert_eq!(orders.len(), 1, "{orders:?}");
    let want_ids = list![obj! { "type" => "dns", "value" => "relay.example.com" }];
    assert_eq!((&orders[0]["ids"], &orders[0]["profile"]), (&want_ids, &Value::from("shortlived")));
    assert_in("DNS:relay.example.com", &fx.cert());
}

fn what_goes_wrong(t: &mut Ctx) {
    let fx = setup(t);
    // 不是公网的地址：证书机构不会给签，不去要
    let r = fx.acme("https://127.0.0.1:1/dir", &[], &["192.168.1.5"], false);
    assert_eq!(r.status.py(), 2, "{}", r.err());
    assert_in("192.168.1.5 不是公网的地址", &r.err());
    let r = fx.acme("https://127.0.0.1:1/dir", &[], &["not a name"], false);
    assert_eq!(r.status.py(), 2, "{}", r.err());
    // 连不上证书机构
    let r = fx.acme("https://127.0.0.1:1/dir", &[], IP, true);
    assert_eq!(r.status.py(), 1, "{}", r.err());
    assert_in("连不上证书机构", &r.err());
    assert_in("手里没有用得了的证书", &r.err());
    // 证书机构从“公网”连不上应答的端口（80 端口没放行）：说是验证没过，不留下证书
    let ca = fx.authority(Knobs { http_port: fsx::free_port(), ..fx.knobs() });
    let r = fx.acme(&ca.directory(), &[], IP, true);
    assert_eq!(r.status.py(), 1, "{}", r.err());
    assert_in("验证这个地址是你的（证书机构从公网连 80 端口）没过", &r.err());
    assert!(!fx.d.join("cert/fullchain.pem").exists());
    assert_eq!(values(&with_key(&fx.log(), "validated"), "ok"), [Value::from(false)]);
}

fn no_short_lived_profile(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(Knobs { shortlived: false, ..fx.knobs() });
    let r = fx.acme(&ca.directory(), &[], IP, true);
    assert_eq!(r.status.py(), 1, "{}", r.err());
    assert_in("这个证书机构没有给 IP 地址签的那种证书（shortlived）", &r.err());
    assert_eq!(with_key(&fx.log(), "order"), Vec::<&Value>::new(), "没有那一种就不下单");
    // 域名的：证书机构没有 shortlived 时不写种类，照它默认的
    let r = fx.acme(&ca.directory(), &[], &["relay.example.com"], false);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    assert_eq!(values(&with_key(&fx.log(), "order"), "profile"), [Value::Null]);
}

/// 80 端口上已经有别的网站服务：不自己听端口，验证的文件写进一个目录，由那个服务给出去；验完就删
fn webroot(t: &mut Ctx) {
    let fx = setup(t);
    let root = fx.d.join("site");
    fsx::write(root.join("index.html"), "别的网站");
    // 别的网站：这个进程里照目录给文件的服务，只听 127.0.0.1
    let site = httpd::Server::bind(("127.0.0.1", fx.http_port))
        .unwrap_or_else(|e| panic!("别的网站起不来（端口 {}）：{e}", fx.http_port))
        .http10()
        .start(httpd::files(&root))
        .expect("别的网站起不来");
    let addr = SocketAddr::from(([127, 0, 0, 1], fx.http_port));
    wait_for("那个网站起来", WAIT, || TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok());
    let ca = fx.authority(fx.knobs());
    let r = fx.acme(&ca.directory(), &["--webroot", p(&root)], IP, true);
    assert_eq!(r.status.py(), 0, "{}", r.err());
    let log = fx.log();
    let pairs: Vec<(Value, Value)> =
        with_key(&log, "validated").iter().map(|e| (e["validated"].clone(), e["ok"].clone())).collect();
    assert_eq!(pairs, vec![(Value::from("127.0.0.1"), Value::from(true))]);
    assert_in("IP Address:127.0.0.1", &fx.cert());
    let held = root.join(".well-known/acme-challenge");
    assert_eq!((held.is_dir(), names(&held)), (true, Vec::<String>::new()), "验证的文件验完就删了");
    assert_eq!(fsx::read_text(root.join("index.html")), "别的网站");
    drop(site);
}

/// 几个 Team 共用一个：每个地址只要一张（两个 Team 用同一个地址也是一张），放进各自的目录（私钥 640：组是那个 Team 的）；
/// 目录还没建的那个 Team 说一声；建了以后不再要、只放进去；看不懂的登记跳过；手里没有、Team 的目录里有用得了的（以前它自己的证书助手
/// 要的）就接着用它，不再要一张
fn several_teams(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(fx.knobs());
    let directory = ca.directory();
    let (a, b, c) = ("cca-team-aaaaaaaa", "cca-team-bbbbbbbb", "cca-team-cccccccc");
    fx.register(a, "127.0.0.1", true);
    fx.register(b, "127.0.0.1", true);
    fx.register(c, "Relay.Example.com", false);
    fx.register("cca-team-dddddddd", "not a name", true);
    let r = fx.teams(&directory);
    assert_eq!(r.status.py(), 1, "{}", r.err());
    assert_in("cca-team-dddddddd 登记的地址用不了", &r.err());
    assert_in(&format!("{c} 的目录 {} 不在", fx.d.join("out").join(c).display()), &r.err());
    let log = fx.log();
    let mut ordered: Vec<String> = with_key(&log, "order").iter().map(|e| e["ids"][0]["value"].str().to_string()).collect();
    ordered.sort();
    assert_eq!(ordered, ["127.0.0.1", "relay.example.com"], "一个地址一张，两个 Team 共用 127.0.0.1 的那张");
    let store = fx.d.join("store");
    for team in [a, b] {
        for f in ["fullchain.pem", "privkey.pem"] {
            assert_eq!(fsx::read(fx.d.join("out").join(team).join(f)), fsx::read(store.join("ip-127.0.0.1").join(f)), "{team}{f}");
        }
        assert_eq!([fx.mode(&format!("out/{team}/privkey.pem")), fx.mode(&format!("out/{team}/fullchain.pem"))], [0o640, 0o644]);
        assert_eq!(names(&fx.d.join("out").join(team)), ["fullchain.pem", "privkey.pem"], "没有留下半截的文件");
    }
    assert_eq!([fx.mode("store"), fx.mode("store/ip-127.0.0.1"), fx.mode("store/ip-127.0.0.1/privkey.pem")], [0o700, 0o700, 0o600]);
    let san = openssl(&["x509", "-in", p(&store.join("dns-relay.example.com/fullchain.pem")), "-noout", "-ext", "subjectAltName"]);
    assert_in("DNS:relay.example.com", &String::from_utf8(san).expect("openssl 输出的不是 UTF-8"));
    // c 的目录建好了：不再要，只放进去
    std::fs::create_dir(fx.d.join("out").join(c)).expect("建 c 的目录");
    let n = fx.log().len();
    let r = fx.teams(&directory);
    assert_eq!((r.status.py(), fx.log().len()), (0, n), "{}", r.err());
    assert_eq!(
        fsx::read(fx.d.join("out").join(c).join("fullchain.pem")),
        fsx::read(store.join("dns-relay.example.com/fullchain.pem"))
    );
    // 存的那一份没了（新装的证书助手），a 的目录里有用得了的：接着用它，不再要一张
    std::fs::rename(&store, fx.d.join("store.old")).expect("挪走 store");
    std::fs::remove_file(fx.d.join("teams").join(c)).expect("去掉 c 的登记");
    let r = fx.teams(&directory);
    assert_eq!((r.status.py(), fx.log().len()), (0, n), "{}", r.err());
    assert_in(&format!("127.0.0.1 接着用 {a} 原来的证书"), &r.err());
    assert_eq!(fsx::read(store.join("ip-127.0.0.1/privkey.pem")), fsx::read(fx.d.join("out").join(a).join("privkey.pem")));
    assert!(!store.join("dns-relay.example.com").exists(), "没人用的地址不要");
}

/// 一直运行：登记的变了（安装脚本加了一个 Team）过几秒就照新的，不用重启
fn several_teams_running(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(fx.knobs());
    fx.register("cca-team-aaaaaaaa", "127.0.0.1", true);
    let (mut p, err) = fx.spawn(&fx.teams_argv(&ca.directory(), false), "acme.err");
    fx.wait_file("第一个 Team 的证书", &fx.d.join("out/cca-team-aaaaaaaa/fullchain.pem"), WAIT, &mut p, &err);
    fx.register("cca-team-bbbbbbbb", "127.0.0.1", true);
    fx.wait_file("第二个 Team 的证书", &fx.d.join("out/cca-team-bbbbbbbb/fullchain.pem"), Duration::from_secs(30), &mut p, &err);
    assert!(p.try_wait().expect("看证书助手退出了没有").is_none());
    assert_eq!(with_key(&fx.log(), "issued").len(), 1, "同一个地址：还是那一张");
}

/// 不带 --once：拿到证书以后接着运行（等着续）；应答的端口上别的路径都是 404
fn the_responder_and_running_forever(t: &mut Ctx) {
    let fx = setup(t);
    let ca = fx.authority(fx.knobs());
    let mut argv = acme_argv();
    argv.extend(
        [
            "--out",
            p(&fx.d.join("cert")),
            "--account",
            p(&fx.d.join("account")),
            "--listen",
            &format!("127.0.0.1:{}", fx.http_port),
            "--directory",
            &ca.directory(),
            "--ca",
            p(&fx.cad.join("ca.pem")),
            "--id",
            "127.0.0.1",
            "--any-ip",
            "--port-file",
            p(&fx.d.join("aport")),
        ]
        .map(String::from),
    );
    let (mut p, err) = fx.spawn(&argv, "acme.err");
    fx.wait_file("证书", &fx.d.join("cert/fullchain.pem"), WAIT, &mut p, &err);
    assert_eq!(fsx::read_text(fx.d.join("aport")).trim().parse::<u16>().expect("aport 里是端口"), fx.http_port);
    let long = format!("/.well-known/acme-challenge/{}", "a".repeat(300));
    for path in [
        "/",
        "/.well-known/acme-challenge/",
        "/.well-known/acme-challenge/nope",
        "/.well-known/acme-challenge/../../etc/passwd",
        "/privkey.pem",
        long.as_str(),
    ] {
        assert_eq!(http_get(fx.http_port, path), (404, Vec::new()), "{path}");
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(p.try_wait().expect("看证书助手退出了没有").is_none(), "拿到证书以后接着运行");
    assert_eq!(with_key(&fx.log(), "issued").len(), 1);
}
