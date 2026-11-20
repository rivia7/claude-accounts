"""设备统计和共享 API key（cli/src/cmd/presence.rs、keyshare.rs）：key-sync、presence-agent。

服务器是假的：沙箱 bin/ 里的 curl 把每次请求的参数和 stdin（签过名的 JSON）记进 ~/curl.log，照 ~/server/ 里准备好的回——
~/server/<op>.<第几次> 没有就用 ~/server/<op>，第一行是「curl 的退出码 HTTP 状态码」，后面是响应体；都没有就是连不上（退出码 7）。
ssh-keygen 也是假的（签名是固定的一段，参数记进 ~/keygen.log；沙箱里有 sign_fails 就签不出来）。钥匙串用 cases/secrets.py 的
假 security。请求里的时间、随机数、密文每次不一样，比之前换掉（MASK）；密文的算法由 keyshare.rs 的单元测试拿 CPython 算的
期望值核对。服务器回的密文是夹具里拿 PY_SRC 自己的 keyshare_seal 现算的（src()）。

上报进程（presence-agent）要看进程：ps 也是假的，照 ~/ps.out 回（没有这个文件就是 ps 读不了）；问单个进程启动时间的（-p）
一律回 T0。会话进程有两种：进程号 999999（不可能有这个进程：kqueue 盯不上，上报进程每秒回来看一次）和 before 里真的起一个
sleep（kqueue 盯着它退出）。上报进程要到没有会话了、或者被 SIGTERM 停掉才退出：during 里等它报了该报的，再改 ps.out / 发信号。"""
import json
import os
import re
import signal
import subprocess
import time
from unittest import mock

import run
from run import case
from cases.secrets import ALICE, BOB, PRESENCE, R1, R2, REF, SECURITY, SHARING, kc, sharing, switch
from cases.session import T0, T0_EPOCH   # 假的 ps 给的启动时间和它的时间戳（Asia/Shanghai）

SALT = PRESENCE['salt']
UUID = '0b0c0d0e-aaaa-4bbb-8ccc-0123456789ab'
MASK = [
    (r'"ts":\d+', '"ts":<TS>'), (r'"nonce":"[0-9a-f]{32}"', '"nonce":"<N>"'),
    (r'"n":"[A-Za-z0-9+/=]+","c":"[A-Za-z0-9+/=]+","t":"[A-Za-z0-9+/=]+"', '<BLOB>'),
    (r'"(synced|retry)": \d+\.\d+', r'"\1": <T>'), (r'"cli":\[\[\d+,\d+\]\]', '"cli":[[<OPEN>,<IDLE>]]'),
    (r'"pid": \d+', '"pid": <PID>'),
]
AGENT_MASK = MASK + [(r'"at": 17\d{8}\.\d+', '"at": <NOW>')]   # status.json 里没报成的时间

KEYGEN = r'''printf 'KEYGEN %s\n' "$*" >> "$HOME/keygen.log"
cat > /dev/null
[ -f "$HOME/sign_fails" ] && exit 1
printf -- '-----BEGIN SSH SIGNATURE-----\nRkFLRQ==\n-----END SSH SIGNATURE-----\n'
'''
CURL = r'''body=$(cat)
url=""; for a in "$@"; do url="$a"; done
op="${url##*/}"
{ printf 'CURL'; for a in "$@"; do printf ' [%s]' "$a"; done; printf '\nBODY %s\n' "$body"; } >> "$HOME/curl.log"
[ -f "$HOME/slow_$op" ] && sleep 3
k=$(cat "$HOME/curl.$op.n" 2>/dev/null || echo 0); k=$((k + 1)); echo "$k" > "$HOME/curl.$op.n"
f="$HOME/server/$op.$k"; [ -f "$f" ] || f="$HOME/server/$op"
[ -f "$f" ] || exit 7
read -r rc code < "$f"
[ "$rc" = 0 ] || exit "$rc"
tail -n +2 "$f"
printf '\n%s' "$code"
'''
PS = r'''case " $* " in
	*" -p "*) for a in "$@"; do pid="$a"; done; printf '%s %s\n' "$pid" "''' + T0 + r'''" ;;
	*) cat "$HOME/ps.out" 2>/dev/null ;;
esac
'''

_NS = {}


def src():
    """PY_SRC 里的定义（拿它的 keyshare_seal、presence_aid 给假服务器准备密文）"""
    if not _NS:
        defs = run.py_src().split('\nif len(sys.argv) < 2', 1)[0]
        with mock.patch.dict(os.environ, {'HOME': '/nonexistent', 'CCA_HOME': '/nonexistent/.cca', 'CCA_TOOL': 'claude'}, clear=True):
            exec(compile(defs, 'PY_SRC', 'exec'), _NS)
    return _NS


def aid_of(email, tool='CLAUDE'):
    return src()['presence_aid'](SALT, src()[tool], email)


def blob(email, key, at, ident=UUID, tool='CLAUDE'):
    """服务器上存的这个账号的那一份：key（None 是删掉了）和时间"""
    ns = src()
    return ns['keyshare_seal'](ns['keyshare_keys'](SALT, ns[tool], email, ident), aid_of(email, tool), key, at)


def reply(b, op, body, code=200, rc=0, nth=None):
    b.write('server/%s%s' % (op, '.%d' % nth if nth else ''), '%d %d\n%s\n' % (rc, code, body if isinstance(body, str) else json.dumps(body)))


def login(b, aid=ALICE, uuid=UUID, raw=None):
    """账号目录里的 .claude.json：登录过（有 accountUuid）"""
    b.write('.cca/accounts/%s/claude/.claude.json' % aid, raw if raw is not None else json.dumps({'oauthAccount': {'accountUuid': uuid, 'emailAddress': 'x'}}))


def pcase(name, argv, **kw):
    """登记一个用例：先放好假的 security、ssh-keygen、curl、ps"""
    kw.setdefault('mask', AGENT_MASK if name.startswith('agent/') else MASK)
    kw.setdefault('settle', 0.4)   # 等后台起的 cca _cost-sync（假的，只记一笔）写完

    def reg(setup):
        def with_fakes(b):
            for n, s in (('security', SECURITY), ('ssh-keygen', KEYGEN), ('curl', CURL), ('ps', PS)):
                b.fake(n, s)
            setup(b)
        return case('presence/' + name, argv, **kw)(with_fakes)
    return reg


def keys_json(**acc):
    return {'v': 1, 'acc': {'claude:%s' % k: v for k, v in acc.items()}}


def booked(b, at=None, frm=None, ids=(ALICE,), **cfg):
    """config.json 里记着这些账号有记下备查的 key（key_hints 里可以带记下的时间）"""
    hint = {'host': 'reference', 'hint': 'sk-ant-…WXYZ'}
    if at is not None:
        hint['at'] = at
    if frm:
        hint['from'] = frm
    return dict({'ref_keys': sorted(ids), 'key_hints': {i: dict(hint) for i in ids}}, **cfg)


# --- key-sync ---------------------------------------------------------------------------------------------------------
@pcase('key-sync/no-presence', ['key-sync', 'force'])
def _(b):
    sharing(b, policy=dict(SHARING, presence=None))
    login(b)


@pcase('key-sync/presence-off', ['key-sync', 'force'])
def _(b):
    sharing(b, cfg={'presence': False})
    login(b)


@pcase('key-sync/no-deploy-key', ['key-sync', 'force'])
def _(b):
    sharing(b, key=False)
    login(b)


@pcase('key-sync/too-many-args', ['key-sync', 'force', 'x'])
def _(b):
    sharing(b)


for name, argv, keys in [
    ('not-yet', ['key-sync'], dict(keys_json(), synced=time.time() - 30)),               # 上次同步才过去 30 秒：不同步，但 keys.json 照样写回去
    ('not-yet-retry', ['key-sync'], dict(keys_json(), synced=1.0, retry=time.time() + 600)),   # 旧的服务器：还没到再试的时候
    ('due', ['key-sync'], dict(keys_json(), synced=time.time() - 700)),
    ('first-time', ['key-sync'], None),
    ('forced-although-recent', ['key-sync', 'force'], dict(keys_json(), synced=time.time() - 30, retry=time.time() + 600, error='旧的')),
    ('now', ['key-sync', 'now'], dict(keys_json(), synced=time.time() - 30)),
    ('odd-mode', ['key-sync', 'whatever'], dict(keys_json(), synced=time.time() - 30)),
    ('bad-keys-file', ['key-sync', 'force'], '[1, 2'),
    ('keys-wrong-version', ['key-sync', 'force'], {'v': 2, 'acc': {'x': 1}}),
    ('keys-acc-not-dict', ['key-sync', 'force'], {'v': 1, 'acc': []}),
    ('synced-not-a-number', ['key-sync'], dict(keys_json(), synced='yesterday', retry=True)),
]:
    @pcase('key-sync/when-' + name, argv)
    def _(b, keys=keys):
        sharing(b, keys=keys, cfg=booked(b, at=1794000000.5))
        login(b)
        kc(b, REF + ALICE, 'reference', R1)
        reply(b, 'keys', {'keys': {}})


@pcase('key-sync/nobody-logged-in', ['key-sync', 'force'])   # 没有 accountUuid：这台设备没登录，不问服务器
def _(b):
    sharing(b, cfg=booked(b, at=1794000000.5))
    kc(b, REF + ALICE, 'reference', R1)
    reply(b, 'keys', {'keys': {}})


for name, raw in [('not-json', '{oops'), ('list', '[]'), ('no-oauth', '{}'), ('oauth-not-dict', '{"oauthAccount": 3}'),
                  ('uuid-short', '{"oauthAccount": {"accountUuid": "1234567"}}'), ('uuid-8', '{"oauthAccount": {"accountUuid": "12345678"}}'),
                  ('uuid-number', '{"oauthAccount": {"accountUuid": 1234567890}}'), ('uuid-wide', json.dumps({'oauthAccount': {'accountUuid': '账号账号账号账号'}}))]:
    @pcase('key-sync/ident-' + name, ['key-sync', 'force'])
    def _(b, raw=raw):
        sharing(b, cfg=booked(b, at=1794000000.5))
        login(b, raw=raw)
        kc(b, REF + ALICE, 'reference', R1)
        reply(b, 'keys', {'keys': {}})
        reply(b, 'keys', {}, nth=2)


@pcase('key-sync/table-unreadable', ['key-sync', 'force'])
def _(b):
    sharing(b)
    login(b)
    b.write('.cca/accounts.json', '{oops')


@pcase('key-sync/table-is-a-list', ['key-sync', 'force'])
def _(b):
    sharing(b, table=[])
    login(b)


@pcase('key-sync/config-unreadable', ['key-sync', 'force'])   # die：说了原因，退出码还是 0
def _(b):
    sharing(b)
    login(b)
    b.write('.cca/config.json', '{oops')


# 服务器没成、回的看不懂
for name, kw in [
    ('unreachable', None), ('404-old-server', dict(body={'error': 'no such op'}, code=404)), ('403', dict(body={}, code=403)),
    ('429', dict(body={}, code=429)), ('500', dict(body='<html>oops</html>', code=500)), ('pin-mismatch', dict(body='', rc=90)),
    ('curl-fails', dict(body='', rc=28)), ('clock', dict(body={'error': 'clock', 'now': 1794000000}, code=400)),
    ('clock-float', dict(body={'error': 'clock', 'now': 1794000000.75}, code=400)), ('clock-not-number', dict(body={'error': 'clock', 'now': 'x'}, code=400)),
    ('200-not-json', dict(body='hello')), ('200-list', dict(body=[])), ('200-keys-not-dict', dict(body={'keys': []})), ('code-000', dict(body='{}', code=0)),
    ('200-empty-body', dict(body='')),
]:
    @pcase('key-sync/server-' + name, ['key-sync', 'force'], mask=MASK + [(r'差了 \d+ 秒', '差了 <N> 秒')])
    def _(b, kw=kw):
        sharing(b, cfg=booked(b, at=1794000000.5))
        login(b)
        kc(b, REF + ALICE, 'reference', R1)
        if kw:
            reply(b, 'keys', **kw)


@pcase('key-sync/sign-fails', ['key-sync', 'force'])
def _(b):
    sharing(b, cfg=booked(b, at=1794000000.5))
    login(b)
    kc(b, REF + ALICE, 'reference', R1)
    switch(b, 'sign_fails')


# 这台设备上的新（或者服务器上没有、认不出）：传上去
for name, kw in [
    ('server-empty', dict(remote=None)),
    ('server-older', dict(remote=(R2, 1793000000.0))),
    ('server-same-time', dict(remote=(R2, 1794000000.5))),          # 一样新：不动
    ('server-forged', dict(remote='forged')),                       # 认证不过：当没有
    ('server-other-account', dict(remote='other')),                 # 别的账号的密文挪过来：认证不过
    ('server-blob-not-dict', dict(remote='junk')),
    ('known-sum', dict(remote=None, keys=keys_json(**{ALICE: {'at': 1793500000.25, 'sum': 'abcdef0123456789'}}), hint_at=None)),
    ('known-sum-hint-newer', dict(remote=None, keys=keys_json(**{ALICE: {'at': 1793500000.25, 'sum': 'abcdef0123456789'}}))),
    ('keys-entry-without-sum', dict(remote=None, keys=keys_json(**{ALICE: {'at': 1793500000.25, 'sum': None}}), hint_at=None)),
    ('keys-entry-at-not-number', dict(remote=None, keys=keys_json(**{ALICE: {'at': 'x', 'sum': 'abcd'}}), hint_at=None)),
    ('keys-entry-not-dict', dict(remote=None, keys=keys_json(**{ALICE: 5}), hint_at=None)),
    ('no-hint-time', dict(remote=None, hint_at=None)),              # key_hints 里没有时间：看钥匙串这一条的修改时间（假的没有：当作很早以前）
    ('hint-time-zero', dict(remote=None, hint_at=0)),
    ('hint-time-bool', dict(remote=None, hint_at=True)),
    ('deleted-here', dict(remote=(R2, 1793000000.0), keys=keys_json(**{ALICE: {'at': 1793900000.0, 'sum': None}}), book=False)),   # 这台设备上删掉了、比服务器的新：把删除传上去
    ('keychain-lost', dict(remote=None, keychain=False)),           # config.json 记着有，钥匙串里没有了：去掉记录，下一轮取服务器上的
    ('keychain-unreadable', dict(remote=None, unreadable=True)),
    ('put-fails', dict(remote=None, put=dict(body={}, code=500))),
    ('put-unreachable', dict(remote=None, put=None)),
]:
    @pcase('key-sync/push-' + name, ['key-sync', 'force'])
    def _(b, kw=kw):
        hint_at = kw.get('hint_at', 1794000000.5)
        cfg = booked(b, at=hint_at) if kw.get('book', True) else {}
        sharing(b, cfg=cfg, keys=kw.get('keys'))
        login(b)
        if kw.get('keychain', True) and kw.get('book', True):
            kc(b, REF + ALICE, 'reference', R1)
        if kw.get('unreadable'):
            switch(b, 'keychain_unreadable')
        aid, remote = aid_of('alice@example.com'), kw['remote']
        rec = {}
        if isinstance(remote, tuple):
            rec = {aid: {'blob': blob('alice@example.com', *remote), 'name': '书房的 Mac'}}
        elif remote == 'forged':
            x = blob('alice@example.com', R2, 1795000000.0)
            rec = {aid: {'blob': dict(x, t=x['t'][:-4] + 'AAA=')}}
        elif remote == 'other':
            rec = {aid: {'blob': blob('bob@example.com', R2, 1795000000.0)}}
        elif remote == 'junk':
            rec = {aid: {'blob': 'not a blob'}}
        reply(b, 'keys', {'keys': rec}, nth=1)
        if 'put' not in kw:
            reply(b, 'keys', {'ok': True}, nth=2)
        elif kw['put']:
            reply(b, 'keys', nth=2, **kw['put'])


# 服务器上的新：照它改这台设备上的
for name, kw in [
    ('new-key', dict(remote=(R2, 1795000000.25))),
    ('new-key-from-this-mac', dict(remote=(R2, 1795000000.25), me=True)),          # 是这台设备自己传的：不记 from
    ('new-key-long-name', dict(remote=(R2, 1795000000.25), name='很长的名字' * 20)),   # from 只留 64 个字
    ('new-key-name-not-str', dict(remote=(R2, 1795000000.25), name=5)),
    ('new-key-nothing-here', dict(remote=(R2, 1795000000.25), book=False)),        # 这台设备上没有过：取回来
    ('same-key-newer', dict(remote=(R1, 1795000000.25), keys=keys_json(**{ALICE: {'at': 1794000000.5, 'sum': None}}), sum_of=R1)),   # 同一把：不重存，只记时间
    ('deleted-there', dict(remote=(None, 1795000000.25))),                           # 别的设备上删掉了：这台也删
    ('deleted-there-sticky', dict(remote=(None, 1795000000.25), sticky=True)),       # 钥匙串里删不掉：先不记，下一轮再删
    ('deleted-there-nothing-here', dict(remote=(None, 1795000000.25), book=False)),
    ('new-key-keychain-locked', dict(remote=(R2, 1795000000.25), locked=True)),      # 存不进钥匙串
    ('new-key-int-time', dict(remote=(R2, 1795000000))),
    ('remote-bad-key', dict(remote=('short', 1795000000.25))),                      # key 不合规矩：不认
    ('remote-time-zero', dict(remote=(R2, 0))),
]:
    @pcase('key-sync/pull-' + name, ['key-sync', 'force'])
    def _(b, kw=kw):
        keys = kw.get('keys')
        if kw.get('sum_of'):
            keys['acc']['claude:' + ALICE]['sum'] = src()['keyshare_sum'](kw['sum_of'])
        sharing(b, cfg=booked(b, at=1794000000.5) if kw.get('book', True) else {}, keys=keys)
        login(b)
        if kw.get('book', True):
            kc(b, REF + ALICE, 'reference', R1)
        for sw in ('sticky', 'locked'):
            if kw.get(sw):
                switch(b, 'keychain_' + sw)
        rec = {'blob': blob('alice@example.com', *kw['remote']), 'name': kw.get('name', '书房的 Mac')}
        if kw.get('me'):
            rec['me'] = True
        reply(b, 'keys', {'keys': {aid_of('alice@example.com'): rec}}, nth=1)
        reply(b, 'keys', {'ok': True}, nth=2)


# key_hints 里没有记下的时间：看钥匙串里这一条的修改时间（security 不带 -w 列出的属性里的 mdat，UTC）
ATTRS = '''keychain: "/Users/x/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    0x00000007 <blob>="cca-ref-x"
    "acct"<blob>="reference"
    "cdat"<timedate>=0x32303235303130313030303030305A00  "20250101000000Z\\000"
    "mdat"<timedate>=%s
    "svce"<blob>="cca-ref-x"
'''
for name, mdat in [
    ('ok', '0x00  "20261107040855Z\\000"'), ('leap-day', '0x00  "20240229235959Z\\000"'), ('feb-30', '0x00  "20260230000000Z\\000"'),
    ('feb-29-not-leap', '0x00  "20260229000000Z\\000"'), ('month-13', '0x00  "20261307040855Z\\000"'), ('month-00', '0x00  "20260007040855Z\\000"'),
    ('day-00', '0x00  "20261100040855Z\\000"'), ('hour-24', '0x00  "20261107240855Z\\000"'), ('minute-60', '0x00  "20261107046055Z\\000"'),
    ('second-60', '0x00  "20261107040860Z\\000"'), ('second-61', '0x00  "20261107040861Z\\000"'), ('second-62', '0x00  "20261107040862Z\\000"'),
    ('year-0000', '0x00  "00001107040855Z\\000"'), ('year-0001', '0x00  "00010101000000Z\\000"'), ('year-9999', '0x00  "99991231235959Z\\000"'),
    ('13-digits', '0x00  "2026110704085Z\\000"'), ('15-digits', '0x00  "202611070408555Z\\000"'), ('no-z', '0x00  "20261107040855\\000"'),
    ('not-quoted', '20261107040855Z'), ('epoch', '0x00  "19700101000000Z\\000"'), ('before-epoch', '0x00  "19691231235959Z\\000"'),
    ('fullwidth-digits', '0x00  "２０２６1107040855Z\\000"'), ('two-values', '0x00  "20261107040855Z\\000" "20200101000000Z"'),
]:
    @pcase('key-sync/keychain-mdat-' + name, ['key-sync', 'force'])
    def _(b, mdat=mdat):
        sharing(b, cfg=booked(b))
        login(b)
        kc(b, REF + ALICE, 'reference', R1)
        b.write('keychain_attrs', ATTRS % mdat)
        reply(b, 'keys', {'keys': {}}, nth=1)
        reply(b, 'keys', {'ok': True}, nth=2)


@pcase('key-sync/keyfile-store', ['key-sync', 'force'], env={'CCA_KEYSTORE': 'file'})   # 没有钥匙串的系统：密钥文件，时间看文件的修改时间
def _(b):
    sharing(b, cfg=booked(b))
    login(b)
    p = b.write('.cca/secrets/%s%s' % (REF, ALICE), json.dumps({'reference': R1}) + '\n')
    os.chmod(os.path.dirname(p), 0o700)
    os.utime(p, (1794000000.123456, 1794000000.123456))
    reply(b, 'keys', {'keys': {}}, nth=1)
    reply(b, 'keys', {'ok': True}, nth=2)


@pcase('key-sync/two-accounts', ['key-sync', 'force'])   # 一个取回来、一个传上去
def _(b):
    policy = dict(SHARING, claude={'alice@example.com': 'US-A', 'bob@example.com': 'US-B'})
    sharing(b, policy=policy, cfg=booked(b, at=1794000000.5, ids=(BOB,)))
    login(b)
    login(b, BOB, 'bob-uuid-0001')
    kc(b, REF + BOB, 'reference', R1)
    reply(b, 'keys', {'keys': {aid_of('alice@example.com'): {'blob': blob('alice@example.com', R2, 1795000000.25), 'name': '书房的 Mac'}}}, nth=1)
    reply(b, 'keys', {'ok': True}, nth=2)


@pcase('key-sync/agy-never', ['key-sync', 'force'], tool='agy')   # Antigravity 的账号不同步；Claude 的照常（和当前工具无关）
def _(b):
    sharing(b, agy={'gina@example.com': 'US-B'}, cfg=booked(b, at=1794000000.5))
    login(b)
    kc(b, REF + ALICE, 'reference', R1)
    reply(b, 'keys', {'keys': {}}, nth=1)
    reply(b, 'keys', {'ok': True}, nth=2)


# --- presence-agent ---------------------------------------------------------------------------------------------------
ACCT = '.cca/accounts/%s' % ALICE
FAKE_PID = 999999   # 不可能有这个进程
LAUNCHD = None      # 下面 ps_row 定义以后填上


def ps_row(pid, ppid=1, tty='??', cpu='0:00.50', cmd='claude'):
    return '%5d %5d %-8s %9s %s %s\n' % (pid, ppid, tty, cpu, T0, cmd)


LAUNCHD = ps_row(1, 0, cmd='/sbin/launchd')


def mark(b, pid, since=None):
    b.write('%s/cli-%d.json' % (ACCT, pid), json.dumps({'v': 1, 'tool': 'claude', 'id': ALICE, 'pid': pid, 'started': T0_EPOCH, 'since': since or time.time() - 100}))


def wait_for(b, rel, needle, count=1, secs=8):
    """等沙箱里的这个文件里出现 count 次 needle"""
    end = time.time() + secs
    while time.time() < end:
        try:
            with open(b.path(rel), encoding='utf-8') as f:
                if f.read().count(needle) >= count:
                    return True
        except OSError:
            pass
        time.sleep(0.05)
    return False


def wait_log(b, needle, count=1, secs=8):
    return wait_for(b, 'curl.log', needle, count, secs)


def agent_pid(b):
    with open(b.path('.cca/presence/agent.json')) as f:
        return json.load(f)['pid']


def stop_agent(b):
    os.kill(agent_pid(b), signal.SIGTERM)


@pcase('agent/no-presence', ['presence-agent'])
def _(b):
    sharing(b, policy=dict(SHARING, presence=None))


@pcase('agent/too-many-args', ['presence-agent', 'x'])
def _(b):
    sharing(b)


@pcase('agent/nothing-open', ['presence-agent'])   # 没有会话：报一次「什么都没开」就退出
def _(b):
    sharing(b)
    b.write('ps.out', LAUNCHD)
    reply(b, 'report', {'ok': True})


@pcase('agent/no-mapped-accounts', ['presence-agent'])   # 映射里的账号这台设备上一个都没有：不看进程，照样报一次就退出
def _(b):
    sharing(b, table={'bob@example.com': 'US-B'})
    reply(b, 'report', {'ok': True})


@pcase('agent/nothing-open-server-down', ['presence-agent'], during=lambda b: (wait_log(b, '/v1/report'), time.sleep(0.5), stop_agent(b)))
def _(b):   # 报不出去：记下原因，等着再试（被停掉时没报成过，不用再说一声）
    sharing(b)
    b.write('ps.out', LAUNCHD)


@pcase('agent/ps-fails', ['presence-agent'], during=lambda b: (time.sleep(1.0), stop_agent(b)))
def _(b):   # 进程表读不了：这一轮不报（不能当成什么都没开），5 秒后再看
    sharing(b)


@pcase('agent/table-unreadable', ['presence-agent'], during=lambda b: (time.sleep(1.0), stop_agent(b)))
def _(b):
    sharing(b)
    b.write('ps.out', LAUNCHD)
    b.write('.cca/accounts.json', '{oops')


def hold_lock(b):
    """另一个上报进程拿着 agent.lock"""
    import fcntl
    os.makedirs(b.path('.cca/presence'), mode=0o700, exist_ok=True)
    f = open(b.path('.cca/presence/agent.lock'), 'a')
    fcntl.flock(f, fcntl.LOCK_EX)
    return f.close


@pcase('agent/lock-held', ['presence-agent'], before=hold_lock)
def _(b):
    sharing(b)
    b.write('ps.out', LAUNCHD)
    reply(b, 'report', {'ok': True})


def session_then_gone(b):
    """开着一个命令行会话 → 报一次；会话没了 → 再报一次「什么都没开」，退出"""
    wait_log(b, '"cli":[[')
    b.write('ps.out', LAUNCHD)


TTY = sorted(n for n in os.listdir('/dev') if re.match(r'ttys\d+$', n))[:1]   # 这台机器上真有的一个终端（没有就不测那一条）
for name, kw in [('cli-session', {}), ('cli-session-tty-gone', {'tty': 'ttys99999'}), ('cli-session-odd-tty', {'tty': 'console'})] + [('cli-session-on-a-tty', {'tty': t}) for t in TTY]:
    @pcase('agent/' + name, ['presence-agent'], during=session_then_gone, timeout=30)
    def _(b, kw=kw):
        sharing(b)
        login(b)
        mark(b, FAKE_PID)
        b.write('ps.out', LAUNCHD + ps_row(FAKE_PID, **kw))
        reply(b, 'report', {'ok': True})
        reply(b, 'keys', {'keys': {}})


@pcase('agent/two-sessions-one-stale', ['presence-agent'], during=session_then_gone, timeout=30)   # 登记还在、进程没了的不算
def _(b):
    sharing(b)
    mark(b, FAKE_PID)
    mark(b, FAKE_PID - 5)
    b.write('ps.out', LAUNCHD + ps_row(FAKE_PID))
    reply(b, 'report', {'ok': True})


@pcase('agent/session-pid-reused', ['presence-agent'])   # 进程号被别的进程重用了（启动时间对不上）：不算
def _(b):
    sharing(b)
    mark(b, FAKE_PID)
    b.write('ps.out', LAUNCHD + ps_row(FAKE_PID).replace('12:08:55', '12:09:30'))
    reply(b, 'report', {'ok': True})


def app_then_gone(b):
    wait_log(b, '"app":[')
    b.write('ps.out', LAUNCHD)


for name, tail in [('desktop-app', ''), ('desktop-app-more-args', ' --proxy-server=http://192.0.2.10:7901'), ('desktop-app-trailing-slash', '/')]:
    @pcase('agent/' + name, ['presence-agent'], during=app_then_gone, timeout=30,
           mask=AGENT_MASK + [(r'"app":\[\d+,\d+\]', '"app":[<OPEN>,<IDLE>]')])
    def _(b, tail=tail):
        sharing(b)
        os.makedirs(b.path(ACCT + '/desktop'))
        app = '/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s%s' % (b.path(ACCT + '/desktop'), tail)
        b.write('ps.out', LAUNCHD + ps_row(FAKE_PID, cpu='1-02:03:04.50', cmd=app)
                + ps_row(FAKE_PID - 1, ppid=FAKE_PID, cpu='12:34.56', cmd='/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper --type=renderer'))
        reply(b, 'report', {'ok': True})


@pcase('agent/desktop-app-realpath', ['presence-agent'], during=app_then_gone, timeout=30, mask=AGENT_MASK + [(r'"app":\[\d+,\d+\]', '"app":[<OPEN>,<IDLE>]')])
def _(b):   # 资料目录经过链接：cca app 传给 App 的是真实路径（带空格、# 这些要转义的字符）
    sharing(b)
    real = b.path('real desk #1 (a+b)')
    os.makedirs(real)
    os.makedirs(b.path(ACCT))
    os.symlink(real, b.path(ACCT + '/desktop'))
    b.write('ps.out', LAUNCHD + ps_row(FAKE_PID, cmd='/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s --lang=zh' % real))
    reply(b, 'report', {'ok': True})


@pcase('agent/desktop-app-other-profile', ['presence-agent'])   # 别的资料目录的 App 不算这个账号的
def _(b):
    sharing(b)
    os.makedirs(b.path(ACCT + '/desktop'))
    b.write('ps.out', ps_row(FAKE_PID, cmd='/Applications/Claude.app/Contents/MacOS/Claude --user-data-dir=%s-2' % b.path(ACCT + '/desktop')))
    reply(b, 'report', {'ok': True})


def stop_after_report(b):
    wait_log(b, '"cli":[[')
    time.sleep(0.3)
    stop_agent(b)


@pcase('agent/stopped-with-session', ['presence-agent'], during=stop_after_report, timeout=30)   # 被停掉：告诉服务器 end=down
def _(b):
    sharing(b)
    mark(b, FAKE_PID)
    b.write('ps.out', ps_row(FAKE_PID))
    reply(b, 'report', {'ok': True})


@pcase('agent/stopped-server-rejects', ['presence-agent'], during=stop_after_report, timeout=30)   # 服务器不认这台 Mac：10 分钟后再试；被停掉时没报成过
def _(b):
    sharing(b)
    mark(b, FAKE_PID)
    b.write('ps.out', ps_row(FAKE_PID))
    reply(b, 'report', {'error': 'unknown key'}, code=403)


for name, slow, first in [('stopped-mid-report', 'report', '/v1/report'), ('stopped-mid-key-sync', 'keys', '/v1/keys')]:
    # 请求发到一半被停掉：子进程（curl）杀掉，不记「没报成」，也不再发别的（还没报成过）
    @pcase('agent/' + name, ['presence-agent'], during=lambda b, first=first: (wait_log(b, first), time.sleep(0.4), stop_agent(b)), timeout=30, settle=3.2)
    def _(b, slow=slow):
        sharing(b, cfg=booked(b, at=1794000000.5))
        login(b)
        kc(b, REF + ALICE, 'reference', R1)
        mark(b, FAKE_PID)
        b.write('ps.out', ps_row(FAKE_PID))
        b.write('slow_' + slow, '')
        reply(b, 'report', {'ok': True})
        reply(b, 'keys', {'keys': {}})


# 真的起一个进程当会话：kqueue 盯着它（上面那些用的是不存在的进程号，上报进程每秒回来看一次）
LIVE = {}


def real_session(b):
    p = subprocess.Popen(['/bin/sleep', '30'], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    LIVE['p'] = p
    mark(b, p.pid)
    b.write('ps.out', ps_row(p.pid))

    def cleanup():
        p.kill()
        p.wait()
        for n in os.listdir(b.path(ACCT)):
            if n.startswith('cli-'):   # 进程号每次不一样：登记删掉再比沙箱
                os.unlink(b.path('%s/%s' % (ACCT, n)))
        b.write('ps.out', '')
    return cleanup


def exits(b):
    wait_log(b, '"cli":[[')
    time.sleep(0.5)   # 等它睡进 kqueue
    b.write('ps.out', LAUNCHD)
    LIVE['p'].kill()
    wait_log(b, '"end":"none"')


def killed(b):
    wait_log(b, '"cli":[[')
    time.sleep(0.5)
    stop_agent(b)
    wait_log(b, '"end":"down"')


def updated(b):
    """cca 更新了（文件换了）：账号目录里一有动静，上报进程就换成新版本"""
    wait_log(b, '"cli":[[')
    time.sleep(0.5)
    os.utime(b.path('.cca/bin/cca'), (1, 1))
    b.write(ACCT + '/poke', '')
    wait_for(b, 'calls.log', '_presence-agent')


def second_session(b):
    """账号目录里多了一个登记（另一个会话）：马上醒来再报"""
    wait_log(b, '"cli":[[')
    time.sleep(0.5)
    b.write('ps.out', ps_row(LIVE['p'].pid) + ps_row(FAKE_PID))
    mark(b, FAKE_PID)
    wait_log(b, '/v1/report', 2)
    time.sleep(0.5)   # 等这一次报完（假 curl 记完账）再停
    stop_agent(b)
    wait_log(b, '"end":"down"')


for name, during in [('real-session-exits', exits), ('real-session-stopped', killed), ('real-session-cca-updated', updated), ('real-session-second-session', second_session)]:
    @pcase('agent/' + name, ['presence-agent'], before=real_session, during=during, timeout=30, settle=0.8,
           mask=AGENT_MASK + [(r'"cli":\[\[\d+,\d+\],\[\d+,\d+\]\]', '"cli":[[<OPEN>,<IDLE>],[<OPEN>,<IDLE>]]')])
    def _(b):
        sharing(b)
        reply(b, 'report', {'ok': True})


# --- presence-view（cca devices）---------------------------------------------------------------------------------------
# 服务器回的是“多久以前”（秒），cca 照收到回答的时刻换成时间再说成“几点 / 昨天几点 / 周几 / 几月几日”“闲置多久”“多久前”。
# 夹具里的“多久以前”在放夹具的那一刻现算：让换出来的时间落在某一分钟的第 30 秒、时长离整分钟差 30 秒，两边先后跑差的那一两秒
# 不会跨过去（跨零点的那一分钟里跑会差一天，碰上了重跑）
VIEW_MASK = MASK + [(r'"(generated_epoch|since_epoch|last_active_epoch|last_used_epoch|seen_epoch)": \d+(\.\d+)?', r'"\1": <T>')]
# 自己的同步服务器（docs/dev/hub.md）以后多的：账号问的是哪台（hub）、各台的名字和设置（hubs）。原来的 Python 没有，去掉再比
HUB_MASK = [(r',\s*"hub": (?:null|"(?:maintainer|own)")', ''), (r', "hubs": \[(?:\{[^{}]*\}(?:, )?)*\]', '')]
VIEW_MASK += HUB_MASK


def ago_to(secs_back):
    """多久以前，使得「现在 - 它」落在 secs_back 秒以前那一分钟的第 30 秒"""
    now = time.time()
    return now - ((now - secs_back) // 60 * 60 + 30)


EACH_LOG = '#!/bin/sh\nprintf \'%s EVENTS=[%s] TOOL=[%s]\\n\' "cca $*" "$CCA_EVENTS" "$CCA_TOOL" >> "$HOME/calls$1.log"\n'


def vcase(name, argv, **kw):
    """cca devices 顺带在后台起 _key-sync 和 _cost-sync：两个进程谁先写完不一定，假的 cca 各记各的文件"""
    kw.setdefault('mask', VIEW_MASK)
    kw.setdefault('settle', 0.5)

    def reg(setup):
        def own_logs(b):
            setup(b)
            b.write('.cca/bin/cca', EACH_LOG, 0o755)
        return pcase('view/' + name, argv, **kw)(own_logs)
    return reg


def view_world(b, rows=None, body=None, code=200, rc=0, table=None, **kw):
    sharing(b, table=table, **kw)
    if body is None:
        body = {'accounts': {aid_of('alice@example.com'): rows if rows is not None else []}, 'me_name': '这台 Mac', 'idle': 600, 'ttl': 150}
    reply(b, 'view', body, code=code, rc=rc)


def all_states():
    return [
        {'fp': 'SHA256:other-1', 'name': '书房的 Mac', 'state': 'idle', 'cli': 1, 'app': False, 'since_age': ago_to(5 * 3600), 'idle_age': 3600 + 12 * 60 + 30, 'used_age': 3600 + 12 * 60 + 30, 'seen_age': 20},
        {'fp': FP_SELF, 'name': '这台 Mac', 'state': 'in_use', 'cli': 2, 'app': True, 'since_age': ago_to(3 * 3600), 'idle_age': 5, 'used_age': 5, 'seen_age': 5},
        {'fp': 'SHA256:other-2', 'name': '笔记本', 'state': 'offline', 'cli': 1, 'app': False, 'since_age': ago_to(30 * 3600), 'idle_age': 7200, 'used_age': ago_to(26 * 3600), 'seen_age': 4 * 3600 + 30},
        {'fp': 'SHA256:other-3', 'name': '刚断的', 'state': 'offline', 'cli': 0, 'app': True, 'used_age': ago_to(600), 'seen_age': 30},
        {'fp': 'SHA256:other-4', 'name': '很久没消息', 'state': 'offline', 'used_age': None, 'seen_age': None},
        {'fp': 'SHA256:other-5', 'name': '上周用过', 'state': 'none', 'used_age': ago_to(4 * 86400)},
        {'fp': 'SHA256:other-6', 'name': '上个月用过', 'state': 'none', 'used_age': ago_to(25 * 86400)},
        {'fp': 'SHA256:other-7', 'name': '去年用过', 'state': 'none', 'used_age': ago_to(400 * 86400)},
        {'fp': 'SHA256:other-8', 'name': '昨天用过', 'state': 'none', 'used_age': ago_to(24 * 3600)},
        {'fp': 'SHA256:other-9', 'name': '没用过', 'state': 'none'},
        {'fp': 'SHA256:other-a', 'name': '只开着 App', 'state': 'in_use', 'cli': 0, 'app': True},
        {'fp': 'SHA256:other-b', 'name': '闲置没说多久', 'state': 'idle', 'cli': True, 'app': 1},
        {'fp': 'SHA256:other-c', 'name': '状态不认识', 'state': 'sleeping'}, {'fp': 'SHA256:other-d', 'state': 'idle'}, {'name': 5, 'state': 'idle'}, 'junk', None,
        {'name': '没有指纹', 'state': 'idle', 'cli': 2.0, 'app': 'yes', 'idle_age': 'x', 'since_age': True},
    ]


import base64 as _b64   # noqa: E402
import hashlib as _hl   # noqa: E402
from cases.secrets import PUB as _PUB   # noqa: E402
FP_SELF = 'SHA256:' + _b64.b64encode(_hl.sha256(_b64.b64decode(_PUB.split()[1])).digest()).decode().rstrip('=')   # 夹具里那把公钥的指纹

for name, argv, tool in [('all-text', ['presence-view'], 'claude'), ('all-json', ['presence-view', '--json'], 'claude'), ('by-alias', ['presence-view', '主号'], 'claude'),
                         ('by-email-json', ['presence-view', '--json', 'alice@example.com'], 'claude'), ('json-last', ['presence-view', 'alice@example.com', '--json'], 'claude'),
                         ('same-account-twice', ['presence-view', '主号', 'alice@example.com'], 'claude')]:
    @vcase('states-' + name, argv, tool=tool)
    def _(b):
        view_world(b, all_states())


for name, argv, why in [('unknown-name', ['presence-view', 'ghost'], None), ('not-mapped', ['presence-view', 'bob@example.com'], None), ('api-account', ['presence-view', '直连'], None),
                        ('mapped-then-not', ['presence-view', '主号', 'bob@example.com'], None), ('empty-name', ['presence-view', ''], None)]:
    @vcase('names-' + name, argv)
    def _(b):
        view_world(b, all_states())


for name, kw in [('no-presence', dict(policy=dict(SHARING, presence=None))), ('presence-off', dict(cfg={'presence': False})), ('no-deploy-key', dict(key=False)),
                 ('bad-presence', dict(policy=dict(SHARING, presence=dict(PRESENCE, pins=[]))))]:
    for mode in ([], ['--json']):
        @vcase('unusable-%s%s' % (name, '-json' if mode else ''), ['presence-view'] + mode)
        def _(b, kw=kw):
            view_world(b, all_states(), **kw)


@vcase('nobody-mapped-here', ['presence-view'])   # 映射里的账号这台设备上一个都没有：不问服务器
def _(b):
    view_world(b, table={'bob@example.com': 'US-B'})


@vcase('nobody-mapped-here-json', ['presence-view', '--json'])
def _(b):
    view_world(b, table={'bob@example.com': 'US-B'})


@vcase('no-devices', ['presence-view'])
def _(b):
    view_world(b, [])


@vcase('bad-table', ['presence-view'])
def _(b):
    view_world(b, table={'主号(alice@example.com)': 'NOPE'})


for name, kw in [
    ('unreachable', None), ('403', dict(body={}, code=403)), ('500', dict(body='oops', code=500)), ('429', dict(body={}, code=429)), ('pin-mismatch', dict(body='', rc=90)),
    ('clock', dict(body={'error': 'clock', 'now': 1794000000}, code=400)), ('200-not-json', dict(body='hello')), ('200-accounts-list', dict(body={'accounts': []})),
    ('200-no-accounts', dict(body={'me_name': 'x'})),
]:
    for mode in ([], ['--json']):
        @vcase('server-%s%s' % (name, '-json' if mode else ''), ['presence-view'] + mode, mask=VIEW_MASK + [(r'差了 \d+ 秒', '差了 <N> 秒')])
        def _(b, kw=kw):
            sharing(b)
            if kw:
                reply(b, 'view', **kw)


for name, body in [
    ('rows-is-dict', {'accounts': {'%AID%': {'a': 1}}}), ('rows-is-str', {'accounts': {'%AID%': 'xyz'}}), ('rows-null', {'accounts': {'%AID%': None}}),
    ('account-missing', {'accounts': {}}), ('me-name-number', {'accounts': {'%AID%': []}, 'me_name': 5, 'idle': 'x', 'ttl': [1]}),
    ('me-name-empty', {'accounts': {'%AID%': []}, 'me_name': '', 'idle': None}), ('no-extras', {'accounts': {'%AID%': []}}),
]:
    for mode in ([], ['--json']):
        @vcase('shape-%s%s' % (name, '-json' if mode else ''), ['presence-view'] + mode)
        def _(b, body=body):
            sharing(b)
            reply(b, 'view', json.dumps(body).replace('%AID%', aid_of('alice@example.com')))


@vcase('sorted', ['presence-view', '--json'])   # 这台设备在前，再按状态（在用、闲置、离线、没在用）、最近用过的在前、名字
def _(b):
    rows = [{'fp': 'f%d' % i, 'name': n, 'state': s, 'used_age': u} for i, (n, s, u) in enumerate([
        ('丙', 'none', 500), ('乙', 'none', 100), ('甲', 'none', 100), ('丁', 'none', None), ('b', 'offline', 10), ('a', 'idle', 900), ('c', 'in_use', 5000), ('B', 'in_use', 5000),
        ('无', 'none', 0)])]
    view_world(b, rows + [{'fp': FP_SELF, 'name': 'zz 这台', 'state': 'none', 'used_age': 99999}])


for name, files in [('report-failing', {'status.json': {'error': '连不上设备统计的服务器（https://192.0.2.10:18443）', 'at': 1.5}}), ('report-ok', {'status.json': {'error': '', 'at': 1.5}}),
                    ('key-sync-failing', {'keys.json': {'v': 1, 'acc': {}, 'synced': 1.0, 'error': '读不了这台设备的钥匙串'}}),
                    ('both-failing', {'status.json': {'error': 'x'}, 'keys.json': {'v': 1, 'acc': {}, 'synced': 1.0, 'error': 'y'}}),
                    ('status-not-json', {'status.json': '{oops'}), ('keys-error-not-str', {'keys.json': {'v': 1, 'acc': {}, 'error': 5}})]:
    for mode in ([], ['--json']):
        @vcase('notes-%s%s' % (name, '-json' if mode else ''), ['presence-view'] + mode)
        def _(b, files=files):
            view_world(b, all_states()[:2])
            for n, data in files.items():
                b.write('.cca/presence/' + n, data if isinstance(data, str) else json.dumps(data))
            os.chmod(b.path('.cca/presence'), 0o700)


@vcase('codex', ['presence-view'], tool='codex')
def _(b):
    sharing(b, codex={'erin@example.com': 'US-A'})
    reply(b, 'view', {'accounts': {aid_of('erin@example.com', 'CODEX'): [{'fp': FP_SELF, 'name': '这台 Mac', 'state': 'in_use', 'cli': 1, 'app': True, 'since_age': ago_to(3600)}]}, 'me_name': '这台 Mac'})


@vcase('codex-json', ['presence-view', '--json'], tool='codex')
def _(b):
    sharing(b, codex={'erin@example.com': 'US-A'})
    reply(b, 'view', {'accounts': {aid_of('erin@example.com', 'CODEX'): [{'fp': 'x', 'name': '别的', 'state': 'idle', 'cli': 1, 'idle_age': 660 + 30}]}})


@vcase('agy', ['presence-view'], tool='agy')
def _(b):
    sharing(b, agy={'gina@example.com': 'US-B'})
    reply(b, 'view', {'accounts': {aid_of('gina@example.com', 'AGY'): [{'fp': 'x', 'name': '别的', 'state': 'in_use', 'cli': 3, 'app': False}]}})


@vcase('ro', ['presence-view'], env={'CCA_RO': '1'})
def _(b):
    view_world(b, all_states()[:3])
