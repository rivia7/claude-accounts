//! 从连接上读：一行一行取、一段一段取，每次取多少都有上限。http-01 的应答读请求（respond.rs）、连证书机构读回应（fetch.rs）
//! 都经它——对面是公网上随便谁，说多长就给多长的缓冲不行。
//!
//! 两个取法和 Python 的缓冲读一样（那两处的规矩都是照着 `rfile.readline(65537)`、`fp.read(n)` 写的）：
//! - `line(limit)`：到换行为止（带着换行），最多 limit 个字节；没等到换行就读到头了，是剩下的那些（可能是空的）
//! - `take(n)`：n 个字节；没读够就读到头了，是剩下的那些
//!
//! 缓冲只随着真的读到的字节长（一次最多读 8192 个），不照对面说的长度先分配。

use std::io::{self, Read};

/// 一次从连接上最多读这么多
const CHUNK: usize = 8192;

pub struct Lines<R> {
    src: R,
    buf: Vec<u8>,
    /// buf 里这之前的已经取走了
    at: usize,
    /// 读到头了
    eof: bool,
}

impl<R: Read> Lines<R> {
    pub fn new(src: R) -> Lines<R> {
        Lines { src, buf: Vec::new(), at: 0, eof: false }
    }

    /// 缓冲里还没取走的
    fn pending(&self) -> &[u8] {
        self.buf.get(self.at..).unwrap_or(&[])
    }

    /// 再读一块进来；读到头了记下
    fn fill(&mut self) -> io::Result<()> {
        if self.at >= CHUNK {
            // 取走的那一截别一直占着
            self.buf.drain(..self.at.min(self.buf.len()));
            self.at = 0;
        }
        let mut chunk = [0u8; CHUNK];
        loop {
            match self.src.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            return Ok(());
        }
    }

    /// 取走缓冲里前 n 个字节
    fn consume(&mut self, n: usize) -> Vec<u8> {
        let have = self.pending();
        let out = have.get(..n.min(have.len())).unwrap_or(&[]).to_vec();
        self.at = self.at.saturating_add(out.len());
        if self.at >= self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        out
    }

    /// 一行：到换行为止（带着换行），最多 limit 个字节。读到头了是剩下的那些；什么都没有了是空的
    pub fn line(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        let mut checked = 0; // 这之前的已经找过、没有换行：对面一个字节一个字节地送时不用每次从头找
        loop {
            let have = self.pending();
            let window = have.get(..limit.min(have.len())).unwrap_or(&[]);
            if let Some(i) = window.get(checked..).unwrap_or(&[]).iter().position(|b| *b == b'\n') {
                return Ok(self.consume(checked.saturating_add(i).saturating_add(1)));
            }
            checked = window.len();
            if have.len() >= limit || self.eof {
                return Ok(self.consume(checked));
            }
            self.fill()?;
        }
    }

    /// n 个字节；没读够就读到头了，是剩下的那些
    pub fn take(&mut self, n: usize) -> io::Result<Vec<u8>> {
        while self.pending().len() < n && !self.eof {
            self.fill()?;
        }
        Ok(self.consume(n))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
pub(super) mod tests {
    use super::*;
    use crate::py::obj::List;

    /// 文字按 latin-1 写成字节（一个字一个字节）：对照数据里的请求、回应是这么记的
    pub fn latin1(text: &str) -> Vec<u8> {
        text.chars().map(|c| u8::try_from(u32::from(c)).unwrap()).collect()
    }

    /// 字节按 latin-1 当成文字
    pub fn as_text(bytes: &[u8]) -> String {
        bytes.iter().map(|b| char::from(*b)).collect()
    }

    /// 对照数据里“几段接起来”的字节：每段是一串字，或者 [一串字, 重复几次]
    pub fn from_parts(parts: &List) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts.items() {
            match part.as_str() {
                Some(s) => out.extend(latin1(s)),
                None => {
                    let part = part.as_list().unwrap();
                    out.extend(latin1(part.at(0).as_str().unwrap()).repeat(part.at(1).as_int().unwrap() as usize));
                }
            }
        }
        out
    }

    /// 一次只给几个字节的来源（对面慢慢送的样子）；errors 里的错先一个个给出去（从后往前）
    pub struct Drip<'a> {
        pub data: &'a [u8],
        pub step: usize,
        pub errors: Vec<io::ErrorKind>,
    }

    impl Read for Drip<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(kind) = self.errors.pop() {
                return Err(kind.into());
            }
            let n = self.step.min(self.data.len()).min(buf.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn lines_like_readline() {
        let mut r = Lines::new(&b"GET / HTTP/1.1\r\nHost: x\r\n\r\nrest"[..]);
        assert_eq!(r.line(100).unwrap(), b"GET / HTTP/1.1\r\n");
        assert_eq!(r.line(100).unwrap(), b"Host: x\r\n");
        assert_eq!(r.line(100).unwrap(), b"\r\n");
        assert_eq!(r.line(100).unwrap(), b"rest", "没等到换行就读到头了：剩下的那些");
        assert_eq!(r.line(100).unwrap(), b"");
        assert_eq!(r.line(100).unwrap(), b"", "读到头以后一直是空的");
        // 有上限：一行太长时只给这么多，后面的下次再给
        let mut r = Lines::new(&b"abcdefgh\nxy"[..]);
        assert_eq!(r.line(3).unwrap(), b"abc");
        assert_eq!(r.line(3).unwrap(), b"def");
        assert_eq!(r.line(3).unwrap(), b"gh\n", "换行正好在上限里");
        assert_eq!(r.line(0).unwrap(), b"");
        assert_eq!(r.line(2).unwrap(), b"xy");
        // 上限比一次读的还多：65537 的上限、七万个字节的一行（http.server 判“太长”的那种）
        let long = [vec![b'a'; 70000], b"\nnext\n".to_vec()].concat();
        let mut r = Lines::new(&long[..]);
        let first = r.line(65537).unwrap();
        assert_eq!((first.len(), first.contains(&b'\n')), (65537, false));
        assert_eq!(r.line(65537).unwrap().len(), 70000 - 65537 + 1);
        assert_eq!(r.line(65537).unwrap(), b"next\n");
    }

    #[test]
    fn takes_like_read() {
        let mut r = Lines::new(&b"0123456789"[..]);
        assert_eq!(r.take(4).unwrap(), b"0123");
        assert_eq!(r.take(0).unwrap(), b"");
        assert_eq!(r.take(100).unwrap(), b"456789", "没读够就读到头了");
        assert_eq!(r.take(5).unwrap(), b"");
        // 和一行一行取的混着用：缓冲里多读进来的不丢
        let mut r = Lines::new(&b"5\r\nhello\r\n0\r\n\r\n"[..]);
        assert_eq!(r.line(100).unwrap(), b"5\r\n");
        assert_eq!(r.take(5).unwrap(), b"hello");
        assert_eq!(r.take(2).unwrap(), b"\r\n");
        assert_eq!(r.line(100).unwrap(), b"0\r\n");
        // 大的：一块一块读进来，不多不少
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut r = Lines::new(&big[..]);
        assert_eq!(r.take(100_000).unwrap(), &big[..100_000]);
        assert_eq!(r.take(1 << 20).unwrap(), &big[100_000..]);
    }

    /// 对面一个字节一个字节地送：结果一样；被信号打断的那一下接着读；别的错原样报出来
    #[test]
    fn slow_sources_and_errors() {
        let data = b"line one\nline two\ntail";
        let mut r = Lines::new(Drip { data, step: 1, errors: vec![io::ErrorKind::Interrupted] });
        assert_eq!(r.line(100).unwrap(), b"line one\n");
        assert_eq!(r.take(4).unwrap(), b"line");
        assert_eq!(r.line(100).unwrap(), b" two\n");
        assert_eq!(r.line(100).unwrap(), b"tail");
        let mut r = Lines::new(Drip { data, step: 3, errors: vec![io::ErrorKind::TimedOut] });
        assert_eq!(r.line(100).unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert_eq!(r.line(100).unwrap(), b"line one\n", "出过错以后还能接着读（调用方一般不会）");
        // 取走的那一截会腾出来：读很多行也不会越攒越多
        let many = b"0123456789abcdef\n".repeat(5000);
        let mut r = Lines::new(Drip { data: &many, step: 4000, errors: vec![] });
        for _ in 0..5000 {
            assert_eq!(r.line(100).unwrap(), b"0123456789abcdef\n");
            assert!(r.buf.len() <= 3 * CHUNK, "{}", r.buf.len());
        }
        assert_eq!(r.line(100).unwrap(), b"");
    }
}
