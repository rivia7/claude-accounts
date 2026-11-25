//! Team 的各个 op：管理员登记设备、发邀请、存各台设备的配置、看名单、移除设备，设备拿邀请加入、取自己的配置、退出
//! （Py:651-675、690-871、1971-1975、1999-2026）。`impl Store` 的一部分，由 `Store::op` 调。
//! 谁发的、签名对不对在调用方验过了（管理员的是根密钥或者管理员名单里的密钥签的，换名单的只认根密钥，加入的是要加入的那把密钥签的）。

use super::view::ago;
use super::{bad, error, say, DocEntry, Inv, Machine, Store};
use crate::dict;
use crate::py::obj::{Dict, List, V};
use crate::py::{hash, num, text, R};
use crate::server::omap::OMap;
use crate::server::sshsig;
use crate::server::wire::{self, Blob, DocIn, Enroll, Num};
use crate::server::{API, INVITE_KEEP, INVITE_TTL, MAX_ASTATE, SEQ_MAX};
use std::collections::HashSet;

impl Store {
    /// 名字被别的设备用了就在后面加 -2、-3……（最长 64 个字符）（Py:651-659）
    fn free_name(&self, name: &str, fp: &str) -> String {
        let taken: HashSet<&str> = self.machines.iter().filter(|(f, _)| *f != fp).map(|(_, m)| m.name.as_str()).collect();
        for i in 1..taken.len() + 2 {
            let tail = if i == 1 { String::new() } else { format!("-{i}") };
            let cand = format!("{}{tail}", text::head(name, 64usize.saturating_sub(tail.len())));
            if !taken.contains(cand.as_str()) {
                return cand;
            }
        }
        name.to_string()
    }

    /// 名单里的一台设备：什么时候加入的、最近一次请求（内存里记的、上报的心跳，取新的）（Py:661-665）
    fn member_row(&self, f: &str, m: &Machine, now: f64) -> Dict {
        let mem = self.seen.get(f).copied().unwrap_or(0.0);
        let beat = self.beats.get(f).and_then(|b| b.seen).map_or(0.0, Num::f);
        // max(a or 0, b or 0)，然后 `if seen`：都没有（0）就是 None
        let seen = if beat > mem { beat } else { mem };
        dict! {
            "fp" => f,
            "name" => &m.name,
            "joined_age" => m.joined_time().map(|j| ago(now, j.f())),
            "seen_age" => (seen != 0.0).then(|| ago(now, seen)),
        }
    }

    /// 一台设备的密钥就此不认，它在这里的记录跟着删掉：上报的、传过的折合金额（forget_gone；记下的 API key 的密文是账号的，
    /// 留着）（Py:667-675）
    fn drop_machine(&mut self, fp: &str) -> R<()> {
        self.machines.remove(fp);
        self.beats.remove(fp);
        self.units.remove(fp);
        self.seen.remove(fp);
        self.have.remove(fp);
        self.forget_gone()
    }

    /// 认得的设备编号：加入了的、有邀请的（过期了、还没清掉的也算）（Py:692-694）
    fn team_devs(&self) -> HashSet<String> {
        self.machines.values().map(|m| m.dev.clone()).chain(self.invites.values().map(|i| i.dev.clone())).collect()
    }

    /// 不认得的编号的配置不要了（邀请过期很久被清掉了；移除设备时写了状态文件、还没写这一份就停了）（Py:696-703）
    pub(super) fn docs_prune(&mut self) -> R<()> {
        let known = self.team_devs();
        let before = self.docs.items.len();
        self.docs.items.retain(|dev, _| known.contains(dev));
        if self.docs.items.len() != before {
            self.save_docs()?;
        }
        Ok(())
    }

    /// 过期了、用掉了很久的邀请不记了，给它们存的配置跟着删掉（Py:705-713）
    fn invite_prune(&mut self, now: f64) -> R<()> {
        let keep = INVITE_KEEP as f64;
        let (invites, spent) = (self.invites.len(), self.spent.len());
        self.invites.retain(|_, i| now - i.exp.f() < keep);
        self.spent.retain(|_, t| now - t.f() < keep);
        if self.invites.len() != invites || self.spent.len() != spent {
            self.save()?;
            self.docs_prune()?;
        }
        Ok(())
    }

    /// 还没过期的邀请有几个
    fn live_invites(&self, now: f64) -> usize {
        self.invites.values().filter(|i| now < i.exp.f()).count()
    }

    /// 管理员直接登记设备的公钥（它自己的那台；服务器的记录没了以后把各台加回去；给设备改名字：同一把公钥、新的名字）：devs 是
    /// parse_enroll 的结果。这个编号原来的那把密钥就此不认（换了密钥：drop_machine，它传的折合金额跟着删掉），还没用掉的邀请作废；
    /// 公钥已经是别的编号的设备是 409。同一把公钥再登记：加入的时间、加入时带的认证码（proof）照旧（Py:727-752）
    fn team_enroll(&mut self, devs: &OMap<Enroll>, now: f64) -> R<(u16, V)> {
        if devs.values().any(|e| e.fp == self.root_fp) {
            return Ok(bad()); // 根密钥不是设备
        }
        if devs.iter().any(|(dev, e)| self.machines.get(&e.fp).is_some_and(|m| m.dev != dev)) {
            return Ok(error(409, "taken"));
        }
        let mut all: HashSet<&str> = self.machines.values().map(|m| m.dev.as_str()).collect();
        all.extend(devs.keys());
        if all.len() > self.lim.max_team_members {
            return Ok(error(507, "full"));
        }
        let names = Dict::new();
        for (dev, e) in devs.iter() {
            let replaced: Vec<String> =
                self.machines.iter().filter(|(f, m)| m.dev == dev && *f != e.fp).map(|(f, _)| f.to_string()).collect();
            for f in replaced {
                self.drop_machine(&f)?;
            }
            let old = self.machines.get(&e.fp);
            let joined = old.and_then(Machine::joined_time).unwrap_or(Num::Float(now));
            let proof = old.and_then(|m| m.proof.clone()).filter(|p| !p.is_empty());
            let name = self.free_name(&e.name, &e.fp);
            names.set(dev, &name);
            self.machines.insert(&*e.fp, Machine { name, key: e.key.clone(), joined: Some(joined), dev: dev.to_string(), proof });
        }
        self.invites.retain(|_, i| !devs.contains(&i.dev));
        self.save()?;
        Ok((
            200,
            V::Dict(dict! { "v" => API, "ok" => true, "names" => names, "members" => self.machines.len(), "gen" => &self.gen }),
        ))
    }

    /// 管理员发一个邀请：服务器只存令牌的 SHA-256，ttl 秒后过期，只能用一次。同一个编号再发，前一个作废；这个编号的设备已经
    /// 加入了是 409（先移除它）（Py:754-770）
    fn team_invite(&mut self, token_hash: &str, dev: &str, ttl: i64, now: f64) -> R<(u16, V)> {
        self.invite_prune(now)?;
        if self.machines.values().any(|m| m.dev == dev) {
            return Ok(error(409, "taken"));
        }
        if self.spent.contains(token_hash) {
            return Ok(error(409, "used")); // 用过的令牌不能再发一次
        }
        let live = self.invites.values().filter(|i| i.dev != dev && now < i.exp.f()).count();
        if live >= self.lim.max_invites {
            return Ok(error(507, "full"));
        }
        self.invites.retain(|_, i| i.dev != dev);
        self.invites.insert(token_hash, Inv { dev: dev.to_string(), exp: Num::Float(now + ttl as f64), at: Some(Num::Float(now)) });
        self.save()?;
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "invites" => self.live_invites(now), "gen" => &self.gen })))
    }

    /// team_join 的请求（Py:1971-1975）：名字、认证码的样子不对是 400。签名是拿请求里的 pub 验的、token 看过是个邀请
    /// （调用方，Py:1935-1944），这里照样再取一遍
    pub(super) fn join_op(&mut self, fp: &str, j: &Dict, now: f64) -> R<(u16, V)> {
        let proof = match j.get("proof") {
            None | Some(V::None) => None,
            Some(V::Str(p)) if wire::tag_re().matches(&p) => Some(p.to_string()),
            Some(_) => return Ok(bad()),
        };
        let line = j.str("pub");
        let b64 = line.as_deref().and_then(|l| wire::key_re().match_(l)).map(|k| k.g(1));
        let (Some(b64), Some(name), Some(token)) = (b64, j.str("name").filter(|n| wire::name_text_ok(n)), j.str("token")) else {
            return Ok(bad());
        };
        self.team_join(fp, &format!("ssh-ed25519 {b64}"), &name, &token, now, proof)
    }

    /// 设备拿邀请里的令牌加入：令牌就此用掉，这把密钥记成邀请里的那个编号。proof 是设备拿邀请里那串随机数给自己的公钥算的认证码
    /// （服务器不懂，原样存着、回给管理员：管理员据此知道这把公钥真是拿着邀请的那台设备的，不是服务器换的）（Py:772-794）
    fn team_join(&mut self, fp: &str, key: &str, name: &str, token: &str, now: f64, proof: Option<String>) -> R<(u16, V)> {
        let h = hash::sha256_hex(token);
        let dev = match self.invites.get(&h) {
            Some(inv) if now < inv.exp.f() => inv.dev.clone(),
            // 验签之后、拿到锁之前被别的设备用掉了、过期了
            Some(_) => return Ok(error(403, "expired")),
            None => return Ok(error(403, "used")),
        };
        if self.machines.contains(fp) {
            return Ok(error(409, "taken")); // 这把密钥已经是一台设备了（管理员登记的、用过别的邀请）：先让管理员移除它
        }
        if self.machines.len() >= self.lim.max_team_members {
            return Ok(error(507, "full"));
        }
        let name = self.free_name(name, fp);
        let m = Machine {
            name: name.clone(),
            key: key.to_string(),
            joined: Some(Num::Float(now)),
            dev: dev.clone(),
            proof: proof.filter(|p| !p.is_empty()),
        };
        self.machines.insert(fp, m);
        self.invites.remove(&h);
        self.spent.insert(h, Num::Float(now));
        self.seen.insert(fp.to_string(), now);
        self.save()?;
        say(format!("cca-presence: 加入了一台设备，现在 {} 台", self.machines.len()));
        Ok((
            200,
            V::Dict(
                dict! { "v" => API, "ok" => true, "name" => name, "dev" => dev, "members" => self.machines.len(), "gen" => &self.gen },
            ),
        ))
    }

    /// 管理员存各台设备的配置（parse_team_docs 的结果）：序号要比存着的大（一样、认证码也一样的算存过了：没收到回复再发一次）；
    /// 不认得的编号（被移除了、邀请过期很久了）、序号不比存着的大的不存，回给管理员（Py:796-818）
    fn team_put(&mut self, docs: &OMap<DocIn>, now: f64) -> R<(u16, V)> {
        self.invite_prune(now)?;
        let known = self.team_devs();
        let (mut stored, stale, unknown) = (Vec::new(), Dict::new(), List::new());
        for (dev, d) in docs.iter() {
            match self.docs.items.get(dev) {
                _ if !known.contains(dev) => unknown.push(dev),
                Some(have) if d.seq < have.seq || (d.seq == have.seq && d.ct.t != have.ct.t) => stale.set(dev, have.seq),
                _ => stored.push((dev, d)),
            }
        }
        let size = |e: &DocEntry| e.ct.c.len();
        let kept: usize =
            self.docs.items.iter().filter(|(dev, _)| !stored.iter().any(|(s, _)| s == dev)).map(|(_, e)| size(e)).sum();
        let coming: usize = stored.iter().map(|(_, d)| d.ct.c.len()).sum();
        if kept + coming > self.docs_bytes {
            return Ok(error(507, "full"));
        }
        for (dev, d) in &stored {
            let e = DocEntry { seq: d.seq, ct: d.ct.clone(), sig: d.sig.clone(), by: d.by.clone(), at: Some(Num::Float(now)) };
            self.docs.items.insert(*dev, e);
        }
        if !stored.is_empty() {
            self.save_docs()?;
        }
        let stored = List::from_vec(stored.iter().map(|(dev, _)| V::str(dev)).collect());
        Ok((
            200,
            V::Dict(
                dict! { "v" => API, "ok" => true, "stored" => stored, "stale" => stale, "unknown" => unknown, "gen" => &self.gen },
            ),
        ))
    }

    /// 设备取自己的那一份配置：have 是它手里那一份的序号（记下来，管理员看名单时看得到各台用上了没有），一样就不带（doc 是 null）；
    /// seq 是存着的序号（还没有是 0）（Py:820-831）
    fn team_get(&mut self, fp: &str, have: Option<i64>) -> R<(u16, V)> {
        let Some(m) = self.machines.get(fp) else {
            // 验签之后、拿到锁之前被移除了
            return Ok((403, V::Dict(dict! { "error" => "unknown_device", "gen" => &self.gen })));
        };
        match have {
            Some(seq) => self.have.insert(fp.to_string(), seq),
            None => self.have.remove(fp),
        };
        let e = self.docs.items.get(&m.dev);
        let doc = e.filter(|e| Some(e.seq) != have).map(|e| {
            let d = dict! { "seq" => e.seq, "ct" => e.ct.to_v(), "sig" => &e.sig };
            if let Some(by) = &e.by {
                d.set("by", by.as_str());
            }
            d
        });
        Ok((
            200,
            V::Dict(dict! {
                "v" => API,
                "ok" => true,
                "dev" => &m.dev,
                "me_name" => &m.name,
                "seq" => e.map_or(0, |e| e.seq),
                "doc" => doc,
                "gen" => &self.gen,
            }),
        ))
        .map(|(code, v)| (code, self.with_admins(v)))
    }

    /// 有管理员名单（3.6.15 起）时回复里带上它（`admins`: {list, sig}；没有时不带这一项：和以前的回复逐字节一样）
    fn with_admins(&self, v: V) -> V {
        if let (Some(a), Some(d)) = (&self.admins, v.as_dict()) {
            let gen = d.pop("gen");
            d.set("admins", a.to_v());
            if let Some(g) = gen {
                d.set("gen", g);
            }
        }
        v
    }

    /// 根密钥换管理员名单（parse_admins 验过签名的）：纪元要比存着的大，不然 409 stale（带着存着的纪元）
    fn team_admins(&mut self, a: wire::Admins) -> R<(u16, V)> {
        let cur = self.admins.as_ref().map_or(0, |a| a.epoch);
        if a.epoch <= cur {
            return Ok((409, V::Dict(dict! { "error" => "stale", "epoch" => cur })));
        }
        let (epoch, n) = (a.epoch, a.keys.len());
        self.admins = Some(a);
        if !self.team_keys.contains(&super::TeamKey::Admins) {
            self.team_keys.push(super::TeamKey::Admins);
        }
        self.save()?;
        say(format!("cca-presence: 管理员名单换成了第 {epoch} 版：除了根密钥还有 {n} 位"));
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "epoch" => epoch, "gen" => &self.gen })))
    }

    /// 几位管理员共用的状态（密文，服务器不懂）：rev 是版本号（还没有是 0，ct 是 null）
    fn team_astate_get(&self) -> (u16, V) {
        let (rev, ct) = self.astate.as_ref().map_or((0, V::None), |(r, c)| (*r, c.to_v()));
        (200, V::Dict(dict! { "v" => API, "ok" => true, "rev" => rev, "ct" => ct, "gen" => &self.gen }))
    }

    /// 存共用的状态：base 是它照着改的那一版（还没有是 0），要和存着的一样，存成 base + 1；不一样是 409 stale（带着存着的版本号）
    fn team_astate_put(&mut self, base: i64, ct: Blob) -> R<(u16, V)> {
        let cur = self.astate.as_ref().map_or(0, |(r, _)| *r);
        if base != cur || base + 1 >= SEQ_MAX {
            return Ok((409, V::Dict(dict! { "error" => "stale", "rev" => cur })));
        }
        self.astate = Some((base + 1, ct));
        if !self.team_keys.contains(&super::TeamKey::AState) {
            self.team_keys.push(super::TeamKey::AState);
        }
        self.save()?;
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "rev" => base + 1, "gen" => &self.gen })))
    }

    /// 管理员看现在有哪些设备（公钥也带着：服务器的记录没了以后照它登记回去）、它们各自存着的配置的序号（seq）和设备同步配置时
    /// 说的手里那一份的序号（have：重启以后、还没同步过是 null）、还没用掉的邀请（Py:833-844）
    fn team_members(&mut self, now: f64) -> R<(u16, V)> {
        self.invite_prune(now)?;
        let seq = |dev: &str| self.docs.items.get(dev).map_or(0, |e| e.seq);
        let rows = List::new();
        for (f, m) in self.machines.iter() {
            let row = self.member_row(f, m, now);
            row.set("dev", &m.dev);
            row.set("pub", &m.key);
            row.set("proof", m.proof.as_deref());
            row.set("seq", seq(&m.dev));
            row.set("have", self.have.get(f).copied());
            rows.push(row);
        }
        let invites = List::new();
        for i in self.invites.values().filter(|i| now < i.exp.f()) {
            invites.push(dict! { "dev" => &i.dev, "expires_in" => num::round(i.exp.f() - now), "seq" => seq(&i.dev) });
        }
        let reply = V::Dict(dict! { "v" => API, "ok" => true, "members" => rows, "invites" => invites, "gen" => &self.gen });
        Ok((200, self.with_admins(reply)))
    }

    /// 管理员移除一台设备：它的密钥就此不认，给它的配置、还没用掉的邀请、它传过的折合金额一起删掉（Py:846-863）
    fn team_revoke(&mut self, dev: &str) -> R<(u16, V)> {
        let fps: Vec<String> = self.machines.iter().filter(|(_, m)| m.dev == dev).map(|(f, _)| f.to_string()).collect();
        let pending = self.invites.values().any(|i| i.dev == dev);
        if fps.is_empty() && !pending && !self.docs.items.contains(dev) {
            return Ok(error(404, "no_device"));
        }
        for f in &fps {
            self.drop_machine(f)?;
        }
        self.invites.retain(|_, i| i.dev != dev);
        self.save()?;
        self.docs_prune()?;
        let n = self.machines.len();
        if !fps.is_empty() {
            say(format!("cca-presence: 移除了一台设备，现在 {n} 台"));
        }
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "members" => n })))
    }

    /// 设备自己退出：和被管理员移除一样（Py:865-871）
    fn team_leave(&mut self, fp: &str) -> R<(u16, V)> {
        let Some(dev) = self.machines.get(fp).map(|m| m.dev.clone()) else {
            // 验签之后、拿到锁之前被移除了
            return Ok((403, V::Dict(dict! { "error" => "unknown_device", "gen" => &self.gen })));
        };
        self.team_revoke(&dev)
    }

    /// Team 的服务器上除了加入以外的 op（Py:1999-2026）：请求里的各项不合格是 400
    pub(super) fn team_op(&mut self, op: &str, fp: &str, j: &Dict, now: f64) -> R<(u16, V)> {
        match op {
            "team_members" => self.team_members(now),
            "team_get" => match j.get("have") {
                None | Some(V::None) => self.team_get(fp, None),
                Some(have) if wire::seq_ok(&have) => self.team_get(fp, have.as_int()),
                Some(_) => Ok(bad()),
            },
            "team_invite" => {
                let token = j.str("token").filter(|t| wire::hash_re().matches(t));
                let dev = j.str("dev").filter(|d| wire::dev_re().matches(d));
                let ttl = j.get("ttl").and_then(|t| t.as_int()).filter(|t| (INVITE_TTL.0..=INVITE_TTL.1).contains(t));
                match (token, dev, ttl) {
                    (Some(token), Some(dev), Some(ttl)) => self.team_invite(&token, &dev, ttl, now),
                    _ => Ok(bad()),
                }
            }
            "team_put" => match j.get("docs").as_ref().and_then(wire::parse_team_docs) {
                Some(docs) => self.team_put(&docs, now),
                None => Ok(bad()),
            },
            "team_enroll" => match j.get("devices").as_ref().and_then(wire::parse_enroll) {
                Some(devs) => self.team_enroll(&devs, now),
                None => Ok(bad()),
            },
            "team_revoke" => match j.str("dev").filter(|d| wire::dev_re().matches(d)) {
                Some(dev) => self.team_revoke(&dev),
                None => Ok(bad()),
            },
            "team_leave" => self.team_leave(fp),
            "team_admins" => {
                let blob = sshsig::key_blob(&self.root_key).unwrap_or_default();
                let list = j.get("list").unwrap_or(V::None);
                match wire::parse_admins(&list, &j.get("sig").unwrap_or(V::None), &blob, &self.team_id) {
                    Some(a) => self.team_admins(a),
                    None => Ok(bad()),
                }
            }
            "team_astate_get" => Ok(self.team_astate_get()),
            "team_astate_put" => {
                let base = j.get("base").and_then(|b| match b {
                    V::Int(i) if (0..SEQ_MAX).contains(&i) => Some(i),
                    _ => None,
                });
                match (base, j.get("ct").and_then(|c| wire::parse_blob(&c, MAX_ASTATE))) {
                    (Some(base), Some(ct)) => self.team_astate_put(base, ct),
                    _ => Ok(bad()),
                }
            }
            _ => Ok(bad()),
        }
    }
}
