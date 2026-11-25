#!/bin/bash
# Claude Code 的 PostToolUse hook（.claude/settings.json：Edit、Write 之后）：刚改的那个文件马上过一遍回归里也有的几项检查，
# 一两秒就完，不用等约 12 分钟的回归。没过：说明写到 stderr、退出码 2，Claude Code 把它交给 Claude 当场改；过了什么都不说。
# 只报刚改的那个文件的问题（别的会话、代理正在改的文件不管）：
#   bash -n          cca、*.sh（/bin/bash：macOS 上就是回归用的 bash 3.2）；server/hub-install.sh 另加 dash -n（服务器上的 sh 可能是 dash）
#   lint dollar      tests/lint_dollar.sh 列的文件（发出去的 bash、App 里拼的 bash、这里的 hook）：变量名后面紧跟非 ASCII 字符
#   lint prompts     改了 cca、macos/PROTOCOL.md 时（回归的 lint_prompts）
#   lint ifaces      改了 cca、cli/src/cmd/corenet/bind.rs 时（回归的 lint_ifaces）
#   不放 Python       .py 这类文件名、第一行是 python 的脚本（和回归的 cca-test lint nopython 一样，. 开头的目录、target、build 这些不管）
#   rustfmt --check  *.rs（在 cli/ 里跑：工具链照 cli/rust-toolchain.toml，格式照仓库根的 rustfmt.toml，和回归的 cargo fmt --all --check 一样）
#   大小上限          CLAUDE.md、.claude/rules/*.md（tests/claude_md_size.sh）
# 三项 lint 用 tests/ 的小工具 cca-test（CCA_TEST_BIN，没设就是 cli/target/kit/cca-test；编：cd cli && cargo build --profile kit -p cca-tests），
# 没编出来时跳过。stdin 是 Claude Code 给的 JSON，只要 tool_input.file_path（sed 就够：不靠 jq、Python）
ROOT=${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}
f=$(sed -n 's/.*"file_path" *: *"\([^"]*\)".*/\1/p' | head -n 1)
case "$f" in
"$ROOT"/*) rel=${f#"$ROOT"/} ;;
*) exit 0 ;;   # 仓库外的文件不管
esac
[ -f "$f" ] || exit 0
TB=${CCA_TEST_BIN:-$ROOT/cli/target/kit/cca-test}
[ -x "$TB" ] || TB=
B=/bin/bash
[ -x "$B" ] || B=bash
msg=
say() {
	msg="${msg}- $*
"
}
mine() {   # mine "lint 打出来的那一行"：「：甲、乙…」里说的是刚改的这个文件的那几项（「文件」「文件:行号」「文件 …」），空格隔开
	printf '%s\n' "${1#：}" | awk -v f="$rel" '{ n = split($0, a, "、"); for (i = 1; i <= n; i++) if (a[i] == f || index(a[i], f ":") == 1 || index(a[i], f " ") == 1) printf "%s ", a[i] }' | sed 's/ *$//'
}

case "$rel" in
cca | *.sh)
	if ! e=$("$B" -n "$f" 2>&1); then
		e=${e//"$f"/$rel}
		say "bash -n：$(printf '%s\n' "$e" | head -n 5)"
	fi
	if [ "$rel" = server/hub-install.sh ] && [ -x /bin/dash ] && ! e=$(/bin/dash -n "$f" 2>&1); then
		e=${e//"$f"/$rel}
		say "dash -n：$(printf '%s\n' "$e" | head -n 5)"
	fi
	;;
esac
if [ -n "$TB" ]; then
	case "$rel" in
	cca | *.sh | *.swift)
		if [ -f "$ROOT/tests/lint_dollar.sh" ]; then
			hits=$(mine "$(bash "$ROOT/tests/lint_dollar.sh" "$TB" "$ROOT" "$ROOT/cca" 2>&1)")
			[ -z "$hits" ] || say "lint dollar：${hits} 这几行的变量名后面紧跟着非 ASCII 字符，macOS 的 bash 3.2 会把字的头一个字节吞进变量名：写成 \${变量}"
		fi
		;;
	esac
	case "$rel" in
	cca | macos/PROTOCOL.md)
		r=$("$TB" lint prompts "$ROOT/cca" "$ROOT/macos/PROTOCOL.md" 2>&1)
		[ "$r" = ok ] || say "lint prompts（提问只经提问的基元，别处不读 fd 3；题号都写进 PROTOCOL 4.6 的映射表）${r}"
		;;
	esac
	case "$rel" in
	cca | cli/src/cmd/corenet/bind.rs)
		r=$("$TB" lint ifaces "$ROOT/cca" "$ROOT/cli/src/cmd/corenet/bind.rs" 2>&1)
		[ "$r" = ok ] || say "lint ifaces（bash 的 VIRTUAL_IFACE_RE 和辅助程序的要一样）${r}"
		;;
	esac
fi
case "/$rel" in
*/.* | */target/* | */build/* | */node_modules/* | */__pycache__/*) ;;
*)
	py=
	case "$rel" in *.py | *.pyc | *.pyi | *.pyw) py=1 ;; esac
	head -n 1 "$f" 2>/dev/null | grep -q '^#!.*python' && py=1
	[ -z "$py" ] || say "仓库里不放 Python 的程序（cca、辅助程序、服务端、测试都不靠 python3：要算的写进 cli/src，测试用的写进 tests/ 的 Rust 或者 sh；回归的 lint nopython 查这个）"
	;;
esac
case "$rel" in
*.rs)
	if command -v rustfmt >/dev/null 2>&1; then
		r=$(cd "$ROOT/cli" && RUSTUP_AUTO_INSTALL=0 rustfmt --edition 2021 --color never --check --config-path "$ROOT/rustfmt.toml" "$f" 2>&1)
		if [ $? -ne 0 ]; then
			# 它也查这个文件 mod 进来的子模块：只留这个文件自己的那几段
			d=$(printf '%s\n' "$r" | awk -v f="$f" '/^Diff in / { on = (index($0, f) > 0) } on' | head -n 25)
			d=${d//"$f"/$rel}
			if [ -n "$d" ]; then
				say "rustfmt --check 没过（回归的 cargo fmt --all --check 也查；在 cli/ 里跑 cargo fmt，或者照下面改）：
${d}"
			else
				case "$r" in
				*"$f"*)
					r=${r//"$f"/$rel}
					say "rustfmt 读不了这个文件：
$(printf '%s\n' "$r" | head -n 12)"
					;;
				esac
			fi
		fi
	fi
	;;
esac
case "$rel" in
CLAUDE.md | */CLAUDE.md | .claude/rules/*.md)
	if [ -f "$ROOT/tests/claude_md_size.sh" ]; then
		hits=$(mine "$(bash "$ROOT/tests/claude_md_size.sh" "$ROOT" 2>&1)")
		[ -z "$hits" ] || say "超过大小上限：${hits}（tests/claude_md_size.sh）。只跟某一块有关的挪进 .claude/rules/（带 paths）或者 docs/dev/ 对应的那一份，别提高上限"
	fi
	;;
esac
[ -z "$msg" ] && exit 0
printf '改完 %s 后的快速检查没过（.claude/hooks/quick-lint.sh；回归里也查这些，现在就改）：\n%s' "$rel" "$msg" >&2
exit 2
