//! 出口检测的解析（`coffee_parse`、`cmd_coffee`）：claude.ai（Codex 账号是 chatgpt.com）的 trace 加上 ip.net.coffee 的
//! /api/iprisk，变成给人看的一行和退出码。只看参数，不读写文件、不联网；每次启动都靠它决定放不放行。

use super::type_name;
use crate::cmd::arity;
use crate::core::base::RESTRICTED;
use crate::core::tool::t;
use crate::py::json;
use crate::py::obj::{repr_str, str_list, Dict, List, V};
use crate::py::text;
use crate::py::uni;
use crate::py::{exit, Fail, R};
use crate::{dict, out};
use std::cmp::Ordering;

/// Python 的整数（多大都行），只用来比大小：符号，和去掉前导 0 的十进制数字（0 写成 `0`，不带符号）
#[derive(Debug, PartialEq)]
struct Int {
    neg: bool,
    digits: String,
}

impl Int {
    fn of(i: i64) -> Int {
        Int { neg: i < 0, digits: i.unsigned_abs().to_string() }
    }

    fn cmp(&self, o: &Int) -> Ordering {
        match (self.neg, o.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (neg, _) => {
                let size = self.digits.len().cmp(&o.digits.len()).then_with(|| self.digits.cmp(&o.digits));
                if neg {
                    size.reverse()
                } else {
                    size
                }
            }
        }
    }
}

/// `int(s)`：前后的空白、正负号、数字之间的下划线、各种文字的十进制数字都认（和 `py::text::to_int` 认的写法一样），
/// 只是放不进 i64 的也算数：config.json 的 min_trust 写多大 Python 都照比。认不出是 ValueError（这里是 None）
fn py_int(s: &str) -> Option<Int> {
    let s = text::strip(s);
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut digits = String::new();
    let mut prev_digit = false;
    for c in body.chars() {
        if c == '_' {
            if !prev_digit {
                return None;
            }
            prev_digit = false;
            continue;
        }
        digits.push(char::from_digit(uni::decimal_value(c)?, 10)?);
        prev_digit = true;
    }
    // 空的、以下划线结尾的不认；超过 4300 位 Python 也不认（sys.int_max_str_digits 的默认值）
    if !prev_digit || digits.len() > 4300 {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    Some(if digits.is_empty() { Int::of(0) } else { Int { neg, digits: digits.to_string() } })
}

/// 评分（整数、小数、布尔值）向下取整以后和整数 m 比。m 是整数，所以 `score < m` 就是「取整后小于 m」，
/// `score >= m` 就是「取整后不小于 m」，不用把大整数换成小数（会丢精度）。NaN 和谁比都是假：None
fn cmp_score(score: &V, m: &Int) -> Option<Ordering> {
    let floor = match score {
        V::Bool(b) => Int::of(*b as i64),
        V::Int(i) => Int::of(*i),
        V::Big(digits) => py_int(digits)?,
        V::Float(f) if f.is_nan() => return None,
        V::Float(f) if f.is_infinite() => return Some(if *f > 0.0 { Ordering::Greater } else { Ordering::Less }),
        // 定点写法是这个小数精确的十进制展开，多大的数都对
        V::Float(f) => py_int(&format!("{:.0}", f.floor()))?,
        _ => return None,
    };
    Some(floor.cmp(m))
}

/// `score >= n`
fn score_ge(score: &V, n: i64) -> bool {
    matches!(cmp_score(score, &Int::of(n)), Some(Ordering::Greater | Ordering::Equal))
}

/// 解析 claude.ai（Codex 账号是 chatgpt.com）的 trace 与 ip.net.coffee /api/iprisk 结果 → dict：
/// rc（0 正常，1 纯净度偏低，2 受限地区，3 拿不到出口 IP，4 没有评分数据）、line（给人看的一行）、note（补充说明），
/// 以及 ip、country、org、trust、trust_label、tags、restricted_region
pub fn coffee_parse(min_trust: &str, trace: &str, risk: &str) -> R<Dict> {
    // kv = dict(l.split('=', 1) for l in trace.splitlines() if '=' in l)：键原样比（不去空白），同名的以后面的为准
    let (mut ip, mut loc) = ("", "");
    for l in text::splitlines(trace) {
        match l.split_once('=') {
            Some(("ip", v)) => ip = v,
            Some(("loc", v)) => loc = v,
            _ => {}
        }
    }
    let loc = loc.to_uppercase();
    let or_none = |s: &str| if s.is_empty() { V::None } else { V::str(s) };
    let r = dict! {
        "rc" => 3, "ip" => or_none(ip), "country" => or_none(&loc), "org" => V::None, "trust" => V::None,
        "trust_label" => V::None, "tags" => List::new(), "restricted_region" => V::None, "line" => "", "note" => "",
    };
    if ip.is_empty() {
        r.set("line", format!("拿不到 {} 看到的出口 IP（端口或节点不通）", t().trace_host));
        return Ok(r);
    }
    // 不是 JSON、不是对象都当作没有数据
    let d = if risk.is_empty() { Dict::new() } else { json::loads(risk).ok().and_then(|v| v.as_dict()).unwrap_or_default() };
    // cc = (d.get('countryCode') or loc or '').upper()
    let cc = match d.get("countryCode").filter(|v| v.truthy()) {
        Some(V::Str(s)) => s.to_uppercase(),
        Some(v) => return Err(Fail::Other("AttributeError", vec![format!("'{}' object has no attribute 'upper'", type_name(&v))])),
        None => loc.to_uppercase(),
    };
    // isinstance(…, (int, float))：布尔值也算数（原来就这样，True 当 1 分）
    let mut score = d.get("trust_score").filter(|v| v.isinstance_num());
    let is_true = |k: &str| matches!(d.get(k), Some(V::Bool(true)));
    let mut tags: Vec<&str> = Vec::new();
    if is_true("isResidential") {
        tags.push("住宅IP");
    } else if is_true("is_datacenter") {
        tags.push("机房IP");
    }
    for (f, name) in [("is_vpn", "VPN"), ("is_proxy", "代理"), ("is_tor", "Tor"), ("is_abuser", "有滥用记录")] {
        if is_true(f) {
            tags.push(name);
        }
    }
    let restricted = RESTRICTED.iter().find(|(code, _)| *code == cc).map(|(_, name)| *name);
    if restricted.is_some() {
        score = Some(V::Int(0));
    }
    let (label, level) = match &score {
        None => (None, "纯净度 无数据".to_string()),
        Some(s) => {
            let label = if score_ge(s, 95) {
                "极度纯净"
            } else if score_ge(s, 80) {
                "纯净"
            } else if score_ge(s, 50) {
                "良好"
            } else if score_ge(s, 25) {
                "中性"
            } else {
                "可疑"
            };
            (Some(label), format!("纯净度 {}（{}）", s.py_str(), label))
        }
    };
    // org = d.get('asOrganization') or d.get('company_name') or ''
    let org = d.get("asOrganization").filter(|v| v.truthy()).or_else(|| d.get("company_name").filter(|v| v.truthy()));
    // line = ' · '.join(x for x in [ip, cc or '?', org, level] + tags if x)：org 不是文字时 join 是没人接的 TypeError
    let mut line = vec![ip.to_string(), if cc.is_empty() { "?".to_string() } else { cc.clone() }];
    match &org {
        Some(V::Str(s)) => line.push(s.to_string()),
        Some(v) => panic!("TypeError: sequence item 2: expected str instance, {} found", type_name(v)),
        None => {}
    }
    line.push(level);
    line.extend(tags.iter().map(|x| x.to_string()));
    r.set("country", or_none(&cc));
    r.set("org", org.filter(|o| o.is_str()));
    r.set("trust", score.clone());
    r.set("trust_label", label);
    r.set("tags", str_list(&tags));
    r.set("rc", 0);
    r.set("line", line.join(" · "));
    if let Some(region) = restricted {
        r.set("rc", 2);
        r.set("restricted_region", region);
        r.set("note", format!("出口在{}：{} 不可用地区，会被严格风控", region, t().region_name));
    } else if let Some(s) = &score {
        // 走到这里才看 min_trust：受限地区、没有评分时写错了也不报
        let min =
            py_int(min_trust).ok_or_else(|| Fail::Value(format!("invalid literal for int() with base 10: {}", repr_str(min_trust))))?;
        if cmp_score(s, &min) == Some(Ordering::Less) {
            r.set("rc", 1);
            r.set("note", format!("纯净度低于设定的 {min_trust}"));
        }
    } else {
        r.set("rc", 4);
        r.set("note", "ip.net.coffee 没有返回评分（检测服务可能暂时不可用）");
    }
    Ok(r)
}

/// 出口检测给 bash 用：第一行是 line，有补充说明再一行；退出码是 rc
pub fn cmd_coffee(args: &[String]) -> R<()> {
    arity("cmd_coffee", args, 3, 3)?;
    let r = coffee_parse(&args[0], &args[1], &args[2])?;
    out!("{}", r.at("line").py_str());
    if r.truthy("note") {
        out!("{}", r.at("note").py_str());
    }
    match r.at("rc").as_int() {
        Some(rc) if rc != 0 => exit(rc as i32),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::json::{dumps, Fmt};

    const TRACE: &str = "fl=1\nip=203.0.113.1\nloc=US\n";

    fn parse(min: &str, trace: &str, risk: &str) -> String {
        dumps(&V::Dict(coffee_parse(min, trace, risk).unwrap()), Fmt::new().utf8())
    }

    #[test]
    fn dict_shape_like_python() {
        // 右边是 Python 的 json.dumps(coffee_parse(…), ensure_ascii=False)
        assert_eq!(
            parse("50", "", ""),
            r#"{"rc": 3, "ip": null, "country": null, "org": null, "trust": null, "trust_label": null, "tags": [], "restricted_region": null, "line": "拿不到 claude.ai 看到的出口 IP（端口或节点不通）", "note": ""}"#
        );
        assert_eq!(
            parse("50", TRACE, r#"{"isResidential": true, "is_vpn": true, "asOrganization": "Example Networks", "countryCode": "us", "trust_score": 97}"#),
            r#"{"rc": 0, "ip": "203.0.113.1", "country": "US", "org": "Example Networks", "trust": 97, "trust_label": "极度纯净", "tags": ["住宅IP", "VPN"], "restricted_region": null, "line": "203.0.113.1 · US · Example Networks · 纯净度 97（极度纯净） · 住宅IP · VPN", "note": ""}"#
        );
        assert_eq!(
            parse("50", TRACE, r#"{"countryCode": "hk", "trust_score": 88.5, "company_name": "HK Net"}"#),
            r#"{"rc": 2, "ip": "203.0.113.1", "country": "HK", "org": "HK Net", "trust": 0, "trust_label": "可疑", "tags": [], "restricted_region": "香港", "line": "203.0.113.1 · HK · HK Net · 纯净度 0（可疑）", "note": "出口在香港：Claude 不可用地区，会被严格风控"}"#
        );
        assert_eq!(
            parse("50", TRACE, "not json"),
            r#"{"rc": 4, "ip": "203.0.113.1", "country": "US", "org": null, "trust": null, "trust_label": null, "tags": [], "restricted_region": null, "line": "203.0.113.1 · US · 纯净度 无数据", "note": "ip.net.coffee 没有返回评分（检测服务可能暂时不可用）"}"#
        );
        assert_eq!(
            parse("50", TRACE, r#"{"trust_score": 49.9, "is_datacenter": true}"#),
            r#"{"rc": 1, "ip": "203.0.113.1", "country": "US", "org": null, "trust": 49.9, "trust_label": "中性", "tags": ["机房IP"], "restricted_region": null, "line": "203.0.113.1 · US · 纯净度 49.9（中性） · 机房IP", "note": "纯净度低于设定的 50"}"#
        );
    }

    #[test]
    fn int_like_python() {
        let n = |s: &str| py_int(s).map(|i| format!("{}{}", if i.neg { "-" } else { "" }, i.digits));
        assert_eq!(n(" 50 "), Some("50".into()));
        assert_eq!(n("+0_7"), Some("7".into()));
        assert_eq!(n("-0"), Some("0".into()));
        assert_eq!(n("-00012"), Some("-12".into()));
        assert_eq!(n("５０"), Some("50".into()));
        assert_eq!(n("99999999999999999999999"), Some("99999999999999999999999".into()));
        for bad in ["", " ", "_1", "1_", "1__0", "5.0", "0x10", "- 1", "+", "abc", "None", "True"] {
            assert_eq!(n(bad), None, "{bad:?}");
        }
        assert!(py_int(&"9".repeat(4300)).is_some() && py_int(&"9".repeat(4301)).is_none());
    }

    #[test]
    fn score_comparisons_like_python() {
        let lt = |score: V, m: &str| cmp_score(&score, &py_int(m).unwrap()) == Some(Ordering::Less);
        // score < int(min_trust)
        assert!(lt(V::Int(49), "50") && !lt(V::Int(50), "50") && lt(V::Float(49.999), "50") && !lt(V::Float(50.0), "50"));
        assert!(lt(V::Bool(true), "2") && !lt(V::Bool(true), "1") && lt(V::Bool(false), "1"));
        assert!(lt(V::Int(-5), "-4") && !lt(V::Int(-4), "-4") && lt(V::Float(-4.5), "-4") && !lt(V::Float(-3.5), "-4"));
        assert!(!lt(V::Float(-0.0), "0") && lt(V::Float(-0.1), "0"));
        assert!(!lt(V::Float(f64::NAN), "50") && !lt(V::Float(f64::INFINITY), "50") && lt(V::Float(f64::NEG_INFINITY), "-99999999999999999999"));
        // 放不进 i64 的也照比
        assert!(lt(V::Int(i64::MAX), "9223372036854775808") && !lt(V::Big("9223372036854775808".into()), "9223372036854775808"));
        assert!(lt(V::Big("-9223372036854775809".into()), "-9223372036854775808") && !lt(V::Big("12345678901234567890".into()), "50"));
        assert!(lt(V::Float(1e30), "1000000000000000019884624838657") && !lt(V::Float(1e30), "1000000000000000019884624838656"));
        assert!(score_ge(&V::Float(95.0), 95) && !score_ge(&V::Float(94.99), 95) && !score_ge(&V::Float(f64::NAN), 0));
        assert!(score_ge(&V::Big("12345678901234567890".into()), 95) && !score_ge(&V::Big("-12345678901234567890".into()), 25));
    }
}
