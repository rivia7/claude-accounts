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
    check(dirOpen(keys, pid: pid, sid: sid, d["ends"])?.canEnd == true && info?.canEnd == false && control?.canEnd == false,
          "说明里 end 是 true（远程启动的会话、能操作的配对）才结束得了")
    check(["ends_view", "ends_odd", "ends_one"].allSatisfy { dirOpen(keys, pid: pid, sid: sid, d[$0])?.canEnd == false },
          "只让看的、end 不是 true 的：结束不了")
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

// 会话推来的通知（NOTIFY 帧里的 JSON）：和辅助程序的 Note::parse 一样（attach.rs 的 notes_become_terminal_notifications 用的同一条）
let note = Note.parse(Bytes(utf8: #"{"title": "a\u001b]52;c;x\u0007b", "subtitle": "\u202eevil", "body": "line1\nline2"}"#))
check(note == Note(title: "a]52;c;xb", subtitle: "evil", body: "line1line2"), "通知：控制字符、双向控制字符去掉")
check(Note.parse(Bytes(utf8: #"{"title": " ", "subtitle": "只有副标题", "body": 3}"#)) == nil, "通知：没有标题也没有正文的不算")
check(Note.parse(Bytes(utf8: "做完了")) == nil && Note.parse(Bytes(utf8: #"["做完了"]"#)) == nil, "通知：不是 JSON 对象")
check(Note.parse(Bytes(utf8: #"{"body": "\#(String(repeating: "字", count: 300))"}"#))?.body.count == 240, "通知：正文最多 240 个字")
check(Note.parse(Bytes(utf8: #"{"title": "做完了", "id": "0123456789abcdef"}"#))?.id == "0123456789abcdef", "通知：带着编号")
check(Note.parse(Bytes(utf8: #"{"title": "做完了", "id": "0123456789ABCDEF"}"#))?.id == "", "通知：编号不是那个样子的当没有")
var budget = NoteBudget(now: 100)
check((0..<6).map { _ in budget.take(now: 100) } == [true, true, true, true, true, false], "通知的限流：一下子最多 5 条")
check(budget.take(now: 103) && !budget.take(now: 103), "通知的限流：之后每 3 秒一条")

// 推送的通知：kit::spec 封的（和辅助程序的 note_seal 一样），这边拿通知密钥解（像通知扩展那样每一把都试）
if let nt = v["note"] as? [String: Any] {
    let p = hx(nt["p"]), pid = nt["pid"] as? String ?? ""
    let key = NoteKey(pid: pid, p: p, name: "box\u{202e}")
    check(key.raw == hx(nt["key"]), "通知密钥：HMAC(P, cca-remote/v1/note)")
    let stranger = NoteKey(pid: nt["other_pid"] as? String ?? "", p: hx(nt["other_p"]), name: "别的")
    let w = nt["want"] as? [String: Any] ?? [:]
    let n = openPushed(["aps": ["mutable-content": 1], "n": nt["blob"] ?? NSNull()], keys: [stranger, key])
    check(n?.pid == pid && n?.machine == "box\u{202e}" && n?.id == w["id"] as? String && n?.sid == w["sid"] as? String
          && n?.at == jsonNumber(w["at"]) && n?.title == w["title"] as? String && n?.subtitle == w["subtitle"] as? String
          && n?.body == w["body"] as? String, "解开推送的通知：\(String(describing: n))")
    let card = n.flatMap { NoteCard($0) }
    check(card == NoteCard(title: w["title"] as? String ?? "", body: w["body"] as? String ?? "", machine: "box", what: w["subtitle"] as? String ?? "")
          && card?.subtitle == "来自「box」 · \(w["subtitle"] as? String ?? "")", "弹出来的字：\(String(describing: card))")
    check(openPushed(["n": nt["blob"] ?? NSNull()], keys: [stranger]) == nil, "别的配对的密钥解不开")
    check(openPushed(["n": nt["blob"] ?? NSNull()], keys: [NoteKey(pid: nt["other_pid"] as? String ?? "", raw: key.raw, name: "")]) == nil,
          "配对编号不对解不开")
    for bad in ["forged", "no_id", "bad_id", "bad_sid", "empty", "v2", "dir"] {
        check(openPushed(["n": nt[bad] ?? NSNull()], keys: [key]) == nil, "不认：\(bad)")
    }
    check(openPushed(["aps": [:] as [String: Any]], keys: [key]) == nil && openPushed(["n": "x"], keys: [key]) == nil, "推送里没有封好的那一份")
}
check(NoteCard(title: "", body: "", machine: "m", what: "w") == nil, "弹出来的字：标题、正文都是空的不弹")
check(NoteCard(title: "", body: "改好了", machine: "", what: "")?.title == "远程会话"
      && NoteCard(title: "", body: "改好了", machine: "", what: "")?.subtitle == "来自「另一台机器」", "弹出来的字：没写标题、不知道哪台机器")
check(NoteCard(title: "Claude Code 做完了", body: "", machine: "", what: "")?.sound == .done
      && NoteCard(title: "Codex 在等你", body: "改好了", machine: "", what: "")?.sound == .ask
      && NoteCard(title: "", body: "改好了", machine: "", what: "")?.sound == .ask
      && NoteCard(title: "做完了", body: "", machine: "", what: "", sound: .ask)?.sound == .ask && NoteSound.done.file == "done.caf",
      "提示音：标题以「做完了」结尾的响 done，别的响 ask")
check(plainLine("a\u{1b}b\u{2028}c\u{202e}d  ", max: 4) == "ab c", "plainLine：控制字符、看不见的排版字符去掉，截短")

// 账号的状态：kit::spec 封的（和辅助程序的 status_seal 一样），这边解开、读成模型；额度用完的规则和辅助程序 status.rs 的 exhausted 一样
if let st = v["status"] as? [String: Any] {
    let p = hx(st["p"]), pid = st["pid"] as? String ?? "", now = jsonNumber(st["now"]) ?? 0
    let w = st["want"] as? [String: Any] ?? [:]
    check(hmac(p, Bytes(utf8: "cca-remote/v1/status")) == hx(st["key"]), "账号的状态的密钥：HMAC(P, cca-remote/v1/status)")
    let s = statusOpen(statusKeys(p), pid: pid, st["blob"], age: 12, received: now)
    check(s?.machine == w["machine"] as? String && s?.more == jsonInt(w["more"]) && s?.at == jsonNumber(w["at"]) && s?.off == false
          && s?.age == 12 && s?.putAt == now - 12, "账号的状态：\(String(describing: s?.machine)) \(String(describing: s?.more))")
    check(s?.accounts.map(\.name) == w["names"] as? [String], "账号（编号不对的不要）：\(s?.accounts.map(\.name) ?? [])")
    let used = w["exhausted"] as? [String: Any] ?? [:]
    for a in s?.accounts ?? [] {
        let got = a.exhausted(now: now)
        let want = used[a.id] as? [[Any]] ?? []
        check(got.count == want.count && zip(got, want).allSatisfy { $0.slot.rawValue == $1[0] as? String && $0.resetsEpoch == jsonNumber($1[1]) },
              "额度用完：\(a.name) \(got)")
    }
    check(s?.accounts.filter { $0.nearLimit(now: now) }.map(\.name) == w["near"] as? [String], "快用完了（数据旧的不算）")
    if let a = s?.accounts.first, let f = w["first"] as? [String: Any] {
        check(a.aid == f["aid"] as? String && a.color == f["color"] as? String && a.badge == f["badge"] as? String && a.plan == f["plan"] as? String
              && a.tier == f["tier"] as? String && a.busy && a.kind == "oauth" && a.tool == "claude", "第一个账号：\(a)")
        check(a.usage?.fiveHour == UsageWindow(utilization: 100, resetsEpoch: jsonNumber(f["five_resets"]), reset: false)
              && a.usage?.sevenDay?.utilization == 41 && a.usage?.modelScoped.first?.name == "Fable"
              && a.usage?.modelScoped.first?.allowancePercentOfWeekly == 50 && a.usage?.source == "live",
              "第一个账号的用量：\(String(describing: a.usage))")
        check(a.weeks?.weeks.count == 2 && a.weeks?.weeks.last?.current == true && a.weeks?.weeks.first?.usedPercent == 63
              && a.weeks?.summary.averagePercent == 63 && a.weeks?.summary.weeks == 1, "每周用量按时间排好：\(String(describing: a.weeks))")
    }
    let gem = s?.accounts.first { $0.tool == "agy" }
    check(gem?.usage?.quotaGroups.map(\.displayName) == w["groups"] as? [String] && gem?.usage?.hasQuota == true
          && gem?.usage?.quotaMost(.fiveHour, now: now)?.group == "Gemini 模型", "Antigravity 的几组：\(gem?.usage?.quotaGroups.map(\.displayName) ?? [])")
    check(gem?.weeksGroups?.map(\.displayName) == w["group_weeks"] as? [String] && gem?.weeksGroups?.map(\.id) == ["gemini-weekly", "3p-weekly"]
          && gem?.weeksGroups?.first?.weeks.weeks.first?.usedPercent == 52 && gem?.weeksGroups?.first?.weeks.weeks.first?.current == true
          && gem?.weeksGroups?.last?.weeks.weeks.isEmpty == true && s?.accounts.first?.weeksGroups == nil,
          "Antigravity 每组的每周用量（没有 id 的不要，别的工具没有）：\(String(describing: gem?.weeksGroups))")
    // 在用什么、折合金额（status.rs 的 row_of、cost_of）：按周、按天的起止照最后一项结束的时间和各项的小时数首尾相接推出来
    let wc = w["cost"] as? [String: Any] ?? [:]
    let we = jsonNumber(wc["week_end"]) ?? 0, de = jsonNumber(wc["day_end"]) ?? 0
    let first = s?.accounts.first
    check(first?.appOpen == true && first?.cliSessions == 2 && first?.cliSince == jsonNumber(wc["cli_since"]), "在用什么：\(String(describing: first))")
    if let c = first?.cost {
        check(c.weeks.map(\.cents) == [0, 1250, 4800, 483] && c.weeks.map(\.partial) == [true, true, false, false]
              && c.weeks.map(\.current) == [false, false, false, true] && c.weeks.last?.endEpoch == we && c.weeks[2].startEpoch == we - 288 * 3600
              && c.weeks[2].endEpoch == we - 168 * 3600 && c.weeks[0].startEpoch == we - 624 * 3600, "按周的几项（有一项是 120 小时）：\(c.weeks)")
        check(c.days.map(\.cents) == [120, 0, 377] && c.days.first?.startEpoch == de - 72 * 3600 && c.days.last?.current == true
              && !c.days.contains(where: \.partial), "按天的几项：\(c.days)")
        check(c.weeksTotal == 6533 && c.daysTotal == 497 && c.averageWeek == 4800 && c.averageDay == nil && c.averageWeeks == 1 && c.averageDays == 0
              && c.cacheHitWeeks == 0.973 && c.cacheHitDays == nil && c.current(days: false) == 483 && c.current(days: true) == 377,
              "合计、平均、缓存命中：\(c)")
        check(c.sync == "error" && c.missing == 2, "同步的状态、没算进来的设备")
        check(c.models?.map(\.name) == ["claude-opus-5-5", nil, "claude-haiku-5-6"] && c.models?.map(\.pricedAs) == [nil, nil, "claude-haiku-5-5"]
              && c.models?.first?.weeks == 5000 && c.models?.first?.days == 400, "按模型的：\(String(describing: c.models))")
        check(c.devices.map(\.name) == ["studio-mac", "office-imac"] && c.devices.map(\.me) == [true, false] && c.devices.map(\.stale) == [false, true]
              && c.devices.first?.syncedEpoch == now - 60 && c.devices.last?.syncedEpoch == nil, "按设备的（少几项的不要，名字清理过）：\(c.devices)")
    } else {
        check(false, "第一个账号带着折合金额")
    }
    let bob = s?.accounts.first { $0.name == "bob" }
    check(bob != nil && bob?.cost == nil && bob?.appOpen == false && bob?.cliSessions == 0 && bob?.cliSince == nil, "没有折合金额、没在用的账号")
    // 出口检测、为什么用不了、第三方 API 的模型、哪个 Team 给的、套餐过时（status.rs 的 net_of、why_of、row_of、usage_of）：清理过的样子，
    // App 照它算的那几样（AccountMood 用的：算数的那一份、出口有问题、可能泄露）
    let wm = st["want_more"] as? [String: Any] ?? [:]
    let an = first?.net
    check(an == AccountNet(result: "restricted", region: wm["region"] as? String, trust: 40, trustLabel: "可疑", dns: "leak", webrtc: "note",
                           checkedEpoch: now - 101, atLaunch: true)
          && an?.current != nil && an?.exitBad == true && an?.hasLeak == true, "出口检测：\(String(describing: an))")
    check(first?.whyApp == "app_missing" && first?.whyCLI == "tool_missing" && first?.team == wm["team"] as? String
          && first?.usage?.outdatedLoginPlan == wm["outdated"] as? String && first?.model == nil, "为什么用不了、Team、套餐过时：\(String(describing: first))")
    check(bob?.net == AccountNet(result: "") && bob?.net?.checked == false && bob?.net?.current == nil && bob?.model == wm["model"] as? String
          && bob?.team == nil && bob?.whyApp == nil && bob?.whyCLI == nil, "从没测过出口（{}）、第三方 API 的模型：\(String(describing: bob?.net))")
    let carol = s?.accounts.first { $0.name == "carol" }
    check(first?.port == "closed" && bob?.port == nil && carol?.port == nil, "端口池的实况：只认 closed、drift（\(String(describing: first?.port))）")
    check(carol?.net?.checked == true && carol?.net?.stale == true && carol?.net?.applies == false && carol?.net?.current == nil
          && carol?.net?.trust == 100 && carol?.net?.exitBad == false, "过时的、换了节点测的不算数（纯净度到 100 为止）：\(String(describing: carol?.net))")
    check(s?.accounts.first { $0.name == "dave" }?.net == nil, "旧版的宿主不给 net")
    // 那台电脑：cca 的版本、「远程会话」的开关、这台手机能不能操作、页顶的提示（host_of）
    let probs = s?.problems ?? []
    check(s?.ccaVersion == wm["cca"] as? String && s?.update == wm["update"] as? String && s?.remoteOn == true && s?.allow == "view"
          && probs.map(\.code) == ["table_invalid", "settings_problems", "usage_consent", "core_down"]
          && probs.map(\.tool) == ["claude", "codex", "codex", nil] && probs.first?.items == [] && probs.dropFirst().first?.items == wm["items"] as? [String],
          "那台电脑那几样：\(String(describing: s?.ccaVersion)) \(String(describing: s?.update)) \(probs)")
    // 那台电脑的折合金额那几样（cost_meta）、这个配对所在的 Team（team_of）；放不下时不带的
    check(s?.cost?.pricesDate == "2026-11-01" && s?.cost?.updatedEpoch == now - 240 && s?.cost?.unattributed == 312 && s?.cost?.brief == true
          && s?.cost?.scanning == false && s?.costLeftOut == false && s?.weeksLeftOut == false, "折合金额那几样：\(String(describing: s?.cost))")
    let team = s?.team
    check(team?.name == "示例 Team" && team?.isAdmin == true && team?.isOwner == false && team?.state == "error" && team?.error == "连不上服务器"
          && team?.seq == 12 && team?.accounts == 3 && team?.device == "studio-mac" && team?.host == "team.example.com:18445"
          && team?.joinedEpoch == now - 86400 && team?.update == "服务器不认 status_put", "Team：\(String(describing: team))")
    let lean = statusOpen(statusKeys(p), pid: pid, st["lean"], age: 0, received: now)
    check(lean != nil && lean?.cost == nil && lean?.costLeftOut == true && lean?.weeksLeftOut == true && lean?.team == nil,
          "放不下时不带折合金额、每周用量：\(String(describing: lean?.costLeftOut))")
    check(lean?.ccaVersion == wm["cca"] as? String && lean?.update == wm["update"] as? String && lean?.remoteOn == true && lean?.allow == "view"
          && lean?.problems.count == 4, "放不下时那台电脑那几样照样带着")
    check(statusOpen(statusKeys(p), pid: st["other_pid"] as? String ?? "", st["blob"], age: 0, received: now) == nil, "配对编号不对解不开")
    for bad in ["forged", "dir", "note", "v2"] {
        check(statusOpen(statusKeys(p), pid: pid, st[bad], age: 0, received: now) == nil, "账号的状态不认：\(bad)")
    }
}

// 额度的通知：kit::spec 封的（和辅助程序的 usage_note 一样，kind 是 usage）；字在这里照窗口、重置时间写（时间用 UTC）
if let un = v["usage"] as? [String: Any] {
    let p = hx(un["p"]), pid = un["pid"] as? String ?? "", now = jsonNumber(un["now"]) ?? 0
    let key = NoteKey(pid: pid, p: p, name: "Mac mini")
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC")!
    let want = un["want"] as? [String: Any] ?? [:]
    for k in ["used", "used_tomorrow", "back"] {
        let n = openPushed(["n": un[k] ?? NSNull()], keys: [key])
        let card = n.flatMap { NoteCard($0, now: now, calendar: utc) }
        check(n?.sid == "" && n?.usage != nil && card.map { [$0.title, $0.subtitle, $0.body] } == want[k] as? [String],
              "额度的通知 \(k)：\(String(describing: card))")
        check(card?.sound == (k == "back" ? NoteSound.done : NoteSound.ask), "额度的通知 \(k) 的提示音：\(String(describing: card?.sound))")
    }
    let u = openPushed(["n": un["used"] ?? NSNull()], keys: [key])?.usage
    check(u?.event == .usedUp && u?.account == un["account"] as? String && u?.aid == un["aid"] as? String && u?.slot == .fiveHour
          && u?.tool == "claude" && u?.name == "alice" && u?.resetsEpoch == jsonNumber(un["resets"]), "额度的通知：\(String(describing: u))")
    check(openPushed(["n": un["back"] ?? NSNull()], keys: [key])?.usage?.event == .back, "恢复的那一条")
    for bad in ["bad_window", "other_kind", "bad_account", "no_event"] {
        check(openPushed(["n": un[bad] ?? NSNull()], keys: [key]) == nil, "额度的通知不认：\(bad)")
    }
}

// 远程启动：启动器的说明（kit::spec 封的，和辅助程序的 launch_seal 一样），这边解开、清理；LAUNCH 帧这边出（Rust 那头核对），
// LAUNCHED 的回话这边读
if let ln = v["launch"] as? [String: Any] {
    let p = hx(ln["p"]), pid = ln["pid"] as? String ?? "", sid = ln["sid"] as? String ?? ""
    let keys = launchKeys(p)
    let info = launchOpen(keys, pid: pid, sid: sid, ln["blob"])
    let w = ln["want"] as? [String: Any] ?? [:]
    let wt = w["t"] as? [[String: Any]] ?? []
    check(info != nil && info?.machine == w["machine"] as? String && info?.more == jsonInt(w["more"]) && info?.templates.count == wt.count,
          "启动器的说明：\(String(describing: info?.machine)) \(String(describing: info?.more)) \(info?.templates.count ?? -1) 个模板")
    for (t, x) in zip(info?.templates ?? [], wt) {
        check(t.id == x["i"] as? String && t.digest == x["h"] as? String && t.name == x["n"] as? String && t.tool == x["k"] as? String
              && t.account == x["a"] as? String && t.aid == (x["aid"] as? String ?? "") && t.dir == x["d"] as? String
              && t.args == x["x"] as? String && t.line == jsonBool(x["l"]), "模板：\(t)")
    }
    // 能挑的账号（3.7.10 起）：编号、工具不对的、重复的丢掉，名字清理过
    let wa = w["accounts"] as? [[String: Any]] ?? []
    check(info?.accounts?.count == wa.count, "能挑的账号：\(info?.accounts?.count ?? -1) 个")
    for (a, x) in zip(info?.accounts ?? [], wa) {
        check(a.id == x["i"] as? String && a.tool == x["k"] as? String && a.name == x["n"] as? String && a.aid == (x["aid"] as? String ?? ""),
              "能挑的账号：\(a)")
    }
    if let t0 = info?.templates.first {
        check(info?.accounts(for: t0)?.map(\.id) == wa.filter { $0["k"] as? String == t0.tool }.map { $0["i"] as? String ?? "" },
              "按模板的工具挑账号")
    }
    if let old = ln["old_blob"] {
        let o = launchOpen(keys, pid: pid, sid: sid, old)
        check(o != nil && o?.accounts == nil && o.flatMap { i in i.templates.first.flatMap { i.accounts(for: $0) } } == nil,
              "旧版的 cca 的说明：没有 accounts（模板用它自己的账号，不用挑）")
    }
    check(launchOpen(keys, pid: pid, sid: sid, ln["forged"]) == nil, "启动器的说明：别的密钥封的不认")
    check(launchOpen(keys, pid: pid, sid: sid, ln["dir"]) == nil, "启动器的说明：会话的说明不是它")
    check(dirOpen(dirKeys(p), pid: pid, sid: sid, ln["blob"]) == nil, "启动器的说明不是会话的说明（旧的 App 当它不存在）")
    check(launchOpen(keys, pid: pid, sid: ln["other_sid"] as? String ?? "", ln["blob"]) == nil, "启动器的说明：编号不对解不开")
    if let a = ln["ask"] as? [String: Any] {
        let id = a["t"] as? String ?? "", h = a["h"] as? String ?? "", c = jsonInt(a["c"]) ?? 0, r = jsonInt(a["r"]) ?? 0
        out["launch_frame"] = LaunchAsk.frame(id: id, digest: h, cols: c, rows: r, line: a["l"] as? String, account: a["a"] as? String)?.hex ?? ""
        out["launch_frame_plain"] = LaunchAsk.frame(id: id, digest: h, cols: c, rows: r)?.hex ?? ""
        for bad in ln["bad_lines"] as? [String] ?? [] {
            check(LaunchAsk.frame(id: id, digest: h, cols: c, rows: r, line: bad) == nil, "开场白不能发：\(bad.debugDescription.prefix(40))")
        }
    }
    for row in ln["launched"] as? [[String: Any]] ?? [] {
        let payload = hx(row["payload_hex"])
        var want: LaunchResult?
        if let x = row["want"] as? [String: Any] {
            want = jsonBool(x["ok"]) == true ? .launched(sid: x["sid"] as? String ?? "")
                : .refused(code: x["code"] as? String ?? "", text: x["text"] as? String ?? "")
        }
        let got = LaunchResult.parse(payload)
        check(got == want, "LAUNCHED \(String(decoding: payload, as: UTF8.self).prefix(80))：\(String(describing: got))")
    }
}
// LAUNCH 帧（不靠输入的几条）：键的先后、转义、范围，和辅助程序 launch.rs 的 Ask::parse 认的一样
let askOK = LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: 80, rows: 24)
check(askOK.map { String(decoding: $0, as: UTF8.self) } == #"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24}"#, "LAUNCH 的内容")
check(LaunchAsk.frame(id: "0123abcd", digest: "0123456789abcdef", cols: 80, rows: 24) == askOK.map { [Frame.launch] + u32be($0.count) + $0 }
      && Frame.launch == 0x09 && Frame.launched == 0x18, "LAUNCH 帧")
check(LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: 20, rows: 300, line: #"看 "a\b" 好"#)
        .map { String(decoding: $0, as: UTF8.self) } == #"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":20,"r":300,"l":"看 \"a\\b\" 好"}"#,
      "LAUNCH 的开场白：引号、反斜杠转义，非 ASCII 原样")
check([(19, 24), (501, 24), (80, 4), (80, 301)].allSatisfy { LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: $0.0, rows: $0.1) == nil },
      "LAUNCH：大小不在范围里不发")
check(LaunchAsk.payload(id: "0123ABCD", digest: "0123456789abcdef", cols: 80, rows: 24) == nil
      && LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcde", cols: 80, rows: 24) == nil, "LAUNCH：编号、摘要不对不发")
check(LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: 80, rows: 24, line: "好", account: "89abcdef01234567")
        .map { String(decoding: $0, as: UTF8.self) } == #"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"l":"好","a":"89abcdef01234567"}"#
      && LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: 80, rows: 24, account: "89abcdef01234567")
        .map { String(decoding: $0, as: UTF8.self) } == #"{"v":1,"t":"0123abcd","h":"0123456789abcdef","c":80,"r":24,"a":"89abcdef01234567"}"#,
      "LAUNCH 的账号：a 放在最后（开场白后面）")
check(["89ABCDEF01234567", "89abcdef0123456", "89abcdef012345678", "", "zzzzzzzzzzzzzzzz"].allSatisfy {
    LaunchAsk.payload(id: "0123abcd", digest: "0123456789abcdef", cols: 80, rows: 24, account: $0) == nil
}, "LAUNCH：账号的编号不是 16 个小写的十六进制字符不发")
check(LaunchAsk.lineOK(String(repeating: "字", count: 500)) && !LaunchAsk.lineOK(String(repeating: "字", count: 501)) && !LaunchAsk.lineOK("")
      && !LaunchAsk.lineOK(" \u{3000}") && !LaunchAsk.lineOK("a\u{7f}") && !LaunchAsk.lineOK("a\u{85}b") && LaunchAsk.lineOK("a\u{2028}b"),
      "开场白：500 个字以内、不是空白、没有控制字符")
check(LaunchAsk.clamp(cols: 9999, rows: 1) == (500, 5) && LaunchAsk.clamp(cols: 3, rows: 900) == (20, 300), "新会话的大小夹到范围里")
let newSid = String(repeating: "ab", count: 16)
check(LaunchResult.parse(Bytes(utf8: #"{"v":1,"ok":true,"sid":"\#(newSid)"}"#)) == .launched(sid: newSid)
      && LaunchResult.parse(Bytes(utf8: #"{"v":1,"ok":false,"code":"no_dir","text":" 文件夹\u001b没了 "}"#)) == .refused(code: "no_dir", text: "文件夹没了")
      && LaunchResult.parse(Bytes(utf8: #"{"v":1,"ok":false,"code":"spawn"}"#)) == .refused(code: "spawn", text: ""),
      "LAUNCHED：起来了、没起（原因清理过）")
for bad in [#"{"v":2,"ok":true,"sid":"\#(newSid)"}"#, #"{"v":1,"ok":1,"sid":"\#(newSid)"}"#, #"{"v":1,"ok":true,"sid":"\#(newSid.uppercased())"}"#,
            #"{"v":1,"ok":false,"code":"No-Dir"}"#, #"{"v":1,"ok":false,"code":"no_dir\n"}"#, #"{"v":1,"ok":false}"#, #"["ok"]"#, "起来了"] {
    check(LaunchResult.parse(Bytes(utf8: bad)) == nil, "LAUNCHED 不认：\(bad)")
}

// 签名：Rust 那头拿 ssh-keygen -Y verify 核对
if let s = v["sign"] as? [String: Any], let id = Identity(raw: hx(s["raw"])) {
    out["sign"] = ["pub": id.pub, "fp": id.fp, "sig": id.sign(hx(s["data"]))]
}

let data = try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
FileManager.default.createFile(atPath: args[2], contents: data)
// 会话的先后（SessionOrder，App 的「远程会话」页拖动排的）：记着的在前、照记着的先后，没记着的接在后面、照原来的先后；排过的一组写在
// 最前面；最多记 limit 个（多了的从最后去掉）。和 Mac App 的 RemoteOrder 同一个规矩（macos/Tests/ContractTests/RemoteOrderTests.swift）
do {
    var o = SessionOrder(ids: ["c", "a", "", "c", "b"])
    check(o.ids == ["c", "a", "b"], "会话的先后：去掉重复的、空的")
    check(o.sorted(["d", "b", "e", "a"], id: { $0 }) == ["a", "b", "d", "e"], "会话的先后：记着的照记着的先后，没记着的接在后面")
    check(SessionOrder().sorted(["d", "b", "e"], id: { $0 }) == ["d", "b", "e"], "会话的先后：没排过的照原样")
    o.record(["b", "a"])
    check(o.ids == ["b", "a", "c"] && o.sorted(["a", "c", "b"], id: { $0 }) == ["b", "a", "c"], "会话的先后：排过的一组写在最前面")
    var big = SessionOrder(ids: (0..<500).map { String($0) })
    check(big.ids.count == SessionOrder.limit && big.ids.last == String(SessionOrder.limit - 1), "会话的先后：最多记 \(SessionOrder.limit) 个")
    big.record(["x"])
    check(big.ids.count == SessionOrder.limit && big.ids.first == "x" && big.ids.last == String(SessionOrder.limit - 2),
          "会话的先后：多了的从最后去掉")
}

print("通过 \(passed)，失败 \(failed)")
exit(failed == 0 ? 0 : 1)
