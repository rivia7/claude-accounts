//! 终端输出的最小识别：只认到“这一段是什么序列”，不模拟屏幕。
//!
//! 会话宿主把客户端的输出转给各个接入方，有三件事要认出序列才做得对：
//! - **查询**（问终端的能力、光标位置、颜色……）：每个接入方的终端都会回答，回答当成键盘输入进了客户端。所以只有一个接入方
//!   （主的那个，`Route`）收到查询，别的收到的输出里去掉；回放给后接入的也不带
//! - **模式**（备用屏幕、括号粘贴、鼠标上报、光标显隐、kitty 键盘协议……）：后接入的终端没看到当初打开它们的那几条序列，
//!   接入时要先补上（`Modes::restore`）；断开时接入的那个终端要退回原样（`Modes::undo`）
//! - **回放**（`Ring`）：最近的输出留在内存里（不落盘），整条序列一起进出，不会从半条序列开始
//!
//! 客户端的输出里可能有别人能左右的内容（文件、网页、模型说的话）。主的那个终端收到的和直接运行时逐字节一样；别的
//! 终端和回放**只收认得出来、完整、确定不是查询的序列**：残缺的、被打断的、太长的、认不出的一律不给（`Class::Junk`）。
//! 不然去掉中间一条查询以后，前后两截残缺的字节会在别的终端上拼成一条原来没有的序列（比如写剪贴板）。
//! 状态机照 VT500 的那一套（ESC 在哪都重新开始；CAN、SUB 中止；序列中间的控制字符照常执行、序列接着；字符串遇到 ESC 就结束）。

use std::collections::VecDeque;

/// 一段输出是什么
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Class {
    /// 普通的字符和控制字符
    Text,
    /// 一条完整的 CSI / ESC 序列，不是查询（进回放）
    Seq,
    /// 超链接（OSC 8：进回放，不然后面的字没有链接）
    Link,
    /// 有别的效果、不该重放的：别的 OSC（标题、通知、颜色）、窗口操作
    Effect,
    /// DCS / APC / PM / SOS 的字符串（图片之类；不进回放）
    Str,
    /// OSC 52：写剪贴板（只给主的那个终端）
    Clip,
    /// 要终端回答的查询（只给主的那个终端）
    Query,
    /// 残缺的、被打断的、太长的、认不出的（只给主的那个终端）
    Junk,
}

impl Class {
    /// 不是主的终端也收
    pub fn for_all(self) -> bool {
        matches!(self, Class::Text | Class::Seq | Class::Link | Class::Effect | Class::Str)
    }

    /// 进回放
    pub fn replayed(self) -> bool {
        matches!(self, Class::Text | Class::Seq | Class::Link)
    }
}

/// 交出来的这一段是整条的，还是一条太长的序列的一截
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Part {
    /// 整条序列，或者一截普通字节
    Whole,
    /// 一条太长的序列的开头（后面还有）
    Head,
    /// 接着上一截（最后一截也是它）
    More,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Seg {
    pub class: Class,
    pub part: Part,
    /// 一条字符串（OSC、DCS……）自己没带结束符：结束它的是下一段开头的那个 ESC。下一段要是不发给某个终端，
    /// 就得给它补一个结束符（`Route`），不然它后面的输出都被当成这条字符串的内容
    pub open: bool,
}

/// 还在哪一种序列里
#[derive(Clone, Copy, PartialEq)]
enum St {
    Ground,
    /// 刚看到 0xC2：下一个字节是 0x80–0x9F 的话是 UTF-8 写法的 C1 控制字符
    C2,
    Esc,
    /// ESC 后面带中间字节的（`ESC ( B`）
    EscInter,
    Csi,
    /// 太长的 CSI：到结尾字节为止都不要了
    CsiIgnore,
    Osc,
    /// DCS、APC、PM、SOS
    Str,
    /// 太长的字符串：开头已经交出去了，后面的照同一种一截截交，到结束为止（bool：是 OSC，BEL 也能结束它）
    Long(Class, bool),
}

/// 一条序列先攒着，完整了才知道是不是查询；字符串攒到这么长还没完就不等了
const HOLD_MAX: usize = 4096;
/// CSI 没有这么长的
const CSI_MAX: usize = 256;

/// 看完一个字节
enum Step {
    /// 收下了，序列还没完
    More,
    /// 序列完了（这个字节算在里面）
    End(Class),
    /// 序列完了，这个字节不算在里面，回到开头重新看它（bool：是一条没带结束符的字符串）
    Redo(Class, bool),
    /// 太长了：攒着的先当开头交出去，后面的照这一种一截截交
    Spill(Class, St),
}

/// 边读边认。`feed` 把这一块输出切成一段段交给 `sink`；一条序列跨了两次 `feed` 时先攒着，完整了再交
/// （太长的不等：先交开头，后面的一截截交）。交出去的字节接起来就是喂进来的，一个不多一个不少
pub struct Scanner {
    st: St,
    cur: Vec<u8>,
}

impl Default for Scanner {
    fn default() -> Self {
        Scanner { st: St::Ground, cur: Vec::new() }
    }
}

fn whole(class: Class) -> Seg {
    Seg { class, part: Part::Whole, open: false }
}

impl Scanner {
    /// 不在一条序列的半中间：这时候往终端里插一条自己的序列不会打断会话的
    pub fn idle(&self) -> bool {
        matches!(self.st, St::Ground)
    }

    pub fn feed(&mut self, data: &[u8], sink: &mut dyn FnMut(Seg, &[u8])) {
        let mut text_from = 0; // Ground 里还没交出去的普通字节从哪开始（只在 Ground 里有意义）
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            match self.st {
                St::Ground => {
                    let c1 = b == 0xc2 && matches!(data.get(i + 1), Some(0x80..=0x9f) | None);
                    if b != 0x1b && !c1 {
                        i += 1;
                        continue;
                    }
                    if text_from < i {
                        sink(whole(Class::Text), &data[text_from..i]);
                    }
                    if b == 0x1b {
                        self.cur.clear();
                        self.cur.push(b);
                        self.st = St::Esc;
                        i += 1;
                    } else if i + 1 < data.len() {
                        // UTF-8 写法的 C1 控制字符（有的终端把它当 CSI、OSC 的开头）：别的终端不给
                        sink(whole(Class::Junk), &data[i..i + 2]);
                        i += 2;
                        text_from = i;
                    } else {
                        self.st = St::C2; // 下一个字节还没到
                        i += 1;
                    }
                }
                St::C2 => {
                    if (0x80..=0x9f).contains(&b) {
                        sink(whole(Class::Junk), &[0xc2, b]);
                        i += 1;
                    } else {
                        sink(whole(Class::Text), &[0xc2]); // 只是一个字的头一个字节：这个字节回到开头照常看
                    }
                    self.st = St::Ground;
                    text_from = i;
                }
                St::CsiIgnore => {
                    let start = i;
                    let mut done = false;
                    while i < data.len() && !done {
                        let c = data[i];
                        if c == 0x1b || c >= 0x80 {
                            done = true; // 不算在里面
                        } else {
                            i += 1;
                            done = (0x40..=0x7e).contains(&c) || c == 0x18 || c == 0x1a;
                        }
                    }
                    if start < i {
                        sink(Seg { class: Class::Junk, part: Part::More, open: false }, &data[start..i]);
                    }
                    if done {
                        self.st = St::Ground;
                        text_from = i;
                    }
                }
                St::Long(class, osc) => {
                    let start = i;
                    let mut end: Option<bool> = None; // Some(是不是被 ESC 结束的)
                    while i < data.len() && end.is_none() {
                        let c = data[i];
                        if c == 0x1b {
                            end = Some(true); // 不算在里面
                        } else {
                            i += 1;
                            if (c == 0x07 && osc) || c == 0x18 || c == 0x1a {
                                end = Some(false);
                            }
                        }
                    }
                    // 被 ESC 结束的那一截哪怕是空的也要交：接收的一边靠它知道这条字符串没带结束符
                    if start < i || end == Some(true) {
                        sink(Seg { class, part: Part::More, open: end == Some(true) }, &data[start..i]);
                    }
                    if end.is_some() {
                        self.st = St::Ground;
                        text_from = i;
                    }
                }
                _ => match self.step(b) {
                    Step::More => i += 1,
                    Step::End(class) => {
                        sink(whole(class), &self.cur);
                        self.cur.clear();
                        self.st = St::Ground;
                        i += 1;
                        text_from = i;
                    }
                    Step::Redo(class, open) => {
                        sink(Seg { class, part: Part::Whole, open }, &self.cur);
                        self.cur.clear();
                        self.st = St::Ground;
                        text_from = i;
                    }
                    Step::Spill(class, next) => {
                        sink(Seg { class, part: Part::Head, open: false }, &self.cur);
                        self.cur.clear();
                        self.st = next;
                        i += 1;
                    }
                },
            }
        }
        if self.st == St::Ground && text_from < data.len() {
            sink(whole(Class::Text), &data[text_from..]);
        }
    }

    /// 在一条序列里（Esc、EscInter、Csi、Osc、Str）看一个字节
    fn step(&mut self, b: u8) -> Step {
        let keep = |cur: &mut Vec<u8>| cur.push(b);
        match (self.st, b) {
            // 哪里都一样：CAN、SUB 中止这条序列（它们自己算在里面）
            (_, 0x18 | 0x1a) => {
                keep(&mut self.cur);
                Step::End(Class::Junk)
            }
            (St::Esc, b'[') => self.enter(b, St::Csi),
            (St::Esc, b']') => self.enter(b, St::Osc),
            (St::Esc, b'P' | b'_' | b'^' | b'X') => self.enter(b, St::Str),
            (St::Esc | St::EscInter, 0x20..=0x2f) => self.enter(b, St::EscInter),
            (St::Esc | St::EscInter, 0x30..=0x7e) => {
                keep(&mut self.cur);
                Step::End(if clean(&self.cur) == b"\x1bZ" { Class::Query } else { Class::Seq })
            }
            (St::Csi, 0x40..=0x7e) => {
                keep(&mut self.cur);
                Step::End(csi_class(&clean(&self.cur)))
            }
            // ESC 重新开始；不是 ASCII 的字节不该出现在这几种序列里：前面攒的都不要了
            (St::Esc | St::EscInter | St::Csi, 0x1b | 0x80..=0xff) => Step::Redo(Class::Junk, false),
            // 序列中间的控制字符、DEL：终端照常执行（或者不理），序列接着
            (St::Esc | St::EscInter, _) => {
                keep(&mut self.cur);
                Step::More
            }
            (St::Csi, _) => {
                keep(&mut self.cur);
                if self.cur.len() > CSI_MAX {
                    Step::Spill(Class::Junk, St::CsiIgnore)
                } else {
                    Step::More
                }
            }
            (St::Osc, 0x07) => {
                keep(&mut self.cur);
                Step::End(osc_class(&self.cur[2..self.cur.len() - 1]))
            }
            (St::Osc, 0x1b) => Step::Redo(osc_class(&self.cur[2..]), true),
            (St::Osc, _) => {
                keep(&mut self.cur);
                if self.cur.len() > HOLD_MAX {
                    // 这么长的只认写剪贴板；别的（一长串颜色查询、认不出的）别的终端不给
                    let class = if osc_class(&self.cur[2..]) == Class::Clip { Class::Clip } else { Class::Junk };
                    Step::Spill(class, St::Long(class, true))
                } else {
                    Step::More
                }
            }
            (St::Str, 0x1b) => Step::Redo(str_class(&self.cur), true),
            (St::Str, _) => {
                keep(&mut self.cur);
                if self.cur.len() > HOLD_MAX {
                    let class = str_class(&self.cur);
                    Step::Spill(class, St::Long(class, false))
                } else {
                    Step::More
                }
            }
            (St::Ground | St::C2 | St::CsiIgnore | St::Long(..), _) => unreachable!(),
        }
    }

    fn enter(&mut self, b: u8, st: St) -> Step {
        self.cur.push(b);
        self.st = st;
        Step::More
    }
}

/// 序列里夹着的控制字符、DEL 去掉（终端执行掉或者不理它们，认序列时当它们不在）
fn clean(seq: &[u8]) -> Vec<u8> {
    seq.iter().enumerate().filter(|(i, b)| *i == 0 || (**b >= 0x20 && **b != 0x7f)).map(|(_, b)| *b).collect()
}

/// `ESC [ 前缀 参数 中间字节 结尾`拆开：(前缀，参数那一段，中间字节，结尾)。不是 CSI 是 None
pub fn csi_parts(seq: &[u8]) -> Option<(u8, &[u8], &[u8], u8)> {
    if seq.len() < 3 || &seq[..2] != b"\x1b[" {
        return None;
    }
    let last = *seq.last()?;
    let body = &seq[2..seq.len() - 1];
    let (prefix, body) = match body.first() {
        Some(p @ (b'?' | b'>' | b'<' | b'=')) => (*p, &body[1..]),
        _ => (0, body),
    };
    let split = body.iter().position(|b| (0x20..=0x2f).contains(b)).unwrap_or(body.len());
    Some((prefix, &body[..split], &body[split..], last))
}

/// 参数（`;` 隔开；空的、不是数的当 None）
pub fn csi_nums(params: &[u8]) -> Vec<Option<u32>> {
    params
        .split(|b| *b == b';')
        .map(|p| {
            // 子参数（`4:3`）只看冒号前面的
            let p = p.split(|b| *b == b':').next().unwrap_or(p);
            std::str::from_utf8(p).ok().and_then(|s| s.parse().ok())
        })
        .collect()
}

/// 一条完整的 CSI（夹着的控制字符已经去掉）
fn csi_class(seq: &[u8]) -> Class {
    let Some((prefix, params, inter, last)) = csi_parts(seq) else { return Class::Junk };
    let query = match (last, inter) {
        (b'c', b"") => prefix != b'?',             // DA1 / DA2 / DA3（`CSI ? … c` 是 Linux 控制台的光标样子）
        (b'n', b"") => matches!(prefix, 0 | b'?'), // DSR：状态、光标位置、配色（`CSI > … n` 是关掉 modifyOtherKeys）
        (b'u', b"") => prefix == b'?',             // kitty 键盘协议：现在的标志
        (b'q', b"") => prefix == b'>',             // XTVERSION
        (b'p', b"$") => true,                      // DECRQM
        (b'x', b"") => true,                       // DECREQTPARM
        (b'm', b"") => prefix == b'?',             // XTQMODKEYS
        (b'S', b"") => prefix == b'?',             // XTSMGRAPHICS
        (b'y', b"*") => true,                      // DECRQCRA
        (b'w', b"$") => true,                      // DECRQPSR
        (b'u', b"&") => true,                      // DECRQUPSS
        (b't', b"") => matches!(csi_nums(params).first(), Some(Some(11 | 13..=16 | 18..=21))), // 窗口大小、标题
        _ => false,
    };
    match (query, last, inter) {
        (true, ..) => Class::Query,
        (false, b't', b"") => Class::Effect, // 别的窗口操作（改大小、标题入栈）：不重放
        _ => Class::Seq,
    }
}

/// OSC 开头的那个数（`052` 也是 52；没有数是 None）
fn osc_number(body: &[u8]) -> Option<u32> {
    let digits = body.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 9 || !matches!(body.get(digits), None | Some(b';')) {
        return None;
    }
    std::str::from_utf8(&body[..digits]).ok()?.parse().ok()
}

/// 这几种 OSC 的内容是给人看的字（标题、通知）：里面单独一个 `?` 不是在问
const OSC_PLAIN: &[u32] = &[0, 1, 2, 9, 777];

/// 一条 OSC 的内容（`ESC ]` 和结束符之间的）
fn osc_class(body: &[u8]) -> Class {
    let Some(n) = osc_number(body) else { return Class::Junk };
    let fields = || body.split(|b| *b == b';').skip(1);
    let asks = fields().any(|p| p == b"?")                        // OSC 4 / 10 / 11 / 12 / 52 / 7501……的“现在是什么”
        || (n == 21 && fields().any(|p| p.ends_with(b"=?")))      // kitty 的颜色协议
        || (n == 99 && body.windows(3).any(|w| w == b"p=?")); // kitty 通知协议问支持什么
    match n {
        _ if asks && !OSC_PLAIN.contains(&n) => Class::Query,
        52 => Class::Clip,
        8 => Class::Link,
        _ => Class::Effect,
    }
}

/// 一条 DCS / APC / PM / SOS（从 ESC 起；可能还没到结尾）
fn str_class(seq: &[u8]) -> Class {
    let body = seq.get(2..).unwrap_or(b"");
    match seq.get(1) {
        Some(b'P') => {
            // 参数后面是 `+q`（XTGETTCAP）、`$q`（DECRQSS）的是查询
            let rest = &body[body.iter().take_while(|b| (0x30..=0x3f).contains(*b)).count()..];
            if rest.starts_with(b"+q") || rest.starts_with(b"$q") {
                Class::Query
            } else {
                Class::Str
            }
        }
        // kitty 的图片协议：带编号的命令终端都会回话，一律只给主的那个
        Some(b'_') if body.starts_with(b"G") => Class::Query,
        _ => Class::Str,
    }
}

/// 一个接入方（或者回放）该收到哪些段。两条规矩：太长的序列，开头没收到的，后面那几截也不收（半路接入的、
/// 半路换了主的）；收了一条没带结束符的字符串、下一段又不收时，补一个结束符
#[derive(Default)]
pub struct Route {
    in_long: bool,
    pending_st: bool,
}

impl Route {
    /// wanted：这一种这个接入方收不收。该给它的字节接到 out 后面
    pub fn take(&mut self, wanted: bool, seg: Seg, bytes: &[u8], out: &mut Vec<u8>) {
        let deliver = match seg.part {
            Part::Whole => wanted,
            Part::Head => {
                self.in_long = wanted;
                wanted
            }
            Part::More => self.in_long,
        };
        if deliver {
            out.extend_from_slice(bytes);
            self.pending_st = seg.open;
        } else if self.pending_st {
            out.extend_from_slice(b"\x1b\\");
            self.pending_st = false;
        }
    }
}

/// 终端现在开着哪些模式（只跟踪会影响后接入的终端的那些）
#[derive(Default, Clone)]
pub struct Modes {
    /// DEC 私有模式（`CSI ? n h / l`）：见过的，按第一次出现的先后。只记 `DEC_TRACKED` 里的
    dec: Vec<(u32, bool)>,
    /// kitty 键盘协议压进去的标志（`CSI > f u`）
    kitty: Vec<u32>,
    /// 见过 kitty 键盘协议的序列（没见过就不往终端发它的序列）
    kitty_seen: bool,
    /// `CSI > 4 ; n m`
    modify_other_keys: Option<u32>,
    /// `ESC =`
    keypad_app: bool,
    /// `CSI n SP q`
    cursor_style: Option<u32>,
}

/// 备用屏幕
const DEC_ALT: &[u32] = &[47, 1047, 1049];
/// 跟踪的 DEC 私有模式：光标键、反显、原点、自动换行、光标闪烁和显隐、备用屏幕、小键盘、鼠标上报的几种、焦点、
/// 括号粘贴、字素、配色通知、窗口大小通知、win32 输入。别的不记（一条乱写的序列占不满这张表；会清屏、换成 VT52 的
/// 那几个也不该重放）；同步输出（2026）只是一阵子的
const DEC_TRACKED: &[u32] =
    &[1, 5, 6, 7, 12, 25, 47, 66, 1000, 1002, 1003, 1004, 1005, 1006, 1015, 1016, 1047, 1049, 2004, 2027, 2031, 2048, 9001];
const KITTY_MAX: usize = 16;

impl Modes {
    /// 看一条完整的序列（`Scanner` 交出来的 `Class::Seq`）
    pub fn observe(&mut self, seq: &[u8]) {
        let seq = &clean(seq);
        match seq.as_slice() {
            b"\x1b=" => self.keypad_app = true,
            b"\x1b>" => self.keypad_app = false,
            b"\x1bc" => *self = Modes { kitty_seen: self.kitty_seen, ..Modes::default() }, // RIS：整个复位
            _ => {}
        }
        let Some((prefix, params, inter, last)) = csi_parts(seq) else { return };
        let nums = csi_nums(params);
        match (prefix, last, inter) {
            (b'?', b'h' | b'l', b"") => {
                for n in nums.into_iter().flatten().filter(|n| DEC_TRACKED.contains(n)) {
                    match self.dec.iter_mut().find(|(m, _)| *m == n) {
                        Some(slot) => slot.1 = last == b'h',
                        None => self.dec.push((n, last == b'h')),
                    }
                }
            }
            (b'>', b'u', b"") => {
                self.kitty_seen = true;
                if self.kitty.len() < KITTY_MAX {
                    self.kitty.push(nums.first().copied().flatten().unwrap_or(0));
                }
            }
            (b'<', b'u', b"") => {
                self.kitty_seen = true;
                let n = nums.first().copied().flatten().unwrap_or(1).max(1) as usize;
                self.kitty.truncate(self.kitty.len().saturating_sub(n));
            }
            (b'=', b'u', b"") => {
                self.kitty_seen = true;
                let flags = nums.first().copied().flatten().unwrap_or(0);
                let how = nums.get(1).copied().flatten().unwrap_or(1);
                if let Some(top) = self.kitty.last_mut() {
                    *top = match how {
                        2 => *top | flags,
                        3 => *top & !flags,
                        _ => flags,
                    };
                } else if how != 3 && flags != 0 {
                    self.kitty.push(flags); // 没压过就直接设：断开时照样要弹掉
                }
            }
            (b'>', b'm', b"") => {
                if nums.first() == Some(&Some(4)) {
                    self.modify_other_keys = nums.get(1).copied().flatten().filter(|n| *n != 0);
                }
            }
            (0, b'q', b" ") => self.cursor_style = nums.first().copied().flatten().filter(|n| *n != 0),
            _ => {}
        }
    }

    /// 在备用屏幕里
    #[cfg(test)]
    pub fn alt_screen(&self) -> bool {
        self.dec.iter().any(|(m, on)| *on && DEC_ALT.contains(m))
    }

    /// 让一个刚接入的终端进到同样的模式：接入时先发这一段，再回放，最后是 `restore_keys`
    pub fn restore(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // 备用屏幕先切过去，后面的回放才画在对的那一屏上
        for (m, on) in &self.dec {
            if DEC_ALT.contains(m) && *on {
                out.extend_from_slice(format!("\x1b[?{m}h").as_bytes());
            }
        }
        for (m, on) in &self.dec {
            if !DEC_ALT.contains(m) {
                out.extend_from_slice(format!("\x1b[?{}{}", m, if *on { 'h' } else { 'l' }).as_bytes());
            }
        }
        if let Some(n) = self.modify_other_keys {
            out.extend_from_slice(format!("\x1b[>4;{n}m").as_bytes());
        }
        if self.keypad_app {
            out.extend_from_slice(b"\x1b=");
        }
        if let Some(n) = self.cursor_style {
            out.extend_from_slice(format!("\x1b[{n} q").as_bytes());
        }
        out
    }

    /// kitty 键盘协议的标志是一层层压进去的，回放里又有当时压、弹的那几条：放在回放**后面**，先全弹掉再照现在的压一遍，
    /// 层数才和客户端以为的一样（不然后接入的终端多压了几层，客户端弹掉自己那几层以后它还开着）
    pub fn restore_keys(&self) -> Vec<u8> {
        if !self.kitty_seen {
            return Vec::new();
        }
        let mut out = b"\x1b[<99u".to_vec();
        for f in &self.kitty {
            out.extend_from_slice(format!("\x1b[>{f}u").as_bytes());
        }
        out
    }

    /// 断开时让这个终端退回原样：关掉开着的模式、弹掉 kitty 的标志、光标显出来、颜色复位，最后才离开备用屏幕
    pub fn undo(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (m, on) in &self.dec {
            match (*m, *on) {
                (m, _) if DEC_ALT.contains(&m) => {}
                (7 | 25, false) => out.extend_from_slice(format!("\x1b[?{m}h").as_bytes()), // 自动换行、光标：本来是开着的
                (7 | 25, true) => {}
                (m, true) => out.extend_from_slice(format!("\x1b[?{m}l").as_bytes()),
                (_, false) => {}
            }
        }
        if !self.kitty.is_empty() {
            out.extend_from_slice(format!("\x1b[<{}u", self.kitty.len()).as_bytes());
        }
        if self.modify_other_keys.is_some() {
            out.extend_from_slice(b"\x1b[>4m");
        }
        if self.keypad_app {
            out.extend_from_slice(b"\x1b>");
        }
        if self.cursor_style.is_some() {
            out.extend_from_slice(b"\x1b[0 q");
        }
        out.extend_from_slice(b"\x1b[0m");
        for (m, on) in &self.dec {
            if DEC_ALT.contains(m) && *on {
                out.extend_from_slice(format!("\x1b[?{m}l").as_bytes());
            }
        }
        out
    }
}

/// 一块最多攒这么多再另起一块（整块整块地丢）
const CHUNK: usize = 4096;

/// 最近的输出（只在内存里）。进来的都是整段（一条完整的序列，或者一截普通字节），按整块丢：回放不会从半条序列开始
pub struct Ring {
    chunks: VecDeque<Vec<u8>>,
    size: usize,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring { chunks: VecDeque::new(), size: 0, cap }
    }

    /// 接上一段
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        match self.chunks.back_mut() {
            Some(last) if last.len() + bytes.len() <= CHUNK => last.extend_from_slice(bytes),
            _ => self.chunks.push_back(bytes.to_vec()),
        }
        self.size += bytes.len();
        while self.size > self.cap && self.chunks.len() > 1 {
            if let Some(c) = self.chunks.pop_front() {
                self.size -= c.len();
            }
        }
    }

    /// 回放给后接入的：开头如果正好是半个 UTF-8 字符的后半截，去掉
    pub fn replay(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size);
        for c in &self.chunks {
            out.extend_from_slice(c);
        }
        let skip = out.iter().take(3).take_while(|b| (0x80..=0xbf).contains(*b)).count();
        out.drain(..skip);
        out
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.size
    }
}

/// 一块输出里进回放的那些：普通字节（响铃去掉：回放不该再响一遍）、完整的序列、超链接
pub fn replay_bytes(seg: Seg, bytes: &[u8]) -> Vec<u8> {
    match seg.class {
        Class::Text => bytes.iter().copied().filter(|b| *b != 0x07).collect(),
        _ => bytes.to_vec(),
    }
}

/// 接入方（或者启动它的终端）的一块输入是不是在打字：整块都是鼠标上报（点、拖、移动、滚轮）和焦点进出（`CSI I`、`CSI O`）
/// 的不算——手机上滚着看、鼠标从窗口上划过、切到这个窗口，都不该让会话改成照它的大小排（宿主的 `lead`）。认不出来的都算打字
pub fn typing(input: &[u8]) -> bool {
    let mut rest = input;
    while !rest.is_empty() {
        match report_len(rest) {
            Some(n) => rest = &rest[n..],
            None => return true,
        }
    }
    false
}

/// 开头是一条鼠标上报、焦点进出时它有几个字节。鼠标上报三种写法：SGR（`CSI < 键 ; 列 ; 行` 加 `M` 按下、`m` 松开；
/// Claude Code 这些用的）、urxvt（`CSI 键 ; 列 ; 行 M`）、X10（`CSI M` 后面三个字节，各是 32 加上键、列、行）
fn report_len(b: &[u8]) -> Option<usize> {
    let body = b.strip_prefix(b"\x1b[")?;
    match *body.first()? {
        b'I' | b'O' => Some(3),
        b'M' => body.get(1..4).filter(|xyz| xyz.iter().all(|c| *c >= 0x20)).map(|_| 6),
        first @ (b'<' | b'0'..=b'9') => {
            let sgr = first == b'<';
            let nums = &body[usize::from(sgr)..];
            let end = nums.iter().position(|c| !c.is_ascii_digit() && *c != b';')?;
            let fields = &nums[..end];
            let three = fields.split(|c| *c == b';').filter(|f| !f.is_empty() && f.len() <= 5).count() == 3
                && fields.iter().filter(|c| **c == b';').count() == 2;
            let fin = nums[end];
            (three && (fin == b'M' || (sgr && fin == b'm'))).then_some(2 + usize::from(sgr) + end + 1)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<(Seg, Vec<u8>)> {
        let mut s = Scanner::default();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut |seg, bytes| out.push((seg, bytes.to_vec())));
        }
        out
    }

    fn classes(data: &[u8]) -> Vec<(Class, Vec<u8>)> {
        scan(&[data]).into_iter().map(|(s, b)| (s.class, b)).collect()
    }

    /// 不是主的终端收到的（`Route`，每个字节一次地喂：结果不该和一次给完有差别）
    fn others(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for step in [data.len().max(1), 1, 7] {
            let (mut s, mut r, mut got) = (Scanner::default(), Route::default(), Vec::new());
            for c in data.chunks(step) {
                s.feed(c, &mut |seg, bytes| r.take(seg.class.for_all(), seg, bytes, &mut got));
            }
            if step != data.len().max(1) {
                assert_eq!(got, out, "分成 {step} 个字节一块喂，结果不一样");
            }
            out = got;
        }
        out
    }

    #[test]
    fn text_and_sequences() {
        assert_eq!(classes(b"hello"), vec![(Class::Text, b"hello".to_vec())]);
        assert_eq!(
            classes(b"a\x1b[1;32mb\x1b[0m\r\n"),
            vec![
                (Class::Text, b"a".to_vec()),
                (Class::Seq, b"\x1b[1;32m".to_vec()),
                (Class::Text, b"b".to_vec()),
                (Class::Seq, b"\x1b[0m".to_vec()),
                (Class::Text, b"\r\n".to_vec()),
            ]
        );
        // 两个字节的、带中间字节的
        assert_eq!(
            classes(b"\x1b7\x1b(B\x1b8"),
            vec![(Class::Seq, b"\x1b7".to_vec()), (Class::Seq, b"\x1b(B".to_vec()), (Class::Seq, b"\x1b8".to_vec())]
        );
        // 标题（BEL 结尾）；ST 结尾的：字符串到 ESC 为止，`ESC \` 自己是一条
        assert_eq!(
            classes(b"\x1b]0;title\x07x"),
            vec![(Class::Effect, b"\x1b]0;title\x07".to_vec()), (Class::Text, b"x".to_vec())]
        );
        assert_eq!(
            classes(b"\x1b]8;;http://example.com\x1b\\"),
            vec![(Class::Link, b"\x1b]8;;http://example.com".to_vec()), (Class::Seq, b"\x1b\\".to_vec())]
        );
        assert!(scan(&[b"\x1b]8;;x\x1b\\"])[0].0.open && !scan(&[b"\x1b]8;;x\x07"])[0].0.open);
        // 中文、别的 UTF-8 是普通字节；0xC2 开头的普通字（¢）也是
        assert_eq!(classes("你好 ¢5".as_bytes()), vec![(Class::Text, "你好 ¢5".as_bytes().to_vec())]);
    }

    #[test]
    fn every_byte_comes_out_once() {
        let mut data =
            b"ab\x1b[6nc\x1b]11;?\x1b\\d\x1b[?1049h\x1bP+q54\x1b\\e\x1b\x1b[c]52;c;aGk=\x07\x1b[1\x18x\x1b(\x1b[c0".to_vec();
        data.extend("\u{9b}c 你 \u{a2}".as_bytes());
        data.extend_from_slice(b"\x1b]52;c;");
        data.extend(std::iter::repeat_n(b'A', 6000));
        data.extend_from_slice(b"\x07\x1b[");
        data.extend(std::iter::repeat_n(b'1', 400));
        data.extend_from_slice(b"mtail\x1b_Gf=1;");
        data.extend(std::iter::repeat_n(b'B', 5000));
        data.extend_from_slice(b"\x1b\\end\xc2");
        for step in [data.len(), 1, 2, 3, 5, 64, 4095, 4096, 4097] {
            let mut s = Scanner::default();
            let mut got = Vec::new();
            for c in data.chunks(step) {
                s.feed(c, &mut |_seg, bytes| got.extend_from_slice(bytes));
            }
            // 结尾那个 0xC2 还攒着（等下一个字节）
            assert_eq!(got, data[..data.len() - 1], "{step} 个字节一块");
        }
    }

    #[test]
    fn queries() {
        for q in [
            &b"\x1b[c"[..],
            b"\x1b[0c",
            b"\x1b[>c",
            b"\x1b[>0c",
            b"\x1b[=c",
            b"\x1b[5n",
            b"\x1b[6n",
            b"\x1b[?6n",
            b"\x1b[?996n",
            b"\x1b[?u",
            b"\x1b[>q",
            b"\x1b[>0q",
            b"\x1b[?2026$p",
            b"\x1b[2004$p",
            b"\x1b[14t",
            b"\x1b[18t",
            b"\x1b[21t",
            b"\x1b[?4m",
            b"\x1b[?1;1;0S",
            b"\x1b[1;1;1;1;1;1*y",
            b"\x1b[1$w",
            b"\x1b[&u",
            b"\x1b[x",
            b"\x1bZ",
            b"\x1b]10;?\x07",
            b"\x1b]4;1;?\x07",
            b"\x1b]52;c;?\x07",
            b"\x1b]052;c;?\x07",
            b"\x1b]011;?\x07",
            b"\x1b]7501;?\x07", // Claude Code 2.1.328 启动时发的
            b"\x1b]21;foreground=?\x07",
            b"\x1b]99;i=1:p=?;\x07",
            // 序列中间夹着控制字符：终端执行掉它，序列照样是查询
            b"\x1b[\x0b?u",
            b"\x1b\r[6\x7fn",
        ] {
            assert_eq!(classes(q), vec![(Class::Query, q.to_vec())], "{:?}", String::from_utf8_lossy(q));
            assert_eq!(others(q), b"", "{:?}", String::from_utf8_lossy(q));
        }
        // ST 结尾的查询：字符串是查询，后面的 `ESC \` 留着（单独一个结束符什么都不做）
        for q in [
            &b"\x1b]11;?"[..],
            b"\x1bP+q544e",
            b"\x1bP$qm",
            b"\x1bP1+q544e",
            b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA",
            b"\x1b_Gf=24,i=7;AA",
        ] {
            let mut seq = q.to_vec();
            seq.extend_from_slice(b"\x1b\\");
            assert_eq!(classes(&seq), vec![(Class::Query, q.to_vec()), (Class::Seq, b"\x1b\\".to_vec())]);
            assert_eq!(others(&seq), b"\x1b\\");
        }
        for (n, class) in [
            (&b"\x1b[2 q"[..], Class::Seq), // 光标的样子
            (b"\x1b[u", Class::Seq),        // 恢复光标
            (b"\x1b[>1u", Class::Seq),      // kitty：压一层
            (b"\x1b[<u", Class::Seq),       // kitty：弹一层
            (b"\x1b[=1;1u", Class::Seq),    // kitty：设
            (b"\x1b[>4;2m", Class::Seq),    // modifyOtherKeys
            (b"\x1b[>4n", Class::Seq),      // 关掉 modifyOtherKeys（不是 DSR）
            (b"\x1b[?2c", Class::Seq),      // Linux 控制台的光标样子（不是 DA）
            (b"\x1b[?25l", Class::Seq),
            (b"\x1b[1;1H", Class::Seq),
            (b"\x1b[38;2;1;2;3m", Class::Seq),
            (b"\x1b[8;24;80t", Class::Effect), // 改窗口大小
            (b"\x1b[22;0t", Class::Effect),    // 标题入栈
            (b"\x1b]0;x\x07", Class::Effect),
            (b"\x1b]0;?\x07", Class::Effect), // 标题就是一个问号
            (b"\x1b]2;a;?\x07", Class::Effect),
            (b"\x1b]9;?\x07", Class::Effect),                   // 通知
            (b"\x1b]11;rgb:0000/0000/0000\x07", Class::Effect), // 设背景色，不是问
            (b"\x1b]8;id=1;http://example.com/?\x07", Class::Link),
            (b"\x1b]52;c;aGk=\x07", Class::Clip),
            (b"\x1b]nonsense\x07", Class::Junk), // 没有数的 OSC：认不出
        ] {
            assert_eq!(classes(n), vec![(class, n.to_vec())], "{:?}", String::from_utf8_lossy(n));
        }
        assert_eq!(classes(b"\x1bPq#0;2;0;0;0\x1b\\")[0], (Class::Str, b"\x1bPq#0;2;0;0;0".to_vec()));
        assert_eq!(classes(b"\x1b^private\x1b\\")[0].0, Class::Str);
    }

    /// 去掉查询以后，它前后的残缺字节不能在别的终端上拼成一条原来没有的序列
    #[test]
    fn dropping_a_query_never_splices_its_neighbours() {
        // ESC ESC [ c ] 52;c;aGk= BEL：主的终端看到一个 ESC、一条 DA、一截字。别的终端不能看到 ESC ] 52…（写剪贴板）
        assert_eq!(others(b"\x1b\x1b[c]52;c;aGk=\x07"), b"]52;c;aGk=\x07");
        assert_eq!(others(b"\x1b\x1b[c]0;title\x07"), b"]0;title\x07");
        // ESC ( ESC [ c 0：不能变成 ESC ( 0（换成画线字符集）
        assert_eq!(others(b"\x1b(\x1b[c0"), b"0");
        // ESC [ 1 ESC [ c m：不能变成 ESC [ 1 m
        assert_eq!(others(b"\x1b[1\x1b[cm"), b"m");
        // 被 CAN / SUB 中止的、夹着不是 ASCII 的字节的：整截不给
        assert_eq!(others(b"\x1b[1\x18;2H"), b";2H");
        assert_eq!(others("\x1b[1é2H".as_bytes()), "é2H".as_bytes());
        // UTF-8 写法的 C1 控制字符（有的终端把 U+009B 当 CSI）：别的终端不给，后面的就只是字
        assert_eq!(others("\u{9b}c\u{9d}52;c;aGk=\u{9c}".as_bytes()), b"c52;c;aGk=");
        // 字符串没带结束符、结束它的那条序列又不给：补一个结束符，后面的字才不会被当成这条字符串的内容
        assert_eq!(others(b"\x1b]0;title\x1b[cvisible"), b"\x1b]0;title\x1b\\visible");
        assert_eq!(others(b"\x1bPq#0\x1b\x1b[6nvisible"), b"\x1bPq#0\x1b\\visible");
        // 带了结束符的、后面那条照发的：不多补
        assert_eq!(others(b"\x1b]0;title\x07\x1b[cx"), b"\x1b]0;title\x07x");
        assert_eq!(others(b"\x1b]0;title\x1b\\x"), b"\x1b]0;title\x1b\\x");
        assert_eq!(others(b"\x1b]0;title\x1b[1mx"), b"\x1b]0;title\x1b[1mx");
    }

    /// 靠“太长”绕不过去
    #[test]
    fn overlong_sequences_are_not_passed_on() {
        // 参数很多的 DA：整条（连结尾的 c）别的终端都不收
        let mut da = b"\x1b[".to_vec();
        da.extend(b"0;".repeat(300));
        da.extend_from_slice(b"cafter");
        assert_eq!(others(&da), b"after");
        let segs = scan(&[&da]);
        assert!(segs[..segs.len() - 1].iter().all(|(s, _)| s.class == Class::Junk));
        assert_eq!((segs[0].0.part, segs[1].0.part), (Part::Head, Part::More));
        // 一长串颜色查询
        let mut osc = b"\x1b]4;".to_vec();
        osc.extend(b"0;?;".repeat(2000));
        osc.extend_from_slice(b"\x07after");
        assert_eq!(others(&osc), b"after");
        // 很长的 XTGETTCAP
        let mut dcs = b"\x1bP+q".to_vec();
        dcs.extend(std::iter::repeat_n(b'5', 9000));
        dcs.extend_from_slice(b"\x1b\\after");
        assert_eq!(others(&dcs), b"\x1b\\after");
        // 很长的剪贴板：只给主的；很长的图片（sixel）：都给
        let mut clip = b"\x1b]52;c;".to_vec();
        clip.extend(std::iter::repeat_n(b'A', 20000));
        clip.extend_from_slice(b"\x07tail");
        assert_eq!(others(&clip), b"tail");
        let segs = scan(&[&clip[..9000], &clip[9000..]]);
        assert!(segs[..segs.len() - 1].iter().all(|(s, _)| s.class == Class::Clip));
        let mut img = b"\x1bPq".to_vec();
        img.extend(std::iter::repeat_n(b'~', 9000));
        img.extend_from_slice(b"\x1b\\ok");
        assert_eq!(others(&img), img);
        // 长字符串里的 BEL 不是 DCS / APC 的结束符，ESC 才是
        let segs = scan(&[b"\x1b^", &[b'x'; 5000], b"\x07still", b"\x1b\\Z"]);
        assert_eq!(segs.last().unwrap(), &(whole(Class::Text), b"Z".to_vec()));
        assert!(segs.iter().any(|(s, b)| s.class == Class::Str && b.ends_with(b"\x07still")));
    }

    /// 半路接进来的（或者半路才变成主的）：一条太长的序列只收到后半截的话，不收
    #[test]
    fn a_late_route_skips_the_rest_of_a_long_sequence() {
        let mut img = b"\x1bPq".to_vec();
        img.extend(std::iter::repeat_n(b'~', 9000));
        img.extend_from_slice(b"\x1b\\after\x1bPq~~\x1b\\");
        let (mut s, mut early, mut late) = (Scanner::default(), Route::default(), Route::default());
        let (mut a, mut b) = (Vec::new(), Vec::new());
        s.feed(&img[..6000], &mut |seg, bytes| early.take(true, seg, bytes, &mut a));
        s.feed(&img[6000..], &mut |seg, bytes| {
            early.take(true, seg, bytes, &mut a);
            late.take(true, seg, bytes, &mut b);
        });
        assert_eq!(a, img);
        assert_eq!(b, b"\x1b\\after\x1bPq~~\x1b\\", "前半截没收到的图片，后半截也不收；下一条照常");
    }

    #[test]
    fn modes_restore_and_undo() {
        let mut m = Modes::default();
        let mut s = Scanner::default();
        let mut see = |m: &mut Modes, stream: &[u8]| {
            s.feed(stream, &mut |seg, bytes| {
                if seg.class == Class::Seq {
                    m.observe(bytes);
                }
            })
        };
        see(
            &mut m,
            b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[?1004h\x1b[>1u\x1b[?2026h\x1b[?2026l\x1b[?1000;1006h\x1b=\x1b[2 q\x1b[>4;2m",
        );
        assert!(m.alt_screen());
        let restore = String::from_utf8(m.restore()).unwrap();
        assert!(restore.starts_with("\x1b[?1049h"), "{restore:?}");
        for part in ["\x1b[?25l", "\x1b[?2004h", "\x1b[?1004h", "\x1b[?1000h", "\x1b[?1006h", "\x1b[>4;2m", "\x1b=", "\x1b[2 q"] {
            assert!(restore.contains(part), "{part:?} 不在 {restore:?} 里");
        }
        assert!(!restore.contains("2026"), "同步输出只是一阵子的");
        assert!(!restore.contains('u'), "kitty 的标志放在回放后面");
        assert_eq!(m.restore_keys(), b"\x1b[<99u\x1b[>1u");
        let undo = String::from_utf8(m.undo()).unwrap();
        for part in [
            "\x1b[?25h",
            "\x1b[?2004l",
            "\x1b[?1004l",
            "\x1b[?1000l",
            "\x1b[?1006l",
            "\x1b[<1u",
            "\x1b[>4m",
            "\x1b>",
            "\x1b[0 q",
            "\x1b[0m",
        ] {
            assert!(undo.contains(part), "{part:?} 不在 {undo:?} 里");
        }
        assert!(undo.ends_with("\x1b[?1049l"), "最后才离开备用屏幕：{undo:?}");

        // 关掉了的不用再关；kitty 弹掉了、键盘模式退回了
        see(&mut m, b"\x1b[?1049l\x1b[?25h\x1b[?2004l\x1b[<u\x1b>\x1b[>4m\x1b[0 q");
        assert!(!m.alt_screen());
        let undo = String::from_utf8(m.undo()).unwrap();
        assert!(!undo.contains("1049") && !undo.contains("\x1b[<") && !undo.contains("2004"), "{undo:?}");
        assert_eq!(m.restore_keys(), b"\x1b[<99u", "见过 kitty 的序列：后接入的也全弹掉");
        // 整个复位
        m.observe(b"\x1bc");
        assert_eq!(m.restore(), b"");
        assert_eq!(m.undo(), b"\x1b[0m");
        // 从没见过 kitty 的序列：不往终端发它的
        assert_eq!(Modes::default().restore_keys(), b"");
    }

    /// 一条乱写的序列占不满模式表；会清屏、换成 VT52 的那几个不记
    #[test]
    fn only_known_modes_are_tracked() {
        let mut m = Modes::default();
        let many: Vec<String> = (3000..3200).map(|n| n.to_string()).collect();
        m.observe(format!("\x1b[?{}h", many.join(";")).as_bytes());
        m.observe(b"\x1b[?3h");
        m.observe(b"\x1b[?2l");
        assert_eq!(m.restore(), b"");
        m.observe(b"\x1b[?1049h");
        m.observe(b"\x1b[?25;2004;1003h");
        m.observe(b"\x1b[?1003l");
        assert_eq!(m.restore(), b"\x1b[?1049h\x1b[?25h\x1b[?2004h\x1b[?1003l");
        assert_eq!(m.undo(), b"\x1b[?2004l\x1b[0m\x1b[?1049l");
        // 三种备用屏幕都认
        let mut m = Modes::default();
        m.observe(b"\x1b[?47h");
        m.observe(b"\x1b[?1047h");
        assert!(m.alt_screen());
        assert!(m.undo().ends_with(b"\x1b[?47l\x1b[?1047l"));
    }

    #[test]
    fn kitty_set_without_push() {
        let mut m = Modes::default();
        m.observe(b"\x1b[=5;1u");
        assert_eq!(m.restore_keys(), b"\x1b[<99u\x1b[>5u");
        m.observe(b"\x1b[=4;3u"); // 去掉 4
        assert_eq!(m.restore_keys(), b"\x1b[<99u\x1b[>1u");
        m.observe(b"\x1b[=8;2u"); // 加上 8
        assert_eq!(m.restore_keys(), b"\x1b[<99u\x1b[>9u");
        assert_eq!(m.undo(), b"\x1b[<1u\x1b[0m");
        m.observe(b"\x1b[<9u"); // 弹得比压的多
        assert_eq!(m.restore_keys(), b"\x1b[<99u");
        // 接入的那一头也看得到 restore_keys：它的层数和终端的对得上
        let mut seen = Modes::default();
        for seq in [&b"\x1b[>1u"[..], b"\x1b[>3u", b"\x1b[<99u", b"\x1b[>7u"] {
            seen.observe(seq);
        }
        assert_eq!(seen.undo(), b"\x1b[<1u\x1b[0m");
    }

    #[test]
    fn ring_drops_whole_chunks() {
        let mut r = Ring::new(10_000);
        let mut s = Scanner::default();
        // 很多条“一截字加一条颜色序列”：丢掉前面的以后，回放仍然从一条序列或者一截字的开头开始
        let line = b"\x1b[38;5;200mline of text 0123456789\x1b[0m\r\n";
        for _ in 0..2000 {
            s.feed(line, &mut |_seg, bytes| r.push(bytes));
        }
        assert!(r.len() <= 10_000 + CHUNK);
        let text = String::from_utf8(r.replay()).unwrap();
        assert!(text.len() >= 5000 && text.ends_with("\x1b[0m\r\n"));
        let first = &text[..40];
        assert!(first.starts_with("\x1b[") || first.starts_with("line") || first.starts_with("\r\n"), "{first:?}");
        // 一段比整个回放还大：留着它（下一块进来时才丢）
        let mut r = Ring::new(100);
        r.push(&[b'x'; CHUNK + 1]);
        assert_eq!(r.replay().len(), CHUNK + 1);
        r.push(b"y");
        assert_eq!(r.replay(), b"y");
    }

    /// 进回放的只有普通字节（不带响铃）、完整的序列和超链接：查询、剪贴板、通知、标题、图片、窗口操作、残缺的都不进
    #[test]
    fn what_goes_into_the_replay() {
        let stream = b"a\x07b\x1b[1mc\x1b[6n\x1b]52;c;aGk=\x07\x1b]9;notify\x07\x1b]0;title\x07\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\\x1bPq#0\x1b\\\x1b[8;24;80t\x1b[\x18d";
        let (mut s, mut route, mut ring) = (Scanner::default(), Route::default(), Ring::new(1 << 20));
        s.feed(stream, &mut |seg, bytes| {
            let mut out = Vec::new();
            route.take(seg.class.replayed() && seg.part == Part::Whole, seg, &replay_bytes(seg, bytes), &mut out);
            ring.push(&out);
        });
        assert_eq!(ring.replay(), b"ab\x1b[1mc\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\\x1b\\d");
    }

    #[test]
    fn replay_skips_half_a_character() {
        let mut r = Ring::new(100);
        r.push(&"你".as_bytes()[1..]);
        r.push(b"ok");
        assert_eq!(r.replay(), b"ok");
    }

    #[test]
    fn what_counts_as_typing() {
        let cases: &[(&[u8], bool)] = &[
            (b"a", true),
            (b"\r", true),
            (b"\x1b", true),
            (b"\x1b[A", true),                  // 方向键
            (b"\x1b[200~paste\x1b[201~", true), // 粘贴
            (b"\x1b[97;5u", true),              // kitty 键盘协议
            (b"\x1b[15~", true),                // F5
            (b"\x1b[<64;10;5M", false),         // 滚轮往上（SGR）
            (b"\x1b[<65;10;5M\x1b[<65;10;5M", false),
            (b"\x1b[<35;80;24M", false),          // 鼠标移动
            (b"\x1b[<0;3;4M\x1b[<0;3;4m", false), // 点一下
            (b"\x1b[I", false),                   // 焦点进来、出去
            (b"\x1b[O\x1b[I", false),
            (b"\x1b[Ma!!", false),      // X10 的滚轮往下
            (b"\x1b[97;10;5M", false),  // urxvt 的
            (b"\x1b[<64;10;5Mx", true), // 滚轮后面跟着字
            (b"x\x1b[<64;10;5M", true),
            (b"\x1b[<64;10M", true), // 不完整的、样子不对的
            (b"\x1b[<64;10;5", true),
            (b"\x1b[<64;;5M", true),
            (b"\x1b[<64;1;2;3M", true),
            (b"\x1b[97;10;5m", true),
            (b"\x1b[M!!", true),
            (b"", false),
        ];
        for (input, typed) in cases {
            assert_eq!(typing(input), *typed, "{:?}", String::from_utf8_lossy(input));
        }
    }
}
