#!/bin/bash
# Claude Code 的 SessionStart hook（.claude/settings.json）：会话开始、/clear、续接、压缩上下文之后，告诉 Claude 这份检出现在的样子。
# 别的会话、代理（Claude Code、Codex、Antigravity）可能正在同一份检出里改文件、提交、跑回归或者发布：不是这次会话改的文件别回退、覆盖、提交；
# 这份检出里的 release.sh 在跑时别改文件（它在回归跑完后才拷 cca，中途改的会进一个不含它的版本）。
# 打到 stdout 的会放进 Claude 的上下文，没什么要提醒的时候只有一行。只读：不改任何文件
ROOT=${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}
cd "$ROOT" 2>/dev/null || exit 0
src=$(sed -n 's/.*"source" *: *"\([^"]*\)".*/\1/p' | head -n 1)   # startup、resume、clear、compact

br=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
head=$(git log -1 --format='%h %s' 2>/dev/null)
ver=$(sed -n 's/^VERSION="\([^"]*\)"$/\1/p' cca 2>/dev/null | head -n 1)
up=
set -- $(git rev-list --left-right --count '@{upstream}...HEAD' 2>/dev/null)
[ "${2:-0}" -gt 0 ] 2>/dev/null && up="，比上游多 ${2} 个提交没推送"
echo "这份检出：${br:-?} 分支，HEAD ${head:-读不到}；cca 的 VERSION 是 ${ver:-读不到}${up}（.claude/hooks/tree-status.sh）"

st=$(git status --short 2>/dev/null)
if [ -n "$st" ]; then
	n=$(printf '%s\n' "$st" | grep -c '')
	case "$src" in
	resume | compact) echo "现在有 ${n} 处未提交的改动。不是这次会话改的（别的会话、代理或者维护者的）别回退、覆盖、提交，拿不准是谁改的先问：" ;;
	*) echo "会话开始前就有 ${n} 处未提交的改动，不是这次会话改的（别的会话、代理或者维护者的）：别回退、覆盖、提交它们，要改同一个文件先问：" ;;
	esac
	printf '%s\n' "$st" | head -n 15
	[ "$n" -le 15 ] || echo "……还有 $((n - 15)) 处，看 git status"
fi

# 正在跑的回归、发布、Linux 测试：只报最外面那一层（子 shell 的命令行和它一样）；bash 把脚本开在 fd 255 上，照它认是不是这份检出里的
runs=$(ps -axo pid=,ppid=,etime=,command= 2>/dev/null | awk '
	{ s = ""
	  if ($4 ~ /(^|\/)(release|regress|linux)\.sh$/) s = $4
	  else if ($4 ~ /(^|\/)(bash|sh|dash|zsh)$/) { for (i = 5; i <= NF; i++) if ($i !~ /^-/) { s = $i; break } }
	  if (s ~ /(^|\/)(release|regress|linux)\.sh$/) hit[$1] = $2 " " $3 " " s }
	END { for (p in hit) { split(hit[p], v, " "); if (!(v[1] in hit)) print p, v[2], v[3] } }')
if [ -n "$runs" ]; then
	here=
	while read -r p et nm; do
		sp=$(lsof -a -p "$p" -d 255 -Fn 2>/dev/null | sed -n 's/^n//p' | head -n 1)
		case "$sp" in
		"$ROOT"/*) w="这份检出里的，"; here=1 ;;
		'') w=; here=1 ;;
		*) w="在别的目录 ${sp%/*}，" ;;
		esac
		echo "正在跑：${nm##*/}（${w}pid ${p}，已运行 ${et}）"
	done <<EOF
$runs
EOF
	[ -z "$here" ] || echo "这份检出里的回归、发布跑完之前：别改它们要测、要发的文件，也别另起一遍回归"
fi

# 同一份检出里别的 Claude Code、Codex、Antigravity（agy）：工作目录在这里、又不是这个 hook 的上级进程（那是这次会话自己）
anc=" $$ "
p=$PPID
for _ in 1 2 3 4 5 6 7 8 9 10; do
	case "$p" in '' | 0 | 1) break ;; esac
	anc="${anc}${p} "
	p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')
done
others=
for pid in $(lsof -d cwd -Fpn 2>/dev/null | awk -v root="$ROOT" '/^p/ { pid = substr($0, 2) } /^n/ { d = substr($0, 2); if (d == root || index(d, root "/") == 1) print pid }'); do
	case "$anc" in *" ${pid} "*) continue ;; esac
	c=$(ps -o comm= -p "$pid" 2>/dev/null)
	c=${c##*/}
	case "$c" in
	claude | codex | codex-* | agy) ;;
	[0-9]*.[0-9]*.[0-9]*) c=claude ;;   # Claude Code 的程序文件按版本号命名（~/.local/share/claude/versions/2.1.332）
	*) continue ;;
	esac
	others="${others}${c}（pid ${pid}，已运行 $(ps -o etime= -p "$pid" 2>/dev/null | tr -d ' ')） "
done
others=${others% }
[ -z "$others" ] || echo "同一份检出里还有别的会话：${others}——改文件、提交、换分支之前先看 git status、git log，别和它们改同一个文件"

TB=${CCA_TEST_BIN:-$ROOT/cli/target/kit/cca-test}
[ -x "$TB" ] || echo "tests/ 的 cca-test 还没编（cd cli && cargo build --profile kit -p cca-tests）：改完文件后的快速检查现在只做 bash -n、rustfmt 和 CLAUDE.md 的大小"
exit 0
