#!/bin/bash
# 发布维护者的账号 → 节点映射（cca policy 拉取的那份）：在私有映射仓库的工作目录里改好 policy.json，然后
#   ./policy.sh 映射仓库的目录 ["提交说明"]
# 检查格式 → 确认仓库是私有的 → 用发布密钥签名（命名空间 cca-policy）→ 自检 → 提交并推送。
# 签名私钥默认 ~/.ssh/cca_release_ed25519（可用 CCA_RELEASE_KEY 指定），和发版用的是同一把。
# policy.json 的格式：{"version": 1, "claude": {"name@example.com": "节点"}, "codex": {"name@example.com": "节点"}}
set -euo pipefail
KEY="${CCA_RELEASE_KEY:-$HOME/.ssh/cca_release_ed25519}"
fail() { echo "✗ $*" >&2; exit 1; }

[ -n "${1:-}" ] && [ -f "$1/policy.json" ] || fail "用法：./policy.sh 映射仓库的目录 [\"提交说明\"]（目录里要有 policy.json）"
dir=$(cd "$1" && pwd)   # 先换成绝对路径，再进 cca 的仓库
cd "$(dirname "$0")"
pub=$(sed -n 's/^RELEASE_KEY="\(.*\)"$/\1/p' cca)
[ "$(cut -d' ' -f1,2 "$KEY.pub")" = "$(printf '%s' "$pub" | cut -d' ' -f1,2)" ] ||
	fail "cca 里的 RELEASE_KEY 和 $KEY.pub 不是一对：用这把签的映射谁也不认"
out=$(bash ./cca _policy-check "$dir/policy.json") || fail "policy.json 有问题：
${out}"

# 映射里有邮箱和节点名：只往私有仓库推（看的是 origin 实际推送的地址）
url=$(git -C "$dir" remote get-url --push origin) || fail "${dir} 不是 git 仓库，或者没有 origin"
repo=$(printf '%s' "$url" | sed -E 's#^(git@github\.com:|ssh://git@(ssh\.)?github\.com(:443)?/|https://github\.com/)##; s#\.git$##')
vis=$(gh repo view "$repo" --json visibility -q .visibility 2>/dev/null) || fail "确认不了 ${repo} 是不是私有仓库（gh 登录了吗？）"
[ "$vis" = PRIVATE ] || fail "${repo} 不是私有仓库（${vis}），映射不能推上去"

rm -f "$dir/policy.json.sig"
ssh-keygen -Y sign -q -n cca-policy -f "$KEY" "$dir/policy.json"
allowed=$(mktemp)
trap 'rm -f "$allowed"' EXIT
printf 'cca-release %s\n' "$pub" > "$allowed"
ssh-keygen -Y verify -f "$allowed" -I cca-release -n cca-policy -s "$dir/policy.json.sig" < "$dir/policy.json" >/dev/null ||
	fail "签名自检失败"

git -C "$dir" add policy.json policy.json.sig
if git -C "$dir" diff --cached --quiet; then echo "• 映射没有变化，不用推送"; exit 0; fi
git -C "$dir" commit -q -m "${2:-更新账号 → 节点映射}"
git -C "$dir" push -q origin HEAD
echo "✓ 已发布到 ${repo}：配置了这个仓库的 cca 下次拉取时换上（每天自动拉，cca policy pull 马上拉）"
