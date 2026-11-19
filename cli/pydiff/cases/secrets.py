"""钥匙串和密钥文件（cli/src/cmd/secrets.rs）：api-key-out、secret-check / set / del / status、secrets-missing、secrets-forget、
key-show、key-forget、key-share。

真的钥匙串绝不能碰：这里的用例一律用 kcase 登记（不直接用 run.case），它先把假的 security 放进沙箱的 bin/（PATH 最前面，
/usr/bin 里真的那个轮不到）。假 security 照 tests/setup.sh 里回归用的那个写：条目存在 ~/keychain/<服务名>/<账户名>（文件内容
就是秘密），每次的命令行记在 ~/security.log，-i 从 stdin 读到的命令记在 ~/security.stdin。对拍连沙箱里的文件一起比，所以两边
交给 security 的参数、stdin、先后和次数差一点都看得出来。开关（沙箱里放一个同名的空文件，见 switch）：
    keychain_locked      存不进去（add 退出码 36；-i 照样返回 0，读回来比对才发现）
    keychain_unreadable  读不了（find 退出码 51）
    keychain_sticky      删不掉、却说删掉了（delete 一直返回 0：看最多删几次）
    keychain_mangle      存进去的不是给它的那个（读回来对不上）
密钥文件那一支（不是 macOS 时用的）用 env=FILE（CCA_KEYSTORE=file）：这时不该调 security，沙箱里不该出现 security.log。

用例里的 key 都是编的。夹具里只用假数据：节点 US-A…、路由器 192.0.2.10、邮箱和接口 example.com。

设备统计开着（映射里有 presence 一节）时记下、删掉 key 要经 keyshare 模块，单独配了邮箱的账号（config.json 的 mail.accounts）
清理时要经 mail 模块：这两块还没移植，cca-core 走到那里是 unimplemented!，这里没有它们的用例。"""
import hashlib
import json
import os
import re

from run import case

NODES = ['US-A', 'US-B', 'HK-A']
FILE = {'CCA_KEYSTORE': 'file'}
AT = [(r'"at": \d+\.\d+', '"at": <记下的时间>')]   # 官方账号记下备查的 key 带着记下的时间：每次不一样

SECURITY = r'''# 假的钥匙串（对拍用，见 cases/secrets.py 开头）
T="$HOME"; KC="$T/keychain"
printf 'SECURITY %s\n' "$*" >> "$T/security.log"
one() {
	[ $# -gt 0 ] || return 1
	cmd="$1"; shift
	svc=""; acct=""; hex=""; w=0; prev=""
	for a in "$@"; do
		case "$prev" in -s) svc="$a" ;; -a) acct="$a" ;; -X) hex="$a" ;; esac
		[ "$a" = -w ] && w=1
		prev="$a"
	done
	[ -n "$svc" ] || return 1
	if [ -n "$acct" ]; then f="$KC/$svc/$acct"; else f=$(ls "$KC/$svc/"* 2>/dev/null | head -n 1); fi   # 不给 -a：第一条
	case "$cmd" in
		add-generic-password)
			[ -f "$T/keychain_locked" ] && return 36
			mkdir -p "$KC/$svc"; f="$KC/$svc/${acct:-_}"
			printf '%s' "$hex" | xxd -r -p > "$f"
			[ -f "$T/keychain_mangle" ] && printf 'X' >> "$f"
			return 0 ;;
		find-generic-password)
			[ -f "$T/keychain_unreadable" ] && return 51
			[ -n "$f" ] && [ -f "$f" ] || return 44
			[ $w = 1 ] && { cat "$f"; echo; }
			return 0 ;;
		delete-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			[ -f "$T/keychain_sticky" ] && return 0
			rm -f "$f"; rmdir "$KC/$svc" 2>/dev/null
			return 0 ;;
	esac
	return 1
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do
		printf '%s\n' "$line" >> "$T/security.stdin"
		set -f; set -- $line; set +f
		one "$@"
	done
	exit 0   # 和真的一样：-i 里的命令失败了也返回 0
fi
one "$@"
'''


def kcase(name, argv, **kw):
    """和 run.case 一样，只是先把假的 security 放进沙箱"""
    def reg(setup):
        def with_fake(b):
            b.fake('security', SECURITY)
            setup(b)
        return case('secrets/' + name, argv, **kw)(with_fake)
    return reg


def ident(k):
    """账号目录名（PY_SRC 的 ident）"""
    s = re.sub(r'[^a-z0-9]+', '-', k.lower()).strip('-')[:20].strip('-') or 'acct'
    return '%s-%s' % (s, hashlib.sha1(k.encode('utf-8')).hexdigest()[:6])


ALICE, BOB = ident('alice@example.com'), ident('bob@example.com')
DIRECT = ident('直连')     # 没写 id 的第三方 API 账号：按键认
RELAY = ident('zjd')       # 写了 id 的：按 id 认
INTABLE = ident('表里有')
ERIN = ident('erin@example.com')
S1, S2, S3 = 'sk-test-AAAA1111', 'sk-test-BBBB2222', 'sk-test-CCCC3333'   # 短的：没有掩码
R1 = 'sk-ant-api03-REFtest0123456789abcdWXYZ'                             # 长的：掩码是 sk-ant-…WXYZ
R2 = 'sk-proj-REFtest9876543210zyxwvutsQRST'
TABLE = {
    '主号(alice@example.com)': 'US-A',
    'bob@example.com': 'US-B',
    '直连': {'base_url': 'https://api.example.com/anthropic', 'model': 'm-1'},
    '走节点': {'base_url': 'https://api2.example.com/v1/', 'node': 'HK-A', 'id': 'zjd'},
    '表里有': {'base_url': 'https://api3.example.com', 'key': 'sk-table-KEY-000111'},
}
CODEX = {'erin@example.com': 'US-A', 'kimi': {'base_url': 'https://api.example.com/v1'}}
KIMI = ident('kimi')
API, REF = 'cca-api-', 'cca-ref-'
XAPI, XREF = 'cca-codex-api-', 'cca-codex-ref-'
GAPI, GREF = 'cca-agy-api-', 'cca-agy-ref-'


def world(b, table=None, cfg=None, codex=None, agy=None, policy=None):
    """两个官方账号加三个第三方 API 账号（一个按键认、一个按 id 认、一个 key 写在表里）"""
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


def kc(b, svc, acct, secret):
    """假钥匙串里的一条"""
    b.write('keychain/%s/%s' % (svc, acct), secret)


def kf(b, svc, data):
    """一份密钥文件（data 是字典就写成 JSON，别的原样写）"""
    p = b.write('.cca/secrets/' + svc, json.dumps(data) + '\n' if isinstance(data, dict) else data)
    os.chmod(os.path.dirname(p), 0o700)


def switch(b, name):
    b.write(name, '')


def hint(host, s=''):
    """key_hints 里的一项（s 是掩码）"""
    return {'host': host, 'hint': s}


# --- secret-check ----------------------------------------------------------------------------------------------------
for name, data in [
    ('ok', S1), ('ok-padded', '  %s \n' % S1), ('ok-crlf', S1 + '\r\n'), ('ok-8', 'abcd-123'), ('ok-4096', 'k' * 4096),
    ('ok-all-marks', '!~!~!~!~'), ('ok-ctrl-padded', '\x1c%s\x1f' % S1), ('ok-wide-space-padded', '　%s\xa0' % S1),
    ('long-4097', 'k' * 4097), ('short-7', 'abc-123'), ('empty', ''), ('blank', ' \n\t'), ('inner-space', 'sk test AAAA1111'),
    ('inner-tab', 'sk-test\tAAAA1111'), ('two-lines', S1 + '\n' + S2), ('chinese', 'sk-中文-12345678'),
    ('not-utf8', b'sk-\xff\xfe-12345678'), ('nul', 'sk-test\x00AAAA1111'), ('del-char', 'sk-test\x7fAAAA1111'),
    ('zero-width', '​' + S1),
]:
    @kcase('secret-check/' + name, ['secret-check'], stdin=data)
    def _(b):
        pass


@kcase('secret-check/codex', ['secret-check'], stdin='short', tool='codex')
def _(b):
    pass


@kcase('secret-check/extra-arg', ['secret-check', 'x'], stdin=S1)
def _(b):
    pass


# --- api-key-out -----------------------------------------------------------------------------------------------------
@kcase('api-key-out/no-arg', ['api-key-out'])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('api-key-out/no-arg-codex', ['api-key-out'], tool='codex')
def _(b):
    world(b, codex=CODEX)


@kcase('api-key-out/no-arg-agy', ['api-key-out'], tool='agy')
def _(b):
    world(b)


@kcase('api-key-out/keychain', ['api-key-out', DIRECT])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, API + RELAY, 'api2.example.com', S2)


@kcase('api-key-out/by-id', ['api-key-out', RELAY])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, API + RELAY, 'api2.example.com', S2)


@kcase('api-key-out/key-of-account-with-id', ['api-key-out', ident('走节点')])   # 写了 id 的账号不按键认
def _(b):
    world(b)
    kc(b, API + ident('走节点'), 'api2.example.com', S2)


@kcase('api-key-out/table-key', ['api-key-out', INTABLE])
def _(b):
    world(b)
    kc(b, API + INTABLE, 'api3.example.com', S3)   # 表里写了 key：用表里的，不读钥匙串


@kcase('api-key-out/table-key-bad', ['api-key-out', ident('x')])
def _(b):
    world(b, table={'x': {'base_url': 'https://api.example.com', 'key': 'short'}})
    kc(b, API + ident('x'), 'api.example.com', S1)


@kcase('api-key-out/table-key-not-str', ['api-key-out', ident('x')])
def _(b):
    world(b, table={'x': {'base_url': 'https://api.example.com', 'key': 12345678}})


@kcase('api-key-out/table-key-newline', ['api-key-out', ident('x')])   # 正则的 $：结尾多一个换行也算合格，原样打出来
def _(b):
    world(b, table={'x': {'base_url': 'https://api.example.com', 'key': S1 + '\n'}})


@kcase('api-key-out/official-never', ['api-key-out', ALICE])   # 官方账号记下备查的 key 绝不给
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, API + ALICE, 'reference', R1)


@kcase('api-key-out/unknown', ['api-key-out', 'nobody-000000'])
def _(b):
    world(b)
    kc(b, API + 'nobody-000000', 'api.example.com', S1)   # 删掉的账号：钥匙串里还留着也不给


@kcase('api-key-out/other-provider', ['api-key-out', DIRECT])   # 换了服务商：旧服务商的 key 取不到
def _(b):
    world(b)
    kc(b, API + DIRECT, 'old.example.com', S1)


@kcase('api-key-out/unreadable', ['api-key-out', DIRECT])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)
    switch(b, 'keychain_unreadable')


@kcase('api-key-out/no-security', ['api-key-out', DIRECT], env={'PATH': '/nonexistent-cca-pydiff'})   # 起不了 security
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)


for name, stored in [('not-ascii', 'sk-中文-12345678'), ('too-short', 'short'), ('inner-space', 'sk test AAAA1111'),
                     ('not-utf8', b'sk-\xff\xfe-12345678'), ('padded', '  %s \t\n' % S1), ('empty', '')]:
    @kcase('api-key-out/stored-' + name, ['api-key-out', DIRECT])
    def _(b, stored=stored):
        world(b)
        kc(b, API + DIRECT, 'api.example.com', stored)


@kcase('api-key-out/empty-host', ['api-key-out', ident('x')])   # 没写地址：账户名是空的
def _(b):
    world(b, table={'x': {'base_url': ''}})
    kc(b, API + ident('x'), 'whatever', S1)


@kcase('api-key-out/odd-url', ['api-key-out', ident('x')])   # 不像网址：整串当账户名
def _(b):
    world(b, table={'x': {'base_url': 'api.example.com/v1/'}})
    kc(b, API + ident('x'), 'api.example.com', S1)


@kcase('api-key-out/url-with-port', ['api-key-out', ident('x')])
def _(b):
    world(b, table={'x': {'base_url': 'https://API.Example.com:8443/v1'}})
    kc(b, API + ident('x'), 'api.example.com', S1)


@kcase('api-key-out/empty-key-name', ['api-key-out', ident('')])   # 键是空字符串的那一项不给
def _(b):
    world(b, table={'': {'base_url': 'https://api.example.com'}})
    kc(b, API + ident(''), 'api.example.com', S1)


@kcase('api-key-out/bad-table-json', ['api-key-out', DIRECT])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"直连": ')


@kcase('api-key-out/table-not-object', ['api-key-out', DIRECT])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '["直连"]')


@kcase('api-key-out/no-table', ['api-key-out', DIRECT])
def _(b):
    pass


@kcase('api-key-out/two-args', ['api-key-out', DIRECT, 'x'])
def _(b):
    world(b)


@kcase('api-key-out/empty-arg', ['api-key-out', ''])
def _(b):
    world(b)


@kcase('api-key-out/same-id-twice', ['api-key-out', ident('same')])   # 两个账号写了同一个 id：给表里靠前的那个
def _(b):
    world(b, table={'后': 'US-A', '甲': {'base_url': 'https://a.example.com', 'id': 'same'}, '乙': {'base_url': 'https://b.example.com', 'id': 'same'}})
    kc(b, API + ident('same'), 'a.example.com', S1)
    kc(b, API + ident('same'), 'b.example.com', S2)


@kcase('api-key-out/codex', ['api-key-out', KIMI], tool='codex')
def _(b):
    world(b, codex=CODEX)
    kc(b, XAPI + KIMI, 'api.example.com', S1)
    kc(b, API + KIMI, 'api.example.com', S2)   # Claude 的同名条目不相干


@kcase('api-key-out/agy', ['api-key-out', KIMI], tool='agy')   # agy 没有第三方 API 账号，表里硬写了一个：照样按前缀找
def _(b):
    world(b, agy=CODEX)
    kc(b, GAPI + KIMI, 'api.example.com', S1)


@kcase('api-key-out/file', ['api-key-out', DIRECT], env=FILE)
def _(b):
    world(b)
    kf(b, API + DIRECT, {'v': 1, 'host': 'api.example.com', 'key': S1})
    kc(b, API + DIRECT, 'api.example.com', S2)   # 钥匙串里的不看


@kcase('api-key-out/file-other-provider', ['api-key-out', DIRECT], env=FILE)
def _(b):
    world(b)
    kf(b, API + DIRECT, {'v': 1, 'host': 'old.example.com', 'key': S1})


@kcase('api-key-out/file-missing', ['api-key-out', DIRECT], env=FILE)
def _(b):
    world(b)


@kcase('api-key-out/file-codex', ['api-key-out', KIMI], tool='codex', env=FILE)
def _(b):
    world(b, codex=CODEX)
    kf(b, XAPI + KIMI, {'v': 1, 'host': 'api.example.com', 'key': S1})


@kcase('api-key-out/file-store-off-value', ['api-key-out', DIRECT], env={'CCA_KEYSTORE': 'keychain'})   # 只认 file
def _(b):
    world(b)
    kf(b, API + DIRECT, {'v': 1, 'host': 'api.example.com', 'key': S1})
    kc(b, API + DIRECT, 'api.example.com', S2)


# --- secret-status ---------------------------------------------------------------------------------------------------
@kcase('secret-status/no-such-key', ['secret-status', 'nobody'])
def _(b):
    world(b)


@kcase('secret-status/null-value', ['secret-status', 'x'])
def _(b):
    world(b, table={'x': None})


@kcase('secret-status/official-none', ['secret-status', 'bob@example.com'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})


@kcase('secret-status/official-ref', ['secret-status', '主号(alice@example.com)'])   # 只看 config.json 的记录，不读钥匙串
def _(b):
    world(b, cfg={'ref_keys': [BOB, ALICE]})


@kcase('secret-status/official-fresh-home', ['secret-status', 'bob@example.com'])
def _(b):
    b.table({'bob@example.com': 'US-B'})


for name, value in [('string', 'x-%s-y' % ALICE), ('dict', {ALICE: 1}), ('number', 5), ('true', True), ('float', 0.5),
                    ('zero', 0), ('list-of-lists', [[ALICE]]), ('list-mixed', [1, None, ALICE]), ('empty-string', '')]:
    @kcase('secret-status/ref-keys-' + name, ['secret-status', '主号(alice@example.com)'])   # ref_keys 的类型不对
    def _(b, value=value):
        world(b, cfg={'ref_keys': value})


@kcase('secret-status/official-bad-config', ['secret-status', 'bob@example.com'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{"ref_keys": [')


@kcase('secret-status/official-codex', ['secret-status', 'erin@example.com'], tool='codex')   # Codex 的记在 codex 一节
def _(b):
    world(b, cfg={'ref_keys': [ERIN], 'codex': {'ports': {}, 'last': '', 'ref_keys': [ERIN]}}, codex=CODEX)


@kcase('secret-status/official-codex-top-level-only', ['secret-status', 'erin@example.com'], tool='codex')
def _(b):
    world(b, cfg={'ref_keys': [ERIN]}, codex=CODEX)


@kcase('secret-status/official-agy', ['secret-status', 'gina@example.com'], tool='agy')
def _(b):
    world(b, cfg={'agy': {'ref_keys': [ident('gina@example.com')]}}, agy={'gina@example.com': 'US-A'})


@kcase('secret-status/value-not-node', ['secret-status', 'x'])   # 值不是字符串也不是对象：照官方账号看
def _(b):
    world(b, table={'x': 5}, cfg={'ref_keys': [ident('x')]})


@kcase('secret-status/table', ['secret-status', '表里有'])
def _(b):
    world(b)


@kcase('secret-status/api-none', ['secret-status', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})


@kcase('secret-status/keychain-not-booked', ['secret-status', '直连'])   # config.json 里没记着：不补掩码
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-backfill', ['secret-status', '直连'])   # 旧版本存的：补上掩码和域名
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-backfill-short', ['secret-status', '直连'])   # 短的 key 没有掩码：hint 是空字符串
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('secret-status/keychain-hint-current', ['secret-status', '直连'])   # 已经对得上：不动 config.json
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: {'host': 'api.example.com', 'hint': 'sk-ant-…WXYZ', 'more': 1}}})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-hint-stale-host', ['secret-status', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT, RELAY], 'key_hints': {RELAY: hint('api2.example.com'),
                                                              DIRECT: {'host': 'old.example.com', 'hint': 'sk-ant-…WXYZ', 'at': 5}}})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-hint-stale-mask', ['secret-status', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('api.example.com', 'sk-old-…0000')}})
    kc(b, API + DIRECT, 'api.example.com', R1)


for name, value in [('hints-not-dict', ['x']), ('hint-not-dict', {DIRECT: 'sk-ant-…WXYZ'}), ('hint-fields-odd', {DIRECT: {'host': 1, 'hint': None}})]:
    @kcase('secret-status/keychain-' + name, ['secret-status', '直连'])
    def _(b, value=value):
        world(b, cfg={'api_keys': [DIRECT], 'key_hints': value})
        kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-api-keys-string', ['secret-status', '直连'])   # api_keys 是字符串：按“是不是它的一段”算记着，再拆成字符
def _(b):
    world(b, cfg={'api_keys': DIRECT})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-api-keys-number', ['secret-status', '直连'])
def _(b):
    world(b, cfg={'api_keys': 7})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-bad-config', ['secret-status', '直连'])   # 读到 key 以后才发现 config.json 写坏了
def _(b):
    world(b)
    b.write('.cca/config.json', '{oops')
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/keychain-unreadable', ['secret-status', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', R1)
    switch(b, 'keychain_unreadable')


@kcase('secret-status/keychain-codex', ['secret-status', 'kimi'], tool='codex')
def _(b):
    world(b, cfg={'codex': {'ports': {}, 'last': '', 'api_keys': [KIMI]}}, codex=CODEX)
    kc(b, XAPI + KIMI, 'api.example.com', R1)


@kcase('secret-status/keychain-codex-no-section', ['secret-status', 'kimi'], tool='codex')
def _(b):
    world(b, cfg={'api_keys': [KIMI]}, codex=CODEX)
    kc(b, XAPI + KIMI, 'api.example.com', R1)


@kcase('secret-status/file', ['secret-status', '直连'], env=FILE)
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kf(b, API + DIRECT, {'v': 1, 'host': 'api.example.com', 'key': R1})


@kcase('secret-status/file-none', ['secret-status', '直连'], env=FILE)
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-status/no-args', ['secret-status'])
def _(b):
    world(b)


@kcase('secret-status/two-args', ['secret-status', '直连', 'x'])
def _(b):
    world(b)


@kcase('secret-status/bad-table', ['secret-status', '直连'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '[]')


# --- secrets-missing -------------------------------------------------------------------------------------------------
@kcase('secrets-missing/some', ['secrets-missing'])
def _(b):
    world(b)
    kc(b, API + RELAY, 'api2.example.com', S2)


@kcase('secrets-missing/none', ['secrets-missing'])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, API + RELAY, 'api2.example.com', S2)


@kcase('secrets-missing/all', ['secrets-missing'])   # 换了服务商的、存的不是可见 ASCII 的都算没有
def _(b):
    world(b)
    kc(b, API + DIRECT, 'old.example.com', S1)
    kc(b, API + RELAY, 'api2.example.com', 'has space 123')


@kcase('secrets-missing/no-api-accounts', ['secrets-missing'])
def _(b):
    world(b, table={'bob@example.com': 'US-B'})


@kcase('secrets-missing/unreadable', ['secrets-missing'])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)
    switch(b, 'keychain_unreadable')


@kcase('secrets-missing/agy', ['secrets-missing'], tool='agy')
def _(b):
    world(b, agy=dict(CODEX, other={'base_url': 'https://api9.example.com', 'key': S3}))
    kc(b, XAPI + KIMI, 'api.example.com', S1)   # Codex 的条目不算


@kcase('secrets-missing/codex', ['secrets-missing'], tool='codex')
def _(b):
    world(b, codex=dict(CODEX, other={'base_url': 'https://api9.example.com', 'id': 'o9'}))
    kc(b, XAPI + ident('o9'), 'api9.example.com', S1)


@kcase('secrets-missing/file', ['secrets-missing'], env=FILE)
def _(b):
    world(b)
    kf(b, API + DIRECT, {'v': 1, 'host': 'api.example.com', 'key': S1})
    kf(b, API + RELAY, {'v': 1, 'host': 'old.example.com', 'key': S2})


@kcase('secrets-missing/bad-table', ['secrets-missing'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{')


@kcase('secrets-missing/extra-arg', ['secrets-missing', 'x'])
def _(b):
    world(b)


# --- secret-set ------------------------------------------------------------------------------------------------------
@kcase('secret-set/api', ['secret-set', '直连'], stdin=S1)
def _(b):
    world(b)


@kcase('secret-set/api-long-key-padded-input', ['secret-set', '直连'], stdin='  %s\n' % R1)
def _(b):
    world(b)


@kcase('secret-set/api-by-id', ['secret-set', '走节点'], stdin=S2)
def _(b):
    world(b)


@kcase('secret-set/api-replace', ['secret-set', '直连'], stdin=S2)   # 已经存过：换掉，记录原位更新
def _(b):
    world(b, cfg={'api_keys': [RELAY, DIRECT, 'zzz-000000'], 'ref_keys': [ALICE],
                  'key_hints': {DIRECT: hint('api.example.com', 'sk-ant-…WXYZ'), RELAY: hint('api2.example.com'), ALICE: hint('reference')}})
    kc(b, API + DIRECT, 'api.example.com', R1)
    kc(b, API + RELAY, 'api2.example.com', S1)


@kcase('secret-set/api-other-provider', ['secret-set', '直连'], stdin=S2)   # 换了服务商：旧服务商的那一把先删掉
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('old.example.com')}})
    kc(b, API + DIRECT, 'old.example.com', S1)
    kc(b, API + DIRECT, 'older.example.com', S3)


@kcase('secret-set/api-has-table-key', ['secret-set', '表里有'], stdin=S2)   # 表里写着 key 也照存
def _(b):
    world(b)


@kcase('secret-set/official', ['secret-set', '主号(alice@example.com)'], stdin=R1, mask=AT)
def _(b):
    world(b)


@kcase('secret-set/official-short', ['secret-set', 'bob@example.com'], stdin=S1, mask=AT)
def _(b):
    world(b)


@kcase('secret-set/official-replace', ['secret-set', '主号(alice@example.com)'], stdin=R2, mask=AT)
def _(b):
    world(b, cfg={'ref_keys': [ALICE, BOB], 'key_hints': {ALICE: {'host': 'reference', 'hint': 'sk-ant-…WXYZ', 'at': 1700000000.5, 'from': 'bob-mini'},
                                                           BOB: hint('reference')}})
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, REF + BOB, 'reference', S1)


@kcase('secret-set/official-mapped-no-presence', ['secret-set', '主号(alice@example.com)'], stdin=R1, mask=AT)   # 映射里的账号，没有设备统计：不说同步
def _(b):
    world(b, policy={'version': 1, 'claude': {'alice@example.com': 'US-A'}})


@kcase('secret-set/official-presence-turned-off', ['secret-set', '主号(alice@example.com)'], stdin=R1, mask=AT)   # config.json 里关掉了设备统计
def _(b):
    world(b, cfg={'presence': False},
          policy={'version': 1, 'claude': {'alice@example.com': 'US-A'},
                  'presence': {'url': 'https://192.0.2.20:18443', 'pins': ['sha256//' + 'A' * 43 + '='], 'salt': 'ab' * 16}})


@kcase('secret-set/official-policy-not-object', ['secret-set', '主号(alice@example.com)'], stdin=R1, mask=AT)
def _(b):
    world(b)
    b.write('.cca/policy/policy.json', '["presence"]')


@kcase('secret-set/official-policy-presence-null', ['secret-set', '主号(alice@example.com)'], stdin=R1, mask=AT)
def _(b):
    world(b, policy={'version': 1, 'claude': {'alice@example.com': 'US-A'}, 'presence': None})


WEIRD = 'sk-{}%s"\'\\$(x)`<>|&*?[]~!#^=+,./:@_;'   # 可见的 ASCII 里各种有讲究的符号：原样存、原样取


@kcase('secret-set/api-weird-chars', ['secret-set', '直连'], stdin=WEIRD)
def _(b):
    world(b)


@kcase('secret-set/api-4096', ['secret-set', '直连'], stdin='sk-' + 'Kk9_' * 1023 + 'k')
def _(b):
    world(b)


@kcase('secret-set/api-input-crlf', ['secret-set', '直连'], stdin=S1 + '\r\n')
def _(b):
    world(b)


@kcase('secret-set/value-null', ['secret-set', 'x'], stdin=R1, mask=AT)   # 值是 null：照官方账号记下备查的存
def _(b):
    world(b, table={'x': None})


@kcase('secret-set/no-such-account', ['secret-set', 'nobody'], stdin=S1)
def _(b):
    world(b)


for name, data in [('bad-short', 'short'), ('bad-space', 'sk test AAAA1111'), ('bad-empty', ''), ('bad-chinese', 'sk-中文-12345678')]:
    @kcase('secret-set/' + name, ['secret-set', '直连'], stdin=data)   # 不合格的 key：钥匙串一下都不碰
    def _(b):
        world(b, cfg={'api_keys': [DIRECT]})
        kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('secret-set/locked', ['secret-set', '直连'], stdin=S2)   # 存不进去：旧的已经删了，记录也去掉
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('api.example.com')}})
    kc(b, API + DIRECT, 'api.example.com', S1)
    switch(b, 'keychain_locked')


@kcase('secret-set/locked-official', ['secret-set', '主号(alice@example.com)'], stdin=R2)
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})
    kc(b, REF + ALICE, 'reference', R1)
    switch(b, 'keychain_locked')


@kcase('secret-set/mangled', ['secret-set', '直连'], stdin=S2)   # 存进去的读回来对不上
def _(b):
    world(b)
    switch(b, 'keychain_mangle')


@kcase('secret-set/unreadable', ['secret-set', '直连'], stdin=S2)   # 存了但读不回来
def _(b):
    world(b)
    switch(b, 'keychain_unreadable')


@kcase('secret-set/no-security', ['secret-set', '直连'], stdin=S2, env={'PATH': '/nonexistent-cca-pydiff'})
def _(b):
    world(b)


@kcase('secret-set/sticky-old-key', ['secret-set', '直连'], stdin=S2)   # 旧的删不掉：最多删 20 次
def _(b):
    world(b)
    kc(b, API + DIRECT, 'old.example.com', S1)
    switch(b, 'keychain_sticky')


for name, url in [('empty-host', ''), ('host-with-space', 'https://api example.com/v1'), ('host-with-wide-space', 'https://api　example.com/v1')]:
    @kcase('secret-set/' + name, ['secret-set', 'x'], stdin=S1)   # 账户名是空的、带空白：不存（旧的也不删）
    def _(b, url=url):
        world(b, table={'x': {'base_url': url}}, cfg={'api_keys': [ident('x')]})
        kc(b, API + ident('x'), 'old.example.com', S2)


@kcase('secret-set/host-not-ascii', ['secret-set', 'x'], stdin=S1)   # 钥匙串那一支：security -i 的命令要是 ASCII，原实现在这里出错退出（旧的已经删了）
def _(b):
    world(b, table={'x': {'base_url': 'https://api.例子.com/v1'}}, cfg={'api_keys': [ident('x')]})
    kc(b, API + ident('x'), 'old.example.com', S2)


@kcase('secret-set/host-not-ascii-file', ['secret-set', 'x'], stdin=S1, env=FILE)   # 密钥文件那一支存得进去
def _(b):
    world(b, table={'x': {'base_url': 'https://api.例子.com/v1'}})


@kcase('secret-set/bad-config', ['secret-set', '直连'], stdin=S2)   # config.json 写坏了：删旧 key 之后才发现
def _(b):
    world(b)
    b.write('.cca/config.json', '{"api_keys": [}')
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('secret-set/config-not-object', ['secret-set', '直连'], stdin=S2)
def _(b):
    world(b)
    b.write('.cca/config.json', '[1, 2]')


@kcase('secret-set/no-config', ['secret-set', '直连'], stdin=S2)   # 没有 config.json：照默认值建一份
def _(b):
    b.table(TABLE)


for name, value in [('string', 'ab'), ('dict', {'k2': 1, 'k1': 2}), ('number', 3), ('true', True), ('list-of-numbers', [3, 1, 2]),
                    ('list-with-null', [None]), ('list-with-list', [['x']]), ('list-dup', ['b', 'a', 'b'])]:
    @kcase('secret-set/api-keys-' + name, ['secret-set', '直连'], stdin=S2)   # api_keys 的类型不对
    def _(b, value=value):
        world(b, cfg={'api_keys': value})


@kcase('secret-set/key-hints-not-dict', ['secret-set', '直连'], stdin=S2)
def _(b):
    world(b, cfg={'key_hints': 'oops'})


@kcase('secret-set/bad-table', ['secret-set', '直连'], stdin=S2)
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"直连": {')


@kcase('secret-set/no-args', ['secret-set'], stdin=S2)
def _(b):
    world(b)


@kcase('secret-set/two-args', ['secret-set', '直连', 'x'], stdin=S2)
def _(b):
    world(b)


@kcase('secret-set/codex-api', ['secret-set', 'kimi'], stdin=S1, tool='codex')
def _(b):
    world(b, cfg={'api_keys': [KIMI]}, codex=CODEX)
    kc(b, API + KIMI, 'api.example.com', S3)   # Claude 的同名条目和记录不动


@kcase('secret-set/codex-official', ['secret-set', 'erin@example.com'], stdin=R1, tool='codex', mask=AT)
def _(b):
    world(b, cfg={'codex': {'ports': {'erin@example.com': 7903}, 'last': 'erin@example.com', 'ref_keys': ['zzz-000000']}}, codex=CODEX)


@kcase('secret-set/agy-official', ['secret-set', 'gina@example.com'], stdin=R1, tool='agy', mask=AT)
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@kcase('secret-set/agy-api', ['secret-set', 'kimi'], stdin=S1, tool='agy')
def _(b):
    world(b, agy=CODEX)


@kcase('secret-set/file-api', ['secret-set', '直连'], stdin=S1, env=FILE)
def _(b):
    world(b)


@kcase('secret-set/file-official', ['secret-set', '主号(alice@example.com)'], stdin=R1, env=FILE, mask=AT)
def _(b):
    world(b)


@kcase('secret-set/file-replace', ['secret-set', '直连'], stdin=S2, env=FILE)   # 已有的换掉；目录权限不对的改成 700
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('old.example.com')}})
    kf(b, API + DIRECT, {'v': 1, 'host': 'old.example.com', 'key': S1})
    kf(b, API + RELAY, {'v': 1, 'host': 'api2.example.com', 'key': S3})
    os.chmod(b.path('.cca/secrets'), 0o755)


@kcase('secret-set/file-dir-readonly', ['secret-set', '直连'], stdin=S2, env=FILE)   # 目录是 500：改回 700 再存
def _(b):
    world(b)
    os.makedirs(b.path('.cca/secrets'))
    os.chmod(b.path('.cca/secrets'), 0o500)


@kcase('secret-set/file-over-symlink', ['secret-set', '直连'], stdin=S2, env=FILE)   # 原来是个链接：换成普通文件，链接指向的不动
def _(b):
    world(b)
    b.write('elsewhere.json', '{"v": 1, "host": "api.example.com", "key": "%s"}\n' % S1)
    os.makedirs(b.path('.cca/secrets'))
    os.symlink(b.path('elsewhere.json'), b.path('.cca/secrets/' + API + DIRECT))


@kcase('secret-set/file-dir-dangling', ['secret-set', '直连'], stdin=S2, env=FILE)   # 存不进去：secrets 是指向不存在的地方的链接
def _(b):
    world(b)
    os.symlink('/nonexistent-cca-pydiff/secrets', b.path('.cca/secrets'))


@kcase('secret-set/file-dir-is-file', ['secret-set', '直连'], stdin=S2, env=FILE)   # secrets 是个文件：删旧 key 那一步就出错
def _(b):
    world(b)
    b.write('.cca/secrets', 'not a directory')


@kcase('secret-set/file-target-is-dir', ['secret-set', '直连'], stdin=S2, env=FILE)   # 密钥文件的位置上是个目录
def _(b):
    world(b)
    os.makedirs(b.path('.cca/secrets/' + API + DIRECT))


@kcase('secret-set/file-codex', ['secret-set', 'erin@example.com'], stdin=R2, tool='codex', env=FILE, mask=AT)
def _(b):
    world(b, codex=CODEX)


@kcase('secret-set/file-cca-home-elsewhere', ['secret-set', '直连'], stdin=S1, env=dict(FILE, CCA_HOME='alt/cca'))   # CCA_HOME 换了地方：密钥文件跟着走
def _(b):
    world(b)   # ~/.cca 里的这一套不该被碰
    b.json('alt/cca/accounts.json', {'直连': {'base_url': 'https://api9.example.com/v1'}})


@kcase('key-show/cca-home-elsewhere', ['key-show', '直连'], env={'CCA_HOME': 'alt/cca'})
def _(b):
    world(b)
    b.json('alt/cca/accounts.json', {'直连': {'base_url': 'https://api9.example.com/v1'}})
    b.json('alt/cca/config.json', {'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api9.example.com', R1)
    kc(b, API + DIRECT, 'api.example.com', S1)


# --- secret-del ------------------------------------------------------------------------------------------------------
@kcase('secret-del/api', ['secret-del', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT, RELAY], 'key_hints': {DIRECT: hint('api.example.com'), RELAY: hint('api2.example.com')}})
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, API + DIRECT, 'old.example.com', S3)
    kc(b, API + RELAY, 'api2.example.com', S2)


@kcase('secret-del/api-last', ['secret-del', '直连'])   # 最后一个：api_keys 留着空列表，key_hints 去掉
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('api.example.com')}})
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('secret-del/official', ['secret-del', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE, BOB], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ'), BOB: hint('reference')}})
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, REF + BOB, 'reference', S1)


@kcase('secret-del/official-last', ['secret-del', '主号(alice@example.com)'])   # 最后一个：ref_keys 整项去掉
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('secret-del/both', ['secret-del', '主号(alice@example.com)'])   # 两种都记着（账号从 API 改成了官方）
def _(b):
    world(b, cfg={'api_keys': [ALICE], 'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference')}})
    kc(b, API + ALICE, 'api.example.com', S1)
    kc(b, REF + ALICE, 'reference', R1)


@kcase('secret-del/nothing', ['secret-del', 'bob@example.com'])
def _(b):
    world(b)


@kcase('secret-del/booked-but-gone', ['secret-del', '直连'])   # 钥匙串里已经没有了，记录照样去掉
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('api.example.com')}})


@kcase('secret-del/not-in-table', ['secret-del', 'gone'])
def _(b):
    world(b, cfg={'api_keys': [ident('gone')]})
    kc(b, API + ident('gone'), 'api.example.com', S1)


@kcase('secret-del/by-id', ['secret-del', '走节点'])
def _(b):
    world(b, cfg={'api_keys': [RELAY]})
    kc(b, API + RELAY, 'api2.example.com', S2)
    kc(b, API + ident('走节点'), 'api2.example.com', S3)


@kcase('secret-del/bad-table-loses-id', ['secret-del', '走节点'])   # 账号表读不了：认不出 id，按键算目录名
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"走节点": ')
    kc(b, API + RELAY, 'api2.example.com', S2)
    kc(b, API + ident('走节点'), 'api2.example.com', S3)


@kcase('secret-del/sticky', ['secret-del', '直连'])   # 删不掉却一直说删掉了：每个服务名最多 20 次
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, REF + DIRECT, 'reference', R1)
    switch(b, 'keychain_sticky')


@kcase('secret-del/no-security', ['secret-del', '直连'], env={'PATH': '/nonexistent-cca-pydiff'})
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})


@kcase('secret-del/bad-config', ['secret-del', '直连'])
def _(b):
    world(b)
    b.write('.cca/config.json', 'null x')
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('secret-del/no-config', ['secret-del', '直连'])
def _(b):
    b.table(TABLE)
    kc(b, API + DIRECT, 'api.example.com', S1)


for name, value in [('string', DIRECT + '!'), ('dict', {DIRECT: 1, 'other': 2}), ('number', 3), ('list-mixed', [DIRECT, 5]),
                    ('list-two-numbers-and-me', [2, DIRECT, 1]), ('list-with-null', [DIRECT, None]), ('list-with-list', [DIRECT, []])]:
    @kcase('secret-del/api-keys-' + name, ['secret-del', '直连'])
    def _(b, value=value):
        world(b, cfg={'api_keys': value})
        kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('secret-del/codex', ['secret-del', 'kimi'], tool='codex')
def _(b):
    world(b, cfg={'api_keys': [KIMI], 'codex': {'ports': {}, 'last': '', 'api_keys': [KIMI], 'key_hints': {KIMI: hint('api.example.com')}}},
          codex=CODEX)
    kc(b, XAPI + KIMI, 'api.example.com', S1)
    kc(b, API + KIMI, 'api.example.com', S2)


@kcase('secret-del/codex-section-emptied', ['secret-del', 'erin@example.com'], tool='codex')   # codex 一节空了就整个去掉
def _(b):
    world(b, cfg={'codex': {'ports': {}, 'last': '', 'ref_keys': [ERIN], 'key_hints': {ERIN: hint('reference')}}}, codex=CODEX)
    kc(b, XREF + ERIN, 'reference', R1)


@kcase('secret-del/agy', ['secret-del', 'gina@example.com'], tool='agy')
def _(b):
    world(b, cfg={'agy': {'ports': {'gina@example.com': 7903}, 'last': '', 'ref_keys': [ident('gina@example.com')]}},
          agy={'gina@example.com': 'US-A'})
    kc(b, GREF + ident('gina@example.com'), 'reference', R1)


@kcase('secret-del/file', ['secret-del', '直连'], env=FILE)
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'ref_keys': [DIRECT]})
    kf(b, API + DIRECT, {'v': 1, 'host': 'old.example.com', 'key': S1})   # 不管哪个服务商的都删
    kf(b, REF + DIRECT, 'not json at all')
    kf(b, API + RELAY, {'v': 1, 'host': 'api2.example.com', 'key': S2})
    kc(b, API + DIRECT, 'api.example.com', S3)   # 钥匙串不碰


@kcase('secret-del/file-nothing', ['secret-del', '直连'], env=FILE)
def _(b):
    world(b)


@kcase('secret-del/file-is-dir', ['secret-del', '直连'], env=FILE)   # 密钥文件的位置上是个目录：删不了，原实现在这里出错退出
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    os.makedirs(b.path('.cca/secrets/' + API + DIRECT))


@kcase('secret-del/file-dangling-link', ['secret-del', '直连'], env=FILE)   # 是个断了的链接：链接本身删掉
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    os.makedirs(b.path('.cca/secrets'))
    os.symlink('/nonexistent-cca-pydiff/key', b.path('.cca/secrets/' + API + DIRECT))


@kcase('secret-del/empty-arg', ['secret-del', ''])
def _(b):
    world(b, cfg={'api_keys': [ident('')]})
    kc(b, API + ident(''), 'api.example.com', S1)


@kcase('secret-del/no-args', ['secret-del'])
def _(b):
    world(b)


@kcase('secret-del/two-args', ['secret-del', '直连', 'x'])
def _(b):
    world(b)


# --- key-forget ------------------------------------------------------------------------------------------------------
@kcase('key-forget/official', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'ref_keys': [ALICE, BOB],
                  'key_hints': {ALICE: {'host': 'reference', 'hint': 'sk-ant-…WXYZ', 'at': 1700000000.5}, BOB: hint('reference'), DIRECT: hint('api.example.com')}})
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, REF + BOB, 'reference', S1)
    kc(b, API + DIRECT, 'api.example.com', S2)


@kcase('key-forget/official-last', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-forget/official-nothing', ['key-forget', 'bob@example.com'])
def _(b):
    world(b)


@kcase('key-forget/api-account', ['key-forget', '直连'])   # 第三方 API 账号：它启动要用的那一把不删
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'ref_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, REF + DIRECT, 'reference', R1)


@kcase('key-forget/mapped-no-presence', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE]}, policy={'version': 1, 'claude': {'alice@example.com': 'US-A'}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-forget/presence-turned-off', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'presence': False, 'ref_keys': [ALICE]},
          policy={'version': 1, 'claude': {'alice@example.com': 'US-A'},
                  'presence': {'url': 'https://192.0.2.20:18443', 'pins': ['sha256//' + 'A' * 43 + '='], 'salt': 'ab' * 16}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-forget/no-such-account', ['key-forget', 'nobody'])
def _(b):
    world(b, cfg={'ref_keys': [ident('nobody')]})
    kc(b, REF + ident('nobody'), 'reference', R1)


@kcase('key-forget/bad-config', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{"ref_keys": ')
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-forget/bad-table', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '5')


@kcase('key-forget/sticky', ['key-forget', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kc(b, REF + ALICE, 'reference', R1)
    switch(b, 'keychain_sticky')


@kcase('key-forget/codex', ['key-forget', 'erin@example.com'], tool='codex')
def _(b):
    world(b, cfg={'ref_keys': [ERIN], 'codex': {'ports': {}, 'last': '', 'ref_keys': [ERIN, 'zzz-000000'], 'key_hints': {ERIN: hint('reference')}}},
          codex=CODEX)
    kc(b, XREF + ERIN, 'reference', R1)
    kc(b, REF + ERIN, 'reference', R2)


@kcase('key-forget/agy', ['key-forget', 'gina@example.com'], tool='agy')
def _(b):
    world(b, cfg={'agy': {'ref_keys': [ident('gina@example.com')]}}, agy={'gina@example.com': 'US-A'})
    kc(b, GREF + ident('gina@example.com'), 'reference', R1)


@kcase('key-forget/file', ['key-forget', '主号(alice@example.com)'], env=FILE)
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})
    kf(b, REF + ALICE, {'v': 1, 'host': 'reference', 'key': R1})
    kf(b, API + ALICE, {'v': 1, 'host': 'api.example.com', 'key': S1})


@kcase('key-forget/no-args', ['key-forget'])
def _(b):
    world(b)


@kcase('key-forget/two-args', ['key-forget', 'bob@example.com', 'x'])
def _(b):
    world(b)


# --- key-share -------------------------------------------------------------------------------------------------------
@kcase('key-share/official', ['key-share', '主号(alice@example.com)'])
def _(b):
    world(b)


@kcase('key-share/mapped-no-presence', ['key-share', '主号(alice@example.com)'])
def _(b):
    world(b, policy={'version': 1, 'claude': {'alice@example.com': 'US-A'}})


@kcase('key-share/presence-turned-off', ['key-share', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'presence': False},
          policy={'version': 1, 'claude': {'alice@example.com': 'US-A'},
                  'presence': {'url': 'https://192.0.2.20:18443', 'pins': ['sha256//' + 'A' * 43 + '='], 'salt': 'ab' * 16}})


@kcase('key-share/bad-policy', ['key-share', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.write('.cca/policy/policy.json', '{"presence": {')


@kcase('key-share/bad-config', ['key-share', '主号(alice@example.com)'])   # config.json 读不了也不碍事（只看有没有关掉）
def _(b):
    world(b)
    b.write('.cca/config.json', '{"presence": ')


@kcase('key-share/api-account', ['key-share', '直连'])
def _(b):
    world(b)


@kcase('key-share/no-such-account', ['key-share', 'nobody'])
def _(b):
    world(b)


@kcase('key-share/bad-table', ['key-share', '直连'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '{"a": 1 "b": 2}')


@kcase('key-share/codex', ['key-share', 'erin@example.com'], tool='codex')
def _(b):
    world(b, codex=CODEX, policy={'version': 1, 'codex': {'erin@example.com': 'US-A'}})


@kcase('key-share/agy', ['key-share', 'gina@example.com'], tool='agy')
def _(b):
    world(b, agy={'gina@example.com': 'US-A'})


@kcase('key-share/no-args', ['key-share'])
def _(b):
    world(b)


@kcase('key-share/two-args', ['key-share', '直连', 'x'])
def _(b):
    world(b)


# --- secrets-forget --------------------------------------------------------------------------------------------------
GONE_A, GONE_R, GONE_R2 = ident('gone-api'), ident('gone@example.com'), ident('gone2@example.com')
BOOKS = {'api_keys': [RELAY, GONE_A, DIRECT], 'ref_keys': [GONE_R2, ALICE, GONE_R],
         'key_hints': {GONE_A: hint('api.example.com'), DIRECT: hint('api.example.com'), ALICE: hint('reference', 'sk-ant-…WXYZ'),
                       GONE_R: {'host': 'reference', 'hint': 'sk-proj-…QRST', 'at': 1700000000.5}, GONE_R2: hint('reference')}}


def books(b):
    """记着五把 key：两个还在的第三方 API 账号、一个还在的官方账号，一个删掉了的 API 账号、两个删掉了的官方账号"""
    kc(b, API + DIRECT, 'api.example.com', S1)
    kc(b, API + RELAY, 'api2.example.com', S2)
    kc(b, API + GONE_A, 'api.example.com', S3)
    kc(b, API + GONE_A, 'old.example.com', S3)
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, REF + GONE_R, 'reference', R2)
    kc(b, REF + GONE_R2, 'reference', S1)


@kcase('secrets-forget/nothing-booked', ['secrets-forget'])
def _(b):
    world(b)
    kc(b, API + GONE_A, 'api.example.com', S3)   # 没记着的不删


@kcase('secrets-forget/all-kept', ['secrets-forget'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT, RELAY], 'ref_keys': [ALICE]})
    books(b)


@kcase('secrets-forget/gone', ['secrets-forget'])   # 删掉的账号：key 清掉；官方账号记下备查的清掉时说一声（带掩码）
def _(b):
    world(b, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/gone-readonly', ['secrets-forget'], env={'CCA_RO': '1'})
def _(b):
    world(b, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/all', ['secrets-forget', 'all'])   # cca restore：全部清掉，不说
def _(b):
    world(b, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/other-scope', ['secrets-forget', 'ALL'])   # 只认 all
def _(b):
    world(b, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/kind-changed', ['secrets-forget'])   # 账号换了种类：官方账号名下的 API key、API 账号名下记下备查的都清掉
def _(b):
    world(b, cfg={'api_keys': [ALICE, DIRECT], 'ref_keys': [DIRECT, ALICE], 'key_hints': {DIRECT: hint('reference', 'sk-ant-…WXYZ')}})
    kc(b, API + ALICE, 'api.example.com', S1)
    kc(b, REF + ALICE, 'reference', R1)
    kc(b, API + DIRECT, 'api.example.com', S2)
    kc(b, REF + DIRECT, 'reference', R1)


for name, value in [('no-hints', None), ('hints-not-dict', [GONE_R]), ('hint-not-dict', {GONE_R: 'sk-…'}), ('hint-not-str', {GONE_R: {'hint': 5}}),
                    ('hint-empty', {GONE_R: {'hint': ''}})]:
    @kcase('secrets-forget/ref-' + name, ['secrets-forget'])   # 清掉时说的那句话里的掩码
    def _(b, value=value):
        world(b, cfg=dict({'ref_keys': [GONE_R]}, **({} if value is None else {'key_hints': value})))
        kc(b, REF + GONE_R, 'reference', R2)


@kcase('secrets-forget/sticky', ['secrets-forget'])
def _(b):
    world(b, cfg={'api_keys': [GONE_A]})
    kc(b, API + GONE_A, 'api.example.com', S3)
    switch(b, 'keychain_sticky')


@kcase('secrets-forget/empty-table', ['secrets-forget'])
def _(b):
    world(b, table={}, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/policy-merge-first', ['secrets-forget'])   # ctx 先按映射对账（说在 stderr 上），再清
def _(b):
    world(b, cfg=BOOKS, policy={'version': 1, 'claude': {'gone@example.com': 'US-B', 'bob@example.com': 'HK-A'}})
    books(b)


for name, key, value in [('api-keys-string', 'api_keys', 'ba'), ('api-keys-dict', 'api_keys', {GONE_A: 1}), ('api-keys-number', 'api_keys', 3),
                         ('api-keys-list-numbers', 'api_keys', [2, 1]), ('api-keys-list-mixed', 'api_keys', [GONE_A, 1]),
                         ('api-keys-list-null', 'api_keys', [None]), ('api-keys-list-with-list', 'api_keys', [GONE_A, [1]]),
                         ('ref-keys-number', 'ref_keys', 3), ('ref-keys-list-mixed', 'ref_keys', [GONE_R, None]), ('ref-keys-string', 'ref_keys', 'zy')]:
    @kcase('secrets-forget/' + name, ['secrets-forget'])   # 记录的类型不对
    def _(b, key=key, value=value):
        world(b, cfg={'api_keys': [GONE_A], 'ref_keys': [GONE_R], key: value})
        kc(b, API + GONE_A, 'api.example.com', S3)
        kc(b, REF + GONE_R, 'reference', R2)


@kcase('secrets-forget/mail-default-only', ['secrets-forget'])   # 配了邮箱助手、没有单独配邮箱的账号：不用清
def _(b):
    world(b, cfg=dict(BOOKS, mail={'user': 'inbox@example.com', 'accounts': {}}))
    books(b)


@kcase('secrets-forget/mail-not-dict', ['secrets-forget'])
def _(b):
    world(b, cfg={'mail': ['x'], 'api_keys': [GONE_A]})


@kcase('secrets-forget/mail-accounts-not-dict', ['secrets-forget'])
def _(b):
    world(b, cfg={'mail': {'user': 'inbox@example.com', 'accounts': ['x']}})


@kcase('secrets-forget/bad-table', ['secrets-forget'])
def _(b):
    world(b, cfg=BOOKS)
    b.write('.cca/accounts.json', '{"a": ')
    books(b)


@kcase('secrets-forget/bad-config', ['secrets-forget'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{"api_keys": [')


@kcase('secrets-forget/fresh-home', ['secrets-forget'])
def _(b):
    pass


@kcase('secrets-forget/two-args', ['secrets-forget', 'all', 'x'])
def _(b):
    world(b, cfg=BOOKS)


@kcase('secrets-forget/empty-scope', ['secrets-forget', ''])
def _(b):
    world(b, cfg=BOOKS)
    books(b)


@kcase('secrets-forget/codex', ['secrets-forget'], tool='codex')   # Codex 的记在 codex 一节，服务名前缀也不一样；不清邮箱
def _(b):
    world(b, cfg=dict(BOOKS, mail={'accounts': {'ghost-000000': {'user': 'ghost@example.com'}}},
                      codex={'ports': {}, 'last': '', 'api_keys': [KIMI, GONE_A], 'ref_keys': [GONE_R, ERIN],
                             'key_hints': {GONE_R: hint('reference', 'sk-proj-…QRST')}}), codex=CODEX)
    books(b)
    kc(b, XAPI + KIMI, 'api.example.com', S1)
    kc(b, XAPI + GONE_A, 'api.example.com', S3)
    kc(b, XREF + ERIN, 'reference', R1)
    kc(b, XREF + GONE_R, 'reference', R2)


@kcase('secrets-forget/codex-all', ['secrets-forget', 'all'], tool='codex')
def _(b):
    world(b, cfg=dict(BOOKS, codex={'ports': {}, 'last': '', 'api_keys': [KIMI], 'ref_keys': [ERIN]}), codex=CODEX)
    books(b)
    kc(b, XAPI + KIMI, 'api.example.com', S1)
    kc(b, XREF + ERIN, 'reference', R1)


@kcase('secrets-forget/agy', ['secrets-forget'], tool='agy')
def _(b):
    world(b, cfg={'mail': {'accounts': {'ghost-000000': {'user': 'ghost@example.com'}}},
                  'agy': {'ref_keys': [GONE_R, ident('gina@example.com')], 'api_keys': [KIMI]}}, agy={'gina@example.com': 'US-A'})
    kc(b, GREF + GONE_R, 'reference', R2)
    kc(b, GREF + ident('gina@example.com'), 'reference', R1)
    kc(b, GAPI + KIMI, 'api.example.com', S1)


@kcase('secrets-forget/claude-all-skips-mail', ['secrets-forget', 'all'])   # all 时不清邮箱（cca restore 另有 mail forget）
def _(b):
    world(b, cfg=dict(BOOKS, mail={'accounts': {'ghost-000000': {'user': 'ghost@example.com'}}}))
    books(b)


@kcase('secrets-forget/file', ['secrets-forget'], env=FILE)
def _(b):
    world(b, cfg=BOOKS)
    for svc, host, s in [(API + DIRECT, 'api.example.com', S1), (API + GONE_A, 'old.example.com', S3), (REF + ALICE, 'reference', R1),
                         (REF + GONE_R, 'reference', R2)]:
        kf(b, svc, {'v': 1, 'host': host, 'key': s})
    books(b)   # 钥匙串不碰


@kcase('secrets-forget/file-all', ['secrets-forget', 'all'], env=FILE)
def _(b):
    world(b, cfg=BOOKS)
    for svc, host, s in [(API + DIRECT, 'api.example.com', S1), (REF + ALICE, 'reference', R1), ('cca-mail', 'inbox@example.com', 'abcdefghijklmnop')]:
        kf(b, svc, {'v': 1, 'host': host, 'key': s})


# --- key-show --------------------------------------------------------------------------------------------------------
@kcase('key-show/table-key', ['key-show', '表里有'])
def _(b):
    world(b)
    kc(b, API + INTABLE, 'api3.example.com', S3)


@kcase('key-show/api', ['key-show', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('api.example.com', 'sk-ant-…WXYZ')}})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('key-show/api-backfill', ['key-show', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', R1)


@kcase('key-show/api-not-booked', ['key-show', '直连'])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', S1)


@kcase('key-show/api-missing', ['key-show', '直连'])   # 没有：退出码 3；第三方 API 账号的记录不删（可能还存着旧服务商的那一把）
def _(b):
    world(b, cfg={'api_keys': [DIRECT], 'key_hints': {DIRECT: hint('old.example.com')}})
    kc(b, API + DIRECT, 'old.example.com', S1)


@kcase('key-show/api-unreadable', ['key-show', '直连'])   # 读不了：退出码 4
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', S1)
    switch(b, 'keychain_unreadable')


@kcase('key-show/api-table-key-bad', ['key-show', 'x'])
def _(b):
    world(b, table={'x': {'base_url': 'https://api.example.com', 'key': 'has space 1234'}})
    kc(b, API + ident('x'), 'api.example.com', S1)


@kcase('key-show/official', ['key-show', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: {'host': 'reference', 'hint': 'sk-ant-…WXYZ', 'at': 1700000000.5}}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-backfill', ['key-show', '主号(alice@example.com)'])   # 旧版本存的：补上掩码
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-backfill-drops-at', ['key-show', '主号(alice@example.com)'])   # 掩码对不上：整项重记（时间、来自哪台设备就没了）
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: {'host': 'reference', 'hint': 'sk-old-…0000', 'at': 1700000000.5, 'from': 'bob-mini'}}})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-not-booked', ['key-show', '主号(alice@example.com)'])
def _(b):
    world(b)
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-missing-booked', ['key-show', '主号(alice@example.com)'])   # 钥匙串里的被删了：记录也删掉，退出码 3
def _(b):
    world(b, cfg={'ref_keys': [ALICE, BOB], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ'), BOB: hint('reference')}})
    kc(b, REF + BOB, 'reference', S1)


@kcase('key-show/official-missing', ['key-show', 'bob@example.com'])
def _(b):
    world(b)


@kcase('key-show/official-unreadable', ['key-show', '主号(alice@example.com)'])   # 读不了：退出码 4，不当成没有、不删记录
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference', 'sk-ant-…WXYZ')}})
    kc(b, REF + ALICE, 'reference', R1)
    switch(b, 'keychain_unreadable')


@kcase('key-show/official-garbage', ['key-show', '主号(alice@example.com)'])   # 读到的不是可见 ASCII：当成没有（退出码 3，记录删掉）
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kc(b, REF + ALICE, 'reference', 'sk-中文-12345678')


@kcase('key-show/official-no-security', ['key-show', '主号(alice@example.com)'], env={'PATH': '/nonexistent-cca-pydiff'})
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-bad-config', ['key-show', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{')
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/official-missing-bad-config', ['key-show', '主号(alice@example.com)'])
def _(b):
    world(b)
    b.write('.cca/config.json', '{')


@kcase('key-show/official-ref-keys-number', ['key-show', '主号(alice@example.com)'])
def _(b):
    world(b, cfg={'ref_keys': 1})
    kc(b, REF + ALICE, 'reference', R1)


@kcase('key-show/no-such-account', ['key-show', 'nobody'])
def _(b):
    world(b)


@kcase('key-show/bad-table', ['key-show', '直连'])
def _(b):
    world(b)
    b.write('.cca/accounts.json', '"x"')


@kcase('key-show/weird-chars-table', ['key-show', 'x'])
def _(b):
    world(b, table={'x': {'base_url': 'https://api.example.com', 'key': WEIRD}})


@kcase('key-show/weird-chars-keychain', ['key-show', '直连'])
def _(b):
    world(b, cfg={'api_keys': [DIRECT]})
    kc(b, API + DIRECT, 'api.example.com', WEIRD)


@kcase('key-show/api-4096', ['key-show', '直连'])
def _(b):
    world(b)
    kc(b, API + DIRECT, 'api.example.com', 'sk-' + 'Kk9_' * 1023 + 'k')


@kcase('key-show/value-null', ['key-show', 'x'])   # 值是 null：照官方账号看
def _(b):
    world(b, table={'x': None}, cfg={'ref_keys': [ident('x')]})
    kc(b, REF + ident('x'), 'reference', R1)


@kcase('key-show/empty-arg', ['key-show', ''])
def _(b):
    world(b)


@kcase('key-show/no-args', ['key-show'])
def _(b):
    world(b)


@kcase('key-show/two-args', ['key-show', '直连', 'x'])
def _(b):
    world(b)


@kcase('key-show/codex-official', ['key-show', 'erin@example.com'], tool='codex')
def _(b):
    world(b, cfg={'codex': {'ports': {}, 'last': '', 'ref_keys': [ERIN]}}, codex=CODEX)
    kc(b, XREF + ERIN, 'reference', R1)
    kc(b, REF + ERIN, 'reference', R2)


@kcase('key-show/codex-official-missing', ['key-show', 'erin@example.com'], tool='codex')
def _(b):
    world(b, cfg={'ref_keys': [ERIN], 'codex': {'ports': {}, 'last': '', 'ref_keys': [ERIN]}}, codex=CODEX)
    kc(b, REF + ERIN, 'reference', R2)


@kcase('key-show/codex-api', ['key-show', 'kimi'], tool='codex')
def _(b):
    world(b, codex=CODEX)
    kc(b, XAPI + KIMI, 'api.example.com', S1)


@kcase('key-show/agy-official', ['key-show', 'gina@example.com'], tool='agy')
def _(b):
    world(b, cfg={'agy': {'ref_keys': [ident('gina@example.com')]}}, agy={'gina@example.com': 'US-A'})
    kc(b, GREF + ident('gina@example.com'), 'reference', R1)


@kcase('key-show/file-official', ['key-show', '主号(alice@example.com)'], env=FILE)
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    kf(b, REF + ALICE, {'v': 1, 'host': 'reference', 'key': R1})
    kc(b, REF + ALICE, 'reference', R2)


@kcase('key-show/file-api', ['key-show', '直连'], env=FILE)
def _(b):
    world(b)
    kf(b, API + DIRECT, {'host': 'api.example.com', 'key': S1, 'extra': [1]})


@kcase('key-show/file-missing', ['key-show', '主号(alice@example.com)'], env=FILE)
def _(b):
    world(b, cfg={'ref_keys': [ALICE], 'key_hints': {ALICE: hint('reference')}})
    kc(b, REF + ALICE, 'reference', R2)


@kcase('key-show/file-no-secrets-dir-is-file', ['key-show', '主号(alice@example.com)'], env=FILE)   # secrets 是个文件：读不了（4）
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    b.write('.cca/secrets', 'x')


for name, data, booked in [
    ('other-host', {'v': 1, 'host': 'api.example.com', 'key': R1}, True),        # 域名对不上：算没有（3），记录删掉
    ('host-not-str', {'v': 1, 'host': ['reference'], 'key': R1}, True),
    ('no-host', {'v': 1, 'key': R1}, False),
    ('key-not-str', {'v': 1, 'host': 'reference', 'key': 12345678}, True),
    ('no-key', {'v': 1, 'host': 'reference'}, True),
    ('not-object', '["reference", "%s"]\n' % R1, True),
    ('null', 'null', True),
    ('bad-json', '{"host": "reference", "key": ', True),                        # 读不了（4），记录留着
    ('empty', '', True),
    ('not-utf8', b'{"host": "reference", "key": "sk-\xff-12345678"}', True),
    ('bom', b'\xef\xbb\xbf{"host": "reference", "key": "%s"}' % R1.encode(), True),
    ('crlf', '{"host": "reference",\r\n "key": "%s"}\r\n' % R1, True),
    ('key-too-short', {'v': 1, 'host': 'reference', 'key': 'short'}, True),      # 读到了但不是合格的 key：算没有（3）
    ('key-not-ascii', {'v': 1, 'host': 'reference', 'key': 'sk-中文-12345678'}, True),
    ('key-lone-surrogate', '{"host": "reference", "key": "sk-\\ud83d-12345678"}', True),
    ('key-newline', {'v': 1, 'host': 'reference', 'key': R1 + '\n'}, True),      # 结尾多一个换行：原样给（正则的 $）
    ('key-two-newlines', {'v': 1, 'host': 'reference', 'key': R1 + '\n\n'}, True),
    ('key-padded', {'v': 1, 'host': 'reference', 'key': ' %s ' % R1}, True),     # 密钥文件里的不去空白
    ('dup-keys', '{"host": "x", "key": "short", "host": "reference", "key": "%s"}' % R1, False),
]:
    @kcase('key-show/file-' + name, ['key-show', '主号(alice@example.com)'], env=FILE)
    def _(b, data=data, booked=booked):
        world(b, cfg={'ref_keys': [ALICE]} if booked else None)
        kf(b, REF + ALICE, data)


@kcase('key-show/file-is-dir', ['key-show', '主号(alice@example.com)'], env=FILE)   # 是个目录：读不了（4）
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    os.makedirs(b.path('.cca/secrets/' + REF + ALICE))


@kcase('key-show/file-dangling-link', ['key-show', '主号(alice@example.com)'], env=FILE)   # 断了的链接：算没有（3）
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    os.makedirs(b.path('.cca/secrets'))
    os.symlink('/nonexistent-cca-pydiff/key', b.path('.cca/secrets/' + REF + ALICE))


@kcase('key-show/file-link-loop', ['key-show', '主号(alice@example.com)'], env=FILE)   # 链接成环：读不了（4）
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    os.makedirs(b.path('.cca/secrets'))
    os.symlink(REF + ALICE, b.path('.cca/secrets/' + REF + ALICE))


@kcase('key-show/file-via-link', ['key-show', '主号(alice@example.com)'], env=FILE)   # 链接指向一份好的：照读
def _(b):
    world(b, cfg={'ref_keys': [ALICE]})
    b.write('elsewhere.json', '{"host": "reference", "key": "%s"}' % R1)
    os.makedirs(b.path('.cca/secrets'))
    os.symlink(b.path('elsewhere.json'), b.path('.cca/secrets/' + REF + ALICE))


@kcase('key-show/file-codex', ['key-show', 'kimi'], tool='codex', env=FILE)
def _(b):
    world(b, cfg={'codex': {'ports': {}, 'last': '', 'api_keys': [KIMI]}}, codex=CODEX)
    kf(b, XAPI + KIMI, {'v': 1, 'host': 'api.example.com', 'key': R2})
