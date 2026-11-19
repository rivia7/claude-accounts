#!/usr/bin/env python3
"""生成 src/cmd/mail/html5.rs：HTML 的字符引用要用的三张表，取自运行它的这个 Python 的 html 模块。

    python3 -I cli/tools/gen_mail_tables.py > cli/src/cmd/mail/html5.rs

邮箱助手从登录邮件的 HTML 正文里取链接，属性值里的字符引用（&amp;、&#38;……）照 Python 的 html.parser 解；
原来用的是用户机器上那个 Python 的表，Rust 这边固定成一份。换 Python 版本后重新生成，提交生成的文件。"""
import html
import html.entities
import sys


def rs_str(s):
    return '"%s"' % ''.join('\\u{%x}' % ord(c) if not (0x20 <= ord(c) < 0x7f) or c in '"\\' else c for c in s)


print('// 由 cli/tools/gen_mail_tables.py 生成，不要手改。CPython %d.%d 的 html.entities.html5、html._invalid_charrefs、' % sys.version_info[:2])
print('// html._invalid_codepoints。')
print()
names = sorted(html.entities.html5)   # 按码位排：Rust 那边二分查找
print('/// `html.entities.html5`：名字（有的带分号、有的不带，都是原样的键）→ 字符（%d 项）' % len(names))
print('pub const HTML5: &[(&str, &str)] = &[')
line = '   '
for n in names:
    item = ' (%s, %s),' % (rs_str(n), rs_str(html.entities.html5[n]))
    if len(line) + len(item) > 128:
        print(line)
        line = '   '
    line += item
if line.strip():
    print(line)
print('];')
print()
print('/// `html._invalid_charrefs`：这些数字的字符引用换成另一个字符（Windows-1252 的那一段、0、回车）')
print('pub const INVALID_CHARREFS: &[(u32, u32)] = &[')
line = '   '
for k, v in sorted(html._invalid_charrefs.items()):
    item = ' (0x%x, 0x%x),' % (k, ord(v))
    if len(line) + len(item) > 128:
        print(line)
        line = '   '
    line += item
if line.strip():
    print(line)
print('];')
print()
print('/// `html._invalid_codepoints`：这些数字的字符引用整个去掉（控制字符、非字符），从小到大')
print('pub const INVALID_CODEPOINTS: &[u32] = &[')
line = '   '
for k in sorted(html._invalid_codepoints):
    item = ' 0x%x,' % k
    if len(line) + len(item) > 128:
        print(line)
        line = '   '
    line += item
if line.strip():
    print(line)
print('];')
