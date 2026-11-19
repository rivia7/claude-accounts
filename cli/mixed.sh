#!/bin/bash
# 移植期间的“混合版”：拿原样的 cca 跑，其中已经移植成 Rust 的子命令由 cca-core 做，没移植的照旧交给 Python。
# 用法：
#   bash cli/mixed.sh regress [regress.sh 的环境变量和参数照常，例如 ONLY=、CKPT=]   整套回归跑在混合版上
#   bash cli/mixed.sh wrap                                                         只打印那个小脚本的路径（自己设 CCA_PYTHON 用）
# 做法：cca 认 CCA_PYTHON；这里造一个小脚本顶替 python3，它带着真的 python3 的路径转给 cca-core，cca-core 看子命令
# 移植了没有（src/main.rs 的 as_python）。cca 本身一个字不改。
# CCA_CORE_SKIP="子命令 …"：这几个子命令强制走 Python（回归挂了时用它定位是哪个子命令移植错了）。
# 跑完打印这一轮里各有多少次是 Rust / Python 做的。切换到只有 Rust 的那天，这个脚本和 as_python 一起删掉。
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
core="${CCA_CORE:-$here/target/release/cca-core}"
[ -x "$core" ] || { echo "找不到 ${core}：先在 cli/ 里 cargo build --release" >&2; exit 1; }
real=$(PATH=/opt/homebrew/bin:/usr/bin:/bin python3 -I -c 'import sys; print(sys.executable)')
dir=$(mktemp -d "${TMPDIR:-/tmp}/cca-mixed.XXXXXX")
wrap="$dir/python3"
log="$dir/calls.log"
: > "$log"
printf '#!/bin/sh\nCCA_REAL_PYTHON=%q CCA_CORE_LOG=%q CCA_CORE_SKIP=%q exec %q "$@"\n' "$real" "$log" "${CCA_CORE_SKIP:-}" "$core" > "$wrap"
chmod 755 "$wrap"
case "${1:-}" in
	wrap) echo "$wrap" ;;
	regress)
		shift
		rc=0
		CCA_PYWRAP="$wrap" bash "$here/../tests/regress.sh" "$@" || rc=$?
		awk -F'\t' '{n[$1]++} END{printf "---- 混合版：cca-core 做了 %d 次，Python 做了 %d 次\n", n["R"], n["P"]}' "$log"
		awk -F'\t' '$1=="P"{n[$3]++} END{for (s in n) printf "%6d  %s\n", n[s], s}' "$log" | sort -rn | head -n "${TOP:-15}" | sed 's/^/     还在 Python：/'
		rm -rf "$dir"
		exit "$rc" ;;
	*) echo "用法: bash cli/mixed.sh regress | wrap" >&2; exit 1 ;;
esac
