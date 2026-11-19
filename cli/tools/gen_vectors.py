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


# ---- TOML：tomllib.loads 认什么、不认什么、给出什么值（Rust 的移植在 src/py/toml.rs）----------------------------------
import datetime
import random
import tomllib

# 让 tomllib.loads 自己落在调用栈的这一层（最外层的模块是第 1 层）：行内数组、行内表套得太深时在哪一层出 RecursionError
# 跟着它走。cca 里 py settings-check 读共享的 config.toml 时它就在第 8 层
TOML_FRAME = 8


def toml_nested(opening, closing, n, inner=''):
    return 'a = ' + opening * n + inner + closing * n


TOML_DOCS = [
    # 空的、空白、注释、换行
    '', ' ', '\t', '\n', '\n\n\n', ' \t \n', '# 注释', '#', '# c\n', 'a = 1 # 行尾的注释', 'a = 1 #', 'a = 1#c', '# 制表符\t可以', '#\x00', '# x\x7f',
    '# x\x1f', '# x\x0b', '# a\r\n# b\r\n', 'a = 1\r\nb = 2\r\n', 'a = 1\rb = 2', '\r', 'a = 1\r', '# c\r', '# c\r\n', '\ufeffa = 1', '\x0c', '\x00',
    'a = 1\x00', '\u00a0a = 1', 'a = 1\u00a0', 'a = 1\u2028b = 2', 'a = 1\n\n\nb = 2\n', '\n\na=1', 'a=1\n', '  a  =  1  ', '\ta\t=\t1\t',
    # 一句话的样子不对
    'a = 1 b = 2', 'a = 1, b = 2', 'a', 'a =', 'a = ', '= 1', ' = 1', 'a == 1', 'a = = 1', 'a : 1', 'a = 1 ;', '!', 'é = 1', '中 = 1', '$ = 1',
    '"a" = 1 "b" = 2', 'a = # c', 'a = \n1', 'a\n= 1', 'a = 1 = 2', '=', '.', ',', ']', '}', '{', '{}', 'a b', '😀 = 1', '"😀" = 1',
    # 键：裸的、带引号的、带点的、重复的
    'key = "value"', 'bare_key = 1', 'bare-key = 1', '1234 = 1', '- = 1', '_ = 1', '-_-_ = 1', 'A-Z_a-z_0-9 = 1', '"192.0.2.1" = 1',
    '"character encoding" = 1', '"ʎǝʞ" = 1', "'key2' = 1", "'quoted \"value\"' = 1", '"" = 1', "'' = 1", '"\\u0041" = 1', '"a\\tb" = 1',
    '"a\\nb" = 1', '"\\"" = 1', "'a\\b' = 1", '"a\nb" = 1', "'a\nb' = 1", '"""a""" = 1', "'''a''' = 1", '"a\\x" = 1', '"a\\ud800" = 1', '"a = 1',
    "'a = 1", '"a"b = 1', 'a"b" = 1', "a'b' = 1", '"a" . "b" = 1', '1979-05-27 = 1', '07:32:00 = 1', '1.5 = 1', '1e5 = 1', '+1 = 1', 'true = 1',
    'inf = 1', 'nan = false', '0x10 = 1', 'a.true = 1', 'true.false = 1',
    'physical.color = "orange"', 'physical.shape = "round"\nphysical.color = "red"', 'site."example.com" = true', 'a . b . c = 1', 'a\t.\tb = 1',
    '3.14159 = "pi"', 'a."b".\'c\' = 1', 'a. = 1', '.a = 1', 'a..b = 1', 'a.b. = 1', 'a.\nb = 1', '"a"."b" = 1', 'a.b = 1\na.c = 2', 'a.b = 1\na.b = 2',
    'a.b = 1\na.b.c = 2', 'a.b.c = 1\na.b = 2', 'a = 1\na.b = 2', 'a.b = 1\na = 2', 'a = "x"\na.b = 2', 'a = [1]\na.b = 2', 'a.b = 1\n"a".c = 2',
    'apple.type = "fruit"\norange.type = "fruit"\napple.skin = "thin"\norange.skin = "thick"', 'fruit.apple.smooth = true\nfruit.orange = 2',
    'fruit.apple = 1\nfruit.apple.smooth = true', 'name = 1\nname = 2', 'spelling = 1\n"spelling" = 2', "a = 1\n'a' = 2", '"a b" = 1\n"a b" = 2',
    'A = 1\na = 2', '"\\u0061" = 1\na = 2', 'a.b.c.d.e.f = 1', 'a.b.c = 1\na.b.d = 2\na.e = 3\nf = 4', '"it\'s".\'say "hi"\' = 1\n"it\'s".\'say "hi"\' = 2',
    # 基本字符串和转义
    'a = "I\'m a string. \\"You can quote me\\". Name\\tJos\\u00E9\\nLocation\\tSF."', 'a = "\\b\\t\\n\\f\\r\\"\\\\"',
    'a = "\\u00e9\\u00E9\\U0001F600\\U0001f600"', 'a = "\\u0000"', 'a = "\\u007f"', 'a = "\\ud7ff\\ue000"', 'a = "\\U0010FFFF"', 'a = "\\ud800"',
    'a = "\\udfff"', 'a = "\\U00110000"', 'a = "\\UFFFFFFFF"', 'a = "\\u12"', 'a = "\\u12g4"', 'a = "\\U0001F60"', 'a = "\\u"', 'a = "\\U"',
    'a = "\\u123', 'a = "\\x41"', 'a = "\\e"', 'a = "\\/"', 'a = "\\ "', 'a = "\\a"', 'a = "\\0"', 'a = "\\\'"', 'a = "\\', 'a = "\\"', 'a = "abc\\',
    'a = "\\é"', 'a = "\\u00é9"', 'a = "\\u４１４１"', 'a = "\\😀"', 'a = "tab\there"', 'a = "line\nbreak"', 'a = "nul\x00"', 'a = "del\x7f"',
    'a = "us\x1f"', 'a = "cr\rhere"', 'a = "crlf\r\nhere"', 'a = "中文 é 😀"', 'a = "', 'a = "abc', 'a = ""', 'a = "" "b"', 'a = "a" "b"', 'a = "a"b',
    'a = "#not a comment" # comment', 'a = "\u2028\u2029\u0085\u00a0"', 'a = "\\\\\\\\"', 'a = "\\\\\\"', 'a = "é\x01"', 'a = "中\\q"',
    # 多行的基本字符串
    'a = """\nRoses are red\nViolets are blue"""', 'a = """Roses"""', 'a = """"""', 'a = """\n"""', 'a = """\n\n"""', 'a = """\r\nline\r\n"""',
    'a = """The quick brown \\\n\n\n  fox jumps over \\\n    the lazy dog."""', 'a = """\\\n       The quick brown \\\n       fox.\\\n       """',
    'a = """a \\   \n   b"""', 'a = """a \\ \t \n\n b"""', 'a = """a \\  b"""', 'a = """a \\', 'a = """a \\  ', 'a = """a \\\n', 'a = """a\\\n"""',
    'a = """a \\\t\n b"""', 'a = """a \\\r\n b"""', 'a = """a\\ \r\n b"""',
    'a = """Here are two quotation marks: "". Simple enough."""', 'a = """Here are three quotation marks: ""\\"."""',
    'a = """Here are fifteen quotation marks: ""\\"""\\"""\\"""\\"""\\"."""', 'a = """"This," she said, "is just a pointless statement.""""',
    'a = """"a""""', 'a = """a"""""', 'a = """a""""""', 'a = """""a"""', 'a = """""""', 'a = """"""""', 'a = """\\u00e9\\t\\\\"""', 'a = """\\x"""',
    'a = """ctrl\x01"""', 'a = """tab\tnl\n"""', 'a = """cr\rx"""', 'a = """del\x7f"""', 'a = """abc', 'a = """abc"', 'a = """abc""', 'a = """\n\n\nabc"""',
    'a = """a"""b', 'a = """a""" # c', 'a = """中文\n😀"""', 'a = """\\ud800"""', 'a = """\\', 'a = """\\"""',
    # 字面字符串
    "a = 'C:\\Users\\nodejs\\templates'", "a = '\\\\ServerX\\admin$\\system32\\'", "a = 'Tom \"Dubs\" Preston-Werner'", "a = '<\\i\\c*\\s*>'", "a = ''",
    "a = 'tab\there'", "a = 'nl\nhere'", "a = 'nul\x00'", "a = 'del\x7f'", "a = 'abc", "a = '", "a = 'a' 'b'", "a = 'é中😀'", "a = 'abc\nb = 'x'",
    "a = 'cr\rx'", "a = 'é\x01'", "a = 'x'y", "a = 'x' # c",
    "a = '''\nThe first newline is\ntrimmed in raw strings.\n   All other whitespace\n   is preserved.\n'''", "a = '''I [dw]on't need \\d{2} apples'''",
    "a = ''''That,' she said, 'is still pointless.''''", "a = '''Here are fifteen quotation marks: \"\"\"\"\"\"\"\"\"\"\"\"\"\"\"'''", "a = '''a'''''",
    "a = '''a''''''", "a = ''''''", "a = '''''''", "a = ''''''''", "a = '''\n'''", "a = '''ctrl\x01'''", "a = '''tab\tok'''", "a = '''cr\rx'''",
    "a = '''\r\nx\r\n'''", "a = '''abc", "a = '''abc''", "a = '''\\\n  x'''", "a = '''中\x00'''", "a = '''a'''b",
    # 整数
    'a = +99', 'a = 42', 'a = 0', 'a = -17', 'a = +0', 'a = -0', 'a = 1_000', 'a = 5_349_221', 'a = 53_49_221', 'a = 1_2_3_4_5', 'a = 0xDEADBEEF',
    'a = 0xdeadbeef', 'a = 0xdead_beef', 'a = 0xDead_Beef', 'a = 0o01234567', 'a = 0o755', 'a = 0b11010110', 'a = 0b0', 'a = 0x0', 'a = 0o0',
    'a = 0x00FF', 'a = 0b0000_0001', 'a = 01', 'a = 00', 'a = +01', 'a = -01', 'a = 1__0', 'a = _1', 'a = 1_', 'a = 1_ ', 'a = 0x', 'a = 0x_1',
    'a = 0x1_', 'a = 0x1__2', 'a = 0X1', 'a = 0O1', 'a = 0B1', 'a = +0x1', 'a = -0x1', 'a = 0xG', 'a = 0xfg', 'a = 0b2', 'a = 0b12', 'a = 0o8',
    'a = 0o78', 'a = 0_0', 'a = +', 'a = -', 'a = --1', 'a = ++1', 'a = +-1', 'a = 1 000', 'a = 1,000', 'a = 1a', 'a = 0b', 'a = 0o', 'a = ٣',
    'a = １２', 'a = 9223372036854775807', 'a = 9223372036854775808', 'a = -9223372036854775808', 'a = -9223372036854775809',
    'a = +9223372036854775808', 'a = 0x7FFFFFFFFFFFFFFF', 'a = 0x8000000000000000', 'a = 0xFFFFFFFFFFFFFFFF', 'a = 0o777777777777777777777',
    'a = 0o1000000000000000000000', 'a = 0o1777777777777777777777', 'a = 0b' + '1' * 63, 'a = 0b' + '1' * 64, 'a = 0b' + '1' * 65,
    'a = 123456789012345678901234567890', 'a = -123_456_789_012_345_678_901_234_567_890', 'a = 0x' + 'F' * 40,
    'a = 0x000' + 'abcdef0123456789' * 4, 'a = 0o' + '1234567' * 20, 'a = 0b' + '10' * 100,
    # 十进制最多 4300 位（int() 的上限，下划线、正负号不算）；0x / 0o / 0b 不限，十进制写不下 4300 位的记成 hex()
    'a = ' + '9' * 4300, 'a = ' + '9' * 4301, 'a = -' + '1' * 4301, 'a = ' + '_'.join(['12'] * 2150), 'a = ' + '_'.join(['12'] * 2151),
    'a = 0x' + 'f' * 3571, 'a = 0x' + 'f' * 3572, 'a = 0o1' + '0' * 4761, 'a = 0o1' + '7' * 4762, 'a = 0b1' + '0' * 14284, 'a = 0b1' + '0' * 14285,
    # 小数
    'a = +1.0', 'a = 3.1415', 'a = -0.01', 'a = 5e+22', 'a = 1e06', 'a = -2E-2', 'a = 6.626e-34', 'a = 224_617.445_991_228', 'a = 0.0', 'a = +0.0',
    'a = -0.0', 'a = 0e0', 'a = -0e0', 'a = 1e1_0', 'a = 1_0e1_0', 'a = 0.1e1', 'a = 1E5', 'a = 1e+5', 'a = 1e-5', 'a = 1.0e0', 'a = 9007199254740993.0',
    'a = 0.30000000000000004', 'a = 1e400', 'a = -1e400', 'a = 1e-400', 'a = 4.9e-324', 'a = 2.5e-324', 'a = 1.7976931348623157e308',
    'a = 1.7976931348623159e308', 'a = 123456789012345678901234567890.0', 'a = 0.1234567890123456789012345678901234567890',
    'a = ' + '9' * 5000 + '.0', 'a = 1.' + '0' * 400 + '1', 'a = 1e16', 'a = 1e-5', 'a = 0.0001', 'a = 123456789.125', 'a = 1e22', 'a = 1e23',
    'a = inf', 'a = +inf', 'a = -inf', 'a = nan', 'a = +nan', 'a = -nan', 'a = Inf', 'a = NaN', 'a = INF', 'a = infinity', 'a = nanx', 'a = +in',
    'a = in', 'a = na', 'a = 1.2.3', 'a = 1e2.3', 'a = 1_e2', 'a = 1e_2', 'a = 1._2', 'a = 1_.2', 'a = 1.2_', 'a = 1.', 'a = .1', 'a = 1.e1', 'a = 1e',
    'a = 1e+', 'a = 1E', 'a = +.5', 'a = 0x1.8p1', 'a = 1e1e1', 'a = 01.5', 'a = 1.5f', 'a = 1,5', 'a = -inf_', 'a = +nan1', 'a = 0.', 'a = 0e',
    # 布尔值
    'a = true', 'a = false', 'a = True', 'a = FALSE', 'a = truex', 'a = falsey', 'a = tru', 'a = fals', 'a = t', 'a = f', 'a = true false',
    'a = yes', 'a = null', 'a = none',
    # 日期和时间
    'a = 1979-05-27T07:32:00Z', 'a = 1979-05-27T00:32:00-07:00', 'a = 1979-05-27T00:32:00.999999-07:00', 'a = 1979-05-27 07:32:00Z',
    'a = 1979-05-27t07:32:00z', 'a = 1979-05-27T07:32:00', 'a = 1979-05-27T00:32:00.999999', 'a = 1979-05-27', 'a = 07:32:00', 'a = 00:32:00.999999',
    'a = 00:00:00', 'a = 23:59:59.999999', 'a = 07:32:00.5', 'a = 07:32:00.123456789', 'a = 07:32:00.000001', 'a = 07:32:00.0000001',
    'a = 1979-05-27T07:32:00.1234567890123Z', 'a = 1979-05-27T07:32:00+00:00', 'a = 1979-05-27T07:32:00-00:00', 'a = 1979-05-27T07:32:00+23:59',
    'a = 1979-05-27T07:32:00-23:59', 'a = 1979-05-27T07:32:00+05:30', 'a = 1979-05-27T07:32:00-00:30', 'a = 0001-01-01',
    'a = 9999-12-31T23:59:59.999999+23:59', 'a = 2024-02-29', 'a = 2000-02-29', 'a = 1600-02-29T00:00:00Z', 'a = 0000-01-01',
    'a = 0000-01-01T00:00:00Z', 'a = 1979-02-30', 'a = 2023-02-29', 'a = 1900-02-29', 'a = 2100-02-29', 'a = 1979-04-31', 'a = 1979-06-31T00:00:00',
    'a = 1979-09-31', 'a = 1979-11-31', 'a = 1979-12-31', 'a = 1979-01-31', 'a = 1979-13-01', 'a = 1979-00-10', 'a = 1979-05-32', 'a = 1979-05-00',
    'a = 1979-05-27T24:00:00', 'a = 1979-05-27T07:60:00', 'a = 1979-05-27T07:32:60', 'a = 1979-05-27T07:32', 'a = 1979-05-27T07', 'a = 1979-05-27T',
    'a = 1979-05-27 ', 'a = 1979-05-27 # c', 'a = 1979-05-27 x', 'a = 1979-05-27T07:32:00+24:00', 'a = 1979-05-27T07:32:00+07:60',
    'a = 1979-05-27T07:32:00+07', 'a = 1979-05-27T07:32:00+0700', 'a = 1979-05-27T07:32:00 +07:00', 'a = 1979-05-27T07:32:00.',
    'a = 1979-05-27T07:32:00.Z', 'a = 1979-05-27T07:32:00ZZ', 'a = 07:32', 'a = 7:32:00', 'a = 24:00:00', 'a = 07:32:00Z', 'a = 07:32:00+01:00',
    'a = 07:32:00.', 'a = 1979-5-27', 'a = 79-05-27', 'a = 19790527', 'a = 1979-05-27T07:32:00z ', 'a = 12345-01-01', 'a = 1979-05-271',
    'a = 1979-05-27-01', 'a = [1979-05-27, 07:32:00, 1979-05-27T07:32:00Z]', 'a = {d = 1979-05-27}', 'a = 1979-05-27\n[a.b]', 'a = 1979-05-27\na.b = 1',
    'a = 1979-05-27  07:32:00', 'a = 1979-05-27\t07:32:00', 'a = 1979-05-27_07:32:00', 'a = １979-05-27',
    # 数组
    'a = [1, 2, 3]', 'a = [ "red", "yellow", "green" ]', 'a = [ [ 1, 2 ], [3, 4, 5] ]', 'a = [ [ 1, 2 ], ["a", "b", "c"] ]',
    'a = [ "all", \'strings\', """are the same""", \'\'\'type\'\'\' ]', 'a = [ 0.1, 0.2, 0.5, 1, 2, 5 ]',
    'a = [ "Foo Bar <foo@example.com>", { name = "Baz Qux", email = "bazqux@example.com", url = "https://example.com/bazqux" } ]', 'a = []',
    'a = [ ]', 'a = [\n]', 'a = [\n1, 2, 3\n]', 'a = [\n1,\n2, # this is ok\n]', 'a = [1,]', 'a = [1, ]', 'a = [,]', 'a = [,1]', 'a = [1,,2]',
    'a = [1 2]', 'a = [1', 'a = [1,', 'a = [', 'a = ]', 'a = [1]]', 'a = [[]]', 'a = [[[[]]]]', 'a = [[], [[]], [[], []]]',
    'a = [ # c\n 1 # c\n , # c\n 2 # c\n ] # c', 'a = [\n# only a comment\n]', 'a = [1, # c\x00\n]', 'a = [ {x = 1}, {x = 2} ]', 'a = [true, false]',
    'a = [1.5, inf, nan]', 'a = ["a"\n,\n"b"\n]', 'a = [1\n\n\n]', 'a = [1;2]', 'a = [a]', 'a = [1=2]', 'a = [\t1\t,\t2\t]', 'a = [1,2,3,]',
    'a = [1, 2] 3', 'a = [1, 2]]', 'a = [1, [2, [3, [4, {b = [5, {c = 6}]}]]]]', 'a = [\r\n1,\r\n2\r\n]', 'a = [1, # 中文\n 2]', 'a = ["é", \'中\', 😀]',
    # 表
    '[table]', '[table]\nkey = 1', '[a.b.c]', '[ d.e.f ]', '[ g .  h  . i ]', '[ j . "ʞ" . \'l\' ]', '[a]\n[b]', '[a]\nx = 1\n[b]\nx = 2', '[x.y.z.w]\n[x]',
    '[a.b]\n[a]', '[a]\n[a.b]', '[a]\n[a]', '[a]\nb = 1\n[a]', '[a]\n[b]\n[a]', '[a]\nb = 1\n[a.b]', '[a]\nb = 1\n[a.b.c]', '[a.b]\nc = 1\n[a]\nb = 2',
    '[a]\nb.c = 1\n[a.b]', '[a]\nb.c = 1\n[a.b.d]', '[a]\nb.c = 1\n[a.b.c]', '[]', '[ ]', '[a.]', '[.a]', '[a..b]', '[a', '[a]]', '[a] b = 1', '[a] # c',
    '[a]#c', '[a]x', '  [a]  ', '\t[a]\t', '[a\n]', '[\na]', '["a"]', "['a']", '[""]', '["a.b"]', '[a."b.c".d]', '[a b]', '[a=b]', '[a]\n[a."b"]\n[a.b]',
    '[é]', '["é"]', '[1]', '[-]', '[a]\n\n\n[b]\n\n', 'a = 1\n[a]', 'a.b = 1\n[a]', 'a.b = 1\n[a.c]', 'a.b.c = 1\n[a.b.d]', 'a.b.c = 1\n[a.b]',
    '[a]\nb.c = 1\nb.d = 2', '[a]\nb.c = 1\n[x]\n[a.b]', '[a]\nb.c = 1\n[x]\nb.c = 1', '[a]\nb.c = 1\n[x]\n[a]',
    '[fruit]\napple.color = "red"\napple.taste.sweet = true\n[fruit.apple]', '[fruit]\napple.color = "red"\napple.taste.sweet = true\n[fruit.apple.taste]',
    '[fruit]\napple.color = "red"\napple.taste.sweet = true\n[fruit.apple.texture]\nsmooth = true', '[a]\nb = {}\n[a.b]', '[a]\nb = {}\n[a.b.c]',
    '[a]\nb = []\n[a.b]', '[a]\nb = []\n[[a.b]]', '[a]\nb = [{}]\n[a.b.c]', 'a = {}\na.b = 1', 'a = {b = 1}\na.c = 2', 'a = []\na.b = 1',
    'a = [{}]\na.b = 1', '[t]\na = {}\n[t.a]', 'a = {b = {}}\n[a.b.c]', '[a.b.c]\nx = 1\n[a]\ny = 2\n[a.b]\nz = 3', '[a.b.c]\n[a.b.c]',
    '[a.b.c]\n[a.b]\n[a.b]', '[a]\n"b" = 1\n[a]\nc = 2', '[a]\n[a.b]\n[a.b.c]\n[a.b]', '[a."b c"]\nd = 1\n[a."b c".e]', '[a]\n\tb = 1\n  [c]\n\td = 2',
    '[a]\nb = 1 [c]', '[a][b]', '[a]\n[b', '[a.b]\n[a.b.c]\n[a]\nb.d = 1',
    # 出错的说法里带着键（Python 的 repr：引号、转义、不能打印的字符）
    '["it\'s"]\n["it\'s"]', '[\'say "hi"\']\n[\'say "hi"\']', '["a\\nb"]\n["a\\nb"]', '["é"."中"]\n["é"."中"]', '["\\u0001"]\n["\\u0001"]',
    '["😀"]\n["😀"]', '["both \' and \\""]\n["both \' and \\""]', '["\\u00a0\\u200b\\u2028"]\n["\\u00a0\\u200b\\u2028"]',
    '["\\U000E0001"]\n["\\U000E0001"]', '["\\u007f"]\n["\\u007f"]', '["a\\\\b"]\n["a\\\\b"]', '[""]\n[""]', '["".""]\n["".""]',
    '[a."b\\tc"]\nd = 1\n[a]\n"b\\tc".e = 2', '"é" = {}\n["é".x]', '[["it\'s"]]\n"k\\r" = []\n[["it\'s"."k\\r"]]', 'a."\\u0085" = []\na."\\u0085".b = 1',
    # 行内表
    'a = { first = "Alice", last = "Example" }', 'a = { x = 1, y = 2 }', 'a = { type.name = "pug" }', 'a = {}', 'a = { }', 'a = {\t}',
    'a = {a = {b = {c = {}}}}', 'a = { x = 1, }', 'a = {,}', 'a = { , x = 1}', 'a = { x = 1,, y = 2 }', 'a = { x = 1 y = 2 }', 'a = {\n x = 1 }',
    'a = { x = 1\n}', 'a = { x = 1,\n y = 2 }', 'a = { x = [\n1,\n2\n] }', 'a = { x = """\nmultiline""" }', 'a = { x = 1', 'a = {', 'a = { x',
    'a = { x = }', 'a = { = 1 }', 'a = {x=1}}', 'a = }', 'a = { x = 1, x = 2 }', 'a = { x = 1, "x" = 2 }', 'a = { a.b = 1, a.c = 2 }',
    'a = { a.b = 1, a.b = 2 }', 'a = { a = 1, a.b = 2 }', 'a = { a.b = 1, a = 2 }', 'a = { a = {b = 1}, a.c = 2 }', 'a = { a = [1], a.b = 2 }',
    'a = { a = {}, a.b = 2 }', 'a = { a.b = {c = 1}, a.b.d = 2 }', 'a = { a.b.c = 1, a.b.d = 2, a.e = 3 }', 'a = { a = [{}], a.b = 1 }',
    'a = { "a b" = 1, \'c d\' = 2 }', 'a = { x = 1 } # c', 'a = { x = 1 # c\n}', 'a = { # c\n}', 'point = { x = 1, y = 2 }\nanimal = { type.name = "pug" }',
    'name = { first = "Alice" }\nname.last = "x"', '[product]\ntype = { name = "Nail" }\ntype.edible = false',
    '[product]\ntype.name = "Nail"\ntype = { edible = false }', 'a = { b = 1 }\n[a]', 'a = { b = 1 }\n[a.c]', 'a = { b = 1 }\n[[a]]',
    'a = { b = 1 }\n[[a.c]]', 'a = {x = {y = [ {z = 1} ]}}', 'a = { "it\'s" = 1, "it\'s" = 2 }', 'a = { \'say "hi"\' = 1, \'say "hi"\' = 2 }',
    'a = { "中\\n" = 1, "中\\n" = 2 }', 'a = { "\\u0001" = 1, "\\u0001" = 2 }', 'a = { "😀" = 1, "😀" = 2 }', 'a = { b.c = 1, b.c.d = 2 }',
    'a = { b = [], b.c = 1 }', 'a = { b.c = [], b.c.d = 1 }', 'a = { b.c = 1, b = 2 }', 'a = { x = { y = 1 }, x = 2 }', 'a = {x = 1}{y = 2}',
    # 表的数组
    '[[products]]\nname = "Hammer"\nsku = 738594937\n\n[[products]]  # empty table within the array\n\n[[products]]\nname = "Nail"\nsku = 284758393\n'
    'color = "gray"',
    '[[fruits]]\nname = "apple"\n[fruits.physical]\ncolor = "red"\nshape = "round"\n[[fruits.varieties]]\nname = "red delicious"\n[[fruits.varieties]]\n'
    'name = "granny smith"\n[[fruits]]\nname = "banana"\n[[fruits.varieties]]\nname = "plantain"',
    '[fruit.physical]\ncolor = "red"\n[[fruit]]\nname = "apple"', 'fruits = []\n[[fruits]]',
    '[[fruits]]\nname = "apple"\n[[fruits.varieties]]\nname = "red delicious"\n[fruits.varieties]\nname = "granny smith"',
    '[[fruits]]\nname = "apple"\n[fruits.physical]\ncolor = "red"\n[[fruits.physical]]\ncolor = "green"', '[[a]]', '[[a]]\n[[a]]', '[[a]]\n[a]',
    '[a]\n[[a]]', '[[a]]\n[a.b]', '[[a]]\n[a.b]\n[[a]]\n[a.b]', '[[a]]\n[a.b]\n[a.b]', '[[a]]\n[[a.b]]\n[a.b]', '[[a]]\n[[a.b]]\n[[a.b]]\n[[a]]\n[[a.b]]',
    '[[a.b]]\n[[a]]', '[[a.b]]\n[a]\nc = 1', '[[a.b]]\n[a]\nb = 1', '[[a]]\nb = 1\n[[a]]\nb = 2\n[a.c]\nd = 3', '[[]]', '[[ ]]', '[[a]', '[[a]]]', '[[a] ]',
    '[ [a]]', '[[a]]x', '[[a]] # c', '[[a.]]', '[[a."b"]]', '[["a.b"]]', 'a = 1\n[[a]]', 'a = [1]\n[[a]]', 'a.b = 1\n[[a]]', 'a.b = 1\n[[a.b]]',
    '[a]\nb = 1\n[[a.b]]', '[[a]]\nb.c = 1\n[[a.b]]', '[[a]]\nb.c = 1\n[a.b]', '[[a]]\nb = {}\n[a.b]', '[[a]]\nb = []\n[[a.b]]',
    '[[a]]\n[[a.b.c]]\n[a.b.c.d]\n[[a.b.c]]\n[a.b.c.d]', '[[a]]\nb.c = 1\n[[a]]\nb.c = 2', '[[a]]\n[a.b]\nc = 1\n[[a]]\n[a.b]\nc = 2\n[[a.b.d]]',
    '[[a]]\n[[a.b]]\n[[a.b.c]]\nx = 1\n[[a]]\n[[a.b]]\n[a.b.c]\nx = 2', '[[a]] [[a]]', '[[a]]\n[[a', '[["a"]]\n[[\'a\']]\n[[a]]',
    # 整份的文件
    '# This is a TOML document\n\ntitle = "TOML Example"\n\n[owner]\nname = "Alice Example"\ndob = 1979-05-27T07:32:00-08:00\n\n[database]\n'
    'enabled = true\nports = [ 8000, 8001, 8002 ]\ndata = [ ["delta", "phi"], [3.14] ]\ntemp_targets = { cpu = 79.5, case = 72.0 }\n\n[servers]\n\n'
    '[servers.alpha]\nip = "192.0.2.1"\nrole = "frontend"\n\n[servers.beta]\nip = "192.0.2.2"\nrole = "backend"\n',
    'model = "sample-model"\nmodel_reasoning_effort = "ultra"\nservice_tier = "priority"\ntext = """A note containing chatgpt_base_url = "example"\n'
    'features = { respect_system_proxy = true }\n"""\nwhen = 2026-11-08T12:00:00Z\nchoices = [1, 2, 3]\n[features]\nrespect_system_proxy = false\n'
    'system_proxy_fallback = false\n[projects."/tmp/test-project"]\ntrust_level = "trusted"\n[[sample]]\nname = "one"\n',
    'chatgpt_base_url = """\nhttps://example.invalid"""\n', "chatgpt_base_url = '''\nhttps://example.invalid'''\n",
    '"\\u0063hatgpt_base_url" = "https://example.invalid"\n', '[profiles.other]\nchatgpt_base_url = "https://example.invalid"\n',
    'profiles.other.chatgpt_base_url = "https://example.invalid"\n', 'profiles = { other = { chatgpt_base_url = "https://example.invalid" } }\n',
    'features = { respect_system_proxy = true }', 'features.respect_system_proxy = true', '["features"]\n"respect_system_proxy" = true',
    'profiles = { "deep.review" = { features = { respect_system_proxy = true } } }', '"features.respect_system_proxy" = true',
    'profile = "safe"\n[profiles.safe]\nmodel="m"\n[profiles."unused.review"]\ncli_auth_credentials_store="keyring"', 'model="unterminated',
    'model="a"\nmodel="b"', 'features = "not a table"', 'profiles = []', 'sqlite_home = [', 'features={',
    '[package]\nname = "x"\nversion = "0.1.0"\n\n[dependencies]\nlibc = "0.2"\nregex = { version = "1", default-features = false, features = ["std"] }\n\n'
    '[profile.release]\nopt-level = 2\nlto = "fat"\n\n[[bin]]\nname = "x"\npath = "src/main.rs"\n',
    # 行内数组、行内表套到上限前后（loads 在第 8 层：数组到 494 层、行内表到 329 层还行）
    toml_nested('[', ']', 100), toml_nested('[', ']', 494), toml_nested('[', ']', 495), toml_nested('[', ']', 600), toml_nested('[', ']', 5000),
    toml_nested('{a = ', '}', 100, '1'), toml_nested('{a = ', '}', 329, '1'), toml_nested('{a = ', '}', 330, '1'), toml_nested('{a = ', '}', 400, '1'),
    toml_nested('[{a = ', '}]', 197, '1'), toml_nested('[{a = ', '}]', 198, '1'), toml_nested('[{a = ', '}]', 199, '1'), toml_nested('[', ']', 493, '"x"'),
    toml_nested('[', ']', 494, '"x"'), toml_nested('[', ']', 492, '"\\u00e9"'), toml_nested('[', ']', 493, '"\\u00e9"'), toml_nested('{"a" = ', '}', 328, '1'),
    toml_nested('{"a" = ', '}', 329, '1'), 'a = ' + '[' * 10, 'a = ' + '[' * 494, 'a = ' + '[' * 495, 'a = ' + '[' * 496, 'a = ' + '[' * 2000,
    'a = ' + '{a = ' * 329, 'a = ' + '{a = ' * 330, 'a = ' + '[' * 494 + '?', 'a = ' + '[' * 495 + '?', '.'.join(['a'] * 500) + ' = 1',
    '[' + '.'.join(['a'] * 300) + ']\nb = 1', toml_nested('[', ']', 494) + '\nb = ' + '[' * 495 + ']' * 495,
]

# 拿来随机改动的几份（改动出来的大多不合法：出错的地方、说法也要一样）
TOML_MUTATED = [
    'title = "TOML"\n[owner]\nname = "Alice"\ndob = 1979-05-27T07:32:00-08:00\n',
    'a.b = 1\n[c]\nd = { e = [1, 2.5, "x"], f.g = true }\n[[h]]\ni = 0x1F\n[[h]]\ni = 07:32:00\n',
    'k = """\nmulti \\\n   line"""\nl = \'literal\'\nm = \'\'\'raw\n\'\'\'\n"quoted key" = "\\u00e9\\t"\n',
    '[a]\nb.c = 1\n[a.d]\ne = [ [1, 2], [{f = 1}] ] # c\n[[a.g]]\nh = -0.5e+3\n',
    'x = [ 1, # 一\n  2, # 二\n]\ny = { "中" = "文", z = 1979-05-27 }\n',
    'n = +1_000\no = 0o17\np = 0b11\nq = inf\nr = -nan\ns = 1e-2\nt = 0.5\n',
    '[[t]]\n[t.u]\nv = 1\n[[t]]\n[t.u]\nv = 2\n[w]\nx.y.z = "é😀"\n',
]


def toml_mutants():
    rng = random.Random(20261119)
    pieces = ['"', "'", '"""', "'''", '\\', '\n', '\r\n', '\t', ' ', '[', ']', '[[', ']]', '{', '}', '=', '.', ',', '#', '0', '1', '_', '-', '+', ':',
              'T', 'Z', 'e', 'x', 'inf', 'nan', 'true', '\\u00e9', '\\U0001F600', 'é', '中', '😀', '\x00', '\x7f', 'a', 'a.b', ' = ', '1979-05-27', '07:32:00']
    out = []
    for doc in TOML_MUTATED:
        for _ in range(64):
            i, j, op, piece = rng.randrange(len(doc) + 1), rng.randrange(len(doc) + 1), rng.randrange(5), pieces[rng.randrange(len(pieces))]
            if op == 0:
                out.append(doc[:i] + doc[i + 1:])           # 删一个字符
            elif op == 1:
                out.append(doc[:i] + piece + doc[i:])       # 插一段
            elif op == 2:
                out.append(doc[:i] + piece + doc[i + 1:])   # 换一个字符
            elif op == 3:
                out.append(doc[:max(i, j)] + doc[min(i, j):])   # 重复一段
            else:
                out.append(doc[:i])                         # 截断
    return out


# 结果刚好卡在调用栈上限上的：(文件, 结果)。生成时找出 loads 最深在第几层还是这个结果，再深一层就是 RecursionError
TOML_FRAMES = [
    ('', 'ok'), ('# c', 'ok'), ('a = 1', 'ok'), ('a = true', 'ok'), ('a = "x"', 'ok'), ('a = "x\\u00e9"', 'ok'), ('"k" = 1', 'ok'),
    ('"k\\u0041" = 1', 'ok'), ("a = 'x'", 'ok'), ("'k' = 1", 'ok'), ('a = """x"""', 'ok'), ('a = """x\\u00e9"""', 'ok'), ('a = """x\\\n  y"""', 'ok'),
    ("a = '''x'''", 'ok'), ('a = []', 'ok'), ('a = [1]', 'ok'), ('a = [[[]]]', 'ok'), ('a = [ # c\n ]', 'ok'), ('a = {}', 'ok'), ('a = {b = 1}', 'ok'),
    ('a = {b = {c = {d = 1}}}', 'ok'), ('a = {"b" = 1}', 'ok'), ('a = 1979-05-27', 'ok'), ('a = 1979-05-27T07:32:00-07:13', 'ok'), ('a = 07:32:00', 'ok'),
    ('a = 1.5', 'ok'), ('a = inf', 'ok'), ('[a]', 'ok'), ('[[a]]', 'ok'), ('["a\\u0041"]', 'ok'), ('a.b.c = 1', 'ok'), ('a = [[["x"]]]', 'ok'),
    ('a = [[[{}]]]', 'ok'), ('a = [[[{a = 1, b = 2}]]]', 'ok'), ('a = [[[1979-05-27T07:32:00+03:21]]]', 'ok'), ('[[a]]\n[[a]]\n[a.b]\nc = 1', 'ok'),
    ("a = 'x\x01'", 'TOMLDecodeError'), ("a = 'x", 'TOMLDecodeError'), ('a = "x\x01"', 'TOMLDecodeError'), ('a = "\\u12"', 'TOMLDecodeError'),
    ('a = "\\ud800"', 'TOMLDecodeError'), ('a = ?', 'TOMLDecodeError'), ('?', 'TOMLDecodeError'), ('a = 1\na = 2', 'TOMLDecodeError'),
    ('a = [[[1, 2', 'TOMLDecodeError'), ('a = {b = {c = 1, c = 2}}', 'TOMLDecodeError'), ('a = 1979-02-30', 'TOMLDecodeError'),
]

TOML_INT_LIMIT = 10 ** 4300   # 十进制超过 4300 位的整数 Python 转不成十进制的文字（str()、json.dumps 都是 ValueError）


def toml_frame_index():
    """调用者那一层在调用栈的第几层（最外层的模块是第 1 层）"""
    f, n = sys._getframe(1), 0
    while f:
        n += 1
        f = f.f_back
    return n


def toml_loads_at(doc, frame):
    """让 tomllib.loads 自己落在调用栈的第 frame 层"""
    here = toml_frame_index()
    if here >= frame:
        raise AssertionError('调用栈已经有 %d 层，到不了第 %d 层' % (here, frame))
    if here == frame - 1:
        return tomllib.loads(doc)
    return toml_loads_at(doc, frame)


def toml_result(doc, frame):
    """(结果的种类, 解析出来的值或者出错的说法)"""
    try:
        return 'ok', toml_loads_at(doc, frame)
    except tomllib.TOMLDecodeError as e:
        return 'TOMLDecodeError', str(e)
    except RecursionError:
        return 'RecursionError', ''
    except ValueError as e:
        return 'ValueError', str(e)


def toml_plain(v):
    """日期、时间和大到转不成十进制的整数换成带记号的字符串，别的原样"""
    if isinstance(v, dict):
        return {k: toml_plain(x) for k, x in v.items()}
    if isinstance(v, list):
        return [toml_plain(x) for x in v]
    if isinstance(v, datetime.datetime):
        return '@datetime ' + v.isoformat()
    if isinstance(v, datetime.date):
        return '@date ' + v.isoformat()
    if isinstance(v, datetime.time):
        return '@time ' + v.isoformat()
    if isinstance(v, int) and not isinstance(v, bool) and abs(v) >= TOML_INT_LIMIT:
        return '@int ' + hex(v)
    return v


def toml_text(value):
    """解析出来的值写成 JSON 的文字：Rust 那边照 json.dumps(…, ensure_ascii=False) 的写法逐字比（键的先后、整数还是小数都在里面）"""
    limit = sys.getrecursionlimit()
    sys.setrecursionlimit(100000)   # 套了几百层的值：只在换写法时放宽，解析时用的还是默认的上限
    try:
        return json.dumps(toml_plain(value), ensure_ascii=False)
    finally:
        sys.setrecursionlimit(limit)


def toml_vectors():
    docs = []
    for doc in dict.fromkeys(TOML_DOCS + toml_mutants()):
        kind, value = toml_result(doc, TOML_FRAME)
        if kind == 'ok':
            docs.append({'in': doc, 'ok': toml_text(value)})
        elif kind == 'RecursionError':
            docs.append({'in': doc, 'err': kind})
        else:
            docs.append({'in': doc, 'err': kind, 'msg': value})
    frames = []
    for doc, want in TOML_FRAMES:
        lo, hi = TOML_FRAME, sys.getrecursionlimit() + 1   # 第 lo 层是这个结果，第 hi 层不是
        if toml_result(doc, lo)[0] != want:
            raise AssertionError('%r 在第 %d 层不是 %s' % (doc, lo, want))
        while hi - lo > 1:
            mid = (lo + hi) // 2
            if toml_result(doc, mid)[0] == want:
                lo = mid
            else:
                hi = mid
        if toml_result(doc, hi)[0] != 'RecursionError':
            raise AssertionError('%r 在第 %d 层不是 RecursionError' % (doc, hi))
        frames.append({'in': doc, 'want': 'ok' if want == 'ok' else 'ValueError', 'max': lo})
    return {'frame': TOML_FRAME, 'docs': docs, 'frames': frames}


def zlib_vectors():
    """zlib.decompressobj().decompress(data, max_length)：解出来的（摘要）、有没有没解完的输入（unconsumed_tail）；坏了是 error"""
    import zlib
    plain = [b'', b'a', b'hello hello hello hello', bytes(range(256)) * 3, b'{"v":1,"at":1795000000.5,"h":{"1794024000":[1,2,3,4,5,6,7]},"um":[]}' * 40,
             b''.join(hashlib.sha256(b'%d' % i).digest() for i in range(40))]
    cases = []

    def add(name, data, limit):
        z = zlib.decompressobj()
        try:
            raw = z.decompress(data, limit)
            cases.append({'name': name, 'hex': data.hex(), 'limit': limit, 'len': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(), 'tail': bool(z.unconsumed_tail)})
        except zlib.error:
            cases.append({'name': name, 'hex': data.hex(), 'limit': limit, 'error': True})
    for i, p in enumerate(plain):
        for level in (0, 1, 6, 9):
            c = zlib.compress(p, level)
            add('whole-%d-l%d' % (i, level), c, 4 << 20)
            if level == 9:
                add('exact-limit-%d' % i, c, max(len(p), 1))
                add('one-short-%d' % i, c, max(len(p) - 1, 1))
                add('small-limit-%d' % i, c, 16)
                add('garbage-after-%d' % i, c + b'trailing junk', 4 << 20)
                add('no-trailer-%d' % i, c[:-4], 4 << 20)
                add('half-trailer-%d' % i, c[:-2], 4 << 20)
                add('cut-in-half-%d' % i, c[:len(c) // 2], 4 << 20)
                add('first-byte-only-%d' % i, c[:1], 4 << 20)
                add('bad-checksum-%d' % i, c[:-1] + bytes([c[-1] ^ 0x55]), 4 << 20)
                add('bad-header-%d' % i, b'\x00\x00' + c[2:], 4 << 20)
                add('flipped-middle-%d' % i, c[:len(c) // 2] + bytes([c[len(c) // 2] ^ 0xff]) + c[len(c) // 2 + 1:], 4 << 20)
    add('empty-input', b'', 4 << 20)
    add('not-zlib', b'plain text, not compressed at all', 4 << 20)
    add('gzip-not-zlib', bytes.fromhex('1f8b0800000000000003cb48cdc9c90700866a6c3605000000'), 4 << 20)
    add('raw-deflate', zlib.compress(b'hello', 9)[2:-4], 4 << 20)
    add('preset-dict-flag', bytes([0x78, 0xbb]) + zlib.compress(b'hello', 9)[2:], 4 << 20)
    return cases


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    files = {
        'toml.json': toml_vectors(),
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
        'zlib.json': zlib_vectors(),
    }
    for name, data in files.items():
        with open(os.path.join(out, name), 'w', encoding='utf-8') as f:
            json.dump({'python': '%d.%d' % sys.version_info[:2], 'cases': data}, f, ensure_ascii=False, indent=1)
            f.write('\n')


main()
