//! 接入端从按键里认出“断开”：先按 Ctrl-\，再按 d（或者 D、.）。连按两次 Ctrl-\ 给会话本身发一个；前缀后面是别的键，
//! 两个都照发。
//!
//! 会话里的程序会让终端换一种办法报按键，Ctrl-\ 就不再是一个字节 0x1c：kitty 键盘协议是 `CSI 92 ; 5 u`（开了“报松开”
//! 时松开是 `CSI 92 ; 5 : 3 u`，大写锁定、数字锁定开着时修饰那一项还要再加 64、128），modifyOtherKeys 是
//! `CSI 27 ; 5 ; 92 ~`；d 也可能报成 `CSI 100 u`。这几种都认。粘贴的内容（括号粘贴的两个标记之间）原样送过去，
//! 里面碰巧有这两个键也不算。

/// 括号粘贴的开头、结尾
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
/// 修饰里的大写锁定、数字锁定：不算
const LOCKS: u32 = 64 | 128;
const CTRL: u32 = 4;
const SHIFT: u32 = 1;

/// 终端报上来的一个键（kitty 键盘协议或者 modifyOtherKeys 的写法）
struct Key {
    /// 这条序列多长
    len: usize,
    code: u32,
    mods: u32,
    /// 1 按下、2 按住重复、3 松开
    event: u32,
}

impl Key {
    fn bits(&self) -> u32 {
        self.mods.saturating_sub(1) & !LOCKS
    }

    /// Ctrl-\
    fn is_prefix(&self) -> bool {
        self.code == 92 && self.bits() == CTRL
    }

    /// d、D、.（带不带 Shift）
    fn is_detach(&self) -> bool {
        matches!(self.code, 100 | 68 | 46) && self.bits() & !SHIFT == 0
    }

    fn released(&self) -> bool {
        self.event == 3
    }
}

/// 这一截的开头是不是一个报上来的键。Ok(None)：不是；Err：可能是，还没到齐
fn key_at(input: &[u8]) -> Result<Option<Key>, ()> {
    let Some(body) = input.strip_prefix(b"\x1b[") else { return Ok(None) };
    let Some(end) = body.iter().position(|b| !(b.is_ascii_digit() || matches!(b, b';' | b':'))) else {
        return if body.len() <= 32 { Err(()) } else { Ok(None) };
    };
    // 各项用 ; 隔开，一项里再用 : 隔开
    let num = |field: usize, sub: usize| -> Option<u32> {
        let f = body[..end].split(|b| *b == b';').nth(field)?;
        std::str::from_utf8(f.split(|b| *b == b':').nth(sub)?).ok()?.parse().ok()
    };
    let key = match body[end] {
        b'u' => num(0, 0).map(|code| Key { len: end + 3, code, mods: num(1, 0).unwrap_or(1), event: num(1, 1).unwrap_or(1) }),
        b'~' if num(0, 0) == Some(27) => num(2, 0).map(|code| Key { len: end + 3, code, mods: num(1, 0).unwrap_or(1), event: 1 }),
        _ => None,
    };
    Ok(key)
}

#[derive(Default)]
pub struct Keys {
    /// 按过前缀了（记着按下去的原样：后面不是断开键时原样补发）
    armed: Option<Vec<u8>>,
    /// 按过前缀以后来了半条序列：等它到齐再看是不是断开键
    partial: Vec<u8>,
    /// 在粘贴的内容里：结尾的标记已经对上了几个字节
    paste: Option<usize>,
}

impl Keys {
    /// 这一批按键里要送给会话的写进 out；true：要断开
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) -> bool {
        let joined;
        let input = if self.partial.is_empty() {
            input
        } else {
            joined = [std::mem::take(&mut self.partial).as_slice(), input].concat();
            joined.as_slice()
        };
        let mut i = 0;
        while i < input.len() {
            let rest = &input[i..];
            if let Some(matched) = self.paste {
                // 粘贴的内容原样过去，到结尾的标记为止
                let b = rest[0];
                out.push(b);
                i += 1;
                let matched = if b == PASTE_END[matched] {
                    matched + 1
                } else if b == PASTE_END[0] {
                    1
                } else {
                    0
                };
                self.paste = (matched < PASTE_END.len()).then_some(matched);
                continue;
            }
            if rest.starts_with(PASTE_START) {
                out.extend(self.armed.take().unwrap_or_default());
                out.extend_from_slice(PASTE_START);
                i += PASTE_START.len();
                self.paste = Some(0);
                continue;
            }
            let key = match key_at(rest) {
                Ok(key) => key,
                Err(()) if self.armed.is_some() => {
                    self.partial = rest.to_vec();
                    return false;
                }
                Err(()) => None,
            };
            let tok = &rest[..key.as_ref().map_or(1, |k| k.len)];
            let (prefix, detach, release) = match &key {
                Some(k) => (k.is_prefix() && !k.released(), k.is_detach() && !k.released(), k.is_prefix() && k.released()),
                None => (tok[0] == 0x1c, matches!(tok[0], b'd' | b'D' | b'.'), false),
            };
            match self.armed.take() {
                Some(held) if release => self.armed = Some(held), // 前缀松开了：不算一个键
                Some(_) if detach => return true,
                Some(held) if prefix => out.extend_from_slice(&held), // 连按两次：给会话一个
                Some(held) => {
                    out.extend_from_slice(&held);
                    out.extend_from_slice(tok);
                }
                None if prefix => self.armed = Some(tok.to_vec()),
                None => out.extend_from_slice(tok),
            }
            i += tok.len();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(keys: &mut Keys, input: &[u8]) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let detach = keys.feed(input, &mut out);
        (out, detach)
    }

    /// 一批批喂进去：送给会话的，和有没有断开
    fn run(batches: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut k = Keys::default();
        let mut all = Vec::new();
        for b in batches {
            let (out, detach) = feed(&mut k, b);
            all.extend(out);
            if detach {
                return (all, true);
            }
        }
        (all, false)
    }

    #[test]
    fn plain_bytes() {
        assert_eq!(run(&[b"hello\r"]), (b"hello\r".to_vec(), false));
        // 前缀 + d：断开，前缀不送过去
        assert_eq!(run(&[b"ab\x1c", b"d"]), (b"ab".to_vec(), true));
        assert_eq!(run(&[b"x\x1c."]), (b"x".to_vec(), true));
        assert_eq!(run(&[b"\x1cD"]), (b"".to_vec(), true));
        // 连按两次：给会话一个
        assert_eq!(run(&[b"\x1c\x1cz"]), (b"\x1cz".to_vec(), false));
        // 前缀后面是别的键：两个都照发；不在前缀后面的 d 就是 d
        assert_eq!(run(&[b"\x1c", b"q", b"d"]), (b"\x1cqd".to_vec(), false));
        // 别的序列原样过去（方向键、功能键、鼠标）
        let other = b"\x1b[A\x1b[1;5C\x1bOP\x1b[<0;10;5M\x1b[15~\x1b";
        assert_eq!(run(&[other]), (other.to_vec(), false));
        // 前缀后面跟着方向键：都照发
        assert_eq!(run(&[b"\x1c\x1b[A"]), (b"\x1c\x1b[A".to_vec(), false));
    }

    #[test]
    fn kitty_and_modify_other_keys() {
        // kitty 键盘协议下的 Ctrl-\ 和 d
        assert_eq!(run(&[b"\x1b[92;5u", b"\x1b[100u"]), (b"".to_vec(), true));
        assert_eq!(run(&[b"\x1b[92;5u", b"d"]), (b"".to_vec(), true));
        assert_eq!(run(&[b"\x1b[92;5u\x1b[100;1u"]), (b"".to_vec(), true));
        assert_eq!(run(&[b"\x1b[92;5u\x1b[100;;100u"]), (b"".to_vec(), true), "带着文字那一项的");
        assert_eq!(run(&[b"\x1b[92;5u\x1b[100;2u"]), (b"".to_vec(), true), "Shift-d");
        assert_eq!(run(&[b"\x1b[92;5u\x1b[46u"]), (b"".to_vec(), true));
        // 开了“报松开”：前缀松开不打断；松开的 d 不算
        assert_eq!(run(&[b"\x1b[92;5:1u", b"\x1b[92;5:3u", b"\x1b[100;1:1u"]), (b"".to_vec(), true));
        // 大写锁定、数字锁定开着
        assert_eq!(run(&[b"\x1b[92;69u", b"\x1b[100;65u"]), (b"".to_vec(), true));
        assert_eq!(run(&[b"\x1b[92;133u\x1b[100;129u"]), (b"".to_vec(), true));
        // modifyOtherKeys
        assert_eq!(run(&[b"\x1b[27;5;92~", b"d"]), (b"".to_vec(), true));
        // 连按两次：给会话一个（按下的那条）
        assert_eq!(run(&[b"\x1b[92;5u\x1b[92;5u"]), (b"\x1b[92;5u".to_vec(), false));
        // 别的键：前缀照发
        assert_eq!(run(&[b"\x1b[92;5u\x1b[A"]), (b"\x1b[92;5u\x1b[A".to_vec(), false));
        assert_eq!(run(&[b"\x1b[92;5u\x1b[97u"]), (b"\x1b[92;5u\x1b[97u".to_vec(), false));
        // 不是 Ctrl 的 \（Alt-\、Ctrl-Shift-\）、带 Ctrl 的 d：都不是
        assert_eq!(run(&[b"\x1b[92;3u", b"d"]), (b"\x1b[92;3ud".to_vec(), false));
        assert_eq!(run(&[b"\x1b[92;6u", b"d"]), (b"\x1b[92;6ud".to_vec(), false));
        assert_eq!(run(&[b"\x1b[92;5u\x1b[100;5u"]), (b"\x1b[92;5u\x1b[100;5u".to_vec(), false));
    }

    #[test]
    fn a_key_split_across_reads() {
        // 前缀后面的 d 分两次到：等它到齐
        assert_eq!(run(&[b"\x1b[92;5u", b"\x1b[10", b"0u"]), (b"".to_vec(), true));
        assert_eq!(run(&[b"\x1c\x1b[", b"100;1u"]), (b"".to_vec(), true));
        // 到齐了不是断开键：一个字节都不少
        assert_eq!(run(&[b"\x1c\x1b[1", b";5C", b"z"]), (b"\x1c\x1b[1;5Cz".to_vec(), false));
        // 没按过前缀时不等：半条序列照常马上送过去
        let mut k = Keys::default();
        assert_eq!(feed(&mut k, b"\x1b[1"), (b"\x1b[1".to_vec(), false));
        assert_eq!(feed(&mut k, b";5C"), (b";5C".to_vec(), false));
        // 单独一个 Esc 键不等
        assert_eq!(run(&[b"\x1c", b"\x1b", b"x"]), (b"\x1c\x1bx".to_vec(), false));
    }

    #[test]
    fn pasted_text_is_never_the_gesture() {
        // 粘贴的内容里碰巧有 Ctrl-\ 和 d：原样过去，不断开
        let paste = b"\x1b[200~line one\x1cd line two\x1b[92;5u\x1b[100u\x1b[201~";
        assert_eq!(run(&[paste]), (paste.to_vec(), false));
        // 结尾的标记分几次到；粘贴完了手势照常
        assert_eq!(run(&[b"\x1b[200~a\x1cd\x1b[2", b"01", b"~", b"\x1cd"]), (b"\x1b[200~a\x1cd\x1b[201~".to_vec(), true));
        // 内容里有半个结尾的标记
        assert_eq!(run(&[b"\x1b[200~\x1b[20\x1b[201~\x1c", b"x"]), (b"\x1b[200~\x1b[20\x1b[201~\x1cx".to_vec(), false));
        // 按了前缀以后粘贴：前缀照发
        assert_eq!(run(&[b"\x1c\x1b[200~d\x1b[201~"]), (b"\x1c\x1b[200~d\x1b[201~".to_vec(), false));
    }
}
