//! 核对一封邮件是不是这个账号这一次登录的登录邮件，再从正文里取出登录链接（`mail_tags` 到 `mail_digest`）。
//!
//! 只信两样：Gmail 收信时写在最上面的那条 Authentication-Results（`mail_ar`），和它验过的那个 DKIM 签名盖住的头
//! （`mail_signed_for`）。签名之外的头谁都能改、能加，所以发件人、收件人、标题必须都在签名里、各只有一个。
//! 这一块是安全上要紧的逻辑：每个条件都照原文，tests/mail_view.py 里的检查都搬过来了（对照数据在
//! tests/vectors/mail.json 的 judge 等几节）。

use super::header::{mail_addrs, mail_text, parseaddr};
use super::html;
use super::message::{decode_text, hstr, Message};
use super::{MailRule, MAIL_ADDR_RE, MAIL_DOMAIN_RE, MAIL_ORDER, MAIL_SIGNED, MAIL_SKEW};
use crate::py::hash::sha256_hex;
use crate::py::url::urlsplit;
use crate::py::{text, uni};
use crate::py::R;
use crate::re;

/// DKIM-Signature 的 tag=value（值去掉空白）：名字小写，同名的只留第一个
pub fn mail_tags(v: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for part in v.split(';') {
        let (k, val) = part.split_once('=').unwrap_or((part, ""));
        let k = text::strip(k).to_lowercase();
        if !k.is_empty() && !out.iter().any(|(x, _)| *x == k) {
            out.push((k, val.chars().filter(|c| !uni::is_space(*c)).collect()));
        }
    }
    out
}

fn tag<'a>(tags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Authentication-Results 的值按 RFC 8601 切开：(authserv-id, [[(名字, 值), …], …])，每项结果的第一对是 method=result，
/// 后面是 ptype.property=value（名字小写）。注释（可以嵌套、带反斜杠转义）去掉；引号里的整段算值的一部分，里面的分号、等号、
/// 括号都不算（smtp.mailfrom 的本地部分是发件人写的，Gmail 照抄）。写法不对（引号、括号不配对，名字=值对不上）是 None：整条不信
pub fn mail_ar_items(v: &str) -> Option<(String, Vec<Vec<(String, String)>>)> {
    let v: Vec<char> = v.chars().collect();
    let mut segs: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut tok = String::new();
    let mut depth = 0i64;
    let mut i = 0;
    while i < v.len() {
        let ch = v[i];
        if depth != 0 {
            // 注释里
            match ch {
                '\\' => i += 1,
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        } else if ch == '"' {
            let mut j = i + 1;
            while j < v.len() && v[j] != '"' {
                j += if v[j] == '\\' { 2 } else { 1 };
            }
            if j >= v.len() {
                return None;
            }
            tok.extend(&v[i..=j]);
            i = j;
        } else if ch == '(' || ch == ';' || ch == '=' || uni::is_space(ch) {
            if !tok.is_empty() {
                cur.push(std::mem::take(&mut tok));
            }
            match ch {
                '(' => depth = 1,
                '=' => cur.push("=".to_string()),
                ';' => segs.push(std::mem::take(&mut cur)),
                _ => {}
            }
        } else {
            tok.push(ch);
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    if !tok.is_empty() {
        cur.push(tok);
    }
    segs.push(cur);
    if segs[0].is_empty() || segs[0].iter().any(|t| t == "=") {
        return None;
    }
    let mut out = Vec::new();
    for seg in &segs[1..] {
        if seg.is_empty() || (seg.len() == 1 && seg[0] == "none") {
            continue;
        }
        if seg.len() % 3 != 0 || seg.chunks(3).any(|t| t[1] != "=" || t[0] == "=" || t[2] == "=") {
            return None;
        }
        out.push(seg.chunks(3).map(|t| (t[0].to_lowercase(), t[2].clone())).collect());
    }
    Some((segs[0][0].to_lowercase(), out))
}

/// 最上面那一条 Authentication-Results 里的 DKIM 结果 [(结果, 域名, header.b)]（Gmail 收信时把它加在最上面，邮件里原有的、
/// 发件人写的都在它下面）：它不是 mx.google.com 写的、写法不对时是 None。域名取 header.d，没有时取 header.i 的 @ 后面；
/// 名字重复、域名或 header.b 的样子不对（header.b 至少 8 个 base64 字符）的那一项记成 ?，不算通过
pub fn mail_ar(msg: &Message) -> Option<Vec<(String, String, String)>> {
    let ars = msg.get_all("Authentication-Results");
    let first = ars.first()?;
    let (authserv, items) = mail_ar_items(&text::split_ws(&hstr(first)).join(" "))?;
    if authserv != "mx.google.com" {
        return None;
    }
    let mut out = Vec::new();
    for pairs in &items {
        if pairs[0].0 != "dkim" {
            continue;
        }
        // dict(pairs[1:])：同名的后面的盖掉前面的
        let mut p: Vec<(&str, &str)> = Vec::new();
        for (k, v) in &pairs[1..] {
            match p.iter_mut().find(|(x, _)| x == k) {
                Some(slot) => slot.1 = v,
                None => p.push((k, v)),
            }
        }
        let get = |name: &str| p.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
        let from_i = re!(r"^[A-Za-z0-9._+-]*@(.+)$").match_(get("header.i").unwrap_or("")).map(|m| m.g(1).to_string());
        let d = get("header.d").filter(|d| !d.is_empty()).map(str::to_string).or(from_i).unwrap_or_default().to_lowercase();
        let b = get("header.b").unwrap_or("");
        let ok = p.len() == pairs.len() - 1 && MAIL_DOMAIN_RE.matches(&d) && re!(r"^[A-Za-z0-9+/]{8,}$").matches(b);
        out.push(if ok { (pairs[0].1.to_lowercase(), d, b.to_string()) } else { ("?".to_string(), String::new(), String::new()) });
    }
    Some(out)
}

/// 域名 d 是不是信任的中转 forwarder（按标签认后缀，和 domain_ok 一样）
pub fn mail_fwd_ok(d: &str, forwarder: &str) -> bool {
    let (d, f) = (d.to_lowercase(), forwarder.to_lowercase());
    !f.is_empty() && (d == f || d.ends_with(&format!(".{f}")))
}

/// From 的显示名里的邮箱（信任中转把原始别名放在显示名里，像 "alias@x" <noreply@fwd>，用来认是哪个账号）；没有是空字符串
pub fn mail_from_alias(msg: &Message) -> R<String> {
    let name = text::strip(&parseaddr(&msg.get_str("From"))?.0).to_lowercase();
    Ok(if MAIL_ADDR_RE.matches(&name) { name } else { String::new() })
}

/// Gmail 验过的、dom_ok(域名) 为真的那个 DKIM 签名覆盖了哪些头（小写）；没有是 None。按 header.b（签名的开头几个字符）
/// 认是哪个签名；对上好几个时分不清，不认
pub fn mail_signed_for(msg: &Message, dom_ok: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    for (res, dom, b) in mail_ar(msg).unwrap_or_default() {
        if res != "pass" || !dom_ok(&dom) {
            continue;
        }
        let sigs: Vec<Vec<(String, String)>> = msg
            .get_all("DKIM-Signature")
            .into_iter()
            .map(|v| mail_tags(&hstr(v)))
            .filter(|tg| {
                let d = tag(tg, "d").unwrap_or("");
                let dl = d.to_lowercase();
                dom_ok(d) && (dom == dl || dom.ends_with(&format!(".{dl}"))) && tag(tg, "b").unwrap_or("").starts_with(b.as_str())
            })
            .collect();
        if sigs.len() == 1 {
            let h = tag(&sigs[0], "h").unwrap_or("");
            return Some(h.split(':').map(|h| text::strip(h).to_lowercase()).filter(|h| !h.is_empty()).collect());
        }
    }
    None
}

pub fn mail_signed(msg: &Message, rule: &MailRule) -> Option<Vec<String>> {
    mail_signed_for(msg, &|d| rule.domain_ok(d))
}

fn domain_of(addr: &str) -> &str {
    addr.rsplit_once('@').map_or(addr, |x| x.1)
}

/// 这封邮件是不是 acct（小写的邮箱）这一次登录的登录邮件：是返回 None，不是返回原因（MAIL_WHY 的键）。got 是 Gmail 收到它的时间。
/// forwarder 给定时（这个账号开了「信任转发中转」，默认不开）：From 是 Anthropic 时照严格路走，From 是这个中转时走 mail_judge_fwd
pub fn mail_judge(rule: &MailRule, msg: &Message, got: f64, acct: &str, since: f64, forwarder: Option<&str>) -> R<Option<&'static str>> {
    let _frame = super::py_frame();
    if got < since - MAIL_SKEW {
        return Ok(Some("old"));
    }
    if !rule.subject_ok(&mail_text(msg.get("Subject"))) {
        return Ok(Some("subject"));
    }
    if MAIL_SIGNED.iter().any(|h| msg.get_all(h).len() > 1) {
        return Ok(Some("dup"));
    }
    let frm = mail_addrs(msg, "From")?;
    if frm.len() != 1 {
        return Ok(Some("from"));
    }
    let dom = domain_of(&frm[0]);
    if rule.domain_ok(dom) {
        let Some(signed) = mail_signed(msg, rule) else {
            return Ok(Some("dkim"));
        };
        if !MAIL_SIGNED.iter().all(|h| signed.iter().any(|s| s == h)) {
            return Ok(Some("unsigned"));
        }
        if acct.is_empty() || !mail_addrs(msg, "To")?.iter().any(|a| a == acct) {
            return Ok(Some("to"));
        }
        return Ok(None);
    }
    if let Some(f) = forwarder.filter(|f| !f.is_empty() && mail_fwd_ok(dom, f)) {
        return mail_judge_fwd(rule, msg, acct, f);
    }
    Ok(Some("from"))
}

/// 信任的中转 forwarder 转发的登录邮件（账号显式开了「信任转发中转」，不安全、默认关）：中转没有 Anthropic 的直签了，所以靠
/// 「中转的 DKIM 覆盖 From/To/Subject/Reply-To（中转之后改不了这几项）+ From 显示名是这个账号的别名（路由）+ Reply-To 是单个
/// Anthropic 地址（中转标注原件来自 Anthropic）」。安全边界只到「信任这个中转 + 别名不泄露」：别名泄露，知道的人可经中转注入登录链接。
/// 返回 None 放行，否则原因（MAIL_WHY 的键）
pub fn mail_judge_fwd(rule: &MailRule, msg: &Message, acct: &str, forwarder: &str) -> R<Option<&'static str>> {
    let _frame = super::py_frame();
    let signed = mail_signed_for(msg, &|d| mail_fwd_ok(d, forwarder));
    let covered = signed.is_some_and(|s| ["from", "to", "subject", "reply-to"].iter().all(|h| s.iter().any(|x| x == h)));
    if !covered {
        return Ok(Some("dkim"));
    }
    if acct.is_empty() || mail_from_alias(msg)? != acct {
        return Ok(Some("to"));
    }
    let rt = mail_addrs(msg, "Reply-To")?;
    if rt.len() != 1 || !rule.domain_ok(domain_of(&rt[0])) {
        return Ok(Some("from"));
    }
    Ok(None)
}

/// (过了的那条规则, None)；都没过时 (核对得最远的那条, 原因)
pub fn mail_judge_any(
    rules: &[&'static MailRule],
    msg: &Message,
    got: f64,
    acct: &str,
    since: f64,
    forwarder: Option<&str>,
) -> R<(Option<&'static MailRule>, Option<&'static str>)> {
    let _frame = super::py_frame();
    let order = |why: &str| MAIL_ORDER.iter().position(|w| *w == why).unwrap_or(0);
    let mut best: (Option<&'static MailRule>, &'static str) = (rules.first().copied(), "subject");
    for &r in rules {
        match mail_judge(r, msg, got, acct, since, forwarder)? {
            None => return Ok((Some(r), None)),
            Some(why) => {
                if order(why) > order(best.1) {
                    best = (Some(r), why);
                }
            }
        }
    }
    Ok((best.0, Some(best.1)))
}

/// 正文里的链接（HTML 的 <a href>、纯文本里的网址），按出现的先后；附件里的不算
pub fn mail_links(msg: &Message) -> R<Vec<String>> {
    let mut links: Vec<String> = Vec::new();
    for part in msg.walk() {
        if part.is_multipart() || part.get_content_disposition().as_deref() == Some("attachment") {
            continue;
        }
        let ct = part.get_content_type();
        if ct != "text/html" && ct != "text/plain" {
            continue;
        }
        let raw = part.get_payload_decoded().unwrap_or_default();
        let body = decode_text(part, &raw)?;
        if ct == "text/html" {
            links.extend(html::hrefs(&body));
        } else {
            links.extend(re!(r"https://[^\s<>\x22\x27\\]+").findall(&body).into_iter().map(str::to_string));
        }
    }
    Ok(links.iter().map(|l| text::strip(l).to_string()).collect())
}

pub fn mail_extract(rule: &MailRule, msg: &Message) -> R<Option<String>> {
    Ok(mail_links(msg)?.into_iter().find(|u| rule.link_ok(u)))
}

/// 链接的样子（给 cca mail test 看）：协议、主机和路径，后面的部分只说有多长，不打出来
pub fn mail_shape(url: &str) -> String {
    let Ok(u) = urlsplit(url) else {
        return format!("（认不出的链接，{} 个字符）", text::len(url));
    };
    let rest: Vec<String> =
        [("?", &u.query), ("#", &u.fragment)].iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{} 后面 {} 个字符", k, text::len(v))).collect();
    let tail = if rest.is_empty() { String::new() } else { format!("（{}，不显示）", rest.join("、")) };
    format!("{}://{}{}{}", u.scheme, u.netloc, u.path, tail)
}

/// 打开过的登录邮件记的摘要：Message-ID（没有就用邮件夹和 UID）加 Gmail 收到它的时间
pub fn mail_digest(msg: &Message, folder: &str, uid: u64, got: f64) -> String {
    let mid = text::split_ws(&msg.get_str("Message-ID")).join(" ");
    let mid = if mid.is_empty() { format!("{folder}|{uid}") } else { mid };
    text::head(&sha256_hex(format!("{}\n{}", mid, got.trunc() as i64)), 16).to_string()
}

/// `urlsplit_host(url)`：链接的主机（小写），认不出是空串
pub fn urlsplit_host(url: &str) -> String {
    urlsplit(url).ok().and_then(|u| u.hostname()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::mail::message::mail_parse;
    use crate::cmd::mail::testing::{bytes_of, vectors};
    use crate::cmd::mail::MAIL_RULES;
    use crate::py::obj::V;
    use crate::py::Fail;

    fn rule() -> &'static MailRule {
        &MAIL_RULES[0]
    }

    fn opt(v: Option<V>) -> Option<String> {
        v.filter(|v| !v.is_none()).map(|v| v.py_str())
    }

    #[test]
    fn tags_like_python() {
        let cases = vectors("tags");
        assert!(cases.len() > 30);
        for c in cases {
            let got = V::from(mail_tags(&c.at("in").py_str()).into_iter().map(|(k, v)| V::from(crate::list![k, v])).collect::<Vec<_>>());
            assert!(got.py_eq(&c.at("out")), "{:?}: {got:?} / {:?}", c.at("in"), c.at("out"));
        }
    }

    #[test]
    fn ar_items_like_python() {
        let cases = vectors("ar_items");
        assert!(cases.len() > 150);
        for c in cases {
            let input = c.at("in").py_str();
            let got = match mail_ar_items(&input) {
                None => V::None,
                Some((id, items)) => V::from(crate::list![
                    id,
                    V::from(
                        items
                            .into_iter()
                            .map(|pairs| V::from(pairs.into_iter().map(|(k, v)| V::from(crate::list![k, v])).collect::<Vec<_>>()))
                            .collect::<Vec<_>>()
                    )
                ]),
            };
            assert!(got.py_eq(&c.at("out")), "{input:?}\n  这里：{got:?}\n  Python：{:?}", c.at("out"));
        }
    }

    #[test]
    fn rules_like_python() {
        let cases = vectors("subject");
        assert!(cases.len() > 300);
        for c in cases {
            let s = c.at("in").py_str();
            assert_eq!(rule().subject_ok(&s), c.at("ok").truthy(), "标题 {s:?}");
        }
        let cases = vectors("link");
        assert!(cases.len() > 70);
        for c in cases {
            let u = c.at("in").py_str();
            assert_eq!(rule().link_ok(&u), c.at("ok").truthy(), "链接 {u:?}");
            assert_eq!(mail_shape(&u), c.at("shape").py_str(), "链接 {u:?}");
            assert_eq!(urlsplit_host(&u), c.at("host").py_str(), "链接 {u:?}");
        }
        let cases = vectors("domain");
        assert!(cases.len() > 15);
        for c in cases {
            let d = c.at("in").py_str();
            assert_eq!(rule().domain_ok(&d), c.at("ok").truthy(), "域名 {d:?}");
            assert_eq!(mail_fwd_ok(&d, "relay.example.com"), c.at("fwd").truthy(), "中转 {d:?}");
        }
    }

    /// 一封邮件上的各个判断：Gmail 的验证结果、签名盖住的头、地址、核对的结论、链接、摘要
    #[test]
    fn judgement_like_python() {
        let cases = vectors("judge");
        assert!(cases.len() > 120);
        for c in cases {
            let raw = bytes_of(&c.at("raw"));
            let what = c.at("name").py_str();
            let msg = match mail_parse(&raw) {
                Ok(m) => m,
                Err(e) => {
                    assert_eq!(c.get("error").map(|e| e.py_str()), Some(class_of(&e).to_string()), "{what}");
                    continue;
                }
            };
            let ar = match mail_ar(&msg) {
                None => V::None,
                Some(items) => V::from(items.into_iter().map(|(r, d, b)| V::from(crate::list![r, d, b])).collect::<Vec<_>>()),
            };
            assert!(ar.py_eq(&c.at("ar")), "{what}：验证结果 {ar:?} / {:?}", c.at("ar"));
            let mut signed = mail_signed(&msg, rule());
            if let Some(s) = signed.as_mut() {
                s.sort();
                s.dedup();
            }
            let want_signed: Option<Vec<String>> = c.at("signed").as_list().map(|l| l.strs().iter().map(|s| s.to_string()).collect());
            assert_eq!(signed, want_signed, "{what}：签名盖住的头");
            assert_eq!(mail_text(msg.get("Subject")), c.at("subject").py_str(), "{what}：标题");
            for (key, name) in [("from", "From"), ("to", "To"), ("reply_to", "Reply-To")] {
                let got = mail_addrs(&msg, name).map_err(|e| class_of(&e));
                let want: Result<Vec<String>, &str> = match c.at(key) {
                    V::Str(e) => Err(if &*e == "RecursionError" { "RecursionError" } else { "?" }),
                    v => Ok(v.as_list().unwrap().strs().iter().map(|s| s.to_string()).collect()),
                };
                assert_eq!(got, want, "{what}：{name}");
            }
            assert_eq!(mail_from_alias(&msg).unwrap_or_default(), c.at("alias").py_str(), "{what}：显示名里的别名");
            for j in c.at("judge").as_list().unwrap().items() {
                let j = j.as_dict().unwrap();
                let fwd = opt(j.get("forwarder"));
                let (got, since) = (j.at("got").as_num().unwrap(), j.at("since").as_num().unwrap());
                let why = mail_judge(rule(), &msg, got, &j.at("acct").py_str(), since, fwd.as_deref());
                let want = opt(j.get("why"));
                match &why {
                    Ok(w) => assert_eq!(w.map(str::to_string), want, "{what}：{j:?}"),
                    Err(e) => assert_eq!(Some(format!("!{}", class_of(e))), want, "{what}：{j:?}"),
                }
                // 只有一条规则时 mail_judge_any 说的原因：比「标题不像」核对得更远才换（old 报成 subject）
                match mail_judge_any(&[rule()], &msg, got, &j.at("acct").py_str(), since, fwd.as_deref()) {
                    Ok((r, w)) => assert_eq!((r.is_some(), w.map(str::to_string)), (true, opt(j.get("any"))), "{what}：mail_judge_any {j:?}"),
                    Err(e) => assert_eq!(Some(format!("!{}", class_of(&e))), opt(j.get("any")), "{what}：mail_judge_any {j:?}"),
                }
                let none = mail_judge_any(&[], &msg, got, &j.at("acct").py_str(), since, fwd.as_deref()).unwrap();
                assert!(none.0.is_none() && none.1 == Some("subject"), "{what}：没有规则时");
            }
            // 正文的字符集 Python 认识、这里不认识（GBK、UTF-16……）时这里照 UTF-8 解，链接可能不一样：
            // 对照数据里另记了一份照这样解的结果（links_unknown），有意不一样的那一处
            let (want_links, want_extract) = match c.get("links_unknown") {
                Some(alt) => (alt, c.get("extract_unknown")),
                None => (c.at("links"), c.get("extract")),
            };
            match (mail_links(&msg), want_links) {
                (Err(e), V::Str(want)) => assert_eq!(format!("!{}", class_of(&e)), &*want, "{what}：链接"),
                (Ok(links), want) => {
                    let want: Vec<String> = want.as_list().unwrap_or_else(|| panic!("{what}：Python 出了 {want:?}")).strs().iter().map(|s| s.to_string()).collect();
                    assert_eq!(links, want, "{what}：链接");
                    assert_eq!(mail_extract(rule(), &msg).unwrap(), opt(want_extract), "{what}：登录链接");
                }
                (Err(e), want) => panic!("{what}：这里出了 {e:?}，Python 是 {want:?}"),
            }
            assert_eq!(mail_digest(&msg, "[Gmail]/All Mail", 7, 1794708000.0), c.at("digest").py_str(), "{what}：摘要");
        }
    }

    fn class_of(e: &Fail) -> &'static str {
        match e {
            Fail::Value(_) => "ValueError",
            Fail::Other(name, _) => name,
            _ => "?",
        }
    }

    /// tests/mail_view.py 的 Judge、Links、Clean 里的每一条检查，结论照那边写的（邮件是对照数据 judge 一节里同名的那一封：
    /// 生成对照数据时就是照那些测试里的写法做的）。上面是和真的 Python 逐项比，这里是把「该是什么」再钉一遍
    #[test]
    fn every_check_in_mail_view() {
        let mails = vectors("judge");
        let msg = |name: &str| {
            let c = mails.iter().find(|c| c.at("name").py_str() == name).unwrap_or_else(|| panic!("对照数据里没有 {name}"));
            mail_parse(&bytes_of(&c.at("raw"))).unwrap()
        };
        let now = 1794708000.0;
        let alice = "alice@example.com";
        let judge = |name: &str, acct: &str, got: f64, fwd: Option<&str>| mail_judge(rule(), &msg(name), got, acct, now - 60.0, fwd).unwrap();
        // Judge.test_good_and_each_failure
        assert_eq!(judge("good", alice, now, None), None);
        assert_eq!(judge("good", alice, now - 200.0, None), Some("old"));
        assert_eq!(judge("good", alice, now - 100.0, None), None, "登录开始前一分钟内收到的也算");
        for (name, why) in [
            ("subject receipt", "subject"),
            ("dup from", "dup"),
            ("dup to", "dup"),
            ("from evil", "from"),
            ("from two", "from"),
            ("ar fail", "dkim"),
            ("ar none", "dkim"),
            ("ar missing", "dkim"),
            ("dkim other domain", "dkim"),
            ("unsigned to", "unsigned"),
            ("to bob", "to"),
        ] {
            assert_eq!(judge(name, alice, now, None), Some(why), "{name}");
        }
        assert_eq!(judge("good", "", now, None), Some("to"));
        // Judge.test_only_trusts_the_topmost_gmail_result：最上面那条说没过、不是 mx.google.com 写的、pass 只在注释里
        for name in ["forged below", "other server on top", "comment pass"] {
            assert_eq!(judge(name, alice, now, None), Some("dkim"), "{name}");
        }
        // Judge.test_only_what_gmail_itself_wrote_counts
        assert_eq!(judge("ar control", alice, now, None), None, "对照");
        assert_eq!(judge("ar nested comment", alice, now, None), None, "注释可以嵌套、带转义");
        for i in 0..8 {
            assert_eq!(judge(&format!("ar not gmail-written {i}"), alice, now, None), Some("dkim"), "第 {i} 种");
        }
        // Judge.test_trusted_forwarder
        let relay = Some("relay.example.com");
        assert_eq!(judge("fwd good", alice, now, relay), None, "正常转发 + 登记了中转 → 放行");
        assert_eq!(judge("fwd sub relay", alice, now, relay), None, "中转按标签认后缀");
        assert_eq!(judge("fwd good", alice, now, None), Some("from"), "默认不开 → 拒");
        assert_eq!(judge("fwd good", alice, now, Some("evil.com")), Some("from"), "登记的是别的域名 → 拒");
        for (name, why) in [
            ("fwd reply-to evil", "from"),
            ("fwd reply-to missing", "from"),
            ("fwd alias bob", "to"),
            ("fwd no alias", "to"),
            ("fwd unsigned reply-to", "dkim"),
            ("fwd ar fail", "dkim"),
            ("fwd signed by evil", "dkim"),
            ("fwd subject portal", "subject"),
        ] {
            assert_eq!(judge(name, alice, now, relay), Some(why), "{name}");
        }
        assert_eq!(judge("good", alice, now, relay), None, "严格直签 + 开了中转 → 仍放行");
        assert_eq!(judge("fwd good", "bob@example.com", now, relay), Some("to"), "发给 alice 的转发邮件对 bob 不算");
        // Judge.test_signature_matched_by_header_b
        assert_eq!(judge("twin signature", alice, now, None), Some("dkim"), "两个签名的 b 开头一样：分不清，不认");
        assert_eq!(judge("other signature", alice, now, None), None, "b 开头不一样的另一个签名不影响");
        // Judge.test_judge_any_reports_the_furthest_reason
        let (r, why) = mail_judge_any(&[rule()], &msg("to bob"), now, alice, 0.0, None).unwrap();
        assert!(r.is_some_and(|r| std::ptr::eq(r, rule())) && why == Some("to"));
        // Links
        assert_eq!(mail_extract(rule(), &msg("body two links")).unwrap().as_deref(), Some("https://claude.ai/magic-link#tok-X"));
        assert_eq!(mail_extract(rule(), &msg("body text only")).unwrap().as_deref(), Some("https://claude.ai/magic-link#tok-T"));
        assert_eq!(mail_extract(rule(), &msg("body tracking link")).unwrap(), None);
        assert_eq!(mail_extract(rule(), &msg("attachment ignored")).unwrap(), None, "附件里的不算");
        let shape = mail_shape("https://claude.ai/magic-link?a=1#tok-secret");
        assert!(!shape.contains("secret") && shape.contains("https://claude.ai/magic-link"), "{shape}");
        // Clean：别人写的文字不能带着转义序列打到终端上
        let hostile = "\u{1b}[2K\u{1b}[1A✓ 通过核对\u{9b}31m\u{202e}\u{7}";
        let encoded = format!("=?utf-8?b?{}?=", crate::py::base64::b64encode(hostile.as_bytes()));
        let shown = super::super::header::mail_text_str(&encoded);
        assert!(shown.contains("通过核对"), "{shown:?}");
        let control = |s: &str| s.chars().any(|c| matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f | 0x202e));
        assert!(!control(&shown), "{shown:?}");
        assert!(!control(&crate::cmd::mail::mail_oserr("a\u{1b}]52;c;ZXZpbA==\u{7}b\u{9d}", "Exception")));
    }
}
