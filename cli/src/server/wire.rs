//! 线上来的东西怎么查：按字节取的游标、数字、各个正则、请求里各项的检查（Py:89-99、242-350、933-941）。
//!
//! 请求的 JSON 在处理它的线程里解析成 `V`，这里的检查把它取成普通的值（`Blob`、`OMap<…>`），之后才进共用的状态。
//! 每个检查都照 Python 的那一个写，宽严一样：
//! - 布尔值不算数字（Python 里 `True` 是 `int`，所以原来处处写着 `not isinstance(x, bool)`）
//! - `x.get(k) or 默认`：给了假的值（`null`、`0`、空串、空的列表和字典）和没给一样
//! - 正则照原样用 `re!` 写：Python 的 `$` 也认结尾那个换行之前，`re!` 留着这个行为（原来收得下的，这里也收得下）
//! - 长度按字符数（`py::text::len`）
//!
//! 注意 `py::obj` 的 `Dict::at`、`List::at` 没有那一项时是 panic：这棵目录里只用 `get`。

use super::omap::OMap;
use super::sshsig;
use super::{MAX_ACCOUNTS, MAX_AGE, MAX_TEAM_BATCH, MAX_TEAM_DOC, MAX_TEAM_SIG, MAX_UNITS, SEQ_MAX};
use crate::py::obj::{Dict, List, V};
use crate::py::re::Re;
use crate::py::{base64, text};
use crate::re;

// ---------------------------------------------------------------- 游标

/// 一段字节上的游标：一段一段往后取，不够就是 None（不会 panic）。线上来的二进制都经它读
pub struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    pub fn new(bytes: &'a [u8]) -> Cur<'a> {
        Cur(bytes)
    }

    /// 往后取 n 个字节
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let head = self.0.get(..n)?;
        self.0 = self.0.get(n..)?;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    /// 2 个字节的整数（高位在前）
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    /// 4 个字节的整数（高位在前）
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    /// 8 个字节的整数（高位在前）
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    /// 还没取的
    pub fn rest(&self) -> &'a [u8] {
        self.0
    }

    /// 取完了
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ---------------------------------------------------------------- 数字

/// JSON 里的一个数，记着它原来是整数还是小数：状态文件里读进来的时间原样写回去（原来是 `1101` 的不变成 `1101.0`）；
/// 服务器自己新记的时间是 `Float(time())`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    /// 是数字才给（布尔值不算）。放不进 i64 的整数当小数：只有手改坏了的状态文件里才有，写回去时样子会变
    pub fn of(v: &V) -> Option<Num> {
        match v {
            V::Int(i) => Some(Num::Int(*i)),
            V::Float(f) => Some(Num::Float(*f)),
            V::Big(_) => v.as_num().map(Num::Float),
            _ => None,
        }
    }

    /// `float(x)`
    pub fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    /// 写回 JSON：原来是什么样还是什么样
    pub fn v(self) -> V {
        match self {
            Num::Int(i) => V::Int(i),
            Num::Float(f) => V::Float(f),
        }
    }
}

// ---------------------------------------------------------------- 正则（都照 Python 里的原样）

/// 公钥的指纹（Py:89）
pub fn fp_re() -> &'static Re {
    re!(r"^SHA256:[A-Za-z0-9+/]{43}$")
}

/// 账号摘要；配对编号、配对的 rid 这些 32 个十六进制字符的也拿它查（Py:90、1092）
pub fn aid_re() -> &'static Re {
    re!(r"^[0-9a-f]{32}$")
}

/// 请求的 nonce（Py:91）
pub fn nonce_re() -> &'static Re {
    re!(r"^[0-9a-f]{32}$")
}

/// 设备的公钥那一行；第 1 组是 base64 的那一段（Py:92）
pub fn key_re() -> &'static Re {
    re!(r"^ssh-ed25519 (AAAAC3NzaC1lZDI1NTE5AAAAI[A-Za-z0-9+/]{43})$")
}

/// 名字里不许有的控制字符，拿 `found` 查（Py:93）
pub fn name_bad() -> &'static Re {
    re!(r"[\x00-\x1f\x7f]")
}

/// 规整的 base64（Py:94）
pub fn b64_re() -> &'static Re {
    re!(r"^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$")
}

/// 认证码：32 个字节的 base64（Py:95）
pub fn tag_re() -> &'static Re {
    re!(r"^[A-Za-z0-9+/]{43}=$")
}

/// 邀请里那串令牌的 SHA-256，服务器只存它（Py:96）
pub fn hash_re() -> &'static Re {
    re!(r"^[0-9a-f]{64}$")
}

/// 加入用的令牌（邀请里的）（Py:97）
pub fn admit_re() -> &'static Re {
    re!(r"^[0-9a-f]{32}$")
}

/// 这一份记录的编号（Py:98）
pub fn gen_re() -> &'static Re {
    re!(r"^[0-9a-f]{32}$")
}

/// 设备在 Team 里的编号（Py:99）
pub fn dev_re() -> &'static Re {
    re!(r"^[0-9a-f]{16}$")
}

/// WebSocket 握手的 Sec-WebSocket-Key（Py:933）
pub fn ws_key_re() -> &'static Re {
    re!(r"^[A-Za-z0-9+/]{22}==$")
}

/// --web-url 的样子；第 1 组是去掉结尾那个 / 的（Py:934）。端口的 `\d` 和 Python 一样也认别的文字里的数字，
/// 取端口时要照 Python 的 `int()` 来（`py::text::to_int`）
pub fn web_url_re() -> &'static Re {
    re!(r"^(https://[A-Za-z0-9.-]+(?::\d{1,5})?|http://(?:localhost|127\.0\.0\.1)(?::\d{1,5})?)/?$")
}

/// 页面的文件名（Py:935）
pub fn web_name_re() -> &'static Re {
    re!(r"^[a-z0-9][a-z0-9._-]{0,63}$")
}

/// WebSocket 那个地址里 ? 后面的；第 1 组是票据的编号（Py:936）
pub fn ws_ticket_re() -> &'static Re {
    re!(r"^t=([0-9a-f]{32})$")
}

/// 数据端口的开场白：角色、票据（或令牌）、连接的随机数、认证码（Py:941）。Python 里是按字节匹配的：先看是不是 UTF-8
/// （不是就一定对不上：里面全是 ASCII），再拿它查
pub fn relay_line() -> &'static Re {
    re!(r"^CCA-RELAY/1 (host|view|join) ([0-9a-f]{32}) ([0-9a-f]{16}) ([A-Za-z0-9_-]{43})$")
}

/// index.html 的标题，换成带 Team 名字的（只换头一个）（Py:1608）
pub fn title_re() -> &'static Re {
    re!(r"<title>[^<]*</title>")
}

/// index.html 里 iPhone 主屏幕上的名字；第 1、2 组是它前后的（只换头一个）（Py:1609）
pub fn app_title_re() -> &'static Re {
    re!(r#"(<meta name="apple-mobile-web-app-title" content=")[^"]*(")"#)
}

// ---------------------------------------------------------------- 各项的检查

/// 上报的“开了多久、闲置多久”：0 到 MAX_AGE 的数（Py:242-243）
pub fn num_ok(x: &V) -> bool {
    match x {
        V::Int(i) => (0..=MAX_AGE).contains(i),
        V::Float(f) => 0.0 <= *f && *f <= MAX_AGE as f64,
        _ => false, // 放不进 i64 的整数一定在范围外
    }
}

/// 服务器记的时间（时间戳）：正数（Py:246-248）
pub fn is_time(x: &V) -> bool {
    match x {
        V::Int(i) => *i > 0,
        V::Float(f) => *f > 0.0,
        V::Big(s) => !s.starts_with('-'),
        _ => false,
    }
}

/// 从 1 起数的次数：正整数（Py:251-253）
pub fn is_count(x: &V) -> bool {
    match x {
        V::Int(i) => *i > 0,
        V::Big(s) => !s.starts_with('-'),
        _ => false,
    }
}

/// `[开了多久, 闲置多久]`（秒）；不合格是 None（Py:256-260）
pub fn pair(p: &V) -> Option<[f64; 2]> {
    let l = p.as_list()?;
    if l.len() != 2 {
        return None;
    }
    let (a, b) = (l.get(0)?, l.get(1)?);
    if !(num_ok(&a) && num_ok(&b)) {
        return None;
    }
    Some([a.as_num()?, b.as_num()?])
}

/// report 里一个账号的会话
#[derive(Clone, Debug, PartialEq)]
pub struct UnitIn {
    /// 命令行会话，每个是 `[开了多久, 闲置多久]`
    pub cli: Vec<[f64; 2]>,
    /// 桌面 App
    pub app: Option<[f64; 2]>,
}

/// report 的 units：`{账号摘要: {"cli": [[开了多久, 闲置多久], …], "app": [开了多久, 闲置多久] | null}}`；不合格是 None（Py:263-280）
pub fn parse_units(u: &V) -> Option<OMap<UnitIn>> {
    let d = u.as_dict()?;
    if d.len() > MAX_ACCOUNTS {
        return None;
    }
    let mut out = OMap::new();
    for (aid, x) in d.items() {
        let x = x.as_dict()?;
        if !aid_re().matches(&aid) {
            return None;
        }
        let cli = match x.get("cli").filter(V::truthy) {
            Some(c) => c.as_list()?,
            None => List::new(),
        };
        if cli.len() > MAX_UNITS {
            return None;
        }
        let pairs = cli.items().iter().map(pair).collect::<Option<Vec<_>>>()?;
        let app = match x.get("app") {
            None | Some(V::None) => None,
            Some(a) => Some(pair(&a)?),
        };
        out.insert(&*aid, UnitIn { cli: pairs, app });
    }
    Some(out)
}

/// 设备传上来的一份密文：`{"n": nonce, "c": 密文, "t": 校验}`，都是 base64。服务器不懂里面是什么，只查形状和长度
#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub n: String,
    pub c: String,
    pub t: String,
}

impl Blob {
    /// 写回 JSON：n、c、t 这个先后（Py:289）
    pub fn to_v(&self) -> V {
        V::Dict(crate::dict! { "n" => &self.n, "c" => &self.c, "t" => &self.t })
    }
}

/// 一份密文合不合格：正好 n、c、t 三项，都是规整的 base64，n 24 个字符、t 44 个、c 不空且不超过 limit；不合格是 None（Py:283-289）
pub fn parse_blob(b: &V, limit: usize) -> Option<Blob> {
    let d = b.as_dict()?;
    if d.len() != 3 {
        return None;
    }
    let field = |k: &str| d.str(k).filter(|s| b64_re().matches(s));
    let (n, c, t) = (field("n")?, field("c")?, field("t")?);
    let size = text::len(&c);
    if text::len(&n) != 24 || text::len(&t) != 44 || size == 0 || size > limit {
        return None;
    }
    Some(Blob { n: n.to_string(), c: c.to_string(), t: t.to_string() })
}

/// keys、ledger 的请求里的三项
#[derive(Clone, Debug, PartialEq)]
pub struct SyncIn {
    /// 要存的：账号摘要 → 密文
    pub put: OMap<Blob>,
    /// 要取的账号摘要（重复的只留头一个）
    pub get: Vec<String>,
    /// 设备手里已经有的：账号摘要 → {设备指纹: 认证码}，没变的不回密文（只有 ledger 用）
    pub have: OMap<OMap<String>>,
}

/// keys、ledger 的 put、get、have；不合格是 None（Py:292-306）
pub fn parse_sync(j: &Dict, limit: usize) -> Option<SyncIn> {
    let given = |k: &str| j.get(k).filter(V::truthy);
    let put = match given("put") {
        Some(v) => v.as_dict()?,
        None => Dict::new(),
    };
    let get = match given("get") {
        Some(v) => v.as_list()?,
        None => List::new(),
    };
    let have = match given("have") {
        Some(v) => v.as_dict()?,
        None => Dict::new(),
    };
    if put.len().max(get.len()).max(have.len()) > MAX_ACCOUNTS {
        return None;
    }
    let mut wanted: Vec<String> = Vec::new();
    for a in get.items() {
        let a = a.as_str().filter(|a| aid_re().matches(a))?;
        if !wanted.iter().any(|w| w == a) {
            wanted.push(a.to_string());
        }
    }
    let mut tags = OMap::new();
    for (aid, h) in have.items() {
        let h = h.as_dict()?;
        if !aid_re().matches(&aid) || h.len() > 256 {
            return None;
        }
        let mut of_devices = OMap::new();
        for (fp, tag) in h.items() {
            let tag = tag.as_str().filter(|t| tag_re().matches(t))?;
            if !fp_re().matches(&fp) {
                return None;
            }
            of_devices.insert(&*fp, tag.to_string());
        }
        tags.insert(&*aid, of_devices);
    }
    let mut blobs = OMap::new();
    for (aid, b) in put.items() {
        if !aid_re().matches(&aid) {
            return None;
        }
        blobs.insert(&*aid, parse_blob(&b, limit)?);
    }
    Some(SyncIn { put: blobs, get: wanted, have: tags })
}

/// 设备的名字：1～64 个字符，没有控制字符，前后没有空白（Py:309-311）
pub fn name_ok(name: &V) -> bool {
    name.as_str().is_some_and(name_text_ok)
}

/// `name_ok`，已经知道是字符串时
pub fn name_text_ok(name: &str) -> bool {
    let n = text::len(name);
    0 < n && n <= 64 && text::strip(name) == name && !name_bad().found(name)
}

/// 设备在 Team 里的编号（Py:314-316）
pub fn dev_ok(dev: &V) -> bool {
    dev.as_str().is_some_and(|d| dev_re().matches(d))
}

/// 配置的序号：从 1 起，比 SEQ_MAX 小（Py:319-321）
pub fn seq_ok(seq: &V) -> bool {
    matches!(seq, V::Int(i) if (1..SEQ_MAX).contains(i))
}

/// team_enroll 里的一台设备
#[derive(Clone, Debug, PartialEq)]
pub struct Enroll {
    /// 公钥的指纹
    pub fp: String,
    /// 公钥那一行（`ssh-ed25519 AAAA…`，不带说明）
    pub key: String,
    pub name: String,
}

/// team_enroll 的 devices：`{设备编号: {"pub": 'ssh-ed25519 …', "name": 名字}}`，一把公钥只能是一台设备；不合格是 None（Py:324-335）
pub fn parse_enroll(devs: &V) -> Option<OMap<Enroll>> {
    let d = devs.as_dict()?;
    if d.is_empty() || d.len() > MAX_TEAM_BATCH {
        return None;
    }
    let mut out: OMap<Enroll> = OMap::new();
    for (dev, x) in d.items() {
        let x = x.as_dict()?;
        let line = x.str("pub")?;
        let b64 = key_re().match_(&line)?.g(1);
        let name = x.str("name").filter(|n| name_text_ok(n))?;
        if !dev_re().matches(&dev) {
            return None;
        }
        let fp = sshsig::key_fp(&base64::b64decode(b64, false).ok()?);
        if out.values().any(|e| e.fp == fp) {
            return None;
        }
        out.insert(&*dev, Enroll { fp, key: format!("ssh-ed25519 {b64}"), name: name.to_string() });
    }
    Some(out)
}

/// team_put 里给一台设备的配置
#[derive(Clone, Debug, PartialEq)]
pub struct DocIn {
    pub seq: i64,
    /// 密文
    pub ct: Blob,
    /// 根密钥的签名（armored 签名的 base64）
    pub sig: String,
}

/// team_put 的 docs：`{设备编号: {"seq": 序号, "ct": 密文, "sig": 签名}}`；不合格是 None（Py:338-350）
pub fn parse_team_docs(docs: &V) -> Option<OMap<DocIn>> {
    let d = docs.as_dict()?;
    if d.is_empty() || d.len() > MAX_TEAM_BATCH {
        return None;
    }
    let mut out = OMap::new();
    for (dev, x) in d.items() {
        let x = x.as_dict()?;
        if !dev_re().matches(&dev) || x.len() != 3 {
            return None;
        }
        let seq = x.get("seq").filter(seq_ok)?.as_int()?;
        let ct = parse_blob(&x.get("ct")?, MAX_TEAM_DOC)?;
        let sig = x.str("sig").filter(|s| b64_re().matches(s))?;
        let size = text::len(&sig);
        if size == 0 || size > MAX_TEAM_SIG {
            return None;
        }
        out.insert(&*dev, DocIn { seq, ct, sig: sig.to_string() });
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::super::{MAX_COST_BLOB, MAX_KEY_BLOB};
    use super::*;
    use crate::dict;
    use crate::py::json::{self, Fmt};
    use std::collections::BTreeMap;

    /// server/cca-presence.py 里各个正则、各项检查给的结果（生成的办法见文件里的 how）
    fn vectors() -> Dict {
        json::loads(include_str!("../../tests/vectors/server_wire.json")).unwrap().as_dict().unwrap()
    }

    /// 写成一行 JSON 来比：整数和小数（1 和 1.0）、字典里各项的先后都分得出来
    fn text(v: &V) -> String {
        json::dumps(v, Fmt::new())
    }

    fn pair_v(p: [f64; 2]) -> V {
        V::from(vec![V::Float(p[0]), V::Float(p[1])])
    }

    #[test]
    fn cursor_takes_what_is_there_and_never_panics() {
        let mut c = Cur::new(&[1, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 4, 9, 8]);
        assert_eq!((c.u8(), c.u16(), c.u32(), c.u64()), (Some(1), Some(2), Some(3), Some(4)));
        assert_eq!((c.rest(), c.is_empty()), (&[9u8, 8][..], false));
        // 不够：什么都不取，游标不动
        assert_eq!((c.u32(), c.u64(), c.take(3), c.take(usize::MAX), c.rest()), (None, None, None, None, &[9u8, 8][..]));
        assert_eq!((c.take(0), c.u16(), c.is_empty()), (Some(&[] as &[u8]), Some(0x0908), true));
        assert_eq!((c.u8(), c.u16(), c.take(1), c.take(0), c.rest()), (None, None, None, Some(&[] as &[u8]), &[] as &[u8]));
        // 高位在前
        let mut c = Cur::new(&[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert_eq!((c.u16(), c.u32(), c.u64()), (Some(0x1234), Some(0x5678_9abc), Some(0xdef0_1122_3344_5566)));
        assert!(Cur::new(&[]).is_empty() && Cur::new(&[]).u8().is_none());
    }

    #[test]
    fn num_keeps_integers_and_floats_apart() {
        let of = |t: &str| Num::of(&json::loads(t).unwrap());
        assert_eq!(
            (of("1101"), of("1101.0"), of("-3"), of("2.5")),
            (Some(Num::Int(1101)), Some(Num::Float(1101.0)), Some(Num::Int(-3)), Some(Num::Float(2.5)))
        );
        assert_eq!((of("true"), of("false"), of("null"), of("\"1\""), of("[1]"), of("{}")), (None, None, None, None, None, None));
        // 写回去还是原来的样子（状态文件里读进来的时间不变样）
        for t in ["1101", "1101.0", "0", "-0.0", "1e+16", "1.5e-07", "1763971200.123456", "9223372036854775807"] {
            assert_eq!(text(&of(t).unwrap().v()), t);
        }
        assert_eq!((Num::Int(7).f(), Num::Float(7.5).f(), Num::Int(-1).f()), (7.0, 7.5, -1.0));
        // 放不进 i64 的整数当小数
        assert_eq!(of("9223372036854775808"), Some(Num::Float(9_223_372_036_854_775_808.0)));
        assert_eq!(of("-123456789012345678901234567890").map(Num::f), Some(-1.2345678901234568e29));
    }

    /// 每个正则都用一遍（写错了的要到头一次用才 panic），给的结果和 Python 的一样：对不对得上、各组是什么。
    /// 结尾带换行的那些也在里面：Python 的 $ 认结尾那个换行之前，原来收得下的这里也收得下
    #[test]
    fn regexes_match_like_python() {
        // 名字（Python 里的）、正则、Python 那边用的是不是 search（别的是 match）
        let table: [(&str, &Re, bool); 18] = [
            ("FP_RE", fp_re(), false),
            ("AID_RE", aid_re(), false),
            ("NONCE_RE", nonce_re(), false),
            ("KEY_RE", key_re(), false),
            ("NAME_BAD", name_bad(), true),
            ("B64_RE", b64_re(), false),
            ("TAG_RE", tag_re(), false),
            ("HASH_RE", hash_re(), false),
            ("ADMIT_RE", admit_re(), false),
            ("GEN_RE", gen_re(), false),
            ("DEV_RE", dev_re(), false),
            ("WS_KEY_RE", ws_key_re(), false),
            ("WEB_URL_RE", web_url_re(), false),
            ("WEB_NAME_RE", web_name_re(), false),
            ("WS_TICKET_RE", ws_ticket_re(), false),
            ("RELAY_LINE", relay_line(), false),
            ("TITLE", title_re(), true),
            ("APP_TITLE", app_title_re(), true),
        ];
        let mut seen: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for case in vectors().list("re").unwrap().items() {
            let case = case.as_dict().unwrap();
            let name = case.str("re").unwrap();
            let (_, re, search) =
                table.iter().find(|(n, _, _)| *n == &*name).unwrap_or_else(|| panic!("对照数据里的 {name} 这里没有"));
            // RELAY_LINE 在 Python 里是按字节匹配的（输入写成 base64）：不是 UTF-8 的一定对不上
            let input: Option<String> = match case.str("b64") {
                Some(b64) => String::from_utf8(base64::b64decode(&b64, true).unwrap()).ok(),
                None => Some(case.str("s").unwrap().to_string()),
            };
            let found = input.as_deref().and_then(|s| if *search { re.search(s) } else { re.match_(s) });
            let got: Option<Vec<Option<&str>>> = found.as_ref().map(|m| (0..=m.groups()).map(|i| m.group(i)).collect());
            let want_groups = case.list("m").map(|l| l.items());
            let want: Option<Vec<Option<&str>>> = want_groups.as_ref().map(|l| l.iter().map(V::as_str).collect());
            assert_eq!(got, want, "{name} {:?}", input);
            // 只问对不对得上的那两个写法说的也一样
            if let Some(s) = input.as_deref() {
                assert_eq!(if *search { re.found(s) } else { re.matches(s) }, want.is_some(), "{name} {s:?}");
            }
            let count = seen.entry(name.to_string()).or_default();
            if want.is_some() {
                count.0 += 1;
            } else {
                count.1 += 1;
            }
        }
        assert_eq!(seen.len(), table.len(), "对照数据里每个正则都有：{seen:?}");
        assert!(seen.values().all(|(hit, miss)| *hit >= 2 && *miss >= 2), "每个正则都有对得上的、对不上的：{seen:?}");
        assert_eq!(seen.values().map(|(hit, miss)| hit + miss).sum::<usize>(), 223);
    }

    /// 请求里各项的检查给的结果和 Python 的一样：收不收、收下以后取成什么（字典里各项的先后、整数还是小数都比）
    #[test]
    fn checks_match_python() {
        let mut seen: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for case in vectors().list("fn").unwrap().items() {
            let case = case.as_dict().unwrap();
            let (name, input) = (case.str("fn").unwrap(), case.str("in").unwrap());
            let x = json::loads(&input).unwrap();
            let limit = || case.get("limit").and_then(|l| l.as_int()).map(|l| l as usize).unwrap();
            let got: V = match &*name {
                "num_ok" => num_ok(&x).into(),
                "is_time" => is_time(&x).into(),
                "is_count" => is_count(&x).into(),
                "seq_ok" => seq_ok(&x).into(),
                "name_ok" => name_ok(&x).into(),
                "dev_ok" => dev_ok(&x).into(),
                "pair" => pair(&x).map(pair_v).into(),
                "parse_units" => parse_units(&x)
                    .map(|units| {
                        units.to_v(|u| {
                            let cli: Vec<V> = u.cli.iter().map(|p| pair_v(*p)).collect();
                            V::Dict(dict! { "cli" => cli, "app" => u.app.map(pair_v) })
                        })
                    })
                    .into(),
                "parse_blob" => parse_blob(&x, limit()).map(|b| b.to_v()).into(),
                "parse_sync" => parse_sync(&x.as_dict().unwrap(), limit())
                    .map(|s| {
                        let get: Vec<V> = s.get.iter().map(V::from).collect();
                        V::from(vec![s.put.to_v(Blob::to_v), V::from(get), s.have.to_v(|tags| tags.to_v(|t| V::from(t)))])
                    })
                    .into(),
                "parse_enroll" => parse_enroll(&x)
                    .map(|devs| devs.to_v(|e| V::from(vec![V::from(&e.fp), V::from(&e.key), V::from(&e.name)])))
                    .into(),
                "parse_team_docs" => parse_team_docs(&x)
                    .map(|docs| docs.to_v(|d| V::Dict(dict! { "seq" => d.seq, "ct" => d.ct.to_v(), "sig" => &d.sig })))
                    .into(),
                other => panic!("对照数据里的 {other} 这里没有"),
            };
            let want = case.get("out").unwrap();
            assert_eq!(text(&got), text(&want), "{name}({input})");
            let count = seen.entry(name.to_string()).or_default();
            if want.truthy() || matches!(want, V::Dict(_) | V::List(_)) {
                count.0 += 1;
            } else {
                count.1 += 1;
            }
        }
        assert_eq!(seen.len(), 12, "{seen:?}");
        assert!(seen.values().all(|(yes, no)| *yes >= 3 && *no >= 8), "每一项都有收的、不收的：{seen:?}");
        assert_eq!(seen.values().map(|(yes, no)| yes + no).sum::<usize>(), 429);
    }

    /// 各个上限的两头：正好到上限的收，多一点的不收。这些输入太大，没放进对照数据，这里现造；期望的结果是拿 Python 的那几个函数
    /// 对过的（造法一样）
    #[test]
    fn limits_at_the_boundaries() {
        let aid = |i: usize| format!("{i:032x}");
        let fp = |i: usize| format!("SHA256:{i:043}");
        let dev = |i: usize| format!("{i:016x}");
        let key = |i: u8| {
            let mut blob = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20".to_vec();
            blob.extend([0u8; 31]);
            blob.push(i);
            format!("ssh-ed25519 {}", base64::b64encode(&blob))
        };
        let tag = format!("{}=", "A".repeat(43));
        let blob = |c: &str| V::Dict(dict! { "n" => "A".repeat(24), "c" => c, "t" => &tag });
        let small = || blob("QUJD");
        let many = |n: usize, item: &dyn Fn(usize) -> (String, V)| V::Dict(Dict::from_pairs((0..n).map(item)));
        let empty = || V::Dict(Dict::new());
        let sync = |k: &str, v: V| parse_sync(&dict! { k => v }, MAX_KEY_BLOB);

        // 一次最多 MAX_ACCOUNTS 个账号
        assert_eq!(MAX_ACCOUNTS, 64);
        assert!(
            parse_units(&many(64, &|i| (aid(i), empty()))).is_some() && parse_units(&many(65, &|i| (aid(i), empty()))).is_none()
        );
        assert!(sync("put", many(64, &|i| (aid(i), small()))).is_some() && sync("put", many(65, &|i| (aid(i), small()))).is_none());
        assert!(
            sync("have", many(64, &|i| (aid(i), empty()))).is_some() && sync("have", many(65, &|i| (aid(i), empty()))).is_none()
        );
        let get =
            |n: usize, same: bool| sync("get", V::from((0..n).map(|i| V::from(aid(if same { 0 } else { i }))).collect::<Vec<V>>()));
        assert!(get(64, false).is_some() && get(65, false).is_none());
        // 重复的只留头一个，但个数是照来的算的
        assert_eq!((get(64, true).map(|s| s.get), get(65, true)), (Some(vec![aid(0)]), None));
        // 一个账号下最多 256 台设备的认证码
        let tags = |n: usize| sync("have", V::Dict(dict! { aid(0) => many(n, &|i| (fp(i), V::from(&tag))) }));
        assert_eq!((tags(256).map(|s| s.have.get(&aid(0)).map(OMap::len)), tags(257)), (Some(Some(256)), None));

        // 一份密文的长度：keys 是 MAX_KEY_BLOB，ledger 是 MAX_COST_BLOB，一台设备的配置是 MAX_TEAM_DOC
        assert_eq!((MAX_KEY_BLOB, MAX_COST_BLOB, MAX_TEAM_DOC, MAX_TEAM_SIG), (8192, 98304, 131_072, 1024));
        let sized = |n: usize, limit: usize| parse_blob(&blob(&"A".repeat(n)), limit).is_some();
        assert_eq!((sized(8192, MAX_KEY_BLOB), sized(8196, MAX_KEY_BLOB), sized(8196, MAX_COST_BLOB)), (true, false, true));
        assert_eq!((sized(98304, MAX_COST_BLOB), sized(98308, MAX_COST_BLOB)), (true, false));
        assert!(parse_blob(&blob(&format!("{}=", "A".repeat(8191))), MAX_KEY_BLOB).is_some());
        let put = |n: usize| sync("put", V::Dict(dict! { aid(1) => blob(&"A".repeat(n)) }));
        assert!(put(8192).is_some() && put(8196).is_none());
        let doc = |ct: V, sig: &str| V::Dict(dict! { "seq" => 1, "ct" => ct, "sig" => sig });
        let one_doc = |n: usize| parse_team_docs(&V::Dict(dict! { dev(0) => doc(blob(&"A".repeat(n)), "QUJD") }));
        assert!(one_doc(8196).is_some() && one_doc(131_072).is_some() && one_doc(131_076).is_none());
        let signed = |n: usize| parse_team_docs(&V::Dict(dict! { dev(0) => doc(small(), &"A".repeat(n)) }));
        assert!(signed(1024).is_some() && signed(1028).is_none());

        // team_enroll、team_put 一次最多 MAX_TEAM_BATCH 台设备
        assert_eq!(MAX_TEAM_BATCH, 64);
        let enroll =
            |n: usize| parse_enroll(&many(n, &|i| (dev(i), V::Dict(dict! { "pub" => key(i as u8), "name" => format!("n{i}") }))));
        assert_eq!((enroll(64).map(|d| d.len()), enroll(65).map(|d| d.len())), (Some(64), None));
        let docs = |n: usize| parse_team_docs(&many(n, &|i| (dev(i), doc(small(), "QUJD"))));
        assert_eq!((docs(64).map(|d| d.len()), docs(65).map(|d| d.len())), (Some(64), None));
        // 指纹是照公钥算的（和 ssh-keygen -l 的写法一样）；一把公钥只能是一台设备
        let one = parse_enroll(&V::Dict(dict! { dev(1) => dict! { "pub" => key(1), "name" => "n" } })).unwrap();
        let e = one.get(&dev(1)).unwrap();
        assert_eq!(
            (e.fp.as_str(), e.key.as_str(), e.name.as_str()),
            ("SHA256:gOPgXFWkBoXrFVwpjf0SBo3P1kFs+IvH5q/vnQhNpFw", key(1).as_str(), "n")
        );
        let twice =
            dict! { dev(1) => dict! { "pub" => key(1), "name" => "a" }, dev(2) => dict! { "pub" => key(1), "name" => "b" } };
        assert!(parse_enroll(&V::Dict(twice)).is_none());
    }
}
