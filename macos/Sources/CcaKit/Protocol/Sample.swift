import Foundation

extension Account {
    /// 示意用的账号（新手教程的小图、截图模式的画廊、测试）：字段照 `_gui state` 的写法，没给的照常宽松解码。key 必须有。
    static func sample(_ fields: [String: Any], tool: ToolID = .claude) -> Account {
        let data = (try? JSONSerialization.data(withJSONObject: fields)) ?? Data()
        guard var a = try? JSONDecoder().decode(Account.self, from: data) else { fatalError("示意账号缺 key：\(fields)") }
        a.tool = tool
        return a
    }
}

/// 编进 App 里的一小份样例（假数据）：`cca.app/Contents/MacOS/cca --self-check` 用它确认解码器能用。
enum SampleDocs {
    static let hello = #"""
    {"version": 1, "kind": "hello", "api": 1, "cca_version": "0.0.0", "installed": true, "home": "/Users/me/.cca",
     "tools": {"claude": {"enabled": true, "bin": null}, "codex": {"enabled": true, "bin": null}}}
    """#

    static let state = #"""
    {"version": 1, "kind": "state", "generated_at": 1794042000.0, "errors": [], "notices": [],
     "routing": {"first_port": 7901, "pool_size": 8, "sync": "ok", "local_sync": "n/a",
       "slots": [{"slot": 1, "group": "CCA-1", "port": 7901, "tool": "claude", "key": "alice@example.com", "node": "US-A"}], "nodes": []},
     "tools": [{"id": "claude", "cli_prefix": [], "enabled": true, "chrome": true, "settings_problems": [], "errors": [],
       "accounts": [{"key": "alice@example.com", "kind": "oauth", "login": "logged_in", "plan": "Max", "plan_tier": "advanced", "node": "US-A", "port": 7901,
         "badge": {"label": "A", "color": "#0A84FF"}, "cli": {"running": true, "sessions": 2, "since_epoch": 1794041000.5},
         "can": {"app": true, "web": true, "usage": true, "check": true},
         "usage": {"key": "alice@example.com", "source": "cache", "fetched_epoch": 1794041000.0, "stale": false,
                   "five_hour": {"utilization": 13.0, "resets_epoch": 1794045210.12, "reset": false}},
         "health": {"version": 1, "checked_at": 1794041000.0, "route": "direct", "result": "api_ok"}}]}]}
    """#

    static let live = #"""
    {"version": 1, "kind": "live", "generated_at": 1794042010.0, "route": "router", "proxy_host": "192.0.2.10",
     "slots": [{"slot": 1, "port": 7901, "tool": "claude", "key": "alice@example.com", "state": "drift"}],
     "summary": {"ok": 0, "drift": 1, "closed": 0, "unknown": 0, "empty": 0}}
    """#

    static let usage = #"""
    {"version": 1, "kind": "usage", "generated_epoch": 1794042000.0, "tool": "codex",
     "accounts": [{"key": "frank@example.com", "source": "live", "fetched_epoch": 1794041998.0,
                   "seven_day": {"utilization": 76.0, "resets_at": 1794470400, "resets_epoch": 1794470400.0, "reset": false},
                   "reset_credits": {"availableCount": 2}}], "skipped_api_accounts": []}
    """#

    static let check = #"""
    {"version": 1, "kind": "check", "tool": "claude", "generated_at": 1794042100.0, "route": "router",
     "accounts": [{"key": "alice@example.com", "port": 7901, "health": {"version": 1, "checked_at": 1794042090.0, "result": "ok"}}],
     "shared_exit_ips": []}
    """#

    static let events = #"""
    {"v":1,"ev":"step","code":"sync"}
    {"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","severity":"warn","default":"yes"}
    {"v":1,"ev":"terminal","code":"login","argv":["login","alice@example.com"],"display":"cca login alice@example.com","message":""}
    {"v":1,"ev":"fail","code":"port_closed","message":"端口 192.0.2.10:7901 不通"}
    {"v":1,"ev":"result","code":"opened","route":"router","node":"US-A","port":7901,"message":"已打开"}
    """#

    /// 全部解码一遍；出错时抛出说明。
    static func verify() throws {
        struct Bad: Error, CustomStringConvertible { let description: String }
        let h = try DocumentDecoder.decode(HelloDoc.self, from: Data(hello.utf8))
        let s = try DocumentDecoder.decode(StateDoc.self, from: Data(state.utf8))
        let l = try DocumentDecoder.decode(LiveDoc.self, from: Data(live.utf8))
        let u = try DocumentDecoder.decode(UsageDoc.self, from: Data(usage.utf8))
        let c = try DocumentDecoder.decode(CheckDoc.self, from: Data(check.utf8))
        let e = EventLog.parse(Data(events.utf8))
        guard GUIAPI.isSupported(h.api), s.allAccounts.count == 1, s.allAccounts.first?.health?.route == .direct,
              s.allAccounts.first?.planTier == .advanced, s.allAccounts.first?.cliSessions == 2,
              l.hasDrift, l.proxyHost == "192.0.2.10", u.accounts.first?.sevenDay?.resetsEpoch == 1794470400,
              c.accounts.count == 1, e.map(\.type) == [.step, .consent, .terminal, .fail, .result] else {
            throw Bad(description: "样例解码的结果不对")
        }
    }
}
