//! 设备上报的会话和看它们：report、prune、view（Py:387-399、535-563、873-897）。`impl Store` 的一部分，由 `Store::op` 调。

use super::{or_zero, truthy, Beat, Machine, Store, Unit};
use crate::dict;
use crate::py::num;
use crate::py::obj::{Dict, List, V};
use crate::py::R;
use crate::server::omap::OMap;
use crate::server::wire::{Num, UnitIn};
use crate::server::API;

/// 一台设备上一个账号开着的会话：命令行会话加桌面 App，每个是 [开始, 最后动静]（服务器时间）（Py:387-389）
pub(super) fn sessions(e: &Unit) -> Vec<[Num; 2]> {
    let mut all = e.cli.clone();
    all.extend(e.app);
    all
}

/// 一台设备上一个账号最后一次有动静：记下的上次使用和开着的会话里最晚的（Py:392-394）。和 Python 的 max 一样，
/// 一样大的留前面那个（所以没有会话、没用过时是整数 0）
pub(super) fn last_used(e: &Unit) -> Num {
    let mut best = or_zero(e.used);
    for [_, last] in sessions(e) {
        if last.f() > best.f() {
            best = last;
        }
    }
    best
}

/// t 以来多少秒（取整，不为负）（Py:397-399）
pub(super) fn ago(now: f64, t: f64) -> i64 {
    num::round((now - t).max(0.0))
}

/// `ago(now, t) if t else None`（Py:877）
fn age(now: f64, t: Num) -> Option<i64> {
    truthy(t).then(|| ago(now, t.f()))
}

impl Store {
    /// 一台设备报它现在开着的会话（Py:535-548）：units 是 `parse_units` 的结果（每个会话是开了多久、闲置多久），
    /// 换成服务器的时间记下来；这次没报的账号会话都关了（上次使用照留）
    pub(super) fn report(&mut self, fp: &str, units: &OMap<UnitIn>, end: Option<&str>, now: f64) -> R<()> {
        self.beats.insert(fp, Beat { seen: Some(Num::Float(now)), end: end.map(str::to_string) });
        let cur = self.units.setdefault(fp, OMap::new);
        cur.for_each_mut(|aid, e| {
            if !units.contains(aid) {
                e.cli.clear();
                e.app = None;
            }
        });
        let since = |p: &[f64; 2]| {
            let [age, idle] = *p;
            [Num::Float(now - age), Num::Float(now - idle)]
        };
        for (aid, u) in units.iter() {
            let e = cur.setdefault(aid, || Unit { cli: Vec::new(), app: None, used: Some(Num::Int(0)) });
            e.cli = u.cli.iter().map(since).collect();
            e.app = u.app.as_ref().map(since);
            e.used = Some(last_used(e));
        }
        self.prune(now);
        self.save()
    }

    /// 不在名单里的设备的、retain 秒没心跳的设备的记录不留；没有会话、上次使用也过了 retain 秒的账号不留（Py:550-563）
    pub(super) fn prune(&mut self, now: f64) {
        let old = now - self.cfg.retain;
        let (machines, units) = (&self.machines, &mut self.units);
        self.beats.retain(|fp, b| {
            let gone = !machines.contains(fp) || b.seen.map_or(0.0, Num::f) < old;
            if gone {
                units.remove(fp);
            }
            !gone
        });
        let beats = &self.beats;
        self.units.retain(|fp, accts| {
            if !machines.contains(fp) || !beats.contains(fp) {
                return false;
            }
            accts.retain(|_, e| !(sessions(e).is_empty() && or_zero(e.used).f() < old));
            true
        });
    }

    /// 一台设备上一个账号的那一行（Py:873-887）。state：offline（开着会话却没心跳了、设备说它要关了）、none（没有会话）、
    /// in_use、idle
    fn row(&self, fp: &str, m: &Machine, e: &Unit, now: f64) -> Dict {
        let beat = self.beats.get(fp);
        let seen = or_zero(beat.and_then(|b| b.seen));
        let down = beat.is_some_and(|b| b.end.as_deref() == Some("down"));
        let units = sessions(e);
        // 开着的会话最后一次有动静、最早的开始（max、min 的 default=0）
        let (mut last, mut first) = (None::<Num>, None::<Num>);
        for [start, active] in &units {
            if last.is_none_or(|l| active.f() > l.f()) {
                last = Some(*active);
            }
            if first.is_none_or(|f| start.f() < f.f()) {
                first = Some(*start);
            }
        }
        let (last, first) = (last.unwrap_or(Num::Int(0)), first.unwrap_or(Num::Int(0)));
        let state = if !units.is_empty() && (down || now - seen.f() > self.cfg.ttl) {
            "offline"
        } else if units.is_empty() {
            "none"
        } else if now - last.f() < self.cfg.idle {
            "in_use"
        } else {
            "idle"
        };
        dict! {
            "fp" => fp,
            "name" => &m.name,
            "state" => state,
            "cli" => e.cli.len(),
            "app" => e.app.is_some(),
            "since_age" => age(now, first),
            "idle_age" => age(now, last),
            "used_age" => e.used.and_then(|u| age(now, u)),
            "seen_age" => age(now, seen),
        }
    }

    /// 这些账号各在哪些设备上开着、上次什么时候用的（Py:889-897）。没有会话、上次使用也过了 retain 秒的不列
    pub(super) fn view(&self, me: &str, aids: &[String], now: f64) -> V {
        let out = Dict::new();
        for aid in aids {
            let rows = List::new();
            for (fp, accts) in self.units.iter() {
                let (Some(e), Some(m)) = (accts.get(aid), self.machines.get(fp)) else {
                    continue;
                };
                let row = self.row(fp, m, e, now);
                let none = row.str("state").is_some_and(|s| &*s == "none");
                let used_age = row.get("used_age").and_then(|a| a.as_int()).unwrap_or(0);
                if !none || (used_age as f64) < self.cfg.retain {
                    rows.push(row);
                }
            }
            out.set(aid.as_str(), rows);
        }
        V::Dict(dict! {
            "v" => API,
            "me_name" => self.machines.get(me).map(|m| m.name.as_str()),
            "ttl" => self.ttl_out.v(),
            "idle" => self.idle_out.v(),
            "accounts" => out,
        })
    }
}
