//! 各种上限和限流。
//!
//! - `Limits`：存多少、同时几个这一类的上限。Python 的测试用 `mock.patch` 把模块里的常量换成小的来测“满了怎么办”；这里换成
//!   `Store`、`Remote` 各带一份 `Limits`，测试写 `Limits { max_keys: 3, ..Limits::DEFAULT }`。
//! - `Buckets`（令牌桶）、`Nonces`（用过的 nonce）、`Slots`（同时处理几个连接）：Python 里各自带着锁、自己看时间；这里不带锁
//!   （由用它的地方放进 `Mutex`），时间由调用方给（`now`），测试不用等。
//!
//! 这里的都是普通的值，可以跨线程。

use super::{MAX_CONN, MAX_CONN_IP, TS_WINDOW};
use std::collections::HashMap;

/// 测试会换小的那几个上限；平常就是 `Limits::DEFAULT`（Python 开头的常量）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// API key 的密文最多存几个账号（MAX_KEYS，Py:80）
    pub max_keys: usize,
    /// 折合金额：一台设备最多存几个账号的（COST_SLOTS，Py:83）
    pub cost_slots: usize,
    /// 折合金额：一台设备存的密文加起来最多这么大（COST_DEVICE_BYTES，Py:84）
    pub cost_device_bytes: usize,
    /// 折合金额：所有设备加起来最多这么大（COST_TOTAL_BYTES，Py:85）
    pub cost_total_bytes: usize,
    /// 折合金额：一次回的密文最多这么多，多了的只带认证码不带密文（COST_RESP，Py:86）
    pub cost_resp: usize,
    /// 一个 Team 最多几台设备（MAX_TEAM_MEMBERS，Py:100）
    pub max_team_members: usize,
    /// 还没用掉的邀请最多几个（MAX_INVITES，Py:101）
    pub max_invites: usize,
    /// 一台设备同时开着几个配对（PAIR_SLOTS，Py:906）
    pub pair_slots: usize,
    /// 一台设备最多和几台配对（EDGES_DEV，Py:910）
    pub edges_dev: usize,
    /// 配对一共最多几个（EDGES_MAX，Py:911）
    pub edges_max: usize,
    /// 一台设备手里没用掉的票据（TICKETS_DEV，Py:914 是 16；3.7.10 起 128：100 个会话一起重连）
    pub tickets_dev: usize,
    /// 一台设备同时开着几个可以接入的会话（SESSIONS_DEV，Py:915 是 8；3.7.10 起 101：100 个远程会话加上启动器）
    pub sessions_dev: usize,
    /// 一个会话同时几处接入（VIEWERS，Py:916）
    pub viewers: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_keys: 4096,
        cost_slots: 128,
        cost_device_bytes: 4 << 20,
        cost_total_bytes: 32 << 20,
        cost_resp: 512 * 1024,
        max_team_members: 256,
        max_invites: 64,
        pair_slots: 4,
        edges_dev: 16,
        edges_max: 4096,
        tickets_dev: 128,
        sessions_dev: 101,
        viewers: 4,
    };
}

/// 令牌桶限流：每个键每秒 rate 个，最多攒 burst 个（Py:353-369）
pub struct Buckets {
    rate: f64,
    burst: f64,
    /// 键 → (还剩几个, 上次是什么时候)
    b: HashMap<String, (f64, f64)>,
    /// 上次把整张表过一遍是什么时候
    swept: f64,
}

impl Buckets {
    /// 键多于这么多个时，把很久没动的清掉（Py:361）
    const SWEEP_OVER: usize = 4096;
    /// 这么多秒没动过的算很久（Py:362）
    const SWEEP_IDLE: f64 = 600.0;
    /// 过了一遍以后至少隔这么多秒再过。Python 是键多了以后每次都过一遍：600 秒里来过几万个不同的地址时（每个只来一次，
    /// 没有一个到期），每个请求都要拿着锁把整张表数一遍，别的请求都等着。隔一秒过一遍，清掉的东西一样
    const SWEEP_GAP: f64 = 1.0;

    pub fn new(rate: f64, burst: f64) -> Buckets {
        Buckets { rate, burst, b: HashMap::new(), swept: f64::NEG_INFINITY }
    }

    /// 这个键还有没有配额：有就扣一个。now 是单调时钟（`py::time::monotonic()`）
    pub fn take(&mut self, key: &str, now: f64) -> bool {
        // now < swept：时钟倒着走了（不该有），照常过一遍
        if self.b.len() > Self::SWEEP_OVER && (now - self.swept >= Self::SWEEP_GAP || now < self.swept) {
            self.b.retain(|_, v| now - v.1 < Self::SWEEP_IDLE);
            self.swept = now;
        }
        let (tokens, last) = self.b.get(key).copied().unwrap_or((self.burst, now));
        let tokens = self.burst.min(tokens + (now - last) * self.rate);
        if tokens < 1.0 {
            self.b.insert(key.to_string(), (tokens, now));
            return false;
        }
        self.b.insert(key.to_string(), (tokens - 1.0, now));
        true
    }

    /// 记着几个键（测试看清没清掉旧的）
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.b.len()
    }
}

/// 用过的 nonce（只在内存里；重启后靠 ts 的 5 分钟窗口挡重放）（Py:372-384）
pub struct Nonces {
    /// nonce → 记到什么时候
    seen: HashMap<String, f64>,
}

impl Nonces {
    /// 多于这么多个时，把过了期的清掉（Py:379）
    const SWEEP_OVER: usize = 20000;

    pub fn new() -> Nonces {
        Nonces { seen: HashMap::new() }
    }

    /// 记下这个 nonce；已经用过（还没过期）是 false。now 是服务器的时间（`py::time::time()`）
    pub fn add(&mut self, nonce: &str, now: f64) -> bool {
        if self.seen.len() > Self::SWEEP_OVER {
            self.seen.retain(|_, t| *t > now);
        }
        if self.seen.get(nonce).copied().unwrap_or(0.0) > now {
            return false;
        }
        self.seen.insert(nonce.to_string(), now + 2.0 * TS_WINDOW as f64 + 60.0);
        true
    }

    /// 记着几个（测试看清没清掉旧的）
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.seen.len()
    }
}

/// 按 IP 限流的（每秒几个、最多攒几个）：Py 是 120 个，3.7.10 起 240 个（同一个地址后面一台机器的 100 个会话一起重连）
pub const IP_RATE: (f64, f64) = (2.0, 240.0);
/// 按设备限流的：Py 是 30 个，3.7.10 起 150 个（一台设备的 100 个会话一起重连，各要一张票据）
pub const FP_RATE: (f64, f64) = (0.5, 150.0);
/// Team 的管理员的
pub const ROOT_RATE: (f64, f64) = (2.0, 120.0);

/// 两个监听共用的那一份限流：同一个请求不能在两个端口上各用一次（Py:2043-2047）
pub struct Guards {
    /// 按 IP：IP_RATE
    pub ip: Buckets,
    /// 按设备的指纹（验过签名才扣）：FP_RATE
    pub fp: Buckets,
    /// Team 的管理员（根密钥签的）：一条命令要问两三次，设备的那个限额不够用
    pub root: Buckets,
    pub nonces: Nonces,
}

impl Guards {
    pub fn new() -> Guards {
        let new = |(rate, burst): (f64, f64)| Buckets::new(rate, burst);
        Guards { ip: new(IP_RATE), fp: new(FP_RATE), root: new(ROOT_RATE), nonces: Nonces::new() }
    }
}

/// 同时处理几个连接：总数和同一个 IP 的数各有上限（Py:2048-2076；网页端的 WebSocket 也是这个样子，Py:1652-1667）
pub struct Slots {
    max: usize,
    max_ip: usize,
    used: usize,
    per_ip: HashMap<String, usize>,
}

impl Slots {
    pub fn new(max: usize, max_ip: usize) -> Slots {
        Slots { max, max_ip, used: 0, per_ip: HashMap::new() }
    }

    /// 一个监听处理请求的名额：一共 64 个，同一个 IP 8 个
    pub fn for_listener() -> Slots {
        Slots::new(MAX_CONN, MAX_CONN_IP)
    }

    /// 占一个名额；满了是 false（连接太多：直接断开，不排队）。any_ip：不看同一个 IP 的上限（--behind-proxy 时连过来的都是代理）
    pub fn take(&mut self, ip: &str, any_ip: bool) -> bool {
        let n = self.per_ip.get(ip).copied().unwrap_or(0);
        if (n >= self.max_ip && !any_ip) || self.used >= self.max {
            return false;
        }
        self.used += 1;
        self.per_ip.insert(ip.to_string(), n + 1);
        true
    }

    /// 还一个名额
    pub fn free(&mut self, ip: &str) {
        match self.per_ip.get(ip).copied().unwrap_or(1).saturating_sub(1) {
            0 => {
                self.per_ip.remove(ip);
            }
            n => {
                self.per_ip.insert(ip.to_string(), n);
            }
        }
        self.used = self.used.saturating_sub(1);
    }

    /// 占着几个（测试看名额还了没有）
    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// 默认的上限就是 Python 开头的那些常量（票据、会话 3.7.10 起放大了）
    #[test]
    fn default_limits_are_the_python_constants() {
        let d = Limits::DEFAULT;
        assert_eq!(
            (d.max_keys, d.cost_slots, d.cost_device_bytes, d.cost_total_bytes, d.cost_resp),
            (4096, 128, 4194304, 33554432, 524288)
        );
        assert_eq!((d.max_team_members, d.max_invites), (256, 64));
        assert_eq!((d.pair_slots, d.edges_dev, d.edges_max, d.tickets_dev, d.sessions_dev, d.viewers), (4, 16, 4096, 128, 101, 4));
        let small = Limits { max_keys: 3, ..Limits::DEFAULT };
        assert_eq!((small.max_keys, small.cost_slots), (3, 128));
    }

    /// 令牌桶：先有 burst 个，用完了不给，过一会儿按 rate 涨回来，最多涨到 burst（和 Python 的那一份逐步对过）
    #[test]
    fn buckets_refill_at_rate_up_to_burst() {
        let mut b = Buckets::new(0.5, 3.0);
        assert!(b.take("a", 100.0) && b.take("a", 100.0) && b.take("a", 100.0));
        assert!(!b.take("a", 100.0), "三个用完了");
        assert!(!b.take("a", 101.0), "一秒只涨半个");
        assert!(b.take("a", 102.0), "从上次问的时候起又过了一秒，攒够一个");
        assert!(!b.take("a", 102.0));
        assert!(b.take("b", 102.0), "各个键各算各的");
        // 很久不用：涨到 burst 为止，不会更多
        assert!(b.take("a", 10_000.0) && b.take("a", 10_000.0) && b.take("a", 10_000.0) && !b.take("a", 10_000.0));
        // 时间倒着走（不该有）：配额变少、不出错
        assert!(!b.take("a", 9_000.0));
    }

    /// 没要到的那几次也记着攒了多少：一直来问的，各次之间攒的那一点点加得起来（Py:366）
    #[test]
    fn refused_takes_keep_what_has_accrued() {
        let mut b = Buckets::new(1.0, 1.0);
        assert!(b.take("k", 0.0));
        assert!(!b.take("k", 0.5) && !b.take("k", 0.75), "0.5 + 0.25 还不到 1");
        assert!(b.take("k", 1.0), "再加 0.25 正好够");
        assert!(!b.take("k", 1.5) && b.take("k", 2.0));
    }

    /// 键多于 4096 个时，600 秒没动过的清掉（正好 4096 个时还不清）
    #[test]
    fn buckets_forget_idle_keys_once_there_are_many() {
        let mut b = Buckets::new(2.0, 120.0);
        for i in 0..4096 {
            assert!(b.take(&format!("ip-{i}"), 0.0));
        }
        assert!(b.take("fresh", 500.0));
        assert_eq!(b.len(), 4097);
        assert!(b.take("late", 600.0), "这一次先清：0 秒的那 4096 个正好满 600 秒");
        assert_eq!(b.len(), 2);
        assert!(b.take("fresh", 600.0));
    }

    /// 键很多、又都没到期时不是每次都把整张表过一遍：过了一遍以后隔一秒才再过（到期的晚不到一秒清掉，给不给配额不受影响）
    #[test]
    fn buckets_sweep_at_most_once_a_second() {
        let mut b = Buckets::new(2.0, 120.0);
        assert!(b.take("old", 0.0));
        for i in 0..4100 {
            assert!(b.take(&format!("ip-{i}"), 0.5));
        }
        assert_eq!(b.len(), 4101);
        assert!(b.take("a", 600.0), "过一遍：只有 0 秒的那一个满 600 秒了");
        assert_eq!(b.len(), 4101);
        assert!(b.take("b", 600.6), "0.5 秒的那 4100 个到期了，可离上一遍还不到一秒：先不过");
        assert_eq!(b.len(), 4102);
        assert!(b.take("c", 601.0), "满一秒了：过一遍");
        assert_eq!(b.len(), 3);
        // 配额照旧各算各的
        assert!(b.take("a", 601.0) && b.take("ip-0", 601.0));
    }

    /// nonce：用过的 660 秒里不认第二次，过了就又能用（那时请求里的时间早就不在 5 分钟的窗口里了）
    #[test]
    fn nonces_refuse_a_replay_until_it_expires() {
        let mut n = Nonces::new();
        assert!(n.add("aa", 1000.0));
        assert!(!n.add("aa", 1000.0) && !n.add("aa", 1659.9));
        assert!(n.add("aa", 1660.0), "记到 now + 2 × 300 + 60，到点就不算了");
        assert!(n.add("bb", 1000.0));
        // 多于 20000 个时清掉过期的
        for i in 0..20000 {
            assert!(n.add(&format!("n{i}"), 0.0));
        }
        assert_eq!(n.len(), 20002);
        assert!(n.add("cc", 700.0), "这一次先清：0 秒记下的那两万个都过期了");
        assert_eq!(n.len(), 3);
    }

    /// 两个监听共用的那一份：三个桶的速度和 Python 的一样，攒得下的按 IP、按设备的 3.7.10 起多了（Python 是 120、30）
    #[test]
    fn guards_use_the_python_rates() {
        let mut g = Guards::new();
        let count = |b: &mut Buckets| (0..400).filter(|_| b.take("k", 0.0)).count();
        assert_eq!((count(&mut g.ip), count(&mut g.fp), count(&mut g.root)), (240, 150, 120));
        assert!(g.fp.take("k", 2.0) && !g.fp.take("k", 2.0), "每秒涨 0.5 个");
        assert!(g.nonces.add("x", 0.0) && !g.nonces.add("x", 1.0));
    }

    /// 名额：同一个 IP 8 个、一共 64 个；--behind-proxy 时只看总数
    #[test]
    fn slots_cap_per_ip_and_in_total() {
        let mut s = Slots::for_listener();
        assert!((0..8).all(|_| s.take("192.0.2.1", false)));
        assert!(!s.take("192.0.2.1", false), "同一个 IP 第 9 个");
        assert!(s.take("192.0.2.2", false));
        s.free("192.0.2.1");
        assert!(s.take("192.0.2.1", false) && !s.take("192.0.2.1", false));
        assert_eq!(s.used(), 9);
        // 代理后面：同一个 IP 可以过 8 个，到总数为止
        let mut p = Slots::for_listener();
        assert!((0..64).all(|_| p.take("198.51.100.7", true)));
        assert!(!p.take("198.51.100.7", true) && !p.take("192.0.2.9", true), "总数满了");
        p.free("198.51.100.7");
        assert!(p.take("192.0.2.9", false));
        // 都还掉以后不留记录；多还一次不出错
        let mut q = Slots::new(2, 1);
        assert!(q.take("a", false) && !q.take("a", false) && q.take("b", false) && !q.take("c", false));
        q.free("a");
        q.free("b");
        q.free("b");
        assert_eq!((q.used(), q.per_ip.len()), (0, 0));
        assert!(q.take("c", false));
    }
}
