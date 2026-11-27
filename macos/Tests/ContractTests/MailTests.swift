import Foundation

// 邮箱助手（PROTOCOL 2.4、4.11）：命令的样子；账号页「邮箱」一节按需拉登录邮件（不随开页自动拉）；
// 打开某封进 launch 闸门；配置走提问会话（要 hello.mail）；cca 说这个账号没有（can.mail 是 false、旧版 cca）就不显示。

@MainActor func mailTests() async {
    section("邮箱助手")

    // MARK: 命令的样子
    let list = CcaCommand.mailOpen(.claude, key: "工作(bob@example.com)", msg: "[Gmail]/All Mail:12345:678")
    check("命令：mail open KEY --msg；跑完为止、进 launch 闸门、120 秒",
          list.arguments == ["mail", "open", "工作(bob@example.com)", "--msg", "[Gmail]/All Mail:12345:678"]
          && list.runsToCompletion && list.gate == "launch:claude:工作(bob@example.com)" && list.timeout == 120)
    let forgetAcct = CcaCommand.mailForget(.claude, key: "工作(bob@example.com)", scope: .account)
    let forgetDef = CcaCommand.mailForget(.claude, key: nil, scope: .default)
    check("命令：mail forget --account / --default",
          forgetAcct.arguments == ["mail", "forget", "--account", "工作(bob@example.com)"]
          && forgetDef.arguments == ["mail", "forget", "--default"] && forgetAcct.gate == nil)
    // 信任转发中转（PROTOCOL 4.11）：关闭是快速改 config.json 的普通命令（不占闸门、不跑完为止、60 秒）；
    // 开启是不安全操作，走提问会话（要问域名、要同意）。
    let trustOff = CcaCommand.mailTrustForwarderOff(.claude, key: "工作(bob@example.com)")
    check("命令：mail trust-forwarder KEY off（不占闸门、不跑完为止、60 秒）",
          trustOff.arguments == ["mail", "trust-forwarder", "工作(bob@example.com)", "off"]
          && trustOff.gate == nil && !trustOff.runsToCompletion && trustOff.timeout == 60 && trustOff.key == "工作(bob@example.com)")
    let trust = Handoff.mailTrustForwarder(account: AccountRef(tool: .claude, key: "工作(bob@example.com)"))
    check("mail trust-forwarder 开启是提问会话（要 forms）",
          trust.arguments == ["mail", "trust-forwarder", "工作(bob@example.com)"] && trust.inApp && trust.needsForms)

    // MARK: 配置的提问会话（Handoff）
    let def = Handoff.mailSetup(account: nil)
    let acct = Handoff.mailSetup(account: AccountRef(tool: .claude, key: "工作(bob@example.com)"))
    check("mail setup 是提问会话（要 forms）",
          def.arguments == ["mail", "setup"] && def.inApp && def.needsForms
          && acct.arguments == ["mail", "setup", "--account", "工作(bob@example.com)"] && acct.inApp && acct.needsForms)

    // MARK: 文档解码
    if let d = fixture("mail_list.json"), let doc = try? DocumentDecoder.decode(MailListDoc.self, from: d) {
        check("mail-list 解码：4 封，第一封 Anthropic 直签能打开、via 为空、链接只给样子",
              doc.rows.count == 4 && doc.error == nil && doc.rows[0].openable
              && doc.rows[0].msg?.argument == "[Gmail]/All Mail:12345:678"
              && doc.rows[0].linkShape != nil && doc.rows[0].verdict.ok && doc.rows[0].via == nil && !doc.rows[0].viaForwarder)
        // 经可信中转（不安全）的那封：via==forwarder、能打开、通过核对、有句柄、只给链接样子
        if let fwd = doc.rows.first(where: { $0.viaForwarder }) {
            check("mail-list：经可信中转的行 via==forwarder、可打开、通过核对、有句柄、不含真链接",
                  fwd.via == "forwarder" && fwd.openable && fwd.verdict.ok && fwd.msg != nil && fwd.linkShape != nil)
        } else {
            check("mail_list.json 里有一封经可信中转的（via==forwarder）", false)
        }
        // 标题不像登录邮件的：why==subject、App 默认折叠（isNonLogin）、不能打开
        if let nonLogin = doc.rows.first(where: { $0.isNonLogin }) {
            check("mail-list：非登录邮件 why==subject、isNonLogin、不能打开",
                  nonLogin.verdict.why == "subject" && !nonLogin.openable && nonLogin.via == nil)
        } else {
            check("mail_list.json 里有一封非登录邮件（why==subject）", false)
        }
        check("mail-list：没通过核对的说原因、不能打开",
              doc.rows.contains { !$0.openable && $0.verdict.ok == false && nonEmpty($0.verdict.whyText) != nil })
    } else {
        check("mail_list.json 夹具能解码", false)
    }

    // Team 给的账号（cca 3.7.9 起）：信任哪些转发中转照 Team 的配置，这台设备上登记的不算数
    if let st = loadFixture(StateDoc.self, "state.json") {
        func mail(_ key: String) -> MailConfig? { st.allAccounts.first { $0.key == key }?.mail }
        let bob = mail("工作(bob@example.com)"), grace = mail("grace@example.com"), dave = mail("测试(dave@example.com)"), erin = mail("erin@example.com")
        check("mail.team_forwarders：Team 给的账号带着 Team 的编号、名字和名单（forwarder 是 nil）；这台设备登记过、不算数的在 local；不是 Team 给的、旧版 cca 是 nil",
              bob?.forwarder == nil && bob?.teamForwarders?.team == "5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b" && bob?.teamForwarders?.teamName == "书房组"
              && bob?.teamForwarders?.domains == ["relay.example.com"] && bob?.teamForwarders?.local == nil
              && grace?.teamForwarders?.local == "old-relay.example.net"
              && dave?.teamForwarders == nil && dave?.forwarder == "relay.example.com" && erin != nil && erin?.teamForwarders == nil)
    }

    // MARK: 模型：按需拉、打开、gate
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let mail = fixture("mail_list.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false); return
    }
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if Array(request.arguments.prefix(2)) == ["_gui", "mail-list"] { return response(0, stdout: mail) }
        if Array(request.arguments.prefix(2)) == ["mail", "open"] { return response(0) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m = makeModel(t)
    await m.start()
    check("hello.mail 为真时 mailSupported", m.mailSupported)
    func askedMail() -> [[String]] { t.requests.map(\.arguments).filter { Array($0.prefix(2)) == ["_gui", "mail-list"] } }
    guard let bob = m.accounts(.claude).first(where: { $0.can.mail && $0.mail?.configured == true }) else {
        check("夹具里有配置了邮箱的 Claude 账号（can.mail）", false); return
    }
    m.setMainVisible(true)
    m.selection = .account(bob.ref)
    check("开账号页不自动拉登录邮件（翻账号不卡）", askedMail().isEmpty)
    m.refreshMail(bob.ref)
    check("点「查看」后拉到了", await waitUntil { m.mailLists[bob.ref]?.doc?.rows.count == 4 }, "\(askedMail())")
    check("问的带 --account 这个账号、24 小时", askedMail().first == ["_gui", "mail-list", "--account", bob.key, "24"])
    // 打开某封（进 launch 闸门，不该 crash）
    if let h = m.mailLists[bob.ref]?.doc?.rows.first(where: { $0.openable })?.msg {
        m.openMailMessage(bob.ref, h)
        check("打开某封：跑了 mail open", await waitUntil { t.requests.contains { Array($0.arguments.prefix(2)) == ["mail", "open"] } })
    }

    // MARK: 旧版 cca（没有 hello.mail）：不显示
    let helloOld = hello.replacingOccurrences(of: "\"mail\": 1,", with: "")
    let t2 = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: helloOld) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m2 = makeModel(t2)
    await m2.start()
    check("旧版 cca 没有 hello.mail：mailSupported 为假", !m2.mailSupported)
    if let bob2 = m2.accounts(.claude).first(where: { $0.can.mail }) {
        m2.refreshMail(bob2.ref)
        check("mailSupported 为假时不拉登录邮件", t2.requests.allSatisfy { Array($0.arguments.prefix(2)) != ["_gui", "mail-list"] })
    }
}
