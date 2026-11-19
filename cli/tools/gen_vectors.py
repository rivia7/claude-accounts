#!/usr/bin/env python3
"""生成 cli/tests/vectors/*.json：Python 标准库对一批输入给出的结果，Rust 的移植照着核对（cargo test 读这些文件，
不用装 Python）。

    python3 -I cli/tools/gen_vectors.py cli/tests/vectors

原来的实现用的是用户机器上那个 Python 的标准库，各版本的行为有出入；Rust 这边固定成生成这些文件的那个版本。
换 Python 版本重新生成后，差异会在 git diff 和 cargo test 里看得到。"""
import ipaddress
import json
import os
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
    }
    for name, data in files.items():
        with open(os.path.join(out, name), 'w', encoding='utf-8') as f:
            json.dump({'python': '%d.%d' % sys.version_info[:2], 'cases': data}, f, ensure_ascii=False, indent=1)
            f.write('\n')


main()
