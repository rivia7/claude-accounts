// CcaRemoteKit 的对照测试：tests/ios_kit.rs 用 tests/kit/spec.rs（照 server/README.md 独立写的那一份）出数据，
// 这里逐项核对；这一份算出来的（封的配对消息、发的记录、签名）写到输出文件里，再由 Rust 那头核对。
// 用法：kit-tests 输入.json 输出.json。和 CcaRemoteKit 的源码编在同一个模块里（要用到 internal 的函数）
import Foundation

var failed = 0
var passed = 0

func check(_ ok: Bool, _ what: String) {
    if ok {
        passed += 1
    } else {
        failed += 1
        print("FAIL \(what)")
    }
}

/// 收一条记录的结果：short（还不够一条）、throws（该断开）、内容的十六进制
func opened(_ ch: Channel, _ b: Bytes) -> String {
    do {
        guard let r = try ch.open(b[...]) else { return "short" }
        return r.data.hex
    } catch {
        return "throws"
    }
}

func hx(_ x: Any?) -> Bytes {
    Bytes.unhex(x as? String ?? "") ?? []
}

let args = CommandLine.arguments
guard args.count == 3, let raw = FileManager.default.contents(atPath: args[1]),
      let v = (try? JSONSerialization.jsonObject(with: raw)) as? [String: Any]
else {
    print("用法：kit-tests 输入.json 输出.json")
    exit(64)
}
var out = [String: Any]()

// 配对码
for c in v["codes"] as? [[String: Any]] ?? [] {
    let b = hx(c["hex"])
    let text = c["text"] as? String ?? ""
    check(codeText(b) == text, "codeText \(text)")
    check(codeBytes(text) == b, "codeBytes \(text)")
    check(codeBytes(text.lowercased().replacingOccurrences(of: "-", with: " ")) == b, "codeBytes 小写、空格 \(text)")
    check(pairKeys(b).rid == c["rid"] as? String, "rid \(text)")
}
check(codeBytes("IL0O-0000-0000-0000") == codeBytes("1100-0000-0000-0000"), "I L 当 1、O 当 0")
for bad in v["bad_codes"] as? [String] ?? [] {
    check(codeBytes(bad) == nil, "不认 \(bad)")
}

// 配对：电脑（a）那一份 kit::spec 封的，这边解开；这边（b）封一份给 kit::spec 解；两边算出同一把 P
if let p = v["pair"] as? [String: Any] {
    let k = pairKeys(hx(p["code"]))
    let rid = k.rid
    let opened = pairOpen(k.keys, rid: rid, role: "a", p["msg_a"])
    check(opened?.fp == p["fp_a"] as? String && opened?.n == hx(p["n_a"]) && opened?.name == p["name_a"] as? String, "解开电脑那一份")
    check(pairOpen(k.keys, rid: rid, role: "b", p["msg_a"]) == nil, "角色不对解不开")
    check(pairOpen(pairKeys(hx(p["other_code"])).keys, rid: rid, role: "a", p["msg_a"]) == nil, "配对码不对解不开")
    let s = pairSecret(k.kc, na: hx(p["n_a"]), nb: hx(p["nb"]), fpa: p["fp_a"] as? String ?? "", fpb: p["fp_b"] as? String ?? "")
    check(s.p == hx(p["p"]) && s.pid == p["pid"] as? String, "P 和配对编号")
    out["msg_b"] = pairMsg(k.keys, rid: rid, role: "b", fp: p["fp_b"] as? String ?? "", nonce: hx(p["nb"]), name: "我的 iPhone")
}

// 会话的说明
if let d = v["dir"] as? [String: Any] {
    let keys = dirKeys(hx(d["p"]))
    let pid = d["pid"] as? String ?? "", sid = d["sid"] as? String ?? ""
    let info = dirOpen(keys, pid: pid, sid: sid, d["blob"])
    let w = d["want"] as? [String: Any] ?? [:]
    check(info?.machine == w["machine"] as? String && info?.tool == w["tool"] as? String && info?.name == w["name"] as? String
          && info?.cwd == w["cwd"] as? String && info?.since == jsonNumber(w["since"]) && info?.viewOnly == (w["view"] as? Bool),
          "会话的说明：\(String(describing: info))")
    check(dirOpen(keys, pid: pid, sid: String(sid.reversed()), d["blob"]) == nil, "会话编号不对解不开")
    check(dirOpen(keys, pid: pid, sid: sid, d["forged"]) == nil, "别的密钥封的不认")
    let control = dirOpen(keys, pid: pid, sid: sid, d["control"])
    check(control?.viewOnly == false && control?.since == 0, "没写 access 是能操作；since 不是数是 0")
}

// 握手和记录：kit::spec 扮演宿主
if let c = v["chan"] as? [String: Any], let ch = Channel(p: hx(c["p"]), pid: c["pid"] as? String ?? "", sid: c["sid"] as? String ?? "",
                                                           fpH: c["fph"] as? String ?? "", fpV: c["fpv"] as? String ?? "") {
    check(ch.hello(nv: hx(c["nv"])) == hx(c["hello"]), "hello")
    var bad = hx(c["reply"])
    bad[40] ^= 1
    let probe = Channel(p: hx(c["p"]), pid: c["pid"] as? String ?? "", sid: c["sid"] as? String ?? "", fpH: c["fph"] as? String ?? "",
                        fpV: c["fpv"] as? String ?? "")!
    _ = probe.hello(nv: hx(c["nv"]))
    check(probe.confirm(bad) == nil, "宿主的证明不对：不回确认")
    check(ch.confirm(hx(c["reply"])) == hx(c["confirm"]), "确认")
    var buf = Bytes()
    for r in c["records"] as? [String] ?? [] {
        buf += hx(r)
    }
    var at = 0
    var got = [Int]()
    var allOK = true
    do {
        while let rec = try ch.open(buf[at...]) {
            at += rec.used
            got.append(rec.data.count)
            allOK = allOK && rec.data == (0..<rec.data.count).map { UInt8(truncatingIfNeeded: $0 * 7) }
        }
    } catch {
        allOK = false
    }
    let lens = (c["lens"] as? [Int]) ?? []
    check(got == lens && allOK && at == buf.count, "收宿主的记录 \(got)")
    // 一半的一条：还不够
    let short = Bytes(hx(c["tail"]).prefix(20))
    check(opened(ch, short) == "short", "不够一条")
    var tampered = hx(c["tail"])
    tampered[12] ^= 1
    check(opened(ch, tampered) == "throws", "改过的记录：断开")
    let first = hx((c["records"] as? [String])?.first)
    check(opened(ch, first) == "throws", "重放的（序号不对）：断开")
    check(opened(ch, hx(c["tail"])) == "09", "接着的那一条照收")
    out["records"] = lens.map { n in (try? ch.seal((0..<n).map { UInt8(truncatingIfNeeded: $0 * 7) }))?.hex ?? "" }
    check((try? ch.seal(Bytes(repeating: 0, count: 16385))) == nil, "超长的不封")
}

// 开场白
if let r = v["relay"] as? [String: Any] {
    let l = relayLine(tid: r["tid"] as? String ?? "", tkey: hx(r["tkey"]), cn: r["cn"] as? String)
    check(l.line == r["line"] as? String && l.expect + "\n" == r["expect"] as? String, "开场白")
}

// 帧
check(Frame.make(Frame.hello, [1, 0] + Frame.size(80, 24)) == [1, 0, 0, 0, 6, 1, 0, 0, 80, 0, 24], "HELLO 帧")
check(Frame.validSize(2, 1000) && !Frame.validSize(1, 24) && !Frame.validSize(80, 1001), "窗口大小的范围")

// 证书的公钥指纹（和 openssl 算的一样）
for s in v["spki"] as? [[String: Any]] ?? [] {
    check(pinOf(certificate: hx(s["der"])) == s["pin"] as? String, "公钥指纹 \(s["what"] ?? "")")
}
check(pinOf(certificate: [0x30, 0x03, 0x02, 0x01, 0x01]) == nil, "不是证书")

// 二维码里的链接
for l in v["links"] as? [[String: Any]] ?? [] {
    let inv = PairInvite(l["link"] as? String ?? "")
    check(inv?.server.url == l["url"] as? String && inv?.server.pins == l["pins"] as? [String] && inv.map { codeText($0.code) } == l["code"] as? String
          && inv?.machine == l["name"] as? String && inv?.team == l["team"] as? String, "链接 \(l["link"] ?? "")")
}
for bad in v["bad_links"] as? [String] ?? [] {
    check(PairInvite(bad) == nil, "不认的链接 \(bad)")
}

// 文字的清理
check(clean("a\u{1b}[31mb\u{202e}c\u{7f}", 48) == "a[31mbc", "去掉控制字符")
check(clean("  一二三四五  ", 5) == "一二三", "先按码位限长、再去掉两头的空白（和 core.js 的 clean 一样）")
check(clean(42, 10) == "", "不是字符串")

// 签名：Rust 那头拿 ssh-keygen -Y verify 核对
if let s = v["sign"] as? [String: Any], let id = Identity(raw: hx(s["raw"])) {
    out["sign"] = ["pub": id.pub, "fp": id.fp, "sig": id.sign(hx(s["data"]))]
}

let data = try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
FileManager.default.createFile(atPath: args[2], contents: data)
print("通过 \(passed)，失败 \(failed)")
exit(failed == 0 ? 0 : 1)
