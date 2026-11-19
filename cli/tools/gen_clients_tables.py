#!/usr/bin/env python3
"""生成客户端更新（cli/src/cmd/clients）和它用到的兼容层要的几张表，取自运行它的这个 Python：

    python3 -I cli/tools/gen_clients_tables.py casefold > cli/src/cmd/clients/casefold_table.rs
    python3 -I cli/tools/gen_clients_tables.py xml      > cli/src/py/xml_tables.rs
    python3 -I cli/tools/gen_clients_tables.py cp437    > cli/src/py/zip_cp437.rs

- casefold：`str.casefold()` 会变的字符（安装包里的名字按不分大小写、不分 Unicode 写法查重）
- xml：XML 名字里能用的非 ASCII 字符。不照 XML 标准的附录抄：拿 expat（pyexpat）一个字符一个字符试出来的，要的就是它认哪些
- cp437：zip 里没标 UTF-8 的文件名用的编码（0x80–0xFF 那一半）

换 Python / expat 版本时重新生成，提交生成的文件。"""
import sys
import unicodedata
import xml.etree.ElementTree as ET
from xml.parsers import expat


def emit_ranges(name, doc, rs):
    print('/// %s（%d 段）' % (doc, len(rs)))
    print('pub const %s: &[(u32, u32)] = &[' % name)
    line = '   '
    for a, b in rs:
        item = ' (0x%x, 0x%x),' % (a, b)
        if len(line) + len(item) > 118:
            print(line)
            line = '   '
        line += item
    if line.strip():
        print(line)
    print('];')
    print()


def to_ranges(cps):
    out = []
    for cp in cps:
        if out and out[-1][1] == cp - 1:
            out[-1][1] = cp
        else:
            out.append([cp, cp])
    return [tuple(r) for r in out]


def rust_str(s):
    return '"' + ''.join('\\u{%x}' % ord(c) for c in s) + '"'


def casefold():
    print('// 由 cli/tools/gen_clients_tables.py 生成，不要手改。Unicode %s（CPython %d.%d 的 str.casefold）。' % (
        unicodedata.unidata_version, sys.version_info[0], sys.version_info[1]))
    print()
    rows = []
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        c = chr(cp)
        if c.casefold() != c:
            rows.append((cp, c.casefold()))
    print('/// `c.casefold()` 和 c 不一样的字符：(字符, 变成什么)，按字符排好（%d 项）' % len(rows))
    print('pub const CASEFOLD: &[(u32, &str)] = &[')
    line = '   '
    for cp, folded in rows:
        item = ' (0x%x, %s),' % (cp, rust_str(folded))
        if len(line) + len(item) > 118:
            print(line)
            line = '   '
        line += item
    if line.strip():
        print(line)
    print('];')


def parses(data):
    try:
        ET.fromstring(data)
        return True
    except ET.ParseError:
        return False


def xml():
    print('// 由 cli/tools/gen_clients_tables.py 生成，不要手改。expat %s（CPython %d.%d 的 pyexpat）一个字符一个字符试出来的。' % (
        expat.EXPAT_VERSION.replace('expat_', ''), sys.version_info[0], sys.version_info[1]))
    print()
    start, more = [], []
    for cp in range(0x80, 0x10000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        c = chr(cp).encode('utf-8')
        if parses(b'<' + c + b'/>'):
            start.append(cp)
        elif parses(b'<a' + c + b'/>'):
            more.append(cp)
    # 辅助平面的字符（四个字节的 UTF-8）expat 一概不让进名字：抽查一遍，有例外就别生成了
    for cp in list(range(0x10000, 0x10400)) + list(range(0x1D400, 0x1D800)) + list(range(0x20000, 0x20100)) + [0x1F600, 0x2F800, 0xE0100, 0x10FFFD]:
        c = chr(cp).encode('utf-8')
        if parses(b'<' + c + b'/>') or parses(b'<a' + c + b'/>'):
            sys.exit('expat 让 U+%X 进了名字：表要重新设计' % cp)
    emit_ranges('NAME_START', 'U+0080 以上能当名字开头的字符（UTF-8 的文档；别的编码里 U+0100 以上的也是这张）', to_ranges(start))
    emit_ranges('NAME_MORE', 'U+0080 以上能在名字里、不能开头的字符', to_ranges(more))
    # ISO-8859-1 的文档里 0x80–0xFF 另有一张表（expat 的 latin1tab.h：比上面多认 ª µ º）；UTF-16 的文档里 U+00FF 以下的也照它
    decl = b'<?xml version="1.0" encoding="ISO-8859-1"?>'
    l_start = [b for b in range(0x80, 0x100) if parses(decl + b'<' + bytes([b]) + b'/>')]
    l_more = [b for b in range(0x80, 0x100) if b not in l_start and parses(decl + b'<a' + bytes([b]) + b'/>')]
    for big in (True, False):
        codec = 'utf-16-be' if big else 'utf-16-le'
        w_start = [cp for cp in range(0x80, 0x100) if parses(('<%s/>' % chr(cp)).encode(codec))]
        w_more = [cp for cp in range(0x80, 0x100) if cp not in w_start and parses(('<a%s/>' % chr(cp)).encode(codec))]
        if (w_start, w_more) != (l_start, l_more):
            sys.exit('UTF-16 的文档里 U+00FF 以下的名字字符和 ISO-8859-1 的不一样：表要重新设计')
        for cp in list(range(0x100, 0x400)) + list(range(0x3000, 0x3100)) + [0x4E2D, 0xAC00, 0xFFFD]:
            got = (parses(('<%s/>' % chr(cp)).encode(codec)), parses(('<a%s/>' % chr(cp)).encode(codec)))
            if got != (cp in start, cp in start or cp in more):
                sys.exit('UTF-16 的文档里 U+%X 能不能进名字和 UTF-8 的不一样：表要重新设计' % cp)
    emit_ranges('LATIN1_START', 'ISO-8859-1、UTF-16 的文档里 U+0080–U+00FF 能当名字开头的字符', to_ranges(l_start))
    emit_ranges('LATIN1_MORE', 'ISO-8859-1、UTF-16 的文档里 U+0080–U+00FF 能在名字里、不能开头的字符', to_ranges(l_more))


def cp437():
    print('// 由 cli/tools/gen_clients_tables.py 生成，不要手改。CPython %d.%d 的 cp437 编码。' % (sys.version_info[0], sys.version_info[1]))
    print()
    high = bytes(range(0x80, 0x100)).decode('cp437')
    assert bytes(range(0x80)).decode('cp437') == ''.join(map(chr, range(0x80)))
    print('/// cp437 的 0x80–0xFF 依次是这些字符（0x00–0x7F 和 ASCII 一样）')
    print('pub const CP437_HIGH: [char; 128] = [')
    for i in range(0, 128, 16):
        print('    ' + ' '.join("'\\u{%x}'," % ord(c) for c in high[i:i + 16]))
    print('];')


def newer():
    """casefold.rs 的 NEWER：unicode-normalization 库认、这个 CPython 还不认的字符（带分解的、组合类不是 0 的）。
    库的两张表由单元测试导出：CCA_CLIENTS_UNICODE_DUMP=文件 cargo test folds_like_python，再把那个文件交给这里"""
    import unicodedata
    lib_nfd, lib_ccc = {}, {}
    with open(sys.argv[2]) as f:
        for line in f:
            kind, _, rest = line.strip().partition(' ')
            cp, _, value = rest.partition(':')
            if kind == 'NFD':
                lib_nfd[int(cp, 16)] = value
            elif kind == 'CCC':
                lib_ccc[int(cp, 16)] = int(value)
    odd, extra = [], set()
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:
            continue
        c = chr(cp)
        d = unicodedata.normalize('NFD', c)
        mine = (','.join('%x' % ord(x) for x in d) if d != c else None, unicodedata.combining(c))
        theirs = (lib_nfd.get(cp), lib_ccc.get(cp, 0))
        if mine == theirs:
            continue
        # 只有“库多认了一个字符”这一种不一样是 NEWER 管得了的：Python 这边没分配、不分解、组合类是 0
        if mine == (None, 0) and unicodedata.category(c) == 'Cn':
            extra.add(cp)
        else:
            odd.append('U+%04X：CPython %r，库 %r' % (cp, mine, theirs))
    if odd:
        sys.exit('这些字符两边不一样，又不是库新认的字符，NEWER 管不了：\n' + '\n'.join(odd))
    print('// CPython %d.%d（Unicode %s）不认、库认的 %d 个字符' % (sys.version_info[0], sys.version_info[1], unicodedata.unidata_version, len(extra)))
    print('const NEWER: &[(u32, u32)] = &[%s];' % ', '.join('(0x%x, 0x%x)' % r for r in to_ranges(sorted(extra))))


if __name__ == '__main__':
    {'casefold': casefold, 'xml': xml, 'cp437': cp437, 'newer': newer}[sys.argv[1]]()
