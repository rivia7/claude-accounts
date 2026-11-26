import CcaRemoteKit
import Foundation

/// 界面上的文字：说法照 Mac App（macos/Sources/CcaKit/Model/Texts.swift 的「远程会话」那一段），照实说、不用比喻
enum Texts {
    static func nonEmpty(_ s: String?) -> String? {
        guard let s, !s.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return s
    }

    /// 别的机器上的程序写的一行字（终端的标题）显示之前：控制字符、看不见的排版字符去掉，截短（CcaRemoteKit 的 plainLine：
    /// 通知扩展弹推来的通知也用它）
    static func plainLine(_ s: String, max: Int) -> String {
        CcaRemoteKit.plainLine(s, max: max)
    }

    /// 会话里跑的是哪个命令行工具
    static func cliProduct(_ tool: String) -> String? {
        switch tool {
        case "claude": return "Claude Code"
        case "codex": return "Codex"
        case "agy": return "Antigravity"
        case "": return nil
        default: return tool
        }
    }

    /// 一个会话的名字：「Claude Code · 工作」
    static func sessionTitle(_ s: SessionInfo) -> String {
        [cliProduct(s.tool), nonEmpty(s.name)].compactMap { $0 }.joined(separator: " · ")
    }

    static func sessionTitle(_ s: RemoteSession) -> String {
        nonEmpty(sessionTitle(s.info)) ?? "会话 \(s.sid.prefix(6))"
    }

    /// 会话的第二行：「~/srv/api · 开了 2 小时 · 另有 1 处接着」
    static func sessionDetail(_ s: RemoteSession, now: Double) -> String {
        var parts: [String] = []
        if let d = nonEmpty(s.info.cwd) { parts.append(d) }
        if s.info.since > 0 { parts.append("开了 " + span(now - s.info.since)) }
        if s.viewers > 0 { parts.append("另有 \(s.viewers) 处接着") }
        return parts.joined(separator: " · ")
    }

    static func span(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if m < 1 { return "不到 1 分钟" }
        if m < 60 { return "\(m) 分钟" }
        let h = m / 60, mm = m % 60
        if h < 24 { return mm > 0 ? "\(h) 小时 \(mm) 分" : "\(h) 小时" }
        let d = h / 24, hh = h % 24
        return hh > 0 ? "\(d) 天 \(hh) 小时" : "\(d) 天"
    }

    static func agoPhrase(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if seconds < 60 { return "刚刚" }
        if m < 60 { return "\(m) 分钟前" }
        if m < 1440 { return "\(m / 60) 小时前" }
        return "\(m / 1440) 天前"
    }

    /// 配对过的电脑的第二行：「11 月 3 日配对 · 示例 Team」
    static func peerDetail(_ p: Peer, now: Double) -> String {
        let c = Calendar.current.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: p.at))
        let thisYear = Calendar.current.component(.year, from: Date(timeIntervalSince1970: now))
        let day = c.year == thisYear ? "\(c.month ?? 0) 月 \(c.day ?? 0) 日" : "\(c.year ?? 0) 年 \(c.month ?? 0) 月 \(c.day ?? 0) 日"
        return [day + "配对", p.team].compactMap { $0 }.joined(separator: " · ")
    }

    // MARK: 出了什么事

    /// 配对没成：RemoteFailure 的 code → 一句话
    static func pairFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器的证书和二维码里的对不上：可能连到了别的服务器。换个网络再扫一次；还是这样，请那台电脑的主人看看服务器。"
        case "unreachable": return "连不上服务器。看看网络，再扫一次。"
        case "no_pairing": return "这个二维码已经用过或者过期了。在电脑上重新点「和手机配对…」，扫新的二维码。"
        case "no_answer": return "那台电脑一直没有回应。电脑上的配对窗口还开着吗？重新点「和手机配对…」再扫一次。"
        case "bad_peer": return "对方的回答验证不过，没有配对。重新扫一次。"
        case "rate", "busy": return "服务器这会儿太忙，过一会儿再试。"
        case "clock": return "这台手机的时间和服务器差太多。打开「设置」→「通用」→「日期与时间」里的「自动设置」。"
        case "edges": return "配对的设备太多了（一台最多 16 个），先在电脑上取消几个不用的配对。"
        case "mismatch": return "两边算出来的配对对不上，没有配对。在电脑上重新点「和手机配对…」再扫一次。"
        case "full": return "服务器存不下了，请那台电脑的主人看看服务器。"
        default: return "没配对成功（\(code)）。重新扫一次。"
        }
    }

    /// 没列出会话：一台电脑所在的服务器没问到
    static func listFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器的证书变了，和配对时记下的对不上"
        case "unreachable": return "连不上服务器"
        case "denied", "unknown_device": return "服务器不认这台手机了：配对可能已经取消"
        case "no_key": return "这台手机上没有这个配对的密钥，要重新配对"
        case "rate", "busy": return "服务器这会儿太忙"
        case "clock": return "这台手机的时间和服务器差太多"
        default: return "没问到（\(code)）"
        }
    }

    /// 终端那一页顶上的那句话：连接的状态
    static func linkState(_ s: Attachment.State) -> String? {
        switch s {
        case .connecting: return "正在接入…"
        case .open: return nil
        case .retrying: return "连接断了，正在重新接上…"
        case .closed(let why): return closed(why)
        }
    }

    static func closed(_ why: String) -> String {
        switch why {
        case "detached": return "已断开，会话在那边照常运行"
        case "ended": return "会话已经结束"
        case "gone": return "没有这个会话了（可能已经结束）"
        case "unpaired": return "那台电脑取消了配对，接入不了了"
        case "gave_up": return "连接断了，一直没有再接上"
        case "refused": return "那台电脑不让这台手机接入这个会话"
        case "version": return "那台电脑上的 cca 版本不一样，先把两边都更新"
        case "auth": return "对方证明不了自己是配对的那台电脑：已经断开，什么都没发给它"
        case "nohost": return "那台电脑这会儿没连着服务器"
        case "unreachable": return "连不上服务器的数据端口"
        case "busy", "rate", "sessions": return "服务器这会儿太忙，过一会儿再试"
        case "ticket": return "服务器没认这次接入，再试一次"
        default: return "没接上（\(why)）"
        }
    }
}
