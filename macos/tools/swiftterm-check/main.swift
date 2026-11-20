import Foundation
import SwiftTerm

// macos/build.sh 编好 SwiftTerm 以后跑一遍（只在重编了的时候）：图形协议确实不处理了。
//
// 终端里的内容来自别的机器（docs/dev/remote.md「界面 App」）。SwiftTerm 的 Kitty 图形协议能让终端里的内容叫它打开这台 Mac 上的
// 文件（t=f）、删临时文件（t=t）、去掉共享内存（t=s），并且把成不成写回给对方——对方就能探这台 Mac 上有哪些文件。上游没有开关，
// build.sh 编的时候把分派它的那一行换掉；这里验证编出来的这一份真的不理它：不处理就不会回话。
// 失败时打印以 `error:` 开头的一行（build.sh 只挑这种行给人看），退出码 1。

final class Probe: TerminalDelegate {
    var sent: [UInt8] = []
    func send(source: Terminal, data: ArraySlice<UInt8>) { sent += data }
}

let probe = Probe()
let terminal = Terminal(delegate: probe, options: TerminalOptions(cols: 80, rows: 24))
var failed = false

func feed(_ bytes: [UInt8]) -> [UInt8] {
    probe.sent = []
    terminal.feed(byteArray: bytes)
    return probe.sent
}

func bytes(_ s: String) -> [UInt8] { Array(s.utf8) }

func expectSilent(_ what: String, _ input: [UInt8]) {
    let answer = feed(input)
    if !answer.isEmpty {
        failed = true
        print("error: SwiftTerm 还在处理 \(what)（回了 \(answer.count) 个字节）：图形协议没关掉，不能这样编出去")
    }
}

let esc: [UInt8] = [0x1b]
let st: [UInt8] = [0x1b, 0x5c]

// 先确认探针收得到回话：问它是什么终端（DA1），一定有回答。没有的话下面“没回话”什么都证明不了
if feed(esc + bytes("[c")).isEmpty {
    failed = true
    print("error: 自检的探针没收到终端对 DA1 的回答：SwiftTerm 的接口变了，macos/tools/swiftterm-check 要跟着改")
}

// Kitty 图形：查询（a=q）处理了就会回 `ESC _ G i=…;OK ESC \` 或者一句错
expectSilent("Kitty 图形协议（直接给数据）", esc + bytes("_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA") + st)

// 让它读这台机器上的文件：存在的、不存在的都不能有回话（有回话就能探文件在不在）
let path = NSTemporaryDirectory() + "cca-swiftterm-check-\(getpid()).bin"
FileManager.default.createFile(atPath: path, contents: Data([0, 0, 0]))
let there = Data(path.utf8).base64EncodedString()
let missing = Data((path + ".missing").utf8).base64EncodedString()
expectSilent("Kitty 图形协议（读本机文件）", esc + bytes("_Gi=32,s=1,v=1,a=q,t=f,f=24;\(there)") + st)
expectSilent("Kitty 图形协议（读本机文件，不存在的）", esc + bytes("_Gi=33,s=1,v=1,a=q,t=f,f=24;\(missing)") + st)
expectSilent("Kitty 图形协议（临时文件）", esc + bytes("_Gi=34,s=1,v=1,a=q,t=t,f=24;\(there)") + st)
if !FileManager.default.fileExists(atPath: path) {
    failed = true
    print("error: SwiftTerm 把自检放的临时文件删掉了：Kitty 图形协议（t=t）还在处理")
}
try? FileManager.default.removeItem(atPath: path)

// 另一个入口：别的序列中间的 0x9f 也被它当成 APC 的开头（OSC 里、CSI 里）
expectSilent("Kitty 图形协议（从 OSC 中间的 C1 进来）", esc + bytes("]") + [0x9f] + bytes("Gi=35,s=1,v=1,a=q,t=d,f=24;AAAA") + st)
expectSilent("Kitty 图形协议（从 CSI 中间的 C1 进来）", esc + bytes("[") + [0x9f] + bytes("Gi=36,s=1,v=1,a=q,t=d,f=24;AAAA") + st)
// 分几块到的
probe.sent = []
terminal.feed(byteArray: esc + bytes("_Gi=37,s=1,v=1,"))
terminal.feed(byteArray: bytes("a=q,t=d,f=24;AA"))
terminal.feed(byteArray: bytes("AA") + st)
if !probe.sent.isEmpty {
    failed = true
    print("error: SwiftTerm 还在处理 Kitty 图形协议（分几块到的）")
}

// 之后终端照常：还答得了 DA1
if feed(esc + bytes("[c")).isEmpty {
    failed = true
    print("error: 喂过图形协议的序列以后终端不回话了")
}

exit(failed ? 1 : 0)
