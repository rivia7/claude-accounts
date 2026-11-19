//! 随机数：`os.urandom`、`secrets.token_hex`、`uuid.uuid4`。都取系统的随机源。

/// `os.urandom(n)`
pub fn urandom(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    for chunk in buf.chunks_mut(256) {
        // SAFETY: chunk 是有效的可写内存，长度不超过 256（getentropy 的上限；getrandom 这么短也是一次给全）
        #[cfg(target_os = "linux")]
        let ok = unsafe { libc::getrandom(chunk.as_mut_ptr().cast(), chunk.len(), 0) } == chunk.len() as isize;
        #[cfg(not(target_os = "linux"))]
        let ok = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) } == 0;
        if !ok {
            // 系统调用用不了（很旧的 Linux 内核没有 getrandom、被 seccomp 挡住、被信号打断……）：和 CPython 一样退回去读 /dev/urandom
            let err = std::io::Error::last_os_error();
            let read = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, chunk));
            if let Err(e) = read {
                panic!("取不到系统的随机数：{err}；/dev/urandom 也读不了：{e}");
            }
        }
    }
    buf
}

/// `secrets.token_hex(n)` / `os.urandom(n).hex()`
pub fn token_hex(n: usize) -> String {
    super::hash::to_hex(&urandom(n))
}

/// `str(uuid.uuid4())`
pub fn uuid4() -> String {
    let mut b = urandom(16);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = super::hash::to_hex(&b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(urandom(300).len(), 300);
        assert_eq!(token_hex(32).len(), 64);
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(uuid4(), uuid4());
    }
}
