#!/usr/bin/env python3
"""把 cca 里内嵌的 Python 换成原生的辅助程序（cli/ 下的 Rust 程序）：一次性的改写，切换那天在分支上跑一遍、把结果提交成新的 cca。

    python3 -I cli/switch.py cca 输出文件

做的事（每一处都先核对原文在不在、是不是只有一处：主干的 cca 变了、对不上的地方会报出来，不会悄悄漏改）：
1. 去掉 PYTHON 的查找、PY_SRC（内嵌的 Python 源码）、PY_BOOT（编译缓存）和打包传参的那一段，换成辅助程序在哪、怎么解出来
   （HELPER；发布的 cca 把四个平台的辅助程序接在文件最后，见 cli/pack.py）；`py` 和别处用的 PYRUN 改成直接运行它
2. bash 里零散内嵌的几小段 Python 换成辅助程序的 sh-… 子命令（cli/src/cmd/shell.rs）
3. 程序坞的后台小 App 不再传 python3 的路径
4. 安装时不再查 python3

主干还在往 cca 里加功能的时候，这个脚本可以反复跑：把主干最新的 cca 拿来再改写一遍，和分支上的比。"""
import io
import re
import sys

src_path, out_path = sys.argv[1], sys.argv[2]
s = io.open(src_path, encoding='utf-8').read()
DONE = []


def must(cond, what):
    if not cond:
        sys.exit('switch.py：%s（cca 这一处和写这个脚本时不一样了，照新的原文改脚本）' % what)


def sub(old, new, what):
    """原文 old 换成 new：old 必须正好出现一次"""
    global s
    must(s.count(old) == 1, '%s：原文出现了 %d 次' % (what, s.count(old)))
    s = s.replace(old, new)
    DONE.append(what)


def cut(start, end, new, what, keep_end=False):
    """从 start 开头到 end 结尾（含）的那一段换成 new；两个标记都必须正好出现一次、先后对"""
    global s
    must(s.count(start) == 1 and s.count(end) >= 1, '%s：开头出现 %d 次、结尾出现 %d 次' % (what, s.count(start), s.count(end)))
    a = s.index(start)
    b = s.index(end, a)
    must(b > a, '%s：结尾在开头前面' % what)
    s = s[:a] + new + s[b if keep_end else b + len(end):]
    DONE.append(what)


# --- 1. 辅助程序在哪 -------------------------------------------------------------------------------------------------
sub('PYTHON="${CCA_PYTHON:-$(command -v python3 2>/dev/null)}"\n'
    '# 内嵌 Python 的缓存目录（PY_BOOT）：这个用户自己的临时目录（/var/folders/…/T/，别人进不去；TMPDIR 不是它时问 getconf）。\n'
    '# Linux 没有它：用登录会话的 XDG_RUNTIME_DIR（tmpfs，只有自己能进），没有就 ~/.cache（PY_BOOT 照样查目录是不是自己的、别人进不进得去）\n',
    '# cca 自己的缓存目录（辅助程序解出来放在这，程序坞整理的排队记录也在它旁边）：这个用户自己的临时目录（/var/folders/…/T/，\n'
    '# 别人进不去；TMPDIR 不是它时问 getconf）。Linux 没有它：用登录会话的 XDG_RUNTIME_DIR（tmpfs，只有自己能进），没有就 ~/.cache\n',
    'PYTHON 的查找')
cut('# /usr/bin/python3 是命令行工具的转发程序：每次先找开发者目录',
    '\t\tunset _p\n\tfi\nfi\n', '', '/usr/bin/python3 换成真正的 python3 那一段')

HELPER_BOOT = r'''# ---------------------------------------------------------------------------
# 辅助程序（cca-core，Rust，源码在仓库的 cli/）：配置/账号表、共享链接、.claude.json 三方合并、出口检测解析……
# 原来这些是内嵌的 Python；子命令、参数、输出都没变（py 子命令 参数…）。
# 发布的 cca 把四个平台的辅助程序（gzip + base64）接在文件最后、exit 的后面：bash 运行时读不到那里。第一次用时把这台机器的
# 那一份解到 HELPER_DIR（上面那个缓存目录旁边，只有自己能读写），文件名带着它的 SHA-256：换了版本就是另一个文件，校验过才改名
# 换上；以后每次只多一次“文件在不在、是不是我的”的判断，不起任何进程。HELPER_SUMS 空着是开发模式（仓库里的 cca）：用 cli/ 里
# 刚编的那一份（cargo build --release），或者 CCA_HELPER 指的。
# ---------------------------------------------------------------------------
HELPER_SUMS=""   # 发布时由 cli/pack.py 填上：一行一个「目标 SHA-256」
helper_find() {
	local os arch t sum d tmp got line
	if [ -z "$HELPER_SUMS" ]; then
		HELPER="${CCA_HELPER:-${SELF%/*}/cli/target/release/cca-core}"
		[ -x "$HELPER" ] || { printf '%s\n' "cca: 开发模式下要先编出辅助程序：cd ${SELF%/*}/cli && cargo build --release（或者设 CCA_HELPER）" >&2; return 1; }
		return 0
	fi
	case "$OSTYPE" in darwin*) os=apple-darwin ;; linux*) os=unknown-linux-musl ;; *) os="" ;; esac
	case "$HOSTTYPE" in arm64|aarch64) arch=aarch64 ;; x86_64|amd64) arch=x86_64 ;; *) arch="" ;; esac
	t="$arch-$os"; sum=""
	while read -r line; do [ "${line%% *}" = "$t" ] && sum="${line#* }"; done <<EOF
$HELPER_SUMS
EOF
	[ -n "$arch" ] && [ -n "$os" ] && [ -n "$sum" ] || { printf '%s\n' "cca: 没有这种机器（${HOSTTYPE} ${OSTYPE}）用的辅助程序" >&2; return 1; }
	d="${PYC_DIR%/cca-py}/cca-helper"
	HELPER="$d/$sum"
	[ -x "$HELPER" ] && [ -O "$HELPER" ] && [ -O "$d" ] && [ ! -L "$d" ] && return 0
	# 第一次（或者缓存目录被清了）：解出来、校验、改名换上。目录不是自己的、是链接就不用它
	{ [ -d "$d" ] || mkdir -p -m 700 "$d"; } 2>/dev/null
	[ -d "$d" ] && [ ! -L "$d" ] && [ -O "$d" ] || { printf '%s\n' "cca: 缓存目录 ${d} 用不了（不是自己的，或者是个链接）" >&2; return 1; }
	tmp="$HELPER.$$"
	sed -n "/^#CCA-HELPER ${t}\$/,/^CCA-HELPER-END\$/p" "$SELF" | sed '1,2d;$d' | { base64 -d 2>/dev/null || base64 -D; } | gzip -dc > "$tmp" 2>/dev/null
	if command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$tmp" 2>/dev/null); else got=$(sha256sum "$tmp" 2>/dev/null); fi
	if [ "${got%% *}" != "$sum" ]; then
		rm -f "$tmp"
		printf '%s\n' "cca: 解不出辅助程序（这个 cca 文件不完整？重新安装：见 https://github.com/rivia7/claude-accounts）" >&2
		return 1
	fi
	chmod 700 "$tmp" && mv -f "$tmp" "$HELPER" || { rm -f "$tmp"; return 1; }
	# 顺手删掉 30 天没用过的旧版本
	find "$d" -type f -name '[0-9a-f]*' ! -name "$sum" -atime +30 -delete 2>/dev/null
	return 0
}
helper_find || exit 1
PYRUN=("$HELPER")
'''
cut("# ---------------------------------------------------------------------------\n# Python 辅助：配置/账号表、共享链接、.claude.json 三方合并、出口检测解析\n",
    'unset PY_PACKED\n', HELPER_BOOT, 'PY_SRC、PY_BOOT、打包传参')
# 注释里提到 PY_SRC 的地方（“PY_SRC 的 xxx 是同一份”）：改口叫辅助程序
mentions = s.count('PY_SRC')
s = s.replace('PY_SRC 里的', '辅助程序里的').replace('PY_SRC 的', '辅助程序的')
must('PY_SRC' not in s and 'PY_BOOT' not in s and 'PY_PACKED' not in s, '还有 PY_SRC / PY_BOOT / PY_PACKED 的引用')
DONE.append('注释里的 PY_SRC（%d 处）' % mentions)

# --- 2. bash 里零散内嵌的几小段 Python --------------------------------------------------------------------------------
cut(''' | "$PYTHON" -I -X utf8 -c '
import json, sys
try:
    d = json.load(sys.stdin)''', '''print((d.get("email") or "") if d.get("loggedIn") else "")')''', ' | "${PYRUN[@]}" sh-auth-email)', 'claude_email')
cut('''"$PYTHON" -I -X utf8 -c 'import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.pop("oauthAccount",None);''', '''"$ACC_HOME/.claude.json" 2>/dev/null''',
    '"${PYRUN[@]}" sh-oauth-drop "$ACC_HOME/.claude.json" 2>/dev/null', 'claude_wipe')
FLOCK = '''"$PYTHON" -I -c 'import fcntl, sys
try:
    fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)
except OSError:
    sys.exit(1)\''''
must(s.count(FLOCK) == 2, '启动锁的那一小段应该有两处（浏览器、桌面 App），现在是 %d 处' % s.count(FLOCK))
s = s.replace(FLOCK, '"${PYRUN[@]}" sh-flock 9')
DONE.append('chrome_launch_lock / app_launch_lock')
sub('''pattern=$("$PYTHON" -I -c 'import re, sys; print(re.escape(sys.argv[1]))' "$2") || return 2''', 'pattern=$("${PYRUN[@]}" sh-re-escape "$2") || return 2', 'app_pid')
cut('''ps -axo pid=,ppid= | "$PYTHON" -I -c 'import sys
root = int(sys.argv[1])''', '''print(",".join(map(str, sorted(pids))))' "$1"''', 'ps -axo pid=,ppid= | "${PYRUN[@]}" sh-ps-tree "$1"', 'app_process_tree')
cut('''-Fn 2>/dev/null | "$PYTHON" -I -c 'import sys
prefix = "n" + sys.argv[1] + "/"''', '''else 1)' "$dir"''', '-Fn 2>/dev/null | "${PYRUN[@]}" sh-lsof-under "$dir"', 'app_profile_in_use')
cut('''printf '%s\\n' "$out" | "$PYTHON" -I -c 'import json,sys; d=json.load(sys.stdin); print("客户端自动检查："''', '''if c.get("status") != "missing"]\'''',
    '''printf '%s\\n' "$out" | "${PYRUN[@]}" sh-clients-text''', 'cmd_clients')

# --- 3. 程序坞的后台小 App -------------------------------------------------------------------------------------------
sub('''[ "$#" -eq 4 ] || exit 0
export CCA_PYTHON="$2"   # 和起它的 cca 用同一个 python3（这里的 PATH 是 launchd 的，找到的不一定是那一个）
exec "$1" _dock-wait "$3" "$4" </dev/null >/dev/null 2>&1\'''', '''[ "$#" -eq 3 ] || exit 0
exec "$1" _dock-wait "$2" "$3" </dev/null >/dev/null 2>&1\'''', 'DOCK_APP_SH')
cut("DOCK_WAIT='import fcntl, os, select, sys\n", "    os.execv(argv[0], argv)\nexcept OSError:\n    pass'\n", '', 'DOCK_WAIT')
sub('''--args "$SELF" "$PYTHON" "$1" "$2" </dev/null''', '''--args "$SELF" "$1" "$2" </dev/null''', 'dock_hand')
sub('''exec "$PYTHON" -I -c "$DOCK_WAIT" "$1" "$DOCK_DIR" "$SELF" _dock-tidy "${2:-}"''', '''exec "${PYRUN[@]}" sh-dock-wait "$1" "$DOCK_DIR" "$SELF" _dock-tidy "${2:-}"''', 'cmd_dock_wait')

# --- 4. 安装时不再查 python3 -----------------------------------------------------------------------------------------
cut("\t# macOS 的 /usr/bin/python3 只是个占位，没装命令行工具时一运行就弹安装窗口\n", '是 ${v:-更早的版本}）"\n', '', 'need_deps 里的 python3')
sub('''	[ -n "$PYTHON" ] || die "需要 python3（3.8 或更新）$(pkg_hint python3)"\n''', '', '启动时查 python3')
sub('need_deps() {   # 安装时查依赖：install.sh 下载之前查的是同一份（命令、Python 和 OpenSSH 的版本），两边一起改',
    'need_deps() {   # 安装时查依赖：install.sh 下载之前查的是同一份（命令和 OpenSSH 的版本），两边一起改', 'need_deps 的说明')

left = [m.group(0) for m in re.finditer(r'.*(\$PYTHON|\bCCA_PYTHON\b).*', s)]
must(not left, '还有用到 PYTHON 的地方：\n  ' + '\n  '.join(x[:160] for x in left[:10]))
# 发布的 cca 在 main 后面接辅助程序：bash 读到 exit 就停，后面的不会被当成脚本
must(s.endswith('\nmain "$@"\n'), 'cca 的结尾不是 main "$@"')
s += 'exit\n'
DONE.append('结尾加 exit')
io.open(out_path, 'w', encoding='utf-8').write(s)
print('switch.py：改了 %d 处；%s → %s（%d 行）' % (len(DONE), src_path, out_path, s.count('\n')))
