// 通知的两种提示音：ios/Sounds/ 下的 done.caf、ask.caf（App 和通知扩展里各带一份；响哪一声由 CcaRemoteKit 的 NoteSound 挑，
// docs/dev/ios.md「App」的 ios/Sounds/）。系统默认的那一声又短又轻，手机放在一边时常常听不见：这两声长一点、亮一点。
//   xcrun swiftc -O ios/tools/make-sounds/main.swift -o make-sounds
//   ./make-sounds 目录           写出 done.caf、ask.caf（改了这里的声音，用它换掉 ios/Sounds/ 里的那两个）
//   ./make-sounds --check 目录   照这里重新合成，和目录里的比：格式、长度一样，每个采样最多差 1（sin、exp 在别的机器上最后一位可能不一样）。
//                                ios/test.sh 拿它查 ios/Sounds/ 里的就是这里合成的
// 加法合成：每一下是几个泛音，各自指数衰减，3 毫秒起音、最后 20 毫秒收掉（免得咔哒一声）；整段的峰值归一到 -1 dBFS。
// 写成 CAF：16 位整数、小端、44.1 kHz、单声道（通知的声音认的格式之一，不到 30 秒）
import Foundation

let rate = 44100.0

/// 一个泛音：频率是这一下的几倍、多响、衰减到 1/e 要几秒
struct Partial {
    let ratio, amp, decay: Double
}

/// 一下：从第几秒起、基音的频率、多响、音色（几个泛音）、多长（秒）
struct Hit {
    let at, freq, gain: Double
    let partials: [Partial]
    let length: Double
}

let bell = [Partial(ratio: 1, amp: 1, decay: 0.55), Partial(ratio: 2, amp: 0.45, decay: 0.3), Partial(ratio: 3, amp: 0.22, decay: 0.18),
            Partial(ratio: 4.16, amp: 0.12, decay: 0.12)]
let longBell = [Partial(ratio: 1, amp: 1, decay: 0.9), Partial(ratio: 2, amp: 0.4, decay: 0.45), Partial(ratio: 3.01, amp: 0.2, decay: 0.25),
                Partial(ratio: 5.4, amp: 0.08, decay: 0.1)]
let ping = [Partial(ratio: 1, amp: 1, decay: 0.16), Partial(ratio: 2, amp: 0.5, decay: 0.09), Partial(ratio: 3, amp: 0.2, decay: 0.05)]
let c6 = 1046.5, g6 = 1567.98, c7 = 2093.0

let sounds: [(name: String, hits: [Hit])] = [
    // 做完了：叮—咚往上走（C6 到 G6），后一下长、慢慢散掉
    ("done", [Hit(at: 0, freq: c6, gain: 0.8, partials: bell, length: 0.6), Hit(at: 0.16, freq: g6, gain: 1, partials: longBell, length: 1.25)]),
    // 在等你：高音（C7）短促的两下，隔一会儿再两下
    ("ask", [Hit(at: 0, freq: c7, gain: 1, partials: ping, length: 0.3), Hit(at: 0.15, freq: c7, gain: 1, partials: ping, length: 0.3),
             Hit(at: 0.6, freq: c7, gain: 1, partials: ping, length: 0.3), Hit(at: 0.75, freq: c7, gain: 1, partials: ping, length: 0.45)]),
]

func render(_ hits: [Hit]) -> [Double] {
    let total = hits.map { $0.at + $0.length }.max() ?? 0
    var out = [Double](repeating: 0, count: Int(total * rate))
    for h in hits {
        let start = Int(h.at * rate), n = Int(h.length * rate)
        for i in 0..<n where start + i < out.count {
            let t = Double(i) / rate
            let attack = min(1, t / 0.003)
            let release = min(1, (h.length - t) / 0.02)
            var s = 0.0
            for p in h.partials { s += p.amp * exp(-t / p.decay) * sin(2 * .pi * h.freq * p.ratio * t) }
            out[start + i] += h.gain * attack * release * s
        }
    }
    let peak = out.map { abs($0) }.max() ?? 1
    return out.map { $0 / peak * 0.89 }
}

/// CAF 的头：文件头 8 个字节、desc 那一段 44 个、data 那一段的头 16 个，后面是采样
let header = 68

func caf(_ x: [Double]) -> Data {
    var d = Data()
    func be16(_ v: UInt16) { withUnsafeBytes(of: v.bigEndian) { d.append(contentsOf: $0) } }
    func be32(_ v: UInt32) { withUnsafeBytes(of: v.bigEndian) { d.append(contentsOf: $0) } }
    func be64(_ v: UInt64) { withUnsafeBytes(of: v.bigEndian) { d.append(contentsOf: $0) } }
    func tag(_ s: String) { d.append(contentsOf: Array(s.utf8)) }
    tag("caff"); be16(1); be16(0)
    tag("desc"); be64(32); be64(rate.bitPattern)
    tag("lpcm"); be32(2) /* 整数、小端 */; be32(2); be32(1); be32(1); be32(16)
    tag("data"); be64(UInt64(4 + x.count * 2)); be32(0)
    for v in x { withUnsafeBytes(of: Int16(max(-1, min(1, v)) * 32767).littleEndian) { d.append(contentsOf: $0) } }
    return d
}

func fail(_ s: String) -> Never {
    FileHandle.standardError.write(Data((s + "\n").utf8))
    exit(1)
}

/// 第 i 个字节起的那个采样
func sample(_ b: [UInt8], _ i: Int) -> Int {
    Int(Int16(bitPattern: UInt16(b[i]) | UInt16(b[i + 1]) << 8))
}

let args = Array(CommandLine.arguments.dropFirst())
if args.count == 2, args[0] == "--check" {
    for (name, hits) in sounds {
        let path = args[1] + "/" + name + ".caf"
        let want = [UInt8](caf(render(hits)))
        guard let data = FileManager.default.contents(atPath: path) else { fail("没有 \(path)") }
        let got = [UInt8](data)
        guard got.count == want.count, got[..<header] == want[..<header] else { fail("\(path) 和合成的不一样：格式或者长度") }
        for i in stride(from: header, to: got.count, by: 2) where abs(sample(got, i) - sample(want, i)) > 1 {
            fail("\(path) 和合成的不一样：第 \((i - header) / 2) 个采样是 \(sample(got, i))，合成的是 \(sample(want, i))")
        }
    }
    print("ok")
} else if args.count == 1, !args[0].hasPrefix("-") {
    for (name, hits) in sounds {
        do {
            try caf(render(hits)).write(to: URL(fileURLWithPath: args[0] + "/" + name + ".caf"))
        } catch {
            fail("写不了 \(args[0])/\(name).caf：\(error.localizedDescription)")
        }
    }
} else {
    fail("用法：make-sounds 目录 | make-sounds --check 目录")
}
