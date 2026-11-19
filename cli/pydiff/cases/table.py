"""账号表和 config.json 上的小命令（cli/src/cmd/table.rs）。

用例的写法：

    @case('名字', ['子命令', '参数…'], stdin='…', env={…}, tool='codex', mask=[(正则, 替换)])
    def _(b):            # b 是沙箱（run.py 的 Box）：b.cfg(…)、b.table(…)、b.policy(…)、b.write(相对 HOME 的路径, 内容)
        world(b)

夹具里只用假数据：节点 US-A…US-D、HK-A，路由器 192.0.2.10，邮箱 example.com。"""
from run import case

NODES = ['US-A', 'US-B', 'HK-A']
TABLE = {
    '主号(alice@example.com)': 'US-A',
    'bob@example.com': 'US-B',
    '直连': {'base_url': 'https://api.example.com/anthropic', 'model': 'm-1'},
    '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A', 'env': {'X': '1'}, 'id': 'zjd'},
}


def world(b, table=None, cfg=None, codex=None, agy=None, policy=None, fill=None):
    """常用的一套：三个可选节点、路由器、两个官方账号加两个第三方 API 账号"""
    c = {'version': 2, 'router': {'host': '192.0.2.10', 'port': 22, 'user': 'root', 'key': '~/.cca/router_ed25519'},
         'proxy_host': '192.0.2.10', 'nodes': list(NODES), 'min_trust': 50}
    c.update(cfg or {})
    b.cfg(c)
    b.table(TABLE if table is None else table)
    if codex is not None:
        b.table(codex, 'codex')
    if agy is not None:
        b.table(agy, 'agy')
    if policy is not None:
        b.policy(policy)
    if fill is not None:
        b.fill(fill)


FILL = {'version': 1, 'group': 'OpenAi', 'proxies': [
    {'name': 'US-C', 'type': 'vless', 'server': 'c.example.com', 'port': 443, 'servername': 'sni.example.com'},
    {'name': 'US-D', 'type': 'ss', 'server': '192.0.2.44', 'port': '8388'},
]}
POLICY = {'version': 1, 'claude': {'Alice@example.com': 'US-B', 'carol@example.com': 'US-C', 'dave@example.com': 'ZZ-none'},
          'codex': {'erin@example.com': 'HK-A'}, 'aliases': {'carol@example.com': '卡罗', 'alice@example.com': '别的名字'},
          'install_node': 'US-D'}


@case('init/empty-home', ['init'])
def _(b):
    pass


@case('init/existing', ['init'])
def _(b):
    world(b)


@case('init/codex', ['init'], tool='codex')
def _(b):
    world(b)


@case('keys/all', ['keys'])
def _(b):
    world(b)


@case('keys/official', ['keys', 'official'])
def _(b):
    world(b)


@case('keys/writes-ports-first-time', ['keys'])
def _(b):
    world(b, cfg={'ports': {'bob@example.com': 7905, 'gone@example.com': 7901}, 'last': 'gone@example.com'})


@case('keys/bad-table-json', ['keys'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"a": ')


@case('keys/table-not-object', ['keys'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '[1, 2]')


@case('keys/bad-config-json', ['keys'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{"nodes": [1,]}')


@case('keys/unknown-tool', ['keys'], tool='nope')
def _(b):
    world(b)


@case('dispatch/unknown', ['no-such-command', 'x', "it's"])
def _(b):
    world(b)


@case('dispatch/none', [])
def _(b):
    world(b)


for q in ['主号', 'alice@example.com', 'ALICE', '2', '4', '9', 'bob', '直', 'zzz', '', 'a', '主号(alice@example.com)', '0', 'b']:
    @case('resolve/%s' % (q or 'empty'), ['resolve', q])
    def _(b):
        world(b, table=dict(TABLE, **{'ali(alina@example.com)': 'US-A'}))


@case('validate/ok', ['validate'])
def _(b):
    world(b)


@case('validate/problems', ['validate'])
def _(b):
    world(b, table={'a b': 'US-A', 'x(y)': 'US-A', '甲(a@example.com)': 'US-A', '甲(A@example.com)': 'ZZ', '空': '', '数': 1,
                    'api(c@example.com)': {'base_url': 'http://api.example.com', 'model': 'a b', 'node': 'ZZ', 'extra': 1, 'key': 'short',
                                           'env': {'1X': 'a', 'HTTPS_PROXY': 'x', 'MY_TOKEN': 'x', 'OK': True, 'NL': 'a\nb'}, 'id': 'a@b'}})


@case('validate/all', ['validate', 'all'])
def _(b):
    world(b, codex={'x y': 'US-A', 'erin@example.com': 'US-B'}, agy={'api': {'base_url': 'https://x.example.com'}})
    b.write('.cca/agy.json', '{oops')


@case('validate/no-nodes', ['validate'])
def _(b):
    world(b, cfg={'nodes': []})


for f in ['last', 'synced', 'update', 'policy_declined', 'nodes', 'min_trust', 'router', 'nope', 'version']:
    @case('get/%s' % f, ['get', f])
    def _(b):
        world(b, cfg={'last': 'bob@example.com', 'synced': 'abc', 'policy_declined': True})

    @case('get/%s-codex' % f, ['get', f], tool='codex')
    def _(b):
        world(b, cfg={'last': 'bob@example.com', 'codex': {'ports': {}, 'last': 'erin@example.com'}}, codex={'erin@example.com': 'US-A'})


@case('set/synced', ['set', 'synced', 'deadbeef'])
def _(b):
    world(b)


@case('set/update-fresh-home', ['set', 'update', 'notify'])
def _(b):
    pass


@case('ports/pool', ['ports'])
def _(b):
    world(b)


@case('ports/named', ['ports', 'named'])
def _(b):
    world(b)


@case('ports/named-all', ['ports', 'named', 'all'])
def _(b):
    world(b, codex={'erin@example.com': 'US-A', '科(frank@example.com)': 'HK-A'}, agy={'gina@example.com': 'US-B'})


@case('ports/reordered', ['ports', 'named'])
def _(b):
    world(b, cfg={'ports': {'主号(alice@example.com)': 7901, 'bob@example.com': 7902, '走节点': 7903},
                  'order_base': ['bob-example-com-xxxxxx']})


@case('ports/big-pool', ['ports'])
def _(b):
    world(b, table={'u%d@example.com' % i: NODES[i % 3] for i in range(11)})


for name, argv, kw in [
    ('plain', ['accounts-json'], {}),
    ('all-tools', ['accounts-json'], {'codex': {'erin@example.com': 'US-A', '科(frank@example.com)': 'HK-A'}, 'agy': {'gina@example.com': 'US-B'}}),
    ('codex-tool', ['accounts-json'], {'codex': {'erin@example.com': 'US-A'}}),
    ('bad-table', ['accounts-json'], {'table': {'主号(alice@example.com)': 'NOPE'}}),
    ('bad-other-table', ['accounts-json'], {'codex': {'erin@example.com': 'NOPE'}}),
    ('extra-arg', ['accounts-json', 'x'], {}),
]:
    @case('accounts-json/' + name, argv, tool='codex' if name == 'codex-tool' else 'claude')
    def _(b, kw=kw):
        world(b, **kw)


@case('digest/plain', ['digest'], stdin='router code\nline 2\r\nline 3\n')
def _(b):
    world(b)


@case('digest/with-fill-and-codex', ['digest'], stdin='x')
def _(b):
    world(b, codex={'erin@example.com': 'HK-A'}, fill=FILL, cfg={'nodes': NODES + ['US-C']})


@case('digest/empty-table', ['digest'], stdin='')
def _(b):
    world(b, table={})


@case('router-env/full', ['router-env'])
def _(b):
    world(b, cfg={'local': {'client': 'builtin'}, 'route': 'local'})


@case('router-env/fresh', ['router-env'])
def _(b):
    pass


@case('router-env/odd-values', ['router-env'])
def _(b):
    world(b, cfg={'router': {'host': None, 'port': 0, 'user': '', 'key': "it's here"}, 'proxy_host': '', 'route': '', 'local': []})


@case('router-set/basic', ['router-set', '192.0.2.99', '2222', 'admin', '/k y'])
def _(b):
    world(b)


@case('router-set/bad-port', ['router-set', 'h', 'x22', 'u', 'k'])
def _(b):
    world(b)


for cc in ['cn', ' HK ', 'US', '', 'ru']:
    @case('restricted/%s' % (cc.strip() or 'empty'), ['restricted', cc])
    def _(b):
        pass


@case('name/alias', ['name', '主号(alice@example.com)'])
def _(b):
    pass


@case('name/empty', ['name', ''])
def _(b):
    pass


@case('email/logged-in', ['email', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.json('.cca/accounts/alice-example-com-xxxxxx/claude/.claude.json', {'oauthAccount': {'emailAddress': 'someone@example.com'}})


@case('email/not-logged-in', ['email', 'bob@example.com'])
def _(b):
    world(b)


@case('policy/merge-on-keys', ['keys'])
def _(b):
    world(b, policy=POLICY, fill=FILL)


@case('policy/merge-codex', ['keys'], tool='codex')
def _(b):
    world(b, policy=POLICY, fill=FILL)


@case('policy/readonly', ['keys'], env={'CCA_RO': '1'})
def _(b):
    world(b, policy=POLICY, fill=FILL)


@case('policy/alias-fill', ['keys'])
def _(b):
    world(b, table={'carol@example.com': 'US-A', 'bob@example.com': 'US-B'}, policy=dict(POLICY, claude={'carol@example.com': 'US-A'}))


@case('policy/renamed-node', ['keys'])
def _(b):
    world(b, cfg={'node_renames': {'OLD': 'MID', 'MID': 'US-B'}}, policy={'version': 1, 'claude': {'bob@example.com': 'OLD'}})


@case('policy/rows', ['policy-rows'])
def _(b):
    world(b, cfg={'policy_skip': ['dave@example.com'], 'node_renames': {'OLD-B': 'US-B'}},
          policy=dict(POLICY, claude=dict(POLICY['claude'], **{'bob@example.com': 'OLD-B', 'zed@example.com': 'US-A'})), fill=FILL)


@case('policy/rows-none', ['policy-rows'])
def _(b):
    world(b)


@case('policy/summary', ['policy-summary'])
def _(b):
    world(b, policy=POLICY)


@case('policy/summary-agy', ['policy-summary'])
def _(b):
    world(b, policy=dict(POLICY, agy={'g@example.com': 'US-A'}))


@case('policy/summary-none', ['policy-summary'])
def _(b):
    world(b)


for e in ['alice@example.com', ' ALICE@example.com ', 'nobody@example.com']:
    @case('policy/node-%s' % e.strip(), ['policy-node', e])
    def _(b):
        world(b, policy=POLICY)


@case('policy/repo', ['policy-repo'])
def _(b):
    world(b, cfg={'policy': 'example/cca-policy'})


@case('policy/repo-none', ['policy-repo'])
def _(b):
    pass


@case('policy/ask-yes', ['policy-ask'])
def _(b):
    world(b)


@case('policy/ask-configured', ['policy-ask'])
def _(b):
    world(b, cfg={'policy': 'example/cca-policy'})


@case('policy/ask-declined', ['policy-ask'])
def _(b):
    world(b, cfg={'policy_declined': True})


@case('policy/decline', ['policy-decline'])
def _(b):
    world(b, cfg={'policy': 'example/cca-policy'})


@case('policy/set', ['policy-set', 'example/cca-policy'])
def _(b):
    world(b, cfg={'policy_declined': True})


@case('policy/have', ['policy-have'])
def _(b):
    world(b, policy=POLICY)


@case('policy/forget', ['policy-forget', 'alice@example.com carol@example.com zed@example.com'])
def _(b):
    world(b, policy=POLICY, cfg={'policy_skip': ['old@example.com']})


@case('policy/forget-codex', ['policy-forget', 'erin@example.com'], tool='codex')
def _(b):
    world(b, policy=POLICY, codex={})


@case('nodes-import/server', ['nodes-import'])
def _(b):
    world(b, cfg={'nodes': [], 'router': {}}, table={}, policy=POLICY, fill=FILL)


@case('nodes-import/nothing', ['nodes-import'])
def _(b):
    world(b, cfg={'nodes': [], 'router': {}}, table={})
