#!/bin/bash
# 发布当前提交（版本号取自 cca 里的 VERSION）：跑回归 → 签名 → 打 tag → 推送 → 创建 GitHub Release
# （附件：cca、cca.sig、一键安装脚本 install.sh）
# 用法：./release.sh ["更新说明"]；签名私钥默认 ~/.ssh/cca_release_ed25519（可用 CCA_RELEASE_KEY 指定）
set -euo pipefail
cd "$(dirname "$0")"
KEY="${CCA_RELEASE_KEY:-$HOME/.ssh/cca_release_ed25519}"
fail() { echo "✗ $*" >&2; exit 1; }

v=$(sed -n 's/^VERSION="\([0-9.]*\)"$/\1/p' cca | head -n 1)
[[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "cca 里的 VERSION 不对：${v}"
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

git push origin HEAD || fail "推送失败"
git tag -a "v$v" -m "v$v"
git push origin "refs/tags/v$v" || { git tag -d "v$v" >/dev/null; fail "推送 tag 失败，本地 tag 已删除，修好后重新运行"; }
notes=(--generate-notes)
[ -n "${1:-}" ] && notes=(--notes "$1")
gh release create "v$v" "$out/cca" "$out/cca.sig" "$out/install.sh" --title "v$v" "${notes[@]}"
echo "✓ 已发布 v$v"
