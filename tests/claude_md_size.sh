#!/bin/bash
# 给 Claude 看的规矩有大小上限（字节）：根目录的 CLAUDE.md 每次会话都整份加载；子目录的 CLAUDE.md 读到那个目录的文件时加载；
# .claude/rules/*.md 读到它开头 paths 里列的文件时加载。上限是 2026-11-26 拆分时的大小再留 1.5 KB 左右：超了就把只跟某一块有关的
# 挪进 .claude/rules/（带 paths）或者 docs/dev/ 对应的那一份，别提高上限。
#   bash tests/claude_md_size.sh [仓库的根]
# 都没超：打出 ok；超了：打出「：文件 N 字节（上限 M）、…」。退出码都是 0（和 cca-test lint 一样，看打出来的那一行）。
# 回归第 11 节、.claude/hooks/quick-lint.sh 都用它
root=${1:-$(cd "$(dirname "$0")/.." && pwd)}
cd "$root" || exit 1
cap() {   # cap 相对路径：这一份的上限
	case "$1" in
	CLAUDE.md) echo 23000 ;;
	tests/CLAUDE.md) echo 18000 ;;
	macos/CLAUDE.md) echo 15000 ;;
	.claude/rules/team.md) echo 11000 ;;
	.claude/rules/remote.md) echo 9000 ;;
	.claude/rules/*) echo 8000 ;;
	*) echo 4000 ;;   # 别的子目录的 CLAUDE.md（cli/、ios/ 的）：指向 docs/dev/ 的那一份，再加这一块的几条规矩
	esac
}
bad=
for f in CLAUDE.md */CLAUDE.md */*/CLAUDE.md .claude/rules/*.md; do
	[ -f "$f" ] || continue
	n=$(wc -c < "$f" | tr -d ' ')
	max=$(cap "$f")
	[ "$n" -le "$max" ] || bad="${bad:+${bad}、}${f} ${n} 字节（上限 ${max}）"
done
if [ -z "$bad" ]; then echo ok; else echo "：${bad}"; fi
