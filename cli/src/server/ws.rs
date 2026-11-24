//! 网页端的 WebSocket（RFC 6455 里用得着的那一点）。Python 的 `ws_frame`、`ws_frames`、`ws_pump`（Py:1720-1796）。
//!
//! 浏览器开不了普通的 TCP 连接，所以它把本该发到数据端口的字节装在二进制帧里发过来：这里拆出来，当成一条字节流交给中转
//! （`UnixStream` 的一头经 `relay::Handle::adopt` 交过去），中转回的装成帧发回去。里面照旧是开场白和两头之间的密文，这里一个字节都不看。
//!
//! 现在只有发帧的 `frame`：拆帧（`frames`：用了扩展位的、没加掩码的、超过 `WS_FRAME` 的、控制帧超过 125 个字节的都不收）、
//! 搬数据的 `pump` 还没移植。

/// 服务器发的一帧（不加掩码）：op 是操作码（2 二进制、8 关闭、10 pong）（Py:1720-1724）
pub fn frame(op: u8, data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = Vec::with_capacity(n.saturating_add(10));
    out.push(0x80 | op);
    match (u8::try_from(n), u16::try_from(n)) {
        (Ok(small), _) if small < 126 => out.push(small),
        (_, Ok(medium)) => {
            out.push(126);
            out.extend_from_slice(&medium.to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// 长度的三种写法，和 Python 的 ws_frame 给的一样
    #[test]
    fn frame_lengths() {
        assert_eq!(frame(2, b""), [0x82, 0x00]);
        assert_eq!(frame(8, b""), [0x88, 0x00]);
        assert_eq!(frame(10, b"hi"), [0x8a, 0x02, b'h', b'i']);
        let f = frame(2, &[7u8; 125]);
        assert_eq!((&f[..2], f.len()), (&[0x82, 125][..], 127));
        let f = frame(2, &[7u8; 126]);
        assert_eq!((&f[..4], f.len()), (&[0x82, 126, 0, 126][..], 130));
        let f = frame(2, &[7u8; 65535]);
        assert_eq!((&f[..4], f.len()), (&[0x82, 126, 0xff, 0xff][..], 65539));
        let f = frame(2, &[7u8; 65536]);
        assert_eq!((&f[..10], f.len()), (&[0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0][..], 65546));
        assert!(f[10..].iter().all(|b| *b == 7));
    }
}
