#!/bin/bash
# 维护者用：发布账号 → 节点映射（cca policy 拉取的那份），管理各台机器拉映射用的 deploy key。
#
#   ./policy.sh 映射仓库的目录 ["提交说明"]
#     在私有映射仓库的工作目录里改好 policy.json 后发布：检查格式 → 确认仓库是私有的 → 用发布密钥签名（命名空间
#     cca-policy）→ 自检 → 提交并推送。签名私钥默认 ~/.ssh/cca_release_ed25519（可用 CCA_RELEASE_KEY 指定），和发版
#     用的是同一把。policy.json 的格式：{"version": 1, "claude": {"name@example.com": "节点"}, "codex": {"name@example.com": "节点"},
#     "aliases": {"name@example.com": "别名"}}。aliases 可以不写：各台机器按映射自动加账号时用的别名（Claude、Codex 通用）
#     目录里放着你自己的 Clash 配置 custom.yaml 时，顺带发布其中策略组 OpenAi（别的组名用 CCA_FILL_GROUP=组名 指定）
#     引用的节点：抽出来写成 nodes.json，同样签名（命名空间 cca-nodes）推上去；各台机器把它们补进自己的路由器和本机
#     Clash Verge 的配置里缺的地方，名字补进同名策略组。custom.yaml 本身不提交（里面有你全部的规则和设备），已经提交了会拒绝
#
#   ./policy.sh nodes 参考配置 [策略组]
#     只看不发布：打印会写进 nodes.json 的内容（策略组默认 OpenAi）
#
#   ./policy.sh lock claude|codex 邮箱 节点 [-C 映射仓库的目录] [-y]
#     把账号锁到节点：写进 policy.json，列出会改什么，确认后照常检查、签名、提交并推送（-y 不问）。别的机器都只有只读 key，
#     cca policy request（App 账号菜单里的「请求维护者锁定节点」）生成的就是这条命令，发给维护者在 cca 的仓库里运行。
#     映射仓库的目录默认是环境变量 CCA_POLICY_DIR，没有就是 cca 仓库旁边的 cca-policy。节点不在参考节点（nodes.json）里时提醒一句
#
#   ./policy.sh key [ls]                       列出各台机器的 deploy key：添加时间、最近一次拉取、别名
#   ./policy.sh key add [公钥文件 | -] [别名]    加一把只读 deploy key：不写文件就用剪贴板里的公钥（- 是从标准输入读），
#                                              不写别名就提示起一个（默认是公钥备注里的机器名）；别名不能重复
#   ./policy.sh key rm 别名 [-y]                删掉一把：那台机器以后拉不到新映射（已经拉到的照用）
#   ./policy.sh key mv 别名 [新别名]             改别名
#   别名就是 GitHub 上 deploy key 的标题。GitHub 的 deploy key 不能改，改别名是删掉后用同一把公钥重新加上。
#   默认管 cca 里写的默认映射仓库（POLICY_REPO_DEFAULT，环境变量 CCA_POLICY_DEFAULT 可以换掉），别的仓库加 -R 维护者/仓库。
#   经 gh 调 GitHub 的接口（先 gh auth login）；交互输入从终端读，CCA_TTY=答案文件 时按行读（测试用）。
#   映射里配置了设备统计的服务器时，add / rm / mv 之后顺带把设备清单同步过去（见下面的 presence sync）。
#
#   ./policy.sh presence deploy root@主机 [-p SSH 端口] [-i 私钥] [--port 端口] [--dir 服务器上的目录] [-C 映射仓库的目录]
#     经 SSH 把设备统计的服务器（server/ 下的 compose.yaml 和 cca-presence.py：官方 python 镜像，只用标准库，不用构建）
#     装到一台有 docker compose 的 Linux 上的一个目录里（默认 /root/projects/cca-presence，要 root），第一次生成自签的
#     P-256 证书（私钥不出服务器），docker compose up -d，默认端口 18443（不用 80、443）。再从这台 Mac 钉住证书公钥连一次，
#     把 presence 一节（地址、证书公钥的指纹、账号摘要用的盐）写进映射目录的 policy.json，推送设备清单。重复运行就是
#     更新服务端（证书和盐不变）；迁移是把整个目录拷到新服务器后对它运行一次。之后照常 ./policy.sh 目录 发布映射
#   ./policy.sh presence sync [-C 目录] [-R 仓库]
#     把仓库上各台机器的 deploy key（只读、启用的 ed25519）作为设备清单，用发布密钥签名（命名空间 cca-devices）推给服务器：
#     服务器只认这份清单里的机器，名字就是 key 的别名
#   ./policy.sh presence status [-C 目录]     服务器在不在、认得几台设备、签名自检过没过
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
KEY="${CCA_RELEASE_KEY:-$HOME/.ssh/cca_release_ed25519}"
fail() { echo "✗ $*" >&2; exit 1; }
usage() {
	cat >&2 <<'U'
用法：
  ./policy.sh 映射仓库的目录 ["提交说明"]     发布映射（目录里要有 policy.json；有 custom.yaml 时顺带发布参考节点）
  ./policy.sh nodes 参考配置 [策略组]          只看不发布：打印会写进 nodes.json 的参考节点（策略组默认 OpenAi）
  ./policy.sh lock claude|codex 邮箱 节点 [-C 映射仓库的目录] [-y]
                                               把账号锁到节点，确认后发布（目录默认是 CCA_POLICY_DIR，没有就是 cca 仓库旁边的 cca-policy）
  ./policy.sh key [ls]                         列出各台机器的 deploy key：添加时间、最近一次拉取、别名
  ./policy.sh key add [公钥文件 | -] [别名]    加一把只读 deploy key：不写文件就用剪贴板里的公钥，不写别名就提示起一个
  ./policy.sh key rm 别名 [-y]                 删掉一把
  ./policy.sh key mv 别名 [新别名]             改别名
key 默认管 cca 里写的默认映射仓库，别的仓库加 -R 维护者/仓库
  ./policy.sh presence deploy root@主机 [-p SSH 端口] [-i 私钥] [--port 端口] [--dir 服务器上的目录] [-C 映射仓库的目录]
                                               经 SSH 用 docker compose 部署设备统计的服务器，把它写进映射（之后照常发布映射）
  ./policy.sh presence sync | status           把各台机器的 deploy key 同步成服务器的设备清单 / 看服务器的状态
U
	exit 1
}

# ---------------------------------------------------------------------------
# deploy key：每台机器一把只读 key（cca policy setup 生成，公钥备注是 cca-policy@机器名），别名记在 key 的标题里
# ---------------------------------------------------------------------------
MINE="${CCA_HOME:-$HOME/.cca}/policy/id_ed25519.pub"   # 这台机器自己的
REPO="" YES="" KEYS="" LINE="" FP="" GH_ERR="" TMPD="" TTY_OK=0 PDIR="" P_URL="" P_PINS="" P_ERR="" P_N=0 P_HEALTH=""

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
	repo_pick
	TMPD=$(mktemp -d)
	trap 'rm -rf "$TMPD"' EXIT
	open_tty
	set -- ${args[@]+"${args[@]}"}
	case "${1:-ls}" in
		ls|list) key_ls ;;
		add) shift; key_add "$@"; presence_after ;;
		rm|remove) shift; key_rm "$@"; presence_after ;;
		mv|rename) shift; key_mv "$@"; presence_after ;;
		*) usage ;;
	esac
}
repo_pick() {   # REPO：-R 写的，其次环境变量 CCA_POLICY_DEFAULT（有定义就用，空的也算），最后是 cca 里写的默认仓库
	if [ -z "$REPO" ]; then
		if [ -n "${CCA_POLICY_DEFAULT+x}" ]; then REPO="$CCA_POLICY_DEFAULT"
		else REPO=$(sed -n 's/^POLICY_REPO_DEFAULT="\${CCA_POLICY_DEFAULT-\([^}]*\)}"$/\1/p' "$HERE/cca"); fi
	fi
	[[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail "不知道是哪个映射仓库：加上 -R 维护者/仓库"
	command -v gh >/dev/null 2>&1 || fail "要用 GitHub 的命令行工具 gh：brew install gh，再运行 gh auth login"
	command -v python3 >/dev/null 2>&1 || fail "要用 python3：先运行 xcode-select --install"
}

# ---------------------------------------------------------------------------
# 参考节点：维护者配置里的一个策略组（默认 OpenAi）和它引用的节点，各台机器补进自己配置里缺的地方
# ---------------------------------------------------------------------------
GROUP="${CCA_FILL_GROUP:-OpenAi}"
nodes_extract() {   # nodes_extract 参考配置 策略组：打印 nodes.json 的内容（说明写 stderr）。组里的策略组、内置出站不算节点
	ruby -E UTF-8 -ryaml -rjson -e '
		s = File.read(ARGV[0], encoding: "UTF-8")
		c = begin
		  YAML.load(s, aliases: true)
		rescue ArgumentError
		  YAML.load(s)
		end
		abort "✗ #{ARGV[0]} 不像 Clash 的配置" unless c.is_a?(Hash)
		g = (c["proxy-groups"] || []).find { |x| x.is_a?(Hash) && x["name"].to_s == ARGV[1] }
		abort "✗ #{ARGV[0]} 里没有叫「#{ARGV[1]}」的策略组（别的组名用 CCA_FILL_GROUP=组名 指定）" unless g
		px = {}
		(c["proxies"] || []).each { |p| px[p["name"].to_s] ||= p if p.is_a?(Hash) }
		mem = (g["proxies"] || []).map(&:to_s).uniq
		out = mem.select { |n| px[n] }.map { |n| px[n].merge("name" => n) }
		abort "✗ 策略组「#{ARGV[1]}」里没有节点（只有策略组、内置出站，或者节点来自 proxy-providers）" if out.empty?
		skip = mem.reject { |n| px[n] }
		$stderr.puts "• 参考节点：策略组「#{ARGV[1]}」里的 #{out.size} 个节点" + (skip.empty? ? "" : "（#{skip.join("、")} 不是节点，不算）")
		puts JSON.pretty_generate("version" => 1, "group" => ARGV[1], "proxies" => out)' "$1" "$2"
}

# ---------------------------------------------------------------------------
# 设备统计的服务器（presence）：各台机器经 cca 用映射里的账号时报告给它，App 的账号页显示这个账号连着哪些设备。
# 服务端是 server/cca-presence.py；各台机器用自己的 deploy key 签名，服务器只认发布密钥签的设备清单（命名空间 cca-devices）
# ---------------------------------------------------------------------------
PRESENCE_USAGE="用法：./policy.sh presence deploy root@主机 [-p SSH 端口] [-i 私钥] [--port 端口] [--dir 服务器上的目录] [-C 映射仓库的目录]
      ./policy.sh presence sync | status [-C 映射仓库的目录] [-R 仓库]"
policy_dir() { printf '%s' "${PDIR:-${CCA_POLICY_DIR:-$HERE/../cca-policy}}"; }
mapping_repo() {   # mapping_repo 目录：映射目录推送到的 GitHub 仓库（维护者/仓库）；不是 git 仓库、没有 origin 时是空的
	git -C "$1" remote get-url --push origin 2>/dev/null |
		sed -E 's#^(git@github\.com:|ssh://git@(ssh\.)?github\.com(:443)?/|https://github\.com/)##; s#\.git$##' || true
}
release_pub() {   # 旁边那份 cca 里的发布公钥（类型 内容），和 $KEY.pub 不是一对就退出：用这把签的东西谁也不认
	local pub
	pub=$(sed -n 's/^RELEASE_KEY="\(.*\)"$/\1/p' "$HERE/cca" | cut -d' ' -f1,2)
	[ -n "$pub" ] && [ "$(cut -d' ' -f1,2 "$KEY.pub" 2>/dev/null || true)" = "$pub" ] ||
		fail "cca 里的 RELEASE_KEY 和 $KEY.pub 不是一对：用这把签的映射、设备清单谁也不认"
	printf '%s' "$pub"
}   # 映射仓库的目录（和 lock 一样：-C、CCA_POLICY_DIR、cca 仓库旁边的 cca-policy）
presence_conf() {   # presence_conf 映射目录：P_URL、P_PINS（curl 的写法，; 隔开）从 policy.json 的 presence 一节读；没有返回 1
	local out
	out=$(python3 -I - "$1/policy.json" 2>/dev/null <<'PY'
import json, sys
try:
    p = json.load(open(sys.argv[1], encoding="utf-8")).get("presence")
except Exception:
    p = None
if isinstance(p, dict) and isinstance(p.get("url"), str) and isinstance(p.get("pins"), list):
    print(p["url"].rstrip("/"))
    print(";".join(x for x in p["pins"] if isinstance(x, str)))
PY
) || true
	P_URL=$(printf '%s\n' "$out" | sed -n 1p); P_PINS=$(printf '%s\n' "$out" | sed -n 2p)
	[ -n "$P_URL" ] && [ -n "$P_PINS" ]
}
presence_curl() {   # presence_curl 路径 [文件 签名文件]：直连、钉住证书公钥（自签的证书，不验 CA）；输出 body，最后一行是 HTTP 状态码
	local args=(curl -q -sS -k --proxy '' --noproxy '*' --proto =https --max-redirs 0 --connect-timeout 8 -m 20 --pinnedpubkey "$P_PINS" -w '\n%{http_code}')
	if [ -n "${2:-}" ]; then
		args+=(-H 'Content-Type: application/json' -H "X-Cca-Sig: $(base64 < "$3" | tr -d '\n')" --data-binary "@$2")
	fi
	"${args[@]}" "$P_URL$1" 2>"$TMPD/curl.err"
}
presence_down() {   # presence_down curl 的退出码：连不上的原因
	local e
	e=$(sed -n '$p' "$TMPD/curl.err" 2>/dev/null || true)
	case "$1" in
		90) echo "服务器的证书和映射里钉住的指纹对不上（${P_URL}）：可能连到了别的机器，或者服务器换了证书" ;;
		6|7|28) echo "连不上 ${P_URL}：服务没起来，或者云服务商的安全组 / 防火墙还没放行这个 TCP 端口${e:+（${e}）}" ;;
		*) echo "连 ${P_URL} 出错（curl ${1}${e:+：${e}}）" ;;
	esac
}
presence_health() {   # 问服务器的 /v1/health：那一行 JSON 放在 P_HEALTH；连不上时 P_ERR 是原因。别放在 $(…) 里调（P_ERR 会留在子 shell 里）
	local out rc=0
	P_HEALTH=""
	out=$(presence_curl /v1/health) || rc=$?
	if [ "$rc" != 0 ]; then P_ERR=$(presence_down "$rc"); return 1; fi
	if [ "$(printf '%s' "$out" | tail -n 1)" != 200 ]; then P_ERR="服务器回了 HTTP $(printf '%s' "$out" | tail -n 1)"; return 1; fi
	P_HEALTH=$(printf '%s' "$out" | sed '$d')
}
presence_sync() {   # 把仓库上各台机器的 deploy key（只读、启用的 ed25519）作为设备清单，用发布密钥签名后推给服务器；没成时 P_ERR 是原因
	local dir mrepo out rc=0 code
	dir=$(policy_dir)
	presence_conf "$dir" || { P_ERR="${dir}/policy.json 里还没有设备统计的服务器（先 ./policy.sh presence deploy 主机）"; return 1; }
	mrepo=$(mapping_repo "$dir")
	if [ -n "$mrepo" ] && [ "$mrepo" != "$REPO" ]; then   # 别的仓库的 key 推上去，服务器就不认这个映射的机器了
		P_ERR="映射目录 ${dir} 推送到的是 ${mrepo}，不是 ${REPO}：设备清单要用 ${mrepo} 上的 key（./policy.sh presence sync -R ${mrepo}）"
		return 1
	fi
	[ -f "$KEY" ] || { P_ERR="没有发布私钥 ${KEY}：设备清单要用它签名"; return 1; }
	keys_load || { P_ERR="读不到 ${REPO} 上的 deploy key：${GH_ERR}"; return 1; }
	presence_health || return 1
	printf '%s\n' "$KEYS" > "$TMPD/keys.tsv"
	# 设备清单：序号比服务器上的大；标题重复的（不是经 policy.sh 加的 key）加上编号，服务器要求不重复。stdout 是有几台
	P_N=$(python3 -I - "$TMPD/keys.tsv" "$TMPD/machines.json" "$P_HEALTH" <<'PY'
import json, sys, time
try:
    seq = max(int(time.time()), int(json.loads(sys.argv[3]).get("seq") or 0) + 1)
except (ValueError, AttributeError):
    seq = int(time.time())
ms, names = [], set()
for line in open(sys.argv[1], encoding="utf-8"):
    f = line.rstrip("\n").split("\t")
    if len(f) < 7 or f[5] != "ro" or f[6] != "on" or not f[2].startswith("ssh-ed25519 "):
        continue
    base = " ".join(f[1].split())[:60] or "mac"
    name, n = base, 2
    while name in names:
        name, n = "%s (%d)" % (base, n), n + 1
    names.add(name)
    ms.append({"key": f[2], "name": name})
with open(sys.argv[2], "w", encoding="utf-8") as out:
    out.write(json.dumps({"version": 1, "seq": seq, "machines": ms}, ensure_ascii=False, separators=(",", ":")))
print(len(ms))
PY
) || { P_ERR="设备清单生成失败"; return 1; }
	rm -f "$TMPD/machines.json.sig"
	ssh-keygen -Y sign -q -n cca-devices -f "$KEY" "$TMPD/machines.json" 2>/dev/null || { P_ERR="用 ${KEY} 签名失败"; return 1; }
	out=$(presence_curl /v1/machines "$TMPD/machines.json" "$TMPD/machines.json.sig") || rc=$?
	code=$(printf '%s' "$out" | tail -n 1)
	[ "$rc" = 0 ] || { P_ERR=$(presence_down "$rc"); return 1; }
	case "$code" in
		200) return 0 ;;
		403) P_ERR="服务器不认这份签名：它装的发布公钥和 ${KEY} 不是一对（重新运行 ./policy.sh presence deploy）" ;;
		409) P_ERR="服务器上的设备清单比这份还新：稍后再运行 ./policy.sh presence sync" ;;
		*) P_ERR="服务器回了 HTTP ${code}" ;;
	esac
	return 1
}
presence_after() {   # key add / rm / mv 之后：映射里配置了设备统计的服务器，就把新的设备清单推过去
	presence_conf "$(policy_dir)" || return 0
	if presence_sync; then echo "✓ 设备统计的服务器认得的设备跟着更新了（${P_N} 台）"
	else echo "! 设备清单没同步到服务器：${P_ERR}（修好后运行 ./policy.sh presence sync）"; fi
}
presence_remote() {   # presence_remote 服务器上的目录 端口 发布公钥：在服务器上运行的安装脚本（root；文件以 base64 带过去）
	local f
	cat <<SH
set -eu
D='${1}'
fail() { echo "✗ \$*"; exit 1; }
[ "\$(id -u)" = 0 ] || fail "要用 root 登录（目录在 \${D}，还要用 docker）"
command -v docker >/dev/null 2>&1 || fail "服务器上没有 docker"
docker compose version >/dev/null 2>&1 || fail "服务器上没有 docker compose（Docker 的 compose 插件）"
command -v openssl >/dev/null 2>&1 || fail "服务器上没有 openssl（第一次生成证书要用）"
mkdir -p "\$D/app" "\$D/config" "\$D/certs" "\$D/data"
put() { printf '%s' "\$2" | base64 -d > "\$D/\$1.new.\$\$" && chmod 644 "\$D/\$1.new.\$\$" && mv -f "\$D/\$1.new.\$\$" "\$D/\$1"; }
SH
	for f in compose.yaml README.md; do printf "put %s '%s'\n" "$f" "$(base64 < "$HERE/server/$f" | tr -d '\n')"; done
	printf "put app/cca-presence.py '%s'\n" "$(base64 < "$HERE/server/cca-presence.py" | tr -d '\n')"
	printf "put config/release.pub '%s'\n" "$(printf '%s\n' "$3" | base64 | tr -d '\n')"
	printf "PORT='%s'\n" "$2"
	cat <<'SH'
if [ ! -f "$D/certs/cert.pem" ]; then   # 第一次：自签的 P-256 证书（macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住），私钥不出服务器
	openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 -subj /CN=cca-presence \
		-keyout "$D/certs/key.pem" -out "$D/certs/cert.pem" >/dev/null 2>&1 || fail "生成证书失败"
	echo "生成了新的证书（自签 P-256，10 年）"
fi
chown -R 10001:10001 "$D/certs" "$D/data"   # 容器里用 10001 运行（compose.yaml 的 user）
chmod 700 "$D/certs" "$D/data"; chmod 600 "$D/certs/key.pem"; chmod 644 "$D/certs/cert.pem"; chmod 755 "$D" "$D/app" "$D/config"
cd "$D"
# 官方的 python 镜像：上次用的那个，再依次试官方镜像的几个同步源（国内常常连不上 Docker Hub）。不改 Docker 的全局配置
img=""
for i in $(sed -n 's/^CCA_PRESENCE_IMAGE=//p' .env 2>/dev/null) public.ecr.aws/docker/library/python:3.13-slim \
	mirror.ccs.tencentyun.com/library/python:3.13-slim docker.m.daocloud.io/library/python:3.13-slim python:3.13-slim; do
	if timeout 300 docker pull -q "$i" >/dev/null 2>&1 || docker image inspect "$i" >/dev/null 2>&1; then img="$i"; break; fi
done
[ -n "$img" ] || fail "拉不到 python:3.13-slim 镜像（Docker Hub 和几个同步源都连不上）：给 Docker 配上镜像加速，或者在别处 docker save 一份再 docker load 进来"
printf 'CCA_PRESENCE_PORT=%s\nCCA_PRESENCE_IMAGE=%s\n' "$PORT" "$img" > .env.new && mv -f .env.new .env
docker compose up -d --remove-orphans --force-recreate >/dev/null 2>&1 || { docker compose up -d --force-recreate; fail "docker compose up 失败"; }
sleep 3
[ "$(docker inspect -f '{{.State.Running}}' cca-presence 2>/dev/null)" = true ] || { echo "✗ 容器没起来："; docker compose logs --tail 20; exit 1; }
echo "容器 cca-presence 在跑（${D}，镜像 $(docker inspect -f '{{.Config.Image}}' cca-presence 2>/dev/null)）"
docker compose logs --tail 1 2>/dev/null | sed 's/^[^|]*| *//'
echo "PIN sha256//$(openssl x509 -in "$D/certs/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | openssl base64 -A)"
SH
}
presence_write() {   # presence_write 目录 网址 指纹：把 presence 一节写进 policy.json。盐只在第一次生成；证书换了时旧的指纹也留着，各台机器拉到新映射前照样连得上
	python3 -I - "$1/policy.json" "$2" "$3" <<'PY'
import json, os, secrets, sys
path, url, pin = sys.argv[1:4]
j = json.load(open(path, encoding="utf-8"))
old = j.get("presence") if isinstance(j.get("presence"), dict) else {}
salt = old.get("salt") if isinstance(old.get("salt"), str) and len(old["salt"]) >= 32 else secrets.token_hex(16)
pins = [pin] + ([p for p in old.get("pins") or [] if isinstance(p, str) and p != pin][:1] if old.get("url") == url else [])
j["presence"] = {"url": url, "pins": pins, "salt": salt}
with open(path + ".tmp", "w", encoding="utf-8") as f:
    json.dump(j, f, ensure_ascii=False, indent=2)
    f.write("\n")
os.replace(path + ".tmp", path)
PY
}
presence_deploy() {   # presence_deploy [用户@]主机 [-p SSH 端口] [-i 私钥] [--port 端口]
	local target="" sshport="" ident="" port=18443 rdir=/root/projects/cca-presence dir host hp pub out rc=0 pin ssh_args
	while [ $# -gt 0 ]; do
		case "$1" in
			-p) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; sshport="$2"; shift ;;
			-i) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; ident="$2"; shift ;;
			--port) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; port="$2"; shift ;;
			--dir) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; rdir="$2"; shift ;;
			-*) fail "$PRESENCE_USAGE" ;;
			*) [ -z "$target" ] || fail "$PRESENCE_USAGE"; target="$1" ;;
		esac
		shift
	done
	[ -n "$target" ] || fail "$PRESENCE_USAGE"
	[[ "$port" =~ ^[0-9]{1,5}$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ] && [ "$port" != 80 ] && [ "$port" != 443 ] ||
		fail "--port 要是 1～65535 的端口，不用 80、443（国内没备案的服务器这两个端口会被拦）"
	host=${target#*@}
	[[ "$host" =~ ^[A-Za-z0-9.:-]+$ ]] || fail "主机写错了：${host}"
	[[ "$rdir" =~ ^/[A-Za-z0-9._/-]+$ ]] && [[ "$rdir" != *..* ]] || fail "--dir 要是服务器上的绝对路径（只用字母、数字和 . _ - /）"
	case "$host" in *:*) hp="[${host}]" ;; *) hp="$host" ;; esac
	dir=$(policy_dir)
	[ -f "$dir/policy.json" ] || fail "${dir} 里没有 policy.json：用 -C 指定映射仓库的目录（默认是 cca 仓库旁边的 cca-policy）"
	dir=$(cd "$dir" && pwd)
	pub=$(release_pub) || exit 1
	ssh_args=(-o BatchMode=yes -o ConnectTimeout=15)
	[ -z "$ident" ] || ssh_args+=(-i "$ident" -o IdentitiesOnly=yes)
	[ -z "$sshport" ] || ssh_args+=(-p "$sshport")
	echo "• 部署设备统计的服务器到 ${target}:${rdir}（docker compose，端口 ${port}）…"
	out=$(presence_remote "$rdir" "$port" "$pub" | ssh "${ssh_args[@]}" "$target" 'sh -s' 2>&1) || rc=$?
	printf '%s\n' "$out" | grep -v '^PIN ' | sed 's/^/  /' || true
	pin=$(printf '%s\n' "$out" | sed -n 's/^PIN //p' | tail -n 1)
	[ "$rc" = 0 ] && [[ "$pin" =~ ^sha256//[A-Za-z0-9+/]{43}=$ ]] || fail "部署没完成（上面是服务器那边的输出）"
	presence_write "$dir" "https://${hp}:${port}" "$pin" || fail "没能写进 ${dir}/policy.json"
	presence_conf "$dir" || fail "写进 ${dir}/policy.json 的 presence 一节读不出来"
	echo "✓ 已写进 ${dir}/policy.json：presence = ${P_URL}，证书公钥指纹 ${pin}"
	if presence_health; then
		printf '%s' "$P_HEALTH" | grep -q '"sig_ok":true' || echo "! 服务器上的验签自检没通过：各台机器的上报会被拒绝（在服务器上 docker compose logs 看看）"
		echo "✓ 从这台 Mac 连得上（证书和指纹对得上）"
		if presence_sync; then echo "✓ 设备清单已推给服务器（${P_N} 台）"
		else echo "! 设备清单没推上去：${P_ERR}（修好后运行 ./policy.sh presence sync）"; fi
	else
		echo "! 从这台 Mac 连不上：${P_ERR}"
		echo "  腾讯云、阿里云等要在控制台的安全组（或轻量应用服务器的防火墙）里放行 TCP ${port}，放行后运行 ./policy.sh presence sync"
	fi
	echo "• 下一步：发布映射，各台机器拉到后就开始报告：./policy.sh '${dir}' '设备统计的服务器'"
}
presence_status() {
	presence_conf "$(policy_dir)" || fail "$(policy_dir)/policy.json 里还没有设备统计的服务器：先运行 ./policy.sh presence deploy root@主机"
	presence_health || fail "$P_ERR"
	printf '%s' "$P_HEALTH" | python3 -I -c 'import json, sys
j = json.load(sys.stdin)
print("✓ 设备统计的服务器在（%s）：认得 %d 台设备（设备清单 #%s），签名自检%s" % (sys.argv[1], j.get("machines") or 0, j.get("seq"),
      "通过" if j.get("sig_ok") else "没通过：各台机器的上报都会被拒绝"))' "$P_URL"
}
presence_cmd() {
	local sub="${1:-}" args=()
	[ $# -eq 0 ] || shift
	while [ $# -gt 0 ]; do
		case "$1" in
			-C) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; PDIR="$2"; shift 2 ;;
			-R|--repo) [ $# -ge 2 ] || fail "$PRESENCE_USAGE"; REPO="$2"; shift 2 ;;
			*) args+=("$1"); shift ;;
		esac
	done
	command -v python3 >/dev/null 2>&1 || fail "要用 python3：先运行 xcode-select --install"
	TMPD=$(mktemp -d)
	trap 'rm -rf "$TMPD"' EXIT
	set -- ${args[@]+"${args[@]}"}
	case "$sub" in
		deploy) repo_pick; presence_deploy "$@" ;;
		sync) [ $# -eq 0 ] || fail "$PRESENCE_USAGE"; repo_pick
			if presence_sync; then echo "✓ 设备清单已推给服务器（${P_N} 台）"; else fail "$P_ERR"; fi ;;
		status) [ $# -eq 0 ] || fail "$PRESENCE_USAGE"; presence_status ;;
		*) fail "$PRESENCE_USAGE" ;;
	esac
}

# ---------------------------------------------------------------------------
# 把账号锁到节点：cca policy request 生成的就是这条命令（发给维护者在这里运行），确认之后写进映射，后面照常发布
# ---------------------------------------------------------------------------
LOCK_DIR="" LOCK_MSG=""
LOCK_USAGE="用法：./policy.sh lock claude|codex 邮箱 节点 [-C 映射仓库的目录] [-y]"
lock_account() {   # lock_account 工具 邮箱 节点 [-C 目录] [-y]：写好 policy.json，发布要用的 LOCK_DIR、LOCK_MSG 留给后面
	local dir yes="" tool="" email="" node="" plan rc=0
	while [ $# -gt 0 ]; do
		case "$1" in
			-y) yes=1 ;;
			-C) [ $# -ge 2 ] || fail "$LOCK_USAGE"; PDIR="$2"; shift ;;
			*) if [ -z "$tool" ]; then tool="$1"; elif [ -z "$email" ]; then email="$1"; elif [ -z "$node" ]; then node="$1"; else fail "$LOCK_USAGE"; fi ;;
		esac
		shift
	done
	[ -n "$node" ] || fail "$LOCK_USAGE"
	dir=$(policy_dir)
	[ -f "$dir/policy.json" ] || fail "${dir} 里没有 policy.json：用 -C 指定映射仓库的目录，或者设环境变量 CCA_POLICY_DIR（默认是 cca 仓库旁边的 cca-policy）"
	dir=$(cd "$dir" && pwd)
	open_tty
	plan=$(bash "$HERE/cca" _policy-lock plan "$dir/policy.json" "$tool" "$email" "$node") || rc=$?
	case "$rc" in
		0) ;;
		3) printf '%s\n' "$plan"
			if git -C "$dir" diff --quiet HEAD -- policy.json 2>/dev/null; then echo "• 已经是这样了，不用发布"
			else echo "• 已经是这样了，不过 policy.json 还有没发布的改动：运行 ./policy.sh ${dir} 发布"; fi
			exit 0 ;;
		*) fail "$plan" ;;
	esac
	echo "• 要写进映射（${dir}）："
	printf '%s\n' "$plan"
	if [ -z "$yes" ]; then
		[ "$TTY_OK" = 1 ] || fail "要先确认：在终端里运行，或者加 -y"
		case "$(ask "  写进 policy.json、签名并推送吗？所有配置了这个映射仓库的机器都会照做 [y/N]：" n)" in
			y|Y|yes) ;;
			*) echo "• 没有改"; exit 0 ;;
		esac
	fi
	LOCK_MSG=$(bash "$HERE/cca" _policy-lock apply "$dir/policy.json" "$tool" "$email" "$node") || fail "没能写进 policy.json：${LOCK_MSG}"
	LOCK_DIR="$dir"
}

case "${1:-}" in
	key|keys) shift; key_cmd "$@"; exit 0 ;;
	presence) shift; presence_cmd "$@"; exit 0 ;;
	lock) shift; lock_account "$@"; set -- "$LOCK_DIR" "$LOCK_MSG" ;;
	nodes)
		[ -f "${2:-}" ] || fail "用法：./policy.sh nodes 参考配置 [策略组]"
		command -v ruby >/dev/null 2>&1 || fail "要用 ruby 读 Clash 的配置（macOS 自带；没有的话 brew install ruby）"
		nodes_extract "$2" "${3:-$GROUP}"; exit 0 ;;
	''|-h|--help|help) usage ;;
esac

# ---------------------------------------------------------------------------
# 发布映射
# ---------------------------------------------------------------------------
[ -f "$1/policy.json" ] || fail "${1} 里没有 policy.json（用法：./policy.sh 映射仓库的目录 [\"提交说明\"]）"
dir=$(cd "$1" && pwd)   # 先换成绝对路径，再进 cca 的仓库
cd "$HERE"
pub=$(release_pub) || exit 1
out=$(bash ./cca _policy-check "$dir/policy.json") || fail "policy.json 有问题：
${out}"

# 参考节点：目录里有 custom.yaml 就重新抽一遍（custom.yaml 本身绝不提交）；只有 nodes.json 时照它发布
if [ -f "$dir/custom.yaml" ]; then
	! git -C "$dir" ls-files --error-unmatch custom.yaml >/dev/null 2>&1 ||
		fail "custom.yaml 已经提交进映射仓库了：里面是你的整份配置（规则、局域网设备……），每台机器都拉得到。先运行 git -C '${dir}' rm --cached custom.yaml 再发布（推上去过的话，历史里的也要清掉）"
	ex=$(git -C "$dir" rev-parse --git-path info/exclude) || fail "${dir} 不是 git 仓库"
	case "$ex" in /*) ;; *) ex="$dir/$ex" ;; esac
	grep -qx '/custom.yaml' "$ex" 2>/dev/null || { mkdir -p "${ex%/*}" && echo '/custom.yaml' >> "$ex"; }   # 以后 git add -A 也不会带上它
	command -v ruby >/dev/null 2>&1 || fail "要用 ruby 读 custom.yaml（macOS 自带；没有的话 brew install ruby）"
	nodes_extract "$dir/custom.yaml" "$GROUP" > "$dir/nodes.json.tmp" || { rm -f "$dir/nodes.json.tmp"; exit 1; }
	mv -f "$dir/nodes.json.tmp" "$dir/nodes.json"
fi
if [ -f "$dir/nodes.json" ]; then
	out=$(bash ./cca _fill-check "$dir/nodes.json") || fail "nodes.json 有问题：
${out}"
fi

# 映射里有邮箱和节点名：只往私有仓库推（看的是 origin 实际推送的地址）
repo=$(mapping_repo "$dir"); [ -n "$repo" ] || fail "${dir} 不是 git 仓库，或者没有 origin"
vis=$(gh repo view "$repo" --json visibility -q .visibility 2>/dev/null) || fail "确认不了 ${repo} 是不是私有仓库（gh 登录了吗？）"
[ "$vis" = PRIVATE ] || fail "${repo} 不是私有仓库（${vis}），映射不能推上去"

rm -f "$dir/policy.json.sig"
ssh-keygen -Y sign -q -n cca-policy -f "$KEY" "$dir/policy.json"
allowed=$(mktemp)
trap 'rm -f "$allowed"' EXIT
printf 'cca-release %s\n' "$pub" > "$allowed"
ssh-keygen -Y verify -f "$allowed" -I cca-release -n cca-policy -s "$dir/policy.json.sig" < "$dir/policy.json" >/dev/null ||
	fail "签名自检失败"
files=(policy.json policy.json.sig)
if [ -f "$dir/nodes.json" ]; then   # 参考节点用另一个命名空间签：两份文件的签名不能互相顶替
	rm -f "$dir/nodes.json.sig"
	ssh-keygen -Y sign -q -n cca-nodes -f "$KEY" "$dir/nodes.json"
	ssh-keygen -Y verify -f "$allowed" -I cca-release -n cca-nodes -s "$dir/nodes.json.sig" < "$dir/nodes.json" >/dev/null ||
		fail "参考节点的签名自检失败"
	files+=(nodes.json nodes.json.sig)
elif git -C "$dir" ls-files --error-unmatch nodes.json >/dev/null 2>&1; then   # 删掉了 nodes.json：不再发布参考节点
	rm -f "$dir/nodes.json.sig"
	files+=(nodes.json nodes.json.sig)
fi

git -C "$dir" add -A -- "${files[@]}"
if git -C "$dir" diff --cached --quiet; then echo "• 映射和参考节点都没有变化，不用推送"; exit 0; fi
git -C "$dir" commit -q -m "${2:-更新账号 → 节点映射}"
git -C "$dir" push -q origin HEAD
echo "✓ 已发布到 ${repo}：配置了这个仓库的 cca 下次拉取时换上（每天自动拉，cca policy pull 马上拉）"
