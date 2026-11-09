#!/usr/bin/env python3
"""回归用：扮演 cca App 的提问会话（macos/PROTOCOL.md 4.6）。

用法：gui_session.py 事件文件 答案表.json [--eof-at 题号] [--stale] [--wrong-field 题号] [--keep] -- 命令…

答案表：{"题号": [第一次问时的答案, 第二次问时的答案…]}，每次的答案是 {"字段 id": "值"}。--keep：没写的字段按页面上现有的值回答（字段多的表单）。
命令在新会话里运行，fd 3 是答案管道（和 App 一样）；问到答案表里没有的题号时关掉管道（cca 按取消处理）。
退出码就是命令的退出码，收到信号时是 128 + 信号。
"""
import fcntl
import json
import os
import subprocess
import sys
import time

events, table_path = sys.argv[1:3]
args = sys.argv[3:]
eof_at = stale = wrong = keep = None
while args and args[0] != '--':
    if args[0] == '--eof-at':
        eof_at = args[1]
        args = args[2:]
    elif args[0] == '--wrong-field':
        wrong = args[1]
        args = args[2:]
    elif args[0] == '--stale':
        stale = True
        args = args[1:]
    elif args[0] == '--keep':
        keep = True
        args = args[1:]
    else:
        sys.exit('不认识的参数：%s' % args[0])
cmd = args[1:]
with open(table_path, encoding='utf-8') as f:
    table = json.load(f)

r0, w = os.pipe()
r = fcntl.fcntl(r0, fcntl.F_DUPFD, 10)   # 读端碰巧是 3 时 dup2(3, 3) 什么也不做，close-on-exec 也清不掉
os.close(r0)
p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, start_new_session=True, close_fds=False, preexec_fn=lambda: os.dup2(r, 3))
os.close(r)
fcntl.fcntl(w, fcntl.F_SETFD, fcntl.FD_CLOEXEC)


def say(text):
    try:
        os.write(w, text.encode('utf-8'))
    except BrokenPipeError:
        pass


last, offset, buf, closed = '', 0, b'', False
used = {}
while True:
    with open(events, 'rb') as f:
        f.seek(offset)
        chunk = f.read()
    offset += len(chunk)
    buf += chunk
    while b'\n' in buf:
        row, buf = buf.split(b'\n', 1)
        e = json.loads(row)
        if e.get('ev') != 'ask' or closed:
            continue
        q = e['q']
        i = used.get(q, 0)
        used[q] = i + 1
        rounds = table.get(q) or []
        if q == eof_at or i >= len(rounds):
            os.close(w)
            closed = True
            continue
        ans = rounds[i]
        if stale and last:
            say(last)   # 上一页的答案又发了一遍（比如点了两下）：序号对不上，要丢掉
        text = ''
        for field in e['fields']:
            if field['kind'] == 'code':   # 只显示，不用回答
                continue
            fid = field['id']
            text += '%d %s %s\n' % (e['seq'], 'wrong' if q == wrong else fid, ans.get(fid, (field.get('value') or field.get('default') or '') if keep else ''))
        say(text)
        last = text
    if p.poll() is not None:
        break
    time.sleep(0.02)
if not closed:
    os.close(w)
sys.exit(p.returncode if p.returncode >= 0 else 128 - p.returncode)
