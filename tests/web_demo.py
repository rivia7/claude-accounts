#!/usr/bin/env python3
"""在本机把远程会话的网页端跑起来看（不联网、不碰真实的 ~/.cca）：一个真的服务器（网页端开在 http://localhost）、一台临时的
“电脑”（真的 cca 辅助程序，上面一个在宿主里运行的会话），再替它开一次给网页端的配对。照打印出来的地址在这台 Mac 的浏览器里
打开、输入配对码，就能接进那个会话。手机上试不了这个（手机要 HTTPS 和信得过的证书：见 docs/dev/remote.md「网页端」）。

用法：python3 tests/web_demo.py [--port 端口] [--] [会话里跑的命令…]（默认是 tests/remote_host.py 的假客户端；比如 -- vim、-- bash -l）
Ctrl-C 结束。"""
import argparse
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import remote_relay as rr   # noqa: E402


class Demo(rr.Base):
    def runTest(self):
        pass


def main():
    ap = argparse.ArgumentParser(description='本机试远程会话的网页端')
    ap.add_argument('--port', type=int, default=0, help='网页端听哪个端口（默认随便挑一个）')
    ap.add_argument('cmd', nargs='*', help='会话里跑的命令（默认是测试用的假客户端）')
    a = ap.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))   # 被叫停也照样收场（停掉服务器、会话）
    t = Demo()
    port = a.port
    if not port:   # 先挑一个空着的端口：页面的地址（--web-url）里要写它
        s = socket.socket()
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
        s.close()
    t.server_extra = lambda d: ['--web-url', 'http://localhost:%d/' % port, '--web-listen', '127.0.0.1:%d' % port]
    t.setUp()
    try:
        box = t.device('box')
        t.machines(box)
        cmd = a.cmd or [sys.executable, str(box.home / 'client.py')]
        host = rr.Term([rr.HELPER, 'remote-host', 'claude', rr.Device.KEY, 'alice', '--', *cmd], box.env, str(box.work))
        t.terms.append(host)
        print('网页端：http://localhost:%d/' % port, flush=True)
        while True:
            p = subprocess.Popen([rr.HELPER, 'remote-pair-show', '--web'], env=box.env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True)
            line = p.stdout.readline()
            if not line.startswith('code '):
                print('没开成配对：' + p.stderr.read(), flush=True)
                break
            print('配对码：' + line.split()[1], flush=True)
            p.stdout.readline()
            while p.poll() is None:
                host.pump(0.2)
            out = p.stdout.read()
            print('已配对：' + out.strip() if p.returncode == 0 else '这个配对码没用上（%s），换一个' % p.stderr.read().strip(), flush=True)
            for f in (p.stdout, p.stderr):
                f.close()
            if p.returncode == 0:
                break
        while host.pump(0.2):   # 会话结束（宿主退出）为止
            host.out = b''
        print('会话结束了（服务器还开着：页面上看得到“没有可以接入的会话”；Ctrl-C 结束）', flush=True)
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        t.doCleanups()


if __name__ == '__main__':
    main()
