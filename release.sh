#!/bin/bash
# 发布当前提交（版本号取自 cca 里的 VERSION）：跑回归 → 签名 → 编译界面 App → 打 tag → 推送 → 创建 GitHub Release
# （附件：cca、cca.sig、一键安装脚本 install.sh、界面 App cca-app.zip 和它的签名 cca-app.zip.sig）
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

echo "• 跑回归测试…"
res=$(bash tests/regress.sh 2>&1 | tail -n 1)
case "$res" in *", 0 failed") echo "  $res" ;; *) fail "回归测试没通过：${res}" ;; esac

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
cp cca "$out/cca"
cp install.sh "$out/install.sh"
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
gh release create "v$v" "$out/cca" "$out/cca.sig" "$out/install.sh" "$out/cca-app.zip" "$out/cca-app.zip.sig" \
	--title "v$v" "${notes[@]}" ${pre[@]+"${pre[@]}"}
if [ ${#pre[@]} -gt 0 ]; then echo "✓ 已发布测试版 v${v}（手动装：cca update ${v}）"; else echo "✓ 已发布 v${v}"; fi
