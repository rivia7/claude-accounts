#!/usr/bin/env python3
"""生成 cli/tests/vectors/corenet.json：cca 里内嵌的 Python（PY_SRC）的 node_parse、node_clean_name 对一批输入给出的结果，
cli/src/cmd/corenet/parse.rs 的单元测试照着核对（cargo test 读这个文件，不用装 Python）。

    python3 -I cli/tools/gen_corenet_vectors.py cli/tests/vectors

和 gen_vectors.py 不一样：那边记的是 Python 标准库的行为，这边记的是 PY_SRC 自己的函数。主干改了 node_parse（认新的链接、
换了报错的说法）以后要重新生成；对拍（pydiff/cases/corenet.py 的 node-check，拿现在的 PY_SRC 实跑）会先发现两边不一样。

输入：手写的一批（每条路、每句报错都有：分享链接的各种写法、JSON、一行写完的、块写法的 YAML），加上拿它们随机改几个字符
变出来的（种子固定，每次生成的一样；变出像别的域名的不要，结果里有落单的代理项、写不成 UTF-8 的不收）。密钥都是假的
（test-password、全 0 的 uuid），服务器是 *.example.com、192.0.2.x。
PY_SRC 只当库载入（不跑子命令），HOME / CCA_HOME 指向临时目录。"""
import base64
import atexit
import json
import os
import random
import re
import shutil
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def load():
    """PY_SRC 里的定义（不跑最后的子命令分派）"""
    sandbox = tempfile.mkdtemp(prefix='cca-vectors.')
    atexit.register(shutil.rmtree, sandbox, True)   # 临时的主目录：用完删掉
    os.environ.update(HOME=sandbox, CCA_HOME=os.path.join(sandbox, '.cca'), CCA_TOOL='claude')
    with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
        src = f.read().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]
    ns = {}
    exec(compile(src.split('\nif len(sys.argv) < 2', 1)[0], 'cca', 'exec'), ns)
    return ns


def b64(obj, urlsafe=False, pad=True):
    raw = obj if isinstance(obj, bytes) else (obj if isinstance(obj, str) else json.dumps(obj, ensure_ascii=False)).encode('utf-8')
    s = (base64.urlsafe_b64encode if urlsafe else base64.b64encode)(raw).decode('ascii')
    return s if pad else s.rstrip('=')


ZERO = '00000000-0000-0000-0000-000000000000'
VM = {'v': '2', 'ps': 'US-A', 'add': 'a.example.com', 'port': '443', 'id': ZERO, 'aid': '0', 'scy': 'auto',
      'net': 'ws', 'type': 'none', 'host': 'h.example.com', 'path': '/ws', 'tls': 'tls', 'sni': 'sni.example.com'}


def vm(**kw):
    """一条 vmess 链接；值是 KeyError 的那一项去掉"""
    d = dict(VM)
    for k, v in kw.items():
        if v is KeyError:
            d.pop(k, None)
        else:
            d[k] = v
    return 'vmess://' + b64(d)


SS = b64('aes-256-gcm:test-password')
CORPUS = [
    ('empty', ['', ' ', '\n\n', '\t \u3000\n']),
    ('scheme', ['ssr://abc', 'SSR://abc', 'sSr://', 'ssr:/x', 'http://x', 'x', 'tuıc://u@h.example.com:1', 'tuİc://u@h.example.com:1', 'ſs://' + b64('m:p') + '@h:1', 'ſſr://x',
                'sſr://x', 'vmeſs://' + b64(VM), 'vleſs://u@h:1', 'hyſteria2://p@h:1', 'hysterİa2://p@h:1', 'trojan ://p@h:1', 'trojan:/p@h:1', 'xtrojan://p@h:1', 'ss:/x', 'ss:x',
                'Kss://x', 'ｓｓ://x', ' ss://' + SS + '@h:1 ', 'hy2:// ', 'TUIC://', 'Hysteria2://p@h:1']),
    ('vmess', [
        vm(), vm(port=443), vm(port=443.0), vm(port=None), vm(port=True), vm(port=0), vm(port=65535), vm(port=65536), vm(port='0x10'), vm(port=' 443 '), vm(port='4_43'),
        vm(port='４４３'), vm(port='-1'), vm(port=''), vm(port=[443]), vm(port={'a': 1}), vm(port='99999999999999999999999'), vm(port=10**30), vm(port='+443'), vm(port='443\n'),
        vm(port=KeyError), vm(port='1' * 4300), vm(port='1' * 4301), vm(port='0' * 4301 + '1'), vm(port='_1'), vm(port='1_'), vm(port='1__0'), vm(port='٤٤٣'), vm(port=1.5),
        vm(aid=0), vm(aid=''), vm(aid=None), vm(aid='abc'), vm(aid=1.9), vm(aid=True), vm(aid=[1]), vm(aid='1_0'), vm(aid=10**30), vm(aid='-5'), vm(aid=-2.5), vm(aid=1e30),
        vm(aid={'a': 1}), vm(aid=' 7 '), vm(aid='٣'), vm(aid=KeyError), vm(aid=False), vm(aid='0'), vm(aid='00'), vm(aid='1' * 4301), vm(aid='+3'), vm(aid='1.0'), vm(aid=-0.0),
        vm(aid=9.3e18), vm(aid=-9.3e18), vm(aid=2**63), vm(aid=-2**63 - 1),
        vm(net='grpc'), vm(net='tcp'), vm(net='none'), vm(net='h2'), vm(net=''), vm(net=5), vm(net=['ws']), vm(net=None), vm(net=KeyError), vm(net='WS'), vm(net='grpc', path=''),
        vm(net='ws', path='', host=''), vm(net='ws', path=5, host=['h']), vm(net='ws', host=KeyError), vm(net='ws', path=KeyError, host=KeyError), vm(net=True), vm(net={'a': 1}),
        vm(tls=''), vm(tls='none'), vm(tls=True), vm(tls=None), vm(tls='TLS'), vm(tls=KeyError), vm(sni=''), vm(sni=5), vm(sni=KeyError), vm(sni=['x']),
        vm(ps=''), vm(ps=None), vm(ps=5), vm(ps='CCA-1'), vm(ps='CCA-CCA- x '), vm(ps='a\tb\x7fc'), vm(ps='名字 😀'), vm(ps='x' * 200), vm(ps='DIRECT'), vm(ps=' REJECT '),
        vm(ps=['a']), vm(ps=KeyError), vm(add=''), vm(add=None), vm(add=5), vm(add=KeyError), vm(add=['a']), vm(id=None), vm(id=5), vm(scy=''), vm(scy='aes-128-gcm'), vm(scy=None),
        'vmess://', 'vmess://!!!', 'vmess://a', 'vmess://ab', 'vmess://abc', 'vmess://e30', 'vmess://e30=', 'vmess://' + b64('[1]'), 'vmess://' + b64('"x"'), 'vmess://' + b64('nul'),
        'vmess://' + b64('{"port": 1'), 'vmess://' + b64(VM, urlsafe=True, pad=False), 'vmess://' + b64(dict(VM, ps='？？？>>>'), urlsafe=True), 'VMESS://' + b64(VM),
        'vmess://' + b64(VM)[:20] + '\n' + b64(VM)[20:], 'vmess://' + b64(VM) + '中', 'vmess://' + b64(b'\xff\xfe{"port": 1}'), 'vmess://' + b64('\ufeff{"port": 1}'),
        'vmess://' + b64(VM) + '  ', '  vmess://' + b64(VM), 'vmess://' + b64(VM) + '#frag', 'vmess://' + b64(dict(VM, port=1, extra={'k': [1, None, 1.5]})),
        'vmess://' + b64('{"port": 443, "add": "a.example.com", "port": 8443}'), 'vmess://' + b64('{"port": NaN, "add": "a"}'), 'vmess://' + b64('{"port": 1e2, "add": "a"}'),
        'vmess://' + b64('{"port": 443, "add": "a", "aid": NaN}'), 'vmess://' + b64('{"port": 443, "add": "a", "aid": Infinity}'),
        'vmess://' + b64('{"port": 443, "add": "a", "aid": -Infinity}'), 'vmess://' + b64('{"port": 443, "add": "a.example.com"}'), 'vmess://' + b64('{"port": 443}'),
        'vmess://' + b64(' {"port": 443, "add": "a"} \n'), 'vmess://' + b64('null'), 'vmess://' + b64('5'), 'vmess://' + b64(VM).replace('=', ''), 'vmess://=' + b64(VM),
        'vmess://' + b64(VM) + '====', 'vmess://' + b64(VM)[:-3], 'vmess://' + b64(VM)[:-2] + '*', 'vmeSS://' + b64(VM).replace('+', '-').replace('/', '_'),
    ]),
    ('ss', [
        'ss://' + SS + '@192.0.2.1:8388#US-A', 'ss://' + b64('aes-256-gcm:test-password', pad=False) + '@s.example.com:8388',
        'ss://' + b64('aes-256-gcm:test-password@s.example.com:8388') + '#name', 'ss://' + b64('aes-256-gcm:test-password@s.example.com:8388', urlsafe=True, pad=False),
        'ss://aes-256-gcm:test-password@s.example.com:8388#plain', 'ss://aes-256-gcm:test%2Dpassword@s.example.com:8388/?plugin=obfs#x%20y',
        'ss://' + b64('aes-256-gcm:pa:ss:word') + '@[2001:db8::1]:8388', 'ss://' + b64('nocolon') + '@s.example.com:8388', 'ss://%6eocolon@s.example.com:8388',
        'ss://' + b64('m:p') + '@s.example.com', 'ss://' + b64('m:p') + '@s.example.com:', 'ss://' + b64('m:p') + '@:8388', 'ss://' + b64('m:p') + '@s.example.com:abc',
        'ss://' + b64('m:p') + '@s.example.com:0', 'ss://' + b64('m:p') + '@s.example.com:65536', 'ss://' + b64('m:p') + '@a@b:1', 'ss://@s.example.com:1', 'ss://:@s.example.com:1',
        'ss://a', 'ss://ab', 'ss://abc', 'ss://', 'ss://#x', 'ss://?x', 'ss://' + b64('abc@host:8388'), 'ss://' + b64('nohost'), 'ss://' + b64('a:b'), 'ss://' + b64('a@b'),
        'ss://' + b64(':p@h:1'), 'ss://' + b64('m:@h:1'), 'ss://' + b64('m:p@h:1') + '?x=1#%E5%90%8D', 'SS://' + b64('m:p') + '@H.Example.com:1#N', 'ss://中文@h:1', 'ss://中:文@h:1',
        'ss://' + b64('m:p') + '@h:1#a#b', 'ss://' + b64('m:p') + '@h:1#', 'ss://' + b64('m:p') + '@h:1#%ZZ%41', 'ss://' + b64(b'\xff:\xfe') + '@h:1', 'ss://m%3Ap@h:1', 'ss://%@h:1',
        'ss://' + b64('m:p') + '@h:1#CCA-x', 'ss://' + b64('m:p') + '@h: 8388', 'ss://' + b64('m:p') + '@h:8388 ', 'ss://' + b64('m:p@h:8_3'), 'ss://' + b64('m:p@h:٨٣'),
        'ss://' + b64('m:p@h@i:1'), 'ss://' + b64('m:p@[::1]:1'), 'ss://' + b64('m:p@h:1').replace('=', '') + '#x', 'ss://' + b64('@h:1'), 'ss://' + b64(':@:1'), 'ss://*@h:1',
        'ss://' + b64('a') + '@h:1', 'ss://a:b:c@h:1', 'ss://a@b@c:1', 'ss://' + b64('m:p') + '@h:1?#', 'ss://' + b64('m:p') + '?x@h:1', 'ss://' + b64('m:p') + '#x@h:1',
    ]),
    ('vless', [
        'vless://' + ZERO + '@v.example.com:443?type=ws&path=%2Fws%3Fed%3D2048&host=h.example.com&security=tls&sni=sni.example.com&fp=chrome#US-B',
        'vless://uuid@v.example.com:443?security=reality&pbk=test-public-key&sid=abcd&flow=xtls-rprx-vision&type=tcp#r', 'vless://uuid@v.example.com:443?security=reality#r2',
        'vless://uuid@v.example.com:443?type=grpc&serviceName=svc&security=none', 'vless://uuid@v.example.com:443?type=grpc', 'vless://uuid@v.example.com:443?net=ws&path=%252F',
        'vless://uuid@v.example.com:443?type=none&net=ws', 'vless://uuid@v.example.com:443?type=original', 'vless://uuid@v.example.com:443?type=&net=ws&host=h',
        'vless://uuid@v.example.com:443?type=ws&type=grpc&serviceName=s', 'vless://uuid@v.example.com:443?sni=a&sni=b&flow=&fp', 'vless://uuid@v.example.com:443?security=TLS',
        'vless://uuid@v.example.com', 'vless://uuid@v.example.com:', 'vless://uuid@v.example.com:0', 'vless://uuid@v.example.com:65536', 'vless://uuid@v.example.com:abc',
        'vless://uuid@v.example.com:99999', 'vless://uuid@v.example.com:-1', 'vless://uuid@v.example.com:٤٤٣', 'vless://uuid@[2001:db8::1]:443', 'vless://uuid@[::1:443',
        'vless://uuid@::1]:443', 'vless://uuid@[v1.x]:443', 'vless://uuid@[1.2.3.4]:443', 'vless://@v.example.com:443', 'vless://v.example.com:443', 'vless://:443', 'vless://',
        'vless:///path', 'vless://u:p@h:1', 'vless://u%40x:p%3Aq@H.Example.COM:1?a=1#%E5%90%8D%E5%AD%97', 'vless://u@h:1#CCA-CCA-n', 'vless://u@h:1#a\tb', 'vless://u@h:1#' + 'n' * 300,
        'vless://u@h:1# sp ', 'vless://u@h:1#DIRECT', 'vless://u@h:1?x=1#f?y=2', 'vless://u@h:1/path?type=ws', 'vless://u@h:1?type=ws&path=/a+b&host=x+y', 'VLESS://U@H:1?TYPE=ws',
        'vless://u@h\t:1', 'vless://u@h:1\n?type=ws', 'vless://u@\u2100.example.com:1', 'vless://u@例子.example.com:1', 'vless://u@h:1?type=ws&path=%E4%B8%AD&host=%FF',
        'vless://u@h:1?security=reality&pbk=&sid=1', 'vless://u@h:1?security=reality&pbk=k', 'vless://u@h:1?type=ws&path=%', 'vless://u@h:1?type=ws;host=x', 'vless://u@h:1?=x&type',
        'vless://u@[fe80::1%25en0]:1', 'vless://u@h:1?fp=%E4%B8%AD%E6%96%87&flow=a%26b', 'vless://a@b@h:1', 'vless://u@h:01', 'vless://u@h: 1', 'vless://u@h:1 ', 'vless://u@H:1#%',
        'vless://\x01u@h:1', 'vless://u@h:1?type=ws&host=a&host=b&path=/1&path=/2',
    ]),
    ('trojan', [
        'trojan://test-password@t.example.com:443?sni=s.example.com&allowInsecure=1#HK-A', 'trojan://:test-password@t.example.com:443',
        'trojan://u:test-password@t.example.com:443?allowInsecure=true', 'trojan://test%2Dpassword@t.example.com:443?allowInsecure=0&type=ws&path=%2F', 'trojan://t.example.com:443',
        'trojan://@t.example.com:443?allowInsecure=TRUE', 'TROJAN://P@H:443?type=grpc&serviceName=x', 'trojan://p@h:443?sni=', 'trojan://p@h', 'trojan://:@h:1', 'trojan://%E5%AF%86@h:1',
        'trojan://p@h:443?allowInsecure=1&allowInsecure=0', 'trojan://p@h:443?type=ws&host=h2&sni=s', 'trojan://:@h:1?type=tcp',
    ]),
    ('hysteria2', [
        'hysteria2://test-password@y.example.com:8443?sni=s.example.com&obfs=salamander&obfs-password=test-obfs&insecure=1#HY', 'hy2://test-password@y.example.com:8443?insecure=true',
        'hy2://:pw@y.example.com:8443?insecure=0', 'hysteria2://y.example.com:8443', 'HY2://P@Y:1?obfs=&obfs-password=x', 'hysteria2://p@y:1?mport=1-2', 'hy2://u:p@y:1',
        'hysteria2://p@y:1?type=ws&insecure=TRUE',
    ]),
    ('tuic', [
        'tuic://' + ZERO + ':test-password@u.example.com:443?sni=s.example.com&congestion_control=bbr&alpn=h3,h3-29#TU', 'tuic://token@u.example.com:443', 'tuic://u.example.com:443',
        'tuic://:@u.example.com:443?alpn=', 'tuic://a:@u.example.com:443?alpn=,', 'tuic://a:b:c@u.example.com:443?alpn=h3', 'TUIC://A:B@U:1?ALPN=h3&alpn=h2', 'tuic://%41:%42@u:1?alpn=a,,b,',
    ]),
    ('json', [
        '{"name": "US-A", "type": "ss", "server": "s.example.com", "port": 8388, "cipher": "aes-256-gcm", "password": "test-password"}',
        '[{"name": "US-A", "type": "ss", "server": "s.example.com", "port": "8388"}, {"name": "x"}]', '{"proxies": [{"type": "ss", "server": "s.example.com", "port": 8388}]}',
        '{"proxies": []}', '{"proxies": {}}', '{"proxies": [1]}', '{"proxies": "x", "type": "ss", "server": "s", "port": 1}', '[]', '[[]]', '[1]', '"abc"', '5', 'null', 'true', '1.5', 'NaN',
        '{}', '{"type": "ss"}', '{"type": "ss", "server": "s"}', '{"type": "ss", "server": "s", "port": 0}', '{"type": "ss", "server": "s", "port": "0"}', '{"type": "", "server": "s", "port": 1}',
        '{"type": 5, "server": "s", "port": 1}', '{"type": "ss", "server": 5, "port": 1}', '{"type": ["ss"], "server": {"a": 1}, "port": 1}', '{"type": "ss", "server": "s", "port": 1.0}',
        '{"type": "ss", "server": "s", "port": true}', '{"type": "ss", "server": "s", "port": [1]}', '{"type": "ss", "server": "s", "port": {"a": 1}}', '{"type": "ss", "server": "s", "port": "65536"}',
        '{"type": "ss", "server": "s", "port": 99999999999999999999}', '{"type": "ss", "server": "s", "port": "abc"}', '{"type": "ss", "server": "s", "port": " 80 "}',
        '{"type": "ss", "server": "s", "port": -1}', '{"type": "ss", "server": "s", "port": 1, "name": 5}', '{"type": "ss", "server": "s", "port": 1, "name": null}',
        '{"type": "ss", "server": "s", "port": 1, "name": ""}', '{"type": "ss", "server": "s", "port": 1, "name": "CCA-"}', '{"type": "ss", "server": "s", "port": 1, "name": "  "}',
        '{"type": "ss", "server": "s", "port": 1, "name": "中文 节点"}',
        '{"type": "ss", "server": "s", "port": 1, "name": "a", "x": [1, 2.5, null, true, {"y": "z"}], "n": NaN, "i": Infinity, "big": 123456789012345678901234567890}',
        '{"type": "ss", "server": "s", "port": 1, "type": "vmess"}', '{"port": 1, "server": "s", "type": "ss", "name": "last"}', '  {"type": "ss", "server": "s", "port": 1}  ',
        '{"type": "ss", "server": "s", "port": 1,}', "{'type': 'ss', 'server': 's', 'port': 1}", '{"type": "ss", "server": "s", "port": 1} trailing', '\ufeff{"type": "ss", "server": "s", "port": 1}',
        '{"type": "ss", "server": "s", "port": "1e2"}', '{"type": "ss", "server": "s", "port": 1e2}', '{"type": "ss", "server": "s", "port": "٤٤٣"}', '{"type": "ss", "server": "s", "port": "4_4_3"}',
        '{"type": "ss", "server": "s", "port": 65535, "password": "a\\u0000b\\ud83d\\ude00\\n", "名": "值"}', '{"type": "ss", "server": "s", "port": 1, "ws-opts": {"headers": {"Host": "h"}}}',
        '{"proxies": [{"type": "ss", "server": "s", "port": 1}], "type": "x"}', '{"proxies": [[{"type": "ss", "server": "s", "port": 1}]]}', '[[{"type": "ss", "server": "s", "port": 1}]]',
        '{"type": "ss", "server": "s", "port": 1, "f": 1.0, "e": 1e-7, "g": 1e16, "z": -0.0}',
    ]),
    ('flow', [
        '{name: US-A, type: ss, server: s.example.com, port: 8388, cipher: aes-256-gcm, password: test-password}', '{name: "a, b", type: ss, server: s, port: 1}',
        "{name: 'a: b', type: ss, server: s, port: 1}", '{name: a, type: ss, server: s, port: 1, ws-opts: {path: /x, headers: {Host: h}}}', '{name: a, type: ss, server: s, port: 1, alpn: [h2, h3]}',
        '{type: ss, server: s, port: "443"}', '{type: ss, server: s, port: 443, udp: true, tfo: FALSE, x: ~, y: null, z: Null, w: }', '{type:ss,server:s,port:1}', '{ type : ss , server : s , port : 1 }',
        '{type: ss, server: s, port}', '{type: ss, server: s, port: 1, }', '{, type: ss, server: s, port: 1}', '{type: ss, server: s, port: 1,,}', '{:x, type: ss, server: s, port: 1}',
        '{1: a, true: b, type: ss, server: s, port: 5}', '{1: a, "1": b, 01: c, true: d, True: e, ~: f, null: g, : h, type: ss, server: s, port: 5}', '{type: ss, server: s, port: 5, url: http://x/y}',
        '{type: ss, server: s, port: 007}', '{type: ss, server: s, port: -5}', '{type: ss, server: s, port: 1.5}', '{type: ss, server: [a, b], port: 1}', '{type: ss, server: {a: b}, port: 1}',
        '{type: ss, server: s, port: 1, name: CCA-CCA-我}', '[{name: a, type: ss, server: s, port: 1}]', '[a, b]', '[ ]', '[,]', '{', '}', '[', ']', '{]', '[}', '{a}', '{a:}', '{:}', '{a: {b: c}}',
        '{type: ss, server: s, port: 1, q: "unterminated}', "{type: ss, server: s, port: 1, q: 'a, b: c'}", '{type: ss, server: s, port: 1, n: 12345678901234567890123}',
        '{type: ss, server: s, port: 1, n: ' + '9' * 4301 + '}', '{type: ss, server: s, port: ' + '0' * 4300 + '5}', '{0: a, false: b, False: c, -0: d, 00: e, type: ss, server: s, port: 1}',
        '{type: ss, server: s, port: 1, k: [a, [b, c], {d: e}], "q,r": "s,t"}', '{type: ss, server: s, port: 1, k: 1.50, l: 1e3, m: -7, n: +7, o: 0x1F, p: .5}', '{"type": "ss", server: s, port: 1}',
        '{proxies: [a]}', '{type: ss, server: s, port: 1, ９: 全角}', '{type: ss, server: s, port: 1, ٣: x, ３: y, 3: z}',
    ]),
    ('yaml', [
        'name: US-A\ntype: ss\nserver: s.example.com\nport: 8388\ncipher: aes-256-gcm\npassword: test-password\nudp: true',
        '- name: US-A\n  type: ss\n  server: s.example.com\n  port: 8388', 'proxies:\n  - name: n1\n    type: ss\n    server: s.example.com\n    port: 443\n  - name: n2\n    type: ss\n    server: t.example.com\n    port: 444',
        'proxies:\n- name: n1\n  type: ss\n  server: s.example.com\n  port: 443', 'proxies:\n  - {name: n1, type: ss, server: s.example.com, port: 443}\n  - {name: n2}',
        'proxies: [{"name": "j", "type": "ss", "server": "s", "port": 1}]', 'proxies: []', 'proxies:', 'proxies:\n  x: 1', 'proxies:\n  - a\n  - b',
        '# comment\n---\nname: a\ntype: vmess\nserver: v.example.com\nport: 443\nuuid: ' + ZERO + '\nalterId: 0\ncipher: auto\ntls: true\nnetwork: ws\nws-opts:\n  path: /ws\n  headers:\n    Host: h.example.com\n...',
        'name: a\r\ntype: ss\r\nserver: s\r\nport: 1\r\n', 'name: a\ntype: ss\nserver: s\nport: 1\nalpn:\n  - h2\n  - h3', 'name: a\ntype: ss\nserver: s\nport: 1\nalpn:\n- h2\n- h3',
        'name: a\ntype: ss\nserver: s\nport: 1\nopts:\n    deep:\n        deeper: 1\n    back: 2', 'name: a\ntype: ss\nserver: s\nport: 1\nopts:\n  a: 1\n b: 2', 'name: a\ntype: ss\nserver: s\nport: 1\n  x: 1',
        '  name: a\n  type: ss\n  server: s\n  port: 1', '\tname: a\n\ttype: ss\n\tserver: s\n\tport: 1', 'name: a\n\ttype: ss', 'name:a', 'key:value', 'a: b: c\ntype: ss\nserver: s\nport: 1',
        '"name": "a"\n"type": ss\n\'server\': s\nport: 1', '"na:me": a\ntype: ss\nserver: s\nport: 1', "'na:me' : a\ntype: ss\nserver: s\nport: 1", '"a\\"b": 1\ntype: ss\nserver: s\nport: 1',
        'name : a\ntype\t:\tss\nserver:  s\nport:\t1', 'name: a # not a comment\ntype: ss\nserver: s\nport: 1', 'name:\ntype: ss\nserver: s\nport: 1', 'name: ""\ntype: ss\nserver: s\nport: 1',
        'type: ss\nserver: s\nport: "8388"', "type: ss\nserver: s\nport: '8388'", 'type: ss\nserver: s\nport: 8388.0', 'type: ss\nserver: s\nport: 0x10', 'type: ss\nserver: s\nport: +5',
        'type: ss\nserver: s\nport: 5_0', 'type: ss\nserver: s\nport: ٥', 'type: ss\nserver: s\nport: 00080', 'type: ss\nserver: s\nport: -0', 'type: ss\nserver: s\nport: true', 'type: ss\nserver: s\nport: ~',
        'type: ss\nserver: s\nport:', 'type: ss\nserver: s\nport:\n  a: 1\n  2: [x]', 'type: ss\nserver: s\nport:\n  - 1', 'type: ss\nserver: 5\nport: 1', 'type: true\nserver: s\nport: 1',
        'type: ss\nserver: [a]\nport: 1', 'type: ss\nserver: s\nport: 1\n1: x\n"1": y\ntrue: z\n~: n\nfalse: f\n0: g\n: e', 'TYPE: ss\nserver: s\nport: 1', 'type: SS\nserver: S.Example.COM\nport: 1',
        'type: ss\nserver: s\nport: 1\nname: CCA-x', 'type: ss\nserver: s\nport: 1\nname: ａｂ　ｃ', 'type: ss\nserver: s\nport: 1\nname: a\x01b', 'type: ss\nserver: s\nport: 1\nurl: http://x:80/y?z=1',
        'type: ss\nserver: s\nport: 1\nk: {a: 1, b: [x, y]}', 'type: ss\nserver: s\nport: 1\nk: [1, "two", three, 4.5, null, ~, true]', 'type: ss\nserver: s\nport: 1\nk: "quoted: value"',
        "type: ss\nserver: s\nport: 1\nk: 'it''s'", 'type: ss\nserver: s\nport: 1\nk: |\n  multi\n  line', 'type: ss\nserver: s\nport: 1\nk: &anchor v\nj: *anchor', 'type: ss\nserver: s\nport: 1\n<<: {a: 1}',
        'type: ss\nserver: s\nport: 1\nk: 12345678901234567890123', 'type: ss\nserver: s\nport: 1\nk: NaN\nl: -Infinity\nm: 1e3', '-\n', '- ', '-', '- a', '- a\n- b', '-  name: x',
        '- name: x\n  type: ss\n  server: s\n  port: 1\n- name: y', '- name: x\n type: ss', '- name: x\n   type: ss', '- - a', '- {a: 1}', '- [a]', '- "a: b"',
        '- name: x\n  type: ss\n  server: s\n  port: 1\n  ws-opts:\n    path: /p\n    headers:\n      Host: h', 'a:\n- b\n- c', 'a:\n  - b\n  - c\ntype: ss\nserver: s\nport: 1', 'a: b\n   c: d',
        'a:\n  b:\n    c: 1\n  d: 2\ne: 3', 'a:\n    b: 1\n  c: 2', '---', '...', '#', '# only comment', '---\n...\n# x', 'a', 'a b', ':', ': x', 'a:', 'a: ', '"a": b', "'a': b", '"a: b', 'a": b',
        '"a" x: b', '"a"x: b', 'a\x0bb: c', 'a:\x0bb', 'a:\u3000b\ntype: ss\nserver: s\nport: 1', 'type:\u00a0ss\nserver: s\nport: 1', 'type: ss\x0cserver: s\nport: 1', 'type: ss\x1cserver: s\nport: 1',
        'type: ss\u2028server: s\nport: 1', 'type: ss\nserver: s\nport: 1\n\n\n   \n# c\n  # c2\nname: n', 'type: ss\nserver: s\nport: 1\nname: n\n   ', 'x\ny', 'x: 1\ny', 'x: 1\n- y', '- x\ny: 1',
        '- x: 1\ny: 2', 'x:\n  - a\n  b: 1', 'type: ss\nserver: s\nport: 1\nk: ' + '7' * 4301, 'type: ss\nserver: s\nport: 1\n' + '8' * 4301 + ': k', 'type: ss\nserver: s\nport: 1\nk:\n  - a: 1\n    b: 2\n  - c',
        'type: ss\nserver: s\nport: 1\nk:\n  -\n  - x', 'type: ss\nserver: s\nport: 1\nk:\n  l:\n  m: 1', 'type: ss\r\nserver: s\rport: 1', 'type: ss\nserver: s\nport: 1\n"k" : "v"\n\'s\':\'t\'',
        'type: ss\nserver: s\nport: 1\n- x', 'proxies:\n  - name: 一\n    type: ss\n    server: s\n    port: 1\nproxy-groups:\n  - name: g\n    proxies: [一]', 'proxies:\n\n  # the node\n  - name: n\n    type: ss\n    server: s\n    port: 2',
        'type: ss\nserver: s\nport: 1\nkey with spaces: v\n"": empty\n\'\': e2', 'type: ss\nserver: s\nport: 1\na: "x\\"y"\nb: \'p\\q\'\nc: "\\u4e2d"', 'type : ss\nserver : s\nport : 1\n: v',
    ]),
]
CLEAN = ['US-A', '', ' a ', 'CCA-1', 'CCA-CCA- x ', 'CCA-', 'cca-x', 'a\tb\x7fc\x00d', '\x1fx\x1f', 'x' * 200, ' ' + 'y' * 127 + ' z', '名字 😀', '\u3000全角空格\u3000', 'CCA- CCA-CCA-', 'a\nb\r\nc',
         'x' * 126 + ' CCA-', 5, None, True, ['a'], {'a': 1}, 1.5, 'CCA-' * 40, '\x85a\x85', 'a\x80b', ' CCA-x', 'CCA-\x00CCA-y']


def mutate(rng, s):
    """随机改几处：删、插、换、调位置、重复一段、删一段"""
    alphabet = '{}[]:,-#\'" \n\t~0123456789abcxyz.=&?@/%\\_+|*<>中'
    s = list(s)
    for _ in range(rng.randint(1, 4)):
        op = rng.randint(0, 5)
        i = rng.randint(0, len(s)) if s else 0
        if op == 0 and s:
            del s[min(i, len(s) - 1)]
        elif op == 1:
            s.insert(i, rng.choice(alphabet))
        elif op == 2 and s:
            s[min(i, len(s) - 1)] = rng.choice(alphabet)
        elif op == 3 and len(s) > 2:
            a, b = rng.randint(0, len(s) - 1), rng.randint(0, len(s) - 1)
            s[a], s[b] = s[b], s[a]
        elif op == 4 and s:
            a = rng.randint(0, len(s) - 1)
            s[i:i] = s[a:min(len(s), a + rng.randint(1, 8))]
        elif op == 5 and s:
            a = rng.randint(0, len(s) - 1)
            del s[a:a + rng.randint(1, 8)]
    return ''.join(s)


HOSTISH = re.compile(r'(?i)[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:com|net|org|io|cn)\b')


def stays_fake(s):
    """变出来的文字里像域名的（.com 这类结尾）还得是 example.com 和它下面的：仓库是公开的，不留碰巧像真域名的东西。
    写进文件时的样子（JSON 的转义）、里面 base64 的那几段解出来的也看（解析的结果会把它们带出来）"""
    views = [s, json.dumps(s, ensure_ascii=False)]
    tail = s.split('://', 1)[-1]
    for piece in [tail] + re.split(r'[#?@]', tail) + re.findall(r'[A-Za-z0-9+/_-]{8,}', s):
        raw = piece.strip().replace('-', '+').replace('_', '/')   # 和 PY_SRC 的 _b64 一样宽：不是 base64 的字符跳过去
        try:
            views.append(base64.b64decode(raw + '=' * (-len(raw) % 4)).decode('utf-8', 'replace'))
        except ValueError:
            pass
    return all(m.lower() == 'example.com' or m.lower().endswith('.example.com') for v in views for m in HOSTISH.findall(v))


def mutants(rng, seeds, count):
    out = []
    while len(out) < count:
        m = mutate(rng, rng.choice(seeds))
        if stays_fake(m):
            out.append(m)
    return out


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(REPO, 'cli', 'tests', 'vectors')
    ns = load()
    rng = random.Random(20261119)
    groups = [(g, list(items)) for g, items in CORPUS]
    seeds = [x for g, items in CORPUS for x in items if x.strip() and len(x) < 400]
    groups.append(('mutated', mutants(rng, seeds, 600)))
    cases, seen = [], set()
    for group, items in groups:
        n = 0
        for text in items:
            if text in seen:
                continue
            seen.add(text)
            n += 1
            c = {'name': '%s/%03d' % (group, n), 'in': text}
            try:
                # 解析成的节点照 json.dumps(p, ensure_ascii=False) 记（键的先后、不是字符串的键怎么写都在里面）
                c['ok'] = json.dumps(ns['node_parse'](text), ensure_ascii=False)
            except ns['NodeParseError'] as e:
                c['err'] = str(e)
            except Exception as e:   # 原来没人接的异常（cmd_node_check 只接 NodeParseError）：记下是哪一类
                c['raise'] = next(k.__name__ for k in (ValueError, TypeError, OverflowError, Exception) if isinstance(e, k))
            try:
                json.dumps(c, ensure_ascii=False).encode('utf-8')
            except UnicodeEncodeError:   # 结果里有落单的代理项（\ud83d 这类转义被改坏了）：写不成 UTF-8 的文件，这一条不收
                n -= 1
                continue
            cases.append(c)
    clean = [{'in': x, 'out': ns['node_clean_name'](x)} for x in CLEAN]
    doc = {'python': sys.version.split()[0], 'node_parse': cases, 'node_clean_name': clean}
    text = json.dumps(doc, ensure_ascii=False, indent=1) + '\n'   # 先整份写成文字：中途出错不留下半个文件
    with open(os.path.join(out_dir, 'corenet.json'), 'w', encoding='utf-8') as f:
        f.write(text)
    kinds = {}
    for c in cases:
        k = 'ok' if 'ok' in c else 'raise ' + c['raise'] if 'raise' in c else c['err'].split('：')[0].split('（')[0][:12]
        kinds[k] = kinds.get(k, 0) + 1
    print('%d 条 node_parse（%s），%d 条 node_clean_name' % (len(cases), '、'.join('%s %d' % kv for kv in sorted(kinds.items(), key=lambda kv: -kv[1])), len(clean)))


if __name__ == '__main__':
    main()
