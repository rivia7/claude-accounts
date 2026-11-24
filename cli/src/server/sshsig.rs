//! 验设备的签名：`ssh-keygen -Y sign` 的输出（SSHSIG，OpenSSH 的 PROTOCOL.sshsig），只认 Ed25519（Py:115-239）。
//!
//! Python 那一份自己实现了 Ed25519 的验签（服务器上没有别的可靠办法）；这里用 ring 的。拆签名的每一步照 `ssh_verify` 的先后和宽严：
//! 哪一步不对都只是“验不过”（false），不出错、不 panic——签名是没验过身份的人发来的。
//!
//! 和 Python 不一样的一处：ring 不认写法不规范的公钥和签名（比如坐标没取模的），Python 的参考实现有的认。这只影响故意造出来的输入。

use super::wire::Cur;
use crate::py::{base64, hash, text};

const BEGIN: &str = "-----BEGIN SSH SIGNATURE-----";
const END: &str = "-----END SSH SIGNATURE-----";
const KIND: &[u8] = b"ssh-ed25519";

/// 公钥那一行（`ssh-ed25519 AAAA… 说明`）里 base64 那一段解出来的字节；没有第二段、解不开是 None。
/// 和 Python 的 `base64.b64decode(pubkey.split()[1])` 一样宽：不是 base64 的字符跳过
pub fn key_blob(line: &str) -> Option<Vec<u8>> {
    let b64 = text::split_ws(line).get(1).copied()?;
    base64::b64decode(b64, false).ok()
}

/// 公钥的指纹，和 `ssh-keygen -l` 的写法一样：`SHA256:` 加上摘要的 base64（去掉结尾的 `=`）（Py:115-117）
pub fn key_fp(blob: &[u8]) -> String {
    format!("SHA256:{}", base64::b64encode(&hash::sha256(blob)).trim_end_matches('='))
}

/// Team 的编号：根公钥（解出来的字节）的 SHA-256 的前 32 个十六进制字符（Py:120-122）
pub fn team_id(root_blob: &[u8]) -> String {
    hash::sha256_hex(root_blob).chars().take(32).collect()
}

/// SSH 线上格式的 string：4 个字节的长度（高位在前）加内容（Py:193-200）
fn string<'a>(cur: &mut Cur<'a>) -> Option<&'a [u8]> {
    let n = cur.u32()?;
    cur.take(usize::try_from(n).ok()?)
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) -> Option<()> {
    out.extend_from_slice(&u32::try_from(s.len()).ok()?.to_be_bytes());
    out.extend_from_slice(s);
    Some(())
}

/// 一个 string 里正好是 `ssh-ed25519` 和另一个 string（公钥、签名都是这个样子）：给出后面那一个
fn ed25519_part(blob: &[u8]) -> Option<&[u8]> {
    let mut cur = Cur::new(blob);
    let (kind, body) = (string(&mut cur)?, string(&mut cur)?);
    (kind == KIND && cur.is_empty()).then_some(body)
}

/// armored 是 `ssh-keygen -Y sign` 的输出，pub_blob 是公钥解出来的字节（`key_blob`）：是不是这把密钥在这个命名空间里对 data 签的。
/// 签名里的公钥必须就是这一把，命名空间必须一样（Py:203-232）
pub fn ssh_verify(pub_blob: &[u8], ns: &str, data: &[u8], armored: &[u8]) -> bool {
    check(pub_blob, ns, data, armored).is_some()
}

fn check(pub_blob: &[u8], ns: &str, data: &[u8], armored: &[u8]) -> Option<()> {
    // sig.decode('ascii').strip().splitlines()：不是 ASCII 的不认；前后的空白不算；按 Python 认的那些换行断开
    if !armored.is_ascii() {
        return None;
    }
    let lines = text::splitlines(text::strip(std::str::from_utf8(armored).ok()?));
    let (first, rest) = lines.split_first()?;
    let (last, middle) = rest.split_last()?;
    if middle.is_empty() || *first != BEGIN || *last != END {
        return None;
    }
    let blob = base64::b64decode(&middle.concat(), true).ok()?;
    let mut cur = Cur::new(&blob);
    if cur.take(6)? != b"SSHSIG" || cur.u32()? != 1 {
        return None;
    }
    let pk = string(&mut cur)?;
    let namespace = string(&mut cur)?;
    let reserved = string(&mut cur)?;
    let halg = string(&mut cur)?;
    let sig = string(&mut cur)?;
    if !cur.is_empty() || pk != pub_blob || namespace != ns.as_bytes() {
        return None;
    }
    let digest = match halg {
        b"sha512" => ring::digest::digest(&ring::digest::SHA512, data),
        b"sha256" => ring::digest::digest(&ring::digest::SHA256, data),
        _ => return None,
    };
    let (point, raw) = (ed25519_part(pk)?, ed25519_part(sig)?);
    // 签的不是 data 本身，是这一串：SSHSIG、命名空间、reserved、哈希算法、data 的摘要
    let mut signed = b"SSHSIG".to_vec();
    put_string(&mut signed, namespace)?;
    put_string(&mut signed, reserved)?;
    put_string(&mut signed, halg)?;
    put_string(&mut signed, digest.as_ref())?;
    ed25519_verify(point, &signed, raw).then_some(())
}

/// Ed25519 验签：公钥 32 个字节、签名 64 个字节，别的长度、不在曲线上的点都是 false（Py:181-190）
fn ed25519_verify(public: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    public.len() == 32
        && sig.len() == 64
        && ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public).verify(msg, sig).is_ok()
}

/// 验签自己验一遍（RFC 8032 的测试向量 1，和改了一个字节的）：结果在 /v1/health 的 sig_ok、启动时说的那一句里（Py:235-239）
pub fn self_test() -> bool {
    let (Some(public), Some(sig)) = (
        hash::from_hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"),
        hash::from_hex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
    ) else {
        return false;
    };
    let mut changed = sig.clone();
    match changed.last_mut() {
        Some(b) if *b == 0x0b => *b = 0x0c,
        _ => return false,
    }
    ed25519_verify(&public, b"", &sig) && !ed25519_verify(&public, b"x", &sig) && !ed25519_verify(&public, b"", &changed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::py::json;
    use crate::py::obj::Dict;

    /// 对照数据（cli/tests/vectors/server_sshsig.json）：签名是真的 `ssh-keygen -Y sign -n cca-presence` 签的（OpenSSH 10.5p1；
    /// 一次性的密钥，签完就删了，文件里只有公钥），sha512 和 `-O hashalg=sha256` 的各一份；别的各项是照着它们改出来的。
    /// 每一项的 ok 是 server/cca-presence.py 的 `ssh_verify` 给的结果：两边要一项不差
    fn vectors() -> Dict {
        json::loads(include_str!("../../tests/vectors/server_sshsig.json")).unwrap().as_dict().unwrap()
    }

    fn b64(d: &Dict, k: &str) -> Vec<u8> {
        base64::b64decode(&d.str(k).unwrap(), true).unwrap()
    }

    #[test]
    fn matches_the_python_verdict_on_every_vector() {
        let doc = vectors();
        let pubs: Vec<Vec<u8>> = doc.list("pub").unwrap().strs().iter().map(|line| key_blob(line).unwrap()).collect();
        let cases = doc.list("cases").unwrap().items();
        assert!(cases.len() >= 60);
        let (mut good, mut bad) = (0, 0);
        for c in cases {
            let c = c.as_dict().unwrap();
            let name = c.str("name").unwrap();
            let which = c.get("pub").unwrap().as_int().unwrap() as usize - 1;
            let want = c.get("ok").unwrap().as_bool().unwrap();
            let got = ssh_verify(&pubs[which], &c.str("ns").unwrap(), &b64(&c, "data"), &b64(&c, "sig"));
            assert_eq!(got, want, "{name}");
            if want {
                good += 1;
            } else {
                bad += 1;
            }
        }
        assert!(good >= 20 && bad >= 30, "验得过的 {good} 项、验不过的 {bad} 项");
    }

    /// 真的 ssh-keygen 签的那两份（sha512、sha256）验得过；命名空间、内容、公钥随便哪个不对就验不过
    #[test]
    fn real_ssh_keygen_signatures() {
        let doc = vectors();
        let pubs = doc.list("pub").unwrap().strs();
        let (key, other) = (key_blob(&pubs[0]).unwrap(), key_blob(&pubs[1]).unwrap());
        let cases = doc.list("cases").unwrap().items();
        let case = |name: &str| cases.iter().map(|c| c.as_dict().unwrap()).find(|c| &*c.str("name").unwrap() == name).unwrap();
        for name in ["sha512", "sha256"] {
            let c = case(name);
            let (data, sig) = (b64(&c, "data"), b64(&c, "sig"));
            assert!(String::from_utf8_lossy(&sig).starts_with("-----BEGIN SSH SIGNATURE-----\n"));
            assert!(ssh_verify(&key, "cca-presence", &data, &sig), "{name}");
            assert!(!ssh_verify(&key, "cca-team", &data, &sig), "{name}：命名空间不一样");
            assert!(!ssh_verify(&key, "", &data, &sig), "{name}：没有命名空间");
            assert!(!ssh_verify(&other, "cca-presence", &data, &sig), "{name}：另一把公钥");
            assert!(!ssh_verify(&[], "cca-presence", &data, &sig), "{name}：没有公钥");
            assert!(!ssh_verify(&key, "cca-presence", b"", &sig), "{name}：内容是空的");
            let mut longer = data.clone();
            longer.push(b' ');
            assert!(!ssh_verify(&key, "cca-presence", &longer, &sig), "{name}：内容多了一个字节");
            // 签名里随便哪个字节改掉一位都验不过（base64 那几行里的每个位置都试一遍）
            let text = String::from_utf8(sig.clone()).unwrap();
            let body_at = text.find('\n').unwrap() + 1;
            let body_end = text.rfind("\n-----END").unwrap();
            for i in body_at..body_end {
                let mut changed = sig.clone();
                changed[i] = match changed[i] {
                    b'\n' => continue,
                    b'A' => b'B',
                    _ => b'A',
                };
                assert!(!ssh_verify(&key, "cca-presence", &data, &changed), "{name}：第 {i} 个字节改了还验得过");
            }
        }
    }

    /// 指纹和 ssh-keygen -l 的一样、Team 的编号和 Python 算的一样（对照数据里的是它们给的）
    #[test]
    fn fingerprint_and_team_id() {
        let doc = vectors();
        let (pubs, fps, ids) =
            (doc.list("pub").unwrap().strs(), doc.list("fp").unwrap().strs(), doc.list("team_id").unwrap().strs());
        for i in 0..2 {
            let blob = key_blob(&pubs[i]).unwrap();
            assert_eq!(blob.len(), 51, "4 + 11 + 4 + 32 个字节");
            assert_eq!((key_fp(&blob), team_id(&blob)), (fps[i].to_string(), ids[i].to_string()));
            assert!(crate::server::wire::fp_re().matches(&key_fp(&blob)) && team_id(&blob).len() == 32);
        }
        // 公钥那一行：说明可有可无，前后、中间的空白随意；没有第二段是 None
        let b64 = pubs[0].split(' ').nth(1).unwrap();
        let blob = key_blob(&pubs[0]).unwrap();
        assert_eq!(key_blob(&format!("  ssh-ed25519\t{b64}   某台设备 的说明\n")).unwrap(), blob);
        assert_eq!(key_blob(&format!("x {b64}")).unwrap(), blob, "头一段是什么不看");
        assert!(key_blob("ssh-ed25519").is_none() && key_blob("").is_none() && key_blob("   ").is_none());
        assert!(key_blob("ssh-ed25519 A").is_none(), "解不开的 base64");
        // 空的摘要：SHA-256("") 的两种写法
        assert_eq!(key_fp(b""), "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU");
        assert_eq!(team_id(b""), "e3b0c44298fc1c149afbf4c8996fb924");
    }

    /// 启动时的自检：RFC 8032 的测试向量 1
    #[test]
    fn self_test_passes() {
        assert!(self_test());
        // 长度不对的公钥、签名：验不过，不出错
        let key = [7u8; 32];
        assert!(!ed25519_verify(&key[..31], b"", &[0u8; 64]) && !ed25519_verify(&key, b"", &[0u8; 63]));
        assert!(!ed25519_verify(&key, b"", &[0u8; 64]) && !ed25519_verify(&[], b"", &[]));
    }

    /// 同一个签名的 s 加上群的阶 L：照验签的式子算也成立，但 s 必须小于 L（RFC 8032 5.1.7；不然同一份内容就多出一个验得过的签名）。
    /// Python 那一份自己查这一条（原来 tests/presence.py 的 VerifyTest），这里看的是 ring 也不收
    #[test]
    fn s_plus_the_group_order_is_refused() {
        let public = hash::from_hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a").unwrap();
        let sig = hash::from_hex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        )
        .unwrap();
        // L = 2^252 + 27742317777372353535851937790883648493，低位在前
        let order = hash::from_hex("edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010").unwrap();
        let mut moved = sig.clone();
        let mut carry = 0u16;
        for (b, l) in moved[32..].iter_mut().zip(&order) {
            let sum = u16::from(*b) + u16::from(*l) + carry;
            *b = u8::try_from(sum & 0xff).unwrap();
            carry = sum >> 8;
        }
        assert_eq!(carry, 0, "s + L 还放得进 32 个字节");
        // Python 算的同一个签名（int.from_bytes(s, 'little') + EL），它的 ed25519_verify 不收
        assert_eq!(
            hash::to_hex(&moved),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901554c8c7872aa064e049dbb3013fbf29380d25bf5f0595bbe24655141438e7a101b"
        );
        assert!(ed25519_verify(&public, b"", &sig) && !ed25519_verify(&public, b"", &moved));
    }

    /// 随便什么字节都只是验不过：截断的、长度写得很大的、全是 0 的
    #[test]
    fn garbage_never_panics() {
        let key = key_blob(&vectors().list("pub").unwrap().strs()[0]).unwrap();
        let armor = |blob: &[u8]| format!("{BEGIN}\n{}\n{END}\n", base64::b64encode(blob)).into_bytes();
        let mut blobs: Vec<Vec<u8>> = vec![vec![], b"SSHSIG".to_vec(), b"SSHSIG\0\0\0\x01".to_vec(), vec![0; 64], vec![0xff; 64]];
        // 长度是 0xffffffff 的 string、一层套一层的长度
        blobs.push([b"SSHSIG\0\0\0\x01".as_slice(), &[0xff; 4]].concat());
        blobs.push([b"SSHSIG\0\0\0\x01".as_slice(), &[0, 0, 0, 51], &key, &[0x7f, 0xff, 0xff, 0xff], b"cca"].concat());
        for n in 0..40 {
            blobs.push([b"SSHSIG\0\0\0\x01".as_slice(), &vec![0u8; n]].concat());
        }
        for blob in &blobs {
            assert!(!ssh_verify(&key, "cca-presence", b"data", &armor(blob)));
            assert!(!ssh_verify(&key, "cca-presence", b"data", blob));
        }
        for raw in [
            &b"\n\n\n"[..],
            b"-----BEGIN SSH SIGNATURE-----",
            b"-----BEGIN SSH SIGNATURE-----\n\n-----END SSH SIGNATURE-----",
            b"\xff\xfe",
        ] {
            assert!(!ssh_verify(&key, "cca-presence", b"data", raw));
        }
    }
}
