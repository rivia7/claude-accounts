//! 拿真的服务器（server/cca-presence.py 的进程）答的重放一遍：`cli/tests/vectors/server_remote.json` 是对着它录的——
//! 设备、访客签了名经 HTTPS 发的请求，和数据端口上收发的字节，照先后一步一步记着。这里照同样的先后来一遍：请求交给 `handle_op`
//! （前面由处理请求的那一层做的那几步照 Python 的做一遍：在名单里的不问 `guest_key`；不在名单里的要有它给的公钥、签名的又正是
//! 这把，否则回 403 unknown_device；名单变了调 `keep_only`），数据端口那些连的是这个进程里跑着的中转。逐项比：状态码、回的内容、
//! 客户端在数据端口上收到的每一个字节。随机的只比样子（`<hex32>`、`<b64:32>`），`@名字` 指前面换到的票据、令牌。
//! 录的时候真的等了的那两种（5 秒没说完开场白、15 秒没人来接）这里不等：走到那一步以后把那一项时限调成 0。
//! 重新录：照文件开头 about 说的再对着 Python 那一份跑一遍（录的脚本不在仓库里）；有意改了行为就直接改这份数据。

use super::tests::{read_ctl, read_line, read_n, relay_line, wait_for, Net};
use super::*;
use crate::py::obj::Dict;
use crate::server::limits::Limits;
use crate::server::remote::handle_op;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn text(v: &V) -> String {
    json::dumps(v, Fmt::new().utf8().compact())
}

fn b64(s: &Dict, k: &str) -> Vec<u8> {
    base64::b64decode(&s.str(k).unwrap(), true).unwrap()
}

/// 读到对面关掉为止
fn drain(s: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => out,
        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => out,
        Err(e) => panic!("等不到对面关：{e}（已经收到 {:?}）", String::from_utf8_lossy(&out)),
    }
}

/// remote.json 里的时间换成 <time>（录的时候是当时的时间）
fn shape(v: &V, key: &str) -> V {
    match v {
        V::Dict(d) => {
            let out = Dict::new();
            for (k, x) in d.items() {
                out.set(k.clone(), shape(&x, &k));
            }
            V::Dict(out)
        }
        V::Float(_) if key == "at" || key == "used" => V::str("<time>"),
        other => other.clone(),
    }
}

/// 一个请求的结果：状态码、回的内容（随机的、随时间变的换成了样子）、换到的票据（编号和密钥）
type Answer = (i64, String, Option<(String, Vec<u8>)>);

/// 处理请求的那一层知道的：各把密钥和名单。等着的 pair_get 是另开线程发的，所以这里不放 V（它不能跨线程），请求以文字进来
#[derive(Default)]
struct Front {
    /// 名字 → (指纹, 公钥)
    devices: HashMap<String, (String, String)>,
    /// 名单：指纹 → 设备的名字
    roster: HashMap<String, String>,
}

impl Front {
    /// 设备、访客的一个请求：处理请求的那一层先做的几步（Py:1949-1957），再交给 handle_op
    fn ask(&self, sh: &Shared, who: &str, signer: &str, op: &str, req: &str) -> Answer {
        let fp = &self.devices[who].0;
        let req = json::loads(req).unwrap();
        let now = time::time();
        let listed = self.roster.contains_key(fp);
        // 名单里的设备拿别的密钥签名是 403 denied，到不了中转这一层：录的里面没有这一种
        assert!(!listed || signer == who, "{who} 的请求是 {signer} 签的");
        let gkey = if listed { None } else { sh.lock().guest_key(fp, op, &req, now) };
        if !listed && gkey.as_deref() != Some(self.devices[signer].1.as_str()) {
            // 不在名单里，又没有验得过的公钥
            return (403, r#"{"error":"unknown_device","gen":"<hex32>"}"#.to_string(), None);
        }
        let name_of = |fp: &str| self.roster.get(fp).cloned();
        let (st, reply) = handle_op(sh, op, fp, &req, now, gkey.as_deref(), &name_of).unwrap();
        let r = reply.as_dict().unwrap();
        let ticket = r.str("tid").zip(r.str("tkey")).map(|(tid, tkey)| {
            assert!(wire::aid_re().matches(&tid) && tid.len() == 32, "{tid}");
            let key = base64::b64decode(&tkey, true).unwrap();
            assert_eq!(key.len(), 32);
            r.set("tid", "<hex32>");
            r.set("tkey", "<b64:32>");
            (tid.to_string(), key)
        });
        for rows in r.dict("sessions").map(|d| d.values()).unwrap_or_default() {
            for row in rows.as_list().unwrap().items() {
                row.as_dict().unwrap().set("age", "<age>");
            }
        }
        (i64::from(st), text(&reply), ticket)
    }

    /// 录的那一步里的请求
    fn ask_step(&self, sh: &Shared, s: &Dict) -> Answer {
        let who = s.str("who").unwrap();
        self.ask(sh, &who, &s.str("signer").unwrap_or(who.clone()), &s.str("op").unwrap(), &text(&s.at("req")))
    }
}

/// 录的那一步里记着的回答
fn recorded(s: &Dict) -> (i64, String) {
    (s.at("status").as_int().unwrap(), text(&s.at("reply")))
}

/// 重放时记着的：票据、令牌、开着的连接
#[derive(Default)]
struct Play {
    front: Front,
    /// 名字 → (票据的编号, 密钥)
    tickets: HashMap<String, (String, Vec<u8>)>,
    tokens: HashMap<String, String>,
    conns: HashMap<String, TcpStream>,
    /// 等着的接入方该收到的那句 OK
    oks: HashMap<String, Vec<u8>>,
    /// 连上不说话、占着名额的那些
    held: Vec<TcpStream>,
}

impl Play {
    /// 一步请求：回的和录的一样（随机的、随时间变的只比样子）。还一样不了的是 Err(这边回的)
    fn op(&mut self, net: &Net, s: &Dict) -> Result<(), String> {
        let (st, reply, ticket) = self.front.ask_step(&net.sh, s);
        if (st, &reply) != (recorded(s).0, &recorded(s).1) {
            return Err(format!("{st} {reply}"));
        }
        assert_eq!(ticket.is_some(), s.str("save").is_some(), "换到票据的那几步才记名字");
        if let Some(ticket) = ticket {
            self.tickets.insert(s.str("save").unwrap().to_string(), ticket);
        }
        Ok(())
    }

    /// 等着对方的 pair_get（另开线程发，真的等在里面），和这时候另一台设备发来、让它不用再等的那个请求：两个回的都和录的一样，
    /// 而且是被叫醒的（录的时候要等满是 8 秒）
    fn op_during(&mut self, net: &Net, s: &Dict, other: &Dict, at: &str) {
        let (who, op, req) = (s.str("who").unwrap().to_string(), s.str("op").unwrap().to_string(), text(&s.at("req")));
        let (front, sh) = (&self.front, &*net.sh);
        assert_eq!(sh.waiters.load(Ordering::SeqCst), 0, "{at}");
        let t0 = Instant::now();
        let (waited, meanwhile) = std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                let (st, reply, _) = front.ask(sh, &who, &who, &op, &req);
                (st, reply)
            });
            wait_for("pair_get 等在里面", || sh.waiters.load(Ordering::SeqCst) == 1 || waiter.is_finished());
            assert!(!waiter.is_finished(), "{at}\n没有等对方就回了");
            let (st, reply, _) = front.ask_step(sh, other);
            (waiter.join().unwrap(), (st, reply))
        });
        assert!(t0.elapsed() < Duration::from_secs(4), "{at}\n等满了才回的：没有被叫醒");
        assert_eq!(meanwhile, recorded(other), "{at}");
        assert_eq!(waited, recorded(s), "{at}");
    }

    /// 连上数据端口、说开场白：回的和录的一样
    fn enter(&mut self, net: &Net, s: &Dict, at: &str) {
        let (conn, role, ident, key) =
            (s.str("conn").unwrap(), s.str("role").unwrap(), s.str("ident").unwrap(), s.str("key").unwrap());
        let ident = match ident.strip_prefix('@') {
            Some(label) if &*role == "join" => self.tokens.get(label).unwrap().clone(),
            Some(label) => self.tickets.get(label).unwrap().0.clone(),
            None => ident.to_string(),
        };
        let key = match key.strip_prefix('@') {
            Some(label) => self.tickets.get(label).unwrap().1.clone(),
            None => base64::b64decode(&key, true).unwrap(),
        };
        let cn = s.str("cn").unwrap();
        let (line, ok) = relay_line(&role, &ident, &key, &cn);
        // mac 不对的那几步：拿另一把密钥算的
        let line = if s.at("mac").truthy() { line } else { relay_line(&role, &ident, &[0u8; 32], &cn).0 };
        let mut sock = net.dial();
        let said = [line.as_bytes(), &s.str("extra_b64").map(|_| b64(s, "extra_b64")).unwrap_or_default()].concat();
        // 带 split 的：这一行分两截到（中间隔一会儿，中转先读到前一截）
        let (first, rest) = said.split_at(s.get("split").and_then(|n| n.as_int()).map_or(0, |n| usize::try_from(n).unwrap()));
        if !first.is_empty() {
            sock.write_all(first).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        sock.write_all(rest).unwrap();
        match &*s.str("recv").unwrap() {
            "ok" => {
                assert_eq!(String::from_utf8_lossy(&read_line(&mut sock)), ok, "{at}");
                self.conns.insert(conn.to_string(), sock);
            }
            "wait" => {
                // 接入方：宿主那一头来接之前什么都收不到
                self.oks.insert(conn.to_string(), ok.into_bytes());
                self.conns.insert(conn.to_string(), sock);
            }
            refused => assert_eq!(String::from_utf8_lossy(&drain(&mut sock)), refused, "{at}"),
        }
    }
}

#[test]
fn the_python_server_step_by_step() {
    let doc = json::loads(include_str!("../../../tests/vectors/server_remote.json")).unwrap().as_dict().unwrap();
    let port = u16::try_from(doc.at("port").as_int().unwrap()).unwrap();
    let mut net = Net::open(Limits::DEFAULT, port, doc.str("web").map(|s| s.to_string()), |_| {});
    let mut play = Play::default();
    for (name, d) in doc.dict("devices").unwrap().items() {
        let d = d.as_dict().unwrap();
        play.front.devices.insert(name.to_string(), (d.str("fp").unwrap().to_string(), d.str("pub").unwrap().to_string()));
    }
    let steps = doc.list("steps").unwrap().items();
    assert!(steps.len() > 320, "对照数据不全");
    let mut seen: Vec<String> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let s = step.as_dict().unwrap();
        let at = format!("第 {i} 步：{}", text(step));
        let conn = s.str("conn").map(|c| c.to_string()).unwrap_or_default();
        let kind = s.str("do").unwrap();
        if !seen.iter().any(|k| **k == *kind) {
            seen.push(kind.to_string());
        }
        match &*kind {
            "roster" => {
                let devices = &play.front.devices;
                play.front.roster = s
                    .dict("members")
                    .unwrap()
                    .items()
                    .iter()
                    .map(|(who, name)| (devices[&**who].0.clone(), name.as_str().unwrap().to_string()))
                    .collect();
                let fps: Vec<String> = play.front.roster.keys().cloned().collect();
                net.sh.lock().keep_only(&fps, time::time()).unwrap();
            }
            "op" => match s.dict("meanwhile") {
                Some(other) => play.op_during(&net, &s, &other, &at),
                None => {
                    // 带 tune 的：服务器的那一项时限到了回的才变（录的时候真的等了一分钟）。这里先看时限没到时还不是这个回答，
                    // 再把那一项调成 0
                    let tuned = s.str("tune");
                    if let Some(what) = &tuned {
                        assert_eq!(&**what, "grace", "{at}");
                        std::thread::sleep(Duration::from_millis(30));
                        assert!(play.op(&net, &s).is_err(), "{at}\n时限还没到就是这个回答了");
                        net.tune(|k| k.grace = 0.0);
                    }
                    // 带 retry 的：数据端口那边的事要等中转的线程做完才看得到，问到一样为止
                    let t0 = Instant::now();
                    while let Err(got) = play.op(&net, &s) {
                        assert!(
                            s.get("retry").is_some_and(|r| r.truthy()) && t0.elapsed() < Duration::from_secs(10),
                            "{at}\n这边回的是 {got}"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    if tuned.is_some() {
                        net.tune(|k| k.grace = Knobs::DEFAULT.grace);
                    }
                }
            },
            "file" => {
                let file = json::loads(&std::fs::read_to_string(&net.path).unwrap()).unwrap();
                assert_eq!(text(&shape(&file, "")), text(&s.at("json")), "{at}");
            }
            "say" => {
                let mut sock = net.dial();
                sock.write_all(&b64(&s, "send_b64")).unwrap();
                assert_eq!(String::from_utf8_lossy(&drain(&mut sock)), &*s.str("recv").unwrap(), "{at}");
            }
            "slow" => {
                // 连上不说话的、开场白只说了一半的：录的时候等了 5 秒
                wait_for("别的连接都关了", || net.conns() == 0);
                let mut sock = net.dial();
                sock.write_all(&b64(&s, "send_b64")).unwrap();
                wait_for("收下了", || net.conns() == 1);
                std::thread::sleep(Duration::from_millis(20)); // 说了一半的那几个字节也读进去
                net.tune(|k| k.preamble_wait = 0.0);
                assert_eq!(String::from_utf8_lossy(&drain(&mut sock)), &*s.str("recv").unwrap(), "{at}");
                net.tune(|k| k.preamble_wait = 600.0);
            }
            "enter" => play.enter(&net, &s, &at),
            "ok" => assert_eq!(read_line(play.conns.get_mut(&conn).unwrap()), play.oks.remove(&conn).unwrap(), "{at}"),
            "frame" => {
                // 带 times 的：内容是那一段重复这么多遍（很长的一帧，写得短）
                let kind = u8::try_from(s.at("kind").as_int().unwrap()).unwrap();
                let times = s.get("times").and_then(|n| n.as_int()).map_or(1, |n| usize::try_from(n).unwrap());
                play.conns.get_mut(&conn).unwrap().write_all(&frame(kind, &b64(&s, "payload_b64").repeat(times))).unwrap();
            }
            "raw" => {
                play.conns.get_mut(&conn).unwrap().write_all(&b64(&s, "data_b64")).unwrap();
                if s.get("pause").is_some_and(|p| p.truthy()) {
                    std::thread::sleep(Duration::from_millis(20)); // 中转先读到这一截，后面的才来
                }
            }
            "shut" => play.conns.get(&conn).unwrap().shutdown(std::net::Shutdown::Write).unwrap(),
            "recv_frame" => {
                let (kind, payload) = read_ctl(play.conns.get_mut(&conn).unwrap());
                assert_eq!(i64::from(kind), s.at("kind").as_int().unwrap(), "{at}");
                match s.str("save") {
                    Some(label) => {
                        let said = String::from_utf8(payload).unwrap();
                        let token = json::loads(&said).unwrap().as_dict().unwrap().str("token").unwrap();
                        assert!(wire::aid_re().matches(&token) && token.len() == 32, "{at}");
                        assert_eq!(said.replace(&*token, "<hex32>"), &*s.str("payload").unwrap(), "{at}");
                        play.tokens.insert(label.to_string(), token.to_string());
                    }
                    None => assert_eq!(payload, b64(&s, "payload_b64"), "{at}"),
                }
            }
            "bytes" => {
                let data = b64(&s, "data_b64");
                play.conns.get_mut(&*s.str("from").unwrap()).unwrap().write_all(&data).unwrap();
                assert!(read_n(play.conns.get_mut(&*s.str("to").unwrap()).unwrap(), data.len()) == data, "{at}");
            }
            "close" => drop(play.conns.remove(&conn).unwrap()),
            "eof" => {
                play.oks.remove(&conn); // 等着的接入方被断开了：那句 OK 等不到了
                assert_eq!(drain(&mut play.conns.remove(&conn).unwrap()), b64(&s, "recv_b64"), "{at}");
            }
            "stall" => {
                // 没人来接的接入方：录的时候等了 15 秒
                assert!(play.oks.remove(&conn).is_some(), "{at}");
                net.tune(|k| k.join_wait = -k.preamble_wait);
                assert_eq!(drain(&mut play.conns.remove(&conn).unwrap()), b64(&s, "recv_b64"), "{at}");
                net.tune(|k| k.join_wait = Knobs::DEFAULT.join_wait);
            }
            "hold" => {
                // 一个地址能有的连接都占上（这时没有别的连接）：后面那一步再来一条，直接被关掉
                assert!(play.conns.is_empty(), "{at}");
                wait_for("别的连接都关了", || net.conns() == 0);
                let n = usize::try_from(s.at("n").as_int().unwrap()).unwrap();
                assert_eq!(n, Knobs::DEFAULT.conn_ip, "{at}");
                play.held = (0..n).map(|_| net.dial()).collect();
                wait_for("都收下了", || net.conns() == n);
            }
            "release" => {
                play.held.clear();
                wait_for("占着的都关了", || net.conns() == 0);
            }
            other => panic!("对照数据里有不认识的一步 {other}"),
        }
    }
    assert!(play.conns.is_empty() && play.oks.is_empty() && play.held.is_empty(), "录的最后没有开着的连接");
    // 录的里面各种步骤都有（少了哪一种，就是对照数据被换成了不全的）
    for kind in [
        "roster",
        "op",
        "file",
        "say",
        "slow",
        "enter",
        "ok",
        "frame",
        "raw",
        "shut",
        "recv_frame",
        "bytes",
        "close",
        "eof",
        "stall",
        "hold",
        "release",
    ] {
        assert!(seen.iter().any(|k| k == kind), "对照数据里没有 {kind} 这一步");
    }
}
