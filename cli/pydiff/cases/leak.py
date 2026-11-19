"""出口检测的解析和 DNS / WebRTC 泄露检测（cli/src/cmd/leak.rs）：coffee、leak-probe、leak-verdict。

不联网：
- curl 是沙箱里的假命令（下面的 CURL）：照网址执行 fx/ 下的夹具（一小段 sh：回答什么、等多久、退出码），没有夹具就是连不上；
  收到的参数记进 curl-log/（一类网址一份文件，同时发出的请求各记各的，先后才是确定的）。对拍连沙箱里的文件一起比，
  两边给 curl 的参数差一个字都看得出来。
- STUN 只发给本机回环：CCA_STUN 指向没人听的 127.0.0.1:9，或者这份文件里起的假 STUN 服务器（STUN，回答什么由用例定，
  收到的请求长什么样记进沙箱的 stun-a.log / stun-b.log）。leak-probe 的用例一律经 probe() 登记：它总带着 CCA_STUN——
  不带的话原来的代码会去问真的 STUN 服务器。

每次都不一样的只有两样，比之前换掉（MASK）：这次运行写下的时间戳（小数；夹具里的时间是整数，不会被换掉）、一次性的
token（uuid4 的前 20 个十六进制字符，照它的样子认：第 13 个是 4，第 17 个是 8 9 a b 之一）。问解析器问了好几轮的用例另外
把 curl 的 -m 换掉（SLOW：后几轮的时限照剩下的时间算）；别的用例里 -m 是定数，原样比。

夹具里的时间写成 Ago(多少秒之前)，夹具函数跑的时候才换成时间戳：缓存新不新（6 小时、10 分钟、1 小时、7 天）只看它，
不看整套对拍跑了多久。cca check 的开始时间（参数）用的是 2001 年、2100 年这样的定数。

同时发出的几个请求谁先回来，决定了结果里的先后（原来就这样）：这种用例让其中一个晚 0.6 秒回答，先后就是确定的。
有几个用例是在等时间的（没人回答的 STUN 等 1 秒、问满四轮 1.5 秒、curl 不回答到被杀 4 秒、到点不等了 6 秒），
这一份跑完约两分钟。

tests/isolation_leaks.py 的 13 个测试都在这里（名字里带 iso- 的）；只能调内部函数的那一半（stun_probe 连不上、超时）在
cli/src/cmd/leak/stun.rs 的单元测试里。"""
import json
import os
import select
import shlex
import socket
import struct
import threading
import time

from run import case

MASK = [(r'"t": 1\d{9}\.\d+', '"t": T'), (r'[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{3}', 'TOKEN')]   # 时间戳：十位整数的秒，带小数
SLOW = MASK + [(r'\[-m\]\[\d+\.\d\]', '[-m][N]')]   # 问了好几轮的：后几轮的时限是照剩下的时间算的，每次不一样
KEY = 'router|US-A'
PROXY = 'http://192.0.2.10:7901'
EXIT_IP = '203.0.113.10'
RESOLVER = '203.0.113.53'
COUNTRY = {'us': 'United States', 'cn': 'China', 'ca': 'Canada', 'hk': 'Hong Kong', 'mo': 'Macao', 'tw': 'Taiwan', 'jp': 'Japan', 'de': 'Germany'}

CURL = r'''# 假 curl：参数记进 curl-log/，照网址执行 fx/ 下的夹具
L="$HOME/curl-log"; mkdir -p "$L"
url=""; line=""
for a in "$@"; do
	case "$a" in http*) url="$a" ;; esac
	line="$line[$a]"
done
case "$url" in
	https://*.d.ip.net.coffee/pixel.gif)
		t="${url#https://}"; t="${t%%.d.ip.net.coffee/*}"
		printf '%s\n' "$line" >> "$L/pixel-${t##*-}"
		printf '%s\n' "${t%-*}" >> "$L/tokens"
		f="$HOME/fx/pixel" ;;
	https://ip.net.coffee/api/dns/result/*)
		# 问的 token 是不是刚才那两个子域名用的那个，也记下来
		if grep -qx "${url##*/}" "$L/tokens" 2>/dev/null; then same=same-token; else same=other-token; fi
		printf '%s %s\n' "$line" "$same" >> "$L/dns-result"
		n=$(wc -l < "$L/dns-result" | tr -d ' ')
		f="$HOME/fx/dns.$n"; [ -f "$f" ] || f="$HOME/fx/dns" ;;
	https://ip.net.coffee/api/geoip/*)
		ip="${url##*/}"
		printf '%s\n' "$line" >> "$L/geoip-$ip"
		f="$HOME/fx/geo/$ip" ;;
	*) printf '%s\n' "$line" >> "$L/unexpected"; exit 6 ;;
esac
[ -f "$f" ] || exit 7
. "$f"
'''


def reply(body='', rc=0, wait=0):
    """夹具：假 curl 遇到这个网址时执行的一小段 sh。body 是字典、列表时写成 JSON"""
    s = 'sleep %s\n' % wait if wait else ''
    if not isinstance(body, str):
        body = json.dumps(body)
    if body:
        s += 'printf %%s %s\n' % shlex.quote(body)
    return s + 'exit %d\n' % rc


def where(cc='us', isp='Example ISP', **kw):
    """ip.net.coffee 的 /api/geoip 回答"""
    return dict({'country': COUNTRY.get(cc, ''), 'region': '', 'city': '', 'isp': isp, 'country_code': cc}, **kw)


class Ago:
    """夹具里的时间：这个用例的夹具函数跑的那一刻往前这么多秒（负数是往后），到写文件时才换成时间戳（settle）。
    不在登记用例时就定下来：整套对拍跑多久、这一份排在第几个，都不该影响缓存算新的还是旧的。
    整数秒换成整数的时间戳：和被测的程序这次运行写下的（带小数，MASK 会换掉）分得开"""

    def __init__(self, seconds):
        self.seconds = seconds


CLOCK = {}   # 这个用例的「现在」：夹具函数开头定一次（tick，net() 里就调了），这个用例的夹具都照它算


def tick():
    CLOCK['now'] = int(time.time())


def settle(x):
    """把夹具里的 Ago 换成时间戳"""
    if isinstance(x, Ago):
        return CLOCK['now'] - x.seconds
    if isinstance(x, dict):
        return {k: settle(v) for k, v in x.items()}
    if isinstance(x, list):
        return [settle(v) for v in x]
    return x


def geo(cc='us', age=60, **kw):
    """leak.json、探测结果里记的一条归属地，age 秒之前查到的"""
    return dict({'t': Ago(age), 'cc': cc, 'country': COUNTRY.get(cc, ''), 'isp': 'Example ISP'}, **kw)


def net24(ip):
    return ip.rsplit('.', 1)[0] + '.0/24'


def net(b, dns=None, geoip=None, pixel=None, cache=None, cfg=None):
    """沙箱里的假网络和缓存。dns：/api/dns/result 的回答，一个，或者一列（第几次问给第几个，问完了给最后一个）；
    geoip：{IP: 回答}，没写的 IP 连不上；pixel：两个一次性子域名的图片请求（默认马上成功）；cache：leak.json；cfg：config.json"""
    STUN.arm(None)   # 上一个用例定的回答不留到这一个
    tick()
    b.fake('curl', CURL)
    b.write('fx/pixel', reply() if pixel is None else pixel)
    answers = dns if isinstance(dns, list) else [dns]
    for n, a in enumerate(answers, 1):
        if a is not None:
            b.write('fx/dns.%d' % n, a if isinstance(a, str) else reply(a))
    if answers[-1] is not None:
        b.write('fx/dns', answers[-1] if isinstance(answers[-1], str) else reply(answers[-1]))
    for ip, a in (geoip or {}).items():
        b.write('fx/geo/' + ip, a if isinstance(a, str) else reply(a))
    if cache is not None:
        b.write('.cca/leak.json', cache) if isinstance(cache, (str, bytes)) else b.json('.cca/leak.json', settle(cache))
    if cfg is not None:
        b.write('.cca/config.json', cfg) if isinstance(cfg, (str, bytes)) else b.cfg(cfg)


def probe_file(b, found):
    """leak-probe 打印的那个结果，写进沙箱的 probe.json（leak-verdict 的第一个参数）"""
    b.write('probe.json', json.dumps(settle(found)) + '\n')


class FakeStun:
    """只在 127.0.0.1 上的假 STUN 服务器，跑在对拍这个进程的一个后台线程里（对拍结束就没了）。两个端口（a、b）：
    CCA_STUN 里写两个服务器时各问各的。回答什么由用例的夹具函数用 arm() 定；每收到一个请求，把它的样子记进沙箱的
    stun-a.log / stun-b.log（先记再回答：被测的进程拿到回答时日志已经写好了）。

    地址都写数字的 127.0.0.1，不写 localhost：macOS 上新进程里两个线程同时解析主机名，约千分之一的概率有一个卡住一秒以上
    （原来的代码、cca-core 都会遇到，stun_ips 到时间不等它），用例就时过时不过。所以经 IPv6 发请求的那条路不在这里，
    在 cli/src/cmd/leak/stun.rs 的单元测试里（直接给 ::1，不用解析）；回答里的 IPv6 地址这里照样测"""

    def __init__(self):
        self.home, self.plan = None, {}
        self.socks = {name: socket.socket(socket.AF_INET, socket.SOCK_DGRAM) for name in 'ab'}
        for s in self.socks.values():
            s.bind(('127.0.0.1', 0))
        self.port = {name: s.getsockname()[1] for name, s in self.socks.items()}
        threading.Thread(target=self.serve, daemon=True).start()

    def arm(self, box, **plan):
        """这个用例里端口 a / b 怎么回答：几个回答依次发回去，每个是 (哪种, IP)。哪种：xor（XOR-MAPPED-ADDRESS）、old（0x8020）、
        mapped（MAPPED-ADDRESS）、other-tid（事务号对不上的）、error（出错的响应）；或者 'junk'（不是 STUN 的几个字节）。
        没写的端口收到请求不回答"""
        self.home, self.plan = box and box.home, plan

    def serve(self):
        names = {s: name for name, s in self.socks.items()}
        while True:
            for s in select.select(list(names), [], [])[0]:
                try:
                    self.answer(s, names[s])
                except Exception:   # 用例之间沙箱不在、对方已经走了：不管
                    pass

    def answer(self, s, name):
        d, peer = s.recvfrom(2048)
        if not self.home or not os.path.isdir(self.home):
            return
        with open(os.path.join(self.home, 'stun-%s.log' % name), 'a') as f:
            f.write('len=%d type=%s length=%s magic=%s\n' % (len(d), d[:2].hex(), d[2:4].hex(), d[4:8].hex()))
        if len(d) < 20:
            return
        for r in self.plan.get(name, []):
            if r == 'junk':
                s.sendto(b'junk', peer)
                continue
            kind, ip = r
            tid = bytes(12) if kind == 'other-tid' else d[8:20]
            a = socket.inet_pton(socket.AF_INET6 if ':' in ip else socket.AF_INET, ip)
            if kind != 'mapped':
                a = bytes(x ^ y for x, y in zip(a, d[4:8] + tid))
            code = {'old': 0x8020, 'mapped': 0x0001}.get(kind, 0x0020)
            attr = struct.pack('!HHBBH', code, 4 + len(a), 0, 2 if ':' in ip else 1, peer[1] ^ 0x2112) + a
            s.sendto(struct.pack('!HHI', 0x0111 if kind == 'error' else 0x0101, len(attr), 0x2112A442) + tid + attr, peer)


STUN = FakeStun()
STUN_A = '127.0.0.1:%d' % STUN.port['a']   # 按 IPv6 解析 127.0.0.1：macOS 给的是 ::ffff: 映射的地址（不算），别的系统解析不了
STUN_B = '127.0.0.1:%d' % STUN.port['b']
DEAD = '127.0.0.1:9'                   # 没人听：connect 过的 UDP 套接字马上知道，不用等


# ---- coffee：只看参数（最低纯净度、trace、iprisk 的 JSON），第一行结论、有补充说明再一行、退出码 0–4 --------------------------

TRACE = ('fl=1\nh=claude.ai\nip=203.0.113.1\nts=1795058400.123\nvisit_scheme=https\nuag=curl/8.7.1\ncolo=LAX\nsliver=none\n'
         'http=http/2\nloc=US\ntls=TLSv1.3\nsni=plaintext\nwarp=off\ngateway=off\nrbi=off\nkex=X25519\n')
RISK = {'is_datacenter': False, 'isResidential': True, 'is_vpn': False, 'is_proxy': False, 'is_tor': False, 'is_abuser': False,
        'asOrganization': 'Example Networks', 'countryCode': 'us', 'trust_score': 97}


def coffee(name, risk=RISK, trace=TRACE, min_trust='50', tool='claude'):
    @case('coffee/' + name, ['coffee', min_trust, trace, risk if isinstance(risk, str) else json.dumps(risk)], tool=tool)
    def _(b):
        pass


def risk(**kw):
    return dict(RISK, **kw)


coffee('clean')
coffee('clean-codex', tool='codex')
coffee('clean-agy', tool='agy')
coffee('ensure-ascii-risk', json.dumps(risk(asOrganization='例子网络 ☁ 😀'), ensure_ascii=True))
coffee('utf8-risk', json.dumps(risk(asOrganization='例子网络 ☁ 😀'), ensure_ascii=False))

# 纯净度的档位：95、80、50、25；整数、小数、布尔值、放不进 64 位的整数、NaN、无穷大都是“数”
for s in ['100', '96', '95', '94', '80', '79', '50', '49', '25', '24', '1', '0', '-1', '95.0', '94.99', '79.5', '49.999999', '25.0',
          '24.999', '-0.0', '1e16', '1e400', '-1e400', 'true', 'false', '12345678901234567890', '-12345678901234567890', 'NaN',
          'Infinity', '-Infinity', '97.50', '5E1', '0.1']:
    coffee('score/%s' % s, json.dumps(risk()).replace('97', s))

# 最低纯净度（config.json 的 min_trust，bash 原样传来）：照 int() 认；评分低于它是退出码 1，说明里是原样的写法
for m in ['0', '59', '60', '61', '100', ' 61 ', '+61', '6_1', '６１', '-5', '061', '', 'abc', '60.5', 'None', 'True', '0x3d', '6__1', '_61', '61_',
          '99999999999999999999', '-99999999999999999999', '6 1', '６_１']:
    coffee('min/%s' % (m or 'empty'), risk(trust_score=60), min_trust=m)
for s, m in [('59.9', '60'), ('60.0', '60'), ('60.5', '61'), ('true', '1'), ('true', '2'), ('false', '0'), ('false', '1'), ('NaN', '50'),
             ('Infinity', '50'), ('-Infinity', '-99999999999999999999'), ('-3.5', '-4'), ('-4.5', '-4'), ('9223372036854775807', '9223372036854775808'),
             ('9223372036854775808', '9223372036854775808'), ('9223372036854775809', '9223372036854775808'), ('1e30', '1000000000000000019884624838657'),
             ('1e30', '1000000000000000019884624838656'), ('-0.0', '0'), ('-0.1', '0')]:
    coffee('min-vs/%s-%s' % (s, m), json.dumps(risk()).replace('97', s), min_trust=m)

# 受限地区：评分直接算 0，退出码 2；地区先看 iprisk 的 countryCode，没有才看 trace 的 loc
for cc in ['cn', 'CN', 'Hk', 'mo', 'ru', 'kp', 'ir', 'sy', 'cu', 'by', 've']:
    coffee('restricted/%s' % cc, risk(countryCode=cc))
coffee('restricted/codex', risk(countryCode='hk'), tool='codex')
coffee('restricted/agy', risk(countryCode='ru'), tool='agy')
coffee('restricted/by-loc-no-risk', '', trace='ip=203.0.113.1\nloc=hk\n')
coffee('restricted/by-loc-risk-without-cc', {'trust_score': 99, 'isResidential': True}, trace='ip=203.0.113.1\nloc=CN\n')
coffee('restricted/risk-cc-wins-over-loc', risk(countryCode='hk'))
coffee('restricted/loc-ignored-when-risk-has-cc', risk(countryCode='us'), trace='ip=203.0.113.1\nloc=HK\n')
coffee('restricted/bad-min-trust-not-looked-at', risk(countryCode='cn'), min_trust='abc')
coffee('restricted/with-tags', risk(countryCode='ir', is_vpn=True, is_abuser=True, isResidential=False, is_datacenter=True))

# 没有评分：退出码 4
for name, r in [('empty', ''), ('not-json', 'upstream connect error'), ('html', '<html>502</html>'), ('list', '[1, 2]'), ('number', '5'),
                ('null', 'null'), ('string', '"ok"'), ('empty-object', '{}'), ('score-string', risk(trust_score='97')),
                ('score-null', risk(trust_score=None)), ('score-list', risk(trust_score=[97])), ('score-object', risk(trust_score={'v': 97})),
                ('truncated', '{"trust_score": 97'), ('trailing', '{"trust_score": 97} x'), ('bom', '﻿{"trust_score": 97}'),
                ('only-org', {'asOrganization': 'Example Networks'}), ('only-cc', {'countryCode': 'de'})]:
    coffee('no-score/%s' % name, r)
coffee('no-score/bad-min-trust-not-looked-at', '', min_trust='abc')
coffee('no-score/no-loc-either', '', trace='ip=203.0.113.1\n')

# 标签：住宅 / 机房二选一（住宅优先），其余四个各自独立；只认 JSON 的 true
coffee('tags/datacenter', risk(isResidential=False, is_datacenter=True))
coffee('tags/residential-wins', risk(isResidential=True, is_datacenter=True))
coffee('tags/neither', risk(isResidential=False))
coffee('tags/all', risk(is_datacenter=True, is_vpn=True, is_proxy=True, is_tor=True, is_abuser=True))
for f in ['is_vpn', 'is_proxy', 'is_tor', 'is_abuser']:
    coffee('tags/%s' % f, risk(**{f: True}))
coffee('tags/truthy-but-not-true', risk(isResidential=1, is_datacenter='true', is_vpn=1, is_proxy='yes', is_tor=[1], is_abuser=1.0))
coffee('tags/low-score', risk(trust_score=10, is_vpn=True, is_abuser=True))

# 归属的机构：asOrganization，没有（或者算假）就用 company_name；不是文字的在 Python 里是没人接的异常
coffee('org/company-name', {'company_name': 'Example Co', 'trust_score': 90, 'countryCode': 'us'})
coffee('org/as-org-wins', risk(company_name='Example Co'))
coffee('org/empty-as-org', risk(asOrganization='', company_name='Example Co'))
coffee('org/zero-as-org', risk(asOrganization=0, company_name='Example Co'))
coffee('org/none', risk(asOrganization=None))
coffee('org/both-empty', risk(asOrganization='', company_name=''))
coffee('org/falsy-not-text', risk(asOrganization=[], company_name=False))
for name, v in [('number', 5), ('list', ['Example']), ('object', {'name': 'Example'}), ('true', True), ('float', 1.5)]:
    coffee('org/bad-%s' % name, risk(asOrganization=v))
coffee('org/bad-company-name', risk(asOrganization='', company_name=7))
coffee('org/spaces-and-dots', risk(asOrganization='  A · B  '))

# 地区代码：转成大写；不是文字的（算真的）在 Python 里是没人接的异常
coffee('cc/upper', risk(countryCode='De'))
coffee('cc/sharp-s', risk(countryCode='ß'))
coffee('cc/empty-falls-back-to-loc', risk(countryCode=''), trace='ip=203.0.113.1\nloc=jp\n')
coffee('cc/null-falls-back-to-loc', risk(countryCode=None))
coffee('cc/zero-falls-back-to-loc', risk(countryCode=0))
coffee('cc/none-anywhere', risk(countryCode=''), trace='ip=203.0.113.1\n')
coffee('cc/none-anywhere-no-risk', '', trace='ip=203.0.113.1\nloc=\n')
for name, v in [('number', 5), ('list', ['us']), ('true', True), ('object', {'cc': 'us'})]:
    coffee('cc/bad-%s' % name, risk(countryCode=v))

# trace：一行一个 键=值，键原样比；拿不到 ip 是退出码 3（不看别的）
coffee('trace/empty', trace='')
coffee('trace/empty-codex', trace='', tool='codex')
coffee('trace/empty-agy', trace='', tool='agy')
coffee('trace/garbage', trace='<html>captive portal</html>')
coffee('trace/no-ip-line', trace='fl=1\nloc=US\n')
coffee('trace/empty-ip', trace='ip=\nloc=US\n')
coffee('trace/ip-last-wins', trace='ip=198.51.100.1\nloc=DE\nip=203.0.113.9\nloc=us\n')
coffee('trace/ip-emptied-later', trace='ip=198.51.100.1\nip=\n')
coffee('trace/value-with-equals', trace='ip=a=b=c\nloc=x=y\n', risk='')
coffee('trace/crlf', trace='fl=1\r\nip=203.0.113.1\r\nloc=US\r\n')
coffee('trace/cr-only', trace='ip=203.0.113.1\rloc=US')
coffee('trace/no-final-newline', trace='loc=us\nip=203.0.113.1')
coffee('trace/leading-space-key', trace=' ip=203.0.113.1\nloc=US\n')
coffee('trace/upper-key', trace='IP=203.0.113.1\nLOC=US\n')
coffee('trace/spaces-in-value', trace='ip= 203.0.113.1 \nloc= us \n', risk='')
coffee('trace/ipv6', trace='ip=2001:db8::1\nloc=DE\n', risk=risk(countryCode='de'))
for name, sep in [('vt', '\x0b'), ('ff', '\x0c'), ('fs', '\x1c'), ('gs', '\x1d'), ('rs', '\x1e'), ('nel', '\x85'), ('ls', ' '), ('ps', ' ')]:
    coffee('trace/sep-%s' % name, trace='fl=1%sip=203.0.113.1%sloc=US%s' % (sep, sep, sep))
coffee('trace/not-a-separator', trace='fl=1\x1fip=203.0.113.1\tloc=US\n')
coffee('trace/unicode-loc', trace='ip=203.0.113.1\nloc=ǆß\n', risk='')


@case('coffee/args-none', ['coffee'])
def _(b):
    pass


@case('coffee/args-two', ['coffee', '50', TRACE])
def _(b):
    pass


@case('coffee/args-four', ['coffee', '50', TRACE, json.dumps(RISK), 'extra'])
def _(b):
    pass


# ---- leak-probe：后台测一次，打印一行 JSON ---------------------------------------------------------------------------------

DNS_OK = {'token': 'x', 'dns_servers': [RESOLVER]}
FRESH = {'dns': {KEY: {'t': Ago(60), 'ok': True, 'servers': [RESOLVER]}}, 'geo': {net24(RESOLVER): geo('us')}}


def probe(name, argv=(PROXY, KEY, '', '1'), stun=DEAD, mask=MASK, env=None, tool='claude', arm=None, **world):
    """登记一个 leak-probe 的用例。world 交给 net()；arm 是假 STUN 服务器的两个端口这次怎么回答（{'a': […], 'b': […]}）"""
    @case('leak-probe/' + name, ['leak-probe'] + list(argv), env=dict({'CCA_STUN': stun}, **(env or {})), mask=mask, tool=tool)
    def _(b):
        net(b, **world)
        STUN.arm(b, **(arm or {}))


probe('off', cfg={'leak_check': False}, dns=DNS_OK)
probe('off-webrtc-0', argv=(PROXY, KEY, '', '0'), cfg={'leak_check': False, 'nodes': ['US-A']})
for name, v in [('zero', 0), ('null', None), ('string', 'false'), ('true', True)]:
    probe('leak-check-%s-is-not-off' % name, cfg={'leak_check': v}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('config-bad-json', cfg='{"leak_check": fal', dns=DNS_OK, geoip={RESOLVER: where()})
probe('config-not-object', cfg='[false]', dns=DNS_OK, geoip={RESOLVER: where()})

# DNS：两个一次性的子域名经账号的端口，再问是哪些解析器来查的，解析器的归属地也经它查
probe('dns-ok', dns=DNS_OK, geoip={RESOLVER: where()})
probe('dns-ok-codex', dns=DNS_OK, geoip={RESOLVER: where()}, tool='codex')
probe('dns-cn-resolver', dns={'dns_servers': ['223.5.5.5']}, geoip={'223.5.5.5': where('cn', 'Alibaba', region='Zhejiang', city='Hangzhou')})
probe('webrtc-off', argv=(PROXY, KEY, '', '0'), dns=DNS_OK, geoip={RESOLVER: where()})
probe('webrtc-other-value', argv=(PROXY, KEY, '', 'no'), dns=DNS_OK, geoip={RESOLVER: where()})
probe('args-two', argv=(PROXY, KEY), dns=DNS_OK, geoip={RESOLVER: where()})
probe('args-three', argv=(PROXY, KEY, ''), dns=DNS_OK, geoip={RESOLVER: where()})
probe('args-one', argv=(PROXY,))
probe('args-five', argv=(PROXY, KEY, '', '1', 'x'))
probe('direct-when-no-proxy', argv=('', KEY, '', '1'), dns=DNS_OK, geoip={RESOLVER: where()})
probe('local-proxy-key', argv=('http://127.0.0.1:7902', 'local|节点 B', '', '1'), dns=DNS_OK, geoip={RESOLVER: where()})

# 缓存：按 线路|节点，测成的 6 小时、没测成的 10 分钟；能用就不测 DNS（一个请求都不发）
probe('cache-hit', cache=FRESH, dns=DNS_OK)
probe('cache-hit-webrtc-off', argv=(PROXY, KEY, '', '0'), cache=FRESH, dns=DNS_OK)
probe('cache-other-key', cache={'dns': {'local|US-A': FRESH['dns'][KEY]}, 'geo': FRESH['geo']}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-ok-expired', cache={'dns': {KEY: {'t': Ago(6 * 3600 + 60), 'ok': True, 'servers': [RESOLVER]}}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-ok-almost-expired', cache={'dns': {KEY: {'t': Ago(6 * 3600 - 120), 'ok': True, 'servers': [RESOLVER]}}}, dns=DNS_OK)
probe('cache-failed-fresh', cache={'dns': {KEY: {'t': Ago(60), 'ok': False, 'servers': []}}}, dns=DNS_OK)   # iso-3：没测成的只短暂缓存
probe('cache-failed-expired', cache={'dns': {KEY: {'t': Ago(700), 'ok': False, 'servers': []}}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-ok-missing-counts-as-failed', cache={'dns': {KEY: {'t': Ago(700), 'servers': [RESOLVER]}}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-float-and-bool-time', cache={'dns': {KEY: {'t': Ago(60.5), 'ok': 1, 'servers': [RESOLVER]}}}, dns=DNS_OK)
for name, entry in [('no-time', {'ok': True}), ('time-string', {'t': '1795058400', 'ok': True}), ('time-null', {'t': None, 'ok': True}),
                    ('not-object', [1795058400, True]), ('time-true', {'t': True, 'ok': True}), ('time-future', {'t': Ago(-3600), 'ok': True})]:
    probe('cache-entry-%s' % name, cache={'dns': {KEY: entry}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-bad-json', cache='{"dns": ', dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-not-object', cache='["dns"]', dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-sections-null', cache={'dns': None, 'geo': None}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-sections-empty-list', cache={'dns': [], 'geo': ''}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('cache-geo-not-object', cache={'geo': ['x']}, dns=DNS_OK)                      # Python：没人接的 AttributeError
probe('cache-dns-not-object', cache={'dns': 'x'}, dns=DNS_OK)
probe('cache-geo-time-string', cache={'geo': {net24(RESOLVER): geo('us', t='yesterday')}}, dns=DNS_OK)   # Python：没人接的 TypeError
probe('cache-geo-time-null', cache={'geo': {net24(RESOLVER): geo('us', t=None)}}, dns=DNS_OK)

# cca check（给了开始时间）：那之前测的不算；每个请求多等一会儿（图片 6 秒、问解析器 8 秒、归属地 8 秒）
probe('since-after-cached', argv=(PROXY, KEY, '4102444800', '1'), cache=FRESH, dns=DNS_OK)
probe('since-before-cached', argv=(PROXY, KEY, '1000000000', '1'), cache=FRESH, dns=DNS_OK)
probe('since-float', argv=(PROXY, KEY, ' 4102444800.5 ', '1'), cache=FRESH, dns=DNS_OK)
probe('since-patient-geo', argv=(PROXY, KEY, '1795058400', '0'), dns=DNS_OK, geoip={RESOLVER: where()})
probe('since-direct', argv=('', KEY, '1795058400.25', '1'), dns=DNS_OK, geoip={RESOLVER: where()})
probe('since-zero', argv=(PROXY, KEY, '0', '1'), cache=FRESH, dns=DNS_OK)
probe('since-nan', argv=(PROXY, KEY, 'nan', '1'), cache=FRESH, dns=DNS_OK)
probe('since-inf', argv=(PROXY, KEY, 'inf', '1'), cache=FRESH, dns=DNS_OK)
probe('since-bad', argv=(PROXY, KEY, 'yesterday', '1'), cache=FRESH, dns=DNS_OK)     # Python：没人接的 ValueError

# 解析器的归属地：按 /24、/48 网段记 7 天（没查到的 1 小时），缓存里有就不再查
probe('geo-known', cache={'geo': {net24(RESOLVER): geo('ca', isp='Known ISP', extra=[1, {'k': 'v'}])}}, dns=DNS_OK)
probe('geo-known-expired', cache={'geo': {net24(RESOLVER): geo('ca', age=7 * 86400 + 60)}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('geo-known-miss-fresh', cache={'geo': {net24(RESOLVER): geo('', age=60, miss=True, country='')}}, dns=DNS_OK)
probe('geo-known-miss-expired', cache={'geo': {net24(RESOLVER): geo('', age=3700, miss=True, country='')}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('geo-known-not-object', cache={'geo': {net24(RESOLVER): 'us'}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('geo-known-no-time', cache={'geo': {net24(RESOLVER): {'cc': 'us'}}}, dns=DNS_OK, geoip={RESOLVER: where()})
probe('geo-down', dns=DNS_OK)
probe('geo-http-error', dns=DNS_OK, geoip={RESOLVER: reply('{"error": "rate limited"}', rc=22)})
probe('geo-not-json', dns=DNS_OK, geoip={RESOLVER: reply('<html>502</html>')})
probe('geo-empty-body', dns=DNS_OK, geoip={RESOLVER: reply('')})
probe('geo-list', dns=DNS_OK, geoip={RESOLVER: [where()]})
probe('geo-empty-object', dns=DNS_OK, geoip={RESOLVER: {}})
probe('geo-odd-types', dns=DNS_OK, geoip={RESOLVER: {'country_code': 86, 'country': ['中国', 'CN'], 'isp': {'name': 'x'}}})
probe('geo-nulls-and-case', dns=DNS_OK, geoip={RESOLVER: {'country_code': 'US', 'country': None, 'isp': False}})
probe('geo-unicode', dns=DNS_OK, geoip={RESOLVER: where('cn', '中国电信 ☁', country='中国')})
probe('geo-bytes-not-utf8', dns=DNS_OK, geoip={RESOLVER: "printf '{\"country_code\": \"us\", \"isp\": \"caf\\351\"}'\n"})
probe('geo-ipv6-resolver', dns={'dns_servers': ['2001:db8:85a3:1::53', '2001:DB8:85a3:2::53']}, geoip={'2001:db8:85a3:1::53': where('de')})
probe('geo-same-net-once', dns={'dns_servers': ['203.0.113.53', '203.0.113.54', '203.0.113.200']}, geoip={RESOLVER: where()})
probe('geo-two-nets-slow-one-last', dns={'dns_servers': ['198.51.100.53', RESOLVER]},
      geoip={'198.51.100.53': reply(where('jp'), wait=0.6), RESOLVER: where()})
probe('geo-known-first-then-looked-up', cache={'geo': {'198.51.100.0/24': geo('jp')}}, dns={'dns_servers': [RESOLVER, '198.51.100.53']},
      geoip={RESOLVER: where()})

# 问解析器：只收是 IP 样子的文字，去重，最多 16 个
probe('servers-filtered', dns={'dns_servers': [RESOLVER, 5, None, 'not an ip', RESOLVER, '', 'x', ['203.0.113.60'], '203.0.113.61', '2001:db8::1',
                                               'A' * 46, 'g::1', True, {'ip': RESOLVER}]},
      geoip={RESOLVER: where(), '2001:db8::1': reply(where('de'), wait=0.6)})
probe('servers-hex-word-passes-for-an-ip', dns={'dns_servers': ['ab']})   # 只看字符像不像：归属地查不到，汇总时算没测成
# 正则的 $ 也认结尾那个换行之前：带着换行的“IP”照收（原来就这样），归属地的网址里也带着它
probe('servers-trailing-newline', dns={'dns_servers': ['203.0.113.53\n']})
probe('servers-capped-at-16', dns={'dns_servers': ['203.0.113.%d' % i for i in range(1, 30)]}, geoip={'203.0.113.1': where()})
probe('dns-retry-then-ok', dns=[{'dns_servers': []}, DNS_OK], geoip={RESOLVER: where()}, mask=SLOW)
# 问满四次都不能用（连不上、不是对象、dns_servers 不是列表、里面没有一个像 IP）：不算测成
probe('dns-never-usable', dns=[reply(rc=7), ['203.0.113.53'], {'dns_servers': RESOLVER}, {'dns_servers': ['nope', 7]}], mask=SLOW)
# iso-12：解析器一直没来查过（回答里是空的）——问满四次，不算测成
probe('iso-dns-polling-without-resolvers', dns={'dns_servers': []}, pixel=reply({'dns_servers': []}), mask=SLOW)
probe('pixel-fails', pixel=reply(rc=7), dns=DNS_OK, geoip={RESOLVER: where()})
probe('pixel-hangs', pixel=reply(wait=3), dns=DNS_OK, geoip={RESOLVER: where()}, mask=SLOW)   # 不等图片请求做完：最多一秒就去问
probe('no-curl', env={'PATH': '/nonexistent-cca-pydiff'}, dns=DNS_OK, mask=SLOW)   # curl 起不来：当作拿不到
# 归属地迟迟查不回来：到时间（5 秒，再宽限 1 秒）不等了，结果里没有 dns——汇总时算没测成
probe('deadline-drops-unfinished-dns', pixel=reply(wait=2), dns=DNS_OK, geoip={RESOLVER: reply(where(), wait=7)}, mask=SLOW)

# WebRTC：本机 UDP 直连问 STUN（下面这些 DNS 都用缓存，只看 stun）
X = [('xor', '198.51.100.77')]
probe('stun-answer', stun=STUN_A, cache=FRESH, arm={'a': X})
probe('stun-two-servers-sorted', stun='%s,%s' % (STUN_A, STUN_B), cache=FRESH, arm={'a': X, 'b': [('xor', '192.0.2.1')]})
probe('stun-two-servers-same-ip', stun='%s,%s' % (STUN_B, STUN_A), cache=FRESH, arm={'a': X, 'b': X})
probe('stun-one-answers-one-silent', stun='%s,%s' % (STUN_A, STUN_B), cache=FRESH, arm={'b': X})
probe('stun-same-server-twice', stun='%s,%s' % (STUN_A, STUN_A), cache=FRESH, arm={'a': X})
probe('stun-ipv6-address-in-answer', stun=STUN_A, cache=FRESH, arm={'a': [('xor', '2001:db8:85a3::8a2e:370:7334')]})
probe('stun-ipv6-mapped-address-in-answer', stun=STUN_A, cache=FRESH, arm={'a': [('mapped', '2001:DB8::1')]})
probe('stun-mapped-address', stun=STUN_A, cache=FRESH, arm={'a': [('mapped', '192.0.2.1')]})
probe('stun-old-xor-attr', stun=STUN_A, cache=FRESH, arm={'a': [('old', '192.0.2.2')]})
probe('stun-junk-then-answer', stun=STUN_A, cache=FRESH, arm={'a': ['junk', ('other-tid', '192.0.2.9'), ('error', '192.0.2.8')] + X})
probe('stun-no-answer', stun=STUN_A, cache=FRESH)
probe('stun-only-error-response', stun=STUN_A, cache=FRESH, arm={'a': [('error', '192.0.2.8')]})
probe('stun-with-dns', stun=STUN_A, dns=DNS_OK, geoip={RESOLVER: where()}, arm={'a': X})
probe('stun-env-nothing-usable', stun='nonsense,:9,host:,a:b:1,host:port,', cache=FRESH)
probe('stun-env-picks-usable', stun='nonsense,%s,::1:9,%s' % (DEAD, STUN_A), cache=FRESH, arm={'a': X})
probe('stun-env-trailing-newline-and-fullwidth-port', stun='127.0.0.1:９\n', cache=FRESH)
probe('stun-host-python-cannot-encode', stun='a..b:9', cache=FRESH)   # Python：线程里没人接的 UnicodeError（结果一样是没有）
probe('stun-port-out-of-range', stun='127.0.0.1:99999', cache=FRESH)
probe('stun-off-never-asks', argv=(PROXY, KEY, '', '0'), stun=STUN_A, cache=FRESH, arm={'a': X})


# ---- leak-verdict：汇总探测结果，写 leak.json，打印横幅那一句和 W / N / S 行 ------------------------------------------------

def dns(cc='us', servers=None, **kw):
    """探测结果里的 dns 一节（tests/isolation_leaks.py 的同名函数）"""
    servers = [RESOLVER] if servers is None else servers
    return dict({'ok': True, 'servers': servers, 'geo': {net24(ip): geo(cc) for ip in servers}}, **kw)


def verdict(name, found, key=KEY, exit_ip=EXIT_IP, exit_cc='US', detail='detail', mask=MASK, tool='claude', argv=None, **world):
    """登记一个 leak-verdict 的用例：found 是 leak-probe 打印的那个结果（写进沙箱的 probe.json）"""
    args = [key, exit_ip, exit_cc] + ([] if detail is None else [detail]) if argv is None else list(argv)
    @case('leak-verdict/' + name, ['leak-verdict', 'probe.json'] + args, mask=mask, tool=tool)
    def _(b):
        net(b, **world)
        if isinstance(found, str):
            b.write('probe.json', found)
        elif found is not None:
            probe_file(b, found)


# 探测没做、没做完、结果读不了：什么都不说（不声称没问题），也不写缓存
verdict('no-file', None)
verdict('empty-file', '')
verdict('bad-json', '{"stun": {"ips": [')
verdict('not-object', '[{"stun": {"ips": []}}]')
verdict('iso-disabled-check-emits-no-claim', {'off': True})
verdict('off-truthy', {'off': 1, 'stun': {'ips': [EXIT_IP]}})
verdict('off-false-is-not-off', {'off': False, 'stun': {'ips': [EXIT_IP]}})

# iso-1：没有 STUN 的结果（空的、没有这一节、整个是空的）算没测成
verdict('iso-empty-stun', {'dns': dns(), 'stun': {'ips': []}})
verdict('iso-missing-stun', {'dns': dns()})
verdict('iso-nothing-probed', {})
# iso-2：服务器上（leak-probe 写明 webrtc: off）不测也不说 WebRTC
verdict('iso-webrtc-off', {'dns': dns(), 'webrtc': 'off'})
verdict('webrtc-off-no-dns', {'webrtc': 'off'})
verdict('webrtc-off-but-stun-present', {'webrtc': 'off', 'dns': dns(), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where('cn')})
verdict('webrtc-other-value', {'webrtc': 'OFF', 'dns': dns()})
# iso-3：解析器一个都没拿到——没测成，缓存里记的也是没测成
verdict('iso-empty-dns-response', {'dns': dns(servers=[]), 'stun': {'ips': []}})
# iso-4：解析器的归属地没查到（空的、不是两个小写字母）——没测成
for cc in ['', 'unknown', '??', 'US', 'u', 'us ']:
    verdict('iso-missing-dns-geo/%s' % (cc or 'empty'), {'dns': dns(cc), 'stun': {'ips': [EXIT_IP]}})
# iso-5：解析器在别的国家：只说明没发现地区异常；STUN 看到的就是出口：只确认 STUN 出口一致
verdict('iso-foreign-resolver', {'dns': dns('ca'), 'stun': {'ips': [EXIT_IP]}})
verdict('foreign-resolver-brief', {'dns': dns('ca'), 'stun': {'ips': [EXIT_IP]}}, detail=None)
verdict('foreign-resolver-empty-detail', {'dns': dns('ca'), 'stun': {'ips': [EXIT_IP]}}, detail='')
# iso-6：有一个中国大陆的解析器就报，另一个归属地不明也不影响
verdict('iso-cn-resolver-with-unknown', {'dns': dict(dns('cn'), servers=[RESOLVER, '192.0.2.53']), 'stun': {'ips': [EXIT_IP]}})
# iso-7 / iso-8 / iso-9 / iso-10：STUN 看到的不是出口——中国大陆的才算风险，别的地区只作提醒，归属地查不到算没测成
verdict('iso-mainland-stun-ip', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us'), '192.0.2.0/24': geo('cn')}), 'stun': {'ips': ['192.0.2.77']}})
for cc in ['us', 'hk', 'mo', 'tw']:
    verdict('iso-other-region-is-a-note/%s' % cc,
            {'dns': dict(dns(), geo={net24(RESOLVER): geo('us'), '192.0.2.0/24': geo(cc)}), 'stun': {'ips': ['192.0.2.77']}})
verdict('iso-any-mainland-address', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us'), '192.0.2.0/24': geo('us'), '198.51.100.0/24': geo('cn')}),
                                     'stun': {'ips': ['192.0.2.77', '198.51.100.77']}})
verdict('iso-unknown-location', {'dns': dns(), 'stun': {'ips': ['192.0.2.77']}})      # 归属地直连去查，查不到
# iso-11 的后一半：STUN 解析不了、超时（结果是空的）——没测成
verdict('iso-stun-failed-no-dns', {'stun': {'ips': []}})

# DNS 的三种结论和各自的说法
verdict('dns-leak', {'dns': dns('cn', ['223.5.5.5']), 'stun': {'ips': [EXIT_IP]}}, key='router|US-B')
verdict('dns-leak-brief', {'dns': dns('cn', ['223.5.5.5']), 'stun': {'ips': [EXIT_IP]}}, detail=None)
verdict('dns-leak-five-cn-lists-three', {'dns': dns('cn', ['223.5.5.5', '223.6.6.6', '114.114.114.114', '119.29.29.29', RESOLVER]),
                                         'stun': {'ips': [EXIT_IP]}})
verdict('dns-leak-cn-among-others', {'dns': dict(dns(servers=[RESOLVER, '223.5.5.5', '198.51.100.53']),
                                                 geo={net24(RESOLVER): geo('us'), '223.5.5.0/24': geo('cn', isp='Alibaba'), '198.51.100.0/24': geo('jp')})})
verdict('dns-leak-no-isp', {'dns': dict(dns('cn', ['223.5.5.5']), geo={'223.5.5.0/24': {'t': Ago(60), 'cc': 'cn', 'country': '', 'isp': ''}})})
verdict('dns-leak-key-without-bar', {'dns': dns('cn', ['223.5.5.5'])}, key='US-A')
verdict('dns-leak-key-with-two-bars', {'dns': dns('cn', ['223.5.5.5'])}, key='local|节点|1')
verdict('dns-leak-exit-cc-lower', {'dns': dns('cn', ['223.5.5.5'])}, exit_cc='us')
verdict('dns-cn-resolver-exit-in-cn', {'dns': dns('cn', ['223.5.5.5'])}, exit_cc='cn')
verdict('dns-cn-resolver-exit-in-cn-mixed-case', {'dns': dns('cn', ['223.5.5.5'])}, exit_cc='Cn')
verdict('dns-cn-resolver-no-exit-cc', {'dns': dns('cn', ['223.5.5.5'])}, exit_cc='')
verdict('dns-ok-no-exit-cc', {'dns': dns()}, exit_cc='')
verdict('dns-not-ok-with-servers', {'dns': dns(ok=False)})
verdict('dns-not-ok-cn-resolver', {'dns': dns('cn', ok=False)})
verdict('dns-ok-truthy', {'dns': dns(ok=1)})
verdict('dns-many-countries', {'dns': dict(dns(servers=['198.51.100.1', '198.51.101.1', '198.51.102.1', '198.51.103.1', '198.51.104.1']),
                                           geo={'198.51.100.0/24': geo('us'), '198.51.101.0/24': geo('ca'), '198.51.102.0/24': geo('us'),
                                                '198.51.103.0/24': geo('jp'), '198.51.104.0/24': geo('de')})})
verdict('dns-three-countries', {'dns': dict(dns(servers=['198.51.100.1', '198.51.101.1', '198.51.102.1']),
                                            geo={'198.51.100.0/24': geo('us'), '198.51.101.0/24': geo('ca'), '198.51.102.0/24': geo('jp')})})
verdict('dns-country-unnamed', {'dns': dict(dns(), geo={net24(RESOLVER): {'t': Ago(60), 'cc': 'us', 'country': '', 'isp': ''}})})
verdict('dns-ipv6-resolver', {'dns': dict(dns(servers=['2001:db8:85a3:1::53']), geo={'2001:db8:85a3::/48': geo('de')})})
verdict('dns-servers-not-text-dropped', {'dns': dict(dns(), servers=[RESOLVER, 5, None, ['x']])})
verdict('dns-not-an-object', {'dns': [True], 'stun': {'ips': [EXIT_IP]}})
verdict('dns-geo-missing', {'dns': {'ok': True, 'servers': [RESOLVER]}})
verdict('dns-probe-geo-wins-over-cache', {'dns': dns('ca')}, cache={'geo': {net24(RESOLVER): geo('cn', age=30)}, 'dns': {}})
# 探测结果里的归属地不筛新旧：没有时间的照收，排在最后；时间是 true 的当 1
verdict('dns-probe-geo-without-time-sorts-last',
        {'dns': dict(dns(), geo={'198.51.100.0/24': {'cc': 'jp', 'country': 'Japan'}, '198.51.101.0/24': {'t': True, 'cc': 'de'},
                                 net24(RESOLVER): geo('us', age=5000)})},
        cache={'geo': {'192.0.2.0/24': geo('ca', age=6000)}})

# 没有这次的 DNS 结果（探测用了缓存）：照缓存里的说；缓存里的归属地过期了就是没测成，留下的也改成没测成
verdict('cached-dns-ok', {'stun': {'ips': [EXIT_IP]}}, cache=FRESH, mask=[])
verdict('cached-dns-float-times-kept', {'stun': {'ips': [EXIT_IP]}}, mask=[],
        cache={'dns': {KEY: {'t': Ago(60.25), 'ok': True, 'servers': [RESOLVER]}}, 'geo': {net24(RESOLVER): geo('us', t=Ago(3600.5), extra={'k': [1.5, None]})},
               'other': {'kept': True}})
verdict('cached-dns-leak', {'stun': {'ips': [EXIT_IP]}}, mask=[],
        cache={'dns': {KEY: {'t': Ago(60), 'ok': True, 'servers': ['223.5.5.5']}}, 'geo': {'223.5.5.0/24': geo('cn', isp='Alibaba')}})
verdict('cached-dns-geo-expired', {'stun': {'ips': [EXIT_IP]}}, mask=[],
        cache={'dns': {KEY: {'t': Ago(60), 'ok': True, 'servers': [RESOLVER]}}, 'geo': {net24(RESOLVER): geo('us', age=7 * 86400 + 60)}})
verdict('cached-dns-failed', {'stun': {'ips': [EXIT_IP]}}, cache={'dns': {KEY: {'t': Ago(60), 'ok': False, 'servers': []}}}, mask=[])
verdict('cached-dns-expired', {'stun': {'ips': [EXIT_IP]}}, cache={'dns': {KEY: {'t': Ago(6 * 3600 + 60), 'ok': True, 'servers': [RESOLVER]}},
                                                                   'geo': FRESH['geo']}, mask=[])
verdict('cached-other-keys-kept-or-dropped', {'dns': dns()},
        cache={'geo': {'198.51.100.0/24': geo('jp', age=120), '192.0.2.0/24': geo('de', age=8 * 86400), '203.0.114.0/24': geo('', age=30, miss=True),
                       '203.0.115.0/24': geo('', age=4000, miss=True), '2001:db8::/48': geo('ca', age=10)},
               'dns': {'local|US-B': {'t': Ago(100), 'ok': True, 'servers': ['198.51.100.9']}, KEY: {'t': Ago(50), 'ok': False, 'servers': []},
                       'router|OLD': {'t': Ago(7 * 3600), 'ok': True, 'servers': []}, 'router|FAILED': {'t': Ago(700), 'ok': False, 'servers': []},
                       'router|BAD': 'x', 'router|NO-T': {'ok': True}},
               'version': 1})
verdict('cached-servers-as-text', {'stun': {'ips': [EXIT_IP]}}, mask=[],
        cache={'dns': {KEY: {'t': Ago(60), 'ok': True, 'servers': 'ab'}}, 'geo': {'a': geo('us'), 'b': geo('ca')}})
verdict('cache-bad-json', {'dns': dns(), 'stun': {'ips': [EXIT_IP]}}, cache='{"geo": ')
verdict('cache-not-object', {'dns': dns(), 'stun': {'ips': [EXIT_IP]}}, cache='[]')
verdict('geo-capped-at-300-newest-first', {'stun': {'ips': [EXIT_IP]}}, mask=[],
        cache={'geo': dict([('10.%d.%d.0/24' % (i // 250, i % 250), geo('us', age=600 + (i * 7) % 311)) for i in range(320)]), 'dns': {}})

# WebRTC 的四种结论；STUN 看到的地址直连去查归属地（不经账号的端口），查到的记进缓存
verdict('webrtc-leak-looked-up', {'dns': dns(), 'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn', 'Example CN')})
verdict('webrtc-leak-codex', {'dns': dns(), 'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn', 'Example CN')}, tool='codex')
verdict('webrtc-leak-agy', {'dns': dns(), 'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn', 'Example CN')}, tool='agy')
verdict('webrtc-leak-no-where', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': {'country_code': 'CN'}})
verdict('webrtc-leak-not-builtin-proxy', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn')},
        cfg={'local': {'client': 'clash-verge', 'udp': 'US-A'}, 'route': 'local'})
verdict('webrtc-leak-local-empty', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn')}, cfg={'local': {}})
verdict('webrtc-leak-config-bad-json', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn')}, cfg='{"local": ')
verdict('webrtc-leak-config-not-object', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn')}, cfg='["builtin"]')
verdict('webrtc-leak-local-not-object', {'stun': {'ips': ['198.51.100.77']}}, geoip={'198.51.100.77': where('cn')}, cfg={'local': ['builtin']})
verdict('webrtc-note-looked-up', {'dns': dns(), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()})
verdict('webrtc-note-codex', {'dns': dns(), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()}, tool='codex')
verdict('webrtc-note-agy', {'dns': dns(), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()}, tool='agy')
verdict('webrtc-note-no-isp', {'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': {'country_code': 'jp', 'country': 'Japan'}})
verdict('webrtc-note-isp-only', {'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': {'country_code': 'JP', 'isp': 'Example KK'}})
verdict('webrtc-unknown-not-json', {'dns': dns(), 'stun': {'ips': ['198.18.0.77']}}, geoip={'198.18.0.77': reply('not json')})
verdict('webrtc-unknown-no-code', {'dns': dns(), 'stun': {'ips': ['198.18.0.77']}}, geoip={'198.18.0.77': {'country': 'Somewhere', 'isp': 'ISP'}})
verdict('webrtc-unknown-one-of-two', {'stun': {'ips': ['192.0.2.77', '198.18.0.77']}}, geoip={'192.0.2.77': reply(where(), wait=0.6)})
verdict('webrtc-not-an-ip-is-never-looked-up', {'stun': {'ips': ['not an ip', 'x']}})
verdict('webrtc-lookup-hangs-and-is-killed', {'stun': {'ips': ['198.18.0.77']}}, geoip={'198.18.0.77': reply(where('cn'), wait=6)})
verdict('webrtc-two-lookups-newest-first', {'stun': {'ips': ['192.0.2.77', '198.51.100.77']}},
        geoip={'192.0.2.77': where('jp'), '198.51.100.77': reply(where('cn', 'Example CN'), wait=0.6)})
verdict('webrtc-same-net-looked-up-once', {'stun': {'ips': ['192.0.2.77', '192.0.2.78']}}, geoip={'192.0.2.77': where('de')})
verdict('webrtc-known-from-cache', {'stun': {'ips': ['192.0.2.77']}}, cache={'geo': {'192.0.2.0/24': geo('cn', isp='Cached CN')}}, mask=[])
verdict('webrtc-cached-miss-fresh', {'stun': {'ips': ['192.0.2.77']}}, cache={'geo': {'192.0.2.0/24': geo('', miss=True, country='', isp='')}},
        geoip={'192.0.2.77': where('cn')}, mask=[])
verdict('webrtc-cached-miss-expired', {'stun': {'ips': ['192.0.2.77']}},
        cache={'geo': {'192.0.2.0/24': geo('', age=3700, miss=True, country='', isp='')}}, geoip={'192.0.2.77': where('cn')})
verdict('webrtc-ipv6', {'stun': {'ips': ['2001:db8:85a3::8a2e:370:7334', EXIT_IP]}}, geoip={'2001:db8:85a3::8a2e:370:7334': where('cn', 'Example CN6')})
verdict('webrtc-exit-is-ipv6', {'stun': {'ips': ['2001:db8::1']}}, exit_ip='2001:db8::1')
verdict('webrtc-one-matches-one-not', {'stun': {'ips': [EXIT_IP, '192.0.2.77']}}, geoip={'192.0.2.77': where()})
verdict('webrtc-ips-not-text-dropped', {'stun': {'ips': [5, None, EXIT_IP, ['192.0.2.77']]}})
verdict('webrtc-stun-not-object', {'stun': [EXIT_IP]})
verdict('webrtc-ips-null', {'stun': {'ips': None}})
verdict('exit-ip-empty', {'dns': dns(), 'stun': {'ips': []}}, exit_ip='')
verdict('dns-leak-and-webrtc-leak', {'dns': dict(dns('cn', ['223.5.5.5']), geo={'223.5.5.0/24': geo('cn', isp='Alibaba'), '198.51.100.0/24': geo('cn')}),
                                     'stun': {'ips': ['198.51.100.77']}})
verdict('dns-failed-and-webrtc-note', {'dns': dns(servers=[]), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where('hk')})

# 写不了缓存照样说结论；参数个数不对、探测结果里的东西不成样子是 Python 里没人接的异常
verdict('args-three', {'stun': {'ips': []}}, argv=[KEY, EXIT_IP])
verdict('args-six', {'stun': {'ips': []}}, argv=[KEY, EXIT_IP, 'US', 'detail', 'x'])
verdict('crash-cache-geo-not-object', {'stun': {'ips': []}}, cache={'geo': [1]})
verdict('crash-cache-dns-not-object', {'stun': {'ips': []}}, cache={'dns': 'x'})
verdict('crash-cache-geo-time-text', {'stun': {'ips': []}}, cache={'geo': {'192.0.2.0/24': geo('us', t='now')}})
verdict('crash-probe-geo-is-number', {'dns': dict(dns(), geo=5), 'stun': {'ips': []}})
# 探测结果里的 geo 是“一串两项的东西”时 Python 的 dict.update 也收（两个字符的文字也算两项）；别的列表、文字是没人接的异常
verdict('probe-geo-as-pairs', {'dns': dict(dns(), geo=[[net24(RESOLVER), geo('ca')], ['192.0.2.0/24', geo('jp', age=30)]])})
verdict('crash-probe-geo-two-char-text-later', {'dns': dict(dns(), geo=['us']), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()})
verdict('crash-probe-geo-three-items', {'dns': dict(dns(), geo=[['a', 'b', 'c']]), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()})
verdict('crash-probe-geo-text', {'dns': dict(dns(), geo='ab'), 'stun': {'ips': ['192.0.2.77']}}, geoip={'192.0.2.77': where()})
verdict('crash-probe-geo-list-of-numbers', {'dns': dict(dns(), geo=[5]), 'stun': {'ips': []}})
verdict('crash-probe-geo-entry-not-object', {'dns': dict(dns(), geo={net24(RESOLVER): 'us'}), 'stun': {'ips': []}})
verdict('crash-probe-geo-entry-null', {'dns': dict(dns(), geo={'192.0.2.0/24': None}), 'stun': {'ips': []}})
verdict('crash-probe-geo-time-null', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us', t=None)}), 'stun': {'ips': []}})
verdict('crash-probe-servers-number', {'dns': dict(dns(), servers=5), 'stun': {'ips': []}})
verdict('crash-cached-server-not-text', {'stun': {'ips': []}}, cache={'dns': {KEY: {'t': Ago(60), 'ok': True, 'servers': [5]}}})
verdict('crash-cc-not-text', {'dns': dict(dns(), geo={net24(RESOLVER): geo(86)}), 'stun': {'ips': []}})
verdict('crash-country-not-text', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us', country=86)}), 'stun': {'ips': []}})
verdict('country-not-text-but-brief', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us', country=86)}), 'stun': {'ips': []}}, detail=None)
verdict('crash-country-is-list', {'dns': dict(dns(), geo={net24(RESOLVER): geo('us', country=['US'])}), 'stun': {'ips': []}}, detail=None)
verdict('crash-where-not-text', {'dns': dict(dns('cn'), geo={net24(RESOLVER): geo('cn', isp=5)}), 'stun': {'ips': []}})
verdict('crash-ips-number', {'stun': {'ips': 5}})


@case('leak-verdict/cache-unwritable', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK)
def _(b):
    net(b)
    probe_file(b, {'dns': dns(), 'stun': {'ips': [EXIT_IP]}})
    b.write('.cca', 'not a directory')   # ~/.cca 是个文件：读不到缓存，也写不了，结论照说


@case('leak-verdict/cache-mode-reset-to-600', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK)
def _(b):
    net(b)
    b.json('.cca/leak.json', {'geo': {}, 'dns': {}}, mode=0o644)   # 缓存一律写成只有自己能读写
    probe_file(b, {'dns': dns(), 'stun': {'ips': [EXIT_IP]}})


@case('leak-verdict/fresh-home', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK)
def _(b):
    tick()
    b.fake('curl', CURL)   # 还没有 ~/.cca：写缓存时建出来
    probe_file(b, {'dns': dns(), 'stun': {'ips': [EXIT_IP]}})


@case('leak-verdict/readonly-env-still-writes', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK, env={'CCA_RO': '1'})
def _(b):
    net(b, cache={'dns': {}, 'geo': {}})
    probe_file(b, {'dns': dns(), 'stun': {'ips': [EXIT_IP]}})


@case('leak-verdict/cache-is-a-symlink', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK)
def _(b):
    net(b)
    b.json('elsewhere/leak.json', settle({'geo': {'198.51.100.0/24': geo('jp')}}), mode=0o644)
    os.makedirs(b.path('.cca'), exist_ok=True)
    os.symlink('../elsewhere/leak.json', b.path('.cca/leak.json'))
    probe_file(b, {'dns': dns(), 'stun': {'ips': [EXIT_IP]}})


@case('leak-verdict/probe-path-absolute-missing', ['leak-verdict', '/nonexistent-cca-pydiff/probe.json', KEY, EXIT_IP, 'US'])
def _(b):
    net(b)


# CCA_HOME 换了地方：缓存、config.json 都跟着（相对路径照当前目录算，对拍的当前目录就是沙箱）
@case('leak-verdict/cca-home-elsewhere', ['leak-verdict', 'probe.json', KEY, EXIT_IP, 'US'], mask=MASK, env={'CCA_HOME': 'alt/home'})
def _(b):
    net(b, cache={'geo': {'198.51.100.0/24': geo('cn')}})          # 不在 CCA_HOME 里的这一份不该被读到
    b.json('alt/home/leak.json', settle({'geo': {'198.51.100.0/24': geo('jp')}}))
    probe_file(b, {'dns': dns(), 'stun': {'ips': ['198.51.100.77']}})


@case('leak-probe/cca-home-elsewhere', ['leak-probe', PROXY, KEY, '', '1'], mask=MASK, env={'CCA_HOME': 'alt/home', 'CCA_STUN': DEAD})
def _(b):
    net(b, cfg={'leak_check': False}, dns=DNS_OK, geoip={RESOLVER: where()})   # 这一份 config.json 不算数：照测
    b.json('alt/home/config.json', {'leak_check': True})
