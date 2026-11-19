#!/usr/bin/env python3
"""把四个平台的辅助程序接到 cca 后面（发布、回归里的 mkver 用）：

    python3 -I cli/pack.py 改写好的cca 输出文件 [目标=辅助程序的路径 …]

不写「目标=路径」就用 cli/target/<目标>/release/cca-core（先在 cli/ 里把四个目标都编出来，见 cli/.cargo/config.toml）。
做两件事：`HELPER_SUMS=""` 那一行换成各目标的 SHA-256（cca 第一次运行时按它校验解出来的文件，也按它给文件起名）；
在文件最后（`exit` 的后面，bash 运行时读不到）接上各目标的辅助程序：gzip（不带时间戳，同样的输入出同样的字节）再 base64，
包在 `: <<'CCA-HELPER-END'` 的 here-document 里（bash -n 检查发布的文件时这样最快：7 MB 也只要 0.03 秒）。"""
import base64
import gzip
import hashlib
import io
import os
import sys

TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl']
HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    src, out = sys.argv[1], sys.argv[2]
    paths = {t: os.path.join(HERE, 'target', t, 'release', 'cca-core') for t in TARGETS}
    for spec in sys.argv[3:]:
        t, _, p = spec.partition('=')
        if t not in TARGETS or not p:
            sys.exit('pack.py：参数写成 目标=路径，目标是 %s 之一' % '、'.join(TARGETS))
        paths[t] = p
    s = io.open(src, encoding='utf-8').read()
    marker = 'HELPER_SUMS=""   # 发布时由 cli/pack.py 填上：一行一个「目标 SHA-256」\n'
    if s.count(marker) != 1:
        sys.exit('pack.py：%s 里没有（或者不止一处）HELPER_SUMS="" 那一行：先用 cli/switch.py 改写' % src)
    if not s.endswith('\nmain "$@"\nexit\n'):
        sys.exit('pack.py：%s 的结尾不是 main "$@" 和 exit（已经接过辅助程序了？）' % src)
    sums, tail = [], []
    for t in TARGETS:
        try:
            raw = open(paths[t], 'rb').read()
        except OSError as e:
            sys.exit('pack.py：读不了 %s 的辅助程序（%s）：先编出来，或者用 %s=路径 指定' % (t, e, t))
        sums.append('%s %s' % (t, hashlib.sha256(raw).hexdigest()))
        packed = base64.encodebytes(gzip.compress(raw, 9, mtime=0)).decode('ascii')
        tail.append("#CCA-HELPER %s\n: <<'CCA-HELPER-END'\n%sCCA-HELPER-END\n" % (t, packed))
    s = s.replace(marker, 'HELPER_SUMS="%s"\n' % '\n'.join(sums)) + ''.join(tail)
    with io.open(out, 'w', encoding='utf-8') as f:
        f.write(s)
    os.chmod(out, 0o755)
    print('pack.py：%s（%d 字节，辅助程序 %s）' % (out, len(s.encode('utf-8')), '、'.join('%s %.1f MB' % (t.split('-')[0] + ('-mac' if 'darwin' in t else '-linux'), os.path.getsize(paths[t]) / 1e6) for t in TARGETS)))


main()
