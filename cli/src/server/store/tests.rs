//! Store 的单元测试。
//!
//! - `replays_the_python_store`：tests/vectors/server_store.json 是 Python 的 Store 照一串请求、给定的时间走一遍的结果
//!   （每一步回的内容、往 stderr 说的话、变了的状态文件的内容）；这里照样走一遍，逐字节对。开头的文件是手写的 Python 的格式：
//!   整数和小数的时间、中文的名字、不认识的键。
//! - 其余几条是从外面测不了的那几段（Store 的 team_* 的规矩、记下的 API key 和折合金额的各个上限、
//!   以前的版本留下的折合金额：黑盒的那一份 tests/presence.rs 里没有这几段，在这里）。上限换小的用 `Limits`，时间自己给。

use super::*;
use crate::py::base64;
use crate::py::fs::TempDir;
use crate::server::{INVITE_KEEP, TEAM_ADMIN_OPS};
use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;

thread_local! {
    /// say 说过的话（这个线程的）
    pub(super) static SAID: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// say 说过的话：取走
pub(in crate::server) fn said() -> Vec<String> {
    SAID.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// 一把样子对的公钥那一行（32 个字节都是 n：Store 不验签名，只看样子）
pub(in crate::server) fn key_line(n: u8) -> String {
    let mut blob = Vec::new();
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend_from_slice(&[n; 32]);
    format!("ssh-ed25519 {}", base64::b64encode(&blob))
}

/// 那把公钥的指纹
pub(in crate::server) fn fp(n: u8) -> String {
    sshsig::key_fp(&sshsig::key_blob(&key_line(n)).unwrap())
}

/// 根公钥用的那一把
const ROOT: u8 = 200;

fn dev(n: u8) -> String {
    format!("{n:02x}").repeat(8)
}

fn aid(n: u8) -> String {
    format!("{n:02x}").repeat(16)
}

fn token(n: u8) -> String {
    format!("{n:02x}").repeat(16)
}

/// 认证码的样子（32 个字节的 base64）
fn tag(n: u8) -> String {
    base64::b64encode(&[n; 32])
}

/// 一份密文（JSON 文字）：密文那一段是 size 个字符（要是 4 的倍数），seed 不一样认证码就不一样
fn blob(size: usize, seed: u8) -> String {
    format!(r#"{{"n":"{}","c":"{}","t":"{}"}}"#, base64::b64encode(&[seed; 16]), "A".repeat(size), tag(seed))
}

fn js(text: &str) -> V {
    json::loads(text).unwrap_or_else(|e| panic!("{e}：{text}"))
}

/// 一个临时目录里的四个状态文件
struct Dir(TempDir);

impl Dir {
    fn new() -> Dir {
        said();
        Dir(TempDir::new().unwrap())
    }

    fn at(&self, name: &str) -> String {
        format!("{}/{name}", self.0.path)
    }

    fn cfg(&self) -> StoreCfg {
        StoreCfg {
            path: self.at("state.json").into(),
            ttl: 150.0,
            idle: 600.0,
            retain: 30.0 * 86400.0,
            keys_path: None,
            cost_path: None,
            cost_keep: 100.0 * 86400.0,
        }
    }

    fn open(&self) -> Store {
        Store::open(&self.cfg(), &key_line(ROOT), Limits::DEFAULT, 0.0).unwrap()
    }

    fn exists(&self, name: &str) -> bool {
        std::path::Path::new(&self.at(name)).exists()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.at(name)).unwrap()
    }

    fn json(&self, name: &str) -> Dict {
        js(&self.read(name)).as_dict().unwrap()
    }

    /// 手写一个文件，Python 的 json.dumps 默认的写法（带空格、中文写成转义的）
    fn put(&self, name: &str, v: &Dict) {
        std::fs::write(self.at(name), json::dumps(&V::Dict(v.clone()), Fmt::new())).unwrap();
    }

    fn mode(&self, name: &str) -> u32 {
        std::fs::metadata(self.at(name)).unwrap().permissions().mode() & 0o777
    }

    fn mtime(&self, name: &str) -> std::time::SystemTime {
        std::fs::metadata(self.at(name)).unwrap().modified().unwrap()
    }
}

/// 管理员的请求（key 是根公钥的指纹）
fn admin(st: &mut Store, op: &str, body: &str, now: f64) -> (u16, Dict) {
    let root = st.root_fp().to_string();
    let (code, reply) = st.op(op, &root, &js(body), now).unwrap();
    (code, reply.as_dict().unwrap())
}

/// 设备的请求：和 handler 一样先记下它来过
fn device(st: &mut Store, op: &str, who: u8, body: &str, now: f64) -> (u16, Dict) {
    st.seen(&fp(who), now);
    let (code, reply) = st.op(op, &fp(who), &js(body), now).unwrap();
    (code, reply.as_dict().unwrap())
}

fn txt(r: (u16, Dict)) -> (u16, String) {
    (r.0, text_of(&V::Dict(r.1)))
}

fn invite(st: &mut Store, token: &str, dev: &str, ttl: i64, now: f64) -> (u16, Dict) {
    admin(st, "team_invite", &format!(r#"{{"token":"{}","dev":"{dev}","ttl":{ttl}}}"#, hash::sha256_hex(token)), now)
}

fn join(st: &mut Store, who: u8, name: &str, token: &str, now: f64, proof: Option<&str>) -> (u16, Dict) {
    let proof = proof.map_or(String::new(), |p| format!(r#","proof":"{p}""#));
    let body = format!(r#"{{"pub":"{}","name":"{name}","token":"{token}"{proof}}}"#, key_line(who));
    let (code, reply) = st.op("team_join", &fp(who), &js(&body), now).unwrap();
    (code, reply.as_dict().unwrap())
}

/// team_put：每一项是 (编号, 序号, 密文多长)
fn put_docs(st: &mut Store, docs: &[(String, i64, usize)], now: f64) -> (u16, Dict) {
    let items: Vec<String> =
        docs.iter().map(|(dev, seq, size)| format!(r#""{dev}":{{"seq":{seq},"ct":{},"sig":"AAAA"}}"#, blob(*size, 0))).collect();
    admin(st, "team_put", &format!(r#"{{"docs":{{{}}}}}"#, items.join(",")), now)
}

/// team_enroll：每一项是 (编号, 哪把公钥, 名字)
fn enroll(st: &mut Store, devs: &[(String, u8, &str)], now: f64) -> (u16, Dict) {
    let items: Vec<String> =
        devs.iter().map(|(dev, who, name)| format!(r#""{dev}":{{"pub":"{}","name":"{name}"}}"#, key_line(*who))).collect();
    admin(st, "team_enroll", &format!(r#"{{"devices":{{{}}}}}"#, items.join(",")), now)
}

fn revoke(st: &mut Store, dev: &str) -> (u16, String) {
    txt(admin(st, "team_revoke", &format!(r#"{{"dev":"{dev}"}}"#), 0.0))
}

fn members(st: &mut Store, now: f64) -> (u16, Dict) {
    admin(st, "team_members", "{}", now)
}

fn strs(d: &Dict, key: &str) -> Vec<String> {
    d.list(key).unwrap().strs().iter().map(|s| s.to_string()).collect()
}

fn look(st: &mut Store, token: &str, now: f64) -> Invite {
    st.invite_state(&V::str(token), now)
}

#[test]
fn invite_errors_name_only_the_two_safe_cases() {
    assert_eq!((Invite::Expired.error(), Invite::Used.error()), (Some("expired"), Some("used")));
    assert_eq!((Invite::Ok.error(), Invite::Missing.error()), (None, None));
}

/// Python 的 Store 照同样的请求走一遍的结果（tests/vectors/server_store.json）：回的内容、说的话、每个文件的每个字节都一样
#[test]
fn replays_the_python_store() {
    let doc = js(include_str!("../../../tests/vectors/server_store.json")).as_dict().unwrap();
    let d = Dir::new();
    let mut last: Vec<(String, String)> = Vec::new();
    for (name, text) in doc.dict("files").unwrap().items() {
        std::fs::write(d.at(&name), text.as_str().unwrap()).unwrap();
        last.push((name.to_string(), text.as_str().unwrap().to_string()));
    }
    let root = doc.str("root").unwrap();
    let mut st: Option<Store> = None;
    let steps = doc.list("steps").unwrap().items();
    assert!(steps.len() > 60);
    for (i, step) in steps.iter().enumerate() {
        let step = step.as_dict().unwrap();
        let op = step.str("op").unwrap();
        let what = format!("第 {i} 步 {op}：{}", step.str("body").unwrap_or_default());
        let now = step.get("now").unwrap().as_num().unwrap();
        if &*op == "_open" {
            st = Some(Store::open(&d.cfg(), &root, Limits::DEFAULT, now).unwrap());
        } else {
            let st = st.as_mut().unwrap();
            let who = step.str("fp").unwrap();
            if &*op != "team_join" && !TEAM_ADMIN_OPS.contains(&&*op) {
                st.seen(&who, now); // handler 做的那一步（Py:1969-1970）
            }
            let (code, reply) = st.op(&op, &who, &js(&step.str("body").unwrap()), now).unwrap();
            assert_eq!(
                (i64::from(code), text_of(&reply)),
                (step.get("code").unwrap().as_int().unwrap(), step.str("reply").unwrap().to_string()),
                "{what}"
            );
        }
        assert_eq!(said(), strs(&step, "said"), "{what}");
        for (name, text) in step.dict("files").unwrap().items() {
            last.iter_mut().find(|(n, _)| *n == *name).unwrap().1 = text.as_str().unwrap().to_string();
        }
        for (name, text) in &last {
            assert_eq!(&d.read(name), text, "{what}：{name}");
        }
    }
}

/// Python 写的文件（上面那份数据里每一步之后的样子）读进来、原样再存一遍：一个字节都不变
#[test]
fn python_written_files_round_trip() {
    let doc = js(include_str!("../../../tests/vectors/server_store.json")).as_dict().unwrap();
    let root = doc.str("root").unwrap();
    // 开头那四个是手写的（带空格、带缩进）：Python 写过一次以后的才拿来比
    let mut now: Vec<(String, String, bool)> =
        doc.dict("files").unwrap().items().iter().map(|(n, t)| (n.to_string(), t.as_str().unwrap().into(), false)).collect();
    let mut checked = 0;
    for step in doc.list("steps").unwrap().items() {
        let changed = step.as_dict().unwrap().dict("files").unwrap();
        if changed.is_empty() {
            continue;
        }
        for (name, text) in changed.items() {
            let file = now.iter_mut().find(|(n, _, _)| *n == *name).unwrap();
            (file.1, file.2) = (text.as_str().unwrap().to_string(), true);
        }
        let d = Dir::new();
        for (name, text, _) in &now {
            std::fs::write(d.at(name), text).unwrap();
        }
        let st = Store::open(&d.cfg(), &root, Limits::DEFAULT, 0.0).unwrap();
        st.save().unwrap();
        st.save_keys().unwrap();
        st.save_cost().unwrap();
        st.save_docs().unwrap();
        for (name, text, _) in now.iter().filter(|f| f.2) {
            assert_eq!(&d.read(name), text, "{name}");
            checked += 1;
        }
    }
    assert!(checked > 40, "{checked}");
}

/// 邀请过期、用掉、很久以后不记了；满了；改名字；手改坏了的状态文件
#[test]
fn invites_members_and_a_broken_state_file() {
    let d = Dir::new();
    let mut st = d.open();
    let (t1, t2) = (token(0xaa), token(0xbb));
    assert_eq!(invite(&mut st, &t1, &dev(1), 60, 1000.0).0, 200);
    assert_eq!(
        (look(&mut st, &t1, 1059.0), look(&mut st, &t1, 1060.0), look(&mut st, &t2, 1000.0), look(&mut st, "zz", 1000.0)),
        (Invite::Ok, Invite::Expired, Invite::Missing, Invite::Missing)
    );
    assert_eq!(st.invite_state(&V::Int(7), 1000.0), Invite::Missing);
    assert_eq!(txt(join(&mut st, 1, "late", &t1, 1100.0, None)), (403, r#"{"error":"expired"}"#.into()));
    assert_eq!(
        strs(&put_docs(&mut st, &[(dev(1), 1, 1024)], 1100.0).1, "stored"),
        [dev(1)],
        "过期不久的邀请：配置照存（管理员多半会再发一个）"
    );
    assert!(members(&mut st, 1100.0).1.list("invites").unwrap().is_empty(), "过期的邀请不列出来");
    assert_eq!(invite(&mut st, &t2, &dev(1), 60, 1100.0).0, 200, "同一个编号再发");
    assert!(said().is_empty());
    assert_eq!(join(&mut st, 1, "mac", &t2, 1101.0, Some(&tag(9))).0, 200);
    assert_eq!(said(), ["cca-presence: 加入了一台设备，现在 1 台"]);
    assert_eq!(look(&mut st, &t2, 1102.0), Invite::Used);
    assert_eq!(txt(join(&mut st, 2, "again", &t2, 1102.0, None)), (403, r#"{"error":"used"}"#.into()));
    let later = 1101.0 + INVITE_KEEP as f64 + 1.0;
    assert_eq!(members(&mut st, later).0, 200); // 看名单、存配置、发邀请时顺带清
    assert_eq!(look(&mut st, &t2, later), Invite::Missing, "用掉很久的不记了");
    // 给邀请存的配置：邀请过期很久被清掉时跟着删；只剩配置的编号也移除得掉
    assert_eq!(invite(&mut st, &token(0xcc), &dev(2), 60, later).0, 200);
    assert_eq!(strs(&put_docs(&mut st, &[(dev(2), 1, 1024)], later).1, "stored"), [dev(2)]);
    assert_eq!(strs(&put_docs(&mut st, &[(dev(1), 2, 1024)], later + 60.0 + INVITE_KEEP as f64).1, "stored"), [dev(1)]);
    assert_eq!(d.json("team-docs.json").dict("docs").unwrap().keys().len(), 1, "02 的邀请清掉了，配置跟着删");
    assert!(d.json("team-docs.json").dict("docs").unwrap().has(&dev(1)));
    let left = DocEntry {
        seq: 1,
        ct: wire::parse_blob(&js(&blob(1024, 0)), MAX_TEAM_DOC).unwrap(),
        sig: "AAAA".into(),
        by: None,
        at: Some(Num::Int(1)),
    };
    st.docs.items.insert(dev(3), left); // 移除设备时写了一半留下的
    assert_eq!(revoke(&mut st, &dev(3)), (200, r#"{"v":1,"ok":true,"members":1}"#.into()));
    assert_eq!(revoke(&mut st, &dev(3)), (404, r#"{"error":"no_device"}"#.into()));
    assert!(said().is_empty(), "没有设备被移除：不说话");

    // 满了
    st.lim.max_team_members = 1;
    assert_eq!(txt(enroll(&mut st, &[(dev(4), 2, "two")], later)), (507, r#"{"error":"full"}"#.into()));
    assert_eq!(invite(&mut st, &token(0xdd), &dev(5), 60, later).0, 200);
    assert_eq!(txt(join(&mut st, 2, "two", &token(0xdd), later, None)), (507, r#"{"error":"full"}"#.into()));
    let renamed = enroll(&mut st, &[(dev(1), 1, "renamed")], later).1;
    assert_eq!(text_of(&renamed.get("names").unwrap()), format!(r#"{{"{}":"renamed"}}"#, dev(1)), "已经在的设备改名字不算多");
    st.lim = Limits::DEFAULT;
    let m = st.machines.get(&fp(1)).unwrap();
    assert_eq!(
        (m.name.as_str(), m.joined, m.proof.clone()),
        ("renamed", Some(Num::Float(1101.0)), Some(tag(9))),
        "改名字：加入的时间、加入时的认证码照旧"
    );
    let rows = members(&mut st, later).1.list("members").unwrap().items();
    let row = rows.iter().map(|r| r.as_dict().unwrap()).find(|r| *r.str("dev").unwrap() == *dev(1)).unwrap();
    assert_eq!(
        (&*row.str("name").unwrap(), &*row.str("proof").unwrap()),
        ("renamed", tag(9).as_str()),
        "管理员照旧核对得了它的公钥"
    );
    assert_eq!(revoke(&mut st, &dev(5)).0, 200);

    // 状态文件手改坏了：样子不对的邀请、设备不要，照常启动、照常回话
    let raw = d.json("state.json");
    let team = raw.dict("team").unwrap();
    let invites = team.dict("invites").unwrap();
    invites.set("x", 1);
    invites.set("y", dict! { "dev" => dev(6) });
    invites.set("z", dict! { "dev" => "nope", "exp" => 9 });
    team.set("spent", dict! { "x" => "soon" });
    let machines = raw.dict("machines").unwrap();
    machines.set("SHA256:bad", dict! { "name" => "no-dev", "key" => key_line(3) });
    machines.set("SHA256:worse", 7);
    d.put("state.json", &raw);
    let mut st = d.open();
    assert_eq!((st.invites.len(), st.spent.len(), st.fps()), (0, 0, vec![fp(1)]));
    assert_eq!((members(&mut st, later).0, look(&mut st, &token(0xaa), later)), (200, Invite::Missing));
    let fixed = d.json("state.json");
    assert_eq!(fixed.dict("machines").unwrap().keys().len(), 1, "筛掉了东西：状态文件重写了");
    assert_eq!(text_of(&fixed.dict("team").unwrap().get("invites").unwrap()), "{}");
    // 没筛掉什么的再启动一次：不重写
    let before = d.mtime("state.json");
    std::thread::sleep(std::time::Duration::from_millis(20));
    d.open();
    assert_eq!(d.mtime("state.json"), before);
    assert!(said().is_empty());
}

/// 没用掉的邀请最多 MAX_INVITES 个（过期的不算）；配置加起来有上限（换掉已有的一份不算多）
#[test]
fn invite_and_doc_caps() {
    let d = Dir::new();
    let mut st = d.open();
    assert_eq!(invite(&mut st, &token(0xf0), &dev(0xff), 60, 0.0).0, 200);
    for i in 0..Limits::DEFAULT.max_invites {
        assert_eq!(invite(&mut st, &format!("{i:032x}"), &format!("{i:016x}"), 3600, 100.0).0, 200, "{i}");
    }
    assert_eq!(st.invites.len(), 65, "过期的那一个还记着，只是不算数");
    assert_eq!(txt(invite(&mut st, &token(0xee), &dev(0xee), 3600, 100.0)), (507, r#"{"error":"full"}"#.into()));

    let d = Dir::new();
    let mut st = d.open();
    st.docs_bytes = 3 * 1024;
    for i in 0..4u8 {
        assert_eq!(invite(&mut st, &token(i), &dev(i), 3600, 100.0).0, 200);
    }
    assert_eq!(put_docs(&mut st, &[(dev(0), 1, 1024), (dev(1), 1, 1024)], 100.0).0, 200);
    assert_eq!(put_docs(&mut st, &[(dev(2), 1, 1024)], 100.0).0, 200);
    assert_eq!(txt(put_docs(&mut st, &[(dev(3), 1, 1024)], 100.0)), (507, r#"{"error":"full"}"#.into()));
    assert_eq!(put_docs(&mut st, &[(dev(0), 2, 1024)], 100.0).0, 200, "换掉已有的一份不算多");
    assert_eq!(txt(put_docs(&mut st, &[(dev(0), 3, 1028)], 100.0)).0, 507, "换成更大的一份放不下");
    assert_eq!(d.mode("team-docs.json"), 0o600);
}

/// 状态文件是以前别的服务器的（3.5.3 以前的自己的同步服务器、3.6.0 以前的维护者的服务器）、换了根公钥：从头开始，
/// 各台设备的配置也删掉，日志里说一句；第一次启动不说话，也不留一个空的 team-docs.json
#[test]
fn starts_over_for_another_server() {
    let d = Dir::new();
    let mut st = d.open();
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "mac")], 5.0).0, 200);
    assert_eq!(put_docs(&mut st, &[(dev(1), 1, 1024)], 6.0).0, 200);
    let st = d.open();
    assert_eq!((st.machines(), st.docs.items.len()), (1, 1), "同一个根公钥：接着用");
    let gen = st.gen().to_string();
    assert!(wire::gen_re().matches(&gen) && said().is_empty());

    // 3.5.3 以前的自己的同步服务器（--mode own）留下的状态文件
    let raw = d.json("state.json");
    raw.pop("team");
    raw.set("own", dict! { "seed" => "0".repeat(64), "admit" => "0".repeat(64), "gen" => "ab".repeat(16) });
    d.put("state.json", &raw);
    let mut st = d.open();
    assert_eq!((st.machines(), st.docs.items.len(), st.rest.clone()), (0, 0, vec![Rest::Team]));
    assert!(st.gen() != gen && st.gen() != "ab".repeat(16) && wire::gen_re().matches(st.gen()));
    assert_eq!(said(), ["cca-presence: 以前是自己的同步服务器（3.5.3 以前的 --mode own）：以前的 1 台设备和存的密文都不要了"]);
    assert_eq!(d.read("team-docs.json"), r#"{"v":1,"docs":{}}"#);
    assert!(!d.json("state.json").has("own"));
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "mac")], 5.0).0, 200);

    // 3.6.0 以前的维护者的服务器（--mode maintainer）留下的：清单里的设备不是这个 Team 的，存的密文也不要了
    let m = Dir::new();
    m.put(
        "state.json",
        &js(&format!(
            r#"{{"v":1,"seq":7,"machines":{{"SHA256:m":{{"name":"old-mac","key":"{}"}}}},"beats":{{}},"units":{{}}}}"#,
            key_line(2)
        ))
        .as_dict()
        .unwrap(),
    );
    m.put(
        "keys.json",
        &js(&format!(
            r#"{{"v":1,"acc":{{"{}":{{"blob":{},"fp":"SHA256:m","name":"old-mac","at":1}}}}}}"#,
            aid(0xab),
            blob(1024, 0)
        ))
        .as_dict()
        .unwrap(),
    );
    let mut st = m.open();
    assert_eq!((st.machines(), st.seq.as_str(), st.keys.items.len(), st.root_key()), (0, "0", 0, key_line(ROOT).as_str()));
    assert_eq!(said(), ["cca-presence: 以前是维护者的服务器（3.6.0 以前的 --mode maintainer）：以前的 1 台设备和存的密文都不要了"]);
    assert_eq!(m.read("keys.json"), r#"{"v":1,"acc":{}}"#, "存的密文也不要了");
    assert_eq!(m.read("cost-slots.json"), r#"{"v":1,"acc":{}}"#);
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "mac")], 5.0).0, 200);
    assert!(!m.exists("team-docs.json"), "没有配置要删：旁边不留一个空的 team-docs.json");

    // 换了一个 Team（根公钥不一样）
    let other = Store::open(&m.cfg(), &key_line(201), Limits::DEFAULT, 0.0).unwrap();
    assert_eq!(said(), ["cca-presence: 换了一个 Team（根公钥不一样）：以前的 1 台设备和存的密文都不要了"]);
    assert_eq!((other.machines(), other.root_key()), (0, key_line(201).as_str()));
    assert_eq!(m.json("state.json").dict("team").unwrap().str("root").unwrap().to_string(), key_line(201));

    // 第一次启动（什么都没有）：不说话，也不留一个空的 team-docs.json
    let none = Dir::new();
    let st = none.open();
    assert!(said().is_empty() && !none.exists("team-docs.json"));
    assert_eq!(
        none.read("state.json"),
        format!(
            r#"{{"v":1,"seq":0,"machines":{{}},"beats":{{}},"units":{{}},"team":{{"root":"{}","gen":"{}","invites":{{}},"spent":{{}}}}}}"#,
            key_line(ROOT),
            st.gen()
        )
    );
    assert_eq!((none.mode("state.json"), none.mode("keys.json"), none.mode("cost-slots.json")), (0o600, 0o600, 0o600));
}

/// 读不了、格式不对的文件：在日志里说一声，从空的开始（没有这个文件不说话）
#[test]
fn unreadable_files_start_empty() {
    let d = Dir::new();
    std::fs::write(d.at("state.json"), "{\"machines\": ").unwrap();
    std::fs::write(d.at("keys.json"), [0xffu8, b'{']).unwrap();
    std::fs::create_dir(d.at("cost-slots.json")).unwrap();
    std::fs::write(d.at("team-docs.json"), "[1]").unwrap(); // 不是字典：不说话，当它没有
    let err = Store::open(&d.cfg(), &key_line(ROOT), Limits::DEFAULT, 0.0).map(|_| ()).unwrap_err();
    assert_eq!(
        said(),
        [
            format!("cca-presence: {} 读不了（Expecting value: line 1 column 14 (char 13)），从空的开始", d.at("state.json")),
            format!(
                "cca-presence: {} 读不了（'utf-8' codec can't decode byte 0xff in position 0: invalid start byte），从空的开始",
                d.at("keys.json")
            ),
            format!("cca-presence: {0} 读不了（[Errno 21] Is a directory: '{0}'），从空的开始", d.at("cost-slots.json")),
        ]
    );
    // 存不了（那个位置上是个目录，换不过去）：启动不了，说得出是哪个文件
    let text = err.to_string();
    assert!(text.contains("cost-slots.json") && text.contains(" -> ") && text.starts_with("[Errno "), "{text}");
    assert!(
        matches!(Store::open(&d.cfg(), "ssh-ed25519", Limits::DEFAULT, 0.0).map(|_| ()), Err(Fail::Value(_))),
        "根公钥那一行不对"
    );
}

/// 状态文件旁边的三个文件在哪：照 Python 拼（Py:437-439）；--keys、--cost 给了就用给的
#[test]
fn side_files_sit_beside_the_state_file() {
    let d = Dir::new();
    let st = d.open();
    assert_eq!(
        (st.keys_path.clone(), st.cost_path.clone(), st.docs_path.clone()),
        (d.at("keys.json"), d.at("cost-slots.json"), d.at("team-docs.json"))
    );
    let d = Dir::new();
    let mut cfg = d.cfg();
    cfg.keys_path = Some(d.at("k.json").into());
    cfg.cost_path = Some(std::path::PathBuf::new());
    let st = Store::open(&cfg, &key_line(ROOT), Limits::DEFAULT, 0.0).unwrap();
    assert_eq!((st.keys_path.clone(), st.cost_path.clone()), (d.at("k.json"), d.at("cost-slots.json")));
    assert!(d.exists("k.json") && !d.exists("keys.json"));
    // 状态文件只写了个名字（当前目录下）：旁边就是 ./
    cfg.path = "state.json".into();
    cfg.keys_path = None;
    assert_eq!(
        path::join(
            &match path::dirname("state.json") {
                d if d.is_empty() => ".".to_string(),
                d => d,
            },
            "keys.json"
        ),
        "./keys.json"
    );
}

/// 名字被别的设备用了就加 -2、-3……，最长 64 个字符
#[test]
fn taken_names_get_a_suffix() {
    let d = Dir::new();
    let mut st = d.open();
    let long = "长".repeat(64);
    let r =
        enroll(&mut st, &[(dev(1), 1, "mac"), (dev(2), 2, "mac"), (dev(3), 3, "mac"), (dev(4), 4, &long), (dev(5), 5, &long)], 5.0)
            .1;
    let names = r.dict("names").unwrap();
    let name = |n: u8| names.str(&dev(n)).unwrap().to_string();
    assert_eq!((name(1), name(2), name(3), name(4)), ("mac".into(), "mac-2".into(), "mac-3".into(), long.clone()));
    assert_eq!(name(5), format!("{}-2", "长".repeat(62)));
    // 同一把公钥再登记同一个名字：还是它自己的名字
    let again = enroll(&mut st, &[(dev(2), 2, "mac-2")], 6.0).1;
    assert_eq!(again.dict("names").unwrap().str(&dev(2)).unwrap().to_string(), "mac-2");
}

/// 存满了（MAX_KEYS 个账号）：新的账号不收，已有的照常换
#[test]
fn key_cap() {
    let d = Dir::new();
    let mut st = d.open();
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "alice-mbp")], 5.0).0, 200);
    st.lim.max_keys = 2;
    let put = |st: &mut Store, items: &[(u8, u8)]| {
        let body: Vec<String> = items.iter().map(|(a, seed)| format!(r#""{}":{}"#, aid(*a), blob(256, *seed))).collect();
        device(st, "keys", 1, &format!(r#"{{"put":{{{}}}}}"#, body.join(",")), 100.0)
    };
    assert_eq!(put(&mut st, &[(1, 0), (2, 0)]).0, 200);
    assert_eq!(txt(put(&mut st, &[(3, 0)])), (507, r#"{"error":"full"}"#.into()));
    assert_eq!(put(&mut st, &[(1, 7)]).0, 200);
    assert_eq!(st.keys.items.get(&aid(1)).unwrap().blob.t, tag(7));
    assert_eq!(d.mode("keys.json"), 0o600);
}

/// 折合金额的各个上限、回的密文的上限、很久没再传的清掉、换了密钥旧的那把传的跟着删掉
#[test]
fn cost_caps() {
    let d = Dir::new();
    let mut cfg = d.cfg();
    (cfg.retain, cfg.cost_keep) = (86400.0, 1000.0);
    let mut st = Store::open(&cfg, &key_line(ROOT), Limits::DEFAULT, 0.0).unwrap();
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "alice-mbp"), (dev(2), 2, "bob-mini")], 5.0).0, 200);
    let now = 1_700_000_000.0;
    let (x, y, z) = (1u8, 2u8, 3u8);
    let size = 2048usize;
    let put = |st: &mut Store, who: u8, items: &[(u8, usize)], now: f64| {
        let body: Vec<String> = items.iter().map(|(a, n)| format!(r#""{}":{}"#, aid(*a), blob(*n, who))).collect();
        device(st, "ledger", who, &format!(r#"{{"put":{{{}}}}}"#, body.join(",")), now)
    };
    let full = (507, r#"{"error":"full"}"#.to_string());
    st.lim.cost_slots = 2;
    assert_eq!(put(&mut st, 1, &[(x, size), (y, size)], now).0, 200);
    assert_eq!(txt(put(&mut st, 1, &[(z, size)], now)), full, "一台设备最多这么多个账号");
    assert_eq!(put(&mut st, 1, &[(x, size)], now).0, 200, "已有的照常换");
    assert_eq!(put(&mut st, 2, &[(z, size)], now).0, 200, "别的设备不受影响");
    st.lim = Limits { cost_device_bytes: size * 2 + 10, ..Limits::DEFAULT };
    assert_eq!(txt(put(&mut st, 2, &[(x, size), (y, size)], now)), full, "一台设备存的加起来有上限");
    st.lim = Limits { cost_total_bytes: size * 4 - 1, ..Limits::DEFAULT };
    assert_eq!(txt(put(&mut st, 2, &[(y, size)], now)), full, "所有设备加起来有上限");
    st.lim = Limits::DEFAULT;
    assert_eq!(put(&mut st, 2, &[(x, 4000)], now + 5.0).0, 200);
    assert_eq!(enroll(&mut st, &[(dev(0x0c), 3, "carol-air")], now).0, 200);
    st.lim.cost_resp = 4500;
    let r = device(&mut st, "ledger", 3, &format!(r#"{{"get":["{}"]}}"#, aid(x)), now + 6.0).1;
    let rows: Vec<(String, bool)> = r
        .dict("cost")
        .unwrap()
        .list(&aid(x))
        .unwrap()
        .items()
        .iter()
        .map(|r| r.as_dict().unwrap())
        .map(|r| (r.str("name").unwrap().to_string(), r.has("blob")))
        .collect();
    assert_eq!(
        rows,
        [("bob-mini".to_string(), true), ("alice-mbp".to_string(), false)],
        "回的密文有上限：最近传的先给，别的下次再取"
    );
    st.lim = Limits::DEFAULT;
    assert_eq!(device(&mut st, "ledger", 9, &format!(r#"{{"get":["{}"]}}"#, aid(x)), now).0, 403);
    st.lim.cost_total_bytes = size * 2;
    assert_eq!(
        put(&mut st, 2, &[(y, size)], now + 2000.0).0,
        200,
        "存满了：先清掉 cost_keep 秒没再传的，再看放不放得下（不然一直清不掉）"
    );
    assert_eq!(st.cost.items.keys().collect::<Vec<_>>(), [aid(y).as_str()], "cost_keep 秒没再传的删掉");
    // 换了密钥（同一个编号登记另一把公钥）：旧的那把传的跟着删掉
    assert_eq!(put(&mut st, 1, &[(x, size)], now + 2000.0).0, 200);
    assert_eq!(enroll(&mut st, &[(dev(2), 4, "bob-mini")], now + 2000.0).0, 200);
    let left: Vec<(String, Vec<String>)> =
        st.cost.items.iter().map(|(a, s)| (a.to_string(), s.keys().map(str::to_string).collect())).collect();
    assert_eq!(left, [(aid(x), vec![fp(1)])]);
    assert_eq!(d.json("cost-slots.json").dict("acc").unwrap().keys().len(), 1, "文件里也没有了");
    assert_eq!(d.mode("cost-slots.json"), 0o600);
}

/// 以前的版本移除设备时留下的：照旧的根公钥接着用（重启、更新）时清掉——不在名单里的设备传的折合金额删掉，记下的 API key 的密文
/// 留着、不再记着是它传的；名单里的照旧；没什么要清的不重写
#[test]
fn leftovers_of_removed_devices_are_cleaned_at_start() {
    let d = Dir::new();
    let mut st0 = d.open();
    assert_eq!(enroll(&mut st0, &[(dev(1), 1, "alice-mbp")], 5.0).0, 200);
    let (fa, fb, now) = (fp(1), fp(2), 1_700_000_000.5);
    let (x, y, b) = (aid(1), aid(2), blob(64, 0));
    let slot = |name: &str| format!(r#"{{"blob":{b},"name":"{name}","at":{now:?}}}"#);
    let cost = format!(
        r#"{{"v":1,"acc":{{"{x}":{{"{fa}":{},"{fb}":{}}},"{y}":{{"{fb}":{}}}}}}}"#,
        slot("alice-mbp"),
        slot("bob-mini"),
        slot("bob-mini")
    );
    let key = |f: &str, name: &str| format!(r#"{{"blob":{b},"fp":"{f}","name":"{name}","at":{now:?}}}"#);
    let keys = format!(r#"{{"v":1,"acc":{{"{x}":{},"{y}":{}}}}}"#, key(&fb, "bob-mini"), key(&fa, "alice-mbp"));
    std::fs::write(d.at("cost-slots.json"), &cost).unwrap();
    std::fs::write(d.at("keys.json"), &keys).unwrap();
    let st1 = d.open();
    assert_eq!(st1.fps(), std::slice::from_ref(&fa), "照旧的根公钥：名单接着用");
    assert_eq!(d.read("cost-slots.json"), format!(r#"{{"v":1,"acc":{{"{x}":{{"{fa}":{}}}}}}}"#, slot("alice-mbp")));
    assert_eq!(
        d.read("keys.json"),
        format!(r#"{{"v":1,"acc":{{"{x}":{{"blob":{b},"at":{now:?}}},"{y}":{}}}}}"#, key(&fa, "alice-mbp"))
    );
    assert_eq!(
        (st1.cost.items.len(), st1.keys.items.get(&x).unwrap().fp.clone(), st1.keys.items.get(&y).unwrap().fp.clone()),
        (1, None, Some(fa))
    );
    let before = (d.mtime("cost-slots.json"), d.mtime("keys.json"), d.mtime("state.json"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    d.open();
    assert_eq!((d.mtime("cost-slots.json"), d.mtime("keys.json"), d.mtime("state.json")), before, "没什么要清的：不重写");
    assert!(said().is_empty());
}

/// 手改坏了的心跳、会话、API key 的密文：样子不对的那一项当它没有（Python 要到用到它的请求才出错），别的照常
#[test]
fn malformed_entries_are_dropped_at_load() {
    let d = Dir::new();
    let mut st = d.open();
    assert_eq!(enroll(&mut st, &[(dev(1), 1, "mac"), (dev(2), 2, "mini")], 5.0).0, 200);
    let raw = d.json("state.json");
    raw.set("beats", js(&format!(r#"{{"{}":7,"{}":{{"seen":"x","end":5}}}}"#, fp(1), fp(2))));
    let units = format!(
        r#"{{"{}":{{"{}":{{"cli":[[1,2,3]],"app":null}},"{}":{{"cli":"ab"}},"{}":{{"cli":[[5.5,6]],"app":[],"used":true}}}},"{}":[1]}}"#,
        fp(1),
        aid(1),
        aid(2),
        aid(3),
        fp(2)
    );
    raw.set("units", js(&units));
    d.put("state.json", &raw);
    std::fs::write(
        d.at("keys.json"),
        format!(
            r#"{{"v":1,"acc":{{"{}":{{"blob":{{"n":"x"}}}},"{}":7,"{}":{{"blob":{},"fp":5,"at":"x"}}}}}}"#,
            aid(1),
            aid(2),
            aid(3),
            blob(8, 1)
        ),
    )
    .unwrap();
    let st = d.open();
    assert!(said().is_empty());
    assert_eq!(st.beats.keys().collect::<Vec<_>>(), [fp(2).as_str()]);
    assert_eq!(st.beats.get(&fp(2)), Some(&Beat { seen: None, end: None }));
    let kept = st.units.get(&fp(1)).unwrap();
    assert_eq!(kept.keys().collect::<Vec<_>>(), [aid(3).as_str()]);
    assert_eq!(kept.get(&aid(3)), Some(&Unit { cli: vec![[Num::Float(5.5), Num::Int(6)]], app: None, used: None }));
    assert!(!st.units.contains(&fp(2)));
    assert_eq!(st.keys.items.keys().collect::<Vec<_>>(), [aid(3).as_str()]);
    let e = st.keys.items.get(&aid(3)).unwrap();
    assert_eq!((e.fp.clone(), e.name.clone(), e.at), (None, None, None));
}
