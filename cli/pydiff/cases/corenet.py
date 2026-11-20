"""内置代理（cli/src/cmd/corenet.rs 和 corenet/ 下的几份）：本机端口池、自定义节点、内核的配置和节点定义、辅助安装的临时代理、
本机 UDP 走节点、守着内核的常驻进程。

不动这台机器、不联网：
- 探测网卡和路由的 route / netstat / ipconfig 一律是沙箱里的假命令（下面的 net()）：回答什么照 net/ 下的夹具，收到的参数和 PATH 记进
  net.log（对拍连它一起比）。系统的 DNS 读 CCA_RESOLV_CONF 指的夹具（用例都带着这个变量，不读真的 /etc/resolv.conf）。
  这一份的用例都经 home() 放夹具，它总会放这三个假命令：没放的话会跑到这台机器真的 route 上。
- 节点定义里的密钥都是一眼看得出的假货（test-password、全 0 的 uuid），服务器是 *.example.com、192.0.2.x。
  只有「服务器是公网 IPv4」那条路要一个算公网的地址，用的是 8.8.8.8（只交给假的 route，不会去连）。
- udp-probe 的 STUN 只发给 cases/leak.py 里那个本机回环上的假服务器，归属地经假的 curl。
- core-keep 起的「内核」是沙箱里的假脚本（.cca/bin/cca，CCA_SELF 指着它）：每次启动往 kernel.log 记一行（参数、环境变量、
  开着的文件描述符、输入、是不是自己一个会话），再照用例定的做（马上退出、睡着等信号……）。给守着它的进程发信号、改配置在
  during 里做：假内核把两个进程号写进沙箱，during 读出来就删掉（两边不一样，不留给比对）。

- udp-env/real-home 是唯一一条 HOME 不在沙箱里的：HOME 是这个用户真正的主目录时 root 那边的位置才是 /Library 下的那几个。
  config.json 这些照 CCA_HOME 还在沙箱里；它只读一眼 /Library/Application Support/cca-udp/info.json 在不在，什么都不写。

每次不一样的东西比之前换掉：core-nodes-save 记下的时间（小数；夹具里的时间都是整数，不会被换掉）、install-node proxy 取的空闲端口、
udp-build 记下的时间和带着它的 info.json 的摘要。

夹具里“多久以前”的时间在夹具函数跑的那一刻才算（ago()），偏在档位中间：两边先后跑差不了一秒，落在同一档。
现场要和原版自己写出来的一模一样时（“内容没变就不写”、“装着的就是这一份”），夹具里拿原版的 PY_SRC 先跑一趟（oracle()）。

core-keep 的用例要等真实的时间（重启前的退避 1 秒、2 秒、4 秒，停内核等 3 秒），那一段跑完约一分钟；local-env 里 route / netstat
卡住不回来的两条各等 5 秒（两边各一遍）；整份约四分钟。

tests/local_udp.py 的检查都在这里（udp- 开头的用例）和 cli/src/cmd/corenet/udp.rs 的单元测试里。
分享链接、Clash 节点块的解析另有一批对照数据（cli/tests/vectors/corenet.json，cli/tools/gen_corenet_vectors.py 生成），
这里的 node-check 用例是其中有代表性的一部分，加上起名字、读配置的那几条路。
最后一节（udp_leak_hint）要等 cli/src/cmd/leak.rs 换掉替身才打开，见那里的说明。"""
import base64
import fcntl
import hashlib
import json
import os
import signal
import tempfile
import time

from run import case
from cases.leak import DEAD, MASK as LEAK_MASK, STUN, STUN_A, STUN_B, geo, net as leak_net, net24, where
from cases.router import fp
from cases.table import FILL, NODES, POLICY, TABLE, world

ZERO = '00000000-0000-0000-0000-000000000000'
ENV = {'CCA_RESOLV_CONF': '%HOME%/resolv.conf'}
# 可选节点 US-A / US-B / HK-A 的定义（和 cases/router.py 里 A、B、H 的地址一样，摘要对得上）
A = {'name': 'US-A', 'type': 'vless', 'server': 'a.example.com', 'port': 443, 'uuid': ZERO, 'udp': True, 'tls': True,
     'servername': 'sni-a.example.com', 'flow': 'xtls-rprx-vision', 'reality-opts': {'public-key': 'test-public-key', 'short-id': 'abcd'}}
B = {'name': 'US-B', 'type': 'ss', 'server': '192.0.2.21', 'port': 8388, 'cipher': 'aes-256-gcm', 'password': 'test-password'}
H = {'name': 'HK-A', 'type': 'trojan', 'server': 'hk.example.com', 'port': 443, 'password': 'test-password'}
HY = {'name': 'HY-节点', 'type': 'hysteria2', 'server': 'hy.example.com', 'port': 8443, 'password': 'test-password'}
BUILTIN = {'client': 'builtin'}


def nfp(p):
    """节点定义的摘要（照 cca 的 fill_fp）"""
    return fp(p.get('type'), p.get('server'), p.get('port'), p.get('servername') or p.get('sni'))


def fill_id(fill):
    """参考节点的 id（随内容变）：照 cca 的 policy_fill 现算"""
    return hashlib.sha256(json.dumps([fill['group'], fill['proxies']], ensure_ascii=False, sort_keys=True).encode('utf-8')).hexdigest()[:16]


def ago(seconds):
    """夹具函数跑的这一刻往前这么多秒（整数的时间戳）"""
    return int(time.time()) - seconds


ROUTE = '''printf '%s\\n' "route $* [PATH=$PATH]" >> "$HOME/net.log"
for a; do last="$a"; done
f="$HOME/net/route-$last"; [ -f "$f" ] || f="$HOME/net/route"
[ -f "$f" ] || exit 1
cat "$f"
'''
NETSTAT = '''printf '%s\\n' "netstat $* [PATH=$PATH]" >> "$HOME/net.log"
[ -f "$HOME/net/netstat" ] || exit 1
cat "$HOME/net/netstat"
'''
IPCONFIG = '''printf '%s\\n' "ipconfig $* [PATH=$PATH]" >> "$HOME/net.log"
[ -f "$HOME/net/ipconfig-$2" ] || exit 1
cat "$HOME/net/ipconfig-$2"
'''


def route_text(iface):
    return ('   route to: 192.0.2.1\ndestination: default\n       mask: default\n    gateway: 192.0.2.254\n  interface: %s\n'
            '      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire\n'
            '       0         0         0         0         0         0      1500         0 \n' % iface)


def netstat_text(*ifaces):
    return ('Routing tables\n\nInternet:\nDestination        Gateway            Flags               Netif Expire\n'
            + ''.join('default            192.0.2.254        UGScg     %16s       \n' % i for i in ifaces)
            + '127                127.0.0.1          UCS                   lo0       \n192.0.2/24         link#14            UCS                   en0      !\n')


def dhcp_text(*dns):
    return ('op = BOOTREPLY\nhtype = 1\nflags = 0\nhlen = 6\nhops = 0\nxid = 0x12345678\nsecs = 0\nciaddr = 0.0.0.0\nyiaddr = 192.0.2.77\n'
            'options:\ndhcp_message_type (uint8): ACK 0x5\nserver_identifier (ip): 192.0.2.254\nlease_time (uint32): 0x15180\n'
            'subnet_mask (ip): 255.255.255.0\nrouter (ip_mult): {192.0.2.254}\ndomain_name_server (ip_mult): {%s}\nend (none):\n' % ', '.join(dns))


def net(b, route='en0', routes=None, netstat=None, ipconfig=None, resolv=None):
    """假的 route / netstat / ipconfig。route：连任何地址都走这块网卡（None：route 失败）；routes：{地址: 网卡}（个别的地址另说）；
    netstat：默认路由上的网卡（一列，照先后），或者整段输出；ipconfig：{网卡: DHCP 给的 DNS 的列表，或者整段输出}；
    resolv：系统的 resolv.conf（用例要带着 ENV）"""
    b.fake('route', ROUTE)
    b.fake('netstat', NETSTAT)
    b.fake('ipconfig', IPCONFIG)
    if route is not None:
        b.write('net/route', route if '\n' in route else route_text(route))
    for addr, iface in (routes or {}).items():
        b.write('net/route-' + addr, iface if '\n' in iface else route_text(iface))
    if netstat is not None:
        b.write('net/netstat', netstat if isinstance(netstat, str) else netstat_text(*netstat))
    for iface, dns in (ipconfig or {}).items():
        b.write('net/ipconfig-' + iface, dns if isinstance(dns, str) else dhcp_text(*dns))
    if resolv is not None:
        b.write('resolv.conf', resolv, 0o644)


def cache(b, proxies, saved=60, nodes=None, **more):
    """core/nodes.json：在家时从路由器拷的节点定义。saved：多少秒以前拷的（None：不写 saved_at）"""
    j = {'version': 1}
    if saved is not None:
        j['saved_at'] = ago(saved)
    j['nodes'] = list(NODES) if nodes is None else nodes
    j['proxies'] = proxies
    j.update(more)
    os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
    return b.json('.cca/core/nodes.json', j)


def custom(b, proxies):
    """core/custom-nodes.json：用户自己加的节点"""
    os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
    return b.json('.cca/core/custom-nodes.json', proxies if isinstance(proxies, dict) else {'version': 1, 'proxies': proxies})


ABSENT = object()   # 夹具里“没有这一项”（None 是写成 null）


def home(b, local=ABSENT, cfg=None, defs=None, mine=None, route='en0', **kw):
    """这一份的用例都用它放夹具：常用的那套账号和节点（cases/table.py 的 world），加上假的 route / netstat / ipconfig。
    local：config.json 的 local 一节；defs：core/nodes.json 里的节点定义；mine：自定义节点；别的参数交给 world"""
    c = dict(cfg or {})
    if local is not ABSENT:
        c['local'] = local
    world(b, cfg=c, **kw)
    net(b, route=route)
    if defs is not None:
        cache(b, defs)
    if mine is not None:
        custom(b, mine)


def tun(b, dns=(), resolv='nameserver 198.18.0.2\n', phys=('utun4', 'en0'), via='utun4', routes=None):
    """别的程序开着 TUN：公网的流量走虚拟网卡 utun4，物理网卡是 en0。resolv：系统配的 DNS（默认是 TUN 自己的地址，到不了）；
    dns：物理网卡从 DHCP 拿到的（None：ipconfig 失败）；phys：默认路由上的网卡（None：netstat 失败）；routes：个别地址另走哪块网卡"""
    net(b, route=via, routes=dict({'198.18.0.2': 'utun4', '192.0.2.53': 'en0', '192.0.2.54': 'en0'}, **(routes or {})), netstat=phys,
        ipconfig={'en0': list(dns)} if dns is not None else None, resolv=resolv)


# ---- select-lines：每个槽位应选中的节点 -------------------------------------------------------------------------------------

RENAMED = [dict(A, name='美国 A1'), B, dict(H, name='HK-甲')]     # 路由器上给 US-A、HK-A 改了名（内置代理里还是拷来时的叫法）
IDS = {'US-A': nfp(A), 'US-B': nfp(B), 'HK-A': nfp(H)}

for name, argv, kw in [
    ('router', [], {}), ('local-not-configured', ['local'], {}), ('local-builtin-same-names', ['local'], {'local': BUILTIN, 'defs': [A, B, H]}),
    ('local-builtin-renamed', ['local'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('builtin-but-not-local', [], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('other-word', ['Local'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('local-old-client', ['local'], {'local': {'client': 'clash-verge'}, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('local-no-defs', ['local'], {'local': BUILTIN}), ('no-accounts', ['local'], {'local': BUILTIN, 'table': {}}),
    ('local-section-empty-list', ['local'], {'local': []}), ('local-section-text', ['local'], {'local': 'builtin'}),
    ('local-section-number', ['local'], {'local': 7}), ('local-section-text-not-asked', [], {'local': 'builtin'}),
    ('too-many-args', ['local', 'x'], {}),
]:
    @case('select-lines/' + name, ['select-lines'] + argv, env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('select-lines/two-tools', ['select-lines', 'local'], env=ENV, tool='codex')
def _(b):
    home(b, local=BUILTIN, defs=RENAMED, cfg={'node_ids': IDS, 'ports': {'bob@example.com': 7902}}, codex={'erin@example.com': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'US-A'}})


@case('select-lines/readonly', ['select-lines', 'local'], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, local=BUILTIN, defs=RENAMED, cfg={'node_ids': IDS})


@case('select-lines/empty-home', ['select-lines'], env=ENV)
def _(b):
    net(b)


# ---- slot-rows：端口池每个槽位——组名、端口、应选中的节点、用它的账号 ---------------------------------------------------------

for name, kw in [
    ('not-configured', {}), ('builtin', {'local': BUILTIN, 'defs': [A, B, H]}), ('builtin-renamed', {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('no-accounts', {'local': BUILTIN, 'table': {}}), ('high-port', {'cfg': {'ports': {'bob@example.com': 7910}}}),
    ('nodes-with-odd-items', {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'nodes': ['US-A', 5, None, True, 'US-B', 'US-B', 1.5, 'HK-A']}}),
    ('nodes-with-a-list', {'cfg': {'nodes': ['US-A', ['US-B'], 'US-B', 'HK-A']}}), ('nodes-with-an-object', {'local': BUILTIN, 'cfg': {'nodes': ['US-A', {}, 'US-B', 'HK-A']}}),
    ('nodes-is-text', {'cfg': {'nodes': 'US-A'}, 'table': {'a@example.com': 'U'}}), ('nodes-is-object', {'cfg': {'nodes': {'US-A': 1, 'US-B': 2}}, 'table': {'a@example.com': 'US-A'}}),
    ('nodes-is-number', {'cfg': {'nodes': 5}, 'table': {}}), ('nodes-is-null', {'cfg': {'nodes': None}, 'table': {}}),
    ('reject-in-nodes', {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'nodes': ['REJECT', 'US-A', 'US-B', 'HK-A']}}),
]:
    @case('slot-rows/' + name, ['slot-rows'], env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('slot-rows/three-tools', ['slot-rows'], env=ENV, tool='agy')
def _(b):
    home(b, local=BUILTIN, defs=RENAMED, cfg={'node_ids': IDS}, codex={'erin@example.com': 'HK-A'}, agy={'工作(gina@example.com)': 'US-B'})


@case('slot-rows/readonly', ['slot-rows'], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, local=BUILTIN, defs=RENAMED, cfg={'node_ids': IDS})


@case('slot-rows/args', ['slot-rows', 'x'], env=ENV)
def _(b):
    home(b)


# ---- local-node：账号的节点在内置代理的节点定义里的叫法 ------------------------------------------------------------------------

for name, argv, kw in [
    ('not-configured', ['US-A'], {'defs': RENAMED, 'cfg': {'node_ids': IDS}}), ('builtin-renamed', ['US-A'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('builtin-same', ['US-B'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}), ('builtin-unknown', ['ZZ'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('builtin-new-name-is-not-a-key', ['美国 A1'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('builtin-no-recorded-ids', ['US-A'], {'local': BUILTIN, 'defs': RENAMED}), ('builtin-ids-not-object', ['US-A'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': ['x']}}),
    ('builtin-id-not-text', ['US-A'], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': {'US-A': 5, 'HK-A': ['x']}}}),
    ('builtin-same-address-twice', ['US-A'], {'local': BUILTIN, 'defs': [dict(A, name='one'), dict(A, name='two'), B, H], 'cfg': {'node_ids': IDS}}),
    ('builtin-same-address-twice-own-name', ['US-A'], {'local': BUILTIN, 'defs': [dict(A, name='one'), A, B, H], 'cfg': {'node_ids': IDS}}),
    ('empty-name', [''], {'local': BUILTIN, 'defs': RENAMED}), ('no-args', [], {}), ('two-args', ['US-A', 'US-B'], {}),
    ('local-section-text', ['US-A'], {'local': 'builtin'}), ('nodes-with-a-list', ['US-A'], {'local': BUILTIN, 'cfg': {'nodes': ['US-A', ['x']]}}),
]:
    @case('local-node/' + name, ['local-node'] + argv, env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('local-node/custom-and-fill', ['local-node', 'US-C'], env=ENV)
def _(b):
    # 参考节点里的 US-C 在自定义节点里另有一个叫法（同一个服务器）：按地址认到自定义的那个
    home(b, local=BUILTIN, defs=[A, B], mine=[dict(FILL['proxies'][0], name='我的 C')], fill=FILL,
         cfg={'nodes': NODES + ['US-C'], 'node_ids': dict(IDS, **{'US-C': nfp(FILL['proxies'][0])})})


@case('local-node/bad-config', ['local-node', 'US-A'], env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"local": ')


@case('local-node/empty-home', ['local-node', 'US-A'], env=ENV)
def _(b):
    net(b)


# ---- local-set：config.json 的 local 一节 --------------------------------------------------------------------------------

FULL = {'client': 'builtin', 'udp': 'US-A', 'applied': 'abc', 'core_off': '1', 'fill': 'x'}
for name, argv, local in [
    ('first-client', ['client', 'builtin'], ABSENT), ('same-client-keeps-rest', ['client', 'builtin'], FULL), ('other-client-clears', ['client', 'other'], FULL),
    ('client-off-records-core-off', ['client', '', 'core_off', '1'], FULL), ('client-empty-when-none', ['client', ''], {'core_off': '1', 'applied': 'x'}),
    ('client-empty-when-empty', ['client', ''], {'client': '', 'applied': 'x'}), ('applied', ['applied', 'deadbeef'], BUILTIN), ('applied-empty', ['applied', ''], FULL),
    ('udp-on', ['udp', 'US-A'], BUILTIN), ('udp-off', ['udp', ''], FULL), ('udp-off-when-none', ['udp', ''], BUILTIN), ('any-key', ['什么', '值 x'], BUILTIN),
    ('several', ['udp', 'HK-A', 'applied', '1', 'client', 'builtin', 'udp', ''], BUILTIN), ('order-matters', ['udp', 'US-A', 'client', 'x', 'applied', '2'], BUILTIN),
    ('odd-count', ['applied', 'x', 'udp'], BUILTIN), ('one-arg', ['client'], FULL), ('no-args', [], FULL), ('client-number-in-config', ['client', '5'], {'client': 5, 'x': 1}),
    ('section-is-list', ['applied', 'x'], []), ('section-is-list-client', ['client', 'builtin'], []), ('section-is-list-udp-off', ['udp', ''], []),
    ('section-is-text', ['client', 'builtin'], 'builtin'), ('section-is-null', ['applied', 'x'], None), ('section-is-number-no-args', [], 5),
]:
    @case('local-set/' + name, ['local-set'] + argv, env=ENV)
    def _(b, local=local):
        home(b, local=local)


@case('local-set/codex', ['local-set', 'client', 'builtin'], env=ENV, tool='codex')
def _(b):
    home(b, codex={'erin@example.com': 'HK-A'}, cfg={'codex': {'ports': {'erin@example.com': 7904}, 'last': ''}})


@case('local-set/empty-home', ['local-set', 'client', 'builtin'], env=ENV)
def _(b):
    net(b)


@case('local-set/readonly', ['local-set', 'client', 'builtin', 'udp', 'US-A'], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, local={'core_off': '1'})


@case('local-set/bad-config', ['local-set', 'client', 'builtin'], env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '[1, 2')


@case('local-set/config-is-link', ['local-set', 'applied', 'x'], env=ENV)
def _(b):
    home(b, local=BUILTIN)
    os.rename(b.path('.cca/config.json'), b.path('real-config.json'))
    os.symlink(b.path('real-config.json'), b.path('.cca/config.json'))


# ---- local-env：给 bash 的 LOCAL_*（客户端、端口池、要不要重新加载的摘要、出站绑定、本机 UDP 走节点）---------------------------

def udp_root(b, info=None, raw=None):
    """root 那边装着的（沙箱里换进 ~/udp-root/cca-udp）"""
    if raw is not None:
        return b.write('udp-root/cca-udp/info.json', raw, 0o644)
    return b.json('udp-root/cca-udp/info.json', info, mode=0o644, indent=1)


for name, kw in [
    ('not-configured', {}), ('not-configured-defs-ignored', {'defs': [A, B, H]}), ('builtin-no-defs', {'local': BUILTIN}), ('builtin', {'local': BUILTIN, 'defs': [A, B, H]}),
    ('builtin-renamed', {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}), ('builtin-applied', {'local': dict(BUILTIN, applied='0123abcd'), 'defs': [A, B, H]}),
    ('builtin-applied-number', {'local': dict(BUILTIN, applied=5), 'defs': [A, B, H]}), ('builtin-applied-null', {'local': dict(BUILTIN, applied=None)}),
    ('builtin-applied-list', {'local': dict(BUILTIN, applied=['a', 1])}), ('old-client', {'local': {'client': 'clash-verge', 'applied': 'x', 'controller': 'http://127.0.0.1:9097'}}),
    ('manual-client', {'local': {'client': 'manual'}}), ('client-number', {'local': {'client': 1}}), ('client-empty', {'local': {'client': '', 'core_off': '1'}}),
    ('no-accounts', {'local': BUILTIN, 'defs': [A, B, H], 'table': {}}), ('nine-slots', {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'ports': {'bob@example.com': 7909}}}),
    ('defs-order-does-not-matter-keys-do', {'local': BUILTIN, 'defs': [dict(reversed(list(H.items()))), B, A]}),
    ('defs-extra-nodes-not-counted', {'local': BUILTIN, 'defs': [A, B, H, HY]}), ('defs-with-dialer', {'local': BUILTIN, 'defs': [A, dict(B, **{'dialer-proxy': 'HY-节点'}), H, HY]}),
    ('custom-nodes', {'local': BUILTIN, 'defs': [A, B], 'mine': [H, HY]}), ('fill-builtin', {'local': BUILTIN, 'defs': [A, B, H], 'fill': FILL, 'cfg': {'nodes': NODES + ['US-C']}}),
    ('fill-not-builtin-same-id', {'local': {'fill': fill_id(FILL)}, 'fill': FILL}), ('fill-not-builtin-other-id', {'local': {'fill': 'something-else'}, 'fill': FILL}),
    ('fill-not-builtin-no-fill-recorded', {'fill': FILL}), ('bind-off', {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'core_bind': False}}),
    ('bind-zero-is-not-off', {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'core_bind': 0}}), ('bind-null-is-not-off', {'local': BUILTIN, 'cfg': {'core_bind': None}}),
    ('route-fails', {'local': BUILTIN, 'defs': [A, B, H], 'route': None}), ('route-garbage', {'local': BUILTIN, 'defs': [A, B, H], 'route': 'interface:\nfoo bar\n'}),
    ('local-is-list', {'local': []}), ('local-is-text', {'local': 'builtin'}), ('local-is-null', {'local': None}), ('local-is-number', {'local': 0}),
    ('nodes-mixed-types', {'local': BUILTIN, 'cfg': {'nodes': ['US-A', 5, 'US-B', 'HK-A']}}), ('nodes-numbers', {'table': {}, 'cfg': {'nodes': [3, True, 1.5, 2]}}),
    ('nodes-one-odd', {'table': {}, 'cfg': {'nodes': [None]}}), ('nodes-with-a-list', {'table': {}, 'cfg': {'nodes': [['x']]}}),
    ('nodes-empty', {'local': BUILTIN, 'table': {}, 'cfg': {'nodes': []}}), ('nodes-is-text', {'table': {}, 'cfg': {'nodes': 'cba'}}),
]:
    @case('local-env/' + name, ['local-env'], env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


for name, tool in [('codex', 'codex'), ('agy', 'agy')]:
    @case('local-env/tool-' + name, ['local-env'], env=ENV, tool=tool)
    def _(b):
        home(b, local=BUILTIN, defs=[A, B, H], codex={'erin@example.com': 'HK-A'}, agy={'gina@example.com': 'US-B'})


@case('local-env/readonly', ['local-env'], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])


@case('local-env/empty-home', ['local-env'], env=ENV)
def _(b):
    net(b)


@case('local-env/args', ['local-env', 'x'], env=ENV)
def _(b):
    home(b)


@case('local-env/table-errors-do-not-stop-it', ['local-env'], env=ENV)
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H], table={'a b': 'US-A', 'bob@example.com': 'ZZ'})


# 别的程序开着 TUN：出站绑定物理网卡（只在 macOS 上）
for name, kw, tunkw in [
    ('tun-system-dns-unreachable', {'defs': [A, B, H]}, {'dns': ['192.0.2.53', '192.0.2.54']}),
    ('tun-system-dns-reachable', {'defs': [A, B, H]}, {'dns': ['192.0.2.53'], 'resolv': 'nameserver 198.18.0.2\nnameserver 192.0.2.53\n'}),
    ('tun-no-resolv-conf', {'defs': [A, B, H]}, {'dns': ['192.0.2.53'], 'resolv': None}),
    ('tun-dhcp-gives-nothing', {'defs': [A, B, H]}, {'dns': []}), ('tun-ipconfig-fails', {'defs': [A, B, H]}, {'dns': None}),
    ('tun-dhcp-gives-tun-and-loopback', {'defs': [A, B, H]}, {'dns': ['198.18.0.2', '127.0.0.1', '192.0.2.53', '192.0.2.53', '0.0.0.0', 'junk', '::1', '8.8.8.8']}),
    ('tun-no-physical-default-route', {'defs': [A, B, H]}, {'phys': ['utun4', 'tailscale0']}), ('tun-physical-first', {'defs': [A, B, H]}, {'phys': ['en5', 'utun4', 'en0'], 'dns': None}),
    ('tun-other-virtual-names', {'defs': [A, B, H]}, {'via': 'Mihomo', 'dns': ['192.0.2.53']}), ('tun-not-builtin', {'local': {}}, {'dns': ['192.0.2.53']}),
    ('tun-bind-off', {'cfg': {'core_bind': False}}, {'dns': ['192.0.2.53']}),
    ('tun-resolv-odd-lines', {}, {'dns': ['192.0.2.53'], 'resolv': '# x\n\nnameserver\nsearch example.com\n nameserver\t127.0.0.1 extra\nNameserver 192.0.2.53\nnameserver ::1\nnameserver 0.0.0.0\nnameserver fe80::1%utun4\n'}),
    ('tun-resolv-scoped-physical', {}, {'dns': ['192.0.2.53'], 'resolv': 'nameserver 198.18.0.2\nnameserver fe80::1%en0\n'}),
    ('tun-resolv-public', {}, {'dns': ['192.0.2.53'], 'resolv': 'nameserver 8.8.8.8\n'}), ('tun-resolv-crlf-not-utf8', {}, {'dns': ['192.0.2.53'], 'resolv': b'nameserver 198.18.0.2\r\nname\xffserver 192.0.2.53\rnameserver 192.0.2.54\r\n'}),
    ('tun-netstat-fails', {'defs': [A, B, H]}, {'phys': None, 'dns': ['192.0.2.53']}),
    # IPv6 的 DNS：问路由时带 -inet6；走物理网卡的算到得了，走 TUN 的不算（再看 DHCP 给的）
    ('tun-resolv-ipv6-on-physical', {}, {'dns': ['192.0.2.53'], 'resolv': 'nameserver fd00::53\n', 'routes': {'fd00::53': 'en0'}}),
    ('tun-resolv-ipv6-on-tun', {}, {'dns': ['192.0.2.53'], 'resolv': 'nameserver fd00::53\nnameserver ::ffff:127.0.0.1\nnameserver ::ffff:0.0.0.0\n', 'routes': {'fd00::53': 'utun4'}}),
    ('tun-resolv-route-unknown', {}, {'dns': ['192.0.2.99'], 'resolv': 'nameserver 192.0.2.98\n', 'routes': {'192.0.2.98': 'interface:\n', '192.0.2.99': 'no route\n'}}),
]:
    @case('local-env/' + name, ['local-env'], env=ENV)
    def _(b, kw=kw, tunkw=tunkw):
        home(b, **dict({'local': BUILTIN}, **kw))
        tun(b, **tunkw)


@case('local-env/tun-route-hangs', ['local-env'], env=ENV, timeout=60)
def _(b):
    # route 卡住不回来：等 5 秒，当它没查到（不绑定，也不接着问 netstat）。它晚到的回答（走虚拟网卡）不算数
    home(b, local=BUILTIN, defs=[A, B, H])
    tun(b, dns=['192.0.2.53'])
    b.fake('route', 'printf \'%s\\n\' "route $*" >> "$HOME/net.log"\nsleep 7\ncat "$HOME/net/route"\n')


@case('local-env/tun-netstat-hangs', ['local-env'], env=ENV, timeout=60)
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])
    tun(b, dns=['192.0.2.53'])
    b.fake('netstat', 'printf \'%s\\n\' "netstat $*" >> "$HOME/net.log"\nsleep 7\ncat "$HOME/net/netstat"\n')


@case('local-env/tun-probe-first-public-ipv4', ['local-env'], env=ENV)
def _(b):
    # 看节点服务器里第一个写成公网 IPv4 的走哪块网卡：192.0.2.21（不算公网）、域名、IPv6 都跳过
    home(b, local=BUILTIN, defs=[A, B, dict(H, server='2606:4700:4700::1111'), dict(HY, name='PUB', server='8.8.8.8'), dict(HY, name='PUB2', server='8.8.4.4')],
         cfg={'nodes': NODES + ['PUB', 'PUB2']})
    net(b, route='en0', routes={'8.8.8.8': 'utun4', '198.18.0.2': 'utun4'}, netstat=['utun4', 'en0'], ipconfig={'en0': ['192.0.2.53']}, resolv='nameserver 198.18.0.2\n')


@case('local-env/tun-probe-server-not-text', ['local-env'], env=ENV)
def _(b):
    home(b, local=BUILTIN, defs=[dict(A, server=5), dict(B, server=None), dict(H, server=['8.8.8.8'])])
    tun(b, dns=['192.0.2.53'])


@case('local-env/tun-resolv-conf-is-a-directory', ['local-env'], env=ENV)
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])
    tun(b, dns=['192.0.2.53'], resolv=None)
    os.makedirs(b.path('resolv.conf'))


@case('local-env/commands-missing', ['local-env'], env=dict(ENV, PATH='%HOME%/nowhere'))
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])


# 本机 UDP 走节点的状态（只在 macOS 上看）
def info_for(node, **change):
    """照 udp-build 会写出的样子的 info.json（sum 是占位的：对不上就是 stale）"""
    return dict({'v': 1, 'helper': 2, 'node': node, 'sum': 'x' * 64, 'core': '1.19.33', 'bin': 'b' * 64, 'run': 'r' * 64, 'plist': 'p' * 64,
                 'flag': '/nowhere/.cca/udp/on', 'saved_at': 1795000000}, **change)


for name, local, info in [
    ('udp-missing', dict(BUILTIN, udp='US-A'), None), ('udp-stale', dict(BUILTIN, udp='US-A'), info_for('US-A')),
    ('udp-undefined-no-udp', dict(BUILTIN, udp='US-B'), info_for('US-B')), ('udp-undefined-unknown', dict(BUILTIN, udp='NOPE'), None),
    ('udp-not-builtin', {'client': '', 'udp': 'US-A'}, info_for('US-A')), ('udp-not-text', dict(BUILTIN, udp=['US-A']), None),
    ('udp-bad-name', dict(BUILTIN, udp='US-A,DIRECT'), None), ('udp-info-wrong-version', dict(BUILTIN, udp='US-A'), info_for('US-A', v=2)),
    ('udp-info-not-json', dict(BUILTIN, udp='US-A'), '{oops'), ('udp-info-list', dict(BUILTIN, udp='US-A'), '[1]'),
]:
    @case('local-env/' + name, ['local-env'], env=ENV)
    def _(b, local=local, info=info):
        home(b, local=local, defs=[A, B, H])
        if isinstance(info, str):
            udp_root(b, raw=info)
        elif info is not None:
            udp_root(b, info)


# ---- node-check：粘贴的分享链接 / Clash 节点块 → 一个节点的 JSON（起个不重名的名字）------------------------------------------

def b64(obj, urlsafe=False, pad=True):
    raw = obj if isinstance(obj, bytes) else (obj if isinstance(obj, str) else json.dumps(obj, ensure_ascii=False)).encode('utf-8')
    s = (base64.urlsafe_b64encode if urlsafe else base64.b64encode)(raw).decode('ascii')
    return s if pad else s.rstrip('=')


VM = {'v': '2', 'ps': 'JP-X', 'add': 'v.example.com', 'port': '443', 'id': ZERO, 'aid': '0', 'scy': 'auto', 'net': 'ws', 'type': 'none',
      'host': 'h.example.com', 'path': '/ws', 'tls': 'tls', 'sni': 'sni.example.com'}


def vm(**kw):
    return 'vmess://' + b64(dict(VM, **kw))


SS = b64('aes-256-gcm:test-password')
YAML = 'name: JP-X\ntype: ss\nserver: s.example.com\nport: 8388\ncipher: aes-256-gcm\npassword: test-password\n'


def check(name, text, tool='claude', env=None, argv=(), **kw):
    @case('node-check/' + name, ['node-check'] + list(argv), stdin=text, env=dict(ENV, **(env or {})), tool=tool)
    def _(b):
        home(b, **kw)


# 每种链接、每种写法各一两个，每句报错至少一个（解析的全部对照在 cli/tests/vectors/corenet.json）
PARSE = [
    ('empty', ''), ('blank', ' \n\t\n'), ('ssr', 'ssr://abcdef'), ('ssr-upper', 'SSR://abcdef'), ('not-a-link', 'ftp://example.com/'),
    ('vmess', vm()), ('vmess-grpc', vm(net='grpc', path='svc')), ('vmess-tcp-no-tls', vm(net='tcp', tls='', sni='')), ('vmess-int-fields', vm(port=8443, aid=2)),
    ('vmess-urlsafe-no-padding', 'vmess://' + b64(dict(VM, ps='？？？>>>'), urlsafe=True, pad=False)), ('vmess-upper-scheme', 'VMESS://' + b64(VM)),
    ('vmess-wrapped', 'vmess://' + b64(VM)[:30] + '\n' + b64(VM)[30:] + '\n'), ('vmess-not-base64', 'vmess://!!!'), ('vmess-one-char', 'vmess://a'),
    ('vmess-not-json', 'vmess://' + b64('{"port": 1')), ('vmess-not-object', 'vmess://' + b64('[1]')), ('vmess-non-ascii', 'vmess://' + b64(VM) + '中'),
    ('vmess-bom', 'vmess://' + b64('\ufeff{"port": 1}')), ('vmess-no-port', 'vmess://' + b64({'add': 'v.example.com'})), ('vmess-port-zero', vm(port=0)),
    ('vmess-port-big', vm(port='65536')), ('vmess-port-float', vm(port=443.0)), ('vmess-port-bool', vm(port=True)), ('vmess-port-underscore', vm(port='4_43')),
    ('vmess-port-fullwidth', vm(port='４４３')), ('vmess-port-huge', vm(port=10**30)), ('vmess-no-server', vm(add='')), ('vmess-server-number', vm(add=5)),
    ('vmess-aid-text', vm(aid='abc')), ('vmess-aid-float', vm(aid=1.9)), ('vmess-aid-list', vm(aid=[1])), ('vmess-aid-huge', vm(aid=10**30)),
    ('vmess-aid-nan', 'vmess://' + b64('{"port": 443, "add": "a", "aid": NaN}')), ('vmess-aid-infinity', 'vmess://' + b64('{"port": 443, "add": "a", "aid": Infinity}')),
    ('vmess-net-number', vm(net=5)), ('vmess-ws-odd-opts', vm(path=5, host=['h'])), ('vmess-name-not-text', vm(ps=5)),
    ('vmess-huge-int-is-not-json', 'vmess://' + b64('{"port": 443, "add": "a", "x": %s}' % ('9' * 4301))),
    ('ss', 'ss://' + SS + '@192.0.2.1:8388#JP-X'), ('ss-whole-base64', 'ss://' + b64('aes-256-gcm:test-password@s.example.com:8388') + '#JP-X'),
    ('ss-plain', 'ss://aes-256-gcm:test%2Dpassword@s.example.com:8388/?plugin=x#x%20y'), ('ss-ipv6', 'ss://' + SS + '@[2001:db8::1]:8388'),
    ('ss-no-colon-in-userinfo', 'ss://' + b64('nocolon') + '@s.example.com:8388'), ('ss-no-port', 'ss://' + SS + '@s.example.com'), ('ss-port-word', 'ss://' + SS + '@s.example.com:abc'),
    ('ss-port-zero', 'ss://' + SS + '@s.example.com:0'), ('ss-no-host', 'ss://' + SS + '@:8388'), ('ss-whole-no-at', 'ss://' + b64('a:b')), ('ss-whole-no-colon', 'ss://' + b64('a@b')),
    ('ss-bad-base64', 'ss://a'), ('ss-whole-colon-only-in-host', 'ss://' + b64('abc@host:8388')), ('ss-nothing', 'ss://'), ('ss-non-ascii', 'ss://中:文@h.example.com:1'),
    ('vless-ws-tls', 'vless://' + ZERO + '@v.example.com:443?type=ws&path=%2Fws%3Fed%3D2048&host=h.example.com&security=tls&sni=sni.example.com&fp=chrome#JP-X'),
    ('vless-reality', 'vless://' + ZERO + '@v.example.com:443?security=reality&pbk=test-public-key&sid=abcd&flow=xtls-rprx-vision&type=tcp#r'),
    ('vless-reality-bare', 'vless://u@v.example.com:443?security=reality'), ('vless-grpc', 'vless://u@v.example.com:443?type=grpc&serviceName=svc'),
    ('vless-net-fallback', 'vless://u@v.example.com:443?type=&net=ws&host=h'), ('vless-first-value-wins', 'vless://u@v.example.com:443?type=ws&type=grpc&sni=a&sni=b'),
    ('vless-no-port', 'vless://u@v.example.com'), ('vless-port-zero', 'vless://u@v.example.com:0'), ('vless-port-word', 'vless://u@v.example.com:abc'),
    ('vless-port-too-big', 'vless://u@v.example.com:99999'), ('vless-bad-bracket', 'vless://u@[::1:443'), ('vless-ipv4-in-brackets', 'vless://u@[1.2.3.4]:443'),
    ('vless-ipv6', 'vless://u@[2001:DB8::1]:443'), ('vless-no-host', 'vless://:443'), ('vless-nfkc-host', 'vless://u@\u2100.example.com:1'),
    ('vless-tab-and-newline', 'vless://u@v.' + '\t' + 'example.com:44' + '\n' + '3?type=ws'),
    ('vless-encoded-everything', 'vless://u%40x:p%3Aq@V.Example.COM:1?type=ws&path=%E4%B8%AD+x&host=%FF#%E5%90%8D%E5%AD%97'),
    ('trojan', 'trojan://test-password@t.example.com:443?sni=s.example.com&allowInsecure=1#JP-X'),
    ('trojan-password-after-colon', 'trojan://:test-password@t.example.com:443?allowInsecure=true'),
    ('trojan-no-password', 'trojan://t.example.com:443?allowInsecure=TRUE&type=ws&path=%2F'),
    ('hysteria2', 'hysteria2://test-password@y.example.com:8443?sni=s.example.com&obfs=salamander&obfs-password=test-obfs&insecure=1#JP-X'),
    ('hy2', 'hy2://:test-password@y.example.com:8443?insecure=0'),
    ('tuic', 'tuic://' + ZERO + ':test-password@u.example.com:443?sni=s.example.com&congestion_control=bbr&alpn=h3,h3-29#JP-X'),
    ('tuic-token', 'tuic://token@u.example.com:443?alpn=,'), ('scheme-long-s', 'ſs://' + SS + '@h.example.com:1'), ('scheme-dotless-i', 'tuıc://u@h.example.com:1'),
    ('scheme-dotted-capital-i', 'hysterİa2://p@h.example.com:1'), ('scheme-space', 'trojan ://p@h.example.com:1'),
    ('json', json.dumps(dict(B, name='JP-X'))), ('json-list', json.dumps([dict(B, name='JP-X'), A])), ('json-config', json.dumps({'proxies': [dict(H, name='JP-X')], 'rules': []})),
    ('json-empty-list', '[]'), ('json-number', '5'), ('json-text', '"abc"'), ('json-empty-object', '{}'), ('json-no-server', '{"type": "ss"}'), ('json-no-port', '{"type": "ss", "server": "s"}'),
    ('json-port-zero', '{"type": "ss", "server": "s", "port": 0}'), ('json-port-text', '{"type": "ss", "server": "s", "port": " 80 "}'),
    ('json-port-float', '{"type": "ss", "server": "s", "port": 80.0}'), ('json-port-true', '{"type": "ss", "server": "s", "port": true}'),
    ('json-port-list', '{"type": "ss", "server": "s", "port": [1]}'), ('json-port-huge', '{"type": "ss", "server": "s", "port": 99999999999999999999}'),
    ('json-type-number', '{"type": 5, "server": "s", "port": 1}'), ('json-server-list', '{"type": "ss", "server": ["s"], "port": 1}'),
    ('json-keeps-everything', '{"type": "ss", "server": "s", "port": 1, "name": "JP-X", "x": [1, 2.5, null, true, {"y": "z"}], "n": NaN, '
                              '"big": 123456789012345678901234567890, "f": 1e-7, "名": "值\\u0000"}'),
    ('json-duplicate-keys', '{"type": "ss", "server": "s", "port": 1, "type": "vmess", "name": "a", "name": "b"}'), ('json-trailing-comma', '{"type": "ss", "server": "s", "port": 1,}'),
    ('flow', '{name: JP-X, type: ss, server: s.example.com, port: 8388, cipher: aes-256-gcm, password: test-password, udp: true}'),
    ('flow-quotes-and-nesting', '{name: "a, b", type: ss, server: s, port: "443", opts: {path: /x}, alpn: [h2, h3], q: \'c: d\'}'), ('flow-no-colon', '{type: ss, server: s, port}'),
    ('flow-odd-keys', '{1: a, "1": b, true: c, ~: d, : e, type: ss, server: s, port: 5}'), ('flow-list', '[{name: a, type: ss, server: s, port: 1}]'), ('flow-unbalanced', '{type: ss'),
    ('flow-port-leading-zero', '{type: ss, server: s, port: 007}'), ('flow-huge-int', '{type: ss, server: s, port: 1, n: %s}' % ('9' * 4301)),
    ('yaml', YAML), ('yaml-list-item', '- name: JP-X\n  type: ss\n  server: s.example.com\n  port: 8388\n- name: second\n'),
    ('yaml-config', 'mixed-port: 7890\nproxies:\n  # 第一个\n  - name: JP-X\n    type: vmess\n    server: v.example.com\n    port: 443\n    uuid: %s\n    alterId: 0\n    cipher: auto\n'
                    '    tls: true\n    network: ws\n    ws-opts:\n      path: /ws\n      headers:\n        Host: h.example.com\n  - name: second\n    type: ss\nproxy-groups:\n  - name: g\n'
                    '    proxies: [JP-X]\n' % ZERO),
    ('yaml-config-flow-items', 'proxies:\n  - {name: JP-X, type: ss, server: s.example.com, port: 443}\n  - {name: n2}\n'), ('yaml-crlf', YAML.replace('\n', '\r\n')),
    ('yaml-document-marks', '---\n' + YAML + '...\n'), ('yaml-tabs', YAML.replace('name', '\tname')), ('yaml-indented', ''.join('  ' + l + '\n' for l in YAML.splitlines())),
    ('yaml-no-space-after-colon', 'name:a\ntype: ss'), ('yaml-bad-indent', YAML + '  extra: 1\n'), ('yaml-nested-dedent', YAML + 'opts:\n    a: 1\n  b: 2\n'),
    ('yaml-quoted-keys', '"name": "JP-X"\n\'type\' : ss\n"ser:ver": x\nserver: s\nport: \'8388\'\n'),
    ('yaml-odd-keys', 'type: ss\nserver: s\nport: 1\n1: x\n"1": y\ntrue: z\n~: n\n0: f\nfalse: g\n'),
    ('yaml-values', 'type: ss\nserver: s\nport: 1\nk: [1, "two", three, 4.5, null, ~, true]\nl: {a: 1, b: [x, y]}\nm: "q: v"\nn: NaN\no: 1e3\np: -0\nq:\nurl: http://x:80/y?z=1\n'),
    ('yaml-port-block', 'type: ss\nserver: s\nport:\n  a: 1\n  2: [x]\n'), ('yaml-port-plus', 'type: ss\nserver: s\nport: +5'), ('yaml-port-arabic', 'type: ss\nserver: s\nport: ٥٠'),
    ('yaml-dash-only', '-'), ('yaml-scalar-list', '- a\n- b'), ('yaml-proxies-empty', 'proxies: []'), ('yaml-proxies-scalars', 'proxies:\n  - a\n  - b'),
    ('yaml-comment-only', '# nothing here\n---\n'), ('yaml-unicode-space', 'type:\u3000ss\nserver:\u00a0s\nport: 1\n'),
    ('yaml-huge-int', 'type: ss\nserver: s\nport: 1\nk: %s\n' % ('7' * 4301)), ('yaml-uppercase-keys', 'TYPE: ss\nserver: s\nport: 1'),
    # Go 的 yaml 库写出来的样子：列表不缩进。合法的 YAML，这里读不了（见 FINDINGS.md）
    ('yaml-list-not-indented-under-key', 'proxies:\n- name: JP-X\n  type: ss\n  server: s.example.com\n  port: 443\n'),
    ('yaml-list-item-with-nested-blocks', '- name: JP-X\n  type: vless\n  server: v.example.com\n  port: 443\n  uuid: %s\n  ws-opts:\n    path: /ws\n    headers:\n      Host: h.example.com\n'
                                          '  alpn:\n    - h2\n    - http/1.1\n  reality-opts: {public-key: test-public-key, short-id: abcd}\n' % ZERO),
]
for name, text in PARSE:
    check('parse/' + name, text)


def nested(n, last='k: 1'):
    """块写法一层套一层，套 n 层（每层多缩一格），最里面是 last"""
    return 'type: ss\nserver: s.example.com\nport: 1\n' + ''.join('%sk:\n' % (' ' * i) for i in range(n)) + ' ' * n + last + '\n'


# 块是一层层递归着读的：原来套到一千层上下是没人接的 RecursionError。具体第几层随最里面那一行写的是什么、Python 的版本差两三层
# （实测套到 987～991 层还行），Rust 这边固定成套 990 层还行：这里只比离界线远的（见 FINDINGS.md）
check('parse/yaml-nested-deep', nested(960))
check('parse/yaml-nested-too-deep', nested(1020))
check('parse/yaml-nested-too-deep-list-inside', nested(1020, last='- a'))

# 起名字：去掉控制字符、首尾空格、开头的 CCA-，截到 128；没有就用类型；被占了的接 -2、-3……
NAMED = 'trojan://test-password@t.example.com:443#'
for name, frag, kw in [
    ('free', 'JP-X', {}), ('taken-by-nodes', 'US-A', {}), ('taken-twice', 'US-A', {'cfg': {'nodes': NODES + ['US-A-2', 'US-A-3']}}), ('taken-by-cache', 'JP-X', {'defs': [dict(A, name='JP-X')]}),
    ('taken-by-fill', 'US-C', {'fill': FILL}), ('custom-nodes-do-not-count', 'MINE', {'mine': [dict(B, name='MINE')]}), ('builtin-direct', 'DIRECT', {}), ('builtin-reject', ' REJECT ', {}),
    ('builtin-global', 'GLOBAL', {}), ('no-name', '', {}), ('no-name-type-taken', '', {'cfg': {'nodes': NODES + ['trojan', 'trojan-2']}}), ('cca-prefix', 'CCA-CCA-%20x%20', {}),
    ('only-cca-prefix', 'CCA-', {}), ('control-chars', 'a%09b%7Fc%00d', {}), ('long', 'n' * 300, {}), ('long-taken', 'n' * 300, {'cfg': {'nodes': ['n' * 128]}, 'table': {}}),
    ('spaces-inside-after-cut', 'x' * 126 + '%20%20yz', {}), ('unicode', '%E5%90%8D%E5%AD%97%20%F0%9F%98%80', {}), ('case-matters', 'us-a', {}),
    ('nodes-is-text', 'S', {'cfg': {'nodes': 'US'}, 'table': {}}), ('nodes-is-object', 'k', {'cfg': {'nodes': {'k': 1}}, 'table': {}}), ('nodes-is-number', 'x', {'cfg': {'nodes': 5}, 'table': {}}),
    ('nodes-is-null', 'x', {'cfg': {'nodes': None}, 'table': {}}), ('nodes-has-a-list', 'x', {'cfg': {'nodes': ['US-A', ['x']]}, 'table': {}}),
    ('nodes-has-odd-items', '1', {'cfg': {'nodes': [1, True, None, 1.5]}, 'table': {}}), ('cache-entry-is-text', 'x', {'defs': [A, 'junk']}), ('cache-entry-has-no-name', 'x', {'defs': [A, {'type': 'ss'}]}),
    ('cache-name-is-a-list', 'x', {'defs': [dict(A, name=['x'])]}), ('cache-name-is-a-number', '5', {'defs': [dict(A, name=5)]}), ('cache-entry-is-null', 'x', {'defs': [None]}),
    ('bad-fill-ignored', 'US-C', {'fill': dict(FILL, version=2)}),
]:
    check('name/' + name, NAMED + frag, **kw)

check('name/from-json-not-text', '{"name": ["x"], "type": "ss", "server": "s", "port": 1}')
check('name/from-yaml-number', 'name: 5\ntype: ss\nserver: s\nport: 1')
check('name/appended-last-when-missing', 'type: ss\nserver: s\nport: 1\ncipher: none')
check('codex', vm(), tool='codex', codex={'erin@example.com': 'HK-A'})
check('readonly', vm(), env={'CCA_RO': '1'})
check('args', vm(), argv=['JP-X', 'JP-X-2'])   # 要避开的名字（在家时路由器上的）：起成 JP-X-3


@case('node-check/cache-proxies-not-a-list', ['node-check'], stdin=NAMED + 'US-C', env=ENV)
def _(b):
    home(b)
    cache(b, {'US-C': 1})


@case('node-check/bad-config-after-parse', ['node-check'], stdin=vm(), env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('node-check/bad-config-parse-error-first', ['node-check'], stdin='ssr://x', env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('node-check/empty-home', ['node-check'], stdin=vm(), env=ENV)
def _(b):
    net(b)


# ---- node-testconfig：只含这一个节点的最小配置（给内核 -t 校验）--------------------------------------------------------------

for name, text in [
    ('node', json.dumps(dict(A, name='JP-X'), ensure_ascii=False)),
    ('unicode', json.dumps({'name': '名字 😀', 'type': 'ss', 'server': 's', 'port': 1, 'x': 'é\u0000'}, ensure_ascii=False)),
    ('ascii-escapes', json.dumps({'name': '名字', 'type': 'ss'})), ('name-number', '{"name": 5}'), ('name-null', '{"name": null, "type": "ss"}'), ('name-list', '{"name": ["a"]}'),
    ('no-name', '{"type": "ss"}'), ('list', '[{"name": "a"}]'), ('text', '"abc"'), ('number', '7'), ('null', 'null'), ('not-json', 'name: a'), ('empty', ''), ('trailing', '{"name": "a"} x'),
    ('nan-and-big', '{"name": "a", "n": NaN, "i": -Infinity, "big": 123456789012345678901234567890, "f": 1.0}'), ('huge-int', '{"name": "a", "n": %s}' % ('3' * 4301)),
    ('duplicate-name', '{"name": "a", "type": "ss", "name": "b"}'), ('crlf', '{"name": "a",\r\n "type": "ss"}\r\n'),
]:
    @case('node-testconfig/' + name, ['node-testconfig'], stdin=text, env=ENV)
    def _(b):
        net(b)


@case('node-testconfig/args', ['node-testconfig', 'x'], stdin='{"name": "a"}', env=ENV)
def _(b):
    net(b)


# ---- node-save：存进 core/custom-nodes.json，名字加进可选节点、记下摘要 ---------------------------------------------------------

NEW = dict(HY, name='JP-X')
for name, text, kw in [
    ('first', NEW, {}), ('second', NEW, {'mine': [dict(B, name='MINE')]}), ('replaces-same-name', NEW, {'mine': [dict(B, name='JP-X'), dict(H, name='MINE')]}),
    ('already-in-nodes', dict(NEW, name='US-A'), {'cfg': {'node_ids': {'US-A': 'old', 'gone': 'x'}}}),
    ('drops-junk-entries', NEW, {'mine': ['junk', {'type': 'ss'}, dict(B, name='MINE'), dict(B, name=5), None]}),
    ('custom-proxies-not-a-list', NEW, {'mine': {'version': 1, 'proxies': {'a': 1}}}), ('custom-extra-fields-dropped', NEW, {'mine': {'version': 9, 'proxies': [], 'note': 'x'}}),
    ('ids-not-an-object', NEW, {'cfg': {'node_ids': ['x']}}), ('no-server-no-id', {'name': 'JP-X', 'type': 'ss'}, {}), ('sni-in-id', dict(NEW, sni='sni.example.com'), {}),
    ('servername-wins', dict(NEW, sni='sni.example.com', servername='sn.example.com'), {}), ('nodes-is-text-contains', dict(NEW, name='S-'), {'cfg': {'nodes': 'US-A'}, 'table': {}}),
    ('nodes-is-text-new', NEW, {'cfg': {'nodes': 'US'}, 'table': {}}), ('nodes-is-object', NEW, {'cfg': {'nodes': {'JP-X': 1}}, 'table': {}}),
    ('nodes-is-object-new', NEW, {'cfg': {'nodes': {'k': 1}}, 'table': {}}), ('nodes-is-number', NEW, {'cfg': {'nodes': 5}, 'table': {}}),
    ('nodes-is-null', NEW, {'cfg': {'nodes': None}, 'table': {}}), ('nodes-empty', NEW, {'cfg': {'nodes': []}, 'table': {}}),
    ('name-number', dict(NEW, name=5), {}), ('name-number-already-there', dict(NEW, name=5), {'cfg': {'nodes': NODES + [5.0], 'node_ids': {'x': 'old'}}}),
    ('name-null', dict(NEW, name=None), {}), ('name-true', dict(NEW, name=True), {}), ('name-float', dict(NEW, name=1.5), {}), ('name-list', dict(NEW, name=['x']), {}),
    ('name-object', dict(NEW, name={'a': 1}), {}), ('name-number-in-text-nodes', dict(NEW, name=5), {'cfg': {'nodes': 'US'}, 'table': {}}),
    ('name-list-in-object-nodes', dict(NEW, name=['x']), {'cfg': {'nodes': {'k': 1}}, 'table': {}}),
    ('no-name', {'type': 'ss'}, {}), ('list', [NEW], {}), ('text', 'JP-X', {}), ('number', 7, {}),
]:
    @case('node-save/' + name, ['node-save'], stdin=json.dumps(text, ensure_ascii=False), env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


for name, text in [('not-json', '{"name": '), ('empty', ''), ('huge-int', '{"name": "a", "n": %s}' % ('3' * 4301))]:
    @case('node-save/stdin-' + name, ['node-save'], stdin=text, env=ENV)
    def _(b):
        home(b)


@case('node-save/custom-file-not-json', ['node-save'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    home(b)
    os.makedirs(b.path('.cca/core'), 0o700)
    b.write('.cca/core/custom-nodes.json', '{"proxies": [')


@case('node-save/custom-file-is-a-link', ['node-save'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    home(b)
    b.json('elsewhere.json', {'version': 1, 'proxies': [dict(B, name='MINE')]})
    os.makedirs(b.path('.cca/core'), 0o700)
    os.symlink(b.path('elsewhere.json'), b.path('.cca/core/custom-nodes.json'))


@case('node-save/custom-file-loose-mode', ['node-save'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    home(b, mine=[])
    os.chmod(b.path('.cca/core/custom-nodes.json'), 0o644)
    os.chmod(b.path('.cca/core'), 0o755)


@case('node-save/bad-config-after-saving', ['node-save'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('node-save/empty-home', ['node-save'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    net(b)


@case('node-save/codex-readonly', ['node-save'], stdin=json.dumps(NEW), env=dict(ENV, CCA_RO='1'), tool='codex')
def _(b):
    home(b, codex={'erin@example.com': 'HK-A'}, cfg={'codex': {'ports': {'erin@example.com': 7904}, 'last': ''}})


@case('node-save/args', ['node-save', 'x'], stdin=json.dumps(NEW), env=ENV)
def _(b):
    home(b)


# ---- node-ls / node-rm -------------------------------------------------------------------------------------------------

MINE = [dict(HY, name='JP-X'), dict(B, name='MINE'), {'name': '光名字'}, dict(H, name='零', port=0, type='', server=None), dict(H, name='怪', port='8443', type=5, server=['s'])]
USERS = {'erin@example.com': 'JP-X', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'JP-X'}, '科(frank@example.com)': 'MINE'}

for name, kw in [
    ('none', {}), ('several', {'mine': MINE}),
    ('with-users', {'mine': MINE, 'cfg': {'nodes': NODES + ['JP-X', 'MINE']}, 'table': dict(TABLE, **{'carol@example.com': 'JP-X'}), 'codex': USERS, 'agy': {'gina@example.com': 'MINE'}}),
    ('junk', {'mine': ['junk', {'type': 'ss'}, dict(B, name=5), dict(B, name='ok'), None, [1]]}), ('not-object', {'mine': {'version': 1, 'proxies': 'x'}}),
    ('same-name-twice', {'mine': [dict(B, name='MINE'), dict(H, name='MINE')]}),
]:
    @case('node-ls/' + name, ['node-ls'], env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('node-ls/broken-tables', ['node-ls'], env=ENV)
def _(b):
    home(b, mine=MINE, cfg={'nodes': NODES + ['JP-X']}, table=dict(TABLE, **{'carol@example.com': 'JP-X'}))
    b.write('.cca/codex.json', '{broken')
    b.write('.cca/agy.json', '["JP-X"]')


@case('node-ls/args', ['node-ls', 'x'], env=ENV)
def _(b):
    home(b, mine=MINE)


for name, argv, kw in [
    ('removes', ['JP-X'], {'mine': MINE, 'cfg': {'nodes': NODES + ['JP-X', 'MINE'], 'node_ids': dict(IDS, **{'JP-X': nfp(HY), 'MINE': nfp(B)})}}),
    ('not-custom', ['US-A'], {'mine': MINE}), ('none-at-all', ['JP-X'], {}),
    ('in-use-claude', ['JP-X'], {'mine': MINE, 'cfg': {'nodes': NODES + ['JP-X']}, 'table': dict(TABLE, **{'carol@example.com': 'JP-X'})}),
    ('in-use-other-tools', ['JP-X'], {'mine': MINE, 'cfg': {'nodes': NODES + ['JP-X', 'MINE']}, 'codex': USERS, 'agy': {'gina@example.com': 'JP-X'}}),
    ('not-in-nodes-no-ids', ['MINE'], {'mine': MINE}), ('ids-not-an-object', ['MINE'], {'mine': MINE, 'cfg': {'node_ids': 'x'}}),
    ('same-name-twice', ['MINE'], {'mine': [dict(B, name='MINE'), dict(H, name='MINE'), dict(HY, name='JP-X')]}),
    ('keeps-junk-out', ['MINE'], {'mine': ['junk', dict(B, name='MINE'), {'type': 'ss'}, dict(HY, name='JP-X')]}),
    ('nodes-with-odd-items', ['MINE'], {'mine': MINE, 'cfg': {'nodes': ['MINE', 5, None, 'US-A', ['MINE'], 'MINE']}, 'table': {}}),
    ('nodes-is-text', ['MINE'], {'mine': MINE, 'cfg': {'nodes': 'MINE'}, 'table': {}}), ('nodes-is-object', ['MINE'], {'mine': MINE, 'cfg': {'nodes': {'MINE': 1, 'US-A': 2}}, 'table': {}}),
    ('nodes-is-number', ['MINE'], {'mine': MINE, 'cfg': {'nodes': 5}, 'table': {}}), ('nodes-is-zero', ['MINE'], {'mine': MINE, 'cfg': {'nodes': 0}, 'table': {}}),
    ('empty-name', [''], {'mine': MINE}), ('no-args', [], {'mine': MINE}), ('two-args', ['JP-X', 'MINE'], {'mine': MINE}),
]:
    @case('node-rm/' + name, ['node-rm'] + argv, env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('node-rm/custom-file-is-a-link', ['node-rm', 'MINE'], env=ENV)
def _(b):
    home(b)
    b.json('elsewhere.json', {'version': 1, 'proxies': MINE})
    os.makedirs(b.path('.cca/core'), 0o700)
    os.symlink(b.path('elsewhere.json'), b.path('.cca/core/custom-nodes.json'))


@case('node-rm/bad-config-after-saving', ['node-rm', 'MINE'], env=ENV)
def _(b):
    home(b, mine=MINE)
    b.write('.cca/config.json', '{"nodes": [')


@case('node-rm/codex', ['node-rm', 'MINE'], env=ENV, tool='codex')
def _(b):
    home(b, mine=MINE, cfg={'nodes': NODES + ['MINE']}, codex={'erin@example.com': 'HK-A'})


@case('node-rm/readonly', ['node-rm', 'MINE'], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, mine=MINE, cfg={'nodes': NODES + ['MINE'], 'node_ids': {'MINE': nfp(MINE[1])}})


# ---- 在夹具里拿原版 Python 跑一个子命令：造“已经生成过 / 已经装好”的现场 ----------------------------------------------------

ORACLE = []


def oracle(b, *argv, stdin=b'', tool='claude', env=None):
    """在沙箱上拿原版的 PY_SRC 跑一个子命令（夹具用：现场要和它自己写出来的一模一样时，比如“内容没变就不写”、“装着的就是这一份”）"""
    import atexit
    import subprocess
    import sys
    from run import py_src
    if not ORACLE:
        fd, path = tempfile.mkstemp(prefix='cca-pydiff-oracle.', suffix='.py')
        with os.fdopen(fd, 'w', encoding='utf-8') as f:
            f.write(py_src())
        atexit.register(os.remove, path)
        ORACLE.append(path)
    e = {'HOME': b.home, 'PATH': '%s/bin:/usr/bin:/bin:/usr/sbin:/sbin' % b.home, 'LANG': 'en_US.UTF-8', 'TZ': 'Asia/Shanghai', 'CCA_TOOL': tool,
         'CCA_SELF': os.path.join(b.home, '.cca', 'bin', 'cca'), 'CCA_BIN': '', 'CCA_RESOLV_CONF': b.path('resolv.conf')}
    e.update(env or {})
    r = subprocess.run([sys.executable, '-I', '-X', 'utf8', ORACLE[0]] + list(argv), input=stdin, capture_output=True, env=e, cwd=b.home)
    assert r.returncode == 0, (argv, r.stdout, r.stderr)
    if os.path.exists(b.path('net.log')):   # 夹具这一趟问过的不算进用例里
        os.remove(b.path('net.log'))
    return r.stdout.decode('utf-8')


# ---- core-config：写内置代理的配置 core/config.yaml（JSON 写的），内容变了才写 ---------------------------------------------------

SOCK = '/tmp/cca-pydiff/cca-core-0123abcd.sock'
CHAIN = [A, dict(B, **{'dialer-proxy': 'HY-节点'}), dict(H, **{'dialer-proxy': 'US-B'}), HY]
for name, argv, kw in [
    ('no-defs', [SOCK], {'local': BUILTIN}), ('defs', [SOCK], {'local': BUILTIN, 'defs': [A, B, H]}), ('not-configured', [SOCK], {'defs': [A, B, H]}),
    ('renamed', [SOCK], {'local': BUILTIN, 'defs': RENAMED, 'cfg': {'node_ids': IDS}}), ('renamed-but-not-builtin', [SOCK], {'defs': RENAMED, 'cfg': {'node_ids': IDS}}),
    ('one-missing', [SOCK], {'local': BUILTIN, 'defs': [A, B]}), ('extra-defs-left-out', [SOCK], {'local': BUILTIN, 'defs': [HY, A, B, H, dict(HY, name='别的')]}),
    ('dialer-chain', [SOCK], {'local': BUILTIN, 'defs': CHAIN}), ('dialer-to-a-group', [SOCK], {'local': BUILTIN, 'defs': [A, B, dict(H, **{'dialer-proxy': 'OpenAi'})]}),
    ('dialer-loop', [SOCK], {'local': BUILTIN, 'defs': [dict(A, **{'dialer-proxy': 'US-B'}), dict(B, **{'dialer-proxy': 'US-A'}), H]}),
    ('dialer-not-text', [SOCK], {'local': BUILTIN, 'defs': [dict(A, **{'dialer-proxy': 5}), dict(B, **{'dialer-proxy': None}), dict(H, **{'dialer-proxy': ''})]}),
    ('same-name-twice-first-wins', [SOCK], {'local': BUILTIN, 'defs': [A, dict(B, name='US-A'), B, H]}),
    ('fill', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'fill': FILL, 'cfg': {'nodes': NODES + ['US-C', 'US-D']}, 'table': dict(TABLE, **{'carol@example.com': 'US-C'})}),
    ('fill-same-server-uses-theirs', [SOCK], {'local': BUILTIN, 'defs': [A, B, H, dict(FILL['proxies'][0], name='我的 C')], 'fill': FILL,
                                               'cfg': {'nodes': NODES + ['US-C'], 'node_ids': dict(IDS, **{'US-C': nfp(FILL['proxies'][0])})}, 'table': dict(TABLE, **{'carol@example.com': 'US-C'})}),
    ('custom', [SOCK], {'local': BUILTIN, 'defs': [A, B], 'mine': [H, dict(HY, name='JP-X')], 'cfg': {'nodes': NODES + ['JP-X']}, 'table': dict(TABLE, **{'carol@example.com': 'JP-X'})}),
    ('no-accounts', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'table': {}}), ('sixteen-slots', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'ports': {'bob@example.com': 7909}}}),
    ('junk-in-cache', [SOCK], {'local': BUILTIN, 'defs': ['junk', {'type': 'ss'}, A, dict(B, name=5), B, None, H]}),
    ('unicode', [SOCK], {'local': BUILTIN, 'defs': [dict(A, name='美国 😀'), B, H], 'cfg': {'nodes': ['美国 😀', 'US-B', 'HK-A']}, 'table': {'a@example.com': '美国 😀'}}),
    ('nodes-with-odd-items', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'nodes': ['US-A', 5, None, True, 'US-B', 'US-B', 1.5, 'HK-A']}}),
    ('nodes-with-a-list', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'nodes': ['US-A', ['x'], 'US-B', 'HK-A']}}),
    ('reject-in-nodes', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'nodes': ['REJECT', 'US-A', 'US-B', 'HK-A']}}),
    ('bind-off', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'cfg': {'core_bind': False}}), ('route-fails', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'route': None}),
    ('empty-sock', [''], {'local': BUILTIN, 'defs': [A, B, H]}), ('odd-sock', ['/tmp/带 空格/"引号"\\.sock'], {'local': BUILTIN, 'defs': [A, B, H]}),
    ('no-args', [], {'local': BUILTIN}), ('two-args', [SOCK, 'x'], {'local': BUILTIN}), ('table-errors', [SOCK], {'local': BUILTIN, 'defs': [A, B, H], 'table': {'a b': 'US-A', 'bob@example.com': 'ZZ'}}),
    ('local-is-text', [SOCK], {'local': 'builtin', 'defs': [A, B, H]}),
]:
    @case('core-config/' + name, ['core-config'] + argv, env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


for name, tool in [('codex', 'codex'), ('agy', 'agy')]:
    @case('core-config/tool-' + name, ['core-config', SOCK], env=ENV, tool=tool)
    def _(b):
        home(b, local=BUILTIN, defs=RENAMED, cfg={'node_ids': IDS}, codex={'erin@example.com': 'HK-A', '接口': {'base_url': 'https://api.example.com/v1', 'node': 'US-A'}}, agy={'gina@example.com': 'US-B'})


@case('core-config/readonly', ['core-config', SOCK], env=dict(ENV, CCA_RO='1'))
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])


@case('core-config/empty-home', ['core-config', SOCK], env=ENV)
def _(b):
    net(b)


for name, tunkw in [('tun-bind-with-dns', {'dns': ['192.0.2.53', '192.0.2.54']}), ('tun-bind-system-dns-fine', {'dns': ['192.0.2.53'], 'resolv': 'nameserver 192.0.2.53\n'}),
                    ('tun-no-physical', {'phys': ['utun4']})]:
    @case('core-config/' + name, ['core-config', SOCK], env=ENV)
    def _(b, tunkw=tunkw):
        home(b, local=BUILTIN, defs=[A, B, H])
        tun(b, **tunkw)


def existing(change=None, mode=0o644, dirmode=0o755):
    """core/config.yaml 已经在了：先拿原版生成一份（内容就是这次该写的），再照 change 改；权限故意放宽，写没写过看得出来"""
    def setup(b):
        home(b, local=BUILTIN, defs=[A, B, H])
        oracle(b, 'core-config', SOCK)
        p = b.path('.cca/core/config.yaml')
        if change:
            with open(p, 'rb') as f:
                data = f.read()
            with open(p, 'wb') as f:
                f.write(change(data))
        os.chmod(p, mode)
        os.chmod(b.path('.cca/core'), dirmode)
    return setup


for name, setup in [
    ('same-content-not-written', existing()), ('same-content-crlf-not-written', existing(lambda d: d.replace(b'\n', b'\r\n'))), ('changed-is-rewritten', existing(lambda d: d.replace(b'warning', b'info'))),
    ('missing-final-newline-is-rewritten', existing(lambda d: d.rstrip(b'\n'))), ('empty-is-rewritten', existing(lambda d: b'')), ('not-utf8', existing(lambda d: d + b'\xff')),
    ('same-content-other-socket', existing()),
]:
    case('core-config/existing-' + name, ['core-config', SOCK + ('.other' if name.endswith('other-socket') else '')], env=ENV)(setup)


@case('core-config/existing-is-a-link', ['core-config', SOCK], env=ENV)
def _(b):
    home(b, local=BUILTIN, defs=[A, B, H])
    b.write('elsewhere.yaml', 'old\n', 0o644)
    os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
    os.symlink(b.path('elsewhere.yaml'), b.path('.cca/core/config.yaml'))


@case('core-config/core-is-a-file', ['core-config', SOCK], env=ENV)
def _(b):
    home(b, local=BUILTIN)
    b.write('.cca/core', 'not a directory\n')


# ---- core-nodes-save：路由器端 defs 给的节点定义（stdin，带结束标记）→ core/nodes.json -------------------------------------------

END = '\n# cca-defs-end\n'
SAVED = [(r'"saved_at": \d+\.\d+', '"saved_at": <现在>')]


def defs_in(proxies, end=END, **more):
    return json.dumps(dict({'proxies': proxies}, **more), ensure_ascii=False) + end


for name, text, kw in [
    ('saves', defs_in([HY, A, B, H]), {}), ('renamed-by-address', defs_in(RENAMED), {'cfg': {'node_ids': IDS}}), ('renamed-without-ids', defs_in(RENAMED), {}),
    ('dialer', defs_in([A, dict(B, **{'dialer-proxy': 'HY-节点'}), dict(H, **{'dialer-proxy': 'OpenAi'}), HY]), {}), ('junk-entries', defs_in(['junk', {'type': 'ss'}, A, dict(B, name=5), None, B, [H]]), {}),
    ('same-name-twice', defs_in([A, dict(B, name='US-A'), B]), {}), ('none-wanted', defs_in([HY]), {}), ('empty-proxies', defs_in([]), {}), ('extra-top-level', defs_in([A], version=3, other={'k': 1}), {}),
    ('no-nodes', defs_in([A, B, H]), {'cfg': {'nodes': []}, 'table': {}}), ('unicode', defs_in([dict(A, name='美国 😀', note='é\u0000 ')]), {'cfg': {'nodes': ['美国 😀']}, 'table': {}}),
    ('no-end-marker', json.dumps({'proxies': [A]}), {}), ('end-marker-no-final-newline', json.dumps({'proxies': [A]}) + '\n# cca-defs-end', {}), ('end-marker-crlf', defs_in([A], end='\r\n# cca-defs-end\r\n'), {}),
    ('end-marker-twice', defs_in([A]) + END, {}), ('only-end-marker', END, {}), ('empty', '', {}), ('not-json', '{"proxies": [' + END, {}), ('yaml-not-json', 'proxies:\n  - name: US-A' + END, {}),
    ('proxies-not-a-list', defs_in({'US-A': A}), {}), ('top-level-list', json.dumps([A]) + END, {}), ('no-proxies', json.dumps({'nodes': []}) + END, {}), ('huge-int', '{"proxies": [], "n": %s}' % ('5' * 4301) + END, {}),
    ('nodes-with-odd-items', defs_in([A, B, H]), {'cfg': {'nodes': ['US-A', 5, None, 'HK-A']}, 'table': {}}), ('nodes-with-a-list', defs_in([A, B, H]), {'cfg': {'nodes': ['US-A', ['x']]}, 'table': {}}),
    ('nodes-is-text', defs_in([dict(A, name='U')]), {'cfg': {'nodes': 'US'}, 'table': {}}), ('whitespace-around', '\n ' + defs_in([A]), {}),
]:
    @case('core-nodes-save/' + name, ['core-nodes-save'], stdin=text, env=ENV, mask=SAVED)
    def _(b, kw=kw):
        home(b, **kw)


@case('core-nodes-save/replaces-old-and-fixes-modes', ['core-nodes-save'], stdin=defs_in([A, B]), env=ENV, mask=SAVED)
def _(b):
    home(b, defs=[H])
    os.chmod(b.path('.cca/core/nodes.json'), 0o644)
    os.chmod(b.path('.cca/core'), 0o755)


@case('core-nodes-save/replaces-a-link', ['core-nodes-save'], stdin=defs_in([A, B]), env=ENV, mask=SAVED)
def _(b):
    home(b)
    b.write('elsewhere.json', '{"keep": "me"}\n', 0o644)
    os.makedirs(b.path('.cca/core'), 0o700)
    os.symlink(b.path('elsewhere.json'), b.path('.cca/core/nodes.json'))


@case('core-nodes-save/bad-config', ['core-nodes-save'], stdin=defs_in([A]), env=ENV, mask=SAVED)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('core-nodes-save/empty-home', ['core-nodes-save'], stdin=defs_in([A]), env=ENV, mask=SAVED)
def _(b):
    net(b)


@case('core-nodes-save/codex-readonly', ['core-nodes-save'], stdin=defs_in([A, B, H]), env=dict(ENV, CCA_RO='1'), tool='codex', mask=SAVED)
def _(b):
    home(b, codex={'erin@example.com': 'HK-A'})


@case('core-nodes-save/args', ['core-nodes-save', 'x'], stdin=defs_in([A]), env=ENV, mask=SAVED)
def _(b):
    home(b)


# ---- core-stale：在家启动后要在后台做的（nodes：重新拷节点定义；bin：下载内核文件）------------------------------------------------

HOUR = 3600


def stale(name, argv=(), saved=ABSENT, nodes=None, raw=None, more=None, tool='claude', **kw):
    """saved：缓存是多少秒以前拷的（ABSENT：没有缓存；None：缓存里没写时间）"""
    @case('core-stale/' + name, ['core-stale'] + list(argv), env=ENV, tool=tool)
    def _(b):
        home(b, **kw)
        if raw is not None:
            os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
            b.write('.cca/core/nodes.json', raw)
        elif saved is not ABSENT:
            cache(b, [A, B, H], saved=saved, nodes=nodes, **(more or {}))


stale('no-cache')
stale('fresh', saved=HOUR)
stale('almost-old', saved=12 * HOUR - 120)
stale('old', saved=12 * HOUR + 120)
stale('from-the-future', saved=-HOUR)
stale('no-time-recorded', saved=None)
stale('nodes-changed', saved=HOUR, nodes=['US-A', 'US-B'])
stale('nodes-reordered', saved=HOUR, nodes=['US-B', 'US-A', 'HK-A'])
stale('nodes-not-recorded', saved=HOUR, raw=json.dumps({'saved_at': 4102444800, 'proxies': []}))
stale('time-is-zero', raw=json.dumps({'saved_at': 0, 'nodes': NODES, 'proxies': []}))
stale('time-is-false', raw=json.dumps({'saved_at': False, 'nodes': NODES, 'proxies': []}))
stale('time-is-true', raw=json.dumps({'saved_at': True, 'nodes': NODES, 'proxies': []}))
stale('time-is-float', raw=json.dumps({'saved_at': 4102444800.5, 'nodes': NODES, 'proxies': []}))
stale('time-is-text', raw=json.dumps({'saved_at': '4102444800', 'nodes': NODES, 'proxies': []}))
stale('time-is-a-list', raw=json.dumps({'saved_at': [1], 'nodes': NODES, 'proxies': []}))
stale('time-is-nan', raw='{"saved_at": NaN, "nodes": %s, "proxies": []}' % json.dumps(NODES))
stale('time-is-huge', raw='{"saved_at": %s, "nodes": %s, "proxies": []}' % ('9' * 30, json.dumps(NODES)))
# 大到放不进小数的整数：减的时候出错（OverflowError）。界线是最大的小数和 2**1024 正中间（照“四舍六入五成双”进到 2**1024 就算放不进）
for name, n in [('time-is-too-big-for-a-float', int('9' * 400)), ('time-is-too-small-for-a-float', -int('9' * 400)), ('time-is-the-largest-float', int(1.7976931348623157e308)),
                ('time-is-just-below-the-float-limit', 2 ** 1024 - 2 ** 970 - 1), ('time-is-at-the-float-limit', 2 ** 1024 - 2 ** 970), ('time-is-two-to-the-1024', 2 ** 1024)]:
    stale(name, raw='{"saved_at": %d, "nodes": %s, "proxies": []}' % (n, json.dumps(NODES)))
stale('cache-not-json', raw='{"proxies": [')
stale('cache-proxies-not-a-list', raw=json.dumps({'saved_at': 4102444800, 'nodes': NODES, 'proxies': {}}))
stale('cache-is-a-list', raw='[1]')
stale('core-off', local={'core_off': '1'})
stale('core-off-zero', local={'core_off': 0})
stale('old-client', local={'client': 'clash-verge'})
stale('client-number', local={'client': 5})
stale('client-empty', local={'client': ''})
stale('builtin-no-cache', local=BUILTIN)
stale('builtin-fresh', local=BUILTIN, saved=HOUR)
stale('builtin-have-bin', argv=['1'], local=BUILTIN, saved=HOUR)
stale('builtin-have-bin-empty-arg', argv=[''], local=BUILTIN, saved=HOUR)
stale('builtin-have-bin-no-cache', argv=['yes'], local=BUILTIN)
stale('not-builtin-no-bin', saved=HOUR)
stale('no-router', cfg={'router': {}}, local=BUILTIN)
stale('router-no-host', cfg={'router': {'port': 22, 'host': ''}}, local=BUILTIN)
stale('router-null', cfg={'router': None})
stale('router-is-text', cfg={'router': '192.0.2.10'})
stale('router-is-text-not-eligible', cfg={'router': '192.0.2.10'}, local={'core_off': '1'})
stale('router-is-text-builtin-have-bin', argv=['1'], cfg={'router': '192.0.2.10'}, local={'client': 'x'})
stale('local-is-text', local='builtin')
stale('local-is-empty-list', local=[])
stale('two-args', argv=['1', '2'])
stale('codex', tool='codex', codex={'erin@example.com': 'HK-A'}, local=BUILTIN, saved=13 * HOUR)


@case('core-stale/bad-config', ['core-stale'], env=ENV)
def _(b):
    home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('core-stale/empty-home', ['core-stale'], env=ENV)
def _(b):
    net(b)


# ---- core-missing / core-summary --------------------------------------------------------------------------------------------

for name, kw in [
    ('none', {'defs': [A, B, H]}), ('some', {'defs': [B]}), ('no-cache', {}), ('renamed', {'defs': RENAMED, 'cfg': {'node_ids': IDS}}), ('renamed-without-ids', {'defs': RENAMED}),
    ('dialer-broken', {'defs': [A, B, dict(H, **{'dialer-proxy': 'OpenAi'})]}), ('fill-supplies', {'defs': [A, B, H], 'fill': FILL, 'cfg': {'nodes': NODES + ['US-C', 'US-D', 'US-E']}}),
    ('custom-supplies', {'defs': [A], 'mine': [B], 'local': BUILTIN}), ('no-nodes', {'cfg': {'nodes': []}, 'table': {}}), ('nodes-with-a-number', {'defs': [A, B, H], 'cfg': {'nodes': ['US-A', 5]}, 'table': {}}),
    ('nodes-with-a-list', {'defs': [A, B, H], 'cfg': {'nodes': [['US-A']]}, 'table': {}}), ('nodes-is-text', {'defs': [dict(A, name='U')], 'cfg': {'nodes': 'US'}, 'table': {}}),
    ('nodes-is-null', {'cfg': {'nodes': None}, 'table': {}}), ('unicode', {'defs': [A], 'cfg': {'nodes': ['US-A', '美国 😀', 'a\tb']}, 'table': {}}),
]:
    @case('core-missing/' + name, ['core-missing'], env=ENV)
    def _(b, kw=kw):
        home(b, **kw)


@case('core-missing/args', ['core-missing', 'x'], env=ENV)
def _(b):
    home(b)


def summary(name, saved=ABSENT, raw=None, proxies=None, **kw):
    @case('core-summary/' + name, ['core-summary'], env=ENV)
    def _(b):
        home(b, **kw)
        if raw is not None:
            os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
            b.write('.cca/core/nodes.json', raw)
        elif saved is not ABSENT:
            cache(b, [A, B, H] if proxies is None else proxies, saved=saved)


def at(v):
    return '{"saved_at": %s, "nodes": [], "proxies": [%s]}' % (v, json.dumps(A))


summary('no-cache')
summary('just-now', saved=5)
summary('minutes', saved=30 * 60 + 30)
summary('fifty-nine-minutes', saved=59 * 60 + 30)
summary('one-hour', saved=HOUR + 30)
summary('hours', saved=5 * HOUR + 1800)
summary('forty-seven-hours', saved=47 * HOUR + 1800)
summary('two-days', saved=48 * HOUR + 1800)
summary('days', saved=10 * 86400 + 43200)
summary('from-the-future', saved=-HOUR)
summary('no-time-recorded', saved=None)
summary('time-is-true', raw=at('true'))
summary('time-is-text', raw=at('"yesterday"'))
summary('time-is-nan', raw=at('NaN'))
summary('time-is-infinity', raw=at('Infinity'))
summary('time-is-minus-infinity', raw=at('-Infinity'))
summary('time-is-very-negative', raw=at('-1e300'))
summary('time-is-huge', raw=at('1e300'))
summary('time-is-too-big-for-a-float', raw=at('9' * 400))
summary('time-is-a-very-negative-integer', raw=at('-' + '9' * 300))
summary('missing-nodes', saved=600, proxies=[B])
summary('fill-fills-in', saved=600, fill=FILL, cfg={'nodes': NODES + ['US-C', 'US-D']})
summary('fill-not-used-not-counted', saved=600, fill=FILL)
summary('custom-nodes-are-counted-as-fill', saved=600, proxies=[A, B], mine=[H, dict(HY, name='JP-X')], cfg={'nodes': NODES + ['JP-X']})
summary('everything', saved=3 * 86400 + 43200, proxies=[A, 'junk', 5], fill=FILL, cfg={'nodes': NODES + ['US-C', 'ZZ']})
summary('no-cache-but-fill-and-custom', fill=FILL, mine=[B], cfg={'nodes': NODES + ['US-D']})
summary('renamed', saved=600, proxies=RENAMED, cfg={'node_ids': IDS})
summary('junk-not-objects-ignored', saved=600, proxies=[A, 'junk', None, 5, ['x'], B, H])
summary('object-without-name', saved=600, proxies=[A, {'type': 'ss'}])
summary('name-is-a-list', saved=600, proxies=[A, dict(B, name=['x'])])
summary('name-is-a-number', saved=600, proxies=[A, dict(B, name=5), B, H])
summary('cache-not-json', raw='{"proxies": [')
summary('missing-has-a-number', saved=600, cfg={'nodes': ['US-A', 5]}, table={})
summary('nodes-with-a-list', saved=600, cfg={'nodes': [['US-A']]}, table={})
summary('no-nodes', saved=600, cfg={'nodes': []}, table={})


@case('core-summary/args', ['core-summary', 'x'], env=ENV)
def _(b):
    home(b)


@case('core-summary/bad-config', ['core-summary'], env=ENV)
def _(b):
    home(b, defs=[A])
    b.write('.cca/config.json', '{"nodes": [')


# ---- install-env：辅助安装——官方安装脚本用什么 shell、另加哪些环境变量 ---------------------------------------------------------

def inst(name, tool='claude', settings=None, argv=('%HOME%/inst dir',), pre=None):
    @case('install-env/' + name, ['install-env'] + list(argv), env=ENV, tool=tool)
    def _(b):
        net(b)
        if settings is not None:
            b.write('.claude/settings.json', settings if isinstance(settings, (str, bytes)) else json.dumps(settings), 0o644)
        if pre:
            pre(b)


inst('claude')
inst('claude-latest', settings={'autoUpdatesChannel': 'latest', 'env': {'X': '1'}})
inst('claude-stable', settings={'autoUpdatesChannel': 'stable'})
inst('claude-unknown-channel', settings={'autoUpdatesChannel': 'beta'})
inst('claude-channel-not-text', settings={'autoUpdatesChannel': ['stable']})
inst('claude-settings-not-json', settings='{"autoUpdatesChannel": "stable"')
inst('claude-settings-not-object', settings='["stable"]')
inst('claude-folder-exists', pre=lambda b: os.makedirs(b.path('inst dir/claude'), 0o700))
inst('claude-stable-settings-exist', settings={'autoUpdatesChannel': 'stable'}, pre=lambda b: b.write('inst dir/claude/settings.json', '{"old": true}\n', 0o644))
inst('claude-folder-is-a-file', pre=lambda b: b.write('inst dir', 'x'))
inst('claude-relative', argv=('rel',))
inst('claude-empty-folder-arg', argv=('',))
inst('codex', tool='codex', settings={'autoUpdatesChannel': 'stable'})
inst('agy', tool='agy')
inst('no-args', argv=())
inst('two-args', argv=('a', 'b'))


# ---- install-node：辅助安装走哪个节点、临时内核的配置 ----------------------------------------------------------------------------

PORT = [(r'\A\d{4,5}\n\Z', '<空闲端口>\n'), (r'("listen": "127\.0\.0\.1",\s+"port": )\d+', r'\1<空闲端口>')]
PROXY = ['proxy', 'US-A', '%HOME%/inst/proxy.json']
THEIRS = dict(FILL['proxies'][1], name='我这边的 D')     # 和参考节点 US-D 是同一个服务器，叫法不同


def node(name, argv, env=None, tool='claude', pre=None, **kw):
    @case('install-node/' + name, ['install-node'] + list(argv), env=dict(ENV, **(env or {})), tool=tool, mask=PORT)
    def _(b):
        home(b, **kw)
        os.makedirs(b.path('inst'), 0o700)
        if pre:
            pre(b)


node('get-no-policy', ['get'])
node('get-default-action', [], policy=POLICY, fill=FILL)
node('get-in-fill', ['get'], policy=POLICY, fill=FILL)
node('get-in-cache', ['get'], policy=dict(POLICY, install_node='US-B'), defs=[A, B, H])
node('get-in-custom', ['get'], policy=dict(POLICY, install_node='JP-X'), mine=[dict(HY, name='JP-X')])
node('get-not-defined', ['get'], policy=POLICY)
node('get-not-set', ['get'], policy={'version': 1, 'claude': {}})
node('get-builtin-name-ignored', ['get'], policy=dict(POLICY, install_node='DIRECT'), fill=FILL)
node('get-not-text-ignored', ['get'], policy=dict(POLICY, install_node=5), fill=FILL)
node('get-bad-name-ignored', ['get'], policy=dict(POLICY, install_node='CCA-1'), fill=FILL)
node('get-renamed-on-router', ['get'], policy=dict(POLICY, install_node='OLD'), defs=[A, B, H], cfg={'node_renames': {'OLD': 'MID', 'MID': 'US-B'}})
node('get-by-recorded-address', ['get'], policy=dict(POLICY, install_node='OLD'), defs=RENAMED, cfg={'node_ids': {'OLD': nfp(H)}})
node('get-by-fill-address', ['get'], policy=POLICY, fill=FILL, defs=[A, THEIRS])
node('get-recorded-address-unknown', ['get'], policy=dict(POLICY, install_node='OLD'), defs=RENAMED, cfg={'node_ids': {'OLD': 'feedfeedfeedfeed'}})
node('get-recorded-id-not-text', ['get'], policy=dict(POLICY, install_node='OLD'), defs=RENAMED, cfg={'node_ids': {'OLD': 5}})
node('get-recorded-id-is-a-list', ['get'], policy=dict(POLICY, install_node='OLD'), defs=RENAMED, cfg={'node_ids': {'OLD': ['x']}})
node('get-node-without-server-never-matches', ['get'], policy=dict(POLICY, install_node='OLD'), defs=[{'name': 'bare', 'type': 'ss'}], cfg={'node_ids': {'OLD': ''}})
node('get-codex', ['get'], tool='codex', policy=POLICY, fill=FILL, codex={'erin@example.com': 'HK-A'})
node('get-extra-args-ignored', ['get', 'x', 'y'], policy=POLICY, fill=FILL)
node('choices', ['choices'], defs=[A, B, 'junk', dict(B, name='US-A'), H], mine=[dict(HY, name='JP-X'), dict(H, name='HK-A')], fill=FILL)
node('choices-none', ['choices'])
node('choices-fill-only', ['choices'], fill=FILL)
node('choices-fill-same-server', ['choices'], fill=FILL, defs=[THEIRS])
node('unknown-action', ['list'])
node('empty-action', [''])
node('too-many-args', ['proxy', 'US-A', 'f', 'x'], defs=[A])
node('proxy', PROXY, defs=[A, B, H])
node('proxy-dialer-chain', ['proxy', 'HK-A', '%HOME%/inst/proxy.json'], defs=CHAIN)
node('proxy-fill-node', ['proxy', 'US-D', '%HOME%/inst/proxy.json'], fill=FILL)
node('proxy-custom-node', ['proxy', 'JP-X', '%HOME%/inst/proxy.json'], mine=[dict(HY, name='JP-X')])
node('proxy-not-defined', PROXY, defs=[B, H])
node('proxy-dialer-broken', ['proxy', 'HK-A', '%HOME%/inst/proxy.json'], defs=[A, dict(H, **{'dialer-proxy': 'OpenAi'})])
node('proxy-no-name', ['proxy'], defs=[A])
node('proxy-no-file', ['proxy', 'US-A'], defs=[A])
node('proxy-file-exists', PROXY, defs=[A], pre=lambda b: b.write('inst/proxy.json', 'old\n'))
node('proxy-folder-missing', ['proxy', 'US-A', '%HOME%/nowhere/proxy.json'], defs=[A])
node('proxy-bind-off', PROXY, defs=[A], cfg={'core_bind': False})
node('proxy-tun', PROXY, defs=[A, B, H], pre=lambda b: tun(b, dns=['192.0.2.53']))
node('proxy-tun-probe-public-server', ['proxy', 'PUB', '%HOME%/inst/proxy.json'], defs=[A, dict(HY, name='PUB', server='8.8.8.8')],
     pre=lambda b: net(b, routes={'8.8.8.8': 'utun4'}, netstat=['utun4', 'en0'], resolv='nameserver 192.0.2.53\n'))
node('proxy-readonly', PROXY, defs=[A], env={'CCA_RO': '1'})


@case('install-node/bad-config', ['install-node', 'choices'], env=ENV)
def _(b):
    home(b, defs=[A])
    b.write('.cca/config.json', '{"nodes": [')


@case('install-node/empty-home', ['install-node'], env=ENV)
def _(b):
    net(b)


# ---- dialer-proxy 连成很长的一串 ---------------------------------------------------------------------------------------------
# 顺着 dialer-proxy 找（core_pick）是一层层递归的：原来到一千层上下是没人接的 RecursionError。具体第几层随子命令（调用栈有多深）
# 和 Python 的版本差一两层（实测 994～996 层还行），Rust 这边固定成 996 层还行。这里只比离界线远的两种：够长但还行的、太长的
# （见 FINDINGS.md）

def chain(n):
    """n 个节点一个接一个经 dialer-proxy 连成一串（最后一个直接连）"""
    return [dict(B, name='n%d' % i, server='s%d.example.com' % i, **({'dialer-proxy': 'n%d' % (i + 1)} if i + 1 < n else {})) for i in range(n)]


# 切换以后 udp-env 多给的两行（TUN 的网卡名、给哪块网卡上的 VPN 让开了）原来的 Python 没有：去掉了再比（规则在 cargo test、tests/local_udp.py、回归 11b4u 里测）
UDP_ENV_MASK = [(r'UDP_TUN=[^\n]*\nUDP_OTHER=[^\n]*\n', '')]

for cmd, argv, kw in [('core-missing', [], {}), ('local-env', [], {'local': BUILTIN}), ('core-config', ['/tmp/core.sock'], {'local': BUILTIN}),
                      ('core-summary', [], {}), ('udp-env', [], {'local': dict(BUILTIN, udp='n0')}), ('install-node', ['proxy', 'n0', '%HOME%/proxy.json'], {})]:
    for name, n in [('dialer-chain-long', 980), ('dialer-chain-too-long', 1010)]:
        @case('%s/%s' % (cmd, name), [cmd] + argv, env=ENV, mask={'install-node': PORT, 'udp-env': UDP_ENV_MASK}.get(cmd))
        def _(b, n=n, kw=kw):
            home(b, defs=chain(n), cfg={'nodes': ['n0']}, table={}, **kw)

# 从路由器拷来的时候也一样：太长的存不下来（什么都不写）
for name, n in [('dialer-chain-long', 980), ('dialer-chain-too-long', 1010)]:
    @case('core-nodes-save/' + name, ['core-nodes-save'], stdin=defs_in(chain(n)), env=ENV, mask=SAVED)
    def _(b):
        home(b, cfg={'nodes': ['n0']}, table={})


# ---- 本机 UDP 走节点：udp-env / udp-node / udp-build / udp-probe（tests/local_udp.py 的检查都在这里）-----------------------------

# tests/local_udp.py 的四个节点：写了 udp: true 的 vless、没写的 trojan、本来就转发 UDP 的 hysteria2、经 HK-A 连的 ss
U_VLESS = dict(A, udp=True)
U_NOUDP = {'name': 'US-B', 'type': 'trojan', 'server': '192.0.2.11', 'port': 443, 'password': 'test-password'}
U_HY2 = {'name': 'HK-A', 'type': 'hysteria2', 'server': '192.0.2.12', 'port': 8443, 'password': 'test-password'}
U_CHAIN = {'name': 'US-C', 'type': 'ss', 'server': '192.0.2.13', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'test-password', 'udp': True, 'dialer-proxy': 'HK-A'}
U_DEFS = [U_VLESS, U_NOUDP, U_HY2, U_CHAIN]
U_NODES = ['US-A', 'US-B', 'HK-A', 'US-C']


def udp_home(b, udp=ABSENT, local=ABSENT, defs=U_DEFS, **kw):
    """用着内置代理的机器（udp：设的节点）"""
    if local is ABSENT:
        local = dict(BUILTIN) if udp is ABSENT else dict(BUILTIN, udp=udp)
    cfg = dict({'nodes': U_NODES}, **kw.pop('cfg', {}))
    home(b, local=local, defs=defs, cfg=cfg, **dict({'table': {}}, **kw))


def install_udp(b, node, version='1.19.33', env=None):
    """root 那边装好了：拿原版的 udp-build 生成 info.json，放进沙箱里的 root 目录（和 bash 装的时候一样，只是不拷内核和配置）"""
    import shutil
    b.write('build/cca-udp', '#!/bin/sh\nexec true\n', 0o755)
    b.write('core-bin', 'fake core\n', 0o755)
    oracle(b, 'udp-build', node, b.path('build'), b.path('core-bin'), version, env=env)
    root = (env or {}).get('CCA_UDP_ROOT') or b.path('udp-root/cca-udp')
    os.makedirs(root, exist_ok=True)
    os.rename(b.path('build/info.json'), os.path.join(root, 'info.json'))
    shutil.rmtree(b.path('build'))


def udp_env(name, argv=(), env=None, tool='claude', installed=None, info=None, **kw):
    @case('udp-env/' + name, ['udp-env'] + list(argv), env=dict(ENV, **(env or {})), tool=tool, mask=UDP_ENV_MASK)
    def _(b):
        udp_home(b, **kw)
        if installed:
            install_udp(b, installed)
        if isinstance(info, str):
            udp_root(b, raw=info)
        elif info is not None:
            udp_root(b, info)


udp_env('off')
udp_env('not-configured', local={})
udp_env('not-builtin', local={'client': '', 'udp': 'US-A'}, installed='US-A')
udp_env('missing', udp='US-A')
udp_env('ok', udp='US-A', installed='US-A')
udp_env('ok-chain', udp='US-C', installed='US-C')
udp_env('stale-other-node-installed', udp='US-A', installed='HK-A')
udp_env('stale-helper-version', udp='US-A', info=info_for('US-A', helper=1))
udp_env('undefined-no-udp', udp='US-B', installed='US-A')
udp_env('undefined-gone', udp='ZZ')
udp_env('undefined-bad-name', udp='a,b', defs=[dict(U_VLESS, name='a,b')], cfg={'nodes': ['a,b']})
udp_env('off-but-installed', installed='US-A')
udp_env('installed-node-not-text', info=info_for(5))
udp_env('installed-wrong-version', udp='US-A', info=info_for('US-A', v='1'))
udp_env('installed-version-true', udp='US-A', info=info_for('US-A', v=True))
udp_env('installed-not-json', udp='US-A', info='{oops')
udp_env('installed-is-a-list', udp='US-A', info='[]')
udp_env('udp-not-text', local=dict(BUILTIN, udp=5))
udp_env('local-is-text', local='builtin')
udp_env('codex', udp='US-A', installed='US-A', tool='codex')
udp_env('args', argv=['x'])
udp_env('paths-from-env', udp='US-A', env={'CCA_UDP_ROOT': '%HOME%/elsewhere/root', 'CCA_UDP_PLIST': '%HOME%/elsewhere/x.plist', 'CCA_UDP_LOG': '%HOME%/elsewhere/x.log'})
udp_env('paths-from-env-empty', udp='US-A', env={'CCA_UDP_ROOT': '', 'CCA_UDP_PLIST': '', 'CCA_UDP_LOG': ''}, installed='US-A')


@case('udp-env/paths-from-env-installed-there', ['udp-env'], env=dict(ENV, CCA_UDP_ROOT='%HOME%/elsewhere/root'), mask=UDP_ENV_MASK)
def _(b):
    udp_home(b, udp='US-A')
    install_udp(b, 'US-A', env={'CCA_UDP_ROOT': b.path('elsewhere/root')})


@case('udp-env/stale-node-changed', ['udp-env'], env=ENV, mask=UDP_ENV_MASK)
def _(b):
    # 装好以后路由器上改了这个节点：装着的配置旧了
    udp_home(b, udp='US-A')
    install_udp(b, 'US-A')
    cache(b, [dict(U_VLESS, server='192.0.2.99')] + U_DEFS[1:], nodes=U_NODES)


@case('udp-env/real-home', ['udp-env'], env=dict(ENV, HOME=os.path.realpath(os.path.expanduser('~')), CCA_HOME='%HOME%/.cca'), mask=UDP_ENV_MASK)
def _(b):
    # HOME 是这个用户真正的主目录时用的是 /Library 下的位置（只读：看一眼那边装没装）；config.json 这些照 CCA_HOME 在沙箱里
    udp_home(b)


@case('udp-env/bad-config', ['udp-env'], env=ENV, mask=UDP_ENV_MASK)
def _(b):
    udp_home(b)
    b.write('.cca/config.json', '{"nodes": [')


@case('udp-env/empty-home', ['udp-env'], env=ENV, mask=UDP_ENV_MASK)
def _(b):
    net(b)


for name, argv, kw in [
    ('picks', ['US-A'], {}), ('picks-hysteria2', ['HK-A'], {}), ('picks-chain', ['US-C'], {}), ('not-forwarding-udp', ['US-B'], {}), ('unknown', ['NOPE'], {}), ('no-arg', [], {}),
    ('empty-arg', [''], {}), ('two-args', ['US-A', 'x'], {}), ('none-can', ['US-B'], {'defs': [U_NOUDP, dict(U_VLESS, udp=False), dict(U_HY2, type='http', udp=True)]}),
    ('none-defined', ['US-A'], {'defs': []}), ('bad-names-left-out', ['a,b'], {'defs': [dict(U_VLESS, name='a,b'), dict(U_HY2, name='c\td'), U_HY2], 'cfg': {'nodes': ['a,b', 'HK-A']}}),
    ('renamed-on-router', ['OLD'], {'cfg': {'nodes': U_NODES, 'node_renames': {'OLD': 'US-A'}}}), ('by-recorded-address', ['OLD'], {'cfg': {'nodes': U_NODES, 'node_ids': {'OLD': nfp(U_HY2)}}}),
    ('by-recorded-address-no-udp', ['OLD'], {'cfg': {'nodes': U_NODES, 'node_ids': {'OLD': nfp(U_NOUDP)}}}), ('first-definition-decides', ['US-A'], {'defs': [dict(U_VLESS, udp=False), U_VLESS, U_HY2]}),
    ('custom-and-fill', ['US-D'], {'defs': [U_NOUDP], 'mine': [dict(HY, name='JP-X')], 'fill': {'version': 1, 'group': 'OpenAi', 'proxies': [dict(FILL['proxies'][1], udp=True), FILL['proxies'][0]]}}),
    ('udp-truthy-is-not-true', [], {'defs': [dict(U_VLESS, udp=1), dict(U_NOUDP, udp='true'), dict(U_CHAIN, udp=True)]}),
    ('types', [], {'defs': [dict(U_NOUDP, name=t, type=t, udp=u) for t in ('hysteria', 'tuic', 'wireguard', 'ssh', 'http', 'socks5', 'vmess') for u in (True,)] + [dict(U_NOUDP, name='tuic-no-flag', type='tuic')]}),
    ('not-builtin-still-answers', ['US-A'], {'local': {}}),
]:
    @case('udp-node/' + name, ['udp-node'] + argv, env=ENV)
    def _(b, kw=kw):
        udp_home(b, **kw)


BUILD = [(r'"saved_at": \d+', '"saved_at": <现在>'), (r'B_INFO=[0-9a-f]{64}', 'B_INFO=<随时间变>')]


def build(name, node='US-A', version='1.19.33', argv=None, env=None, installed=None, pre=None, script=True, binary=b'fake core\n', **kw):
    args = [node, '%HOME%/build', '%HOME%/core-bin', version] if argv is None else argv
    @case('udp-build/' + name, ['udp-build'] + list(args), env=dict(ENV, **(env or {})), mask=BUILD)
    def _(b):
        udp_home(b, **kw)
        if installed:
            install_udp(b, *installed)
        os.makedirs(b.path('build'), 0o700, exist_ok=True)
        if script:
            b.write('build/cca-udp', '#!/bin/sh\nexec true\n', 0o755)
        if binary is not None:
            b.write('core-bin', binary, 0o755)
        if pre:
            pre(b)


build('new')
build('readonly', env={'CCA_RO': '1'})
build('hysteria2', node='HK-A')
build('chain-keeps-dialer', node='US-C')
build('same-as-installed', installed=('US-A',))
build('other-node-installed', installed=('HK-A',))
build('other-core-version', version='1.19.34', installed=('US-A',))
build('other-core-file', installed=('US-A',), binary=b'another core\n')
build('node-changed-since', installed=('US-A',), pre=lambda b: cache(b, [dict(U_VLESS, server='192.0.2.99')] + U_DEFS[1:], nodes=U_NODES))
build('not-forwarding-udp', node='US-B')
build('unknown-node', node='NOPE')
build('bad-name', node='a,b', defs=[dict(U_VLESS, name='a,b')])
build('unicode-node', node='美国 😀', defs=[dict(U_VLESS, name='美国 😀')])
build('big-core-file', binary=b'0123456789abcdef' * (160 * 1024))          # 2.5 MB：一块一块算摘要
build('empty-core-file', binary=b'')
build('no-core-file', binary=None)
build('core-file-is-a-directory', binary=None, pre=lambda b: os.makedirs(b.path('core-bin')))
build('no-run-script', script=False)
build('config-already-there', pre=lambda b: b.write('build/config.yaml', 'old\n'))
build('plist-already-there', pre=lambda b: b.write('build/plist', 'old\n', 0o644))
build('info-already-there', pre=lambda b: b.write('build/info.json', 'old\n', 0o644))
build('folder-missing', argv=['US-A', '%HOME%/nowhere', '%HOME%/core-bin', '1.19.33'])
build('empty-version', version='')
build('three-args', argv=['US-A', '%HOME%/build', '%HOME%/core-bin'])
build('five-args', argv=['US-A', '%HOME%/build', '%HOME%/core-bin', '1', '2'])
build('root-from-env', env={'CCA_UDP_ROOT': '%HOME%/else where/root'})
build('not-builtin-still-builds', local={})


def udp_probe(name, stun, arm=None, argv=(), **world):
    @case('udp-probe/' + name, ['udp-probe'] + list(argv), env=dict(ENV, CCA_STUN=stun), mask=LEAK_MASK)
    def _(b):
        leak_net(b, **world)
        net(b)
        STUN.arm(b, **(arm or {}))


SEEN, SEEN2, SEEN6 = '203.0.113.9', '198.51.100.77', '2001:db8:85a3::8a2e:370:7334'
udp_probe('no-answer', DEAD)
udp_probe('one-address', STUN_A, arm={'a': [('xor', SEEN)]}, geoip={SEEN: where('us')})
udp_probe('china', STUN_A, arm={'a': [('xor', SEEN)]}, geoip={SEEN: where('cn', 'Example CN')})
udp_probe('geo-cached', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo('jp')}})
udp_probe('geo-cache-expired', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo('jp', age=8 * 86400)}}, geoip={SEEN: where('de')})
udp_probe('geo-down', STUN_A, arm={'a': [('xor', SEEN)]})
udp_probe('geo-empty-answer', STUN_A, arm={'a': [('xor', SEEN)]}, geoip={SEEN: {}})
udp_probe('geo-only-isp', STUN_A, arm={'a': [('xor', SEEN)]}, geoip={SEEN: {'isp': 'Example ISP'}})
udp_probe('geo-cached-miss', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo('', miss=True, country='', isp='')}})
udp_probe('two-servers', STUN_A + ',' + STUN_B, arm={'a': [('xor', SEEN)], 'b': [('mapped', SEEN2)]}, geoip={SEEN: where('us'), SEEN2: where('ca', 'Example CA')})
udp_probe('two-servers-same-address', STUN_A + ',' + STUN_B, arm={'a': [('xor', SEEN)], 'b': [('old', SEEN)]}, geoip={SEEN: where('us')})
udp_probe('one-dead-one-alive', DEAD + ',' + STUN_B, arm={'b': [('xor', SEEN)]}, geoip={SEEN: where('us')})
udp_probe('ipv6-address', STUN_A, arm={'a': [('xor', SEEN6)]}, geoip={SEEN6: where('us')})
udp_probe('cache-geo-is-a-list', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': ['x']})
udp_probe('cache-geo-time-is-text', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo('us', t='yesterday')}})
udp_probe('cache-geo-odd-fields', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo(5, country=None, isp='')}})
udp_probe('cache-geo-country-not-text', STUN_A, arm={'a': [('xor', SEEN)]}, cache={'geo': {net24(SEEN): geo('us', country=5)}})
udp_probe('cache-not-json', STUN_A, arm={'a': [('xor', SEEN)]}, cache='{"geo": ', geoip={SEEN: where('us')})
udp_probe('no-servers-in-env', 'not a server')
udp_probe('args', DEAD, argv=['x'])


# ---- core-keep：守着内核的常驻进程 -----------------------------------------------------------------------------------------

KERNEL = r'''#!/bin/sh
# 假内核（cca _core run）：每次启动记一行——参数、环境变量、开着的文件描述符、输入有几个字节、是不是自己一组；
# 再给自己发一个 SIGHUP（守着它的进程不理 SIGHUP，这一条是传给子进程的：活着才记得下第二行），然后照用例定的做
n=$(($(cat "$HOME/runs" 2>/dev/null || echo 0) + 1)); echo "$n" > "$HOME/runs"
{
	printf 'run %s: args=[%s] EVENTS=[%s] GRANT=[%s] RO=[%s] PROMPT=[%s] TTY=[%s] YES=[%s] TOOL=[%s] KEEP=[%s] SELF=[%s]' \
		"$n" "$*" "$CCA_EVENTS" "$CCA_GRANT" "$CCA_RO" "$CCA_PROMPT" "$CCA_TTY" "$CCA_YES" "$CCA_TOOL" "$KEEP_ME" "${CCA_SELF#"$HOME"}"
	printf ' fds=[%s] stdin=[%s]' "$(ls /dev/fd | tr '\n' ' ')" "$(wc -c | tr -d ' ')"
	if [ "$(ps -o pgid= -p $$ | tr -d ' ')" = "$$" ]; then printf ' own-group'; fi
	printf '\n'
} >> "$HOME/kernel.log"
kill -HUP $$
echo "run $n: still here after HUP" >> "$HOME/kernel.log"
echo "kernel $n says hello"; echo "kernel $n complains" >&2
'''
PIDS = '''echo $$ > "$HOME/kernel.pid.tmp"; mv "$HOME/kernel.pid.tmp" "$HOME/kernel.pid"
echo $PPID > "$HOME/keeper.pid.tmp"; mv "$HOME/keeper.pid.tmp" "$HOME/keeper.pid"
'''
SLEEP = 'sleep 8 &\nwait $!\n'
FAILS = 'exit 3\n'                                                                      # 起来就退出
TWICE = '[ "$n" -ge 2 ] && rm -f "$HOME/.cca/core/config.yaml"\nexit 0\n'                # 第二次起来时配置已经撤了：守着它的进程跟着退出
FOUR = '[ "$n" -ge 4 ] && rm -f "$HOME/.cca/core/config.yaml"\nexit 0\n'
WAITS = 'trap \'echo "run $n: got TERM" >> "$HOME/kernel.log"; exit 0\' TERM\n' + PIDS + SLEEP      # 一直跑着，收到 SIGTERM 记一笔再退出
STUBBORN = 'trap "" TERM\n' + PIDS + SLEEP                                                       # 不理 SIGTERM：3 秒以后被 SIGKILL
# 收到 SIGTERM 记一笔、但不退出（再活 6 秒）：停它的那 3 秒里看得到它记的那一笔
STAYS = ('trap \'echo "run $n: got TERM" >> "$HOME/kernel.log"\' TERM\n(trap "" TERM; exec sleep 6) &\n' + PIDS + 'while kill -0 $! 2>/dev/null; do wait $!; done\n')
KEEP_ENV = dict(ENV, CCA_EVENTS='/dev/null', CCA_GRANT='granted', CCA_RO='1', CCA_PROMPT='p', CCA_TTY='%HOME%/tty', CCA_YES='1', KEEP_ME='yes')
HELD = []   # 对拍这个进程一直拿着的锁（扮“已经有一个在托管”）


def keeper(b, tail=FAILS, cfg=None, yaml='{}\n', kernel='.cca/bin/cca', mode=0o755):
    """core-keep 的现场：本机客户端是内置代理、core/config.yaml 在、CCA_SELF 指着假内核"""
    b.cfg(dict({'version': 2, 'local': dict(BUILTIN)}, **(cfg or {})))
    net(b)
    os.makedirs(b.path('.cca/core'), 0o700, exist_ok=True)
    if yaml is not None:
        b.write('.cca/core/config.yaml', yaml)
    if kernel:
        b.write(kernel, KERNEL + tail, mode)


def take(box, rel, timeout=10.0):
    """等假内核把进程号写进沙箱，读出来、删掉（两边不一样，不留给比对）"""
    end = time.time() + timeout
    while time.time() < end:
        try:
            with open(box.path(rel)) as f:
                pid = int(f.read())
            os.remove(box.path(rel))
            return pid
        except (OSError, ValueError):
            time.sleep(0.02)
    return None


def gone(pid, timeout=8.0):
    """这个进程没了吗（等它被收走）"""
    end = time.time() + timeout
    while time.time() < end:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        time.sleep(0.05)
    return False


def logged(box, text, count=1, timeout=15.0):
    """等 kernel.log 里出现这段话（至少 count 次）"""
    end = time.time() + timeout
    while time.time() < end:
        try:
            with open(box.path('kernel.log')) as f:
                if f.read().count(text) >= count:
                    return True
        except OSError:
            pass
        time.sleep(0.02)
    return False


def kill(pid, sig):
    try:
        os.kill(pid, sig)
    except ProcessLookupError:
        pass


def while_running(act, patience=8.0):
    """假内核跑起来以后做一件事（act(box, 守着它的进程号)），再看内核有没有被停掉（最多等 patience 秒），记进沙箱的 after.txt"""
    def during(box):
        kernel, parent = take(box, 'kernel.pid'), take(box, 'keeper.pid')
        if kernel and parent:
            act(box, parent)
            with open(box.path('after.txt'), 'w') as f:
                f.write('kernel gone: %s\n' % gone(kernel, patience))
    return during


def send(*signals):
    def act(box, parent):
        for n, sig in enumerate(signals):
            if n:
                time.sleep(0.4)
            kill(parent, sig)
    return while_running(act)


def after_runs(n, act):
    """假内核第 n 次起来以后做一件事"""
    def during(box):
        if logged(box, ': args=', n):
            act(box)
    return during


def touch_while_backing_off(box):
    """第三次退出以后要等 4 秒才重启：这时碰一下配置文件，应该半秒里就重启。两秒还没重启的记下来"""
    def runs():
        try:
            with open(box.path('kernel.log')) as f:
                return sum(1 for line in f if ': args=' in line)
        except OSError:
            return 0

    def wait_for(n, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if runs() >= n:
                return True
            time.sleep(0.02)
        return False
    if wait_for(3, 15):
        time.sleep(0.4)
        os.utime(box.path('.cca/core/config.yaml'))
        with open(box.path('after.txt'), 'w') as f:
            f.write('restarted right after the touch: %s\n' % wait_for(4, 2.0))


for name, kw in [
    ('not-wanted-no-config', {'cfg': None, 'yaml': '{}\n'}), ('not-wanted-old-client', {'cfg': {'local': {'client': 'clash-verge'}}}), ('not-wanted-no-local', {'cfg': {'local': {}}}),
    ('not-wanted-local-null', {'cfg': {'local': None}}), ('not-wanted-no-config-yaml', {'yaml': None}), ('not-wanted-local-is-text', {'cfg': {'local': 'builtin'}}),
]:
    @case('core-keep/' + name, ['core-keep'], env=KEEP_ENV)
    def _(b, kw=kw, name=name):
        keeper(b, **kw)
        if name == 'not-wanted-no-config':
            os.remove(b.path('.cca/config.json'))


@case('core-keep/not-wanted-config-yaml-is-a-directory', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, yaml=None)
    os.makedirs(b.path('.cca/core/config.yaml'))


@case('core-keep/not-wanted-bad-config', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b)
    b.write('.cca/config.json', '{"local": ')


@case('core-keep/not-wanted-config-is-a-list', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b)
    b.write('.cca/config.json', '[{"local": {"client": "builtin"}}]')


@case('core-keep/args', ['core-keep', 'x'], env=KEEP_ENV)
def _(b):
    keeper(b)


@case('core-keep/restarts-until-not-wanted', ['core-keep'], env=KEEP_ENV)
def _(b):
    # 内核一起来就退出：隔 1 秒再起；第二次起来时配置撤了，守着它的进程也退出。kernel.log 里是它带给内核的环境和文件描述符
    keeper(b, TWICE)
    os.chmod(b.path('.cca/core'), 0o755)


@case('core-keep/kernel-keeps-failing', ['core-keep'], env=KEEP_ENV, timeout=30,
      during=after_runs(2, lambda box: os.remove(box.path('.cca/core/config.yaml'))))
def _(b):
    # 内核起不来（一起来就退出码 3）：隔 1 秒再起；第二次也退出以后本来要等 2 秒，这时配置撤了，等的时候就看得到，不再起
    keeper(b, FAILS)


@case('core-keep/kernel-missing', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, kernel=None)


@case('core-keep/kernel-not-executable', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, mode=0o644)


@case('core-keep/kernel-is-a-directory', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, kernel=None)
    os.makedirs(b.path('.cca/bin/cca'))


@case('core-keep/kernel-by-name-on-path', ['core-keep'], env=dict(KEEP_ENV, CCA_SELF='fake-kernel'))
def _(b):
    keeper(b, TWICE, kernel='bin/fake-kernel')


@case('core-keep/kernel-name-not-on-path', ['core-keep'], env=dict(KEEP_ENV, CCA_SELF='no-such-kernel'))
def _(b):
    keeper(b, kernel=None)


@case('core-keep/already-running', ['core-keep'], env=KEEP_ENV)
def _(b):
    # 另一个进程拿着 core/keep.lock：这里是对拍这个进程自己拿着（锁文件放在沙箱外面、经链接指过去，两边跑的是同一把锁）
    import atexit
    keeper(b, TWICE)
    fd, outside = tempfile.mkstemp(prefix='cca-pydiff-keep.', suffix='.lock')
    fcntl.flock(fd, fcntl.LOCK_EX)
    HELD.append(fd)
    atexit.register(os.remove, outside)
    os.symlink(outside, b.path('.cca/core/keep.lock'))


@case('core-keep/lock-left-behind-is-free', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, TWICE)
    b.write('.cca/core/keep.lock', 'stale\n', 0o600)


@case('core-keep/lock-is-a-directory', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, TWICE)
    os.makedirs(b.path('.cca/core/keep.lock'))


@case('core-keep/log-is-a-directory', ['core-keep'], env=KEEP_ENV)
def _(b):
    keeper(b, TWICE)
    os.makedirs(b.path('.cca/core/core.log'))


for name, size in [('log-over-1mb-is-cleared', 1048577), ('log-at-1mb-is-kept', 1048576)]:
    @case('core-keep/' + name, ['core-keep'], env=KEEP_ENV)
    def _(b, size=size):
        keeper(b, TWICE)
        b.write('.cca/core/core.log', 'old line\n' * (size // 9) + 'x' * (size % 9), 0o644)


for name, signals in [('sigterm', [signal.SIGTERM]), ('sigint', [signal.SIGINT]), ('sighup-is-ignored', [signal.SIGHUP, signal.SIGTERM])]:
    @case('core-keep/' + name, ['core-keep'], env=KEEP_ENV, during=send(*signals), timeout=30)
    def _(b):
        keeper(b, WAITS)


@case('core-keep/sigterm-kernel-ignores-it', ['core-keep'], env=KEEP_ENV, during=send(signal.SIGTERM), timeout=30)
def _(b):
    keeper(b, STUBBORN)


def term_again(box, parent):
    kill(parent, signal.SIGTERM)
    if logged(box, 'got TERM'):      # 守着它的进程已经在停内核了（等它退出的那 3 秒）：这时再来一个
        kill(parent, signal.SIGTERM)


@case('core-keep/second-sigterm-while-stopping-the-kernel', ['core-keep'], env=KEEP_ENV, during=while_running(term_again, patience=1.5), timeout=30)
def _(b):
    # 原来这时是没人接的异常：不等内核停下就退出（退出码 1），内核还活着（这个假内核 6 秒后自己退出）
    keeper(b, STAYS)


def unwanted_then_term(box, parent):
    os.remove(box.path('.cca/core/config.yaml'))
    if logged(box, 'got TERM'):      # 配置撤了，它在停内核：这时来的信号接得住，从头再停一遍（再发一次 SIGTERM，3 秒后 SIGKILL）
        kill(parent, signal.SIGTERM)


@case('core-keep/sigterm-while-stopping-an-unwanted-kernel', ['core-keep'], env=KEEP_ENV, during=while_running(unwanted_then_term), timeout=40)
def _(b):
    keeper(b, STAYS)


@case('core-keep/config-removed-while-running', ['core-keep'], env=KEEP_ENV, timeout=30,
      during=while_running(lambda box, parent: os.remove(box.path('.cca/core/config.yaml'))))
def _(b):
    keeper(b, WAITS)


@case('core-keep/client-changed-while-running', ['core-keep'], env=KEEP_ENV, timeout=30,
      during=while_running(lambda box, parent: Box_cfg(box, {'version': 2, 'local': {'core_off': '1'}})))
def _(b):
    keeper(b, WAITS)


@case('core-keep/config-broken-while-running', ['core-keep'], env=KEEP_ENV, timeout=30,
      during=while_running(lambda box, parent: Box_cfg(box, {'version': 2, 'local': 'builtin'})))
def _(b):
    # config.json 的 local 被改成了文字：原来是没人接的 AttributeError，但先把内核停了才退出
    keeper(b, WAITS)


@case('core-keep/restarts-at-once-when-config-touched', ['core-keep'], env=KEEP_ENV, during=touch_while_backing_off, timeout=40)
def _(b):
    keeper(b, FOUR)


def Box_cfg(box, data):
    """运行期间换掉 config.json（先写旁边再改名：守着内核的进程不会读到半份）"""
    tmp = box.path('.cca/config.json.new')
    with open(tmp, 'w') as f:
        json.dump(data, f)
    os.replace(tmp, box.path('.cca/config.json'))


# ---- udp_leak_hint：WebRTC 直连风险那一句后面的补充（用着内置代理时）---------------------------------------------------------
# 它是经 leak-verdict 说出来的（cli/src/cmd/leak.rs 调 cmd::corenet::udp_leak_hint）
from cases.leak import EXIT_IP, KEY, probe_file

def hint(name, local, tool='claude', installed=None, defs=U_DEFS):
    @case('leak-verdict/udp-hint-' + name, ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US', 'detail'], env=ENV, mask=LEAK_MASK, tool=tool)
    def _(b):
        leak_net(b, geoip={SEEN2: where('cn', 'Example CN')}, cfg={'version': 2, 'nodes': U_NODES, 'local': local})
        net(b)
        if defs is not None:
            cache(b, defs, nodes=U_NODES)
        if installed:
            install_udp(b, installed)
        probe_file(b, {'stun': {'ips': [SEEN2]}})

hint('off', dict(BUILTIN))
hint('off-codex', dict(BUILTIN), tool='codex')
hint('ok', dict(BUILTIN, udp='US-A'), installed='US-A')
hint('missing', dict(BUILTIN, udp='US-A'))
hint('stale', dict(BUILTIN, udp='US-A'), installed='HK-A')
hint('undefined', dict(BUILTIN, udp='US-B'))
hint('undefined-no-defs', dict(BUILTIN, udp='US-A'), defs=None)
hint('udp-not-text', dict(BUILTIN, udp=5))
hint('udp-empty', dict(BUILTIN, udp=''))
hint('odd-name', dict(BUILTIN, udp='节点 "A"'), defs=[dict(U_VLESS, name='节点 "A"')])


# ---- file-replace：把一个文件的内容原子地换进另一个（保留权限），再删掉来源。原来的 CMDS 里有、现在没人调 -----------------------
for name, src, dst, mode in [
    ('plain', '新的内容\n第二行\n', '旧的\n', 0o600), ('keeps-mode-644', 'new\n', 'old\n', 0o644), ('keeps-mode-755', '#!/bin/sh\necho hi\n', '#!/bin/sh\n', 0o755),
    ('dst-missing', 'new\n', None, None), ('crlf-kept', 'a\r\nb\rc\n', 'old', 0o600), ('empty', '', 'old', 0o640), ('no-final-newline', 'x', 'old\n', 0o600),
    ('not-utf8', b'caf\xe9\n', 'old\n', 0o600), ('bom', '﻿x\n', 'old\n', 0o600), ('src-missing', None, 'old\n', 0o600),
]:
    @case('file-replace/' + name, ['file-replace', '%HOME%/src.txt', '%HOME%/dst.txt'])
    def _(b, src=src, dst=dst, mode=mode):
        if src is not None:
            b.write('src.txt', src, 0o640)
        if dst is not None:
            b.write('dst.txt', dst, mode)


@case('file-replace/dst-is-a-link', ['file-replace', '%HOME%/src.txt', '%HOME%/dst.txt'])   # 链接本身被换成普通文件，指向的文件不动
def _(b):
    b.write('src.txt', 'new\n')
    b.write('real.txt', 'real\n', 0o640)
    os.symlink(b.path('real.txt'), b.path('dst.txt'))


@case('file-replace/src-is-a-directory', ['file-replace', '%HOME%/src', '%HOME%/dst.txt'])
def _(b):
    os.makedirs(b.path('src'))
    b.write('dst.txt', 'old\n')


for name, argv in [('no-args', ['file-replace']), ('one-arg', ['file-replace', 'a']), ('three-args', ['file-replace', 'a', 'b', 'c'])]:
    @case('file-replace/' + name, argv)
    def _(b):
        pass
