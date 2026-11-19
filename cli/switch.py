#!/usr/bin/env python3
"""把 cca 里内嵌的 Python 换成原生的辅助程序（cli/ 下的 Rust 程序）：一次性的改写，切换那天在分支上跑一遍、把结果提交成新的 cca。

    python3 -I cli/switch.py cca 输出文件

做的事（每一处都先核对原文在不在、是不是只有一处：主干的 cca 变了、对不上的地方会报出来，不会悄悄漏改）：
1. 去掉 PYTHON 的查找、PY_SRC（内嵌的 Python 源码）、PY_BOOT（编译缓存）和打包传参的那一段，换成辅助程序在哪、怎么解出来
   （HELPER；发布的 cca 把四种机器的辅助程序接在文件最后，见 cli/pack.py）；`py` 和别处用的 PYRUN 改成直接运行它
2. bash 里零散内嵌的几小段 Python 换成辅助程序的 sh-… 子命令（cli/src/cmd/shell.rs）
3. 程序坞的后台小 App 不再传 python3 的路径
4. 安装时不再查 python3，改查解辅助程序要用的 base64

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


def sub(old, new, what, count=1):
    """原文 old 换成 new：old 必须正好出现 count 次"""
    global s
    must(s.count(old) == count, '%s：原文出现了 %d 次，应该是 %d 次' % (what, s.count(old), count))
    s = s.replace(old, new)
    DONE.append(what)


def cut(start, end, new, what):
    """从 start 开头到 end 结尾（含）的那一段换成 new；开头必须正好出现一次，结尾取它后面的第一处"""
    global s
    must(s.count(start) == 1 and s.count(end) >= 1, '%s：开头出现 %d 次、结尾出现 %d 次' % (what, s.count(start), s.count(end)))
    a = s.index(start)
    b = s.find(end, a)
    must(b > a, '%s：结尾在开头前面' % what)
    s = s[:a] + new + s[b + len(end):]
    DONE.append(what)


# --- 1. 辅助程序在哪 -------------------------------------------------------------------------------------------------
cut('PYTHON="${CCA_PYTHON:-$(command -v python3 2>/dev/null)}"\n', '\t\tunset _p\n\tfi\nfi\n', '''\
# cca 的缓存目录（辅助程序解出来放在这，程序坞整理的排队记录也在这）：macOS 上是这个用户自己的临时目录（/var/folders/…/T/，
# 别人进不去；TMPDIR 不是它时问 getconf）；Linux 上是 ~/.cache（重启、退出登录以后还在，不占内存盘）
case "${TMPDIR:-}:$OSTYPE" in
	/var/folders/*) CACHE_DIR="${TMPDIR%/}" ;;
	*:darwin*) CACHE_DIR=$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null); CACHE_DIR="${CACHE_DIR%/}" ;;
	*) CACHE_DIR="" ;;
esac
[ -n "$CACHE_DIR" ] || CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}"
''', 'PYTHON 的查找、缓存目录')

HELPER_BOOT = r'''# ---------------------------------------------------------------------------
# 辅助程序（cca-core，Rust 写的，源码在仓库的 cli/）：配置/账号表、共享链接、.claude.json 三方合并、出口检测解析……
# 原来是内嵌的 Python（每次起一个 Python 进程）；子命令、参数、输出都没变（py 子命令 参数…）。
# 发布的 cca 把四种机器的辅助程序（gzip + base64）接在文件最后、exit 的后面：bash 读到 exit 就不往下读了。第一次用时把这台
# 机器的那一份解到 CACHE_DIR/cca-helper（只有自己能读写），文件名就是它的 SHA-256：换了版本就是另一个文件；校验对了、
# 试着运行过才改名换上。以后每次只看一眼“文件在不在、是不是我的”，不起任何进程。
# 仓库里的 cca 没接辅助程序（HELPER_SUMS 空着）：用 cli/ 里编出来的那一份（cd cli && cargo build --release）。
# CCA_HELPER：指定用哪一份（测试用）。
# ---------------------------------------------------------------------------
HELPER_SUMS=""   # 发布时由 cli/pack.py 填上：「目标=SHA-256」，空格隔开
helper_find() {
	local t sum d
	if [ -n "${CCA_HELPER:-}" ]; then HELPER="$CCA_HELPER"; return 0; fi
	if [ -z "$HELPER_SUMS" ]; then
		HELPER="${SELF%/*}/cli/target/release/cca-core"
		[ -x "$HELPER" ] && return 0
		printf '%s\n' "cca: 这一份 cca 是仓库里的源码，没带辅助程序：先编出来（cd ${SELF%/*}/cli && cargo build --release）" >&2
		return 1
	fi
	t="$HOSTTYPE"
	case "$t" in arm64|aarch64|x86_64|amd64) ;; *) t=$(uname -m 2>/dev/null) ;; esac   # 32 位的 bash 跑在 64 位的系统上：照系统的
	case "$t" in arm64|aarch64) t=aarch64 ;; x86_64|amd64) t=x86_64 ;; *) t="" ;; esac
	case "$OSTYPE" in darwin*) t="$t-apple-darwin" ;; linux*) t="$t-unknown-linux-musl" ;; *) t="" ;; esac
	sum=""
	case " $HELPER_SUMS " in *" $t="*) sum="${HELPER_SUMS#*$t=}"; sum="${sum%% *}" ;; esac
	if [ -z "$sum" ]; then
		printf '%s\n' "cca: 没有这种机器用的辅助程序（${HOSTTYPE} ${OSTYPE}）：cca 支持 Intel / Apple 芯片的 Mac，x86_64 / arm64 的 Linux" >&2
		return 1
	fi
	d="$CACHE_DIR/cca-helper"
	HELPER="$d/$sum"
	[ -x "$HELPER" ] && [ -O "$HELPER" ] && [ -O "$d" ] && [ ! -L "$d" ] && return 0
	helper_unpack "$t" "$sum" "$d"
}
helper_unpack() {   # helper_unpack 目标 SHA-256 目录：第一次用（或者缓存目录被清过）时解出来。目录不是自己的、是链接就不用
	local t="$1" sum="$2" d="$3" tmp got c miss=""
	for c in sed base64 gunzip; do command -v "$c" >/dev/null 2>&1 || miss="${miss:+$miss }$c"; done
	[ -z "$miss" ] || { printf '%s\n' "cca: 缺少命令 ${miss// /、}（解出辅助程序要用），先装上" >&2; return 1; }
	{ [ -d "$d" ] || mkdir -p -m 700 "$d"; } 2>/dev/null
	if [ ! -d "$d" ] || [ -L "$d" ] || [ ! -O "$d" ] || ! chmod 700 "$d" 2>/dev/null; then
		printf '%s\n' "cca: 缓存目录 ${d} 用不了（建不出来、不是自己的，或者是个链接）" >&2
		return 1
	fi
	tmp="$d/$sum.$$"
	case "$OSTYPE" in darwin*) c=-D ;; *) c=-d ;; esac   # macOS 12 以前的 base64 只认 -D
	sed -n "/^#CCA-HELPER ${t}\$/,/^CCA-HELPER-END\$/p" "$SELF" | sed '1,2d;$d' | base64 "$c" 2>/dev/null | gunzip -c > "$tmp" 2>/dev/null
	case "$OSTYPE" in darwin*) got=$(shasum -a 256 "$tmp" 2>/dev/null) ;; *) got=$(sha256sum "$tmp" 2>/dev/null) ;; esac
	if [ "${got%% *}" != "$sum" ]; then
		rm -f "$tmp"
		printf '%s\n' "cca: 解不出辅助程序（${SELF} 不完整，或者 ${d} 写不进去）：重新安装 cca，见 ${UPDATE_REPO}" >&2
		return 1
	fi
	# 试着运行一次：目录所在的盘不许执行程序（noexec）、系统太旧时，在这里说清楚，不等到后面每一步都出错
	if ! chmod 700 "$tmp" || [ "$("$tmp" sh-re-escape ok 2>/dev/null)" != ok ]; then
		rm -f "$tmp"
		printf '%s\n' "cca: 辅助程序在这台机器上运行不了（${d} 所在的盘不许执行程序？系统太旧？）" >&2
		return 1
	fi
	mv -f "$tmp" "$HELPER" || { rm -f "$tmp"; return 1; }
	# 顺手删掉 7 天前解出来的（旧版本的，还有没解完的临时文件）
	find "$d" -type f ! -name "$sum" -mtime +7 -exec rm -f {} + 2>/dev/null
	return 0
}
helper_find || exit 1
'''
cut("# ---------------------------------------------------------------------------\n# Python 辅助：配置/账号表、共享链接、.claude.json 三方合并、出口检测解析\n",
    'unset PY_PACKED\n', HELPER_BOOT, 'PY_SRC、PY_BOOT、打包传参')
# 运行辅助程序的地方：原来是 "${PYRUN[@]}"（python3 -I -X utf8 -c PY_BOOT 缓存目录 打包的源码）
n = s.count('"${PYRUN[@]}"')
must(n >= 10, '"${PYRUN[@]}" 只出现了 %d 次' % n)
s = s.replace('"${PYRUN[@]}"', '"$HELPER"')
must('PYRUN' not in s, '还有别的写法的 PYRUN')
DONE.append('PYRUN（%d 处）' % n)
# 注释里提到 PY_SRC 的地方（“PY_SRC 的 xxx 是同一份”）：改口叫辅助程序
mentions = s.count('PY_SRC')
s = s.replace('PY_SRC 里的', '辅助程序里的').replace('PY_SRC 的', '辅助程序的')
must('PY_SRC' not in s and 'PY_BOOT' not in s and 'PY_PACKED' not in s, '还有 PY_SRC / PY_BOOT / PY_PACKED 的引用')
DONE.append('注释里的 PY_SRC（%d 处）' % mentions)

# --- 2. bash 里零散内嵌的几小段 Python --------------------------------------------------------------------------------
cut(''' | "$PYTHON" -I -X utf8 -c '
import json, sys
try:
    d = json.load(sys.stdin)''', '''print((d.get("email") or "") if d.get("loggedIn") else "")')''', ' | "$HELPER" sh-auth-email)', 'claude_email')
cut('''"$PYTHON" -I -X utf8 -c 'import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.pop("oauthAccount",None);''', '''"$ACC_HOME/.claude.json" 2>/dev/null''',
    '"$HELPER" sh-oauth-drop "$ACC_HOME/.claude.json" 2>/dev/null', 'claude_wipe')
sub('''"$PYTHON" -I -c 'import fcntl, sys
try:
    fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)
except OSError:
    sys.exit(1)\'''', '"$HELPER" sh-flock 9', 'chrome_launch_lock / app_launch_lock', count=2)
sub('''pattern=$("$PYTHON" -I -c 'import re, sys; print(re.escape(sys.argv[1]))' "$2") || return 2''', 'pattern=$("$HELPER" sh-re-escape "$2") || return 2', 'app_pid')
cut('''ps -axo pid=,ppid= | "$PYTHON" -I -c 'import sys
root = int(sys.argv[1])''', '''print(",".join(map(str, sorted(pids))))' "$1"''', 'ps -axo pid=,ppid= | "$HELPER" sh-ps-tree "$1"', 'app_process_tree')
cut('''-Fn 2>/dev/null | "$PYTHON" -I -c 'import sys
prefix = "n" + sys.argv[1] + "/"''', '''else 1)' "$dir"''', '-Fn 2>/dev/null | "$HELPER" sh-lsof-under "$dir"', 'app_profile_in_use')
cut('''printf '%s\\n' "$out" | "$PYTHON" -I -c 'import json,sys; d=json.load(sys.stdin); print("客户端自动检查："''', '''if c.get("status") != "missing"]\'''',
    '''printf '%s\\n' "$out" | "$HELPER" sh-clients-text''', 'cmd_clients')

# --- 3. 程序坞的后台小 App -------------------------------------------------------------------------------------------
sub('# 小 App 里是一个很小的 Python 等那个实例退出（kqueue，不轮询，十来 MB），退出了再换成 cca _dock-tidy App 去整理。\n',
    '# 小 App 里是辅助程序等那个实例退出（kqueue，不轮询，1 MB 左右），退出了再换成 cca _dock-tidy App 去整理。\n', '程序坞小 App 的说明')
sub('DOCK_DIR="${PYC_DIR:+${PYC_DIR%/cca-py}/cca-dock}"\n', 'DOCK_DIR="${CACHE_DIR:+$CACHE_DIR/cca-dock}"\n', 'DOCK_DIR')
sub('''[ "$#" -eq 4 ] || exit 0
export CCA_PYTHON="$2"   # 和起它的 cca 用同一个 python3（这里的 PATH 是 launchd 的，找到的不一定是那一个）
exec "$1" _dock-wait "$3" "$4" </dev/null >/dev/null 2>&1\'''', '''[ "$#" -eq 3 ] || exit 0
exec "$1" _dock-wait "$2" "$3" </dev/null >/dev/null 2>&1\'''', 'DOCK_APP_SH')
cut("DOCK_WAIT='import fcntl, os, select, sys\n", "    os.execv(argv[0], argv)\nexcept OSError:\n    pass'\n", '', 'DOCK_WAIT')
sub('''--args "$SELF" "$PYTHON" "$1" "$2" </dev/null''', '''--args "$SELF" "$1" "$2" </dev/null''', 'dock_hand')
sub('''exec "$PYTHON" -I -c "$DOCK_WAIT" "$1" "$DOCK_DIR" "$SELF" _dock-tidy "${2:-}"''', '''exec "$HELPER" sh-dock-wait "$1" "$DOCK_DIR" "$SELF" _dock-tidy "${2:-}"''', 'cmd_dock_wait')

# --- 4. 安装时不再查 python3，改查 base64 -----------------------------------------------------------------------------
cut("\t# macOS 的 /usr/bin/python3 只是个占位，没装命令行工具时一运行就弹安装窗口\n", '是 ${v:-更早的版本}）"\n', '', 'need_deps 里的 python3')
sub('''	[ -n "$PYTHON" ] || die "需要 python3（3.8 或更新）$(pkg_hint python3)"\n''', '', '启动时查 python3')
sub('need_deps() {   # 安装时查依赖：install.sh 下载之前查的是同一份（命令、Python 和 OpenSSH 的版本），两边一起改',
    'need_deps() {   # 安装时查依赖：install.sh 下载之前查的是同一份（命令和 OpenSSH 的版本），两边一起改', 'need_deps 的说明')
sub('	for c in ssh ssh-keygen nc curl perl gunzip find; do command -v "$c" >/dev/null 2>&1 || miss="${miss:+$miss }$c"; done\n',
    '	for c in ssh ssh-keygen nc curl perl gunzip base64 find; do command -v "$c" >/dev/null 2>&1 || miss="${miss:+$miss }$c"; done\n', 'need_deps 的命令')
sub('			pacman:python3) p=python ;;\n			*:gunzip) p=gzip ;;\n', '			*:gunzip) p=gzip ;;\n			*:base64) p=coreutils ;;\n', 'pkg_hint')
# 发布的 cca 接着四种机器的辅助程序，从 0.9 MB 变成约 10 MB：下载的时限照界面 App、内核那两处（300 秒）
sub('''	if retry3 curl -fsSL -m 120 -o "$tmp/cca" "$UPDATE_REPO/releases/download/v$v/cca" 2>/dev/null &&''',
    '''	if retry3 curl -fsSL -m 300 -o "$tmp/cca" "$UPDATE_REPO/releases/download/v$v/cca" 2>/dev/null &&''', '下载新版本的时限')

# cca restore：这个版本解出来的辅助程序也删掉（别的版本的、别人指定的不动）。放在最后：前面每一步都还要用它
sub('''	rm -rf "${CCA_HOME:?}"
	info "已还原。''', '''	rm -rf "${CCA_HOME:?}"
	case "$HELPER" in "$CACHE_DIR/cca-helper/"*) rm -f "$HELPER"; rmdir "$CACHE_DIR/cca-helper" 2>/dev/null ;; esac   # 解出来的辅助程序（缓存目录里这个版本的那一份）
	info "已还原。''', 'restore 删掉解出来的辅助程序')

# --- 5. 注释里说到 Python 的地方 --------------------------------------------------------------------------------------
sub('（如 zh_CN.GBK、GB18030）时，Python 等会按 GBK 编码输出，', '（如 zh_CN.GBK、GB18030）时，有的命令会按 GBK 编码输出，', '注释：locale')
sub('用 tool 步骤名 调用；Python 那边按 CCA_TOOL 选同一个工具', '用 tool 步骤名 调用；辅助程序那边按 CCA_TOOL 选同一个工具', '注释：tool')
sub('（同一个文件、大小和修改时间都没变）就不再起 python', '（同一个文件、大小和修改时间都没变）就不再起辅助程序', '注释：load_router')
sub('# flock 锁留在当前 shell 的 fd 9；Python 退出后仍持有，', '# flock 锁留在当前 shell 的 fd 9；辅助程序退出后仍持有，', '注释：启动锁')
sub('（连同它们起的 python、curl；', '（连同它们起的辅助程序、curl；', '注释：停掉后台的查询')

left = [m.group(0) for m in re.finditer(r'.*(\$PYTHON|\bCCA_PYTHON\b|\bPYC_DIR\b|[Pp]ython).*', s)]
left = [x for x in left if '原来是内嵌的 Python' not in x]
must(not left, '还有提到 Python / PYC_DIR 的地方：\n  ' + '\n  '.join(x[:160] for x in left[:10]))
# 发布的 cca 在 main 后面接辅助程序：bash 读到 exit 就停，后面的不会被当成脚本
must(s.endswith('\nmain "$@"\n'), 'cca 的结尾不是 main "$@"')
s += 'exit\n'
DONE.append('结尾加 exit')
io.open(out_path, 'w', encoding='utf-8').write(s)
print('switch.py：改了 %d 处；%s → %s（%d 行）' % (len(DONE), src_path, out_path, s.count('\n')))
