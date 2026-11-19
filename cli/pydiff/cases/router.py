"""多设备共用一个路由器的合并、槽位核对、卸载（cli/src/cmd/router.rs）。"""
import json

from run import case
from cases.table import world, FILL, NODES, POLICY, TABLE

ME = 'aaaaaaaaaaaaaaaa'
PORTS = {'主号(alice@example.com)': 7901, 'bob@example.com': 7902, '走节点': 7903}


def ident(k):
    """账号在路由器端口池里的名字就是账号目录名：照 cca 的 ident 现算"""
    import hashlib
    import re
    s = re.sub(r'[^a-z0-9]+', '-', k.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(k.encode('utf-8')).hexdigest()[:6])


def remote(accounts, owners=None, sep=True):
    acc = json.dumps({'base': 7900, 'slots': 8, 'nodes': ['US-A'], 'accounts': accounts}, ensure_ascii=False)
    if owners is None:
        return acc
    return acc + ('\n---CCA-OWN---\n' if sep else '\n') + json.dumps(owners) + '\n'


def mine(b, **cfg):
    world(b, cfg=dict({'ports': dict(PORTS), 'device_id': ME}, **cfg))


MASK_ID = [(r'(?<![0-9a-f])[0-9a-f]{16}(?![0-9a-f])', '<随机的设备标识>')]


@case('router-merge/fresh-router', ['router-merge'], stdin='')
def _(b):
    mine(b)


@case('router-merge/lazy-device-id', ['router-merge'], stdin='', mask=MASK_ID)
def _(b):
    world(b, cfg={'ports': dict(PORTS)})


@case('router-merge/no-owners', ['router-merge'], stdin=remote([{'slot': 5, 'name': 'someone', 'node': 'HK-A'}]))
def _(b):
    mine(b)


@case('router-merge/all-mine', ['router-merge'], stdin=remote([{'slot': 5, 'name': 'old-account-of-mine', 'node': 'HK-A'}], {'5': ME}))
def _(b):
    mine(b)


def multi():
    a, bo = ident('alice@example.com'), ident('bob@example.com')
    return remote([
        {'slot': 1, 'name': 'other-1', 'node': 'JP-X'},          # 别的设备占着我的 7901
        {'slot': 2, 'name': bo, 'node': 'US-B'},                 # 共用账号：别的设备推的
        {'slot': 4, 'name': a, 'node': 'US-A'},                  # 我上次推的：沿用 4
        {'slot': 6, 'name': 'other-reject', 'node': 'REJECT'},
        {'slot': 9, 'name': 'other-9', 'node': 'JP-X'},
        {'slot': 3, 'name': '', 'node': 'ZZ'},                   # 没名字的不算
        {'slot': 7, 'name': 'no-owner', 'node': 'ZZ'},           # owners 里没有：不是别的设备的
        'junk', {'name': 'no-slot'},
    ], {'1': 'bbbbbbbbbbbbbbbb', '2': 'bbbbbbbbbbbbbbbb', '4': ME, '6': 'cccccccccccccccc', '9': 'cccccccccccccccc', '3': 'bbbbbbbbbbbbbbbb'})


MULTI = multi()

for name, tool in [('claude', 'claude'), ('codex', 'codex')]:
    @case('router-merge/multi-device-%s' % name, ['router-merge'], tool=tool, stdin=MULTI)
    def _(b):
        mine(b, codex={'ports': {'erin@example.com': 7904}, 'last': ''})
        b.table({'erin@example.com': 'HK-A'}, 'codex')


@case('router-merge/multi-device-fill', ['router-merge'], stdin=MULTI)
def _(b):
    mine(b)
    b.fill(FILL)


@case('router-merge/readonly', ['router-merge'], env={'CCA_RO': '1'}, stdin=MULTI, mask=MASK_ID)
def _(b):
    world(b, cfg={'ports': dict(PORTS)})


@case('router-merge/table-errors', ['router-merge'], stdin='')
def _(b):
    mine(b)
    b.table({'x y': 'US-A'}, 'codex')
    b.write('.cca/agy.json', '{bad')


@case('router-merge/garbage-stdin', ['router-merge'], stdin='not json\n---CCA-OWN---\n[1,2')
def _(b):
    mine(b)


@case('router-merge/owners-without-separator', ['router-merge'], stdin=remote([{'slot': 5, 'name': 'x', 'node': 'HK-A'}], {'5': 'bbbbbbbbbbbbbbbb'}, sep=False))
def _(b):
    mine(b)


@case('router-merge/float-and-string-slots', ['router-merge'],
      stdin=remote([{'slot': '3', 'name': 'str-slot', 'node': 'JP-X'}, {'slot': 5.0, 'name': 'float-slot', 'node': None}, {'slot': 0, 'name': 'zero', 'node': 'ZZ'}],
                   {'3': 'bbbbbbbbbbbbbbbb', '5.0': 'bbbbbbbbbbbbbbbb', '0': 'bbbbbbbbbbbbbbbb'}))
def _(b):
    mine(b)


for slot in ['1', '2', '9', '0', 'x']:
    @case('slot-name/%s' % slot, ['slot-name', slot],
          stdin=json.dumps({'accounts': [{'slot': 1, 'name': 'first'}, {'slot': 2.0, 'name': None}, 'junk', {'slot': 1, 'name': 'dup'}]}))
    def _(b):
        pass


@case('slot-name/empty-stdin', ['slot-name', '1'], stdin='')
def _(b):
    pass


@case('slot-name/garbage', ['slot-name', '1'], stdin='{oops')
def _(b):
    pass


@case('slot-name/not-object', ['slot-name', '1'], stdin='[1]')
def _(b):
    pass


@case('router-evict/only-me', ['router-evict'], stdin=remote([{'slot': 1, 'name': 'x', 'node': 'US-A'}], {'1': ME}))
def _(b):
    mine(b)


@case('router-evict/no-owners', ['router-evict'], stdin=remote([{'slot': 1, 'name': 'x', 'node': 'US-A'}]))
def _(b):
    mine(b)


@case('router-evict/empty-lazy-id', ['router-evict'], stdin='', mask=MASK_ID)
def _(b):
    world(b)


@case('router-evict/others-remain', ['router-evict'],
      stdin=remote([{'slot': 9, 'name': 'o9', 'node': 'JP-X'}, {'slot': 1, 'name': 'me1', 'node': 'US-A'}, {'slot': 3, 'name': 'o3', 'node': 'REJECT'},
                    {'slot': 2, 'name': 'o2', 'node': 'JP-X'}, {'slot': 4, 'name': 'o4'}],
                   {'9': 'bbbbbbbbbbbbbbbb', '1': ME, '3': 'cccccccccccccccc', '2': 'bbbbbbbbbbbbbbbb', '4': 'bbbbbbbbbbbbbbbb'}))
def _(b):
    mine(b)


@case('router-evict/readonly', ['router-evict'], env={'CCA_RO': '1'}, stdin=remote([{'slot': 2, 'name': 'o2', 'node': 'JP-X'}], {'2': 'bbbbbbbbbbbbbbbb'}), mask=MASK_ID)
def _(b):
    world(b)


# --- 路由器上的节点列表（cca-router.sh nodes 的输出）：改名跟随、代替节点、参考节点 ---
def fp(kind='', server='', port='', sni=''):
    """节点的摘要：照 cca 的 node_fp 现算"""
    import hashlib
    server = str(server or '').strip().lower().rstrip('.')
    if not server:
        return ''
    s = '%s|%s|%s|%s' % (str(kind or '').strip().lower(), server, str(port or '').strip(), str(sni or '').strip().lower().rstrip('.'))
    return hashlib.sha256(s.encode('utf-8')).hexdigest()[:16]


A = ('vless', 'a.example.com', '443', 'sni-a.example.com')
B = ('ss', '192.0.2.21', '8388', '')
H = ('trojan', 'hk.example.com', '443')
IDS = {'US-A': fp(*A), 'US-B': fp(*B), 'HK-A': fp(*H)}


def row(name, spec=()):
    return '\t'.join(('P', name) + tuple(spec))


def nodes(*rows):
    return '\n'.join(rows + ('G\tOpenAi', 'C\tsomething', 'X\tjunk', '')) + '\n'


SAME = nodes(row('US-A', A), row('US-B', B), row('HK-A', H), row('JP-X'))
USERS = {'erin@example.com': 'US-A', '科(frank@example.com)': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'US-A'}}


@case('nodes-follow/first-record', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b)


@case('nodes-follow/nothing-to-do', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/renamed', ['nodes-follow'], stdin=nodes(row('美国-A1', A), row('US-B', B), row('HK-甲', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS), 'ports': {'bob@example.com': 7902}}, codex=USERS, agy={'gina@example.com': 'US-B'})


@case('nodes-follow/renamed-no-users', ['nodes-follow'], stdin=nodes(row('US-A', A), row('US-B', B), row('HK-A', H), row('新名字', ('ss', 'x.example.com', '1'))))
def _(b):
    world(b, cfg={'nodes': NODES + ['US-X'], 'node_ids': dict(IDS, **{'US-X': fp('ss', 'x.example.com', '1')})})


@case('nodes-follow/lost-with-users', ['nodes-follow'], stdin=nodes(row('US-A', A), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)}, codex={'erin@example.com': 'US-B', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'US-B'}})


@case('nodes-follow/lost-no-users', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'nodes': NODES + ['US-X', 'US-Y'], 'node_ids': dict(IDS, **{'US-X': 'feedfeedfeedfeed'})})


@case('nodes-follow/merge-refused', ['nodes-follow'], stdin=nodes(row('NEW', A), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': {'US-A': fp(*A), 'US-B': fp(*A), 'HK-A': fp(*H)}})


@case('nodes-follow/target-in-use', ['nodes-follow'], stdin=nodes(row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': {'US-A': fp(*B), 'US-B': fp(*B), 'HK-A': fp(*H)}})


@case('nodes-follow/swap', ['nodes-follow'], stdin=nodes(row('US-B', A), row('US-A', B), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)}, codex=USERS)


@case('nodes-follow/same-address-twice-keeps-name', ['nodes-follow'], stdin=nodes(row('US-A-copy', A), row('US-A', A), row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/same-address-twice-lost', ['nodes-follow'], stdin=nodes(row('one', A), row('two', A), row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/address-changed-name-kept', ['nodes-follow'], stdin=nodes(row('US-A', ('vless', 'moved.example.com', '443')), row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/renames-cleanup', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'node_ids': dict(IDS), 'node_renames': {'OLD': 'US-A', 'JP-X': 'HK-A'}})


@case('nodes-follow/renames-all-gone', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'node_ids': dict(IDS), 'node_renames': {'JP-X': 'HK-A'}})


@case('nodes-follow/renames-not-a-dict', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'node_ids': dict(IDS), 'node_renames': ['junk']})


@case('nodes-follow/renames-chain', ['nodes-follow'], stdin=nodes(row('US-A3', A), row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, table={'a@example.com': 'US-A2'}, cfg={'nodes': ['US-A2', 'US-B', 'HK-A'], 'node_ids': {'US-A2': fp(*A)},
                                                   'node_renames': {'US-A': 'US-A2', 'US-A3': 'somewhere'}})


@case('nodes-follow/renames-keep-last-50', ['nodes-follow'], stdin=nodes(row('US-A3', A), row('US-B', B), row('HK-A', H)))
def _(b):
    world(b, table={'a@example.com': 'US-A2'},
          cfg={'nodes': ['US-A2', 'US-B', 'HK-A'], 'node_ids': {'US-A2': fp(*A)}, 'node_renames': {'old-%02d' % i: 'new-%02d' % i for i in range(55)}})


@case('nodes-follow/no-nodes-read', ['nodes-follow'], stdin='G\tOpenAi\nP\t\tss\tx\t1\nP\n\n')
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/empty-stdin', ['nodes-follow'], stdin='')
def _(b):
    world(b)


@case('nodes-follow/names-only', ['nodes-follow'], stdin='P\tUS-A\nP\tUS-B\nP\tHK-A')
def _(b):
    world(b, cfg={'node_ids': {'US-A': fp(*A), 'gone': 'x', 'HK-A': ''}})


@case('nodes-follow/names-only-nothing-recorded', ['nodes-follow'], stdin='P\tUS-A\nP\tUS-B\nP\tHK-A')
def _(b):
    world(b, cfg={'node_ids': 'junk'})


@case('nodes-follow/crlf', ['nodes-follow'], stdin=SAME.replace('\n', '\r\n'))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})


@case('nodes-follow/broken-tables', ['nodes-follow'], stdin=nodes(row('美国-A1', A), row('US-B', B)))
def _(b):
    world(b, cfg={'node_ids': dict(IDS)})
    b.write('.cca/codex.json', '{broken')
    b.write('.cca/agy.json', '["US-A"]')


@case('nodes-follow/odd-nodes-list', ['nodes-follow'], stdin=SAME)
def _(b):
    world(b, cfg={'nodes': ['US-A', 'US-A', 5, 'US-B']})


@case('nodes-follow/no-config', ['nodes-follow'], stdin=SAME)
def _(b):
    pass


@case('nodes-rename/not-on-router', ['nodes-rename', 'US-B', 'ZZ'], stdin=SAME)
def _(b):
    world(b)


@case('nodes-rename/replace', ['nodes-rename', 'US-B', 'JP-X'], stdin=SAME)
def _(b):
    world(b, cfg={'node_ids': dict(IDS), 'node_renames': {'OLD': 'US-B'}}, codex={'erin@example.com': 'US-B', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'US-B'}})


@case('nodes-rename/onto-existing', ['nodes-rename', 'US-B', 'US-A'], stdin=SAME)
def _(b):
    world(b)


@case('nodes-rename/old-not-used', ['nodes-rename', 'nope', 'JP-X'], stdin=SAME)
def _(b):
    world(b)


@case('nodes-rename/too-few-args', ['nodes-rename', 'US-B'], stdin=SAME)
def _(b):
    world(b)


FILL2 = {'version': 1, 'group': 'OpenAi', 'proxies': [
    {'name': 'US-C', 'type': 'vless', 'server': 'c.example.com', 'port': 443, 'servername': 'sni.example.com', 'sni': 'ignored.example.com'},
    {'name': 'US-D', 'type': 'ss', 'server': '192.0.2.44', 'port': '8388', 'sni': 'only-sni.example.com'},
    {'name': 'US-A', 'type': 'ss', 'server': 'other.example.com', 'port': 1},            # 同名：用户自己的，不加
    {'name': 'OpenAi', 'type': 'ss', 'server': 'g.example.com', 'port': 2},              # 名字被策略组占了
    {'name': 'B-again', 'type': 'ss', 'server': '192.0.2.21', 'port': 8388},             # 和 US-B 是同一个节点
    {'name': 'via', 'type': 'ss', 'server': 'v.example.com', 'port': 3, 'dialer-proxy': 'US-C'},
    {'name': 'via-missing', 'type': 'ss', 'server': 'w.example.com', 'port': 4, 'dialer-proxy': 'nowhere'},
    {'name': 'via-user', 'type': 'ss', 'server': 'u.example.com', 'port': 5, 'dialer-proxy': 'HK-A'},
]}

for name, text in [('plain', SAME), ('no-final-newline', SAME.rstrip('\n')), ('empty', ''), ('groups-only', 'G\tOpenAi\nC\tx\n'),
                   ('names-only', 'P\tUS-A\nP\tUS-C'), ('crlf', SAME.replace('\n', '\r\n'))]:
    @case('nodes-fill/%s' % name, ['nodes-fill'], stdin=text)
    def _(b):
        world(b, fill=FILL2)

    @case('nodes-fill/%s-no-fill' % name, ['nodes-fill'], stdin=text)
    def _(b):
        world(b)


@case('nodes-fill/simple-fill', ['nodes-fill'], stdin=SAME)
def _(b):
    world(b, fill=FILL)


@case('nodes-fill/bad-fill', ['nodes-fill'], stdin=SAME)
def _(b):
    world(b, fill={'version': 2, 'group': 'OpenAi', 'proxies': FILL['proxies']})


for name, fill in [('two', FILL), ('eight', FILL2), ('none', None), ('bad', {'version': 1, 'group': 'CCA-x', 'proxies': FILL['proxies']})]:
    @case('fill-summary/%s' % name, ['fill-summary'])
    def _(b, fill=fill):
        world(b, fill=fill)


BAD_FILLS = [
    FILL, FILL2, [], 'x', None, {}, {'version': 1}, {'version': '1', 'group': 'g', 'proxies': []}, {'version': 1, 'group': ' g', 'proxies': {}},
    {'version': 1, 'group': 'CCA-g', 'proxies': [1]}, {'version': 1, 'group': 'a\tb', 'proxies': ['x', None]},
    {'version': True, 'group': 'g' * 129, 'proxies': [{}]},
    {'version': 1.0, 'group': 'g', 'proxies': [
        {'name': 'DIRECT', 'type': 'ss', 'server': 's', 'port': 1}, {'name': 'n', 'type': 'ss', 'server': 's', 'port': 1},
        {'name': 'n', 'type': 'bad type', 'server': 'has space', 'port': 0}, {'name': 5, 'type': 5, 'server': 5, 'port': '65536'},
        {'name': 'p1', 'type': 'x' * 33, 'server': 's' * 256, 'port': True}, {'name': 'p2', 'type': 'ss', 'server': 's', 'port': 1.0},
        {'name': 'p3', 'type': 'ss', 'server': 's', 'port': '00443'}, {'name': 'p4', 'type': 'ss', 'server': 's', 'port': '４４３'},
        {'name': 'p5', 'type': 'ss\n', 'server': 's\n', 'port': '443\n'}, {'name': ' p6', 'type': '', 'server': '', 'port': None},
        {'name': 'p7', 'type': 'ss', 'server': '服务器.example.com', 'port': 65535}, {'name': 'CCA-p8', 'type': 'ss', 'server': 's', 'port': 99999999999999999999},
    ]},
]
for i, fill in enumerate(BAD_FILLS):
    @case('fill-check/%02d' % i, ['fill-check', '%HOME%/f.json'])
    def _(b, fill=fill):
        b.json('f.json', fill)


@case('fill-check/missing-file', ['fill-check', '%HOME%/nope.json'])
def _(b):
    pass


@case('fill-check/not-json', ['fill-check', '%HOME%/f.json'])
def _(b):
    b.write('f.json', '{"version": 1,}')


@case('fill-check/not-utf8', ['fill-check', '%HOME%/f.json'])
def _(b):
    b.write('f.json', b'{"group": "\xff\xfe"}')


@case('fill-check/is-a-directory', ['fill-check', '%HOME%/.cca'])
def _(b):
    world(b)


@case('fill-check/empty-file', ['fill-check', '%HOME%/f.json'])
def _(b):
    b.write('f.json', '')


# --- 映射文件的检查、请求锁定、维护者锁定 ---
PIN = 'sha256//' + 'A' * 43 + '='
BAD_POLICIES = [
    POLICY, [], 'x', None, {}, {'version': 2}, {'version': '1'}, {'version': True}, {'version': 1.0},
    {'version': 1, 'claude': [], 'codex': 'x', 'agy': None},
    {'version': 1, 'claude': {'not-an-email': 'US-A', 'a@example.com': '', 'b@example.com': 5, 'A@example.com': 'US-A', 'c@example.com': '  ', 'd@example.com': None,
                              'e@example.com\n': 'US-A', 'f(x)@example.com': 'US-A'}},
    {'version': 1, 'claude': {}, 'codex': {}, 'agy': {'g@example.com': 'US-A'}, 'extra': 1, '别的': []},
    {'version': 1, 'claude': {'a@example.com': 'US-A', 'b@example.com': 'US-A'}, 'codex': {'a@example.com': 'US-B', 'c@example.com': 'US-B'},
     'aliases': {'a@example.com': '同名', 'b@example.com': '同名', 'A@example.com': '又一个', 'zz@example.com': 'ok', 'c@example.com': 'x y',
                 'B@Example.com': 5}},
    {'version': 1, 'aliases': 'junk'}, {'version': 1, 'aliases': {}},
    {'version': 1, 'claude': {'a@example.com': 'US-A', 'b@example.com': 'US-B'}, 'codex': {'b@example.com': 'US-A', 'c@example.com': 'US-A'},
     'aliases': {'a@example.com': '甲', 'b@example.com': '乙', 'c@example.com': '甲'}},
    {'version': 1, 'install_node': 5}, {'version': 1, 'install_node': ''}, {'version': 1, 'install_node': 'DIRECT'}, {'version': 1, 'install_node': 'US-A'},
    {'version': 1, 'install_node': None},
    {'version': 1, 'presence': 'x'}, {'version': 1, 'presence': {}},
    {'version': 1, 'presence': {'url': 'https://192.0.2.10:18443', 'pins': [PIN], 'salt': 'ab' * 16}},
    {'version': 1, 'presence': {'url': 'https://192.0.2.10:18443/', 'pins': [PIN] * 4, 'salt': 'ab' * 64}},
    {'version': 1, 'presence': {'url': 'http://192.0.2.10:18443', 'pins': [], 'salt': 'AB' * 16}},
    {'version': 1, 'presence': {'url': 'https://192.0.2.10:0', 'pins': [PIN] * 5, 'salt': 'abc' * 11}},
    {'version': 1, 'presence': {'url': 'https://192.0.2.10:65536', 'pins': [PIN, 5], 'salt': 'ab' * 65}},
    {'version': 1, 'presence': {'url': 'https://[2001:db8::1]:443', 'pins': 'x', 'salt': 5}},
    {'version': 1, 'presence': {'url': 'https://host.example.com:99999', 'pins': [PIN + '\n'], 'salt': 'ab' * 16 + '\n'}},
    {'version': 1, 'presence': {'url': 'https://host.example.com:１２', 'pins': ['sha256//' + 'A' * 44], 'salt': ''}},
    {'version': 1, 'presence': {'url': 5, 'pins': None}},
]
for i, pol in enumerate(BAD_POLICIES):
    for strict in ('', 'strict', 'other'):
        @case('policy-check/%02d-%s' % (i, strict or 'plain'), ['policy-check', '%HOME%/p.json'] + ([strict] if strict else []))
        def _(b, pol=pol):
            b.json('p.json', pol)


@case('policy-check/missing-file', ['policy-check', '%HOME%/nope.json', 'strict'])
def _(b):
    pass


@case('policy-check/not-json', ['policy-check', '%HOME%/p.json'])
def _(b):
    b.write('p.json', 'version: 1\n')


@case('policy-check/crlf', ['policy-check', '%HOME%/p.json', 'strict'])
def _(b):
    b.write('p.json', '{\r\n  "version": 1,\r\n  "claude": {"a@example.com": "US-A"}\r\n}\r\n')


@case('policy-check/bom', ['policy-check', '%HOME%/p.json'])
def _(b):
    b.write('p.json', '﻿{"version": 1}')


@case('policy-check/no-args', ['policy-check'])
def _(b):
    pass


REQUESTS = [
    ('主号(alice@example.com)', {}, None), ('bob@example.com', {'policy': 'example/cca-policy'}, None),
    ('bob@example.com', {'policy': 'example/cca-policy'}, {'version': 1, 'claude': {'BOB@example.com': 'US-B'}}),
    ('bob@example.com', {'policy': 'example/cca-policy'}, {'version': 1, 'claude': {'other@example.com': 'US-A'}, 'codex': {'bob@example.com': 'US-A'}}),
    ('直连', {'policy': 'example/cca-policy'}, None), ('走节点', {}, None), ('光别名', {'policy': 'example/cca-policy'}, None),
    ("o'brien@example.com", {'policy': 'example/cca-policy'}, None), ('带空格(Space@Example.com)', {'policy': 'example/cca-policy'}, None),
]
for i, (key, cfg, pol) in enumerate(REQUESTS):
    @case('policy-request/%02d' % i, ['policy-request', key])
    def _(b, cfg=cfg, pol=pol):
        world(b, table=dict(TABLE, **{'光别名': 'US-A', "o'brien@example.com": 'HK-A', '带空格(Space@Example.com)': '节点 带 空格'}),
              cfg=dict(cfg, nodes=NODES + ['节点 带 空格']), policy=pol)


@case('policy-request/codex', ['policy-request', 'erin@example.com'], tool='codex')
def _(b):
    world(b, cfg={'policy': 'example/cca-policy'}, codex={'erin@example.com': 'HK-A'}, policy={'version': 1, 'claude': {'erin@example.com': 'US-A'}})


@case('policy-request/renamed-pin', ['policy-request', 'bob@example.com'])
def _(b):
    world(b, cfg={'policy': 'example/cca-policy', 'node_renames': {'OLD-B': 'US-B'}}, policy={'version': 1, 'claude': {'bob@example.com': 'OLD-B'}})


LOCK = {'version': 1, 'claude': {'Alice@example.com': 'US-B', 'carol@example.com': 'US-C'}, 'aliases': {'carol@example.com': '卡罗'}}
LOCKS = [
    ([], LOCK), (['plan'], LOCK), (['check', '%HOME%/r/policy.json', 'claude', 'a@example.com', 'US-A'], LOCK),
    (['plan', '%HOME%/r/policy.json'], LOCK), (['plan', '%HOME%/r/policy.json', 'nope', 'bad', ' x'], LOCK),
    (['plan', '%HOME%/r/policy.json', 'claude', 'a\x07b@example.com', 'CCA-1'], LOCK), (['plan', '%HOME%/r/policy.json', 'claude', 'a@example.com', 'a\tb'], LOCK),
    (['plan', '%HOME%/r/missing.json', 'claude', 'a@example.com', 'US-A'], LOCK), (['plan', '%HOME%/r/missing.json', 'zzz', 'a@example.com', 'US-A'], LOCK),
    (['plan', '%HOME%/r/policy.json', 'claude', 'a@example.com', 'US-A'], {'version': 2}), (['plan', '%HOME%/r/policy.json', 'claude', 'a@example.com', 'US-A'], []),
    (['plan', '%HOME%/r/policy.json', 'claude', 'ALICE@example.com', 'US-B'], LOCK), (['apply', '%HOME%/r/policy.json', 'claude', 'alice@example.com', 'US-B'], LOCK),
    (['plan', '%HOME%/r/policy.json', 'claude', 'ALICE@example.com', 'US-D'], LOCK), (['apply', '%HOME%/r/policy.json', 'claude', 'ALICE@example.com', 'US-D'], LOCK),
    (['plan', '%HOME%/r/policy.json', 'claude', 'New@Example.com', 'US-C'], LOCK), (['apply', '%HOME%/r/policy.json', 'claude', 'New@Example.com', 'US-C'], LOCK),
    (['plan', '%HOME%/r/policy.json', 'codex', 'new@example.com', '不在参考节点里'], LOCK), (['apply', '%HOME%/r/policy.json', 'agy', 'new@example.com', 'ZZ'], LOCK),
    (['apply', '%HOME%/r/policy.json', 'claude', 'new@example.com', 'US-C'], dict(LOCK, extra=1)),
    (['plan', '%HOME%/r/policy.json', 'claude', 'new@example.com', 'US-C'], dict(LOCK, aliases={'gone@example.com': 'x'})),
    (['apply', '%HOME%/r/policy.json', 'claude', 'new@example.com', 'US-C', 'one-too-many'], LOCK),
]
for i, (argv, pol) in enumerate(LOCKS):
    @case('policy-lock/%02d' % i, ['policy-lock'] + argv)
    def _(b, pol=pol):
        b.json('r/policy.json', pol, mode=0o644)
        b.json('r/nodes.json', FILL, mode=0o644)


@case('policy-lock/no-nodes-file', ['policy-lock', 'plan', '%HOME%/r/policy.json', 'claude', 'new@example.com', 'anything'])
def _(b):
    b.json('r/policy.json', LOCK)


@case('policy-lock/odd-nodes-file', ['policy-lock', 'apply', '%HOME%/r/policy.json', 'claude', 'new@example.com', 'anything'])
def _(b):
    b.json('r/policy.json', LOCK)
    b.json('r/nodes.json', {'proxies': [{'type': 'ss'}, 'junk']})


@case('policy-lock/relative-path', ['policy-lock', 'apply', 'policy.json', 'claude', 'new@example.com', 'US-C'], cwd='%HOME%/r')
def _(b):
    b.json('r/policy.json', LOCK)
    b.json('r/nodes.json', FILL)
