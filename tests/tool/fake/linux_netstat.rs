//! netstat 文件… [-- netstat 的参数…]：tests/linux.sh 的假路由器上的假 netstat。原来是 linux.sh 写进假路由器 bin/ 里的一小段
//! Python，现在那里是两行 sh（exec cca-test netstat /proc/net/tcp /proc/net/tcp6 -- "$@"）。
//!
//! 路由器端的脚本（cca-router.sh）拿 netstat 看端口池的端口有没有人监听。假路由器和测试在同一台机器上，所以真的去读内核的表
//! （/proc/net/tcp、/proc/net/tcp6：第一行是表头，之后一行一个 socket，第 2 栏是 “本机地址:端口”（十六进制），第 4 栏是状态，
//! 0A 是在监听）：每个在监听的打一行 “tcp 0 0 0.0.0.0:端口 0.0.0.0:* LISTEN”（地址一律写 0.0.0.0：脚本只看端口）。
//!
//! “--” 前面是要读的文件（单元测试、对拍时换成别的文件；linux.sh 写的就是上面那两个），后面是调用的人给 netstat 的参数——
//! 和原来一样不看。读不了的文件跳过（没开 IPv6 的机器上没有 tcp6）。原来 Python 会出异常的地方（第 2 栏里没有冒号、冒号后面不是
//! 十六进制数）在这里也是 stderr 上一行、退出码 1，前面已经打出来的行照样出去。Python 的 int(…, 16) 还认 0x 开头、正负号、
//! 数字之间的下划线，这里只认光是十六进制数字的（内核写的就是）。

use super::*;

/// 一个文件里在监听的端口：一个一行，接在 lines 后面（中途出了异常，前面的那些已经在里面了）
fn listing(text: &str, lines: &mut Vec<String>) -> R<()> {
    for row in splitlines(text).into_iter().skip(1) {
        let p: Vec<&str> = row.split(is_py_space).filter(|w| !w.is_empty()).collect();
        if p.len() > 3 && p[3] == "0A" {
            // int(p[1].rsplit(':', 1)[1], 16)
            let Some((_, hex)) = p[1].rsplit_once(':') else { return fail("IndexError: list index out of range") };
            let plain = !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
            let Some(port) = plain.then(|| u128::from_str_radix(hex, 16).ok()).flatten() else {
                return fail(format!("ValueError: invalid literal for int() with base 16: {}", repr_str(hex)));
            };
            lines.push(format!("tcp 0 0 0.0.0.0:{port} 0.0.0.0:* LISTEN"));
        }
    }
    Ok(())
}

pub fn run(a: &[String]) -> R<i32> {
    for f in a.iter().take_while(|x| *x != "--") {
        // except OSError: continue
        let Ok(raw) = std::fs::read(f) else { continue };
        let text = String::from_utf8(raw).map_err(|e| {
            Fail(format!("UnicodeDecodeError: 'utf-8' codec can't decode byte at position {}", e.utf8_error().valid_up_to()))
        })?;
        let mut lines = Vec::new();
        let r = listing(&text.replace("\r\n", "\n").replace('\r', "\n"), &mut lines);
        lines.iter().for_each(|l| print(l));
        r?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 几个文件挨个读下来：出没出异常、打出来的行
    fn listed(texts: &[&str]) -> (R<()>, Vec<String>) {
        let mut lines = Vec::new();
        let r = texts.iter().try_for_each(|t| listing(t, &mut lines));
        (r, lines)
    }

    // 一台 Linux 上 /proc/net/tcp 的样子（地址换成了文档用的）；期望的输出是原来那段 Python 对着同一份文字给的
    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1EDD 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 30231 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1EDE 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 30232 1 0000000000000000 100 0 0 10 0
   2: 0A0000C0:8D2E 0A0000C0:01BB 01 00000000:00000000 02:000006E2 00000000  1000        0 41001 2 0000000000000000 20 4 30 10 -1
   3: 0200007F:2FBE 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 30299 1 0000000000000000 100 0 0 10 0
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:4BC6 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 30300 1 0000000000000000 100 0 0 10 0
";

    #[test]
    fn listening_sockets_become_netstat_lines() {
        let (r, out) = listed(&[TCP, TCP6]);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            out,
            [
                "tcp 0 0 0.0.0.0:7901 0.0.0.0:* LISTEN",
                "tcp 0 0 0.0.0.0:7902 0.0.0.0:* LISTEN",
                "tcp 0 0 0.0.0.0:12222 0.0.0.0:* LISTEN",
                "tcp 0 0 0.0.0.0:19398 0.0.0.0:* LISTEN"
            ]
        );
    }

    #[test]
    fn the_header_and_short_rows_are_skipped() {
        // 只有表头、空文件、不到 4 栏的行：什么都不打
        for text in ["", "sl local_address rem_address st\n", "h\n\n 0: x 0A\n"] {
            let (r, out) = listed(&[text]);
            assert!(r.is_ok() && out.is_empty(), "{text:?}：{r:?} {out:?}");
        }
        // 第一行不管写的什么都当表头
        let (_, out) = listed(&["0: 00000000:1EDD 00000000:0000 0A\n"]);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn rows_python_would_choke_on() {
        let (r, out) = listed(&["h\n 0: 00000000:0050 x 0A\n 1: nocolon x 0A\n"]);
        assert_eq!(out, ["tcp 0 0 0.0.0.0:80 0.0.0.0:* LISTEN"]);
        assert_eq!(r.unwrap_err().0, "IndexError: list index out of range");
        let (r, _) = listed(&["h\n 0: 00000000:zz x 0A\n"]);
        assert_eq!(r.unwrap_err().0, "ValueError: invalid literal for int() with base 16: 'zz'");
    }
}
