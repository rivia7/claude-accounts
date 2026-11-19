//! 随机数：`os.urandom`、`secrets.token_hex`、`uuid.uuid4`。都取系统的随机源。

/// `os.urandom(n)`
pub fn urandom(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    for chunk in buf.chunks_mut(256) {
        // SAFETY: chunk 是有效的可写内存，长度不超过 getentropy 的上限 256
        let rc = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
        if rc != 0 {
            panic!("取不到系统的随机数：{}", std::io::Error::last_os_error());
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
