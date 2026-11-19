#!/usr/bin/env python3
"""生成 src/py/uni_tables.rs：字符宽度、可打印、空白、数字这几张表，取自运行它的这个 Python 的 unicodedata。

    python3 -I cli/tools/gen_unicode.py > cli/src/py/uni_tables.rs

cca 原来的 Python 实现用的是用户机器上那个 Python 的表（各版本的 Unicode 不一样），Rust 这边固定成一份。
换 Unicode 版本时用新的 Python 重新生成，提交生成的文件。"""
import sys
import unicodedata


def ranges(pred):
    out, start = [], None
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF:   # 代理项不是 Rust 的字符，跳过去也不打断区间
            continue
        if pred(chr(cp)):
            if start is None:
                start = cp
            last = cp
        elif start is not None:
            out.append((start, last))
            start = None
    if start is not None:
        out.append((start, last))
    merged = []
    for a, b in out:   # 只隔着代理项的两段并成一段
        if merged and merged[-1][1] == 0xD7FF and a == 0xE000:
            merged[-1] = (merged[-1][0], b)
        else:
            merged.append((a, b))
    return merged


def table(name, doc, pred):
    rs = ranges(pred)
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


print('// 由 cli/tools/gen_unicode.py 生成，不要手改。Unicode %s（CPython %d.%d 的 unicodedata）。' % (
    unicodedata.unidata_version, sys.version_info[0], sys.version_info[1]))
print()
table('WIDE', '`unicodedata.east_asian_width(c) in "WF"`：终端里占两格的字符', lambda c: unicodedata.east_asian_width(c) in 'WF')
table('NONPRINT', '`not c.isprintable()`', lambda c: not c.isprintable())
table('SPACE', '`c.isspace()`：`str.strip()`、`str.split()` 认的空白', lambda c: c.isspace())
table('DIGIT', '`c.isdigit()`', lambda c: c.isdigit())
table('DECIMAL', '`c.isdecimal()`：`int()` 认的数字', lambda c: c.isdecimal())
table('ALNUM', '`c.isalnum()`', lambda c: c.isalnum())
