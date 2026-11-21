#!/bin/bash
# 卸载时去掉 shell 启动文件里 cca 加的那两行（cca 的 path_unhint）：把函数和它认的那两行从 cca 里切出来，在临时目录当 HOME 跑各种情况。
#   bash tests/path_unhint.sh [cca 的路径]
# 只打印没过的；全过退出码 0。回归的 unit 一节跑它（整条 cca uninstall 的在第 12 节）。只用临时目录，不碰真的 HOME
CCA=${1:-$(cd "$(dirname "$0")/.." && pwd)/cca}
W=$(mktemp -d "${TMPDIR:-/tmp}/cca-unhint.XXXXXX") || exit 1
trap '[ -z "$W" ] || { chmod -R u+w "$W" 2>/dev/null; rm -rf "$W"; }' EXIT
{
	printf 'info() { printf "INFO %%s\\n" "$*"; }\nwarn() { printf "WARN %%s\\n" "$*"; }\n'
	grep -E "^PATH_(MARK|LINE)=" "$CCA"
	sed -n '/^path_unhint() {/,/^}/p' "$CCA"
} > "$W/fn.sh"
[ "$(grep -c '' "$W/fn.sh")" -gt 10 ] || { echo "没从 ${CCA} 里切出 path_unhint"; exit 1; }
# 装的时候写下的就是这三行：一个空行、注释、export（path_hint）
BLOCK=$(printf '# cca（Claude Code 多账号）\nexport PATH="$HOME/.local/bin:$PATH"')
grep -qF "printf '\\n%s\\n%s\\n' \"\$PATH_MARK\" \"\$PATH_LINE\"" "$CCA" || { echo "path_hint 写下的不再是「空行、PATH_MARK、PATH_LINE」：这份测试要跟着改"; exit 1; }
t() { env -i LANG=en_US.UTF-8 HOME="$1" PATH=/usr/bin:/bin /bin/bash -c '. "$1"; path_unhint' _ "$W/fn.sh" 2>&1; }
fail=0
ck() { eval "$2" || { fail=$((fail + 1)); echo "FAIL $1"; }; }
mode() { stat -f %Lp "$1" 2>/dev/null || stat -c %a "$1"; }

# 1. 典型：文件末尾是 cca 追加的三行，~/.local/bin 空了 → 去掉，文件逐字节回到原来，权限不变，空目录删掉
H="$W/h1"; mkdir -p "$H/.local/bin"; printf 'export FOO=1\nalias ll="ls -l"\n' > "$H/orig"; cp "$H/orig" "$H/.zshrc"; chmod 640 "$H/.zshrc"
printf '\n%s\n' "$BLOCK" >> "$H/.zshrc"
out=$(t "$H")
ck "末尾的三行去掉、逐字节还原、权限不变、空目录删掉" 'cmp -s "$H/orig" "$H/.zshrc" && [ "$(mode "$H/.zshrc")" = 640 ] && [ ! -e "$H/.local/bin" ] && printf "%s" "$out" | grep -q "INFO 已从 .*/.zshrc 去掉"'

# 2. 在文件中间、后面还有内容；反斜杠、行尾空格、没有结尾换行的最后一行都原样
H="$W/h2"; mkdir -p "$H"; printf 'a\\tb  \n\n%s\nlast-no-newline' "$BLOCK" > "$H/.bashrc"
out=$(t "$H")
ck "中间的去掉，别的行原样" '[ "$(cat "$H/.bashrc")" = "$(printf "a\\\\tb  \nlast-no-newline")" ]'

# 3. ~/.local/bin 里还有别的命令（claude 的官方安装器也装在那）→ 留着，说一声
H="$W/h3"; mkdir -p "$H/.local/bin"; : > "$H/.local/bin/claude"; printf 'x\n\n%s\n' "$BLOCK" > "$H/.zshrc"; cp "$H/.zshrc" "$H/before"
out=$(t "$H")
ck "目录里还有别的命令时留着" 'cmp -s "$H/before" "$H/.zshrc" && [ -d "$H/.local/bin" ] && printf "%s" "$out" | grep -q "INFO .*那两行留着"'

# 4. 用户改过的（第二行不一样）、只有注释那一行的：不动，也不说话
H="$W/h4"; mkdir -p "$H"; printf '# cca（Claude Code 多账号）\nexport PATH="$HOME/.local/bin:/opt/x:$PATH"\n' > "$H/.zshrc"; printf 'y\n# cca（Claude Code 多账号）\n' > "$H/.profile"
cp "$H/.zshrc" "$H/b1"; cp "$H/.profile" "$H/b2"
out=$(t "$H")
ck "改过的不动" 'cmp -s "$H/b1" "$H/.zshrc" && cmp -s "$H/b2" "$H/.profile" && [ -z "$out" ]'

# 5. 启动文件是链接（dotfiles 管着的）：不改，警告
H="$W/h5"; mkdir -p "$H/dot"; printf 'z\n\n%s\n' "$BLOCK" > "$H/dot/zshrc"; ln -s dot/zshrc "$H/.zshrc"; cp "$H/dot/zshrc" "$H/before"
out=$(t "$H")
ck "链接不改" '[ -L "$H/.zshrc" ] && cmp -s "$H/before" "$H/dot/zshrc" && printf "%s" "$out" | grep -q "WARN .*是个链接，没有改"'

# 6. 文件里只有 cca 写的（装的时候新建的）：变成空文件，留着
H="$W/h6"; mkdir -p "$H"; printf '\n%s\n' "$BLOCK" > "$H/.bash_profile"
out=$(t "$H")
ck "只有 cca 写的：变成空文件" '[ -f "$H/.bash_profile" ] && [ ! -s "$H/.bash_profile" ]'

# 7. 换不上去（目录写不了）：原文件不变，警告，不留临时文件。root 什么都写得了，不测这一条
if [ "$(id -u)" != 0 ]; then
	H="$W/h7"; mkdir -p "$H/ro"; printf 'q\n\n%s\n' "$BLOCK" > "$H/ro/.zshrc"; cp "$H/ro/.zshrc" "$H/before"; chmod 555 "$H/ro"
	out=$(t "$H/ro")
	chmod 755 "$H/ro"
	ck "写不了时原文件不变" 'cmp -s "$H/before" "$H/ro/.zshrc" && [ "$(ls -A "$H/ro" | wc -l | tr -d " ")" = 1 ] && printf "%s" "$out" | grep -q "WARN 没能改"'
fi

# 8. 几个启动文件里都有（换过 shell）：都去掉
H="$W/h8"; mkdir -p "$H"; for f in .zshrc .bash_profile .bashrc .profile; do printf 'k\n\n%s\n' "$BLOCK" > "$H/$f"; done
out=$(t "$H")
ck "四个文件都看" '[ "$(cat "$H/.zshrc" "$H/.bash_profile" "$H/.bashrc" "$H/.profile" | tr -d "\n")" = kkkk ] && [ "$(printf "%s\n" "$out" | grep -c "INFO 已从")" = 4 ]'

# 9. 没有任何启动文件、没有目录：什么都不做
H="$W/h9"; mkdir -p "$H"; out=$(t "$H"); ck "什么都没有" '[ -z "$out" ] && [ -z "$(ls -A "$H")" ]'

[ "$fail" -eq 0 ]
