#!/bin/bash
# 维护者用：发布账号 → 节点映射（cca policy 拉取的那份），管理各台机器拉映射用的 deploy key。
#
#   ./policy.sh 映射仓库的目录 ["提交说明"]
#     在私有映射仓库的工作目录里改好 policy.json 后发布：检查格式 → 确认仓库是私有的 → 用发布密钥签名（命名空间
#     cca-policy）→ 自检 → 提交并推送。签名私钥默认 ~/.ssh/cca_release_ed25519（可用 CCA_RELEASE_KEY 指定），和发版
#     用的是同一把。policy.json 的格式：{"version": 1, "claude": {"name@example.com": "节点"}, "codex": {"name@example.com": "节点"},
#     "aliases": {"name@example.com": "别名"}}。aliases 可以不写：各台机器按映射自动加账号时用的别名（Claude、Codex 通用）
#
#   ./policy.sh key [ls]                       列出各台机器的 deploy key：添加时间、最近一次拉取、别名
#   ./policy.sh key add [公钥文件 | -] [别名]    加一把只读 deploy key：不写文件就用剪贴板里的公钥（- 是从标准输入读），
#                                              不写别名就提示起一个（默认是公钥备注里的机器名）；别名不能重复
#   ./policy.sh key rm 别名 [-y]                删掉一把：那台机器以后拉不到新映射（已经拉到的照用）
#   ./policy.sh key mv 别名 [新别名]             改别名
#   别名就是 GitHub 上 deploy key 的标题。GitHub 的 deploy key 不能改，改别名是删掉后用同一把公钥重新加上。
#   默认管 cca 里写的默认映射仓库（POLICY_REPO_DEFAULT，环境变量 CCA_POLICY_DEFAULT 可以换掉），别的仓库加 -R 维护者/仓库。
#   经 gh 调 GitHub 的接口（先 gh auth login）；交互输入从终端读，CCA_TTY=答案文件 时按行读（测试用）。
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
KEY="${CCA_RELEASE_KEY:-$HOME/.ssh/cca_release_ed25519}"
fail() { echo "✗ $*" >&2; exit 1; }
usage() {
	cat >&2 <<'U'
用法：
  ./policy.sh 映射仓库的目录 ["提交说明"]     发布映射（目录里要有 policy.json）
  ./policy.sh key [ls]                         列出各台机器的 deploy key：添加时间、最近一次拉取、别名
  ./policy.sh key add [公钥文件 | -] [别名]    加一把只读 deploy key：不写文件就用剪贴板里的公钥，不写别名就提示起一个
  ./policy.sh key rm 别名 [-y]                 删掉一把
  ./policy.sh key mv 别名 [新别名]             改别名
key 默认管 cca 里写的默认映射仓库，别的仓库加 -R 维护者/仓库
U
	exit 1
}

# ---------------------------------------------------------------------------
# deploy key：每台机器一把只读 key（cca policy setup 生成，公钥备注是 cca-policy@机器名），别名记在 key 的标题里
# ---------------------------------------------------------------------------
MINE="${CCA_HOME:-$HOME/.cca}/policy/id_ed25519.pub"   # 这台机器自己的
REPO="" YES="" KEYS="" LINE="" FP="" GH_ERR="" TMPD="" TTY_OK=0

open_tty() {   # 交互输入从 fd 3 读：CCA_TTY=答案文件（测试用），否则是终端
	if [ -n "${CCA_TTY:-}" ]; then
		exec 3<"$CCA_TTY" && TTY_OK=1
	elif { exec 3</dev/tty; } 2>/dev/null; then
		TTY_OK=1
	fi
}
ask() {   # x=$(ask "提示" 默认值)：提示写 stderr；终端里用 readline（退格能删掉整个中文字符）
	local v=""
	if [ "$TTY_OK" = 1 ]; then
		if [ -z "${CCA_TTY:-}" ] && [ -t 3 ]; then
			IFS= read -e -r -p "$1" v <&3 || v=""
		else
			printf '%s' "$1" >&2
			IFS= read -r v <&3 || v=""
		fi
	fi
	printf '%s' "${v:-${2:-}}"
}
norm() { printf '%s' "$1" | tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//'; }   # 别名：空白都换成一个空格，去掉首尾的
ymd() {   # GitHub 给的时间（UTC）→ 本地日期
	local s
	s=$(TZ=UTC date -j -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s 2>/dev/null) && date -r "$s" +%F && return 0
	printf '%s' "${1%%T*}"
}
join_() { awk 'NF { if (n++) printf "、"; printf "%s", $0 }'; }   # 一行一个 → 用顿号连起来

gh_msg() {   # gh_msg 文件：GitHub 回的错误说明（errors 里的 message，没有就用外层的 message）
	python3 -I - "$1" 2>/dev/null <<'PY'
import json, sys
try:
    j = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    sys.exit(0)
if isinstance(j, dict):
    m = [str(e.get("message") or e.get("code") or "") for e in (j.get("errors") or []) if isinstance(e, dict)]
    print("; ".join(x for x in m if x) or str(j.get("message") or ""))
PY
}
gh_api() {   # gh_api 参数…：调 gh api，回应在 $TMPD/out；失败时 GH_ERR 是看得懂的原因。
	# 连不上、GitHub 一时出错就再试（一共三次）；GitHub 明确拒绝的（4xx）不重试
	local i m e
	for i in 1 2 3; do
		gh api "$@" >"$TMPD/out" 2>"$TMPD/err" && return 0
		grep -q '(HTTP 4[0-9][0-9])' "$TMPD/err" && break
		[ "$i" = 3 ] || sleep 2
	done
	m=$(gh_msg "$TMPD/out"); e=$(grep -m 1 '[^[:space:]]' "$TMPD/err" || true)
	case "$m $e" in
		*"already in use"*) GH_ERR="这把公钥 GitHub 上已经有地方在用（别的仓库的 deploy key，或者谁账号里的 SSH key；一把公钥只能加一处）：请对方发 cca policy 里显示的那一把" ;;
		*"HTTP 404"*) GH_ERR="${REPO} 不存在，或者 gh 登录的账号不是它的管理员" ;;
		*"HTTP 401"*|*"gh auth login"*) GH_ERR="gh 还没登录 GitHub：先运行 gh auth login" ;;
		*"HTTP 403"*) GH_ERR="没有权限（gh 登录的账号要是 ${REPO} 的管理员）：${m:-${e}}" ;;
		*"error connecting"*|*[Tt]imeout*) GH_ERR="连不上 GitHub（${e}）" ;;
		*) GH_ERR="${m:-${e:-gh 出错了}}" ;;
	esac
	return 1
}

keys_parse() {   # keys_parse 文件：gh api --paginate 的输出（好几页时是几个数组首尾相接）→ 每把 key 一行：
	# 编号<TAB>别名<TAB>公钥（类型 内容）<TAB>添加时间<TAB>最近使用<TAB>ro|rw<TAB>on|off；空的写成 -（read 会把连着的 TAB 并成一个）
	python3 -I - "$1" <<'PY'
import json, sys
s = open(sys.argv[1], encoding="utf-8").read()
dec = json.JSONDecoder()
i, keys = 0, []
while True:
    while i < len(s) and s[i].isspace():
        i += 1
    if i >= len(s):
        break
    v, i = dec.raw_decode(s, i)
    keys.extend(v if isinstance(v, list) else [v])
def f(x):
    return " ".join(str("" if x is None else x).split()) or "-"
for k in keys:
    if isinstance(k, dict):
        print("\t".join([f(k.get("id")), f(k.get("title")), f(" ".join(str(k.get("key") or "").split()[:2])),
                         f(k.get("created_at")), f(k.get("last_used")),
                         "rw" if k.get("read_only") is False else "ro", "off" if k.get("enabled") is False else "on"]))
PY
}
keys_load() {   # KEYS：仓库上现有的 deploy key（一把一行，见 keys_parse）
	gh_api "repos/${REPO}/keys?per_page=100" --paginate || return 1
	KEYS=$(keys_parse "$TMPD/out") || { GH_ERR="看不懂 GitHub 的回应"; return 1; }
}
key_by() {   # key_by 第几列 值：KEYS 里这一列等于这个值的行
	printf '%s\n' "$KEYS" | Q="$2" awk -F'\t' -v c="$1" '$c == ENVIRON["Q"]'
}
existing() {   # （现有：别名、别名…）
	local l
	l=$(printf '%s\n' "$KEYS" | cut -f2 | join_)
	[ -z "$l" ] || printf '（现有：%s）' "$l"
}
key_find() {   # key_find 别名或编号 → 那把 key 的一行
	local q hits n
	q=$(norm "$1")
	hits=$(key_by 2 "$q")
	[ -n "$hits" ] || hits=$(key_by 1 "$q")
	n=$(printf '%s' "$hits" | grep -c . || true)
	[ "$n" != 0 ] || fail "${REPO} 上没有叫「${q}」的 key$(existing)"
	[ "$n" = 1 ] || fail "有 ${n} 把都叫「${q}」（编号 $(printf '%s\n' "$hits" | cut -f1 | join_)）：用编号指定"
	printf '%s' "$hits"
}
key_create() {   # key_create 别名 公钥 [ro|rw]：加一把 deploy key
	local ro=true err
	[ "${3:-ro}" != rw ] || ro=false
	gh_api -X POST "repos/${REPO}/keys" -f title="$1" -f key="$2" -F read_only="$ro" && return 0
	# 第一次其实加上了、只是回应丢了，重试时 GitHub 会说这把已经在用：再查一遍
	err="$GH_ERR"
	keys_load && [ -n "$(key_by 3 "$2" | Q="$1" awk -F'\t' '$2 == ENVIRON["Q"]')" ] && return 0
	GH_ERR="$err"
	return 1
}

alias_check() {   # alias_check 别名 [这把 key 的编号]：能用返回 0；不能用时输出原因
	[ -n "$1" ] || { echo "别名不能是空的"; return 1; }
	if [ -n "$(key_by 2 "$1" | ME="${2:-}" awk -F'\t' '$1 != ENVIRON["ME"]')" ]; then
		echo "别名「${1}」已经有了，换一个"
		return 1
	fi
}
alias_pick() {   # alias_pick 提示 命令里写的别名 默认值 [这把 key 的编号]：写了就只检查；没写就提示（最多问 5 次）
	local a why i
	a=$(norm "$2")
	if [ -n "$a" ]; then
		why=$(alias_check "$a" "${4:-}") || { echo "✗ ${why}" >&2; return 1; }
		printf '%s' "$a"
		return 0
	fi
	[ "$TTY_OK" = 1 ] || { echo "✗ 没有终端可以提问：把别名写在命令后面" >&2; return 1; }
	for i in 1 2 3 4 5; do
		a=$(norm "$(ask "${1}${3:+ [${3}]}：" "$3")")
		why=$(alias_check "$a" "${4:-}") && { printf '%s' "$a"; return 0; }
		echo "  ${why}" >&2
	done
	return 1
}

key_scan() {   # key_scan 文字 来源：LINE 是里面的公钥（可以夹在聊天消息里；同一把出现几次算一把）
	case "$1" in
		*"PRIVATE KEY"*) fail "${2}里是私钥，不是公钥！别加，也别再转给别人：请对方发 cca 显示的那一行公钥（ssh-ed25519 开头）" ;;
	esac
	LINE=$(printf '%s\n' "$1" | tr -d '\r' |
		grep -oE '(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp(256|384|521)|sk-ssh-ed25519@openssh\.com|sk-ecdsa-sha2-nistp256@openssh\.com) AAAA[A-Za-z0-9+/]+=*( [A-Za-z0-9._+-]*@[A-Za-z0-9._+-]+)?' |
		awk '!seen[$1 " " $2]++' || true)
}
key_read() {   # key_read 来源（文件、- 或者空＝剪贴板）→ LINE：一行公钥（类型 内容 [备注]），FP：它的指纹
	local src="$1" text from n
	case "$src" in
		'') text=$(pbpaste 2>/dev/null || true); from="剪贴板" ;;
		-) text=$(cat); from="标准输入" ;;
		*) text=$(cat "$src") || fail "读不了 ${src}"; from="$src" ;;
	esac
	key_scan "$text" "$from"
	if [ -z "$LINE" ] && [ -z "$src" ] && [ "$TTY_OK" = 1 ]; then   # 剪贴板里没有：让维护者直接粘贴
		from="粘贴的内容"
		key_scan "$(ask "  剪贴板里没有公钥。粘贴对方发来的公钥（一行），回车：" "")" "$from"
	fi
	n=$(printf '%s' "$LINE" | grep -c . || true)
	[ "$n" != 0 ] || fail "${from}里没有公钥（应该是一行 ssh-ed25519 AAAA… 开头的文字）"
	[ "$n" = 1 ] || fail "${from}里有 ${n} 把公钥：一次加一把"
	printf '%s\n' "$LINE" > "$TMPD/new.pub"
	FP=$(ssh-keygen -l -f "$TMPD/new.pub" 2>/dev/null | awk '{print $2}' || true)
	[ -n "$FP" ] || fail "${from}里的公钥不完整（复制的时候可能漏了几个字符）"
}

key_ls() {
	local n mine id title pub created used ro en note
	keys_load || fail "读不到 ${REPO} 上的 deploy key：${GH_ERR}"
	n=$(printf '%s' "$KEYS" | grep -c . || true)
	if [ "$n" = 0 ]; then
		echo "• ${REPO} 上还没有 deploy key：有人发来公钥后复制它，运行 ./policy.sh key add"
		return 0
	fi
	mine=$(cut -d' ' -f1,2 "$MINE" 2>/dev/null || true)
	echo "• ${REPO} 上有 ${n} 把 deploy key："
	echo "  添加于      最近使用    别名"
	while IFS=$'\t' read -r id title pub created used ro en; do
		note=""
		[ "$pub" != "$mine" ] || note="${note}（这台机器）"
		[ "$ro" = ro ] || note="${note}（能写！删掉后用 ./policy.sh key add 重新加成只读）"
		[ "$en" = on ] || note="${note}（已停用）"
		if [ "$used" = - ]; then used="还没用过  "; else used=$(ymd "$used"); fi
		echo "  $(ymd "$created")  ${used}  ${title}${note}"
	done <<< "$KEYS"
}
key_add() {
	local a src="" name="" pub comment def hit mine=""
	for a in "$@"; do
		if [ "$a" = - ] || [ -f "$a" ]; then
			[ -z "$src" ] || fail "一次加一把"
			src="$a"
		elif [ -z "$name" ]; then
			name="$a"
		else
			fail "用法：./policy.sh key add [公钥文件 | -] [别名]"
		fi
	done
	key_read "$src"
	pub=$(printf '%s' "$LINE" | cut -d' ' -f1,2)
	comment=$(printf '%s' "$LINE" | cut -s -d' ' -f3-)
	keys_load || fail "读不到 ${REPO} 上的 deploy key：${GH_ERR}"
	hit=$(key_by 3 "$pub")
	if [ -n "$hit" ]; then
		echo "• 这把公钥已经加过了，别名「$(printf '%s' "$hit" | cut -f2)」（改别名：./policy.sh key mv 别名 新别名）"
		return 0
	fi
	[ "$(cut -d' ' -f1,2 "$MINE" 2>/dev/null || true)" != "$pub" ] || mine=1
	def="${comment#cca-policy@}"   # cca 生成的公钥，备注是 cca-policy@机器名
	echo "  要加的公钥：${FP}${comment:+（${comment}）}${mine:+，是这台机器的}"
	name=$(alias_pick "  给它起个别名（谁的哪台机器，以后按别名查、删、改）" "$name" "$def") || exit 1
	key_create "$name" "$pub" ro || fail "没加上：${GH_ERR}"
	echo "✓ 已把「${name}」加成 ${REPO} 的只读 deploy key：那台机器在引导里按回车，或者运行 cca policy pull，就能拉到映射"
}
key_rm() {
	[ $# -eq 1 ] || fail "用法：./policy.sh key rm 别名 [-y]"
	local hit id title pub
	keys_load || fail "读不到 ${REPO} 上的 deploy key：${GH_ERR}"
	hit=$(key_find "$1") || exit 1
	id=$(printf '%s' "$hit" | cut -f1); title=$(printf '%s' "$hit" | cut -f2); pub=$(printf '%s' "$hit" | cut -f3)
	if [ -z "$YES" ]; then
		[ "$TTY_OK" = 1 ] || fail "没有终端可以确认：确定要删就加上 -y"
		case "$(ask "  删掉「${title}」吗？那台机器以后拉不到新映射（已经拉到的照用）[y/N] " n)" in
			[Yy]*) ;;
			*) echo "• 没删"; return 0 ;;
		esac
	fi
	gh_api -X DELETE "repos/${REPO}/keys/${id}" || fail "没删掉：${GH_ERR}"
	echo "✓ 已删掉「${title}」：那台机器以后拉不到新映射（已经拉到的照用）"
	echo "  它的公钥（要加回来就复制这一行，运行 ./policy.sh key add）：${pub}"
}
key_mv() {
	[ $# -ge 1 ] && [ $# -le 2 ] || fail "用法：./policy.sh key mv 别名 [新别名]"
	local hit id title pub ro new
	keys_load || fail "读不到 ${REPO} 上的 deploy key：${GH_ERR}"
	hit=$(key_find "$1") || exit 1
	id=$(printf '%s' "$hit" | cut -f1); title=$(printf '%s' "$hit" | cut -f2)
	pub=$(printf '%s' "$hit" | cut -f3); ro=$(printf '%s' "$hit" | cut -f6)
	new=$(alias_pick "  「${title}」的新别名" "${2:-}" "" "$id") || exit 1
	if [ "$new" = "$title" ]; then echo "• 别名没变"; return 0; fi
	gh_api -X DELETE "repos/${REPO}/keys/${id}" || fail "没改成：${GH_ERR}"
	key_create "$new" "$pub" "$ro" || fail "旧的「${title}」已经删了，可新的没加上：${GH_ERR}
  那台机器现在拉不到新映射，尽快加回来：复制下面这一行，运行 ./policy.sh key add '${new}'
  ${pub}"
	echo "✓ 「${title}」改叫「${new}」（GitHub 上的 deploy key 不能改，是删掉后用同一把公钥重新加的：那台机器什么都不用做，添加时间和最近使用从现在算起）"
}
key_cmd() {
	local args=()
	while [ $# -gt 0 ]; do
		case "$1" in
			-R|--repo) [ $# -ge 2 ] || fail "-R 后面写仓库（维护者/仓库）"; REPO="$2"; shift 2 ;;
			-y|--yes) YES=1; shift ;;
			-h|--help) usage ;;
			*) args+=("$1"); shift ;;
		esac
	done
	if [ -z "$REPO" ]; then
		if [ -n "${CCA_POLICY_DEFAULT+x}" ]; then REPO="$CCA_POLICY_DEFAULT"
		else REPO=$(sed -n 's/^POLICY_REPO_DEFAULT="\${CCA_POLICY_DEFAULT-\([^}]*\)}"$/\1/p' "$HERE/cca"); fi
	fi
	[[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail "不知道是哪个映射仓库：加上 -R 维护者/仓库"
	command -v gh >/dev/null 2>&1 || fail "要用 GitHub 的命令行工具 gh：brew install gh，再运行 gh auth login"
	command -v python3 >/dev/null 2>&1 || fail "要用 python3：先运行 xcode-select --install"
	TMPD=$(mktemp -d)
	trap 'rm -rf "$TMPD"' EXIT
	open_tty
	set -- ${args[@]+"${args[@]}"}
	case "${1:-ls}" in
		ls|list) key_ls ;;
		add) shift; key_add "$@" ;;
		rm|remove) shift; key_rm "$@" ;;
		mv|rename) shift; key_mv "$@" ;;
		*) usage ;;
	esac
}

case "${1:-}" in
	key|keys) shift; key_cmd "$@"; exit 0 ;;
	''|-h|--help|help) usage ;;
esac

# ---------------------------------------------------------------------------
# 发布映射
# ---------------------------------------------------------------------------
[ -f "$1/policy.json" ] || fail "${1} 里没有 policy.json（用法：./policy.sh 映射仓库的目录 [\"提交说明\"]）"
dir=$(cd "$1" && pwd)   # 先换成绝对路径，再进 cca 的仓库
cd "$HERE"
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
