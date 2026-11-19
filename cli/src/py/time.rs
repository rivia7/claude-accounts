//! `time`、`calendar`：现在几点、本地时间、格式化。本地时区、夏令时都交给 C 库（和 Python 用的是同一套函数），
//! 所以 `TZ` 环境变量、`/etc/localtime` 的效果和原来一样。

use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `time.time()`
pub fn time() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// `time.monotonic()`
pub fn monotonic() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts 是有效的可写内存
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// `time.sleep(secs)`
pub fn sleep(secs: f64) {
    if secs > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }
}

/// `time.struct_time`：字段的取值和 Python 一样（月份 1–12，周一是 0，一年里的第几天从 1 起）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tm {
    pub year: i32,
    pub mon: i32,
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
}

impl Tm {
    fn from_c(t: &libc::tm) -> Tm {
        Tm {
            year: t.tm_year + 1900,
            mon: t.tm_mon + 1,
            mday: t.tm_mday,
            hour: t.tm_hour,
            min: t.tm_min,
            sec: t.tm_sec,
            wday: (t.tm_wday + 6) % 7,
            yday: t.tm_yday + 1,
            isdst: t.tm_isdst,
        }
    }

    fn to_c(self) -> libc::tm {
        // SAFETY: tm 是纯数据，全 0 是合法的值（时区名的指针为空）
        let mut t: libc::tm = unsafe { std::mem::zeroed() };
        t.tm_year = self.year - 1900;
        t.tm_mon = self.mon - 1;
        t.tm_mday = self.mday;
        t.tm_hour = self.hour;
        t.tm_min = self.min;
        t.tm_sec = self.sec;
        t.tm_wday = (self.wday + 1) % 7;
        t.tm_yday = self.yday - 1;
        t.tm_isdst = self.isdst;
        t
    }
}

extern "C" {
    /// C 库的 tzset（libc 这个库没给 macOS 导出它）
    fn tzset();
}

fn tz_ready() {
    static ONCE: Once = Once::new();
    // SAFETY: tzset 只读环境变量和时区文件，进程里只调一次
    ONCE.call_once(|| unsafe { tzset() });
}

/// `time.localtime(t)`
#[allow(deprecated)] // musl 上 libc::time_t 标着“以后要改成 64 位”：只是提醒，写法不用变
pub fn localtime(t: f64) -> Tm {
    tz_ready();
    let secs = t.floor() as libc::time_t;
    // SAFETY: 两个指针都指向有效的内存
    unsafe {
        let mut out: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut out);
        Tm::from_c(&out)
    }
}

/// `time.gmtime(t)`
#[allow(deprecated)]
pub fn gmtime(t: f64) -> Tm {
    let secs = t.floor() as libc::time_t;
    // SAFETY: 同上
    unsafe {
        let mut out: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&secs, &mut out);
        Tm::from_c(&out)
    }
}

/// `time.mktime((年, 月, 日, 时, 分, 秒, 0, 0, isdst))`：本地时间 → 时间戳。日、时这些超出范围时由 C 库归一化
/// （`日 + 7` 这种写法靠它），isdst 传 -1 让它自己判断夏令时
pub fn mktime(year: i32, mon: i32, mday: i32, hour: i32, min: i32, sec: i32, isdst: i32) -> f64 {
    tz_ready();
    let mut t = Tm { year, mon, mday, hour, min, sec, wday: 0, yday: 1, isdst }.to_c();
    // SAFETY: t 是有效的可写内存
    unsafe { libc::mktime(&mut t) as f64 }
}

/// `calendar.timegm((年, 月, 日, 时, 分, 秒))`：UTC 时间 → 时间戳
pub fn timegm(year: i64, mon: i64, mday: i64, hour: i64, min: i64, sec: i64) -> i64 {
    // 公历日期 → 自 1970-01-01 起的天数（Howard Hinnant 的 days_from_civil）
    let y = if mon <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (mon + if mon > 2 { -3 } else { 9 }) + 2) / 5 + mday - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + hour * 3600 + min * 60 + sec
}

/// `time.strftime(fmt, tm)`：C 库的 strftime，星期、月份是英文（Python 也不改 LC_TIME）
pub fn strftime(fmt: &str, tm: &Tm) -> String {
    tz_ready();
    if fmt.is_empty() {
        return String::new();
    }
    let cfmt = std::ffi::CString::new(fmt).expect("格式里没有 0 字节");
    let mut t = tm.to_c();
    // %Z、%z 要时区的名字和偏移：让 C 库照这个时间重新算一遍
    // SAFETY: t 是有效的可写内存；mktime 只改 t
    unsafe {
        let mut probe = t;
        libc::mktime(&mut probe);
        t.tm_gmtoff = probe.tm_gmtoff;
        t.tm_zone = probe.tm_zone;
    }
    let mut buf = vec![0u8; 256];
    loop {
        // SAFETY: buf 的长度如实给出，cfmt 以 0 结尾
        let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &t) };
        if n > 0 || buf.len() > 4096 {
            buf.truncate(n);
            return String::from_utf8_lossy(&buf).into_owned();
        }
        buf.resize(buf.len() * 4, 0);
    }
}

/// `time.strftime(fmt, time.localtime(t))`
pub fn strftime_local(fmt: &str, t: f64) -> String {
    strftime(fmt, &localtime(t))
}

/// `time.strftime(fmt, time.gmtime(t))`
pub fn strftime_utc(fmt: &str, t: f64) -> String {
    let tm = gmtime(t);
    // gmtime 的结果直接格式化（不能经 mktime，那是按本地时间算的）
    let cfmt = std::ffi::CString::new(fmt).expect("格式里没有 0 字节");
    let c = tm.to_c();
    let mut buf = vec![0u8; 256];
    // SAFETY: 同 strftime
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &c) };
    buf.truncate(n);
    String::from_utf8_lossy(&buf).into_owned()
}

/// `time.mktime(time.strptime(s, fmt))`：照格式把本地时间的文字读成时间戳，读不出是 ValueError（这里是 None）
pub fn strptime_local(s: &str, fmt: &str) -> Option<f64> {
    tz_ready();
    let (cs, cf) = (std::ffi::CString::new(s).ok()?, std::ffi::CString::new(fmt).ok()?);
    // SAFETY: 两个字符串都以 0 结尾，t 是有效的可写内存
    unsafe {
        let mut t: libc::tm = std::mem::zeroed();
        let end = libc::strptime(cs.as_ptr(), cf.as_ptr(), &mut t);
        if end.is_null() || *end != 0 {
            return None;
        }
        t.tm_isdst = -1;
        Some(libc::mktime(&mut t) as f64)
    }
}

/// `(date.fromtimestamp(a) - date.fromtimestamp(b)).days`：按本地日历差几天
pub fn local_days_between(a: f64, b: f64) -> i64 {
    let day = |t: f64| {
        let tm = localtime(t);
        timegm(tm.year as i64, tm.mon as i64, tm.mday as i64, 0, 0, 0) / 86400
    };
    day(a) - day(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timegm_like_python() {
        assert_eq!(timegm(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(timegm(2026, 11, 19, 3, 20, 0), 1795058400);
        assert_eq!(timegm(2000, 2, 29, 23, 59, 59), 951868799);
        assert_eq!(timegm(1969, 12, 31, 23, 59, 59), -1);
    }

    #[test]
    fn gmtime_fields() {
        let t = gmtime(1795058400.7);
        assert_eq!((t.year, t.mon, t.mday, t.hour, t.min, t.sec), (2026, 11, 19, 3, 20, 0));
        assert_eq!(t.wday, 3); // 周四（周一是 0）
        assert_eq!(t.yday, 323);
        assert_eq!(strftime_utc("%Y-%m-%dT%H:%M:%SZ %a %b", 1795058400.0), "2026-11-19T03:20:00Z Thu Nov");
    }

    #[test]
    fn local_round_trip() {
        let now = time().floor();
        let tm = localtime(now);
        assert_eq!(mktime(tm.year, tm.mon, tm.mday, tm.hour, tm.min, tm.sec, -1), now);
        // 日期溢出由 C 库归一化：下个月 0 日就是这个月最后一天
        let end = mktime(2026, 3, 0, 0, 0, 0, -1);
        assert_eq!(localtime(end).mday, 28);
        assert_eq!(strftime_local("%Y%m%d-%H%M%S", now).len(), 15);
        let text = strftime_local("%a %b %d %H:%M:%S %Y", now);
        assert_eq!(strptime_local(&text, "%a %b %d %H:%M:%S %Y"), Some(now));
        assert_eq!(strptime_local("nonsense", "%a %b %d %H:%M:%S %Y"), None);
        assert_eq!(local_days_between(now, now - 86400.0 * 3.0), 3);
    }

    #[test]
    fn monotonic_moves_forward() {
        let a = monotonic();
        sleep(0.01);
        assert!(monotonic() - a >= 0.009);
    }
}
