//! 远程会话的中转记着的东西，和设备发来的中转的请求（RELAY_OPS）。Python 的 `Remote`（Py:952-1230）。
//!
//! - edges（remote.json，移除设备、180 天没用过才删）：`{配对编号: {"a", "b", "at", "used"}}`，服务器只知道是哪两台设备配了对
//! - guests（也在 remote.json 里）：访客（手机上的网页端，不在名单里）的公钥
//! - 只在内存里的：slots（正在进行的配对）、tickets（进数据端口的票据）、sessions（开着的会话）
//!
//! Python 里是一把可重入的锁加一个条件变量；这里是 `Shared`：`Mutex<Remote>` + `Condvar`。pair_get 要等对方放密文进来：
//! 等的时候放开锁（`cv.wait_timeout`），同时等着的最多 `PAIR_WAITERS` 个（多了的不等，照现在的回）；配对的状态每变一次
//! `notify_all`。锁的先后：拿着 Remote 的锁可以再拿 Store 的（`name_of` 就是），反过来不行。
//!
//! 现在只有接口：里面的还没移植。

use super::limits::Limits;
use super::unfinished;
use crate::py::obj::V;
use crate::py::R;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Mutex};

pub struct Remote {
    /// remote.json
    path: PathBuf,
    /// 客户端连数据端口用的端口（--relay-port，默认是监听的那个）：票据里回给设备、/v1/health 里也有
    port: u16,
    /// 网页端的地址（开着网页端时）：给了才让设备开“给网页端的配对”、才有访客
    web: Option<String>,
    lim: Limits,
}

/// 几个线程共用的那一份：处理请求的线程、中转的线程都经它
pub struct Shared {
    pub m: Mutex<Remote>,
    /// 配对的状态变了（对方放了密文、报了配对编号、不做了）
    pub cv: Condvar,
    /// 正等在 pair_get 里的请求有几个
    pub waiters: AtomicUsize,
}

impl Shared {
    pub fn new(remote: Remote) -> Shared {
        Shared { m: Mutex::new(remote), cv: Condvar::new(), waiters: AtomicUsize::new(0) }
    }
}

impl Remote {
    /// 读 remote.json、筛掉样子不对的配对和访客（Py:958-973）；没有、读不了从空的开始（读不了的在日志里说一声）
    pub fn open(path: &Path, port: u16, web: Option<String>, lim: Limits) -> Remote {
        Remote { path: path.to_path_buf(), port, web, lim }
    }

    /// 客户端连数据端口用的端口
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 记着几个配对（启动时说的那一句里用，Py:2257）
    pub fn edges(&self) -> usize {
        0 // 还没移植
    }

    /// 名单变了（管理员登记、移除了设备，设备退出了）：不在名单里的设备的配对、票据、会话都删掉；访客不在名单里，领它进来的设备
    /// 没了，它的配对跟着没有，它也就删掉了（Py:1030-1042）。有删掉的配对要存，存不了是 Err
    pub fn keep_only(&mut self, fps: &[String], now: f64) -> R<()> {
        let _ = (fps, now, &self.path, &self.lim);
        unfinished("team-server 的中转（Remote::keep_only）")
    }

    /// 不在名单里的这把密钥能不能发这个请求：能的话是验它签名用的公钥那一行，不能是 None（Py:997-1017）。两种：已经配了对的访客；
    /// 正拿着配对码来的（那次配对是开给网页端的、还没有别的设备放过密文，公钥在请求的 pub 里、指纹要对得上）
    pub fn guest_key(&self, fp: &str, op: &str, j: &V, now: f64) -> Option<String> {
        let _ = (fp, op, j, now, &self.web);
        None // 还没移植
    }

    /// 网页端要开一条 WebSocket：它说的票据在不在（接入用的、没过期、还没有别的 WebSocket 拿它进来过）；在就记下已经有人拿它进来了
    /// （Py:1072-1080）
    pub fn ws_claim(&mut self, tid: &str, now: f64) -> bool {
        let _ = (tid, now);
        false // 还没移植
    }
}

/// 签名、时间、nonce 都验过了的中转的请求（Py:1090-1230）。返回 (状态码, 回的内容)。gkey：发请求的是访客（不在名单里），
/// 这是验它签名用的公钥那一行——配对成了要把它记进 guests，所以要的是公钥本身，不只是“是不是访客”。
/// name_of 问名单里一台设备的名字（会去拿 Store 的锁）。Err 只有 remote.json 存不了的时候
pub fn handle_op(
    sh: &Shared,
    op: &str,
    fp: &str,
    j: &V,
    now: f64,
    gkey: Option<&str>,
    name_of: &dyn Fn(&str) -> Option<String>,
) -> R<(u16, V)> {
    let _ = (&sh.m, &sh.cv, &sh.waiters, op, fp, j, now, gkey, name_of);
    unfinished("team-server 的中转（remote::handle_op）")
}
