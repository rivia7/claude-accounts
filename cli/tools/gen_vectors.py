#!/usr/bin/env python3
"""生成 cli/tests/vectors/*.json：Python 标准库对一批输入给出的结果，Rust 的移植照着核对（cargo test 读这些文件，
不用装 Python）。

    python3 -I cli/tools/gen_vectors.py cli/tests/vectors

原来的实现用的是用户机器上那个 Python 的标准库，各版本的行为有出入；Rust 这边固定成生成这些文件的那个版本。
换 Python 版本重新生成后，差异会在 git diff 和 cargo test 里看得到。"""
import base64
import datetime
import hashlib
import ipaddress
import json
import os
import plistlib
import struct
import sys
from urllib.parse import urlsplit, parse_qsl, quote, unquote, urljoin

URLS = [
    '', 'https://api.example.com/anthropic', 'https://api.example.com/v1/', 'HTTP://Example.COM:8080/a?b=1#c',
    'http://user:pw@host.example.com:99/p', 'http://user@host/', 'http://:pw@host/', 'http://@host/', 'http://a:b:c@host/',
    'https://[::1]:8443/x', 'https://[fe80::1%25eth0]/', 'http://[::1', 'http://::1]/', 'http://[v1.x]/', 'http://[1.2.3.4]/',
    'http://x[::1]/', 'http://[::1]x/', 'http://[::1]x:80/', 'http://host:80a/', 'http://host:99999/', 'http://host:/p', 'http://host:-1/',
    'http://host:٣/', 'http://HOST.Example.com./p', 'http:/onlypath', 'http:onlypath', '//host/p', '/just/path?q#f', 'no-scheme.example.com/p',
    'mailto:me@example.com', 'file:///etc/passwd', 'javascript:alert(1)', 'h+t.t-p://x/', '1http://x/', 'ht_tp://x/',
    ' https://lead.example.com/', 'https://trail.example.com/ ', '\x01https://c0.example.com/', 'ht\ttps://tab.example.com/a\nb',
    'https://例子.example.com/路径?查询=值#片段', 'https://ex\u2100mple.com/', 'https://a.example.com/p?x=1&y=2;z=3', 'https://a.example.com?x',
    'https://a.example.com#f', 'https://a.example.com/p#f?notquery', 'http://localhost:7901', 'http://127.0.0.1:7901/', 'socks5://127.0.0.1:1080',
    'https://claude.ai/oauth/authorize?code=true&client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&state=abc',
    'unix:/tmp/x.sock', 'http://host\\evil/', 'http://host/a b', 'HTTPS://UPPER/', 'https://-bad-.example.com/', 'https:///nohost', 'https://:443/',
    'http://[::ffff:1.2.3.4]:80/', 'http://[::1]:/', 'http://[::1]:0x50/', 'http://user:p%40ss@host/', 'ws://x', 'https://x:65535', 'https://x:65536',
]

IPS = [
    '0.0.0.0', '0.255.255.255', '1.1.1.1', '8.8.8.8', '10.0.0.1', '11.0.0.0', '100.64.0.1', '100.127.255.255', '100.128.0.0', '127.0.0.1', '127.255.255.255',
    '128.0.0.1', '169.254.1.1', '169.255.0.0', '172.15.255.255', '172.16.0.1', '172.31.255.255', '172.32.0.0', '192.0.0.1', '192.0.0.9', '192.0.0.10',
    '192.0.0.170', '192.0.0.171', '192.0.0.172', '192.0.1.0', '192.0.2.10', '192.0.3.0', '192.88.99.1', '192.168.2.1', '192.169.0.0', '198.17.255.255',
    '198.18.0.1', '198.19.255.255', '198.20.0.0', '198.51.100.7', '203.0.113.9', '203.0.114.0', '224.0.0.1', '239.255.255.255', '240.0.0.1', '255.255.255.255',
    '01.2.3.4', '1.2.3', '1.2.3.4.5', '256.1.1.1', '1.2.3.-4', ' 1.2.3.4', '1.2.3.4 ', '١.2.3.4', '1..3.4', '0x7f.0.0.1', '1.2.3.04',
    '::', '::1', '::2', '::ffff:1.2.3.4', '::ffff:10.0.0.1', '::ffff:127.0.0.1', '::1.2.3.4', '64:ff9b::1.2.3.4', '64:ff9b:1::1', '100::1', '100:0:0:1::',
    '2001::1', '2001:1::1', '2001:1::2', '2001:1::3', '2001:3::1', '2001:4:112::1', '2001:20::1', '2001:30::1', '2001:db8::1', '2001:200::1', '2002::1',
    '2400:cb00::1', '2606:4700:4700::1111', '3fff::1', '3fff:fff:ffff::1', '3fff:1000::', 'fc00::1', 'fd12:3456::1', 'fe80::1', 'fe80::1%eth0', 'fec0::1', 'ff02::1',
    '1:2:3:4:5:6:7:8', '1:2:3:4:5:6:7::', '::1:2:3:4:5:6:7', '1::8', '1:2:3:4:5:6:7:8:9', ':::', '1::2::3', '12345::', 'g::1', '1:2:3:4:5:6:1.2.3.4', '::1%', '::1%a%b', '[::1]', '',
    'localhost', 'example.com',
]

QUERIES = ['', 'a=1&b=2', 'a=1;b=2', 'a=%E4%B8%AD&b=x+y', 'a&b=', '=x', 'a=1&a=2', 'code=abc%0A&state=x', 'a=%ZZ&b=%ff', 'a==b&c=d=e', '&&a=1&&']
QUOTES = ['', 'abc', 'a b', 'a/b?c=d&e', '中文', 'a%20b', "it's~_.-", '\x00\x7f', 'a+b']
JOINS = [('https://a.example.com/x/y', 'z'), ('https://a.example.com/x/y', '/z'), ('https://a.example.com/x/y', '//b.example.com/z'),
         ('https://a.example.com/x/y', 'https://c.example.com/'), ('https://a.example.com/x/y', '../z'), ('https://a.example.com/x/y?q', '?r'),
         ('https://a.example.com/x/', './a/../b'), ('https://a.example.com', 'p'), ('https://a.example.com/x/y', ''), ('https://a.example.com/x/y', '#f')]


B64 = [
    '', 'YQ==', 'YWI=', 'YWJj', 'YWJjZA==', 'YQ', 'YQ=', 'YWI', 'Y', 'YWJjZ', 'YWJjZGVmZ', '====', '=', '==', '===', 'YQ===', 'YQ====', 'YQ==YQ==', 'YQ==x',
    'YQ==\n', 'YWJj\n', 'YW Jj', 'YW\nJj\n', ' YWJj', 'YW-_', 'YW+/', '-_-_', '+/+/', '*YQ==', 'Y*Q*=*=', 'YQ=a', 'YQ=aa', 'YQ=a=', 'a=aa', 'ab=c', 'abc=d',
    'abcd=', 'abcd==', 'abcde===', '=abcd', '=YQ==', 'Y=Q==', 'YQ=\n=', '中文', 'YQ==中', 'YQ==\x00', '\x00', 'eyJhbGciOiJIUzI1NiJ9', '////', '++++', 'AAAA', 'AA==',
    'AB==', 'AAB=', 'YR==', 'YWJ=', 'YW=', 'YWJjZA=', 'YWJjZGU=', 'YWJjZGVm', 'YWJjZGVmZw', 'YWJjZGVmZw=', 'YWJjZGVmZw==', 'YWJjZGVmZw===', 'A' * 200 + 'B',
    'c3ViamVjdHM_X3h5eg', 'c3ViamVjdHM/X3h5eg==', 'eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IuS4reaWhyJ9', 'e30', 'bnVsbA', 'dHJ1ZQ=='
]
B64_RAW = [b'', b'a', b'ab', b'abc', b'abcd', b'\x00', b'\xff\xfe\xfd', b'\xfb\xff', b'\xfb\xff\xbf', bytes(range(256)), '中文'.encode('utf-8'),
           b'{"alg":"none"}'] + [bytes((i * 37 + j * 101) % 256 for j in range(i)) for i in range(1, 20)]


def b64_case(s):
    return {'in': s, 'loose': attempt_hex(lambda: base64.b64decode(s)), 'strict': attempt_hex(lambda: base64.b64decode(s, validate=True)),
            'urlsafe': attempt_hex(lambda: base64.urlsafe_b64decode(s))}


def attempt_hex(f):
    try:
        return {'ok': f().hex()}
    except ValueError as e:
        return {'err': str(e)}


def attempt(f):
    try:
        return {'ok': f()}
    except ValueError as e:
        return {'err': str(e)}


def url_case(u):
    def run():
        p = urlsplit(u)
        return {'scheme': p.scheme, 'netloc': p.netloc, 'path': p.path, 'query': p.query, 'fragment': p.fragment,
                'username': p.username, 'password': p.password, 'hostname': p.hostname, 'port': attempt(lambda: p.port)}
    return {'in': u, 'out': attempt(run)}


def ip_case(s):
    def run():
        ip = ipaddress.ip_address(s)
        return {'version': ip.version, 'str': str(ip), 'private': ip.is_private, 'loopback': ip.is_loopback, 'global': ip.is_global,
                'net48': str(ipaddress.ip_network(str(ip).split('%')[0] + '/48', strict=False)) if ip.version == 6 else None,
                'net24': str(ipaddress.ip_network(str(ip) + '/24', strict=False)) if ip.version == 4 else None}
    return {'in': s, 'out': attempt(run)}


# --- plistlib：写出（XML、二进制）和读入 ---
EPOCH = datetime.datetime(2001, 1, 1)


def p_tag(v):
    """Python 的值 → 带类型标记的写法（Rust 的测试照它搭出同一个值）"""
    if v is None:
        return {'t': 'null'}
    if v is True or v is False:
        return {'t': 'bool', 'v': v}
    if isinstance(v, int):
        return {'t': 'int', 'v': str(v)}
    if isinstance(v, float):
        return {'t': 'real', 'v': struct.pack('>d', v).hex()}
    if isinstance(v, str):
        return {'t': 'str', 'v': v}
    if isinstance(v, (bytes, bytearray)):
        if len(v) > 4096 and bytes(v) == data_seq(len(v)):   # 大块的数据不整个写进向量文件：两边照同一个规律现造
            return {'t': 'dataseq', 'v': str(len(v))}
        return {'t': 'data', 'v': bytes(v).hex()}
    if isinstance(v, datetime.datetime):
        return {'t': 'date', 'v': str((v - EPOCH) // datetime.timedelta(microseconds=1))}
    if isinstance(v, plistlib.UID):
        return {'t': 'uid', 'v': str(v.data)}
    if isinstance(v, list):
        return {'t': 'array', 'v': [p_tag(x) for x in v]}
    if isinstance(v, dict):
        return {'t': 'dict', 'v': [[k, p_tag(x)] for k, x in v.items()]}
    raise TypeError(v)


def data_seq(n):
    return bytes((i * 7) % 256 for i in range(n))


def p_err(e):
    return {'err': type(e).__name__, 'msg': str(e)}


def nest(depth, leaf):
    return leaf if depth == 0 else [nest(depth - 1, leaf)]


def p_dumps():
    """(名字, 值)：每个值都现搭，里面的数组 / 字典互不共用（plistlib 按对象的 id 合并，Rust 那边是一棵树）"""
    dt = datetime.datetime
    return [
        ('true', True), ('false', False), ('none', None), ('int-0', 0), ('int-1', 1), ('int-255', 255), ('int-256', 256), ('int-65535', 65535),
        ('int-65536', 65536), ('int-2^32-1', 2 ** 32 - 1), ('int-2^32', 2 ** 32), ('int-2^63-1', 2 ** 63 - 1), ('int-2^63', 2 ** 63),
        ('int-2^64-1', 2 ** 64 - 1), ('int-2^64', 2 ** 64), ('int--1', -1), ('int--2^63', -2 ** 63), ('int--2^63-1', -2 ** 63 - 1),
        ('real-0', 0.0), ('real--0', -0.0), ('real-1.5', 1.5), ('real-1e100', 1e100), ('real-1e16', 1e16), ('real-inf', float('inf')), ('real-pi', 3.14),
        ('real-tiny', 5e-324), ('str-ascii', 'ascii'), ('str-empty', ''), ('str-cjk', '中文 mixed'), ('str-emoji', 'emoji 😀'), ('str-escape', 'a&b<c>d "q" \'s\''),
        ('str-newlines', 'line1\r\nline2\rline3\nline4'), ('str-tab', 'tab\tok'), ('str-control', 'ctrl\x01'), ('str-del', 'del\x7f ok'), ('str-long', 'x' * 300),
        ('str-latin1', 'café'), ('data-empty', b''), ('data-short', b'short'), ('data-256', bytes(range(256))), ('data-57', b'\x00' * 57), ('data-58', b'\xff' * 58),
        ('data-big', data_seq(70000)), ('date-epoch', dt(2001, 1, 1)), ('date-leap', dt(2024, 2, 29, 12, 34, 56)),
        ('date-micros', dt(2024, 2, 29, 12, 34, 56, 789000)), ('date-min', dt(1, 1, 1)), ('date-max', dt(9999, 12, 31, 23, 59, 59, 999999)),
        ('date-1970', dt(1970, 1, 1)), ('date-far-future', dt(2400, 1, 1, 1, 2, 3, 7)), ('date-far-past', dt(1700, 5, 6, 7, 8, 9, 123457)),
        ('date-odd', dt(8765, 4, 3, 2, 1, 0, 999999)), ('uid-5', plistlib.UID(5)), ('uid-70000', plistlib.UID(70000)), ('uid-big', plistlib.UID(2 ** 40)),
        ('uid-max', plistlib.UID(2 ** 64 - 1)), ('array-empty', []), ('dict-empty', {}), ('nested-empty', [[], {}, [[]], {'a': {}, 'b': []}]),
        ('dict-order', {'b': 1, 'a': 2, 'B': 3, '中': 4, 'é': 5, '': 6, 'a b': 7, 'a&b': 8, 'Z': 9, '😀': 10, '�': 11}),
        ('dedup', [1, 1, True, True, 1.0, 1.0, 'a', 'a', b'a', b'a', 0.0, -0.0, 0, False, 'key', {'key': 'key', 'a': 1}, None, None, dt(2024, 1, 1), dt(2024, 1, 1)]),
        ('data-depths', [nest(d, bytes(range(100))) for d in range(10)]),
        ('array-20', list(range(20))), ('dict-16', {'k%02d' % i: i for i in range(16)}), ('array-300', ['s%d' % i for i in range(300)]),
        ('dict-key-control', {'bad\x02key': 1}), ('dict-with-none', {'a': None}), ('int-in-list-overflow', [1, 2 ** 64]),
        ('info-plist', {'CFBundleExecutable': 'cca-launcher', 'CFBundleIdentifier': 'com.example.cca.launcher.alice-example-com-b47f4d',
                        'CFBundleName': 'Claude 主号', 'CFBundleIconFile': 'AppIcon', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1',
                        'LSUIElement': True, 'NSHighResolutionCapable': True, 'LSMinimumSystemVersion': '11.0'}),
        ('launchd-plist', {'Label': 'com.example.cca.udp', 'ProgramArguments': ['/usr/local/libexec/cca-udp/cca-udp'], 'RunAtLoad': True, 'KeepAlive': True,
                           'StandardErrorPath': '/var/log/cca-udp.log', 'ThrottleInterval': 5}),
        ('dock-tile', {'persistent-apps': [{'GUID': 123456789, 'tile-data': {'book': b'book\x00\x01\x02' * 9, 'bundle-identifier': 'com.example.app',
                                                                            'file-data': {'_CFURLString': 'file:///Applications/Example.app/', '_CFURLStringType': 15},
                                                                            'file-label': '例子', 'file-mod-date': 3700000000, 'file-type': 41},
                                            'tile-type': 'file-tile'}], 'mod-count': 42, 'tilesize': 48.0, 'autohide': False}),
    ]


def p_dump_case(name, value):
    out = {'name': name, 'value': p_tag(value)}
    for kind, fmt in (('xml', plistlib.FMT_XML), ('binary', plistlib.FMT_BINARY)):
        try:
            raw = plistlib.dumps(value, fmt=fmt)
        except Exception as e:
            out[kind] = p_err(e)
            continue
        try:   # 写出去的再读回来（不一定读得回来：最大的日期写成秒数时进了位）
            back = {'ok': p_tag(plistlib.loads(raw))}
        except Exception as e:
            back = p_err(e)
        # 大的输出只记摘要
        out[kind] = {'ok': raw.hex(), 'back': back} if len(raw) <= 1024 else {'sha256': hashlib.sha256(raw).hexdigest(), 'len': len(raw), 'back': back}
    return out


XML_HEAD = ('<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
            '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n')


def x(body, head=XML_HEAD):
    return (head + '<plist version="1.0">\n' + body + '\n</plist>\n').encode('utf-8')


def bplist(objects, top=0, ref_size=1, offset_size=1, num=None, table_at=None, offsets=None):
    """手工拼一个二进制属性列表：objects 是每个对象的字节；offsets 给了就用它当偏移表（指到别处去）"""
    out, offs = b'bplist00', []
    for o in objects:
        offs.append(len(out))
        out += o
    at = len(out)
    out += b''.join(o.to_bytes(offset_size, 'big') for o in (offs if offsets is None else offsets)) if offset_size else b''
    return out + struct.pack('>6xBBQQQ', offset_size, ref_size, len(objects) if num is None else num, top, at if table_at is None else table_at)


def p_loads():
    d = lambda f: b'\x33' + struct.pack('>d', f)
    cases = [
        ('xml-empty-dict', x('<dict/>')), ('xml-string', x('<string>x</string>')), ('xml-no-plist-wrapper', b'<?xml version="1.0"?><dict><key>a</key><true/></dict>'),
        ('xml-no-declaration', b'<plist version="1.0"><array><integer>1</integer><false/></array></plist>'),
        ('xml-entities', x('<string>a &amp; b &lt;c&gt; &quot;q&quot; &apos;s&apos; &#65;&#x42;&#x4e2d;&#128512;</string>')),
        ('xml-cdata', x('<string><![CDATA[<raw> & stuff]]></string>')), ('xml-cdata-mixed', x('<string>a<![CDATA[ & ]]>b &amp; c</string>')),
        ('xml-comments', x('<dict><!-- c --><key>a</key><!-- d\n e --><string>b</string><?pi data?></dict>')),
        ('xml-crlf', x('<dict>\r\n\t<key>a</key>\r\n\t<string>line1\r\nline2\rline3</string>\r\n</dict>').replace(b'?>\n', b'?>\r\n')),
        ('xml-whitespace-string', x('<string>  spaced\n\tout  </string>')), ('xml-empty-string', x('<string></string>')), ('xml-self-closed-string', x('<string/>')),
        ('xml-nested', x('<dict><key>a</key><array><dict><key>b</key><array/></dict><string>s</string></array><key>c</key><dict/></dict>')),
        ('xml-duplicate-keys', x('<dict><key>a</key><integer>1</integer><key>b</key><integer>2</integer><key>a</key><integer>3</integer></dict>')),
        ('xml-empty-key', x('<dict><key></key><true/></dict>')), ('xml-empty-key-pending', x('<array><dict><key></key></dict><string>x</string></array>')),
        ('xml-empty-key-pending-root', x('<dict><key></key></dict>')), ('xml-empty-key-twice', x('<dict><key></key><key>b</key><true/></dict>')),
        ('xml-multiple-roots', b'<plist><string>a</string><string>b</string></plist>'), ('xml-no-root-object', b'<plist></plist>'), ('xml-plist-self-closed', b'<plist/>'),
        ('xml-unknown-wrapper', b'<plist><foo><string>x</string></foo></plist>'), ('xml-unknown-inside-string', x('<string>a<b>c</b>d</string>')),
        ('xml-text-then-child', x('<dict><key>a</key>junk<string>b</string>more</dict>')), ('xml-attributes', x('<dict a="1" b=\'2\'><key x="&lt;">a</key><true y=""/></dict>')),
        ('xml-missing-value', x('<dict>\n<key>a</key>\n</dict>')), ('xml-unexpected-element', x('<dict>\n\n<string>x</string>\n</dict>')),
        ('xml-unexpected-key', x('<dict>\n<key>a</key>\n<key>b</key>\n<true/></dict>')), ('xml-key-in-array', x('<array><key>a</key></array>')),
        ('xml-key-at-root', b'<plist><key>a</key></plist>'), ('xml-value-after-key-in-array', x('<array><dict><key>a</key><array><key>b</key></array></dict></array>')),
        ('xml-container-in-dict-without-key', x('<dict>\n<array/>\n</dict>')), ('xml-scalar-after-root-dict-key', x('<dict><key>a</key><dict><true/></dict></dict>')),
        ('xml-mismatched', x('<dict><key>a</key></string></dict>')), ('xml-unclosed', b'<plist><dict><key>a</key>'), ('xml-junk-after', b'<plist/><plist/>'),
        ('xml-text-after', b'<plist/>trailing'), ('xml-space-after', b'<plist/>\n\n  '), ('xml-comment-after', b'<plist/><!-- bye -->'),
        ('xml-undefined-entity', x('<string>a&nbsp;b</string>')), ('xml-undefined-entity-no-doctype', x('<string>&nbsp;</string>', '<?xml version="1.0"?>\n')),
        ('xml-undefined-entity-standalone', x('<string>&nbsp;</string>', XML_HEAD.replace('encoding="UTF-8"', 'encoding="UTF-8" standalone="yes"'))),
        ('xml-undefined-entity-standalone-no', x('<string>&nbsp;</string>', XML_HEAD.replace('encoding="UTF-8"', "encoding='UTF-8' standalone='no'"))),
        ('xml-undefined-entity-internal-only', x('<string>&nbsp;</string>', '<!DOCTYPE plist [<!ELEMENT plist ANY>]>\n')),
        ('xml-undefined-entity-in-attribute', x('<dict a="&nbsp;"/>')), ('xml-bad-char-ref', x('<string>&#0;</string>')), ('xml-raw-control', x('<string>a\x01b</string>')),
        ('xml-bare-ampersand', x('<string>a & b</string>')), ('xml-entity-declaration', x('<string>&x;</string>', XML_HEAD.replace('.dtd">', '.dtd" [<!ENTITY x "y">]>'))),
        ('xml-internal-subset', x('<string>ok</string>', XML_HEAD.replace('.dtd">', '.dtd" [<!ELEMENT plist ANY> <!-- c -->]>'))),
        ('xml-doctype-system', x('<string>ok</string>', '<?xml version="1.0"?>\n<!DOCTYPE plist SYSTEM "file://localhost/System/Library/DTDs/PropertyList.dtd">\n')),
        ('xml-duplicate-attribute', x('<dict a="1" a="2"/>')), ('xml-unquoted-attribute', x('<dict a=1/>')), ('xml-lt-in-attribute', x('<dict a="<"/>')),
        ('xml-declaration-late', b'\n<?xml version="1.0"?><plist/>'), ('xml-declaration-inside', b'<plist><?xml version="1.0"?></plist>'),
        ('xml-bom', b'\xef\xbb\xbf' + x('<string>bom</string>')), ('xml-utf16-be', b'\xfe\xff' + '<?xml version="1.0" encoding="UTF-16"?><plist><string>中</string></plist>'.encode('utf-16-be')),
        ('xml-utf16-le-no-declaration', b'\xff\xfe' + '<plist><string>中😀</string></plist>'.encode('utf-16-le')),
        ('xml-latin1', b'<?xml version="1.0" encoding="ISO-8859-1"?><plist><string>caf\xe9</string></plist>'),
        ('xml-bad-utf8', x('<string>X</string>').replace(b'X', b'\xff\xfe')), ('xml-empty-file', b''), ('xml-not-a-plist', b'hello world'),
        ('xml-only-declaration', b'<?xml version="1.0"?>'), ('xml-comment-double-dash', x('<!-- a -- b --><true/>')),
        ('xml-cdata-end-in-text', x('<string>a ]]> b</string>')), ('xml-plist-prefix-only', b'<plistx/>'), ('xml-close-with-space', b'<plist ><true /></plist >'),
    ]
    for raw in [' 42 ', '0x1F', '0XfF', '-5', '+7', '1_000', '0x', 'abc', '', '-0x10', '１２', '0x_1f', '18446744073709551615', '99999999999999999999999', '1 2',
                '0x1G', '4.0', '\n7\n', '0b11', '007', '-', '_1', '1__0', '0x 1f']:
        cases.append(('xml-integer %r' % raw, x('<integer>%s</integer>' % raw)))
    for raw in ['1.5', ' -2e3 ', 'inf', '-Infinity', 'nan', 'abc', '1_0.5', '', '.5', '5.', '1e400', '0x10', '１.５']:
        cases.append(('xml-real %r' % raw, x('<real>%s</real>' % raw)))
    for raw in ['aGVsbG8=', '\n\taGVs\n\tbG8=\n\t', '', 'aGVsbG8', 'a', '!!!!', 'aGVsbG8=trailing', '====']:
        cases.append(('xml-data %r' % raw, x('<data>%s</data>' % raw)))
    for raw in ['2024-02-29T12:34:56Z', '2024-02-29T12:34Z', '2024-02-29T12Z', '2024-02-29Z', '2024-02Z', '2024Z', '2024-02-29T12:34:56', 'garbage', '',
                '2024-13-01T00:00:00Z', '2023-02-29T00:00:00Z', '2024-02-29T24:00:00Z', '2024-02-29T23:60:00Z', '2024-02-29T23:59:60Z', '0000-01-01T00:00:00Z',
                '2024-00-10Z', '2024-01-00Z', '2024-04-31Z', '2024-02-29T12:34:56Zjunk', ' 2024-02-29T12:34:56Z', '2024-02-29T12:34:5Z', '2024-02-29T12:34:56.789Z',
                '2024-02-29 12:34:56Z', '9999-12-31T23:59:59Z', '0001-01-01T00:00:00Z', '1900-02-29Z', '2000-02-29Z', '２０２４-02-29Z', '2024-2-29Z']:
        cases.append(('xml-date %r' % raw, x('<date>%s</date>' % raw)))
    u16 = lambda s: s.encode('utf-16-be')
    cases += [
        ('bin-null', bplist([b'\x00'])), ('bin-false', bplist([b'\x08'])), ('bin-true', bplist([b'\x09'])), ('bin-fill', bplist([b'\x0f'])),
        ('bin-float32', bplist([b'\x22' + struct.pack('>f', 1.5)])), ('bin-float32-third', bplist([b'\x22' + struct.pack('>f', 1 / 3)])),
        ('bin-float64', bplist([b'\x23' + struct.pack('>d', -2.5e-300)])), 
        # 偏移指到文件的最后一个字节（偏移表位置的低字节，凑成 0x23 / 0x5f）：标记后面读不到东西
        ('bin-float64-short', bplist([b'\x5f\x10\x18' + b'a' * 24], offsets=[67])), ('bin-size-marker-missing', bplist([b'\x4f\x10\x54' + b'd' * 84], offsets=[127])),
        ('bin-int-1', bplist([b'\x10\xff'])), ('bin-int-2', bplist([b'\x11\xff\xfe'])), ('bin-int-4', bplist([b'\x12\xff\xff\xff\xff'])),
        ('bin-int-8-negative', bplist([b'\x13' + b'\xff' * 8])), ('bin-int-8-max', bplist([b'\x13\x7f' + b'\xff' * 7])), ('bin-int-8-min', bplist([b'\x13\x80' + b'\x00' * 7])),
        ('bin-int-16-unsigned64', bplist([b'\x14' + b'\x00' * 8 + b'\xff' * 8])), ('bin-int-16-negative', bplist([b'\x14\x80' + b'\x00' * 15])),
        ('bin-int-16-minus-one', bplist([b'\x14' + b'\xff' * 16])), ('bin-int-16-huge', bplist([b'\x14\x7f' + b'\xff' * 15])),
        ('bin-ascii', bplist([b'\x55hello'])), ('bin-ascii-long', bplist([b'\x5f\x10\x14' + b'a' * 20])), ('bin-ascii-size-2', bplist([b'\x5f\x11\x01\x2c' + b'b' * 300], offset_size=2)),
        ('bin-ascii-not-ascii', bplist([b'\x52h\xe9'])), ('bin-ascii-short', bplist([b'\x5f\x10\xff' + b'a' * 20])),
        ('bin-utf16', bplist([b'\x62' + u16('中文')])), ('bin-utf16-pair', bplist([b'\x62' + u16('😀')])), ('bin-utf16-lone', bplist([b'\x61\xd8\x3d'])),
        ('bin-utf16-short', bplist([b'\x63' + u16('ab')])), ('bin-data', bplist([b'\x43\x00\x01\x02'])), ('bin-data-short', bplist([b'\x4f\x10\x40' + b'x' * 10])),
        ('bin-uid-1', bplist([b'\x80\x05'])), ('bin-uid-4', bplist([b'\x83\x00\x01\x11\x70'])), ('bin-uid-8', bplist([b'\x87' + b'\xff' * 8])),
        ('bin-uid-16', bplist([b'\x8f' + b'\x01' + b'\x00' * 15])), ('bin-unknown-token', bplist([b'\xc1\x00'])), ('bin-unknown-token-0', bplist([b'\x05'])),
        ('bin-array', bplist([b'\xa3\x01\x02\x01', b'\x10\x07', b'\x51x'])), ('bin-dict', bplist([b'\xd2\x01\x02\x03\x03', b'\x51a', b'\x51b', b'\x09'])),
        ('bin-dict-duplicate-keys', bplist([b'\xd2\x01\x01\x02\x03', b'\x51a', b'\x10\x01', b'\x10\x02'])),
        ('bin-shared-container', bplist([b'\xa2\x01\x01', b'\xa1\x02', b'\x51x'])), ('bin-dict-array-key', bplist([b'\xd1\x01\x02', b'\xa0', b'\x09'])),
        ('bin-top-not-first', bplist([b'\x51x', b'\xa1\x00'], top=1)), ('bin-bad-top', bplist([b'\x09'], top=5)), ('bin-bad-ref', bplist([b'\xa1\x07'])),
        ('bin-bad-offset', bplist([b'\x09'], offsets=[200])),
        ('bin-ref-size-0', bplist([b'\xa0'], ref_size=0)), ('bin-ref-size-0-used', bplist([b'\xa1\x00'], ref_size=0)), ('bin-offset-size-0', bplist([b'\x09'], offset_size=0)),
        ('bin-offset-size-3', bplist([b'\xa1\x01', b'\x09'], offset_size=3)), ('bin-ref-size-3', bplist([b'\xa1\x00\x00\x01', b'\x09'], ref_size=3)),
        ('bin-ref-size-2', bplist([b'\xa2\x00\x01\x00\x01', b'\x51z'], ref_size=2)), ('bin-no-objects', bplist([], num=0)),
        ('bin-too-many-objects', bplist([b'\x09'], num=2 ** 60, offset_size=8)), ('bin-header-only', b'bplist00'), ('bin-31-bytes', b'bplist00' + b'\x00' * 23),
        ('bin-32-zero-bytes', b'bplist00' + b'\x00' * 24), ('bin-wrong-version', b'bplist01' + bplist([b'\x09'])[8:]),
        ('bin-array-long', bplist([b'\xaf\x10\x10' + b'\x01' * 16, b'\x09'])), ('bin-size-8-bytes', bplist([b'\x5f\x13' + (3).to_bytes(8, 'big') + b'abc'])),
        ('bin-size-huge', bplist([b'\x5f\x13' + b'\xff' * 8 + b'abc'])),
        ('bin-deep', bplist([b'\xa1' + bytes([i + 1]) for i in range(100)] + [b'\x09'])),
    ]
    for f in [0.0, 1.5, -1.25, 0.0000005, 0.0000015, 0.0000025, -0.0000005, -0.0000015, 1e18, -1e18, float('nan'), float('inf'), 725846400.1234567, 725846400.9999995,
              -63113904000.0, -63113904000.000001, 252455615999.999999, 252455616000.0, 1e-7, 0.1, 0.7, 2 ** 53 + 0.5, 12345.6789015, 99.9999995, -99.9999995,
              0.999999499999, 0.9999995, 86399.9999995, 1e12, -1e10, 3.5e-6, 2.5e-6, 4.5e-6]:
        cases.append(('bin-date %r' % f, bplist([d(f)])))
    out = []
    for name, raw in cases:
        try:
            got = {'ok': p_tag(plistlib.loads(raw))}
        except Exception as e:
            got = p_err(e)
        out.append({'name': name, 'hex': raw.hex(), 'out': got})
    return out


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    files = {
        'urlsplit.json': [url_case(u) for u in URLS],
        'ipaddress.json': [ip_case(s) for s in IPS],
        'urlquery.json': {
            'parse_qsl': [{'in': q, 'out': [list(x) for x in parse_qsl(q)], 'keep': [list(x) for x in parse_qsl(q, keep_blank_values=True)]} for q in QUERIES],
            'quote': [{'in': s, 'quote': quote(s), 'quote_safe_empty': quote(s, safe=''), 'unquote': unquote(s)} for s in QUOTES],
            'urljoin': [{'base': b, 'url': u, 'out': urljoin(b, u)} for b, u in JOINS],
        },
        'base64.json': {
            'decode': [b64_case(s) for s in B64],
            'encode': [{'hex': r.hex(), 'b64': base64.b64encode(r).decode('ascii'), 'urlsafe': base64.urlsafe_b64encode(r).decode('ascii')} for r in B64_RAW],
        },
        'plist.json': {'dumps': [p_dump_case(n, v) for n, v in p_dumps()], 'loads': p_loads()},
    }
    for name, data in files.items():
        with open(os.path.join(out, name), 'w', encoding='utf-8') as f:
            json.dump({'python': '%d.%d' % sys.version_info[:2], 'cases': data}, f, ensure_ascii=False, indent=1)
            f.write('\n')


main()
