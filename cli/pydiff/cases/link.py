"""设备标识、.claude.json 的三方合并、共享目录的链接（cli/src/cmd/link/）：子命令 device、sync、link。

tests/device_identity.py、tests/shared_state.py 里的场景都在这里（同样的夹具），外加各条报错和边角的路。

这几个子命令一个场景常常要跑好几步（先链接，改点东西，再链接）。对拍一次只比一条命令，所以多步的场景用 chain()：
每条命令登记成一个用例，它前面的步骤在夹具里做完——其中的命令由原版 Python 跑（py()）。两边因此从同一份状态出发。

run.py 之外，这份文件给自己的用例多加了几样（见 run_one；别的模块的用例不受影响）：

- 随机生成的设备标识、冲突副本的名字不能逐字节比，但也不是整个抹掉：同一个值在 stdout、stderr、文件名、文件内容里
  换成同一个编号（<hex64#1>、<uuid#2>…），所以“device.json 和客户端文件里是同一个值”“userID 和 machineID 不一样”
  “报错里说的路径就是存下来的那个文件”这些关系照样在比。夹具里给定的标识写成一个字符重复到底（HEX、UUID），
  不会被当成随机的：留着、换掉、迁移这几条路上，原来的值还在不在一眼看得出。
- 文件名也参加比较前的替换（run.py 的 mask 只管内容）；conflicts 里按时间戳起的名字换成 <ts>。
- hardlinks：夹具里的硬链接。run.py 把沙箱拷来拷去时硬链接会变成两份文件，所以每次跑之前重新连上。
- chmod：跑之前才改的权限（只读的目录、读不了的文件）；比完改回去，沙箱才收得了场。
- mtimes：这些路径的修改时间也比（shutil.copy2 要带着原文件的修改时间，merge_file 靠它定新旧）。
- 文件的硬链接数不是 1、带着文件标志（macOS 的 chflags）时写进比较的内容里（设备文件拆没拆开；copy2 连标志一起拷）。
- 原版 Python 从编译好的字节码起（oracle()），不是每次从源码起：快三倍多。

走到还没移植的函数的用例，名字以 link-cost/（`link 键 cost`，最后要 cost 模块的 cost_kick）、link-codex/（Codex 的 link，
最后要 codex 模块的 codex_fix_rollouts）开头。cca-core 在那里是 unimplemented!：前面的事（设备标识、链接、并回、冲突副本、
基线）都做完了，最后 panic、退出码 1。这些用例现在比到那一步为止：cca-core 的 stderr 正好以那个函数的 unimplemented 结尾时，
去掉这两行、退出码当 0（forgive），其余的 stderr、文件照常逐字节比。那两个函数接上以后 forgive 自然不再起作用，
它们就是普通的用例。想看不宽限的结果：
    PYDIFF_STRICT=1 python3 -I cli/pydiff/run.py -k link-cost -k link-codex
这些用例的夹具让原版 Python 的那两个函数什么也不做（没有会话库、config.toml 是好的；CCA_COST=off 或没有 config.json）。

夹具里只用假数据：邮箱 example.com、节点 US-A。"""
import atexit
import fnmatch
import hashlib
import json
import marshal
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time

import run
from run import case

A, B = 'alice@example.com', 'bob@example.com'
STRICT = os.environ.get('PYDIFF_STRICT') == '1'

ROOT = {'claude': '.cca/accounts', 'codex': '.cca/codex/accounts', 'agy': '.cca/agy/accounts'}
SUB = {'claude': 'claude', 'codex': 'codex', 'agy': 'gemini'}
MASTER = {'claude': '.claude', 'codex': '.codex', 'agy': '.gemini'}
FIELDS = {'claude': ('userID', 'machineID'), 'codex': ('installation_id',), 'agy': ('installation_id', 'installation_uuid')}


# --- 对拍工具的补充 ---------------------------------------------------------------------------------------------------

# 随机生成的东西长什么样。夹具里给定的值（一个字符重复到底的）不算
RANDOM = [
    ('hex64', r'(?<!sha256:)(?<![0-9a-f])(?!([0-9a-f])\1{63}(?![0-9a-f]))[0-9a-f]{64}(?![0-9a-f])'),
    ('uuid', r'(?<![0-9a-f-])(?!([0-9a-f])\1{7}-\1{4}-[0-9a-f]\1{3}-[0-9a-f]\1{3}-\1{12})'
             r'[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}(?![0-9a-f-])'),
    ('hex32', r'(?<![0-9a-f])[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}(?![0-9a-f])'),
]
# 每次不一样、又不值得编号的：aside 起的名字（原名.时间戳）、半路出错留下的临时文件名里的进程号
VARYING = [(r'(conflicts/[^/\s]+/[^/\s]+\.)\d{10}(?!\d)', r'\1<ts>'), (r'\.cca-\d+\.tmp(?![0-9A-Za-z])', '.cca-<pid>.tmp')]


def canon(r):
    """随机值换成编号：按第一次出现的先后（stdout、stderr，再是文件——文件照抹掉随机值以后的样子排序）"""
    seen = {}

    def varying(text):
        for pat, rep in VARYING:
            text = re.sub(pat, rep, text)
        return text

    def number(text):
        for kind, pat in RANDOM:
            def rep(m, kind=kind):
                tok = m.group(0)
                if tok not in seen:
                    seen[tok] = '<%s#%d>' % (kind, 1 + sum(1 for v in seen.values() if v.startswith('<%s#' % kind)))
                return seen[tok]
            text = re.sub(pat, rep, text)
        return varying(text)

    def blind(text):
        for kind, pat in RANDOM:
            text = re.sub(pat, '<%s>' % kind, text)
        return varying(text)

    r['stdout'], r['stderr'] = number(r['stdout']), number(r['stderr'])
    tree = {}
    for rel, what in sorted(r['tree'].items(), key=lambda kv: (blind(kv[0]), blind(kv[1]))):
        rel, what = number(rel), number(what)
        tree[rel] = what if rel not in tree else tree[rel] + '\n=== 同名的另一个 ===\n' + what
    r['tree'] = tree


def tree_of(home, extra):
    """沙箱里每一项的样子（同 run.tree_digest），外加硬链接数、文件标志（macOS）、要比的修改时间；读不了、进不去的先改成能读的"""
    out, stack = {}, [home]
    while stack:
        d = stack.pop()
        for n in sorted(os.listdir(d)):
            p = os.path.join(d, n)
            rel = os.path.relpath(p, home)
            st = os.lstat(p)
            mode = st.st_mode & 0o7777
            more = (' 硬链接数 %d' % st.st_nlink if st.st_nlink != 1 and not stat.S_ISDIR(st.st_mode) else '') \
                + (' 标志 %#x' % st.st_flags if getattr(st, 'st_flags', 0) else '')
            if stat.S_ISLNK(st.st_mode):
                out[rel] = 'link%s -> %s' % (more, os.readlink(p))
            elif stat.S_ISDIR(st.st_mode):
                out[rel] = 'dir %o%s' % (mode, more)
                if mode & 0o700 != 0o700:
                    os.chmod(p, mode | 0o700)
                stack.append(p)
            else:
                if mode & 0o600 != 0o600:
                    os.chmod(p, mode | 0o600)
                with open(p, 'rb') as f:
                    data = f.read()
                try:
                    text = data.decode('utf-8')
                except UnicodeDecodeError:
                    text = 'sha256:' + hashlib.sha256(data).hexdigest()
                out[rel] = 'file %o%s\n%s' % (mode, more, text)
            if any(fnmatch.fnmatch(rel, pat) for pat in extra['mtimes']):
                out[rel + ' 的修改时间'] = str(st.st_mtime_ns)
    return out


# 还没移植的函数：cca-core 走到它时 panic，stderr 最后两行是「cca-core: 内部错误：panicked at 文件:行:列:」「not implemented: …（这个记号）」
UNPORTED = {'cost': '（cost 模块：cost_kick）', 'codex': '（codex 模块：codex_fix_rollouts'}


FORGIVEN = []


def forgive(r, pending, name):
    """cca-core 做完了前面的、停在 pending 里某个还没移植的函数上：去掉那两行，退出码当 0。别的照常比"""
    lines = r['stderr'].splitlines(keepends=True)
    if (r['rc'] == 1 and len(lines) >= 2 and lines[-2].startswith('cca-core: 内部错误：panicked at ')
            and lines[-1].startswith('not implemented: ') and any(UNPORTED[p] in lines[-1] for p in pending.split())):
        r['rc'], r['stderr'] = 0, ''.join(lines[:-2])
        FORGIVEN.append(name)


@atexit.register
def forgiven_note():
    # 宽限了多少个说一声（写到 stderr，不占 run.py 最后那行汇总）：那两个函数接上以后这句话自己就没了
    if FORGIVEN:
        sys.stderr.write('     link：其中 %d 个用例（link-cost/、link-codex/）只比到还没移植的 cost_kick / codex_fix_rollouts 为止，'
                         'PYDIFF_STRICT=1 时不宽限\n' % len(FORGIVEN))


ORACLE = {}
BOOT = ("import marshal,sys;f=sys.argv.pop(1);"
        "exec(marshal.loads(open(f,'rb').read()),{'__name__':'__main__','__builtins__':__builtins__})")


def oracle():
    """起原版 Python 的命令。PY_SRC 只编译一次、存成字节码（和 cca 的 PY_BOOT 一个路数）：run.py 那样每次从源码起要 0.13 秒，
    这样 0.04 秒——这份文件有几百个用例，夹具里还要跑上千次"""
    if not ORACLE:
        d = tempfile.mkdtemp(prefix='cca-pydiff-link.')
        atexit.register(shutil.rmtree, d, True)
        with open(os.path.join(d, 'py_src.bin'), 'wb') as f:
            f.write(marshal.dumps(compile(run.py_src(), 'py_src.py', 'exec')))
        ORACLE.update(cmd=[sys.executable, '-I', '-X', 'utf8', '-c', BOOT, os.path.join(d, 'py_src.bin')], tmp=d)
    return ORACLE['cmd']


_run_one = run.run_one


def run_one(cmd, c, home, tmp):
    extra = c.get('link')
    if extra is None:
        return _run_one(cmd, c, home, tmp)
    rust = cmd[0] == run.CORE
    for src, dst in extra['hardlinks']:
        src, dst = os.path.join(home, src), os.path.join(home, dst)
        if not os.path.samefile(src, dst):
            os.unlink(dst)
            os.link(src, dst)
    for rel, mode in extra['chmod'].items():
        os.chmod(os.path.join(home, rel), mode)
    r = _run_one(cmd if rust else oracle(), dict(c, tree=False), home, tmp)
    r['tree'] = tree_of(home, extra)
    if extra['pending'] and rust and not STRICT:
        forgive(r, extra['pending'], c['name'])
    canon(r)
    return r


run.run_one = run_one


def lcase(name, argv, tool='claude', hardlinks=(), chmod=None, mtimes=(), pending=None, **kw):
    """登记一个用例：run.case 的参数之外可以带 hardlinks、chmod、mtimes（见文件开头）；pending 是它最后会走到的那个还没移植的函数"""
    def reg(setup):
        case(name, argv, tool=tool, **kw)(setup)
        run.CASES[-1]['link'] = dict(hardlinks=list(hardlinks), chmod=dict(chmod or {}), mtimes=list(mtimes), pending=pending)
        return setup
    return reg


def py(b, *argv, tool='claude', rc=0, env=None):
    """夹具里先跑的命令：原版 Python，环境和 run.py 起它时一样"""
    cmd = oracle()
    e = {'HOME': b.home, 'PATH': '%s/bin:/usr/bin:/bin:/usr/sbin:/sbin' % b.home, 'LANG': 'en_US.UTF-8', 'TZ': 'Asia/Shanghai',
         'TMPDIR': ORACLE['tmp'], 'CCA_TOOL': tool, 'CCA_SELF': os.path.join(b.home, '.cca', 'bin', 'cca'), 'CCA_BIN': ''}
    e.update(env or {})
    p = subprocess.run(cmd + list(argv), env=e, cwd=b.home, capture_output=True, timeout=60)
    if p.returncode != rc:
        raise AssertionError('夹具里的 %s 退出码 %d（应该是 %d）：%s' % (' '.join(argv), p.returncode, rc, p.stderr.decode('utf-8', 'replace')))
    return p


class step:
    """chain() 里的一条命令"""

    def __init__(self, *argv, rc=0, **kw):
        self.argv, self.rc, self.kw = list(argv), rc, kw


def chain(name, steps, tool='claude', **kw):
    """多步的场景。steps 里：函数（拿到沙箱 b，改夹具）和 step('子命令', 参数…)。每条命令登记成一个用例「名字/第几条-子命令」，
    它前面的步骤在夹具里做完（命令由原版 Python 跑，rc 是那时该有的退出码）。step 里另给的参数（tool、env、mtimes…）盖过 chain 的"""
    count = 0
    for i, s in enumerate(steps):
        if not isinstance(s, step):
            continue
        count += 1
        opts = dict(kw, tool=tool)
        opts.update(s.kw)

        def setup(b, upto=i):
            for prev in steps[:upto]:
                if isinstance(prev, step):
                    py(b, *prev.argv, tool=prev.kw.get('tool', tool), rc=prev.rc, env=prev.kw.get('env', kw.get('env')))
                else:
                    prev(b)
        lcase('%s/%d-%s' % (name, count, s.argv[0]), s.argv, **opts)(setup)


# --- 夹具 -------------------------------------------------------------------------------------------------------------

HEX = {c: c * 64 for c in '0123456789abcdef'}   # 给定的 Claude 设备标识


def UUID(c):
    """给定的 UUID"""
    return '-'.join(c * n for n in (8, 4, 4, 4, 12))


# 各工具几组给定的设备标识（按字段的先后）
IDS = {'claude': [(HEX['c'], HEX['d']), (HEX['e'], HEX['f']), (HEX['1'], HEX['2'])],
       'codex': [(UUID('c'),), (UUID('e'),), (UUID('1'),)],
       'agy': [(UUID('c'), UUID('d')), (UUID('e'), UUID('f')), (UUID('1'), UUID('2'))]}
TOOLS = ('claude', 'codex', 'agy')


def ident(k):
    s = re.sub(r'[^a-z0-9]+', '-', k.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(k.encode('utf-8')).hexdigest()[:6])


def adir(tool, key=A):
    """账号目录（相对沙箱的 HOME）"""
    return '%s/%s' % (ROOT[tool], ident(key))


def cdir(tool, key=A):
    return '%s/%s' % (adir(tool, key), SUB[tool])


def put(b, rel, text, mode=0o644, mtime=None):
    """写一个文件；mtime 是要定死的修改时间（秒）"""
    p = b.write(rel, text, mode)
    if mtime is not None:
        os.utime(p, (mtime, mtime))
    return p


def jput(b, rel, data, mode=0o644, mtime=None):
    """像客户端那样写的 JSON：一行、中文原样、结尾没有换行"""
    return put(b, rel, json.dumps(data, ensure_ascii=False), mode, mtime)


def mkdir(b, rel, mode=None, mtime=None):
    p = b.path(rel)
    os.makedirs(p, exist_ok=True)
    if mode is not None:
        os.chmod(p, mode)
    if mtime is not None:
        os.utime(p, (mtime, mtime))
    return p


def symlink(b, target, rel):
    """rel 处建一个链接；target 以 ~/ 开头的换成沙箱的 HOME"""
    p = b.path(rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    os.symlink(b.path(target[2:]) if target.startswith('~/') else target, p)
    return p


def hardlink(b, src, rel):
    p = b.path(rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    os.link(b.path(src), p)
    return p


def atomic(b, rel, text, mode=0o644, mtime=None):
    """像客户端的原子保存那样换掉一个文件：写旁边的临时文件再改名（rel 是链接时，链接就此变成普通文件）"""
    p = b.path(rel)
    put(b, rel + '.test-new', text, mode, mtime)
    os.replace(p + '.test-new', p)


def stamp(b, rel, when):
    """这一项和它里面的每一项的修改时间都定成 when"""
    top = b.path(rel)
    for root, dirs, files in os.walk(top):
        for n in dirs + files:
            os.utime(os.path.join(root, n), (when, when), follow_symlinks=False)
    os.utime(top, (when, when), follow_symlinks=False)


def tables(b):
    for t in ('accounts', 'codex', 'agy'):
        jput(b, '.cca/%s.json' % t, dict.fromkeys((A, B), 'US-A'))


def dev_world(b):
    """tests/device_identity.py 的 setUp"""
    for t in ('accounts', 'codex'):
        jput(b, '.cca/%s.json' % t, dict.fromkeys((A, B), 'US-A'))
    jput(b, '.claude.json', {'theme': 'dark', 'userID': HEX['a'], 'machineID': HEX['b']})
    put(b, '.codex/installation_id', '11111111-1111-4111-8111-111111111111\n')


def shared_world(b):
    """tests/shared_state.py 的 setUp"""
    tables(b)
    jput(b, '.claude.json', {'theme': 'dark', 'userID': HEX['a'], 'machineID': HEX['b'], 'oauthAccount': {'emailAddress': 'default@example.com'}})


def agy_master(b, ids=(UUID('a'), UUID('b'))):
    """~/.gemini 自己的两个装机标识"""
    put(b, '.gemini/antigravity-cli/installation_id', ids[0])
    put(b, '.gemini/antigravity-cli/jetski_state.pbtxt', 'onboarded: true\ninstallation_uuid: "%s"\n' % ids[1])


def record(b, tool, ids, key=A, **more):
    """账号的固定记录 device.json"""
    return b.json(adir(tool, key) + '/device.json', dict(dict({'version': 1}, **dict(zip(FIELDS[tool], ids))), **more))


def client(b, tool, ids, key=A, **more):
    """客户端自己存的设备标识（Claude 的 .claude.json 里另外带上 more）"""
    cd = cdir(tool, key)
    if tool == 'claude':
        return jput(b, cd + '/.claude.json', dict(dict(zip(FIELDS[tool], ids)), **more), 0o600)
    if tool == 'codex':
        return put(b, cd + '/installation_id', ids[0] + '\n', 0o600)
    put(b, cd + '/antigravity-cli/installation_id', ids[0], 0o600)
    return put(b, cd + '/antigravity-cli/jetski_state.pbtxt', 'installation_uuid:  "%s"\n' % ids[1], 0o600)


def client_file(tool, key=A):
    """设备标识所在的那个客户端文件（agy 有两个，这是第一个）"""
    return cdir(tool, key) + {'claude': '/.claude.json', 'codex': '/installation_id', 'agy': '/antigravity-cli/installation_id'}[tool]


def master_file(tool):
    return {'claude': '.claude.json', 'codex': '.codex/installation_id', 'agy': '.gemini/antigravity-cli/installation_id'}[tool]


def managed(b, tool, ids, key=A, **more):
    """cca 已经接管的账号：固定记录和客户端文件一致"""
    record(b, tool, ids, key)
    client(b, tool, ids, key, **more)


# --- device：设备标识（三种工具） ------------------------------------------------------------------------------------

for T in TOOLS:
    @lcase('device/%s/empty-home' % T, ['device', A], tool=T)
    def _(b):
        pass

    @lcase('device/%s/fresh' % T, ['device', A], tool=T)
    def _(b):
        dev_world(b)
        agy_master(b)

    @lcase('device/%s/managed-unchanged' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        managed(b, T, IDS[T][0], localPreference=True)

    @lcase('device/%s/alias-is-the-same-account' % T, ['device', '工作(%s)' % A], tool=T)
    def _(b, T=T):
        dev_world(b)
        managed(b, T, IDS[T][0])

    @lcase('device/%s/client-file-gone' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        record(b, T, IDS[T][0])

    @lcase('device/%s/client-changed' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        record(b, T, IDS[T][0])
        client(b, T, IDS[T][1], oauthAccount={'emailAddress': A}, localPreference=True)

    @lcase('device/%s/adopts-unique-legacy-value' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        client(b, T, IDS[T][0], oauthAccount={'emailAddress': A}, localPreference=True)

    @lcase('device/%s/duplicate-legacy-first' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        for k in (A, B):
            client(b, T, IDS[T][0], k, oauthAccount={'emailAddress': k})

    @lcase('device/%s/duplicate-legacy-second' % T, ['device', B], tool=T)
    def _(b, T=T):
        # 第一个账号已经换了新的：第二个留着原来那个值
        dev_world(b)
        managed(b, T, IDS[T][1], A)
        client(b, T, IDS[T][0], B)

    @lcase('device/%s/legacy-copy-does-not-rotate-managed' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        managed(b, T, IDS[T][0], A)
        client(b, T, IDS[T][0], B)

    @lcase('device/%s/copy-of-managed-gets-new' % T, ['device', B], tool=T)
    def _(b, T=T):
        dev_world(b)
        managed(b, T, IDS[T][0], A)
        client(b, T, IDS[T][0], B)

    @lcase('device/%s/same-as-master' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 客户端文件里是共享主目录的那个值（从 ~/ 拷来的副本）：换掉
        dev_world(b)
        agy_master(b)
        client(b, T, {'claude': (HEX['a'], HEX['b']), 'codex': ('11111111-1111-4111-8111-111111111111',), 'agy': (UUID('a'), UUID('b'))}[T])

    @lcase('device/%s/client-is-symlink-to-master' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        agy_master(b)
        symlink(b, '~/' + master_file(T), client_file(T))

    @lcase('device/%s/client-is-hardlink-to-master' % T, ['device', B], tool=T, hardlinks=[(master_file(T), client_file(T, B))])
    def _(b, T=T):
        dev_world(b)
        agy_master(b)
        hardlink(b, master_file(T), client_file(T, B))

    @lcase('device/%s/managed-but-client-is-symlink' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 已有固定记录：链接换成普通文件，里面是记录里的值
        dev_world(b)
        agy_master(b)
        record(b, T, IDS[T][0])
        symlink(b, '~/' + master_file(T), client_file(T))

    @lcase('device/%s/symlinked-account-alias-is-skipped' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        managed(b, T, IDS[T][0])
        symlink(b, '~/' + adir(T), ROOT[T] + '/linked-copy')

    @lcase('device/%s/stray-file-among-accounts' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        put(b, ROOT[T] + '/README', 'not an account\n')
        mkdir(b, ROOT[T] + '/empty-dir')

    @lcase('device/%s/account-dir-is-symlink' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        mkdir(b, 'elsewhere/' + SUB[T])
        symlink(b, '~/elsewhere', adir(T))

    @lcase('device/%s/config-dir-is-symlink' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        mkdir(b, 'elsewhere')
        symlink(b, '~/elsewhere', cdir(T))

    @lcase('device/%s/config-dir-is-a-file' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        put(b, cdir(T), 'oops')

    @lcase('device/%s/record-bad-json' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        client(b, T, IDS[T][0])
        put(b, adir(T) + '/device.json', '{"version": 1,')

    for NAME, TEXT in (('list', '[1]'), ('null', 'null'), ('version-2', '{"version": 2}'), ('version-null', '{"version": null}'),
                       ('version-text', '{"version": "1"}')):
        @lcase('device/%s/record-%s' % (T, NAME), ['device', A], tool=T)
        def _(b, T=T, TEXT=TEXT):
            dev_world(b)
            client(b, T, IDS[T][0])
            put(b, adir(T) + '/device.json', TEXT)

    for NAME, VERSION in (('float', 1.0), ('true', True)):
        @lcase('device/%s/record-version-%s' % (T, NAME), ['device', A], tool=T)
        def _(b, T=T, VERSION=VERSION):
            # 1.0、true 在 Python 里都等于 1：记录有效，也不用重写
            dev_world(b)
            client(b, T, IDS[T][0])
            record(b, T, IDS[T][0], version=VERSION)

    @lcase('device/%s/record-without-version-and-extra-keys' % T, ['device', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        client(b, T, IDS[T][0])
        b.json(adir(T) + '/device.json', dict(zip(FIELDS[T] + ('note',), IDS[T][0] + ('x',))))

    @lcase('device/%s/record-invalid-values' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 记录里的值不合法：用客户端自己的（合法、没撞）
        dev_world(b)
        client(b, T, IDS[T][0])
        b.json(adir(T) + '/device.json', dict({'version': 1}, **dict.fromkeys(FIELDS[T], 'nope')))

    @lcase('device/%s/record-is-symlink' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 记录是个链接：里面的值不认，链接换成普通文件，原来指向的那份不动
        dev_world(b)
        b.json('elsewhere/device.json', dict({'version': 1}, **dict(zip(FIELDS[T], IDS[T][0]))))
        symlink(b, '~/elsewhere/device.json', adir(T) + '/device.json')

    @lcase('device/%s/record-mode-is-fixed' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 记录没变就不重写：原来的权限（这里 644）也就留着
        dev_world(b)
        managed(b, T, IDS[T][0])
        os.chmod(b.path(adir(T) + '/device.json'), 0o644)

    @lcase('device/%s/other-record-unreadable' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 别的账号的记录读不出：照它客户端文件里的值算“未迁移的副本”，撞了的换掉
        dev_world(b)
        client(b, T, IDS[T][0], A)
        client(b, T, IDS[T][0], B)
        put(b, adir(T, B) + '/device.json', '{oops')

    @lcase('device/%s/other-record-not-isolated' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 别的账号的记录是个链接（不独立）：它的值只算“未迁移”，自己第一次接管时撞了要换
        dev_world(b)
        client(b, T, IDS[T][0], A)
        b.json('elsewhere/device.json', dict({'version': 1}, **dict(zip(FIELDS[T], IDS[T][0]))))
        symlink(b, '~/elsewhere/device.json', adir(T, B) + '/device.json')

    @lcase('device/%s/shared-record-first-prepare' % T, ['device', A], tool=T, hardlinks=[(adir(T, A) + '/device.json', adir(T, B) + '/device.json')])
    def _(b, T=T):
        # 两个账号的记录是同一个文件（硬链接）：先准备的那个整个换新，断开链接；另一份原样
        dev_world(b)
        managed(b, T, IDS[T][0], A)
        hardlink(b, adir(T, A) + '/device.json', adir(T, B) + '/device.json')

    @lcase('device/%s/shared-record-other-side-keeps' % T, ['device', B], tool=T)
    def _(b, T=T):
        # 上一条之后：A 已经换了新的，B 的记录成了独立文件，照它恢复客户端文件
        dev_world(b)
        managed(b, T, IDS[T][1], A)
        record(b, T, IDS[T][0], B)

    @lcase('device/%s/copied-account-dir-first' % T, ['device', A], tool=T)
    def _(b, T=T):
        # 两个账号各有一份独立的记录、里面是同一个值（整个账号目录拷出来的）：先准备的那个换新的
        dev_world(b)
        managed(b, T, IDS[T][0], A, oauthAccount={'emailAddress': A})
        managed(b, T, IDS[T][0], B, oauthAccount={'emailAddress': B})

    @lcase('device/%s/copied-account-dir-second' % T, ['device', B], tool=T)
    def _(b, T=T):
        # 上一条之后：另一个已经换了，这个留着原来的
        dev_world(b)
        managed(b, T, IDS[T][1], A)
        managed(b, T, IDS[T][0], B)

    @lcase('device/%s/no-arguments' % T, ['device'], tool=T)
    def _(b):
        dev_world(b)

    @lcase('device/%s/too-many-arguments' % T, ['device', A, 'x'], tool=T)
    def _(b):
        dev_world(b)

    @lcase('device/%s/alias-only-key' % T, ['device', '别名'], tool=T)
    def _(b):
        dev_world(b)

    @lcase('device/%s/empty-key' % T, ['device', ''], tool=T)
    def _(b):
        dev_world(b)

    @lcase('device/%s/other-cca-home' % T, ['device', A], tool=T, env={'CCA_HOME': 'alt/cca-home'})
    def _(b, T=T):
        dev_world(b)
        client(b, T, IDS[T][0])   # 默认位置的不算数


@lcase('device/claude/api-account-by-id', ['device', '直连'])
def _(b):
    # 第三方 API 账号的目录按 id 认（改别名不换目录）
    dev_world(b)
    jput(b, '.cca/accounts.json', {A: 'US-A', '直连': {'base_url': 'https://api.example.com/anthropic', 'id': 'zjd'}})
    b.json('.cca/accounts/%s/device.json' % ident('zjd'), {'version': 1, 'userID': HEX['c'], 'machineID': HEX['d']})


@lcase('device/codex/api-account-by-id', ['device', '直连'], tool='codex')
def _(b):
    dev_world(b)
    jput(b, '.cca/codex.json', {'直连': {'base_url': 'https://api.example.com/v1', 'id': 'zjd'}})
    b.json('.cca/codex/accounts/%s/device.json' % ident('zjd'), {'version': 1, 'installation_id': UUID('c')})


for NAME, TEXT in (('bad-json', '{"userID": '), ('list', '[]'), ('null', 'null'), ('text', '"x"'), ('number', '0')):
    @lcase('device/claude/config-%s' % NAME, ['device', A])
    def _(b, TEXT=TEXT):
        dev_world(b)
        record(b, 'claude', IDS['claude'][0])
        put(b, cdir('claude') + '/.claude.json', TEXT, 0o600)


@lcase('device/claude/invalid-values-are-replaced', ['device', A])
def _(b):
    # 大写、长度不对、不是字符串的都不算合法的旧值
    dev_world(b)
    jput(b, cdir('claude') + '/.claude.json', {'userID': HEX['c'].upper(), 'machineID': 12345, 'keep': [1, 2]}, 0o600)


@lcase('device/claude/half-valid-values', ['device', A])
def _(b):
    dev_world(b)
    jput(b, cdir('claude') + '/.claude.json', {'machineID': HEX['d'], 'userID': HEX['c'][1:]}, 0o600)


@lcase('device/claude/keeps-key-order-and-other-data', ['device', A])
def _(b):
    # 修回设备字段时别的键、键的先后都留着（userID 原位，machineID 是新加的排最后）
    dev_world(b)
    record(b, 'claude', IDS['claude'][0])
    jput(b, cdir('claude') + '/.claude.json', {'numStartups': 7, 'userID': HEX['e'], 'oauthAccount': {'emailAddress': A, '名字': '爱丽丝'}, 'tipsHistory': {}}, 0o600)


@lcase('device/claude/master-unreadable', ['device', A])
def _(b):
    # ~/.claude.json 读不出：当作没有，账号里和它一样的值也就留着
    dev_world(b)
    put(b, '.claude.json', '{"userID": "%s", ' % HEX['c'])
    client(b, 'claude', IDS['claude'][0])


@lcase('device/claude/master-not-an-object', ['device', A])
def _(b):
    dev_world(b)
    put(b, '.claude.json', '["%s"]' % HEX['c'])
    client(b, 'claude', IDS['claude'][0])


@lcase('device/claude/readonly-flag-is-ignored', ['device', A], env={'CCA_RO': '1'})
def _(b):
    # device 不经过 ctx()：CCA_RO 管不到它，照样写
    dev_world(b)


for NAME, TEXT in (('uppercase', UUID('c').upper() + '\n'), ('braces', '{%s}\n' % UUID('c')), ('urn', 'urn:uuid:%s\n' % UUID('c')),
                   ('no-hyphens', UUID('c').replace('-', '')), ('padded', '  %s \r\n' % UUID('c')), ('hex-prefix', '0x' + 'c' * 30 + '\n'),
                   ('not-a-uuid', 'hello\n'), ('too-short', UUID('c')[:-1] + '\n'), ('empty', ''), ('not-utf8', b'\xff\xfe\n'),
                   ('two-lines', UUID('c') + '\n' + UUID('d') + '\n')):
    @lcase('device/codex/written-as-%s' % NAME, ['device', A], tool='codex')
    def _(b, TEXT=TEXT):
        # 认得出的旧值照标准写法记下来，客户端文件也改成标准写法；认不出的换新的
        dev_world(b)
        put(b, cdir('codex') + '/installation_id', TEXT, 0o600)


@lcase('device/codex/installation-id-is-a-directory', ['device', A], tool='codex')
def _(b):
    dev_world(b)
    mkdir(b, cdir('codex') + '/installation_id')


@lcase('device/codex/master-missing', ['device', A], tool='codex')
def _(b):
    tables(b)
    client(b, 'codex', IDS['codex'][0])


AGY_STATE = 'onboarded: true\nlast_selected_model {\n  installation_uuid: "inner"\n}\ninstallation_uuid: "%s"\nmetrics_enabled: false\n'

for NAME, STATE in (('state-with-other-lines', AGY_STATE % UUID('9')), ('state-crlf', (AGY_STATE % UUID('9')).replace('\n', '\r\n')),
                    ('state-without-the-line', 'onboarded: true'), ('state-empty', ''), ('state-line-with-comment', 'installation_uuid: "%s" # x\n' % UUID('9')),
                    ('state-two-lines', 'installation_uuid: "%s"\ninstallation_uuid: "%s"\n' % (UUID('8'), UUID('9'))),
                    ('state-not-utf8', b'installation_uuid: "\xff"\n'), ('state-invalid-value', 'installation_uuid:\t"nope" \n')):
    @lcase('device/agy/%s' % NAME, ['device', A], tool='agy')
    def _(b, STATE=STATE):
        tables(b)
        agy_master(b)
        put(b, cdir('agy') + '/antigravity-cli/installation_id', UUID('c'), 0o600)
        put(b, cdir('agy') + '/antigravity-cli/jetski_state.pbtxt', STATE, 0o600)


@lcase('device/agy/only-state-file', ['device', A], tool='agy')
def _(b):
    # installation_id 读不了时 jetski_state.pbtxt 里的值也不认（两个一起换新的），别的行留着
    tables(b)
    put(b, cdir('agy') + '/antigravity-cli/jetski_state.pbtxt', AGY_STATE % UUID('9'), 0o600)


@lcase('device/agy/only-id-file', ['device', A], tool='agy')
def _(b):
    tables(b)
    put(b, cdir('agy') + '/antigravity-cli/installation_id', UUID('c') + '\n', 0o600)


@lcase('device/agy/managed-state-rewritten-in-place', ['device', A], tool='agy')
def _(b):
    tables(b)
    record(b, 'agy', IDS['agy'][0])
    put(b, cdir('agy') + '/antigravity-cli/installation_id', UUID('c'), 0o600)
    put(b, cdir('agy') + '/antigravity-cli/jetski_state.pbtxt', AGY_STATE % UUID('9'), 0o644)


@lcase('device/agy/state-is-symlink', ['device', A], tool='agy')
def _(b):
    # 两个文件里有一个是链接：两个都不认、都换成自己的普通文件；链接指向的那份不动
    tables(b)
    agy_master(b)
    put(b, cdir('agy') + '/antigravity-cli/installation_id', UUID('c'), 0o600)
    symlink(b, '~/.gemini/antigravity-cli/jetski_state.pbtxt', cdir('agy') + '/antigravity-cli/jetski_state.pbtxt')


@lcase('device/agy/nest-is-symlink', ['device', A], tool='agy')
def _(b):
    # 账号目录里的 antigravity-cli 是个链接：原实现照样经它写（见 FINDINGS）
    tables(b)
    mkdir(b, 'elsewhere')
    symlink(b, '~/elsewhere', cdir('agy') + '/antigravity-cli')


# --- sync：~/.claude.json 里共享的键和账号的 .claude.json 三方合并（只有 Claude；别的工具什么都不做） -----------------

def sync_world(b, account=None, master=None, base=None, key=A, ids=0):
    """sync 之前的样子：接管好的 Claude 账号，它的 .claude.json（account，另带设备标识）、~/.claude.json、基线"""
    tables(b)
    record(b, 'claude', IDS['claude'][ids], key)
    jput(b, cdir('claude', key) + '/.claude.json', dict(account or {}, **dict(zip(FIELDS['claude'], IDS['claude'][ids]))), 0o600)
    if master is not None:
        jput(b, '.claude.json', master, 0o644)
    if base is not None:
        b.json(cdir('claude', key) + '/.cca-sync-base.json', base)


OWN = {'oauthAccount': {'emailAddress': A}, 'cachedUsageUtilization': {'account': A}, 'numStartups': 12}   # 不共享的键
MINE = {'userID': HEX['a'], 'machineID': HEX['b'], 'oauthAccount': {'emailAddress': 'default@example.com'}}   # ~/.claude.json 自己的

for T in ('codex', 'agy'):
    @lcase('sync/%s/does-nothing' % T, ['sync', A], tool=T)
    def _(b):
        dev_world(b)

    @lcase('sync/%s/does-nothing-in-empty-home' % T, ['sync', A], tool=T)
    def _(b):
        pass

    @lcase('sync/%s/no-arguments' % T, ['sync'], tool=T)
    def _(b):
        pass


@lcase('sync/claude/empty-home', ['sync', A])
def _(b):
    pass


@lcase('sync/claude/initializes-without-link', ['sync', A])
def _(b):
    # tests/device_identity.py：test_sync_can_initialize_claude_without_link
    dev_world(b)


@lcase('sync/claude/no-arguments', ['sync'])
def _(b):
    dev_world(b)


@lcase('sync/claude/too-many-arguments', ['sync', A, 'x'])
def _(b):
    dev_world(b)


@lcase('sync/claude/nothing-changed', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='dark', verbose=True), dict(MINE, theme='dark', verbose=True), {'theme': 'dark', 'verbose': True})


@lcase('sync/claude/account-changed', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='light', editorMode='vim'), dict(MINE, theme='dark'), {'theme': 'dark'})


@lcase('sync/claude/master-changed', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='dark'), dict(MINE, theme='light', editorMode='vim'), {'theme': 'dark'})


@lcase('sync/claude/both-changed-the-same-way', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='light'), {'theme': 'dark'})


@lcase('sync/claude/both-changed-conflict', ['sync', A])
def _(b):
    # 冲突：留共享的值，账号的 .claude.json 整份存进 conflicts
    sync_world(b, dict(OWN, theme='light', verbose=True), dict(MINE, theme='blue', editorMode='vim'), {'theme': 'dark'})


@lcase('sync/claude/conflict-box-already-there', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='blue'), {'theme': 'dark'})
    put(b, '.cca/conflicts/%s/earlier' % ident(A), 'x')


@lcase('sync/claude/account-removed-a-key', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='dark'), dict(MINE, theme='dark', verbose=True), {'theme': 'dark', 'verbose': True})


@lcase('sync/claude/master-removed-a-key', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='dark', verbose=True), dict(MINE, theme='dark'), {'theme': 'dark', 'verbose': True})


@lcase('sync/claude/removed-here-changed-there', ['sync', A])
def _(b):
    # 一边删了、另一边改了：也算冲突，留共享的
    sync_world(b, dict(OWN, theme='dark'), dict(MINE, theme='dark', verbose=False), {'theme': 'dark', 'verbose': True})


@lcase('sync/claude/changed-here-removed-there', ['sync', A])
def _(b):
    # 共享的那边删了、账号这边改了：冲突，照共享的删掉
    sync_world(b, dict(OWN, theme='dark', verbose=False), dict(MINE, theme='dark'), {'theme': 'dark', 'verbose': True})


@lcase('sync/claude/no-base-yet', ['sync', A])
def _(b):
    # 还没有基线：一样的不动，只有一边有的传过去，两边不一样的算冲突
    sync_world(b, dict(OWN, theme='light', verbose=True, diffTool='auto'), dict(MINE, theme='dark', editorMode='vim', diffTool='auto'))


@lcase('sync/claude/no-master-file', ['sync', A])
def _(b):
    # 没有 ~/.claude.json：账号里共享的键写出一份新的（600）
    sync_world(b, dict(OWN, theme='light', mcpServers={'docs': {'command': 'docs-mcp'}}))


@lcase('sync/claude/no-master-file-and-nothing-shared', ['sync', A])
def _(b):
    sync_world(b, dict(OWN))


@lcase('sync/claude/nested-fields-merge', ['sync', A])
def _(b):
    base = {'mcpServers': {'docs': {'command': 'docs-mcp', 'args': ['--old']}, 'gone': {'command': 'x'}},
            'projects': {'/w/a': {'allowedTools': [], 'history': ['1']}, '/w/b': {'trust': True}}}
    account = dict(OWN, mcpServers={'docs': {'command': 'docs-mcp', 'args': ['--new']}, 'mine': {'command': 'm'}},
                   projects={'/w/a': {'allowedTools': ['Bash'], 'history': ['1']}, '/w/b': {'trust': True}, '/w/c': {'trust': False}})
    master = dict(MINE, mcpServers={'docs': {'command': 'docs-mcp2', 'args': ['--old']}, 'gone': {'command': 'x'}, 'theirs': {'command': 't'}},
                  projects={'/w/b': {'trust': True, 'extra': 1}, '/w/a': {'allowedTools': [], 'history': ['1', '2']}})
    sync_world(b, account, master, base)


@lcase('sync/claude/nested-conflict', ['sync', A])
def _(b):
    base = {'projects': {'/w/a': {'allowedTools': [], 'model': 'x'}}}
    sync_world(b, dict(OWN, projects={'/w/a': {'allowedTools': ['Bash'], 'model': 'mine'}}),
               dict(MINE, projects={'/w/a': {'allowedTools': [], 'model': 'theirs'}}), base)


@lcase('sync/claude/type-changes', ['sync', A])
def _(b):
    # 一边是对象、另一边成了别的类型：整项冲突；null 是个值，和“没有这个键”不一样
    base = {'projects': {'/w': {}}, 'theme': None, 'tipsHistory': {'a': 1}}
    sync_world(b, dict(OWN, projects=['/w'], theme=None, tipsHistory={'a': 1, 'b': 2}),
               dict(MINE, projects={'/w': {'x': 1}}, tipsHistory=None), base)


@lcase('sync/claude/equal-numbers-are-not-changes', ['sync', A])
def _(b):
    # 1、1.0、true 在 Python 里相等：不算改动，各自的写法留着（基线里记的是共享那边的写法）
    sync_world(b, dict(OWN, verbose=1, autoUpdates=0, tipsHistory={'n': 2.0}), dict(MINE, verbose=True, autoUpdates=False, tipsHistory={'n': 2}),
               {'verbose': 1.0, 'autoUpdates': 0.0, 'tipsHistory': {'n': 2}})


@lcase('sync/claude/order-and-unicode', ['sync', A])
def _(b):
    # 已有的键留在原位，新来的排在后面；中文、表情原样
    account = {'theme': 'dark', 'numStartups': 3, 'editorMode': 'vim', 'oauthAccount': {'displayName': '爱丽丝 😀'}}
    master = {'verbose': True, 'machineID': HEX['b'], 'theme': 'light', 'mcpServers': {'文档': {'command': 'docs-mcp', 'env': {'语言': '中文'}}}, 'userID': HEX['a']}
    sync_world(b, account, master, {'theme': 'dark', 'editorMode': 'emacs'})


@lcase('sync/claude/big-and-odd-numbers', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, tipsHistory={'big': 12345678901234567890, 'f': 0.1, 'e': 1e-7, 'neg': -0.0}), dict(MINE), {})


@lcase('sync/claude/master-is-symlink', ['sync', A])
def _(b):
    # ~/.claude.json 是个链接（dotfiles）：改的是它指向的文件，权限照旧
    sync_world(b, dict(OWN, theme='light'), None, {'theme': 'dark'})
    jput(b, 'dotfiles/claude.json', dict(MINE, theme='dark'), 0o640)
    symlink(b, '~/dotfiles/claude.json', '.claude.json')


@lcase('sync/claude/master-mode-is-kept', ['sync', A])
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark'), {'theme': 'dark'})
    os.chmod(b.path('.claude.json'), 0o604)


@lcase('sync/claude/base-is-symlink', ['sync', A])
def _(b):
    # 基线文件是个链接：照 save_json 的默认写到它指向的文件里
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark'))
    b.json('elsewhere/base.json', {'theme': 'dark'})
    symlink(b, '~/elsewhere/base.json', cdir('claude') + '/.cca-sync-base.json')


@lcase('sync/claude/account-file-is-symlink-to-master', ['sync', A])
def _(b):
    # 账号的 .claude.json 链到 ~/.claude.json：先拆成自己的一份（换新的设备标识），再合并；~/.claude.json 的标识不动
    tables(b)
    jput(b, '.claude.json', dict(MINE, theme='dark', verbose=True))
    symlink(b, '~/.claude.json', cdir('claude') + '/.claude.json')


@lcase('sync/claude/alias-key', ['sync', '工作(%s)' % A])
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark'), {'theme': 'dark'})


for NAME, TEXT in (('bad-json', '{"theme": '), ('list', '[1]'), ('text', '"dark"'), ('empty-list', '[]'), ('null', 'null'), ('zero', '0'), ('empty-file', '')):
    @lcase('sync/claude/master-%s' % NAME, ['sync', A])
    def _(b, TEXT=TEXT):
        # 读不了的报错退出（设备标识这时已经准备好了）；null、[]、0 这些“假”的当空的；别的不是对象的是没人接的异常
        sync_world(b, dict(OWN, theme='light'), None, {'theme': 'dark'})
        put(b, '.claude.json', TEXT)

    @lcase('sync/claude/base-%s' % NAME, ['sync', A])
    def _(b, TEXT=TEXT):
        sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark'))
        put(b, cdir('claude') + '/.cca-sync-base.json', TEXT, 0o600)


@lcase('sync/claude/account-bad-json', ['sync', A])
def _(b):
    sync_world(b, None, dict(MINE, theme='dark'))
    put(b, cdir('claude') + '/.claude.json', '{"theme": "light"', 0o600)


@lcase('sync/claude/account-not-an-object', ['sync', A])
def _(b):
    sync_world(b, None, dict(MINE, theme='dark'))
    put(b, cdir('claude') + '/.claude.json', '[]', 0o600)


def relogin(key, **changes):
    """客户端改了自己的 .claude.json（原子保存）"""
    def f(b):
        p = cdir('claude', key) + '/.claude.json'
        with open(b.path(p), encoding='utf-8') as fh:
            data = json.load(fh)
        data.update(changes)
        atomic(b, p, json.dumps(data, ensure_ascii=False), 0o600)
    return f


def both(tool, keys=(A, B), seed=True):
    """夹具里：这几个账号都链接好（原版 Python 做）。seed：先给定设备标识，状态里就没有随机值"""
    def f(b):
        for i, k in enumerate(keys):
            if seed:
                record(b, tool, IDS[tool][i], k)
            py(b, 'link', k, tool=tool)
    return f


# tests/shared_state.py：test_native_config_conflicts_keep_shared_preference_and_account_identity
chain('sync/claude/scenario-native-conflicts', [
    shared_world, both('claude'),
    relogin(A, oauthAccount={'emailAddress': A}, cachedUsageUtilization={'account': A}), step('sync', A),
    relogin(B, oauthAccount={'emailAddress': B}, cachedUsageUtilization={'account': B}), step('sync', B),
    relogin(A, theme='light', editorMode='vim'), relogin(B, theme='blue', verbose=True),
    step('sync', A), step('sync', B), step('sync', A), step('sync', B),
])

# tests/shared_state.py：test_shared_preferences_sync_without_copying_account_identity
chain('sync/claude/scenario-preferences', [
    shared_world, both('claude'),
    relogin(A, oauthAccount={'emailAddress': A}, cachedUsageUtilization={'account': A}), step('sync', A),
    relogin(B, oauthAccount={'emailAddress': B}, cachedUsageUtilization={'account': B}), step('sync', B),
    relogin(A, theme='light'), step('sync', A), step('sync', B),
])


# --- link：共享目录的链接 ---------------------------------------------------------------------------------------------
# Codex 的 link 最后要 codex_fix_rollouts（还没移植）：它的用例名字以 link-codex/ 开头，比到那一步为止（见文件开头）；
# Claude、Codex 的 `link 键 cost` 要 cost_kick，同理是 link-cost/。

def group(tool):
    """这个工具的 link 用例叫什么、最后会不会走到还没移植的函数：(名字前缀, lcase / chain 的 pending 参数)"""
    return ('link-codex', {'pending': 'codex'}) if tool == 'codex' else ('link/' + tool, {})


def files(root, items, **kw):
    """夹具里：root 下写这几个文件 {相对路径: 内容}"""
    def f(b):
        for rel, text in items.items():
            put(b, '%s/%s' % (root, rel), text, **kw)
    return f


# --- tests/shared_state.py 的各个场景 ---

SHARED_FILES = {
    'claude': {
        'settings.json': '{"model":"opus"}',
        'settings.local.json': '{"permissions":{"allow":["Read"]}}',
        'CLAUDE.md': '# 通用规则\n',
        'history.jsonl': '{"display":"old prompt"}\n',
        'projects/-workspace/session.jsonl': '{"sessionId":"shared-session"}\n',
        'projects/-workspace/memory/MEMORY.md': '# 项目记忆\n',
        'tasks/shared-session/1.json': '{"id":"1","subject":"续接的任务"}',
        'skills/example/SKILL.md': '# 技能\n',
        'rules/example.md': '# 规则\n',
        'hooks/notify.sh': '#!/bin/sh\n# 通用钩子\n',
    },
    'codex': {
        'config.toml': 'model = "example-model"\n',
        'deep-review.config.toml': 'model_reasoning_effort = "high"\n',
        'AGENTS.md': '# 通用规则\n',
        'history.jsonl': '{"text":"old prompt"}\n',
        'session_index.jsonl': '{"id":"shared-thread"}\n',
        'sessions/2026/11/05/rollout-shared.jsonl': '{"type":"session_meta"}\n',
        'archived_sessions/rollout-old.jsonl': '{"type":"session_meta"}\n',
        'memories/memory.md': '# 项目记忆\n',
        'skills/example/SKILL.md': '# 技能\n',
        'rules/example.rules': '# 规则\n',
    },
    'agy': {
        'GEMINI.md': '# 通用规则\n',
        'antigravity-cli/settings.json': '{"theme":"dark"}',
        'antigravity-cli/history.jsonl': '{"text":"old prompt"}\n',
        'antigravity-cli/conversations/conv-1.pb': 'conversation\n',
        'antigravity-cli/brain/notes.md': '# 记忆\n',
        'antigravity-cli/cache/last_conversations.json': '{"/w":"conv-1"}',
        'antigravity-cli/cache/other-cache.bin': 'not shared\n',
        'antigravity-cli/log/agy.log': 'not shared\n',
        'config/mcp_config.json': '{"mcpServers":{}}',
        'config/skills/example/SKILL.md': '# 技能\n',
        'config/GEMINI.md': '# 全局规则\n',
        'oauth_creds.json': 'not shared\n',
    },
}
MEMORY = {'claude': 'projects/-workspace/memory/MEMORY.md', 'codex': 'memories/memory.md', 'agy': 'antigravity-cli/brain/notes.md'}
HISTORY = {'claude': 'history.jsonl', 'codex': 'history.jsonl', 'agy': 'antigravity-cli/history.jsonl'}


def session_wrote(tool, key=A):
    """会话里经链接写的：新的记忆、历史多一行（都落在共享主目录里）"""
    def f(b):
        put(b, '%s/%s' % (cdir(tool, key), MEMORY[tool]), '# 第一账号新增记忆\n')
        with open(b.path('%s/%s' % (cdir(tool, key), HISTORY[tool])), 'a', encoding='utf-8') as fh:
            fh.write('{"text":"new prompt"}\n')
    return f


for T in TOOLS:
    # test_history_memory_configuration_and_resume_files_remain_shared（agy 是照样子补的）
    NAME, WAIT = group(T)
    chain(NAME + '/scenario-shared-files', [
        shared_world, files(MASTER[T], SHARED_FILES[T]), step('link', A), step('link', B),
        session_wrote(T), step('link', A), step('link', B),
    ], tool=T, **WAIT)


def claude_task_lists(b):
    """tasks 共享以前各账号各有一份：列表 → (共享的那份, 修改时间), (这个账号的那份, 修改时间)"""
    master, mine = '.claude/tasks', cdir('claude') + '/tasks'
    os.unlink(b.path(mine))
    lists = {
        'only-here': ((None, 0), ({'1.json': 'alice 1'}, 100)),
        'same': (({'1.json': 'same 1', '.highwatermark': '1'}, 100), ({'1.json': 'same 1', '.highwatermark': '1'}, 200)),
        'newer-here': (({'1.json': 'old shared 1', '2.json': 'old shared 2', '.highwatermark': '2'}, 100), ({'1.json': 'alice 1', '.highwatermark': '1'}, 200)),
        'older-here': (({'1.json': 'new shared 1', '.highwatermark': '1'}, 200), ({'1.json': 'stale alice 1', '2.json': 'stale alice 2'}, 100)),
    }
    for name, sides in lists.items():
        for root, (items, when) in zip((master, mine), sides):
            if items:
                for f, text in items.items():
                    put(b, '%s/%s/%s' % (root, name, f), text)
                stamp(b, '%s/%s' % (root, name), when)
    for root, text, when in ((master, 'shared note', 100), (mine, 'alice note', 200)):
        put(b, root + '/note.txt', text, mtime=when)


# test_task_lists_are_shared_from_the_first_launch、test_task_lists_kept_per_account_are_adopted_whole
chain('link/claude/scenario-task-lists', [
    shared_world, both('claude'), files(cdir('claude') + '/tasks', {'session-a/1.json': '{"id":"1"}'}), step('link', A),
    claude_task_lists, step('link', A, mtimes=['.claude/tasks/?*', '.cca/conflicts/*/*']), step('link', B),
])

for T in ('claude', 'codex'):
    # test_unknown_runtime_state_is_not_imported_or_shared
    NAME, WAIT = group(T)
    chain(NAME + '/scenario-unknown-runtime-state', [
        shared_world, files(MASTER[T], {'future-runtime.json': 'master state', 'future-runtime/lease': 'master lease'}), both(T),
        files(cdir(T), {'future-runtime.json': 'alice state', 'future-runtime/lease': 'alice lease'}), step('link', A),
    ], tool=T, **WAIT)


def legacy_links(key):
    """旧版 cca 留下的：没在名单上的条目也链到了共享主目录的同名项（相对路径的链接）"""
    def f(b):
        for name in ('future-runtime.json', 'future-runtime'):
            p = b.path('%s/%s' % (cdir('claude', key), name))
            if os.path.islink(p):
                os.unlink(p)
            os.symlink(os.path.relpath(b.path('.claude/' + name), os.path.dirname(p)), p)
    return f


# test_legacy_unknown_shared_links_become_independent_copies
chain('link/claude/scenario-legacy-links-become-copies', [
    shared_world, files('.claude', {'future-runtime.json': 'legacy state', 'future-runtime/lease': 'legacy lease'}),
    lambda b: [stamp(b, '.claude/future-runtime' + x, 300) for x in ('', '.json')], both('claude'),
    legacy_links(A), step('link', A), step('link', A), legacy_links(B), step('link', B), step('link', B),
    files(cdir('claude'), {'future-runtime.json': 'alice private state', 'future-runtime/lease': 'alice private lease'}), step('link', A),
], mtimes=['.cca/accounts/*/claude/future-runtime*'])


def user_links(b):
    client = cdir('claude')
    put(b, 'my-state.json', 'user-owned state')
    symlink(b, '~/my-state.json', client + '/my-state.json')
    put(b, '.claude/different-name.json', 'another user-owned state')
    symlink(b, '~/.claude/different-name.json', client + '/custom-state.json')
    symlink(b, '~/.claude/old-runtime.json', client + '/old-runtime.json')


# test_unknown_user_links_survive_and_broken_legacy_links_are_removed
chain('link/claude/scenario-user-links-survive', [shared_world, both('claude', (A,)), user_links, step('link', A)])


def nested_legacy(b):
    put(b, '.claude/future-runtime/state.json', 'shared source state')
    put(b, 'external-state/marker', 'external target state')
    symlink(b, '~/external-state', '.claude/future-runtime/nested')
    symlink(b, '~/.claude/future-runtime', cdir('claude') + '/future-runtime')


# test_legacy_directory_with_nested_symlink_fails_without_changing_sources
chain('link/claude/scenario-nested-symlink-refused', [shared_world, both('claude', (A,)), nested_legacy, step('link', A, rc=1), step('link', A)])

ATOMIC = (('claude', 'settings.json', '{"model":"old"}', '{"model":"new"}'),
          ('codex', 'config.toml', 'model = "old"\n', 'model = "new"\n'),
          ('codex', 'deep-review.config.toml', 'model_reasoning_effort = "medium"\n', 'model_reasoning_effort = "high"\n'),
          ('agy', 'antigravity-cli/settings.json', '{"theme":"dark"}', '{"theme":"light","lastWriter":"alice"}'),
          ('agy', 'config/GEMINI.md', '# old\n', '# new\n'))
for T, FILE, BEFORE, AFTER in ATOMIC:
    # test_atomic_configuration_save_repairs_link_and_updates_other_account（agy 的两个是照样子补的）
    NAME, WAIT = group(T)
    chain('%s/scenario-atomic-save/%s' % (NAME, FILE.replace('/', '-')), [
        shared_world, files(MASTER[T], {FILE: BEFORE}, mtime=300), both(T),
        lambda b, T=T, FILE=FILE, AFTER=AFTER: atomic(b, '%s/%s' % (cdir(T), FILE), AFTER, mtime=500),
        step('link', A, mtimes=[MASTER[T] + '/' + FILE]), step('link', B),
    ], tool=T, **WAIT)

MERGE_BASE = {'model': 'base', 'theme': 'dark', 'obsolete': True, 'permissions': {'allow': ['Read'], 'deny': ['Write']}}
MERGE_ALICE = {'model': 'alice', 'theme': 'dark', 'permissions': {'allow': ['Read', 'Bash'], 'deny': ['Write']}}
MERGE_BOB = dict(MERGE_BASE, theme='light', permissions={'allow': ['Read'], 'deny': ['Edit']})

# test_json_concurrent_edits_merge_fields_and_deletions
chain('link/claude/scenario-json-fields-merge', [
    shared_world, files('.claude', {'settings.json': json.dumps(MERGE_BASE)}), both('claude'),
    lambda b: atomic(b, cdir('claude', A) + '/settings.json', json.dumps(MERGE_ALICE)),
    lambda b: atomic(b, cdir('claude', B) + '/settings.json', json.dumps(MERGE_BOB)),
    step('link', A), step('link', B), step('link', A),
])

CONFLICTS = (('claude', 'settings.json', '{"model":"base"}', '{"model":"alice"}', '{"model":"bob"}'),
             ('codex', 'config.toml', 'model = "base"\neffort = "medium"\n', 'model = "alice"\neffort = "medium"\n', 'model = "base"\neffort = "high"\n'),
             ('codex', 'deep-review.config.toml', 'model = "base"\n', 'model = "alice"\n', 'model = "bob"\n'),
             ('agy', 'config/mcp_config.json', '{"mcpServers":{"a":{"command":"base"}}}', '{"mcpServers":{"a":{"command":"alice"}}}', '{"mcpServers":{"a":{"command":"bob"}}}'),
             ('agy', 'GEMINI.md', '# base\n', '# alice\n', '# bob\n'))
for T, FILE, INITIAL, ALICE, BOB in CONFLICTS:
    # test_concurrent_config_conflicts_preserve_master_and_complete_local_copy（agy 的两个是照样子补的）
    NAME, WAIT = group(T)
    chain('%s/scenario-concurrent-conflict/%s' % (NAME, FILE.replace('/', '-')), [
        shared_world, files(MASTER[T], {FILE: INITIAL}), both(T),
        lambda b, T=T, FILE=FILE, ALICE=ALICE: atomic(b, '%s/%s' % (cdir(T, A), FILE), ALICE),
        lambda b, T=T, FILE=FILE, BOB=BOB: atomic(b, '%s/%s' % (cdir(T, B), FILE), BOB, 0o640, mtime=400),
        step('link', A), step('link', B, mtimes=[ROOT[T].replace('accounts', 'conflicts') + '/*/*']), step('link', B),
    ], tool=T, **WAIT)

for T, FILE, SHARED, LOCAL in (('claude', 'settings.json', '{"model":"shared"}', '{"model":"local"}'),
                               ('codex', 'config.toml', 'model = "shared"\n', 'model = "local"\n'),
                               ('agy', 'antigravity-cli/keybindings.json', '{"k":"shared"}', '{"k":"local"}')):
    # test_first_adoption_conflict_keeps_shared_config_and_backs_up_local
    NAME, WAIT = group(T)

    @lcase('%s/scenario-first-adoption-conflict' % NAME, ['link', A], tool=T, mtimes=[MASTER[T] + '/' + FILE], **WAIT)
    def _(b, T=T, FILE=FILE, SHARED=SHARED, LOCAL=LOCAL):
        shared_world(b)
        put(b, '%s/%s' % (MASTER[T], FILE), SHARED, mtime=100)
        put(b, '%s/%s' % (cdir(T), FILE), LOCAL, mtime=200)

# test_codex_profile_settings_check_reports_unselected_profile_after_atomic_save 里 link 的那一步
chain('link-codex/scenario-profile-atomic-save', [
    shared_world, files('.codex', {'config.toml': 'model = "shared"\n', 'work.config.toml': 'model_reasoning_effort = "medium"\n',
                                   'deep-review.config.toml': 'model_reasoning_effort = "high"\n'}), both('codex'),
    lambda b: atomic(b, cdir('codex') + '/deep-review.config.toml', 'openai_base_url = "https://evil.invalid"\nmodel_reasoning_effort = "high"\n'),
    step('link', A), step('link', B),
], tool='codex', pending='codex')

# test_invalid_atomic_json_preserves_valid_shared_config_and_original_copy
chain('link/claude/scenario-invalid-json-keeps-shared', [
    shared_world, files('.claude', {'settings.json': '{"model":"shared","permissions":{"allow":["Read"]}}'}), both('claude'),
    lambda b: atomic(b, cdir('claude') + '/settings.json', '{"model":'), step('link', A),
    lambda b: atomic(b, cdir('claude') + '/settings.json', 'not json\n'), step('link', A), step('link', B),
])

# test_invalid_json_without_existing_shared_config_is_not_installed
chain('link/claude/scenario-invalid-json-not-installed', [
    shared_world, both('claude'), lambda b: atomic(b, cdir('claude') + '/settings.json', '{"permissions":{"allow":['),
    step('link', A), step('link', B),
])

MCP = 'config/mcp_config.json'
SERVERS = '{"mcpServers": {"docs": {"command": "docs-mcp"}}}'

# test_blank_json_placeholder_is_shared_without_conflict
chain('link/agy/scenario-blank-json-placeholder', [
    shared_world, both('agy'),
    files(cdir('agy'), {MCP: ''}), step('link', A),                                   # agy 第一次运行建的 0 字节文件：并回去、链接上，不算冲突
    lambda b: atomic(b, cdir('agy') + '/' + MCP, SERVERS), step('link', A),           # 共享的是空文件：账号里写了内容照常并回去
    files(cdir('agy', B), {MCP: ''}), lambda b: os.utime(b.path('.gemini/' + MCP), (100, 100)),
    step('link', B, mtimes=['.gemini/' + MCP]),                                       # 另一个账号新建了个空的：共享的原样不动
    lambda b: atomic(b, cdir('agy') + '/' + MCP, '{"mcpServers": {'), step('link', A),   # 坏的 JSON：不装，留下完整副本
], tool='agy')


# --- tests/device_identity.py 的各个场景（那边只测 Claude、Codex，这里 agy 也照样走一遍）。前面 device/ 那一组是同样的路、
#     标识给定的单步用例；这里照测试原样一步步来（第一步的标识是随机的，后面各步比的是它们留没留住） ---

def seed(tool, ids, key=A):
    """测试里的 seed_client：客户端文件里先有这组标识（Claude 的还带着登录和别的配置）"""
    return lambda b: client(b, tool, ids, key, **({'oauthAccount': {'emailAddress': key}, 'localPreference': True} if tool == 'claude' else {}))


def drop_client(tool, key=A):
    """客户端把自己存的设备标识删了（退出登录、重置）"""
    def f(b):
        os.unlink(b.path(client_file(tool, key)))
        if tool == 'agy':
            os.unlink(b.path(cdir(tool, key) + '/antigravity-cli/jetski_state.pbtxt'))
    return f


def copy_identity(tool, src, dst):
    """dst 的客户端文件里是 src 现在的标识（旧版整个目录拷出来的副本）"""
    def f(b):
        with open(b.path(adir(tool, src) + '/device.json'), encoding='utf-8') as fh:
            j = json.load(fh)
        seed(tool, tuple(j[n] for n in FIELDS[tool]), dst)(b)
    return f


for T in TOOLS:
    NAME, WAIT = group(T)
    WORLD = [dev_world, agy_master]
    RECORDS = (adir(T, A) + '/device.json', adir(T, B) + '/device.json')

    # test_new_accounts_are_distinct_and_stable_after_alias_and_sync
    chain(NAME + '/scenario-identity-new-accounts', WORLD + [
        step('link', A), step('link', B), step('sync', A), step('link', '工作(%s)' % A), step('sync', B), step('link', '工作(%s)' % B),
    ], tool=T, **WAIT)

    # test_client_reset_restores_the_same_identity
    chain(NAME + '/scenario-identity-client-reset', WORLD + [
        step('link', A), drop_client(T), step('link', A), seed(T, IDS[T][0]), step('link', A),
    ], tool=T, **WAIT)

    # test_unique_existing_identity_and_other_account_data_survive_migration
    chain(NAME + '/scenario-identity-unique-existing', WORLD + [seed(T, IDS[T][0]), step('link', A)], tool=T, **WAIT)

    # test_duplicate_legacy_profiles_get_independent_values
    chain(NAME + '/scenario-identity-duplicate-legacy', WORLD + [
        seed(T, IDS[T][0], A), seed(T, IDS[T][0], B), step('link', A), step('link', B), step('link', A), step('link', B),
    ], tool=T, **WAIT)

    # test_shared_links_are_detached_without_changing_master：链接的、硬链接的各一个账号
    chain(NAME + '/scenario-identity-shared-links', WORLD + [
        lambda b, T=T: symlink(b, '~/' + master_file(T), client_file(T, A)), step('link', A),
        lambda b, T=T: hardlink(b, master_file(T), client_file(T, B)), step('link', B, hardlinks=[(master_file(T), client_file(T, B))]),
        step('link', A), step('link', B),
    ], tool=T, **WAIT)

    # test_symlink_alias_of_account_does_not_rotate_identity
    chain(NAME + '/scenario-identity-symlink-alias', WORLD + [
        step('link', A), lambda b, T=T: symlink(b, '~/' + adir(T), ROOT[T] + '/linked-copy'), step('link', A), step('link', A),
    ], tool=T, **WAIT)

    # test_legacy_copy_does_not_rotate_an_already_managed_account
    chain(NAME + '/scenario-identity-legacy-copy', WORLD + [
        step('link', A), copy_identity(T, A, B), step('link', A), step('link', B), step('link', A), step('link', B),
    ], tool=T, **WAIT)

    # test_shared_identity_record_is_fully_migrated_on_first_prepare
    chain(NAME + '/scenario-identity-shared-record', WORLD + [
        step('link', A), lambda b, RECORDS=RECORDS: hardlink(b, *RECORDS), step('device', A, hardlinks=[RECORDS]),
        step('device', A), step('device', A), step('device', B), step('device', A),
    ], tool=T, **WAIT)


# --- 一次会话前后（照 tests/setup.sh 里假客户端写文件的样子；回归的 claude / codex / agy 各节就是这么走的） ---

def session(tool, key):
    """夹具里：这个账号开了一次会话，客户端在自己的配置目录里写了这些"""
    tag = key.split('@')[0]

    def load(p, default):
        if not os.path.exists(p):
            return default
        with open(p, encoding='utf-8') as fh:
            return json.load(fh)

    def write(p, text, how='w'):
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, how, encoding='utf-8') as fh:
            fh.write(text)

    def swap(p, text):   # 在原路径旁边写临时文件再改名：链接就此换成普通文件
        write(p + '.tmp', text)
        os.replace(p + '.tmp', p)

    def f(b):
        cd = b.path(cdir(tool, key))
        if tool == 'claude':
            j = load(cd + '/.claude.json', {})
            j.setdefault('projects', {})['/p-from-' + tag] = {'hasTrustDialogAccepted': True}
            j.setdefault('mcpServers', {})['mcp-' + tag] = {'command': 'echo'}
            write(cd + '/.claude.json', json.dumps(j, indent=2))
            swap(cd + '/settings.json', json.dumps(dict(load(cd + '/settings.json', {}), lastWriter=tag)))
            write(cd + '/output-styles/%s.md' % tag, 'style ' + tag)
            write(cd + '/history.jsonl', json.dumps({'display': 'hi from ' + tag}) + '\n', 'a')
            write(cd + '/projects/-p1/memory/%s.md' % tag, 'memory from ' + tag)
        elif tool == 'codex':
            cfg = os.path.realpath(cd + '/config.toml')   # codex 改 config.toml 是顺着链接改的
            old = ''
            if os.path.exists(cfg):
                with open(cfg, encoding='utf-8') as fh:
                    old = fh.read()
            swap(cfg, old + '# touched by %s\n' % tag)
            write(cd + '/history.jsonl', json.dumps({'text': 'hi from ' + tag}) + '\n', 'a')
            idx, lines = cd + '/session_index.jsonl', []
            if os.path.exists(idx):
                with open(idx, encoding='utf-8') as fh:
                    lines = fh.read().splitlines()
            swap(idx, '\n'.join(lines + [json.dumps({'id': tag, 'thread_name': 'from ' + tag})]) + '\n')
            write(cd + '/sessions/2026/11/03/rollout-%s.jsonl' % tag, '{}\n')
            write(cd + '/models_cache.json', '{}')
            os.makedirs(cd + '/worktrees/' + tag, exist_ok=True)
            os.makedirs(cd + '/skills/from-' + tag, exist_ok=True)
        else:
            cli = cd + '/antigravity-cli'
            write(cli + '/conversations/from-%s.pb' % tag, 'conversation ' + tag)
            write(cli + '/history.jsonl', json.dumps({'display': 'hi from ' + tag}) + '\n', 'a')
            swap(cli + '/settings.json', json.dumps(dict(load(cli + '/settings.json', {}), lastWriter=tag)))
            write(cli + '/cache/last_conversations.json', json.dumps(dict(load(cli + '/cache/last_conversations.json', {}), **{'/w/' + tag: 'conv-' + tag})))
            write(cli + '/log/agy.log', 'log ' + tag)
            os.makedirs(cd + '/config/skills/from-' + tag, exist_ok=True)
            if not os.path.lexists(cd + '/config/mcp_config.json'):   # 真的 agy（1.5.x）：config/ 里没有就建一个 0 字节的
                write(cd + '/config/mcp_config.json', '')
    return f


SESSION_START = {'claude': {'settings.json': '{"theme": "dark"}', 'projects/-p1/memory/shared.md': 'shared memory'},
                 'codex': {'config.toml': 'model = "gpt-test"\n', 'skills/mine/SKILL.md': '# mine\n'},
                 'agy': {'antigravity-cli/settings.json': '{"theme": "dark"}', 'GEMINI.md': '# 共享的规则\n', 'config/skills/mine/SKILL.md': '# mine\n'}}
for T in TOOLS:
    NAME, WAIT = group(T)
    after = (lambda k: [step('link', k), step('sync', k)]) if T == 'claude' else (lambda k: [step('link', k)])
    chain(NAME + '/scenario-sessions', (
        [shared_world, files(MASTER[T], SESSION_START[T]), both(T), session(T, A)] + after(A)        # A 的会话结束
        + [step('link', B), session(T, B)] + after(B)                                                 # B 启动前先链接，会话结束
        + [step('link', A), session(T, A)] + after(A)                                                 # A 再来一次
    ), tool=T, **WAIT)


# --- link：开头几步、参数、基线文件 ---

def linked(b, tool, keys=(A,)):
    """夹具：shared_world，这几个账号已经链接好（设备标识是给定的）"""
    shared_world(b)
    both(tool, keys)(b)


for T in TOOLS:
    NAME, WAIT = group(T)   # 走得到最后的用例（Codex 的要等 codex_fix_rollouts）；半路报错退出的叫 link/工具/…，三种工具都照常比

    @lcase(NAME + '/empty-home', ['link', A], tool=T, **WAIT)
    def _(b):
        pass

    @lcase(NAME + '/fresh', ['link', A], tool=T, **WAIT)
    def _(b):
        dev_world(b)
        agy_master(b)

    @lcase(NAME + '/already-linked', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        linked(b, T)

    @lcase(NAME + '/alias-key', ['link', '工作(%s)' % A], tool=T, **WAIT)
    def _(b, T=T):
        linked(b, T)

    @lcase(NAME + '/second-argument-is-not-cost', ['link', A, 'costs'], tool=T, **WAIT)
    def _(b, T=T):
        linked(b, T)

    @lcase('link/%s/no-arguments' % T, ['link'], tool=T)
    def _(b):
        dev_world(b)

    @lcase('link/%s/too-many-arguments' % T, ['link', A, 'cost', 'x'], tool=T)
    def _(b):
        dev_world(b)

    @lcase('link/%s/account-dir-is-symlink' % T, ['link', A], tool=T)
    def _(b, T=T):
        # 报错之前共享主目录和里面预先要有的目录已经建了
        dev_world(b)
        mkdir(b, 'elsewhere')
        symlink(b, '~/elsewhere', adir(T))

    @lcase('link/%s/config-dir-is-symlink' % T, ['link', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        mkdir(b, 'elsewhere')
        symlink(b, '~/elsewhere', cdir(T))

    @lcase('link/%s/master-is-a-file' % T, ['link', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        if T == 'codex':
            shutil.rmtree(b.path('.codex'))
        put(b, MASTER[T], 'oops')

    @lcase('link/%s/precreated-dir-is-a-file' % T, ['link', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        put(b, '%s/%s' % (MASTER[T], {'claude': 'plugins', 'codex': 'memories', 'agy': 'antigravity-cli/brain'}[T]), 'oops')

    @lcase('link/%s/precreated-dir-is-a-dangling-link' % T, ['link', A], tool=T)
    def _(b, T=T):
        dev_world(b)
        symlink(b, '~/nowhere', '%s/%s' % (MASTER[T], {'claude': 'plugins', 'codex': 'memories', 'agy': 'antigravity-cli/brain'}[T]))

    @lcase(NAME + '/precreated-dir-is-a-link-to-a-dir', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        dev_world(b)
        put(b, 'dotfiles/skills/x/SKILL.md', '# x\n')
        symlink(b, '~/dotfiles/skills', '%s/%s' % (MASTER[T], {'claude': 'skills', 'codex': 'skills', 'agy': 'config/projects'}[T]))

    for KIND, TEXT in (('bad-json', '{"settings.json": '), ('list', '["settings.json"]'), ('number-value', '{"settings.json": 1}'),
                       ('object-value', '{"AGENTS.md": null, "settings.json": {"model": "x"}}'), ('null', 'null'), ('text', '"x"')):
        @lcase('link/%s/base-%s' % (T, KIND), ['link', A], tool=T)
        def _(b, T=T, TEXT=TEXT):
            # 基线读不了、格式不对：停下（设备标识这时已经准备好了），不动账号目录里的东西
            shared_world(b)
            put(b, '%s/AGENTS.md' % MASTER[T], '# shared\n')
            put(b, '%s/AGENTS.md' % cdir(T), '# mine\n')
            put(b, '%s/.cca-shared-base.json' % cdir(T), TEXT, 0o600)

    @lcase(NAME + '/base-with-stale-entries', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 基线里不是共享配置的名字去掉；主目录里已经没有的记成 null
        linked(b, T)
        b.json('%s/.cca-shared-base.json' % cdir(T), {'AGENTS.md': 'old text\n', 'not-a-config': 'x', 'projects': None, 'zzz.config.toml': None})

    @lcase(NAME + '/base-is-symlink', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 基线是个链接：换成普通文件，原来指向的那份不动
        linked(b, T)
        b.json('elsewhere/base.json', {'AGENTS.md': 'from elsewhere\n'})
        os.unlink(b.path('%s/.cca-shared-base.json' % cdir(T)))
        symlink(b, '~/elsewhere/base.json', '%s/.cca-shared-base.json' % cdir(T))

    @lcase(NAME + '/base-unchanged-is-not-rewritten', ['link', A], tool=T, mtimes=['.cca/*/.cca-shared-base.json'], **WAIT)
    def _(b, T=T):
        linked(b, T)
        p = b.path('%s/.cca-shared-base.json' % cdir(T))
        os.chmod(p, 0o644)
        os.utime(p, (100, 100))

    @lcase(NAME + '/shared-config-appears-in-master', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 上次链接以后主目录里多了共享配置、改了共享配置：链接上，基线跟着记（\r\n 记成 \n）
        linked(b, T)
        put(b, '%s/AGENTS.md' % MASTER[T], '# 规则\r\nline 2\r\n')

    @lcase(NAME + '/shared-config-is-a-directory-in-master', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 主目录里共享配置的名字是个目录：照样链接，基线里不记
        shared_world(b)
        put(b, '%s/AGENTS.md/part.md' % MASTER[T], '# part\n')

    @lcase('link/%s/shared-config-not-utf8-in-master' % T, ['link', A], tool=T)
    def _(b, T=T):
        # 主目录里的共享配置不是 UTF-8：链接都做了，记基线时出错退出（原实现是没人接的异常，见 FINDINGS）
        shared_world(b)
        put(b, '%s/AGENTS.md' % MASTER[T], b'caf\xe9\n')

    @lcase('link/%s/shared-config-not-utf8-in-account' % T, ['link', A], tool=T)
    def _(b, T=T):
        linked(b, T)
        put(b, '%s/AGENTS.md' % MASTER[T], '# shared\n')
        put(b, '%s/AGENTS.md' % cdir(T), b'caf\xe9\n')

    @lcase(NAME + '/unknown-entries-stay-private', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 名单以外的：主目录里的不链接，账号目录里的不并回去
        shared_world(b)
        for root, who in ((MASTER[T], 'master'), (cdir(T), 'alice')):
            put(b, root + '/auth.json', who)
            put(b, root + '/cache/x.bin', who)
            put(b, root + '/.hidden.config.toml', who)
            put(b, root + '/sub/work.config.toml', who)

    @lcase(NAME + '/wrong-links-are-replaced', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 共享条目上指向别处的链接（别的写法、别的目录、断的）换成指向主目录同名项的；主目录里没有这一项的就只是去掉
        linked(b, T)
        cd, m = cdir(T), MASTER[T]
        put(b, m + '/AGENTS.md', '# shared\n')
        put(b, 'dotfiles/skills/x/SKILL.md', '# x\n')
        for name, target in (('AGENTS.md', '../../../../' + ('../' if T != 'claude' else '') + m + '/AGENTS.md'), ('skills', b.path('dotfiles/skills')),
                             ('rules', b.path('nowhere')), ('agents', b.path(m) + '//agents')):
            p = b.path(cd + '/' + name)
            if os.path.lexists(p):
                os.unlink(p)
            os.symlink(target, p)

    @lcase(NAME + '/dangling-link-to-missing-master-entry-stays', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 指向主目录同名项的链接，那一项还没有：留着（以后有了就通了）
        linked(b, T)
        name = {'claude': 'rules', 'codex': 'prompts', 'agy': 'config/rules'}[T]
        symlink(b, '~/%s/%s' % (MASTER[T], name), '%s/%s' % (cdir(T), name))


for T in TOOLS:
    NAME, WAIT = group(T)

    @lcase(NAME + '/master-is-a-symlink-to-a-directory', ['link', A], tool=T, **WAIT)
    def _(b, T=T):
        # 共享主目录本身是个链接（dotfiles）：照常用，链接指向的写法还是 ~/.claude/…
        shared_world(b)
        put(b, 'dotfiles/master/AGENTS.md', '# shared\n')
        symlink(b, '~/dotfiles/master', MASTER[T])
        put(b, cdir(T) + '/AGENTS.md', '# shared\n')

    @lcase(NAME + '/other-cca-home', ['link', A], tool=T, env={'CCA_HOME': 'alt/cca-home'}, **WAIT)
    def _(b, T=T):
        shared_world(b)
        put(b, MASTER[T] + '/AGENTS.md', '# shared\n')
        put(b, cdir(T) + '/AGENTS.md', '# 默认位置的账号目录不算数\n')

    @lcase(NAME + '/readonly-flag-is-ignored', ['link', A], tool=T, env={'CCA_RO': '1'}, **WAIT)
    def _(b, T=T):
        # link 不经过 ctx()：CCA_RO 管不到它，照样写
        shared_world(b)
        put(b, MASTER[T] + '/AGENTS.md', '# shared\n')

    @lcase(NAME + '/empty-key', ['link', ''], tool=T, **WAIT)
    def _(b):
        shared_world(b)

for T in ('claude', 'codex'):
    NAME, WAIT = group(T)

    @lcase(NAME + '/api-account-by-id', ['link', '直连'], tool=T, **WAIT)
    def _(b, T=T):
        # 第三方 API 账号的目录按 id 认
        shared_world(b)
        jput(b, '.cca/%s.json' % {'claude': 'accounts'}.get(T, T), {A: 'US-A', '直连': {'base_url': 'https://api.example.com/v1', 'id': 'zjd'}})
        put(b, '%s/%s/%s/AGENTS.md' % (ROOT[T], ident('zjd'), SUB[T]), '# mine\n')


@lcase('sync/claude/readonly-flag-is-ignored', ['sync', A], env={'CCA_RO': '1'})
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark'), {'theme': 'dark'})


@lcase('sync/claude/other-cca-home', ['sync', A], env={'CCA_HOME': 'alt/cca-home'})
def _(b):
    sync_world(b, dict(OWN, theme='light'), dict(MINE, theme='dark', verbose=True), {'theme': 'dark'})


@lcase('sync/claude/account-dir-is-symlink', ['sync', A])
def _(b):
    tables(b)
    jput(b, '.claude.json', dict(MINE, theme='dark'))
    mkdir(b, 'elsewhere/claude')
    symlink(b, '~/elsewhere', adir('claude'))


@lcase('link/agy/cost-argument', ['link', A, 'cost'], tool='agy')
def _(b):
    # agy 没有折合金额：带不带 cost 都一样
    linked(b, 'agy')
    b.cfg({'version': 2})


@lcase('link-cost/claude/no-config-json', ['link', A, 'cost'], pending='cost')
def _(b):
    # 原版在这里什么都不做（还没有 config.json）
    linked(b, 'claude')


@lcase('link-cost/claude/switched-off-by-env', ['link', A, 'cost'], env={'CCA_COST': 'off'}, pending='cost')
def _(b):
    linked(b, 'claude')
    b.cfg({'version': 2})


@lcase('link-cost/claude/switched-off-in-config', ['link', A, 'cost'], pending='cost')
def _(b):
    linked(b, 'claude')
    b.cfg({'version': 2, 'cost': False})


@lcase('link/claude/cost-argument-but-link-fails', ['link', A, 'cost'])
def _(b):
    # 链接没做成就退出：走不到 cost_kick
    dev_world(b)
    put(b, cdir('claude') + '/.cca-shared-base.json', '[]', 0o600)


@lcase('link-codex/cost-argument', ['link', A, 'cost'], tool='codex', env={'CCA_COST': 'off'}, pending='codex cost')
def _(b):
    linked(b, 'codex')
    b.cfg({'version': 2})


# --- link：账号目录里的真实条目并回共享主目录（adopt） ---

def adopt_case(name, tool='claude', keys=(A,), **kw):
    """先链接好，再照被装饰的函数改夹具，然后 link A"""
    prefix, wait = group(tool)

    def reg(change):
        def setup(b):
            linked(b, tool, keys)
            change(b)
        lcase('%s/adopt/%s' % (prefix, name), ['link', A], tool=tool, **dict(wait, **kw))(setup)
        return change
    return reg


def unlinked(b, tool, name):
    """账号目录里这个共享条目的链接去掉（客户端把它换成了真实的文件、目录），返回它的路径（相对 HOME）"""
    rel = '%s/%s' % (cdir(tool), name)
    if os.path.lexists(b.path(rel)):
        os.unlink(b.path(rel))
    return rel


for T, NEW_DIR, NEW_FILE in (('claude', 'rules', 'history.jsonl'), ('codex', 'prompts', 'session_index.jsonl'), ('agy', 'config/rules', 'antigravity-cli/history.jsonl')):
    @adopt_case('new-dir-is-moved', T, mtimes=[MASTER[T] + '/' + NEW_DIR + '*'])
    def _(b, T=T, NEW_DIR=NEW_DIR):
        # 主目录里还没有：整个搬过去（权限、修改时间原样），再链接上
        put(b, '%s/%s/a/one.md' % (cdir(T), NEW_DIR), '# one\n', 0o640, mtime=100)
        symlink(b, 'one.md', '%s/%s/a/link' % (cdir(T), NEW_DIR))
        stamp(b, '%s/%s' % (cdir(T), NEW_DIR), 100)

    @adopt_case('new-file-is-moved', T, mtimes=[MASTER[T] + '/' + NEW_FILE])
    def _(b, T=T, NEW_FILE=NEW_FILE):
        put(b, '%s/%s' % (cdir(T), NEW_FILE), '{"n":1}\n', 0o600, mtime=100)

    @adopt_case('jsonl-lines-are-appended', T)
    def _(b, T=T, NEW_FILE=NEW_FILE):
        # 两边都有的 .jsonl：账号这边多出来的行接到共享的后面（共享的结尾没换行先补上）
        put(b, '%s/%s' % (MASTER[T], NEW_FILE), '{"n":1}\n{"n":2}\r\n{"n":3}')
        put(b, unlinked(b, T, NEW_FILE), '{"n":2}\n{"n":4}\n\n{"n":4}\n{"n":1}\n{"名":"五"}')

    @adopt_case('jsonl-nothing-new', T, mtimes=[MASTER[T] + '/' + NEW_FILE])
    def _(b, T=T, NEW_FILE=NEW_FILE):
        put(b, '%s/%s' % (MASTER[T], NEW_FILE), '{"n":1}\n{"n":2}\n', mtime=100)
        put(b, unlinked(b, T, NEW_FILE), '{"n":2}\r\n{"n":1}')

    @adopt_case('jsonl-into-empty-file', T)
    def _(b, T=T, NEW_FILE=NEW_FILE):
        put(b, '%s/%s' % (MASTER[T], NEW_FILE), '')
        put(b, unlinked(b, T, NEW_FILE), '{"n":1}\n')

    @adopt_case('jsonl-through-master-symlink', T)
    def _(b, T=T, NEW_FILE=NEW_FILE):
        # 主目录里那一项是个链接：接到它指向的文件里，链接留着
        put(b, 'dotfiles/log.jsonl', '{"n":1}\n')
        symlink(b, '~/dotfiles/log.jsonl', '%s/%s' % (MASTER[T], NEW_FILE))
        put(b, unlinked(b, T, NEW_FILE), '{"n":2}\n')

    @adopt_case('file-here-directory-there', T)
    def _(b, T=T, NEW_FILE=NEW_FILE):
        # 类型不一样：账号这边的移进 conflicts（名字后面是时间戳），链接指回共享的
        put(b, '%s/%s/keep' % (MASTER[T], NEW_FILE), 'k')
        put(b, unlinked(b, T, NEW_FILE), '{"n":1}\n')

    @adopt_case('directory-here-file-there', T)
    def _(b, T=T, NEW_DIR=NEW_DIR):
        put(b, '%s/%s' % (MASTER[T], NEW_DIR), 'a file')
        put(b, '%s/x/y.md' % unlinked(b, T, NEW_DIR), '# y\n')

    @adopt_case('directory-here-dangling-link-there', T)
    def _(b, T=T, NEW_DIR=NEW_DIR):
        symlink(b, '~/nowhere', '%s/%s' % (MASTER[T], NEW_DIR))
        put(b, '%s/x/y.md' % unlinked(b, T, NEW_DIR), '# y\n')

    @lcase('link/%s/adopt/file-here-dangling-link-there' % T, ['link', A], tool=T)
    def _(b, T=T, NEW_FILE=NEW_FILE):
        # 主目录里是断掉的链接、账号目录里是真实文件：原实现出错退出（见 FINDINGS），账号这边的原样留着
        linked(b, T)
        symlink(b, '~/nowhere', '%s/%s' % (MASTER[T], NEW_FILE))
        put(b, unlinked(b, T, NEW_FILE), '{"n":1}\n')

    @adopt_case('trees-are-merged', T, mtimes=[MASTER[T] + '/' + NEW_DIR + '/*.md'])
    def _(b, T=T, NEW_DIR=NEW_DIR):
        # 两边都是目录：主目录没有的拷过去（链接照样是链接），都有的文件 .jsonl 并行、别的留较新的；并完账号这边的删掉
        src, dst = unlinked(b, T, NEW_DIR), '%s/%s' % (MASTER[T], NEW_DIR)
        for rel, text, when in (('only-src.md', 's', 100), ('both.md', 'src newer', 200), ('older.md', 'src older', 100), ('same.md', 'same', 900),
                                ('log.jsonl', 'b\nc\n', 100), ('sub/deep/x.md', 'x', 100), ('sub/both.md', 'src', 200), ('file-there-is-dir', 'f', 300),
                                ('empty-dir/.keep', '', 100), ('名字/文件.md', '内容', 100)):
            put(b, '%s/%s' % (src, rel), text, 0o640, mtime=when)
        os.unlink(b.path(src + '/empty-dir/.keep'))
        symlink(b, 'only-src.md', src + '/link')
        symlink(b, 'sub', src + '/dirlink')
        symlink(b, '/nonexistent', src + '/dangling')
        symlink(b, 'sub', src + '/link-there-is-file')
        symlink(b, 'only-src.md', src + '/sub/link-there-is-link')
        for rel, text, when in (('both.md', 'dst', 100), ('older.md', 'dst newer', 200), ('same.md', 'same', 100), ('log.jsonl', 'a\nb\n', 100),
                                ('sub/both.md', 'dst older', 100), ('only-dst.md', 'd', 100), ('file-there-is-dir/keep', 'k', 100), ('link-there-is-file', 'real', 100)):
            put(b, '%s/%s' % (dst, rel), text, mtime=when)
        symlink(b, 'elsewhere', dst + '/sub/link-there-is-link')
        for d in ('sub/deep', 'sub', 'empty-dir', '名字'):
            os.utime(b.path('%s/%s' % (src, d)), (150, 150))

    @adopt_case('tree-through-master-symlink', T)
    def _(b, T=T, NEW_DIR=NEW_DIR):
        # 主目录里那一项是指向目录的链接：并进它指向的目录
        put(b, 'dotfiles/rules/old.md', 'old')
        symlink(b, '~/dotfiles/rules', '%s/%s' % (MASTER[T], NEW_DIR))
        put(b, '%s/new.md' % unlinked(b, T, NEW_DIR), 'new')

    @lcase('link/%s/adopt/tree-blocked-by-a-file' % T, ['link', A], tool=T)
    def _(b, T=T, NEW_DIR=NEW_DIR):
        # 账号这边的子目录在主目录里是个文件：原实现在这里出错退出（没人接的 FileExistsError），账号这边的留着
        linked(b, T)
        put(b, '%s/%s/sub' % (MASTER[T], NEW_DIR), 'a file')
        put(b, '%s/sub/x.md' % unlinked(b, T, NEW_DIR), 'x')

    @adopt_case('readonly-empty-dir-is-copied', T, chmod={'%s/%s' % (cdir(T), NEW_DIR): 0o555}, mtimes=[MASTER[T] + '/' + NEW_DIR])
    def _(b, T=T, NEW_DIR=NEW_DIR):
        # macOS 上没有写权限的目录改名搬不到别的目录下：shutil.move 改成拷过去再删（权限、修改时间跟着）
        mkdir(b, '%s/%s' % (cdir(T), NEW_DIR), mtime=100)

    @lcase('link/%s/adopt/readonly-dir-with-content-is-refused' % T, ['link', A], tool=T, chmod={'%s/%s' % (cdir(T), NEW_DIR): 0o555})
    def _(b, T=T, NEW_DIR=NEW_DIR):
        linked(b, T)
        put(b, '%s/%s/x.md' % (cdir(T), NEW_DIR), 'x')


@adopt_case('plain-file-newer-wins', mtimes=['.claude/memory', '.claude/prompts'])
def _(b):
    # 名单上不是 .jsonl 也不是共享配置的文件：内容不一样时留修改时间较新的（带着它的权限和修改时间）
    put(b, '.claude/memory', 'shared old', mtime=100)
    put(b, cdir('claude') + '/memory', 'mine new', 0o600, mtime=200)
    put(b, '.claude/prompts', 'shared new', mtime=200)
    put(b, cdir('claude') + '/prompts', 'mine old', 0o600, mtime=100)


@adopt_case('plain-file-same-age-keeps-shared', mtimes=['.claude/memory'])
def _(b):
    put(b, '.claude/memory', 'shared', mtime=100)
    put(b, cdir('claude') + '/memory', 'mine', mtime=100)


@adopt_case('config-name-is-a-directory-here')
def _(b):
    # 共享配置的名字在账号目录里是个目录：不走配置合并，当普通条目并回去
    put(b, cdir('claude') + '/CLAUDE.md/part.md', '# part\n')
    put(b, '.claude/keybindings.json/keep', 'k')
    put(b, unlinked(b, 'claude', 'keybindings.json'), '{}')


@adopt_case('several-conflicts-in-order')
def _(b):
    # 一次里有好几处：按名字的先后处理，stderr 里的先后也照这个
    cd = cdir('claude')
    put(b, '.claude/settings.json', '{"model":"shared"}')
    put(b, unlinked(b, 'claude', 'settings.json'), '{"model":')
    put(b, '.claude/CLAUDE.md', '# shared\n')
    put(b, unlinked(b, 'claude', 'CLAUDE.md'), '# mine\n')
    put(b, '.claude/rules', 'file')
    put(b, cd + '/rules/x.md', 'x')
    put(b, '.claude/hooks/keep', 'k')
    put(b, cd + '/hooks', 'file')


# --- link：Claude 的 tasks（任务列表整份认） ---

def tasks_case(name, **kw):
    def reg(change):
        def setup(b):
            linked(b, 'claude')
            os.unlink(b.path(cdir('claude') + '/tasks'))
            change(b, '.claude/tasks', cdir('claude') + '/tasks')
        lcase('link/claude/tasks/' + name, ['link', A], **kw)(setup)
        return change
    return reg


@tasks_case('empty-here')
def _(b, shared, mine):
    put(b, shared + '/s1/1.json', '1')
    mkdir(b, mine)


@tasks_case('links-and-type-mismatches-go-aside')
def _(b, shared, mine):
    # 两边有一个是链接、一个是目录一个是文件：账号这边的移进 conflicts（这时 conflicts 里的目录是普通权限建的）
    put(b, shared + '/a-link-here/1.json', 's')
    put(b, 'other/1.json', 'o')
    symlink(b, '~/other', mine + '/a-link-here')
    symlink(b, '~/other', shared + '/b-link-there')
    put(b, mine + '/b-link-there/1.json', 'm')
    put(b, shared + '/c-file-there', 'file')
    put(b, mine + '/c-file-there/1.json', 'm')
    put(b, shared + '/d-dir-there/1.json', 's')
    put(b, mine + '/d-dir-there', 'file')
    symlink(b, '~/other', mine + '/e-only-here-link')


@tasks_case('aside-then-older-list', mtimes=['.cca/conflicts/*/tasks-*'])
def _(b, shared, mine):
    # conflicts 里的目录已经由 aside 建成普通权限的：后面存较旧的列表时不再改它
    put(b, shared + '/a', 'file')
    put(b, mine + '/a/1.json', 'm')
    put(b, shared + '/b/1.json', 'shared newer')
    put(b, mine + '/b/1.json', 'mine older')
    stamp(b, shared + '/b', 200)
    stamp(b, mine + '/b', 100)


@tasks_case('same-content-different-times')
def _(b, shared, mine):
    # 内容一样（空目录不算内容）：丢掉账号这边的，不管谁新
    for root, when in ((shared, 100), (mine, 900)):
        put(b, root + '/s/1.json', 'same')
        put(b, root + '/s/sub/2.json', 'same 2')
        symlink(b, '1.json', root + '/s/alias')
        stamp(b, root + '/s', when)
    mkdir(b, mine + '/s/empty')


@tasks_case('newest-time-counts-directories-and-links', mtimes=['.claude/tasks/*', '.cca/conflicts/*/tasks-*'])
def _(b, shared, mine):
    # 新旧看整棵里最晚的修改时间：目录自己的、链接自己的也算
    for name, newer in (('by-dir', 'sub'), ('by-link', 'alias'), ('by-top', '')):
        for root, text in ((shared, 'shared'), (mine, 'mine')):
            put(b, '%s/%s/1.json' % (root, name), text)
            mkdir(b, '%s/%s/sub' % (root, name))
            symlink(b, '1.json', '%s/%s/alias' % (root, name))
            stamp(b, '%s/%s' % (root, name), 100)
        os.utime(b.path('%s/%s/%s' % (mine, name, newer)), (200, 200), follow_symlinks=False)


@tasks_case('equal-times-keep-shared', mtimes=['.claude/tasks/*', '.cca/conflicts/*/tasks-*'])
def _(b, shared, mine):
    # 一样新：留共享的，账号这边的移走
    for root, text in ((shared, 'shared'), (mine, 'mine')):
        put(b, root + '/t/1.json', text)
        stamp(b, root + '/t', 100)


@tasks_case('files-directly-in-tasks', mtimes=['.claude/tasks/*.txt'])
def _(b, shared, mine):
    put(b, shared + '/newer.txt', 'shared', mtime=100)
    put(b, mine + '/newer.txt', 'mine', mtime=200)
    put(b, shared + '/older.txt', 'shared', mtime=200)
    put(b, mine + '/older.txt', 'mine', mtime=100)
    put(b, shared + '/log.jsonl', 'a\n', mtime=100)
    put(b, mine + '/log.jsonl', 'b\n', mtime=50)
    put(b, mine + '/only.txt', 'mine', mtime=100)


@lcase('link/agy/tasks-is-nothing-special', ['link', A], tool='agy')
def _(b):
    # 只有 Claude 的 tasks 整份认；agy 的名单里根本没有这个名字
    linked(b, 'agy')
    put(b, '.gemini/tasks/s/1.json', 's')
    put(b, cdir('agy') + '/tasks/s/1.json', 'm')


# --- link：共享配置的三方合并（merge_shared_config） ---

def config_case(name, tool, fname, initial, *changes, keys=(A,), **kw):
    """共享配置 fname 的一条路：主目录里先是 initial（None：没有），keys 这几个账号链接好，再照 changes 改，然后 link A"""
    prefix, wait = group(tool)
    before = [files(MASTER[tool], {fname: initial}, mtime=300)] if initial is not None else []
    chain('%s/config/%s' % (prefix, name), [shared_world] + before + [both(tool, keys)] + list(changes) + [step('link', A, **kw)], tool=tool, **wait)


def mine(tool, fname, text, mode=0o644, key=A, **kw):
    """账号这边原子保存了一份（链接就此成了普通文件）"""
    return lambda b: atomic(b, '%s/%s' % (cdir(tool, key), fname), text, mode, **kw)


def theirs(tool, fname, text, mode=0o644, **kw):
    """共享主目录里的那份变了（别的账号改的）；text 是 None：没了"""
    def f(b):
        if text is None:
            os.unlink(b.path('%s/%s' % (MASTER[tool], fname)))
        else:
            put(b, '%s/%s' % (MASTER[tool], fname), text, mode, **kw)
    return f


for T, JSON_FILE, TEXT_FILE in (('claude', 'settings.json', 'CLAUDE.md'), ('codex', 'hooks.json', 'config.toml'), ('agy', 'antigravity-cli/settings.json', 'config/GEMINI.md')):
    M = [MASTER[T] + '/' + JSON_FILE, MASTER[T] + '/' + TEXT_FILE]
    for F, V0, V1, V2 in ((JSON_FILE, '{"a":1,"b":1}', '{"a":2,"b":1}', '{"a":1,"b":3}'), (TEXT_FILE, 'a = 1\nb = 1\n', 'a = 2\nb = 1\n', 'a = 1\nb = 3\n')):
        K = 'json' if F == JSON_FILE else 'text'
        # 账号这份和共享的一样：只是把链接接回去
        config_case('%s-same-as-shared' % K, T, F, V0, mine(T, F, V0, mtime=500), mtimes=M)
        # 账号这份没变（还是基线那样），共享的变了：照共享的
        config_case('%s-only-shared-changed' % K, T, F, V0, mine(T, F, V0, mtime=500), theirs(T, F, V2, mtime=400), mtimes=M)
        # 只有账号这份变了：整份换上（修改时间、权限跟着账号这份）
        config_case('%s-only-mine-changed' % K, T, F, V0, mine(T, F, V1, 0o600, mtime=500), mtimes=M)
        # 主目录里还没有、基线里记的也是没有：装上去
        config_case('%s-new-here' % K, T, F, None, mine(T, F, V1, mtime=500), mtimes=M)
        # 共享的那份被删了，账号这份也改过：冲突（留副本），不装回去
        config_case('%s-changed-here-deleted-there' % K, T, F, V0, mine(T, F, V1), theirs(T, F, None))
        # 共享的那份被删了，账号这份没变：跟着没了
        config_case('%s-deleted-there' % K, T, F, V0, mine(T, F, V0), theirs(T, F, None))

        NAME, WAIT = group(T)

        @lcase('%s/config/%s-first-adoption-installs' % (NAME, K), ['link', A], tool=T, mtimes=M, **WAIT)
        def _(b, T=T, F=F, V1=V1):
            # 第一次链接、主目录里还没有这个文件（基线里也没有这一项）：账号这份装上去
            shared_world(b)
            put(b, '%s/%s' % (cdir(T), F), V1, 0o600, mtime=500)

        @lcase('%s/config/%s-first-adoption-same' % (NAME, K), ['link', A], tool=T, mtimes=M, **WAIT)
        def _(b, T=T, F=F, V1=V1):
            shared_world(b)
            put(b, '%s/%s' % (MASTER[T], F), V1, mtime=100)
            put(b, '%s/%s' % (cdir(T), F), V1, mtime=500)

        @lcase('%s/config/%s-through-master-symlink' % (NAME, K), ['link', A], tool=T, **WAIT)
        def _(b, T=T, F=F, V0=V0, V1=V1):
            # 主目录里的共享配置是个链接（dotfiles）：换的是它指向的文件，链接留着
            shared_world(b)
            put(b, 'dotfiles/config-file', V0, 0o640)
            symlink(b, '~/dotfiles/config-file', '%s/%s' % (MASTER[T], F))
            both(T, (A,))(b)
            atomic(b, '%s/%s' % (cdir(T), F), V1)

        @lcase('%s/config/%s-both-changed-through-master-symlink' % (NAME, K), ['link', A], tool=T, **WAIT)
        def _(b, T=T, F=F, V0=V0, V1=V1, V2=V2):
            # 同上，两边都改了：JSON 合并的结果也是写进链接指向的文件（权限照旧）；文本留共享的
            shared_world(b)
            put(b, 'dotfiles/config-file', V0, 0o640)
            symlink(b, '~/dotfiles/config-file', '%s/%s' % (MASTER[T], F))
            both(T, (A,))(b)
            atomic(b, '%s/%s' % (cdir(T), F), V1)
            put(b, 'dotfiles/config-file', V2, 0o640)

    # 文本（TOML、Markdown）两边都改了：不逐行拼，留共享的，账号这份存进 conflicts
    config_case('text-both-changed', T, TEXT_FILE, 'a = 1\nb = 1\n', mine(T, TEXT_FILE, 'a = 2\nb = 1\n', 0o640, mtime=500), theirs(T, TEXT_FILE, 'a = 1\nb = 3\n', mtime=400),
                mtimes=M + [ROOT[T].replace('accounts', 'conflicts') + '/*/*'])
    # JSON 两边改的是不同的字段：按字段合并，重写共享的那份（权限照旧）
    config_case('json-fields-merge', T, JSON_FILE, '{"a":1,"b":1,"gone":true,"deep":{"x":[1],"y":"y"}}',
                mine(T, JSON_FILE, '{"b":1,"a":2,"deep":{"x":[1,2],"y":"y"},"mine":"新"}'),
                theirs(T, JSON_FILE, '{"a":1,"b":3,"gone":true,"deep":{"x":[1],"y":"z"},"theirs":null}', 0o640))
    # 同一个字段两边改得不一样：留共享的值、报冲突、存副本；别的字段照常合并
    config_case('json-field-conflict', T, JSON_FILE, '{"a":1,"b":1,"deep":{"x":1}}', mine(T, JSON_FILE, '{"a":2,"b":2,"deep":{"x":2,"m":1}}'),
                theirs(T, JSON_FILE, '{"a":3,"b":1,"deep":{"x":3}}'))
    # 合出来和共享的一样（账号这边的改动共享的也有了）：共享的那份原样不动
    config_case('json-merge-equals-shared', T, JSON_FILE, '{"a":1,"b":1}', mine(T, JSON_FILE, '{"a":2,"b":1}'), theirs(T, JSON_FILE, '{ "b": 3, "a": 2 }', mtime=400), mtimes=M)
    # 账号这份只是换了写法（内容和基线一样），共享的变了：照共享的，不算冲突
    config_case('json-reformatted-here', T, JSON_FILE, '{"a":1,"b":1}', mine(T, JSON_FILE, '{\n  "b": 1.0,\n  "a": 1\n}\n'), theirs(T, JSON_FILE, '{"a":1,"b":3}', mtime=400), mtimes=M)
    # 账号这份只是换了写法，共享的那份被删了：跟着没了
    config_case('json-reformatted-here-deleted-there', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, '{ "a": 1 }'), theirs(T, JSON_FILE, None))
    # 不是对象的 JSON：整个值当一项比
    config_case('json-arrays', T, JSON_FILE, '[1]', mine(T, JSON_FILE, '[1, 2]'), theirs(T, JSON_FILE, '[1, 3]'))
    config_case('json-object-vs-array', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, '{"a":2}'), theirs(T, JSON_FILE, '[]'))
    # 账号这份是坏的 JSON、空白、带 BOM
    config_case('json-broken-here', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, '{"a":'))
    config_case('json-blank-here', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, ' \n'), mtimes=M)
    config_case('json-blank-here-shared-changed', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, ''), theirs(T, JSON_FILE, '{"a":2}', mtime=400), mtimes=M)
    config_case('json-bom-here', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, '﻿{"a":2}'), theirs(T, JSON_FILE, '{"a":3}'))
    # 共享的那份、基线是坏的 JSON：合不了，留副本，共享的原样
    config_case('json-broken-there', T, JSON_FILE, '{"a":1}', mine(T, JSON_FILE, '{"a":2}'), theirs(T, JSON_FILE, '{"a":', mtime=400), mtimes=M)
    config_case('json-broken-base', T, JSON_FILE, '{"a":', mine(T, JSON_FILE, '{"a":2}'), theirs(T, JSON_FILE, '{"a":3}', mtime=400), mtimes=M)
    # 上次链接时主目录里还没有（基线是 null），这回两边各建了一份：没有共同的起点，按字段并，撞了的留共享的
    config_case('json-both-created', T, JSON_FILE, None, mine(T, JSON_FILE, '{"a":1,"mine":1}'), theirs(T, JSON_FILE, '{"a":2,"theirs":2}'))
    config_case('json-both-created-compatible', T, JSON_FILE, None, mine(T, JSON_FILE, '{"mine":1}'), theirs(T, JSON_FILE, '{"theirs":2}', 0o600))
    config_case('text-both-created', T, TEXT_FILE, None, mine(T, TEXT_FILE, 'who = "mine"\n'), theirs(T, TEXT_FILE, 'who = "theirs"\n'))
    # 换行不一样不算改动：\r\n 读进来就是 \n
    config_case('text-crlf-is-not-a-change', T, TEXT_FILE, 'a = 1\nb = 1\n', mine(T, TEXT_FILE, 'a = 1\r\nb = 1\r\n'), theirs(T, TEXT_FILE, 'a = 1\nb = 3\n', mtime=400), mtimes=M)

@lcase('link/claude/config/unreadable-account-copy', ['link', A], chmod={cdir('claude') + '/settings.json': 0o000})
def _(b):
    # 读不了账号这份：原实现是没人接的 PermissionError
    linked(b, 'claude')
    put(b, '.claude/settings.json', '{"a":1}')
    put(b, unlinked(b, 'claude', 'settings.json'), '{"a":2}')


if hasattr(os, 'chflags'):
    @lcase('link/claude/config/copy-keeps-file-flags', ['link', A], mtimes=['.claude/settings.json', '.claude/memory'])
    def _(b):
        # macOS 的文件标志（这里是“隐藏”）：换上去的那份、存进 conflicts 的那份都带着（shutil.copy2 连标志一起拷）
        linked(b, 'claude')
        put(b, '.claude/settings.json', '{"a":1}', mtime=300)
        put(b, '.claude/CLAUDE.md', '# shared\n')
        put(b, '.claude/memory', 'shared old', mtime=100)
        both('claude', (A,), seed=False)(b)
        for name, text in (('settings.json', '{"a":2}'), ('CLAUDE.md', '# mine\n'), ('memory', 'mine new')):
            p = b.path(put(b, unlinked(b, 'claude', name), text, 0o600, mtime=500))
            os.chflags(p, stat.UF_HIDDEN)
        put(b, '.claude/CLAUDE.md', '# shared changed\n')

    @lcase('link/claude/legacy/copies-keep-flags', ['link', A], mtimes=['.cca/accounts/*/claude/future*'])
    def _(b):
        linked(b, 'claude')
        put(b, '.claude/future.json', 'state', mtime=100)
        put(b, '.claude/future-dir/sub/lease', 'lease', mtime=100)
        stamp(b, '.claude/future-dir', 150)
        for rel in ('future.json', 'future-dir', 'future-dir/sub', 'future-dir/sub/lease'):
            os.chflags(b.path('.claude/' + rel), stat.UF_HIDDEN)
        symlink(b, '~/.claude/future.json', cdir('claude') + '/future.json')
        symlink(b, '~/.claude/future-dir', cdir('claude') + '/future-dir')

config_case('profile-is-shared', 'codex', 'work.config.toml', 'model = "a"\n', mine('codex', 'work.config.toml', 'model = "b"\n', mtime=500), mtimes=['.codex/work.config.toml'])
config_case('profile-new-here', 'codex', 'deep.config.toml', None, mine('codex', 'deep.config.toml', 'model = "b"\n'))
config_case('profile-conflict-keeps-base-entry', 'codex', 'work.config.toml', 'model = "a"\n', mine('codex', 'work.config.toml', 'model = "b"\n'),
            theirs('codex', 'work.config.toml', 'model = "c"\n'))
config_case('profile-removed-everywhere', 'codex', 'work.config.toml', 'model = "a"\n', theirs('codex', 'work.config.toml', None))
config_case('env-file', 'codex', '.env', 'A=1\n', mine('codex', '.env', 'A=2\n', 0o600))


# --- link：旧版留下的链接（只有 Claude 拆）、agy 的子目录各层 ---

def legacy_case(name, **kw):
    def reg(change):
        def setup(b):
            linked(b, 'claude')
            change(b, cdir('claude'))
        lcase('link/claude/legacy/' + name, ['link', A], **kw)(setup)
        return change
    return reg


@legacy_case('absolute-and-relative-links', mtimes=['.cca/accounts/*/claude/future*'])
def _(b, cd):
    put(b, '.claude/future.json', 'state', 0o640, mtime=100)
    put(b, '.claude/future-dir/a/b/lease', 'lease', 0o600, mtime=100)
    mkdir(b, '.claude/future-dir/empty', 0o750)
    stamp(b, '.claude/future-dir', 150)
    symlink(b, '~/.claude/future.json', cd + '/future.json')
    symlink(b, '../../../../.claude/future-dir/', cd + '/future-dir')


@legacy_case('links-to-other-places-stay')
def _(b, cd):
    put(b, '.claude/other.json', 'x')
    put(b, 'mine/state.json', 'x')
    symlink(b, '~/.claude/other.json', cd + '/renamed.json')
    symlink(b, '~/mine/state.json', cd + '/state.json')
    symlink(b, '~/mine', cd + '/mine-dir')
    symlink(b, '~/.claude', cd + '/whole-master')
    symlink(b, 'renamed.json', cd + '/chain.json')


@legacy_case('broken-legacy-links-are-removed')
def _(b, cd):
    symlink(b, '~/.claude/gone.json', cd + '/gone.json')
    symlink(b, '../../../../.claude/gone-dir', cd + '/gone-dir')


@legacy_case('nested-link-stops-everything-after-it')
def _(b, cd):
    # 按名字的先后：出错的那一项之前的照常处理，之后的没动（共享条目的链接也还没补）
    put(b, '.claude/a-first.json', 'a')
    symlink(b, '~/.claude/a-first.json', cd + '/a-first.json')
    put(b, '.claude/b-nested/deep/f', 'f')
    symlink(b, '/nonexistent', '.claude/b-nested/deep/inner')
    symlink(b, '~/.claude/b-nested', cd + '/b-nested')
    put(b, '.claude/c-after.json', 'c')
    symlink(b, '~/.claude/c-after.json', cd + '/c-after.json')
    put(b, '.claude/rules/r.md', 'r')


@legacy_case('nested-link-to-a-file-also-stops')
def _(b, cd):
    put(b, '.claude/b-nested/f', 'f')
    symlink(b, 'f', '.claude/b-nested/alias')
    symlink(b, '~/.claude/b-nested', cd + '/b-nested')


@legacy_case('readonly-directory-cannot-be-detached', chmod={'.claude/future-dir': 0o555})
def _(b, cd):
    # 指向只读的非空目录（macOS）：拷好的那份权限也是只读，搬不出临时目录——原实现出错退出（见 FINDINGS），
    # 链接恢复原样，临时目录收掉
    put(b, '.claude/future-dir/lease', 'lease')
    symlink(b, '~/.claude/future-dir', cd + '/future-dir')


@legacy_case('readonly-subdirectory-is-copied', chmod={'.claude/future-dir/sub': 0o555}, mtimes=['.cca/accounts/*/claude/future-dir*'])
def _(b, cd):
    # 里面的子目录只读不碍事：照样拷出一份（权限、修改时间跟着）
    put(b, '.claude/future-dir/sub/lease', 'lease', 0o444, mtime=100)
    stamp(b, '.claude/future-dir', 150)
    symlink(b, '~/.claude/future-dir', cd + '/future-dir')


@lcase('link/agy/legacy-links-are-left-alone', ['link', A], tool='agy')
def _(b):
    # 别的工具不拆：名单以外的链接原样留着
    linked(b, 'agy')
    put(b, '.gemini/future.json', 'state')
    symlink(b, '~/.gemini/future.json', cdir('agy') + '/future.json')
    symlink(b, '~/.gemini/antigravity-cli/log', cdir('agy') + '/antigravity-cli/log')


def agy_case(name, **kw):
    def reg(change):
        def setup(b):
            shared_world(b)
            record(b, 'agy', IDS['agy'][0])
            change(b, cdir('agy'))
        lcase('link/agy/levels/' + name, ['link', A], tool='agy', **kw)(setup)
        return change
    return reg


@agy_case('nested-entries-are-adopted')
def _(b, cd):
    # 子目录里的条目：名单上的并回去、链接上；别的（日志、别的缓存、登录 token）留在账号目录
    client(b, 'agy', IDS['agy'][0])
    put(b, cd + '/antigravity-cli/conversations/c1.pb', 'conv')
    put(b, cd + '/antigravity-cli/history.jsonl', '{"n":1}\n')
    put(b, cd + '/antigravity-cli/cache/last_conversations.json', '{"/w":"c1"}')
    put(b, cd + '/antigravity-cli/cache/other.bin', 'private')
    put(b, cd + '/antigravity-cli/log/agy.log', 'private')
    put(b, cd + '/antigravity-cli/antigravity-oauth-token', 'private', 0o600)
    put(b, cd + '/config/skills/s/SKILL.md', '# s\n')
    put(b, cd + '/config/mcp_config.json', '{"mcpServers":{}}')
    put(b, cd + '/config/unknown.json', 'private')
    put(b, cd + '/GEMINI.md', '# mine\n')
    put(b, cd + '/conversations/top-level-is-not-shared', 'private')


@agy_case('master-has-no-nest-dirs')
def _(b, cd):
    # ~/.gemini 里还没有 config/、antigravity-cli/cache/：要并回去的时候才建
    client(b, 'agy', IDS['agy'][0])
    put(b, cd + '/antigravity-cli/cache/last_conversations.json', '{}')


@agy_case('nest-is-a-file-here')
def _(b, cd):
    # 账号目录里那一层是个文件：这一层（和它下面的 cache）不动
    put(b, '.gemini/config/mcp_config.json', '{}')
    put(b, '.gemini/antigravity-cli/cache/last_conversations.json', '{}')
    put(b, cd + '/config', 'a file')


@agy_case('nest-is-a-symlink-here')
def _(b, cd):
    # 那一层是个链接：这一层不动——但原实现下一层（antigravity-cli/cache）照样经它建目录、放链接，设备标识也经它写（见 FINDINGS）
    put(b, '.gemini/antigravity-cli/settings.json', '{}')
    put(b, '.gemini/antigravity-cli/cache/last_conversations.json', '{}')
    mkdir(b, 'elsewhere')
    symlink(b, '~/elsewhere', cd + '/antigravity-cli')


@agy_case('nest-is-a-dangling-symlink-here')
def _(b, cd):
    put(b, '.gemini/config/mcp_config.json', '{}')
    client(b, 'agy', IDS['agy'][0])
    symlink(b, '~/nowhere', cd + '/config')


@agy_case('cache-level-is-a-symlink-here')
def _(b, cd):
    client(b, 'agy', IDS['agy'][0])
    put(b, '.gemini/antigravity-cli/cache/last_conversations.json', '{}')
    mkdir(b, 'elsewhere')
    symlink(b, '~/elsewhere', cd + '/antigravity-cli/cache')


@agy_case('master-nest-is-a-file')
def _(b, cd):
    # ~/.gemini/config 是个文件：预先要有的 config/projects 建不出来，一开始就出错退出（没人接的异常）
    client(b, 'agy', IDS['agy'][0])
    put(b, '.gemini/config', 'a file')


@agy_case('same-name-in-two-levels')
def _(b, cd):
    # 顶层和 config/ 里各有一个 GEMINI.md、AGENTS.md：各管各的，基线里的名字带着子目录
    client(b, 'agy', IDS['agy'][0])
    put(b, '.gemini/GEMINI.md', '# top shared\n')
    put(b, '.gemini/config/GEMINI.md', '# config shared\n')
    put(b, cd + '/GEMINI.md', '# top mine\n')
    put(b, cd + '/config/AGENTS.md', '# config mine\n')


# --- cost_kick 真的起后台统计的几条路（cost 模块里这一个函数已经移植）。SELF_PATH 是沙箱里假的 cca：把收到的参数记进
# calls.log；settle 等它写完。cost/last 的修改时间每次不一样，只比它在不在（lcase 默认不比修改时间）
COST_AGENT = '#!/bin/sh\nprintf \'%s EVENTS=[%s] TOOL=[%s] KEEP=[%s]\\n\' "cca $*" "$CCA_EVENTS" "$CCA_TOOL" "$KEEP_ME" >> "$HOME/calls.log"\n'
COST_ENV = {'CCA_EVENTS': '/dev/null', 'KEEP_ME': 'yes'}


def costing(b, tool='claude', cfg=None, last=None):
    linked(b, tool)
    b.cfg(dict({'version': 2}, **(cfg or {})))
    put(b, '.cca/bin/cca', COST_AGENT, 0o755)
    if last is not None:
        p = put(b, '.cca/cost/last', '', 0o644)
        os.chmod(os.path.dirname(p), 0o700)
        t = time.time() - last
        os.utime(p, (t, t))


@lcase('link-cost/claude/kicks', ['link', A, 'cost'], env=COST_ENV, settle=0.6)
def _(b):
    costing(b)


@lcase('link-cost/claude/kicks-when-stamp-is-old', ['link', A, 'cost'], env=COST_ENV, settle=0.6)
def _(b):
    costing(b, last=3600)


@lcase('link-cost/claude/too-soon', ['link', A, 'cost'], env=COST_ENV, settle=0.4)
def _(b):
    costing(b, last=30)


@lcase('link-cost/claude/stamp-in-the-future-kicks', ['link', A, 'cost'], env=COST_ENV, settle=0.6)
def _(b):
    costing(b, last=-3600)


@lcase('link-cost/claude/cost-dir-is-a-file', ['link', A, 'cost'], env=COST_ENV, settle=0.4)
def _(b):
    costing(b)
    put(b, '.cca/cost', 'not a directory', 0o600)


@lcase('link-cost/claude/config-not-json-still-kicks', ['link', A, 'cost'], env=COST_ENV, settle=0.6)
def _(b):
    costing(b)
    put(b, '.cca/config.json', '{oops', 0o600)


@lcase('link/claude/no-cost-argument-no-kick', ['link', A], env=COST_ENV, settle=0.4)
def _(b):
    costing(b)
