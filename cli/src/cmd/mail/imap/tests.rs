//! imap.rs 的单元测试：对照数据（tests/vectors/mail.json）里的时间、FETCH 的回应、LIST 的行；`MailBox` 对着照剧本回答的
//! 假服务器（明文、只在 127.0.0.1 上）走一遍，结果和发出去的命令都和 Python 的 imaplib 一样；`MailSource` 配假的会话
//! （tests/mail_view.py 的 Source）。

use super::*;
use crate::cmd::mail::testing::{bytes_of, vectors};
use crate::cmd::mail::{mail_error, MAIL_RULES};
use crate::py::obj::{Dict, V};
use crate::{dict, list};
use std::io::BufRead;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn rule() -> &'static MailRule {
    &MAIL_RULES[0]
}

/// tests/mail_view.py 的 Parse.test_internaldate
#[test]
fn internaldate() {
    let e = |s: &str| mail_epoch(s).unwrap();
    assert_eq!(e("15-Nov-2026 10:00:00 +0800"), e("15-Nov-2026 02:00:00 +0000"));
    assert_eq!(e(" 5-Nov-2026 02:00:00 -0130"), e("05-Nov-2026 03:30:00 +0000"));
    assert_eq!(e("15-Nov-2026 02:00:00 +0000"), Some(1794708000));
    for bad in ["15-Foo-2026 02:00:00 +0000", "2026-11-15 02:00:00", ""] {
        assert_eq!(e(bad), None);
    }
    let cases = vectors("epoch");
    assert!(cases.len() > 30);
    for c in cases {
        let s = c.at("in").py_str();
        match (mail_epoch(&s), c.get("error")) {
            (Ok(t), None) => assert_eq!(t, c.at("out").as_int(), "{s:?}"),
            (Err(e), Some(_)) => assert!(e.is_value(), "{s:?}"),
            (got, want) => panic!("{s:?}：{got:?} / {want:?}"),
        }
    }
}

/// tests/mail_view.py 的 Parse.test_search_criteria
#[test]
fn search_criteria() {
    let t = mail_epoch("15-Nov-2026 00:30:00 +0000").unwrap().unwrap() as f64;
    assert_eq!(mail_since(t), "14-Nov-2026");
    assert_eq!(mail_crit(&[rule()], t), ["SINCE", "14-Nov-2026", "OR", "FROM", "\"anthropic.com\"", "SUBJECT", "\"secure link\""]);
    let two = mail_crit(&[rule(), rule()], t);
    assert_eq!(two[..3], ["SINCE", "14-Nov-2026", "OR"]);
    assert!(two[3].starts_with("(OR FROM") && two[4].starts_with("(OR FROM"));
    let strs = |v: V| -> Vec<String> { v.as_list().unwrap().strs().iter().map(|s| s.to_string()).collect() };
    let cases = vectors("since");
    assert!(cases.len() > 15);
    for c in cases {
        let t = c.at("t").as_num().unwrap();
        assert_eq!(mail_since(t), c.at("since").py_str(), "{t}");
        assert_eq!(mail_crit(&[rule()], t), strs(c.at("crit")));
        assert_eq!(mail_crit(&[rule(), rule()], t), strs(c.at("crit2")));
        assert_eq!(mail_crit(&[rule(), rule(), rule()], t), strs(c.at("crit3")));
        assert_eq!(mail_crit(&[], t), strs(c.at("crit0")));
    }
}

fn item_of(v: &V) -> Item {
    match v {
        V::None => Item::None,
        V::List(pair) => Item::Pair(bytes_of(&pair.at(0)), bytes_of(&pair.at(1))),
        other => Item::Bytes(bytes_of(other)),
    }
}

fn fetched(got: BTreeMap<u64, (i64, Vec<u8>)>) -> V {
    V::from(got.into_iter().map(|(u, (t, lit))| V::from(list![u as i64, t, latin1(&lit)])).collect::<Vec<_>>())
}

/// tests/mail_view.py 的 Parse.test_fetch_parse_any_order，加上对照数据
#[test]
fn fetch_parse_any_order() {
    let d = "\"15-Nov-2026 02:00:00 +0000\"";
    let pair = |head: String, lit: &[u8]| Item::Pair(head.into_bytes(), lit.to_vec());
    let a = [
        pair(format!("1 FETCH (UID 5 INTERNALDATE {d} BODY[HEADER.FIELDS (FROM)] {{9}}"), b"From: x\r\n"),
        Item::Bytes(b")".to_vec()),
        pair(format!("2 FETCH (INTERNALDATE {d} BODY[HEADER.FIELDS (FROM)] {{9}}"), b"From: y\r\n"),
        Item::Bytes(b" UID 7)".to_vec()),
        pair("3 FETCH (UID 8 BODY[] {1}".to_string(), b"z"),
        Item::Bytes(b")".to_vec()),
    ];
    let out = mail_fetch_parse(&a).unwrap();
    assert_eq!(out.keys().copied().collect::<Vec<_>>(), [5, 7], "没有 INTERNALDATE 的不算");
    assert_eq!(out[&7].1, b"From: y\r\n");
    assert!(mail_fetch_parse(&[]).unwrap().is_empty());
    let cases = vectors("fetch_parse");
    assert!(cases.len() > 25);
    for c in cases {
        let data: Vec<Item> = c.at("data").as_list().map(|l| l.items().iter().map(item_of).collect()).unwrap_or_default();
        match (mail_fetch_parse(&data), c.get("error")) {
            (Ok(got), None) => {
                let got = fetched(got);
                assert!(got.py_eq(&c.at("out")), "{:?}\n  这里：{got:?}\n  Python：{:?}", c.at("data"), c.at("out"));
            }
            (Err(e), Some(_)) => assert!(e.is_value(), "{:?}", c.at("data")),
            (got, want) => panic!("{:?}：{got:?} / {want:?}", c.at("data")),
        }
    }
}

#[test]
fn list_rows_like_python() {
    let cases = vectors("list_row");
    assert!(cases.len() > 30);
    for c in cases {
        let s = c.at("in").py_str();
        let got = match list_row(&s) {
            None => V::None,
            Some((attrs, name)) => V::from(list![attrs, name]),
        };
        assert!(got.py_eq(&c.at("out")), "{s:?}：{got:?} / {:?}", c.at("out"));
    }
}

#[test]
fn response_pieces() {
    assert_eq!(literal_size(b"1 (UID 5 BODY[] {12}"), Some(&b"12"[..]));
    assert_eq!(literal_size(b"{0}"), Some(&b"0"[..]));
    for no in [&b"{}"[..], b"{x}", b"{12} ", b"12}", b"{12", b"}", b""] {
        assert_eq!(literal_size(no), None, "{no:?}");
    }
    assert_eq!(response_code(b"[UIDNEXT 4] Predicted next UID."), Some(("UIDNEXT".into(), Some(b"4".to_vec()))));
    assert_eq!(response_code(b"[UIDNEXT 4] Predicted] next"), Some(("UIDNEXT".into(), Some(b"4] Predicted".to_vec()))));
    assert_eq!(response_code(b"[READ-ONLY] x"), Some(("READ-ONLY".into(), None)));
    assert_eq!(response_code(b"[READ-ONLY]"), Some(("READ-ONLY".into(), None)));
    assert_eq!(response_code(b"[A ]"), Some(("A".into(), Some(Vec::new()))));
    for no in [&b" [UIDNEXT 4]"[..], b"[uidnext 4]", b"[UIDNEXT 4", b"[]", b"[A", b"[Ax]", b"x"] {
        assert_eq!(response_code(no), None, "{no:?}");
    }
    let items = [Item::Bytes(b"it's".to_vec()), Item::None, Item::Pair(b"a".to_vec(), b"b".to_vec())];
    assert_eq!(items_repr(&items), "[b\"it's\", None, (b'a', b'b')]");
}

// --- 照剧本回答的假服务器（明文）：和 tools/gen_mail_vectors.py 里 Python 的 Script 一样演 --------------------------------------

/// 一次连接怎么演：连上就发的、每收到一条命令回的（None 是不回）、演完以后怎么收场
type Scene = (Option<Vec<u8>>, Vec<Option<Vec<u8>>>, String);

fn reset(s: &TcpStream) {
    use std::os::fd::AsRawFd;
    let l = libc::linger { l_onoff: 1, l_linger: 0 };
    // SAFETY: s 开着；l 是有效的内存，长度如实给出
    unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_LINGER, (&raw const l).cast(), std::mem::size_of::<libc::linger>() as libc::socklen_t) };
}

fn play(conn: TcpStream, scene: &Scene, sent: &Mutex<Vec<String>>) {
    use std::io::Write;
    conn.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    let mut out = conn.try_clone().unwrap();
    let mut input = std::io::BufReader::new(conn.try_clone().unwrap());
    // 读一条命令、记下来（tag 换成 TAG），返回它的 tag
    let mut next = || -> Option<Vec<u8>> {
        let mut line = Vec::new();
        match input.read_until(b'\n', &mut line) {
            Ok(n) if n > 0 => {}
            _ => return None,
        }
        let tag: Vec<u8> = line.iter().copied().take_while(|b| *b != b' ').collect();
        sent.lock().unwrap().push(format!("TAG{}", latin1(&line[tag.len()..])));
        Some(tag)
    };
    let (greet, steps, end) = scene;
    if let Some(g) = greet {
        let _ = out.write_all(g);
    }
    for reply in steps {
        let Some(tag) = next() else { break };
        if let Some(r) = reply {
            let text = latin1(r).replace("{TAG}", &latin1(&tag));
            let _ = out.write_all(&text.chars().map(|c| c as u8).collect::<Vec<u8>>());
        }
    }
    match end.as_str() {
        "hang" => while next().is_some() {},
        "rst" => reset(&conn),
        "line-rst" => {
            next();
            reset(&conn);
        }
        _ => {}
    }
}

/// 起假服务器：返回端口、收到的命令、停下它的开关
fn serve(scenes: Vec<Scene>) -> (u16, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (sent2, stop2) = (sent.clone(), stop.clone());
    let h = std::thread::spawn(move || {
        for scene in scenes {
            let conn = loop {
                match listener.accept() {
                    Ok((c, _)) => break Some(c),
                    Err(_) if stop2.load(Ordering::SeqCst) => break None,
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            };
            let Some(conn) = conn else { return };
            conn.set_nonblocking(false).unwrap();
            play(conn, &scene, &sent2);
        }
    });
    (port, sent, stop, h)
}

fn connector(port: u16, timeout: f64) -> Connector {
    Box::new(move || {
        let s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs_f64(timeout)))?;
        Ok(Box::new(s) as Box<dyn Wire>)
    })
}

fn outcome(r: R<V>) -> Dict {
    match r {
        Ok(v) => dict! { "ok" => v },
        Err(e) => match mail_error(&e) {
            // 说明里带着这次随机的 tag 的（服务器原样回了它）：和对照数据一样换成 TAG
            Some((code, text)) if e.is("MailRetry") => dict! { "retry" => list![code, re!(r"\b[A-P]{4}(\d+ )").sub(r"TAG\1", text)] },
            Some((code, text)) => dict! { if e.is("MailFatal") { "fatal" } else { "retry" } => list![code, text] },
            None => dict! { "py" => match &e { Fail::Value(_) => "ValueError", Fail::Other(class, _) => class, _ => "?" } },
        },
    }
}

/// MailBox 的各个方法对着假服务器走一遍：结果、出错的分类和说法、发出去的每条命令都和 Python（imaplib）的一样
#[test]
fn sessions_like_imaplib() {
    let cases = vectors("imap");
    assert!(cases.len() > 140);
    for c in cases {
        let name = c.at("name").py_str();
        let scenes: Vec<Scene> = c
            .at("conns")
            .as_list()
            .unwrap()
            .items()
            .iter()
            .map(|conn| {
                let conn = conn.as_dict().unwrap();
                let steps = conn.list("steps").map(|l| l.items()).unwrap_or_default();
                (
                    conn.get("greet").filter(|g| !g.is_none()).map(|g| bytes_of(&g)),
                    steps.iter().map(|s| (!s.is_none()).then(|| bytes_of(s))).collect(),
                    conn.str("end").map_or("close".to_string(), |e| e.to_string()),
                )
            })
            .collect();
        let (port, sent, stop, server) = serve(scenes);
        let timeout = c.at("timeout").as_num().unwrap();
        let mut bx = MailBox::with_connector("inbox@example.com", "abcdefghijklmnop", connector(port, timeout));
        let hdrs = |got: R<BTreeMap<u64, (i64, Vec<u8>)>>| got.map(fetched);
        for (op, want) in c.at("ops").as_list().unwrap().items().iter().zip(c.at("results").as_list().unwrap().items()) {
            let (kind, args) = match op {
                V::List(l) => (l.at(0).py_str(), l.items()[1..].to_vec()),
                other => (other.py_str(), Vec::new()),
            };
            let uid = |i: usize| args[i].as_int().unwrap() as u64;
            let got = outcome(match kind.as_str() {
                "open" => bx.open().map(|()| V::None),
                "open_as" => {
                    bx = MailBox::with_connector(&args[0].py_str(), &args[1].py_str(), connector(port, timeout));
                    bx.open().map(|()| V::None)
                }
                "folders" => bx.folders().map(|f| V::from(f.into_iter().map(|(l, n)| V::from(list![l, n])).collect::<Vec<_>>())),
                "examine" => bx.examine(&args[0].py_str()).map(|(v, n)| V::from(list![v.map(|x| x as i64), n.map(|x| x as i64)])),
                "search" => bx.search(&mail_crit(&[rule()], 1794708000.0)).map(|u| V::from(u.into_iter().map(|x| V::Int(x as i64)).collect::<Vec<_>>())),
                "headers" => hdrs(bx.headers(&Uids::List(args[0].as_list().unwrap().items().iter().map(|u| u.as_int().unwrap() as u64).collect()))),
                "range" => hdrs(bx.headers(&Uids::Range(uid(0), uid(1)))),
                "message" => bx.message(uid(0)).map(|m| V::from(latin1(&m))),
                "logout" => {
                    bx.close(true);
                    Ok(V::None)
                }
                "shutdown" => {
                    bx.close(false);
                    Ok(V::None)
                }
                other => panic!("{other}"),
            });
            got.set("alive", bx.is_open());
            assert!(V::Dict(got.clone()).py_eq(&want), "{name}：{op:?}\n  这里：{got:?}\n  Python：{want:?}");
        }
        drop(bx);
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let want: Vec<String> = c.at("sent").as_list().unwrap().strs().iter().map(|s| s.to_string()).collect();
        assert_eq!(*sent.lock().unwrap(), want, "{name}：发出去的命令");
    }
}

// --- MailSource：tests/mail_view.py 的 Source（假的 MailBox） --------------------------------------------------------------

#[derive(Default)]
struct FakeBox {
    open: bool,
    state: HashMap<String, (u64, u64)>,
    msgs: HashMap<String, BTreeMap<u64, (i64, Vec<u8>)>>,
    hits: HashMap<String, Vec<u64>>,
    calls: Vec<String>,
    cur: String,
    fail: Option<String>,
}

impl FakeBox {
    fn new() -> FakeBox {
        let mut b = FakeBox::default();
        b.state.insert("A".into(), (1, 10));
        b.state.insert("J".into(), (5, 3));
        b.msgs.insert("A".into(), (1..10).map(|u| (u, (u as i64, format!("Subject: {u}\r\n").into_bytes()))).collect());
        b.msgs.insert("J".into(), [(1, (1, b"x".to_vec())), (2, (2, b"y".to_vec()))].into_iter().collect());
        b.hits.insert("A".into(), vec![2, 4]);
        b.hits.insert("J".into(), vec![1]);
        b
    }
}

impl Session for FakeBox {
    fn is_open(&self) -> bool {
        self.open
    }
    fn open(&mut self) -> R<()> {
        self.open = true;
        self.calls.push("open".into());
        Ok(())
    }
    fn folders(&mut self) -> R<Vec<(String, String)>> {
        Ok(vec![("所有邮件".into(), "A".into()), ("垃圾邮件".into(), "J".into())])
    }
    fn examine(&mut self, name: &str) -> R<(Option<u64>, Option<u64>)> {
        self.cur = name.to_string();
        self.calls.push(format!("examine {name}"));
        if self.fail.as_deref() == Some(name) {
            self.fail = None;
            self.open = false;
            return Err(mail_retry("net", "gone"));
        }
        let (v, n) = self.state[name];
        Ok((Some(v), Some(n)))
    }
    fn search(&mut self, _crit: &[String]) -> R<Vec<u64>> {
        self.calls.push(format!("search {}", self.cur));
        Ok(self.hits[&self.cur].clone())
    }
    fn headers(&mut self, uids: &Uids) -> R<BTreeMap<u64, (i64, Vec<u8>)>> {
        let wanted: Vec<u64> = match uids {
            Uids::Range(a, b) => {
                self.calls.push(format!("headers {} {a}:{b}", self.cur));
                (*a..=*b).collect()
            }
            Uids::List(l) => {
                self.calls.push(format!("headers {} {l:?}", self.cur));
                l.clone()
            }
        };
        Ok(wanted.into_iter().filter_map(|u| self.msgs[&self.cur].get(&u).map(|m| (u, m.clone()))).collect())
    }
    fn message(&mut self, uid: u64) -> R<Vec<u8>> {
        Ok(self.msgs[&self.cur][&uid].1.clone())
    }
    fn close(&mut self, _polite: bool) {
        self.open = false;
    }
}

fn source() -> MailSource<FakeBox> {
    MailSource::new(FakeBox::new(), &[rule()])
}

fn uids(out: &[Cand]) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = out.iter().map(|c| (c.folder.clone(), c.uid)).collect();
    v.sort();
    v
}

fn fetches(src: &MailSource<FakeBox>) -> Vec<String> {
    src.bx.calls.iter().filter(|c| c.starts_with("search") || c.starts_with("headers")).cloned().collect()
}

#[test]
fn first_round_searches_and_takes_the_latest_burst() {
    let mut src = source();
    let out = src.poll(0.0).unwrap();
    let mut want: Vec<(String, u64)> = (1..10).map(|u| ("A".to_string(), u)).collect();
    want.extend([("J".to_string(), 1), ("J".to_string(), 2)]);
    assert_eq!(uids(&out), want);
    assert!(src.bx.calls.contains(&"search A".to_string()));
    assert!(src.bx.calls.contains(&"headers A 1:9".to_string()));
    assert_eq!((out[0].label.as_str(), out[0].got, out[0].valid), ("所有邮件", 1.0, Some(1)));
}

#[test]
fn later_rounds_fetch_new_uids_without_searching() {
    let mut src = source();
    src.poll(0.0).unwrap();
    src.bx.calls.clear();
    assert!(src.poll(0.0).unwrap().is_empty());
    assert!(fetches(&src).is_empty(), "UIDNEXT 没变：不搜索也不取");
    src.bx.msgs.get_mut("A").unwrap().insert(10, (10, b"Subject: new\r\n".to_vec()));
    src.bx.state.insert("A".into(), (1, 11));
    src.bx.calls.clear();
    let out = src.poll(0.0).unwrap();
    assert_eq!(uids(&out), [("A".to_string(), 10)]);
    assert_eq!(fetches(&src), ["headers A 10:10"]);
}

#[test]
fn uidvalidity_change_searches_again() {
    let mut src = source();
    src.poll(0.0).unwrap();
    src.bx.state.insert("A".into(), (2, 10));
    src.bx.calls.clear();
    let out = src.poll(0.0).unwrap();
    assert!(src.bx.calls.contains(&"search A".to_string()));
    assert_eq!(out.len(), 9, "换了 UIDVALIDITY：同样的 UID 是另外的邮件");
}

#[test]
fn a_round_cut_short_loses_nothing() {
    let mut src = source();
    src.bx.fail = Some("J".into());
    assert!(src.poll(0.0).unwrap_err().is("MailRetry"));
    let out = src.poll(0.0).unwrap();
    assert!(uids(&out).contains(&("A".to_string(), 4)), "断在第二个邮件夹：第一个邮件夹里的下一轮照样给");
    assert_eq!(src.bx.calls.iter().filter(|c| *c == "open").count(), 2);
}

#[test]
fn burst_is_capped_and_full_fetches_the_message() {
    let mut src = source();
    src.bx.state.insert("A".into(), (1, 1000));
    src.poll(0.0).unwrap();
    assert!(src.bx.calls.contains(&"headers A 950:999".to_string()), "{:?}", src.bx.calls);
    src.bx.state.insert("A".into(), (1, 2000));
    src.bx.calls.clear();
    src.poll(0.0).unwrap();
    assert!(src.bx.calls.contains(&"headers A 1950:1999".to_string()), "一轮最多取最新的 50 封：{:?}", src.bx.calls);
    let cand = Cand { folder: "J".into(), label: "垃圾邮件".into(), uid: 2, got: 2.0, hdr: Vec::new(), valid: Some(5) };
    src.bx.close(false);
    assert_eq!(src.full(&cand).unwrap(), b"y");
    assert!(src.bx.is_open());
}
