//! 记着先后的字典：Python 的 `dict`。
//!
//! 状态文件是照字典里各项的先后写出去的，Rust 版写的要和 Python 写的逐字节一样（两种服务器换着跑同一个目录，文件不该变样），
//! 所以共用的状态里凡是 Python 用字典的地方都用它：
//! - 新的键排在最后；给已有的键赋值，它留在原来的位置；删掉再加进来的排到最后
//! - 遍历、写出去都照这个先后
//! - `==` 和 Python 一样不看先后
//!
//! 值放在 `HashMap` 里、先后另记一份：这样 `setdefault` 这类“没有就加上、再拿来改”的写法不用下标、不用 `unwrap`。
//! 里面是普通的值（不是 `py::obj` 那种 `Rc` 的），可以放进 `Mutex` 跨线程。

use crate::py::obj::{Dict, V};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone)]
pub struct OMap<T> {
    /// 各个键，照加进来的先后
    order: Vec<String>,
    /// 键 → 值；键和 order 里的正好是同一批
    map: HashMap<String, T>,
}

impl<T> OMap<T> {
    pub fn new() -> OMap<T> {
        OMap { order: Vec::new(), map: HashMap::new() }
    }

    /// `len(d)`
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// `k in d`
    pub fn contains(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }

    /// `d.get(k)`
    pub fn get(&self, k: &str) -> Option<&T> {
        self.map.get(k)
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut T> {
        self.map.get_mut(k)
    }

    /// `d[k] = v`：已有的键留在原来的位置（返回原来的值），新的键排到最后
    pub fn insert(&mut self, k: impl Into<String>, v: T) -> Option<T> {
        let k = k.into();
        if let Some(slot) = self.map.get_mut(&k) {
            return Some(std::mem::replace(slot, v));
        }
        self.order.push(k.clone());
        self.map.insert(k, v);
        None
    }

    /// `d.setdefault(k, make())`：没有就加上（排到最后），返回里面的那一个、可以接着改
    pub fn setdefault(&mut self, k: &str, make: impl FnOnce() -> T) -> &mut T {
        match self.map.entry(k.to_string()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                self.order.push(k.to_string());
                e.insert(make())
            }
        }
    }

    /// `d.pop(k, None)`：别的键的先后不变
    pub fn remove(&mut self, k: &str) -> Option<T> {
        let v = self.map.remove(k)?;
        if let Some(i) = self.order.iter().position(|x| x == k) {
            self.order.remove(i);
        }
        Some(v)
    }

    /// `d.clear()`
    pub fn clear(&mut self) {
        self.order.clear();
        self.map.clear();
    }

    /// `list(d)`：各个键，照先后
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// `d.items()`，照先后
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.order.iter().filter_map(|k| self.map.get(k).map(|v| (k.as_str(), v)))
    }

    /// `d.values()`，照先后
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, v)| v)
    }

    /// 各个值，可以改；先后不定（要照先后的用 `for_each_mut`）
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.map.values_mut()
    }

    /// `for k, v in d.items(): 改 v`，照先后
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&str, &mut T)) {
        for k in &self.order {
            if let Some(v) = self.map.get_mut(k) {
                f(k, v);
            }
        }
    }

    /// `d = {k: v for k, v in d.items() if keep(k, v)}`：留下的先后不变
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &mut T) -> bool) {
        let map = &mut self.map;
        self.order.retain(|k| {
            let stay = map.get_mut(k).is_some_and(|v| keep(k, v));
            if !stay {
                map.remove(k);
            }
            stay
        });
    }

    /// 写成 JSON 的值：一个字典，各项照先后，值由 f 转
    pub fn to_v(&self, f: impl Fn(&T) -> V) -> V {
        let d = Dict::new();
        for (k, v) in self.iter() {
            d.set(k, f(v));
        }
        V::Dict(d)
    }

    /// 从 JSON 里的字典读：各项照它的先后，f 说不要（None）的那几项丢掉
    pub fn from_dict(d: &Dict, mut f: impl FnMut(&str, &V) -> Option<T>) -> OMap<T> {
        let mut out = OMap::new();
        for (k, v) in d.items() {
            if let Some(x) = f(&k, &v) {
                out.insert(&*k, x);
            }
        }
        out
    }
}

impl<T> Default for OMap<T> {
    fn default() -> Self {
        OMap::new()
    }
}

/// `dict(pairs)`：重复的键留在头一次的位置、值是最后一次的
impl<K: Into<String>, T> FromIterator<(K, T)> for OMap<T> {
    fn from_iter<I: IntoIterator<Item = (K, T)>>(pairs: I) -> Self {
        let mut out = OMap::new();
        for (k, v) in pairs {
            out.insert(k, v);
        }
        out
    }
}

/// Python 的 `==`：键和值都一样就相等，不看先后
impl<T: PartialEq> PartialEq for OMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for OMap<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::json;

    fn keys<T>(m: &OMap<T>) -> Vec<&str> {
        m.keys().collect()
    }

    /// 和 Python 的字典一样：新的排最后，赋值不挪位置，删了再加排到最后
    #[test]
    fn order_follows_python_dict() {
        let mut m = OMap::new();
        assert!(m.is_empty() && m.insert("b", 1).is_none() && m.insert("a", 2).is_none() && m.insert("c", 3).is_none());
        assert_eq!(m.insert("b", 10), Some(1), "已有的键：换掉值，位置不动");
        assert_eq!((keys(&m), m.len()), (vec!["b", "a", "c"], 3));
        assert_eq!(m.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(), ["b=10", "a=2", "c=3"]);
        assert_eq!(m.remove("b"), Some(10));
        assert_eq!((m.remove("b"), m.remove("zz")), (None, None));
        m.insert("b", 4);
        assert_eq!(keys(&m), ["a", "c", "b"]);
        assert_eq!(m.values().copied().collect::<Vec<_>>(), [2, 3, 4]);
        assert!(m.contains("a") && !m.contains("zz") && m.get("c") == Some(&3) && m.get("zz").is_none());
        *m.get_mut("c").unwrap() += 1;
        assert_eq!(m.get("c"), Some(&4));
        m.clear();
        assert!(m.is_empty() && keys(&m).is_empty() && m.get("a").is_none());
    }

    /// `setdefault`：有就是原来的那一个（make 不调），没有就加在最后；拿到的可以接着改
    #[test]
    fn setdefault_returns_the_stored_value() {
        let mut m: OMap<Vec<i32>> = OMap::new();
        m.setdefault("x", Vec::new).push(1);
        m.setdefault("y", || vec![7]).push(8);
        m.setdefault("x", || panic!("已经有了，不该再造")).push(2);
        assert_eq!((keys(&m), m.get("x"), m.get("y")), (vec!["x", "y"], Some(&vec![1, 2]), Some(&vec![7, 8])));
    }

    /// 挑着留、挨个改：留下的先后不变
    #[test]
    fn retain_and_for_each_mut_keep_order() {
        let mut m: OMap<i32> = [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)].into_iter().collect();
        let mut seen = Vec::new();
        m.retain(|k, v| {
            seen.push(k.to_string());
            *v *= 10;
            *v != 20 && k != "d"
        });
        assert_eq!(seen, ["a", "b", "c", "d", "e"], "照先后一个个问");
        assert_eq!(m.iter().collect::<Vec<_>>(), [("a", &10), ("c", &30), ("e", &50)]);
        assert!(!m.contains("b") && m.len() == 3);
        let mut order = Vec::new();
        m.for_each_mut(|k, v| {
            order.push(k.to_string());
            *v += 1;
        });
        assert_eq!(
            (order, m.values().copied().collect::<Vec<_>>()),
            (vec!["a".to_string(), "c".into(), "e".into()], vec![11, 31, 51])
        );
        m.values_mut().for_each(|v| *v = 0);
        assert!(m.values().all(|v| *v == 0));
        m.insert("b", 1);
        assert_eq!(keys(&m), ["a", "c", "e", "b"], "删掉的再加进来排在最后");
    }

    /// `dict(pairs)` 里重复的键、`==` 不看先后
    #[test]
    fn from_pairs_and_equality_like_python() {
        let m: OMap<i32> = [("a", 1), ("b", 2), ("a", 3)].into_iter().collect();
        assert_eq!(m.iter().collect::<Vec<_>>(), [("a", &3), ("b", &2)]);
        let n: OMap<i32> = [("b", 2), ("a", 3)].into_iter().collect();
        assert!(m == n && keys(&m) != keys(&n), "{{'a': 3, 'b': 2}} == {{'b': 2, 'a': 3}}");
        let other: OMap<i32> = [("a", 3)].into_iter().collect();
        assert!(m != other && OMap::<i32>::new() == OMap::default());
        assert_eq!(format!("{m:?}"), r#"{"a": 3, "b": 2}"#);
    }

    /// 读进来、写出去：先后照 JSON 里的，不要的那几项丢掉；写出来的和 Python 写的一个样
    #[test]
    fn json_round_trip_keeps_order() {
        let text = r#"{"zeta":{"n":1},"alpha":{"n":2},"坏的":"x","mid":{"n":3}}"#;
        let doc = json::loads(text).unwrap();
        let m = OMap::from_dict(&doc.as_dict().unwrap(), |_, v| v.as_dict().and_then(|d| d.get("n")).and_then(|n| n.as_int()));
        assert_eq!(m.iter().collect::<Vec<_>>(), [("zeta", &1), ("alpha", &2), ("mid", &3)]);
        let out = json::dumps(&m.to_v(|n| V::Dict(crate::dict! { "n" => *n })), json::Fmt::new().utf8().compact());
        assert_eq!(out, r#"{"zeta":{"n":1},"alpha":{"n":2},"mid":{"n":3}}"#);
        // 放得进 Mutex、跨得了线程
        fn sendable<X: Send + Sync>(_: &X) {}
        sendable(&m);
    }
}
