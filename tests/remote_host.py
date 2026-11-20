#!/usr/bin/env python3
"""远程会话的会话宿主和接入端（docs/dev/remote.md）：只运行 cca 的辅助程序（cli/），都在临时 HOME 里、真的伪终端上。

宿主（remote-host）顶替客户端的位置：客户端跑在它开的 pty 里，环境、参数、退出码原样；启动它的终端没了会话照样运行；
接入端（remote-attach）经 ~/.cca/remote/ 里的 socket 接进来。客户端是下面的 CLIENT（一行一条命令）。"""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_bin import HELPER

CLIENT = r'''
import fcntl, os, signal, struct, sys, termios

def size():
    return struct.unpack('HHHH', fcntl.ioctl(0, termios.TIOCGWINSZ, b'\0' * 8))[:2]

def say(s):
    os.write(1, s.encode())

def term(*_):
    say('<term>\r\n')
    os._exit(5)

signal.signal(signal.SIGWINCH, lambda *_: say('<size %d %d>\r\n' % size()))
signal.signal(signal.SIGTERM, term)
leader = os.getsid(0) == os.getpid()
try:
    os.close(os.open('/dev/tty', os.O_RDWR))
    ctty = True
except OSError:
    ctty = False
say('<ready %d %d leader=%s ctty=%s tty=%s args=%s>\r\n' % (*size(), leader, ctty, os.isatty(0) and os.isatty(1) and os.isatty(2), sys.argv[1:]))
while True:
    line = sys.stdin.readline()
    if not line:
        say('<eof>\r\n')
        sys.exit(9)
    cmd = line.strip()
    if cmd.startswith('exit '):
        sys.exit(int(cmd.split()[1]))
    elif cmd.startswith('say '):
        say('<%s>\r\n' % cmd[4:])
    elif cmd == 'query':
        say('\x1b[6n\x1b]11;?\x07<asked>\r\n')
    elif cmd == 'evil':
        # 去掉中间那条查询以后，前后两截不能在别的终端上拼成 ESC ] 52…（写剪贴板）
        say('<evil>\x1b\x1b[c]52;c;ZXZpbA==\x07\x1b[' + '0;' * 300 + 'c<done>\r\n')
    elif cmd == 'clip':
        say('\x1b]52;c;Y2xpcA==\x07<clipped>\r\n')
    elif cmd == 'alt':
        say('\x1b[?1049h\x1b[?25l\x1b[?2004h<alt>\r\n')
    elif cmd == 'kill':
        os.kill(os.getpid(), signal.SIGKILL)
    elif cmd == 'flood':
        say(''.join('line %06d of the flood\r\n' % i for i in range(20000)) + '<flooded>\r\n')
    elif cmd == 'dump':
        say(''.join('dump %07d ................................................\r\n' % i for i in range(40000)) + '<dumped>\r\n')
        sys.exit(6)
    elif cmd == 'closetty':
        say('<closing>\r\n')
        for fd in (0, 1, 2):
            os.close(fd)
        import time
        time.sleep(1.5)
        os._exit(4)
'''


class Term:
    """一个伪终端和跑在上面的进程"""

    def __init__(self, argv, env, cwd, rows=24, cols=80):
        self.out = b''
        pid, fd = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
                os.chdir(cwd)
                os.execve(argv[0], argv, env)
            finally:
                os._exit(111)
        self.pid, self.fd, self.code = pid, fd, None

    def resize(self, rows, cols):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))

    def send(self, data):
        os.write(self.fd, data if isinstance(data, bytes) else data.encode())

    def pump(self, timeout=0.05):
        """把现在能读到的读进 self.out；False：对面关了"""
        if self.fd is None:
            return False
        r, _, _ = select.select([self.fd], [], [], timeout)
        if not r:
            return True
        try:
            data = os.read(self.fd, 65536)
        except OSError:
            return False
        self.out += data
        return bool(data)

    def expect(self, pattern, timeout=10, since=0):
        """等输出里（从第 since 个字节起）出现 pattern，返回匹配到的位置的末尾"""
        end = time.monotonic() + timeout
        rx = re.compile(pattern if isinstance(pattern, bytes) else pattern.encode())
        while True:
            m = rx.search(self.out, since)
            if m:
                return m.end()
            if time.monotonic() > end or not self.pump():
                m = rx.search(self.out, since)
                if m:
                    return m.end()
                raise AssertionError('没等到 %r，输出的末尾：%r' % (pattern, self.out[-600:]))

    def wait(self, timeout=10):
        """等进程退出，返回退出码（被信号杀掉是负的）；它用掉的 CPU 时间记在 self.cpu"""
        end = time.monotonic() + timeout
        while self.code is None:
            self.pump(0.02)
            pid, status, usage = os.wait4(self.pid, os.WNOHANG)
            if pid:
                self.code = os.waitstatus_to_exitcode(status)
                self.cpu = usage.ru_utime + usage.ru_stime
            elif time.monotonic() > end:
                raise AssertionError('进程 %d 没有退出，输出的末尾：%r' % (self.pid, self.out[-600:]))
        return self.code

    def hangup(self):
        """关掉这个终端（宿主收到 SIGHUP）"""
        os.close(self.fd)
        self.fd = None

    def close(self):
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None
        if self.code is None:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.kill(self.pid, sig)
                except ProcessLookupError:
                    break
                end = time.monotonic() + 2
                while time.monotonic() < end:
                    pid, status = os.waitpid(self.pid, os.WNOHANG)
                    if pid:
                        self.code = os.waitstatus_to_exitcode(status)
                        return
                    time.sleep(0.02)


class AppTerm(Term):
    """界面 App 那样起的终端（macos/PROTOCOL.md 9.3）：posix_spawn、自己一个会话、伪终端从的那头接成 0/1/2。
    这样起来的进程一开始没有控制终端（macOS 上打开终端不会让它成为控制终端）"""

    def __init__(self, argv, env, rows=24, cols=80):
        self.out = b''
        self.fd, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        actions = [(os.POSIX_SPAWN_OPEN, 0, os.ttyname(slave), os.O_RDWR, 0), (os.POSIX_SPAWN_DUP2, 0, 1), (os.POSIX_SPAWN_DUP2, 0, 2)]
        self.pid, self.code = os.posix_spawn(argv[0], argv, env, file_actions=actions, setsid=True), None
        os.close(slave)


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


class RemoteHost(unittest.TestCase):
    cca_home = None   # 子类换成很深的目录

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='cca-remote-')
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name).resolve()
        self.cca = Path(self.cca_home(self.home)) if self.cca_home else self.home / '.cca'
        self.cca.mkdir(parents=True)
        (self.cca / 'config.json').write_text('{}')
        self.work = self.home / '项目 one'
        self.work.mkdir()
        self.client = self.home / 'client.py'
        self.client.write_text(CLIENT)
        self.env = {'HOME': str(self.home), 'PATH': '/usr/bin:/bin', 'TERM': 'xterm-256color', 'MARK': 'kept'}
        if self.cca_home:
            self.env['CCA_HOME'] = str(self.cca)
        self.terms = []

    def tearDown(self):
        for t in self.terms:
            t.close()

    def term(self, argv, **kw):
        t = Term(argv, self.env, str(self.work), **kw)
        self.terms.append(t)
        return t

    def host(self, *client_args, **kw):
        return self.term([HELPER, 'remote-host', 'claude', '工作(alice@example.com)', '工作', '--',
                          sys.executable, str(self.client), *client_args], **kw)

    def attach(self, *args, **kw):
        return self.term([HELPER, 'remote-attach', *args], **kw)

    def run_helper(self, *args):
        return subprocess.run([HELPER, *args], env=self.env, cwd=self.work, capture_output=True, text=True, timeout=15)

    def files(self):
        d = self.cca / 'remote'
        return sorted(p.name for p in d.iterdir()) if d.is_dir() else []

    def test_client_runs_as_before_and_exit_code_comes_back(self):
        h = self.host('--flag', 'two words')
        h.expect(r"<ready 24 80 leader=True ctty=True tty=True args=\['--flag', 'two words'\]>")
        # 会话目录只给自己，socket 和说明各一个；说明里是哪个账号、哪个文件夹，没有别的
        d = self.cca / 'remote'
        self.assertEqual(stat.S_IMODE(d.stat().st_mode), 0o700)
        names = self.files()
        self.assertEqual([n.split('.')[1] for n in names], ['json', 'sock'])
        self.assertEqual({stat.S_IMODE((d / n).stat().st_mode) for n in names}, {0o600})
        meta = json.loads((d / names[0]).read_text())
        self.assertEqual((meta['v'], meta['tool'], meta['key'], meta['name'], meta['cwd'], meta['pid']),
                         (1, 'claude', '工作(alice@example.com)', '工作', str(self.work), h.pid))
        self.assertEqual(set(meta), {'v', 'sid', 'pid', 'tool', 'key', 'name', 'cwd', 'since'})
        self.assertEqual(names[0], meta['sid'][:12] + '.json')
        # 按键过去、输出回来（中文也一样）
        h.send('say 你好\r')
        h.expect('<你好>')
        # 窗口大小变了，客户端跟着变
        h.resize(30, 100)
        h.expect('<size 30 100>')
        h.send('exit 7\r')
        self.assertEqual(h.wait(), 7)
        self.assertEqual(self.files(), [], '退出以后 socket 和说明都删掉')

    def test_environment_is_untouched(self):
        t = self.term([HELPER, 'remote-host', 'claude', 'k', 'n', '--', '/usr/bin/env'])
        self.assertEqual(t.wait(), 0)
        got = sorted(line for line in t.out.decode().replace('\r', '').split('\n') if line)
        self.assertEqual(got, sorted('%s=%s' % kv for kv in self.env.items()), '宿主不给客户端加、减环境变量')

    def test_killed_client_and_forwarded_signal(self):
        h = self.host()
        h.expect('<ready')
        h.send('kill\r')
        self.assertEqual(h.wait(), 128 + signal.SIGKILL, '客户端被信号杀掉：128 + 信号')
        h = self.host()
        h.expect('<ready')
        os.kill(h.pid, signal.SIGTERM)   # 发给宿主的转给客户端
        h.expect('<term>')
        self.assertEqual(h.wait(), 5)

    def test_slow_terminal_loses_nothing_when_the_client_exits(self):
        h = self.host()
        h.expect('<ready')
        mark = len(h.out)
        h.send('dump\r')
        time.sleep(1.5)   # 终端先不收：客户端写了两兆多就退出，宿主要等终端把它们都收完
        h.expect('<dumped>', timeout=30, since=mark)
        self.assertEqual(h.wait(), 6)
        lines = re.findall(rb'dump (\d{7}) \.+\r+\n', h.out[mark:])   # pty 把 \n 换成 \r\n，所以是 \r\r\n
        self.assertEqual([int(n) for n in lines], list(range(40000)), '一行不少、先后不乱')

    def test_client_closes_the_terminal_and_runs_on(self):
        h = self.host()
        h.expect('<ready')
        h.send('closetty\r')
        h.expect('<closing>')
        self.assertEqual(h.wait(), 4, '客户端关了终端还在跑：等到它真的退出，退出码照旧')
        self.assertLess(h.cpu, 0.5, '等的这一秒多不能空转')

    def test_without_a_terminal_the_client_runs_directly(self):
        r = subprocess.run([HELPER, 'remote-host', 'claude', 'k', 'n', '--', '/bin/sh', '-c', 'echo direct; exit 3'],
                           env=self.env, cwd=self.work, capture_output=True, text=True, timeout=15)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (3, 'direct\n', ''))
        self.assertEqual(self.files(), [])
        # 找不到客户端：和 env 一样 127
        r = subprocess.run([HELPER, 'remote-host', 'claude', 'k', 'n', '--', '/nonexistent/client'],
                           env=self.env, cwd=self.work, capture_output=True, text=True, timeout=15)
        self.assertEqual(r.returncode, 127)
        self.assertIn('/nonexistent/client', r.stderr)

    def test_terminal_gone_session_goes_on_and_can_be_attached(self):
        h = self.host()
        h.expect('<ready')
        h.send('say before\r')
        h.expect('<before>')
        self.assertIn('启动它的终端开着', self.run_helper('remote-list').stdout)
        h.hangup()
        deadline = time.monotonic() + 5
        while '没有终端接着' not in self.run_helper('remote-list').stdout:
            self.assertTrue(alive(h.pid), '终端没了宿主不能退出')
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.05)
        row = self.run_helper('remote-list').stdout
        sid8 = row.split()[0]
        self.assertRegex(row, r'^[0-9a-f]{8}  claude  工作  ~/项目 one  \d\d-\d\d \d\d:\d\d 开始  没有终端接着\n$')
        # 接进去：先看到之前的输出（回放），再能操作
        a = self.attach(rows=20, cols=60)
        a.expect('<before>')
        a.expect('<size 20 60>')   # 只有它一个：照它的大小
        a.send('say again\r')
        a.expect('<again>')
        self.assertIn('1 处接入', self.run_helper('remote-list').stdout)
        # 断开：Ctrl-\ 再按 d。会话还在
        a.send(b'\x1cd')
        self.assertEqual(a.wait(), 0)
        self.assertIn('已断开，会话还在运行（再接入：cca remote attach %s）' % sid8, a.out.decode())
        self.assertTrue(alive(h.pid))
        # 按编号的前缀再接入；会话结束时接入端的退出码是客户端的
        b = self.attach(sid8[:5])
        b.expect('<again>')
        b.send('exit 3\r')
        self.assertEqual(b.wait(), 3)
        self.assertIn('会话已经结束（退出码 3）', b.out.decode())
        self.assertEqual(h.wait(), 3)
        self.assertEqual(self.files(), [])

    def test_two_terminals_share_one_session(self):
        h = self.host(rows=24, cols=80)
        h.expect('<ready')
        a = self.attach(rows=40, cols=60)
        a.expect('<ready')                     # 回放
        h.expect('<size 24 60>')               # 两边都画得下的大小
        mark_h, mark_a = len(h.out), len(a.out)
        # 查询只发给最近有输入的那个终端：一开始是启动的终端
        h.send('query\r')
        h.expect('<asked>', since=mark_h)
        a.expect('<asked>', since=mark_a)
        self.assertIn(b'\x1b[6n', h.out[mark_h:])
        self.assertIn(b'\x1b]11;?\x07', h.out[mark_h:])
        self.assertNotIn(b'\x1b[6n', a.out[mark_a:])
        self.assertNotIn(b'\x1b]11;?', a.out[mark_a:])
        # 接入端敲了键以后是它
        mark_h, mark_a = len(h.out), len(a.out)
        a.send('query\r')
        a.expect('<asked>', since=mark_a)
        h.expect('<asked>', since=mark_h)
        self.assertIn(b'\x1b[6n', a.out[mark_a:])
        self.assertNotIn(b'\x1b[6n', h.out[mark_h:])
        # 只看的接入：按键不过去
        r = self.attach('-r', rows=50, cols=120)
        r.expect('<ready')
        mark_h = len(h.out)
        r.send('say from-reader\r')
        h.send('say from-host\r')
        h.expect('<from-host>', since=mark_h)
        self.assertNotIn(b'from-reader', h.out[mark_h:])
        self.assertIn('另有 2 处接入', self.run_helper('remote-list').stdout)
        # 小的那个走了，大小回到启动的终端的
        mark_h = len(h.out)
        a.send(b'\x1c.')
        self.assertEqual(a.wait(), 0)
        h.expect('<size 24 80>', since=mark_h)
        h.send('exit 0\r')
        self.assertEqual(h.wait(), 0)
        self.assertEqual(r.wait(), 0)
        self.assertIn('会话已经结束', r.out.decode())

    def test_only_the_primary_terminal_gets_what_is_not_plainly_safe(self):
        h = self.host()
        h.expect('<ready')
        a = self.attach()
        a.expect('<ready')
        mark_h, mark_a = len(h.out), len(a.out)
        h.send('evil\r')
        h.expect('<done>', since=mark_h)
        a.expect('<done>', since=mark_a)
        # 主的那个（启动的终端）收到的和客户端写的逐字节一样
        self.assertIn(b'<evil>\x1b\x1b[c]52;c;ZXZpbA==\x07\x1b[' + b'0;' * 300 + b'c<done>', h.out[mark_h:])
        # 另一个：残缺的 ESC、查询、超长的序列都没有；那截字只是字，前面没有 ESC
        got = a.out[mark_a:]
        self.assertIn(b'<evil>]52;c;ZXZpbA==\x07<done>', got)
        self.assertNotIn(b'\x1b]52', got)
        self.assertNotIn(b'\x1b[c', got)
        # 写剪贴板只给主的那个
        mark_h, mark_a = len(h.out), len(a.out)
        h.send('clip\r')
        h.expect('<clipped>', since=mark_h)
        a.expect('<clipped>', since=mark_a)
        self.assertIn(b'\x1b]52;c;Y2xpcA==\x07', h.out[mark_h:])
        self.assertNotIn(b'52;c;Y2xpcA', a.out[mark_a:])
        # 后接入的：回放里也没有
        late = self.attach()
        late.expect('<clipped>')
        self.assertNotIn(b'\x1b]52', late.out)
        self.assertNotIn(b'Y2xpcA', late.out)
        self.assertNotIn(b'\x1b[c', late.out)
        self.assertIn(b'<evil>]52;c;ZXZpbA==<done>', late.out, '回放里那截字还在（响铃去掉了）')
        h.send('exit 0\r')
        self.assertEqual(h.wait(), 0)

    def test_late_viewer_gets_the_modes_and_puts_its_terminal_back(self):
        h = self.host()
        h.expect('<ready')
        h.send('alt\r')
        h.expect('<alt>')
        # 把打开模式的那几条序列挤出回放：后接入的照样要先进备用屏幕
        h.send('flood\r')
        h.expect('<flooded>', timeout=30)
        a = self.attach()
        end = a.expect('<flooded>')
        first = a.out[:200]
        self.assertTrue(first.startswith(b'\x1b[?1049h'), first)
        self.assertIn(b'\x1b[?25l', first)
        self.assertIn(b'\x1b[?2004h', first)
        self.assertNotIn(b'<alt>', a.out[:end], '回放只留最近的')
        self.assertLess(end, 400 * 1024)
        a.send(b'\x1cd')
        self.assertEqual(a.wait(), 0)
        tail = a.out[end:]
        for seq in (b'\x1b[?25h', b'\x1b[?2004l', b'\x1b[0m', b'\x1b[?1049l'):
            self.assertIn(seq, tail)
        self.assertLess(tail.index(b'\x1b[?2004l'), tail.index(b'\x1b[?1049l'), '最后才离开备用屏幕')
        h.send('exit 0\r')
        self.assertEqual(h.wait(), 0)

    def test_attach_from_a_terminal_the_app_owns(self):
        h = self.host()
        h.expect('<ready 24 80')
        v = AppTerm([HELPER, 'remote-attach'], self.env)
        self.terms.append(v)
        v.expect('<ready 24 80')
        # 接入端把这个终端认成了控制终端：窗口大小变了它收得到信号、告诉宿主（不认的话会话的大小永远不跟着变）
        v.resize(20, 70)
        h.expect('<size 20 70>')
        v.send('say typed-in-the-app\n')
        h.expect('<typed-in-the-app>')
        v.expect('<typed-in-the-app>')
        # App 的「断开」是把终端关掉：接入端断开（退出码 0），会话接着运行、大小回到启动它的终端的
        v.hangup()
        self.assertEqual(v.wait(), 0)
        h.expect('<size 24 80>')
        h.send('say still-running\n')
        h.expect('<still-running>')
        h.send('exit 0\n')
        self.assertEqual(h.wait(), 0)

    def test_attach_errors(self):
        r = self.run_helper('remote-attach')
        self.assertEqual((r.returncode, r.stdout), (65, ''))
        self.assertIn('现在没有可以接入的会话', r.stderr)
        self.assertEqual(self.run_helper('remote-attach', 'not-hex').returncode, 64)
        h1 = self.host()
        h1.expect('<ready')
        h2 = self.host()
        h2.expect('<ready')
        rows = self.run_helper('remote-list').stdout.splitlines()
        self.assertEqual(len(rows), 2)
        # 两个会话：要说接入哪个
        r = self.run_helper('remote-attach')
        self.assertEqual(r.returncode, 64)
        self.assertIn('有 2 个会话', r.stderr)
        self.assertEqual(self.run_helper('remote-attach', 'ffffffffffff0000').returncode, 65)
        # 不在终端里：不接
        r = self.run_helper('remote-attach', rows[0].split()[0])
        self.assertEqual(r.returncode, 69)
        self.assertIn('要在终端里运行', r.stderr)
        # 宿主被强行结束：接入端说连接断了；它留下的文件下一次列表时清掉
        a = self.attach(rows[0].split()[0])
        a.expect('<ready')
        target = h1 if rows[0].split()[0] in json.dumps(self.metas(h1.pid)) else h2
        os.kill(target.pid, signal.SIGKILL)
        self.assertEqual(a.wait(), 68)
        self.assertIn('连接断了', a.out.decode())
        target.wait()
        self.assertEqual(len(self.files()), 4)
        self.assertEqual(len(self.run_helper('remote-list').stdout.splitlines()), 1)
        self.assertEqual(len(self.files()), 2)

    def test_end_a_session(self):
        """remote-end：等于关掉会话的终端。里面的程序收到 SIGHUP；不理的过几秒被强行结束"""
        r = self.run_helper('remote-end')
        self.assertEqual((r.returncode, r.stdout), (65, ''))
        self.assertIn('没有在会话宿主里运行的会话', r.stderr)
        self.assertEqual(self.run_helper('remote-end', 'not-hex').returncode, 64)
        h1 = self.host()
        h1.expect('<ready')
        # 不理 SIGHUP 的客户端（忽略是连它起的进程一起的）
        h2 = self.term([HELPER, 'remote-host', 'codex', 'k', '倔', '--', '/bin/sh', '-c',
                        'trap "" HUP; echo stubborn; while :; do sleep 1; done'])
        h2.expect('stubborn')
        # 两个会话：要说结束哪个；--show 只找出是哪一个，不动它
        r = self.run_helper('remote-end')
        self.assertEqual((r.returncode, r.stdout), (64, ''))
        self.assertIn('有 2 个会话', r.stderr)
        self.assertEqual(self.run_helper('remote-end', 'ffffffffffff0000').returncode, 65)
        (m1,), (m2,) = self.metas(h1.pid), self.metas(h2.pid)
        r = self.run_helper('remote-end', '--show', m1['sid'][:6].upper())
        self.assertEqual((r.returncode, r.stderr), (0, ''))
        sid, row = r.stdout.rstrip('\n').split('\t')
        self.assertEqual(sid, m1['sid'])
        self.assertRegex(row, r'^%s  claude  工作  ~/项目 one  .* 开始  启动它的终端开着$' % sid[:8])
        self.assertTrue(alive(h1.pid))
        # 接着的各处都收到“会话结束了”；客户端是被 SIGHUP 结束的（128 + 1），文件都清掉
        h1.hangup()
        a = self.attach(sid[:8])
        a.expect('<ready')
        r = self.run_helper('remote-end', sid)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, '', ''))
        self.assertEqual((a.wait(), h1.wait()), (129, 129))
        self.assertIn('会话已经结束（退出码 129）', a.out.decode())
        self.assertEqual(self.metas(h1.pid), [])
        # 不理 SIGHUP 的：过几秒被强行结束（128 + 9），命令等到它真的结束才回来
        start = time.monotonic()
        r = self.run_helper('remote-end', m2['sid'][:8])
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, '', ''))
        self.assertGreater(time.monotonic() - start, 4)
        self.assertEqual(h2.wait(), 137)
        self.assertEqual(self.files(), [])
        self.assertEqual(self.run_helper('remote-end', sid).returncode, 65, '已经结束的：没有这个会话')

    def test_a_host_that_does_not_answer_is_not_cleaned_up(self):
        h = self.host()
        h.expect('<ready')
        os.kill(h.pid, signal.SIGSTOP)   # 被挂起的宿主不回答：不能当它不在了、把它的 socket 删掉
        try:
            row = self.run_helper('remote-list').stdout
            self.assertRegex(row, r'^[0-9a-f]{8}  claude  工作  .* 开始  没有回应（可能被挂起了、正忙）\n$')
            self.assertEqual(len(self.files()), 2)
        finally:
            os.kill(h.pid, signal.SIGCONT)
        self.assertIn('启动它的终端开着', self.run_helper('remote-list').stdout)
        h.send('exit 0\r')
        self.assertEqual(h.wait(), 0)
        self.assertEqual(self.files(), [])

    def metas(self, pid):
        return [m for m in (json.loads(p.read_text()) for p in (self.cca / 'remote').glob('*.json')) if m['pid'] == pid]

    def test_switch(self):
        cfg = self.cca / 'config.json'
        self.assertEqual(self.run_helper('remote-set', 'on').returncode, 0)
        self.assertIs(json.loads(cfg.read_text())['remote'], True)
        self.assertEqual(self.run_helper('remote-set', 'off').returncode, 0)
        self.assertNotIn('remote', json.loads(cfg.read_text()))
        self.assertEqual(self.run_helper('remote-set', 'maybe').returncode, 1)
        self.assertEqual(self.run_helper('remote-list').stdout, '')


class DeepHome(RemoteHost):
    """~/.cca 的路径很长（socket 的地址放不下整个路径）：照样能开、能列、能接入"""

    @staticmethod
    def cca_home(home):
        return home / ('d' * 60) / ('e' * 60) / '.cca'


if __name__ == '__main__':
    unittest.main()
