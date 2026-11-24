"""cca 的辅助程序（cli/ 的 Rust 程序 cca-core）在哪：tests/ 下直接运行它的测试用。

CCA_HELPER 指的那一份（release.sh、TIMING 跑回归时带着），没有就是仓库里编出来的 cli/target/release/cca-core
（回归开头会编；单独跑某个测试之前先 cd cli && cargo build --release）。

Team 的同步服务器、证书助手用哪一份跑也在这里定：SERVER_ARGV、ACME_ARGV 是命令的开头（后面接参数）。平常是 server/ 下 Python 的
那两份，命令和以前一个字不差；CCA_TEST_SERVER=rust 时是辅助程序里的 Rust 版（cca-core team-server、cca-core team-acme）。
regress.sh 的 tm_srv、hub_box.py 的假 systemctl / docker 认的是同一个开关。RUST_SERVER 为真时，不经服务器进程、直接调 Python
那一份里的函数的用例跳过（PY_ONLY 是跳过时说的话）：Rust 版的那些在 cargo test 里测。"""
import os
from pathlib import Path
import sys

HELPER = os.environ.get('CCA_HELPER') or str(Path(__file__).resolve().parents[1] / 'cli' / 'target' / 'release' / 'cca-core')
if not os.access(HELPER, os.X_OK):
    raise SystemExit('辅助程序不在：%s（先 cd cli && cargo build --release）' % HELPER)

_SERVER_DIR = Path(__file__).resolve().parents[1] / 'server'
RUST_SERVER = os.environ.get('CCA_TEST_SERVER') == 'rust'
SERVER_ARGV = [HELPER, 'team-server'] if RUST_SERVER else [sys.executable, '-I', str(_SERVER_DIR / 'cca-presence.py')]
ACME_ARGV = [HELPER, 'team-acme'] if RUST_SERVER else [sys.executable, '-I', str(_SERVER_DIR / 'cca-acme.py')]
PY_ONLY = '直接调 server/ 下 Python 那一份里的函数（CCA_TEST_SERVER=rust 时不算数：Rust 版的在 cargo test 里测）'


def exited(proc):
    """等服务器写下端口的地方用：它没起来就退出了时，是哪条命令、退出码、在 stderr 上说了什么（stderr 接着管道时）——不然要干等到超时，
    看到的也只是“没等到”。还在运行是 None"""
    if proc.poll() is None:
        return None
    said = proc.stderr.read().decode('utf-8', 'replace').strip() if proc.stderr and not proc.stderr.closed else ''
    return '服务器没起来就退出了（%s，退出码 %s）%s' % (' '.join(SERVER_ARGV), proc.returncode, '：' + said if said else '')
