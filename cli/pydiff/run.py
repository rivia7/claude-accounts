#!/usr/bin/env python3
"""对拍：同一份沙箱状态上分别跑原来的 Python（cca 里的 PY_SRC）和 Rust 的 cca-core，逐字节比 stdout、stderr、退出码
和跑完以后沙箱里的文件。有差别就是移植错了（或者主干改了 PY_SRC，Rust 还没跟上）。

    cargo build && python3 -I cli/pydiff/run.py            全部用例
    python3 -I cli/pydiff/run.py -k policy                  名字里带 policy 的
    python3 -I cli/pydiff/run.py -v -k keys/alias           失败时把两边的输出全打出来
    python3 -I cli/pydiff/run.py -s -k env/codex            通过的也打出输出（确认用例真的走到了想测的那条路）
    CCA_CORE=cli/target/release/cca-core python3 -I …       换一个 cca-core

用例在 cli/pydiff/cases/ 下，一个模块一份文件（写法见 cases/table.py 开头）。沙箱是临时目录里的假 HOME：不碰真的
~/.cca，不联网；要起外部命令的用例自己在沙箱的 bin/ 里放假命令。只是开发时用的工具，不随 cca 发布。"""
import difflib
import hashlib
import importlib
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CLI = os.path.dirname(HERE)
REPO = os.path.dirname(CLI)
CORE = os.path.abspath(os.environ.get('CCA_CORE') or os.path.join(CLI, 'target', 'debug', 'cca-core'))
CASES = []
# 切换成 Rust 以后有意加的输出（子命令 → Rust 的 stdout 里多出来的那几行）：比之前先去掉，别的照旧要逐字节一样。
# env 的 CFG_REMOTE：远程会话的开关（docs/dev/remote.md），原来的 Python 没有
ADDED = {'env': re.compile(r'^CFG_REMOTE=.*\n', re.M)}


_ORACLE = {}
PY_HEAD = "PY_SRC=$(cat <<'PY'\n"
# 对照的 Python 上打的补丁：切换以后有意改了的常量（docs/dev/rust.md「测试」：改了行为的子命令，用例删掉或改掉）。每一处都核对原文，
# 对不上（取的提交换了）就报错。改了的行为本身在 cargo test、tests/*.py、回归里测
ORACLE_PATCHES = [
    # 映射里多了 team 一节（给各台机器的 Team 邀请，./policy.sh team 写的）：发布前的检查认得它，提示里列的字段多一个
    ("        errs += ['不认识的字段 %s（只能有 version、%s、aliases、presence、install_node）' % (k, '、'.join(names))\n"
     "                 for k in j if k not in ('version', 'aliases', 'presence', 'install_node') and k not in names]",
     "        errs += ['不认识的字段 %s（只能有 version、%s、aliases、presence、install_node、team）' % (k, '、'.join(names))\n"
     "                 for k in j if k not in ('version', 'aliases', 'presence', 'install_node', 'team') and k not in names]"),
    # 折合金额读会话记录的规则变了（按会话开始的记录认账号、记下诊断），scan.json 要从头重算一遍：两边用同一个 build，别的照旧比
    ("COST_BUILD = COST_PRICES_DATE + '/1'", "COST_BUILD = COST_PRICES_DATE + '/2'"),
    # 内置代理的节点定义：同名时以本机的自定义节点为准（排在拷来的前面；自定义节点写进路由器以后，拷来的里面也有它）
    ("    px = [p for p in core_cache().get('proxies', []) if isinstance(p, dict) and isinstance(p.get('name'), str)] + core_custom()",
     "    px = core_custom() + [p for p in core_cache().get('proxies', []) if isinstance(p, dict) and isinstance(p.get('name'), str)]"),
    # node-check 后面可以跟要避开的名字（在家时路由器上全部节点和策略组的名字）
    ('def cmd_node_check():', 'def cmd_node_check(*extra):'),
    ('    while not node_name_free(c, name):', '    while name in extra or not node_name_free(c, name):'),
    # 本机 UDP 走节点：root 那边的 TUN 网卡名固定、自己的地址换开 198.18.0.1、只给 route-address（排掉的以外每一段劈成两半，不再和别的
    # TUN 的大段一样大；这里用 ipaddress 另算一遍），启动脚本给别的 VPN 让开：UDP_HELPER 加 1。udp-env 多的两行、gui-state 多的
    # other_vpn 用屏蔽规则去掉（cases/corenet.py 的 UDP_ENV_MASK、cases/gui.py 的 STATE_MASK）
    ('UDP_HELPER = 1   #', 'UDP_HELPER = 2   #'),
    ("UDP_EXCLUDE = ['10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8', '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16',\n"
     "               '224.0.0.0/4', '240.0.0.0/4', 'fc00::/7', 'fe80::/10', 'ff00::/8']   # 不进 TUN：局域网、组播、链路本地、CGNAT（Tailscale）\n",
     "UDP_EXCLUDE = ['0.0.0.0/8', '10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8', '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16',\n"
     "               '198.18.0.0/15', '224.0.0.0/4', '240.0.0.0/4', '::/8', 'fc00::/7', 'fe80::/10', 'ff00::/8']\n\n\n"
     "def udp_routes():\n"
     "    import ipaddress\n"
     "    out = []\n"
     "    for top in map(ipaddress.ip_network, ('0.0.0.0/0', '::/0')):\n"
     "        nets = [top]\n"
     "        for e in (n for n in map(ipaddress.ip_network, UDP_EXCLUDE) if n.version == top.version):\n"
     "            nets = [m for n in nets for m in (n.address_exclude(e) if e.subnet_of(n) else [] if n.subnet_of(e) else [n])]\n"
     "        out += [str(h) for n in ipaddress.collapse_addresses(nets) for h in n.subnets(1)]\n"
     "    return out\n"),
    ("           'tun': {'enable': True, 'stack': 'mixed', 'auto-route': True, 'auto-detect-interface': True, 'strict-route': False,\n"
     "                   'dns-hijack': [], 'route-exclude-address': UDP_EXCLUDE},",
     "           'dns': {'fake-ip-range': '198.19.233.233/30'},\n"
     "           'tun': {'enable': True, 'stack': 'mixed', 'device': 'utun233', 'auto-route': True, 'auto-detect-interface': True,\n"
     "                   'strict-route': False, 'dns-hijack': [], 'route-address': udp_routes()},"),
    # .claude.json 的三方合并（sync）：合并好的 projects 里给交给 Claude Code 的每个 --add-dir（账号目录、共享目录的真实路径）记上
    # hasTrustDialogAccepted（Claude Code 2.1.329 起，没信任过的 --add-dir 每次启动都要问一遍）。照 Rust 的 add_dirs、trusted_dirs、
    # trust_dirs 另写一遍；这里的 CLAUDE_SHARED 是集合，顺序照 Rust 的列表写死
    ('def claude_sync(key):\n',
     "CCA_SHARED_ORDER = ['projects', 'skills', 'commands', 'agents', 'plugins', 'todos', 'tasks', 'plans', 'file-history', 'ide',\n"
     "                    'settings.json', 'settings.local.json', 'history.jsonl', 'CLAUDE.md', 'CLAUDE.local.md', 'AGENTS.md',\n"
     "                    'AGENTS.override.md', 'keybindings.json', 'rules', 'output-styles', 'themes', 'workflows', 'agent-memory',\n"
     "                    'agent-memory-local', 'memory', 'memories', 'prompts', 'hooks']\n\n\n"
     "def cca_add_dirs(key):\n"
     "    c = load_quiet(CFG)\n"
     "    root = os.path.realpath(CCA_HOME)\n"
     "    if (isinstance(c, dict) and c.get('claude_add_dir') is False) or re.search(r'[\\s,()\\x22\\x27\\\\]', root):\n"
     "        return []\n"
     "    home = os.path.realpath(HOME)\n"
     "    within = lambda p, d: p == d or p.startswith(d.rstrip('/') + '/')\n"
     "    out = []\n"
     "    for n in CCA_SHARED_ORDER:\n"
     "        link = os.path.join(cdir(key), n)\n"
     "        if n == 'ide' or not os.path.islink(link):\n"
     "            continue\n"
     "        real = os.path.realpath(link)\n"
     "        if (os.path.isdir(real) and not re.search(r'[\\s,()\\x22\\x27\\\\]', real) and not within(home, real)\n"
     "                and not within(root, real) and not within(real, root) and real not in out):\n"
     "            out.append(real)\n"
     "    return [adir(key)] + out\n\n\n"
     "def trust_dirs(v, key):\n"
     "    dirs = []\n"
     "    for d in cca_add_dirs(key):\n"
     "        for p in (d, os.path.realpath(d)):\n"
     "            if p not in dirs:\n"
     "                dirs.append(p)\n"
     "    if not dirs:\n"
     "        return v\n"
     "    if v is MISSING:\n"
     "        projects = {}\n"
     "    elif isinstance(v, dict):\n"
     "        projects = dict(v)\n"
     "    else:\n"
     "        return v\n"
     "    for d in dirs:\n"
     "        e = projects.get(d)\n"
     "        if isinstance(e, dict) and e.get('hasTrustDialogAccepted') is True:\n"
     "            continue\n"
     "        e = dict(e) if isinstance(e, dict) else {}\n"
     "        e['hasTrustDialogAccepted'] = True\n"
     "        projects[d] = e\n"
     "    return projects\n\n\n"
     "def claude_sync(key):\n"),
    ('            conflict |= collided\n            if v is MISSING:\n',
     "            conflict |= collided\n            if k == 'projects':\n                v = trust_dirs(v, key)\n            if v is MISSING:\n"),
    # 维护者的共享默认关着：config.json 里没有 policy（映射仓库）时，磁盘上留着的映射、参考节点一律不生效（辅助程序的 policy_on；
    # cca policy off 时后台正在拉的那一次可能之后才写回来）；没有 config.json 也是关着，读不出对象时照旧看磁盘上的映射。
    # Box.policy / Box.fill 放了映射的沙箱，config.json 里自动带上 policy
    ("    \"\"\"读不了、格式不对都当作没有（缓存之类，不值得为它报错退出）\"\"\"\n    try:\n",
     "    \"\"\"读不了、格式不对都当作没有（缓存之类，不值得为它报错退出）\"\"\"\n"
     "    if path in (POLICY_FILE, FILL_FILE):\n"
     "        try:\n"
     "            with open(CFG, encoding='utf-8') as f:\n"
     "                c = json.load(f)\n"
     "        except FileNotFoundError:\n"
     "            return None\n"
     "        except (OSError, ValueError):\n"
     "            c = None\n"
     "        if isinstance(c, dict) and not c.get('policy'):\n"
     "            return None\n"
     "    try:\n"),
    # 共享关着时设备统计、锁定请求说的是「没打开维护者的共享（cca policy on）」；开着、还没拉到映射时说还没拉到
    ("        return '这台 Mac 没有配置维护者的映射（cca policy setup）', None\n",
     "        off = c is None and not os.path.exists(CFG) or isinstance(c, dict) and not c.get('policy')\n"
     "        return ('这台 Mac 没打开维护者的共享（cca policy on）' if off else '这台 Mac 还没拉到维护者的映射（cca policy pull）'), None\n"),
    ("'这台 Mac 没有映射的 deploy key（cca policy setup）'", "'这台 Mac 没有映射的 deploy key（cca policy on）'"),
    ("'这台机器还没配置维护者的映射：先运行 cca policy setup'", "'这台机器没打开维护者的共享：先运行 cca policy on'"),
    # 自己的同步服务器（docs/dev/hub.md）：加入信息、这台设备在它上面的私钥、部署到托管平台还没加入时的口令也不给 Claude 读写；
    # Team（docs/dev/team.md）的整个目录也是
    ("CCA_DIR_PRIVATE = ('policy/id_ed25519', 'policy/nodes.json', ",
     "CCA_DIR_PRIVATE = ('policy/id_ed25519', 'policy/nodes.json', 'hub/hub.json', 'hub/id_ed25519', 'hub-hosted.json', 'teams/**', "),
    # cca devices：维护者的服务器用不了时还可以经自己的同步服务器（对拍的沙箱都没加入），说的原因带上它；不经服务器的账号说清楚是哪一种
    # （第三方 API 账号、没有邮箱、不在映射里：要经自己的同步服务器）。JSON 里多的 hub、hubs 在 cases/presence.py 的 VIEW_MASK 去掉
    ("        print(reason)\n        sys.exit(2)\n    t, pm = ctx()[1], policy_raw(T)\n",
     "        _c = load_quiet(CFG)\n"
     "        print(reason if isinstance(_c, dict) and _c.get('presence') is False else reason + '，也没加入自己的同步服务器（cca hub join）')\n"
     "        sys.exit(2)\n    t, pm = ctx()[1], policy_raw(T)\n"),
    ("            print('%s 不在维护者的映射里：只统计映射里的官方账号' % name_of(k))\n",
     "            print('%s 是第三方 API 账号：只统计官方账号' % name_of(k) if is_api(t[k]) else '%s 没有邮箱：只统计官方账号' % name_of(k)\n"
     "                  if not parse_key(k)[1] else '%s 不在维护者的映射里，要经自己的同步服务器统计：还没加入自己的同步服务器（有加入链接时运行 cca hub join）'\n"
     "                  % name_of(k))\n"),
]


def oracle_cca():
    """带着内嵌 Python 的那份 cca 在哪（对照的那一边从它里面取 PY_SRC）。仓库里的 cca 还带着就用它；切换以后它里面没有
    Python 了，从 cli/PORTED_FROM 记的那个主干提交里取出来，放进临时目录（对拍结束时删掉）"""
    if not _ORACLE:
        p = os.path.join(REPO, 'cca')
        with open(p, encoding='utf-8') as f:
            text = f.read()
        if PY_HEAD not in text:
            with open(os.path.join(CLI, 'PORTED_FROM')) as f:
                rev = f.read().split()[0]
            got = subprocess.run(['git', '-C', REPO, 'show', rev + ':cca'], capture_output=True)
            if got.returncode != 0:
                sys.exit('对照用的 cca 取不出来（git show %s:cca）：%s' % (rev, got.stderr.decode('utf-8', 'replace').strip()))
            text = got.stdout.decode('utf-8')
            for old, new in ORACLE_PATCHES:
                if text.count(old) != 1:
                    sys.exit('对照用的 cca 里找不到要打补丁的地方（%d 处）：%s' % (text.count(old), old))
                text = text.replace(old, new)
            d = tempfile.mkdtemp(prefix='cca-oracle.')
            import atexit
            atexit.register(shutil.rmtree, d, True)
            p = os.path.join(d, 'cca')
            with open(p, 'w', encoding='utf-8') as f:
                f.write(text)
        _ORACLE.update(path=p, text=text)
    return _ORACLE['path']


def py_src():
    """原来内嵌在 cca 里的 Python 源码"""
    oracle_cca()
    return _ORACLE['text'].split(PY_HEAD, 1)[1].split('\nPY\n)', 1)[0]


SHARED_REPO = 'example/cca-policy'   # 放了映射的沙箱 config.json 里的映射仓库（打开着共享）


class Box:
    """一个用例的沙箱：home 是假的 HOME，下面的方法往里放夹具"""

    def __init__(self, home):
        self.home = home
        self.cca = os.path.join(home, '.cca')
        self.bin = os.path.join(home, 'bin')   # 假命令放这里（在 PATH 最前面）

    def path(self, rel):
        return os.path.join(self.home, rel)

    def write(self, rel, text, mode=0o600):
        p = self.path(rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, 'wb') as f:
            f.write(text if isinstance(text, bytes) else text.encode('utf-8'))
        os.chmod(p, mode)
        return p

    def json(self, rel, data, mode=0o600, **kw):
        import json
        kw.setdefault('ensure_ascii', False)
        kw.setdefault('indent', 2)
        return self.write(rel, json.dumps(data, **kw) + '\n', mode)

    def cfg(self, data):
        if isinstance(data, dict) and 'policy' not in data and self._shared():
            data = dict(data, policy=SHARED_REPO)
        return self.json('.cca/config.json', data)

    def table(self, data, tool='claude'):
        return self.json('.cca/%s.json' % {'claude': 'accounts'}.get(tool, tool), data)

    def policy(self, data):
        p = self.json('.cca/policy/policy.json', data)
        self._share()
        return p

    def fill(self, data):
        p = self.json('.cca/policy/nodes.json', data)
        self._share()
        return p

    def _shared(self):
        return any(os.path.exists(self.path('.cca/policy/' + f)) for f in ('policy.json', 'nodes.json'))

    def _share(self):
        """放了映射、参考节点的沙箱打开共享（config.json 里写上 policy，见 ORACLE_PATCHES 的共享开关）：已经写了 config.json 的
        补上这一项（写坏了的、不是对象的、自己写了 policy 的不动），还没写的写一份只有它的"""
        import json
        p = self.path('.cca/config.json')
        try:
            with open(p, encoding='utf-8') as f:
                c = json.load(f)
        except FileNotFoundError:
            c = {}
        except ValueError:
            return
        if isinstance(c, dict) and 'policy' not in c:
            c['policy'] = SHARED_REPO
            self.json('.cca/config.json', c)

    def fake(self, name, script):
        """沙箱里的假命令（/bin/sh 脚本）"""
        return self.write('bin/' + name, '#!/bin/sh\n' + script, 0o755)


def case(name, argv, stdin=b'', env=None, tool='claude', mask=None, cwd=None, tree=True, settle=0, fd3=None, during=None, timeout=60, oracle=None, before=None):
    """登记一个用例。被装饰的函数拿到 Box，往沙箱里放夹具。
    mask：[(正则, 替换成什么)…]，比之前两边都先换掉（时间戳、随机数这些每次不一样的东西）
    settle：命令退出以后再等几秒才看沙箱（它起了后台进程，等后台进程把该写的写完）
    fd3：交互输入（cca 的提问从 3 号描述符读答案）：给了就把这些内容接在子进程的 fd 3 上，读完就是到头
    during：命令运行期间并行做的事，during(box)：在另一条线程里跑（比如等它监听了端口以后发一个请求）；两边各跑一遍
    before：命令开始之前先做完的事，before(box)（两边各做一遍，在拷好的沙箱上）：起一个真的进程、先拿住一把锁……
    返回值能调用的话，命令结束、during 也做完以后调它收尾，然后才看沙箱
    timeout：最多等几秒（到时间杀掉，算失败）
    oracle：对照的一边换成这条命令（列表；后面接上 argv 里子命令名之后的参数），不跑 PY_SRC。给 bash 里原来内嵌的那几小段
    Python 用：`oracle=[sys.executable, '-I', '-c', 那一小段]`，cca-core 这边照常跑 argv"""
    def reg(setup):
        raw = lambda x: x if x is None or isinstance(x, bytes) else x.encode('utf-8')
        CASES.append(dict(name=name, argv=argv, stdin=raw(stdin), env=env or {}, tool=tool, mask=mask or [], cwd=cwd, tree=tree,
                          settle=settle, fd3=raw(fd3), during=during, timeout=timeout, oracle=oracle, before=before, setup=setup))
        return setup
    return reg


def tree_digest(root, masks):
    """沙箱里每个文件、目录、链接的样子：路径、种类、权限、链接指向哪、内容"""
    out = {}
    for d, dirs, files in os.walk(root):
        dirs.sort()
        for n in sorted(dirs + files):
            p = os.path.join(d, n)
            rel = os.path.relpath(p, root)
            st = os.lstat(p)
            if stat.S_ISLNK(st.st_mode):
                out[rel] = 'link -> %s' % os.readlink(p)
            elif stat.S_ISDIR(st.st_mode):
                out[rel] = 'dir %o' % (st.st_mode & 0o777)
            elif n.endswith('.sqlite') and os.path.getsize(p):
                # 数据库比内容不比字节：文件头里记着最后改它的 SQLite 的版本，原来用的是 Python 带的库，cca-core 用系统的 sqlite3
                dump = subprocess.run(['/usr/bin/sqlite3', p, '.dump'], capture_output=True)
                out[rel] = 'sqlite %o\n%s' % (st.st_mode & 0o777, dump.stdout.decode('utf-8', 'replace') if dump.returncode == 0 else
                                             'sha256:' + hashlib.sha256(open(p, 'rb').read()).hexdigest())
            else:
                with open(p, 'rb') as f:
                    data = f.read()
                try:
                    text = apply_masks(data.decode('utf-8'), masks)
                except UnicodeDecodeError:
                    text = 'sha256:' + hashlib.sha256(data).hexdigest()
                out[rel] = 'file %o\n%s' % (st.st_mode & 0o777, text)
    return out


# 会动到这台机器、会联网的命令：对拍的沙箱里一律是假的。用例自己放了假命令就用它的，没放的调到就报错（两边都一样报，
# 用例会因为多出 unexpected-commands.log 被看出来）。写了绝对路径的命令（lsregister、taskpolicy）挡不住，用例要自己绕开
GUARDED = ['security', 'osascript', 'open', 'pbcopy', 'launchctl', 'ssh', 'scp', 'curl', 'git', 'nc', 'crontab', 'defaults', 'killall', 'pkill', 'sudo', 'lsappinfo',
           'networksetup', 'scutil', 'codesign', 'spctl', 'hdiutil', 'ditto', 'claude', 'codex', 'agy', 'mihomo']
GUARD = '''echo "pydiff：这个用例没有给 %s 准备假命令" >&2
printf '%%s\\n' "$0 $*" >> "$HOME/unexpected-commands.log"
exit 127
'''


def clone_tree(src, dst):
    """整棵树拷一份：链接照旧是链接，硬链接照旧连在一起（shutil.copytree 会把它们拆成两个文件），权限和时间不变"""
    seen = {}

    def copy(s, d):
        st = os.lstat(s)
        if stat.S_ISLNK(st.st_mode):
            os.symlink(os.readlink(s), d)
            shutil.copystat(s, d, follow_symlinks=False)   # 链接自己的修改时间也照旧
        elif stat.S_ISDIR(st.st_mode):
            os.mkdir(d)
            for n in os.listdir(s):
                copy(os.path.join(s, n), os.path.join(d, n))
            shutil.copystat(s, d)
        elif st.st_nlink > 1 and (st.st_dev, st.st_ino) in seen:
            os.link(seen[(st.st_dev, st.st_ino)], d)
        else:
            shutil.copy2(s, d)
            seen[(st.st_dev, st.st_ino)] = d
    copy(src, dst)


def apply_masks(text, masks):
    for pat, rep in masks:
        text = re.sub(pat, rep, text)
    return text


def hold_fd3(path='/dev/null'):
    """这个进程的 3 号描述符一直占着：平时是 /dev/null，fd3 用例运行期间换成答案文件。不占着的话，用例文件导入时建的
    套接字、文件会落在 3 号上（cases/leak.py 的假 STUN 服务器就碰上过），fd3 用例一 dup2 就把它顶掉了"""
    fd = os.open(path, os.O_RDONLY)
    if fd != 3:
        os.dup2(fd, 3)
        os.close(fd)


def run_one(cmd, c, home, tmp):
    env = {'HOME': home, 'PATH': '%s/bin:/usr/bin:/bin:/usr/sbin:/sbin' % home, 'LANG': 'en_US.UTF-8', 'TZ': 'Asia/Shanghai',
           'TMPDIR': tmp, 'CCA_TOOL': c['tool'], 'CCA_SELF': os.path.join(home, '.cca', 'bin', 'cca'), 'CCA_BIN': ''}
    env.update(c['env'])
    # 用例里写的 %HOME% 换成沙箱的 HOME（登记用例的时候还不知道它在哪）
    env = {k: v.replace('%HOME%', home) for k, v in env.items() if v is not None}
    argv = [a.replace('%HOME%', home) for a in c['argv']]
    cwd = (c['cwd'] or home).replace('%HOME%', home)
    answers = None
    if c['fd3'] is not None:   # 答案放进文件，接在这个进程的 fd 3 上传给子进程（3 号在这里平时没人用）
        answers = os.path.join(tmp, 'fd3')
        with open(answers, 'wb') as f:
            f.write(c['fd3'])
        hold_fd3(answers)
    cleanup = c['before'](Box(home)) if c.get('before') else None
    try:
        p = subprocess.Popen(cmd + argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=cwd,
                             pass_fds=(3,) if answers else ())
        side = threading.Thread(target=c['during'], args=(Box(home),), daemon=True) if c['during'] else None
        if side:
            side.start()
        try:
            out, err = p.communicate(c['stdin'], timeout=c['timeout'])
            rc = p.returncode
        except subprocess.TimeoutExpired:
            p.kill()
            out, err = p.communicate()
            err, rc = err + b'\n[pydiff: timeout]', -999
        if side:
            side.join(10)
    finally:
        if answers:
            hold_fd3()   # 3 号不空出来（见 hold_fd3）
        if callable(cleanup):
            cleanup()
    if c['settle']:
        time.sleep(c['settle'])
    dec = lambda b: apply_masks(b.decode('utf-8', 'replace'), c['mask'])
    return {'stdout': dec(out), 'stderr': dec(err), 'rc': rc, 'tree': tree_digest(home, c['mask']) if c['tree'] else {}}


def diff_text(a, b, what):
    lines = list(difflib.unified_diff(a.splitlines(), b.splitlines(), 'python ' + what, 'rust ' + what, lineterm='', n=2))
    return '\n'.join('      ' + x for x in lines[:60])


def compare(py, rs):
    """两边的差别（空的就是一样）"""
    out = []
    if py['rc'] != rs['rc']:
        out.append('    退出码：python %s，rust %s' % (py['rc'], rs['rc']))
    if py['stdout'] != rs['stdout']:
        out.append('    stdout 不一样：\n' + diff_text(py['stdout'], rs['stdout'], 'stdout'))
    if 'Traceback (most recent call last)' in py['stderr']:
        # Python 那边是没人接的异常：调用栈对不上，只要求 Rust 这边也出错、也说了点什么
        if not rs['stderr'].strip():
            out.append('    python 出了没人接的异常，rust 的 stderr 是空的')
    elif py['stderr'] != rs['stderr']:
        out.append('    stderr 不一样：\n' + diff_text(py['stderr'], rs['stderr'], 'stderr'))
    for rel in sorted(set(py['tree']) | set(rs['tree'])):
        a, b = py['tree'].get(rel), rs['tree'].get(rel)
        if a != b:
            if a is None or b is None:
                out.append('    文件 %s：%s' % (rel, 'python 有，rust 没有' if b is None else 'rust 有，python 没有'))
            else:
                out.append('    文件 %s 不一样：\n%s' % (rel, diff_text(a, b, rel)))
    return out


def main():
    args = sys.argv[1:]
    verbose = '-v' in args
    pick = [args[i + 1] for i, a in enumerate(args) if a == '-k' and i + 1 < len(args)]
    if not os.access(CORE, os.X_OK):
        sys.exit('找不到 %s：先在 cli/ 里 cargo build' % CORE)
    hold_fd3()   # 在导入用例之前
    sys.path.insert(0, HERE)
    sys.modules.setdefault('run', sys.modules['__main__'])   # 用例里的 from run import case 要拿到这一份（不是再导入一份）
    for f in sorted(os.listdir(os.path.join(HERE, 'cases'))):
        if f.endswith('.py') and not f.startswith('_'):
            importlib.import_module('cases.' + f[:-3])
    base = os.path.realpath(tempfile.mkdtemp(prefix='cca-pydiff.'))
    src = os.path.join(base, 'py_src.py')
    with open(src, 'w', encoding='utf-8') as f:
        f.write(py_src())
    home, pre, tmp = os.path.join(base, 'h'), os.path.join(base, 'pre'), os.path.join(base, 'tmp')
    os.makedirs(tmp)
    oracle = [sys.executable, '-I', '-X', 'utf8', src]
    passed = failed = strays = 0
    seen = set()
    try:
        for c in CASES:
            if c['name'] in seen:
                sys.exit('用例重名：%s' % c['name'])
            seen.add(c['name'])
            if pick and not any(p in c['name'] for p in pick):
                continue
            for d in (home, pre):
                shutil.rmtree(d, ignore_errors=True)
            os.makedirs(home)
            box = Box(home)
            c['setup'](box)
            for name in GUARDED:   # 用例没准备假命令的：放一个只会报错的，免得跑到这台机器上真的那个
                if not os.path.lexists(box.path('bin/' + name)):
                    box.fake(name, GUARD % name)
            clone_tree(home, pre)
            py = run_one(oracle, c, home, tmp) if not c['oracle'] else run_one(c['oracle'], dict(c, argv=c['argv'][1:]), home, tmp)
            shutil.rmtree(home)
            clone_tree(pre, home)
            rs = run_one([CORE], c, home, tmp)
            added = ADDED.get(c['argv'][0]) if c['argv'] else None
            if added:   # 切换以后有意加的输出：去掉以后别的仍然要和原来一样
                rs['stdout'] = added.sub('', rs['stdout'])
            diffs = compare(py, rs)
            stray = rs['tree'].get('unexpected-commands.log') or py['tree'].get('unexpected-commands.log')
            if stray:   # 两边一样也要说：这个用例调到了没准备假命令的东西，测的多半不是想测的
                strays += 1
                print('STRAY %s 调到了没有准备假命令的：%s' % (c['name'], ' | '.join(stray.splitlines()[1:])[:300]))
            if diffs:
                failed += 1
                print('FAIL  %s   %s' % (c['name'], ' '.join(c['argv'])))
                print('\n'.join(diffs))
                if verbose:
                    for who, r in (('python', py), ('rust', rs)):
                        print('    --- %s rc=%s\n    stdout: %r\n    stderr: %r' % (who, r['rc'], r['stdout'], r['stderr']))
            else:
                passed += 1
                if verbose:
                    print('PASS  %s' % c['name'])
                if '-t' in args:   # 跑完以后沙箱里有什么（确认夹具和假命令真的起了作用）；PYDIFF_SHOW=curl.log,keychain 只看路径里带这些的
                    show = [x for x in os.environ.get('PYDIFF_SHOW', '').split(',') if x]
                    for rel in sorted(rs['tree']):
                        if show and not any(x in rel for x in show):
                            continue
                        print('      %s: %s' % (rel, rs['tree'][rel][:400].replace('\n', '\n          ')))
                if '-s' in args:   # 通过的也把输出打出来看一眼（确认用例真的走到了想测的那条路）
                    print('      rc=%s\n      stdout: %s\n      stderr: %s' % (rs['rc'], rs['stdout'].replace('\n', '\n              '), rs['stderr'].replace('\n', '\n              ')))
    finally:
        shutil.rmtree(base, ignore_errors=True)
    print('---- 对拍：%d 通过，%d 失败%s' % (passed, failed, '，%d 个调到了没准备假命令的' % strays if strays else ''))
    sys.exit(1 if failed or strays else 0)


if __name__ == '__main__':
    main()
