import CcaRemoteKit
import Foundation

/// 「账号」页推进去的一个账号：哪台电脑（配对编号）上的哪个账号（这个配对里认这个账号的编号，AccountStatus.id）
struct AccountRef: Hashable {
    let pid: String
    let id: String
}

/// 挡住一个账号的问题（Mac App 的 Employee.Mood 里放上来的状态看得出的那几个；端口、出口的问题手机上不知道）
enum AccountTrouble: Equatable {
    case loggedOut
    case wrongAccount
    case expired
    /// 5 小时或本周的额度用完了：until 是用完的窗口里最晚重置的时间
    case usedUp(until: Double?)

    /// 要人动手的（额度用完只是等）
    var needsAttention: Bool {
        if case .usedUp = self { return false }
        return true
    }
}

/// 一个账号现在怎么样，照 Mac App 的总览（Employee、Diagnosis）：在用时头条总是在用，有问题时问题跟在后面；没在用时是那个问题，
/// 没有问题是空闲。快用完了是小标记；建议先用（本周额度快重置了、还剩不少）只在没有问题、额度不紧的时候
struct AccountMood: Equatable {
    let busy: Bool
    let trouble: AccountTrouble?
    let nearLimit: Bool
    let useFirst: UseFirst?

    init(_ a: AccountStatus, now: Double) {
        busy = a.busy
        if a.login == "logged_out" {
            trouble = .loggedOut
        } else if a.login == "email_mismatch" {
            trouble = .wrongAccount
        } else if a.usage?.errorCode == "auth" {
            trouble = .expired
        } else if let b = a.blocking(now: now) {
            trouble = .usedUp(until: b.resetsEpoch)
        } else {
            trouble = nil
        }
        nearLimit = trouble == nil && a.nearLimit(now: now)
        let f = a.usage?.useFirst.flatMap { $0.isActive(now: now) ? $0 : nil }
        useFirst = trouble == nil && !nearLimit && !a.fiveHourPressed(now: now) ? f : nil
    }
}

/// 点名（「账号」页顶上那一行，Mac App 的 OfficeCensus）：几个账号，几个在用、几个空闲、几个额度用完……
struct AccountCensus: Equatable {
    var total = 0, working = 0, idle = 0, resting = 0, absent = 0, wrongPerson = 0, expired = 0

    init(_ all: [AccountStatus], now: Double) {
        total = all.count
        for a in all {
            let m = AccountMood(a, now: now)
            if m.busy {
                working += 1
                continue
            }
            switch m.trouble {
            case nil: idle += 1
            case .usedUp: resting += 1
            case .loggedOut: absent += 1
            case .wrongAccount: wrongPerson += 1
            case .expired: expired += 1
            }
        }
    }
}

extension PeerListing {
    /// 这台电脑上用这个账号的会话（会话的说明里的账号摘要对得上；那台电脑没给摘要时照工具和名字认）
    func sessions(of a: AccountStatus) -> [RemoteSession] {
        sessions.filter { s in
            if !a.aid.isEmpty, !s.info.aid.isEmpty { return s.info.aid == a.aid }
            return s.info.tool == a.tool && s.info.name == a.name && !a.name.isEmpty
        }
    }
}
