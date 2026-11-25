//! 拿 Python 那一份答的重放一遍：`cli/tests/vectors/server_remote_direct.json` 是直接调 server/cca-presence.py 的 `Remote`
//! 录下来的（每一步问了什么、给的时间、它回了什么、remote.json 变成了什么、手里还有几样东西）。这里照样问一遍
//! `handle_op`、`guest_key`、`keep_only`、`ws_claim`、`take_ticket`，逐项比：状态码、回的内容（连键的先后）、文件逐字节。
//! 票据的编号和密钥是随机的：只比样子（对照数据里写成 `<hex32>`、`<b64:32>`），后面的步骤用 `@名字` 指前面换到的那一张。
//! 录的时候把上限调小了（对照数据的 limits）：到顶的那几种回答才走得到。启动时读到不该有的文件（半截的、不是 UTF-8 的、是个目录……）
//! 日志里说的那一句也录了（路径写成 `<path>`）。没动文件的那几步连“原样重写一遍”也不该有：看文件是不是换了一个。
//! 重新录：照文件开头 about 说的再问一遍 Python 那一份（录的脚本不在仓库里）；有意改了行为就直接改这份数据。

use super::*;
use crate::py::base64;
use crate::py::fs::TempDir;
use crate::server::wire;
use crate::server::DIR_BLOB;
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;

fn text(v: &V) -> String {
    json::dumps(v, COMPACT)
}

fn num(d: &Dict, k: &str) -> usize {
    usize::try_from(d.get(k).unwrap().as_int().unwrap()).unwrap()
}

fn strs(v: &V) -> Vec<String> {
    v.as_list().unwrap().strs().iter().map(|s| s.to_string()).collect()
}

/// 对照数据里的会话：照样放进来（平常是数据端口那边建的）
fn session(s: &Dict) -> Session {
    let mut blobs = OMap::new();
    for (pid, b) in s.dict("blobs").unwrap().items() {
        blobs.insert(&*pid, wire::parse_blob(&b, DIR_BLOB).unwrap());
    }
    Session {
        fp: s.str("fp").unwrap().to_string(),
        pids: strs(&s.at("pids")),
        blobs,
        conn: s.at("online").truthy().then_some(0),
        lost: None,
        viewers: num(s, "viewers"),
        since: s.at("since").as_num().unwrap(),
    }
}

/// 一张用掉的票据，写成 Python 那边记的样子（键照字母排；密钥不比）
fn ticket(k: &Ticket) -> V {
    let d = Dict::new();
    d.set("exp", k.exp);
    d.set("fp", &k.fp);
    match k.role {
        Role::Host => d.set("pids", k.pids.iter().map(V::str).collect::<Vec<V>>()),
        Role::View => d.set("pid", k.pid.as_deref()),
    }
    d.set("role", k.role.name());
    d.set("sid", &k.sid);
    if k.ws {
        d.set("ws", true);
    }
    V::Dict(d)
}

#[test]
fn the_python_remote_step_by_step() {
    let doc = json::loads(include_str!("../../../tests/vectors/server_remote_direct.json")).unwrap().as_dict().unwrap();
    let l = doc.dict("limits").unwrap();
    let lim = Limits {
        pair_slots: num(&l, "pair_slots"),
        edges_dev: num(&l, "edges_dev"),
        edges_max: num(&l, "edges_max"),
        tickets_dev: num(&l, "tickets_dev"),
        sessions_dev: num(&l, "sessions_dev"),
        viewers: num(&l, "viewers"),
        ..Limits::DEFAULT
    };
    let port = u16::try_from(num(&doc, "port")).unwrap();
    let names = doc.dict("names").unwrap();
    let name_of = |fp: &str| names.str(fp).map(|s| s.to_string());
    let dir = TempDir::new().unwrap();
    let path = PathBuf::from(format!("{}/remote.json", dir.path));
    let mut sh = Shared::new(old_remote(Remote::open(&path, port, doc.str("web").map(|s| s.to_string()), lim)));
    let mut tids: HashMap<String, String> = HashMap::new();
    let mut last: Option<String> = None;
    // 存一次就是换一个文件（写到临时文件再换过去）：编号变了就是存过
    let ino = |path: &Path| std::fs::metadata(path).ok().map(|m| m.ino());
    let mut last_ino: Option<u64> = None;
    let mut opened = 0;
    let steps = doc.list("steps").unwrap().items();
    assert!(steps.len() > 320, "对照数据不全");
    for (i, step) in steps.iter().enumerate() {
        let s = step.as_dict().unwrap();
        let at = format!("第 {i} 步：{}", text(step));
        let now = s.get("now").and_then(|v| v.as_num()).unwrap_or(0.0);
        // @名字 是前面那一步换到的票据；别的照原样
        let tid = |tids: &HashMap<String, String>| {
            let t = s.str("tid").unwrap();
            t.strip_prefix('@').map_or_else(|| t.to_string(), |label| tids.get(label).unwrap().clone())
        };
        match &*s.str("do").unwrap() {
            "op" => {
                let (fp, op, gkey) = (s.str("fp").unwrap(), s.str("op").unwrap(), s.str("gkey"));
                let (st, reply) = handle_op(&sh, &op, &fp, &s.at("j"), now, gkey.as_deref(), &name_of).unwrap();
                if let Some(label) = s.str("save") {
                    let r = reply.as_dict().unwrap();
                    let (t, key) = (r.str("tid").unwrap(), r.str("tkey").unwrap());
                    assert!(wire::aid_re().matches(&t) && t.len() == 32, "{at}");
                    assert_eq!(base64::b64decode(&key, true).unwrap().len(), 32, "{at}");
                    assert!(tids.values().all(|old| **old != *t), "票据的编号重了：{at}");
                    tids.insert(label.to_string(), t.to_string());
                    r.set("tid", "<hex32>");
                    r.set("tkey", "<b64:32>");
                }
                assert_eq!((i64::from(st), text(&reply)), (s.at("status").as_int().unwrap(), text(&s.at("reply"))), "{at}");
            }
            "guest_key" => {
                let got = sh.lock().guest_key(&s.str("fp").unwrap(), &s.str("op").unwrap(), &s.at("j"), now);
                assert_eq!(got.as_deref(), s.str("key").as_deref(), "{at}");
            }
            "keep_only" => sh.lock().keep_only(&strs(&s.at("fps")), now).unwrap(),
            "ws_claim" => assert_eq!(sh.lock().ws_claim(&tid(&tids), now), s.at("ok").truthy(), "{at}"),
            "take" => {
                let proof = s.at("proof").truthy();
                let got = match &*s.str("role").unwrap() {
                    "host" => sh.lock().take_ticket(&tid(&tids), Role::Host, now, |_| proof),
                    "view" => sh.lock().take_ticket(&tid(&tids), Role::View, now, |_| proof),
                    _ => None, // 数据端口只拿这两种来问（join 用的是令牌）
                };
                assert_eq!(
                    got.as_ref().map(ticket).map(|v| text(&v)),
                    s.get("took").filter(|v| !v.is_none()).map(|v| text(&v)),
                    "{at}"
                );
            }
            "session" => {
                sh.lock().sessions.insert(&*s.str("sid").unwrap(), session(&s));
            }
            "sessions" => {
                let got = Dict::new();
                for (sid, x) in sh.lock().sessions.iter() {
                    let mut blobs: Vec<&str> = x.blobs.keys().collect();
                    blobs.sort_unstable();
                    let row = crate::dict! {
                        "fp" => &x.fp,
                        "pids" => x.pids.iter().map(V::str).collect::<Vec<V>>(),
                        "blobs" => blobs.into_iter().map(V::str).collect::<Vec<V>>(),
                        "viewers" => x.viewers,
                    };
                    got.set(sid, row);
                }
                assert_eq!(text(&V::Dict(got)), text(&s.at("want")), "{at}");
            }
            "reopen" => {
                if let Some(written) = s.str("write") {
                    std::fs::write(&path, written.as_bytes()).unwrap();
                    last = Some(written.to_string());
                    last_ino = ino(&path);
                }
                sh = Shared::new(old_remote(Remote::open(&path, port, s.str("web").map(|s| s.to_string()), lim)));
            }
            "open" => {
                // remote.json 的位置上放一个文件（或者一个目录），再启动：日志里说的那一句
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_dir(&path);
                match s.str("raw_b64") {
                    Some(raw) => std::fs::write(&path, base64::b64decode(&raw, true).unwrap()).unwrap(),
                    None => std::fs::create_dir(&path).unwrap(),
                }
                (last, last_ino) = (std::fs::read_to_string(&path).ok(), ino(&path));
                let shown = path.to_string_lossy().to_string();
                let mut said = Vec::new();
                let web = s.str("web").map(|s| s.to_string());
                sh = Shared::new(old_remote(Remote::open_saying(&path, port, web, lim, &mut |line| {
                    said.push(line.replace(&shown, "<path>"))
                })));
                assert_eq!(said, strs(&s.at("said")), "{at}");
                opened += 1;
            }
            other => panic!("对照数据里有不认识的一步 {other}"),
        }
        // 文件：变了的逐字节一样，没变的不该动（连原样重写一遍也不该）
        let (file, now_ino) = (std::fs::read_to_string(&path).ok(), ino(&path));
        match s.str("file") {
            Some(want) => {
                assert_eq!(file.as_deref(), Some(&*want), "{at}");
                assert_ne!(now_ino, last_ino, "{at}");
                last = file;
            }
            None => assert_eq!((file, now_ino), (last.clone(), last_ino), "这一步不该动文件：{at}"),
        }
        last_ino = now_ino;
        // 手里的东西一样多：配对到一半的、票据、会话、配对、访客
        let r = sh.lock();
        let have = [r.slots.len(), r.tickets.len(), r.sessions.len(), r.edges.len(), r.guests.len()];
        let want: Vec<usize> = s.list("n").unwrap().items().iter().map(|v| usize::try_from(v.as_int().unwrap()).unwrap()).collect();
        assert_eq!(have.as_slice(), want.as_slice(), "{at}");
    }
    assert!(opened > 20, "对照数据里没有“读到不该有的文件”那几步");
}

/// 录下这些数据的 Python 服务端只在开着网页端时认访客（3.7.0 起不开网页端也认：iOS 上的 App）：照它当时的规矩
fn old_remote(mut r: Remote) -> Remote {
    r.guests_on = r.web().is_some();
    r
}
