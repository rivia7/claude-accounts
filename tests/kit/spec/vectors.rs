//! kit::spec 的单元测试。
//!
//! 对照数据（tests/vectors/spec_wire.json）是原来 Python 的那一份（tests/spec_wire.py）在固定的输入上算出来的，这里逐项核对：
//! 定下来的那些要一个字节不差；带随机数的（封出来的密文）Python 封的这里要解得开，拿同一个 nonce 再封一遍要一模一样——
//! 明文怎么写（JSON 的写法、补齐）也就一起对上了。折合金额里压缩过的那一段两边的压缩器压得不一样：比的是解压出来的文字。
//! 数据是冻结的（那份 Python 以后要删）；格式要改时，这里的数据照 server/README.md 另算。
//!
//! 对照数据管不到的也在这里：ssh-keygen 的签名和验签，签过名的请求（对着本机现起的 TLS 服务器）。

use super::*;
use crate::fsx;
use crate::list;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn vectors() -> Value {
    json::parse(include_str!("../../vectors/spec_wire.json")).expect("对照数据是 JSON")
}

fn hx(v: &Value) -> Vec<u8> {
    fromhex(v.str())
}

fn key32(v: &Value) -> [u8; 32] {
    hx(v).try_into().expect("32 个字节")
}

/// 这一项里的两把密钥（enc、mac）
fn keys_in(case: &Value) -> Keys {
    (key32(&case["enc"]), key32(&case["mac"]))
}

fn strs(v: &Value) -> Vec<&str> {
    v.list().iter().map(Value::str).collect()
}

/// Python 封的 blob：拿它的 nonce 把这段明文再封一遍，要一模一样
fn same_seal(label: &str, keys: &Keys, ad: &[&str], plain: &[u8], blob: &Value) {
    assert_eq!(seal_with(label, keys, ad, plain, &unb64(blob["n"].str())), *blob, "{label} {ad:?}");
}

/// blob 的一段（n、c、t）改掉一位
fn flipped(blob: &Value, part: &str) -> Value {
    let mut bytes = unb64(blob[part].str());
    bytes[0] ^= 1;
    let mut out = blob.clone();
    out.as_obj_mut().expect("blob 是对象").insert(part, b64(&bytes));
    out
}

/// 改掉一位的一份
fn flip(bytes: &[u8], at: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at] ^= 1;
    out
}

/// 现造一把 ed25519 的密钥（path、path.pub）；给的是公钥那一行的前两段
fn keygen(path: &Path, comment: &str) -> String {
    let mut cmd = Cmd::new("ssh-keygen");
    cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"]).arg(path).pass_env(&["PATH"]);
    cmd.output().unwrap_or_else(|e| panic!("{e}")).check();
    let mut public = path.as_os_str().to_os_string();
    public.push(".pub");
    pub_line(&fsx::read_text(public))
}

#[test]
fn key_derivation_and_account_digests() {
    let v = vectors();
    for case in v["keys_of"].list() {
        assert_eq!(keys_of(case["salt"].str(), &hx(&case["info"])), keys_in(case), "{case}");
    }
    for case in v["derive"].list() {
        let got = derive(case["label"].str(), case["salt"].str(), case["tool"].str(), case["email"].str(), case["ident"].str());
        assert_eq!(got, keys_in(case), "{case}");
    }
    for case in v["team_keys"].list() {
        assert_eq!(team_keys(case["salt"].str(), case["label"].str()), keys_in(case), "{case}");
        assert_eq!(hex(&team_keys(case["salt"].str(), COST).0), case["default_label_enc"].str(), "Python 那边不写标签就是 COST");
    }
    for case in v["aid"].list() {
        assert_eq!(aid_of(case["salt"].str(), case["tool"].str(), case["email"].str()), case["aid"].str(), "{case}");
    }
    for case in v["orphan_aid"].list() {
        assert_eq!(orphan_aid(case["salt"].str()), case["aid"].str());
    }
}

#[test]
fn stream_tag_and_seal() {
    let v = vectors();
    for case in v["stream"].list() {
        let size = case["size"].i64() as usize;
        let out = stream(&hx(&case["enc"]), &hx(&case["n"]), size);
        assert_eq!(hex(&out), case["out"].str(), "{size}");
        assert_eq!(out.len(), size.div_ceil(32) * 32, "给的是整块的");
    }
    for case in v["tag"].list() {
        let got = tag(case["label"].str(), &hx(&case["mac"]), &strs(&case["ad"]), &hx(&case["n"]), &hx(&case["c"]));
        assert_eq!(hex(&got), case["out"].str(), "{case}");
    }
    for case in v["seal"].list() {
        let (label, keys, ad, pt, blob) = (case["label"].str(), keys_in(case), strs(&case["ad"]), hx(&case["pt"]), &case["blob"]);
        assert_eq!(unseal(label, &keys, &ad, blob), Some(pt.clone()), "{case}");
        same_seal(label, &keys, &ad, &pt, blob);
        // 自己封的自己解得开；nonce 是 16 个随机字节，每次不一样
        let (mine, again) = (seal(label, &keys, &ad, &pt), seal(label, &keys, &ad, &pt));
        assert_eq!(unseal(label, &keys, &ad, &mine), Some(pt.clone()));
        assert_ne!(mine["n"], again["n"]);
        assert_eq!(unb64(mine["n"].str()).len(), 16);
        assert_eq!(mine.obj().keys().collect::<Vec<_>>(), ["n", "c", "t"]);
        // 认证不过：标签、ad、密钥不一样，nonce、密文、认证码改过
        assert_eq!(unseal("cca-other", &keys, &ad, blob), None);
        assert_eq!(unseal(label, &keys, &[&ad[..], &["x"]].concat(), blob), None);
        assert_eq!(unseal(label, &(keys.0, keys.0), &ad, blob), None);
        for part in ["n", "c", "t"] {
            if !unb64(blob[part].str()).is_empty() {
                assert_eq!(unseal(label, &keys, &ad, &flipped(blob, part)), None, "{part}");
            }
        }
    }
}

#[test]
fn recorded_api_keys() {
    for case in vectors()["keyshare"].list() {
        let (keys, aid, doc, blob) = (keys_in(case), case["aid"].str(), &case["doc"], &case["blob"]);
        assert_eq!(keyshare_open(&keys, aid, blob).as_ref(), Some(doc));
        let plain = keyshare_plain(doc);
        assert_eq!(plain.len() as i64, case["plain_len"].i64());
        same_seal(KEYSHARE, &keys, &[aid], &plain, blob);
        assert_eq!(keyshare_open(&keys, aid, &keyshare_seal(&keys, aid, doc)).as_ref(), Some(doc));
        assert_eq!(keyshare_open(&keys, "0123", blob), None, "别的账号的");
    }
}

#[test]
fn cost_ledgers() {
    for case in vectors()["cost"].list() {
        let (label, keys, aid, fp, blob) = (case["label"].str(), keys_in(case), case["aid"].str(), case["fp"].str(), &case["blob"]);
        let z = hx(&case["z"]);
        let plain = unseal(label, &keys, &[aid, fp], blob).expect("认证得过");
        assert_eq!(plain.len() as i64, case["plain_len"].i64());
        assert_eq!(cost_plain(&z), plain, "长度、压缩过的那一段、补的零");
        same_seal(label, &keys, &[aid, fp], &plain, blob);
        assert_eq!(unseal(label, &keys, &[aid, fp], &cost_seal_raw(&keys, aid, fp, &z, label)), Some(plain.clone()));
        let other = if label == COST { COST_V1 } else { COST };
        assert_eq!(cost_open(&keys, aid, fp, blob, other), None, "别的标签");
        assert_eq!(cost_open(&keys, aid, "SHA256:other", blob, label), None, "别的设备的");
        assert_eq!(cost_open(&keys, "00", fp, blob, label), None, "别的账号的");
        let Some(doc) = case.get("doc") else {
            // 直接给的那一段不是 zlib：认证得过，解压不了
            let why = cost_unpack(&plain).unwrap_err();
            assert!(why.starts_with("解压不了"), "{why}");
            assert!(catch_unwind(AssertUnwindSafe(|| cost_open(&keys, aid, fp, blob, label))).is_err());
            continue;
        };
        assert_eq!(cost_open(&keys, aid, fp, blob, label).as_ref(), Some(doc));
        // 压缩前的文字和 Python 的 json.dumps 一样
        let text = miniz_oxide::inflate::decompress_to_vec_zlib(&z).expect("Python 压的解得开");
        assert_eq!(String::from_utf8(text).unwrap(), json::dumps(doc, &Style::default()));
        // 自己封的：解得开，明文补齐到 1024 的整数倍
        let mine = cost_seal(&keys, aid, fp, doc, label);
        assert_eq!(cost_open(&keys, aid, fp, &mine, label).as_ref(), Some(doc));
        let mine_plain = unseal(label, &keys, &[aid, fp], &mine).unwrap();
        assert!(mine_plain.len().is_multiple_of(1024) && !mine_plain.is_empty());
    }
    // 明文不成样子的几种
    let good = cost_plain(&miniz_oxide::deflate::compress_to_vec_zlib(b"{\"v\": 1}", 6));
    assert_eq!((good.len(), cost_unpack(&good)), (1024, Ok(obj! { "v" => 1 })));
    let padding = "补齐到 1024 字节的整数倍，补的是零";
    assert_eq!(cost_unpack(&flip(&good, 1023)).unwrap_err(), padding);
    assert_eq!(cost_unpack(&good[..1023]).unwrap_err(), padding);
    assert_eq!(cost_unpack(&[&good[..], &[0u8]].concat()).unwrap_err(), padding);
    assert!(cost_unpack(&[0, 0, 0]).is_err() && cost_unpack(&[]).is_err());
    assert!(cost_unpack(&cost_plain(&miniz_oxide::deflate::compress_to_vec_zlib(b"not json", 6)))
        .unwrap_err()
        .starts_with("解压出来的不是 JSON"));
}

#[test]
fn public_keys_and_fingerprints() {
    let dir = TempDir::new("spec-unit-");
    for case in vectors()["pub"].list() {
        let line = case["line"].str();
        fsx::write(dir.join("k.pub"), line);
        assert_eq!(fp_of(dir.join("k.pub")), case["fp"].str());
        assert_eq!(fp_of_line(line), case["fp"].str());
        assert_eq!(team_id(line), case["team_id"].str());
        assert_eq!(pub_line(line), case["two"].str());
    }
    // 和 ssh-keygen 自己说的指纹一样
    keygen(&dir.join("id"), "unit");
    let mut cmd = Cmd::new("ssh-keygen");
    cmd.args(["-l", "-f"]).arg(dir.join("id.pub")).pass_env(&["PATH"]);
    let shown = cmd.output().unwrap().check().out();
    assert_eq!(shown.split_whitespace().nth(1), Some(fp_of(dir.join("id.pub")).as_str()), "{shown}");
}

#[test]
fn pairing_codes() {
    let v = vectors();
    for case in v["code_text"].list() {
        let bytes = hx(&case["bytes"]);
        assert_eq!(code_text(&bytes), case["text"].str());
        assert_eq!(code_bytes(case["text"].str()), Some(bytes));
    }
    for case in v["code_bytes"].list() {
        assert_eq!(code_bytes(case["typed"].str()), case["bytes"].as_str().map(fromhex), "{case}");
    }
}

#[test]
fn pairing_messages_and_the_shared_secret() {
    for case in vectors()["pair"].list() {
        let (kc, keys, rid) = pair_keys(&hx(&case["code"]));
        assert_eq!((hex(&kc).as_str(), keys, rid.as_str()), (case["kc"].str(), keys_in(case), case["rid"].str()));
        for (role, other) in [("a", "b"), ("b", "a")] {
            let side = &case[role];
            let (fp, nonce, name, blob) = (side["fp"].str(), hx(&side["nonce"]), side["name"].str(), &side["blob"]);
            let want = obj! { "v" => 1, "fp" => fp, "n" => side["nonce"].str(), "name" => name };
            assert_eq!(pair_open(&keys, &rid, role, blob), Some(want.clone()));
            assert_eq!(pair_open(&keys, &rid, other, blob), None, "另一头放的不能当这一头的");
            let plain = pair_plain(fp, &nonce, name);
            assert_eq!(plain.len() as i64, side["plain_len"].i64());
            same_seal(PAIR, &keys, &[rid.as_str(), role], &plain, blob);
            assert_eq!(pair_open(&keys, &rid, role, &pair_msg(&keys, &rid, role, fp, &nonce, name)), Some(want));
        }
        let (a, b) = (&case["a"], &case["b"]);
        let (p, pid) = pair_secret(&kc, &hx(&a["nonce"]), &hx(&b["nonce"]), a["fp"].str(), b["fp"].str());
        assert_eq!((hex(&p).as_str(), pid.as_str()), (case["p"].str(), case["pid"].str()));
        // 两头的次序是定的：换过来就不是同一个密钥
        assert_ne!(pair_secret(&kc, &hx(&b["nonce"]), &hx(&a["nonce"]), b["fp"].str(), a["fp"].str()).1, pid);
    }
}

#[test]
fn session_notes() {
    for case in vectors()["dir"].list() {
        let (p, pid, sid, doc, blob) = (hx(&case["p"]), case["pid"].str(), case["sid"].str(), &case["doc"], &case["blob"]);
        assert_eq!(dir_keys(&p), keys_in(case));
        assert_eq!(dir_open(&p, pid, sid, blob).as_ref(), Some(doc));
        assert_eq!(dir_plain(doc).len() as i64, case["plain_len"].i64());
        same_seal(DIR, &dir_keys(&p), &[pid, sid], &dir_plain(doc), blob);
        assert_eq!(dir_open(&p, pid, sid, &dir_seal(&p, pid, sid, doc)).as_ref(), Some(doc));
        assert_eq!(dir_open(&p, sid, pid, blob), None, "别的会话的");
    }
}

/// 对照数据里长的那几条记录的内容（Python 那边是同一个算法）
fn fill(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[test]
fn channel_handshake_and_records() {
    for case in vectors()["channel"].list() {
        let (p, pid, sid, fph, fpv) = (hx(&case["p"]), case["pid"].str(), case["sid"].str(), case["fph"].str(), case["fpv"].str());
        let mut viewer = Channel::new("viewer", &p, pid, sid, fph, fpv);
        let mut host = Channel::new("host", &p, pid, sid, fph, fpv);
        let hello = viewer.hello_with(&hx(&case["nv"]));
        assert_eq!(hex(&hello), case["hello"].str());
        let reply = host.reply_with(&hello, &hx(&case["nh"])).expect("hello 是对的");
        assert_eq!(hex(&reply), case["reply"].str());
        let confirm = viewer.confirm(&reply).expect("宿主的证明是对的");
        assert_eq!(hex(&confirm), case["confirm"].str());
        assert!(host.accept(&confirm));
        for record in case["records"].list() {
            let data = match record.get("fill") {
                Some(n) => fill(n.i64() as usize),
                None => hx(&record["data"]),
            };
            let (sender, receiver) =
                if record["from"].str() == "viewer" { (&mut viewer, &mut host) } else { (&mut host, &mut viewer) };
            let wire = sender.seal(&data);
            match record.get("wire") {
                Some(want) => assert_eq!(hex(&wire), want.str()),
                None => assert_eq!(
                    (sha256_hex(&wire).as_str(), wire.len() as i64),
                    (record["wire_sha256"].str(), record["wire_len"].i64())
                ),
            }
            // 一个字节一个字节地到：不够一条是 None，够了给出内容、从缓冲里拿走
            let mut buf = Vec::new();
            for (i, byte) in wire.iter().enumerate() {
                buf.push(*byte);
                let got = receiver.open(&mut buf).expect("记录是对的");
                assert_eq!(got.is_some(), i + 1 == wire.len());
                if let Some(got) = got {
                    assert_eq!(got, data);
                }
            }
            assert!(buf.is_empty());
        }
    }
}

const P: [u8; 32] = [7; 32];
const PID: &str = "00112233445566778899aabbccddeeff";
const SID: &str = "ffeeddccbbaa99887766554433221100";

/// 一对还没握手的通道：(接入方, 宿主)
fn pair() -> (Channel, Channel) {
    (Channel::new("viewer", &P, PID, SID, "SHA256:H", "SHA256:V"), Channel::new("host", &P, PID, SID, "SHA256:H", "SHA256:V"))
}

/// 握完手的一对
fn shaken() -> (Channel, Channel) {
    let (mut viewer, mut host) = pair();
    let reply = host.reply(&viewer.hello()).expect("hello");
    assert!(host.accept(&viewer.confirm(&reply).expect("reply")));
    (viewer, host)
}

#[test]
fn channel_handshake_rejects_what_is_wrong() {
    // 随机数现出的握手：长度是文档写的，每次不一样
    let (mut viewer, mut host) = pair();
    let hello = viewer.hello();
    let reply = host.reply(&hello).unwrap();
    let confirm = viewer.confirm(&reply).unwrap();
    assert_eq!((hello.len(), reply.len(), confirm.len()), (52, 68, 35));
    assert!(host.accept(&confirm));
    assert_ne!(pair().0.hello(), hello);
    // hello：长度、开头、版本、配对的编号不对
    for bad in
        [hello[..51].to_vec(), [&hello[..], &[0]].concat(), flip(&hello, 0), flip(&hello, 2), flip(&hello, 3), flip(&hello, 51)]
    {
        assert_eq!(pair().1.reply(&bad), None);
    }
    // reply：长度、开头不对，证明改过，随机数改过
    for bad in [reply[..67].to_vec(), flip(&reply, 2), flip(&reply, 3), flip(&reply, 10), flip(&reply, 67)] {
        let (mut viewer, _) = pair();
        viewer.hello_with(&hello[4..36]);
        assert_eq!(viewer.confirm(&bad), None);
    }
    // 不知道 P 的宿主、记着的指纹不一样的宿主（中间有人换了公钥）回的：接入方不认
    for mut stranger in [
        Channel::new("host", &[8; 32], PID, SID, "SHA256:H", "SHA256:V"),
        Channel::new("host", &P, PID, SID, "SHA256:H", "SHA256:other"),
        Channel::new("host", &P, PID, PID, "SHA256:H", "SHA256:V"),
    ] {
        let (mut viewer, _) = pair();
        let reply = stranger.reply(&viewer.hello()).expect("hello 本身是对的");
        assert_eq!(viewer.confirm(&reply), None);
    }
    // confirm：长度、开头不对，证明改过
    for bad in [confirm[..34].to_vec(), flip(&confirm, 0), flip(&confirm, 34)] {
        let (mut viewer, mut host) = pair();
        let reply = host.reply(&viewer.hello()).unwrap();
        viewer.confirm(&reply).unwrap();
        assert!(!host.accept(&bad));
    }
    // 别的会话里的 confirm 拿过来（重放）不行
    let (mut viewer, mut host) = pair();
    host.reply(&viewer.hello()).unwrap();
    assert!(!host.accept(&confirm));
}

#[test]
fn channel_records_reject_what_is_wrong() {
    let (mut viewer, mut host) = shaken();
    let (first, second) = (viewer.seal(b"one"), viewer.seal(b"two"));
    // 跳过一条、改过内容、改过认证码：auth，缓冲原样留着
    for bad in [second.clone(), flip(&first, 12), flip(&first, first.len() - 1), flip(&first, 7)] {
        let mut buf = bad.clone();
        assert_eq!(host.open(&mut buf), Err("auth"));
        assert_eq!(buf, bad);
    }
    // 两条连着到：一条一条地给
    let mut buf = [&first[..], &second[..]].concat();
    assert_eq!(host.open(&mut buf), Ok(Some(b"one".to_vec())));
    assert_eq!(buf, second);
    assert_eq!(host.open(&mut buf), Ok(Some(b"two".to_vec())));
    assert_eq!(host.open(&mut buf), Ok(None));
    // 重放；自己发的那个方向的密钥解不了对方发的
    assert_eq!(host.open(&mut first.clone()), Err("auth"));
    let (mut fresh, _) = shaken();
    let own = fresh.seal(b"x");
    assert_eq!(fresh.open(&mut own.clone()), Err("auth"));
    // 长度：0、不是 32 的整数倍、比最长的还长
    for n in [0u16, 31, 33, 16416 + 32] {
        let mut buf = [&[0u8; 8][..], &n.to_be_bytes()].concat();
        assert_eq!(host.open(&mut buf), Err("len"), "{n}");
    }
    assert_eq!(host.open(&mut vec![0; 9]), Ok(None), "还看不出长度");
    // 认证得过、补齐不对（拿这个方向的密钥自己拼一条）：pad，这一条算收过了
    let (enc, mac) = *Channel::side(&viewer.out);
    let craft = |seq: u64, pt: &[u8]| {
        let seq = seq.to_be_bytes();
        let ct = xor(pt, &stream(&enc, &seq, pt.len()));
        let record = [&seq[..], &(ct.len() as u16).to_be_bytes(), &ct].concat();
        [&record[..], &h(&mac, &record)[..16]].concat()
    };
    let mut too_long = vec![0u8; 32];
    too_long[1] = 31; // 说有 31 个字节，可一共只放得下 30 个
    let mut dirty = vec![0u8; 32];
    (dirty[1], dirty[31]) = (1, 9); // 补的不是零
    for pt in [too_long, dirty] {
        let mut buf = craft(host.recv_seq, &pt);
        assert_eq!(host.open(&mut buf), Err("pad"));
        assert!(buf.is_empty());
    }
    // 之后的记录照样收（序号已经往前数了两个）
    viewer.send_seq = host.recv_seq;
    assert_eq!(host.open(&mut viewer.seal(b"after")), Ok(Some(b"after".to_vec())));
    // 一条最多 16384 个字节
    let mut longest = viewer.seal(&vec![1; 16384]);
    assert_eq!(longest.len(), 10 + 16416 + 16);
    assert_eq!(host.open(&mut longest).unwrap().unwrap().len(), 16384);
    assert!(catch_unwind(AssertUnwindSafe(|| viewer.seal(&vec![0; 16385]))).is_err());
    // 握手没做完就收发：panic，不是拿空的密钥算
    assert!(catch_unwind(AssertUnwindSafe(|| pair().0.seal(b"x"))).is_err());
}

#[test]
fn relay_lines() {
    for case in vectors()["relay"].list() {
        let (role, ident, key, cn) = (case["role"].str(), case["ident"].str(), hx(&case["key"]), case["cn"].str());
        let (line, back) = relay_line_with(role, ident, &key, cn);
        assert_eq!(String::from_utf8(line).unwrap(), case["line"].str());
        assert_eq!(String::from_utf8(back).unwrap(), case["reply"].str());
        // 不给 cn：现出一个（16 个十六进制字符），别的照旧
        let (line, back) = relay_line(role, ident, &key);
        let text = String::from_utf8(line.clone()).unwrap();
        let words: Vec<&str> = text.trim_end_matches('\n').split(' ').collect();
        assert_eq!((words.len(), words[0], words[1], words[2], words[3].len()), (5, "CCA-RELAY/1", role, ident, 16));
        assert_eq!(relay_line_with(role, ident, &key, words[3]), (line, back));
    }
}

#[test]
fn invites() {
    let v = vectors();
    for case in v["invite_parts"].list() {
        let got = invite_parts(case["secret"].str());
        match case.get("error") {
            Some(error) => assert_eq!(got.unwrap_err(), error.str(), "{case}"),
            None => {
                let (token, dev, keys) = got.unwrap();
                assert_eq!((token.as_str(), dev.as_str(), keys), (case["token"].str(), case["dev"].str(), keys_in(case)));
            }
        }
    }
    for case in v["team_pub_proof"].list() {
        assert_eq!(team_pub_proof(case["secret"].str(), case["pub"].str()), case["proof"].str());
    }
    for case in v["invite"].list() {
        let exp = Some(&case["exp"]).filter(|exp| !exp.is_null());
        let (url, pins, root) = (case["url"].str(), strs(&case["pins"]), case["root"].str());
        let link = invite_encode(url, &pins, root, case["secret"].str(), case["name"].str(), exp, case["device"].as_str());
        assert_eq!(link, case["link"].str(), "{case}");
        assert_eq!(invite_decode(&link).as_ref(), Ok(&case["decoded"]));
    }
    for case in v["invite_decode"].list() {
        let got = invite_decode(case["text"].str());
        match (case.get("decoded"), case.get("error")) {
            (Some(decoded), _) => assert_eq!(got.as_ref(), Ok(decoded), "{case}"),
            // 开头、校验不对：说的是同一句话；校验对、里面不对的（Python 那边各有各的说法）只看出不出错
            (None, Some(error)) => match error.str().strip_prefix("ValueError: ") {
                Some(text) => assert_eq!(got.unwrap_err(), text, "{case}"),
                None => assert!(got.is_err(), "{case}"),
            },
            (None, None) => panic!("对照数据里这一项没写结果：{case}"),
        }
    }
}

#[test]
fn team_documents() {
    for case in vectors()["team_doc"].list() {
        let (token, dev, keys) = invite_parts(case["secret"].str()).unwrap();
        let (tid, seq, doc, ct) = (case["tid"].str(), case["seq"].i64(), &case["doc"], &case["ct"]);
        assert_eq!(dev, case["dev"].str());
        assert_eq!(team_doc_open(&keys, tid, &dev, seq, ct).as_ref(), Some(doc));
        assert_eq!(team_doc_open(&keys, tid, &dev, seq + 1, ct), None, "别的序号的");
        assert_eq!(team_doc_open(&keys, tid, &token[..16], seq, ct), None, "别的设备的");
        assert_eq!(team_doc_open(&keys, &dev.repeat(2), &dev, seq, ct), None, "别的 Team 的");
        assert_eq!(team_doc_plain(doc).len() as i64, case["plain_len"].i64());
        same_seal(TEAM_DOC, &keys, &[tid, dev.as_str(), seq.to_string().as_str()], &team_doc_plain(doc), ct);
        assert_eq!(team_doc_open(&keys, tid, &dev, seq, &team_doc_seal(&keys, tid, &dev, seq, doc)).as_ref(), Some(doc));
        // 根密钥签的那一段，和 Python 那边拿 ssh-keygen 签的名
        assert_eq!(sha256_hex(&team_doc_signed(tid, &dev, seq, ct)), case["signed_sha256"].str());
        let (root, sig) = (case["root_pub"].str(), case["sig"].str());
        assert!(team_doc_verify(root, tid, &dev, seq, ct, sig));
        assert!(!team_doc_verify(case["other_pub"].str(), tid, &dev, seq, ct, sig), "别的密钥");
        assert!(!team_doc_verify(root, tid, &dev, seq + 1, ct, sig), "别的序号");
        assert!(!team_doc_verify(root, tid, &dev, seq, &flipped(ct, "c"), sig), "改过的密文");
    }
}

/// 辅助程序那边的对照数据（cli/tests/vectors/team_doc_ct.json：原来的 spec_wire.py 固定 nonce 封的一份配置）这里也算得出来。
/// 文件里只有密文 {"n", "c", "t"}，封它用的那几样不在里面：那串随机数是试出来的（"ab" 写 16 遍：照它派生的密钥解开，正好是一份
/// 补齐了的配置）；认证带的 Team 的编号、设备的编号、序号只写在辅助程序的测试里，这里不看那边的代码，所以认证码比不了，
/// 比的是 nonce 和密文——密钥怎么派生、密钥流、明文怎么写和补齐都在里面了
#[test]
fn the_helpers_team_doc_vector() {
    let ct = json::parse(include_str!("../../../cli/tests/vectors/team_doc_ct.json")).expect("对照数据是 JSON");
    let (n, c, t) = (unb64(ct["n"].str()), unb64(ct["c"].str()), unb64(ct["t"].str()));
    assert_eq!((n.len(), c.len() % 1024, t.len()), (16, 0, 32));
    let (_, _, keys) = invite_parts(&"ab".repeat(16)).unwrap();
    let plain = xor(&c, &stream(&keys.0, &n, c.len()));
    let doc = loads(&plain);
    let want = obj! { "v" => 1, "at" => 1790000000, "name" => "测试组", "accounts" => obj! { "claude" => obj! { "alice@example.com" => "US-A" } } };
    assert_eq!(doc.compact(), want.compact());
    assert_eq!(team_doc_plain(&doc), plain, "明文是紧凑的 JSON，空格补齐到 1024 字节的整数倍");
    // 拿同一个 nonce 再封一遍：nonce、密文一个字节不差（认证带的那三样随便写：它们只进认证码）
    let again = seal_with(TEAM_DOC, &keys, &["?", "?", "0"], &plain, &n);
    assert_eq!((again["n"].str(), again["c"].str()), (ct["n"].str(), ct["c"].str()));
}

#[test]
fn signatures_made_here_verify() {
    let dir = TempDir::new("spec-unit-");
    let (root, other) = (keygen(&dir.join("root"), "root"), keygen(&dir.join("other"), "other"));
    let (_, dev, keys) = invite_parts(&"7e".repeat(16)).unwrap();
    let tid = team_id(&root);
    let ct = team_doc_seal(&keys, &tid, &dev, 3, &obj! { "v" => 1, "name" => "书房组" });
    let sig = team_doc_sign(dir.join("root"), &tid, &dev, 3, &ct);
    assert!(team_doc_verify(&root, &tid, &dev, 3, &ct, &sig));
    assert!(!team_doc_verify(&other, &tid, &dev, 3, &ct, &sig), "别的密钥");
    assert!(!team_doc_verify(&root, &tid, &dev, 4, &ct, &sig), "别的序号");
    // 别的命名空间里签的不算
    let signed = team_doc_signed(&tid, &dev, 3, &ct);
    let raw = ssh_sign(&dir.join("root"), "cca-presence", &signed, true).unwrap();
    assert!(ssh_verify(&root, "cca-presence", &signed, &raw));
    assert!(!team_doc_verify(&root, &tid, &dev, 3, &ct, &b64(&raw)));
    // 签不了（没有这把私钥）是 Err，不是一个空的签名
    assert!(ssh_sign(&dir.join("missing"), "cca-team", b"x", true).is_err());
}

#[test]
fn request_bodies() {
    for case in vectors()["post_body"].list() {
        let raw = post_body(&case["extra"], case["op"].str(), case["key"].str(), case["ts"].f64(), case["nonce"].str());
        assert_eq!(String::from_utf8(raw).unwrap(), case["raw"].str());
    }
}

#[test]
fn signed_requests_reach_a_tls_server() {
    use std::io::Write;
    let dir = TempDir::new("spec-unit-");
    crate::tls::testkit::self_signed(&crate::tls::testkit::openssls()[0], &dir);
    let public = keygen(&dir.join("dev"), "dev");
    // 服务器：还欠着几次 429 就回 429；/v1/empty 回空的 404；别的核对签名，把收到的 body 放在回应里
    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let owed = Arc::new(AtomicUsize::new(1));
    let port = {
        let (seen, owed, public) = (seen.clone(), owed.clone(), public.clone());
        crate::tls::testkit::serve(&dir, move |mut stream| {
            let request = crate::http::testkit::read_request(&mut stream);
            let Some(split) = request.windows(4).position(|w| w == b"\r\n\r\n") else { return };
            let (head, body) = (String::from_utf8_lossy(&request[..split]).into_owned(), &request[split + 4..]);
            let line = head.lines().next().unwrap_or_default();
            let sig = head.lines().find_map(|l| l.strip_prefix("X-Cca-Sig: ")).unwrap_or_default();
            seen.lock().unwrap().push(request.clone());
            let (status, text) = if line.contains("/v1/empty") {
                (404, String::new())
            } else if owed.load(Ordering::SeqCst) > 0 {
                owed.fetch_sub(1, Ordering::SeqCst);
                (429, obj! { "error" => "rate" }.compact())
            } else {
                let ok = b64_decode(sig).is_ok_and(|sig| ssh_verify(&public, "cca-presence", body, &sig));
                (200, obj! { "sig_ok" => ok, "line" => line, "got" => json::parse_bytes(body).unwrap_or(Value::Null) }.compact())
            };
            let _ = stream.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{text}", text.len()).as_bytes());
        })
    };
    let quick = Duration::from_millis(5);
    let extra = obj! { "get" => list!["a"], "name" => "书房的 Mac" };
    let (status, j) = post_retrying(&dir.join("dev"), port, "keys", &extra, quick).unwrap();
    assert_eq!((status, j["sig_ok"].bool(), j["line"].str()), (200, true, "POST /v1/keys HTTP/1.1"), "{j}");
    // body：extra 在前，后面是 v、op、key、ts、nonce
    let got = &j["got"];
    assert_eq!(got.obj().keys().collect::<Vec<_>>(), ["get", "name", "v", "op", "key", "ts", "nonce"]);
    assert_eq!((got["get"].clone(), got["name"].str(), got["v"].i64(), got["op"].str()), (list!["a"], "书房的 Mac", 1, "keys"));
    assert_eq!(got["key"].str(), fp_of(dir.join("dev.pub")));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    assert!((got["ts"].f64() - now).abs() < 60.0 && got["nonce"].str().len() == 32, "{got}");
    // 429 那次和重试的那次是同一份请求；头的先后照 http.client，body 里非 ASCII 转义
    let sent = seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    let text = String::from_utf8(sent[0].clone()).unwrap();
    assert!(text.is_ascii());
    assert!(
        text.starts_with(&format!(
            "POST /v1/keys HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept-Encoding: identity\r\nContent-Length: "
        )),
        "{text}"
    );
    assert!(text.contains("\r\nContent-Type: application/json\r\nX-Cca-Sig: "), "{text}");
    assert!(text.contains(&format!("\"name\":{}", json::dumps(&Value::from("书房的 Mac"), &Style::default()))), "{text}");
    // 一直 429：试 6 次，把最后一次的交回去
    owed.store(100, Ordering::SeqCst);
    seen.lock().unwrap().clear();
    assert_eq!(post_retrying(&dir.join("dev"), port, "keys", &extra, quick).unwrap(), (429, obj! { "error" => "rate" }));
    assert_eq!(seen.lock().unwrap().len(), 6);
    // 空的回应当成 {}
    assert_eq!(post_retrying(&dir.join("dev"), port, "empty", &obj! {}, quick).unwrap(), (404, obj! {}));
    // 没人听的端口：Err（post 那个写法是 panic）
    let closed = fsx::free_port();
    assert_eq!(try_post(dir.join("dev"), closed, "keys", &extra).unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
    assert!(catch_unwind(AssertUnwindSafe(|| post(dir.join("dev"), closed, "keys", &extra))).is_err());
}
