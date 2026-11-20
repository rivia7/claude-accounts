import Foundation

/// 界面里的终端（PROTOCOL 9.3）：一个跑在伪终端上的 cca（`cca remote attach`）。和别的调用不一样的地方只有一处——它的
/// 标准输入、输出、错误不是 /dev/null 和管道，而是 App 开的伪终端从的那头；别的照旧：自己一个会话、只带环境和参数。
/// 这样起来的进程一开始没有控制终端（macOS 上打开终端不会让它成为控制终端），cca 的接入端自己把它认下来。
///
/// 终端里的内容（可能有密钥和源码）只在内存里：读到的字节交给 `onEvent` 就不管了，不写文件、不进日志。
enum PtyEvent: Sendable {
    /// 伪终端上读到的字节（终端要显示的）。
    case output(Data)
    /// 进程退出了（退出码；被信号杀掉是 128 + 信号）。在这之前的输出都已经交完。
    case exited(Int32)
}

/// 一个开着的终端：往里写（按键）、改大小、断开。线程安全。
protocol TerminalHandle: AnyObject, Sendable {
    func write(_ data: Data)
    func resize(cols: Int, rows: Int)
    /// 断开：给进程组发 SIGTERM（接入端收到就断开，会话在那边照常运行），3 秒后还在就 SIGKILL。退出了再调没有作用。
    func close()
}

struct TerminalUnsupported: Error, CustomStringConvertible {
    var description: String { "演示数据不开终端" }
}

final class PtyProcess: TerminalHandle, @unchecked Sendable {
    private let lock = NSLock()
    /// 伪终端主的那头；进程退出、读完以后是 -1
    private var master: Int32 = -1
    private var pid: pid_t = 0
    private var exited = false
    private var closing = false
    /// 写的那一份另开（dup）：往里写可能等着（对面一时不读），不能占着上面那把锁
    private var writer: Int32 = -1
    private let writes = DispatchQueue(label: "local.cca.app.pty.write")
    let killGrace: TimeInterval

    static func windowSize(cols: Int, rows: Int) -> winsize {
        winsize(ws_row: UInt16(clamping: max(rows, 1)), ws_col: UInt16(clamping: max(cols, 1)), ws_xpixel: 0, ws_ypixel: 0)
    }

    /// 开伪终端、起进程。`onEvent` 在后台线程上一个接一个地调：调用方处理完一块才读下一块（终端画不过来时，进程自然被压住）。
    init(spec: ProcessSpec, cols: Int, rows: Int, onEvent: @escaping @Sendable (PtyEvent) -> Void) throws {
        killGrace = spec.killGrace
        let m = posix_openpt(O_RDWR | O_NOCTTY)
        guard m >= 0 else { throw ProcessError.pipeFailed(errno: errno) }
        var name = [CChar](repeating: 0, count: 128)
        guard grantpt(m) == 0, unlockpt(m) == 0, ptsname_r(m, &name, name.count) == 0 else {
            let e = errno
            Darwin.close(m)
            throw ProcessError.pipeFailed(errno: e)
        }
        _ = fcntl(m, F_SETFD, FD_CLOEXEC)
        let path = String(cString: name)
        // 从的那头先在这边开一下：它开过以后才能设窗口大小。起好进程就关掉——留着的话进程退出时主的那头读不到头
        let s = open(path, O_RDWR | O_NOCTTY | O_CLOEXEC)
        guard s >= 0 else {
            let e = errno
            Darwin.close(m)
            throw ProcessError.pipeFailed(errno: e)
        }
        var ws = PtyProcess.windowSize(cols: cols, rows: rows)
        _ = ioctl(s, TIOCSWINSZ, &ws)
        let w = fcntl(m, F_DUPFD_CLOEXEC, 10)
        guard w >= 0 else {   // 没有它就是“能看不能打字”：当成没起来
            let e = errno
            Darwin.close(s)
            Darwin.close(m)
            throw ProcessError.pipeFailed(errno: e)
        }
        let child: pid_t
        do {
            child = try CcaProcess.spawnOnTerminal(spec, slavePath: path)
        } catch {
            Darwin.close(s)
            Darwin.close(m)
            Darwin.close(w)
            throw error
        }
        Darwin.close(s)
        master = m
        writer = w
        pid = child
        let t = Thread { [self] in
            var chunk = [UInt8](repeating: 0, count: 65536)
            while true {
                let n = read(m, &chunk, chunk.count)
                if n > 0 {
                    onEvent(.output(Data(chunk[0..<n])))
                } else if n < 0 && (errno == EINTR || errno == EAGAIN) {
                    continue
                } else {
                    break   // 读到头（从的那头都关了）或者 EIO：进程走了
                }
            }
            var status: Int32 = 0
            while waitpid(child, &status, 0) == -1 && errno == EINTR {}
            lock.withLock {
                exited = true
                master = -1
            }
            Darwin.close(m)
            writes.async { [self] in
                let fd = lock.withLock { () -> Int32 in
                    let fd = writer
                    writer = -1
                    return fd
                }
                if fd >= 0 { Darwin.close(fd) }
            }
            onEvent(.exited(CcaProcess.exitInfo(status).0))
        }
        t.name = "cca-pty-\(child)"
        t.start()
    }

    func write(_ data: Data) {
        guard !data.isEmpty else { return }
        writes.async { [self] in
            let fd = lock.withLock { writer }
            if fd >= 0 { CcaProcess.writeAll(fd, data) }
        }
    }

    func resize(cols: Int, rows: Int) {
        lock.withLock {
            guard master >= 0 else { return }
            var ws = PtyProcess.windowSize(cols: cols, rows: rows)
            _ = ioctl(master, TIOCSWINSZ, &ws)
        }
    }

    func close() {
        let p: pid_t? = lock.withLock {
            guard !exited, !closing, pid != 0 else { return nil }
            closing = true
            return pid
        }
        guard let p else { return }
        if kill(-p, SIGTERM) != 0 { kill(p, SIGTERM) }
        DispatchQueue.global().asyncAfter(deadline: .now() + killGrace) { [self] in
            // 退出了就不再发信号：这个进程号可能已经给了别的进程
            if lock.withLock({ !exited }) {
                if kill(-p, SIGKILL) != 0 { kill(p, SIGKILL) }
            }
        }
    }
}
