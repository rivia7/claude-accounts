#!/bin/bash
# 发布当前提交（版本号取自 cca 里的 VERSION）：编辅助程序 → 跑 App 的单元测试 → 跑回归 → 把辅助程序接到 cca 后面 → 签名 → 编译界面 App
# → 打 tag → 推送 → 创建 GitHub Release
# （附件：cca（接着四种机器的辅助程序）、cca.sig、一键安装脚本 install.sh、界面 App cca-app.zip 和它的签名 cca-app.zip.sig、
#  Team 的同步服务器的一键安装脚本 cca-hub-install.sh）
# 要装 Rust（rustup）和四个目标：rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl
# 用法：./release.sh ["更新说明"]；签名私钥默认 ~/.ssh/cca_release_ed25519（可用 CCA_RELEASE_KEY 指定）
# VERSION 是 X.Y.Z-beta.N 时发成测试版（GitHub 的 pre-release，不设为 Latest）：自动更新和一键安装都不会装它，
# 只能 cca update X.Y.Z-beta.N 或 install.sh 指定版本手动装
set -euo pipefail
cd "$(dirname "$0")"
KEY="${CCA_RELEASE_KEY:-$HOME/.ssh/cca_release_ed25519}"
fail() { echo "✗ $*" >&2; exit 1; }

v=$(sed -n 's/^VERSION="\([0-9][0-9a-z.-]*\)"$/\1/p' cca | head -n 1)
[[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]] || fail "cca 里的 VERSION 不对：${v}"
pre=()
case "$v" in *-beta.*) pre=(--prerelease --latest=false) ;; esac
# 每个版本都要写“更新了什么”：界面 App 升级后第一次打开时一张张给你看（「帮助」菜单的「更新说明」能再看）
notes_src=macos/Sources/CcaKit/Model/WhatsNew.swift
grep -qF "ReleaseNote(version: \"$v\"" "$notes_src" ||
	fail "还没写 ${v} 的“更新了什么”：在 ${notes_src} 的 WhatsNew.all 最前面加一条 ReleaseNote(version: \"${v}\", …)；顺便看看新手教程（macos/Sources/CcaApp/Guide.swift 的 TutorialCard）要不要跟着改"
[ -z "$(git status --porcelain)" ] || fail "有未提交的改动，先提交"
! git rev-parse -q --verify "refs/tags/v$v" >/dev/null || fail "v$v 已经发布过，先改 VERSION"
# 嵌在 cca 里的公钥必须和签名私钥是一对，否则发出去的版本谁也装不上
pub=$(sed -n 's/^RELEASE_KEY="\(.*\)"$/\1/p' cca)
[ "$(cut -d' ' -f1,2 "$KEY.pub")" = "$(printf '%s' "$pub" | cut -d' ' -f1,2)" ] ||
	fail "cca 里的 RELEASE_KEY 和 $KEY.pub 不是一对"
[ "$(sed -n 's/^RELEASE_KEY="\(.*\)"$/\1/p' install.sh)" = "$pub" ] || fail "install.sh 里的 RELEASE_KEY 和 cca 里的不一致"

# 辅助程序（cli/ 的 Rust 程序，原来是 cca 里内嵌的 Python）：四种机器各编一份，签名前接到 cca 后面（cca-core pack）。照 Cargo.lock 编
# （--locked）；程序里不留这台机器的路径（--remap-path-prefix：依赖库出错位置的字符串里本来带着 ~/.cargo 的绝对路径）
echo "• 编辅助程序（四种机器）…"
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
# Linux 的两份是在这台 Mac 上交叉编译的：辅助程序带着 ring（TLS、签名），它里面的 C 和汇编要 cli/musl-env.sh 指定编译器和归档工具
# （缺 rustup 的 llvm-tools 组件时它自己说）。四个目标一条 cargo 命令编：cargo 自己并行，不用一个一个等（各个目标最后那一步的
# LTO 是单线程的）
( . cli/musl-env.sh ) || fail "交叉编译 Linux 的目标的环境没备好（上面那一句说了缺什么）"
targs=""; for t in $TARGETS; do targs="$targs --target $t"; done
# shellcheck disable=SC2086
( cd cli && . ./musl-env.sh && RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=." cargo build --release --locked $targs ) >/dev/null 2>&1 ||
	fail "辅助程序编不出来：cd cli && . ./musl-env.sh && cargo build --release --locked${targs}（没装哪个目标就先 rustup target add 它）"
for t in $TARGETS; do
	! LC_ALL=C grep -qaF "$HOME/" "cli/target/$t/release/cca-core" || fail "辅助程序（${t}）里带着这台机器的路径"
	# 编出来的确实是这种机器的程序（Linux 的两份在这台 Mac 上运行不了，至少看文件头：64 位、静态链接）
	case "$t" in
		aarch64-apple-darwin) want='Mach-O 64-bit executable arm64' ;;
		x86_64-apple-darwin) want='Mach-O 64-bit executable x86_64' ;;
		x86_64-unknown-linux-musl) want='ELF 64-bit LSB .*x86-64.*static' ;;
		aarch64-unknown-linux-musl) want='ELF 64-bit LSB .*ARM aarch64.*static' ;;
	esac
	file -b "cli/target/$t/release/cca-core" | grep -q "$want" || fail "辅助程序（${t}）不是这种机器的程序：$(file -b "cli/target/$t/release/cca-core")"
done
case "$(uname -m)" in arm64) host=aarch64-apple-darwin ;; *) host=x86_64-apple-darwin ;; esac

# App 的单元测试（macos/test.sh 不带参数：手写的夹具、各部件的单元测试，更新说明的用词和版本先后也在这里查）。
# 回归里只拿 cca 的真实输出跑契约测试的解码部分（macos/test.sh 目录），这些都不跑。放在回归前面：没通过就不用等回归
echo "• 跑 App 的单元测试…"
ut=$(bash macos/test.sh 2>&1) || {
	printf '%s\n' "$ut" | grep -E '^(FAIL|✗)|: error:' | head -n 20 >&2 || true
	sum=$(printf '%s\n' "$ut" | grep -E '^通过 [0-9]+，失败 [0-9]+' | tail -n 1 || true)
	fail "App 的单元测试没通过：${sum:-没跑完（编译不过或中途出错，见上面）}"
}
echo "  $(printf '%s\n' "$ut" | tail -n 1)"

# 回归用的就是要发布的那一份辅助程序（CCA_HELPER：不另编）
echo "• 跑回归测试…"
res=$(CCA_HELPER="$PWD/cli/target/$host/release/cca-core" bash tests/regress.sh 2>&1 | tail -n 1)
case "$res" in *", 0 failed") echo "  $res" ;; *) fail "回归测试没通过：${res}" ;; esac

out=$(mktemp -d)
# 自检用的缓存目录：放在这个用户的临时目录里（cca 只认 /var/folders/ 下的 TMPDIR），每次都是空的——不然走的是这台机器上早就
# 解好的那一份，接在文件里的那一份根本没解
chk=$(mktemp -d "$(getconf DARWIN_USER_TEMP_DIR)cca-release-check.XXXXXX")
trap 'rm -rf "$out" "$chk"' EXIT
# 发布的 cca：仓库里的 cca 后面接上四种机器的辅助程序。接好以后在一个空的 HOME、空的缓存目录里真的运行一次（解出这台机器的那一份、
# 校验，经它报出版本号：cca _gui hello 只读，不联网），解出来的要和编出来的一字节不差。这台 Mac 能经 Rosetta 跑 Intel 的程序时，
# 另一种芯片的那一份也这样跑一次
"cli/target/$host/release/cca-core" pack cca "$out/cca" >/dev/null || fail "辅助程序没接上（cca-core pack）"
bash -n "$out/cca" || fail "接上辅助程序的 cca 语法不对"
selfcheck() {   # selfcheck 目标 [arch 的参数]：按这种芯片运行一次接好的 cca
	local t="$1" d="$chk/$1" sum; shift
	mkdir -p "$d/home" "$d/tmp"
	env -i HOME="$d/home" TMPDIR="${d#/private}/tmp/" PATH=/usr/bin:/bin:/usr/sbin:/sbin "$@" /bin/bash "$out/cca" _gui hello 2>/dev/null |
		grep -q "\"cca_version\": *\"$v\"" || fail "接上辅助程序的 cca 运行不了（${t}）"
	sum=$(shasum -a 256 "cli/target/$t/release/cca-core" | cut -d' ' -f1)
	[ "$(shasum -a 256 "$d/tmp/cca-helper/$sum" 2>/dev/null | cut -d' ' -f1)" = "$sum" ] || fail "接上辅助程序的 cca 解出来的和编出来的不一样（${t}）"
}
selfcheck "$host"
case "$host" in
	aarch64-apple-darwin) ! arch -x86_64 /usr/bin/true 2>/dev/null || selfcheck x86_64-apple-darwin arch -x86_64 ;;
esac
echo "  发布的 cca：$(( $(wc -c < "$out/cca") / 1024 )) KB"
cp install.sh "$out/install.sh"
# Team 的同步服务器的一键安装脚本（docs/dev/team.md「部署」）：就是 cca team deploy --script 打出来的那一份，用这台机器的辅助程序生成。
# 核对带着这一版的版本号，服务端程序、证书助手和 server/ 下的一字节不差，sh -n 过得去
CCA_VERSION="$v" "cli/target/$host/release/cca-core" hub-installer > "$out/cca-hub-install.sh" || fail "生成不了 Team 的同步服务器的安装脚本"
heredoc() { sed -n "/^	cat <<'$1'\$/,/^$1\$/p" "$out/cca-hub-install.sh" | sed '1d;$d' | shasum -a 256 | cut -d' ' -f1; }   # heredoc 结束行：安装脚本里那一段的 SHA-256
grep -qxF "CCA_HUB_VERSION='$v'" "$out/cca-hub-install.sh" && /bin/dash -n "$out/cca-hub-install.sh" &&
	[ "$(heredoc CCA_HUB_SERVER_PY_EOF)" = "$(shasum -a 256 < server/cca-presence.py | cut -d' ' -f1)" ] &&
	[ "$(heredoc CCA_HUB_ACME_PY_EOF)" = "$(shasum -a 256 < server/cca-acme.py | cut -d' ' -f1)" ] || fail "Team 的同步服务器的安装脚本不对（版本号、服务端程序、证书助手、语法）"
ssh-keygen -Y sign -q -n cca-release -f "$KEY" "$out/cca"
printf 'cca-release %s\n' "$pub" > "$out/allowed"
ssh-keygen -Y verify -f "$out/allowed" -I cca-release -n cca-release -s "$out/cca.sig" < "$out/cca" >/dev/null ||
	fail "签名自检失败"

# 图形界面 App（macos/）：和 cca 同一个版本号，Intel 和 Apple 芯片都能用；签名用单独的命名空间 cca-app，
# 和 cca 本身的签名不能互相顶替（cca gui 下载后按 cca-app 验签）
echo "• 编译界面 App…"
bash macos/build.sh --version "$v" --arch universal --out "$out/app/cca.app" >/dev/null || fail "界面 App 编译失败"
for a in x86_64 arm64; do
	lipo -archs "$out/app/cca.app/Contents/MacOS/cca" | grep -qw "$a" || fail "界面 App 里没有 ${a} 的代码"
done
[ "$(plutil -extract CFBundleShortVersionString raw "$out/app/cca.app/Contents/Info.plist")" = "$v" ] || fail "界面 App 的版本号不是 ${v}"
( cd "$out/app" && ditto -c -k --keepParent cca.app "$out/cca-app.zip" )
ssh-keygen -Y sign -q -n cca-app -f "$KEY" "$out/cca-app.zip"
printf 'cca-app %s\n' "$pub" > "$out/allowed-app"
ssh-keygen -Y verify -f "$out/allowed-app" -I cca-app -n cca-app -s "$out/cca-app.zip.sig" < "$out/cca-app.zip" >/dev/null ||
	fail "界面 App 签名自检失败"
! ssh-keygen -Y verify -f "$out/allowed" -I cca-release -n cca-release -s "$out/cca-app.zip.sig" < "$out/cca-app.zip" >/dev/null 2>&1 ||
	fail "界面 App 的签名不该在 cca-release 命名空间里通过"

git push origin HEAD || fail "推送失败"
git tag -a "v$v" -m "v$v"
git push origin "refs/tags/v$v" || { git tag -d "v$v" >/dev/null; fail "推送 tag 失败，本地 tag 已删除，修好后重新运行"; }
notes=(--generate-notes)
[ -n "${1:-}" ] && notes=(--notes "$1")
gh release create "v$v" "$out/cca" "$out/cca.sig" "$out/install.sh" "$out/cca-app.zip" "$out/cca-app.zip.sig" "$out/cca-hub-install.sh" \
	--title "v$v" "${notes[@]}" ${pre[@]+"${pre[@]}"}
if [ ${#pre[@]} -gt 0 ]; then echo "✓ 已发布测试版 v${v}（手动装：cca update ${v}）"; else echo "✓ 已发布 v${v}"; fi
