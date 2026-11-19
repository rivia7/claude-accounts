#!/usr/bin/env python3
"""生成 cli/tests/vectors/clients.json：客户端更新（cli/src/cmd/clients）和它用到的兼容层（py/xml.rs、py/zip.rs）的对照数据。

    python3 -I cli/tools/gen_clients_vectors.py cli/tests/vectors            正式的两份（提交进仓库）：clients.json、src_clients.json
    python3 -I cli/tools/gen_clients_vectors.py 目录 --fuzz 种子 数量          另出一份 clients_fuzz.json：随机改坏的 XML / zip，
                                                                           量大、不提交；CCA_CLIENTS_EXTRA=那个文件 cargo test 时一起核对

三类：
- clients.json，标准库的行为：`xml.etree.ElementTree.fromstring`、`zipfile`、`urllib.parse.urljoin`、`str.casefold` / NFD；
- src_clients.json，PY_SRC（cca 里内嵌的 Python）里这一块的函数对一批输入给出的结果：地址规则、版本列表、版本号比大小、解压、
  下载……（gen_clients_scenarios.py 的 functions）；
- src_clients.json 里整条路的场景（scenarios）：对拍够不着的检查、下载、暂存、安装、恢复。照 tests/client_updates.py 的办法把 cu_run、cu_host、
  cu_download 换成假的，在临时目录里把每个场景跑一遍，每一步的输出、退出码、起了哪些命令、跑完以后的文件都记下来；
  tests/client_updates.py 里的每一项检查在这里对 Python 的结果先核对一遍（不成立就不生成）。Rust 的测试逐个场景重放，要求
  每一步都和记下来的一样。

PY_SRC 是从仓库里的 cca 取的：切换到只有 Rust 以后 cca 里没有它了，这份文件就是留下来的对照（那以后不用再生成）。
不联网、不碰真的 /Applications：桌面客户端、终端工具都在临时目录里，起命令、下载的函数都是假的。"""
import base64
import contextlib
import hashlib
import io
import json
import os
import plistlib
import random
import re
import shutil
import stat
import struct
import sys
import tempfile
import types
import unicodedata
import xml.etree.ElementTree as ET
import zipfile
import zlib
from unittest import mock
from urllib.parse import urljoin

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def b64(data):
    """字节 → JSON 里的写法：短的是 base64；长的先压缩（{'z': base64}），不然两兆的版本列表这一类会把文件撑到十几兆"""
    if len(data) <= 256:
        return base64.b64encode(data).decode('ascii')
    return {'z': base64.b64encode(zlib.compress(data, 9)).decode('ascii')}


def unb64(v):
    return base64.b64decode(v) if isinstance(v, str) else zlib.decompress(base64.b64decode(v['z']))


def py_src():
    with open(os.path.join(REPO, 'cca'), encoding='utf-8') as f:
        return f.read().split("PY_SRC=$(cat <<'PY'\n", 1)[1].split('\nPY\n)', 1)[0]


DEFINITIONS = py_src().split('\nif len(sys.argv) < 2', 1)[0]


def load(env):
    """PY_SRC 的定义，在给定的环境里执行一遍（HOME、CCA_HOME 是执行时定下的）"""
    ns = {}
    with mock.patch.dict(os.environ, env, clear=True):
        exec(compile(DEFINITIONS, 'PY_SRC', 'exec'), ns)
    return ns


def outcome(f):
    """调一下，记下结果：{'ok': …}，或者出的是哪一类异常（ClientUpdateError 带着 status、message）"""
    try:
        return {'ok': f()}
    except Exception as e:
        if type(e).__name__ == 'ClientUpdateError':
            return {'cu': [e.status, e.message]}
        return {'err': type(e).__name__}


# --- XML -------------------------------------------------------------------------------------------------------------
def xml_dump(e):
    return [e.tag, [[k, v] for k, v in e.attrib.items()], e.text or '', [xml_dump(c) for c in e]]


def xml_outcome(data):
    try:
        return {'ok': xml_dump(ET.fromstring(data))}
    except ET.ParseError:
        return {'err': 'ParseError'}
    except Exception as e:   # 不认识的编码这一类（LookupError、ValueError）
        return {'err': type(e).__name__}


NS = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
XML_DOCS = [
    b'<a/>', b'<a></a>', b'<a>x</a>', b' <a/> ', b'\n<a/>\n', b'<a/>x', b'x<a/>', b'<a/><b/>', b'', b' ', b'<', b'<a', b'<a>', b'</a>', b'<a></b>', b'<a/ >', b'<a / >',
    b'<a b="1"/>', b"<a b='1'/>", b'<a b=1/>', b'<a b/>', b'<a b="1" b="2"/>', b'<a b="1"c="2"/>', b'<a b = "1" c\t=\n\'2\'/>', b'<a b="<"/>', b'<a b=">"/>', b'<a b="&lt;&gt;&amp;&apos;&quot;"/>',
    b'<a b="x\ny\tz\r\nw\rv"/>', b'<a b="&#10;&#9;&#13;"/>', b'<a b="&#x41;&#65;"/>', b'<a b="&#X41;"/>', b'<a b="&#0;"/>', b'<a b="&#1;"/>', b'<a b="&#x1F;"/>', b'<a b="&#x7F;&#x80;&#x9F;"/>',
    b'<a b="&#xD800;"/>', b'<a b="&#xFFFE;"/>', b'<a b="&#xFFFF;"/>', b'<a b="&#x10FFFF;"/>', b'<a b="&#x110000;"/>', b'<a b="&#99999999999999999999;"/>', b'<a b="&#x1FFFE;"/>',
    b'<a b="&#;"/>', b'<a b="&#x;"/>', b'<a b="&#12"/>', b'<a b="&foo;"/>', b'<a b="&"/>', b'<a b="& "/>', b'<a b="&lt"/>', b'<a b="&a:b;"/>',
    b'<a>&lt;&gt;&amp;&apos;&quot;</a>', b'<a>&#65;&#x42;</a>', b'<a>&foo;</a>', b'<a>&</a>', b'<a>&#0;</a>', b'<a>&#xD7FF;&#xE000;&#xFFFD;</a>', b'<a>&#xDFFF;</a>',
    b'<a>x\ry\r\nz\n</a>', b'<a><![CDATA[x<y&z]]></a>', b'<a><![CDATA[x\r\ny\rz]]>w</a>', b'<a><![CDATA[]]></a>', b'<a><![CDATA[x]]</a>', b'<a><![cdata[x]]></a>', b'<![CDATA[x]]><a/>',
    b'<a/><![CDATA[x]]>', b'<a>]]></a>', b'<a>]]</a>', b'<a>]</a>', b'<a>]]]></a>', b'<a>] ]></a>', b'<a>x]]>y</a>',
    b'<a><!-- c --></a>', b'<a>x<!-- c -->y</a>', b'<a><!-- a -- b --></a>', b'<a><!---></a>', b'<a><!----></a>', b'<a><!-- x ---></a>', b'<!-- c --><a/>', b'<a/><!-- c -->', b'<a><!-- c -></a>',
    b'<a><!- c --></a>', b'<!--', b'<a/><!--',
    b'<a><?pi x?></a>', b'<a>x<?pi?>y</a>', b'<?pi?><a/>', b'<a/><?pi x y z ?>', b'<a><?pi</a>', b'<a><?xml x?></a>', b'<a><?XML x?></a>', b'<a><?Xml?></a>', b'<a><?xmlx?></a>', b'<a><?p:i?></a>',
    b'<a><? pi?></a>', b'<a><?pi?x?></a>', b'<a><?pi x?y?></a>', b'<?pi', b'<a><?1?></a>',
    b'<?xml version="1.0"?><a/>', b'<?xml version="1.0" encoding="utf-8"?><a/>', b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a/>', b"<?xml version='1.0' standalone='no'?><a/>",
    b'<?xml version="1.0" standalone="maybe"?><a/>', b'<?xml version="2.0"?><a/>', b'<?xml version=""?><a/>', b'<?xml version="1.0"encoding="utf-8"?><a/>', b'<?xml encoding="utf-8"?><a/>',
    b'<?xml?><a/>', b'<?xml ?><a/>', b'<?xml version="1.0" ?><a/>', b'<?xml  version = "1.0"  ?><a/>', b'<?xml version="1.0" encoding="utf-8" foo="1"?><a/>', b'<?xml version="1.0" standalone="yes" encoding="utf-8"?><a/>',
    b' <?xml version="1.0"?><a/>', b'<a/><?xml version="1.0"?>', b'<?xml version="1.0"?><?xml version="1.0"?><a/>', b'<?xml version="1.0" encoding=""?><a/>', b'<?xml version="1.0" encoding="1x"?><a/>',
    b'<?xml version="1.0" encoding="utf 8"?><a/>', b'<?xml version="1.0" encoding="UTF-8"?>\n<a>\xe4\xb8\xad</a>', b'<?xml version="1.0" encoding="ISO-8859-1"?><a>\xe4\xb8\xad\x80\x9f</a>',
    b'<?xml version="1.0" encoding="iso-8859-1"?><a b="\xff"/>', b'<?xml version="1.0" encoding="US-ASCII"?><a>x</a>', b'<?xml version="1.0" encoding="US-ASCII"?><a>\xe4</a>',
    b'<?xml version="1.0" encoding="us-ascii"?><a/>', b'<?xml version="1.0" encoding="UTF-16"?><a/>', b'<?xml version="1.0" encoding="UTF-16LE"?><a/>', b'<?xml version="1.0" encoding="Utf-8"?><a/>',
    b'<?xml version="1.0" encoding="ISO-8859-1"?><\xaa/>', b'<?xml version="1.0" encoding="ISO-8859-1"?><a\xb7/>', b'<?xml version="1.0" encoding="ISO-8859-1"?><\xd7/>',
    b'<?xml version="1.0" encoding="ISO-8859-1"?><a>\x01</a>', b'<?xml version="1.0" encoding="ISO-8859-1"?><a>\x7f</a>',
    b'\xef\xbb\xbf<a/>', b'\xef\xbb\xbf<?xml version="1.0" encoding="utf-8"?><a/>', b'\xef\xbb\xbf<?xml version="1.0" encoding="ISO-8859-1"?><a>\xe4</a>', b'\xef\xbb\xbf \xef\xbb\xbf<a/>', b'\xef\xbb<a/>',
    '<a>中</a>'.encode('utf-16'), '<?xml version="1.0" encoding="UTF-16"?><a>中</a>'.encode('utf-16'), '<?xml version="1.0" encoding="utf-16"?><a>中</a>'.encode('utf-16-le'),
    '<a>中</a>'.encode('utf-16-be'), b'\xfe\xff' + '<?xml version="1.0" encoding="UTF-16BE"?><a/>'.encode('utf-16-be'), b'\xfe\xff' + '<?xml version="1.0" encoding="UTF-16LE"?><a/>'.encode('utf-16-be'),
    b'\xff\xfe' + '<?xml version="1.0" encoding="UTF-16LE"?><a/>'.encode('utf-16-le'), b'\xff\xfe' + '<?xml version="1.0" encoding="UTF-8"?><a/>'.encode('utf-16-le'),
    b'\xff\xfe' + '<?xml version="1.0" encoding="ISO-8859-1"?><a/>'.encode('utf-16-le'), '<a>\U0001f600</a>'.encode('utf-16-le'), '<\u00aa/>'.encode('utf-16-le'), '<a\u00b7/>'.encode('utf-16-be'),
    '<\U00010400/>'.encode('utf-16-le'), '<a>x</a>'.encode('utf-16-le') + b'\x00', b'\xff\xfe<\x00a\x00/\x00>\x00\xfe\xff', '<a>\ufffe</a>'.encode('utf-16-le'), b'\xff\xfe', b'\xfe\xff<',
    b'\x00<\x00a\x00/\x00>', b'<\x00a\x00/\x00>\x00', b'<\x00', b'\x00', b'a\x00',
    b'<a>\x00</a>', b'<a>\x08</a>', b'<a>\x0b</a>', b'<a>\x0c</a>', b'<a>\x1f</a>', b'<a>\x7f</a>', b'<a>\xc2\x80</a>', b'<a>\xef\xbf\xbe</a>', b'<a>\xef\xbf\xbf</a>', b'<a>\xef\xbf\xbd</a>',
    b'<a>\xed\xa0\x80</a>', b'<a>\xc0\x80</a>', b'<a>\xe0\x80\x80</a>', b'<a>\xf4\x90\x80\x80</a>', b'<a>\xf0\x9f\x98\x80</a>', b'<a>\xff</a>', b'<a>\xe4\xb8</a>', b'<a b="\x01"/>', b'<a><!--\x01--></a>',
    b'<a><?p \x01?></a>', b'<a><![CDATA[\x01]]></a>', b'<a>\xf0\x9f\xbf\xbe</a>', b'<a>\xef\xb7\x90</a>',
    '<中/>'.encode(), '<a中="1"/>'.encode(), '<a b="中"/>'.encode(), b'<\xc3\x97/>', b'<a\xc2\xb7/>', b'<\xc2\xb7a/>', b'<\xc2\xaa/>', b'<\xf0\x90\x90\x80/>', b'<a-b.c_d1/>', b'<1a/>', b'<-a/>', b'<.a/>', b'<_a/>',
    b'<a:b/>', b'<:a/>', b'<a:/>', b'<a:b:c/>', b'<a: b/>', b'<a b:c="1"/>', b'<a :b="1"/>', b'<a b:="1"/>',
    b'<a xmlns="u"/>', b'<a xmlns="u"><b/></a>', b'<a xmlns="u"><b xmlns=""/></a>', b'<a xmlns="u" b="1"/>', b'<p:a xmlns:p="u"/>', b'<p:a xmlns:p="u" p:b="1" b="2"/>', b'<a xmlns:p="u"><p:b/><b/></a>',
    b'<a xmlns:p="u"><b xmlns:p="v"><p:c/></b><p:d/></a>', b'<p:a/>', b'<a p:b="1"/>', b'<a xmlns:p="u" xmlns:q="u" p:b="1" q:b="2"/>', b'<a xmlns:p="u" xmlns:q="v" p:b="1" q:b="2"/>',
    b'<a xmlns:p="u" xmlns:p="v"/>', b'<a xmlns="u" xmlns="v"/>', b'<a xmlns:p=""/>', b'<a xmlns=""/>', b'<a xmlns:xml="http://www.w3.org/XML/1998/namespace"/>', b'<a xmlns:xml="u"/>',
    b'<a xmlns:p="http://www.w3.org/XML/1998/namespace"/>', b'<a xmlns="http://www.w3.org/XML/1998/namespace"/>', b'<a xmlns:xmlns="u"/>', b'<a xmlns:p="http://www.w3.org/2000/xmlns/"/>',
    b'<a xmlns="http://www.w3.org/2000/xmlns/"/>', b'<a xml:lang="en"/>', b'<xml:a/>', b'<xmlns:a/>', b'<a xmlns:p="u}v"/>', b'<a xmlns="u}v"/>', b'<a xmlns:p="u{v"/>', b'<a xmlns:p="a b"/>',
    b'<a xmlns:p="&#x7D;"/>', b'<a xmlns:p="u"><b xmlns:p=""/></a>', b'<a xmlns:P="u"><p:b/></a>', b'<a XMLNS="u"/>', b'<a xmlns:p="u\nv"/>', b'<a xmlns:p="u"><p:b></p:b></a>',
    b'<a xmlns:p="u" xmlns:q="u"><p:b></q:b></a>', b'<a xmlns:p="u"><p:b></b></a>', b'<a></a >', b'<a></ a>', b'<a></a\n>', b'<a></a b>', b'<A></a>',
    b'<a><b>1</b>t<c>2</c>u</a>', b'<a>1<b/>2</a>', b'<a> <b/> </a>', b'<a><b><c><d>deep</d></c></b></a>', b'<a><b/><b/><b x="1"/></a>', b'<a>&#x20;x&#x20;</a>',
    b'<!DOCTYPE a><a/>', b'<!doctype a><a/>', b'<!DOCTYPE a [<!ENTITY e "v">]><a>&e;</a>', b'<a><!DOCTYPE a></a>', b'<a/><!DOCTYPE a>', b'<!ELEMENT a ANY><a/>', b'<!', b'<!a', b'<a><!x></a>',
    '<!DOCTYPE a [<!ENTITY e "v">]><a>&e;</a>'.encode('utf-16'),
    b'<?xml version="1.0" encoding="latin-1"?><a/>', b'<?xml version="1.0" encoding="utf8"?><a/>', b'<?xml version="1.0" encoding="ascii"?><a/>', b'<?xml version="1.0" encoding="windows-1252"?><a>\x80</a>',
    b'<?xml version="1.0" encoding="gbk"?><a/>', b'<?xml version="1.0" encoding="nope-0"?><a/>', b'<?xml version="1.0" encoding="UTF-32"?><a/>',
    ('<rss xmlns:sparkle="%s" version="2.0"><channel><title>t</title><item><sparkle:version>200</sparkle:version><sparkle:shortVersionString>2.0</sparkle:shortVersionString>'
     '<enclosure url="https://persistent.oaistatic.com/x.zip" length="5" sparkle:edSignature="QQ=="/></item></channel></rss>' % NS).encode(),
]
XML_TOKENS = [b'<', b'>', b'/', b'</', b'/>', b'=', b'"', b"'", b' ', b'\n', b'\r', b'\t', b'&', b'&lt;', b'&#65;', b'&#x', b';', b'<!--', b'-->', b'--', b'<![CDATA[', b']]>', b']', b'<?', b'?>', b'xml',
              b'xmlns', b'xmlns:', b':', b'a', b'b', b'p:', b'x', b'1', b'<?xml version="1.0"?>', b' encoding="utf-8"', b'\x00', b'\x01', b'\xc3\xa9', b'\xff', b'\xef\xbb\xbf', b'<!DOCTYPE a>', b'}', b'{',
              b'\xe4\xb8\xad', b'-', b'.', b'_', b'#', b'%', b'<a>', b'</a>', b'<b x="1">', b'</b>', b' xmlns:p="u"', b' xmlns="v"', b'<p:c/>', b' p:y="2"', b'\xf0\x9f\x98\x80', b'\xc2\xb7']


def mutate(rng, data, tokens):
    data = bytearray(data)
    for _ in range(rng.choice((1, 1, 1, 2, 3))):
        kind = rng.randrange(6)
        at = rng.randrange(len(data) + 1)
        if kind == 0 and data:
            del data[rng.randrange(len(data))]
        elif kind == 1:
            data[at:at] = rng.choice(tokens)
        elif kind == 2 and data:
            end = min(len(data), at + rng.randrange(1, 6))
            data[at:end] = rng.choice(tokens)
        elif kind == 3 and data:
            i = rng.randrange(len(data))
            data[i] ^= 1 << rng.randrange(8)
        elif kind == 4 and data:
            del data[at:at + rng.randrange(1, 12)]
        elif data:
            a, b = sorted((rng.randrange(len(data) + 1), rng.randrange(len(data) + 1)))
            data[at:at] = data[a:b]
    return bytes(data)


def xml_vectors(rng, fuzz):
    docs = list(XML_DOCS)
    seeds = [d for d in XML_DOCS if len(d) < 400]
    seen = set(docs)
    while len(docs) < len(XML_DOCS) + fuzz:
        d = mutate(rng, rng.choice(seeds), XML_TOKENS)
        if d not in seen and len(d) < 600:
            seen.add(d)
            docs.append(d)
    return [[b64(d), xml_outcome(d)] for d in docs]


# --- zip -------------------------------------------------------------------------------------------------------------
REG, LNK, DIR = stat.S_IFREG | 0o644, stat.S_IFLNK | 0o777, stat.S_IFDIR | 0o755
ST, DF = zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED


def make_zip(entries, comment=b'', flags=0, zip64=False):
    """entries：[(名字, 内容, 权限位连同类型, 压缩方法)]。时间固定，字节是定的"""
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as z:
        z.comment = comment
        for name, data, mode, method in entries:
            zi = zipfile.ZipInfo(name, (2026, 1, 2, 3, 4, 6))
            zi.create_system = 3
            zi.external_attr = mode << 16
            zi.compress_type = method
            zi.flag_bits |= flags
            if zip64:
                with z.open(zi, 'w', force_zip64=True) as f:
                    f.write(data)
            else:
                z.writestr(zi, data)
    return out.getvalue()


def zip_outcome(data):
    """这个压缩包 zipfile 怎么看：打不开是哪一类异常；打得开就是每一项的属性，和读它的结果（内容的摘要，或者哪一类异常）"""
    path = os.path.join(tempfile.gettempdir(), 'cca-clients-vec-%d.zip' % os.getpid())
    with open(path, 'wb') as f:
        f.write(data)
    try:
        with warnings_off():
            try:
                z = zipfile.ZipFile(path)
            except Exception as e:
                return {'err': type(e).__name__}
            with z:
                rows = []
                for item in z.infolist():
                    row = {'name': item.filename, 'orig': item.orig_filename, 'flags': item.flag_bits, 'method': item.compress_type, 'crc': item.CRC, 'csize': item.compress_size,
                           'size': item.file_size, 'attr': item.external_attr, 'is_dir': item.is_dir()}
                    if item.file_size > 50_000_000 or item.compress_size > 50_000_000:
                        row['read'] = {'skip': True}   # 声称很大的：不读（Python 会真的去要那么多内存）
                    else:
                        try:
                            whole = z.read(item)
                            row['read'] = {'ok': hashlib.sha256(whole).hexdigest(), 'len': len(whole)}
                        except Exception as e:
                            row['read'] = {'err': type(e).__name__}
                        try:   # 一段一段读（shutil.copyfileobj 的读法）：结果应该和整个读一样，出错的类别也是
                            h, n = hashlib.sha256(), 0
                            with z.open(item) as f:
                                while True:
                                    block = f.read(1024 * 1024)
                                    if not block:
                                        break
                                    h.update(block)
                                    n += len(block)
                            row['chunks'] = {'ok': h.hexdigest(), 'len': n}
                        except Exception as e:
                            row['chunks'] = {'err': type(e).__name__}
                    rows.append(row)
                return {'ok': rows}
    finally:
        os.unlink(path)


@contextlib.contextmanager
def warnings_off():
    import warnings
    with warnings.catch_warnings():
        warnings.simplefilter('ignore')
        yield


def patch(data, find, replace, count=1):
    assert find in data, find
    return data.replace(find, replace, count)


def zip64_end(data):
    """在「目录表结束」记录前面加上 zip64 的那两条记录（zipfile 自己只在放不下时才写）"""
    end = data.rindex(b'PK\x05\x06')
    _, _, _, this, total, size, offset, _ = struct.unpack('<4s4H2LH', data[end:end + 22])
    record = struct.pack('<4sQ2H2L4Q', b'PK\x06\x06', 44, 45, 45, 0, 0, this, total, size, offset)
    locator = struct.pack('<4sLQL', b'PK\x06\x07', 0, end, 1)
    return data[:end] + record + locator + data[end:]


def zip_docs():
    plain = make_zip([('A.app/', b'', DIR, ST), ('A.app/x', b'hello world', REG, ST), ('A.app/y', b'compress me ' * 500, REG | 0o111, DF), ('A.app/link', b'x', LNK, ST)])
    big = make_zip([('big', bytes(range(256)) * 9000, REG, DF), ('zeros', b'\0' * 3_000_000, REG, DF), ('empty', b'', REG, DF), ('stored-empty', b'', REG, ST)])
    names = make_zip([('中文/é.txt', b'x', REG, ST), ('e\u0301', b'y', REG, ST), ('a\\b', b'z', REG, ST), ('sp ace', b'', REG, ST)])
    docs = {
        'plain': plain, 'big': big, 'unicode-names': names, 'empty-archive': make_zip([]), 'comment': make_zip([('a', b'x', REG, ST)], comment=b'hello comment'),
        'comment-with-signature': make_zip([('a', b'x', REG, ST)], comment=b'xxPK\x05\x06yy'),
        'comment-64k': make_zip([('a', b'x', REG, ST)], comment=b'c' * 65535),
        'zip64-extra-fields': make_zip([('a', b'x' * 100, REG, DF), ('b', b'y', REG, ST)], zip64=True),
        'zip64': zip64_end(make_zip([('a', b'x' * 100, REG, DF), ('b', b'y', REG, ST)], zip64=True)),
        'zip64-plain-entries': zip64_end(plain), 'zip64-with-comment': zip64_end(make_zip([('a', b'x', REG, ST)], comment=b'hello comment')),
        'zip64-prepended': b'junk junk junk' + zip64_end(plain),
        'not-a-zip': b'hello, this is not a zip file', 'empty-file': b'', 'short': b'PK\x05\x06', 'only-signature-22': b'PK\x05\x06' + b'\0' * 18,
        'truncated-end': plain[:-1], 'truncated-10': plain[:-10], 'truncated-middle': plain[:len(plain) // 2], 'garbage-appended': plain + b'x' * 100,
        'garbage-appended-70k': plain + b'x' * 70000, 'prepended': b'#!/bin/sh\n' * 20 + plain, 'prepended-and-appended': b'junk' + plain + b'\0\0',
        'comment-length-too-big': plain[:-2] + b'\x09\x00', 'comment-length-with-data': plain[:-2] + b'\x03\x00abc', 'comment-shorter-than-claimed': plain[:-2] + b'\x09\x00ab',
        'bad-crc': patch(plain, b'hello world', b'hello World'), 'bad-crc-deflated': None,
        'local-magic': patch(plain, b'PK\x03\x04', b'PK\x03\x05'), 'central-magic': patch(plain, b'PK\x01\x02', b'PK\x01\x03'),
        'second-central-magic': plain[:plain.index(b'PK\x01\x02') + 4] + patch(plain[plain.index(b'PK\x01\x02') + 4:], b'PK\x01\x02', b'PK\x09\x09'),
        'local-name-differs': patch(plain, b'A.app/x', b'A.app/z'), 'central-name-differs': plain[:plain.index(b'PK\x01\x02')] + patch(plain[plain.index(b'PK\x01\x02'):], b'A.app/x', b'A.app/z'),
        'encrypted-flag': make_zip([('a', b'x', REG, ST)], flags=1), 'utf8-flag-ascii-name': make_zip([('a', b'x', REG, ST)], flags=0x800),
        'patched-data-flag': make_zip([('a', b'x', REG, ST)], flags=0x20), 'strong-encryption-flag': make_zip([('a', b'x', REG, ST)], flags=0x40),
    }
    # 一项一项改目录表里的字段
    one = make_zip([('name', b'payload payload payload', REG, DF)])
    cd = one.index(b'PK\x01\x02')

    def central(offset, fmt, value):
        return one[:cd + offset] + struct.pack(fmt, value) + one[cd + offset + struct.calcsize(fmt):]

    def local(offset, fmt, value):
        return one[:offset] + struct.pack(fmt, value) + one[offset + struct.calcsize(fmt):]
    raw = zlib.compressobj(9, zlib.DEFLATED, -15)
    deflated = raw.compress(b'payload payload payload') + raw.flush()
    at = one.index(deflated)
    docs.update({
        'one': one, 'extract-version-64': central(6, '<B', 64), 'extract-version-63': central(6, '<B', 63), 'method-99': central(10, '<H', 99), 'method-bzip2': central(10, '<H', 12),
        'method-lzma': central(10, '<H', 14), 'method-zstd': central(10, '<H', 93), 'method-stored-but-deflated-data': central(10, '<H', 0), 'method-1': central(10, '<H', 1),
        'crc-0': central(16, '<L', 0), 'csize-smaller': central(20, '<L', 5), 'csize-bigger': central(20, '<L', 4000), 'csize-0': central(20, '<L', 0),
        'csize-plus-1': central(20, '<L', len(deflated) + 1), 'size-smaller': central(24, '<L', 5), 'size-bigger': central(24, '<L', 500), 'size-0': central(24, '<L', 0),
        'size-huge': central(24, '<L', 0xfffffffe), 'name-length-longer': central(28, '<H', 60), 'name-length-0': central(28, '<H', 0), 'extra-length-wrong': central(30, '<H', 7),
        'comment-length-wrong': central(32, '<H', 7), 'header-offset-1': central(42, '<L', 1), 'header-offset-past-end': central(42, '<L', len(one) + 10),
        'header-offset-into-central': central(42, '<L', cd), 'external-attr-fifo': central(38, '<L', (stat.S_IFIFO | 0o644) << 16), 'flags-utf8-central-only': central(8, '<H', 0x800),
        'local-flags-utf8-only': local(6, '<H', 0x800), 'local-name-length-longer': local(26, '<H', 9), 'local-extra-length-5': local(28, '<H', 5),
        'local-method-differs': local(8, '<H', 0), 'local-sizes-zero': local(18, '<LL', 0) if False else one[:18] + b'\0' * 8 + one[26:],
        'deflate-first-byte-flipped': one[:at] + bytes([one[at] ^ 0xff]) + one[at + 1:], 'deflate-last-byte-flipped': one[:at + len(deflated) - 1] + bytes([one[at + len(deflated) - 1] ^ 0x55]) + one[at + len(deflated):],
        'deflate-zeroed': one[:at] + b'\0' * len(deflated) + one[at + len(deflated):], 'deflate-ff': one[:at] + b'\xff' * len(deflated) + one[at + len(deflated):],
        'deflate-truncated-stream': central(20, '<L', len(deflated) - 3), 'end-record-entries-wrong': one[:-12] + struct.pack('<HH', 9, 9) + one[-8:],
        'end-record-size-smaller': one[:-10] + struct.pack('<L', 10) + one[-6:], 'end-record-size-bigger': one[:-10] + struct.pack('<L', 4000) + one[-6:],
        'end-record-offset-0': one[:-6] + struct.pack('<L', 0) + one[-2:], 'end-record-offset-big': one[:-6] + struct.pack('<L', 0x7fffffff) + one[-2:],
        'end-record-disk-1': one[:-18] + struct.pack('<H', 1) + one[-16:],
    })
    docs['bad-crc-deflated'] = central(16, '<L', 12345)
    # 文件名的几种编码
    cp = make_zip([('placeholder-name', b'x', REG, ST)])
    docs['cp437-name'] = cp.replace(b'placeholder-name', b'caf\x82 \x80\xe1\xff \x01\x7f ok')
    docs['utf8-flag-bad-name'] = make_zip([('placeholder-name', b'x', REG, ST)], flags=0x800).replace(b'placeholder-name', b'bad \xff\xfe name!!!!')
    docs['utf8-flag-good-name'] = make_zip([('placeholder-name!!', b'x', REG, ST)], flags=0x800).replace(b'placeholder-name!!', '中文名字的文件'.encode()[:18].ljust(18, b'_'))
    docs['nul-in-name'] = cp.replace(b'placeholder-name', b'before\x00after-nul')
    docs['name-all-nul'] = cp.replace(b'placeholder-name', b'\x00' * 16)
    # 附加字段
    def with_extra(extra, name=b'name', data=b'x'):
        local_h = struct.pack('<4s2B4HL2L2H', b'PK\x03\x04', 20, 0, 0, 0, 0, 0x21, zlib.crc32(data), len(data), len(data), len(name), 0) + name + data
        central_h = struct.pack('<4s4B4HL2L5H2L', b'PK\x01\x02', 20, 3, 20, 0, 0, 0, 0, 0x21, zlib.crc32(data), len(data), len(data), len(name), len(extra), 0, 0, 0, REG << 16, 0) + name + extra
        return local_h + central_h + struct.pack('<4s4H2LH', b'PK\x05\x06', 0, 0, 1, 1, len(central_h), len(local_h), 0)
    crc = zlib.crc32(b'name')
    docs.update({
        'extra-unknown': with_extra(struct.pack('<HH', 0x9999, 3) + b'abc'), 'extra-truncated': with_extra(struct.pack('<HH', 0x9999, 30) + b'abc'), 'extra-3-bytes': with_extra(b'abc'),
        'extra-unicode-path': with_extra(struct.pack('<HHBL', 0x7075, 5 + 9, 1, crc) + '新名字'.encode()), 'extra-unicode-path-wrong-crc': with_extra(struct.pack('<HHBL', 0x7075, 5 + 3, 1, crc ^ 1) + b'new'),
        'extra-unicode-path-version-2': with_extra(struct.pack('<HHBL', 0x7075, 5 + 3, 2, crc) + b'new'), 'extra-unicode-path-bad-utf8': with_extra(struct.pack('<HHBL', 0x7075, 5 + 2, 1, crc) + b'\xff\xfe'),
        'extra-unicode-path-short': with_extra(struct.pack('<HH', 0x7075, 3) + b'\x01ab'), 'extra-unicode-path-empty': with_extra(struct.pack('<HHBL', 0x7075, 5, 1, crc)),
        'extra-unicode-path-with-nul': with_extra(struct.pack('<HHBL', 0x7075, 5 + 5, 1, crc) + b'ab\x00cd'), 'extra-zip64-unused': with_extra(struct.pack('<HHQ', 1, 8, 77)),
        'extra-two-fields': with_extra(struct.pack('<HH', 0x9999, 1) + b'a' + struct.pack('<HHBL', 0x7075, 5 + 3, 1, crc) + b'two'),
    })

    def zip64_extra(size, csize, offset, extra, data=b'x', name=b'name'):
        local_h = struct.pack('<4s2B4HL2L2H', b'PK\x03\x04', 45, 0, 0, 0, 0, 0x21, zlib.crc32(data), len(data), len(data), len(name), 0) + name + data
        central_h = struct.pack('<4s4B4HL2L5H2L', b'PK\x01\x02', 45, 3, 45, 0, 0, 0, 0, 0x21, zlib.crc32(data), csize, size, len(name), len(extra), 0, 0, 0, REG << 16, offset) + name + extra
        return local_h + central_h + struct.pack('<4s4H2LH', b'PK\x05\x06', 0, 0, 1, 1, len(central_h), len(local_h), 0)
    F = 0xffffffff
    docs.update({
        'zip64-extra-size': zip64_extra(F, 1, 0, struct.pack('<HHQ', 1, 8, 1)), 'zip64-extra-all-three': zip64_extra(F, F, F, struct.pack('<HHQQQ', 1, 24, 1, 1, 0)),
        'zip64-extra-missing-size': zip64_extra(F, 1, 0, struct.pack('<HH', 1, 0)), 'zip64-extra-missing-csize': zip64_extra(F, F, 0, struct.pack('<HHQ', 1, 8, 1)),
        'zip64-extra-missing-offset': zip64_extra(1, 1, F, struct.pack('<HH', 1, 0)), 'zip64-extra-offset-only': zip64_extra(1, 1, F, struct.pack('<HHQ', 1, 8, 0)),
        'zip64-extra-size-huge': zip64_extra(F, 1, 0, struct.pack('<HHQ', 1, 8, 2 ** 40)), 'zip64-extra-offset-huge': zip64_extra(1, 1, F, struct.pack('<HHQ', 1, 8, 2 ** 40)),
        'zip64-extra-offset-2-63': zip64_extra(1, 1, F, struct.pack('<HHQ', 1, 8, 2 ** 63)), 'no-zip64-extra-but-ffffffff': zip64_extra(F, F, F, b''),
    })
    # zip64 的结束记录
    z64 = docs['zip64']
    loc = z64.rindex(b'PK\x06\x07')
    rec = z64.rindex(b'PK\x06\x06')
    docs.update({
        'zip64-locator-disk-1': z64[:loc + 4] + struct.pack('<L', 1) + z64[loc + 8:], 'zip64-locator-two-disks': z64[:loc + 16] + struct.pack('<L', 2) + z64[loc + 20:],
        'zip64-record-magic': z64[:rec] + b'PK\x06\x09' + z64[rec + 4:], 'zip64-record-size-huge': z64[:rec + 40] + struct.pack('<Q', 2 ** 40) + z64[rec + 48:],
        'zip64-record-size-2-63': z64[:rec + 40] + struct.pack('<Q', 2 ** 63) + z64[rec + 48:], 'zip64-record-offset-2-63': z64[:rec + 48] + struct.pack('<Q', 2 ** 63) + z64[rec + 56:],
        'zip64-record-offset-huge': z64[:rec + 48] + struct.pack('<Q', 2 ** 40) + z64[rec + 56:], 'zip64-locator-at-start': b'PK\x06\x07' + b'\0' * 16 + b'PK\x05\x06' + b'\0' * 18,
    })
    # 两项指着同一处、互相压着
    two = make_zip([('a', b'first entry data', REG, ST), ('b', b'second', REG, ST)])
    cd2 = two.index(b'PK\x01\x02')
    second = two.index(b'PK\x01\x02', cd2 + 4)
    docs.update({
        'two': two, 'overlap-second-points-to-first': two[:second + 42] + struct.pack('<L', 0) + two[second + 46:],
        'overlap-first-runs-into-second': two[:cd2 + 20] + struct.pack('<L', 60) + two[cd2 + 24:],
        'entries-out-of-order': None,
    })
    docs['entries-out-of-order'] = two[:cd2] + two[second:len(two) - 22] + two[cd2:second] + two[-22:]
    return {k: v for k, v in docs.items() if v is not None}


ZIP_TOKENS = [b'PK\x03\x04', b'PK\x01\x02', b'PK\x05\x06', b'PK\x06\x06', b'PK\x06\x07', b'\x00', b'\xff', b'\xff\xff\xff\xff', b'\x00\x00\x00\x00', b'\x08\x00', b'\x00\x08', b'\x01\x00',
              b'\x75\x70', b'/', b'..', b'A.app', b'\x14\x00']


def zip_vectors(rng, fuzz):
    docs = zip_docs()
    rows = [[name, b64(data), zip_outcome(data)] for name, data in docs.items()]
    seeds = [docs[k] for k in ('plain', 'one', 'two', 'zip64', 'unicode-names', 'comment', 'extra-unicode-path', 'zip64-extra-all-three')]
    seen = set(docs.values())
    n = 0
    while n < fuzz:
        d = mutate(rng, rng.choice(seeds), ZIP_TOKENS)
        if d in seen:
            continue
        seen.add(d)
        rows.append(['fuzz-%d' % n, b64(d), zip_outcome(d)])
        n += 1
    return rows


# --- 小函数 ----------------------------------------------------------------------------------------------------------
def casefold_vectors():
    words = ['ChatGPT.app/Contents/MacOS/ChatGPT', 'Straße', 'İstanbul', 'ǅ', 'ǆ', 'ΣΑΣ', 'ς', 'ﬁ', 'ẞ', 'Ω', 'K', 'Å', '中文', '', 'é', 'e\u0301', 'É', '한', 'ﬃ', 'ŉ', 'ǰ', 'ΐ', 'և', 'ẖ', 'ᾀ', 'ᾈ', 'ῼ',
             'Ⅷ', 'Ⓐ', 'Ａ', '𐐀', '𞤀', 'Ꭰ', 'ᏸ', 'ꭰ', 'ſ', 'µ', 'ͅ', 'ẛ', '\u1e9e', 'ǭ', 'ö\u0304', 'q\u0307\u0323', '\u0344', '\u0f73', '\ufb2c', '\U0001d15e', 'が', 'ｶﾞ', 'A\u030a', '\u2126', '\u1100\u1161',
             'a/B/ç/Ç', 'ΌΣΟΣ', 'ß', 'ẞ/ss/SS', 'ǅ/ǆ/Ǆ',
             # Unicode 16 以后才有的组合符号夹在中间：CPython 3.14 不认它们，两边的组合符号各排各的
             'a\u0323\u1acf\u0301\u0323', 'a\u0301\u05c8\u0323', 'q\u0307\u1ad0\u0323\u0307', 'e\u0301\u0323\U0001e6e3\u0301\u0323', 'A\u030a\U00010ecb\u0301\u0323É']
    rows = [[w, unicodedata.normalize('NFD', w), unicodedata.normalize('NFD', w).casefold()] for w in words]
    # 整张分解表的摘要：unicode-normalization 库比 CPython 新的那几处要在 Rust 这边记成例外，靠这个核对
    h = hashlib.sha256()
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        d = unicodedata.normalize('NFD', chr(cp))
        if d != chr(cp):
            h.update(('%x:%s;' % (cp, ','.join('%x' % ord(c) for c in d))).encode())
    # 重排用的组合类也要一样：库认、CPython 这一版还不认的组合符号，Python 当成不参与重排的普通字符
    ccc = hashlib.sha256()
    for cp in range(0x110000):
        if not 0xD800 <= cp <= 0xDFFF and unicodedata.combining(chr(cp)):
            ccc.update(('%x:%d;' % (cp, unicodedata.combining(chr(cp)))).encode())
    return {'words': rows, 'nfd_table_sha256': h.hexdigest(), 'ccc_table_sha256': ccc.hexdigest(), 'unicode': unicodedata.unidata_version}


JOINS = [
    ('https://a.example.com/x/y', 'z'), ('https://a.example.com/x/y', '/z'), ('https://a.example.com/x/y', '//b.example.com/z'), ('https://a.example.com/x/y', 'https://c.example.com/'),
    ('https://a.example.com/x/y', 'http://c.example.com/'), ('https://a.example.com/x/y?q=1#f', ''), ('https://a.example.com/x/y?q=1#f', '?r=2'), ('https://a.example.com/x/y?q=1#f', '#g'),
    ('https://a.example.com/x/y?q=1', '?'), ('https://a.example.com/x/y?q=1', '#'), ('https://a.example.com/x/y', '../z'), ('https://a.example.com/x/y', '../../../z'), ('https://a.example.com/x/y/', '../z'),
    ('https://a.example.com/x/y', './z'), ('https://a.example.com/x/y', '.'), ('https://a.example.com/x/y', '..'), ('https://a.example.com/x/y', 'a/./b/../c'), ('https://a.example.com/x/y', 'a//b'),
    ('https://a.example.com/x/y', '/a//b'), ('https://a.example.com', 'z'), ('https://a.example.com', ''), ('https://a.example.com/', '//'), ('https://a.example.com/x', '///z'),
    ('https://a.example.com/x', 'mailto:me@example.com'), ('https://a.example.com/x', 'javascript:alert(1)'), ('https://a.example.com/x', 'HTTPS://B.example.com/Y'), ('https://a.example.com/x', 'https:z'),
    ('https://a.example.com/x', 'https:/z'), ('https://a.example.com/x', 'https:'), ('https://a.example.com/x', ' https://lead.example.com/'), ('https://a.example.com/x', 'https://b.example.com/ trail'),
    ('https://a.example.com/x', 'ht\ttps://tab.example.com/a\nb'), ('https://a.example.com/x', '\x01/z'), ('https://a.example.com/x', '//[::1]/z'), ('https://a.example.com/x', '//[::1/z'),
    ('https://a.example.com/x', '//[1.2.3.4]/z'), ('https://a.example.com/x', '//ex\u2100mple.com/'), ('https://a.example.com/x', 'z?a=1#b'), ('https://a.example.com/x', 'z;p=1'),
    ('https://a.example.com/x;p', 'z'), ('https://a.example.com/x', '//user:pw@b.example.com:8443/z'), ('https://a.example.com/x', '1http://x/'), ('https://a.example.com/x', 'unknown-scheme://b/c'),
    ('https://a.example.com/x', 'ws://b/c'), ('https://a.example.com/x', '/'), ('https://a.example.com/x', '/..'), ('https://a.example.com/x', '/.'), ('https://a.example.com/x/y/z', '..'),
    ('https://a.example.com/x/y/z', '../'), ('https://a.example.com/x/y/z', '../..'), ('https://a.example.com/x/y/z', '../../'), ('https://a.example.com/x/y/z', '中文/路径'),
    ('https://a.example.com/x', 'https://a.example.com/y'), ('https://a.example.com/x', 'https://a.example.com'), ('https://a.example.com/x', 'https://a.example.com?q'),
    ('https://downloads.claude.ai/releases/darwin/arm64/4.2.0/Claude-abcdef0.zip', 'Claude-1234567.zip'), ('https://persistent.oaistatic.com/codex-app-prod/appcast.xml', '/codex-app-prod/ChatGPT-darwin-arm64-2.0.zip'),
    ('https://a.example.com/x', 'b:c'), ('https://a.example.com/x', ':c'), ('https://a.example.com/x', 'a:'), ('https://a.example.com/a/b', 'c/../../../d'), ('https://a.example.com/a/b/', ''),
    ('https://a.example.com/a/b', '?'), ('https://a.example.com/a/b#f', '?q'), ('https://a.example.com/a/b?q#f', '#'), ('https://a.example.com//a//b', 'c'), ('https://a.example.com/a/b', '//'),
]


def urljoin_vectors():
    return [[a, b, outcome(lambda: urljoin(a, b))] for a, b in JOINS]


def main():
    out_dir = sys.argv[1]
    fuzz = None
    if '--fuzz' in sys.argv:
        i = sys.argv.index('--fuzz')
        fuzz = (int(sys.argv[i + 1]), int(sys.argv[i + 2]))
    if fuzz:
        rng = random.Random(fuzz[0])
        doc = {'xml': xml_vectors(rng, fuzz[1])[len(XML_DOCS):], 'zip': [r for r in zip_vectors(rng, fuzz[1] // 4) if r[0].startswith('fuzz-')]}
        path = os.path.join(out_dir, 'clients_fuzz.json')
    else:
        rng = random.Random(20261119)
        doc = {'python': '%d.%d' % sys.version_info[:2], 'xml': xml_vectors(rng, 700), 'zip': zip_vectors(rng, 120), 'casefold': casefold_vectors(), 'urljoin': urljoin_vectors()}
        path = os.path.join(out_dir, 'clients.json')
        write(path, doc)
        from gen_clients_scenarios import functions, scenarios
        doc = {'python': '%d.%d' % sys.version_info[:2]}
        doc.update(functions(DEFINITIONS))
        doc['scenarios'] = scenarios(DEFINITIONS)
        path = os.path.join(out_dir, 'src_clients.json')
    write(path, doc)


def write(path, doc):
    with open(path, 'w', encoding='utf-8') as f:
        # 一类一行（场景是一个一行）：重新生成以后 git diff 看得出是哪一类、哪个场景变了
        f.write('{\n')
        keys = list(doc)
        for k in keys:
            v = doc[k]
            dump = lambda x: json.dumps(x, ensure_ascii=False, separators=(',', ':'))
            if k == 'scenarios':
                f.write('%s:[\n%s\n]' % (json.dumps(k), ',\n'.join(dump(x) for x in v)))
            else:
                f.write('%s:%s' % (json.dumps(k), dump(v)))
            f.write(',\n' if k != keys[-1] else '\n')
        f.write('}\n')
    print('%s：%d 字节，%s' % (path, os.path.getsize(path), '，'.join('%s %d' % (k, len(v)) for k, v in doc.items() if isinstance(v, list))))


if __name__ == '__main__':
    sys.path.insert(0, HERE)
    main()
