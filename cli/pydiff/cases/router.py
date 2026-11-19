"""多设备共用一个路由器的合并、槽位核对、卸载（cli/src/cmd/router.rs）。"""
import json

from run import case
from cases.table import world, FILL

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
