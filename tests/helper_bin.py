"""cca 的辅助程序（cli/ 的 Rust 程序 cca-core）在哪：tests/ 下直接运行它的测试用。

CCA_HELPER 指的那一份（release.sh、TIMING 跑回归时带着），没有就是仓库里编出来的 cli/target/release/cca-core
（回归开头会编；单独跑某个测试之前先 cd cli && cargo build --release）。"""
import os
from pathlib import Path

HELPER = os.environ.get('CCA_HELPER') or str(Path(__file__).resolve().parents[1] / 'cli' / 'target' / 'release' / 'cca-core')
if not os.access(HELPER, os.X_OK):
    raise SystemExit('辅助程序不在：%s（先 cd cli && cargo build --release）' % HELPER)
