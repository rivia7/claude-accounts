//! remote 的单元测试：直接调 handle_op（不经网络、不签名，时间由用例给）。
//!
//! 开头两条是限额和记账、没用过的配对到期删掉（时间得由用例给，从外面测不了：黑盒的那一份 tests/remote_relay.rs 里没有）；后面各条把每个
//! op 成功和每一种不成功的回法、状态文件、访客、名单变了以后的清理、pair_get 的等待各走一遍。和 Python 逐项对照的在 replay.rs。

use super::*;
use crate::dict;
use crate::py::fs::TempDir;
use crate::py::re::Re;
use crate::server::{LATER_MAX, LATER_SPAN, PAIR_WAITERS, STATUS_BLOB};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) const NOW: f64 = 1794000000.0;
const OK: &str = r#"{"v":1,"ok":true}"#;
const BLOB: &str = r#"{"n":"AAAAAAAAAAAAAAAAAAAAAA==","c":"AAAA","t":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#;
const WEB: &str = "https://relay.example.com:8443/";

/// 第 i 台设备的指纹（样子对就行）
fn fp(i: u32) -> String {
    format!("SHA256:{i:043}")
}

fn hex(n: u32) -> String {
    format!("{n:032x}")
}

/// 一份合格的密文
fn blob() -> V {
    json::loads(BLOB).unwrap()
}

fn bad(code: u16, why: &str) -> (u16, String) {
    (code, format!(r#"{{"error":"{why}"}}"#))
}

/// 名单里的名字：编号的设备都叫 dev，别的（访客）不在名单里
fn name_of(fp: &str) -> Option<String> {
    fp.starts_with("SHA256:00000000").then(|| "dev".to_string())
}

/// 一把编出来的 Ed25519 公钥：指纹和公钥那一行
fn guest(seed: u8) -> (String, String) {
    let mut key = vec![0, 0, 0, 11];
    key.extend_from_slice(b"ssh-ed25519");
    key.extend_from_slice(&[0, 0, 0, 32]);
    key.extend_from_slice(&[seed; 32]);
    (sshsig::key_fp(&key), format!("ssh-ed25519 {}", base64::b64encode(&key)))
}

fn mtime(path: &str) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// 等到 f 成立（最多 5 秒）
fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(5), "等不到：{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 一个临时目录里的 Remote，和用例自己拨的时间
struct Rig {
    dir: TempDir,
    sh: Arc<Shared>,
    now: f64,
}

impl Rig {
    fn new() -> Rig {
        Rig::with(Limits::DEFAULT, None)
    }

    fn with(lim: Limits, web: Option<&str>) -> Rig {
        let dir = TempDir::new().unwrap();
        let path = format!("{}/remote.json", dir.path);
        let sh = Arc::new(Shared::new(Remote::open(Path::new(&path), 18444, web.map(str::to_string), lim)));
        Rig { dir, sh, now: NOW }
    }

    fn path(&self) -> String {
        format!("{}/remote.json", self.dir.path)
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }

    /// 重启：照文件重新读
    fn reopen(&mut self, web: Option<&str>) {
        let lim = self.sh.lock().lim;
        self.sh = Arc::new(Shared::new(Remote::open(Path::new(&self.path()), 18444, web.map(str::to_string), lim)));
    }

    /// 第 dev 台设备发一个请求（j 是这个 op 自己的那几项）：状态码和回的内容（紧凑的 JSON）
    fn op(&self, op: &str, dev: u32, j: Dict) -> (u16, String) {
        self.op_as(op, &fp(dev), j, None)
    }

    fn op_as(&self, op: &str, who: &str, j: Dict, gkey: Option<&str>) -> (u16, String) {
        let (st, v) = handle_op(&self.sh, op, who, &V::Dict(j), self.now, gkey, &name_of).unwrap();
        (st, json::dumps(&v, COMPACT))
    }

    /// 设备 a、b 配对，配对编号由 n 定：pair_commit 的结果和配对编号
    fn pair(&mut self, a: u32, b: u32, n: u32) -> ((u16, String), String) {
        let (rid, pid) = (hex(1000 + n), hex(n));
        let steps = [
            self.op("pair_open", a, dict! { "rid" => &rid }),
            self.op("pair_put", a, dict! { "rid" => &rid, "m" => blob() }),
            self.op("pair_put", b, dict! { "rid" => &rid, "m" => blob() }),
        ];
        for (st, body) in steps {
            assert_eq!(st, 200, "{body}");
        }
        assert_eq!(self.op("pair_commit", a, dict! { "rid" => &rid, "pid" => &pid }).0, 200);
        let done = self.op("pair_commit", b, dict! { "rid" => &rid, "pid" => &pid });
        self.now += 61.0; // 配好的那一次留一分钟就清掉：不占“同时开着的配对”的名额
        (done, pid)
    }

    /// 开一次配对，b 先放密文（坐下）
    fn seated(&self, a: u32, b: u32, rid: &str) {
        assert_eq!(self.op("pair_open", a, dict! { "rid" => rid }).0, 200);
        assert_eq!(self.op("pair_put", b, dict! { "rid" => rid, "m" => blob() }).0, 200);
    }

    fn edges(&self) -> Vec<String> {
        self.sh.lock().edges.keys().map(str::to_string).collect()
    }

    fn session(&self, sid: &str, dev: u32, pids: &[&str]) {
        self.sh.lock().test_session(sid, &fp(dev), pids, self.now);
    }
}

/// 票据的样子：编号 32 个十六进制字符、密钥 32 个字节的 base64，各项的先后
fn ticket_shape(body: &str) -> bool {
    Re::new(r#"^\{"v":1,"ok":true,"tid":"[0-9a-f]{32}","tkey":"[A-Za-z0-9+/]{43}=","port":18444,"ttl":30\}$"#).matches(body)
}

fn field(body: &str, k: &str) -> String {
    json::loads(body).unwrap().as_dict().unwrap().str(k).unwrap().to_string()
}

// ---------------------------------------------------------------- 限额和记账

#[test]
fn limits() {
    let mut t = Rig::new();
    // 同时开着的配对：一台设备 4 个
    for i in 0..4 {
        assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(i) }).0, 200);
    }
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(9) }), bad(429, "rate"));
    assert_eq!(t.op("pair_open", 2, dict! { "rid" => hex(9) }).0, 200, "别的设备不受影响");
    t.now += 601.0; // 过期了的不算
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(10) }).0, 200);
    // 一台设备最多和 16 台配对
    for i in 0..16 {
        assert_eq!(t.pair(100, 200 + i, i).0, (200, r#"{"v":1,"ok":true,"done":true}"#.to_string()));
    }
    assert_eq!(t.pair(100, 300, 99).0, bad(429, "edges"));
    assert_eq!(t.edges().len(), 16);
    // 没用掉的票据：128 张（3.7.10 起；Python 是 16）；过期了又能换
    let pid = hex(0);
    let (tickets, sessions) = (Limits::DEFAULT.tickets_dev as u32, Limits::DEFAULT.sessions_dev as u32);
    assert_eq!((tickets, sessions), (128, 101));
    for i in 0..tickets {
        assert_eq!(t.op("relay_host", 100, dict! { "sid" => hex(5000 + i), "pids" => vec![V::str(&pid)] }).0, 200);
    }
    assert_eq!(t.op("relay_host", 100, dict! { "sid" => hex(6000), "pids" => vec![V::str(&pid)] }), bad(429, "rate"));
    t.now += 31.0;
    assert_eq!(t.op("relay_host", 100, dict! { "sid" => hex(6000), "pids" => vec![V::str(&pid)] }).0, 200);
    // 开着的会话：101 个（3.7.10 起：100 个远程会话加上启动器；Python 是 8）；别人的会话编号不能拿来用；接入的满 4 处就不给票据
    for i in 0..sessions {
        t.session(&hex(7000 + i), 100, &[&pid]);
    }
    t.now += 31.0;
    assert_eq!(t.op("relay_host", 100, dict! { "sid" => hex(8000), "pids" => vec![V::str(&pid)] }), bad(429, "sessions"));
    let again = t.op("relay_host", 100, dict! { "sid" => hex(7000), "pids" => vec![V::str(&pid)] });
    assert_eq!(again.0, 200, "已经开着的会话换票据（重连）不算新的");
    assert_eq!(t.op("relay_host", 200, dict! { "sid" => hex(7000), "pids" => vec![V::str(&pid)] }), bad(409, "taken"));
    assert_eq!(t.op("relay_attach", 200, dict! { "sid" => hex(7000), "pid" => &pid }).0, 200);
    t.sh.lock().sessions.get_mut(&hex(7000)).unwrap().viewers = 4;
    assert_eq!(t.op("relay_attach", 200, dict! { "sid" => hex(7000), "pid" => &pid }), bad(429, "busy"));
}

#[test]
fn idle_pairings_are_dropped_but_used_ones_stay() {
    let mut t = Rig::new();
    let used = t.pair(1, 2, 1).1;
    let idle = t.pair(1, 3, 2).1;
    t.session(&hex(50), 1, &[&used]);
    // 用过（接入过）的记下时间、写进文件：一天最多写一次；重启以后还记得
    t.now += 100.0 * 86400.0;
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => hex(50), "pid" => &used }).0, 200);
    let saved = mtime(&t.path());
    let text = t.file();
    t.now += 3600.0;
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => hex(50), "pid" => &used }).0, 200);
    assert_eq!((mtime(&t.path()), t.file()), (saved, text), "一小时以后又用：不用再写一次盘");
    t.reopen(None);
    t.now += 100.0 * 86400.0; // 配对以后 200 天：没用过的那个删掉，100 天前用过的留着
    let (st, body) = t.op("relay_list", 1, dict! { "pids" => vec![V::str(&used), V::str(&idle)] });
    let peers = format!(r#""peers":{{"{used}":{{"fp":"{}","name":"dev"}}}}"#, fp(2));
    assert!(st == 200 && body.contains(&peers), "{body}");
    let doc = json::loads(&t.file()).unwrap().as_dict().unwrap();
    assert_eq!(doc.dict("edges").unwrap().keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), [used]);
}

// ---------------------------------------------------------------- 配对

#[test]
fn pairing_step_by_step() {
    let mut t = Rig::new();
    let (rid, pid) = (hex(7), hex(77));
    let none = r#"{"v":1,"ok":true,"m":null,"fp":null,"name":null,"done":false}"#;
    let with = |who: u32, done: bool| format!(r#"{{"v":1,"ok":true,"m":{BLOB},"fp":"{}","name":"dev","done":{done}}}"#, fp(who));
    // rid 不是 32 个十六进制字符：哪个 op 都是 bad
    for op in ["pair_open", "pair_put", "pair_get", "pair_commit", "pair_close"] {
        for j in [dict! {}, dict! { "rid" => 7 }, dict! { "rid" => "7".repeat(31) }, dict! { "rid" => "A".repeat(32) }] {
            assert_eq!(t.op(op, 1, j), bad(400, "bad"), "{op}");
        }
        if op != "pair_open" {
            assert_eq!(t.op(op, 1, dict! { "rid" => &rid, "m" => blob(), "pid" => &pid }), bad(404, "no_pairing"), "{op}");
        }
    }
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid }), (200, r#"{"v":1,"ok":true,"ttl":600}"#.to_string()));
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid }), bad(409, "taken"));
    assert_eq!(t.op("pair_open", 2, dict! { "rid" => &rid }), bad(409, "taken"));
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), (200, none.to_string()));
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => &pid }), bad(400, "bad"), "还没有人输入配对码");
    // 不是这次配对两边之一的：看不了、关不了、报不了
    for op in ["pair_get", "pair_close", "pair_commit"] {
        assert_eq!(t.op(op, 3, dict! { "rid" => &rid, "pid" => &pid }), bad(409, "taken"), "{op}");
    }
    // 头一个放密文进来的别的设备坐下：密文不合格也算坐下了，再来的别的设备进不来
    let long = dict! { "n" => "A".repeat(22) + "==", "c" => "A".repeat(2052), "t" => "A".repeat(43) + "=" };
    for m in [V::None, V::str("x"), V::Dict(dict! { "n" => "x" }), V::Dict(long)] {
        assert_eq!(t.op("pair_put", 2, dict! { "rid" => &rid, "m" => m }), bad(400, "bad"));
    }
    assert_eq!(t.op("pair_put", 3, dict! { "rid" => &rid, "m" => blob() }), bad(409, "taken"));
    assert_eq!(t.op("pair_put", 2, dict! { "rid" => &rid, "m" => blob() }), (200, OK.to_string()));
    assert_eq!(t.op("pair_put", 2, dict! { "rid" => &rid, "m" => blob() }), bad(400, "bad"), "一边只能放一次");
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), (200, with(2, false)));
    assert_eq!(t.op("pair_get", 2, dict! { "rid" => &rid }), (200, none.to_string()), "对方还没放");
    assert_eq!(t.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }), (200, OK.to_string()));
    assert_eq!(t.op("pair_get", 2, dict! { "rid" => &rid }), (200, with(1, false)));
    // 两边报的配对编号一样才记下
    for p in [V::None, V::str("xyz"), V::Int(1)] {
        assert_eq!(t.op("pair_commit", 2, dict! { "rid" => &rid, "pid" => p }), bad(400, "bad"));
    }
    let half = r#"{"v":1,"ok":true,"done":false}"#;
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => &pid }), (200, half.to_string()));
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "until" => "done" }), (200, with(2, false)));
    assert!(!Path::new(&t.path()).exists(), "还没配成：不写文件");
    let full = r#"{"v":1,"ok":true,"done":true}"#;
    assert_eq!(t.op("pair_commit", 2, dict! { "rid" => &rid, "pid" => &pid }), (200, full.to_string()));
    let want = format!(
        r#"{{"v":1,"edges":{{"{pid}":{{"a":"{}","b":"{}","at":1794000000.0,"used":1794000000.0}}}},"guests":{{}}}}"#,
        fp(1),
        fp(2)
    );
    assert_eq!(t.file(), want);
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "until" => "done" }), (200, with(2, true)));
    assert_eq!(t.op("pair_get", 2, dict! { "rid" => &rid }), (200, with(1, true)));
    // 配成以后再报一次：照旧回成了，不再写文件
    let saved = mtime(&t.path());
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => &pid }), (200, full.to_string()));
    assert_eq!(mtime(&t.path()), saved);
    // 配成的那一次只再留一分钟
    t.now += 60.0;
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }).0, 200);
    t.now += 0.5;
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    let list = format!(r#"{{"v":1,"ok":true,"peers":{{"{pid}":{{"fp":"{}","name":"dev"}}}},"sessions":{{"{pid}":[]}}}}"#, fp(2));
    assert_eq!(t.op("relay_list", 1, dict! { "pids" => vec![V::str(&pid)] }), (200, list));
    // Python 的正则里 $ 在结尾的换行前也算数：带着一个换行的编号也认（和 Python 一样）
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(8) + "\n" }).0, 200);
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(8) + "\n\n" }), bad(400, "bad"));
}

#[test]
fn pairing_expires_and_closes() {
    let mut t = Rig::new();
    let rid = hex(1);
    t.seated(1, 2, &rid);
    // 哪一边都可以中途不做了：这次配对作废
    assert_eq!(t.op("pair_close", 2, dict! { "rid" => &rid }), (200, OK.to_string()));
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    assert_eq!(t.op("pair_close", 1, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    // 同一个 rid 可以再开；10 分钟不配完就过期（正好到点还在）
    t.seated(1, 2, &rid);
    t.now += 600.0;
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }).0, 200);
    t.now += 0.001;
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    assert!(t.sh.lock().slots.is_empty() && t.sh.lock().closed.is_empty());
}

#[test]
fn pair_commit_refusals() {
    // 两边报的不一样：这次配对作废
    let t = Rig::new();
    let rid = hex(1);
    t.seated(1, 2, &rid);
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => hex(11) }).0, 200);
    assert_eq!(t.op("pair_commit", 2, dict! { "rid" => &rid, "pid" => hex(12) }), bad(409, "mismatch"));
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    assert!(t.edges().is_empty() && !Path::new(&t.path()).exists());

    // 配对编号已经有了（别的两台设备的）：也是 mismatch
    let mut t = Rig::new();
    assert_eq!(t.pair(1, 2, 5).0 .0, 200);
    assert_eq!(t.pair(3, 4, 5).0, bad(409, "mismatch"));
    assert_eq!(t.edges(), [hex(5)]);

    // 同两台设备重新配对：以前的那个换掉；开着的会话不再给旧的配对看
    t.session(&hex(60), 1, &[&hex(5)]);
    t.sh.lock().sessions.get_mut(&hex(60)).unwrap().blobs.insert(hex(5), wire::parse_blob(&blob(), 4096).unwrap());
    assert_eq!(t.pair(2, 1, 6).0 .0, 200);
    assert_eq!(t.edges(), [hex(6)]);
    {
        let r = t.sh.lock();
        let x = r.sessions.get(&hex(60)).unwrap();
        assert!(x.pids.is_empty() && x.blobs.is_empty());
        assert_eq!((r.edges.get(&hex(6)).unwrap().a.clone(), r.edges.get(&hex(6)).unwrap().b.clone()), (fp(2), fp(1)));
    }
    assert!(t.file().starts_with(&format!(r#"{{"v":1,"edges":{{"{}":{{"a":"{}","b":"{}","at":"#, hex(6), fp(2), fp(1))));

    // 一共只放得下这么多配对：满了回 full，这次配对留着（腾出地方以后再报一次就成）
    let mut t = Rig::with(Limits { edges_max: 1, ..Limits::DEFAULT }, None);
    assert_eq!(t.pair(1, 2, 1).0 .0, 200);
    let rid = hex(1000 + 2);
    assert_eq!(t.pair(3, 4, 2).0, bad(507, "full"));
    t.now -= 61.0;
    assert_eq!(t.op("pair_get", 3, dict! { "rid" => &rid }).0, 200);
    assert_eq!(t.op("pair_drop", 1, dict! { "pid" => hex(1) }), (200, OK.to_string()));
    assert_eq!(
        t.op("pair_commit", 3, dict! { "rid" => &rid, "pid" => hex(2) }),
        (200, r#"{"v":1,"ok":true,"done":true}"#.to_string())
    );
    assert_eq!(t.edges(), [hex(2)]);

    // 一台设备的配对数满了：连和已经配过的那台重新配对也不行（和 Python 一样，先数再换）
    let mut t = Rig::with(Limits { edges_dev: 1, ..Limits::DEFAULT }, None);
    assert_eq!(t.pair(1, 2, 1).0 .0, 200);
    assert_eq!(t.pair(1, 2, 2).0, bad(429, "edges"));
    assert_eq!(t.pair(3, 2, 3).0, bad(429, "edges"));
    assert_eq!(t.edges(), [hex(1)]);
    assert_eq!(t.op("pair_get", 3, dict! { "rid" => hex(1003) }), bad(404, "no_pairing"), "配不成的那一次作废了");
}

#[test]
fn pair_drop_answers() {
    let mut t = Rig::new();
    let pid = t.pair(1, 2, 1).1;
    for j in [dict! {}, dict! { "pid" => 5 }, dict! { "pid" => "zz" }] {
        assert_eq!(t.op("pair_drop", 1, j), bad(400, "bad"));
    }
    // 不是自己的、没有的：照样回成功，什么都不动
    let saved = (mtime(&t.path()), t.file());
    assert_eq!(t.op("pair_drop", 3, dict! { "pid" => &pid }), (200, OK.to_string()));
    assert_eq!(t.op("pair_drop", 1, dict! { "pid" => hex(999) }), (200, OK.to_string()));
    assert_eq!((mtime(&t.path()), t.file()), saved);
    // 输入配对码的那一边也可以取消
    assert_eq!(t.op("pair_drop", 2, dict! { "pid" => &pid }), (200, OK.to_string()));
    assert_eq!(t.file(), r#"{"v":1,"edges":{},"guests":{}}"#);
}

// ---------------------------------------------------------------- 票据、会话

#[test]
fn relay_host_answers() {
    let mut t = Rig::new();
    let (p1, p2) = (t.pair(1, 2, 1).1, t.pair(1, 3, 2).1);
    let other = t.pair(4, 5, 3).1;
    let sid = hex(50);
    let many: Vec<V> = (0..17).map(|i| V::str(hex(i))).collect();
    for j in [
        dict! { "pids" => vec![V::str(&p1)] },
        dict! { "sid" => "x", "pids" => vec![V::str(&p1)] },
        dict! { "sid" => &sid },
        dict! { "sid" => &sid, "pids" => &p1 },
        dict! { "sid" => &sid, "pids" => Vec::<V>::new() },
        dict! { "sid" => &sid, "pids" => many },
        dict! { "sid" => &sid, "pids" => vec![V::str(&p1), V::Int(3)] },
        dict! { "sid" => &sid, "pids" => vec![V::str(&p1), V::str("xyz")] },
    ] {
        assert_eq!(t.op("relay_host", 1, j), bad(400, "bad"));
    }
    // 不是自己的配对、没有的配对：一个都不剩就是 no_pairing
    assert_eq!(
        t.op("relay_host", 1, dict! { "sid" => &sid, "pids" => vec![V::str(&other), V::str(hex(99))] }),
        bad(404, "no_pairing")
    );
    // 重复的只算一次，不是自己的去掉，先后照请求里的
    let pids = vec![V::str(&p2), V::str(&other), V::str(&p1), V::str(&p2)];
    let (st, body) = t.op("relay_host", 1, dict! { "sid" => &sid, "pids" => pids });
    assert!(st == 200 && ticket_shape(&body), "{body}");
    let r = t.sh.lock();
    let k = r.tickets.get(&field(&body, "tid")).unwrap();
    assert_eq!(
        (k.role, k.fp.as_str(), k.sid.as_str(), &k.pids, &k.pid),
        (Role::Host, fp(1).as_str(), sid.as_str(), &vec![p2, p1], &None)
    );
    assert_eq!((k.exp, k.ws, base64::b64encode(&k.key)), (t.now + 30.0, false, field(&body, "tkey")));
}

#[test]
fn relay_attach_answers() {
    let mut t = Rig::new();
    let (p1, other) = (t.pair(1, 2, 1).1, t.pair(3, 4, 2).1);
    let sid = hex(50);
    t.session(&sid, 1, &[&p1]);
    for j in [
        dict! {},
        dict! { "sid" => &sid },
        dict! { "pid" => &p1 },
        dict! { "sid" => "x", "pid" => &p1 },
        dict! { "sid" => &sid, "pid" => 1 },
    ] {
        assert_eq!(t.op("relay_attach", 2, j), bad(400, "bad"));
    }
    // 不是这个配对里的设备；没有这个会话；会话不给这个配对看；会话不是配对的另一头开的（宿主自己、别的设备）
    assert_eq!(t.op("relay_attach", 3, dict! { "sid" => &sid, "pid" => &p1 }), bad(404, "no_session"));
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => hex(51), "pid" => &p1 }), bad(404, "no_session"));
    assert_eq!(t.op("relay_attach", 3, dict! { "sid" => &sid, "pid" => &other }), bad(404, "no_session"));
    assert_eq!(t.op("relay_attach", 1, dict! { "sid" => &sid, "pid" => &p1 }), bad(404, "no_session"));
    t.session(&hex(52), 3, &[&p1]);
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => hex(52), "pid" => &p1 }), bad(404, "no_session"));
    // 成了：接入用的票据
    let (st, body) = t.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &p1 });
    assert!(st == 200 && ticket_shape(&body), "{body}");
    {
        let r = t.sh.lock();
        let k = r.tickets.get(&field(&body, "tid")).unwrap();
        assert_eq!(
            (k.role, k.fp.as_str(), k.sid.as_str(), k.pids.len(), &k.pid),
            (Role::View, fp(2).as_str(), sid.as_str(), 0, &Some(p1.clone()))
        );
    }
    // 手里没用掉的票据太多：rate；满 4 处接入：busy
    for _ in 1..Limits::DEFAULT.tickets_dev {
        assert_eq!(t.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &p1 }).0, 200);
    }
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &p1 }), bad(429, "rate"));
    t.sh.lock().sessions.get_mut(&sid).unwrap().viewers = 4;
    assert_eq!(t.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &p1 }), bad(429, "busy"));
}

#[test]
fn relay_list_rows() {
    let mut t = Rig::new();
    let (p1, p2) = (t.pair(1, 2, 1).1, t.pair(1, 3, 2).1);
    let other = t.pair(4, 5, 3).1;
    let many: Vec<V> = (0..17).map(|i| V::str(hex(i))).collect();
    for j in [dict! {}, dict! { "pids" => &p1 }, dict! { "pids" => many }, dict! { "pids" => vec![V::str("x")] }] {
        assert_eq!(t.op("relay_list", 1, j), bad(400, "bad"));
    }
    assert_eq!(
        t.op("relay_list", 1, dict! { "pids" => Vec::<V>::new() }),
        (200, r#"{"v":1,"ok":true,"peers":{},"sessions":{}}"#.to_string())
    );
    // 设备 2 开着三个会话：两个给 p1 看（一个控制连接断了、接着两处），一个还没有说明；设备 3 的一个给 p2 看；设备 1 自己的不算
    let seal = wire::parse_blob(&blob(), 4096).unwrap();
    {
        let mut r = t.sh.lock();
        for (sid, dev, pid, ago) in [(60, 2, &p1, 0.5), (61, 2, &p1, 1.5), (62, 2, &p1, 2.5), (63, 3, &p2, -5.0), (64, 1, &p1, 9.0)]
        {
            r.test_session(&hex(sid), &fp(dev), &[pid], t.now - ago);
            if sid != 62 {
                r.sessions.get_mut(&hex(sid)).unwrap().blobs.insert(pid.clone(), seal.clone());
            }
        }
        let x = r.sessions.get_mut(&hex(61)).unwrap();
        (x.conn, x.viewers) = (None, 2);
    }
    let row = |sid: u32, age: u32, viewers: u32, online: bool| {
        format!(r#"{{"sid":"{}","blob":{BLOB},"age":{age},"viewers":{viewers},"online":{online}}}"#, hex(sid))
    };
    // 重复的、不是自己的配对、没有的配对：不在回的里面；先后照请求里的；age 取整（Python 的 round：0.5 → 0，1.5 → 2），不为负
    let pids = vec![V::str(&p2), V::str(&other), V::str(&p1), V::str(hex(99)), V::str(&p2)];
    let want = format!(
        r#"{{"v":1,"ok":true,"peers":{{"{p2}":{{"fp":"{}","name":"dev"}},"{p1}":{{"fp":"{}","name":"dev"}}}},"sessions":{{"{p2}":[{}],"{p1}":[{},{}]}}}}"#,
        fp(3),
        fp(2),
        row(63, 0, 0, true),
        row(60, 0, 0, true),
        row(61, 2, 2, false),
    );
    assert_eq!(t.op("relay_list", 1, dict! { "pids" => pids }), (200, want));
    // 另一头看：设备 1 的那个会话
    let want = format!(
        r#"{{"v":1,"ok":true,"peers":{{"{p1}":{{"fp":"{}","name":"dev"}}}},"sessions":{{"{p1}":[{}]}}}}"#,
        fp(1),
        row(64, 9, 0, true)
    );
    assert_eq!(t.op("relay_list", 2, dict! { "pids" => vec![V::str(&p1)] }), (200, want));
}

#[test]
fn tickets_are_single_use_and_need_proof() {
    let mut t = Rig::new();
    let pid = t.pair(1, 2, 1).1;
    let sid = hex(50);
    let host = t.op("relay_host", 1, dict! { "sid" => &sid, "pids" => vec![V::str(&pid)] }).1;
    let (tid, key) = (field(&host, "tid"), base64::b64decode(&field(&host, "tkey"), true).unwrap());
    t.session(&sid, 1, &[&pid]);
    let view = field(&t.op("relay_attach", 2, dict! { "sid" => &sid, "pid" => &pid }).1, "tid");
    let mut r = t.sh.lock();
    // 网页端开 WebSocket：只认接入用的票据，一张只开得了一条；过期的、没有的不认
    assert!(!r.ws_claim(&tid, t.now) && !r.ws_claim(&hex(1), t.now) && !r.ws_claim("", t.now));
    assert!(!r.ws_claim(&view, t.now + 30.001));
    assert!(r.ws_claim(&view, t.now + 30.0) && !r.ws_claim(&view, t.now));
    // 角色不对、证明不了有密钥、过期了：都不算用掉
    assert!(r.take_ticket(&tid, Role::View, t.now, |_| true).is_none());
    assert!(r.take_ticket(&tid, Role::Host, t.now, |_| false).is_none());
    assert!(r.take_ticket(&tid, Role::Host, t.now + 30.001, |_| true).is_none());
    assert!(r.take_ticket(&hex(1), Role::Host, t.now, |_| true).is_none());
    let k = r.take_ticket(&tid, Role::Host, t.now + 30.0, |k| k == key.as_slice()).unwrap();
    assert_eq!((k.sid, k.pids, k.key), (sid, vec![pid], key));
    assert!(r.take_ticket(&tid, Role::Host, t.now, |_| true).is_none(), "只能用一次");
    // 开过 WebSocket 的票据照样要在开场白里证明有密钥才算用掉
    assert!(r.take_ticket(&view, Role::View, t.now, |_| true).is_some());
    assert!(r.tickets.is_empty());
}

#[test]
fn old_tickets_slots_and_idle_pairings_are_swept() {
    let mut t = Rig::new();
    let pid = t.pair(1, 2, 1).1;
    t.now = NOW;
    for i in 0..3 {
        assert_eq!(t.op("relay_host", 1, dict! { "sid" => hex(50 + i), "pids" => vec![V::str(&pid)] }).0, 200);
    }
    assert_eq!(t.sh.lock().tickets.len(), 3);
    t.now += 30.0;
    t.op("relay_list", 1, dict! { "pids" => Vec::<V>::new() });
    assert_eq!(t.sh.lock().tickets.len(), 3, "正好到点的还在");
    t.now += 0.001;
    t.op("nothing", 1, dict! {});
    assert_eq!(t.sh.lock().tickets.len(), 0, "不认识的 op 也先清一遍过期的");
    // 180 天没用过的配对删掉（正好 180 天的还在），文件跟着改
    t.now = NOW + 180.0 * 86400.0;
    t.op("relay_list", 1, dict! { "pids" => Vec::<V>::new() });
    assert_eq!(t.edges(), [pid]);
    t.now += 1.0;
    t.op("relay_list", 1, dict! { "pids" => Vec::<V>::new() });
    assert!(t.edges().is_empty());
    assert_eq!(t.file(), r#"{"v":1,"edges":{},"guests":{}}"#);
}

#[test]
fn unknown_ops() {
    let t = Rig::new();
    assert_eq!(t.op("nothing", 1, dict! {}), bad(404, "not_found"));
    assert_eq!(t.op("report", 1, dict! {}), bad(404, "not_found"));
    // pair_ 开头的别的 op：和 Python 一样先过配对的那几道，再是 not_found
    assert_eq!(t.op("pair_x", 1, dict! {}), bad(400, "bad"));
    assert_eq!(t.op("pair_x", 1, dict! { "rid" => hex(1) }), bad(404, "no_pairing"));
    t.seated(1, 2, &hex(1));
    assert_eq!(t.op("pair_x", 3, dict! { "rid" => hex(1) }), bad(409, "taken"));
    assert_eq!(t.op("pair_x", 1, dict! { "rid" => hex(1) }), bad(404, "not_found"));
    // 请求的内容不是字典（处理请求的那一头查过，到不了）：当它什么都没带
    let (st, v) = handle_op(&t.sh, "pair_open", &fp(1), &V::Int(1), NOW, None, &name_of).unwrap();
    assert_eq!((st, json::dumps(&v, COMPACT)), bad(400, "bad"));
}

// ---------------------------------------------------------------- 访客（手机上的网页端）

#[test]
fn guests() {
    let mut t = Rig::with(Limits::DEFAULT, Some(WEB));
    let ((g, gpub), (g2, g2pub)) = (guest(1), guest(2));
    let (rid, pid) = (hex(1), hex(11));
    let key = |t: &Rig, who: &str, op: &str, j: Dict| t.sh.lock().guest_key(who, op, &V::Dict(j), t.now);
    // 开给网页端的配对：回的里面带着网页端的地址
    let open = format!(r#"{{"v":1,"ok":true,"ttl":600,"web":"{WEB}"}}"#);
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid, "web" => true }), (200, open));
    // 拿着配对码来的（知道 rid）：只有 pair_put、带着自己的公钥、指纹对得上才认
    assert_eq!(key(&t, &g, "pair_put", dict! { "rid" => &rid, "pub" => &gpub }), Some(gpub.clone()));
    assert_eq!(
        key(&t, &g, "pair_put", dict! { "rid" => &rid, "pub" => format!("{gpub}\n") }),
        Some(gpub.clone()),
        "结尾的换行不带进去"
    );
    for j in [
        dict! { "rid" => &rid },
        dict! { "rid" => &rid, "pub" => &g2pub },
        dict! { "rid" => &rid, "pub" => 5 },
        dict! { "rid" => &rid, "pub" => format!("{gpub} phone") },
        dict! { "rid" => hex(2), "pub" => &gpub },
        dict! { "rid" => 1, "pub" => &gpub },
        dict! { "pub" => &gpub },
    ] {
        assert_eq!(key(&t, &g, "pair_put", j), None);
    }
    for op in
        ["pair_get", "pair_commit", "pair_close", "pair_drop", "relay_attach", "relay_list", "pair_open", "relay_host", "report"]
    {
        assert_eq!(key(&t, &g, op, dict! { "rid" => &rid, "pub" => &gpub }), None, "{op}");
    }
    assert_eq!(
        t.sh.lock().guest_key(&g, "pair_put", &V::Dict(dict! { "rid" => &rid, "pub" => &gpub }), t.now + 600.0),
        Some(gpub.clone())
    );
    assert_eq!(
        t.sh.lock().guest_key(&g, "pair_put", &V::Dict(dict! { "rid" => &rid, "pub" => &gpub }), t.now + 600.5),
        None,
        "过期了"
    );
    assert_eq!(t.sh.lock().guest_key(&g, "pair_put", &V::Int(1), t.now), None);
    // 不是开给网页端的配对：访客进不去（不认；就算认了它，op 里也挡着）
    t.seated(3, 4, &hex(3));
    assert_eq!(
        t.op("pair_open", 2, dict! { "rid" => hex(2), "web" => 1 }),
        (200, r#"{"v":1,"ok":true,"ttl":600}"#.to_string()),
        "web 要是 true"
    );
    assert_eq!(key(&t, &g, "pair_put", dict! { "rid" => hex(2), "pub" => &gpub }), None);
    assert_eq!(t.op_as("pair_put", &g, dict! { "rid" => hex(2), "m" => blob() }, Some(&gpub)), bad(403, "denied"));
    assert_eq!(t.op_as("pair_get", &g, dict! { "rid" => hex(3) }, Some(&gpub)), bad(403, "denied"));
    // 访客能发的只有那几个 op
    for op in ["pair_open", "relay_host", "report", "nothing"] {
        assert_eq!(t.op_as(op, &g, dict! { "rid" => hex(9), "web" => true }, Some(&gpub)), bad(403, "denied"), "{op}");
    }
    // 放密文进来（坐下）：之后它的别的请求认它，第二个访客进不来
    assert_eq!(t.op_as("pair_put", &g, dict! { "rid" => &rid, "m" => blob() }, Some(&gpub)), (200, OK.to_string()));
    assert_eq!(key(&t, &g, "pair_get", dict! { "rid" => &rid }), Some(gpub.clone()));
    assert_eq!(key(&t, &g, "pair_commit", dict! { "rid" => &rid }), Some(gpub.clone()));
    assert_eq!(key(&t, &g, "relay_list", dict! {}), None, "还没配成：别的 rid、别的请求不认");
    assert_eq!(key(&t, &g2, "pair_put", dict! { "rid" => &rid, "pub" => &g2pub }), None, "已经有人输入了");
    assert_eq!(t.op_as("pair_put", &g2, dict! { "rid" => &rid, "m" => blob() }, Some(&g2pub)), bad(409, "taken"));
    // 显示配对码的那一边看到它：不在名单里，没有名字
    let seen = format!(r#"{{"v":1,"ok":true,"m":{BLOB},"fp":"{g}","name":null,"done":false}}"#);
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid }), (200, seen));
    assert_eq!(t.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    assert_eq!(t.op_as("pair_commit", &g, dict! { "rid" => &rid, "pid" => &pid }, Some(&gpub)).0, 200);
    assert!(t.sh.lock().guests.is_empty(), "还没配成：不记");
    assert_eq!(
        t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => &pid }),
        (200, r#"{"v":1,"ok":true,"done":true}"#.to_string())
    );
    let want = format!(
        r#"{{"v":1,"edges":{{"{pid}":{{"a":"{}","b":"{g}","at":1794000000.0,"used":1794000000.0}}}},"guests":{{"{g}":{{"key":"{gpub}","at":1794000000.0}}}}}}"#,
        fp(1)
    );
    assert_eq!(t.file(), want);
    // 配成了：它的那几个 op 都认（不用再带公钥），别的照旧不认
    for op in GUEST_OPS {
        assert_eq!(key(&t, &g, op, dict! {}), Some(gpub.clone()), "{op}");
    }
    assert_eq!(key(&t, &g, "relay_host", dict! {}), None);
    let list = format!(r#"{{"v":1,"ok":true,"peers":{{"{pid}":{{"fp":"{}","name":"dev"}}}},"sessions":{{"{pid}":[]}}}}"#, fp(1));
    assert_eq!(t.op_as("relay_list", &g, dict! { "pids" => vec![V::str(&pid)] }, Some(&gpub)), (200, list));
    let list = format!(r#"{{"v":1,"ok":true,"peers":{{"{pid}":{{"fp":"{g}","name":null}}}},"sessions":{{"{pid}":[]}}}}"#);
    assert_eq!(t.op("relay_list", 1, dict! { "pids" => vec![V::str(&pid)] }), (200, list));
    // 名单里的设备也可以输入开给网页端的配对码：它不是访客，不记进 guests
    t.now += 61.0;
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(5), "web" => true }).0, 200);
    assert_eq!(t.op("pair_put", 2, dict! { "rid" => hex(5), "m" => blob() }).0, 200);
    for dev in [1, 2] {
        assert_eq!(t.op("pair_commit", dev, dict! { "rid" => hex(5), "pid" => hex(55) }).0, 200);
    }
    assert_eq!(t.sh.lock().guests.keys().collect::<Vec<_>>(), [g.as_str()]);
    // 关掉网页端重启：访客照旧（iOS 上的 App 不要网页端），旧版 cca 开给网页端的配对开不了
    t.reopen(None);
    assert_eq!(key(&t, &g, "relay_list", dict! {}), Some(gpub.clone()));
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(6), "web" => true }), bad(404, "no_web"));
    t.reopen(Some(""));
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(6), "web" => true }), bad(404, "no_web"));
    // 不认访客（--no-guests）：访客的请求一律不认，给手机的配对开不了，记录留着；再认，它还在
    t.sh.lock().guests_on = false;
    assert_eq!(key(&t, &g, "relay_list", dict! {}), None);
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(6), "guest" => true }), bad(404, "no_guest"));
    t.reopen(Some(WEB));
    t.sh.lock().guests_on = false;
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => hex(6), "web" => true }), bad(404, "no_web"));
    assert_eq!(t.sh.lock().guests.len(), 1);
    t.reopen(Some(WEB));
    assert_eq!(key(&t, &g, "relay_list", dict! {}), Some(gpub.clone()));
    // 访客手里有票据；它最后一个配对取消了：它和它的票据都不留
    t.session(&hex(70), 1, &[&pid]);
    assert_eq!(t.op_as("relay_attach", &g, dict! { "sid" => hex(70), "pid" => &pid }, Some(&gpub)).0, 200);
    assert_eq!(t.op("relay_host", 1, dict! { "sid" => hex(70), "pids" => vec![V::str(&pid)] }).0, 200);
    assert_eq!(t.sh.lock().tickets.len(), 2);
    assert_eq!(t.op_as("pair_drop", &g, dict! { "pid" => &pid }, Some(&gpub)), (200, OK.to_string()));
    {
        let r = t.sh.lock();
        assert!(r.guests.is_empty());
        assert_eq!(r.tickets.values().map(|k| k.fp.clone()).collect::<Vec<_>>(), [fp(1)]);
        assert!(r.sessions.get(&hex(70)).unwrap().pids.is_empty());
    }
    assert_eq!(key(&t, &g, "relay_list", dict! {}), None);
    let doc = json::loads(&t.file()).unwrap().as_dict().unwrap();
    assert!(doc.dict("guests").unwrap().is_empty() && doc.dict("edges").unwrap().len() == 1);
}

/// iOS 上的 App：没开网页端也行，配对开给手机（pair_open 带 guest），回的里面带 guest
#[test]
fn an_app_guest_without_the_web_side() {
    let t = Rig::with(Limits::DEFAULT, None);
    let (g, gpub) = guest(1);
    let (rid, pid) = (hex(1), hex(11));
    let key = |t: &Rig, who: &str, op: &str, j: Dict| t.sh.lock().guest_key(who, op, &V::Dict(j), t.now);
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid, "web" => true }), bad(404, "no_web"), "旧版 cca 要网页端的地址");
    let open = r#"{"v":1,"ok":true,"ttl":600,"guest":true}"#.to_string();
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid, "guest" => true }), (200, open));
    assert_eq!(key(&t, &g, "pair_put", dict! { "rid" => &rid, "pub" => &gpub }), Some(gpub.clone()));
    // guest 要是 true；普通的配对访客进不来
    let plain = (200, r#"{"v":1,"ok":true,"ttl":600}"#.to_string());
    assert_eq!(t.op("pair_open", 2, dict! { "rid" => hex(2), "guest" => 1 }), plain);
    assert_eq!(key(&t, &g, "pair_put", dict! { "rid" => hex(2), "pub" => &gpub }), None);
    assert_eq!(t.op_as("pair_put", &g, dict! { "rid" => hex(2), "m" => blob() }, Some(&gpub)), bad(403, "denied"));
    // 配成：记进 guests，之后它的请求认它
    assert_eq!(t.op_as("pair_put", &g, dict! { "rid" => &rid, "m" => blob() }, Some(&gpub)).0, 200);
    assert_eq!(t.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    assert_eq!(t.op_as("pair_commit", &g, dict! { "rid" => &rid, "pid" => &pid }, Some(&gpub)).0, 200);
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => &pid }).0, 200);
    assert_eq!(t.sh.lock().guests.keys().collect::<Vec<_>>(), [g.as_str()]);
    assert_eq!(key(&t, &g, "relay_list", dict! {}), Some(gpub.clone()));
}

#[test]
fn a_guest_pairs_again_and_keeps_its_place() {
    let mut t = Rig::with(Limits::DEFAULT, Some(WEB));
    let ((g, gpub), (g2, g2pub)) = (guest(1), guest(2));
    let pair = |t: &mut Rig, dev: u32, who: &str, key: &str, n: u32| {
        let (rid, pid) = (hex(1000 + n), hex(n));
        assert_eq!(t.op("pair_open", dev, dict! { "rid" => &rid, "web" => true }).0, 200);
        assert_eq!(t.op_as("pair_put", who, dict! { "rid" => &rid, "m" => blob() }, Some(key)).0, 200);
        assert_eq!(t.op("pair_commit", dev, dict! { "rid" => &rid, "pid" => &pid }).0, 200);
        assert_eq!(t.op_as("pair_commit", who, dict! { "rid" => &rid, "pid" => &pid }, Some(key)).0, 200);
        t.now += 61.0;
    };
    pair(&mut t, 1, &g, &gpub, 1);
    pair(&mut t, 1, &g2, &g2pub, 2);
    // 已经是访客的再和别的设备配对：记录留在原来的位置，时间换成新的
    pair(&mut t, 2, &g, &gpub, 3);
    {
        let r = t.sh.lock();
        assert_eq!(r.guests.keys().collect::<Vec<_>>(), [g.as_str(), g2.as_str()]);
        assert_eq!(r.guests.get(&g).unwrap().at, Val::Num(Num::Float(NOW + 122.0)));
    }
    // 和同一台设备重新配对：旧的配对先删（访客跟着没了），新的记在最后
    pair(&mut t, 1, &g2, &g2pub, 4);
    let r = t.sh.lock();
    assert_eq!(r.edges.keys().collect::<Vec<_>>(), [hex(1), hex(3), hex(4)]);
    assert_eq!(r.guests.keys().collect::<Vec<_>>(), [g.as_str(), g2.as_str()]);
    drop(r);
    pair(&mut t, 1, &g, &gpub, 5);
    let r = t.sh.lock();
    assert_eq!(r.edges.keys().collect::<Vec<_>>(), [hex(3), hex(4), hex(5)]);
    assert_eq!(r.guests.keys().collect::<Vec<_>>(), [g.as_str(), g2.as_str()], "还有别的配对的访客不动");
}

// ---------------------------------------------------------------- 名单变了

#[test]
fn keep_only_drops_what_belongs_to_removed_devices() {
    let mut t = Rig::with(Limits::DEFAULT, Some(WEB));
    let (g, gpub) = guest(1);
    let (p12, p13, p45) = (t.pair(1, 2, 1).1, t.pair(1, 3, 2).1, t.pair(4, 5, 3).1);
    // 设备 3 领进来一个访客
    let (rid, pg) = (hex(9), hex(4));
    assert_eq!(t.op("pair_open", 3, dict! { "rid" => &rid, "web" => true }).0, 200);
    assert_eq!(t.op_as("pair_put", &g, dict! { "rid" => &rid, "m" => blob() }, Some(&gpub)).0, 200);
    assert_eq!(t.op("pair_commit", 3, dict! { "rid" => &rid, "pid" => &pg }).0, 200);
    assert_eq!(t.op_as("pair_commit", &g, dict! { "rid" => &rid, "pid" => &pg }, Some(&gpub)).0, 200);
    // 各有会话、票据
    t.session(&hex(60), 1, &[&p12, &p13]);
    t.session(&hex(61), 3, &[&p13, &pg]);
    t.sh.lock().sessions.get_mut(&hex(60)).unwrap().blobs.insert(p13.clone(), wire::parse_blob(&blob(), 4096).unwrap());
    assert_eq!(t.op("relay_host", 1, dict! { "sid" => hex(60), "pids" => vec![V::str(&p12)] }).0, 200);
    assert_eq!(t.op("relay_host", 3, dict! { "sid" => hex(61), "pids" => vec![V::str(&p13)] }).0, 200);
    assert_eq!(t.op_as("relay_attach", &g, dict! { "sid" => hex(61), "pid" => &pg }, Some(&gpub)).0, 200);
    assert_eq!(t.op("relay_attach", 3, dict! { "sid" => hex(60), "pid" => &p13 }).0, 200);
    // 名单没变：什么都不动，不写文件
    let all: Vec<String> = (1..=5).map(fp).collect();
    let saved = (mtime(&t.path()), t.file());
    t.sh.lock().keep_only(&all, t.now).unwrap();
    assert_eq!((mtime(&t.path()), t.file()), saved);
    {
        let r = t.sh.lock();
        assert_eq!((r.edges.len(), r.tickets.len(), r.sessions.len(), r.guests.len()), (4, 4, 2, 1));
    }
    // 设备 3 被移除：它的配对、票据、会话都没了；它领进来的访客跟着没了；别人的会话不再给那个配对看
    let left: Vec<String> = [1, 2, 4, 5].into_iter().map(fp).collect();
    t.sh.lock().keep_only(&left, t.now).unwrap();
    assert_eq!(t.edges(), [p12.clone(), p45.clone()]);
    {
        let r = t.sh.lock();
        assert!(r.guests.is_empty());
        assert_eq!(r.tickets.values().map(|k| k.fp.clone()).collect::<Vec<_>>(), [fp(1)]);
        assert_eq!(r.sessions.keys().collect::<Vec<_>>(), [hex(60)]);
        let x = r.sessions.get(&hex(60)).unwrap();
        assert!(x.pids == [p12.clone()] && x.blobs.is_empty());
    }
    let doc = json::loads(&t.file()).unwrap().as_dict().unwrap();
    assert_eq!(doc.dict("edges").unwrap().keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), [p12, p45.clone()]);
    assert!(doc.dict("guests").unwrap().is_empty());
    // 没有配对的设备被移除：它的票据、会话没了，文件不用写
    t.session(&hex(62), 9, &[]);
    let saved = (mtime(&t.path()), t.file());
    t.sh.lock().keep_only(&left, t.now).unwrap();
    assert_eq!((mtime(&t.path()), t.file()), saved);
    assert_eq!(t.sh.lock().sessions.len(), 1);
    // 一台都不剩
    t.sh.lock().keep_only(&[], t.now).unwrap();
    assert_eq!(t.file(), r#"{"v":1,"edges":{},"guests":{}}"#);
    assert!(t.sh.lock().tickets.is_empty() && t.sh.lock().sessions.is_empty());
}

// ---------------------------------------------------------------- 状态文件

/// 照文件打开，收着日志里说的话
fn open_file(path: &str, web: Option<&str>) -> (Remote, Vec<String>) {
    let mut said = Vec::new();
    let r = Remote::open_saying(Path::new(path), 18444, web.map(str::to_string), Limits::DEFAULT, &mut |line| said.push(line));
    (r, said)
}

#[test]
fn state_file_missing_or_unreadable_starts_empty() {
    let dir = TempDir::new().unwrap();
    let path = format!("{}/remote.json", dir.path);
    // 没有：不说话
    let (r, said) = open_file(&path, None);
    assert!(r.edges() == 0 && r.port() == 18444 && said.is_empty());
    // 样子不对（不是字典、edges 不是字典）：也不说话，从空的开始
    for text in ["[]", "3", r#"{"v":1}"#, r#"{"edges":[]}"#, r#"{"edges":null,"guests":{}}"#] {
        std::fs::write(&path, text).unwrap();
        let (r, said) = open_file(&path, None);
        assert!(r.edges() == 0 && said.is_empty(), "{text}");
    }
    // 读不了、格式不对：说一声为什么（和 Python 那一句一样）
    for (raw, why) in [
        (&b"{\"edges\":"[..], "Expecting value: line 1 column 10 (char 9)"),
        (b"", "Expecting value: line 1 column 1 (char 0)"),
        (b"\xff{}", "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"),
        (b"\xef\xbb\xbf{}", "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"),
    ] {
        std::fs::write(&path, raw).unwrap();
        let (r, said) = open_file(&path, None);
        assert_eq!((r.edges(), said), (0, vec![format!("cca-presence: {path} 读不了（{why}），从空的开始")]));
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let (r, said) = open_file(&path, None);
    assert_eq!(
        (r.edges(), said),
        (0, vec![format!("cca-presence: {path} 读不了（[Errno 21] Is a directory: '{path}'），从空的开始")])
    );
}

#[test]
fn state_file_keeps_what_python_would_keep() {
    let dir = TempDir::new().unwrap();
    let path = format!("{}/remote.json", dir.path);
    let ((g, gpub), (g2, g2pub), (g3, g3pub)) = (guest(1), guest(2), guest(3));
    let (a, b, c) = (fp(1), fp(2), fp(3));
    // 一份 Python 写的、又被人改过的文件：前后多了不认识的键，时间有整数的，配对里多了一项、少了 used；样子不对的配对和访客
    let (p1, p2, p3, p4, p5, p6) = (hex(1), hex(2), hex(3), hex(4), hex(5), hex(6));
    let text = [
        r#"{"note":"手改的","v":2,"edges":{"#.to_string(),
        format!(r#""{p1}":{{"a":"{a}","b":"{b}","at":1101,"used":1794000000.5}},"#),
        format!(r#""{p2}":{{"b":"{g}","tag":[1,{{"x":null}}],"a":"{a}","at":1.79e9}},"#),
        format!(r#""{p3}":{{"a":"{c}","b":"{g2}","at":null,"used":1793000000}},"#),
        format!(r#""short":{{"a":"{a}","b":"{b}","at":5,"used":5}},"#),
        format!(r#""{p4}":{{"a":"{a}","b":"nobody","at":5,"used":5}},"#),
        format!(r#""{p5}":{{"a":"{a}","at":5,"used":5}},"#),
        format!(r#""{p6}":"x""#),
        r#"},"guests":{"#.to_string(),
        format!(r#""{g2}":{{"key":"{g2pub}","more":1}},"#),
        format!(r#""{g}":{{"key":"{gpub}","at":7}},"#),
        format!(r#""{g3}":{{"key":"{g3pub}","at":8}},"#),
        format!(r#""{c}":{{"key":"{gpub}","at":9}},"#),
        format!(r#""bad":{{"key":"{gpub}"}},"#),
        format!(r#""{b}":"x""#),
        r#"},"seen":{"k":[1.0,2,"三"]},"big":123456789012345678901234567890}"#.to_string(),
    ]
    .concat();
    std::fs::write(&path, &text).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let (r, said) = open_file(&path, Some(WEB));
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(r.edges(), 3);
    assert_eq!(r.guests.keys().collect::<Vec<_>>(), [g2.as_str(), g.as_str()], "没有配对的访客（g3）不留；指纹和公钥对不上的不要");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "读的时候不写文件");
    // 一个请求把最老的那个配对（at 是 1101、used 是 2026 年的）留着；存一次：各项的先后、数字的写法、不认识的键都照旧
    let sh = Shared::new(r);
    let now = 1794000000.5 + 86400.0;
    let (st, _) = handle_op(&sh, "pair_drop", &c, &V::Dict(dict! { "pid" => hex(3) }), now, None, &name_of).unwrap();
    assert_eq!(st, 200);
    let tail = format!(
        r#"}},"guests":{{"{g}":{{"key":"{gpub}","at":7}}}},"note":"手改的","seen":{{"k":[1.0,2,"三"]}},"big":123456789012345678901234567890}}"#
    );
    let want = [
        r#"{"v":2,"edges":{"#.to_string(),
        format!(r#""{p1}":{{"a":"{a}","b":"{b}","at":1101,"used":1794000000.5}},"#),
        format!(r#""{p2}":{{"b":"{g}","tag":[1,{{"x":null}}],"a":"{a}","at":1790000000.0}}"#),
        tail.clone(),
    ]
    .concat();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want);
    // 新写的文件只有自己能读；临时文件不留
    assert_eq!(crate::py::fs::mode_of(&path).unwrap(), 0o600);
    assert_eq!(crate::py::fs::listdir_sorted(&dir.path).unwrap(), ["remote.json"]);
    // 接入一次：used 没有的加在最后；有的原地改
    sh.lock().test_session(&hex(70), &a, &[&hex(2), &hex(1)], now);
    for (pid, who) in [(hex(2), g.as_str()), (hex(1), b.as_str())] {
        let gkey = (who == g).then_some(gpub.as_str());
        let (st, _) =
            handle_op(&sh, "relay_attach", who, &V::Dict(dict! { "sid" => hex(70), "pid" => pid }), now + 2.0, gkey, &name_of)
                .unwrap();
        assert_eq!(st, 200);
    }
    let want = [
        r#"{"v":2,"edges":{"#.to_string(),
        format!(r#""{p1}":{{"a":"{a}","b":"{b}","at":1101,"used":1794086402.5}},"#),
        format!(r#""{p2}":{{"b":"{g}","tag":[1,{{"x":null}}],"a":"{a}","at":1790000000.0,"used":1794086402.5}}"#),
        tail,
    ]
    .concat();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want);
}

#[test]
fn odd_times_in_the_state_file_do_not_break_requests() {
    let dir = TempDir::new().unwrap();
    let path = format!("{}/remote.json", dir.path);
    let (a, b) = (fp(1), fp(2));
    // at、used 不是数字（Python 会每个请求都出错）：当 0，下一个请求就把这个配对删掉；true 是 1；放不进 i64 的整数照它的值
    let text = format!(
        r#"{{"edges":{{"{}":{{"a":"{a}","b":"{b}","at":"x","used":[1]}},"{}":{{"a":"{a}","b":"{b}","at":true,"used":false}},"{}":{{"a":"{a}","b":"{b}","at":99999999999999999999,"used":{{}}}}}}}}"#,
        hex(1),
        hex(2),
        hex(3)
    );
    std::fs::write(&path, &text).unwrap();
    let (r, said) = open_file(&path, None);
    assert!(said.is_empty() && r.edges() == 3);
    let sh = Shared::new(r);
    let (st, v) =
        handle_op(&sh, "relay_list", &a, &V::Dict(dict! { "pids" => vec![V::str(hex(1)), V::str(hex(3))] }), NOW, None, &name_of)
            .unwrap();
    assert_eq!(st, 200);
    assert_eq!(v.as_dict().unwrap().dict("peers").unwrap().keys().iter().map(|k| k.to_string()).collect::<Vec<_>>(), [hex(3)]);
    let want = format!(
        r#"{{"v":1,"edges":{{"{}":{{"a":"{a}","b":"{b}","at":99999999999999999999,"used":{{}}}}}},"guests":{{}}}}"#,
        hex(3)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want);
}

#[test]
fn a_state_file_that_cannot_be_saved_is_an_error() {
    let mut t = Rig::new();
    let rid = hex(1);
    t.seated(1, 2, &rid);
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => hex(11) }).0, 200);
    std::fs::remove_dir_all(&t.dir.path).unwrap();
    let j = V::Dict(dict! { "rid" => &rid, "pid" => hex(11) });
    let e = handle_op(&t.sh, "pair_commit", &fp(2), &j, t.now, None, &name_of).unwrap_err();
    assert!(e.is_os(), "{e}");
    // 内存里已经记下了（和 Python 一样：存不了是没人接的异常，前面改的不退回去）
    assert_eq!(t.edges(), [hex(11)]);
    std::fs::create_dir(&t.dir.path).unwrap();
    t.now += 1.0;
    assert_eq!(t.op("pair_drop", 1, dict! { "pid" => hex(11) }).0, 200);
    assert_eq!(t.file(), r#"{"v":1,"edges":{},"guests":{}}"#);
}

// ---------------------------------------------------------------- pair_get 的等待

/// 另起一个线程发 pair_get：状态码、回的内容、等了多久（秒）
fn get_in_thread(sh: &Arc<Shared>, dev: u32, rid: &str, more: &'static str) -> std::thread::JoinHandle<(u16, String, f64)> {
    let (sh, rid, who) = (sh.clone(), rid.to_string(), fp(dev));
    std::thread::spawn(move || {
        let j = json::loads(&format!(r#"{{"rid":"{rid}"{more}}}"#)).unwrap();
        let t0 = Instant::now();
        let (st, v) = handle_op(&sh, "pair_get", &who, &j, NOW, None, &name_of).unwrap();
        (st, json::dumps(&v, COMPACT), t0.elapsed().as_secs_f64())
    })
}

fn waiting(sh: &Shared) -> usize {
    sh.waiters.load(Ordering::SeqCst)
}

#[test]
fn pair_get_waits_for_the_other_side() {
    let t = Rig::new();
    let rid = hex(1);
    let none = r#"{"v":1,"ok":true,"m":null,"fp":null,"name":null,"done":false}"#;
    let with = |who: u32, done: bool| format!(r#"{{"v":1,"ok":true,"m":{BLOB},"fp":"{}","name":"dev","done":{done}}}"#, fp(who));
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid }).0, 200);
    // 等不到：到点照现在的回
    let t0 = Instant::now();
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "wait" => 0.15 }), (200, none.to_string()));
    let took = t0.elapsed().as_secs_f64();
    assert!((0.15..3.0).contains(&took), "{took}");
    assert_eq!(waiting(&t.sh), 0);
    // wait 不是 0 到上限之间的数：不等
    let t0 = Instant::now();
    for w in [V::Bool(true), V::str("5"), V::Int(-1), V::Float(1e12), V::None, V::Int(0)] {
        assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "wait" => w }), (200, none.to_string()));
    }
    assert!(t0.elapsed().as_secs_f64() < 3.0);
    // 对方放了密文：马上回
    let h = get_in_thread(&t.sh, 1, &rid, r#","wait":10"#);
    wait_for("等上了", || waiting(&t.sh) == 1);
    assert_eq!(t.op("pair_put", 2, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    let (st, body, took) = h.join().unwrap();
    assert!((st, body.as_str()) == (200, with(2, false).as_str()) && took < 5.0, "{body} {took}");
    // 已经有了：不等
    let t0 = Instant::now();
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "wait" => 10 }), (200, with(2, false)));
    assert!(t0.elapsed().as_secs_f64() < 3.0);
    // until 是 done：有了密文也接着等，到两边都报了配对编号
    let (h1, h2) = (
        get_in_thread(&t.sh, 1, &rid, r#","wait":10,"until":"done""#),
        get_in_thread(&t.sh, 2, &rid, r#","wait":10,"until":"done""#),
    );
    wait_for("两边都等上了", || waiting(&t.sh) == 2);
    assert_eq!(t.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => hex(11) }).0, 200);
    std::thread::sleep(Duration::from_millis(30));
    assert!(!h1.is_finished() && !h2.is_finished() && waiting(&t.sh) == 2);
    assert_eq!(t.op("pair_commit", 2, dict! { "rid" => &rid, "pid" => hex(11) }).0, 200);
    let ((st1, body1, took1), (st2, body2, took2)) = (h1.join().unwrap(), h2.join().unwrap());
    assert!((st1, body1.as_str()) == (200, with(2, true).as_str()) && took1 < 5.0, "{body1} {took1}");
    assert!((st2, body2.as_str()) == (200, with(1, true).as_str()) && took2 < 5.0, "{body2} {took2}");
    let r = t.sh.lock();
    assert!(waiting(&t.sh) == 0 && r.waiting.is_empty() && r.closed.is_empty());
}

#[test]
fn pair_get_wakes_when_the_pairing_is_gone() {
    let t = Rig::new();
    let rid = hex(1);
    let none = r#"{"v":1,"ok":true,"m":null,"fp":null,"name":null,"done":false}"#;
    let with = |who: u32| format!(r#"{{"v":1,"ok":true,"m":{BLOB},"fp":"{}","name":"dev","done":false}}"#, fp(who));
    // 对方不做了：等着的马上回，照这次配对被拿掉时的样子（Python 里等的人手里还拿着那个字典）
    t.seated(1, 2, &rid);
    let h = get_in_thread(&t.sh, 2, &rid, r#","wait":10"#);
    wait_for("等上了", || waiting(&t.sh) == 1);
    assert_eq!(t.op("pair_close", 1, dict! { "rid" => &rid }).0, 200);
    let (st, body, took) = h.join().unwrap();
    assert!((st, body.as_str()) == (200, none) && took < 5.0, "{body} {took}");
    assert!(t.sh.lock().closed.is_empty(), "没人等着了：不留");
    assert_eq!(t.op("pair_get", 2, dict! { "rid" => &rid }), bad(404, "no_pairing"));
    // 两边报的不一样：两边等着的都回，各自看到的还是对方放的密文
    t.seated(1, 2, &rid);
    assert_eq!(t.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    assert_eq!(t.op("pair_commit", 1, dict! { "rid" => &rid, "pid" => hex(11) }).0, 200);
    let (h1, h2) = (
        get_in_thread(&t.sh, 1, &rid, r#","wait":10,"until":"done""#),
        get_in_thread(&t.sh, 2, &rid, r#","wait":10,"until":"done""#),
    );
    wait_for("两边都等上了", || waiting(&t.sh) == 2);
    assert_eq!(t.op("pair_commit", 2, dict! { "rid" => &rid, "pid" => hex(12) }), bad(409, "mismatch"));
    assert_eq!((h1.join().unwrap().1, h2.join().unwrap().1), (with(2), with(1)));
    assert!(t.sh.lock().closed.is_empty() && t.sh.lock().waiting.is_empty());
    // 等着的时候过期了、同一个 rid 又开了一次：等着的那个醒来看到的是原来那一次，新的那一次不受影响
    t.seated(1, 2, &rid);
    let h = get_in_thread(&t.sh, 2, &rid, r#","wait":10"#);
    wait_for("等上了", || waiting(&t.sh) == 1);
    let later = Rig { dir: TempDir::new().unwrap(), sh: t.sh.clone(), now: NOW + 601.0 };
    assert_eq!(later.op("pair_open", 1, dict! { "rid" => &rid }).0, 200, "过期的清掉了，可以再开");
    assert_eq!(later.sh.lock().closed.len(), 1);
    assert!(!h.is_finished(), "清过期的不叫醒人");
    assert_eq!(later.op("pair_put", 1, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    let (st, body, took) = h.join().unwrap();
    assert!((st, body.as_str()) == (200, none) && took < 5.0, "{body} {took}");
    assert!(later.sh.lock().closed.is_empty());
    assert_eq!(later.op("pair_get", 1, dict! { "rid" => &rid }), (200, none.to_string()));
    assert_eq!(later.op("pair_put", 3, dict! { "rid" => &rid, "m" => blob() }).0, 200);
    assert_eq!(later.op("pair_get", 3, dict! { "rid" => &rid }), (200, with(1)));
}

#[test]
fn pair_get_does_not_wait_when_too_many_already_do() {
    let t = Rig::new();
    let rid = hex(1);
    assert_eq!(t.op("pair_open", 1, dict! { "rid" => &rid }).0, 200);
    t.sh.waiters.store(PAIR_WAITERS, Ordering::SeqCst);
    let t0 = Instant::now();
    assert_eq!(t.op("pair_get", 1, dict! { "rid" => &rid, "wait" => 10 }).0, 200);
    assert!(t0.elapsed().as_secs_f64() < 3.0);
    assert_eq!(waiting(&t.sh), PAIR_WAITERS);
    // 少一个就等得上
    t.sh.waiters.store(PAIR_WAITERS - 1, Ordering::SeqCst);
    let h = get_in_thread(&t.sh, 1, &rid, r#","wait":10"#);
    wait_for("等上了", || waiting(&t.sh) == PAIR_WAITERS);
    assert_eq!(t.op("pair_close", 1, dict! { "rid" => &rid }).0, 200);
    assert_eq!(h.join().unwrap().0, 200);
    assert_eq!(waiting(&t.sh), PAIR_WAITERS - 1);
}

/// 几个线程共用：Shared 要能跨线程
#[test]
fn shared_is_sendable() {
    fn sendable<X: Send + Sync>() {}
    sendable::<Shared>();
}

// ---------------------------------------------------------------- 推送（push.rs）

/// push_set：配了对的访客（手机）登记、换掉、拿掉它的推送令牌。服务器没开推送是 no_push；名单里的设备、没配过对的是 denied；
/// 一个令牌只记在一个访客上；记进 remote.json（和记着的一样时不写），重启还在，样子不对的读的时候丢掉
#[test]
fn push_registration() {
    let mut t = Rig::new();
    let ((g, gpub), (g2, g2pub), (g3, g3pub)) = (guest(1), guest(2), guest(3));
    {
        let mut r = t.sh.lock();
        r.test_guest(&g, &gpub, NOW);
        r.test_guest(&g2, &g2pub, NOW);
        r.test_edge(&hex(11), &fp(1), &g, NOW);
        r.test_edge(&hex(12), &fp(1), &g2, NOW);
    }
    let tok = "ab".repeat(32);
    let reg = |token: &str, env: &str| dict! { "token" => token, "env" => env };
    let set = |t: &Rig, who: &str, key: &str, j: Dict| t.op_as("push_set", who, j, Some(key));
    assert_eq!(set(&t, &g, &gpub, reg(&tok, "production")), bad(404, "no_push"), "没开推送");
    assert!(!t.sh.lock().push_on());
    t.sh.lock().set_pusher(Pusher::for_test().0);
    assert!(t.sh.lock().push_on());
    t.sh.lock().guests_on = false;
    assert!(!t.sh.lock().push_on(), "不认访客：手机登记不了");
    t.sh.lock().guests_on = true;
    // 名单里的设备、没配过对的：不让登记
    assert_eq!(t.op("push_set", 1, reg(&tok, "production")), bad(403, "denied"));
    assert_eq!(set(&t, &g3, &g3pub, reg(&tok, "production")), bad(403, "denied"));
    // 样子不对
    for j in [
        reg(&"AB".repeat(32), "production"),
        reg(&"ab".repeat(31), "production"),
        reg(&"a".repeat(201), "production"),
        reg(&format!("{tok} "), "production"),
        reg(&tok, "development"),
        reg(&tok, ""),
        dict! { "token" => &tok },
        dict! { "token" => 5, "env" => "production" },
        dict! { "env" => "production" },
    ] {
        let shown = json::dumps(&V::Dict(j.clone()), COMPACT);
        assert_eq!(set(&t, &g, &gpub, j), bad(400, "bad"), "{shown}");
    }
    // 登记上：记进 remote.json 里这个访客的那一项
    assert_eq!(set(&t, &g, &gpub, reg(&tok, "production")), (200, OK.to_string()));
    let entry =
        format!(r#""{g}":{{"key":"{gpub}","at":1794000000.0,"push":{{"token":"{tok}","env":"production","at":1794000000.0}}}}"#);
    assert!(t.file().contains(&entry), "{}", t.file());
    // 一样的再来一次（过了一会儿）：不写盘
    let saved = (mtime(&t.path()), t.file());
    t.now += 30.0;
    assert_eq!(set(&t, &g, &gpub, reg(&tok, "production")), (200, OK.to_string()));
    assert_eq!((mtime(&t.path()), t.file()), saved);
    // 换了环境（Xcode 装的）：换掉
    assert_eq!(set(&t, &g, &gpub, reg(&tok, "sandbox")), (200, OK.to_string()));
    assert!(t.file().contains(&format!(r#""push":{{"token":"{tok}","env":"sandbox","at":1794000030.0}}"#)), "{}", t.file());
    // 同一个令牌换了访客（App 删了重装、换了身份）：原来那一项拿掉
    assert_eq!(set(&t, &g2, &g2pub, reg(&tok, "production")), (200, OK.to_string()));
    let regs = |t: &Rig| {
        let r = t.sh.lock();
        [&g, &g2].map(|who| r.guests.get(who).and_then(|x| x.push.as_ref().map(|p| format!("{} {}", p.token, p.env))))
    };
    assert_eq!(regs(&t), [None, Some(format!("{tok} production"))]);
    // 重启：还在（推送要再开：--push 是命令行里的）
    t.reopen(None);
    assert_eq!(regs(&t), [None, Some(format!("{tok} production"))]);
    assert!(!t.sh.lock().push_on());
    assert_eq!(set(&t, &g2, &g2pub, reg(&tok, "production")), bad(404, "no_push"));
    // 空的令牌：不要推送了
    t.sh.lock().set_pusher(Pusher::for_test().0);
    assert_eq!(set(&t, &g2, &g2pub, dict! { "token" => "" }), (200, OK.to_string()));
    assert_eq!(regs(&t), [None, None]);
    assert!(!t.file().contains(r#""push""#), "{}", t.file());
    // 样子不对的登记：读的时候丢掉，访客留着
    assert_eq!(set(&t, &g2, &g2pub, reg(&tok, "production")), (200, OK.to_string()));
    std::fs::write(t.path(), t.file().replace(r#""env":"production""#, r#""env":"nope""#)).unwrap();
    t.reopen(None);
    assert_eq!(regs(&t), [None, None]);
    assert_eq!(t.sh.lock().guests.len(), 2);
}

/// 宿主交来的 PUSH（push_job）：会话是这条控制连接的、配对是这个会话给看的、另一头是登记了推送的访客，样子也对，才交给推送网关；
/// 网关说令牌没用了（push_gone）：拿掉登记、存盘
#[test]
fn push_jobs() {
    let t = Rig::new();
    let ((g, gpub), (g2, g2pub)) = (guest(1), guest(2));
    let (pid, p2, p3) = (hex(11), hex(12), hex(13));
    let (sid, id) = (hex(50), "0123456789abcdef");
    {
        let mut r = t.sh.lock();
        r.test_guest(&g, &gpub, NOW);
        r.test_guest(&g2, &g2pub, NOW);
        r.test_edge(&pid, &fp(1), &g, NOW);
        r.test_edge(&p2, &fp(1), &g2, NOW);
        r.test_edge(&p3, &fp(1), &fp(2), NOW);
        r.set_pusher(Pusher::for_test().0);
    }
    t.session(&sid, 1, &[&pid, &p2, &p3]);
    t.session(&hex(51), 1, &[&p2]);
    let tok = "cd".repeat(32);
    assert_eq!(t.op_as("push_set", &g, dict! { "token" => &tok, "env" => "sandbox" }, Some(&gpub)), (200, OK.to_string()));
    let frame = |pid: &str, id: &str, note: &str| format!(r#"{{"pid":"{pid}","id":"{id}","note":{note}}}"#);
    let job = |sid: &str, conn: u64, payload: &str| t.sh.lock().push_job(sid, conn, payload.as_bytes());
    let got = job(&sid, 0, &frame(&pid, id, BLOB)).unwrap();
    assert_eq!((got.token.as_str(), got.env.as_str(), got.id.as_str()), (tok.as_str(), "sandbox", id));
    assert_eq!(json::dumps(&got.note.to_v(), COMPACT), BLOB);
    // 最长的通知收得下
    let blob_of = |n: usize| format!(r#"{{"n":"{}==","c":"{}","t":"{}="}}"#, "A".repeat(22), "A".repeat(n), "A".repeat(43));
    assert!(job(&sid, 0, &frame(&pid, id, &blob_of(PUSH_BLOB))).is_some());
    // 对不上的、样子不对的：不交
    for (s, conn, payload, why) in [
        (sid.clone(), 1, frame(&pid, id, BLOB), "不是这个会话现在的控制连接"),
        (hex(52), 0, frame(&pid, id, BLOB), "没有这个会话"),
        (hex(51), 0, frame(&pid, id, BLOB), "这个会话不给这个配对看"),
        (sid.clone(), 0, frame(&p2, id, BLOB), "另一头没登记推送"),
        (sid.clone(), 0, frame(&p3, id, BLOB), "另一头不是访客"),
        (sid.clone(), 0, frame(&hex(14), id, BLOB), "没有这个配对"),
        (sid.clone(), 0, frame(&pid, "0123456789ABCDEF", BLOB), "编号要小写"),
        (sid.clone(), 0, frame(&pid, "0123456789abcde", BLOB), "编号短了"),
        (sid.clone(), 0, frame(&pid, id, r#""junk""#), "密文不对"),
        (sid.clone(), 0, frame(&pid, id, &blob_of(PUSH_BLOB + 4)), "密文太长"),
        (sid.clone(), 0, format!(r#"{{"pid":"{pid}","note":{BLOB}}}"#), "没有编号"),
        (sid.clone(), 0, "[]".to_string(), "不是字典"),
        (sid.clone(), 0, "{".to_string(), "不是 JSON"),
    ] {
        assert_eq!(job(&s, conn, &payload), None, "{why}");
    }
    // 不认访客（--no-guests）：不交
    t.sh.lock().guests_on = false;
    assert_eq!(job(&sid, 0, &frame(&pid, id, BLOB)), None);
    t.sh.lock().guests_on = true;
    // 推送网关说这个令牌没用了：拿掉登记、存盘；别的令牌什么都不动
    let saved = (mtime(&t.path()), t.file());
    t.sh.lock().push_gone(&"ef".repeat(32));
    assert_eq!((mtime(&t.path()), t.file()), saved);
    t.sh.lock().push_gone(&tok);
    assert_eq!(job(&sid, 0, &frame(&pid, id, BLOB)), None);
    assert!(!t.file().contains(r#""push""#), "{}", t.file());
}

/// push_set 带 sids（手机上「已关闭的会话也推送」关着）：只推这几个会话，回的里面多一项 sids；记进 remote.json；空的是一个都不推；
/// 样子不对的不要；再登记不带 sids 是又都推。remote.json 里的 sids 样子不对：当成一个都不推
#[test]
fn push_only_some_sessions() {
    let t = Rig::new();
    let (g, gpub) = guest(1);
    let pid = hex(11);
    let (sid, other) = (hex(50), hex(51));
    {
        let mut r = t.sh.lock();
        r.test_guest(&g, &gpub, NOW);
        r.test_edge(&pid, &fp(1), &g, NOW);
        r.set_pusher(Pusher::for_test().0);
    }
    t.session(&sid, 1, &[&pid]);
    t.session(&other, 1, &[&pid]);
    let tok = "cd".repeat(32);
    let set = |sids: Option<V>| {
        let j = dict! { "token" => &tok, "env" => "sandbox" };
        if let Some(s) = sids {
            j.set("sids", s);
        }
        t.op_as("push_set", &g, j, Some(&gpub))
    };
    let list = |items: &[&str]| V::from(items.iter().map(|s| V::from(*s)).collect::<Vec<V>>());
    let frame = format!(r#"{{"pid":"{pid}","id":"0123456789abcdef","note":{BLOB}}}"#);
    let pushed = |s: &str| t.sh.lock().push_job(s, 0, frame.as_bytes()).is_some();
    let told = r#"{"v":1,"ok":true,"sids":true}"#.to_string();
    // 不带：都推，回的和以前一样
    assert_eq!(set(None), (200, OK.to_string()));
    assert_eq!((pushed(&sid), pushed(&other)), (true, true));
    // 只推这一个
    assert_eq!(set(Some(list(&[&sid]))), (200, told.clone()));
    assert_eq!((pushed(&sid), pushed(&other)), (true, false));
    assert!(t.file().contains(&format!(r#""sids":["{sid}"]"#)), "{}", t.file());
    // 一个都没开着
    assert_eq!(set(Some(list(&[]))), (200, told.clone()));
    assert_eq!((pushed(&sid), pushed(&other)), (false, false));
    // null 和不带一样
    assert_eq!(set(Some(V::None)), (200, OK.to_string()));
    assert_eq!((pushed(&sid), pushed(&other)), (true, true));
    // 样子不对的：不要，记着的不变
    assert_eq!(set(Some(list(&[&other]))), (200, told));
    let nine = [sid.as_str(); PUSH_SIDS + 1];
    for v in [V::from(5), list(&["nope"]), list(&[&"AB".repeat(16)]), V::from(vec![V::from(5)]), list(&nine)] {
        let shown = json::dumps(&v, COMPACT);
        assert_eq!(set(Some(v)), bad(400, "bad"), "{shown}");
    }
    assert_eq!((pushed(&sid), pushed(&other)), (false, true));
    // 再登记不带 sids：又都推，remote.json 里也没有了
    assert_eq!(set(None), (200, OK.to_string()));
    assert_eq!((pushed(&sid), pushed(&other)), (true, true));
    assert!(!t.file().contains("sids"), "{}", t.file());
    // remote.json 里的：没有、null 是都推；样子不对的当成一个都不推（宁可少推）
    let load = |extra: &str| {
        let doc = json::loads(&format!(r#"{{"token":"{tok}","env":"sandbox","at":1.0{extra}}}"#)).unwrap();
        PushReg::loaded(&doc.as_dict().unwrap()).map(|p| p.sids)
    };
    assert_eq!(load(""), Some(None));
    assert_eq!(load(r#","sids":null"#), Some(None));
    assert_eq!(load(&format!(r#","sids":["{sid}"]"#)), Some(Some(vec![sid.clone()])));
    assert_eq!(load(r#","sids":["nope"]"#), Some(Some(vec![])));
    assert_eq!(load(r#","sids":5"#), Some(Some(vec![])));
}

/// 宿主放上来的账号的状态（status_put）：只有配对里的设备放得了，访客（手机）放不了；手机查 relay_list 带了 have 才拿得到
/// （不带的回的和以前一样），手里那一份没变的只回 same；只给另一头放的；配对没了、放了太久的都拿掉
#[test]
fn account_status() {
    let mut t = Rig::new();
    let (g, gpub) = guest(1);
    let pid = hex(11);
    {
        let mut r = t.sh.lock();
        r.test_guest(&g, &gpub, NOW);
        r.test_edge(&pid, &fp(1), &g, NOW);
    }
    let other = t.pair(2, 3, 5).1;
    let put = |t: &Rig, dev: u32, j: Dict| t.op("status_put", dev, j);
    let with_blob = |pid: &str| dict! { "pid" => pid, "blob" => blob() };
    // 样子不对、不是自己的配对、访客发的
    for j in [dict! {}, dict! { "pid" => &pid }, dict! { "pid" => &pid, "blob" => "x" }, dict! { "blob" => blob() }] {
        assert_eq!(put(&t, 1, j), bad(400, "bad"));
    }
    let big = format!(r#"{{"n":"{}==","c":"{}","t":"{}="}}"#, "A".repeat(22), "A".repeat(STATUS_BLOB + 4), "A".repeat(43));
    assert_eq!(put(&t, 1, dict! { "pid" => &pid, "blob" => json::loads(&big).unwrap() }), bad(400, "bad"), "太长");
    assert_eq!(put(&t, 1, with_blob(&other)), bad(404, "no_pairing"));
    assert_eq!(t.op_as("status_put", &g, with_blob(&pid), Some(&gpub)), bad(403, "denied"));
    // 放上来；手机查：不带 have 的和以前一样，带了的多一项 status
    assert_eq!(put(&t, 1, with_blob(&pid)), (200, OK.to_string()));
    let list = |t: &Rig, have: Option<Dict>| {
        let j = dict! { "pids" => vec![V::str(&pid)] };
        if let Some(h) = have {
            j.set("have", h);
        }
        t.op_as("relay_list", &g, j, Some(&gpub))
    };
    let head = format!(r#"{{"v":1,"ok":true,"peers":{{"{pid}":{{"fp":"{}","name":"dev"}}}},"sessions":{{"{pid}":[]}}"#, fp(1));
    assert_eq!(list(&t, None), (200, format!("{head}}}")));
    t.now += 2.4;
    assert_eq!(list(&t, Some(dict! {})), (200, format!(r#"{head},"status":{{"{pid}":{{"blob":{BLOB},"age":2}}}}}}"#)));
    let n = blob().as_dict().unwrap().str("n").unwrap().to_string();
    assert_eq!(list(&t, Some(dict! { &*pid => &*n })), (200, format!(r#"{head},"status":{{"{pid}":{{"same":true,"age":2}}}}}}"#)));
    assert_eq!(list(&t, Some(dict! { &*pid => "x" })).1, format!(r#"{head},"status":{{"{pid}":{{"blob":{BLOB},"age":2}}}}}}"#));
    assert_eq!(t.op_as("relay_list", &g, dict! { "pids" => vec![V::str(&pid)], "have" => 5 }, Some(&gpub)), bad(400, "bad"));
    // 宿主自己查：它放的不回给它
    let mine = t.op("relay_list", 1, dict! { "pids" => vec![V::str(&pid)], "have" => dict! {} }).1;
    assert!(mine.ends_with(r#""status":{}}"#), "{mine}");
    // 放了太久的不给；配对没了的拿掉
    t.now += STATUS_TTL as f64;
    assert!(list(&t, Some(dict! {})).1.ends_with(r#""status":{}}"#));
    assert_eq!(put(&t, 1, with_blob(&pid)), (200, OK.to_string()));
    assert_eq!(t.sh.lock().status.len(), 1);
    assert_eq!(t.op("pair_drop", 1, dict! { "pid" => &pid }), (200, OK.to_string()));
    assert!(t.sh.lock().status.is_empty());
}

/// 一条到点再推的通知（status_put 的 later）
fn later(slot: &str, at: f64, id: &str) -> V {
    V::Dict(dict! { "slot" => slot, "at" => at, "id" => id, "note" => blob() })
}

/// 额度的通知：手机 push_set 带 usage 才推（回的里面多一项 usage）；push_note 马上推、回 sent；status_put 的 later 到点才推，
/// 再放一次换掉原来的，推过的同一条不再推；样子不对的不要
#[test]
fn usage_notes() {
    let t = Rig::new();
    let ((g, gpub), (g2, g2pub)) = (guest(1), guest(2));
    let (pid, p2) = (hex(11), hex(12));
    let (pusher, rx) = Pusher::for_test();
    {
        let mut r = t.sh.lock();
        r.test_guest(&g, &gpub, NOW);
        r.test_guest(&g2, &g2pub, NOW);
        r.test_edge(&pid, &fp(1), &g, NOW);
        r.test_edge(&p2, &fp(1), &g2, NOW);
    }
    let note =
        |t: &Rig, dev: u32, pid: &str| t.op("push_note", dev, dict! { "pid" => pid, "id" => "00000000000000aa", "note" => blob() });
    let sent = |yes: bool| (200, format!(r#"{{"v":1,"ok":true,"sent":{yes}}}"#));
    assert_eq!(note(&t, 1, &pid), sent(false), "没开推送");
    t.sh.lock().set_pusher(pusher);
    assert_eq!(note(&t, 1, &pid), sent(false), "手机没登记推送");
    let tok = "cd".repeat(32);
    let set = |who: &str, key: &str, usage: Option<V>| {
        let j = dict! { "token" => &tok, "env" => "sandbox" };
        if let Some(u) = usage {
            j.set("usage", u);
        }
        t.op_as("push_set", who, j, Some(key))
    };
    assert_eq!(set(&g, &gpub, None), (200, OK.to_string()));
    assert_eq!(note(&t, 1, &pid), sent(false), "登记了推送、没说要额度的通知");
    assert_eq!(set(&g, &gpub, Some(V::from(5))), bad(400, "bad"));
    let told = r#"{"v":1,"ok":true,"usage":true}"#.to_string();
    assert_eq!(set(&g, &gpub, Some(V::Bool(true))), (200, told.clone()));
    assert!(t.file().contains(r#""usage":true"#), "{}", t.file());
    assert_eq!(note(&t, 1, &pid), sent(true));
    let job = rx.try_recv().unwrap();
    assert_eq!((job.token.as_str(), job.env.as_str(), job.id.as_str()), (tok.as_str(), "sandbox", "00000000000000aa"));
    assert!(rx.try_recv().is_err());
    // 样子不对、不是自己的配对、访客发的
    for j in [
        dict! { "pid" => &pid },
        dict! { "pid" => &pid, "id" => "x", "note" => blob() },
        dict! { "pid" => &pid, "id" => "00000000000000aa" },
    ] {
        assert_eq!(t.op("push_note", 1, j), bad(400, "bad"));
    }
    assert_eq!(note(&t, 2, &pid), bad(404, "no_pairing"));
    assert_eq!(
        t.op_as("push_note", &g, dict! { "pid" => &pid, "id" => "00000000000000aa", "note" => blob() }, Some(&gpub)),
        bad(403, "denied")
    );
    // 到点再推的：到点前不推；到点了推一次，同一条再放上来不再推；再放一次（不带 later）是都拿掉
    let put = |l: Vec<V>| t.op("status_put", 1, dict! { "pid" => &pid, "blob" => blob(), "later" => l });
    assert_eq!(put(vec![later("0000000000000001", NOW + 100.0, "00000000000000b1")]), (200, OK.to_string()));
    assert_eq!(t.sh.lock().next_due(), Some(NOW + 100.0));
    assert!(t.sh.lock().due(NOW + 99.0).is_empty());
    let due = t.sh.lock().due(NOW + 100.0);
    assert_eq!(due.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), ["00000000000000b1"]);
    assert_eq!(t.sh.lock().next_due(), None);
    assert_eq!(put(vec![later("0000000000000001", NOW + 100.0, "00000000000000b1")]), (200, OK.to_string()));
    assert!(t.sh.lock().later.is_empty(), "推过的同一条");
    // 换掉：slot 一样、时间变了（编号跟着变）；不带 later 是拿掉
    assert_eq!(put(vec![later("0000000000000001", NOW + 200.0, "00000000000000b2")]), (200, OK.to_string()));
    assert_eq!(
        put(vec![
            later("0000000000000001", NOW + 300.0, "00000000000000b3"),
            later("0000000000000002", NOW + 50.0, "00000000000000c1")
        ])
        .0,
        200
    );
    assert_eq!(t.sh.lock().later.len(), 2);
    assert_eq!(t.sh.lock().next_due(), Some(NOW + 50.0));
    assert_eq!(t.op("status_put", 1, dict! { "pid" => &pid, "blob" => blob() }), (200, OK.to_string()));
    assert!(t.sh.lock().later.is_empty());
    // 到点时手机不要了：不推（拿掉）
    assert_eq!(put(vec![later("0000000000000003", NOW + 10.0, "00000000000000d1")]).0, 200);
    assert_eq!(set(&g, &gpub, Some(V::Bool(false))), (200, told));
    assert!(!t.file().contains(r#""usage""#), "{}", t.file());
    assert!(t.sh.lock().due(NOW + 10.0).is_empty());
    assert!(t.sh.lock().later.is_empty());
    // 样子不对的 later
    let nine: Vec<V> = (0..=LATER_MAX).map(|i| later(&format!("{i:016x}"), NOW + 1.0, "00000000000000e1")).collect();
    for l in [
        nine,
        vec![later("0000000000000001", NOW + LATER_SPAN as f64 + 1.0, "00000000000000e1")],
        vec![later("0000000000000001", NOW, "00000000000000e1"), later("0000000000000001", NOW, "00000000000000e2")],
        vec![later("x", NOW, "00000000000000e1")],
        vec![later("0000000000000001", NOW, "E1")],
        vec![later("0000000000000001", -1.0, "00000000000000e1")],
        vec![V::from(5)],
    ] {
        let shown = json::dumps(&V::from(l.clone()), COMPACT);
        assert_eq!(put(l), bad(400, "bad"), "{shown}");
    }
    assert_eq!(t.op("status_put", 1, dict! { "pid" => &pid, "blob" => blob(), "later" => 5 }), bad(400, "bad"));
    // 别的手机（g2）没要额度的通知：给它的到点了也不推
    assert_eq!(
        t.op(
            "status_put",
            1,
            dict! { "pid" => &p2, "blob" => blob(), "later" => vec![later("0000000000000001", NOW, "00000000000000f1")] }
        )
        .0,
        200
    );
    assert!(t.sh.lock().due(NOW).is_empty());
}
