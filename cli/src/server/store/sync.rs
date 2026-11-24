//! 存着的密文：API key（key_sync）和折合金额（cost_sync、cost_missing、cost_drop、cost_prune、forget_gone）（Py:565-688）。
//! `impl Store` 的一部分，由 `Store::op` 调。服务器不懂密文里面是什么，只管存、按账号和设备给回去。

use super::view::{ago, last_used};
use super::{error, or_zero, truthy, CostEntry, KeyEntry, Store};
use crate::dict;
use crate::py::obj::{Dict, List, V};
use crate::py::R;
use crate::server::omap::OMap;
use crate::server::wire::{Blob, Num, SyncIn};
use crate::server::API;
use std::collections::HashSet;

impl Store {
    /// API key 的密文：先存这台设备传的（整个换掉），再回问的和传的那些账号现在的（没有的不回）。账号太多时不收新的（Py:565-585）
    pub(super) fn key_sync(&mut self, fp: &str, put: &OMap<Blob>, get: &[String], now: f64) -> R<(u16, V)> {
        // len(set(acc) | set(put)) > MAX_KEYS
        let new = put.keys().filter(|aid| !self.keys.items.contains(aid)).count();
        if self.keys.items.len() + new > self.lim.max_keys {
            return Ok(error(507, "full"));
        }
        let Some(name) = self.machines.get(fp).map(|m| m.name.clone()) else {
            return Ok(error(403, "denied")); // 验签之后、拿到锁之前这台被移除了
        };
        for (aid, blob) in put.iter() {
            let e = KeyEntry { blob: blob.clone(), fp: Some(fp.to_string()), name: Some(name.clone()), at: Some(Num::Float(now)) };
            self.keys.items.insert(aid, e);
        }
        if !put.is_empty() {
            self.save_keys()?;
        }
        let out = Dict::new();
        for aid in get.iter().map(String::as_str).chain(put.keys()) {
            if out.has(aid) {
                continue;
            }
            if let Some(e) = self.keys.items.get(aid) {
                out.set(
                    aid,
                    dict! {
                        "blob" => e.blob.to_v(),
                        "name" => e.name.as_deref(),
                        "me" => e.fp.as_deref() == Some(fp),
                        "age" => ago(now, or_zero(e.at).f()),
                    },
                );
            }
        }
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "me_name" => name, "keys" => out })))
    }

    /// 折合金额的密文：先存这台设备传的（每个账号一份、只写自己的那一份），再回问的那些账号各台设备的（have 里认证码一样的、
    /// 这台设备自己的不带密文；只回名单里的设备的，revoked 总是 false：移除设备时它的已经删了），和 retain 秒内经 cca 用过
    /// 这个账号、却没传过的设备（missing）（Py:587-622）
    pub(super) fn cost_sync(&mut self, fp: &str, sync: &SyncIn, now: f64) -> R<(u16, V)> {
        let Some(me_name) = self.machines.get(fp).map(|m| m.name.clone()) else {
            return Ok(error(403, "denied"));
        };
        let put = &sync.put;
        if !put.is_empty() {
            self.cost_prune(now); // 先清掉很久没再传的：不然存满了以后谁都传不上来，也就一直清不掉
            let acc = &self.cost.items;
            // 这台设备存着的和这次传的（同一个账号的算这次的）：几个账号、一共多大
            let kept = acc.iter().filter(|(aid, e)| e.contains(fp) && !put.contains(aid));
            let (mut mine_n, mut mine_bytes) = (put.len(), put.values().map(|b| b.c.len()).sum::<usize>());
            for (_, e) in kept {
                mine_n += 1;
                mine_bytes += e.get(fp).map_or(0, CostEntry::size);
            }
            // 所有设备存着的，这次要换掉的那几份不算，再加上这次传的
            let total: usize = acc
                .iter()
                .flat_map(|(aid, e)| e.iter().filter(move |(f, _)| !(*f == fp && put.contains(aid))))
                .map(|(_, x)| x.size())
                .sum();
            let coming: usize = put.values().map(|b| b.c.len()).sum();
            if mine_n > self.lim.cost_slots || mine_bytes > self.lim.cost_device_bytes || total + coming > self.lim.cost_total_bytes
            {
                return Ok(error(507, "full"));
            }
            for (aid, blob) in put.iter() {
                let e = CostEntry { blob: blob.clone(), name: Some(me_name.clone()), at: Some(Num::Float(now)) };
                self.cost.items.setdefault(aid, OMap::new).insert(fp, e);
            }
            self.save_cost()?;
        }
        let (out, missing) = (Dict::new(), Dict::new());
        let mut budget = self.lim.cost_resp;
        for aid in &sync.get {
            let rows = List::new();
            let slots = self.cost.items.get(aid);
            // 最近传过的先给密文（sorted 是稳定的：一样新的照存着的先后）
            let mut order: Vec<(&str, &CostEntry)> = slots.map(|s| s.iter().collect()).unwrap_or_default();
            order.sort_by(|a, b| b.1.at().total_cmp(&a.1.at()));
            for (f, e) in order {
                let Some(cur) = self.machines.get(f) else {
                    continue; // 不在名单里了（移除设备时就删了：这里只是保险）
                };
                let r = dict! {
                    "fp" => f,
                    "name" => &cur.name,
                    "me" => f == fp,
                    "revoked" => false,
                    "tag" => &e.blob.t,
                    "age" => ago(now, e.at()),
                };
                let had = sync.have.get(aid).and_then(|h| h.get(f));
                if f != fp && had != Some(&e.blob.t) && e.size() <= budget {
                    r.set("blob", e.blob.to_v());
                    budget -= e.size();
                }
                rows.push(r);
            }
            out.set(aid.as_str(), rows);
            missing.set(aid.as_str(), self.cost_missing(aid, now, fp));
        }
        Ok((200, V::Dict(dict! { "v" => API, "ok" => true, "me_name" => me_name, "cost" => out, "missing" => missing })))
    }

    /// retain 秒内经 cca 用过这个账号（上报的记录）、却从来没传过折合金额的设备；问的这台（me）不算：它先问、接着就传（Py:624-634）
    fn cost_missing(&self, aid: &str, now: f64, me: &str) -> List {
        let slots = self.cost.items.get(aid);
        let out = List::new();
        for (f, accts) in self.units.iter() {
            let (Some(e), Some(m)) = (accts.get(aid), self.machines.get(f)) else {
                continue;
            };
            if f == me || slots.is_some_and(|s| s.contains(f)) {
                continue;
            }
            let used = last_used(e);
            if truthy(used) && now - used.f() < self.cfg.retain {
                out.push(dict! { "name" => &m.name, "used_age" => ago(now, used.f()) });
            }
        }
        out
    }

    /// 删掉 gone(设备指纹, 那一份) 为真的折合金额，账号一份都不剩了也删掉；返回删了几份（Py:636-645）
    fn cost_drop(&mut self, mut gone: impl FnMut(&str, &CostEntry) -> bool) -> usize {
        let mut n = 0;
        self.cost.items.retain(|_, slots| {
            let before = slots.len();
            slots.retain(|f, e| !gone(f, e));
            n += before - slots.len();
            !slots.is_empty()
        });
        n
    }

    /// cost_keep 秒没再传过的不要了（那台设备早就不用 cca 了）（Py:647-649）
    fn cost_prune(&mut self, now: f64) {
        let keep = self.cfg.cost_keep;
        self.cost_drop(|_, e| now - e.at() > keep);
    }

    /// 不在名单里的设备的记录不留（移除设备、换了密钥时；启动时清掉以前的版本留下的）：传过的折合金额的密文删掉；记下的
    /// API key 的密文是账号的（别的设备还要用），留着，只是不再记着是哪台传的。有改动才写文件（Py:677-688）
    pub(super) fn forget_gone(&mut self) -> R<()> {
        let listed: HashSet<String> = self.machines.keys().map(str::to_string).collect();
        if self.cost_drop(|f, _| !listed.contains(f)) > 0 {
            self.save_cost()?;
        }
        let mut orphans = 0;
        for e in self.keys.items.values_mut() {
            // ('fp' in e or 'name' in e) and e.get('fp') not in ms
            if (e.fp.is_some() || e.name.is_some()) && !e.fp.as_ref().is_some_and(|f| listed.contains(f)) {
                e.fp = None;
                e.name = None;
                orphans += 1;
            }
        }
        if orphans > 0 {
            self.save_keys()?;
        }
        Ok(())
    }
}
