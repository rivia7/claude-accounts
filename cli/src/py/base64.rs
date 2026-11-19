//! `base64` / `binascii.a2b_base64`：和 CPython 一样的宽严。不严格时不认识的字符丢掉、凑够一组填充就不再往后看；
//! 报错的那几句话也一样（调用方多半只接 ValueError，但有的会把原因显示出来）。照 CPython 3.14 核对过
//! （tests/vectors/base64.json，tools/gen_vectors.py 生成）。

use super::fail::{Fail, R};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.b64encode(data).decode('ascii')`
pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        out.push(ALPHABET[(b[0] >> 2) as usize] as char);
        out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(b[2] & 0x3f) as usize] as char } else { '=' });
    }
    out
}

/// `base64.urlsafe_b64encode(data).decode('ascii')`
#[cfg(test)]
pub fn urlsafe_b64encode(data: &[u8]) -> String {
    b64encode(data).replace('+', "-").replace('/', "_")
}

fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn bad<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Value(msg.into()))
}

/// `binascii.a2b_base64(data, strict_mode=strict)`；出错是 binascii.Error（ValueError 的子类）
pub fn a2b_base64(data: &[u8], strict: bool) -> R<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len().div_ceil(4) * 3);
    if strict && data.first() == Some(&b'=') {
        return bad("Leading padding not allowed");
    }
    let (mut quad, mut left, mut pads, mut padding_started) = (0u8, 0u8, 0u8, false);
    for (i, &c) in data.iter().enumerate() {
        if c == b'=' {
            padding_started = true;
            if strict && quad == 0 {
                return bad("Excess padding not allowed");
            }
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    // 凑够一组了：后面的不再看（严格时后面还有东西是错）
                    if strict && i + 1 < data.len() {
                        return bad("Excess data after padding");
                    }
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(v) = value(c) else {
            if strict {
                return bad("Only base64 data is allowed");
            }
            continue;
        };
        if strict && padding_started {
            return bad("Discontinuous padding not allowed");
        }
        pads = 0;
        match quad {
            0 => left = v,
            1 => {
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                out.push((left << 6) | v);
                left = 0;
            }
        }
        quad = (quad + 1) % 4;
    }
    match quad {
        0 => Ok(out),
        1 => bad(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            out.len() / 3 * 4 + 1
        )),
        _ => bad("Incorrect padding"),
    }
}

/// `base64.b64decode(s, validate=validate)`，s 是 str：有非 ASCII 的字符是 ValueError
pub fn b64decode(s: &str, validate: bool) -> R<Vec<u8>> {
    if !s.is_ascii() {
        return bad("string argument should contain only ASCII characters");
    }
    a2b_base64(s.as_bytes(), validate)
}

/// `base64.urlsafe_b64decode(s)`，s 是 str：`-` `_` 当 `+` `/`，其余和不严格的 b64decode 一样
pub fn urlsafe_b64decode(s: &str) -> R<Vec<u8>> {
    if !s.is_ascii() {
        return bad("string argument should contain only ASCII characters");
    }
    let swapped: Vec<u8> = s
        .bytes()
        .map(|c| match c {
            b'-' => b'+',
            b'_' => b'/',
            c => c,
        })
        .collect();
    a2b_base64(&swapped, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::py::hash::{from_hex, to_hex};
    use crate::py::json;
    use crate::py::obj::V;

    fn expect(got: R<Vec<u8>>, want: &V, what: &str, input: &str) {
        let want = want.as_dict().unwrap();
        match (got, want.get("ok"), want.get("err")) {
            (Ok(g), Some(ok), _) => assert_eq!(to_hex(&g), ok.py_str(), "{what} {input:?}"),
            (Err(Fail::Value(e)), _, Some(err)) => assert_eq!(e, err.py_str(), "{what} {input:?}"),
            (got, ok, err) => panic!("{what} {input:?}：得到 {:?}，应该是 ok={ok:?} err={err:?}", got.map(|g| to_hex(&g)).map_err(|_| "别的错")),
        }
    }

    #[test]
    fn matches_cpython_vectors() {
        let doc = json::loads(include_str!("../../tests/vectors/base64.json")).unwrap();
        let cases = doc.as_dict().unwrap().dict("cases").unwrap();
        let decode = cases.list("decode").unwrap().items();
        assert!(decode.len() > 60);
        for c in decode {
            let c = c.as_dict().unwrap();
            let input = c.at("in").py_str();
            expect(b64decode(&input, false), &c.at("loose"), "b64decode", &input);
            expect(b64decode(&input, true), &c.at("strict"), "b64decode(validate)", &input);
            expect(urlsafe_b64decode(&input), &c.at("urlsafe"), "urlsafe_b64decode", &input);
        }
        let encode = cases.list("encode").unwrap().items();
        assert!(encode.len() > 20);
        for c in encode {
            let c = c.as_dict().unwrap();
            let raw = from_hex(&c.at("hex").py_str()).unwrap();
            assert_eq!(b64encode(&raw), c.at("b64").py_str());
            assert_eq!(urlsafe_b64encode(&raw), c.at("urlsafe").py_str());
            assert_eq!(b64decode(&b64encode(&raw), true).ok(), Some(raw));
        }
    }
}
