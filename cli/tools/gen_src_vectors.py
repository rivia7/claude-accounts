#!/usr/bin/env python3
"""生成 cli/tests/vectors/src_*.json：原来的 Python（cca 里内嵌的 PY_SRC）里几个函数对一批输入给出的结果，Rust 的移植照着
核对（cargo test 读这些文件）。给对拍（cli/pydiff）够不着的地方用：结果被掩码盖住的、要等很久才走得到的（上报进程隔 30 秒
才采一次样的 CPU 判断）。

    python3 -I cli/tools/gen_src_vectors.py cli/tests/vectors

PY_SRC 是从带着它的那一份 cca 里取的：仓库里的 cca 切换以后没有它了，就从 cli/PORTED_FROM 记的那个主干提交里取
（和对拍一样，见 cli/pydiff/run.py 的 oracle_cca）。
函数在一个不存在的 CCA_HOME 下跑（和 cargo test 用的是同一个，见 cli/.cargo/config.toml），不读写任何真的文件。"""
import json
import os
import subprocess
import sys
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
CCA_HOME = '/nonexistent/cca-unit-tests'
SALT = 'ab' * 16


def load():
    head = "PY_SRC=$(cat <<'PY'\n"
    with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
        text = f.read()
    if head not in text:
        with open(os.path.join(REPO, 'cli', 'PORTED_FROM')) as f:
            rev = f.read().split()[0]
        text = subprocess.run(['git', '-C', REPO, 'show', rev + ':cca'], stdout=subprocess.PIPE, check=True).stdout.decode('utf-8')
    src = text.split(head, 1)[1].split('\nPY\n)', 1)[0]
    ns = {}
    with mock.patch.dict(os.environ, {'HOME': '/nonexistent', 'CCA_HOME': CCA_HOME, 'CCA_TOOL': 'claude', 'TZ': 'Asia/Shanghai'}, clear=True):
        exec(compile(src.split('\nif len(sys.argv) < 2', 1)[0], 'PY_SRC', 'exec'), ns)
    return ns


def presence(ns):
    out = {}
    times = ['0:00.03', '12:34.56', '1:02:03', '1-02:03:04.50', '2-00:00:00', '', 'x', '1:x', '-5', '1-', '-', '1-2-3:04', '00:60', '1:2:3:4', ' 5',
             '5 ', '1_0:00', 'inf', 'nan:1', '1e3', '٣:٠٥', '0x10', '1-0:0:0.5', 'a-1:00', '0:00.00', '999:59.99', '100-23:59:59.99', '1.5-0:01', '+1-0:01',
             '0:0:0:0:1', ':', '1:', ':1', '-1:00', '1--2:00', '0.1:0.2:0.3']
    out['cpu_secs'] = [[s, repr(ns['cpu_secs'](s))] for s in times]

    tool, key = ns['CLAUDE'], 'alice@example.com'
    with ns['using'](tool):
        d = ns['adir'](key)
    desk = os.path.join(d, 'desktop')
    app = '/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s --proxy-server=http://192.0.2.10:7901' % desk

    def table(main, child, start, with_app=True):
        procs = {1: (0, '??', 12.5, 1000, '/sbin/launchd'), 777: (1, 'ttys001', 3.0, start, 'unrelated --user-data-dir=/elsewhere')}
        if with_app:
            procs.update({501: (500, '??', child, start + 1, 'Claude Helper --type=renderer'), 500: (1, '??', main, start, app),
                          502: (501, '??', 0.1, start + 2, 'Claude Helper (GPU)'), 503: (502, '??', 0.2, start + 3, 'grandchild'),
                          504: (500, '??', 0.7, start + 3, 'Claude Helper --type=utility')})
        return procs
    now0, old = 1795000000.75, 1794000000
    steps, apps = [], {}
    for label, now, mono, procs in [
        ('开了很久的 App 第一次见到：先当作刚好闲置满 10 分钟', now0, 5000.5, table(100.0, 50.25, old)),
        ('隔了不到 30 秒：不采样', now0 + 20, 5020.5, table(190.0, 60.0, old)),
        ('隔了 30 秒，CPU 没怎么动：还是闲置', now0 + 30.5, 5031.0, table(100.5, 50.25, old)),
        ('又隔了 60 秒，平均用了 3% 多一点：有动静', now0 + 90.5, 5091.0, table(101.5, 51.06, old)),
        ('再隔 60 秒，正好差一点不到 3%', now0 + 150.5, 5151.0, table(102.5, 51.85, old)),
        ('隔 45 秒，用了很多', now0 + 195.5, 5196.0, table(140.0, 60.0, old)),
        ('App 重启了（进程号一样、启动时间不一样）：刚打开的算在用', now0 + 200, 5200.5, table(1.0, 0.5, int(now0) + 190)),
        ('没有 App 了：采样清掉', now0 + 260, 5260.5, table(0, 0, old, with_app=False)),
        ('又开了：重新算', now0 + 300, 5300.5, table(3.0, 0.5, int(now0) + 295)),
    ]:
        units, pids = ns['presence_scan'](SALT, [(tool, key, d)], procs, apps, now, mono)
        steps.append({'label': label, 'now': now, 'mono': mono, 'procs': [[pid] + list(p) for pid, p in procs.items()], 'units': units,
                      'pids': sorted(pids), 'apps': sorted([k[0], k[1]] + [repr(float(x)) for x in v] for k, v in apps.items())})
    out['scan'] = {'salt': SALT, 'key': key, 'dir': d, 'steps': steps}
    return out


def main():
    dest = sys.argv[1]
    ns = load()
    for name, data in [('src_presence', presence(ns))]:
        with open(os.path.join(dest, name + '.json'), 'w', encoding='utf-8') as f:
            json.dump(data, f, ensure_ascii=False, indent=1)
            f.write('\n')
        print('%s.json' % name)


main()
