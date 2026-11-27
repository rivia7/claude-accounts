//! 设备发来的中转的请求：Python 的 `Remote.op`（Py:1090-1230）。签名、时间、nonce 由处理请求的那一头验过了才到这里。
//!
//! 各个 op 回什么（状态码、error 的那个词）照 Python 一个不差；先查什么后查什么也照它（同一个请求两处都不对时回的是先查的那一处）。

use super::{sids_of, Edge, Guest, Later, PushReg, Remote, Role, Shared, Slot, Status, Val};
use crate::py::obj::{Dict, V};
use crate::py::time::monotonic;
use crate::py::{num, R};
use crate::server::wire::{self, Blob, Num};
use crate::server::{API, GUEST_OPS, LATER_MAX, LATER_SPAN, PAIR_MSG, PAIR_TTL, PAIR_WAIT, PAIR_WAITERS, PUSH_BLOB, STATUS_BLOB};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{MutexGuard, PoisonError};
use std::time::Duration;

/// 状态码和回的内容
type Reply = (u16, V);

fn fail(code: u16, why: &str) -> Reply {
    (code, V::Dict(crate::dict! { "error" => why }))
}

fn done() -> Reply {
    (200, V::Dict(crate::dict! { "v" => API, "ok" => true }))
}

/// `hex32(x)`：是 32 个十六进制字符的字符串才给（Py:1092）
fn hex32(x: Option<V>) -> Option<Rc<str>> {
    x.and_then(|x| x.rc_str()).filter(|s| wire::aid_re().matches(s))
}

/// `Remote.op`：见 `super::handle_op`
pub(super) fn handle(
    sh: &Shared,
    op: &str,
    fp: &str,
    j: &V,
    now: f64,
    gkey: Option<&str>,
    name_of: &dyn Fn(&str) -> Option<String>,
) -> R<(u16, V)> {
    let gkey = gkey.filter(|k| !k.is_empty());
    if gkey.is_some() && !GUEST_OPS.contains(&op) {
        return Ok(fail(403, "denied"));
    }
    // 处理请求的那一头查过 j 是字典；不是的话当它什么都没带
    let j = j.as_dict().unwrap_or_default();
    let mut guard = sh.lock();
    guard.sweep(now)?;
    if op.starts_with("pair_") && op != "pair_drop" {
        let Some(rid) = hex32(j.get("rid")) else { return Ok(fail(400, "bad")) };
        let r = &mut *guard;
        if op == "pair_open" {
            return Ok(r.pair_open(&rid, fp, &j, now));
        }
        if let Some(refused) = r.pair_enter(&rid, op, fp, gkey) {
            return Ok(refused);
        }
        match op {
            // 这一边中途不做了：这次配对作废（已经记下的配对不受影响），等着的那一边马上知道
            "pair_close" => {
                r.slot_pop(&rid);
                sh.cv.notify_all();
                return Ok(done());
            }
            "pair_put" => {
                let reply = r.pair_put(&rid, fp, &j);
                if reply.0 == 200 {
                    sh.cv.notify_all();
                }
                return Ok(reply);
            }
            "pair_get" => return Ok(pair_get(sh, guard, &rid, fp, &j, name_of)),
            "pair_commit" => {
                let (reply, wake) = r.pair_commit(&rid, fp, &j, now)?;
                if wake {
                    sh.cv.notify_all();
                }
                return Ok(reply);
            }
            _ => {} // 别的 pair_ 开头的：和 Python 一样接着往下，最后是 not_found
        }
    }
    let r = &mut *guard;
    match op {
        "pair_drop" => r.pair_drop(fp, &j),
        "relay_host" => r.relay_host(fp, &j, now),
        "relay_attach" => r.relay_attach(fp, &j, now),
        "relay_list" => Ok(r.relay_list(fp, &j, now, name_of)),
        "push_set" => r.push_set(fp, gkey.is_some(), &j, now),
        "status_put" => Ok(r.status_put(fp, &j, now)),
        "push_note" => Ok(r.push_note(fp, &j)),
        _ => Ok(fail(404, "not_found")),
    }
}

/// pair_get（Py:1133-1152）：等对方放密文进来（until 是 "done" 时等两边都报了配对编号），最多 wait 秒；等不到照现在的回。
/// 等的时候这次配对被拿掉了（对方不做了、配不上、过期了）也马上回：照它被拿掉时的样子
fn pair_get<'a>(
    sh: &'a Shared,
    mut r: MutexGuard<'a, Remote>,
    rid: &str,
    fp: &str,
    j: &Dict,
    name_of: &dyn Fn(&str) -> Option<String>,
) -> Reply {
    let wait = match j.get("wait") {
        Some(w) if wire::num_ok(&w) => w.as_num().unwrap_or(0.0).min(PAIR_WAIT as f64),
        _ => 0.0,
    };
    let done_only = j.str("until").as_deref() == Some("done");
    let Some(serial) = r.slots.get(rid).map(|s| s.serial) else { return fail(404, "no_pairing") };
    let ready = |r: &Remote| match r.slots.get(rid).filter(|s| s.serial == serial) {
        Some(s) => s.done || (!done_only && s.theirs(fp).1.is_some()),
        None => true, // 已经不是这一次配对了
    };
    if wait > 0.0 && !ready(&r) {
        if sh.waiters.fetch_add(1, Ordering::SeqCst) >= PAIR_WAITERS {
            sh.waiters.fetch_sub(1, Ordering::SeqCst); // 等着的太多了：不等，照现在的回
        } else {
            r.waiting.push(serial);
            let end = monotonic() + wait;
            while !ready(&r) {
                let Ok(left) = Duration::try_from_secs_f64(end - monotonic()) else { break };
                if left.is_zero() {
                    break;
                }
                r = sh.cv.wait_timeout(r, left).unwrap_or_else(PoisonError::into_inner).0;
            }
            if let Some(i) = r.waiting.iter().position(|s| *s == serial) {
                r.waiting.swap_remove(i);
            }
            sh.waiters.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let slot = r.slots.get(rid).filter(|s| s.serial == serial).or_else(|| r.closed.iter().find(|s| s.serial == serial));
    let reply = match slot {
        Some(s) => {
            let (peer, m) = s.theirs(fp);
            let peer = m.and(peer);
            let reply = crate::dict! {
                "v" => API, "ok" => true, "m" => m.map(Blob::to_v), "fp" => peer, "name" => peer.and_then(name_of), "done" => s.done,
            };
            (200, V::Dict(reply))
        }
        None => fail(404, "no_pairing"), // 到不了：等着的时候被拿掉的都留在 closed 里
    };
    // 没有人等着的那几次配对最后的样子不用留了
    let Remote { closed, waiting, .. } = &mut *r;
    closed.retain(|s| waiting.contains(&s.serial));
    reply
}

impl Remote {
    /// push_set：手机登记它的推送令牌（换掉、拿掉；push.rs）。token：推送服务给这台手机上这个 App 的（小写的十六进制，64～200 个
    /// 字符），空的是不要推送了；env：production（App Store、TestFlight 装的）或者 sandbox（Xcode 装的）。只有配了对的访客能登记
    /// （名单里的设备、正在配对的是 403 denied）；这台服务器没开推送是 404 no_push。一个令牌只记在一个访客上（App 删了重装、
    /// 换了身份：旧的那一项不再推）。和记着的一样时不存。sids（可以不带）：只推这几个会话（`PUSH_SIDS` 个以内的会话编号；手机上
    /// 「已关闭的会话也推送」关着时是 App 里正开着的那个），不带、null 是都推；带了的回的里面多一项 `"sids": true`——旧的服务器不认
    /// 它、照旧都推，手机看这一项知道。usage（可以不带）：true 是也推额度的通知（push_note、到点的 later），不带、别的是不推；
    /// 带了的回的里面多一项 `"usage": true`（旧的服务器不认，手机看这一项知道这台服务器推不了）
    fn push_set(&mut self, fp: &str, guest: bool, j: &Dict, now: f64) -> R<Reply> {
        if !guest || !self.guests.contains(fp) {
            return Ok(fail(403, "denied"));
        }
        if self.pusher.is_none() {
            return Ok(fail(404, "no_push"));
        }
        // token 要是字符串（空的是拿掉）；不是空的时 env 要是那两种之一
        let Some(token) = j.str("token") else {
            return Ok(fail(400, "bad"));
        };
        let env = j.str("env").unwrap_or_default();
        let sids = match j.get("sids") {
            None | Some(V::None) => None,
            Some(v) => match sids_of(&v) {
                Some(s) => Some(s),
                None => return Ok(fail(400, "bad")),
            },
        };
        let usage = match j.get("usage") {
            None => None,
            Some(V::Bool(b)) => Some(b),
            Some(_) => return Ok(fail(400, "bad")),
        };
        let told = sids.is_some();
        let reg = match token.is_empty() {
            true => None,
            false if wire::push_token_re().matches(&token) && matches!(&*env, "production" | "sandbox") => Some(PushReg {
                token: token.to_string(),
                env: env.to_string(),
                sids,
                usage: usage == Some(true),
                at: Val::Num(Num::Float(now)),
            }),
            false => return Ok(fail(400, "bad")),
        };
        let mut changed = false;
        if let Some(new) = &reg {
            self.guests.for_each_mut(|other, g| {
                if other != fp && g.push.as_ref().is_some_and(|p| p.token == new.token) {
                    g.push = None;
                    changed = true;
                }
            });
        }
        if let Some(g) = self.guests.get_mut(fp) {
            let key = |r: &Option<PushReg>| r.as_ref().map(|p| (p.token.clone(), p.env.clone(), p.sids.clone(), p.usage));
            if key(&g.push) != key(&reg) {
                g.push = reg;
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        let reply = crate::dict! { "v" => API, "ok" => true };
        if told {
            reply.set("sids", true);
        }
        if usage.is_some() {
            reply.set("usage", true);
        }
        Ok((200, V::Dict(reply)))
    }

    /// status_put（宿主发的）：这台机器上账号的状态（`blob`，封给这个配对的密文，这里解不开），配对的另一头（手机）查 relay_list 时
    /// 一起拿走；`later`（可以不带）：到点再推的通知（额度恢复的那一刻，`[{"slot", "at", "id", "note"}]`，最多 `LATER_MAX` 条），
    /// 这个配对原来放的换成这几条（不带、空的是都拿掉；一天之内到点推过的同一条不再放）。只在内存里；不是这个配对的回 404 no_pairing
    fn status_put(&mut self, fp: &str, j: &Dict, now: f64) -> Reply {
        let (Some(pid), Some(blob)) = (hex32(j.get("pid")), j.get("blob").and_then(|b| wire::parse_blob(&b, STATUS_BLOB))) else {
            return fail(400, "bad");
        };
        let later = match j.get("later") {
            None | Some(V::None) => Vec::new(),
            Some(v) => match later_of(&v, &pid, fp, now) {
                Some(l) => l,
                None => return fail(400, "bad"),
            },
        };
        if !self.member(&pid, fp) {
            return fail(404, "no_pairing");
        }
        self.status.insert((pid.to_string(), fp.to_string()), Status { blob, at: now });
        self.later.retain(|l| !(*l.pid == *pid && l.by == fp));
        let fresh: Vec<Later> = later.into_iter().filter(|l| !self.later_seen(&l.pid, &l.id)).collect();
        self.later.extend(fresh);
        done()
    }

    /// push_note（宿主发的）：一条额度的通知（额度用完了：`{"pid", "id", "note"}`，封给这个配对的），马上交给推送网关。配对的
    /// 另一头得是登记了推送、要额度的通知的访客；推不了（没开推送、对方没登记、不要）时回的 sent 是 false
    fn push_note(&mut self, fp: &str, j: &Dict) -> Reply {
        let id = j.str("id").filter(|i| wire::note_id_re().matches(i));
        let note = j.get("note").and_then(|n| wire::parse_blob(&n, PUSH_BLOB));
        let (Some(pid), Some(id), Some(note)) = (hex32(j.get("pid")), id, note) else { return fail(400, "bad") };
        if !self.member(&pid, fp) {
            return fail(404, "no_pairing");
        }
        let job = self.pusher.as_ref().zip(self.usage_job(&pid, fp, &id, &note));
        let sent = job.is_some();
        if let Some((p, job)) = job {
            p.send(job);
        }
        (200, V::Dict(crate::dict! { "v" => API, "ok" => true, "sent" => sent }))
    }

    /// pair_open（Py:1102-1112）：显示配对码的设备开一次配对
    fn pair_open(&mut self, rid: &str, fp: &str, j: &Dict, now: f64) -> Reply {
        if self.slots.contains_key(rid) {
            return fail(409, "taken");
        }
        if self.slots.values().filter(|x| x.owner == fp).count() >= self.lim.pair_slots {
            return fail(429, "rate");
        }
        // 这次配对开给手机：输入配对码的可以是不在名单里的（iOS 上的 cca App 带 guest；旧版 cca 开给网页端，带 web）
        let web = matches!(j.get("web"), Some(V::Bool(true)));
        let asked = matches!(j.get("guest"), Some(V::Bool(true)));
        let guest = web || asked;
        let web_url = self.web().filter(|_| self.guests_on).map(str::to_string);
        if web && web_url.is_none() {
            return fail(404, "no_web");
        }
        if guest && !self.guests_on {
            return fail(404, "no_guest");
        }
        let reply = crate::dict! { "v" => API, "ok" => true, "ttl" => PAIR_TTL };
        // 带了 guest 的才回 guest（旧版 cca 只带 web：回的和以前一样）
        if asked {
            reply.set("guest", true);
        }
        if let (true, Some(url)) = (web, web_url) {
            reply.set("web", url);
        }
        self.serial += 1;
        let slot = Slot {
            serial: self.serial,
            owner: fp.to_string(),
            joiner: None,
            msgs: HashMap::new(),
            commit: HashMap::new(),
            done: false,
            exp: now + PAIR_TTL as f64,
            guest,
            gkey: None,
        };
        self.slots.insert(rid.to_string(), slot);
        (200, V::Dict(reply))
    }

    /// pair_open 以外的几个配对的 op 开头都要过的（Py:1113-1120）：有没有这次配对；访客只进得了开给手机的；头一个放密文进来的
    /// 别的设备坐下（是访客的话记下它的公钥：配成了才留下）；不是这次配对两边之一的不行。过了是 None，不过是要回的内容
    fn pair_enter(&mut self, rid: &str, op: &str, fp: &str, gkey: Option<&str>) -> Option<Reply> {
        let Some(s) = self.slots.get_mut(rid) else { return Some(fail(404, "no_pairing")) };
        if gkey.is_some() && !s.guest {
            return Some(fail(403, "denied"));
        }
        if fp != s.owner && s.joiner.is_none() && op == "pair_put" {
            s.joiner = Some(fp.to_string());
            s.gkey = gkey.map(str::to_string);
        }
        (!s.party(fp)).then(|| fail(409, "taken"))
    }

    /// pair_put（Py:1126-1132）：放一份密文，一边只能放一次
    fn pair_put(&mut self, rid: &str, fp: &str, j: &Dict) -> Reply {
        let m = j.get("m").and_then(|m| wire::parse_blob(&m, PAIR_MSG));
        match (self.slots.get_mut(rid), m) {
            (Some(s), Some(m)) if !s.msgs.contains_key(fp) => {
                s.msgs.insert(fp.to_string(), m);
                done()
            }
            _ => fail(400, "bad"),
        }
    }

    /// pair_commit（Py:1153-1181）：报配对编号；两边报的一样，配对才记下。回的内容，和要不要叫醒等着的 pair_get
    fn pair_commit(&mut self, rid: &str, fp: &str, j: &Dict, now: f64) -> R<(Reply, bool)> {
        let pid = hex32(j.get("pid"));
        let Some(s) = self.slots.get_mut(rid) else { return Ok((fail(404, "no_pairing"), false)) };
        let (Some(pid), Some(joiner)) = (pid, s.joiner.clone()) else { return Ok((fail(400, "bad"), false)) };
        s.commit.insert(fp.to_string(), pid.to_string());
        let owner = s.owner.clone();
        let both = (s.commit.get(&owner).cloned(), s.commit.get(&joiner).cloned());
        let (already, gkey) = (s.done, s.gkey.clone());
        let (Some(first), Some(second), false) = (both.0, both.1, already) else {
            return Ok(((200, V::Dict(crate::dict! { "v" => API, "ok" => true, "done" => already })), false));
        };
        if first != second || self.edges.contains(&pid) {
            self.slot_pop(rid);
            return Ok((fail(409, "mismatch"), true));
        }
        for who in [&owner, &joiner] {
            if self.edges.values().filter(|e| e.has(who)).count() >= self.lim.edges_dev {
                self.slot_pop(rid);
                return Ok((fail(429, "edges"), true));
            }
        }
        if self.edges.len() >= self.lim.edges_max {
            return Ok((fail(507, "full"), false));
        }
        // 同两台设备以前配过的换掉（重新配对）
        let old: Vec<String> = self.edges.iter().filter(|(_, e)| e.joins(&owner, &joiner)).map(|(p, _)| p.to_string()).collect();
        for p in &old {
            self.drop_edge(p);
        }
        self.edges.insert(pid.to_string(), Edge::new(&owner, &joiner, now));
        // 输入配对码的是访客：两边算出了同一把密钥（它确实知道配对码），这才记下它
        if let Some(key) = gkey.filter(|k| !k.is_empty()) {
            self.guests.insert(joiner, Guest { key, at: Val::Num(Num::Float(now)), push: None });
        }
        if let Some(s) = self.slots.get_mut(rid) {
            s.done = true;
            s.exp = s.exp.min(now + 60.0); // 留一会儿，先报的那一边还要来问一次
        }
        self.save()?;
        Ok(((200, V::Dict(crate::dict! { "v" => API, "ok" => true, "done" => true })), true))
    }

    /// pair_drop（Py:1182-1189）：取消一个配对（哪一边都可以）；不是自己的、没有这个配对也回成功
    fn pair_drop(&mut self, fp: &str, j: &Dict) -> R<Reply> {
        let Some(pid) = hex32(j.get("pid")) else { return Ok(fail(400, "bad")) };
        if self.member(&pid, fp) {
            self.drop_edge(&pid);
            self.save()?;
        }
        Ok(done())
    }

    /// 请求里的一串配对编号（Py:1192、1218）：是列表、不比一台设备能有的配对多、个个是 32 个十六进制字符；重复的只留头一个
    fn pid_list(&self, pids: Option<V>) -> Option<Vec<Rc<str>>> {
        let pids = pids?.as_list()?;
        if pids.len() > self.lim.edges_dev {
            return None;
        }
        let mut out: Vec<Rc<str>> = Vec::new();
        for p in pids.items() {
            let p = hex32(Some(p))?;
            if !out.contains(&p) {
                out.push(p);
            }
        }
        Some(out)
    }

    /// relay_host（Py:1190-1202）：宿主要一张票据，拿它在数据端口上开（或者接回）这个会话的控制连接
    fn relay_host(&mut self, fp: &str, j: &Dict, now: f64) -> R<Reply> {
        let (Some(sid), Some(pids)) = (hex32(j.get("sid")), self.pid_list(j.get("pids")).filter(|p| !p.is_empty())) else {
            return Ok(fail(400, "bad"));
        };
        let pids: Vec<String> = pids.iter().filter(|p| self.member(p, fp)).map(|p| p.to_string()).collect();
        if pids.is_empty() {
            return Ok(fail(404, "no_pairing"));
        }
        match self.sessions.get(&sid) {
            Some(cur) if cur.fp != fp => return Ok(fail(409, "taken")),
            None if self.sessions.values().filter(|x| x.fp == fp).count() >= self.lim.sessions_dev => {
                return Ok(fail(429, "sessions"));
            }
            _ => {}
        }
        self.ticket(now, Role::Host, fp, sid.to_string(), pids, None)
    }

    /// relay_attach（Py:1203-1215）：接入方要一张票据。这个会话得是配对的另一头开的、给这个配对看的
    fn relay_attach(&mut self, fp: &str, j: &Dict, now: f64) -> R<Reply> {
        let (Some(sid), Some(pid)) = (hex32(j.get("sid")), hex32(j.get("pid"))) else { return Ok(fail(400, "bad")) };
        let host = if self.member(&pid, fp) { self.other(&pid, fp) } else { None };
        let viewers = match (self.sessions.get(&sid), host) {
            (Some(x), Some(host)) if x.pids.iter().any(|p| **p == *pid) && x.fp == host => x.viewers,
            _ => return Ok(fail(404, "no_session")),
        };
        if viewers >= self.lim.viewers {
            return Ok(fail(429, "busy"));
        }
        // 最近用过：一天最多记一次、写一次盘（180 天没用过的配对会删掉）
        if let Some(e) = self.edges.get_mut(&pid) {
            if now - e.secs("used") > 86400.0 {
                e.set_used(now);
                self.save()?;
            }
        }
        self.ticket(now, Role::View, fp, sid.to_string(), Vec::new(), Some(pid.to_string()))
    }

    /// relay_list（Py:1216-1229）：这几个配对的另一头是谁、它开着哪些给这个配对看的会话。带了 have（`{配对编号: 手里那一份的 n}`，
    /// 可以是空的）的（3.7.5 起的手机）回的里面多一项 status：另一头放上来的账号的状态（`{配对编号: {"blob", "age"}}`，和手里那一份
    /// 一样的是 `{"same": true, "age"}`；没放过的不在里面）。不带 have 的回的和以前一样
    fn relay_list(&self, fp: &str, j: &Dict, now: f64, name_of: &dyn Fn(&str) -> Option<String>) -> Reply {
        let Some(pids) = self.pid_list(j.get("pids")) else { return fail(400, "bad") };
        let have = match j.get("have") {
            None => None,
            Some(V::Dict(d)) => Some(d),
            Some(_) => return fail(400, "bad"),
        };
        let (out, peers, status) = (Dict::new(), Dict::new(), Dict::new());
        for pid in &pids {
            // 对方取消了配对（或者从来没有）：不在回的里面
            let Some(peer) = self.other(pid, fp).filter(|_| self.member(pid, fp)) else { continue };
            peers.set(pid.clone(), crate::dict! { "fp" => peer, "name" => name_of(peer) });
            if let (Some(have), Some(s)) = (&have, self.status.get(&(pid.to_string(), peer.to_string()))) {
                let age = num::round((now - s.at).max(0.0));
                let row = match have.str(pid).is_some_and(|n| *n == *s.blob.n) {
                    true => crate::dict! { "same" => true, "age" => age },
                    false => crate::dict! { "blob" => s.blob.to_v(), "age" => age },
                };
                status.set(pid.clone(), row);
            }
            let rows: Vec<V> = self
                .sessions
                .iter()
                .filter(|(_, x)| x.fp == peer)
                .filter_map(|(sid, x)| {
                    let row = crate::dict! {
                        "sid" => sid,
                        "blob" => x.blobs.get(pid)?.to_v(),
                        "age" => num::round((now - x.since).max(0.0)), // ago()（Py:397-399）
                        "viewers" => x.viewers,
                        "online" => x.conn.is_some(),
                    };
                    Some(V::Dict(row))
                })
                .collect();
            out.set(pid.clone(), rows);
        }
        let reply = crate::dict! { "v" => API, "ok" => true, "peers" => peers, "sessions" => out };
        if have.is_some() {
            reply.set("status", status);
        }
        (200, V::Dict(reply))
    }
}

/// status_put 的 later：列表，最多 `LATER_MAX` 条，每条 `{"slot", "at", "id", "note"}`（slot、id 是 16 个小写的十六进制字符，at 是
/// Unix 时间、不晚于 `LATER_SPAN` 以后，note 是封好的通知）；slot 不能重复。样子不对是 None
fn later_of(v: &V, pid: &str, by: &str, now: f64) -> Option<Vec<Later>> {
    let list = v.as_list()?;
    if list.len() > LATER_MAX {
        return None;
    }
    let mut out: Vec<Later> = Vec::new();
    for x in list.items() {
        let d = x.as_dict()?;
        let slot = d.str("slot").filter(|s| wire::note_id_re().matches(s))?;
        let id = d.str("id").filter(|s| wire::note_id_re().matches(s))?;
        let at = match d.get("at") {
            Some(V::Int(i)) => i as f64,
            Some(V::Float(f)) => f,
            _ => return None,
        };
        let note = wire::parse_blob(&d.get("note")?, PUSH_BLOB)?;
        if !(at.is_finite() && at > 0.0 && at <= now + LATER_SPAN as f64) || out.iter().any(|l| *l.slot == *slot) {
            return None;
        }
        out.push(Later { pid: pid.to_string(), by: by.to_string(), slot: slot.to_string(), at, id: id.to_string(), note });
    }
    Some(out)
}
