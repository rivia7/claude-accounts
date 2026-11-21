#!/bin/bash
# 全流程回归：端口池（槽位 + 策略组，API 切换不重载）、重载需同意、账号表、出口检测、登录校验、第三方 API 账号、
# 订阅用量、Codex 账号、自动更新、还原
# 用法：bash tests/regress.sh（只在 macOS 上跑；全部用假命令，不联网、不碰真实的 ~/.cca 和路由器）
D=$(cd "$(dirname "$0")" && pwd)
CCA="$D/../cca"; CCA=$(cd "$(dirname "$CCA")" && pwd)/cca
if [ -n "${CKPT:-}" ]; then mkdir -p "$CKPT"; T="${TMPDIR:-/tmp}/cca-ckpt.sandbox"; mkdir "$T.lock" 2>/dev/null || { echo "另一个分节续跑（CKPT/FROM/ONLY）在用同一个沙箱 ${T}：一次只能跑一个，先等它跑完，或删掉 $T.lock" >&2; exit 1; }; [ -n "${FROM:-}${ONLY:-}" ] || rm -rf "$T"; mkdir -p "$T"; else T=$(mktemp -d "${TMPDIR:-/tmp}/cca-test.XXXXXX"); fi; T=$(cd "$T" && pwd -P); H=$T/home; R=$T/router   # 规整成真实路径（TMPDIR 结尾有 / 时会出现 //）；CKPT 时用 $TMPDIR 下的固定沙箱 + $T.lock 防并发（分节续跑：路径要稳定、在 /var/folders 下、一次只能一个）
STUN="" STUN_PID=""   # 假的 STUN 服务器（泄露检测那一节才开）；平时 CCA_STUN 指向不通的端口：UDP 出不去，马上就知道
SVP=""   # 设备统计的测试服务器（11b3c 那一节才开）
MBP=""   # 邮箱助手的假 Gmail（3b3 那一节才开）
HC=""    # 辅助程序的缓存目录（10g 测“解出来”的那几个用例才建；在这个用户的临时目录里）
trap '[ -z "$HC" ] || rm -rf "$HC"; [ -z "$SVP" ] || kill "$SVP" 2>/dev/null; [ -z "$MBP" ] || kill "$MBP" 2>/dev/null; [ -z "$STUN_PID" ] || kill "$STUN_PID" 2>/dev/null; [ ! -f "$T/core/up" ] || kill "$(cat "$T/core/pid")" 2>/dev/null; [ -n "${KEEP:-}" ] && echo "沙箱保留在 $T" || rm -rf "$T"; [ -z "${CKPT:-}" ] || rmdir "$T.lock" 2>/dev/null' EXIT
[ -n "${FROM:-}${ONLY:-}" ] || bash "$D/setup.sh" "$T" >/dev/null || exit 1   # 分节续跑时跳过建沙箱：快照恢复会填好 $T
cd "$T" || exit 1
# 辅助程序（cli/ 的 Rust 程序）：仓库里的 cca 用 cli/target/release 里编出来的那一份，这里先编（没改过就是零点几秒；--locked --offline：
# 照 Cargo.lock、不联网，依赖没下过的先在 cli/ 里 cargo fetch 一次）。CCA_HELPER=路径：不编，整套回归用这一份（release.sh 用它
# 指到要发布的那一份）：仓库里的 cca 认这个变量，mkver 做的假发布版接的也是这一份（发布的 cca 不认这个变量，总是用自己接着的）。
# HOST_TARGET：这台机器的目标（见 cli/pack.py）
HELPER_BIN="${CCA_HELPER:-}"
if [ -z "$HELPER_BIN" ]; then
	HELPER_BIN="$(dirname "$CCA")/cli/target/release/cca-core"
	( cd "$(dirname "$CCA")/cli" && cargo build --release --locked --offline ) > "$T/cargo.log" 2>&1 ||
		{ tail -n 30 "$T/cargo.log" >&2; echo "辅助程序编不出来（cd cli && cargo build --release）" >&2; exit 1; }
fi
[ -x "$HELPER_BIN" ] || { echo "辅助程序不在：$HELPER_BIN" >&2; exit 1; }
case "$(uname -m)" in arm64) HOST_TARGET=aarch64-apple-darwin ;; *) HOST_TARGET=x86_64-apple-darwin ;; esac
# TIMING=文件：找慢在哪。每个 ok 记一行“距上一个 ok 几秒<TAB>调用 ok 的行号<TAB>这段时间 cca 起了几次辅助程序<TAB>sleep 了几秒<TAB>说明”；
# 次数靠 CCA_HELPER 指向的一个记数的小脚本（只在 TIMING 时用；11b 以后装着的是假发布版，它不认 CCA_HELPER，那以后的次数不记），sleep 靠 $T/bin 里的 sleep
#（cca、假命令、经假 ssh 跑的路由器端都从 PATH 找它；后台进程的也算在内）
HELPERWRAP="${CCA_HELPER:-}"
if [ -n "${TIMING:-}" ]; then
	: > "$TIMING"; : > "$T/py.count"; TIMING_PY=0
	HELPERWRAP="$T/pycount"
	# 每次记一行：py 的子命令和它的参数（长的、带换行的不记）
	printf '#!/bin/dash\nfor a in "$@"; do case $a in *"\n"*) ;; *) [ ${#a} -lt 120 ] && printf "%%s " "$a" ;; esac; done >> "%s/py.count"; echo >> "%s/py.count"\nexec "%s" "$@"\n' \
		"$T" "$T" "$HELPER_BIN" > "$HELPERWRAP"
	chmod 755 "$HELPERWRAP"
	printf '#!/bin/dash\necho "$1" >> "%s/sleep.log"\nexec /bin/sleep "$@"\n' "$T" > "$T/bin/sleep"; chmod 755 "$T/bin/sleep"; : > "$T/sleep.log"
	TIMING_T=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
fi
run() { env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_RESOLV_CONF="$T/resolv.conf" CCA_POLICY_DEFAULT= CCA_STUN="${STUN:-127.0.0.1:9}" CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" CCA_HELPER="$HELPERWRAP" CCA_RETRY_WAIT=0 CCA_DOCK_PREFS="$T/dock/dock.plist" CCA_DOCK_SETTLE=0 PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
rr() { env CCA_ROOT="$R" CCA_T="$T" CCA_WAIT=2 HOME="$R/root" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" /bin/dash "$R/etc/cca/cca-router.sh" "$@"; }
ans() { f="$T/ans.$RANDOM$RANDOM"; printf '%s\n' "$@" > "$f"; echo "$f"; }
pass=0; fail=0
ok() {   # ok 说明 '断言'：断言写丢了（比如说明和断言之间少了空格，连成了一个参数）也算失败，不会空过。DEBUG_OUT=1 时失败的附上 $out 的最后几行
	if [ $# -eq 2 ] && [ -n "$2" ] && eval "$2"; then echo "PASS  $1"; pass=$((pass + 1)); else
		echo "FAIL  $1   [${2:-没有断言}]"; fail=$((fail + 1))
		[ -z "${DEBUG_OUT:-}" ] || ok_why "$2"
	fi
	if [ -n "${TIMING:-}" ]; then
		local now n s
		now=$(perl -MTime::HiRes=time -e 'printf "%.3f", time'); n=$(wc -l < "$T/py.count")
		s=$(awk '{ s += $1 } END { printf "%.1f", s }' "$T/sleep.log"); : > "$T/sleep.log"
		printf '%s\t%s\t%s\t%s\t%s\n' "$(perl -e "printf '%.2f', $now - $TIMING_T")" "${BASH_LINENO[0]}" $((n - TIMING_PY)) "$s" "$1" >> "$TIMING"
		TIMING_T=$now TIMING_PY=$((n))
	fi
}
ok_why() {   # DEBUG_OUT 时：$out 的最后几行，和断言里（按顶层的 && 拆开）第一个不成立的那一段
	local part
	printf '%s\n' "${out:-}" | tail -n 25 | sed 's/^/      | /'
	while IFS= read -r -d '' part; do
		eval "$part" >/dev/null 2>&1 || { printf '      ✗ %s\n' "$part"; break; }
	done < <(python3 -I -c '
import sys
s, parts, cur, q, i = sys.argv[1], [], "", None, 0
while i < len(s):
    c = s[i]
    if q:
        cur += c
        if c == "\\" and q == "\"" and i + 1 < len(s):
            cur += s[i + 1]; i += 1
        elif c == q:
            q = None
    elif c in "\x27\"":
        q = c; cur += c
    elif s.startswith(" && ", i):
        parts.append(cur); cur = ""; i += 3
    else:
        cur += c
    i += 1
parts.append(cur)
sys.stdout.write("".join(p.strip() + "\0" for p in parts))' "$1")
}
edit_table() { python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); $1; json.dump(j,open(p,'w'),indent=2,ensure_ascii=False)" "${2:-$H/.cca/accounts.json}"; }   # [账号表]
RC="$R/etc/openclash/custom.yaml"
nl_() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); print((c["listeners"]||[]).count { |l| l["name"].start_with?("cca-") })' "$RC"; }
first() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); g=(c["proxy-groups"]||[]).find { |x| x["name"]==ARGV[1] }; print(g ? g["proxies"][0] : "-")' "$RC" "$1"; }
sel() { cat "$R/sel/$1" 2>/dev/null || echo -; }
unlisten() { grep -vx "$1" "$R/listening" > "$R/l.tmp"; mv "$R/l.tmp" "$R/listening"; }   # 路由器上这个端口不再监听
reloads() { grep -c '^RELOAD' router.log 2>/dev/null || echo 0; }
restarts() { grep -c '^RESTART' router.log 2>/dev/null || echo 0; }   # OpenClash 重启（假的 init 脚本记的）
cfgv() { python3 -c "import json,sys; c=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))" "$H/.cca/config.json" "$1"; }
cfgok() { python3 -c "import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)" "$H/.cca/config.json" "$1"; }   # config.json（c）满足条件
tblok() { python3 -c "import json,sys; t=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)" "${2:-$H/.cca/accounts.json}" "$1"; }   # 账号表（t）满足条件
# 图形界面（cca App）的接口（macos/PROTOCOL.md）：gj = cca _gui …；jok 断言 stdin 的 JSON；事件文件（CCA_EVENTS）的断言
gj() { run cca _gui "$@" </dev/null 2>/dev/null; }
jok() {   # jok '条件' < JSON：stdin 的 JSON（d）满足条件。条件里 tab("accounts.json") 读 ~/.cca 里的文件（保持键的顺序），cfg 是 config.json，
          # acct(工具序号, 键) 是 _gui state 里的账号，uacct(键) 是 usage --json 里的账号
	python3 -c 'import json,sys; H=sys.argv[2]; tab=lambda n: json.load(open(H+"/"+n)); cfg=tab("config.json"); d=json.load(sys.stdin); acct=lambda t, k: next(a for a in d["tools"][t]["accounts"] if a["key"] == k); uacct=lambda k: next(a for a in d["accounts"] if a["key"] == k); sys.exit(0 if eval(sys.argv[1]) else 1)' "$1" "$H/.cca"
}
evq() { python3 -c 'import json,sys; ev=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]; sys.exit(0 if any(eval(sys.argv[2]) for e in ev) else 1)' "$T/ev.jsonl" "$1"; }   # 事件文件里有一条（e）满足条件
evn() { grep -c "\"ev\":\"$1\"" "$T/ev.jsonl"; }   # 这种事件有几条
runev() { : > "$T/ev.jsonl"; run CCA_EVENTS="$T/ev.jsonl" "$@" </dev/null; }   # 带事件文件运行（stdin 不是终端：App 就是这样调的）
claude_device() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["userID"] + ":" + d["machineID"])' "$1/claude/.claude.json"; }
# 分节续跑（省开发时间；发版和平时照样跑全量，sect 恒为真）：
#   CKPT=目录 bash tests/regress.sh                       跑一次全量，在每节开头存下沙箱快照（tar $T + 变量）
#   CKPT=目录 FROM=10h bash tests/regress.sh              从某节恢复快照、只跑那节到结尾
#   CKPT=目录 ONLY="10h 11b4" bash tests/regress.sh       只跑这几节，各自从自己的快照起
# 不带 CKPT/FROM/ONLY 时 sect 恒返回 0，一行不漏地跑全量（下面每节都包在 if sect 节名; then … fi 里）。
sect() {
	local name=$1 _v _fn
	if [ -n "${CKPT:-}" ] && [ -z "${FROM:-}${ONLY:-}" ]; then   # 建快照：这一节开始时的沙箱 + 这时各节新设的变量和函数（跨节用到的 ${VS}、node_rn… 要带上）
		( cd "$T" && tar czf "$CKPT/$name.tgz" . ) 2>/dev/null
		# 只存各节新设的变量：compgen -v 现在的减去前置基线 ${_BASE}。按名字 declare -p（带引号、多行值都对；bash 3.2 不带名字的 declare -p 没有 declare 前缀，不能用）
		# 去掉 declare 前缀存成普通赋值：bash 3.2 没有 declare -g，恢复是在 _ckpt_restore 函数里 source 的，declare -- 会建成函数局部变量、一出函数就没了（跨节的 $GF/$VS 全丢），普通赋值才落到全局
		{ for _v in $(compgen -v 2>/dev/null); do case "$_BASE" in *" $_v "*) continue ;; esac; case "$_v" in name|_v|_fn|_BASE|_BASEF|_GO) continue ;; esac; declare -p "$_v" 2>/dev/null | sed '1s/^declare -[^ ]* //'; done; } > "$CKPT/$name.vars"
		# 各节新定义的函数也要带上（基线以后的；前置阶段的 ok/run/sect… 不动，续跑时用当前这份）
		{ for _fn in $(compgen -A function 2>/dev/null); do case "$_BASEF" in *" $_fn "*) continue ;; esac; declare -f "$_fn" 2>/dev/null; done; } > "$CKPT/$name.fns"
		return 0
	fi
	if [ -n "${ONLY:-}" ]; then   # 只跑列出的这几节，各自从自己的快照起
		case " $ONLY " in *" $name "*) _ckpt_restore "$name"; return $? ;; *) return 1 ;; esac
	fi
	if [ -n "${FROM:-}" ]; then   # 从某节恢复、跑到结尾
		[ -n "${_GO:-}" ] && return 0
		[ "$name" = "$FROM" ] && { _GO=1; _ckpt_restore "$name"; return $?; }
		return 1
	fi
	return 0
}
_ckpt_restore() {   # 恢复某节开始时的沙箱和变量；计数（pass/fail）接着本次续跑的、不取快照里的
	local name=$1 _p=$pass _f=$fail
	[ -f "$CKPT/$name.tgz" ] || { echo "FAIL  缺 $name 的快照：先 CKPT=$CKPT bash tests/regress.sh 建一次"; fail=$((fail + 1)); return 1; }
	find "$T" -mindepth 1 -delete 2>/dev/null
	tar xzf "$CKPT/$name.tgz" -C "$T"
	[ -f "$CKPT/$name.vars" ] && . "$CKPT/$name.vars" 2>/dev/null
	[ -f "$CKPT/$name.fns" ] && . "$CKPT/$name.fns" 2>/dev/null   # 各节新定义的函数（node_rn 这类跨节用的）
	pass=$_p; fail=$_f
	return 0
}
_BASE=" $(compgen -v 2>/dev/null | tr '\n' ' ')"   # 分节续跑的变量基线：前置阶段已有的变量名（建快照时只存这之后各节新设的）
_BASEF=" $(compgen -A function 2>/dev/null | tr '\n' ' ')"   # 函数基线：前置阶段已有的函数（ok/run/sect…；只存各节新定义的）
if sect unit; then
# 辅助程序内部的检查（原来这一节里各份 Python 测试直接调内嵌 Python 的函数查的那些：解析、规则、价格表、时段、客户端更新的每一步……）
# 都在 cargo test 里；下面各份 Python 测试把辅助程序当黑盒跑（tests/helper_bin.py），或者切出 cca 里的 bash 函数来测
ok "辅助程序（cli/）自己的单元测试：cargo test" '( cd "$D/../cli" && cargo test --locked --offline ) > "$T/cargo-test.log" 2>&1'
# 代码照 cli/rustfmt.toml 排过（工具链是固定的，排出来哪台机器都一样）：没排过的改动一合进来，以后谁顺手 cargo fmt 就是一大片不相干的改动。
# 生成的数据表标着 #[rustfmt::skip]，不参与
ok "辅助程序（cli/）的代码排过版：cargo fmt --check（没过就在 cli/ 里运行 cargo fmt）" '( cd "$D/../cli" && cargo fmt --check ) > "$T/cargo-fmt.log" 2>&1'
ok "设备标识：旧配置迁移、共享链接、客户端重置、改别名和并发初始化" 'python3 "$D/device_identity.py"'
ok "共享资料：历史记忆配置跨账号可见，未知状态隔离，配置合并和冲突副本保留" 'python3 "$D/shared_state.py"'
ok "浏览器隔离：不降级默认浏览器，预检和启动失败传递" 'python3 "$D/isolation_browser.py"'
ok "泄露检测：失败结果不误报通过，区分宿主探针与浏览器" 'python3 "$D/isolation_leaks.py"'
ok "本机 UDP 走节点：TUN 配置只把 UDP 交给节点，状态、选节点、要装到 root 那边的文件和摘要" 'python3 "$D/local_udp.py"'
ok "多设备共用路由器：只有这台设备时账号表逐字节不变，合并时保留别的设备的槽位、共用账号不重复、本机端口写回并集槽位" 'python3 "$D/router_merge.py"'
ok "配置隔离：完整 TOML、项目配置与启动参数检查" 'python3 "$D/isolation_settings.py"'
ok "凭证隔离：固定钥匙串目录，屏蔽继承凭证描述符" 'python3 "$D/isolation_credentials.py"'
ok "网络隔离：节点回读、失败阻断、探针不继承代理覆盖" 'python3 "$D/isolation_network.py"'
ok "运行记录：进程复用、代理漂移与目录链接检查" 'python3 "$D/isolation_runtime.py"'
ok "App 隔离：环境清理、启动锁、真实资料目录证据" 'python3 "$D/isolation_apps.py"'
ok "客户端更新（bash 那一半，cmd_clients）：安装要一字不差的令牌、有效的计划和用户的同意；征求同意时说清楚终端工具怎么更新" 'python3 "$D/client_updates.py"'
ok "卸载时去掉 shell 启动文件里 cca 加的那两行（bash 的 path_unhint）：逐字节还原、权限不变；~/.local/bin 里还有别的命令、用户改过、文件是链接、写不了时不动" 'bash "$D/path_unhint.sh" "$CCA" > "$T/path-unhint.log" 2>&1'
ok "设备统计：服务端验签（和 ssh-keygen 交叉核对）、设备清单、状态，客户端（辅助程序）的上报、闲置判定、共享 API key、折合金额同步，经 curl 钉住证书公钥" 'python3 "$D/presence.py" 2>/dev/null'
ok "自己的同步服务器的一键部署：安装脚本在假服务器上装好（systemd、Docker compose，拉不到镜像、起不来改用 systemd）、打出加入链接（口令、盐不落盘）、更新、换链接、卸载，没装完时 .env 换回原来的，别人的目录不碰；cca hub deploy 经 SSH 部署、加入、更新，有设备加入过的不动，加入了的设备不碰别的服务器，连不上时说要放行的端口" 'python3 "$D/hub_deploy.py" 2>/dev/null'
ok "cca usage 的表格（一个账号一行、不超过终端的宽度）、周结算提醒（use_first）、API key 的掩码、每周用量（usage_weeks）" 'python3 "$D/usage_view.py" 2>/dev/null'
ok "折合金额：读会话记录（增量、去重、按账号身份认）、按周 / 按天的时段（夏令时、额度窗口）、App 要的视图、密文的格式" 'python3 "$D/cost_view.py" 2>/dev/null'
ok "邮箱助手：对着假 Gmail（真的 TLS）跑 mail-setup / test / open / watch / forget——只读的 IMAP（经 SOCKS5、HTTP 代理）、登录出错、证书总是校验、代理出错再试、没有 All Mail、搜索滞后、断线重连；密码不进命令行" 'python3 "$D/mail_view.py" 2>/dev/null'
ok "远程会话：会话宿主（客户端的环境、参数、退出码原样；启动它的终端没了会话照样运行）、接入端（回放、补上终端的模式、断开时还原、只看）、几个终端共用一个会话时的大小和查询" 'python3 "$D/remote_host.py" 2>/dev/null'
ok "远程会话经服务器中转：配对（服务器见不到配对码和密钥）、别的机器上的会话列表、接入（两头之间端到端加密：路上没有明文）、对方只能看 / 不让接入由宿主那头把关、取消配对；和照文档独立实现的另一头互通" 'python3 "$D/remote_relay.py" 2>/dev/null'

fi
if sect 1; then
# 1. 安装 + 第一个账号：建端口池需要一次重载，先征求同意
out=$(run CCA_TTY="$(ans y 192.0.2.10 '' '' '1 2 3 4' y alice@example.com '' 1 y)" bash "$CCA" install 2>&1)
ok "install：从路由器的节点里选可选节点" 'python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"nodes\"]==[\"US-A\",\"US-B\",\"US-C\",\"US-D\"] else 1)" "$H/.cca/config.json" && printf "%s" "$out" | grep -q "账号可选的出口节点：US-A、US-B、US-C、US-D"'
ok "install：建端口池前先问，提示重载约 20 秒" 'printf "%s" "$out" | grep -q "约 20 秒，期间局域网的新连接会中断"'
ok "install：8 个槽位 7901-7908，CCA-1 默认 US-A，空槽位默认 REJECT" '[ "$(nl_)" = 8 ] && [ "$(first CCA-1)" = US-A ] && [ "$(first CCA-8)" = REJECT ] && [ "$(sel CCA-1)" = US-A ]'
ok "install：只重载了一次" '[ "$(reloads)" = 1 ]'
ok "install：同时建好 cxa（等于 cca codex），是指向同目录 cca 的相对链接" '[ "$(readlink "$H/.local/bin/cxa")" = cca ] && printf "%s" "$out" | grep -q "以后可以直接用 cxa 代替 cca codex：cxa @别名 直接启动"'
ok "install：同时建好 aga（等于 cca agy），也是相对链接；agy 没有查用量，说明里不提 aga usage" '[ "$(readlink "$H/.local/bin/aga")" = cca ] && printf "%s" "$out" | grep -q "以后可以直接用 aga 代替 cca agy：aga @别名 直接启动" && ! printf "%s" "$out" | grep -q "aga usage"'
python3 - > "$T/accounts.golden" <<'PY'
import hashlib, json
n = 'alice-example-com-' + hashlib.sha1(b'alice@example.com').hexdigest()[:6]
print(json.dumps({'base': 7900, 'slots': 8, 'nodes': ['US-A', 'US-B', 'US-C', 'US-D'], 'accounts': [{'slot': 1, 'name': n, 'node': 'US-A'}]}))
PY
ok "install：路由器上的账号表和以前的版本逐字节一样（只有 Claude 账号时它变了，已安装的机器都会重新部署路由器端）" 'cmp -s "$T/accounts.golden" "$R/etc/cca/accounts.yaml"'
ok "install：钩子 / cron / sysupgrade" 'grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && grep -q ensure "$R/etc/crontabs/root" && grep -qx /etc/cca/ "$R/etc/sysupgrade.conf"'
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-*); alice_device=$(claude_device "$AD")
ok "install：未登录时已生成账号自己的设备标识" 'printf "%s\n" "$alice_device" | grep -Eq "^[0-9a-f]{64}:[0-9a-f]{64}$" && [ -f "$AD/device.json" ]'

fi
if sect 2; then
# 2. 再加账号（邮箱、别名分两行填）：不重载，API 切换
out=$(run CCA_TTY="$(ans bob@example.com 工作 2)" cca add 2>&1)
ok "add bob：不重载，CCA-2 经 API 切到 US-B，文件里默认顺序同步更新" '[ "$(reloads)" = 1 ] && [ "$(sel CCA-2)" = US-B ] && [ "$(first CCA-2)" = US-B ]'
ok "add：邮箱和别名分两行，账号表的键是 别名(邮箱)" 'printf "%s" "$out" | grep -q "登录邮箱：" && printf "%s" "$out" | grep -q "别名（可选，回车跳过）：" && grep -q "\"工作(bob@example.com)\": \"US-B\"" "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "已添加 工作（bob@example.com），出口 US-B"'
BD=$(ls -d "$H"/.cca/accounts/bob-example-com-*); bob_device=$(claude_device "$BD")
ok "不同账号：userID 和 machineID 都独立" '[ "${alice_device%:*}" != "${bob_device%:*}" ] && [ "${alice_device#*:}" != "${bob_device#*:}" ]'
out=$(run CCA_TTY="$(ans alice@example.com nope carol@example.com '工 作' 'a(b)' 工作 小号 3)" cca add 2>&1)
ok "add：重复邮箱、不像邮箱、别名带空格/括号/重名都会说明原因并重填" 'printf "%s" "$out" | grep -q "这个邮箱已经添加过了：alice@example.com" && printf "%s" "$out" | grep -q "这不像邮箱地址" && printf "%s" "$out" | grep -q "别名里不能有空格" && printf "%s" "$out" | grep -q "别名「工作」已经用在 工作（bob@example.com） 上了" && grep -q "\"小号(carol@example.com)\"" "$H/.cca/accounts.json"'
ok "add 选节点：注明已被哪些账号使用（有别名的写别名），默认选中第一个还没人用的 US-C" 'printf "%s" "$out" | grep -qE "1\. US-A +已被 alice@example.com 使用$" && printf "%s" "$out" | grep -qE "2\. US-B +已被 工作 使用$" && printf "%s" "$out" | grep -q "选择节点编号 \[3\]"'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：别名、邮箱分两列，没有别名的别名列留空" 'printf "%s" "$m" | grep -q "1\. *alice@example.com   未登录" && printf "%s" "$m" | grep -q "2\. 工作   bob@example.com     未登录"'
run CCA_TTY="$(ans)" cca web 小号 >/dev/null 2>&1
ok "web：没登录时先用账号邮箱登录（登录页预填），再打开 claude.ai" 'grep -q "ARGS: auth login --claudeai --email carol@example.com" claude.log && grep -q "https://claude.ai/" open.log'
run CCA_TTY="$(ans y)" cca rm 小号 >/dev/null 2>&1

fi
if sect 3; then
# 3. 登录 + 启动 + 出口检测
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "alice：预填邮箱登录 → 启动，退出码透传" '[ $rc -eq 3 ] && grep -q "ARGS: auth login --claudeai --email alice@example.com" claude.log'
ok "Claude 登录时已读到固定设备标识，退出同步不改变" 'grep -qF "LOGIN-DEVICE $alice_device @ $AD/claude" claude.log && [ "$(claude_device "$AD")" = "$alice_device" ]'
ok "alice：出口检测经 :7901（trace + iprisk）" 'grep -q "CURL via :7901 https://ip.net.coffee/api/iprisk/203.0.113.1" curl.log'

fi
if sect 3b; then
# 3b. 有别名的账号：按邮箱登录；只改别名不换账号目录、不重新登录、端口和"上次使用"跟着走
run CCA_TTY="$(ans)" cca @工作 --version >/dev/null 2>&1
d1=$(grep "^ENV: " claude.log | tail -n 1 | sed 's/.*CLAUDE_CONFIG_DIR=\([^ ]*\).*/\1/')
ok "别名(邮箱)：用邮箱预填登录，账号目录按邮箱命名" 'grep -q "ARGS: auth login --claudeai --email bob@example.com" claude.log && case "$d1" in */bob-example-com-*/claude) true ;; *) false ;; esac'
n_login=$(grep -c "ARGS: auth login" claude.log)
edit_table 'j["副号(bob@example.com)"] = j.pop("工作(bob@example.com)")'
run CCA_TTY="$(ans)" cca @副号 --version >/dev/null 2>&1; rc=$?
d2=$(grep "^ENV: " claude.log | tail -n 1 | sed 's/.*CLAUDE_CONFIG_DIR=\([^ ]*\).*/\1/')
ok "改别名：同一个账号目录、不重新登录、端口仍是 7902、上次使用跟着改" '[ $rc -eq 3 ] && [ "$d1" = "$d2" ] && [ "$(grep -c "ARGS: auth login" claude.log)" = "$n_login" ] && python3 -c "import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if c[\"ports\"].get(\"副号(bob@example.com)\")==7902 and c[\"last\"]==\"副号(bob@example.com)\" else 1)" "$H/.cca/config.json"'
out=$(run CCA_TTY="$(ans)" cca login 副号 2>&1); rc=$?
ok "Claude 改别名、退出再登录：设备标识保持不变" '[ $rc -eq 0 ] && [ "$(claude_device "$BD")" = "$bob_device" ] && [ "$(grep -cF "LOGIN-DEVICE $bob_device @ $BD/claude" claude.log)" = 2 ]'
: > open.log
run CCA_ACCOUNT='副号(bob@example.com)' "$H/.cca/bin/open-url" 'https://claude.com/cai/oauth/authorize?code=true&client_id=c&state=s' >/dev/null 2>&1
run CCA_ACCOUNT='副号(bob@example.com)' "$H/.cca/bin/open-url" 'https://claude.com/cai/oauth/authorize?code=true&login_hint=x%40y.z' >/dev/null 2>&1
run CCA_ACCOUNT='副号(bob@example.com)' "$H/.cca/bin/open-url" 'https://example.com/a?b=1' >/dev/null 2>&1
ok "会话里 /login 打开的登录地址没带邮箱时补上 login_hint，其它地址原样打开" 'grep -q "authorize?code=true&client_id=c&state=s&login_hint=bob%40example.com" open.log && grep -q "login_hint=x%40y.z\]" open.log && [ "$(grep -c login_hint open.log)" = 2 ] && grep -q "https://example.com/a?b=1\]" open.log'

fi
if sect 3b2; then
# 3b2. 服务器上登录（Linux 上没有账号专属的 Chrome；CCA_REMOTE_LOGIN=1 在 macOS 上模拟）：claude 照常在这里登录、开本机的回调，
# 它要打开的登录网址经 BROWSER 交给 cca，cca 写成一行 cca login-page 命令（Mac 上运行它打开登录页）；贴回来的验证码核对过 state
# 才转给本机的回调（不经代理），换 token 照常经账号的端口。假 claude 在 $T/oauth_flow 存在时照真的走（内容是这次的 state）
gone() { local i; for i in 1 2 3 4 5 6 7 8 9 10; do pgrep -f "$1" >/dev/null || return 0; sleep 0.2; done; return 1; }   # 进程都退出了
fpof() { printf '%s' "$1" | shasum -a 256 | cut -c1-16; }   # 节点的摘要（node_fp）：类型|服务器|端口|TLS 域名
FPA=$(fpof 'ss|192.0.2.1|8388|'); FPB=$(fpof 'ss|192.0.2.2|8388|'); FPD=$(fpof 'ss|192.0.2.4|8388|')
echo st-remote1 > "$T/oauth_flow"; : > open.log
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans good-1 'good-1#st-wrong' 'good-1#st-remote1')" cca login alice 2>&1); rc=$?
ok "服务器上登录：给出在 Mac 上运行的一行（账号写邮箱、带上节点和它的地址摘要，网址是 claude 本机回调的那个），不在这台机器上开浏览器" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "  cca login-page alice@example.com --node US-A --node-id $FPA '\''https://claude.com/cai/oauth/authorize?" && printf "%s" "$out" | grep -q "redirect_uri=http%3A%2F%2Flocalhost%3A[0-9]*%2Fcallback.*state=st-remote1" && ! grep -q oauth open.log'
ok "服务器上登录：贴的不是 code#state、state 对不上都不转（重新问），对的转给本机的回调；换 token 经账号的端口，登录好了" 'printf "%s" "$out" | grep -q "认不出验证码" && printf "%s" "$out" | grep -q "state 对不上" && [ "$(grep -c "^OAUTH-EXCHANGE" claude.log)" = 1 ] && grep -q "OAUTH-EXCHANGE code=good-1 https_proxy=http://192.0.2.10:7901" claude.log && grep -q "LOGIN alice@example.com @ $AD/claude" claude.log && printf "%s" "$out" | grep -q "已重新登录"'
echo st-remote2 > "$T/oauth_flow"
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans '')" cca login 副号 2>&1); rc=$?
ok "服务器上登录：有别名的账号带上别名（Mac 上还没有它时照这个别名加上）；直接回车取消，本机等着回调的 claude 跟着停掉" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "  cca login-page bob@example.com --alias '\''副号'\'' --node US-B --node-id $FPB '\''https://claude.com/" && printf "%s" "$out" | grep -q "已取消登录" && gone "$T/fake_oauth.py"'
echo st-remote3 > "$T/oauth_flow"
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans 'bad-3#st-remote3')" cca login alice 2>&1); rc=$?
ok "服务器上登录：验证码换不到 token（过期、用过了）：照 claude 的原话说，没登录上" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "登录没有成功：Login failed: Request failed with status code 400" && gone "$T/fake_oauth.py"'
echo st-remote4 > "$T/oauth_flow"
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans 'http://localhost:1/callback?code=good-4&state=st-remote4')" cca login alice 2>&1); rc=$?
ok "服务器上登录：也认浏览器地址栏里跳回 localhost 的整个网址（转给这次的端口，不照贴的）" '[ $rc -eq 0 ] && grep -q "OAUTH-EXCHANGE code=good-4" claude.log && grep -q "LOGIN alice@example.com" claude.log'
rm -f "$T/oauth_flow"
# Mac 上：cca login-page 用账号专属的 Chrome、经账号的端口打开登录页，接住跳回 localhost 的验证码（放进剪贴板，打到 stdout）
lp=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
LPU="https://claude.com/cai/oauth/authorize?code=true&client_id=c&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A$lp%2Fcallback&scope=user%3Ainference&code_challenge=CC&code_challenge_method=S256&state=st-page1&login_hint=alice%40example.com"
: > open.log; rm -f "$T/clipboard"
CCA_T="$T" python3 "$T/fake_browser.py" st-page1 good-page1 > "$T/page.txt" 2>&1 & fb=$!
out=$(run CCA_LOGIN_WAIT=20 cca login-page alice@example.com --node US-A "$LPU" 2>"$T/lp.err"); rc=$?
wait $fb
ok "login-page：在账号专属的 Chrome 里经账号的端口打开登录页；接住带对的 state 跳回来的那一下（favicon、state 不对的不算），验证码进剪贴板、打到 stdout" '[ $rc -eq 0 ] && [ "$out" = "good-page1#st-page1" ] && [ "$(cat "$T/clipboard")" = "good-page1#st-page1" ] && grep -F "state=st-page1" open.log | grep -qF -- "--proxy-server=http://192.0.2.10:7901" && grep -F "state=st-page1" open.log | grep -qF -- "--user-data-dir=$AD/chrome" && grep -qx "favicon 404" "$T/page.txt" && grep -qx "wrong 400" "$T/page.txt" && grep -qx "callback 200" "$T/page.txt" && grep -q "验证码已经复制到剪贴板" "$T/page.txt" && grep -q "回到服务器的终端粘贴" "$T/lp.err"'
: > open.log
out=$(run cca login-page alice@example.com --node HK-A "$LPU" 2>&1); rc=$?
ok "login-page：服务器上的节点和这台 Mac 上的不一样：出口不同，不打开" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "在服务器上走 HK-A，在这台 Mac 上走 US-A" && ! grep -q st-page1 open.log'
out=$(run cca login-page alice@example.com "https://evil.example.com/oauth/authorize?response_type=code&code_challenge=x&state=s&redirect_uri=http%3A%2F%2Flocalhost%3A$lp%2Fcallback" 2>&1); rc=$?
out2=$(run cca login-page alice@example.com "https://auth.openai.com/oauth/authorize?response_type=code&code_challenge=x&state=s&redirect_uri=http%3A%2F%2Flocalhost%3A$lp%2Fauth%2Fcallback" 2>&1); rc2=$?
out3=$(run cca login-page nobody@example.com "$LPU" 2>&1); rc3=$?
ok "login-page：不是这个工具的登录页不打开（Codex 的提示用 cca codex login-page）；这台 Mac 上没有的账号说清楚" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "这不是 Claude 的登录网址" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "Codex 账号用 cca codex login-page" && [ $rc3 -ne 0 ] && printf "%s" "$out3" | grep -q "这台 Mac 上没有 nobody@example.com 这个账号" && ! grep -q st-page1 open.log'
# 占住回调的端口，一直占到下面 kill 它为止：后面这几次 login-page 都靠它（端口被占时打开登录页就返回）。以前只占 20 秒，机器慢时
# （xprotectd 忙着扫新文件，每次运行慢好几倍）没跑到后面就放了，login-page 真的去等回调，干等 15 分钟才失败。都带 CCA_LOGIN_WAIT=20：端口万一空了最多等 20 秒
python3 -c 'import socket, sys, time; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1); open(sys.argv[2], "w").close(); time.sleep(600)' "$lp" "$T/lp.busy" & busy=$!
for i in $(seq 1 100); do [ -e "$T/lp.busy" ] && break; sleep 0.1; done   # 等它真的占上了（机器慢时 python 起来也要一会儿）
out=$(run CCA_LOGIN_WAIT=20 cca login-page alice@example.com --node US-A "$LPU" 2>&1); rc=$?
ok "login-page：回调的端口被别的程序占着：照样打开登录页，说清楚登录后从地址栏复制整个网址贴回服务器" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "被别的程序占着" && grep -q "state=st-page1" open.log'
# 节点按地址摘要认（两边叫法可以不同）；Mac 上还没有这个账号时问要不要用同一个节点加上它。回调的端口还占着：打开登录页就返回
: > open.log
out2=$(run CCA_LOGIN_WAIT=20 cca login-page alice@example.com --node US-A --node-id 0123456789abcdef "$LPU" 2>&1); rc2=$?
n2=$(grep -c st-page1 open.log)
out=$(run CCA_LOGIN_WAIT=20 cca login-page alice@example.com --node US-A1 --node-id "$FPA" "$LPU" 2>&1); rc=$?
ok "login-page：节点按地址摘要认：服务器上叫 US-A1、地址和这边的 US-A 一样，照样打开；同名而地址不同的不打开" '[ $rc -eq 0 ] && grep -q "state=st-page1" open.log && [ $rc2 -ne 0 ] && [ "$n2" = 0 ] && printf "%s" "$out2" | grep -q "在服务器上走 US-A，在这台 Mac 上走 US-A：不是同一个节点"'
t0=$(cat "$H/.cca/accounts.json"); : > open.log
out=$(run CCA_LOGIN_WAIT=20 CCA_TTY="$(ans n)" cca login-page dora@example.com --alias 新号 --node US-D2 --node-id "$FPD" "$LPU" 2>&1); rc=$?
out2=$(run CCA_LOGIN_WAIT=20 CCA_TTY="$(ans y)" cca login-page dora@example.com --node XX-Z --node-id 1111111111111111 "$LPU" 2>&1); rc2=$?
out3=$(run CCA_LOGIN_WAIT=20 CCA_TTY="$(ans n)" cca login-page dora@example.com --node US-C "$LPU" 2>&1); rc3=$?
ok "login-page：Mac 上还没有这个账号：按地址认出这边的 US-D，问要不要照服务器的别名加上，不加就不打开；这边没有这个节点的说清楚；旧版服务器没给摘要时按名字认" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "这台 Mac 上还没有 新号（dora@example.com）：现在加上它吗（出口 US-D）？" && printf "%s" "$out" | grep -q "没有加上，也没打开登录页" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "这台 Mac 的可选节点里没有服务器上的 XX-Z（按地址也认不出）" && ! printf "%s" "$out2" | grep -q "现在加上它吗" && [ $rc3 -ne 0 ] && printf "%s" "$out3" | grep -q "这台 Mac 上还没有 dora@example.com：现在加上它吗（出口 US-C）？" && [ "$(cat "$H/.cca/accounts.json")" = "$t0" ] && ! grep -q st-page1 open.log'
out=$(run CCA_LOGIN_WAIT=20 CCA_TTY="$(ans y)" cca login-page dora@example.com --alias 新号 --node US-D2 --node-id "$FPD" "$LPU" 2>&1); rc=$?
ok "login-page：答应加上：用这边的 US-D 加上 新号（dora@example.com），同步路由器，接着在它专属的 Chrome 里经它的端口打开登录页" '[ $rc -eq 0 ] && tblok "t.get(\"新号(dora@example.com)\") == \"US-D\"" && printf "%s" "$out" | grep -q "已添加 新号（dora@example.com），出口 US-D" && grep -F "state=st-page1" open.log | grep -qF -- "--user-data-dir=$(ls -d "$H"/.cca/accounts/dora-example-com-*)/chrome" && [ -d "$H/Applications/Claude 新号.app" ]'
kill $busy 2>/dev/null; wait $busy 2>/dev/null
run CCA_TTY="$(ans y)" cca rm dora@example.com >/dev/null 2>&1
ok "删掉刚加的 新号：账号表、启动器都照旧" '[ "$(cat "$H/.cca/accounts.json")" = "$t0" ] && [ ! -e "$H/Applications/Claude 新号.app" ] && ! ls -d "$H"/.cca/accounts/dora-example-com-* >/dev/null 2>&1'

fi
if sect 3b3; then
# 3b3. 邮箱助手（cca mail）：Claude 账号的登录邮件转发到一个 Gmail，登录时 cca 经 IMAP（只读）找到它，在账号专属的 Chrome 里打开
# 里面的登录链接。假的 Gmail 是 tests/mail_fake.py（真的 TLS、只读 IMAP：会改邮箱的命令一律回 BAD 并记 VIOLATION），经它的假代理
# （一个端口同时认 SOCKS5 和 HTTP CONNECT）连，证书是测试的 CA（CCA_MAIL_CAFILE：只在连的是本机时才信）；系统代理由假 scutil 给
# （$T/scutil.txt）。meml UID 收件人 记号 [选项…] 往所有邮件里放一封登录邮件（链接是 https://claude.ai/magic-link#记号）
MB="$T/mailbox"
python3 "$D/mail_fake.py" serve "$MB" >/dev/null 2>&1 & MBP=$!
for i in $(seq 1 100); do [ -f "$MB/ports.json" ] && break; sleep 0.1; done
MPX=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["proxy"])' "$MB/ports.json")
MENV=(CCA_MAIL_CAFILE="$MB/pki/ca.pem" CCA_MAIL_POLL=0.2 CCA_MAIL_RETRY=0.2)
meml() { python3 "$D/mail_fake.py" eml "$MB/box/all/$1.eml" --to "$2" --link "https://claude.ai/magic-link#$3" "${@:4}"; }
out=$(run cca mail 2>&1); rc=$?
out2=$(run cca mail open alice --wait 0 2>&1); rc2=$?
ok "邮箱助手：没配置时 cca mail 说怎么配置，cca mail open 说先配置" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "还没配置邮箱助手：运行 cca mail setup" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "还没配置邮箱助手"'   # cca codex mail 在 10f 测：它会让 Codex 算用起来了
printf '<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  SOCKSEnable : 1\n  SOCKSPort : %s\n  SOCKSProxy : 127.0.0.1\n}\n' "$MPX" > "$T/scutil.txt"
: > "$T/security.stdin"
out=$(run "${MENV[@]}" CCA_TTY="$(ans inbox@example.com 'wrong pass word xx' 'abcd efgh ijkl mnop')" cca mail setup 2>&1); rc=$?
ok "mail setup：照系统代理（SOCKS5）连 Gmail、登录一次；密码不对重问；对了才存进钥匙串（经 security -i 的十六进制，不进命令行）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "Gmail 不认这个密码" && printf "%s" "$out" | grep -q "已登录 inbox@example.com，看这些邮件夹：所有邮件、垃圾邮件" && [ "$(cat "$T/keychain/cca-mail/inbox@example.com")" = abcdefghijklmnop ] && grep -q "add-generic-password -U -a inbox@example.com -s cca-mail -X 6162" "$T/security.stdin" && ! grep -q abcdefghijklmnop "$T/security.log" && ! grep -qs abcdefghijklmnop "$T/env.log" && cfgok "c[\"mail\"] == {\"user\": \"inbox@example.com\"}" && grep -q "PROXY socks5 imap.gmail.com:993" "$MB/imap.log"'
out=$(run "${MENV[@]}" cca mail status 2>&1)
ok "mail status：邮箱、连接方式（照系统代理）、登录时自动打开" 'printf "%s" "$out" | grep -q "inbox@example.com（应用专用密码在钥匙串里）" && printf "%s" "$out" | grep -q "经代理 127.0.0.1:${MPX}，SOCKS5（照系统代理）" && printf "%s" "$out" | grep -q "自动打开登录邮件里的链接"'
now=$(date +%s)
meml 1 alice@example.com tok-T1 --at $((now - 600)); meml 2 carol@example.com tok-T2 --at $((now - 300)); meml 3 alice@example.com tok-T3 --ar fail --at $((now - 900))
out=$(run "${MENV[@]}" cca mail test 2>&1); rc=$?
ok "mail test：逐封说最近的登录邮件认不认（收件人是不是账号、Gmail 的验证、链接的样子），不打出链接；只读" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "有 3 封" && printf "%s" "$out" | grep -q "（账号 alice@example.com）" && printf "%s" "$out" | grep -q "不是账号表里的邮箱" && printf "%s" "$out" | grep -q "✓ 通过核对，链接 https://claude.ai/magic-link（# 后面" && printf "%s" "$out" | grep -q "Gmail 的验证结果里没有 Anthropic 的签名通过" && ! printf "%s" "$out" | grep -q "tok-T" && ! grep -q VIOLATION "$MB/imap.log"'
: > open.log
out=$(run "${MENV[@]}" cca mail open alice 2>&1); rc=$?
ok "mail open：最近半小时里这个账号最新的、通过核对的一封，在它专属的 Chrome 里经它的端口打开（链接不在输出里，也不记进 ~/.cca）" '[ $rc -eq 0 ] && grep -F "https://claude.ai/magic-link#tok-T1" open.log | grep -F -- "--user-data-dir=$AD/chrome" | grep -qF -- "--proxy-server=http://192.0.2.10:7901" && ! grep -qE "tok-T[23]" open.log && printf "%s" "$out" | grep -q "已在 alice@example.com 专属的浏览器里打开登录邮件里的链接（经 http://192.0.2.10:7901 → US-A）" && ! printf "%s" "$out" | grep -q "tok-T" && ! grep -rqs "tok-T" "$H/.cca"'
out=$(run "${MENV[@]}" cca mail open alice --wait 0 2>&1); rc=$?
out2=$(run "${MENV[@]}" cca mail open alice --again 2>&1); rc2=$?
ok "mail open：打开过的那封不再打开（--again 再打开一次）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "最近的登录邮件已经打开过了" && [ $rc2 -eq 0 ] && [ "$(grep -c "tok-T1" open.log)" = 2 ]'
out=$(run "${MENV[@]}" cca mail open 副号 --wait 1 2>&1); rc=$?
ok "mail open：没有这个账号的登录邮件：等一会儿（--wait），说清楚怎么做；别的账号的不开" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "等 bob@example.com 的登录邮件（最多 1 秒" && printf "%s" "$out" | grep -q "最近 30 分钟里没有 bob@example.com 的登录邮件" && [ "$(grep -c "magic-link" open.log)" = 2 ]'
# 登录时：假 claude 的 auth login 在 $T/mail_send 在时先跑它（像在登录页上点了用邮箱登录，Anthropic 发来登录邮件），等 open.log 里出现
# $T/mail_login 里的链接才算登录好
printf '%s\n' "https://claude.ai/magic-link#tok-L1" > "$T/mail_login"
printf '#!/bin/bash\npython3 "%s/mail_fake.py" eml "%s/box/all/9.eml" --to alice@example.com --link "https://claude.ai/magic-link#tok-L1"\n' "$D" "$MB" > "$T/mail_send"; chmod 755 "$T/mail_send"
out=$(run "${MENV[@]}" CCA_TTY="$(ans)" cca login alice 2>&1); rc=$?
rm -f "$T/mail_send" "$T/mail_login"
ok "登录时：后台盯着 Gmail，登录邮件一到就在 alice 专属的 Chrome 里打开里面的链接，登录接着完成；登录结束就不再盯" '[ $rc -eq 0 ] && grep -q "MAIL-LOGIN opened" claude.log && grep -F "tok-L1" open.log | grep -qF -- "--user-data-dir=$AD/chrome" && printf "%s" "$out" | grep -q "邮箱助手：已连上 inbox@example.com" && printf "%s" "$out" | grep -q "已在 alice@example.com 专属的浏览器里打开登录邮件里的链接" && printf "%s" "$out" | grep -q "已重新登录" && ! printf "%s" "$out" | grep -q "没通过核对" && gone "mail-watch alice@example.com" && ! grep -q VIOLATION "$MB/imap.log"'
# 账号页的「邮箱」一节（PROTOCOL 4.11）：列表（_gui mail-list，只读 JSON，不含链接）、打开指定这一封（open --msg）、单独配 Gmail（setup --account）
akey=$(python3 -c 'import json,sys;print([k for k in json.load(open(sys.argv[1])) if "alice@example.com" in k][0])' "$H/.cca/accounts.json")
bkey=$(python3 -c 'import json,sys;print([k for k in json.load(open(sys.argv[1])) if "bob@example.com" in k][0])' "$H/.cca/accounts.json")
meml 5 alice@example.com tok-T5 --at $((now - 120))   # alice 的一封新登录邮件（uid 5 = 5.eml，tok-T5）
out=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>&1); rc=$?
mluser=$(printf '%s' "$out" | python3 -c 'import json,sys;print(json.load(sys.stdin)["user"])' 2>/dev/null)
mlshape=$(printf '%s' "$out" | python3 -c 'import json,sys;d=json.load(sys.stdin);op=[r for r in d["rows"] if r["openable"]];print(1 if (d["error"] is None and all(r["to"]==["alice@example.com"] for r in d["rows"]) and op and op[0]["msg"] and op[0]["link_shape"] and "magic-link" in op[0]["link_shape"] and all("tok-" not in (r["link_shape"] or "") for r in d["rows"])) else 0)' 2>/dev/null)
ok "mail-list：实时列这个账号的登录邮件（元数据 + 核对结论 + 链接样子 + 不透明句柄），账号作用域（别的账号的不列），绝不含链接本身；只读" '[ $rc -eq 0 ] && [ "$mluser" = inbox@example.com ] && [ "$mlshape" = 1 ] && ! printf "%s" "$out" | grep -q "tok-T" && ! grep -q VIOLATION "$MB/imap.log"'
# 按 uid 精确取 T5（uid 5）的句柄，和排序 / 时间无关：打开它应得到 tok-T5
handle=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>/dev/null | python3 -c 'import json,sys;r=[x for x in json.load(sys.stdin)["rows"] if x["openable"] and x["msg"]["uid"] == 5][0]["msg"];print("%s:%s:%s"%(r["folder"],r["uid"],r["uidvalidity"]))' 2>/dev/null)
: > open.log
out=$(run "${MENV[@]}" cca mail open "$akey" --msg "$handle" 2>&1); rc=$?
ok "mail open --msg：按句柄重新取、再核对、打开指定的那一封（tok-T5），在 alice 的 Chrome 里经它的端口；链接不在输出里、不进 ~/.cca" '[ $rc -eq 0 ] && grep -F "https://claude.ai/magic-link#tok-T5" open.log | grep -qF -- "--user-data-dir=$AD/chrome" && ! printf "%s" "$out" | grep -q "tok-T" && ! grep -rqs "tok-T" "$H/.cca"'
# 信任转发中转（不安全，默认关，每账号独立，PROTOCOL 4.11）：隐私别名/转发服务（像 relay.example.com）去掉 Anthropic 的 DKIM、用自己域名重签 → 默认拒；登记了才接受
fwdeml() { python3 "$D/mail_fake.py" eml "$MB/box/all/$1.eml" --to inbox@example.com --from "\"$2\" <noreply@relay.example.com>" --dkim-d relay.example.com --sign from:to:subject:reply-to:date --reply-to "Anthropic <no-reply@mail.anthropic.com>" --link "https://claude.ai/magic-link#$3" "${@:4}"; }
fwdeml 6 alice@example.com tok-F1 --at $((now - 60))
fwdeml 7 alice@example.com tok-F2 --reply-to "X <evil@phisher.example.com>" --at $((now - 50))   # Reply-To 非 Anthropic（攻击变体）：登记了也拒
aslug=$(basename "$AD")
has6=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>/dev/null | python3 -c 'import json,sys;print(sum(1 for r in json.load(sys.stdin)["rows"] if "noreply@relay.example.com" in r["from"]))' 2>/dev/null)
ok "信任转发中转：默认不开时，中转重签、去掉了 Anthropic 签名的邮件认不出账号、不进这个账号的列表" '[ "$has6" = 0 ]'
out=$(run CCA_TTY="$(ans y)" cca mail trust-forwarder alice relay.example.com 2>&1); rc=$?
ok "mail trust-forwarder：带不安全警告 + 确认，登记这个账号的信任中转（存 config.mail.accounts[acct_id].forwarder，各账号独立）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "不安全" && printf "%s" "$out" | grep -q "别名一旦泄露" && cfgok "c[\"mail\"][\"accounts\"][\"'"$aslug"'\"][\"forwarder\"] == \"relay.example.com\""'
out=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>&1)
f1ok=$(printf '%s' "$out" | python3 -c 'import json,sys;r=[x for x in json.load(sys.stdin)["rows"] if x["msg"] and x["msg"]["uid"]==6];print(1 if (r and r[0]["openable"] and r[0]["via"]=="forwarder" and r[0]["verdict"]["ok"]) else 0)' 2>/dev/null)
f2rej=$(printf '%s' "$out" | python3 -c 'import json,sys;r=[x for x in json.load(sys.stdin)["rows"] if "noreply@relay.example.com" in x["from"] and x.get("msg") is None and x["verdict"]["why"]=="from"];print(1 if r else 0)' 2>/dev/null)
ok "信任转发中转：登记后，中转转来、标注来自 Anthropic、它的 DKIM 覆盖 Reply-To 的登录邮件被路由到账号、经可信中转认（via=forwarder）、可打开；Reply-To 非 Anthropic 的拒；绝不含链接" '[ "$f1ok" = 1 ] && [ "$f2rej" = 1 ] && ! printf "%s" "$out" | grep -q "tok-F"'
handle=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>/dev/null | python3 -c 'import json,sys;r=[x for x in json.load(sys.stdin)["rows"] if x["openable"] and x["msg"]["uid"]==6][0]["msg"];print("%s:%s:%s"%(r["folder"],r["uid"],r["uidvalidity"]))' 2>/dev/null)
: > open.log
out=$(run "${MENV[@]}" cca mail open "$akey" --msg "$handle" 2>&1); rc=$?
ok "信任转发中转：open --msg 按句柄打开这封（经可信中转认的），在 alice 的 Chrome 里打开 tok-F1" '[ $rc -eq 0 ] && grep -F "https://claude.ai/magic-link#tok-F1" open.log | grep -qF -- "--user-data-dir=$AD/chrome"'
out=$(run cca mail trust-forwarder alice off 2>&1); rc=$?
has6b=$(run "${MENV[@]}" cca _gui mail-list --account "$akey" 2>/dev/null | python3 -c 'import json,sys;print(sum(1 for r in json.load(sys.stdin)["rows"] if "noreply@relay.example.com" in r["from"]))' 2>/dev/null)
ok "mail trust-forwarder off：关掉后回到只认 Anthropic 直签，中转重签的邮件又不进列表了" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已关闭" && ! cfgok "c.get(\"mail\",{}).get(\"accounts\",{}).get(\"'"$aslug"'\",{}).get(\"forwarder\")" && [ "$has6b" = 0 ]'
rm -f "$MB/box/all/6.eml" "$MB/box/all/7.eml"
printf '{"user":"bob-inbox@example.com","password":"abcdefghijklmnop"}' > "$MB/ctl.json"   # 让假 Gmail 认 bob-inbox（单一用户）：测这个账号单独配的 Gmail
out=$(run "${MENV[@]}" CCA_TTY="$(ans bob-inbox@example.com abcdefghijklmnop)" cca mail setup --account 副号 2>&1); rc=$?
bslug=$(basename "$(ls -d "$H"/.cca/accounts/bob-example-com-*)")
ok "mail setup --account：这个账号单独用一个 Gmail（存进 mail.accounts[acct_id]、钥匙串按地址），默认那个不动、互不影响" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "副号 单独用这个 Gmail" && cfgok "c[\"mail\"][\"user\"] == \"inbox@example.com\"" && cfgok "c[\"mail\"][\"accounts\"][\"'"$bslug"'\"][\"user\"] == \"bob-inbox@example.com\"" && [ "$(cat "$T/keychain/cca-mail/bob-inbox@example.com")" = abcdefghijklmnop ] && [ -e "$T/keychain/cca-mail/inbox@example.com" ]'
out=$(run "${MENV[@]}" cca _gui mail-list --account "副号(bob@example.com)" 2>&1); rc=$?
mlbuser=$(printf '%s' "$out" | python3 -c 'import json,sys;print(json.load(sys.stdin)["user"])' 2>/dev/null)
ok "mail-list：账号单独配了 Gmail 就连它的（不是默认那个）" '[ $rc -eq 0 ] && [ "$mlbuser" = bob-inbox@example.com ]'
printf '{}' > "$MB/ctl.json"   # 恢复假 Gmail 认默认 inbox@example.com
out=$(run cca mail forget --account 副号 2>&1); rc=$?
ok "mail forget --account：只移这个账号单独配的（改回默认），默认那个留着，没人用的地址从钥匙串删掉" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "单独配置的 Gmail" && cfgok "c[\"mail\"][\"user\"] == \"inbox@example.com\"" && ! cfgok "c.get(\"mail\", {}).get(\"accounts\")" && [ ! -e "$T/keychain/cca-mail/bob-inbox@example.com" ] && [ -e "$T/keychain/cca-mail/inbox@example.com" ]'
out=$(run cca mail forget --default 2>&1); rc=$?
ok "mail forget --default：只移默认（各账号单独配的留着；这里没有，所以全没了）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已移除默认 Gmail" && ! cfgok "\"mail\" in c"'
run "${MENV[@]}" CCA_TTY="$(ans inbox@example.com abcdefghijklmnop)" cca mail setup >/dev/null 2>&1   # 复原默认，下面的 forget（全部）照常有东西可删
# 删了账号后，它单独配的 Gmail 覆盖（mail.accounts）和没人再用的应用专用密码也清掉（写个账号表里没有的「幽灵」覆盖，跑一条 Claude 命令触发清理）
edit_table 'j.setdefault("mail", {}).setdefault("accounts", {})["ghost-slug-123"] = {"user": "ghost@example.com"}' "$H/.cca/config.json"
printf 'ghostpassword1234' > "$T/keychain/cca-mail/ghost@example.com"
run "${MENV[@]}" cca mail open "$akey" --wait 0 >/dev/null 2>&1   # 任一会同步的 Claude 命令都会走 secrets-forget → 清掉孤儿覆盖（cca rm/edit 删账号后同样）
ok "删账号后清掉它单独配的 Gmail 覆盖和没人用的应用专用密码（默认那个不动）" '! cfgok "\"ghost-slug-123\" in c.get(\"mail\", {}).get(\"accounts\", {})" && [ ! -e "$T/keychain/cca-mail/ghost@example.com" ] && cfgok "c[\"mail\"][\"user\"] == \"inbox@example.com\"" && [ -e "$T/keychain/cca-mail/inbox@example.com" ]'
out=$(run cca mail forget 2>&1); rc=$?
ok "mail forget：删掉钥匙串里的应用专用密码、config.json 的 mail 和 ~/.cca/mail" '[ $rc -eq 0 ] && [ ! -e "$T/keychain/cca-mail" ] && ! cfgok "\"mail\" in c" && [ ! -e "$H/.cca/mail" ]'
rm -f "$T/scutil.txt"
out=$(run "${MENV[@]}" CCA_TTY="$(ans inbox@example.com abcdefghijklmnop)" cca mail setup --proxy "http://127.0.0.1:$MPX" 2>&1); rc=$?
out2=$(run "${MENV[@]}" CCA_TTY="$(ans '' abcdefghijklmnop)" cca mail setup --proxy socks5://127.0.0.1:7901 2>&1); rc2=$?
ok "mail setup --proxy：指定 HTTP 代理（CONNECT）；指到账号的出口（端口池）的不用；再配置时默认用上次的地址" '[ $rc -eq 0 ] && grep -q "PROXY http imap.gmail.com:993" "$MB/imap.log" && cfgok "c[\"mail\"][\"proxy\"] == \"http://127.0.0.1:$MPX\"" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "（回车用 inbox@example.com）" && printf "%s" "$out2" | grep -q "127.0.0.1:7901 是账号的出口，Gmail 不能经它连"'
run cca mail forget >/dev/null 2>&1
kill "$MBP" 2>/dev/null; wait "$MBP" 2>/dev/null; MBP=""

fi
if sect 3c; then
# 3c. Claude App 多开：每个账号一份资料目录、走自己的端口；启动器跟着账号表走
L="$H/Applications"; BD=$(ls -d "$H"/.cca/accounts/bob-example-com-*)
ok "启动器：每个账号一个，改别名后自动改名" '[ -x "$L/Claude alice.app/Contents/MacOS/cca-launcher" ] && [ -d "$L/Claude 副号.app" ] && [ ! -e "$L/Claude 工作.app" ] && plutil -extract CFBundleIdentifier raw "$L/Claude 副号.app/Contents/Info.plist" | grep -q "^local.cca.launcher.bob-example-com-"'
badge() { python3 -c "import json,sys; print(\" \".join(json.load(open(sys.argv[1]))[\"badge\"]))" "$L/$1.app/Contents/Resources/cca-launcher.json"; }
ok "启动器图标：Claude 图标右下角加了账号标识（中文取头两个字，其它取首字母），各账号颜色不同" '[ "$(badge "Claude 副号" | cut -d" " -f1)" = 副号 ] && [ "$(badge "Claude alice" | cut -d" " -f1)" = A ] && [ "$(badge "Claude 副号" | cut -d" " -f2)" != "$(badge "Claude alice" | cut -d" " -f2)" ] && [ "$(sips -g pixelWidth "$L/Claude 副号.app/Contents/Resources/AppIcon.icns" | awk "/pixelWidth/ {print \$2}")" = 1024 ] && ! cmp -s "$T/Claude.app/Contents/Resources/electron.icns" "$L/Claude 副号.app/Contents/Resources/AppIcon.icns"'
cat > "$T/alpha.js" <<'EOF'
ObjC.import('AppKit');
function run(argv) {   // 第二张图哪儿都不比第一张更不透明（按比例坐标取 64×64 个点）就输出 ok
  var r = argv.map(function (p) { return $.NSBitmapImageRep.imageRepWithData($.NSData.dataWithContentsOfFile(p)); }), m = 0;
  for (var i = 0; i < 64; i++) for (var j = 0; j < 64; j++) {
    var a = r.map(function (x) { return x.colorAtXY(Math.floor((i + 0.5) / 64 * x.pixelsWide), Math.floor((j + 0.5) / 64 * x.pixelsHigh)).alphaComponent; });
    m = Math.max(m, a[1] - a[0]);
  }
  return m < 0.05 ? 'ok' : m.toFixed(2);
}
EOF
sips -s format png "$T/Claude.app/Contents/Resources/electron.icns" --out "$T/icon-src.png" >/dev/null 2>&1
sips -s format png "$L/Claude 副号.app/Contents/Resources/AppIcon.icns" --out "$T/icon-out.png" >/dev/null 2>&1
ok "启动器图标：账号标识整个在原图标的轮廓里（伸出去的话 macOS 26 起会把图标缩小、关进灰框）" '[ "$(osascript -l JavaScript "$T/alpha.js" "$T/icon-src.png" "$T/icon-out.png")" = ok ]'
m0=$(stat -f %Fm "$L/Claude 副号.app/Contents/Resources/AppIcon.icns"); run cca app --launchers >/dev/null 2>&1   # 纳秒的修改时间：不用先等一秒
ok "启动器图标：账号标识没变时不重画" '[ "$(stat -f %Fm "$L/Claude 副号.app/Contents/Resources/AppIcon.icns")" = "$m0" ]'
touch "$T/fake_icons"   # 图标检查完了：以后新建、重画的启动器图标用假的 sips / iconutil（setup.sh），真画一个要将近 1 秒
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
ok "cca app：账号自己的资料目录、端口和 CLAUDE_CONFIG_DIR，启动前做出口检测" '[ $rc -eq 0 ] && grep -qF -- "[-n] [-a] [$T/Claude.app] [--env] [CLAUDE_CONFIG_DIR=$BD/claude]" open.log && grep -qF -- "[--args] [--user-data-dir=$BD/desktop] [--proxy-server=http://192.0.2.10:7902] [--proxy-bypass-list=localhost;127.0.0.1;[::1];*.local;10.0.0.0/8;172.16.0.0/12;192.168.0.0/16]" open.log && grep -q "CURL via :7902 https://ip.net.coffee/api/iprisk/203.0.113.2" curl.log && printf "%s" "$out" | grep -q "已打开 副号（bob@example.com） 的 Claude App，出口 US-B（端口 7902）" && printf "%s" "$out" | grep -q "第一次打开要登录 bob@example.com"'
mkdir -p "$T/fakerun/Contents/MacOS"
cat > "$T/fakerun/Contents/MacOS/Claude" <<'SH'
#!/bin/bash
for arg in "$@"; do
	case "$arg" in --user-data-dir=*) exec 8<"${arg#*=}/Local State" ;; esac
done
sleep 30
SH
chmod 755 "$T/fakerun/Contents/MacOS/Claude"
"$T/fakerun/Contents/MacOS/Claude" --user-data-dir="$BD/desktop" --proxy-server=x & FAKEPID=$!
sleep 0.5; n_open=$(grep -c "user-data-dir=$BD/desktop" open.log)
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
ok "cca app：已有实例代理不符、没有可信启动记录时拒绝激活，不开第二份也不杀用户进程" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "启动记录" && [ "$(grep -c "user-data-dir=$BD/desktop" open.log)" = "$n_open" ] && kill -0 "$FAKEPID"'
out=$(run CCA_TTY="$(ans y)" cca rm 副号 2>&1); rc=$?
ok "cca rm：这个账号的 Claude App 开着时不删" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "Claude App 还开着" && grep -q "副号(bob@example.com)" "$H/.cca/accounts.json"'
kill $FAKEPID 2>/dev/null; wait $FAKEPID 2>/dev/null
printf '%s\n' "$$" > "$T/app-owner"
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
n_open=$(grep -c "user-data-dir=$BD/desktop" open.log)
out2=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc2=$?
ok "cca app：本次启动有运行记录，PID、启动时间、账号目录与代理一致时复用实例" '[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && printf "%s" "$out2" | grep -q "副号 的 Claude App 已经开着，已切到前面" && [ "$(grep -c "user-data-dir=$BD/desktop" open.log)" = "$n_open" ] && [ "$(stat -f %Lp "$BD/app-runtime.json")" = 600 ]'
rm -f "$T/app-owner" "$T/app-open/$((900000000 + $$))"
touch "$T/open_noprofile"
out=$(run CCA_APP_WAIT=1 CCA_TTY="$(ans)" cca app alice 2>&1); rc=$?   # 等不到的：只等 1 秒
ok "cca app：Claude App 不认独立资料目录时关掉并说明" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "本次正在使用 alice@example.com 自己的资料目录"'
rm -f "$T/open_noprofile"; : > osascript.log
run "$L/Claude 副号.app/Contents/MacOS/cca-launcher" >/dev/null 2>&1; rc=$?
ok "启动器：调用 cca app 打开对应账号" '[ $rc -eq 0 ] && [ ! -s osascript.log ] && [ "$(grep -c "user-data-dir=$BD/desktop" open.log)" = $((n_open + 1)) ]'
printf '{"countryCode":"CN","trust_score":90}' > "$T/risk/203.0.113.2.json"
run "$L/Claude 副号.app/Contents/MacOS/cca-launcher" >/dev/null 2>&1; rc=$?
ok "启动器：打不开时弹窗说明原因" '[ $rc -ne 0 ] && grep -q "Claude 副号 没有打开" osascript.log && grep -q "中国大陆" osascript.log'
rm -f "$T/risk/203.0.113.2.json"

edit_table 'j["alice@corp.example"] = "US-A"'
run cca app --launchers >/dev/null 2>&1
ok "启动器重名：邮箱 @ 前面一样、又都没有别名时，改用完整邮箱" '[ -d "$L/Claude alice@example.com.app" ] && [ -d "$L/Claude alice@corp.example.app" ] && [ ! -e "$L/Claude alice.app" ] && [ -d "$L/Claude 副号.app" ]'
edit_table 'j.pop("alice@corp.example")'
run cca app --launchers >/dev/null 2>&1
ok "启动器：不再重名后改回短名字" '[ -d "$L/Claude alice.app" ] && [ ! -e "$L/Claude alice@corp.example.app" ] && [ ! -e "$L/Claude alice@example.com.app" ]'

fi
if sect 3d; then
# 3d. DNS / WebRTC 泄露检测（照 ip.net.coffee 的 DNS、WebRTC 检测页）：和出口检测同时在后台跑，只提示不拦。DNS 经账号的端口测，
#     结果按 线路|节点 缓存（启动不用每次等它），cca check 重新测；WebRTC 直连问 STUN（平时 CCA_STUN 指向不通的端口）
rm -f "$H/.cca/leak.json"; : > curl.log; mkdir -p "$T/dns" "$T/geo"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "启动时顺带测泄露：DNS 经账号的端口解析一次性的子域名、再问是哪些解析器来查的（归属地也经它查）；WebRTC 直连问 STUN（未取得 IP 记为未验证）；横幅里一句话" '[ $rc -eq 3 ] && grep -qE "^CURL via :7901 https://[0-9a-f]{20}-1\.d\.ip\.net\.coffee/pixel\.gif$" curl.log && grep -qE "^CURL via :7901 https://ip\.net\.coffee/api/dns/result/[0-9a-f]{20}$" curl.log && grep -q "^CURL via :7901 https://ip.net.coffee/api/geoip/203.0.113.53$" curl.log && printf "%s" "$out" | grep -q "泄露  DNS 未发现地区异常 · WebRTC 没测成" && [ "$(stat -f %Lp "$H/.cca/leak.json")" = 600 ]'
n0=$(grep -c "pixel.gif" curl.log)
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1)
ok "DNS 的结果按 线路|节点 缓存：再启动不再测（不拖慢启动），横幅照样有" '[ "$(grep -c "pixel.gif" curl.log)" = "$n0" ] && printf "%s" "$out" | grep -q "泄露  DNS 未发现地区异常 · WebRTC 没测成"'
printf '%s\n' 223.5.5.5 203.0.113.53 > "$T/dns/7902"
printf '{"country": "China", "region": "Zhejiang", "city": "Hangzhou", "isp": "Alibaba", "country_code": "cn"}' > "$T/geo/223.5.5.5.json"
out=$(run CCA_TTY="$(ans)" cca @副号 --version 2>&1); rc=$?
ok "DNS 泄露（账号出口那边用了中国大陆的解析器）：说明是哪个解析器、该查哪个节点，横幅里标出来，照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "DNS 泄露：经这个账号的端口访问网站时，域名是中国大陆的 DNS 服务器解析的（223.5.5.5 China · Alibaba）" && printf "%s" "$out" | grep -q "上的节点 US-B：" && printf "%s" "$out" | grep -q "泄露  DNS 泄露（1 个解析器在中国大陆） · WebRTC 没测成"'
# cca check 的开始时间是整秒（date +%s），在它之前测的才重测：紧跟着上面那次启动、同一秒里刚测过的还算新的（少两个请求）。
# 辅助程序快了以后常常落在同一秒里：等过了这一秒再跑
ls_now=$(date +%s); while [ "$(date +%s)" = "$ls_now" ]; do sleep 0.1; done
n0=$(grep -c "pixel.gif" curl.log)
out=$(run cca check 2>&1)
ok "cca check：DNS 重新测（不用启动时缓存的），注明解析器在哪；有泄露的那个账号另外说明" '[ "$(grep -c "pixel.gif" curl.log)" = $((n0 + 4)) ] && printf "%s" "$out" | grep -q "DNS 未发现地区异常（1 个解析器：United States；非浏览器实测） · WebRTC 没测成" && printf "%s" "$out" | grep -q "DNS 泄露（1 个解析器在中国大陆）" && printf "%s" "$out" | grep -q "DNS 泄露：经这个账号的端口"'
python3 -I "$T/fake_stun.py" "$T" & STUN_PID=$!
for i in $(seq 1 50); do [ -s "$T/stun.port" ] && break; sleep 0.1; done; STUN="127.0.0.1:$(cat "$T/stun.port")"
echo 192.0.2.77 > "$T/stun_ip"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "WebRTC：宿主 STUN 看到的不是账号的出口，但也不是中国大陆的地址：横幅里只是备注（探针 IP、在哪；归属地直连查，不经账号的出口），不警告" '[ $rc -eq 3 ] && grep -qx "STUN 192.0.2.77" "$T/stun.log" && printf "%s" "$out" | grep -q "泄露  DNS 未发现地区异常 · WebRTC 备注：STUN 看到 192.0.2.77（United States · Example ISP），与账号的出口不同" && grep -qx "CURL direct https://ip.net.coffee/api/geoip/192.0.2.77" curl.log && ! grep -q "via :7901 https://ip.net.coffee/api/geoip/192.0.2.77" curl.log && ! printf "%s" "$out" | grep -q "WebRTC 直连风险"'
ok "WebRTC 备注记进 health.json（状态 note，说明给 App 看）" 'grep -qE "\"webrtc\": *\"note\"" "$H"/.cca/accounts/*/health.json && grep -q "不是中国大陆的地址，只作提醒" "$H"/.cca/accounts/*/health.json'
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
ok "cca app：不是中国大陆的地址不另外说明，照常打开" '[ $rc -eq 0 ] && ! printf "%s" "$out" | grep -q "WebRTC 直连风险"'
echo 198.51.100.77 > "$T/stun_ip"
mkdir -p "$T/geo"
printf '{"country": "China", "region": "", "city": "", "isp": "Example CN", "country_code": "cn"}' > "$T/geo/198.51.100.77.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "WebRTC：看到的是中国大陆的地址才算风险：横幅里标出来；命令行启动不另外警告" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "WebRTC 直连风险：198.51.100.77（China · Example CN）" && ! printf "%s" "$out" | grep -q "WebRTC 直连风险：本机 UDP"'
n0=$(grep -c "^STUN " "$T/stun.log")
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
out2=$(run CCA_REMOTE_LOGIN=1 cca check alice 2>&1)
ok "服务器上（没有账号专属的浏览器和桌面 App，CCA_REMOTE_LOGIN=1 模拟）不测 WebRTC：启动横幅、cca check 里只说 DNS，不问 STUN" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "泄露  DNS 未发现地区异常" && printf "%s" "$out2" | grep -q "DNS 未发现地区异常（1 个解析器" && ! printf "%s" "$out$out2" | grep -q WebRTC && [ "$(grep -c "^STUN " "$T/stun.log")" = "$n0" ]'
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
ok "cca app：中国大陆的地址另外说明（不声称已经验证浏览器），照常打开" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "WebRTC 直连风险：本机 UDP 的 STUN 探测看到中国大陆的地址" && printf "%s" "$out" | grep -q "未验证浏览器"'
edit_table 'j["carol@example.com"] = "US-C"'
out=$(run CCA_TTY="$(ans)" cca @carol --version 2>&1); rc=$?
ok "要在账号的浏览器里登录时，WebRTC 泄露先说一声（登录页在那个浏览器里打开）" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "WebRTC 直连风险：" && grep -q "ARGS: auth login --claudeai --email carol@example.com" claude.log'
run CCA_TTY="$(ans y)" cca rm carol >/dev/null 2>&1
echo 198.18.0.77 > "$T/stun_ip"
printf 'not json' > "$T/geo/198.18.0.77.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "WebRTC：看到了别的地址、归属地查不到：说不清是不是中国大陆，记为没测成，不警告也不当作没问题" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "WebRTC 没测成（STUN 看到 198.18.0.77，归属地没查到" && ! printf "%s" "$out" | grep -q "WebRTC 直连风险"'
echo 203.0.113.1 > "$T/stun_ip"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1)
ok "直连 STUN 看到的就是账号的出口：仅确认 STUN 出口一致" 'printf "%s" "$out" | grep -q "泄露  DNS 未发现地区异常 · WebRTC：STUN 出口一致（浏览器状态未验证）"'
kill "$STUN_PID" 2>/dev/null; wait "$STUN_PID" 2>/dev/null; STUN="" STUN_PID=""
edit_table 'j["leak_check"] = False' "$H/.cca/config.json"; rm -f "$H/.cca/leak.json"; n0=$(grep -c "pixel.gif" curl.log)
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
edit_table 'j.pop("leak_check")' "$H/.cca/config.json"
ok "config.json 的 leak_check 设成 false：不测泄露（一个请求都不发），横幅里没有这一行" '[ $rc -eq 3 ] && [ "$(grep -c "pixel.gif" curl.log)" = "$n0" ] && ! printf "%s" "$out" | grep -q "泄露  "'
touch "$T/dns_down"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
rm -f "$T/dns_down"
ok "ip.net.coffee 的 DNS 检测接口连不上：注明没测成，照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "泄露  DNS 没测成.* · WebRTC 没测成"'
rm -rf "$T/dns" "$T/geo" "$T/stun_ip" "$H/.cca/leak.json"

fi
if sect 3e; then
# 3e. 远程会话（docs/dev/remote.md）：开关没开、不在终端里时照原样直接运行；开着、又在终端里时客户端跑在会话宿主里
#     （环境、参数、退出码都和直接运行一样）。宿主和接入端自己的行为在 tests/remote_host.py（unit 一节）
rs_pty() {   # rs_pty 一项环境：在真的终端里（script，不设 CCA_TTY）运行 cca @alice --version，退出码写进 $T/rs.rc，完了建 $T/rs.done
	rm -f "$T/rs.done" "$T/rs.rc"
	{ for i in $(seq 1 300); do [ -f "$T/rs.done" ] && break; sleep 0.1; done; } |
		script -q "$T/rs.log" env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_RESOLV_CONF="$T/resolv.conf" CCA_POLICY_DEFAULT= CCA_STUN=127.0.0.1:9 CCA_HELPER="$HELPERWRAP" CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" CCA_RETRY_WAIT=0 "$1" \
		PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb /bin/bash -c 'cca @alice --version; echo $? > "$CCA_T/rs.rc"; touch "$CCA_T/rs.done"' >/dev/null 2>&1
}
rs_last() { grep "^$1:" claude.log | tail -n 1; }   # 假 claude 最近一次记下的参数（ARGS）、环境（ENV）
rs_pty CCA_REMOTE=
rs_args=$(rs_last ARGS); rs_env=$(rs_last ENV)
ok "开关没开、在终端里：照原样直接运行（没有会话目录），退出码透传" '[ "$(cat "$T/rs.rc")" = 3 ] && [ -n "$rs_env" ] && [ ! -e "$H/.cca/remote" ]'
: > claude.log
run CCA_REMOTE=1 cca @alice --version >/dev/null 2>&1; rc=$?
ok "CCA_REMOTE=1、不在终端里（界面 App、脚本）：照原样直接运行" '[ $rc -eq 3 ] && [ "$(rs_last ARGS)" = "$rs_args" ] && [ ! -e "$H/.cca/remote" ]'
: > claude.log
rs_pty CCA_REMOTE=1
ok "CCA_REMOTE=1、在终端里：客户端跑在会话宿主里，它看到的参数、环境（代理、配置目录、fd 3 关着……）和直接运行时一样，退出码透传" '[ "$(cat "$T/rs.rc")" = 3 ] && [ "$(rs_last ARGS)" = "$rs_args" ] && [ "$(rs_last ENV)" = "$rs_env" ] && [ "$(stat -f %Lp "$H/.cca/remote")" = 700 ]'
ok "会话结束：登记、socket 和说明都删掉" '! ls "$AD"/cli-*.json >/dev/null 2>&1 && [ -z "$(ls "$H/.cca/remote")" ]'
touch "$T/hold"
rs_pty CCA_REMOTE=1 &
rs_bg=$!
rs_rows=""; for rs_i in $(seq 1 150); do rs_rows=$(run cca remote list 2>/dev/null); [ -n "$rs_rows" ] && break; sleep 0.1; done
ok "会话开着时 cca remote list 列出它：编号、工具、账号、文件夹、几点开始的、启动它的终端开着；登记也在" 'printf "%s\n" "$rs_rows" | grep -Eq "^[0-9a-f]{8}  claude  alice@example.com  /.*  [0-9-]+ [0-9:]+ 开始  启动它的终端开着$" && [ "$(printf "%s\n" "$rs_rows" | wc -l | tr -d " ")" = 1 ] && ls "$AD"/cli-*.json >/dev/null 2>&1'
out=$(run cca remote 2>&1)
ok "cca remote：说开关没开（这一次是 CCA_REMOTE=1），列出可以接入的会话" 'printf "%s" "$out" | grep -q "远程会话：没开（打开：cca remote on" && printf "%s" "$out" | grep -q "这台机器上有 1 个可以接入的会话" && printf "%s" "$out" | grep -q "    ${rs_rows%% *}  claude  alice@example.com"'
out=$(run cca remote list --json 2>/dev/null </dev/null); rc=$?; mkdir -p "$T/guifx"; printf '%s' "$out" > "$T/guifx/remote_local.json"
ok "cca remote list --json（给界面 App，PROTOCOL 9.1）：开关、这台机器上的会话（完整的编号、工具、账号、启动它的终端开着、接得上、能操作），没有配对过的机器" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"version\"] == 1 and d[\"kind\"] == \"remote\" and d[\"enabled\"] is False and isinstance(d[\"relay\"][\"available\"], bool) and d[\"peers\"] == [] and d[\"errors\"] == [] and len(d[\"sessions\"]) == 1 and (lambda s: len(s[\"id\"]) == 32 and s[\"id\"].startswith(\"${rs_rows%% *}\") and s[\"short\"] == \"${rs_rows%% *}\" and s[\"where\"] == \"local\" and s[\"peer\"] is None and s[\"tool\"] == \"claude\" and s[\"name\"] == \"alice@example.com\" and s[\"local_terminal\"] is True and s[\"state\"] == \"ok\" and s[\"access\"] == \"control\" and s[\"viewers\"] == 0 and isinstance(s[\"since_epoch\"], (int, float)))(d[\"sessions\"][0])"'
ok "界面 App 的 hello 说有远程会话（没有这个字段的旧版 cca，App 不显示那一页）" 'gj hello | jok "d[\"remote\"] == 1"'
out=$(run cca remote attach 2>&1 </dev/null); rc=$?
ok "cca remote attach 不在终端里：不接入，退出码 69" '[ $rc -eq 69 ] && printf "%s" "$out" | grep -q "要在终端里运行"'
rm -f "$T/hold"; wait "$rs_bg"
ok "会话结束：退出码透传，列表里没有了" '[ "$(cat "$T/rs.rc")" = 3 ] && [ -z "$(run cca remote list 2>/dev/null)" ] && [ -z "$(ls "$H/.cca/remote")" ]'
out=$(run cca remote attach 2>&1); rc=$?
ok "cca remote attach 没有会话：说明先 cca remote on，退出码 65" '[ $rc -eq 65 ] && printf "%s" "$out" | grep -q "现在没有可以接入的会话（先 cca remote on"'
# cca remote end：结束这台机器上的一个会话（等于关掉它的终端）。终端里先说是哪个再问；界面 App 那样运行时发一条同意事件，
# 同意是对着这一个会话的完整编号给的（PROTOCOL 4.2 的 remote_end）
out=$(run cca remote end 2>&1); rc=$?
ok "cca remote end 没有会话：退出码 65" '[ $rc -eq 65 ] && printf "%s" "$out" | grep -q "这台机器上现在没有在会话宿主里运行的会话"'
touch "$T/hold"
rs_pty CCA_REMOTE=1 &
rs_bg=$!
rs_sid=""; for rs_i in $(seq 1 150); do rs_sid=$(run cca remote list --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; s=json.load(sys.stdin)["sessions"]; print(s[0]["id"] if s else "")' 2>/dev/null); [ -n "$rs_sid" ] && break; sleep 0.1; done
out=$(run CCA_TTY="$(ans n)" cca remote end 2>&1); rc=$?
ok "cca remote end：只有一个会话时不用写编号；先说是哪个会话再问，答否不结束" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "^  ${rs_sid:0:8}  claude  alice@example.com" && printf "%s" "$out" | grep -q "没有结束" && [ -n "$(run cca remote list 2>/dev/null)" ]'
out=$(runev cca remote end ffff0000 2>&1); rc=$?
ok "cca remote end 没有这个编号的会话：退出码 65，fail 事件说原因" '[ $rc -eq 65 ] && printf "%s" "$out" | grep -q "没有编号以 ffff0000 开头的会话" && evq "e[\"ev\"] == \"fail\" and \"ffff0000\" in e[\"message\"]"'
out=$(runev cca remote end "${rs_sid:0:8}" 2>&1); rc=$?
ok "界面 App 结束会话：先不结束，发一条同意事件（对着这个会话的完整编号，默认不同意，说明里是哪个会话、结束意味着什么）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"remote_end\" and e[\"scope\"] == \"$rs_sid\" and e[\"grant\"] == \"remote_end:$rs_sid\" and e[\"default\"] == \"no\" and \"${rs_sid:0:8}  claude\" in e[\"detail\"] and \"关掉它的终端\" in e[\"detail\"]" && [ -n "$(run cca remote list 2>/dev/null)" ]'
out=$(runev CCA_GRANT="remote_end:${rs_sid%????}0000" cca remote end "$rs_sid" 2>&1); rc=$?
ok "别的会话的同意不算数" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"grant\"] == \"remote_end:$rs_sid\"" && [ -n "$(run cca remote list 2>/dev/null)" ]'
out=$(runev CCA_GRANT="remote_end:$rs_sid" cca remote end "$rs_sid" 2>&1); rc=$?
rm -f "$T/hold"; wait "$rs_bg"   # 先撤掉 hold：万一没结束成，会话自己退出（下面的退出码对不上），不会卡在这里
ok "同意以后（CCA_GRANT）：会话结束了才回来，result 事件 remote_updated；里面的程序是被 SIGHUP 结束的（退出码 129），登记、socket 和说明都清掉" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"remote_updated\" and e[\"message\"] == \"会话已经结束\"" && [ "$(cat "$T/rs.rc")" = 129 ] && [ -z "$(run cca remote list 2>/dev/null)" ] && [ -z "$(ls "$H/.cca/remote")" ] && ! ls "$AD"/cli-*.json >/dev/null 2>&1'
ok "界面 App 的 hello 说能结束会话（没有这个字段的旧版 cca，App 不给「结束」）" 'gj hello | jok "d[\"remote_end\"] == 1"'
out=$(run cca remote on 2>&1); rc=$?
ok "cca remote on：记进 config.json，说明怎么接回去、怎么关" '[ $rc -eq 0 ] && cfgok "c[\"remote\"] is True" && printf "%s" "$out" | grep -q "已打开：之后在终端里启动的会话放进会话宿主里运行" && printf "%s" "$out" | grep -q "cca remote attach"'
ok "cca remote：开着" 'run cca remote 2>&1 | grep -q "远程会话：开着"'
rm -rf "$H/.cca/remote"
rs_pty CCA_REMOTE=0
ok "开关开着、这一次 CCA_REMOTE=0：直接运行" '[ "$(cat "$T/rs.rc")" = 3 ] && [ ! -e "$H/.cca/remote" ]'
run cca @alice --version >/dev/null 2>&1; rc=$?
ok "开关开着、不在终端里：直接运行" '[ $rc -eq 3 ] && [ ! -e "$H/.cca/remote" ]'
: > claude.log
rs_pty CCA_REMOTE=
ok "开关开着、在终端里：客户端跑在会话宿主里，参数、环境照旧" '[ "$(cat "$T/rs.rc")" = 3 ] && [ -d "$H/.cca/remote" ] && [ "$(rs_last ARGS)" = "$rs_args" ] && [ "$(rs_last ENV)" = "$rs_env" ]'
out=$(run cca remote off 2>&1); rc=$?
ok "cca remote off：config.json 里不留这一项" '[ $rc -eq 0 ] && cfgok "\"remote\" not in c" && printf "%s" "$out" | grep -q "已关闭" && run cca remote 2>&1 | grep -q "远程会话：没开"'
out=$(run cca remote bogus 2>&1); rc=$?
ok "cca remote 不认识的子命令：说用法，不当成给 claude 的参数" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "用法：cca remote \[status|on|off|list|attach" && ! grep -q "ARGS: .*bogus" claude.log'
ok "cca help 里有远程会话的说明" 'run cca help 2>&1 | grep -q "cca remote attach \[-r\] \[编号\]"'
rm -rf "$H/.cca/remote" "$T"/rs.*

fi
if sect 4; then
# 4. 选中节点被改动（仪表盘手动切 / 缓存恢复错）→ 启动前自动纠正，不重载
printf US-D > "$R/sel/CCA-1"
run cca @alice --version >/dev/null 2>&1
ok "选中被改：启动前经 API 纠正回 US-A，不重载" '[ "$(sel CCA-1)" = US-A ] && [ "$(reloads)" = 1 ]'
printf HK-A > "$R/sel/CCA-1"; rr ensure
ok "选中被改：cron 自愈也会纠正" '[ "$(sel CCA-1)" = US-A ] && [ "$(reloads)" = 1 ]'

fi
if sect 5; then
# 5. 手改账号表换节点：只走 API
edit_table 'j["副号(bob@example.com)"]="US-C"'
run cca @bob >/dev/null 2>&1
ok "手改账号表换节点：启动时同步，CCA-2 → US-C，不重载" '[ "$(sel CCA-2)" = US-C ] && [ "$(reloads)" = 1 ]'

fi
if sect 6; then
# 6. 删除账号：槽位回到 REJECT，不重载
run CCA_TTY="$(ans y)" cca rm bob >/dev/null 2>&1
ok "rm bob：退出登录、删钥匙串；CCA-2 → REJECT；不重载" 'grep -q "delete-generic-password -s Claude Code-credentials-" security.log && [ "$(sel CCA-2)" = REJECT ] && [ "$(reloads)" = 1 ] && ! grep -q bob "$H/.cca/accounts.json"'
ok "rm：账号的启动器一起删掉，别的账号的还在" '[ ! -e "$H/Applications/Claude 副号.app" ] && [ -d "$H/Applications/Claude alice.app" ]'

fi
if sect 7; then
# 7. 端口池不够（第 9 个账号）：需要重载 → 非交互时不重载，交互时问
edit_table 'j.update({"u%d@example.com" % i: "US-D" for i in range(2, 10)})'
out=$(run cca router sync 2>&1); rc=$?
ok "端口池要扩容 + 非交互：不重载，返回 3 并提示" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "没有重载" && [ "$(reloads)" = 1 ] && [ "$(nl_)" = 8 ]'
out=$(runev cca router sync 2>&1); rc=$?
ok "图形界面：要重载时记一条 consent（router_reload，范围是 OpenClash，说明里有约 20 秒），照旧不重载、返回 3" '[ $rc -eq 3 ] && [ "$(evn consent)" = 1 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"router_reload\" and e[\"scope\"] == \"OpenClash\" and e[\"grant\"] == \"router_reload:OpenClash\" and e[\"default\"] == \"yes\" and \"约 20 秒\" in e[\"detail\"]" && [ "$(reloads)" = 1 ]'
run CCA_GRANT=router_reload:ShellCrash cca router sync >/dev/null 2>&1 </dev/null; rc=$?
run CCA_GRANT=router_reload cca router sync >/dev/null 2>&1 </dev/null; rc2=$?
ok "图形界面：范围不对的同意不算（同意的是重载 ShellCrash、没写范围）：不重载" '[ $rc -eq 3 ] && [ $rc2 -eq 3 ] && [ "$(reloads)" = 1 ]'
run CCA_GRANT=router_reload:OpenClash cca router sync >/dev/null 2>&1 </dev/null
ok "端口池扩容：同意后（图形界面里点了同意，带着授权重跑）重载一次，变成 16 个槽位" '[ "$(nl_)" = 16 ] && [ "$(reloads)" = 2 ] && [ "$(sel CCA-9)" = US-D ]'

fi
if sect 8; then
# 8. 重载异常：明确拒绝 / 断开没生效 → 还原；断开但生效 → 成功
edit_table '[j.pop("u%d@example.com" % i) for i in range(2, 10)]'
touch "$R/reload_reject"; before=$(nl_)
out=$(run cca router sync -y 2>&1); rc=$?
ok "重载被 mihomo 拒绝(400)：判失败，运行配置还原" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "拒绝了新配置（HTTP 400）" && [ "$(nl_)" = "$before" ]'
rm -f "$R/reload_reject"; touch "$R/reload_noop"
out=$(run cca router sync -y 2>&1); rc=$?
ok "重载断开且没生效：等待后判失败，运行配置还原" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "运行配置已还原" && [ "$(nl_)" = "$before" ]'
rm -f "$R/reload_noop"; touch "$R/reload_drop"
out=$(run cca router sync -y 2>&1); rc=$?
ok "重载超时拿不到响应但已生效：按端口判成功（缩回 8 个槽位）" '[ $rc -eq 0 ] && [ "$(nl_)" = 8 ] && ! grep -qx 7909 "$R/listening"'
rm -f "$R/reload_drop"

fi
if sect 8b; then
# 8b. 旧入站占着端口（真实路由器上发生过）：第一次重载 7901 绑定失败 → 自动再重载一次
edit_table 'j.update({"u%d@example.com" % i: "US-D" for i in range(2, 10)})'
touch "$R/reload_conflict_once"; n0=$(reloads)
out=$(run cca router sync -y 2>&1); rc=$?
ok "旧入站占端口：第一次没绑上 7901，自动再重载一次后成功" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "再重载一次" && [ "$(reloads)" = $((n0 + 2)) ] && grep -qx 7901 "$R/listening" && [ "$(nl_)" = 16 ]'
edit_table '[j.pop("u%d@example.com" % i) for i in range(2, 10)]'
run cca router sync -y >/dev/null 2>&1

fi
if sect 8c; then
# 8c. 内核落后于运行配置文件（文件重新生成过、内核还没重新加载；真实路由器上遇到过）：内核的 CCA-1 里没有 US-A，文件里有。
#     不能只看文件的结构就当作不用重载：先改为 REJECT，问过再重载一次（同步、启动前核对都一样）
cp "$R/core.yaml" "$T/core.snap"
stale_kernel() {   # 内核加载的那份里 CCA-1 去掉 US-A、选中 HK-A；运行配置文件不动
	touch "$R/kernel_stale"
	ruby -ryaml -e 'c = YAML.load_file(ARGV[0]); c["proxy-groups"].find { |g| g["name"] == "CCA-1" }["proxies"].delete("US-A"); File.write(ARGV[0], YAML.dump(c))' "$R/core.yaml"
	printf HK-A > "$R/sel/CCA-1"
}
stale_kernel; n0=$(reloads)
out=$(run cca router sync 2>&1); rc=$?
ok "内核落后于运行配置文件 + 非交互：说清楚是内核没重新加载（不是节点不在路由器上），CCA-1 先改为 REJECT，不重载，返回 3" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "CCA-1 切换到 US-A 失败：运行配置文件里的 CCA-1 有这个节点，在跑的内核里还没有" && printf "%s" "$out" | grep -q "需要让 OpenClash 重载一次配置" && [ "$(sel CCA-1)" = REJECT ] && [ "$(reloads)" = "$n0" ]'
out=$(run CCA_TTY="$(ans y)" cca router sync 2>&1); rc=$?
ok "内核落后于运行配置文件：问过后重载一次，CCA-1 切回 US-A，同步成功" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "现在重载吗" && [ "$(reloads)" = $((n0 + 1)) ] && [ "$(sel CCA-1)" = US-A ]'
stale_kernel; n0=$(reloads)
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "启动前核对时发现内核落后：问过后重载一次，照常启动（每次连路由器只说一次）" '[ $rc -eq 3 ] && [ "$(reloads)" = $((n0 + 1)) ] && [ "$(sel CCA-1)" = US-A ] && [ "$(printf "%s" "$out" | grep -c "CCA-1 切换到 US-A 失败")" = 2 ]'
stale_kernel; n0=$(reloads)
out=$(run CCA_TTY="$(ans n)" cca @alice --version 2>&1); rc=$?
ok "启动前核对时发现内核落后、不同意重载：不启动（不会走错出口），CCA-1 留在 REJECT" '[ $rc -ne 0 ] && [ $rc -ne 3 ] && printf "%s" "$out" | grep -q "路由器账号节点未能核实" && [ "$(sel CCA-1)" = REJECT ] && [ "$(reloads)" = "$n0" ]'
rm -f "$R/kernel_stale"; cp "$T/core.snap" "$R/core.yaml"
out=$(run cca router sync 2>&1); rc=$?
ok "（还原）内核和运行配置文件一致了：经 API 切回 US-A，不重载" '[ $rc -eq 0 ] && [ "$(sel CCA-1)" = US-A ] && [ "$(reloads)" = "$n0" ]'

fi
if sect 9; then
# 9. OpenClash 升级/重装：钩子丢了、重启后没有端口池
cp pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh"
cp "$R/etc/openclash/config/custom.yaml" "$RC"; printf '7890\n7891\n7892\n7893\n7895\n9090\n' > "$R/listening"; rm -f "$R"/sel/CCA-*
n0=$(reloads); rr ensure
ok "cron 自愈：补回钩子，但不自动重载（只记录）" 'grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ "$(reloads)" = "$n0" ] && grep -q "端口池不在运行中的 OpenClash 里" "$R/tmp/cca-router.log"'
cp pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh"
run CCA_TTY="$(ans y)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "启动时发现端口池不在：征得同意后重载，补回钩子，正常启动" '[ $rc -eq 3 ] && [ "$(nl_)" = 8 ] && [ "$(sel CCA-1)" = US-A ] && grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh"'

fi
if sect 10; then
# 10. OpenClash 启动时由钩子注入端口池（不额外重载）
cp "$R/etc/openclash/config/custom.yaml" "$R/tmp/custom.yaml"
grep -v '^\. /usr/share\|LOG_OUT' "$R/etc/openclash/custom/openclash_custom_overwrite.sh" > ow_run.sh
env CCA_ROOT="$R" CCA_T="$T" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin" /bin/dash ow_run.sh "$R/tmp/custom.yaml"
ok "钩子：启动时把 8 个槽位注入配置，CCA-1 默认 US-A" '[ "$(ruby -ryaml -e "c=YAML.load_file(ARGV[0]); print c[\"listeners\"].size" "$R/tmp/custom.yaml")" = 8 ] && ruby -ryaml -e "c=YAML.load_file(ARGV[0]); exit(c[\"proxy-groups\"].find { |g| g[\"name\"]==\"CCA-1\" }[\"proxies\"][0]==\"US-A\" ? 0 : 1)" "$R/tmp/custom.yaml"'

fi
if sect 10a; then
# 10a. 路由器上给节点改了名：按服务器地址认出来，可选节点和账号表跟着改；认不出时交互让用户选一个代替。
#      运行配置文件被删了：切换节点照常，要调整端口池时征得同意后重启 OpenClash（这一节结束时整个还原）
cp -R "$R" "$T/router.snap"; cp "$H/.cca/config.json" "$T/config.snap"; cp "$H/.cca/accounts.json" "$T/accounts.snap"
SRC="$R/etc/openclash/config/custom.yaml"
oc_restart() { env CCA_ROOT="$R" CCA_T="$T" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" /bin/sh "$R/etc/init.d/openclash" restart; }
node_rn() { sed -i '' "s/{name: $1, type: ss, server: $2,/{name: $3, type: ss, server: $4,/" "$SRC"; }   # 改 OpenClash 自己的配置里节点的名字、地址
ok "选可选节点时记下了各节点的地址摘要（config.json 里只有摘要，没有服务器地址）" 'cfgok "sorted(c[\"node_ids\"]) == [\"US-A\", \"US-B\", \"US-C\", \"US-D\"] and len(set(c[\"node_ids\"].values())) == 4" && ! grep -q "192.0.2.2" "$H/.cca/config.json"'
out=$(run cca router nodes 2>&1)
ok "cca router nodes：只列名字" 'printf "%s" "$out" | grep -qx US-A && printf "%s" "$out" | grep -qx HK-A && ! printf "%s" "$out" | grep -q "192.0.2"'
node_rn US-A 192.0.2.1 US-A2 192.0.2.1; oc_restart
ok "（前提）OpenClash 重启后找不到 US-A：CCA-1 只能 REJECT" '[ "$(sel CCA-1)" = REJECT ] && grep -q "路由器上没有节点「US-A」" "$R/tmp/cca-router.log"'
n0=$(reloads)
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "节点在路由器上改了名：启动时按服务器地址认出 US-A2，可选节点和账号表跟着改，征得同意后重载一次，照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "节点「US-A」在路由器上改名为「US-A2」（服务器地址没变）：可选节点和 .* 的出口已跟着改" && tblok "\"US-A2\" in t.values() and \"US-A\" not in t.values()" && cfgok "c[\"nodes\"] == [\"US-A2\", \"US-B\", \"US-C\", \"US-D\"] and c[\"node_renames\"] == {\"US-A\": \"US-A2\"} and \"US-A2\" in c[\"node_ids\"]" && [ "$(sel CCA-1)" = US-A2 ] && [ "$(reloads)" = $((n0 + 1)) ]'
edit_table 'j.update({k: "US-C" for k in j})'
run cca router sync >/dev/null 2>&1
node_rn US-C 192.0.2.3 US-C9 192.0.2.30; oc_restart
out=$(run cca @alice --version 2>&1)
ok "名字和服务器地址都变了（认不出）、非交互：说明哪个节点找不到、哪些账号暂时用不了，账号表不动" 'printf "%s" "$out" | grep -q "路由器上找不到节点「US-C」了（也认不出它改成了哪个）" && tblok "\"US-C\" in t.values()" && [ "$(sel CCA-1)" = REJECT ]'
# 在真的终端里（pty，不设 CCA_TTY）："换成哪个节点"和"现在重载吗"都要问得出来
rm -f "$T/pty.done"
{ printf '%s\n' 3 y; for i in $(seq 1 300); do [ -f "$T/pty.done" ] && break; sleep 0.1; done; } |
	script -q "$T/pty.log" env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_POLICY_DEFAULT= CCA_HELPER="$HELPERWRAP" CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" \
	PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb /bin/bash -c 'cca @alice --version; touch "$CCA_T/pty.done"' >/dev/null 2>&1
ok "认不出、在真的终端里：列出路由器上的节点让用户选一个代替（选 US-C9），账号表跟着改，问过之后重载，照常启动" '[ -f "$T/pty.done" ] && grep -q "3\. US-C9" "$T/pty.log" && grep -q "「US-C」换成了「US-C9」" "$T/pty.log" && grep -q "现在重载吗" "$T/pty.log" && tblok "t[\"alice@example.com\"] == \"US-C9\"" && [ "$(sel CCA-1)" = US-C9 ]'
ok "用户自己选的代替节点不算改名（维护者的映射里写着 US-C 时照样要求 US-C）" 'cfgok "\"US-C\" not in c.get(\"node_renames\", {})"'
rm -f "$RC"
out=$(run cca router status 2>&1)
ok "运行配置文件被删了：cca router status 说明情况，各槽位照样读得到（控制接口的端口和密钥改从 uci 读）" 'printf "%s" "$out" | grep -q "文件不见了" && printf "%s" "$out" | grep -q "槽位 CCA-1 :7901 监听中 → US-C9"'
edit_table 'j["alice@example.com"] = "US-B"'; n0=$(reloads); r0=$(restarts)
out=$(run cca router sync 2>&1); rc=$?
ok "运行配置文件被删了、只换节点：经控制接口切过去，不重载也不重启" '[ $rc -eq 0 ] && [ "$(sel CCA-1)" = US-B ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = "$r0" ]'
edit_table 'j.update({"u%d@example.com" % i: "US-B" for i in range(2, 10)})'
out=$(run cca router sync 2>&1); rc=$?
ok "运行配置文件被删了、端口池要扩容 + 非交互：不重启，返回 3，说明要重启一次 OpenClash" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "要建立或调整账号端口池只能重启一次 OpenClash" && [ "$(restarts)" = "$r0" ]'
out=$(runev cca router sync 2>&1); rc=$?
run CCA_GRANT=router_reload:OpenClash cca router sync >/dev/null 2>&1 </dev/null; rc2=$?
ok "图形界面：要重启时记的是 router_restart（问的是现在重启 OpenClash 吗）；只同意了重载不算，不重启" '[ $rc -eq 3 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"router_restart\" and e[\"grant\"] == \"router_restart:OpenClash\" and e[\"question\"] == \"现在重启 OpenClash 吗？\"" && [ $rc2 -eq 3 ] && [ "$(restarts)" = "$r0" ]'
out=$(run cca router sync -y 2>&1); rc=$?
ok "同意后重启 OpenClash：重新生成运行配置，钩子注入 16 个槽位" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && [ -f "$RC" ] && [ "$(nl_)" = 16 ] && [ "$(sel CCA-9)" = US-B ] && printf "%s" "$out" | grep -q "已重启 OpenClash"'
# 两个可选节点地址一样（US-D 换到 US-B 的服务器上），之后 US-D 没了：不能当成改名，把它的账号合到 US-B 上
node_rn US-D 192.0.2.4 US-D 192.0.2.2; oc_restart
edit_table 'j["alice@example.com"] = "US-D"'; run cca router sync >/dev/null 2>&1
sed -i '' '/{name: US-D,/d' "$SRC"; oc_restart
out=$(run cca @alice --version 2>&1)
ok "两个可选节点地址一样、其中一个没了：不当成改名合到另一个上（账号不会悄悄换节点），说明找不到" 'printf "%s" "$out" | grep -q "路由器上找不到节点「US-D」" && tblok "t[\"alice@example.com\"] == \"US-D\"" && cfgok "\"US-D\" in c[\"nodes\"] and \"US-D\" not in c.get(\"node_renames\", {})" && [ "$(sel CCA-1)" = REJECT ]'
rm -f "$RC"; r0=$(restarts)
out=$(run cca router sync 2>&1); rc=$?
ok "运行配置文件被删了、账号的节点 OpenClash 的配置里也没有：重启也没用，不提重启，同步判失败" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "切换到 US-D 失败" && ! printf "%s" "$out" | grep -q "重启一次 OpenClash" && [ "$(restarts)" = "$r0" ]'
rm -rf "$R"; cp -R "$T/router.snap" "$R"; cp "$T/config.snap" "$H/.cca/config.json"; cp "$T/accounts.snap" "$H/.cca/accounts.json"

fi
if sect 10a2; then
# 10a2. ShellCrash（juewuy/ShellCrash）：自动认出在跑的是它（OpenClash 也装着但没在跑）；端口池在它启动时由 task/bfstart
#       里的一行钩子注入（订阅配置一个字节不动），热加载经它主目录里的链接（mihomo 不肯重载 /tmp 里的文件）。这一节结束时整个还原
rm -rf "$T/router.snap"; cp -R "$R" "$T/router.snap"; cp "$H/.cca/config.json" "$T/config.snap"; cp "$H/.cca/accounts.json" "$T/accounts.snap"
cp -R "$T/shellcrash/." "$R/"; echo CrashCore > "$R/core"
SCD="$R/etc/ShellCrash"; SCR="$R/tmp/ShellCrash/config.yaml"; cp "$SCD/yamls/config.yaml" "$T/sc_base.orig"
sc_restart() { env CCA_ROOT="$R" CCA_T="$T" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" /bin/sh "$SCD/start.sh" restart; }
scl() { ruby -ryaml -e 'c = YAML.load_file(ARGV[0]); print((c["listeners"] || []).count { |l| l["name"].to_s.start_with?("cca-") })' "$SCR" 2>/dev/null || echo -; }
sc_restart
ok "（前提）ShellCrash 照常启动：还没有 cca 的钩子，运行配置里没有端口池，用户自己的启动前任务照跑" '[ "$(scl)" = 0 ] && ! grep -qx 7901 "$R/listening" && grep -q "TASK 201" router.log'
n0=$(reloads)
out=$(run cca router sync -y 2>&1); rc=$?
ok "ShellCrash：认出在跑的是它，钩子装进 task/bfstart（用户原来的那行不动），问过后热加载一次建好端口池" '[ $rc -eq 0 ] && [ "$(reloads)" = $((n0 + 1)) ] && printf "%s" "$out" | grep -q "正在重载 ShellCrash 配置" && grep -qF "cca-router.sh sc-hook" "$SCD/task/bfstart" && [ "$(head -n 1 "$SCD/task/bfstart")" = "\$CRASHDIR/task/task.sh 201 启动前测试任务" ] && [ "$(scl)" = 8 ] && [ "$(sel CCA-1)" = US-A ] && grep -qx 7908 "$R/listening"'
ok "ShellCrash：热加载的是主目录里指向运行配置的链接，没碰 /tmp 里的路径；重载前用它的内核、它的主目录做了 -t 校验" '! grep -q UNSAFE_PATH router.log && [ -L "$SCD/config.yaml" ] && grep -q "^SCCORE -t -d $SCD -f " router.log'
out=$(run cca router status 2>&1)
ok "cca router status：说明在用的是 ShellCrash、钩子装好了，各槽位照常列出" 'printf "%s" "$out" | grep -q "ShellCrash：运行中" && printf "%s" "$out" | grep -q "钩子：已安装" && printf "%s" "$out" | grep -q "槽位 CCA-1 :7901 监听中 → US-A"'
out=$(run cca router nodes 2>&1)
ok "cca router nodes：只列订阅配置里的节点（ShellCrash 的自定义节点 SC-X 启动时才拼进来，钩子用不上）" 'printf "%s" "$out" | grep -qx US-A && printf "%s" "$out" | grep -qx HK-A && ! printf "%s" "$out" | grep -q SC-X'
printf '%s\n' 'CCA-1,US-D' 'Proxy,US-B' > "$SCD/configs/web_save"
n0=$(reloads); r0=$(restarts)
sc_restart
ok "ShellCrash 重启：钩子把订阅配置换成插好端口池的副本，运行配置里就有 8 个槽位，不额外重载；订阅配置一个字节没动" '[ "$(scl)" = 8 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(cat "$T/sc_core_config")" = "$R/tmp/cca.shellcrash.yaml" ] && cmp -s "$SCD/yamls/config.yaml" "$T/sc_base.orig" && ! grep -q SCBAD router.log && grep -q "【cca】已注入 Claude 账号端口池" "$R/tmp/ShellCrash/ShellCrash.log"'
ok "ShellCrash 重启：自定义节点照样拼进来，CCA-1 默认 US-A；面板保存的选中项里 CCA- 的去掉（不让它还原成旧节点），别的不动" 'grep -q "SC-X" "$SCR" && [ "$(sel CCA-1)" = US-A ] && [ "$(cat "$SCD/configs/web_save")" = "Proxy,US-B" ]'
touch "$R/sc_vmess"; sc_restart
ok "ShellCrash 自己追加的入站（两格缩进）和端口池的接在同一段里也读得出来" '! grep -q SCBAD router.log && [ "$(ruby -ryaml -e "print YAML.load_file(ARGV[0])[\"listeners\"].size" "$SCR")" = 9 ] && grep -qx 7999 "$R/listening"'
rm -f "$R/sc_vmess"; sc_restart
edit_table 'j["alice@example.com"] = "US-B"'; n0=$(reloads)
out=$(run cca router sync 2>&1); rc=$?
ok "ShellCrash：换节点只经控制接口切（CCA-1 → US-B），不重载" '[ $rc -eq 0 ] && [ "$(sel CCA-1)" = US-B ] && [ "$(reloads)" = "$n0" ]'
edit_table 'j["nodes"].append("SC-X")' "$H/.cca/config.json"; edit_table 'j["alice@example.com"] = "SC-X"'
out=$(run cca router sync 2>&1); rc=$?
ok "拿 ShellCrash 的自定义节点当出口：说明路由器上找不到它，端口 REJECT（不会经别的节点出去）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "找不到节点「SC-X」" && [ "$(sel CCA-1)" = REJECT ]'
edit_table 'j["nodes"].remove("SC-X")' "$H/.cca/config.json"; edit_table 'j["alice@example.com"] = "US-A"'; run cca router sync >/dev/null 2>&1
grep -vF sc-hook "$SCD/task/bfstart" > "$T/bf.tmp"; cat "$T/bf.tmp" > "$SCD/task/bfstart"; rr ensure
ok "cron 自愈：task/bfstart 里的钩子没了（比如在 ShellCrash 的任务菜单里清空过）会补回，只补一行" '[ "$(grep -cF "cca-router.sh sc-hook" "$SCD/task/bfstart")" = 1 ]'
touch "$R/stopped"
ok "ShellCrash 和 OpenClash 都没在跑：按开机自启认出是 ShellCrash" 'rr status | grep -q "ShellCrash：未运行"'
rm -f "$R/stopped"
awk '{ print } /^proxy-groups:$/ { print "  - {name: CCA-1, type: select, proxies: [US-A]}" }' "$T/sc_base.orig" > "$SCD/yamls/config.yaml"; sc_restart
ok "订阅配置里已经有叫 CCA-1 的策略组：钩子不插入（免得重名、ShellCrash 起不来），照原样启动并记下原因" '[ "$(cat "$T/sc_core_config")" = "$SCD/yamls/config.yaml" ] && [ "$(grep -c "name: CCA-1" "$SCR")" = 1 ] && ! grep -q SCBAD router.log && grep -q "已经有 cca 的端口池" "$R/tmp/cca-router.log"'
cp "$T/sc_base.orig" "$SCD/yamls/config.yaml"; printf '%s\n' 'listeners:' '  - {name: my-in, type: socks, port: 7998}' > "$SCD/yamls/others.yaml"; sc_restart
ok "ShellCrash 的 others.yaml 里自己写了 listeners：启动时不注入（会和端口池的重复），记下原因" '[ "$(cat "$T/sc_core_config")" = "$SCD/yamls/config.yaml" ] && grep -q "others.yaml 里自己写了 listeners" "$R/tmp/cca-router.log"'
rm -f "$SCD/yamls/others.yaml"; sc_restart
rm -f "$SCR"; edit_table 'j.update({"u%d@example.com" % i: "US-B" for i in range(2, 10)})'; r0=$(restarts)
out=$(run cca router sync 2>&1); rc=$?
ok "ShellCrash 的运行配置文件不见了、端口池要扩容 + 非交互：不重启，返回 3，说明要重启一次 ShellCrash" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "只能重启一次 ShellCrash" && printf "%s" "$out" | grep -q "要等 ShellCrash 下次重启" && [ "$(restarts)" = "$r0" ]'
out=$(run cca router sync -y 2>&1); rc=$?
ok "同意后重启 ShellCrash：它重新拼运行配置，钩子注入 16 个槽位" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(scl)" = 16 ] && [ "$(sel CCA-9)" = US-B ] && printf "%s" "$out" | grep -q "已重启 ShellCrash"'
out=$(run CCA_TTY="$(ans y)" cca router uninstall 2>&1)
ok "cca router uninstall：task/bfstart 只删 cca 那一行（逐字节还原），OpenClash 的钩子也删掉，热加载移除端口池" 'cmp -s "$SCD/task/bfstart" "$T/shellcrash/etc/ShellCrash/task/bfstart" && cmp -s pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ "$(scl)" = 0 ] && ! grep -qx 7901 "$R/listening" && [ ! -e "$R/etc/cca" ]'
# 下一版 ShellCrash（GitHub 上的 dev 分支）：启动前的任务文件挪到 configs/task/，运行目录挪到 configs/directories.env。
# 重新部署后像它的 init.sh 那样升级：旧的任务文件（连同 cca 那一行）mv 过去，TMPDIR/BINDIR 从 command.env 挪进 directories.env
run cca router sync -y >/dev/null 2>&1
mkdir -p "$SCD/configs/task"; mv -f "$SCD/task/bfstart" "$SCD/configs/task/bfstart"; cp "$SCD/start.next.sh" "$SCD/start.sh"
grep -E '^(TMPDIR|BINDIR)=' "$SCD/configs/command.env" > "$SCD/configs/directories.env"; sed -i '' -E '/^(TMPDIR|BINDIR)=/d' "$SCD/configs/command.env"
rr ensure
ok "下一版 ShellCrash（任务文件在 configs/task/）：升级时搬过去的钩子照用，cron 自愈不会在旧位置再写一份，status 认得" '[ "$(grep -cF "cca-router.sh sc-hook" "$SCD/configs/task/bfstart")" = 1 ] && [ ! -e "$SCD/task/bfstart" ] && rr status | grep -q "钩子：已安装"'
n0=$(reloads); sc_restart
ok "下一版 ShellCrash 重启：从 configs/task/bfstart 跑钩子，16 个槽位照样注入，不额外重载" '[ "$(cat "$T/sc_core_config")" = "$R/tmp/cca.shellcrash.yaml" ] && [ "$(scl)" = 16 ] && [ "$(reloads)" = "$n0" ] && [ "$(sel CCA-9)" = US-B ]'
hl=$(grep -F sc-hook "$SCD/configs/task/bfstart"); grep -vF sc-hook "$SCD/configs/task/bfstart" > "$T/bf.tmp"; cat "$T/bf.tmp" > "$SCD/configs/task/bfstart"
mkdir -p "$SCD/task"; printf '%s\n' "$hl" > "$SCD/task/bfstart"   # 旧版 cca 的 cron 照旧位置补上的那一份
run cca router sync >/dev/null 2>&1
ok "下一版 ShellCrash 上重新部署：钩子装进 configs/task/bfstart（用户原来那行不动）；旧位置上旧版 cca 留下的删掉（不然它下次升级会拿旧文件盖掉新的）" '[ "$(grep -cF "cca-router.sh sc-hook" "$SCD/configs/task/bfstart")" = 1 ] && [ "$(head -n 1 "$SCD/configs/task/bfstart")" = "\$CRASHDIR/task/task.sh 201 启动前测试任务" ] && [ ! -e "$SCD/task/bfstart" ]'
sed -i '' 's|^TMPDIR=.*|TMPDIR=/tmp/ShellCrash-next|' "$SCD/configs/directories.env"
ok "下一版 ShellCrash：运行目录从 configs/directories.env 读" 'rr status | grep -q "运行配置：$R/tmp/ShellCrash-next/config.yaml"'
sed -i '' 's|^TMPDIR=.*|TMPDIR=/tmp/ShellCrash|' "$SCD/configs/directories.env"
out=$(run CCA_TTY="$(ans y)" cca router uninstall 2>&1)
ok "下一版 ShellCrash 上 cca router uninstall：configs/task/bfstart 只删 cca 那一行（逐字节还原），端口池移除" 'cmp -s "$SCD/configs/task/bfstart" "$T/shellcrash/etc/ShellCrash/task/bfstart" && [ ! -e "$SCD/task/bfstart" ] && [ "$(scl)" = 0 ] && [ ! -e "$R/etc/cca" ]'
rm -rf "$R"; cp -R "$T/router.snap" "$R"; cp "$T/config.snap" "$H/.cca/config.json"; cp "$T/accounts.snap" "$H/.cca/accounts.json"

fi
if sect 10c; then
# 10c. 第三方 API 账号：直连 / 走节点；key 存钥匙串，经 apiKeyHelper 交给 Claude Code（不进命令行，也不进环境变量）
kcd() { ls -d "$T/keychain/${2:-cca-api-}$1-"* 2>/dev/null | head -n 1; }   # 钥匙串里这个账号的条目（目录里一个服务商一个文件）[服务名前缀]
kc() { local d; d=$(kcd "$1" "${2:-}"); [ -n "$d" ] && cat "$d"/* 2>/dev/null; }
kch() { local d; d=$(kcd "$1"); [ -n "$d" ] && ls "$d"; }   # 存它的服务商（接口的域名）
lastline() { grep "^$1: " "${2:-claude.log}" | tail -n 1; }   # [日志]
lastenv() { grep "^ENV: " claude.log | tail -n 1; }   # 最后一次启动 claude 的环境变量那一行
hookless() {   # hookless "ARGS 那一行" 账号目录：--settings 是记下会话的 hook（只有 SessionStart 回调 cca _session-start 这个账号目录名）时打印去掉它的那一行，别的打印 BAD
	python3 - "$1" "$(basename "$2")" <<'PY'
import json, re, shlex, sys
line, name = sys.argv[1], sys.argv[2]
m = re.match(r'^(.*?) --settings (\{"hooks": .*?\}\]\}\]\}\})( .*)?$', line)
ok = False
if m:
    try:
        h = json.loads(m.group(2))
        c = h['hooks']['SessionStart'][0]['hooks'][0]
        ok = (list(h) == ['hooks'] and list(h['hooks']) == ['SessionStart'] and len(h['hooks']['SessionStart']) == 1 and
              sorted(c) == ['command', 'timeout', 'type'] and c['type'] == 'command' and c['timeout'] == 10 and
              shlex.split(c['command'])[1:] == ['_session-start', name])   # 目录名没有要引的字符时不带引号（和 shlex.quote 一样）
    except (ValueError, KeyError, IndexError, TypeError):
        pass
print(m.group(1) + (m.group(3) or '') if ok else 'BAD')
PY
}
S1=sk-test-AAAA1111; S2=sk-test-BBBB2222; S3=sk-test-CCCC3333
: > ssh.log; : > "$T/env.log"; rm -f "$T/security.stdin"
out=$(run CCA_TTY="$(ans kimi https://api.example.com/anthropic/ "$S1" kimi-k2 '')" cca add api 2>&1); rc=$?
ok "add api：表里是对象（地址去掉末尾的 /，带 id，没有 key），直连不占端口、不同步路由器" '[ $rc -eq 0 ] && python3 -c "import json,sys; e=json.load(open(sys.argv[1]))[\"kimi\"]; c=json.load(open(sys.argv[2])); sys.exit(0 if e==dict(base_url=\"https://api.example.com/anthropic\", model=\"kimi-k2\", node=\"\", id=\"kimi\") and \"kimi\" not in c[\"ports\"] else 1)" "$H/.cca/accounts.json" "$H/.cca/config.json" && ! grep -q "sh -s" ssh.log && printf "%s" "$out" | grep -q "已添加第三方 API 账号 kimi"'
ok "add api：key 经 security -i 的 stdin（十六进制）存进钥匙串，按接口的域名存，不出现在任何命令行里" '[ "$(kc kimi)" = "$S1" ] && [ "$(kch kimi)" = api.example.com ] && grep -q "add-generic-password -U -a api.example.com -s cca-api-kimi-" "$T/security.stdin" && ! grep -q "$S1" "$T/security.stdin" security.log "$T/env.log" ssh.log'
n_trace=$(grep -c "cdn-cgi/trace" curl.log); n_login=$(grep -c "ARGS: auth login" claude.log); : > "$T/env.log"; rm -f "$T/claude.key"
out=$(run CCA_TTY="$(ans)" ANTHROPIC_BASE_URL=http://evil.invalid ANTHROPIC_AUTH_TOKEN=sk-evil-0000 ANTHROPIC_MODEL=evil CLAUDE_CODE_USE_BEDROCK=1 CLAUDE_CODE_USE_MANTLE=1 CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1 https_proxy=http://192.0.2.99:3128 HTTPS_PROXY=http://192.0.2.99:3128 cca @kimi --version 2>&1); rc=$?
api_launch_rc=$rc; api_launch_env=$(lastenv)
case "$out" in *已为本次启动屏蔽环境里继承的*) api_launch_notice=yes ;; *) api_launch_notice=no ;; esac
api_inherited_env_ok() {
	local bad='' name
	[ "$api_launch_rc" -eq 3 ] || bad="${bad} 退出码应为3"
	for name in https_proxy HTTPS_PROXY ANTHROPIC_BASE_URL ANTHROPIC_AUTH_TOKEN ANTHROPIC_MODEL CLAUDE_CODE_USE_BEDROCK CLAUDE_CODE_USE_MANTLE CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST BROWSER; do
		printf '%s\n' "$api_launch_env" | grep -qF "$name=unset" || bad="${bad} ${name}应为unset"
	done
	[ "$api_launch_notice" = yes ] || bad="${bad} 缺少屏蔽环境变量提示"
	if [ -n "$bad" ]; then
		printf '  API 环境断言失败：rc=%s，屏蔽提示=%s；不符合项：%s\n' "$api_launch_rc" "$api_launch_notice" "$bad"
		return 1
	fi
}
ok "API 账号启动：继承来的接口地址、凭证、模型、云厂商开关和代理都不带进去，并提示" 'api_inherited_env_ok'
ok "API 账号启动：地址、模型、关遥测经 --settings 的 env 传；apiKeyHelper 回调 cca 取到钥匙串里的 key" 'lastline SETTINGS | grep -qF "\"ANTHROPIC_BASE_URL\": \"https://api.example.com/anthropic\"" && lastline SETTINGS | grep -qF "\"ANTHROPIC_MODEL\": \"kimi-k2\"" && lastline SETTINGS | grep -qF "\"ANTHROPIC_DEFAULT_HAIKU_MODEL\": \"kimi-k2\"" && lastline SETTINGS | grep -qF "\"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC\": \"1\"" && [ "$(lastline HELPER)" = "HELPER: rc=0" ] && [ "$(cat "$T/claude.key")" = "$S1" ]'
ok "API 账号启动：直连检测接口，不做 claude.ai 出口检测，不走官方登录、不同步路由器；key 不出现在任何日志和命令行里" '! grep -q "sh -s" ssh.log && grep -q "CURL api direct https://api.example.com/anthropic" curl.log && [ "$(grep -c "cdn-cgi/trace" curl.log)" = "$n_trace" ] && [ "$(grep -c "ARGS: auth login" claude.log)" = "$n_login" ] && ! grep -q "$S1" claude.log "$T/env.log" security.log ssh.log curl.log open.log'
out=$(run CCA_TTY="$(ans)" ANTHROPIC_BASE_URL=http://evil.invalid cca @alice --version 2>&1); rc=$?
ok "官方账号：shell 里继承的 ANTHROPIC_BASE_URL 不带进去（否则登录 token 会发到别的地址）；--settings 里只有记下会话的 hook（没有 env、apiKeyHelper）" '[ $rc -eq 3 ] && lastenv | grep -q "ANTHROPIC_BASE_URL=unset" && lastenv | grep -q "BROWSER=$H/.cca/bin/open-url" && [ "$(lastline SETTINGS)" = "SETTINGS: env=null" ] && [ "$(hookless "$(lastline ARGS)" "$(ls -d "$H"/.cca/accounts/alice-example-com-*)")" != BAD ] && printf "%s" "$out" | grep -q "屏蔽环境里继承的 ANTHROPIC_BASE_URL"'
# Claude 默认只把这个账号自己的目录（~/.cca/accounts 下）加成工作目录，不是整个 ~/.cca；登录凭证和浏览器资料不管在不在工作目录里都挡住
P1="--disallowedTools=Read(/$H/.cca/policy/id_ed25519),Edit(/$H/.cca/policy/id_ed25519),Read(/$H/.cca/policy/nodes.json),Edit(/$H/.cca/policy/nodes.json),Read(/$H/.cca/hub/hub.json),Edit(/$H/.cca/hub/hub.json),Read(/$H/.cca/hub/id_ed25519),Edit(/$H/.cca/hub/id_ed25519),Read(/$H/.cca/hub-hosted.json),Edit(/$H/.cca/hub-hosted.json),Read(/$H/.cca/codex/accounts/*/codex/auth.json),Edit(/$H/.cca/codex/accounts/*/codex/auth.json),Read(/$H/.cca/accounts/*/chrome/**),Edit(/$H/.cca/accounts/*/chrome/**),Read(/$H/.cca/accounts/*/desktop/**),Edit(/$H/.cca/accounts/*/desktop/**),Read(/$H/.cca/codex/accounts/*/chrome/**),Edit(/$H/.cca/codex/accounts/*/chrome/**),Read(/$H/.cca/codex/accounts/*/desktop/**),Edit(/$H/.cca/codex/accounts/*/desktop/**),Read(/$H/.cca/core/nodes.json),Edit(/$H/.cca/core/nodes.json),Read(/$H/.cca/core/custom-nodes.json),Edit(/$H/.cca/core/custom-nodes.json),Read(/$H/.cca/core/config.yaml),Edit(/$H/.cca/core/config.yaml),Read(/$H/.cca/accounts/*/claude/.credentials.json),Edit(/$H/.cca/accounts/*/claude/.credentials.json),Read(/$H/.cca/secrets/**),Edit(/$H/.cca/secrets/**),Read(/$H/.cca/agy/accounts/*/gemini/antigravity-cli/antigravity-oauth-token),Edit(/$H/.cca/agy/accounts/*/gemini/antigravity-cli/antigravity-oauth-token),Read(/$H/.cca/agy/accounts/*/chrome/**),Edit(/$H/.cca/agy/accounts/*/chrome/**)"
AAD=$(ls -d "$H"/.cca/accounts/alice-example-com-*); KAD=$(ls -d "$H"/.cca/accounts/kimi-*)
# 账号目录里链到 ~/.claude 的共享目录（这时主目录里有的：预建的九个，和假 claude 上次会话里写的 output-styles）也加上，写真实路径
# （Claude Code 判断权限时先解析链接）；照 CLAUDE_SHARED 的顺序，ide 不加
SD="--add-dir=$H/.claude/projects --add-dir=$H/.claude/skills --add-dir=$H/.claude/commands --add-dir=$H/.claude/agents --add-dir=$H/.claude/plugins --add-dir=$H/.claude/todos --add-dir=$H/.claude/tasks --add-dir=$H/.claude/plans --add-dir=$H/.claude/file-history --add-dir=$H/.claude/output-styles"
out=$(run CCA_TTY="$(ans)" cca @alice "看看 ~/.cca 里的配置" 2>&1); rc=$?
ok "Claude 默认只把这个账号自己的目录（CLAUDE_CONFIG_DIR 的上一层）和它链到的共享目录（ide 除外）加成工作目录，不是整个 ~/.cca；登录凭证和浏览器资料用 --disallowedTools 挡住（整个 ~/.cca 的）；都写成 --x=值，后面的提示词不会被当成目录；再带着记下会话的 hook（--settings，都在用户的参数前面）" '[ $rc -eq 3 ] && [ "$(hookless "$(lastline ARGS)" "$AAD")" = "ARGS: --add-dir=${AAD} $SD $P1 看看 ~/.cca 里的配置" ] && lastenv | grep -qF "CLAUDE_CONFIG_DIR=${AAD}/claude "'
ok "记忆目录交给 Claude Code 的是 ~/.claude 的真实路径（CLAUDE_CODE_REMOTE_MEMORY_DIR：账号的 projects 链到它；不然解析链接以后它认不出自己的记忆，读写都要确认）" '[ "$(lastenv | sed -n "s/.* CLAUDE_CODE_REMOTE_MEMORY_DIR=//p")" = "$H/.claude" ] && [ "$(readlink "$AAD/claude/projects")" = "$H/.claude/projects" ]'
run CCA_TTY="$(ans)" cca @kimi --version >/dev/null 2>&1; a1=$(lastline ARGS)
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['claude_add_dir'] = False; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; a2=$(lastline ARGS)
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('claude_add_dir'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
ok "第三方 API 账号同样只放开它自己的目录和共享目录（在 --settings 前面）；config.json 的 claude_add_dir 设成 false 就不加" 'printf "%s" "$a1" | grep -qF "ARGS: --add-dir=${KAD} $SD $P1 --settings {" && [ "$(hookless "$a2" "$AAD")" = "ARGS: --version" ]'
# Claude Code 2.1.329 起，没信任过的 --add-dir 目录每次启动都要问一遍（Adding a new directory，回答只管这一次会话）：启动前的
# py sync 在 projects 里给交给它的每个目录记上 hasTrustDialogAccepted，账号的 .claude.json（Claude Code 读的那份）和主目录的都有
trust_ok() {   # trust_ok .claude.json 目录…：每个目录在 projects 里都是 {"hasTrustDialogAccepted": true, …}
	python3 -c 'import json,sys; p=json.load(open(sys.argv[1])).get("projects", {}); sys.exit(0 if all(isinstance(p.get(d), dict) and p[d].get("hasTrustDialogAccepted") is True for d in sys.argv[2:]) else 1)' "$@"
}
tdirs=${SD//--add-dir=/}
ok "启动前给交给 Claude Code 的每个 --add-dir 记上信任过（不然每次启动都要一个个确认）：官方账号、第三方 API 账号各自的 .claude.json 和主目录的 ~/.claude.json 里都有；claude_add_dir 关掉那一次不删" 'trust_ok "$AAD/claude/.claude.json" "$AAD" $tdirs && trust_ok "$KAD/claude/.claude.json" "$KAD" $tdirs && trust_ok "$H/.claude.json" "$AAD" "$KAD" $tdirs && ! trust_ok "$H/.claude.json" "$H/.claude"'
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['cost'] = False; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; a3=$(lastline ARGS)
run CCA_TTY="$(ans)" cca @kimi --version >/dev/null 2>&1; s3=$(lastline SETTINGS); a4=$(lastline ARGS)
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('cost'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
ok "折合金额关掉了（config.json 的 cost 是 false）：不带记下会话的 hook（官方账号没有 --settings，第三方 API 账号的 --settings 里没有 hooks）" '[ "$a3" = "ARGS: --add-dir=${AAD} $SD $P1 --version" ] && printf "%s" "$s3" | grep -q "ANTHROPIC_BASE_URL" && ! printf "%s" "$a4" | grep -q "SessionStart"'
out=$(run CCA_TTY="$(ans)" cca @alice -p hi 2>&1); rc=$?; h1=$(lastline HOOK)
out2=$(run CCA_TTY="$(ans)" cca @kimi -p hi 2>&1); rc2=$?; h2=$(lastline HOOK)
ok "Claude Code 开始会话时跑 --settings 里的 SessionStart hook：回调 cca _session-start，会话 id 记进这个账号目录的 sessions.jsonl（600）；hook 什么都不打印（stdout 会进 Claude 的上下文）、退出码 0；第三方 API 账号也记（统计时它的会话不算）" '[ $rc -eq 3 ] && [ $rc2 -eq 3 ] && printf "%s" "$h1" | grep -q " rc=0 stdout=0 stderr=0 " && printf "%s" "$h2" | grep -q " rc=0 stdout=0 stderr=0 " && grep -qF "\"sid\":\"${h1##*sid=}\"" "$AAD/sessions.jsonl" && grep -qF "\"sid\":\"${h2##*sid=}\"" "$KAD/sessions.jsonl" && [ "$(stat -f %Lp "$AAD/sessions.jsonl")" = 600 ] && ! grep -qF "${h1##*sid=}" "$KAD/sessions.jsonl"'
cp "$H/.claude/settings.json" "$T/settings.bak"; n_args=$(grep -c "^ARGS: " claude.log)
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.setdefault(\"env\", {})[\"ANTHROPIC_BASE_URL\"]=\"http://evil.invalid\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享 settings.json 的 env 里有 ANTHROPIC_BASE_URL：拒绝启动并指出来" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了第三方 API（env.ANTHROPIC_BASE_URL）" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
cp "$T/settings.bak" "$H/.claude/settings.json"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j[\"apiKeyHelper\"]=\"echo x\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @kimi --version 2>&1); rc=$?
ok "共享 settings.json 里有 apiKeyHelper：第三方 API 账号也拒绝启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了第三方 API（apiKeyHelper）"'
cp "$T/settings.bak" "$H/.claude/settings.json"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.setdefault(\"env\", {})[\"NO_PROXY\"]=\"*\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享 settings.json 的 env 里有 NO_PROXY：拒绝启动（会让请求绕过账号的端口）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了代理（env.NO_PROXY）"'
cp "$T/settings.bak" "$H/.claude/settings.json"
# 账号目录的 settings.json 被客户端原子替换：预检主配置时仍正常，合并后也必须重新检查。
shared_client="$(ls -d "$H"/.cca/accounts/alice-example-com-*)/claude"
cp "$H/.claude/settings.json" "$T/settings.atomic.bak"
python3 - "$shared_client/settings.json" <<'PY'
import json, os, sys
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data.setdefault('env', {})['ANTHROPIC_BASE_URL'] = 'https://evil.invalid'
with open(path + '.test-new', 'w') as f:
    json.dump(data, f)
os.replace(path + '.test-new', path)
PY
n_args=$(grep -c "^ARGS: " claude.log)
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "账号原子保存的 settings.json 合并后含 ANTHROPIC_BASE_URL：再次检查并拒绝启动客户端" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了第三方 API（env.ANTHROPIC_BASE_URL）" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
cp "$T/settings.atomic.bak" "$H/.claude/settings.json"
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "恢复共享配置后账号正常启动，settings.json 重新保持共享链接" '[ $rc -eq 3 ] && [ -L "$shared_client/settings.json" ] && cmp -s "$shared_client/settings.json" "$H/.claude/settings.json"'
# 另一台电脑上直接在 ~/.claude/settings.json 的 env 里配了第三方 API：说清楚是哪份文件、哪几项、为什么不继续，给出 cca settings fix；
# fix 问过后先把整份原文备份在旁边（600），再只去掉这几项（连同官方账号用不了的模型名），别的设置不动；从头到尾不说值
cp "$H/.claude/settings.json" "$T/settings.fix.bak"
python3 - "$H/.claude/settings.json" <<'PY'
import json, sys
p = sys.argv[1]
j = json.load(open(p))
j['env'] = dict(j.get('env', {}), ANTHROPIC_BASE_URL='https://api.example.com/anthropic', ANTHROPIC_AUTH_TOKEN='sk-fix-0000',
                ANTHROPIC_MODEL='kimi-k2', ANTHROPIC_DEFAULT_OPUS_MODEL='claude-opus-4-5', API_TIMEOUT_MS='600000')
j['model'] = 'kimi-k2'
j['apiKeyHelper'] = 'echo sk-fix-0000'
j['permissions'] = {'allow': ['Bash(ls:*)']}
json.dump(j, open(p, 'w'), indent=2)
PY
cp "$H/.claude/settings.json" "$T/settings.fix.orig"; n_args=$(grep -c "^ARGS: " claude.log)
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享 settings.json 里配了第三方 API：说清楚是哪份文件、哪几项、为什么不继续，给出 cca settings fix 和 cca add api（不说值、不启动）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了第三方 API（env.ANTHROPIC_BASE_URL、env.ANTHROPIC_AUTH_TOKEN、apiKeyHelper）。" && printf "%s" "$out" | grep -qF "这份设置所有账号共用" && printf "%s" "$out" | grep -qF "运行 cca settings fix 修复" && printf "%s" "$out" | grep -qF "用 cca add api 把它加成一个单独的账号" && ! printf "%s" "$out" | grep -q "sk-fix-0000\|api.example.com\|kimi-k2" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
out=$(run cca settings 2>&1); rc=$?
ok "cca settings：同样的说明，退出码 1" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 里设置了第三方 API（"'
out=$(run CCA_TTY="$(ans n)" cca settings fix 2>&1); rc=$?
ok "cca settings fix 先列出要去掉的（连同官方账号用不了的模型名；Claude 的模型名和别的设置不在里面），答 n 什么都不动" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "env.ANTHROPIC_MODEL、model（官方账号用不了的模型名）" && ! printf "%s" "$out" | grep -q "ANTHROPIC_DEFAULT_OPUS_MODEL\|API_TIMEOUT_MS\|permissions" && printf "%s" "$out" | grep -qF "没有改动" && cmp -s "$H/.claude/settings.json" "$T/settings.fix.orig" && ! ls "$H/.claude" | grep -q cca-backup'
out=$(run CCA_TTY="$(ans y)" cca settings fix 2>&1); rc=$?
BK=$(ls "$H/.claude" | grep '^settings\.json\.cca-backup-' | head -n 1)
chk=$(python3 - "$H/.claude/settings.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1]))
e = j.get('env', {})
assert not any(k in e for k in ('ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_MODEL'))
assert e['ANTHROPIC_DEFAULT_OPUS_MODEL'] == 'claude-opus-4-5' and e['API_TIMEOUT_MS'] == '600000'
assert 'apiKeyHelper' not in j and 'model' not in j and j['permissions'] == {'allow': ['Bash(ls:*)']}
print('ok')
PY
)
ok "答 y：原文整份备份在旁边（600），只去掉这几项，别的设置原样留着；说备份在哪、怎么恢复、cca add api（不说值）" '[ $rc -eq 0 ] && [ -n "$BK" ] && cmp -s "$H/.claude/$BK" "$T/settings.fix.orig" && [ "$(stat -f %Lp "$H/.claude/$BK")" = 600 ] && [ "$chk" = ok ] && printf "%s" "$out" | grep -qF "已修复 ~/.claude/settings.json：去掉了 5 项" && printf "%s" "$out" | grep -qF "cp ~/.claude/$BK ~/.claude/settings.json" && printf "%s" "$out" | grep -qF "cca add api" && ! printf "%s" "$out" | grep -q "sk-fix-0000"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
out=$(run CCA_TTY="$(ans)" cca settings fix 2>&1); rc2=$?
ok "修好以后账号照常启动，账号目录里的 settings.json 还是指向共享的那份；再运行一次 fix：没有要修的" '[ $rc -eq 3 ] && [ -L "$shared_client/settings.json" ] && [ $rc2 -eq 0 ] && printf "%s" "$out" | grep -qF "没有要修的"'
rm -f "$H/.claude/$BK"; cp "$T/settings.fix.orig" "$H/.claude/settings.json"
out=$(runev cca settings fix 2>&1); rc=$?
SC=$(python3 -c 'import json,sys; print(next(e["grant"] for e in map(json.loads, open(sys.argv[1])) if e["ev"] == "consent"))' "$T/ev.jsonl" 2>/dev/null)
ok "非交互（界面 App）：记一条 consent（settings_fix，范围是这次要改的摘要，不带值），不改" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"settings_fix\" and len(e[\"scope\"]) == 16 and \"sk-fix-0000\" not in str(e)" && [ "$SC" = "settings_fix:$(printf "%s" "$SC" | cut -d: -f2)" ] && cmp -s "$H/.claude/settings.json" "$T/settings.fix.orig"'
runev CCA_GRANT=settings_fix:0000000000000000 cca settings fix >/dev/null 2>&1; rc=$?
runev CCA_GRANT="$SC" cca settings fix >/dev/null 2>&1; rc2=$?
ok "范围不对的同意不算；带着这次的同意（CCA_GRANT）再跑就改好" '[ $rc -ne 0 ] && [ $rc2 -eq 0 ] && ! grep -q ANTHROPIC_AUTH_TOKEN "$H/.claude/settings.json" && [ "$(ls "$H/.claude" | grep -c "^settings\.json\.cca-backup-")" = 1 ]'
rm -f "$H/.claude"/settings.json.cca-backup-*
mkdir -p "$T/dotfiles"; mv "$H/.claude/settings.json" "$T/settings.fix.real"
python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j.setdefault("env", {})["HTTPS_PROXY"]="http://127.0.0.1:9"; json.dump(j, open(sys.argv[2], "w"))' "$T/settings.fix.real" "$T/dotfiles/settings.json"
ln -s "$T/dotfiles/settings.json" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans y)" cca settings fix 2>&1); rc=$?
ok "settings.json 是指向别处（dotfiles）的链接：改它指向的文件、链接留着，备份在 ~/.claude 里（不写进链接指向的目录）；只有代理时只去掉代理、不提 cca add api" '[ $rc -eq 0 ] && [ -L "$H/.claude/settings.json" ] && ! grep -q HTTPS_PROXY "$T/dotfiles/settings.json" && ls "$H/.claude" | grep -q "^settings\.json\.cca-backup-" && ! ls "$T/dotfiles" | grep -q cca-backup && printf "%s" "$out" | grep -qF "env.HTTPS_PROXY（代理）" && ! printf "%s" "$out" | grep -qF "cca add api"'
rm -f "$H/.claude/settings.json" "$H/.claude"/settings.json.cca-backup-*; mv "$T/settings.fix.real" "$H/.claude/settings.json"
printf '{"env": {' > "$T/settings.bad"; cp "$T/settings.bad" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans y)" cca settings fix 2>&1); rc=$?
ok "不是合法的 JSON：不改、不备份，说清楚" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "~/.claude/settings.json 不是合法的 JSON 设置，cca 不改它" && cmp -s "$H/.claude/settings.json" "$T/settings.bad" && ! ls "$H/.claude" | grep -q cca-backup'
cp "$T/settings.fix.bak" "$H/.claude/settings.json"
mkdir -p "$T/proj/.claude"; printf '{"env": {"ANTHROPIC_BASE_URL": "https://api.example.com"}}' > "$T/proj/.claude/settings.local.json"
out=$(cd "$T/proj" && run CCA_TTY="$(ans y)" cca settings fix 2>&1); rc=$?
ok "项目设置里的 cca settings fix 不改：照实说是哪份文件、要手动删" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -qF "项目设置 " && printf "%s" "$out" | grep -qF "/proj/.claude/settings.local.json 里设置了第三方 API（env.ANTHROPIC_BASE_URL）。" && printf "%s" "$out" | grep -qF "删掉这几项再启动" && grep -q ANTHROPIC_BASE_URL "$T/proj/.claude/settings.local.json" && ! ls "$H/.claude" | grep -q cca-backup'
rm -rf "$T/proj" "$T/dotfiles"
edit_table "j['kimi']['key'] = '$S2'"
rm -f "$T/claude.key"; n_find=$(grep -c "find-generic-password" security.log)
run CCA_TTY="$(ans)" cca @kimi --version >/dev/null 2>&1; rc=$?
ok "账号表里写了 key 字段：用表里的，不去读钥匙串" '[ $rc -eq 3 ] && [ "$(cat "$T/claude.key")" = "$S2" ] && [ "$(grep -c "find-generic-password" security.log)" = "$n_find" ]'
edit_table "j['kimi'].pop('key')"
rm -rf "$(kcd kimi)"
out=$(run cca @kimi --version 2>&1 </dev/null); rc=$?
ok "钥匙串里没有 key、非交互：说明怎么补上，不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "cca key kimi"'
run CCA_TTY="$(ans "$S3")" cca key kimi >/dev/null 2>&1
run CCA_TTY="$(ans)" cca @kimi --version >/dev/null 2>&1; rc=$?
ok "cca key：重新输入 key 存进钥匙串，之后照常启动" '[ $rc -eq 3 ] && [ "$(kc kimi)" = "$S3" ] && [ "$(cat "$T/claude.key")" = "$S3" ]'
edit_table "j['kimi']['base_url'] = 'https://api2.example.com/anthropic'"
n_args=$(grep -c "^ARGS: " claude.log)
out=$(run cca @kimi --version 2>&1 </dev/null); rc=$?
ok "接口地址换成别的服务商：钥匙串里旧服务商的 key 不拿去用，要重新输入" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "换了服务商" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
edit_table "j['kimi']['base_url'] = 'https://api.example.com/anthropic'"
d1=$(lastenv | sed 's/.*CLAUDE_CONFIG_DIR=\([^ ]*\).*/\1/')
edit_table 'j["月之暗面"] = j.pop("kimi")'
out=$(run CCA_TTY="$(ans)" cca @月之暗面 --version 2>&1); rc=$?
d2=$(lastenv | sed 's/.*CLAUDE_CONFIG_DIR=\([^ ]*\).*/\1/')
ok "改别名：按 id 认出同一个账号，账号目录和钥匙串里的 key 都还在，不用重新输入" '[ $rc -eq 3 ] && [ "$d1" = "$d2" ] && [ "$(cat "$T/claude.key")" = "$S3" ] && ! printf "%s" "$out" | grep -q "还没有"'
edit_table 'j["kimi"] = j.pop("月之暗面")'
edit_table 'j.update({"bad1": {"model": "x"}, "bad2": {"base_url": "http://api.example.com"}, "bad3": {"base_url": "https://api.example.com", "baseurl": "x"}, "bad4": {"base_url": "https://api.example.com", "env": {"ANTHROPIC_API_KEY": "x"}}, "bad5@example.com": {"base_url": "https://api.example.com"}, "bad6": {"base_url": "https://api.example.com", "node": "HK-A"}, "bad7": {"base_url": "https://api.example.com", "key": "short"}, "bad8": {"base_url": "https://api.example.com", "id": "kimi"}, "bad9": {"base_url": "https://api.example.com", "env": {"FOO_API_KEY": "x", "API_TIMEOUT_MS": "600000"}}})'
out=$(run cca @alice --version 2>&1 </dev/null); rc=$?
ok "校验第三方 API 账号：缺地址、公网 http、不认识的字段、env 里设凭证或放密钥、键是邮箱、节点不在列表、key 不合格、id 重复" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "「bad1」：缺少 base_url" && printf "%s" "$out" | grep -q "「bad2」：base_url 要用 https" && printf "%s" "$out" | grep -q "「bad3」：不认识的字段 baseurl" && printf "%s" "$out" | grep -q "「bad4」：env 里不能设 ANTHROPIC_API_KEY" && printf "%s" "$out" | grep -q "「bad5@example.com」：第三方 API 账号的键只写别名" && printf "%s" "$out" | grep -q "「bad6」：节点「HK-A」不在可选列表" && printf "%s" "$out" | grep -q "「bad7」：key 应该是" && printf "%s" "$out" | grep -q "「kimi」和「bad8」会用同一个账号目录" && printf "%s" "$out" | grep -q "「bad9」：env 里不要放密钥（FOO_API_KEY）" && ! printf "%s" "$out" | grep -q API_TIMEOUT_MS'
edit_table '[j.pop(k) for k in list(j) if k.startswith("bad")]'
out=$(run CCA_TTY="$(ans tmp2 http://api.example.com/x https://api.example.com/x 'bad key' "$S1" 'bad model' m1 '')" cca add api 2>&1); rc=$?
ok "add api：公网 http 地址、带空格的 key、带空格的模型名都会说明原因并重填" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "base_url 要用 https" && printf "%s" "$out" | grep -q "key 应该是一串可见的 ASCII 字符" && printf "%s" "$out" | grep -q "模型名里不能有空格或引号" && grep -q "\"tmp2\"" "$H/.cca/accounts.json"'
run CCA_TTY="$(ans y)" cca rm tmp2 >/dev/null 2>&1
run CCA_TTY="$(ans tmp3 https://api.example.com/x "$S1" '' '')" cca add api >/dev/null 2>&1
run CCA_TTY="$(ans tmp4 https://api.example.com/y "$S2" '' '')" cca add api >/dev/null 2>&1
ok "（准备）两个临时的第三方 API 账号，key 都在钥匙串里" '[ "$(kc tmp3)" = "$S1" ] && [ "$(kc tmp4)" = "$S2" ]'
run CCA_TTY="$(ans)" EDITOR="$T/edit.py" CCA_TEST_EDIT='j.pop("tmp3")' cca edit >/dev/null 2>&1
ok "在 cca edit 里删掉第三方 API 账号：钥匙串里它的 key 一起删" '[ -z "$(kc tmp3)" ] && [ "$(kc tmp4)" = "$S2" ]'
edit_table 'j.pop("tmp4")'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1
ok "直接改账号表删掉的：下次启动时清掉钥匙串里留下的 key" '[ -z "$(kc tmp4)" ]'
touch "$T/keychain_locked"
out=$(run CCA_TTY="$(ans tmp1 https://api.example.com/x "$S1" '' '')" cca add api 2>&1); rc=$?
ok "key 存不进钥匙串（security -i 失败也返回 0，读回来比对才发现）：说明原因，不留下半个账号" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没存进钥匙串" && ! grep -q tmp1 "$H/.cca/accounts.json"'
rm -f "$T/keychain_locked"
: > ssh.log; n0=$(reloads)
out=$(run CCA_TTY="$(ans relay https://api.example.com/v1 "$S2" '' 4)" cca add api 2>&1); rc=$?
ok "add api 走节点：分到端口 7902，CCA-2 经 API 切到 US-C，不重载" '[ $rc -eq 0 ] && python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"ports\"].get(\"relay\")==7902 else 1)" "$H/.cca/config.json" && [ "$(sel CCA-2)" = US-C ] && [ "$(reloads)" = "$n0" ]'
run CCA_TTY="$(ans)" cca @relay --version >/dev/null 2>&1; rc=$?
ok "走节点的 API 账号：Claude Code 和接口检测都经 7902；没填模型就不设模型变量" '[ $rc -eq 3 ] && lastenv | grep -q "https_proxy=http://192.0.2.10:7902" && grep -q "CURL via :7902 https://api.example.com/v1" curl.log && ! lastline SETTINGS | grep -q ANTHROPIC_MODEL && [ "$(cat "$T/claude.key")" = "$S2" ]'
touch "$T/api_down"
out=$(run CCA_TTY="$(ans)" cca @relay --version 2>&1); rc=$?
ok "接口连不上：不启动，说明可以用 CCA_SKIP_CHECK=1 跳过" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "连不上 https://api.example.com/v1" && printf "%s" "$out" | grep -q CCA_SKIP_CHECK'
run CCA_TTY="$(ans)" CCA_SKIP_CHECK=1 cca @relay --version >/dev/null 2>&1; rc=$?
ok "CCA_SKIP_CHECK=1：跳过接口检测照常启动" '[ $rc -eq 3 ]'
rm -f "$T/api_down"
: > open.log
out=$(run CCA_TTY="$(ans)" cca web kimi 2>&1); rc=$?
out2=$(run CCA_TTY="$(ans)" cca app kimi 2>&1); rc2=$?
ok "第三方 API 账号：cca web / cca app 说明只能用官方账号；也没有 Claude App 启动器" '[ $rc -ne 0 ] && [ $rc2 -ne 0 ] && printf "%s" "$out" | grep -q "网页版只能用官方账号" && printf "%s" "$out2" | grep -q "Claude App 只能用官方账号" && [ ! -s open.log ] && [ ! -e "$H/Applications/Claude kimi.app" ] && [ ! -e "$H/Applications/Claude relay.app" ] && [ -d "$H/Applications/Claude alice.app" ]'
out=$(run CCA_TTY="$(ans)" cca billing kimi 2>&1); rc=$?
ok "第三方 API 账号：cca billing 说明只能用官方账号，不打开浏览器" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "控制台的账单页只能用官方账号" && [ ! -s open.log ]'
out=$(run CCA_TTY="$(ans)" cca limit-resets kimi 2>&1); rc=$?
ok "第三方 API 账号：cca limit-resets 说明只能用官方账号，不打开浏览器" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用量页只能用官方账号" && [ ! -s open.log ]'
out=$(run CCA_TTY="$(ans)" cca @kimi --settings '{}' --version 2>&1); rc=$?
ok "第三方 API 账号不能再加 --settings（会把 cca 传的地址和 key 整个顶掉）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不能再加 --settings"'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：第三方 API 账号显示接口的域名、API、直连或节点；菜单里能直接添加第三方 API 账号" 'printf "%s" "$m" | grep -qE "kimi +api.example.com +API +直连" && printf "%s" "$m" | grep -qE "relay +api.example.com +API +US-C" && printf "%s" "$m" | grep -q "p      添加第三方 API 账号"'
out=$(run cca check 2>&1)
ok "cca check：第三方 API 账号检测接口连不连得上（走节点的经它的端口）" 'printf "%s" "$out" | grep -qE "kimi +直连 +接口连得上" && printf "%s" "$out" | grep -qE "relay +US-C +:7902 +接口连得上"'
run CCA_TTY="$(ans edge https://api.example.com/e "$S3" '' 5)" cca add api >/dev/null 2>&1
run CCA_TTY="$(ans y)" cca rm relay >/dev/null 2>&1
ok "rm 第三方 API 账号：删掉钥匙串里的 key 和账号目录，槽位回到 REJECT" '[ -z "$(kc relay)" ] && [ "$(sel CCA-2)" = REJECT ] && ! grep -q relay "$H/.cca/accounts.json" && [ -z "$(ls -d "$H"/.cca/accounts/relay-* 2>/dev/null)" ]'
portof() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[\"ports\"].get(sys.argv[2], \"-\"))" "$H/.cca/config.json" "$1"; }
p0=$(portof edge)
edit_table 'j["edge2"] = j.pop("edge")'; run cca ls >/dev/null 2>&1
edit_table 'j["edge3"] = j.pop("edge2")'; run cca ls >/dev/null 2>&1
ok "走节点的 API 账号改了两次别名：端口不变（7902 空出来了也不挪过去，开着的会话不会换出口），key 还在" '[ "$p0" = 7903 ] && [ "$(portof edge3)" = 7903 ] && [ "$(kc edge)" = "$S3" ]'
run CCA_TTY="$(ans y)" cca rm edge3 >/dev/null 2>&1
# 官方账号记一把 API key 备查（cca key 账号）：钥匙串里单独的条目（cca-ref-账号目录名，账户名 reference），cca 绝不拿它启动；
# --show 原样打出来（App 的「显示」「复制」经管道读它），--forget 删掉（先问）
R1=sk-ant-api03-REFtest0123456789abcdWXYZ; R2=sk-ant-api03-REFtest9876543210zyxwQRST
AAID=$(basename "$(ls -d "$H"/.cca/accounts/alice-example-com-*)")
: > "$T/env.log"; rm -f "$T/security.stdin" "$T/claude.key"
out=$(run CCA_TTY="$(ans "$R1")" cca key alice 2>&1); rc=$?
ok "cca key 官方账号：记一把 API key 备查，存进钥匙串单独的条目（cca-ref-，账户名 reference），config.json 记下掩码和记下的时间；key 不进命令行和日志；没配置设备统计时不说同步" '[ $rc -eq 0 ] && [ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$R1" ] && [ -z "$(kcd alice)" ] && cfgok "c[\"ref_keys\"] == [\"$AAID\"] and dict(c[\"key_hints\"][\"$AAID\"], at=0) == {\"host\": \"reference\", \"hint\": \"sk-ant-…WXYZ\", \"at\": 0} and c[\"key_hints\"][\"$AAID\"][\"at\"] > 1.7e9" && ! grep -qF "$R1" security.log "$T/security.stdin" "$T/env.log" && printf "%s" "$out" | grep -q "cca 不会拿它登录或启动" && ! printf "%s" "$out" | grep -q "其他设备" && [ ! -e "$H/.cca/presence/keys.json" ]'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
out=$(run cca _api-key "$AAID" 2>/dev/null); rc2=$?
ok "官方账号记的 key 绝不拿来启动：--settings 里只有记下会话的 hook（没有 apiKeyHelper、env），apiKeyHelper 的回调也不给它" '[ $rc -eq 3 ] && [ "$(lastline SETTINGS)" = "SETTINGS: env=null" ] && [ "$(hookless "$(lastline ARGS)" "$H/.cca/accounts/$AAID")" != BAD ] && [ ! -e "$T/claude.key" ] && [ $rc2 -ne 0 ] && [ -z "$out" ]'
v=$(run cca key alice --show 2>/dev/null); rc=$?
ok "cca key 账号 --show：stdout 原样是 key，不换行" '[ $rc -eq 0 ] && [ "$v" = "$R1" ] && [ "$(run cca key alice --show 2>/dev/null | wc -c | tr -d " ")" = "${#R1}" ]'
out=$(run CLAUDECODE=1 cca key alice --show 2>&1); rc=$?
ok "在 Claude Code 的会话里运行 --show：不打出来（会进对话记录）" '[ $rc -ne 0 ] && ! printf "%s" "$out" | grep -qF "$R1" && printf "%s" "$out" | grep -q "会进对话记录"'
run cca rename alice@example.com 主号 >/dev/null 2>&1
v=$(run cca key 主号 --show 2>/dev/null); st=$(gj state)
run cca rename 主号 - >/dev/null 2>&1
ok "改了别名记下的 key 还在（按账号目录名认）；_gui state 的 api_key 照 config.json 说（掩码），不读钥匙串" '[ "$v" = "$R1" ] && printf "%s" "$st" | jok "(lambda k: dict(k, saved_epoch=0) == {\"saved\": True, \"hint\": \"sk-ant-…WXYZ\", \"source\": \"keychain\", \"shared\": False, \"saved_epoch\": 0, \"from_device\": None, \"sync_error\": None} and k[\"saved_epoch\"] > 1.7e9)(acct(0, \"主号(alice@example.com)\")[\"api_key\"]) and acct(0, \"主号(alice@example.com)\")[\"can\"][\"api_key\"]" && ! printf "%s" "$st" | grep -qF "$R1"'
v=$(run cca key kimi --show 2>/dev/null); out=$(runev cca key kimi --forget 2>&1); rc=$?
ok "第三方 API 账号：--show 和启动时取的一样；--forget 不删它的 key（启动要用）" '[ "$v" = "$S3" ] && [ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"api_account\"" && [ "$(kc kimi)" = "$S3" ]'
run CCA_TTY="$(ans "$R2")" cca key alice >/dev/null 2>&1
touch "$T/keychain_unreadable"; out=$(runev cca key alice --show 2>&1); rc=$?; rm -f "$T/keychain_unreadable"
ok "再记一次换成新的；钥匙串读不了（锁着）时说明原因（keychain_unavailable），不当成没有、不删记录" '[ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$R2" ] && [ $rc -ne 0 ] && ! printf "%s" "$out" | grep -qF "$R2" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"keychain_unavailable\"" && cfgok "\"$AAID\" in c[\"ref_keys\"]"'
out=$(runev cca key alice --forget 2>&1); rc=$?
ok "--forget 先要同意：非交互时发 consent 事件（key_forget:账号目录名，默认不删），key 还在" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"grant\"] == \"key_forget:$AAID\" and e[\"default\"] == \"no\"" && [ -f "$T/keychain/cca-ref-$AAID/reference" ]'
out=$(runev CCA_GRANT="key_forget:$AAID" cca key alice --forget 2>&1); rc=$?
v=$(runev cca key alice --show 2>/dev/null); rc2=$?
ok "同意后删掉：钥匙串的条目和 config.json 里的记录都没了，--show 说还没有（key_missing）" '[ $rc -eq 0 ] && [ ! -e "$T/keychain/cca-ref-$AAID" ] && cfgok "\"ref_keys\" not in c and \"$AAID\" not in c.get(\"key_hints\", {})" && [ $rc2 -ne 0 ] && [ -z "$v" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"key_missing\""'
touch "$T/keychain_locked"; out=$(runev CCA_TTY="$(ans "$R2")" cca key alice 2>&1); rc=$?; rm -f "$T/keychain_locked"
ok "记 key 时钥匙串存不进去：说一次原因（App 收到 fail 事件 keychain_unavailable），不留下记录" '[ $rc -ne 0 ] && [ "$(printf "%s" "$out" | grep -c "存进钥匙串失败")" = 1 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"keychain_unavailable\"" && [ ! -e "$T/keychain/cca-ref-$AAID" ] && cfgok "\"ref_keys\" not in c" && ! printf "%s" "$out" | grep -qF "$R2"'

# 没有钥匙串的系统（Linux 服务器）：cca 存的 key 是 ~/.cca/secrets 下权限 600 的文件（服务名当文件名，内容是域名和 key；CCA_KEYSTORE=file
# 在 macOS 上模拟），不调 security；启动、apiKeyHelper 回调、--show、删除照常
KF1=sk-ant-api03-FILEtest0123456789abcdMNOP; KF2=sk-test-FILE2222
nsec=$(wc -l < security.log); : > "$T/env.log"; rm -f "$T/claude.key"
out=$(run CCA_KEYSTORE=file CCA_TTY="$(ans "$KF1")" cca key alice 2>&1); rc=$?
KFP="$H/.cca/secrets/cca-ref-$AAID"
ok "密钥文件：官方账号记的 key 存进 ~/.cca/secrets（目录 700、文件 600，内容是域名和 key），config.json 照样记掩码；不调 security，key 不进命令行" '[ $rc -eq 0 ] && [ "$(stat -f %Lp "$H/.cca/secrets")" = 700 ] && [ "$(stat -f %Lp "$KFP")" = 600 ] && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"host\"] == \"reference\" and d[\"key\"] == sys.argv[2] else 1)" "$KFP" "$KF1" && cfgok "c[\"ref_keys\"] == [\"$AAID\"]" && printf "%s" "$out" | grep -q "存进密钥文件" && [ "$(wc -l < security.log)" = "$nsec" ] && ! grep -qF "$KF1" "$T/env.log"'
v=$(run CCA_KEYSTORE=file cca key alice --show 2>/dev/null); rc=$?
chmod 000 "$KFP"; out=$(run CCA_KEYSTORE=file cca key alice --show 2>&1); rc2=$?; chmod 600 "$KFP"
run CCA_KEYSTORE=file CCA_TTY="$(ans y)" cca key alice --forget >/dev/null 2>&1; rc3=$?
ok "密钥文件：--show 读出来；读不了时说明原因、不当成没有；--forget 删掉文件和记录" '[ $rc -eq 0 ] && [ "$v" = "$KF1" ] && [ $rc2 -ne 0 ] && printf "%s" "$out" | grep -q "读不了 ~/.cca/secrets 里的密钥文件" && [ $rc3 -eq 0 ] && [ ! -e "$KFP" ] && cfgok "\"ref_keys\" not in c"'
out=$(run CCA_KEYSTORE=file CCA_TTY="$(ans kf https://api.example.com/anthropic "$KF2" '' '')" cca add api 2>&1); rc=$?
KFA=$(ls "$H"/.cca/secrets/cca-api-kf-* 2>/dev/null | head -n 1)
run CCA_KEYSTORE=file CCA_TTY="$(ans)" cca @kf --version >/dev/null 2>&1; rc2=$?
ok "密钥文件：第三方 API 账号的 key 按接口的域名存，启动时 apiKeyHelper 回调 cca 从文件里取到" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "key 存在密钥文件里" && [ -n "$KFA" ] && [ "$(stat -f %Lp "$KFA")" = 600 ] && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"host\"] == \"api.example.com\" else 1)" "$KFA" && [ $rc2 -eq 3 ] && [ "$(cat "$T/claude.key")" = "$KF2" ] && [ "$(wc -l < security.log)" = "$nsec" ] && ! grep -qF "$KF2" "$T/env.log" claude.log'
run CCA_KEYSTORE=file CCA_TTY="$(ans y)" cca rm kf >/dev/null 2>&1; rc=$?
ok "密钥文件：删掉第三方 API 账号，它的 key 文件一起删" '[ $rc -eq 0 ] && [ ! -e "$KFA" ] && ! grep -q "\"kf\"" "$H/.cca/accounts.json"'

fi
if sect 10d; then
# 10d. 订阅用量：经每个官方账号自己的出口，让 Claude Code 自己查（get_usage），查不到时显示缓存；菜单里附上最近一次的
setoa() {   # setoa .claude.json 字段=值…：改 oauthAccount（套餐、accountUuid）
	python3 - "$@" <<'PY'
import json, sys
p = sys.argv[1]; j = json.load(open(p)); oa = j.setdefault('oauthAccount', {})
for kv in sys.argv[2:]:
    k, v = kv.split('=', 1); oa[k] = v
json.dump(j, open(p, 'w'), indent=2)
PY
}
canned() {   # canned 邮箱 5小时% 本周% [订阅] [额外的窗口 JSON]：假 claude 给这个账号的 get_usage 回应
	mkdir -p "$T/usage"
	python3 - "$T/usage/$1.json" "$2" "$3" "${4:-max}" "${5:-}" <<'PY'
import json, sys, time
p, five, week, sub, extra = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]), sys.argv[4], json.loads(sys.argv[5] or '{}')
at = lambda s: time.strftime('%Y-%m-%dT%H:%M:%S.123456+00:00', time.gmtime(time.time() + s))
rl = {'five_hour': {'utilization': five, 'resets_at': at(2 * 3600 + 13 * 60 + 30)},
      'seven_day': {'utilization': week, 'resets_at': at(3 * 86400 + 5 * 3600)}, 'seven_day_opus': None, 'seven_day_sonnet': None}
rl.update(extra)
json.dump({'subscription_type': sub, 'rate_limits_available': True, 'rate_limits': rl}, open(p, 'w'))
PY
}
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-*); AJ="$AD/claude/.claude.json"
setoa "$AJ" organizationType=claude_max organizationRateLimitTier=default_claude_max_5x accountUuid=u-alice
run CCA_TTY="$(ans bob@example.com '' 2)" cca add >/dev/null 2>&1
run CCA_TTY="$(ans)" cca @bob --version >/dev/null 2>&1
BJ=$(ls -d "$H"/.cca/accounts/bob-example-com-*)/claude/.claude.json
setoa "$BJ" organizationType=claude_pro organizationRateLimitTier=default_claude_ai accountUuid=u-bob
canned alice@example.com 13 41
canned bob@example.com 98 72 pro '{"seven_day_sonnet": {"utilization": 60, "resets_at": "2099-01-01T00:00:00Z"}}'
out=$(run cca usage 2>&1); rc=$?
ok "cca usage：两个官方账号一起查，表里有套餐（原来的 Max 5x 现在就叫 Max）、5 小时和本周用量、分模型额度；注明第三方 API 账号不查" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qE "alice@example.com +Max +13% 2小时1[0-9]分后 +41% (今天|明天|周.) [0-9:]+" && printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -q "Sonnet 本周 60%" && printf "%s" "$out" | grep -q "第三方 API 账号没有订阅用量，不查：kimi" && [ "$(printf "%s\n" "$out" | grep -c "@example.com")" = 2 ]'
maxw() { python3 -c 'import sys, unicodedata; print(max(sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in l) for l in sys.stdin.read().splitlines()))'; }   # 每行显示宽度的最大值（中文算 2 格）
o50=$(run COLUMNS=50 cca usage 2>/dev/null)
ok "cca usage 一个账号一行：整行不超过终端的宽度（80 列最宽 78，50 列最宽 48），窄了先收起套餐、重置时间，不折行" '[ "$(printf "%s\n" "$out" | grep -v "^•" | maxw)" -le 78 ] && [ "$(printf "%s\n" "$o50" | maxw)" -le 48 ] && [ "$(printf "%s\n" "$o50" | grep -c "@example.com")" = 2 ] && printf "%s" "$o50" | grep -qE "alice@example.com +13% +41%"'
ok "cca usage：每个账号经自己的端口、用自己的配置目录无界面查询（--safe-mode），查之前先看出口地区" 'grep -q "^ARGS: -p --safe-mode --no-session-persistence --strict-mcp-config --input-format stream-json --output-format stream-json --verbose$" claude.log && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=alice@example.com https_proxy=http://192.0.2.10:7901" && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=bob@example.com https_proxy=http://192.0.2.10:7902" && grep -q "CURL via :7902 https://claude.ai/cdn-cgi/trace" curl.log'
out=$(run cca usage --json 2>/dev/null)
ok "cca usage --json：stdout 只有 JSON（套餐、各窗口、数据来源、跳过的第三方 API 账号）" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); a={x[\"account\"]: x for x in d[\"accounts\"]}; sys.exit(0 if a[\"alice@example.com\"][\"five_hour\"][\"utilization\"]==13 and a[\"alice@example.com\"][\"plan\"]==\"Max\" and a[\"bob@example.com\"][\"source\"]==\"live\" and a[\"bob@example.com\"][\"seven_day_sonnet\"][\"utilization\"]==60 and d[\"skipped_api_accounts\"]==[\"kimi\"] and \"reset_credits\" not in a[\"alice@example.com\"] else 1)"'
ok "cca usage：查到的存进账号目录的 usage.json（菜单和查不到时用）" '[ -f "$AD/usage.json" ]'
cp "$AD/usage.json" "$T/alice.usage.keep"
fable='"model_scoped": [{"display_name": "Fable", "utilization": 6, "resets_at": "2099-01-01T00:00:00Z"}], "weekly_scoped_shares": [{"limit_index": 1, "used_percent_of_weekly": 3, "allowance_percent_of_weekly": 50}]'
canned alice@example.com 13 41 max "{$fable, \"limits\": [{\"kind\": \"weekly_all\", \"scope\": null}, {\"kind\": \"weekly_scoped\", \"scope\": {\"model\": {\"id\": null, \"display_name\": \"Fable\"}}}]}"
out=$(run cca usage alice --json 2>/dev/null)
canned alice@example.com 13 41 max "{$fable, \"limits\": null}"
out2=$(run cca usage alice --json 2>/dev/null)
canned alice@example.com 13 41 max "{$fable, \"limits\": [{\"kind\": \"weekly_all\", \"scope\": null}]}"
out3=$(run cca usage alice --json 2>/dev/null)
mixed=$(run CCA_USAGE_JOBS=1 cca usage alice bob --json 2>/dev/null)
ok "cca usage：可以写好几个账号，只查这几个（App 自动刷新只查到期的；CCA_USAGE_JOBS 限制同时查几个）" 'printf "%s" "$mixed" | python3 -c "import json,sys; d=json.load(sys.stdin); sys.exit(0 if len(d[\"accounts\"]) == 2 else 1)"'
canned alice@example.com 13 41; cp "$T/alice.usage.keep" "$AD/usage.json"
ok "cca usage --json：分模型的每周额度（Fable）带上它占本周总额度多少：最多用一半、已用 3%（按 limits 里 limit_index 那一项认出是哪个模型；没有 limits 时只有一项就是它，指错了不写）" 'printf "%s" "$out" | jok "(lambda m: m[\"name\"] == \"Fable\" and m[\"utilization\"] == 6 and m[\"allowance_percent_of_weekly\"] == 50 and m[\"used_percent_of_weekly\"] == 3)(uacct(\"alice@example.com\")[\"model_scoped\"][0])" && printf "%s" "$out2" | jok "uacct(\"alice@example.com\")[\"model_scoped\"][0][\"allowance_percent_of_weekly\"] == 50" && printf "%s" "$out3" | jok "\"allowance_percent_of_weekly\" not in uacct(\"alice@example.com\")[\"model_scoped\"][0]"'
# 有没有 5 小时限制只看接口给没给 five_hour（没有 5 小时限制的套餐不给，limits 里也没有 session 那一项；cca 原样给 null，表格里是 —，App 不画这一行）。
# limits 里的 is_active 不是“生不生效”：每个账号只有一项是 true（用得最多的那一项），5 小时用得比本周少时 session 那一项就是 false；
# 5 小时的窗口还没开始计时是 0%、没有重置时间（beta.12、beta.13 拿 is_active 判断，把 Max 账号的 5 小时用量藏掉了）
lim='{"limits": [{"kind": "session", "group": "session", "percent": %s, "scope": null, "is_active": false}, {"kind": "weekly_all", "group": "weekly", "percent": 41, "scope": null, "is_active": true}]%s}'
canned alice@example.com 5 41 max "$(printf "$lim" 5 '')"
outj=$(run cca usage alice --json 2>/dev/null); st=$(gj state); out=$(run COLUMNS=160 cca usage alice 2>&1)
canned alice@example.com 0 41 max "$(printf "$lim" 0 ', "five_hour": {"utilization": 0, "resets_at": null}')"; outz=$(run cca usage alice --json 2>/dev/null)
canned alice@example.com 0 41 max '{"five_hour": null, "limits": [{"kind": "weekly_all", "group": "weekly", "percent": 41, "scope": null, "is_active": true}]}'
outn=$(run cca usage alice --json 2>/dev/null); stn=$(gj state); outt=$(run COLUMNS=160 cca usage alice 2>&1)
canned alice@example.com 13 41; cp "$T/alice.usage.keep" "$AD/usage.json"
ok "5 小时用得比本周少（limits 里 session 那一项 is_active 是 false）：5 小时照常（usage --json、_gui state、表格里都是 5%；还没开始计时的照常是 0%）" 'printf "%s" "$outj" | jok "uacct(\"alice@example.com\")[\"five_hour\"][\"utilization\"] == 5" && printf "%s" "$st" | jok "acct(0, \"alice@example.com\")[\"usage\"][\"five_hour\"][\"utilization\"] == 5" && printf "%s" "$out" | grep "alice@example.com" | grep -qE " 5% .*41%" && printf "%s" "$outz" | jok "uacct(\"alice@example.com\")[\"five_hour\"][\"utilization\"] == 0"'
ok "没有 5 小时限制的套餐（接口不给 five_hour）：usage --json、_gui state 里是 null（App 不画这一行），表格里是 —，本周照常" 'printf "%s" "$outn" | jok "uacct(\"alice@example.com\")[\"five_hour\"] is None and uacct(\"alice@example.com\")[\"seven_day\"][\"utilization\"] == 41" && printf "%s" "$stn" | jok "acct(0, \"alice@example.com\")[\"usage\"][\"five_hour\"] is None" && printf "%s" "$outt" | grep "alice@example.com" | grep -qE "— .*41%"'
# 周结算提醒（use_first，只给 App）：本周额度还剩至少 20% 时给出 [重置前 24 小时, 重置) 的区间，App 拿当前时间比；剩得少、
# 这次查询说明账号现在用不了（端口不通、没登录……）时是 null；终端的表格里不提示
soon=$(python3 -c 'import time; print(time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 10 * 3600)))')
canned alice@example.com 13 41 max "{\"seven_day\": {\"utilization\": 41, \"resets_at\": \"$soon\"}}"
outj=$(run cca usage alice --json 2>/dev/null); st=$(gj state); out=$(run COLUMNS=160 cca usage alice 2>&1)
ok "usage --json / _gui state：本周额度还剩不少时带 use_first（还剩百分之几、重置前 24 小时到重置）；终端的表格里不提示" 'printf "%s" "$outj" | jok "(lambda a: a[\"use_first\"] == {\"left_percent\": 59, \"from_epoch\": a[\"seven_day\"][\"resets_epoch\"] - 86400, \"until_epoch\": a[\"seven_day\"][\"resets_epoch\"]})(uacct(\"alice@example.com\"))" && printf "%s" "$st" | jok "acct(0, \"alice@example.com\")[\"usage\"][\"use_first\"][\"left_percent\"] == 59" && ! printf "%s" "$out" | grep -q "先用"'
canned alice@example.com 13 80.4; o1=$(run cca usage alice --json 2>/dev/null)
canned alice@example.com 13 80.5; o2=$(run cca usage alice --json 2>/dev/null)
ok "按四舍五入后显示的百分比判断：用了 80.4%（显示 80%）还剩 20% 算，80.5%（显示 81%）不算" 'printf "%s" "$o1" | jok "uacct(\"alice@example.com\")[\"use_first\"][\"left_percent\"] == 20" && printf "%s" "$o2" | jok "uacct(\"alice@example.com\")[\"use_first\"] is None"'
canned alice@example.com 13 41; run cca usage alice >/dev/null 2>&1; cp "$R/listening" "$T/listening.uf"; unlisten 7901
o1=$(run cca usage alice --json 2>/dev/null); cp "$T/listening.uf" "$R/listening"
ok "这次查询端口不通（账号现在用不了）：use_first 是 null，缓存里的用量照样给" 'printf "%s" "$o1" | jok "uacct(\"alice@example.com\")[\"error_code\"] == \"port\" and uacct(\"alice@example.com\")[\"use_first\"] is None and uacct(\"alice@example.com\")[\"seven_day\"][\"utilization\"] == 41"'
# 每周用量（usage_weeks，只给 App）：查到用量时顺带记进账号目录的 usage-weeks.json（600），一周一项，取这一周查到的最高的；
# 比最高的低了超过 5 个百分点算中间重置过（重置前的加上）。_gui state 只读：比记下的新的用量只在内存里并进来
rm -f "$AD/usage-weeks.json"
canned alice@example.com 13 30; run cca usage alice >/dev/null 2>&1; w1=$(gj state)
canned alice@example.com 13 45; run cca usage alice >/dev/null 2>&1
canned alice@example.com 13 44; run cca usage alice >/dev/null 2>&1; w2=$(gj state)
ok "每周用量：查到用量时记进 usage-weeks.json（600）；_gui state 里是还没过完的这一周（7 天），用到这一周查到的最高的（略低一点不算重置），还没有过完的周、没有统计；第三方 API 账号是 null" '[ "$(stat -f %Lp "$AD/usage-weeks.json")" = 600 ] && printf "%s" "$w1" | jok "(lambda u: len(u[\"weeks\"]) == 1 and u[\"weeks\"][0][\"used_percent\"] == 30 and u[\"weeks\"][0][\"current\"] is True and u[\"weeks\"][0][\"partial\"] is False and u[\"weeks\"][0][\"resets\"] == 0 and u[\"summary\"] == {\"weeks\": 0, \"average_percent\": None, \"maxed\": 0, \"under_half\": 0} and isinstance(u[\"since_epoch\"], float))(acct(0, \"alice@example.com\")[\"usage_weeks\"]) and all(a[\"usage_weeks\"] is None for a in d[\"tools\"][0][\"accounts\"] if a[\"kind\"] == \"api\")" && printf "%s" "$w2" | jok "(lambda w: w[\"used_percent\"] == 45 and w[\"resets\"] == 0 and abs(w[\"end_epoch\"] - w[\"start_epoch\"] - 7 * 86400) < 1)(acct(0, \"alice@example.com\")[\"usage_weeks\"][\"weeks\"][-1])"'
cp "$AD/usage-weeks.json" "$T/weeks.keep"
python3 - "$AD/usage.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1]))
j['fetchedAtMs'] += 60000
j['utilization']['seven_day']['utilization'] = 50
json.dump(j, open(sys.argv[1], 'w'))
PY
w3=$(gj state)
ok "_gui state 只读：比记下的新的用量（usage.json、Claude Code 的缓存）只在内存里并进来，不写 usage-weeks.json" 'printf "%s" "$w3" | jok "acct(0, \"alice@example.com\")[\"usage_weeks\"][\"weeks\"][-1][\"used_percent\"] == 50" && cmp -s "$AD/usage-weeks.json" "$T/weeks.keep"'
canned alice@example.com 13 10; run cca usage alice >/dev/null 2>&1; w4=$(gj state)
ok "每周用量：比这一周最高的低了超过 5 个百分点算中间重置过，重置前的加上（45 + 10），记一次" 'printf "%s" "$w4" | jok "(lambda w: w[\"used_percent\"] == 55 and w[\"resets\"] == 1 and w[\"current\"] is True)(acct(0, \"alice@example.com\")[\"usage_weeks\"][\"weeks\"][-1])"'
canned alice@example.com 13 41; cp "$T/alice.usage.keep" "$AD/usage.json"
# Claude Code 记的套餐过时了（登录以后升级了）：查用量时它回报的 subscription_type 是登录时记下的，和账号资料（organizationType）对不上
canned alice@example.com 13 41 pro
out=$(run COLUMNS=160 cca usage 2>&1); outj=$(run cca usage --json 2>/dev/null); st=$(gj state); out80=$(run cca usage 2>/dev/null)
ok "Claude Code 记的套餐和账号资料对不上：表格下面说一句、让重新登录；--json 和 _gui state 的用量里是 outdated_login_plan（对得上的是 null），记进 usage.json" 'printf "%s" "$out" | grep -qF "! alice@example.com：Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）" && [ "$(printf "%s" "$out" | grep -c "里记的套餐还是")" = 1 ] && printf "%s" "$out80" | grep -qF "! 套餐过时（记的是 Pro，现在是 Max）：cca login alice@example.com" && [ "$(printf "%s\n" "$out80" | maxw)" -le 78 ] && printf "%s" "$outj" | jok "uacct(\"alice@example.com\")[\"outdated_login_plan\"] == \"Pro\" and uacct(\"bob@example.com\")[\"outdated_login_plan\"] is None" && printf "%s" "$st" | jok "acct(0, \"alice@example.com\")[\"usage\"][\"outdated_login_plan\"] == \"Pro\"" && python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"login_plan\"] == \"pro\" else 1)" "$AD/usage.json"'
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1)
ok "启动横幅里也说一句（读上次查用量时记下的，不另外查）" 'printf "%s" "$out" | grep -qF "套餐  Claude Code 里记的套餐还是 Pro（登录时记下的，不会自己更新），账号现在是 Max：重新登录一次就会更新（cca login alice@example.com）"'
cp "$R/listening" "$T/listening.plan"; unlisten 7901
out=$(run COLUMNS=160 cca usage alice 2>&1)
cp "$T/listening.plan" "$R/listening"
ok "这次查不到（出口端口不通）：照上次记下的说" 'printf "%s" "$out" | grep -q "出口端口 7901 不通" && printf "%s" "$out" | grep -qF "! alice@example.com：Claude Code 里记的套餐还是 Pro"'
setoa "$AJ" organizationType=claude_pro; out=$(run cca usage alice --json 2>/dev/null); setoa "$AJ" organizationType=claude_max
ok "账号资料也是 Pro（又降回去了）：对上了就不说" 'printf "%s" "$out" | jok "uacct(\"alice@example.com\")[\"outdated_login_plan\"] is None"'
run CCA_TTY="$(ans)" cca login alice >/dev/null 2>&1; rc=$?
setoa "$AJ" organizationType=claude_max organizationRateLimitTier=default_claude_max_5x accountUuid=u-alice   # 假 claude 登录时只写邮箱
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1)
ok "经 cca 重新登录：登录之前把记下的套餐作废（下次查用量按 Claude Code 新记的重新记），横幅里不再说" '[ $rc -eq 0 ] && ! grep -q login_plan "$AD/usage.json" && ! printf "%s" "$out" | grep -q "里记的套餐还是"'
canned alice@example.com 13 41; run cca usage alice >/dev/null 2>&1
edit_table 'j["副号(bob@example.com)"] = j.pop("bob@example.com")'
out=$(run COLUMNS=120 cca usage 2>&1); out80=$(run cca usage 2>/dev/null)
edit_table 'j["bob@example.com"] = j.pop("副号(bob@example.com)")'
ok "cca usage：账号一列和菜单一样写别名和邮箱，两小列对齐（没起别名的别名那格留空）；终端窄了先收起有别名的账号的邮箱，给备注留地方" 'printf "%s" "$out" | grep -qE "^  副号  bob@example.com +Pro +98%" && printf "%s" "$out" | grep -qE "^ {8}alice@example.com +Max +13%" && printf "%s" "$out80" | grep -qE "^  副号 +Pro +98%.*Sonnet 本周 60%" && ! printf "%s" "$out80" | grep -q bob@example.com'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：官方账号附上最近一次的用量（读缓存，不联网）" 'printf "%s" "$m" | grep -q "5h 13% · 周 41%" && printf "%s" "$m" | grep -q "5h 98% · 周 72%"'
cp "$R/listening" "$T/listening.bak"; grep -vx 7902 "$T/listening.bak" > "$R/listening"; canned bob@example.com 50 50 pro
out=$(run COLUMNS=120 cca usage 2>&1); out80=$(run cca usage 2>/dev/null)
ok "出口端口不通：显示上次的缓存和原因（原因在前，窄的终端里先保住它），不拿账号走别的路" 'printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -qE "出口端口 7902 不通；缓存（[0-9]+ 分钟前）；Sonnet 本周 60%" && printf "%s" "$out80" | grep -q "出口端口 7902 不通"'
cp "$T/listening.bak" "$R/listening"
touch "$T/usage/unsupported"
out=$(run COLUMNS=160 cca usage alice 2>&1)
ok "Claude Code 不支持 get_usage：提示升级，用缓存顶上" 'printf "%s" "$out" | grep -q "这个版本的 Claude Code 查不了用量" && printf "%s" "$out" | grep -qE "alice@example.com +Max +13%"'
rm -f "$T/usage/unsupported"; touch "$T/usage/hang"; t0=$(date +%s)
out=$(run CCA_USAGE_TIMEOUT=2 cca usage alice 2>&1); t1=$(date +%s)
ok "查询卡住：到时间就结束（不会一直等），显示缓存" 'printf "%s" "$out" | grep -q "查询超时" && [ $((t1 - t0)) -lt 15 ]'
# 查询途中按 Ctrl-C、关掉终端（TERM）：后台的查询忽略 SIGINT，cca 自己停掉它们和它们起的进程，不等查询超时（没装 procps 的
# Linux 上读 /proc 的那条路在 tests/linux.sh）。$! 是 run 的子 shell，cca 一般是它的子进程（bash 把 env 直接 exec 了的话就是它自己）
n_usage=$(grep -c "^USAGE: " claude.log)
run CCA_USAGE_TIMEOUT=60 cca usage alice >/dev/null 2>&1 & UP=$!
for i in $(seq 1 100); do [ "$(grep -c "^USAGE: " claude.log)" -gt "$n_usage" ] && break; sleep 0.1; done
CP=$UP; ps -o command= -p "$UP" | grep -q 'cca usage' || CP=$(pgrep -P "$UP" | head -n 1)
kids=$(for j in $(pgrep -P "$CP"); do pgrep -P "$j"; done); t0=$(date +%s)
kill -TERM "${CP:-$UP}"; wait "$UP"; rc=$?
for i in $(seq 1 30); do left=""; for k in $kids; do kill -0 "$k" 2>/dev/null && left="$left $k"; done; [ -z "$left" ] && break; sleep 0.1; done; t1=$(date +%s)
pkill -f "$T/fake_usage.py"
ok "查询途中按 Ctrl-C、关掉终端：后台的查询连同它起的进程马上停掉（不等 60 秒的超时），退出码 130" '[ "$rc" = 130 ] && [ -n "$kids" ] && [ -z "$left" ] && [ $((t1 - t0)) -lt 5 ]'
rm -f "$T/usage/hang"; mkdir -p "$T/loc"; printf CN > "$T/loc/7901"; n_usage=$(grep -c "^USAGE: " claude.log)
out=$(run cca usage alice 2>&1)
ok "出口在受限地区：不拿账号去查，说明原因" 'printf "%s" "$out" | grep -q "出口在中国大陆" && [ "$(grep -c "^USAGE: " claude.log)" = "$n_usage" ]'
rm -f "$T/loc/7901"
out=$(run cca usage kimi 2>&1); rc=$?
ok "cca usage 第三方 API 账号：说明它没有订阅用量" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有订阅用量可查"'
setoa "$AJ" organizationRateLimitTier=default_claude_max_20x; setoa "$BJ" organizationType=claude_team organizationRateLimitTier=default_claude_max_5x
out=$(run cca usage 2>&1)
setoa "$AJ" 'planDisplayName=Ultra Plus'; out2=$(run cca usage alice 2>&1)
setoa "$AJ" organizationRateLimitTier=default_claude_max_5x planDisplayName=; setoa "$BJ" organizationType=claude_pro organizationRateLimitTier=default_claude_ai
ok "套餐叫法和官网一样：原来的 Max 20x 现在叫 Ultra，Team 的 5x 档位是 Team Premium；Claude Code 存了官方给的套餐名（planDisplayName）时照它写" 'printf "%s" "$out" | grep -qE "alice@example.com +Ultra +13%" && printf "%s" "$out" | grep -qE "bob@example.com +Team Premium +50%" && printf "%s" "$out2" | grep -qE "alice@example.com +Ultra Plus +13%"'
run CLAUDECODE=1 cca usage alice >/dev/null 2>&1
ok "在 Claude Code 里（! 前缀）运行 cca usage：查询的子进程不带 CLAUDECODE" 'lastenv | grep -q "CLAUDECODE=unset"'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.pop(\"oauthAccount\"); json.dump(j, open(p, \"w\"))" "$BJ"
n_usage=$(grep -c "^USAGE: " claude.log)
out=$(run cca usage 2>&1)
ok "没登录的账号：不启动查询，提示先登录" 'printf "%s" "$out" | grep -q "还没登录（cca @bob@example.com 登录）" && [ "$(grep -c "^USAGE: " claude.log)" = $((n_usage + 1)) ]'
python3 - "$AD/usage.json" <<'PY'
import json, sys, time
p = sys.argv[1]; j = json.load(open(p))
j['fetchedAtMs'] = int((time.time() - 3 * 3600 - 60) * 1000)
j['utilization']['five_hour']['resets_at'] = '2000-01-01T00:00:00Z'
json.dump(j, open(p, 'w'))
PY
m=$(run COLUMNS=160 CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：缓存旧了注明多久前；重置时间过了的窗口写已重置" 'printf "%s" "$m" | grep -q "5h 已重置 · 周 41%（3 小时前）"'
rm -f "$AD/usage.json"
python3 - "$AJ" <<'PY'
import json, sys, time
p = sys.argv[1]; j = json.load(open(p))
w = {'utilization': 7, 'resets_at': '2099-01-01T00:00:00Z'}
j['cachedUsageUtilization'] = {'fetchedAtMs': int(time.time() * 1000), 'accountUuid': 'u-alice',
                               'utilization': {'five_hour': w, 'seven_day': dict(w, utilization=9)}}
json.dump(j, open(p, 'w'))
PY
m=$(run COLUMNS=160 CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：也认 Claude Code 自己的缓存（cachedUsageUtilization）" 'printf "%s" "$m" | grep -q "5h 7% · 周 9%"'
setoa "$AJ" accountUuid=u-other
m=$(run COLUMNS=160 CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：缓存是别的登录存的（accountUuid 对不上）时不显示" '! printf "%s" "$m" | grep -q "5h 7%"'
run CCA_TTY="$(ans "$R1")" cca key bob >/dev/null 2>&1
BAID=$(basename "$(ls -d "$H"/.cca/accounts/bob-example-com-*)"); had=$([ -f "$T/keychain/cca-ref-$BAID/reference" ] && echo yes)
run CCA_TTY="$(ans y)" cca rm bob >/dev/null 2>&1
ok "cca rm 官方账号：记下备查的 API key 一起删掉（钥匙串的条目和 config.json 里的记录）" '[ "$had" = yes ] && [ ! -e "$T/keychain/cca-ref-$BAID" ] && cfgok "\"ref_keys\" not in c and \"$BAID\" not in c.get(\"key_hints\", {})"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1

fi
if sect 10e; then
# 10e. 不用路由器、只用直连的第三方 API 账号
H4="$T/home4"; mkdir -p "$H4/.claude"
r4() { env -i LANG=en_US.UTF-8 HOME="$H4" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_POLICY_DEFAULT= CCA_HELPER="$HELPERWRAP" PATH="$T/bin:$H4/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
r4 CCA_TTY="$(ans n n)" bash "$CCA" install >/dev/null 2>&1
out=$(r4 CCA_TTY="$(ans solo https://api.example.com/anthropic "$S1" '')" cca add api 2>&1); rc=$?
out2=$(r4 CCA_TTY="$(ans)" cca @solo --version 2>&1); rc2=$?
out3=$(r4 cca ls 2>&1)
ok "不配置路由器、只有直连的第三方 API 账号：照样能添加、启动和列出，不提路由器" '[ $rc -eq 0 ] && [ $rc2 -eq 3 ] && ! printf "%s%s%s" "$out" "$out2" "$out3" | grep -q "路由器" && printf "%s" "$out3" | grep -qE "solo +api.example.com +API +直连"'
r4 CCA_TTY="$(ans y)" cca rm solo >/dev/null 2>&1
# 没配置路由器、只配置了本机的代理（服务器上 cca 内置的代理，可选节点来自参考节点或拷来的）：添加官方账号不先要求配置路由器，
# 也不说同步路由器（没有路由器要同步，本机的端口池启动时配好）
cp -p "$H4/.cca/config.json" "$T/h4.config.json"
python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["nodes"]=["US-A"]; c.setdefault("local", {})["client"]="builtin"; json.dump(c, open(p, "w"), indent=1)' "$H4/.cca/config.json"
out=$(r4 CCA_TTY="$(ans erin@example.com '' 1)" cca add 2>&1); rc=$?
ok "没配置路由器、只配置了本机的代理（服务器上）：添加官方账号不先要求配置路由器，也不说同步路由器" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已添加 erin@example.com，出口 US-A" && ! printf "%s" "$out" | grep -q "路由器" && grep -q "\"erin@example.com\": \"US-A\"" "$H4/.cca/accounts.json"'
r4 CCA_TTY="$(ans y)" cca rm erin@example.com >/dev/null 2>&1
cp -p "$T/h4.config.json" "$H4/.cca/config.json"

fi
if sect 10f; then
# 10f. Codex 账号：单独的账号表 codex.json，和 Claude 账号共用端口池；每个账号一个 CODEX_HOME，~/.codex 里白名单上的
# 条目共享；登录经 codex 的 app-server、在账号专属的浏览器里；用量经 app-server 查
X="$H/.codex"; CX="$H/.cca/codex.json"
clast() { lastline "$1" codex.log; }
cedit() { edit_table "$1" "$CX"; }
ckc() { kc "$1" cca-codex-api-; }
ok "（准备）只有 Claude 账号时 config.json 里没有 codex 一节" '[ "$(cfgv "\"codex\" in c")" = False ]'
rm -f "$H/.local/bin/cxa"; out0=$(run cca ls 2>&1)   # 模拟以前装的 cca：那时还没有 cxa
[ -e "$H/.local/bin/cxa" ] || [ -L "$H/.local/bin/cxa" ] && cx0=yes || cx0=no
: > ssh.log; n0=$(reloads)
out=$(run CCA_TTY="$(ans alice@example.com '' 2)" cca codex add 2>&1); rc=$?
CD=$(ls -d "$H"/.cca/codex/accounts/alice-example-com-*); CH="$CD/codex"
codex_device=$(cat "$CH/installation_id")
ok "codex add：登录前已生成独立 installation_id" 'printf "%s\n" "$codex_device" | grep -Eq "^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$" && [ -f "$CD/device.json" ]'
ok "codex add：和 Claude 账号同一个邮箱也能加，记在 codex.json；账号目录在 ~/.cca/codex/accounts 下，不碰 Claude 的" '[ $rc -eq 0 ] && grep -q "\"alice@example.com\": \"US-B\"" "$CX" && [ -d "$CH" ] && [ -d "$(ls -d "$H"/.cca/accounts/alice-example-com-*)" ] && printf "%s" "$out" | grep -q "添加 Codex 账号" && printf "%s" "$out" | grep -q "用 alice@example.com 登录，登录后 cca 会核对"'
ok "codex add：和 Claude 账号共用端口池（Claude 的 7901 不动，Codex 分到 7902），路由器的端口池先 Claude 后 Codex，不重载" '[ "$(cfgv "c[\"ports\"]")" = "{'"'"'alice@example.com'"'"': 7901}" ] && [ "$(cfgv "c[\"codex\"][\"ports\"]")" = "{'"'"'alice@example.com'"'"': 7902}" ] && grep -q "\"slot\": 2, \"name\": \"codex-alice-example-com-" "$R/etc/cca/accounts.yaml" && [ "$(sel CCA-2)" = US-B ] && [ "$(reloads)" = "$n0" ]'
ok "以前装的 cca 升级后还没有 cxa：没用过 Codex 时不补；第一次用 cca codex 时补上，并说明怎么用" '[ $cx0 = no ] && ! printf "%s" "$out0" | grep -q "cxa" && [ "$(readlink "$H/.local/bin/cxa")" = cca ] && [ "$(printf "%s" "$out" | grep -c "以后可以直接用 cxa 代替 cca codex：cxa @别名 直接启动")" = 1 ]'
out=$(run cca ls 2>&1); out2=$(run cca codex ls 2>&1); out3=$(run cxa ls 2>&1)
ok "cca ls / cca codex ls：各列各的账号；两边轮流用不会反复同步路由器" '! printf "%s%s" "$out" "$out2" | grep -q "正在同步" && printf "%s" "$out2" | grep -qE "1\. alice@example.com +未登录 +US-B" && ! printf "%s" "$out" | grep -q "US-B"'
ok "cxa ls 就是 cca codex ls" '[ -n "$out3" ] && [ "$out3" = "$out2" ]'
out=$(run cca codex mail 2>&1); rc=$?
ok "cca codex mail：邮箱助手只用于 Claude 账号" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "邮箱助手只用于 Claude 账号"'
rm -f "$H/.local/bin/cxa"; out=$(run cca ls 2>&1); out2=$(run cca ls 2>&1)
ok "用着 Codex 的机器上 cxa 不见了：下一次运行 cca（哪个子命令都行）就补上，只说一次" '[ "$(readlink "$H/.local/bin/cxa")" = cca ] && printf "%s" "$out" | grep -q "以后可以直接用 cxa 代替 cca codex" && ! printf "%s" "$out2" | grep -q "以后可以直接用 cxa"'
: > open.log; : > curl.log
out=$(run CCA_TTY="$(ans)" OPENAI_API_KEY=sk-evil-1 CODEX_ACCESS_TOKEN=tok-evil CODEX_HOME="$T/evil" CODEX_CA_CERTIFICATE="$T/ca.pem" ALL_PROXY=socks5://evil.invalid cca codex @alice --version 2>&1); rc=$?
ok "Codex 第一次启动：经 app-server 拿登录网址（客户端名和 codex login 一样），在账号专属的浏览器里打开，走账号的端口" '[ $rc -eq 3 ] && grep -q "RPC initialize client=codex_cli_rs" codex.log && grep -F "auth.openai.com/oauth/authorize" open.log | grep -qF -- "--user-data-dir=$CD/chrome" && grep -F "auth.openai.com/oauth/authorize" open.log | grep -qF -- "--proxy-server=http://192.0.2.10:7902" && grep -q "APPSERVER: CODEX_HOME=$CH https_proxy=http://192.0.2.10:7902" codex.log'
ok "Codex 登录网址补上账号的邮箱（login_hint，登录页认的话会预填）" 'grep -F "auth.openai.com/oauth/authorize" open.log | grep -qF "login_hint=alice%40example.com"'
ok "Codex 登录：凭证只在账号自己的 CODEX_HOME 里，~/.codex 原来的登录不动" '[ -f "$CH/auth.json" ] && [ ! -L "$CH/auth.json" ] && grep -q master "$X/auth.json"'
ok "Codex 启动：--no-daemon、关掉改走系统代理的两个开关；CODEX_HOME 是账号目录，数据库在 ~/.codex；走账号的端口" 'clast ARGS | grep -qx "ARGS: --no-daemon --disable system_proxy_fallback --disable respect_system_proxy --version" && clast ENV | grep -q "CODEX_HOME=$CH CODEX_SQLITE_HOME=$X https_proxy=http://192.0.2.10:7902 HTTPS_PROXY=http://192.0.2.10:7902"'
ok "Codex 启动：继承来的 OPENAI_*、CODEX_*（会顶替登录或改地址）和 ALL_PROXY 不带进去并提示，公司根证书 CODEX_CA_CERTIFICATE 留着；fd 3 关着" 'clast ENV | grep -q "OPENAI_API_KEY=unset" && clast ENV | grep -q "CODEX_ACCESS_TOKEN=unset" && clast ENV | grep -q "ALL_PROXY=unset" && clast ENV | grep -q "CODEX_CA_CERTIFICATE=$T/ca.pem" && clast ENV | grep -q "fd3=closed" && printf "%s" "$out" | grep -q "已为本次启动屏蔽环境里继承的" && printf "%s" "$out" | grep -q "~/.codex/config.toml"'
ok "Codex 启动前的出口检测：经 7902 取 chatgpt.com 看到的 IP" 'grep -q "CURL via :7902 https://chatgpt.com/cdn-cgi/trace" curl.log && grep -q "CURL via :7902 https://ip.net.coffee/api/iprisk/203.0.113.2" curl.log'
lk() { [ -L "$CH/$1" ] && [ "$(readlink "$CH/$1")" = "$X/$1" ]; }
ok "共享：config.toml、AGENTS.md、会话、历史、技能、规则链接到 ~/.codex" 'lk config.toml && lk AGENTS.md && lk sessions && lk history.jsonl && lk skills && lk rules && lk session_index.jsonl'
ok "不共享：登录、installation_id、模型缓存、git 工作树各账号一份；桌面 App 的运行状态不链接" '[ ! -L "$CH/installation_id" ] && ! cmp -s "$CH/installation_id" "$X/installation_id" && [ ! -L "$CH/models_cache.json" ] && [ -d "$CH/worktrees" ] && [ ! -L "$CH/worktrees" ] && [ ! -e "$CH/automations" ] && [ ! -e "$CH/browser" ] && [ ! -e "$CH/.codex-global-state.json" ] && [ ! -e "$X/worktrees" ]'
ok "Codex 登录时已读到固定设备标识，启动退出不改变" 'grep -qF "LOGIN-DEVICE $codex_device @ $CH" codex.log && [ "$(cat "$CH/installation_id")" = "$codex_device" ]'
ok "会话里写的共享内容回到 ~/.codex：改 config.toml 不断链接，原子写替换掉的 session_index.jsonl 并回去再链接上" 'grep -q "touched by alice" "$X/config.toml" && grep -q "hi from alice" "$X/history.jsonl" && grep -q "from alice" "$X/session_index.jsonl" && [ -d "$X/skills/from-alice" ] && ls "$X"/sessions/2026/11/03/rollout-*.jsonl >/dev/null 2>&1'
ok "会话库里的路径改写到 ~/.codex（账号删了也找得到）" '[ "$(sqlite3 "$X/state_5.sqlite" "select count(*) from threads where rollout_path like '"'"'$X/sessions/%'"'"'")" = 1 ] && [ "$(sqlite3 "$X/state_5.sqlite" "select count(*) from threads where rollout_path like '"'"'$H/.cca/%'"'"'")" = 0 ]'
m=$(run CCA_TTY="$(ans q)" cca codex 2>&1)
ok "cca codex 菜单：标题是 Codex，账号已登录；cca 的菜单里没有 Codex 账号" 'printf "%s" "$m" | grep -q "选择 Codex 账号" && printf "%s" "$m" | grep -qE "1\. alice@example.com +已登录 +US-B" && printf "%s" "$m" | grep -q "a      添加新账号"'
n_args=$(grep -c "^ARGS: " codex.log)
for bad in 'chatgpt_base_url = "https://evil.invalid"' 'model_provider = "evil"' 'cli_auth_credentials_store = "keyring"' 'profile = "p1"
[profiles.p1]
openai_base_url = "https://evil.invalid"' '[features]
respect_system_proxy = true   # 系统代理优先'; do
	cp "$X/config.toml" "$T/codex-config.bak"; { printf '%s\n' "$bad"; cat "$T/codex-config.bak"; } > "$X/config.toml"   # 插在开头：顶层的键
	out=$(run CCA_TTY="$(ans)" cca codex @alice --version 2>&1); rc=$?
	cp "$T/codex-config.bak" "$X/config.toml"
	[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "共享的 ~/.codex 里有" || { echo "  没拒绝：$bad"; rc=0; }
	[ $rc -ne 0 ] || break
done
ok "共享的 config.toml 里有会把登录发到别处、换账号或改走系统代理的设置（含 profile 选中的那一节、行尾带注释的）：拒绝启动" '[ $rc -ne 0 ] && [ "$(grep -c "^ARGS: " codex.log)" = "$n_args" ]'
cp "$X/config.toml" "$T/codex-config.atomic.bak"
python3 - "$CH/config.toml" <<'PY'
import os, sys
path = sys.argv[1]
with open(path) as f:
    data = f.read()
with open(path + '.test-new', 'w') as f:
    f.write('openai_base_url = "https://evil.invalid"\n' + data)
os.replace(path + '.test-new', path)
PY
n_args=$(grep -c "^ARGS: " codex.log)
out=$(run CCA_TTY="$(ans)" cca codex @alice --version 2>&1); rc=$?
ok "账号原子保存的 config.toml 合并后含 openai_base_url：再次检查并拒绝启动客户端" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "config.toml:openai_base_url" && [ "$(grep -c "^ARGS: " codex.log)" = "$n_args" ]'
cp "$T/codex-config.atomic.bak" "$X/config.toml"
run CCA_TTY="$(ans)" cca codex @alice --version >/dev/null 2>&1; rc=$?
ok "恢复共享配置后 Codex 正常启动，config.toml 重新保持共享链接" '[ $rc -eq 3 ] && lk config.toml'
printf 'HTTPS_PROXY=http://evil.invalid:1\n' > "$X/.env"
out=$(run CCA_TTY="$(ans)" cca codex @alice --version 2>&1); rc=$?
rm -f "$X/.env"
ok "共享的 ~/.codex/.env 里有代理（会盖掉账号的出口）：拒绝启动并指出来" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q ".env:HTTPS_PROXY"'
cp "$X/config.toml" "$T/codex-config.bak"; { printf 'model_provider = "openai"   # 默认\ncli_auth_credentials_store = "file" # 存文件\n'; cat "$T/codex-config.bak"; } > "$X/config.toml"
run CCA_TTY="$(ans)" cca codex @alice --version >/dev/null 2>&1; rc=$?
ok "model_provider = \"openai\"（行尾带注释也认得）、notify、MCP 这些正常的设置照常启动" '[ $rc -eq 3 ]'
run CCA_TTY="$(ans)" cxa @alice --version >/dev/null 2>&1; rc=$?
ok "cxa @alice：启动的是 alice 的 codex（等于 cca codex @alice）" '[ $rc -eq 3 ] && clast ARGS | grep -qx "ARGS: --no-daemon --disable system_proxy_fallback --disable respect_system_proxy --version" && clast ENV | grep -q "CODEX_HOME=$CH "'
echo mallory@example.com > "$T/codex_login_as"
out=$(run CCA_TTY="$(ans n)" cca codex login alice 2>&1); rc=$?
ok "cca codex login：先退出原来的登录；登成了别的邮箱就不用，问过之后不继续" '[ $rc -ne 0 ] && grep -q "LOGOUT $CH" codex.log && printf "%s" "$out" | grep -q "登录的是 mallory@example.com，不是 alice@example.com"'
out=$(run CCA_TTY="$(ans y)" cca codex login alice 2>&1); rc=$?
ok "cca codex login：用对的邮箱重新登录" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "alice@example.com 已重新登录" && grep -q "LOGIN alice@example.com @ $CH" codex.log'
ok "Codex 登错号后重新登录：设备标识保持不变" '[ "$(cat "$CH/installation_id")" = "$codex_device" ] && [ "$(grep -cF "LOGIN-DEVICE $codex_device @ $CH" codex.log)" = 3 ]'
rm -f "$CH/auth.json"; touch "$T/codex_rpc_off"
out=$(run CCA_TTY="$(ans)" cca codex @alice --version 2>&1); rc=$?
rm -f "$T/codex_rpc_off"
ok "这个版本的 codex 不支持经 app-server 登录：停止，不退回系统浏览器" '[ $rc -ne 0 ] && ! grep -q "LOGIN-CLI $CH" codex.log && printf "%s" "$out" | grep -q "已停止登录"'
# 服务器上登录（3b2 那一套，Codex 经 app-server）：假 app-server 在 $T/oauth_flow 存在时照真的走（本机回调、state 不对回 400 接着等、
# 换好 token 才回答浏览器）
echo st-cx1 > "$T/oauth_flow"; : > open.log
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans 'good-cx#st-wrong' 'good-cx#st-cx1')" cca codex login alice 2>&1); rc=$?
ok "Codex 服务器上登录：经 app-server 拿登录网址，给出 cca codex login-page 那一行；验证码转给 codex 本机的回调，换 token 经账号的端口" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "  cca codex login-page alice@example.com --node US-B --node-id $FPB '\''https://auth.openai.com/oauth/authorize?" && printf "%s" "$out" | grep -q "state 对不上" && grep -q "OAUTH-EXCHANGE code=good-cx https_proxy=http://192.0.2.10:7902" codex.log && [ "$(grep -c "^OAUTH-EXCHANGE" codex.log)" = 1 ] && grep -q "LOGIN alice@example.com @ $CH" codex.log && ! grep -q auth.openai.com open.log'
echo st-cx2 > "$T/oauth_flow"
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans '')" cca codex login alice 2>&1); rc=$?
ok "Codex 服务器上登录：取消时 login-flow 和 app-server 都停掉（不留下占着回调端口的登录）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "已取消登录" && gone "$T/fake_codex_server.py" && gone "login-flow $T/"'
echo st-cx3 > "$T/oauth_flow"
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans 'bad-cx#st-cx3')" cca codex login alice 2>&1); rc=$?
ok "Codex 服务器上登录：换不到 token 时照 codex 的原话说" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "登录没有成功：.*Token exchange failed: invalid_grant"'
rm -f "$T/oauth_flow"
lp=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
LPX="https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_x&redirect_uri=http%3A%2F%2Flocalhost%3A$lp%2Fauth%2Fcallback&scope=openid&code_challenge=CC&code_challenge_method=S256&state=st-cxpage&originator=codex_cli_rs"
: > open.log; rm -f "$T/clipboard"
CCA_T="$T" python3 "$T/fake_browser.py" st-cxpage good-cxpage > "$T/page.txt" 2>&1 & fb=$!
out=$(run CCA_LOGIN_WAIT=20 cca codex login-page alice@example.com --node US-B "$LPX" 2>/dev/null); rc=$?
wait $fb
ok "cca codex login-page：在 Codex 账号专属的 Chrome 里经它的端口打开（补上 login_hint），接住 /auth/callback 的验证码" '[ $rc -eq 0 ] && [ "$out" = "good-cxpage#st-cxpage" ] && [ "$(cat "$T/clipboard")" = "good-cxpage#st-cxpage" ] && grep -F "state=st-cxpage" open.log | grep -qF -- "--user-data-dir=$CD/chrome" && grep -F "state=st-cxpage" open.log | grep -qF -- "--proxy-server=http://192.0.2.10:7902" && grep -F "state=st-cxpage" open.log | grep -qF "login_hint=alice%40example.com" && grep -qx "callback 200" "$T/page.txt"'
rm -f "$H/.cca/codex/probe.json"; touch "$T/codex_noflags"
run CCA_TTY="$(ans)" cca codex @alice --version >/dev/null 2>&1; rc=$?
rm -f "$T/codex_noflags" "$H/.cca/codex/probe.json"
ok "codex 不认识要关的开关时（删掉了）就不加 --disable（否则它直接报错退出）；按 codex 程序缓存，不每次都问" '[ $rc -eq 3 ] && clast ARGS | grep -qx "ARGS: --no-daemon --version" && [ "$(grep -c "FEATURES list" codex.log)" -ge 1 ]'
n_f=$(grep -c "FEATURES list" codex.log); run CCA_TTY="$(ans)" cca codex @alice --version >/dev/null 2>&1; run CCA_TTY="$(ans)" cca codex @alice --version >/dev/null 2>&1
ok "（探测结果按 codex 程序缓存）" '[ "$(grep -c "FEATURES list" codex.log)" = $((n_f + 1)) ]'
# 第三方 API 账号（Codex）：兼容 Responses 接口；key 存钥匙串，经 auth.command 回调 cca 取（不进命令行、环境变量）
: > "$T/env.log"; rm -f "$T/security.stdin" "$T/codex.key"
out=$(run CCA_TTY="$(ans kimi https://api.example.com/v1/ "$S1" kimi-k2 '')" cca codex add api 2>&1); rc=$?
ok "codex add api：和 Claude 的第三方 API 账号同名也行；key 存在钥匙串里 Codex 自己的条目" '[ $rc -eq 0 ] && python3 -c "import json,sys; e=json.load(open(sys.argv[1]))[\"kimi\"]; sys.exit(0 if e==dict(base_url=\"https://api.example.com/v1\", model=\"kimi-k2\", node=\"\", id=\"kimi\") else 1)" "$CX" && [ "$(ckc kimi)" = "$S1" ] && [ "$(kc kimi)" = "$S3" ] && grep -q "add-generic-password -U -a api.example.com -s cca-codex-api-kimi-" "$T/security.stdin"'
out=$(run CCA_TTY="$(ans)" cca codex @kimi --version 2>&1); rc=$?
ok "Codex 第三方 API 账号启动：-c 定义模型服务（Responses），auth.command 回调 cca 取到钥匙串里的 key；直连（NO_PROXY=*，不让它用系统代理）" '[ $rc -eq 3 ] && clast ARGS | grep -qF "model_provider=\"cca\"" && clast ARGS | grep -qF "model=\"kimi-k2\"" && grep -q "PROVIDER: base_url=https://api.example.com/v1 wire_api=responses" codex.log && [ "$(cat "$T/codex.key")" = "$S1" ] && clast ENV | grep -q "https_proxy=unset" && clast ENV | grep -q "NO_PROXY=\*" && grep -q "CURL api direct https://api.example.com/v1" curl.log'
ok "Codex 第三方 API 账号：key 不出现在任何命令行、环境变量和日志里" '! grep -qF "$S1" "$T/env.log" codex.log security.log curl.log && ! clast ARGS | grep -q "sk-"'
cedit 'j["kimi"]["env"] = {"X": "1"}'
out=$(run cca codex @kimi --version 2>&1 </dev/null); rc=$?
cedit 'j["kimi"].pop("env")'
ok "Codex 第三方 API 账号没有 env 字段（说明能用的字段）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不认识的字段 env"'
# 用量
ccanned() {   # ccanned 邮箱 5小时% 本周% [套餐] [剩余重置次数]：假 app-server 给这个账号的 account/rateLimits/read 回应；
	            # 不给剩余重置次数就是旧版本的 codex（回应里没有 rateLimitResetCredits）
	mkdir -p "$T/codex_usage"
	python3 - "$T/codex_usage/$1.json" "$2" "$3" "${4:-plus}" "${5:-}" <<'PY'
import json, sys, time
p, five, week, plan, resets = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]
now = int(time.time())
rl = {'limitId': 'codex', 'primary': {'usedPercent': five, 'windowDurationMins': 300, 'resetsAt': now + 2 * 3600 + 13 * 60 + 30},
      'secondary': {'usedPercent': week, 'windowDurationMins': 10080, 'resetsAt': now + 3 * 86400 + 5 * 3600},
      'credits': {'hasCredits': True, 'unlimited': False, 'balance': '12.5'}, 'planType': plan}
other = {'limitId': 'codex_other', 'limitName': 'GPT-5-Pro', 'primary': None,
         'secondary': {'usedPercent': 30, 'windowDurationMins': 10080, 'resetsAt': now + 86400 * 2}}
j = {'rateLimits': rl, 'rateLimitsByLimitId': {'codex': rl, 'codex_other': other}}
if resets:
    j['rateLimitResetCredits'] = {'availableCount': int(resets), 'credits': [
        {'id': 'rc-%d' % i, 'resetType': 'codexRateLimits', 'status': 'available', 'grantedAt': now - 86400,
         'expiresAt': now + 30 * 86400, 'title': None, 'description': None} for i in range(int(resets))]}
json.dump(j, open(p, 'w'))
PY
}
ccanned alice@example.com 13 41 pro 2; n_usage=$(grep -c "^USAGE: " claude.log)
out=$(run COLUMNS=160 cca codex usage 2>&1); rc=$?
ok "cca codex usage：经账号的端口让 codex 的 app-server 查（不发模型请求），表里有套餐（叫法和 codex 的 /status 一样，pro 是 Pro 200）、5 小时、本周，备注里先写剩余的重置次数，再是别的额度和 credits；第三方 API 账号不查" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qE "alice@example.com +Pro 200 +13% 2小时1[0-9]分后 +41% (今天|明天|周.) [0-9:]+" && printf "%s" "$out" | grep -qE "剩余重置 2 次（最早 [0-9]+ 月 [0-9]+ 日过期）；GPT-5-Pro 本周 30%；credits 余额 12.5" && printf "%s" "$out" | grep -q "第三方 API 账号没有订阅用量，不查：kimi" && grep -q "RPC rateLimits email=alice@example.com https_proxy=http://192.0.2.10:7902" codex.log && [ "$(grep -c "^USAGE: " claude.log)" = "$n_usage" ]'
ccanned alice@example.com 13 41 pro 1; out=$(run COLUMNS=160 cca codex usage 2>&1)
ok "cca codex usage：只有一次重置机会时不说“最早”，到期日照样写" 'printf "%s" "$out" | grep -qE "剩余重置 1 次（[0-9]+ 月 [0-9]+ 日过期）"'
ccanned alice@example.com 13 41 pro 2
out=$(run cca codex usage --json 2>/dev/null)
ok "cca codex usage --json：注明 tool=codex，窗口和 Claude 的一样写法；credits、reset_credits（剩余重置次数和明细）是 codex 原样给的" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); a=d[\"accounts\"][0]; sys.exit(0 if d[\"tool\"]==\"codex\" and a[\"plan\"]==\"Pro 200\" and a[\"five_hour\"][\"utilization\"]==13 and a[\"seven_day\"][\"utilization\"]==41 and a[\"credits\"][\"balance\"]==\"12.5\" and a[\"reset_credits\"][\"availableCount\"]==2 and len(a[\"reset_credits\"][\"credits\"])==2 and a[\"source\"]==\"live\" else 1)"'
out=$(run cca usage 2>&1)
ok "cca usage 只查 Claude 账号，不带 Codex 的" '! printf "%s" "$out" | grep -q "Pro" && printf "%s" "$out" | grep -q "alice@example.com"'
m=$(run CCA_TTY="$(ans q)" cca codex 2>&1)
ok "cca codex 菜单：附上最近一次的用量" 'printf "%s" "$m" | grep -q "5h 13% · 周 41%"'
touch "$T/codex_usage/unsupported"
out=$(run COLUMNS=200 cca codex usage alice 2>&1)
rm -f "$T/codex_usage/unsupported"
ok "codex 不支持查用量：提示升级，用缓存顶上（套餐、剩余重置次数也取缓存里的）" 'printf "%s" "$out" | grep -q "这个版本的 Codex 查不了用量" && printf "%s" "$out" | grep -qE "alice@example.com +Pro 200 +13%" && printf "%s" "$out" | grep -qE "升级后再试）；缓存（[0-9]+ 分钟前）；剩余重置 2 次（最早 [0-9]+ 月 [0-9]+ 日过期）"'
touch "$T/codex_usage/hang"; t0=$(date +%s)
out=$(run CCA_USAGE_TIMEOUT=2 cca codex usage alice 2>&1); t1=$(date +%s)
rm -f "$T/codex_usage/hang"
ok "codex 查用量卡住：到时间就结束（不会一直等），显示缓存" 'printf "%s" "$out" | grep -q "查询超时" && [ $((t1 - t0)) -lt 15 ]'
# 登录凭证失效（改过密码、在别处退出过）：本地的 auth.json 还在，cca 算的还是已登录，查用量时 codex 才说
printf 'Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.' > "$T/codex_usage/rpc_error"
out=$(run COLUMNS=200 cca codex usage alice 2>&1); js=$(run cca codex usage alice --json 2>/dev/null); printf '%s' "$js" > "$T/usage_auth.json"
ok "codex 说 token 刷新不了：原因是 auth（登录已失效，说清楚怎么重新登录），用缓存顶上" 'printf "%s" "$out" | grep -q "登录已失效" && printf "%s" "$out" | grep -q "cca codex login " && printf "%s" "$js" | python3 -c "import json,sys; a=json.load(sys.stdin)[\"accounts\"][0]; sys.exit(0 if a[\"error_code\"]==\"auth\" and a[\"source\"]==\"cache\" and \"revoked\" in a[\"error_detail\"] else 1)"'
out=$(runev CCA_GRANT="reset_credit:${CD##*/}" cca codex reset alice --json 2>&1); rc=$?
ok "重置额度遇到登录失效：说要登录（App 给重新登录的按钮），不说可能用掉了一次" '[ $rc -ne 0 ] && evq "e.get(\"ev\") == \"fail\" and e.get(\"code\") == \"not_logged_in\" and \"可能已经用掉\" not in e.get(\"message\", \"\")"'
printf 'failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage failed: 401 Unauthorized; content-type=application/json; body={}' > "$T/codex_usage/rpc_error"
js=$(run cca codex usage alice --json 2>/dev/null)
ok "服务器回 401 也算登录失效" 'printf "%s" "$js" | python3 -c "import json,sys; sys.exit(0 if json.load(sys.stdin)[\"accounts\"][0][\"error_code\"]==\"auth\" else 1)"'
printf 'failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage failed: 502 Bad Gateway; content-type=text/html; body=x' > "$T/codex_usage/rpc_error"
js=$(run cca codex usage alice --json 2>/dev/null)
rm -f "$T/codex_usage/rpc_error"
ok "别的错不算登录失效（不能吓用户去重新登录）" 'printf "%s" "$js" | python3 -c "import json,sys; sys.exit(0 if json.load(sys.stdin)[\"accounts\"][0][\"error_code\"]==\"error\" else 1)"'
ccanned alice@example.com 13 41 promax 0; out=$(run COLUMNS=160 cca codex usage alice 2>&1)
ccanned alice@example.com 13 41 prolite; out2=$(run COLUMNS=160 cca codex usage alice 2>&1)
ok "套餐叫法和 codex 的一样：promax 是 Pro 500、prolite 是 Pro 100；剩余 0 次也写上，codex 没给重置次数（旧版本）就不写" 'printf "%s" "$out" | grep -qE "alice@example.com +Pro 500 +13%" && printf "%s" "$out" | grep -q "剩余重置 0 次；GPT-5-Pro" && printf "%s" "$out2" | grep -qE "alice@example.com +Pro 100 +13%" && printf "%s" "$out2" | grep -q "GPT-5-Pro 本周 30%" && ! printf "%s" "$out2" | grep -q "剩余重置"'
# 重置额度（cca codex reset）：先经账号的出口查还能重置几次，问过之后用掉最早过期的那一次，重置后再读一次用量
ccanned alice@example.com 100 41 pro 2; : > codex.log
out=$(run CCA_TTY="$(ans '')" cca codex reset alice 2>&1); rc=$?
ok "cca codex reset：经账号的端口查到还能重置 2 次、各在什么时候过期；默认不重置（直接回车就不动）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "alice@example.com 还能重置 2 次额度" && [ "$(printf "%s" "$out" | grep -cE "· 完整重置，[0-9]+ 月 [0-9]+ 日 [0-9:]+ 过期")" = 2 ] && printf "%s" "$out" | grep -q "没有重置（还能重置 2 次）" && grep -q "RPC rateLimits email=alice@example.com https_proxy=http://192.0.2.10:7902" codex.log && ! grep -q "RPC resetConsume" codex.log'
out=$(run CCA_TTY="$(ans y)" cca codex reset alice 2>&1); rc=$?
m=$(run CCA_TTY="$(ans q)" cca codex 2>&1)
ok "cca codex reset：答 y 用掉最早过期的那一次（经账号的端口，idempotencyKey 是新生成的），重置后再读一次用量存下来" '[ $rc -eq 0 ] && grep -q "RPC resetConsume email=alice@example.com https_proxy=http://192.0.2.10:7902 credit=rc-0 key=uuid" codex.log && printf "%s" "$out" | grep -q "已重置 alice@example.com 的额度（现在 5h 0% · 周 0%）" && printf "%s" "$m" | grep -q "5h 0% · 周 0%"'
ok "_gui state：重置后的用量（缓存）里还剩 1 次，每一次的明细另有 Unix 秒的 expires_epoch，最早过期的那次 next_expires_epoch" 'gj state | jok "(lambda r: r[\"availableCount\"] == 1 and r[\"credits\"][0][\"id\"] == \"rc-1\" and isinstance(r[\"credits\"][0][\"expires_epoch\"], float) and r[\"next_expires_epoch\"] == r[\"credits\"][0][\"expires_epoch\"])(acct(1, \"alice@example.com\")[\"usage\"][\"reset_credits\"])"'
out=$(run CCA_TTY="$(ans y)" cca codex reset alice 2>&1); rc=$?
ok "cca codex reset：用量已经是 0、不需要重置时 OpenAI 不扣次数，照实说" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "现在的用量还不需要重置，没有用掉重置机会"'
ccanned alice@example.com 100 41 pro 1; printf 'weird' > "$T/codex_usage/reset_outcome"
out=$(runev CCA_GRANT="reset_credit:${CD##*/}" cca codex reset alice --json 2>&1); rc=$?; rm -f "$T/codex_usage/reset_outcome"
ok "cca codex reset：已经请求重置、结果却不知道（codex 回的看不懂、超时）：不说没重置，说可能已经用掉了一次、刷新用量看看" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and \"可能已经用掉了一次\" in e[\"message\"]"'
ccanned alice@example.com 100 41 pro 0; out=$(run CCA_TTY="$(ans y)" cca codex reset alice 2>&1); rc=$?
ccanned alice@example.com 100 41 pro; out2=$(runev cca codex reset alice --json 2>&1); rc2=$?
ok "cca codex reset：没有重置机会了就不问；codex 没给重置次数（旧版本）时说明要升级（fail 原因 reset_unsupported）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "alice@example.com 没有能用的重置机会" && [ $rc2 -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"reset_unsupported\" and \"这个版本的 Codex 不能重置额度\" in e[\"message\"]"'
out=$(run cca reset alice 2>&1); rc=$?; out2=$(run cca codex reset kimi 2>&1); rc2=$?
ok "cca reset：只有 ChatGPT 账号能重置额度；第三方 API 账号没有额度可以重置" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "只有 ChatGPT 账号能重置额度" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "第三方 API 账号，没有额度可以重置"'
ok "cca codex reset：成功、不重置、出错退出，临时目录都清掉了（~/.cca 下没有 usage.*）" '[ -z "$(ls -d "$H"/.cca/usage.* 2>/dev/null)" ]'
ccanned alice@example.com 13 41 pro 2
ok "Codex 的 access / refresh token 从没出现在 cca 写的文件、日志和命令行里（cca 只从 id_token 读邮箱和套餐）" '! grep -rqF -e at-SECRET -e rt-SECRET --exclude auth.json "$H/.cca" && ! grep -qF -e at-SECRET -e rt-SECRET "$T/env.log" codex.log curl.log security.log open.log'
# 网页版、桌面 App 和启动器
: > open.log; rm -f "$T/clipboard"
run CCA_TTY="$(ans)" cca codex web alice >/dev/null 2>&1
ok "cca codex web：用账号专属的浏览器打开 chatgpt.com；网页版不动剪贴板" '[ ! -e "$T/clipboard" ] && grep -F "https://chatgpt.com/" open.log | grep -qF -- "--user-data-dir=$CD/chrome"'
: > open.log
run CCA_TTY="$(ans)" cca codex billing alice >/dev/null 2>&1
ok "cca codex billing：用账号专属的浏览器打开 OpenAI 控制台的账单页，邮箱放进剪贴板" '[ "$(cat "$T/clipboard")" = alice@example.com ] && grep -F "https://platform.openai.com/settings/organization/billing/overview" open.log | grep -qF -- "--user-data-dir=$CD/chrome"'
: > open.log
out=$(run CCA_TTY="$(ans)" cca codex limit-resets alice 2>&1); rc=$?
ok "cca codex limit-resets：ChatGPT 账号的次数在 usage 里，不打开浏览器" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "重置次数和到期日在 cca codex usage 里" && [ ! -s open.log ]'
L="$H/Applications"
ok "启动器：Codex 账号是「ChatGPT 别名」，和 Claude 账号的「Claude 别名」各管各的" '[ -x "$L/ChatGPT alice.app/Contents/MacOS/cca-launcher" ] && grep -q "codex app" "$L/ChatGPT alice.app/Contents/MacOS/cca-launcher" && [ -d "$L/Claude alice.app" ] && plutil -extract CFBundleIdentifier raw "$L/ChatGPT alice.app/Contents/Info.plist" | grep -q "^local.cca.launcher.codex.alice-example-com-"'
run cca app --launchers >/dev/null 2>&1; run cca codex app --launchers >/dev/null 2>&1
ok "启动器：两边各自重新生成时不删对方的" '[ -d "$L/ChatGPT alice.app" ] && [ -d "$L/Claude alice.app" ]'
out=$(run cca codex app alice 2>&1 </dev/null); rc=$?
ok "ChatGPT App：共享的 config.toml 里还没关 system_proxy_fallback、又不能问时：不打开，说明要在终端里运行一次" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "system_proxy_fallback"'
out=$(runev cca codex app alice 2>&1); rc=$?
ok "图形界面：第一次打开 ChatGPT App 要关掉 Codex 的系统代理回退：记一条 consent（codex_fallback_off），不改 config.toml、不打开" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"codex_fallback_off\" and e[\"grant\"] == \"codex_fallback_off\"" && ! grep -q "system_proxy_fallback = false" "$X/config.toml" 2>/dev/null && ! grep -q "FEATURES disable" codex.log'
: > open.log
out=$(run CCA_TTY="$(ans y)" cca codex app alice 2>&1); rc=$?
ok "ChatGPT App：问过之后用 codex 自己的命令关掉 system_proxy_fallback（写进共享的 ~/.codex/config.toml）" '[ $rc -eq 0 ] && grep -q "FEATURES disable system_proxy_fallback @ $X" codex.log && grep -q "system_proxy_fallback = false" "$X/config.toml" && [ ! -L "$X/config.toml" ]'
ok "ChatGPT App：账号自己的 CODEX_HOME、资料目录（CODEX_ELECTRON_USER_DATA_PATH + --user-data-dir）和端口" 'grep -qF -- "[-n] [-a] [$T/ChatGPT.app] [--env] [CODEX_HOME=$CH] [--env] [CODEX_SQLITE_HOME=$X] [--env] [CODEX_ELECTRON_USER_DATA_PATH=$CD/desktop]" open.log && grep -qF -- "[--args] [--user-data-dir=$CD/desktop] [--proxy-server=http://192.0.2.10:7902]" open.log && printf "%s" "$out" | grep -q "已打开 alice@example.com 的 ChatGPT App，出口 US-B（端口 7902）"'
ok "ChatGPT App 第一次打开：说明 App 里要求登录时别在 App 里登，改用 cca codex login" 'printf "%s" "$out" | grep -q "运行 cca codex login alice@example.com，再打开 App"'
sed -i '' '/system_proxy_fallback = false/d' "$X/config.toml"; nf=$(grep -c "FEATURES disable system_proxy_fallback" codex.log)
out=$(runev CCA_GRANT=codex_fallback_off cca codex app alice 2>&1); rc=$?
ok "图形界面：同意之后带着授权重跑：用 codex 自己的命令关掉它，照常打开 ChatGPT App" '[ $rc -eq 0 ] && [ "$(grep -c "FEATURES disable system_proxy_fallback" codex.log)" = $((nf + 1)) ] && grep -q "system_proxy_fallback = false" "$X/config.toml" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"opened\""'
cp "$T/fakerun/Contents/MacOS/Claude" "$T/fakerun/Contents/MacOS/ChatGPT"   # 和上面假的 Claude App 进程一样，换个名字
"$T/fakerun/Contents/MacOS/ChatGPT" --user-data-dir="$CD/desktop" --x & FAKEPID=$!
sleep 0.5; n_open=$(grep -c "user-data-dir=$CD/desktop" open.log)
out=$(run CCA_TTY="$(ans)" cca codex app alice 2>&1); rc=$?
out2=$(run CCA_TTY="$(ans y)" cca codex rm alice 2>&1); rc2=$?
alive=0; kill -0 "$FAKEPID" 2>/dev/null && alive=1
kill $FAKEPID 2>/dev/null; wait $FAKEPID 2>/dev/null
ok "ChatGPT App：已有实例缺少正确代理或启动记录时拒绝激活，不杀进程；开着时不能删账号" '[ $rc -ne 0 ] && [ "$alive" = 1 ] && printf "%s" "$out" | grep -q "启动记录" && [ "$(grep -c "user-data-dir=$CD/desktop" open.log)" = "$n_open" ] && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "ChatGPT App 还开着"'
# 删除：退出登录、删账号目录（同一个邮箱的 Claude 账号不受影响），端口回收；会话还在
rm -f "$H/.local/bin/cxa"; printf '#!/bin/sh\necho mine\n' > "$H/.local/bin/cxa"; chmod 755 "$H/.local/bin/cxa"   # 别人的 cxa
out=$(run CCA_TTY="$(ans dave@example.com '' 3)" cca codex add 2>&1)
ok "选节点只看同一种工具的账号：US-B 注明已被 Codex 的 alice 使用，US-A 上 Claude 的 alice 不提；默认选中第一个没有 Codex 账号用的 US-A" 'printf "%s" "$out" | grep -qE "2\. US-B +已被 alice@example.com 使用$" && printf "%s" "$out" | grep -qE "1\. US-A$" && ! printf "%s" "$out" | grep -q "（Claude）" && printf "%s" "$out" | grep -q "选择节点编号 \[1\]"'
ok "~/.local/bin/cxa 已经是别人的命令：不覆盖，说明一声" '[ ! -L "$H/.local/bin/cxa" ] && grep -q "echo mine" "$H/.local/bin/cxa" && printf "%s" "$out" | grep -q "已经存在，不是 cca 建的：没有建 cxa 这个简写"'
out=$(run cca ls 2>&1)
ok "别人的 cxa 还在：平时运行 cca 不覆盖，也不每次都提示（添加 Codex 账号、cca install 时才说）" '[ ! -L "$H/.local/bin/cxa" ] && grep -q "echo mine" "$H/.local/bin/cxa" && ! printf "%s" "$out" | grep -q "不是 cca 建的"'
rm -f "$H/.local/bin/cxa"; ln -s cca "$H/.local/bin/cxa"
ok "（准备）再加一个 Codex 账号，分到 7903" '[ "$(cfgv "c[\"codex\"][\"ports\"].get(\"dave@example.com\")")" = 7903 ]'
out=$(run CCA_TTY="$(ans y)" cca codex rm alice 2>&1); rc=$?
ok "cca codex rm：提醒 git 工作树会一起删；退出登录、删掉它的目录和启动器；同一个邮箱的 Claude 账号、启动器都不动；7902 回到 REJECT" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "git 工作树" && grep -q "LOGOUT $CH" codex.log && [ ! -e "$CD" ] && [ ! -e "$L/ChatGPT alice.app" ] && [ -d "$L/Claude alice.app" ] && [ -d "$(ls -d "$H"/.cca/accounts/alice-example-com-*)" ] && [ "$(sel CCA-2)" = REJECT ] && [ "$(cfgv "c[\"ports\"]")" = "{'"'"'alice@example.com'"'"': 7901}" ]'
ok "删掉账号后它开过的会话在 ~/.codex 里照样找得到（会话库里的路径早已改写）" '[ "$(sqlite3 "$X/state_5.sqlite" "select count(*) from threads where rollout_path like '"'"'$H/.cca/%'"'"'")" = 0 ] && for p in $(sqlite3 "$X/state_5.sqlite" "select rollout_path from threads"); do [ -f "$p" ] || exit 1; done'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "删了 Codex 账号之后 Claude 的同名账号照常启动" '[ $rc -eq 3 ]'
run CCA_TTY="$(ans)" cca codex @dave --version >/dev/null 2>&1
DD=$(ls -d "$H"/.cca/codex/accounts/dave-example-com-*)
mkdir -p "$H/.cca/codex/accounts/orphan-000000/codex/worktrees/w"; printf '{"tokens": {"access_token": "at-SECRET"}}' > "$H/.cca/codex/accounts/orphan-000000/codex/auth.json"
ok "（准备一键还原）dave 登录着、建过 git 工作树；另有一个从账号表里直接删掉、目录还留着登录凭证的账号" '[ -f "$DD/codex/auth.json" ] && [ -d "$DD/codex/worktrees/dave" ]'

fi
if sect 10f2; then
# 10f2. Antigravity（agy）账号：单独的账号表 agy.json，和 Claude、Codex 共用端口池；每个账号一个数据目录（--gemini_dir），
# 设了 SSH_CONNECTION 让 agy 把登录 token 存进数据目录里的文件（不进钥匙串；cca 先在临时目录里探测这个版本确实这样，按程序缓存）；
# ~/.gemini 里白名单上的条目共享；登录页在账号专属的 Chrome 里打开，授权码照常从 fd 3 问、交给 agy；没有查用量的接口、第三方 API
# 账号和桌面 App。这一节最后删掉 agy 的账号和 agy.json（后面的用例照没用过 agy 写）
G="$H/.gemini"; AGJ="$H/.cca/agy.json"
alast() { lastline "$1" agy.log; }
alk() { [ -L "$AG/$1" ] && [ "$(readlink "$AG/$1")" = "$G/$1" ]; }
mkdir -p "$G/antigravity-cli/conversations" "$G/config/skills/mine"
printf '{"theme": "dark"}' > "$G/antigravity-cli/settings.json"; echo '# 共享的规则' > "$G/GEMINI.md"
printf '00000000-0000-4000-8000-000000000000' > "$G/antigravity-cli/installation_id"
printf '{"id_token_email": "master@example.com"}' > "$G/antigravity-cli/antigravity-oauth-token"
rm -f "$H/.local/bin/aga"; out0=$(run cca ls 2>&1)   # 模拟以前装的 cca：那时还没有 aga
[ -e "$H/.local/bin/aga" ] || [ -L "$H/.local/bin/aga" ] && ag0=yes || ag0=no
: > agy.log; : > open.log; : > curl.log; n0=$(reloads)
out=$(run CCA_TTY="$(ans alice@example.com '' 1)" cca agy add 2>&1); rc=$?
AGD=$(ls -d "$H"/.cca/agy/accounts/alice-example-com-* 2>/dev/null); AG="$AGD/gemini"
agy_iid=$(cat "$AG/antigravity-cli/installation_id" 2>/dev/null); agy_uuid=$(sed -n 's/^installation_uuid:[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$AG/antigravity-cli/jetski_state.pbtxt" 2>/dev/null)
ok "agy add：和 Claude 账号同一个邮箱也能加，记在 agy.json；账号目录在 ~/.cca/agy/accounts 下；说明用 Google 账号登录" '[ $rc -eq 0 ] && grep -q "\"alice@example.com\": \"US-A\"" "$AGJ" && [ -d "$AG" ] && printf "%s" "$out" | grep -q "添加 Antigravity 账号" && printf "%s" "$out" | grep -q "登录 Google（用 alice@example.com 登录"'
ok "agy add：和 Claude、Codex 共用端口池（分到空着的 7902），路由器上的名字带 agy-，不重载" '[ "$(cfgv "c[\"agy\"][\"ports\"]")" = "{'"'"'alice@example.com'"'"': 7902}" ] && grep -q "\"slot\": 2, \"name\": \"agy-alice-example-com-" "$R/etc/cca/accounts.yaml" && [ "$(sel CCA-2)" = US-A ] && [ "$(reloads)" = "$n0" ]'
ok "agy add：登录前就固定好两个装机标识（installation_id 正好 36 个字符、不带换行；jetski_state.pbtxt 的 installation_uuid 是另一个），和 ~/.gemini 的不一样" '[ ${#agy_iid} = 36 ] && printf "%s\n" "$agy_iid" "$agy_uuid" | grep -Eqx "[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}" && [ "$agy_iid" != "$agy_uuid" ] && [ "$agy_iid" != "$(cat "$G/antigravity-cli/installation_id")" ] && [ -f "$AGD/device.json" ]'
ok "以前装的 cca 升级后还没有 aga：没用过 agy 时不补；第一次用 cca agy 时补上，并说明怎么用" '[ $ag0 = no ] && ! printf "%s" "$out0" | grep -q "aga" && [ "$(readlink "$H/.local/bin/aga")" = cca ] && [ "$(printf "%s" "$out" | grep -c "以后可以直接用 aga 代替 cca agy：aga @别名 直接启动")" = 1 ]'
out=$(run cca agy ls 2>&1)
ok "cca agy ls：列出 Antigravity 账号（还没登录）" 'printf "%s" "$out" | grep -qE "1\. alice@example.com +未登录 +US-A"'
out3=$(run aga ls 2>&1)
ok "aga ls 就是 cca agy ls" '[ -n "$out3" ] && [ "$out3" = "$out" ]'
rm -f "$H/.local/bin/aga"; out=$(run cca ls 2>&1); out2=$(run cca ls 2>&1)
ok "用着 agy 的机器上 aga 不见了：下一次运行 cca（哪个子命令都行）就补上，只说一次" '[ "$(readlink "$H/.local/bin/aga")" = cca ] && printf "%s" "$out" | grep -q "以后可以直接用 aga 代替 cca agy" && ! printf "%s" "$out2" | grep -q "以后可以直接用 aga"'
: > agy.log; : > open.log; : > curl.log; rm -f "$H/.cca/agy/probe.json"
out=$(run CCA_TTY="$(ans good-ag1)" GEMINI_API_KEY=evil-1 GOOGLE_CLOUD_PROJECT=evil-proj ANTIGRAVITY_GEMINI_DIR=/evil AGY_ACCOUNT=evil@example.com AGY_CLI_THEME=light AGY_CA_CERT="$T/ca.pem" ALL_PROXY=socks5://evil.invalid cca agy @alice --version 2>&1); rc=$?
penv=$(grep -A1 "cca-agy-probe" agy.log | grep "^ENV: " | head -n 1)
ok "agy 第一次用：先在临时的数据目录里跑一次 acp --login，确认这个版本设了 SSH_CONNECTION 时把登录存进文件；不联网（代理指向关着的端口），结果记下" 'grep -q "^ARGS: --gemini_dir=.*cca-agy-probe\..*/gemini acp --login$" agy.log && printf "%s" "$penv" | grep -q "SSH_CONNECTION=127.0.0.1 22 127.0.0.1 22 https_proxy=unset HTTPS_PROXY=http://127.0.0.1:9 " && printf "%s" "$penv" | grep -q "GEMINI_API_KEY=unset" && cfgok "json.load(open(\"$H/.cca/agy/probe.json\"))[\"files\"] is True"'
ok "agy 登录：在账号自己的数据目录里登录；Google 的登录网址补上 login_hint，在账号专属的 Chrome 里、经账号的端口打开" 'grep -q "^ARGS: --gemini_dir=$AG acp --login$" agy.log && grep -F "accounts.google.com/o/oauth2/auth" open.log | grep -qF -- "--user-data-dir=$AGD/chrome" && grep -F "accounts.google.com/o/oauth2/auth" open.log | grep -qF -- "--proxy-server=http://192.0.2.10:7902" && grep -F "accounts.google.com/o/oauth2/auth" open.log | grep -qF "login_hint=alice%40example.com"'
ok "agy 登录：授权码从终端贴回、交给 agy，换 token 经账号的端口；token 只在账号自己的数据目录里，~/.gemini 原来的登录不动" 'grep -q "AGY-EXCHANGE code=good-ag1 https_proxy=http://192.0.2.10:7902" agy.log && [ -f "$AG/antigravity-cli/antigravity-oauth-token" ] && [ ! -L "$AG/antigravity-cli/antigravity-oauth-token" ] && grep -q master "$G/antigravity-cli/antigravity-oauth-token" && printf "%s" "$out" | grep -q "授权码（登录后页面上显示的那一段" && ! printf "%s" "$out" | grep -q "Open this link"'
ok "agy 启动：--gemini_dir 指向账号自己的数据目录（写在最前面），设了 SSH_CONNECTION，走账号的端口" '[ $rc -eq 3 ] && alast ARGS | grep -qx "ARGS: --gemini_dir=$AG --version" && alast ENV | grep -q "SSH_CONNECTION=127.0.0.1 22 127.0.0.1 22 https_proxy=http://192.0.2.10:7902 HTTPS_PROXY=http://192.0.2.10:7902 "'
ok "agy 启动：继承来的 GEMINI_*、GOOGLE_*、ANTIGRAVITY_*、AGY_ACCOUNT（会换账号、数据目录）和 ALL_PROXY 不带进去并提示；界面开关 AGY_CLI_*、公司根证书 AGY_CA_CERT 留着；fd 3 关着" 'alast ENV | grep -q "GEMINI_API_KEY=unset GOOGLE_API_KEY=unset GOOGLE_CLOUD_PROJECT=unset ANTIGRAVITY_GEMINI_DIR=unset AGY_ACCOUNT=unset AGY_CA_CERT=$T/ca.pem AGY_CLI_THEME=light ALL_PROXY=unset " && alast ENV | grep -q "fd3=closed" && printf "%s" "$out" | grep -q "已为本次启动屏蔽环境里继承的"'
ok "agy 启动前的出口检测：Google 没有 /cdn-cgi/trace，经 7902 看 www.cloudflare.com 看到的 IP" 'grep -q "CURL via :7902 https://www.cloudflare.com/cdn-cgi/trace" curl.log && grep -q "CURL via :7902 https://ip.net.coffee/api/iprisk/203.0.113.2" curl.log'
ok "agy 启动：用的是 cca 固定的两个装机标识（agy 照用，没有换掉）" 'grep -q "AGY-DEVICE $agy_iid $agy_uuid @ $AG" agy.log && [ "$(cat "$AG/antigravity-cli/installation_id")" = "$agy_iid" ]'
ok "共享：会话、记忆（brain）、历史、设置、GEMINI.md、技能，--continue 用的 last_conversations.json 都链接到 ~/.gemini；antigravity-cli、config 本身是账号自己的目录" 'alk GEMINI.md && alk antigravity-cli/conversations && alk antigravity-cli/brain && alk antigravity-cli/history.jsonl && alk antigravity-cli/settings.json && alk antigravity-cli/cache/last_conversations.json && alk config/skills && [ -d "$AG/antigravity-cli" ] && [ ! -L "$AG/antigravity-cli" ] && [ ! -L "$AG/config" ] && [ ! -L "$AG/antigravity-cli/cache" ]'
ok "不共享：登录 token、装机标识、日志各账号一份" '[ ! -L "$AG/antigravity-cli/installation_id" ] && [ ! -L "$AG/antigravity-cli/jetski_state.pbtxt" ] && [ -d "$AG/antigravity-cli/log" ] && [ ! -L "$AG/antigravity-cli/log" ] && [ ! -e "$G/antigravity-cli/log" ]'
ok "会话里写的共享内容回到 ~/.gemini：会话、历史、技能、--continue 的记录；agy 用临时文件改名保存的 settings.json 并回去再链接上" '[ -f "$G/antigravity-cli/conversations/from-alice.pb" ] && grep -q "hi from alice" "$G/antigravity-cli/history.jsonl" && [ -d "$G/config/skills/from-alice" ] && grep -q "conv-alice" "$G/antigravity-cli/cache/last_conversations.json" && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if j.get(\"lastWriter\") == \"alice\" and j.get(\"theme\") == \"dark\" else 1)" "$G/antigravity-cli/settings.json"'
ok "agy 第一次运行时建的空 config/mcp_config.json：退出时并回 ~/.gemini、链接上，不当成坏的 JSON 报冲突" '[ -f "$G/config/mcp_config.json" ] && [ ! -s "$G/config/mcp_config.json" ] && alk config/mcp_config.json && ! printf "%s" "$out" | grep -q "有冲突或无法合并" && [ ! -e "$H/.cca/agy/conflicts" ]'
m=$(run CCA_TTY="$(ans q)" cca agy 2>&1)
ok "cca agy 菜单：标题是 Antigravity，账号已登录；没有添加第三方 API 账号这一项" 'printf "%s" "$m" | grep -q "选择 Antigravity 账号" && printf "%s" "$m" | grep -qE "1\. alice@example.com +已登录 +US-A" && printf "%s" "$m" | grep -q "a      添加新账号" && ! printf "%s" "$m" | grep -q "第三方 API"'
n_args=$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)
run CCA_TTY="$(ans)" aga @alice --version >/dev/null 2>&1; rc=$?
ok "aga @alice：启动的是 alice 的 agy（等于 cca agy @alice）" '[ $rc -eq 3 ] && [ "$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)" = $((n_args + 1)) ] && alast ARGS | grep -qx "ARGS: --gemini_dir=$AG --version"'
out=$(run cca agy usage 2>&1); rc=$?; out2=$(run CCA_TTY="$(ans)" cca agy add api 2>&1); rc2=$?; out3=$(run cca agy app alice 2>&1 </dev/null); rc3=$?; out4=$(run cca agy billing alice 2>&1 </dev/null); rc4=$?
ok "agy 没有查用量的接口、第三方 API 账号、桌面 App 和控制台账单页：说清楚，什么都不动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "在 agy 里用 /usage 看" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "没有第三方 API 账号" && [ $rc3 -ne 0 ] && printf "%s" "$out3" | grep -q "没有桌面 App" && [ $rc4 -ne 0 ] && printf "%s" "$out4" | grep -q "没有控制台的账单页"'
edit_table 'j["kimi"] = {"base_url": "https://api.example.com/v1"}' "$AGJ"
n_args=$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)
out=$(run cca agy @alice --version 2>&1 </dev/null); rc=$?
edit_table 'j.pop("kimi")' "$AGJ"
ok "agy.json 里写成第三方 API 账号：校验不过，说只能写节点名，不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "「kimi」：Antigravity 没有第三方 API 账号：值只能写节点名" && [ "$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)" = "$n_args" ]'
n_args=$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)
cp "$G/antigravity-cli/settings.json" "$T/agy-settings.bak"; printf '{"theme": "dark", "gcp": {"project": "evil-proj"}}' > "$G/antigravity-cli/settings.json"
out=$(run CCA_TTY="$(ans)" cca agy @alice --version 2>&1); rc=$?
cp "$T/agy-settings.bak" "$G/antigravity-cli/settings.json"
out2=$(run CCA_TTY="$(ans)" cca agy @alice --gemini_dir=/tmp/elsewhere --version 2>&1); rc2=$?
ok "共享的设置里给了 Google Cloud 项目（各账号会用同一个项目）、启动参数里另给 --gemini_dir（换掉账号的数据目录和登录）：拒绝启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "共享的 ~/.gemini 里有" && printf "%s" "$out" | grep -q "settings.json:gcp" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q -- "--gemini_dir" && [ "$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)" = "$n_args" ]'
n_p=$(grep -c "cca-agy-probe" agy.log); run CCA_TTY="$(ans)" cca agy @alice --version >/dev/null 2>&1; rc=$?
ok "（探测结果按 agy 程序缓存，不每次都跑）" '[ $rc -eq 3 ] && [ "$(grep -c "cca-agy-probe" agy.log)" = "$n_p" ]'
rm -f "$H/.cca/agy/probe.json"; touch "$T/agy_keychain"; n_args=$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)
out=$(run CCA_TTY="$(ans)" cca agy @alice --version 2>&1); rc=$?
rm -f "$T/agy_keychain" "$H/.cca/agy/probe.json"
ok "换了新版的 agy、设了 SSH_CONNECTION 也不把登录存进文件（各账号会共用钥匙串里的同一份）：停止，不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不再把登录存进账号自己的目录" && [ "$(grep -c "^ARGS: --gemini_dir=$AG --version" agy.log)" = "$n_args" ]'
echo mallory@example.com > "$T/agy_login_as"
out=$(run CCA_TTY="$(ans good-ag2 n)" cca agy login alice 2>&1); rc=$?
ok "cca agy login：先退出原来的登录（删掉 token）；登成了别的邮箱就不用，问过之后不继续" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "登录的是 mallory@example.com，不是 alice@example.com"'
out=$(run CCA_TTY="$(ans 'https://antigravity.google/oauth-callback?code=good-x&state=st-wrong' bad-ag good-ag3)" cca agy login alice 2>&1); rc=$?
ok "cca agy login：贴回的整个网址 state 对不上时不交给 agy、重问；授权码换不到 token 时照 agy 的原话说、再问；用对的邮箱重新登录，装机标识不变" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "state 对不上" && printf "%s" "$out" | grep -q "invalid_grant" && grep -q "AGY-EXCHANGE code=bad-ag " agy.log && ! grep -q "code=good-x" agy.log && printf "%s" "$out" | grep -q "alice@example.com 已重新登录" && grep -q "AGY-LOGIN alice@example.com @ $AG" agy.log && [ "$(cat "$AG/antigravity-cli/installation_id")" = "$agy_iid" ]'
rm -f "$AG/antigravity-cli/antigravity-oauth-token"
out=$(run CCA_TTY="$(ans '')" cca agy @alice --version 2>&1); rc=$?
ok "agy 登录：直接回车取消，不启动，agy 不留在后台" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "已取消登录" && gone "$T/fake_agy.py"'
out=$(runev cca agy @alice --version 2>&1); rc=$?
ok "图形界面里启动还没登录的 agy 账号：要在终端里登录，交接的命令带 agy" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"login\" and e[\"argv\"][:2] == [\"agy\", \"login\"] and e[\"display\"].startswith(\"cca agy login \")"'
touch "$T/agy_noemail"
out=$(run CCA_TTY="$(ans good-ag4)" cca agy @alice --version 2>&1); rc=$?
rm -f "$T/agy_noemail"
ok "token 里没有邮箱时（agy 换了格式）：按 agy 登录时说的 Logged in as 认" '[ $rc -eq 3 ] && python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"email\"] == \"alice@example.com\" else 1)" "$AGD/agy-login.json"'
: > open.log
out=$(run CCA_REMOTE_LOGIN=1 CCA_TTY="$(ans good-ag5)" cca agy login alice 2>&1); rc=$?
ok "agy 服务器上登录：给出 cca agy login-page 那一行（带节点和节点摘要），这里不打开浏览器；授权码贴回这里交给 agy，换 token 经账号的端口" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "  cca agy login-page alice@example.com --node US-A --node-id $FPA '\''https://accounts.google.com/o/oauth2/auth?" && grep -q "AGY-EXCHANGE code=good-ag5 https_proxy=http://192.0.2.10:7902" agy.log && [ ! -s open.log ]'
LPA="https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x.apps.googleusercontent.com&code_challenge=CC&code_challenge_method=S256&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback&response_type=code&scope=openid&state=st-agpage"
: > open.log
out=$(run cca agy login-page alice@example.com --node US-A "$LPA" 2>&1); rc=$?
out2=$(run cca agy login-page alice@example.com --node US-A "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_x&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&code_challenge=CC&code_challenge_method=S256&state=st-x" 2>&1); rc2=$?
ok "cca agy login-page（在 Mac 上）：在 agy 账号专属的 Chrome 里经它的端口打开 Google 的登录页（补上 login_hint），说把页面上的授权码抄回服务器；别的工具的登录网址不开" '[ $rc -eq 0 ] && grep -F "state=st-agpage" open.log | grep -qF -- "--user-data-dir=$AGD/chrome" && grep -F "state=st-agpage" open.log | grep -qF -- "--proxy-server=http://192.0.2.10:7902" && grep -F "state=st-agpage" open.log | grep -qF "login_hint=alice%40example.com" && printf "%s" "$out" | grep -q "回到服务器的终端粘贴" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "这不是 Antigravity 的登录网址" && [ "$(grep -c "auth.openai.com" open.log)" = 0 ]'
: > open.log
run CCA_TTY="$(ans)" cca agy web alice >/dev/null 2>&1
out=$(run cca agy check alice 2>&1)
ok "cca agy web 用账号专属的浏览器打开 gemini.google.com；cca agy check 经账号的端口检测出口" 'grep -F "https://gemini.google.com/" open.log | grep -qF -- "--user-data-dir=$AGD/chrome" && printf "%s" "$out" | grep -qE "alice@example.com +US-A +:7902"'
GF0="$T/guifx"; mkdir -p "$GF0"; out=$(gj state); printf '%s' "$out" > "$GF0/state_agy.json"; out2=$(gj hello); printf '%s' "$out2" > "$GF0/hello_agy.json"
ok "_gui state：第三种工具 agy（Antigravity，命令前缀 agy），features 说明没有桌面 App、第三方 API 和查用量；alice 登录着、档位按 token 里的 user_tier，槽位 2；不能的事 can_why 是 unsupported" 'printf "%s" "$out" | jok "(lambda t: t[\"id\"] == \"agy\" and t[\"label\"] == \"Antigravity\" and t[\"cli_prefix\"] == [\"agy\"] and t[\"features\"] == {\"app\": False, \"api\": False, \"usage\": False, \"cost\": False} and t[\"enabled\"] is True and t[\"bin\"] and (lambda a: a[\"login\"] == \"logged_in\" and a[\"plan\"] == \"Google AI Pro\" and a[\"plan_tier\"] == \"standard\" and a[\"slot\"] == 2 and a[\"can\"][\"app\"] is False and a[\"can_why\"][\"app\"] == \"unsupported\" and a[\"can\"][\"usage\"] is False and a[\"can_why\"][\"usage\"] == \"unsupported\" and a[\"can\"][\"billing\"] is False and a[\"can\"][\"limit_resets\"] is False and a[\"can\"][\"cli\"] is True and a[\"can\"][\"web\"] is True and a[\"usage_weeks\"] is None)(t[\"accounts\"][0]))(d[\"tools\"][2]) and d[\"tools\"][0][\"features\"] == {\"app\": True, \"api\": True, \"usage\": True, \"cost\": True} and d[\"routing\"][\"slots\"][1][\"tool\"] == \"agy\""'
ok "_gui hello：agy 已启用，找到了 agy 命令" 'printf "%s" "$out2" | jok "d[\"tools\"][\"agy\"][\"enabled\"] is True and d[\"tools\"][\"agy\"][\"bin\"]"'
ok "agy 的登录 token 从没出现在 cca 写的文件、日志和命令行里（cca 只读里面的邮箱和档位）" '! grep -rqF -e ya29.test -e 1//test --exclude antigravity-oauth-token "$H/.cca" && ! grep -qF -e ya29.test -e 1//test agy.log open.log curl.log "$T/env.log"'
out=$(run cca help 2>&1)
ok "cca help 有 Antigravity 一节" 'printf "%s" "$out" | grep -q "cca agy add" && printf "%s" "$out" | grep -q "Antigravity（Google 的 agy 命令行"'
out=$(run CCA_TTY="$(ans y)" cca agy rm alice 2>&1); rc=$?
ok "cca agy rm：退出登录、删掉账号目录；~/.gemini 里共享的会话、历史、设置和原来的登录都还在；7902 回到 REJECT；同一个邮箱的 Claude 账号不动" '[ $rc -eq 0 ] && [ ! -e "$AGD" ] && [ -f "$G/antigravity-cli/conversations/from-alice.pb" ] && grep -q "hi from alice" "$G/antigravity-cli/history.jsonl" && grep -q master "$G/antigravity-cli/antigravity-oauth-token" && [ "$(sel CCA-2)" = REJECT ] && [ -d "$(ls -d "$H"/.cca/accounts/alice-example-com-*)" ] && [ "$(cfgv "c[\"ports\"]")" = "{'"'"'alice@example.com'"'"': 7901}" ]'
rm -rf "$AGJ" "$H/.cca/agy"; python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop("agy", None); json.dump(c, open(p, "w"), indent=2, ensure_ascii=False)' "$H/.cca/config.json"
run cca ls >/dev/null 2>&1
ok "（收尾）删掉 agy.json：后面的用例照没用过 agy 写" '[ ! -e "$AGJ" ] && [ "$(cfgv "\"agy\" in c")" = False ]'

fi
if sect 10g; then
# 10g. 图形界面（cca App）的接口（macos/PROTOCOL.md）：只读的 cca _gui hello / state / live（JSON；不写文件、不经过自动更新），
#      事件文件和带范围的同意（CCA_EVENTS / CCA_GRANT），出口检测结果（health.json、check --json），usage --json 的新字段。
#      输出存进 $GF，这一节最后交给 App 的契约测试解码（macos/test.sh；没有 swiftc 时跳过并说明）
GF="$T/guifx"; mkdir -p "$GF"
VER=$(sed -n 's/^VERSION="\(.*\)"$/\1/p' "$CCA")
snap() { ( cd "$H/.cca" && find . -type f ! -path '*/chrome/*' ! -path '*/desktop/*' ! -name 'cm-*' ! -name '*.lock' | LC_ALL=C sort | while IFS= read -r f; do printf '%s ' "$f"; shasum < "$f"; done ) | shasum; }   # ~/.cca 里文件的摘要
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-*)
s0=$(snap)
out=$(gj hello); printf '%s' "$out" > "$GF/hello.json"
ok "_gui hello：接口版本 1、cca 的版本、开发模式、装好了、Codex 已启用、找到了 claude 和 codex、还没装 App" 'printf "%s" "$out" | jok "d[\"kind\"] == \"hello\" and d[\"api\"] == 1 and d[\"cca_version\"] == \"$VER\" and d[\"dev_mode\"] is True and d[\"installed\"] is True and d[\"tools\"][\"codex\"][\"enabled\"] is True and d[\"tools\"][\"claude\"][\"bin\"] and d[\"tools\"][\"codex\"][\"bin\"] and d[\"gui\"][\"app_version\"] is None"'
out=$(gj state); rc=$?; printf '%s' "$out" > "$GF/state.json"
ok "_gui state：Claude、Codex 的账号和账号表一样（顺序也一样），端口和 config.json 一样，账号目录名是 cca 算的" '[ $rc -eq 0 ] && jok "[a[\"key\"] for a in d[\"tools\"][0][\"accounts\"]] == list(tab(\"accounts.json\")) and [a[\"key\"] for a in d[\"tools\"][1][\"accounts\"]] == list(tab(\"codex.json\")) and all(a[\"port\"] == cfg[\"ports\"].get(a[\"key\"]) for a in d[\"tools\"][0][\"accounts\"]) and all(a[\"port\"] == cfg[\"codex\"][\"ports\"].get(a[\"key\"]) for a in d[\"tools\"][1][\"accounts\"]) and acct(0, \"alice@example.com\")[\"id\"] == \"${AD##*/}\"" < "$GF/state.json"'
ok "_gui state：alice 登录着、套餐 Max、有上次的用量（缓存），槽位 1；Codex 的 dave 在槽位 3；路由器已同步" 'jok "(lambda a, x: a[\"login\"] == \"logged_in\" and a[\"plan\"] == \"Max\" and a[\"slot\"] == 1 and a[\"group\"] == \"CCA-1\" and a[\"usage\"][\"source\"] == \"cache\" and a[\"usage\"][\"five_hour\"][\"utilization\"] >= 0 and isinstance(a[\"usage\"][\"five_hour\"][\"resets_epoch\"], float) and a[\"badge\"][\"label\"] == \"A\" and a[\"app\"] == {\"running\": False, \"pid\": None} and x[\"login\"] == \"logged_in\" and x[\"slot\"] == 3)(acct(0, \"alice@example.com\"), acct(1, \"dave@example.com\")) and d[\"routing\"][\"sync\"] == \"ok\" and d[\"routing\"][\"slots\"][2][\"tool\"] == \"codex\" and d[\"routing\"][\"pool_size\"] % 8 == 0" < "$GF/state.json"'
ok "_gui state：共享默认关着（config.policy 的 state 是 off，账号表里没有共享的账号，没有收起来的）" 'jok "d[\"config\"][\"policy\"] == {\"configured\": False, \"repo\": None, \"next_check_epoch\": None, \"state\": \"off\", \"accounts\": 0, \"parked\": 0}" < "$GF/state.json"'
ok "_gui hello 说有自己的同步服务器（hub 是 1）；_gui state：共享关着、没加入时 config.hubs 是空的，账号都不经服务器同步（hub 是 null，没有「设备」）" 'jok "d[\"hub\"] == 1" < "$GF/hello.json" && jok "d[\"config\"][\"hubs\"] == [] and all(a[\"hub\"] is None and not a[\"can\"][\"devices\"] for t in d[\"tools\"] for a in t[\"accounts\"])" < "$GF/state.json"'
ok "_gui state：不联网、不写文件（~/.cca 里一个文件都没变），也不经过自动更新" '[ "$(snap)" = "$s0" ] && ! grep -q "GH " gh.log 2>/dev/null'
# 辅助程序（cca-core）在哪：仓库里的 cca 用 cli/target/release 里编出来的；发布的 cca 把它接在文件后面（cli/pack.py），第一次用时解到
# 这个用户的临时目录（TMPDIR 在 /var/folders 下时 cca 把缓存放在它里面）的 cca-helper 里，文件名是它的 SHA-256。
# 缓存目录单独建一个（HC，退出时删），不碰真的。CCA_HELPER（TIMING、release.sh 跑回归时带着）只对仓库里的 cca 管用，发布的不认
HC=$(mktemp -d "$(getconf DARWIN_USER_TEMP_DIR)cca-test-helper.XXXXXX"); HC=$(cd "$HC" && pwd -P); HCL=${HC#/private}
hrun() { run CCA_HELPER= TMPDIR="$HCL/" "$@"; }   # hrun 命令…：缓存目录是 $HC，不指定辅助程序
HSUM=$(shasum -a 256 "$HELPER_BIN" | cut -d' ' -f1)
mkdir -p "$T/hc/dev"; cp "$CCA" "$T/hc/dev/cca"; : > "$T/hc/dev/.git"
out=$(hrun "$T/hc/dev/cca" version 2>&1); rc=$?
ok "辅助程序：仓库里的 cca（没接辅助程序）还没编出来时说清楚怎么编，不往下走" '[ $rc -eq 1 ] && [ "$out" = "cca: 这一份 cca 是仓库里的源码，没带辅助程序：先编出来（cd $T/hc/dev/cli && cargo build --release）" ]'
out=$(run CCA_HELPER="$HELPER_BIN" "$T/hc/dev/cca" version 2>&1); rc=$?
ok "辅助程序：仓库里的 cca 认 CCA_HELPER（指定用哪一份，测试用）" '[ $rc -eq 0 ] && [ "$out" = "$VER" ]'
python3 -I "$D/../cli/pack.py" "$CCA" "$T/hc/cca" --only "$HOST_TARGET=$HELPER_BIN" >/dev/null
ok "辅助程序：接上以后（cli/pack.py）还是一个语法正确的 bash 文件，校验值写在 HELPER_SUMS 里" 'bash -n "$T/hc/cca" && grep -qx "HELPER_SUMS=\"$HOST_TARGET=$HSUM\"" "$T/hc/cca" && [ "$(tail -n 1 "$T/hc/cca")" = CCA-HELPER-END ]'
out=$(hrun "$T/hc/cca" version 2>&1); rc=$?
ok "辅助程序：发布的 cca 第一次运行时解到缓存目录的 cca-helper 里（文件名是 SHA-256，校验过，只有自己能读写执行）" '[ $rc -eq 0 ] && [ "$out" = "$VER" ] && [ "$(ls "$HC/cca-helper")" = "$HSUM" ] && [ "$(stat -f %Lp "$HC/cca-helper/$HSUM")" = 700 ] && [ "$(stat -f %Lp "$HC/cca-helper")" = 700 ] && [ "$(shasum -a 256 "$HC/cca-helper/$HSUM" | cut -d" " -f1)" = "$HSUM" ]'
out=$(run CCA_HELPER=/nonexistent/cca-core TMPDIR="$HCL/" "$T/hc/cca" _gui hello 2>&1); rc=$?
ok "辅助程序：发布的 cca 不认 CCA_HELPER（环境变量不能让它去运行别的程序），照旧用自己解出来的那一份" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"hello\" and d[\"cca_version\"] == \"$VER\""'
i1=$(stat -f '%i %m' "$HC/cca-helper/$HSUM")
tr=$(hrun bash -x "$T/hc/cca" version 2>&1 >/dev/null)
ok "辅助程序：以后每次直接用解好的那一份，不再解，也不为它起任何进程" '[ "$(stat -f "%i %m" "$HC/cca-helper/$HSUM")" = "$i1" ] && ! printf "%s" "$tr" | grep -qE "^\++ (sed|base64|gunzip|shasum|mkdir|chmod|find|mv) "'
rm -rf "$HC/cca-helper"; mkdir -m 700 "$HC/cca-helper"
for n in aaaa1 aaaa2.4242 bbbb1; do echo x > "$HC/cca-helper/$n"; done
touch -t "$(date -v-32d +%Y%m%d%H%M)" "$HC/cca-helper/aaaa1"; touch -t "$(date -v-90d +%Y%m%d%H%M)" "$HC/cca-helper/aaaa2.4242"; touch -t "$(date -v-28d +%Y%m%d%H%M)" "$HC/cca-helper/bbbb1"
hrun "$T/hc/cca" version >/dev/null 2>&1
ok "辅助程序：解新的一份时顺手删掉 30 天前解出来的（旧版本的、没解完的临时文件），30 天内的留着" '[ "$(ls "$HC/cca-helper" | LC_ALL=C sort | tr "\n" " ")" = "$(printf "%s\n" "$HSUM" bbbb1 | LC_ALL=C sort | tr "\n" " ")" ]'
rm -rf "$HC/cca-helper"
head -c $(( $(wc -c < "$T/hc/cca") - 20000 )) "$T/hc/cca" > "$T/hc/cut"; chmod 755 "$T/hc/cut"
out=$(hrun "$T/hc/cut" version 2>&1); rc=$?
sed "s/^HELPER_SUMS=\"$HOST_TARGET=$HSUM\"$/HELPER_SUMS=\"$HOST_TARGET=${HSUM%????}0000\"/" "$T/hc/cca" > "$T/hc/bad"; chmod 755 "$T/hc/bad"
out2=$(hrun "$T/hc/bad" version 2>&1); rc2=$?
ok "辅助程序：文件不完整（下载到一半）、校验值对不上时不用，说清楚，缓存目录里不留东西" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "解不出辅助程序" && [ $rc2 -eq 1 ] && printf "%s" "$out2" | grep -q "解不出辅助程序" && [ -z "$(ls "$HC/cca-helper")" ]'
rm -rf "$HC/cca-helper"; mkdir "$HC/elsewhere"; ln -s "$HC/elsewhere" "$HC/cca-helper"
out=$(hrun "$T/hc/cca" version 2>&1); rc=$?
ok "辅助程序：缓存目录是个链接时不用它" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "缓存目录 $HCL/cca-helper 用不了" && [ -z "$(ls "$HC/elsewhere")" ]'
rm -f "$HC/cca-helper"
mkdir -p "$T/hc/oddcpu"; printf '#!/bin/sh\n[ "$1" = -m ] && { echo riscv64; exit 0; }\nexec /usr/bin/uname "$@"\n' > "$T/hc/oddcpu/uname"; chmod +x "$T/hc/oddcpu/uname"
out=$(hrun HOSTTYPE=riscv64 PATH="$T/hc/oddcpu:$T/bin:/usr/bin:/bin:/usr/sbin:/sbin" "$T/hc/cca" version 2>&1); rc=$?
out2=$(hrun OSTYPE=linux-gnu "$T/hc/cca" version 2>&1); rc2=$?   # bash 认环境里的 HOSTTYPE、OSTYPE，这里借它模拟；这一份只接了这台 Mac 的
out3=$(hrun HOSTTYPE=armv7l "$T/hc/cca" version 2>&1)   # 32 位的 bash 跑在 64 位的系统上：HOSTTYPE 认不出来时问 uname -m
ok "辅助程序：没有这种机器用的（别的芯片；这一份没接 Linux 的）时说清楚；bash 的 HOSTTYPE 认不出来时照 uname -m" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "没有这种机器用的辅助程序（riscv64 " && [ $rc2 -eq 1 ] && printf "%s" "$out2" | grep -q "没有这种机器用的辅助程序" && [ "$out3" = "$VER" ]'
rm -rf "$HC/cca-helper"
mkdir -p "$T/hc/nogz"; for n in bash sed base64 shasum getconf mkdir chmod mv rm find cat locale grep dirname basename readlink; do ln -sf "$(PATH=/usr/bin:/bin command -v "$n")" "$T/hc/nogz/$n"; done
out=$(hrun PATH="$T/hc/nogz" /bin/bash "$T/hc/cca" version 2>&1); rc=$?
ok "辅助程序：解的时候缺命令（gunzip）说缺什么" '[ $rc -eq 1 ] && printf "%s" "$out" | grep -q "缺少命令 gunzip（解出辅助程序要用）"'
# PATH 里排在前面的是 GNU 的 base64（Homebrew 的 coreutils：不认 -D）、别的 shasum：macOS 上解的时候用系统自带的那两个
mkdir -p "$T/hc/gnu"; printf '#!/bin/sh\necho "base64: invalid option -- D" >&2\nexit 1\n' > "$T/hc/gnu/base64"; printf '#!/bin/sh\nexit 1\n' > "$T/hc/gnu/shasum"; chmod +x "$T/hc/gnu/base64" "$T/hc/gnu/shasum"
out=$(hrun PATH="$T/hc/gnu:$T/bin:/usr/bin:/bin:/usr/sbin:/sbin" "$T/hc/cca" version 2>&1); rc=$?
ok "辅助程序：PATH 里排在前面的 base64 不认 -D（GNU 的）也解得出来：用系统自带的" '[ $rc -eq 0 ] && [ "$out" = "$VER" ] && [ -x "$HC/cca-helper/$HSUM" ]'
# 解好的那一份中途没了：py 自己再解一次。把发布的 cca 里找辅助程序的那一段和 py 取出来单独跑（就像一个开了很久的会话收尾时那样）：
# ① 系统清了临时目录；② 这一份 cca 已经被新版本换掉了（新版本的辅助程序是另一份），上一个版本在 update/cca.prev；③ 哪里都没有了
printf '#!/bin/sh\n[ "$1" = sh-re-escape ] && echo "$2"\n' > "$T/hc/other-helper"; chmod +x "$T/hc/other-helper"
python3 -I "$D/../cli/pack.py" "$CCA" "$T/hc/newer" --only "$HOST_TARGET=$T/hc/other-helper" >/dev/null
mkdir -p "$T/hc/heal/home/.cca/update"; cp "$T/hc/cca" "$T/hc/heal/cca"; cp "$T/hc/cca" "$T/hc/heal/home/.cca/update/cca.prev"
cat > "$T/hc/heal.sh" <<'S'
SELF="$1"; CCA_HOME="$2"; UPDATE_REPO=x; TOOL=claude; TOOL_BIN=""; CACHE_DIR="${TMPDIR%/}"
eval "$(sed -n '/^HELPER_SUMS=/,/^helper_find || exit 1$/p' "$SELF")"
eval "$(grep '^py() ' "$SELF")"
py sh-re-escape one || exit 11
rm -f "$HELPER"; py sh-re-escape two || exit 12
rm -f "$HELPER"; cp "$3" "$SELF.tmp" && mv -f "$SELF.tmp" "$SELF"; py sh-re-escape three || exit 13
[ -x "$HELPER" ] || exit 14
rm -f "$HELPER" "$CCA_HOME/update/cca.prev"; py sh-re-escape four 2>/dev/null; echo "rc=$?"
S
rm -rf "$HC/cca-helper"
out=$(hrun /bin/bash "$T/hc/heal.sh" "$T/hc/heal/cca" "$T/hc/heal/home/.cca" "$T/hc/newer" 2>&1); rc=$?
ok "辅助程序：解好的那一份中途没了（系统清了临时目录）自己再解；这一份 cca 已经被新版本换掉时从 update/cca.prev 里解；都没有了才失败（127）" '[ $rc -eq 0 ] && [ "$out" = "$(printf "%s\n" one two three rc=127)" ]'
rm -rf "$HC/cca-helper"
( hrun "$T/hc/cca" version > "$T/hc/p1" 2>&1 & hrun "$T/hc/cca" version > "$T/hc/p2" 2>&1 & wait )
ok "辅助程序：两个 cca 同时第一次运行，都成功，只留下一份" '[ "$(cat "$T/hc/p1")" = "$VER" ] && [ "$(cat "$T/hc/p2")" = "$VER" ] && [ "$(ls "$HC/cca-helper")" = "$HSUM" ]'
rm -rf "$HC" "$T/hc"; HC=""
edit_table 'j["ports"].pop("alice@example.com")' "$H/.cca/config.json"
out=$(gj state)
ok "_gui state 只在内存里对账：config.json 里少了的端口照样算出来（7901），文件不改；下一次真正运行 cca 时才写回去" 'printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"port\"] == 7901" && cfgok "\"alice@example.com\" not in c[\"ports\"]" && run cca ls >/dev/null 2>&1 && cfgok "c[\"ports\"][\"alice@example.com\"] == 7901"'
mv "$H/.cca/codex.json" "$T/codex.gui"
out=$(gj state); printf '%s' "$out" > "$GF/state_nocodex.json"; out2=$(gj hello)
ok "_gui state / hello：没有 codex.json 时 Codex 算没启用，也不会把它建出来" 'printf "%s" "$out" | jok "d[\"tools\"][1][\"enabled\"] is False and d[\"tools\"][1][\"accounts\"] == []" && printf "%s" "$out2" | jok "d[\"tools\"][\"codex\"][\"enabled\"] is False" && [ ! -e "$H/.cca/codex.json" ]'
mv "$T/codex.gui" "$H/.cca/codex.json"
edit_table 'j["kimi2"] = {"base_url": "https://api.example.com/anthropic", "model": "m1", "node": "", "key": "sk-GUI-SECRET-1234", "id": "kimi2"}'
out=$(gj state)
ok "_gui state：第三方 API 账号只给接口的域名和模型，绝不带 key；没有 App、网页版、用量、邮箱助手" '! printf "%s" "$out" | grep -q sk-GUI-SECRET && printf "%s" "$out" | jok "(lambda a: a[\"kind\"] == \"api\" and a[\"login\"] == \"api\" and a[\"api\"] == {\"host\": \"api.example.com\", \"model\": \"m1\"} and a[\"port\"] is None and a[\"usage\"] is None and a[\"can\"] == {\"app\": False, \"web\": False, \"billing\": False, \"limit_resets\": False, \"usage\": False, \"check\": True, \"cli\": True, \"policy_lock\": False, \"devices\": False, \"api_key\": True, \"mail\": False, \"cost\": False, \"custom_proxy\": True} and a[\"can_why\"][\"app\"] == \"api_account\" and a[\"mail\"] is None and a[\"api_key\"] == {\"saved\": True, \"hint\": \"\", \"source\": \"table\", \"shared\": False, \"saved_epoch\": None, \"from_device\": None, \"sync_error\": None})(acct(0, \"kimi2\"))"'
ok "_gui state：能不能打开账单页，条件和网页版一样（入口的显示由 cca 说了算）" 'printf "%s" "$out" | jok "all(a[\"can\"][\"billing\"] == a[\"can\"][\"web\"] for t in d[\"tools\"] for a in t[\"accounts\"])"'
ok "_gui state：用量页入口只给 Claude 账号，其余条件和网页版一样" 'printf "%s" "$out" | jok "all(a[\"can\"][\"limit_resets\"] == (i == 0 and a[\"can\"][\"web\"]) for i, t in enumerate(d[\"tools\"]) for a in t[\"accounts\"])"'
# 账号页的「邮箱」一节（PROTOCOL 4.11）：配了默认 Gmail（直接写 config.json，不用连 Gmail），state 只读 config.json、不碰钥匙串
edit_table 'j["mail"] = {"user": "team-login@example.com"}' "$H/.cca/config.json"
nfp=$(grep -c "find-generic-password" "$T/security.log" 2>/dev/null || echo 0)
stm=$(gj state)
ok "_gui state：Claude 官方账号 can.mail、account.mail 说走默认 / 单独配 / 没配置（只读 config.json，不碰钥匙串）；Codex、第三方 API 账号没有" 'printf "%s" "$stm" | jok "(lambda a: a[\"can\"][\"mail\"] is True and a[\"mail\"] == {\"configured\": True, \"user\": \"team-login@example.com\", \"source\": \"default\", \"auto\": True, \"forwarder\": None})(acct(0, \"alice@example.com\")) and acct(0, \"kimi2\")[\"can\"][\"mail\"] is False and acct(0, \"kimi2\")[\"mail\"] is None and acct(1, \"dave@example.com\")[\"can\"][\"mail\"] is False and acct(1, \"dave@example.com\")[\"mail\"] is None" && [ "$(grep -c "find-generic-password" "$T/security.log" 2>/dev/null || echo 0)" = "$nfp" ]'
edit_table 'j.pop("mail", None)' "$H/.cca/config.json"
mv "$T/bin/claude" "$T/claude.off"
out=$(run PATH="$T/bin:$H/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin" cca _gui state </dev/null 2>/dev/null)
mv "$T/claude.off" "$T/bin/claude"
ok "_gui state：没装 claude 命令时不能在终端里打开（第三方 API 账号也一样）" 'printf "%s" "$out" | jok "all(a[\"can\"][\"cli\"] is False and a[\"can_why\"][\"cli\"] == \"tool_missing\" for a in d[\"tools\"][0][\"accounts\"])"'
edit_table 'j.pop("kimi2")'; run cca ls >/dev/null 2>&1
# API key 的掩码（PROTOCOL 2.2）：存的时候算好记在 config.json 里，_gui state 照它说，不读钥匙串；短于 24 个字符的 key 没有掩码；
# 第三方 API 账号换了服务商（域名和存的时候不一样）算没存
K1=sk-proj-GUItest0123456789abcdEFGH
run CCA_TTY="$(ans "$K1")" cca key alice >/dev/null 2>&1
edit_table 'j["kimi3"] = {"base_url": "https://api.example.com/anthropic", "model": "m1", "node": "", "key": "sk-GUI-SECRET-0123456789wxyz", "id": "kimi3"}'
n_find=$(grep -c "find-generic-password" security.log); out=$(gj state); printf '%s' "$out" > "$GF/state_api_keys.json"
ok "_gui state 的 api_key：官方账号记的 key 只给掩码（sk-proj-…EFGH），账号表里写的 key 也只给掩码（sk-GUI-…wxyz），没存的是空的；不读钥匙串" '! printf "%s" "$out" | grep -qF "$K1" && ! printf "%s" "$out" | grep -q sk-GUI-SECRET && [ "$(grep -c "find-generic-password" security.log)" = "$n_find" ] && printf "%s" "$out" | jok "(lambda k: dict(k, saved_epoch=0) == {\"saved\": True, \"hint\": \"sk-proj-…EFGH\", \"source\": \"keychain\", \"shared\": False, \"saved_epoch\": 0, \"from_device\": None, \"sync_error\": None} and k[\"saved_epoch\"] > 1.7e9)(acct(0, \"alice@example.com\")[\"api_key\"]) and acct(0, \"kimi3\")[\"api_key\"] == {\"saved\": True, \"hint\": \"sk-GUI-…wxyz\", \"source\": \"table\", \"shared\": False, \"saved_epoch\": None, \"from_device\": None, \"sync_error\": None} and acct(1, \"dave@example.com\")[\"api_key\"] == {\"saved\": False, \"hint\": \"\", \"source\": \"\", \"shared\": False, \"saved_epoch\": None, \"from_device\": None, \"sync_error\": None} and all(a[\"can\"][\"api_key\"] for t in d[\"tools\"] for a in t[\"accounts\"])"'
edit_table 'j.pop("kimi3")'; run CCA_GRANT="key_forget:${AD##*/}" cca key alice --forget >/dev/null 2>&1; run cca ls >/dev/null 2>&1
# 短的 key（kimi 的 sk-test-CCCC3333）没有掩码，也记着存的时候的域名：换了服务商同样算没存
edit_table 'j["kimi"]["base_url"] = j["kimi"]["base_url"].replace("//api.example.com", "//api9.example.com")'; out=$(gj state)
edit_table 'j["kimi"]["base_url"] = j["kimi"]["base_url"].replace("//api9.example.com", "//api.example.com")'; out2=$(gj state)
ok "_gui state 的 api_key：第三方 API 账号换了服务商（域名和存的时候不一样）算没存，短的 key（没有掩码）也一样；换回来又算存了" 'printf "%s" "$out" | jok "acct(0, \"kimi\")[\"api_key\"] == {\"saved\": False, \"hint\": \"\", \"source\": \"\", \"shared\": False, \"saved_epoch\": None, \"from_device\": None, \"sync_error\": None}" && printf "%s" "$out2" | jok "acct(0, \"kimi\")[\"api_key\"] == {\"saved\": True, \"hint\": \"\", \"source\": \"keychain\", \"shared\": False, \"saved_epoch\": None, \"from_device\": None, \"sync_error\": None}"'
cp "$AD/claude/.claude.json" "$T/alice.claude.json"; printf '{"oauthAccount": ' > "$AD/claude/.claude.json"
out=$(gj state); rc=$?; printf '%s' "$out" > "$GF/state_account_unreadable.json"
cp "$T/alice.claude.json" "$AD/claude/.claude.json"
ok "_gui state：某个账号的 .claude.json 坏了也照样输出：这个账号当作没登录，errors 里说是哪个文件" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"login\"] == \"logged_out\" and any(e[\"code\"] == \"account_unreadable\" and e[\"tool\"] == \"claude\" and \".claude.json\" in e[\"message\"] for e in d[\"errors\"])"'
# 实况：在家看路由器，在外面看本机内置代理
s1=$(snap); out=$(gj live); printf '%s' "$out" > "$GF/live_router.json"
ok "_gui live（在家）：看路由器（OpenClash 在运行，钩子、cron 都在），分到账号的槽位都是 ok，空槽位是 empty；只读" '[ "$(snap)" = "$s1" ] && printf "%s" "$out" | jok "d[\"route\"] == \"router\" and d[\"at_home\"] is True and d[\"router\"][\"backend\"] == \"OpenClash\" and d[\"router\"][\"running\"] is True and d[\"router\"][\"hook\"] is True and d[\"router\"][\"cron\"] is True and d[\"router\"][\"error\"] is None and all(s[\"state\"] == (\"ok\" if s[\"key\"] else \"empty\") for s in d[\"slots\"]) and d[\"slots\"][0][\"selected\"] == d[\"slots\"][0][\"expected\"] and d[\"slots\"][0][\"listening\"] is True"'
oth=US-D; [ "$(sel CCA-1)" = US-D ] && oth=US-B
printf %s "$oth" > "$R/sel/CCA-1"; unlisten 7903
out=$(gj live); printf '%s' "$out" > "$GF/live_router_drift.json"
ok "_gui live：仪表盘里被手动切走的槽位是 drift（选中的不是应选的），连不上的端口是 closed" 'printf "%s" "$out" | jok "d[\"slots\"][0][\"state\"] == \"drift\" and d[\"slots\"][0][\"selected\"] == \"$oth\" and d[\"slots\"][2][\"state\"] == \"closed\" and d[\"summary\"][\"drift\"] == 1 and d[\"summary\"][\"closed\"] == 1"'
echo 7903 >> "$R/listening"; run cca router sync >/dev/null 2>&1
ok "（还原）同步一次就纠正回来" '[ "$(sel CCA-1)" != "$oth" ]'
touch "$T/away"
out=$(run CCA_ROUTE=router cca _gui live </dev/null 2>/dev/null); printf '%s' "$out" > "$GF/live_unreachable.json"
out2=$(gj live); printf '%s' "$out2" > "$GF/live_local.json"
rm -f "$T/away"
ok "_gui live（在外面）：指定看路由器时说连不上（槽位 closed）；默认看本机出口（还没配置内置代理）" 'printf "%s" "$out" | jok "d[\"route\"] == \"router\" and d[\"at_home\"] is False and d[\"router\"][\"error\"][\"code\"] == \"unreachable\" and all(s[\"state\"] in (\"closed\", \"empty\") for s in d[\"slots\"])" && printf "%s" "$out2" | jok "d[\"route\"] == \"local\" and d[\"local\"][\"configured\"] is False"'
# 出口检测结果：启动、cca check 都记进账号目录的 health.json
run cca @alice --version >/dev/null 2>&1
ok "启动时记下出口检测结果（health.json，只给自己读）：来源 launch、结果 ok、出口 IP、泄露检测的状态" '[ "$(stat -f %Lp "$AD/health.json")" = 600 ] && jok "d[\"source\"] == \"launch\" and d[\"result\"] == \"ok\" and d[\"ip\"] == \"203.0.113.1\" and d[\"trust\"] == 97 and d[\"port\"] == 7901 and d[\"leak\"][\"dns\"] == \"ok\"" < "$AD/health.json"'
out=$(gj state)
ok "_gui state 里的 health：就是这次的结果，加上多久前、是不是旧了、换没换节点" 'printf "%s" "$out" | jok "(lambda h: h[\"result\"] == \"ok\" and h[\"age_seconds\"] < 120 and h[\"stale\"] is False and h[\"applies\"] is True)(acct(0, \"alice@example.com\")[\"health\"])"'
out=$(run cca check alice --json 2>/dev/null); rc=$?; printf '%s' "$out" > "$GF/check.json"
ok "cca check --json：stdout 只有 JSON（每个账号的检测结果），结果也记进 health.json（来源 check）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"check\" and d[\"tool\"] == \"claude\" and d[\"route\"] == \"router\" and d[\"accounts\"][0][\"key\"] == \"alice@example.com\" and d[\"accounts\"][0][\"health\"][\"result\"] == \"ok\" and d[\"shared_exit_ips\"] == []" && grep -q "\"source\": \"check\"" "$AD/health.json"'
cp "$T/usage_auth.json" "$GF/usage_auth.json"
out=$(run cca check kimi alice --json 2>/dev/null); rc=$?; out1=$(run cca check kimi --json 2>/dev/null)
ok "cca check 可以写几个账号：只测写了的这几个（App 定时检测只测到期的）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "[a[\"key\"] for a in d[\"accounts\"]] == [\"kimi\", \"alice@example.com\"]" && printf "%s" "$out1" | jok "[a[\"key\"] for a in d[\"accounts\"]] == [\"kimi\"]"'
out=$(run cca check 没有这个账号 --json 2>/dev/null); rc=$?
ok "cca check 写错账号：报错退出" '[ $rc -ne 0 ]'
# 调整账号表的顺序（cca reorder，图形界面拖动账号时用）：只改先后，端口池、摘要、启动器的颜色都按调整之前的顺序（base_order）
out=$(gj hello); ok "_gui hello：说明能写回账号表的顺序（旧版 cca 没有，App 就不写）" 'printf "%s" "$out" | jok "d[\"reorder\"] == 1"'
edit_table 'j["zed@example.com"] = "US-B"'; run cca router sync >/dev/null 2>&1
gj state > "$T/order0.json"
order_same() { jok "(lambda v: v(d) == v(json.load(open(\"$T/order0.json\"))) and d[\"routing\"][\"sync\"] == \"ok\")(lambda s: {(t[\"id\"], a[\"key\"]): (a[\"port\"], a[\"slot\"], a[\"badge\"]) for t in s[\"tools\"] for a in t[\"accounts\"]})"; }
keys_now='[a["key"] for a in d["tools"][0]["accounts"]]'
out=$(run cca reorder zed kimi 2>&1); rc=$?; st=$(gj state)
ok "cca reorder：按写的顺序排，没写到的账号接在后面，账号的内容不变，什么都不输出；_gui state 也是这个顺序" '[ $rc -eq 0 ] && [ -z "$out" ] && printf "%s" "$st" | jok "list(tab(\"accounts.json\")) == [\"zed@example.com\", \"kimi\", \"alice@example.com\"] and tab(\"accounts.json\")[\"zed@example.com\"] == \"US-B\" and $keys_now == list(tab(\"accounts.json\"))"'
ok "调整顺序不牵连别的：不用同步路由器（摘要不变），端口、槽位、头像的颜色都不变" 'printf "%s" "$st" | order_same'
n0=$(wc -l < ssh.log); out=$(run cca reorder 2>&1); rc=$?
ok "cca reorder 不写账号：说明用法" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法"'
out=$(run cca reorder 没有这个账号 2>&1); rc=$?; st=$(gj state)
ok "cca reorder 写错账号：报错，账号表不动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有匹配的账号" && printf "%s" "$st" | jok "list(tab(\"accounts.json\"))[0] == \"zed@example.com\""'
run cca reorder alice zed kimi >/dev/null 2>&1; st=$(gj state)
ok "再调整一次：基准顺序还是第一次调整之前的，不用同步路由器" 'printf "%s" "$st" | order_same && [ "$(wc -l < ssh.log)" = "$n0" ]'
edit_table 'j.pop("zed@example.com")'; edit_table 'j.pop("order_base", None)' "$H/.cca/config.json"; run cca router sync >/dev/null 2>&1
# 同意：出口纯净度偏低时问要不要照样启动；授权要带出口 IP，换了 IP 不算
printf '{"countryCode":"US","trust_score":30}' > "$T/risk/203.0.113.1.json"; : > open.log
out=$(run cca app alice 2>&1 </dev/null); rc_plain=$?
out_ev=$(runev cca app alice 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/events_consent.jsonl"
ok "图形界面：纯净度偏低时记一条 consent（low_purity，范围是出口 IP，默认不同意），照旧不启动；有没有事件文件，stderr 和退出码都一样" '[ $rc -ne 0 ] && [ $rc -eq $rc_plain ] && [ "$out" = "$out_ev" ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"low_purity\" and e[\"grant\"] == \"low_purity:203.0.113.1\" and e[\"default\"] == \"no\" and \"纯净度 30\" in e[\"detail\"]" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"low_purity\"" && ! grep -q "user-data-dir=$AD/desktop" open.log'
runev CCA_GRANT=low_purity:203.0.113.9 cca app alice >/dev/null 2>&1; rc=$?
ok "图形界面：同意的是别的出口 IP：不算" '[ $rc -ne 0 ] && ! grep -q "user-data-dir=$AD/desktop" open.log'
runev CCA_GRANT=low_purity:203.0.113.1 cca app alice >/dev/null 2>&1; rc=$?; cp "$T/ev.jsonl" "$GF/events_open.jsonl"
ok "图形界面：同意了这个出口 IP：照样打开 App，事件里有每一步和结果" '[ $rc -eq 0 ] && grep -q "user-data-dir=$AD/desktop" open.log && evq "e[\"ev\"] == \"step\" and e[\"code\"] == \"preflight\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"opened\" and e[\"node\"] and e[\"port\"] == 7901" && jok "d[\"result\"] == \"low_purity\"" < "$AD/health.json"'
rm -f "$T/risk/203.0.113.1.json"
: > open.log; rm -f "$T/clipboard"
runev cca billing alice >/dev/null 2>&1; rc=$?
ok "cca billing：用账号专属的浏览器和出口打开控制台的账单页，步骤和结果事件和网页版一样" '[ $rc -eq 0 ] && grep -F "https://platform.claude.com/settings/billing" open.log | grep -F -- "--user-data-dir=$AD/chrome" | grep -qF -- "--proxy-server=http://" && evq "e[\"ev\"] == \"step\" and e[\"code\"] == \"preflight\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"opened\" and e[\"port\"] == 7901 and \"控制台的账单页\" in e[\"message\"] and \"邮箱 alice@example.com 已经复制好\" in e[\"message\"]" && [ "$(cat "$T/clipboard")" = alice@example.com ]'
: > open.log; rm -f "$T/clipboard"
runev cca limit-resets alice >/dev/null 2>&1; rc=$?
ok "cca limit-resets：用账号专属的浏览器和出口打开 claude.ai 的用量页，步骤和结果事件和网页版一样" '[ $rc -eq 0 ] && grep -F "https://claude.ai/settings/usage" open.log | grep -F -- "--user-data-dir=$AD/chrome" | grep -qF -- "--proxy-server=http://" && evq "e[\"ev\"] == \"step\" and e[\"code\"] == \"preflight\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"opened\" and e[\"port\"] == 7901 and \"用量页\" in e[\"message\"] and \"已经复制好\" in e[\"message\"]" && [ "$(cat "$T/clipboard")" = alice@example.com ]'
# 失败的原因、要在终端里做的事
unlisten 7901; n_cli=$(grep -c '^ARGS:' claude.log)
out=$(run cca @alice --version 2>&1 </dev/null); rc_plain=$?
out_ev=$(runev cca @alice --version 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/events_fail.jsonl"
echo 7901 >> "$R/listening"
ok "图形界面：端口池缺失、未授权重载时 sync_failed 并且不启动；stderr 和退出码和不设事件文件时一样" '[ $rc -ne 0 ] && [ $rc -eq $rc_plain ] && [ "$out" = "$out_ev" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"sync_failed\"" && [ "$(grep -c "^ARGS:" claude.log)" = "$n_cli" ]'
edit_table 'j["测试(zed@example.com)"] = "US-B"'
out=$(runev cca @zed --version 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/events_terminal.jsonl"
ok "图形界面：还没登录的账号：terminal 事件给出在终端里登录的命令（argv 里是原样的键，给人看的命令加了引号，能直接复制进终端），fail 原因 not_logged_in" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"login\" and e[\"argv\"] == [\"login\", \"测试(zed@example.com)\"] and e[\"display\"] == \"cca login '"'"'测试(zed@example.com)'"'"'\"" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"not_logged_in\""'
edit_table 'j.pop("测试(zed@example.com)")'; run cca router sync >/dev/null 2>&1
run cca @alice --version >/dev/null 2>&1 </dev/null   # 先跑一次：上一条删掉的账号，它的启动器在这次启动时才删（会多一行说明）
out=$(run cca @alice --version 2>&1 </dev/null); rc=$?
out2=$(run CCA_EVENTS=/nonexistent/dir/ev.jsonl cca @alice --version 2>&1 </dev/null); rc2=$?
ok "事件文件指向写不了的地方：命令照常，输出和不设事件文件时一样（bash 打不开文件的报错也不漏出来）" '[ $rc -eq 3 ] && [ $rc2 -eq 3 ] && [ "$out" = "$out2" ]'
# Claude Code 记的套餐过时了（outdated_login_plan）：契约测试要解码不是 null 的（下面查用量时 alice 又回到 max）
canned alice@example.com 13 41 pro
out=$(run cca usage --json 2>/dev/null); printf '%s' "$out" > "$GF/usage_claude_outdated.json"
out2=$(gj state); printf '%s' "$out2" > "$GF/state_outdated_plan.json"
ok "usage --json / _gui state：Claude Code 记的套餐和账号资料对不上时 outdated_login_plan 是记下的那个（Pro）；Codex 的条目没有这个字段" 'printf "%s" "$out" | jok "uacct(\"alice@example.com\")[\"outdated_login_plan\"] == \"Pro\"" && printf "%s" "$out2" | jok "acct(0, \"alice@example.com\")[\"usage\"][\"outdated_login_plan\"] == \"Pro\" and all(\"outdated_login_plan\" not in (a[\"usage\"] or {}) for a in d[\"tools\"][1][\"accounts\"])"'
# usage --json 的新字段（alice 带着 Fable 的每周额度：契约测试要解码它占本周总额度的份额）
canned alice@example.com 13 41 max "{$fable, \"limits\": [{\"kind\": \"weekly_all\", \"scope\": null}, {\"kind\": \"weekly_scoped\", \"scope\": {\"model\": {\"id\": null, \"display_name\": \"Fable\"}}}]}"
out=$(run cca usage --json 2>/dev/null); printf '%s' "$out" > "$GF/usage_claude.json"
ok "usage --json：新字段 kind / generated_epoch / error_code / fetched_epoch / stale，窗口有 resets_epoch（Unix 秒）" 'printf "%s" "$out" | jok "d[\"kind\"] == \"usage\" and isinstance(d[\"generated_epoch\"], float) and (lambda a: a[\"source\"] == \"live\" and a[\"error_code\"] is None and a[\"stale\"] is False and isinstance(a[\"fetched_epoch\"], float) and isinstance(a[\"five_hour\"][\"resets_epoch\"], float) and a[\"five_hour\"][\"resets_epoch\"] > a[\"fetched_epoch\"])(uacct(\"alice@example.com\"))"'
cp "$AD/usage.json" "$T/usage.keep"
printf '{"subscription_type": "max", "rate_limits_available": true, "rate_limits": {"five_hour": null, "seven_day": null}}' > "$T/usage/alice@example.com.json"
out=$(run cca usage --json 2>/dev/null)
ok "usage：查到的数据里一个窗口都认不出时不算查到（nolimits），不把好好的缓存换掉" 'printf "%s" "$out" | jok "(lambda a: a[\"source\"] == \"cache\" and a[\"error_code\"] == \"nolimits\" and a[\"five_hour\"][\"utilization\"] == 13)(uacct(\"alice@example.com\"))" && cmp -s "$AD/usage.json" "$T/usage.keep"'
unlisten 7901
out=$(run cca usage --json 2>/dev/null)
echo 7901 >> "$R/listening"
ok "usage --json：端口不通时 error_code 是 port，error_detail 是端口" 'printf "%s" "$out" | jok "(lambda a: a[\"error_code\"] == \"port\" and a[\"error_detail\"] == \"7901\" and a[\"source\"] == \"cache\")(uacct(\"alice@example.com\"))"'
out=$(run cca codex usage --json 2>/dev/null); printf '%s' "$out" > "$GF/usage_codex.json"
ok "codex usage --json：同样的新字段（Codex 的重置时间本来就是 Unix 秒）" 'printf "%s" "$out" | jok "d[\"tool\"] == \"codex\" and all(\"error_code\" in a and \"stale\" in a for a in d[\"accounts\"])"'
# 重置额度（App 的「重置额度…」）：要同意（reset_credit:账号目录名，默认不同意），同意后带着授权重跑，stdout 是 kind=reset 的文档
ccanned dave@example.com 100 41 pro 1; nc=$(grep -c "RPC resetConsume" codex.log)
out=$(runev cca codex reset dave --json 2>/dev/null); rc=$?; cp "$T/ev.jsonl" "$GF/events_reset_consent.jsonl"
ok "图形界面：重置额度先记一条 consent（reset_credit，范围是账号目录名，默认不同意，说明里有还剩几次），stdout 什么都没有，不重置" '[ $rc -ne 0 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"reset_credit\" and e[\"grant\"] == \"reset_credit:${DD##*/}\" and e[\"default\"] == \"no\" and \"还剩 1 次\" in e[\"detail\"]" && [ "$(grep -c "RPC resetConsume" codex.log)" = "$nc" ]'
wb=$(gj state | python3 -c 'import json,sys; d=json.load(sys.stdin); w=next(a for a in d["tools"][1]["accounts"] if a["key"] == "dave@example.com")["usage_weeks"]["weeks"][-1]; print(w["resets"], w["used_percent"])')
out=$(runev CCA_GRANT="reset_credit:${DD##*/}" cca codex reset dave --json 2>/dev/null); rc=$?; printf '%s' "$out" > "$GF/reset.json"
ok "图形界面：同意之后重置，stdout 是 kind=reset 的文档（结果、说明、重置后的用量）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"reset\" and d[\"tool\"] == \"codex\" and d[\"key\"] == \"dave@example.com\" and d[\"outcome\"] == \"reset\" and \"已重置\" in d[\"message\"] and d[\"usage\"][\"five_hour\"][\"utilization\"] == 0 and d[\"usage\"][\"reset_credits\"][\"availableCount\"] == 0" && [ "$(grep -c "RPC resetConsume" codex.log)" = $((nc + 1)) ]'
ok "重置之后：每周用量里这一周记一次重置，用到的还是重置前的那么多（清零的不减掉）" 'gj state | jok "(lambda w: w[\"resets\"] == ${wb%% *} + 1 and w[\"used_percent\"] == ${wb#* } and w[\"current\"] is True)(acct(1, \"dave@example.com\")[\"usage_weeks\"][\"weeks\"][-1])"'
out=$(runev CCA_GRANT="reset_credit:${DD##*/}" cca codex reset dave --json 2>/dev/null); rc=$?
ok "图形界面：没有重置机会了：fail 事件的原因是 no_reset_credit，不再去重置" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"no_reset_credit\"" && [ "$(grep -c "RPC resetConsume" codex.log)" = $((nc + 1)) ]'
# 检查更新（App 的「检查更新…」）：cca update --check --json，按现在的版本挑（这里是开发模式下的测试版：正式版、测试版都看）
mkdir -p "$T/gh/v99.0.0"; printf x > "$T/gh/v99.0.0/cca"; printf 'cca 99.0.0：更新说明的第一行\n第二行\n' > "$T/gh/v99.0.0/notes"
out=$(run cca update --check --json 2>/dev/null </dev/null); rc=$?; printf '%s' "$out" > "$GF/update.json"
rm -rf "$T/gh" "$T/gh.log"   # 11b 的自动更新从空的假 GitHub 开始
ok "update --check --json：stdout 只有 JSON（kind=update：最新版本、有没有新的、更新说明、发布页），开发模式下也能查" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"update\" and d[\"cca_version\"] == \"$VER\" and d[\"latest\"] == \"99.0.0\" and d[\"available\"] is True and \"dev_mode\" not in d and d[\"source\"] == \"releases\" and d[\"notes\"].startswith(\"cca 99.0.0\") and d[\"url\"].endswith(\"/releases/tag/v99.0.0\")"'
# 改别名（cca rename；App 的“改别名…”用它）：只换账号表里的键，账号目录、端口、“上次使用”、路由器的摘要都不变，启动器跟着改名
edit_table 'j["kimi9"] = {"base_url": "https://api.example.com/anthropic", "model": "m1", "node": "", "id": "kimi9"}'; run cca ls >/dev/null 2>&1 </dev/null
sb=$(gj state | python3 -c 'import json,sys; print(json.load(sys.stdin)["routing"]["sync"])')
out=$(runev cca rename alice@example.com 主号 2>/dev/null); rc=$?
ok "rename：alice 改名为「主号」：stdout 是新键、在表里的位置不变，端口还是 7901，账号目录没换，启动器改名，路由器的同步状态不变" '[ $rc -eq 0 ] && [ "$out" = "主号(alice@example.com)" ] && tblok "list(t)[0] == \"主号(alice@example.com)\" and \"alice@example.com\" not in t" && cfgok "c[\"ports\"][\"主号(alice@example.com)\"] == 7901 and \"alice@example.com\" not in c[\"ports\"]" && [ "$(ls -d "$H"/.cca/accounts/alice-example-com-*)" = "$AD" ] && [ -d "$L/Claude 主号.app" ] && [ ! -e "$L/Claude alice.app" ] && gj state | jok "d[\"routing\"][\"sync\"] == \"$sb\" and acct(0, \"主号(alice@example.com)\")[\"port\"] == 7901"'
out=$(runev cca rename 主号 "有 空格" 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/events_rename_fail.jsonl"
ok "rename：别名里有空格：不改，fail 事件的原因是 alias_invalid，说明里写清楚" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"alias_invalid\" and \"空格\" in e[\"message\"]" && tblok "\"主号(alice@example.com)\" in t"'
out=$(runev cca rename kimi9 主号 2>&1); rc=$?
ok "rename：别名和别的账号重了：不改（alias_invalid）" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"alias_invalid\" and \"已经用在\" in e[\"message\"]" && tblok "\"kimi9\" in t"'
out=$(runev cca rename nobody 新名 2>&1); rc=$?
ok "rename：没有这个账号：account_missing" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"account_missing\""'
out=$(run cca rename kimi9 kimi8 2>/dev/null </dev/null); rc=$?
ok "rename：第三方 API 账号改名：键换了，id 还是 kimi9（账号目录、钥匙串里的 key 都按 id 认）" '[ $rc -eq 0 ] && [ "$out" = kimi8 ] && tblok "t[\"kimi8\"][\"id\"] == \"kimi9\" and \"kimi9\" not in t"'
out=$(runev cca rename kimi8 - 2>&1); rc=$?
ok "rename：第三方 API 账号不能去掉别名" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"alias_invalid\"" && tblok "\"kimi8\" in t"'
edit_table 'j.pop("kimi8")'; run cca ls >/dev/null 2>&1 </dev/null
out=$(run cca rename 主号 - 2>/dev/null </dev/null); rc=$?
ok "rename：写 - 去掉别名，只留邮箱：端口、启动器都回到原来的样子" '[ $rc -eq 0 ] && [ "$out" = alice@example.com ] && tblok "list(t)[0] == \"alice@example.com\"" && cfgok "c[\"ports\"][\"alice@example.com\"] == 7901" && [ -d "$L/Claude alice.app" ] && [ ! -e "$L/Claude 主号.app" ]'
edit_table 'j["onlyalias"] = "US-A"'
out=$(runev cca rename onlyalias 新名 2>&1); rc=$?
ok "rename：只写了别名、没有邮箱的官方账号不能改（改了会换掉账号目录），让用 cca edit 补上邮箱" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"alias_invalid\" and \"edit\" in e[\"message\"]" && tblok "\"onlyalias\" in t"'
edit_table 'j.pop("onlyalias")'
out=$(run cca codex rename dave@example.com 二号 2>/dev/null </dev/null); rc=$?
ok "codex rename：Codex 的账号表同样改，端口不变" '[ $rc -eq 0 ] && [ "$out" = "二号(dave@example.com)" ] && tblok "\"二号(dave@example.com)\" in t" "$H/.cca/codex.json" && cfgok "c[\"codex\"][\"ports\"][\"二号(dave@example.com)\"] == 7903"'
run cca codex rename 二号 - >/dev/null 2>&1 </dev/null
run cca router sync >/dev/null 2>&1
# 办公室（App 的首页）要的两样：套餐档位（plan_tier：cca 按结构化的账号信息算，App 不按套餐名猜）和命令行会话的登记（cli）
ok "_gui state：每个账号都有 cli（没有会话：不在运行、0 个、since_epoch 为 null）；alice（Max）的档位是 advanced；第三方 API 账号的档位是空的" 'jok "all(a[\"cli\"] == {\"running\": False, \"sessions\": 0, \"since_epoch\": None} for t in d[\"tools\"] for a in t[\"accounts\"]) and acct(0, \"alice@example.com\")[\"plan_tier\"] == \"advanced\" and all(a[\"plan_tier\"] == \"\" for t in d[\"tools\"] for a in t[\"accounts\"] if a[\"kind\"] == \"api\")" < "$GF/state.json"'
waitmark() { local i; for i in $(seq 1 100); do [ "$(ls "$1"/cli-*.json 2>/dev/null | wc -l)" -ge "${2:-1}" ] && return 0; sleep 0.1; done; return 1; }   # 等账号目录里出现几份会话登记（默认 1 份）
marks() { ls "$1"/cli-*.json 2>/dev/null | sed 's/.*cli-\([0-9]*\)\.json$/\1/'; }   # 登记的进程号，一行一个
pstart() { python3 -I -c 'import os,subprocess,sys,time; s=subprocess.run(["ps","-o","lstart=","-p",sys.argv[1]],capture_output=True,text=True,env=dict(os.environ,LC_ALL="C")).stdout; print(int(time.mktime(time.strptime(" ".join(s.split()),"%a %b %d %H:%M:%S %Y"))))' "$1"; }   # 进程的启动时间戳（和 cca 一样的算法）
mark() { python3 -I -c 'import json,sys,time; d,t,i,p,s=sys.argv[1:6]; json.dump({"v":1,"tool":t,"id":i,"pid":int(p),"started":int(s),"since":time.time()}, open("%s/cli-%s.json" % (d,p),"w"))' "$@"; }   # mark 账号目录 工具 账号目录名 进程号 启动时间戳：手写一份登记
touch "$T/hold"; t0=$(date +%s)   # 假 claude / codex 在 hold 存在时一直不退出：会话开着
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SP1=$!
waitmark "$AD"; mp=$(marks "$AD"); s1=$(snap)
out=$(gj state); printf '%s' "$out" > "$GF/state_cli.json"
ok "命令行会话：cca 启动 claude 前在账号目录登记 cli-<cca 的进程号>.json（只给自己读；记着工具、账号目录名、进程号和它的启动时间）" '[ -n "$mp" ] && [ "$(stat -f %Lp "$AD/cli-$mp.json")" = 600 ] && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if j[\"v\"] == 1 and j[\"tool\"] == \"claude\" and j[\"id\"] == sys.argv[2] and j[\"pid\"] == int(sys.argv[3]) and j[\"started\"] == int(sys.argv[4]) and j[\"since\"] >= int(sys.argv[5]) - 1 else 1)" "$AD/cli-$mp.json" "${AD##*/}" "$mp" "$(pstart "$mp")" "$t0"'
ok "_gui state：会话开着时 alice 的 cli 是在运行、1 个、从登记的时刻算起，别的账号都没有；读 state 不写文件" 'printf "%s" "$out" | jok "(lambda a: a[\"cli\"][\"running\"] is True and a[\"cli\"][\"sessions\"] == 1 and $t0 - 1 <= a[\"cli\"][\"since_epoch\"] <= $t0 + 60 and a[\"app\"][\"running\"] is False)(acct(0, \"alice@example.com\")) and sum(a[\"cli\"][\"sessions\"] for t in d[\"tools\"] for a in t[\"accounts\"]) == 1" && [ "$(snap)" = "$s1" ]'
kill -HUP "$mp" 2>/dev/null; kill -TERM "$mp" 2>/dev/null; sleep 0.5
ok "命令行会话：cca 收到 HUP、TERM（关终端、Ctrl-C）时等工具退出才收尾，登记还在" 'kill -0 "$mp" 2>/dev/null && [ -f "$AD/cli-$mp.json" ] && gj state | jok "acct(0, \"alice@example.com\")[\"cli\"][\"running\"] is True"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SP2=$!
waitmark "$AD" 2; mp2=$(marks "$AD" | grep -vx "$mp")
out=$(gj state)
ok "_gui state：同一个账号开着两个会话：2 个，since_epoch 是先开的那个的登记时刻" 'printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"cli\"][\"sessions\"] == 2 and acct(0, \"alice@example.com\")[\"cli\"][\"since_epoch\"] == tab(\"accounts/${AD##*/}/cli-$mp.json\")[\"since\"]"'
pkill -KILL -P "$mp2"; wait "$SP2" 2>/dev/null
ok "命令行会话：工具被 kill -9 时 cca 照样删掉自己的登记，另一个会话的还在" '[ -n "$mp2" ] && [ ! -e "$AD/cli-$mp2.json" ] && [ -f "$AD/cli-$mp.json" ] && gj state | jok "acct(0, \"alice@example.com\")[\"cli\"][\"sessions\"] == 1"'
rm -f "$T/hold"; wait "$SP1"; rc=$?
ok "命令行会话：工具退出后登记马上删掉（退出码照旧透传），state 里不在运行" '[ $rc -eq 3 ] && [ -z "$(ls "$AD"/cli-*.json 2>/dev/null)" ] && gj state | jok "acct(0, \"alice@example.com\")[\"cli\"] == {\"running\": False, \"sessions\": 0, \"since_epoch\": None}"'
touch "$T/hold"; run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SP3=$!
waitmark "$AD"; mp3=$(marks "$AD"); tp=$(pgrep -P "$mp3")
kill -KILL "$mp3"; sleep 0.3; s2=$(snap); out=$(gj state)
ok "命令行会话：cca 自己被 kill -9（来不及删登记）：state 核对出进程没了，不算在运行，也不删文件（只读）" '[ -f "$AD/cli-$mp3.json" ] && printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"cli\"][\"running\"] is False" && [ "$(snap)" = "$s2" ]'
rm -f "$T/hold"; wait "$SP3" 2>/dev/null; for i in $(seq 1 50); do kill -0 "$tp" 2>/dev/null || break; sleep 0.1; done
DEAD=$(sh -c 'echo $$'); mark "$AD" claude "${AD##*/}" "$DEAD" "$(date +%s)"   # 进程没了
echo '{bad json' > "$AD/cli-4242.json"; mark "$AD" codex "${AD##*/}" 4243 "$(date +%s)"   # 格式坏的；别的工具的登记放错了地方
sleep 300 & SL=$!; mark "$AD" claude "${AD##*/}" "$SL" $(( $(pstart "$SL") - 3600 ))   # 进程号被别的进程用了（启动时间对不上）
out=$(gj state)
ok "_gui state：对不上的旧登记（进程没了、格式坏、别的工具的、进程号被别的进程重用了）都不算在运行" 'printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"cli\"][\"running\"] is False" && [ "$(ls "$AD"/cli-*.json | wc -l)" -ge 5 ]'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1
ok "命令行会话：下一次启动这个账号时清掉对不上的旧登记（自己的登记也随退出删了）" '[ -z "$(ls "$AD"/cli-*.json 2>/dev/null)" ]'
mark "$AD" claude "${AD##*/}" "$SL" "$(pstart "$SL")"
ok "_gui state：进程号和启动时间都对得上的登记算在运行（对照）" 'gj state | jok "acct(0, \"alice@example.com\")[\"cli\"][\"sessions\"] == 1"'
kill "$SL" 2>/dev/null; wait "$SL" 2>/dev/null; rm -f "$AD/cli-$SL.json"
touch "$T/hold"; run CCA_TTY="$(ans)" cca codex @dave --version >/dev/null 2>&1 & SP4=$!
waitmark "$DD"; mp4=$(marks "$DD"); out=$(gj state); printf '%s' "$out" > "$GF/state_cli_codex.json"
ok "命令行会话：Codex 的会话登记在 Codex 的账号目录里，state 里算在 Codex 的 dave 上" '[ -n "$mp4" ] && python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"tool\"] == \"codex\" else 1)" "$DD/cli-$mp4.json" && printf "%s" "$out" | jok "acct(1, \"dave@example.com\")[\"cli\"][\"running\"] is True and sum(a[\"cli\"][\"sessions\"] for a in d[\"tools\"][0][\"accounts\"]) == 0"'
rm -f "$T/hold"; wait "$SP4"
ok "命令行会话：Codex 的会话退出后登记删掉" '[ -z "$(ls "$DD"/cli-*.json 2>/dev/null)" ]'
# 套餐档位的表：Claude 改 alice 的 oauthAccount（种类;额度档位;官方套餐名;应为 套餐|档位），用量缓存先挪开（它的 subscription_type 会顶替空的种类）
tier_of() { gj state | python3 -c 'import json,sys; a=next(a for a in json.load(sys.stdin)["tools"][int(sys.argv[1])]["accounts"] if a["key"] == sys.argv[2]); print(a["plan"] + "|" + a["plan_tier"])' "$1" "$2"; }
cp "$AJ" "$T/alice.oa.bak"; [ ! -f "$AD/usage.json" ] || mv "$AD/usage.json" "$T/alice.usage.bak"; bad=""
while IFS=';' read -r ot tr nm want; do
	setoa "$AJ" "organizationType=$ot" "organizationRateLimitTier=$tr" "planDisplayName=$nm"
	got=$(tier_of 0 alice@example.com); [ "$got" = "$want" ] || bad="$bad [${ot};${tr};${nm} → ${got}，应为 ${want}]"
done <<'EOF'
claude_free;;;Free|entry
claude_pro;default_claude_ai;;Pro|standard
claude_max;default_claude_max_5x;;Max|advanced
claude_max;default_claude_max_20x;;Ultra|premium
claude_team;default_claude_ai;;Team|standard
claude_team;default_claude_max_5x;;Team Premium|advanced
claude_enterprise;default_claude_ai;;Enterprise|standard
claude_max;default_claude_max_20x;Ultra Plus;Ultra Plus|premium
claude_max;default_claude_max_5x;Ultra;Ultra|advanced
claude_max;;Ultra;Ultra|premium
claude_max;;;Max|advanced
claude_team;;Team Premium;Team Premium|advanced
;;Ultra;Ultra|premium
;;Max 20x;Max 20x|premium
;;Team Premium;Team Premium|advanced
;;Ultra Plus;Ultra Plus|
claude_zeta;;Zeta Plan;Zeta Plan|
EOF
ok "套餐档位（Claude）：先看种类和额度倍数（Max 20x 起 premium、Team 5x 起 advanced），倍数缺失才看官方的套餐名，名字认不出就不猜；冲突时种类和倍数赢${bad}" '[ -z "$bad" ]'
setoa "$AJ" organizationType=claude_max organizationRateLimitTier=default_claude_max_20x planDisplayName=Ultra; gj state > "$GF/state_tiers.json"
setoa "$AJ" emailAddress=other@example.com
ok "套餐档位：登录的不是这个账号时套餐和档位都是空的" '[ "$(tier_of 0 alice@example.com)" = "|" ]'
cp "$T/alice.oa.bak" "$AJ"; [ ! -f "$T/alice.usage.bak" ] || mv "$T/alice.usage.bak" "$AD/usage.json"
# ChatGPT：dave 的用量缓存里写原始套餐值（原始值;应为 套餐|档位）；最后一行是缓存里没有套餐，用登录信息里的（plus）
[ ! -f "$DD/usage.json" ] || mv "$DD/usage.json" "$T/dave.usage.bak"; bad=""
while IFS=';' read -r raw want; do
	python3 -c 'import json,sys,time; json.dump({"fetchedAtMs": int(time.time() * 1000), "utilization": {"plan": sys.argv[2]} if sys.argv[2] else {}}, open(sys.argv[1], "w"))' "$DD/usage.json" "$raw"
	got=$(tier_of 1 dave@example.com); [ "$got" = "$want" ] || bad="$bad [${raw} → ${got}，应为 ${want}]"
done <<'EOF'
free;Free|entry
go;Go|entry
plus;Plus|standard
team;Business|standard
business;Enterprise|standard
edu;Edu|standard
prolite;Pro 100|advanced
pro;Pro 200|advanced
PRO;Pro 200|advanced
self_serve_business_prolite;Business Premium|advanced
promax;Pro 500|premium
zz_future;Zz_future|
;Plus|standard
EOF
rm -f "$DD/usage.json"; [ ! -f "$T/dave.usage.bak" ] || mv "$T/dave.usage.bak" "$DD/usage.json"
ok "套餐档位（ChatGPT）：按原始套餐值查表（Pro 100 / 200 是 advanced，Pro 500 是 premium），表里没有的新套餐不猜；用量缓存里没有套餐时用登录信息里的${bad}" '[ -z "$bad" ]'
# 折合金额（PROTOCOL 2.2 的 cost、cost_total，3.5 的 cca cost sync --json）：共享的会话记录（~/.claude/projects、~/.codex/sessions）里每次请求的
# 用量按官方 API 的标价折算，按登录的账号身份（Claude 的 accountUuid、Codex id_token 的 chatgpt_user_id）认是谁的；App 定时跑 cca cost sync --json，
# _gui state 只读统计好的。alice 登录的身份换成 u-alice-0001，dave 的 id_token 里补上用户 id（假 codex 写的没有），这一段结束时还原
cp -p "$AJ" "$T/cost.aj.json"; cp -p "$DD/codex/auth.json" "$T/cost.dave.json"
setoa "$AJ" accountUuid=u-alice-0001
costlog() {   # costlog 家目录 写法…：往会话记录里写（a 追加），见下面的 python
	python3 - "$@" <<'PY'
import base64, json, os, sys, time
home, what = sys.argv[1], sys.argv[2:]
now = time.time()
iso = lambda t: time.strftime('%Y-%m-%dT%H:%M:%S.000Z', time.gmtime(t))
line = lambda d: json.dumps(d, separators=(',', ':')) + '\n'
def reply(t, mid, model, inp, out, sid='s-cost-1'):
    return line({'type': 'assistant', 'timestamp': iso(t), 'requestId': 'req_' + mid, 'sessionId': sid,
                 'message': {'id': mid, 'model': model, 'role': 'assistant', 'usage': {'input_tokens': inp, 'output_tokens': out}}})
d = os.path.join(home, '.claude', 'projects', '-cost')
os.makedirs(d, exist_ok=True)
if 'init' in what:
    open(os.path.join(d, 's-cost-1.jsonl'), 'w').write(
        line({'type': 'session-owner', 'accountUuid': 'u-alice-0001', 'organizationUuid': 'org-1', 'sessionId': 's-cost-1'})
        + line({'type': 'user', 'timestamp': iso(now - 2 * 86400), 'message': {'role': 'user', 'content': 'SECRET-PROMPT-TEXT'}})
        + reply(now - 2 * 86400, 'mc1', 'claude-opus-5-5', 1000, 100)
        + reply(now - 86400, 'mc2', 'claude-sonnet-5-5[1m]', 2000, 1000) + reply(now - 86400, 'mc2', 'claude-sonnet-5-5[1m]', 2000, 1000)
        + reply(now - 3600, 'mc3', 'claude-new-9', 10, 10))
    open(os.path.join(d, 's-cost-0.jsonl'), 'w').write(reply(now - 3 * 3600, 'mc4', 'claude-opus-5-5', 1000, 100, 's-cost-0'))   # 没有 session-owner（旧版本）
    x = os.path.join(home, '.codex', 'sessions', '2026', '11', '12')
    os.makedirs(x, exist_ok=True)
    ev = lambda t, kind, p: line({'timestamp': iso(t), 'type': kind, 'payload': p})
    u = {'input_tokens': 10000, 'cached_input_tokens': 8000, 'output_tokens': 500, 'reasoning_output_tokens': 0, 'total_tokens': 10500}
    tc = {'type': 'token_count', 'info': {'total_token_usage': u, 'last_token_usage': u}}
    open(os.path.join(x, 'rollout-cost.jsonl'), 'w').write(
        ev(now - 7200, 'session_meta', {'id': 'th-cost', 'creator_user_id': 'user-dave-0001', 'model_provider': 'openai'})
        + ev(now - 7200, 'turn_context', {'model': 'gpt-6-astra'}) + ev(now - 7100, 'event_msg', tc) + ev(now - 7099, 'event_msg', tc))
if 'more' in what:
    open(os.path.join(d, 's-cost-1.jsonl'), 'a').write(reply(now - 600, 'mc5', 'claude-opus-5-5', 1000, 100))
if 'dave' in what:   # dave 的 id_token 里补上用户 id（真的 id_token 有 chatgpt_user_id）
    p = sys.argv[-1]
    j = json.load(open(p))
    h, c, sg = j['tokens']['id_token'].split('.')
    claims = json.loads(base64.urlsafe_b64decode(c + '=' * (-len(c) % 4)))
    claims['https://api.openai.com/auth']['chatgpt_user_id'] = 'user-dave-0001'
    j['tokens']['id_token'] = '.'.join([h, base64.urlsafe_b64encode(json.dumps(claims).encode()).decode().rstrip('='), sg])
    json.dump(j, open(p, 'w'))
PY
}
costreq() { python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if sum(r[1] for r in j["acc"]["claude:u-alice-0001"].values()) == int(sys.argv[2]) else 1)' "$H/.cca/cost/local.json" "$1" 2>/dev/null; }   # 统计好的 alice 有这么多次请求
costlog "$H" init dave "$DD/codex/auth.json"
out=$(gj state)
ok "_gui state：还没统计过：账号的 cost 是 null，cost_total 说还没统计；Claude、Codex 有这个功能（features.cost），官方账号 can.cost，第三方 API 账号没有" 'printf "%s" "$out" | jok "d[\"cost_total\"][\"scan\"] == {\"state\": \"never\", \"updated_epoch\": None} and d[\"tools\"][0][\"features\"][\"cost\"] is True and d[\"tools\"][1][\"features\"][\"cost\"] is True and all(a[\"cost\"] is None and a[\"can\"][\"cost\"] == (a[\"kind\"] == \"oauth\") for t in d[\"tools\"] for a in t[\"accounts\"])" && [ ! -e "$H/.cca/cost" ]'
ok "_gui hello：cost 是 1（App 据此定时运行 cca cost sync，读 state 里的 cost、cost_total）" 'gj hello | jok "d[\"cost\"] == 1"'
out=$(run cca cost sync --json 2>/dev/null </dev/null); rc=$?; printf '%s' "$out" > "$GF/cost_sync.json"
ok "cost sync --json：第一次全读会话记录，算进来 5 次请求（同一条回复写了两行的只算一次）；没开设备统计时不同步" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"version\"] == 1 and d[\"kind\"] == \"cost_sync\" and d[\"scan\"][\"state\"] == \"ready\" and d[\"scan\"][\"first\"] is True and d[\"scan\"][\"records\"] == 5 and isinstance(d[\"scan\"][\"updated_epoch\"], float) and d[\"sync\"] == {\"state\": \"skipped\", \"message\": None, \"checked_epoch\": None, \"hubs\": []}"'
ok "统计好的只给自己读（cost/ 700，里面的文件 600），只有用量的数字，没有会话的内容" '[ "$(stat -f %Lp "$H/.cca/cost")" = 700 ] && [ "$(stat -f %Lp "$H/.cca/cost/local.json")" = 600 ] && [ "$(stat -f %Lp "$H/.cca/cost/scan.json")" = 600 ] && ! grep -rq SECRET-PROMPT-TEXT "$H/.cca/cost"'
s2=$(snap); out=$(gj state); printf '%s' "$out" > "$GF/state_cost.json"
ok "_gui state：alice 三次请求（一次没有标价）折合 0.02 美元、dave 0.053；只有这台设备（没开设备统计），按周 13 项、按天 30 项；读 state 不写文件" '[ "$(snap)" = "$s2" ] && printf "%s" "$out" | jok "(lambda a, x: a[\"summary\"][\"days_usd\"] == 0.02 and a[\"summary\"][\"requests\"] == 3 and a[\"summary\"][\"unpriced_requests\"] == 1 and a[\"unpriced_models\"] == [\"claude-new-9\"] and [(v[\"id\"], v[\"me\"]) for v in a[\"devices\"]] == [(cfg.get(\"device_id\") or \"local\", True)] and a[\"sync\"][\"state\"] == \"local_only\" and len(a[\"weeks\"]) == 13 and len(a[\"days\"]) == 30 and a[\"currency\"] == \"USD\" and x[\"summary\"][\"days_usd\"] == 0.053)(acct(0, \"alice@example.com\")[\"cost\"], acct(1, \"dave@example.com\")[\"cost\"])"'
ok "_gui state：cost_total 把各账号加起来、按工具分开；认不出是谁的（没有 session-owner 的旧会话）另算" 'printf "%s" "$out" | jok "(lambda t: t[\"days_usd\"] == 0.073 and t[\"by_tool\"][\"claude\"][\"days_usd\"] == 0.02 and t[\"by_tool\"][\"codex\"][\"days_usd\"] == 0.053 and dict(t[\"unattributed\"], why=[w[\"code\"] for w in t[\"unattributed\"][\"why\"]]) == {\"days_usd\": 0.006, \"requests\": 1, \"why\": [\"unknown\"], \"claude_code\": None, \"claude_code_old\": False} and t[\"scan\"][\"state\"] == \"ready\" and len(t[\"days\"]) == 30)(d[\"cost_total\"])"'
ok "_gui state：按天也有平均（average_day_usd、average_days；账号、设备都有，设备的平均按整个账号的那几周、那几天算）；cost_total.me 是各账号里这台设备的那一份（只有这台设备时和顶层一样）" 'printf "%s" "$out" | jok "(lambda t, a: set(t[\"me\"]) == {\"current_usd\", \"weeks_usd\", \"average_usd\", \"average_day_usd\", \"days_usd\", \"today_usd\", \"days\", \"by_tool\"} and all(t[\"me\"][k] == t[k] for k in t[\"me\"]) and \"average_day_usd\" in t and all(k in a[\"summary\"] for k in (\"average_day_usd\", \"average_days\")) and all(v[\"summary\"][k] == a[\"summary\"][k] for v in a[\"devices\"] for k in (\"average_weeks\", \"average_days\", \"average_day_usd\")))(d[\"cost_total\"], acct(0, \"alice@example.com\")[\"cost\"])"'
ok "_gui state：cost_total.devices（App 的设备菜单）只有这台设备（没开设备统计；id 是 config.json 的 device_id，没有时是 local），数字和 me 一样；账号里的设备都标着用过（used）" 'printf "%s" "$out" | jok "(lambda t: [(x[\"id\"], x[\"me\"], x[\"revoked\"], x[\"stale\"]) for x in t[\"devices\"]] == [(cfg.get(\"device_id\") or \"local\", True, False, False)] and all(t[\"devices\"][0][k] == t[\"me\"][k] for k in t[\"me\"]))(d[\"cost_total\"]) and all(x[\"used\"] is True for t in d[\"tools\"] for a in t[\"accounts\"] if a[\"cost\"] for x in a[\"cost\"][\"devices\"])"'
ok "_gui state：设备的诊断（PROTOCOL 2.2 的 devices[].diag）：这台设备的 cca 版本、命令行里最近用的 Claude Code（会话记录里没写版本：null）、认不出账号的原因（没有 session-owner、也没有会话开始的记录：unknown，没有条数）" 'printf "%s" "$out" | jok "(lambda g: set(g) == {\"cca\", \"claude_code\", \"claude_code_old\", \"why\"} and g[\"cca\"] == \"$VER\" and g[\"claude_code\"] is None and g[\"claude_code_old\"] is False and [sorted(w) for w in g[\"why\"]] == [[\"claude_code\", \"code\", \"last_epoch\"]] and g[\"why\"][0][\"code\"] == \"unknown\" and isinstance(g[\"why\"][0][\"last_epoch\"], float))(acct(0, \"alice@example.com\")[\"cost\"][\"devices\"][0][\"diag\"])"'
costlog "$H" more
run CCA_COST= CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1
for i in $(seq 1 100); do costreq 4 && break; sleep 0.1; done
ok "命令行会话结束后在后台统计一次（cca _cost-sync，最多 5 分钟一次）：新的请求算进来了" 'costreq 4 && [ -f "$H/.cca/cost/last" ] && gj state | jok "acct(0, \"alice@example.com\")[\"cost\"][\"summary\"][\"requests\"] == 4"'
python3 -c 'import os,sys,time; t=time.time()-60; os.utime(sys.argv[1], (t, t))' "$H/.cca/cost/last"   # 上次起是 1 分钟以前
run CCA_COST= CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1
ok "后台统计最多 5 分钟一次：1 分钟以前起过就不再起" '[ $(( $(date +%s) - $(stat -f %m "$H/.cca/cost/last") )) -ge 50 ]'
python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["cost"]=False; json.dump(c, open(p, "w"), indent=2, ensure_ascii=False)' "$H/.cca/config.json"
out=$(runev cca cost sync --json 2>/dev/null); rc=$?; out2=$(gj state)
ok "config.json 里写着 \"cost\": false：cost sync 不统计（退出码 2，fail 事件 cost_off），_gui state 没有折合金额" '[ $rc -eq 2 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cost_off\"" && printf "%s" "$out2" | jok "d[\"cost_total\"] is None and all(a[\"cost\"] is None and a[\"can\"][\"cost\"] is False for t in d[\"tools\"] for a in t[\"accounts\"])"'
python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop("cost"); json.dump(c, open(p, "w"), indent=2, ensure_ascii=False)' "$H/.cca/config.json"
for i in $(seq 1 50); do [ -f "$H/.cca/cost/worker.json" ] || break; sleep 0.1; done   # 后台的统计跑完了（跑完删掉 worker.json）
mv "$H/.cca/cost" "$T/cost.dir"; : > "$H/.cca/cost"
out=$(runev cca cost sync --json 2>/dev/null); rc=$?
rm -f "$H/.cca/cost"; mv "$T/cost.dir" "$H/.cca/cost"
ok "统计不成（~/.cca/cost 不是目录）：退出码 1，fail 事件 cost_unavailable，stdout 不出 JSON" '[ $rc -eq 1 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cost_unavailable\""'
cp -p "$T/cost.aj.json" "$AJ"; cp -p "$T/cost.dave.json" "$DD/codex/auth.json"
fi
if sect 10h; then
# 10h. 提问会话（macos/PROTOCOL.md 4.6）：App 开的封闭流程，提问发成 ask 事件，答案从 fd 3 的管道读（tests/gui_session.py 扮演 App）
sess() {   # sess 答案JSON [驱动的参数…] -- 命令…：带答案管道运行，stdin 接 /dev/null（App 就是这样开流程的）
	local a="$1"; shift
	: > "$T/ev.jsonl"
	printf '%s' "$a" > "$T/sess.answers"
	run CCA_EVENTS="$T/ev.jsonl" CCA_PROMPT=1 python3 -I "$D/gui_session.py" "$T/ev.jsonl" "$T/sess.answers" "$@" </dev/null
}
asks() { python3 -I -c 'import json,sys; print(" ".join(e["q"] for e in map(json.loads, open(sys.argv[1])) if e.get("ev") == "ask"))' "$T/ev.jsonl"; }   # 问过的题号，按顺序
lastres() { python3 -I -c 'import json,sys; print(([e["code"] for e in map(json.loads, open(sys.argv[1])) if e.get("ev") == "result"] or [""])[-1])' "$T/ev.jsonl"; }   # 最后一个 result 事件的 code（App 照它收场）
evbefore() { python3 -I -c 'import json,sys; ev=[e["ev"] + ":" + str(e.get("code")) for e in map(json.loads, open(sys.argv[1])) if e.get("ev") in ("fail", "result")]; a, b = sys.argv[2:4]; sys.exit(0 if a in ev and b in ev and ev.index(a) < ev.index(b) else 1)' "$T/ev.jsonl" "$1" "$2"; }   # evbefore fail:码 result:码：两条都有、前一条在前（做完了、带着提醒：提醒在结果前面）
tsum() { shasum < "$H/.cca/accounts.json"; }
ok "提问会话：hello 说明支持（App 据此决定用不用它）；forms：配置路由器、本机 Clash、账号表能在 App 里填表" 'gj hello | jok "d[\"prompt\"] == 1 and d[\"forms\"] == 1"'
out=$(run CCA_EVENTS="$T/ev.jsonl" CCA_PROMPT=1 cca add 2>&1 </dev/null); rc=$?
ok "提问会话：设了 CCA_PROMPT 却没有答案管道：和非交互一样，不提问" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "需要在终端里交互运行"'
N2=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["nodes"][1])' "$H/.cca/config.json")
before=$(tsum)
out=$(sess '{}' --eof-at add.basic -- cca add 2>&1); rc=$?
ok "提问会话：中途取消（App 关掉答案管道）：账号表不变，退出码不是 0，原因是 cancelled" '[ $rc -ne 0 ] && [ "$(tsum)" = "$before" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
out=$(sess '{"add.basic":[{"email":"x@example.com","alias":""}]}' --wrong-field add.basic -- cca add 2>&1); rc=$?
ok "提问会话：一页的答案字段对不上：停下，不把值放错地方" '[ $rc -ne 0 ] && [ "$(tsum)" = "$before" ]'
out=$(sess '{"add.basic":[{"email":"x@example.com","alias":""}],"add.node":[{"v":"1"}]}' --wrong-field add.node -- cca add 2>&1); rc=$?
ok "提问会话：只有一个字段的题，字段对不上也停下，不干等" '[ $rc -ne 0 ] && [ "$(tsum)" = "$before" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
a='{"add.basic":[{"email":"不是邮箱","alias":"图一"},{"email":"Gui1@Example.com","alias":"图一"}],"add.node":[{"v":"2"}]}'
out=$(sess "$a" --stale -- cca add 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_add.jsonl"
ok "提问会话：添加官方账号：邮箱和别名一页，节点一页；邮箱不合格带着原因重问，已填的带回去；过期的重复答案被丢弃" '[ $rc -eq 0 ] && [ "$(asks)" = "add.basic add.basic add.node" ] && evq "e.get(\"seq\") == 2 and e[\"fields\"][0][\"error\"] and e[\"fields\"][0][\"value\"] == \"不是邮箱\" and e[\"fields\"][1][\"value\"] == \"图一\"" && tblok "t.get(\"图一(gui1@example.com)\") == \"$N2\""'
ok "提问会话：节点的选项有编号、节点名和“已被谁使用”，结果带账号的键" 'evq "e.get(\"q\") == \"add.node\" and e[\"fields\"][0][\"kind\"] == \"choice\" and e[\"fields\"][0][\"options\"][1][\"value\"] == \"2\" and e[\"fields\"][0][\"options\"][1][\"label\"] == \"$N2\" and any(\"已被\" in o[\"note\"] for o in e[\"fields\"][0][\"options\"])" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"added\" and e[\"key\"] == \"图一(gui1@example.com)\" and e[\"tool\"] == \"claude\""'
S1=sk-GUI-PROMPT-SENTINEL-0123456789 S2=sk-GUI-PROMPT-SENTINEL2-9876543210
: > env.log; : > security.log; : > ssh.log; : > curl.log; : > open.log; : > claude.log
a='{"api.basic":[{"alias":"gui2","url":"https://api.example.com/anthropic","key":"S3NT","model":""},{"alias":"gui2","url":"https://api.example.com/anthropic","key":"'$S1'","model":"m-1"}],"api.node":[{"v":"1"}]}'
out=$(sess "$a" -- cca add api 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_add_api.jsonl"
KC2=$(echo "$T"/keychain/cca-api-gui2-*)
ok "提问会话：添加第三方 API 账号：一页填完，key 不合格时带着原因重问（填过的 key 不带回去），选直连，key 进了钥匙串" '[ $rc -eq 0 ] && [ "$(asks)" = "api.basic api.basic api.node" ] && evq "e.get(\"seq\") == 2 and e[\"fields\"][2][\"kind\"] == \"secret\" and e[\"fields\"][2][\"error\"] and not e[\"fields\"][2][\"value\"] and e[\"fields\"][1][\"value\"] == \"https://api.example.com/anthropic\"" && [ "$(cat "$KC2/api.example.com")" = "$S1" ] && tblok "t[\"gui2\"][\"model\"] == \"m-1\" and \"node\" in t[\"gui2\"] and not t[\"gui2\"][\"node\"]"'
ok "提问会话：key 不进事件文件、输出和任何命令行（假 security / ssh / curl / open / claude / env 都记着命令行）" '! printf "%s" "$out" | grep -qF "$S1" && ! grep -qF "$S1" "$T/ev.jsonl" env.log security.log ssh.log curl.log open.log claude.log && ! grep -qF S3NT "$T/ev.jsonl"'
out=$(sess '{"key.secret":[{"s":"'$S2'"}]}' -- cca key gui2 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_key.jsonl"
ok "提问会话：换 key：一个密文框，存进钥匙串，key 不进事件文件" '[ $rc -eq 0 ] && [ "$(asks)" = key.secret ] && [ "$(cat "$KC2/api.example.com")" = "$S2" ] && ! grep -qF "$S2" "$T/ev.jsonl" env.log security.log && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"key_saved\" and e[\"key\"] == \"gui2\""'
AAID=$(basename "$(ls -d "$H"/.cca/accounts/alice-example-com-*)"); K2=sk-ant-api03-SESSref0123456789abcdJKLM
out=$(sess '{"key.secret":[{"s":"'$K2'"}]}' -- cca key alice 2>&1); rc=$?
ok "提问会话：官方账号记一把 API key：同一个密文框（页上说明 cca 不拿它启动），存进 cca-ref- 条目，key 不进事件文件和命令行" '[ $rc -eq 0 ] && [ "$(asks)" = key.secret ] && evq "e.get(\"q\") == \"key.secret\" and \"记录\" in e[\"title\"] and \"不会拿它登录或启动\" in e.get(\"message\", \"\")" && [ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$K2" ] && ! grep -qF "$K2" "$T/ev.jsonl" env.log security.log && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"key_saved\" and e[\"key\"] == \"alice@example.com\""'
run CCA_GRANT="key_forget:$AAID" cca key alice --forget >/dev/null 2>&1
out=$(sess '{"rm.confirm":[{"v":"n"}]}' -- cca rm gui2 2>&1); rc=$?
ok "提问会话：删除账号答否：什么都不动，没有结果事件" '[ $rc -eq 0 ] && tblok "\"gui2\" in t" && [ "$(evn result)" = 0 ] && evq "e.get(\"q\") == \"rm.confirm\" and e[\"fields\"][0][\"default\"] == \"n\" and e[\"fields\"][0][\"severity\"] == \"warn\""'
out=$(sess '{"rm.confirm":[{"v":"y"}]}' -- cca rm gui2 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_rm.jsonl"
ok "提问会话：删除账号答是：账号和钥匙串里的 key 都删了" '[ $rc -eq 0 ] && tblok "\"gui2\" not in t" && [ ! -e "$KC2/api.example.com" ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"removed\""'
out=$(sess '{"rm.confirm":[{"v":"y"}]}' -- cca rm 图一 2>&1); rc=$?
ok "提问会话：删除官方账号" '[ $rc -eq 0 ] && tblok "not any(\"gui1@\" in k for k in t)"'
# 配置向导（_onboard，App 首次运行引导：① 维护者的映射 → ② 网络出口 → ③ 添加账号 → ④ 自己的同步服务器；做完记下 onboarded，以后不再弹）。
# 平时的用例里 CCA_POLICY_DEFAULT 是空的：没有映射那一步（下面单独测）；这里都没加入自己的同步服务器：都有第 4 步（答有、已经加入了的在 11b3c）
cp "$H/.cca/config.json" "$T/cfg.onb.bak"; cp "$H/.cca/accounts.json" "$T/tbl.onb.bak"
out=$(sess '{}' -- cca _onboard --skip 2>&1); rc=$?
ok "配置向导：--skip（App「以后再说」）只记下 onboarded、不提问" '[ $rc -eq 0 ] && [ -z "$(asks)" ] && gj state | jok "cfg.get(\"onboarded\") is True and d[\"config\"][\"onboarded\"] is True"'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
onb_before=$(tsum)
out=$(sess '{"onboard.hub":[{"v":"n"}]}' -- cca _onboard 2>&1); rc=$?
ok "配置向导：路由器已配好、已有账号 → 只问有没有自己的同步服务器的加入链接（只有这一步：标题不写第几步，默认没有）；答没有：记下 onboarded、给完成事件、不动账号表" '[ $rc -eq 0 ] && [ "$(asks)" = onboard.hub ] && evq "e.get(\"q\") == \"onboard.hub\" and e[\"title\"] == \"自己的同步服务器\" and e[\"fields\"][0][\"kind\"] == \"confirm\" and e[\"fields\"][0][\"default\"] == \"n\"" && [ "$(tsum)" = "$onb_before" ] && gj state | jok "cfg.get(\"onboarded\") is True" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"onboarded\"" && [ ! -e "$H/.cca/hub" ]'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
HL9=$(python3 -I -c 'import base64,sys; sys.path.insert(0, sys.argv[1]); import spec_wire as sw; print(sw.link_encode("https://127.0.0.1:9", ["sha256//" + base64.b64encode(bytes(32)).decode()], "5a" * 16, "c3" * 16))' "$D")
out=$(sess "{\"onboard.hub\":[{\"v\":\"y\"}],\"hub.join\":[{\"v\":\"$HL9\"}]}" -- cca _onboard 2>&1); rc=$?
ok "配置向导第 4 步答有、加入没成（服务器连不上）：不重问，向导照样做完、记下 onboarded；没加入成的原因留作提醒（fail 事件），最后的结果还是 onboarded" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.hub hub.join" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_unreachable\"" && [ "$(lastres)" = onboarded ] && [ ! -e "$H/.cca/hub/hub.json" ] && gj state | jok "cfg.get(\"onboarded\") is True" && ! grep -qF "$HL9" "$T/ev.jsonl"'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
out=$(sess "{\"onboard.hub\":[{\"v\":\"y\"}],\"hub.join\":[{\"v\":\"${HL9%???}\"},{\"v\":\"\"}]}" -- cca _onboard 2>&1); rc=$?
ok "配置向导第 4 步答有、贴的链接不对：带着原因重问（说留空是先不加入）；留空交上来：先不加入，向导照样做完，没有失败事件" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.hub hub.join hub.join" ] && evq "e.get(\"q\") == \"hub.join\" and e[\"fields\"][0][\"optional\"] is True and \"留空\" in e[\"fields\"][0][\"hint\"]" && evq "e.get(\"seq\") == 3 and e[\"fields\"][0][\"error\"].endswith(\"（先不加入的话留空，点继续）\")" && ! evq "e[\"ev\"] == \"fail\"" && [ "$(lastres)" = onboarded ] && [ ! -e "$H/.cca/hub/hub.json" ] && gj state | jok "cfg.get(\"onboarded\") is True"'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
out=$(sess '{"onboard.share":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_POLICY_DEFAULT=example/cca-policy cca _onboard 2>&1); rc=$?
ok "配置向导：已有账号、答没有维护者共享的账号（账号表没变，不问第 3 步）：前后的页「共几步」一样，最后一页是第 3 步（共 3 步）" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.share onboard.hub" ] && evq "e.get(\"q\") == \"onboard.share\" and e[\"title\"].startswith(\"第 1 步（共 3 步）\")" && evq "e.get(\"q\") == \"onboard.hub\" and e[\"title\"].startswith(\"第 3 步（共 3 步）\")"'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
out=$(sess '{"onboard.net":[{"v":"skip"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_GUI_OS=26 cca _onboard 2>&1); rc=$?
ok "配置向导：没配过路由器、探不到局域网里的路由器、能用内置代理 → 上网方式默认内置代理（自动模式：探不到就内置代理）；选项仍是路由器/内置代理/跳过；选跳过：记下 onboarded、不配置" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.net onboard.hub" ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"fields\"][0][\"kind\"] == \"choice\" and e[\"fields\"][0][\"options\"][0][\"value\"] == \"router\" and e[\"fields\"][0][\"options\"][-1][\"value\"] == \"skip\" and e[\"fields\"][0][\"default\"] == \"builtin\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"onboarded\""'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
out=$(sess '{"onboard.net":[{"v":"skip"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_GUI_OS=15 cca _onboard 2>&1); rc=$?
ok "配置向导：macOS 26 以下也能用内置代理（内核由 cca 在后台无头运行）→ 探不到路由器时默认也选内置代理" '[ $rc -eq 0 ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"fields\"][0][\"default\"] == \"builtin\""'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
echo "192.168.9.1:9090" > "$T/clash_router"   # 假局域网：192.168.9.1:9090 上有个 Clash 控制口
out=$(sess '{"onboard.net":[{"v":"router"}]}' --eof-at router.basic -- env CCA_GATEWAY=192.168.9.1 cca _onboard 2>&1); rc=$?
rm -f "$T/clash_router"
ok "配置向导：自动探到局域网里装了 Clash 的路由器 → 上网方式默认「路由器」、页说明带探到的地址，选路由器后配置表单里地址已预填" '[ "$(asks)" = "onboard.net router.basic" ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"fields\"][0][\"default\"] == \"router\" and \"192.168.9.1\" in e.get(\"message\", \"\")" && evq "e.get(\"q\") == \"router.basic\" and e[\"fields\"][0][\"value\"] == \"192.168.9.1\""'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
out=$(sess '{"onboard.net":[{"v":"router"}]}' --eof-at router.basic -- cca _onboard 2>&1); rc=$?
ok "配置向导：选「路由器」→ 接着走配置路由器表单（router.basic）；在那页取消就不记 onboarded" '[ $rc -ne 0 ] && [ "$(asks)" = "onboard.net router.basic" ] && gj state | jok "not cfg.get(\"onboarded\")"'
cp "$T/cfg.onb.bak" "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"
out=$(sess '{"onboard.add":[{"v":"y"}]}' --eof-at add.basic -- cca _onboard 2>&1); rc=$?
cp "$T/tbl.onb.bak" "$H/.cca/accounts.json"; cp "$T/cfg.onb.bak" "$H/.cca/config.json"
ok "配置向导：路由器配好、还没有账号 → 问加不加账号（confirm），答是接着走 add.basic（在那页取消就不加）" '[ $rc -ne 0 ] && [ "$(asks)" = "onboard.add add.basic" ] && evq "e.get(\"q\") == \"onboard.add\" and e[\"fields\"][0][\"kind\"] == \"confirm\" and e[\"fields\"][0][\"default\"] == \"y\""'
# 没有路由器笔记本：向导「没有路由器（内置代理）」。有可选节点 → 配好内置代理、接着加账号；没有可选节点 → 加账号时粘贴自定义代理（下面）
cp "$H/.cca/config.json" "$T/cfg.b2.bak"; cp "$H/.cca/accounts.json" "$T/tbl.b2.bak"; [ -f "$H/.cca/codex.json" ] && cp "$H/.cca/codex.json" "$T/cdx.b2.bak"
b2_had_policy=0; [ -d "$H/.cca/policy" ] && { b2_had_policy=1; rm -rf "$T/policy.b2"; cp -R "$H/.cca/policy" "$T/policy.b2"; }
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"; [ -f "$H/.cca/codex.json" ] && printf '{}' > "$H/.cca/codex.json"   # 端口池清空(Claude+Codex)→ LOCAL_SLOTS=0,core_setup 早返回不下载/不起内核
out=$(sess '{"onboard.net":[{"v":"builtin"}],"onboard.add":[{"v":"y"}]}' --eof-at add.basic -- cca _onboard 2>&1); rc=$?
ok "没有路由器笔记本：选「没有路由器（内置代理）」、有可选节点 → 配好内置代理（local.client=builtin）、接着问加账号（onboard.net 第二项就是它）" '[ $rc -ne 0 ] && [ "$(asks)" = "onboard.net onboard.add add.basic" ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"fields\"][0][\"options\"][1][\"value\"] == \"builtin\"" && gj state | jok "cfg.get(\"local\", {}).get(\"client\") == \"builtin\""'
# 没有路由器、没有可选节点、没有映射：加账号不要求先配置路由器（以前会接着走配置路由器的表单），选节点那步可以粘贴自定义代理
cp "$T/cfg.b2.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; c["nodes"]=[]; c.pop("node_ids", None); c.pop("policy", None); json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"; [ -f "$H/.cca/codex.json" ] && printf '{}' > "$H/.cca/codex.json"
rm -rf "$H/.cca/policy"
out=$(sess '{"onboard.net":[{"v":"builtin"}],"onboard.add":[{"v":"y"}],"add.basic":[{"email":"nr@example.com","alias":""}]}' --eof-at add.node -- cca _onboard 2>&1); rc=$?
ok "没有路由器、没有可选节点、没有映射：选内置代理后加账号不要求配置路由器，选节点那步只有「自定义代理…」，页上说明怎么办" '[ $rc -ne 0 ] && [ "$(asks)" = "onboard.net onboard.add add.basic add.node" ] && evq "e.get(\"q\") == \"add.node\" and [o[\"label\"][:5] for o in e[\"fields\"][0][\"options\"]] == [\"自定义代理\"] and \"还没有可选的出口节点\" in e.get(\"message\", \"\")" && ! evq "e.get(\"q\") == \"router.basic\"" && gj state | jok "cfg.get(\"local\", {}).get(\"client\") == \"builtin\""'
# 第 1 步：维护者共享的账号（有默认仓库时才有这一步）：先问有没有（默认没有，答没有就不生成 deploy key、不拉）
cp "$T/cfg.b2.bak" "$H/.cca/config.json"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; c.pop("policy", None); c.pop("policy_declined", None); json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"; [ -f "$H/.cca/codex.json" ] && printf '{}' > "$H/.cca/codex.json"
rm -rf "$H/.cca/policy"; touch "$T/policy_denied"; : > git.log; rm -f "$T/clipboard"
out=$(sess '{"onboard.share":[{"v":"n"}],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_POLICY_DEFAULT=example/cca-policy CCA_POLICY_POLL=1 cca _onboard 2>&1); rc=$?
ok "配置向导第 1 步（有维护者的默认仓库）：先问有没有维护者共享给你的账号（默认没有）；答没有：不生成 deploy key、不拉、不记仓库，接着第 2 步（共 4 步），最后是第 4 步自己的同步服务器" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.share onboard.net onboard.add onboard.hub" ] && evq "e.get(\"q\") == \"onboard.share\" and e[\"title\"].startswith(\"第 1 步（共 4 步）\") and e[\"fields\"][0][\"kind\"] == \"confirm\" and e[\"fields\"][0][\"default\"] == \"n\"" && evq "e.get(\"q\") == \"onboard.net\" and e[\"title\"].startswith(\"第 2 步（共 4 步）\")" && evq "e.get(\"q\") == \"onboard.hub\" and e[\"title\"] == \"第 4 步（共 4 步）：自己的同步服务器\"" && cfgok "not c.get(\"policy\") and c.get(\"onboarded\") is True" && [ ! -e "$H/.cca/policy/id_ed25519" ] && ! grep -q "^GIT clone" git.log'
out=$(sess '{"onboard.share":[{"v":"y"}],"onboard.policy":[{"next":"skip"}],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_POLICY_DEFAULT=example/cca-policy CCA_POLICY_POLL=1 cca _onboard 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_onboard.jsonl"
ok "答有：同一步接着给维护者运行的那一行（只显示，已复制到剪贴板）、正在等的说明，和「先跳过 / 不打开共享」；标题还是第 1 步（共 4 步）" '[ "$(asks)" = "onboard.share onboard.policy onboard.net onboard.add onboard.hub" ] && evq "e.get(\"q\") == \"onboard.policy\" and e[\"title\"].startswith(\"第 1 步（共 4 步）\") and e[\"fields\"][0][\"kind\"] == \"code\" and e[\"fields\"][0][\"value\"].startswith(\"./policy.sh key add \") and \"正在等维护者加上\" in e[\"fields\"][0][\"hint\"] and e[\"fields\"][1][\"kind\"] == \"choice\" and e[\"fields\"][1][\"default\"] == \"skip\" and [o[\"value\"] for o in e[\"fields\"][1][\"options\"]] == [\"skip\", \"decline\"] and \"已复制到剪贴板\" in e[\"message\"]" && grep -q "^\./policy\.sh key add " "$T/clipboard"'
ok "配置向导第 1 步先跳过：记下仓库（共享开着）、留着 deploy key（App 之后自动拉），接着问网络出口（第 2 步）、加账号（第 3 步）、自己的同步服务器（第 4 步），做完记下 onboarded" '[ $rc -eq 0 ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"title\"].startswith(\"第 2 步（共 4 步）\")" && evq "e.get(\"q\") == \"onboard.add\" and e[\"title\"].startswith(\"第 3 步（共 4 步）\") and e[\"fields\"][0][\"default\"] == \"y\"" && cfgok "c.get(\"policy\") == \"example/cca-policy\" and c.get(\"onboarded\") is True" && [ -f "$H/.cca/policy/id_ed25519" ] && grep -q "^GIT clone" git.log && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"onboarded\""'
out=$(sess '{"onboard.policy":[{"next":"decline"}],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_POLICY_DEFAULT=example/cca-policy CCA_POLICY_POLL=1 cca _onboard 2>&1); rc=$?
ok "共享开着、还没拉到：不再问有没有，直接是那一页；选「不打开共享」：去掉仓库、删掉 deploy key（还是同一把：没加上），接着到第 2 步" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.policy onboard.net onboard.add onboard.hub" ] && cfgok "not c.get(\"policy\") and c.get(\"policy_declined\") is True" && [ ! -e "$H/.cca/policy/id_ed25519" ] && [ "$(grep -c "^GIT clone" git.log)" -ge 2 ]'
out=$(sess '{"onboard.share":[{"v":"n"}],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env CCA_POLICY_DEFAULT= cca _onboard 2>&1); rc=$?
ok "不知道维护者的仓库（没有默认仓库、也没关掉过共享）：没有第 1 步（不问也不拉），网络出口是第 1 步（共 3 步）" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.net onboard.add onboard.hub" ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"title\"].startswith(\"第 1 步（共 3 步）\")" && [ ! -e "$H/.cca/policy/id_ed25519" ]'
rm -f "$T/policy_denied"
cp "$T/cfg.b2.bak" "$H/.cca/config.json"; cp "$T/tbl.b2.bak" "$H/.cca/accounts.json"; [ -f "$T/cdx.b2.bak" ] && cp "$T/cdx.b2.bak" "$H/.cca/codex.json"
rm -rf "$H/.cca/policy"; [ "$b2_had_policy" = 1 ] && cp -R "$T/policy.b2" "$H/.cca/policy"
unlisten 7901; n=$(reloads)
out=$(sess '{}' --eof-at consent.router_reload -- cca router sync 2>&1); rc=$?
ok "提问会话：默认“是”的同意，问的时候 App 关掉管道：按取消，不重载" '[ $rc -ne 0 ] && [ "$(reloads)" = "$n" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
out=$(sess '{"consent.router_reload":[{"v":"n"}]}' -- cca router sync 2>&1); rc=$?
ok "提问会话：同意答否：不重载" '[ "$(reloads)" = "$n" ] && [ "$(evn consent)" = 0 ]'
out=$(sess '{"consent.router_reload":[{"v":"y"}]}' -- cca router sync 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_consent.jsonl"
ok "提问会话：同意当场问（不用重跑），带标题和完整的说明（路由器的说明有好几行，最后一行也在），答是就重载" '[ $rc -eq 0 ] && [ "$(reloads)" = "$((n + 1))" ] && evq "e.get(\"q\") == \"consent.router_reload\" and e[\"title\"] and e[\"fields\"][0][\"kind\"] == \"confirm\" and \"约 20 秒\" in e[\"fields\"][0][\"detail\"] and e[\"fields\"][0][\"default\"] == \"y\""'
mv "$H/.cca/router_ed25519" "$T/rk.bak"
before=$(tsum)
out=$(sess '{}' -- cca add 2>&1); rc=$?
ok "提问会话：添加账号不要求路由器能用（路由器的密钥不在了）：有可选节点就直接填账号，不先配置路由器；取消时什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = add.basic ] && [ "$(evn terminal)" = 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\"" && [ "$(tsum)" = "$before" ]'
cp "$H/.cca/config.json" "$T/cfg.rk.bak"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["nodes"]=[]; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
out=$(sess '{}' -- cca add 2>&1); rc=$?
cp "$T/cfg.rk.bak" "$H/.cca/config.json"; mv "$T/rk.bak" "$H/.cca/router_ed25519"
ok "提问会话：配置过路由器、只是现在用不了，又一个可选节点都没有：先重新配置路由器（说明为什么），取消时什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = router.basic ] && evq "e.get(\"q\") == \"router.basic\" and \"还没有可选的出口节点\" in e[\"message\"]" && [ "$(evn terminal)" = 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\"" && [ "$(tsum)" = "$before" ]'
# 配置路由器（router.basic / router.password / router.nodes）：地址不通的重问；密码经 askpass 交给 ssh，不进命令行、环境变量、事件文件；
# 取消时不盖掉原来的配置
cp "$H/.cca/accounts.json" "$T/tbl.bak"; cp "$H/.cca/config.json" "$T/cfg.bak"; RPW='rtr-PW-SENTINEL-1 with space'
cfg_router() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["router"]["port"])' "$H/.cca/config.json"; }
printf '%s' "$RPW" > "$T/router_needs_password"
: > ssh.log; : > env.log
out=$(sess '{"router.basic":[{"host":"-oProxyCommand=x","user":"root","port":"22"},{"host":"192.0.2.10","user":"bad user","port":"0"},{"host":"192.0.2.10","user":"root","port":"2222"}]}' -- cca router setup 2>&1); rc=$?
ok "提问会话：配置路由器：地址、用户、端口一页（预填现在的），不合格的带着原因重问，连不上的说明白；取消时 config.json 不变" '[ $rc -ne 0 ] && [ "$(asks)" = "router.basic router.basic router.basic router.basic" ] && evq "e.get(\"seq\") == 1 and e[\"fields\"][0][\"value\"] == \"192.0.2.10\" and e[\"fields\"][1][\"value\"] == \"root\" and e[\"fields\"][2][\"value\"] == \"22\"" && evq "e.get(\"seq\") == 2 and \"不带\" in e[\"fields\"][0][\"error\"]" && evq "e.get(\"seq\") == 3 and e[\"fields\"][1][\"error\"] and e[\"fields\"][2][\"error\"]" && evq "e.get(\"seq\") == 4 and \"连不上 192.0.2.10:2222\" in e[\"fields\"][0][\"error\"]" && [ "$(cfg_router)" = 22 ] && cmp -s "$H/.cca/config.json" "$T/cfg.bak"'
out=$(sess '{"router.basic":[{"host":"192.0.2.10","user":"root","port":"22"}],"router.password":[{"v":"wrong-one"}]}' -- cca router setup 2>&1); rc=$?
ok "提问会话：配置路由器：密码不对带着原因重问，取消时公钥没装、config.json 不变" '[ $rc -ne 0 ] && [ "$(asks)" = "router.basic router.password router.password" ] && evq "e.get(\"seq\") == 3 and e[\"fields\"][0][\"kind\"] == \"secret\" and \"密码不对\" in e[\"fields\"][0][\"error\"] and not e[\"fields\"][0][\"value\"]" && [ -f "$T/router_needs_password" ] && cmp -s "$H/.cca/config.json" "$T/cfg.bak"'
touch "$T/askpass_twice"; : > ssh.log
out=$(sess '{"router.basic":[{"host":"192.0.2.10","user":"root","port":"22"}],"router.password":[{"v":"'"$RPW"'"}],"router.nodes":[{"v":"1"},{"v":"1 2 3 4"}]}' -- cca router setup 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_router.jsonl"
rm -f "$T/askpass_twice"
ok "提问会话：配置路由器：密码经 askpass 交给 ssh，读完就没了（ssh 再问一遍也不会卡住）；公钥装上了" '[ $rc -eq 0 ] && [ ! -f "$T/router_needs_password" ] && grep -q "^ASKPASS rc=0" ssh.log && grep -q "^ASKPASS2 rc=1" ssh.log && grep -q "cca@" "$R/root/.ssh/authorized_keys"'
ok "提问会话：密码不进事件文件、输出、任何命令行（假 ssh / env 都记着命令行），临时目录不留" '! grep -qF "$RPW" "$T/ev.jsonl" ssh.log env.log && ! printf "%s" "$out" | grep -qF "$RPW" && ! ls "$T"/tmp/cca-askpass.* >/dev/null 2>&1 && ! ls /tmp/cca-askpass.* >/dev/null 2>&1'
ok "提问会话：配置路由器：节点多选一页（现在的预先选好），还有账号在用的不能去掉，说明是谁在用；配好了有结果事件" '[ "$(asks)" = "router.basic router.password router.nodes router.nodes" ] && evq "e.get(\"q\") == \"router.nodes\" and e[\"fields\"][0][\"kind\"] == \"multi\" and e[\"fields\"][0][\"min\"] == 1 and e[\"fields\"][0][\"default\"] and e[\"fields\"][0][\"options\"][0][\"label\"] == \"US-A\"" && evq "e.get(\"seq\") == 4 and \"还有账号在用\" in e[\"fields\"][0][\"error\"]" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"router_ready\" and \"root@192.0.2.10\" in e[\"message\"]"'
out=$(sess '{"router.basic":[{"host":"192.0.2.10","user":"root","port":"22"}],"router.nodes":[{"v":"1 2 3 4"}]}' -- cca router setup 2>&1); rc=$?
ok "提问会话：配置路由器：密钥已经能登录时不问密码，可选节点没变也不多同步一次" '[ $rc -eq 0 ] && [ "$(asks)" = "router.basic router.nodes" ] && [ "$(evn result)" = 1 ]'
cp "$T/tbl.bak" "$H/.cca/accounts.json"; cp "$T/cfg.bak" "$H/.cca/config.json"
# 配置本机出口（local.builtin）：问用不用内置代理
out=$(sess '{"local.builtin":[{"v":"n"}]}' -- cca local setup 2>&1); rc=$?
ok "提问会话：配置本机出口：问用不用内置代理，答否按取消收场，什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = local.builtin ] && evq "e.get(\"q\") == \"local.builtin\" and e[\"fields\"][0][\"kind\"] == \"confirm\"" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\"" && cmp -s "$H/.cca/config.json" "$T/cfg.bak"'
run cca local off >/dev/null 2>&1
cp "$T/tbl.bak" "$H/.cca/accounts.json"; cp "$T/cfg.bak" "$H/.cca/config.json"
# 账号表表单（edit.form）：节点、接口地址、模型、环境变量；cca 校验，不显示 key；表坏了才去终端
cp "$H/.cca/accounts.json" "$T/tbl.bak"; cp "$H/.cca/config.json" "$T/cfg.bak"
edit_table 'j["form1"] = {"base_url": "https://api.example.com/anthropic", "model": "m-1", "node": "", "env": {"FOO_LEVEL": 3, "BAR_NAME": "a b"}, "key": "sk-FORM-TABLE-KEY-0123456789", "id": "form1"}'
: > ssh.log
out=$(sess '{}' --eof-at edit.form -- cca edit 2>&1); rc=$?; tb=$(tsum)
fid() { python3 -I -c 'import json,sys; f=[x for e in map(json.loads, open(sys.argv[1])) if e.get("q") == "edit.form" for x in e["fields"] if x["label"] == sys.argv[2] and (not sys.argv[3] or sys.argv[3] in x.get("section", ""))]; print(f[0]["id"])' "$T/ev.jsonl" "$1" "${2:-}"; }
U=$(fid 接口地址 form1); M=$(fid "模型（可选）" form1); NO=$(fid 出口 form1); EV=$(fid "环境变量（可选）" form1); AL=$(fid "alice@example.com")
ok "提问会话：账号表表单：一页里每个账号一行节点菜单，第三方 API 账号有地址、模型、出口、环境变量，保存按钮；不显示 key；取消什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = edit.form ] && evq "e.get(\"q\") == \"edit.form\" and e[\"submit\"] == \"保存\"" && evq "e.get(\"q\") == \"edit.form\" and any(f[\"id\"] == \"$AL\" and f[\"kind\"] == \"choice\" and f[\"style\"] == \"menu\" and f[\"value\"] for f in e[\"fields\"])" && evq "e.get(\"q\") == \"edit.form\" and any(f[\"id\"] == \"$EV\" and f[\"value\"] == \"FOO_LEVEL=3 BAR_NAME=\x27a b\x27\" and \"form1\" in f[\"section\"] for f in e[\"fields\"])" && evq "e.get(\"q\") == \"edit.form\" and any(f[\"id\"] == \"$NO\" and f[\"optional\"] and f[\"skip\"] for f in e[\"fields\"])" && ! grep -qF "sk-FORM-TABLE-KEY" "$T/ev.jsonl" && [ "$tb" = "$(tsum)" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
out=$(sess '{"edit.form":[{}]}' --keep -- cca edit 2>&1); rc=$?
ok "提问会话：账号表表单：什么都没改：账号表一个字节不动，没有结果事件" '[ $rc -eq 0 ] && [ "$tb" = "$(tsum)" ] && [ "$(evn result)" = 0 ]'
a='{"edit.form":[{"'$U'":"http://api.example.com/x","'$M'":"bad model","'$EV'":"MY_API_KEY=1"},{"'$U'":"https://api.example.com/anthropic","'$M'":"m-2","'$NO'":"US-B","'$EV'":"FOO_LEVEL=3 BAR_NAME=\u0027a b\u0027 NEW_ONE=x"}]}'
out=$(sess "$a" --keep -- cca edit 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_edit.jsonl"
ok "提问会话：账号表表单：不合格的写在对应的字段下面（已填的带回去），都合格才写" '[ $rc -eq 0 ] && [ "$(asks)" = "edit.form edit.form" ] && evq "e.get(\"seq\") == 2 and any(f[\"id\"] == \"$U\" and \"https\" in f[\"error\"] and f[\"value\"] == \"http://api.example.com/x\" for f in e[\"fields\"])" && evq "e.get(\"seq\") == 2 and any(f[\"id\"] == \"$M\" and f[\"error\"] for f in e[\"fields\"])" && evq "e.get(\"seq\") == 2 and any(f[\"id\"] == \"$EV\" and \"密钥\" in f[\"error\"] for f in e[\"fields\"])"'
ok "提问会话：账号表表单：保存后改好了（模型、出口、环境变量），key 不动，分到了端口，同步了路由器；有结果事件" 'tblok "t[\"form1\"][\"model\"] == \"m-2\" and t[\"form1\"][\"node\"] == \"US-B\" and t[\"form1\"][\"env\"] == {\"FOO_LEVEL\": \"3\", \"BAR_NAME\": \"a b\", \"NEW_ONE\": \"x\"} and t[\"form1\"][\"key\"].startswith(\"sk-FORM\")" && cfgok "\"form1\" in c[\"ports\"]" && grep -q "sh -s" ssh.log && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"saved\"" && ! grep -qF "sk-FORM-TABLE-KEY" "$T/ev.jsonl"'
KC3=$(echo "$T"/keychain/cca-api-form1-*); S3=sk-FORM-NEW-KEY-9876543210
a='{"edit.form":[{"'$U'":"https://api2.example.com/anthropic"}],"key.secret":[{"s":"'$S3'"}]}'
out=$(sess "$a" --keep -- cca edit 2>&1); rc=$?
KC3=$(echo "$T"/keychain/cca-api-form1-*)
ok "提问会话：账号表表单：换了服务商（域名变了）：账号表里旧的 key 删掉，当场问新的 key，存进钥匙串（不进事件文件）" '[ $rc -eq 0 ] && [ "$(asks)" = "edit.form key.secret" ] && tblok "\"key\" not in t[\"form1\"] and t[\"form1\"][\"base_url\"] == \"https://api2.example.com/anthropic\"" && [ "$(cat "$KC3/api2.example.com")" = "$S3" ] && ! grep -qF "$S3" "$T/ev.jsonl" env.log security.log'
EN=$(fid "alice@example.com")
out=$(sess '{"edit.form":[{"'$EN'":"US-D"}]}' --keep -- cca edit 2>&1); rc=$?
ok "提问会话：账号表表单：改官方账号的节点" '[ $rc -eq 0 ] && tblok "t[\"alice@example.com\"] == \"US-D\""'
out=$(sess '{}' --eof-at edit.form -- cca codex edit 2>&1); rc=$?
ok "提问会话：账号表表单：ChatGPT（Codex）的账号表一样能改，没有环境变量这一项" '[ "$(asks)" = edit.form ] && evq "e.get(\"q\") == \"edit.form\" and any(\"dave@example.com\" in f[\"label\"] and f[\"kind\"] == \"choice\" for f in e[\"fields\"]) and not any(f[\"label\"].startswith(\"环境变量\") for f in e[\"fields\"])"'
edit_table 'j["alice@example.com"] = "NOT-ALLOWED"'
out=$(sess '{}' --eof-at edit.form -- cca edit 2>&1); rc=$?
ok "提问会话：账号表表单：节点不在可选列表：表单里要重选（没有预选值，说明原因），不用去终端" '[ "$(asks)" = edit.form ] && [ "$(evn terminal)" = 0 ] && evq "e.get(\"q\") == \"edit.form\" and any(f[\"id\"] == \"$EN\" and not f[\"value\"] and \"NOT-ALLOWED\" in f[\"error\"] for f in e[\"fields\"])"'
edit_table 'j["bad key with space"] = "US-A"'
out=$(sess '{}' -- cca edit 2>&1); rc=$?
ok "提问会话：账号表表单：表单改不了的问题（键写错了）：一个问题都不问，交给终端" '[ $rc -ne 0 ] && [ "$(evn ask)" = 0 ] && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"edit\" and e[\"argv\"] == [\"edit\"]"'
cp "$H/.cca/accounts.json" "$T/tbl.broken"; printf '{不是 JSON' > "$H/.cca/accounts.json"
out=$(sess '{}' -- cca edit 2>&1); rc=$?
ok "提问会话：账号表表单：账号表不是 JSON：交给终端" '[ $rc -ne 0 ] && [ "$(evn ask)" = 0 ] && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"edit\""'
cp "$T/tbl.bak" "$H/.cca/accounts.json"; cp "$T/cfg.bak" "$H/.cca/config.json"; rm -rf "$T"/keychain/cca-api-form1-*; run CCA_YES=1 cca router sync -y >/dev/null 2>&1 </dev/null
# 节点在路由器上不见了（也认不出改成了哪个）：选一个代替，没有默认项，可以先不换
cp "$H/.cca/accounts.json" "$T/tbl.bak"; cp "$H/.cca/config.json" "$T/cfg.bak"
edit_table 'j["图四(gui4@example.com)"] = "US-D"'; run cca router sync >/dev/null 2>&1 </dev/null
node_rn US-D 192.0.2.4 US-Z9 192.0.2.44; oc_restart
out=$(sess '{"node.replace":[{"v":""}]}' -- cca router sync 2>&1); rc=$?
pick=$(python3 -I -c 'import json,sys; print(next(o["value"] for e in map(json.loads, open(sys.argv[1])) if e.get("q") == "node.replace" for o in e["fields"][0]["options"] if o["label"] == "US-B"))' "$T/ev.jsonl")
ok "提问会话：节点不见了：选择题，没有默认项，可以先不换（账号表不变）" 'evq "e.get(\"q\") == \"node.replace\" and e[\"fields\"][0][\"kind\"] == \"choice\" and e[\"fields\"][0][\"optional\"] and e[\"fields\"][0][\"skip\"] and not e[\"fields\"][0][\"default\"] and \"US-D\" in e[\"message\"]" && tblok "t.get(\"图四(gui4@example.com)\") == \"US-D\""'
out=$(sess '{"node.replace":[{"v":"'$pick'"}]}' -- cca router sync 2>&1); rc=$?
ok "提问会话：节点不见了：选了哪个就换成哪个" 'tblok "t.get(\"图四(gui4@example.com)\") == \"US-B\""'
cp "$T/tbl.bak" "$H/.cca/accounts.json"; cp "$T/cfg.bak" "$H/.cca/config.json"
node_rn US-Z9 192.0.2.44 US-D 192.0.2.4; oc_restart; run CCA_YES=1 cca router sync -y >/dev/null 2>&1 </dev/null
lint_prompts() {   # 提问只能经基元：别的地方直接读 fd 3，图形界面就收不到这个问题；每个题号都要写进 PROTOCOL 的映射表
	python3 -I - "$CCA" "$D/../macos/PROTOCOL.md" <<'PY'
import re, sys
src = open(sys.argv[1], encoding='utf-8').read()
doc = open(sys.argv[2], encoding='utf-8').read()
allowed = {'ask', 'ask_secret', 'arrow_menu', 'gui_ask', 'gui_answer', 'gui_answer_wait', 'policy_wait', 'cmd_edit', 'node_read_block'}
bad, fn = [], ''
for line in src.split('\n'):
    m = re.match(r'([A-Za-z_][A-Za-z0-9_]*)\(\) \{', line)
    if m:
        fn = m.group(1)
    if '<&3' in line and ': <&3' not in line and not line.lstrip().startswith('#') and fn not in allowed:
        bad.append(fn)
qs = set(re.findall(r'\bq=([a-z_]+\.[a-z_.]+)', src)) | set(re.findall(r'ask_fields ([a-z]+\.[a-z_.]+) ', src))
missing = sorted(q for q in qs if '`%s`' % q not in doc)
print('ok' if not bad and not missing and qs else '：读 fd 3 的 %s；PROTOCOL 里没写的题号 %s' % (bad, missing))
PY
}
lp=$(lint_prompts)
ok "lint：提问只经基元（别处不读 fd 3），每个题号都在 PROTOCOL 的映射表里${lp#ok}" '[ "$lp" = ok ]'
lint_ifaces() {   # 虚拟网卡的名单：bash 的 VIRTUAL_IFACE_RE（at_home）和辅助程序的（内置代理的出站绑定，cli/src/cmd/corenet/bind.rs）是同一份
	python3 -I - "$CCA" "$D/../cli/src/cmd/corenet/bind.rs" <<'PY'
import re, sys
b = re.search(r"^VIRTUAL_IFACE_RE='([^']*)'$", open(sys.argv[1], encoding='utf-8').read(), re.M)
# 等号后面可以折行（rustfmt 排的：这一行超过行宽）
p = re.search(r'^pub const VIRTUAL_IFACE_RE: &str =\s*r"([^"]*)";$', open(sys.argv[2], encoding='utf-8').read(), re.M)
print('ok' if b and p and b.group(1) == p.group(1) else '：bash 的 %s，辅助程序的 %s' % (b and b.group(1), p and p.group(1)))
PY
}
lv=$(lint_ifaces)
ok "虚拟网卡的名单：bash 的 VIRTUAL_IFACE_RE 和辅助程序的一样${lv#ok}" '[ "$lv" = ok ]'
if command -v swiftc >/dev/null 2>&1 && [ -f "$D/../macos/test.sh" ]; then
	ok "App 的契约测试：Swift 模型解码这一节 cca 的真实输出（macos/test.sh）" 'bash "$D/../macos/test.sh" "$GF" > "$T/contract.log" 2>&1'
	S3=sk-GUI-PROMPT-SENTINEL3-1357924680
	printf '%s' '{"api.basic":[{"alias":"gui3","url":"https://api.example.com/anthropic","key":"'$S3'","model":""}],"api.node":[{"v":"1"}],"rm.confirm":[{"v":"y"}]}' > "$T/sess.answers"
	: > env.log; : > security.log
	out=$(run "$D/../macos/build/contract-tests" --session-smoke "$H/.local/bin/cca" "$T/sess.answers" add api 2>&1 </dev/null); rc=$?
	KC3=$(echo "$T"/keychain/cca-api-gui3-*)
	ok "提问会话：App 真正的答案管道（ProcessTransport）对真的 cca：添加第三方 API 账号，key 进了钥匙串，不进命令行" '[ $rc -eq 0 ] && [ "$(cat "$KC3/api.example.com")" = "$S3" ] && ! grep -qF "$S3" env.log security.log && ! printf "%s" "$out" | grep -qF "$S3"'
	run "$D/../macos/build/contract-tests" --session-smoke "$H/.local/bin/cca" "$T/sess.answers" rm gui3 >/dev/null 2>&1 </dev/null
	ok "提问会话：真的答案管道：删除账号" 'tblok "\"gui3\" not in t"'
	# 界面里的终端（PROTOCOL 9.3）：App 真正的伪终端（ProcessTransport.startTerminal）对真的 cca remote attach。会话是直接起在宿主里的
	# 一个小脚本（先说一句、之后收到一行回一行），启动它的终端是 script 开的。rt.end 出现了脚本自己退出、会话结束（不能指望 script
	# 读到头时送的 ^D：宿主把这个终端设成了原始模式，script 只在行模式下送）
	rm -rf "$H/.cca/remote" "$T"/rt.*   # 会话目录先清空：下面取到的一定是这一个会话
	{ for i in $(seq 1 600); do [ -f "$T/rt.end" ] && break; sleep 0.1; done; } |
		script -q "$T/rt.log" env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_HELPER="$HELPERWRAP" PATH="/usr/bin:/bin" TERM=dumb "$HELPER_BIN" remote-host claude alice@example.com alice@example.com -- \
		/bin/bash -c 'echo SMOKE-READY; while [ ! -e "$CCA_T/rt.end" ]; do IFS= read -r -t 1 l && echo "got:$l"; done; exit 0' >/dev/null 2>&1 &
	rt_bg=$!
	rt_id=""; for i in $(seq 1 150); do rt_id=$(run cca remote list --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; s=json.load(sys.stdin)["sessions"]; print(s[0]["id"] if s else "")' 2>/dev/null); [ -n "$rt_id" ] && break; sleep 0.1; done
	run "$D/../macos/build/contract-tests" --terminal-smoke "$H/.local/bin/cca" "$rt_id" "$T/rt.stop" SMOKE-READY ping-from-app got:ping-from-app > "$T/rt.out" 2>&1 </dev/null & rt_sm=$!
	for i in $(seq 1 200); do run cca remote list --json 2>/dev/null </dev/null | jok 'd["sessions"][0]["viewers"] == 1' && break; kill -0 "$rt_sm" 2>/dev/null || break; sleep 0.1; done
	ok "界面里的终端：App 的伪终端上运行 cca remote attach，接上了：会话多了一处接入（启动它的终端也还在）" 'run cca remote list --json 2>/dev/null </dev/null | jok "len(d[\"sessions\"]) == 1 and d[\"sessions\"][0][\"id\"] == \"$rt_id\" and d[\"sessions\"][0][\"viewers\"] == 1 and d[\"sessions\"][0][\"local_terminal\"] is True"'
	touch "$T/rt.stop"; wait "$rt_sm"; rc=$?
	# 冒烟程序退出码 0：先看到了接入之前的输出（SMOKE-READY），App 里敲的那一行会话收到并回了（got:…，只有会话里的脚本会这样回），断开后接入端退出码 0
	ok "界面里的终端：先看到之前的输出，App 里敲的字会话收到、回的字 App 看得到；App 断开（给进程组发 SIGTERM）：接入端退出码 0，会话照常运行，没有人接着了" '[ $rc -eq 0 ] && grep -q "^exit=0 bytes=[1-9]" "$T/rt.out" && run cca remote list --json 2>/dev/null </dev/null | jok "len(d[\"sessions\"]) == 1 and d[\"sessions\"][0][\"viewers\"] == 0"'
	touch "$T/rt.end"
	for i in $(seq 1 100); do kill -0 "$rt_bg" 2>/dev/null || break; sleep 0.1; done
	kill "$rt_bg" 2>/dev/null; wait "$rt_bg" 2>/dev/null   # 10 秒还没走的不等了（下面那条会挂，后面的章节照常）
	ok "会话结束：列表里没有了" 'run cca remote list --json 2>/dev/null </dev/null | jok "d[\"sessions\"] == []"'
	rm -rf "$H/.cca/remote" "$T"/rt.*
else
	echo "SKIP  App 的契约测试（没有 swiftc 或 macos/test.sh）"
fi

fi
if sect 11; then
# 11. 文字和编码
out=$(run cca router sync 2>&1); m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "UTF-8 下中文提示完整" 'printf "%s" "$out" | grep -q "正在同步账号到路由器 192.0.2.10" && printf "%s" "$m" | grep -q "继续使用 alice@example.com" && printf "%s%s" "$out" "$m" | python3 -I -c "import sys; sys.stdin.buffer.read().decode(\"utf-8\")"'
lint_dollar() {   # $变量 紧跟非 ASCII 字符：macOS 的 bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名，要写 ${变量}。
	# 查发出去的 bash：cca、install.sh、release.sh、policy.sh、自己的同步服务器的安装脚本（服务器上的 sh 可能是 bash）、macos 下的脚本，
	# 还有 App 里拼出来的 bash（Swift 源码，比如终端交接脚本）
	python3 -I - "$D/.." "$CCA" install.sh release.sh policy.sh server/hub-install.sh 'macos/*.sh' macos/Sources macos/tools <<'PY'
import glob, os, re, sys
root, files = sys.argv[1], []
for a in sys.argv[2:]:
    ps = sorted(glob.glob(os.path.join(root, a)))
    if not ps and '*' not in a:
        print('：没有 %s' % a)
        sys.exit()
    for p in ps:
        if os.path.isdir(p):
            files += sorted(os.path.join(r, f) for r, _, fs in os.walk(p) for f in fs if f.endswith(('.sh', '.swift')))
        else:
            files.append(p)
bad = ['%s:%d' % (os.path.relpath(f, root), i) for f in files
       for i, line in enumerate(open(f, encoding='utf-8'), 1) if re.search(r'\$[A-Za-z_][A-Za-z0-9_]*(?=[^\x00-\x7f])', line)]
print('ok' if not bad else '：' + '、'.join(bad))
PY
}
ld=$(lint_dollar)
ok "lint：cca、install.sh、release.sh、policy.sh、server/hub-install.sh、macos 的脚本和 App 里拼的 bash 都没有 \$变量 紧跟非 ASCII 字符${ld#ok}" '[ "$ld" = ok ]'
lint_helper_env() {   # 回归里自己拼环境运行 cca 的地方（不经 run 的那几处）都要带着 CCA_HELPER="$HELPERWRAP"：不带的话那一处用的是
	# 仓库里 cli/target/release 下的那一份，release.sh 跑回归时（CCA_HELPER 指着要发布的那一份）它可能是以前编的、甚至没有。
	# 运行 policy.sh 的那一处不算（它不运行 cca）
	local bad
	bad=$(grep -n 'env -i ' "$D/regress.sh" | grep -v 'CCA_HELPER=' | grep -v 'bash "\$PS"' | cut -d: -f1 | tr '\n' ' ')
	[ -z "$bad" ] && echo ok || echo "：第 ${bad}行"
}
lh=$(lint_helper_env)
ok "lint：回归里自己拼环境运行 cca 的地方都带着 CCA_HELPER（release.sh 跑回归时，整套用的才都是要发布的那一份辅助程序）${lh#ok}" '[ "$lh" = ok ]'
out=$(run cca router status 2>&1)
ok "router status：列出槽位、端口和选中节点" 'printf "%s" "$out" | grep -q "槽位 CCA-1 :7901 监听中 → US-A" && printf "%s" "$out" | grep -q "槽位 CCA-2 :7902 监听中 → REJECT"'

fi
if sect 11a2; then
# 11a2. 多设备共用一个路由器（owners 认所有权；只在 macOS 上）：现在路由器上只有 alice@槽位 1（owner = 这台）
cp "$R/etc/cca/accounts.yaml" "$T/md_acc.bak"; cp "$R/etc/cca/owners" "$T/md_own.bak"
cp "$H/.cca/config.json" "$T/md_cfg.bak"; cp "$H/.cca/accounts.json" "$T/md_tbl.bak"
cp "$R/etc/dropbear/authorized_keys" "$T/md_keys.bak" 2>/dev/null
deva=$(cfgv 'c.get("device_id")')
# 直接写路由器的 accounts.yaml + owners，扮演"另一台设备"占了槽位 2（owner bbbb…，节点 US-B）
python3 - "$R/etc/cca/accounts.yaml" "$R/etc/cca/owners" <<'PY'
import json, sys
acc = json.load(open(sys.argv[1])); acc['slots'] = max(acc.get('slots', 0), 8)
acc['accounts'] = [a for a in acc['accounts'] if a['slot'] != 2] + [{'slot': 2, 'name': 'carol-dev2', 'node': 'US-B'}]
acc['nodes'] = list(dict.fromkeys(acc['nodes'] + ['US-B']))
json.dump(acc, open(sys.argv[1], 'w'), ensure_ascii=False)
own = json.load(open(sys.argv[2])); own['2'] = 'bbbb1111bbbb1111'; json.dump(own, open(sys.argv[2], 'w'))
PY
edit_table 'j["bob@example.com"] = "US-C"'
out=$(run CCA_YES=1 cca router sync -y 2>&1)
ok "多设备共用：同步保留别的设备的账号（carol@2），自己的 bob 排到空槽位 3，不互相覆盖" '[ "$(sel CCA-2)" = US-B ] && [ "$(sel CCA-3)" = US-C ] && grep -q "carol-dev2" "$R/etc/cca/accounts.yaml" && python3 -c "import json,sys; o=json.load(open(sys.argv[1])); sys.exit(0 if o.get(\"2\")==\"bbbb1111bbbb1111\" and o.get(\"3\")==sys.argv[2] else 1)" "$R/etc/cca/owners" "$deva"'
# 卸载按 owner：路由器还有别的设备（carol）→ 只撤这台（alice、bob），保留 carol，/etc/cca 不整体删
out=$(run CCA_YES=1 CCA_TTY="$(ans y)" cca router uninstall 2>&1)
ok "多设备共用：卸载时路由器还有别的设备，只撤这台的账号、保留别的设备的（carol），不整体拆除" 'grep -q "carol-dev2" "$R/etc/cca/accounts.yaml" && ! grep -q "alice-example-com" "$R/etc/cca/accounts.yaml" && ! grep -q "bob-example-com" "$R/etc/cca/accounts.yaml" && [ -e "$R/etc/cca" ] && printf "%s" "$out" | grep -q "还有别的设备在用"'
# 还原路由器和账号表，后面的用例照旧
cp "$T/md_acc.bak" "$R/etc/cca/accounts.yaml"; cp "$T/md_own.bak" "$R/etc/cca/owners"
cp "$T/md_cfg.bak" "$H/.cca/config.json"; cp "$T/md_tbl.bak" "$H/.cca/accounts.json"
[ -f "$T/md_keys.bak" ] && cp "$T/md_keys.bak" "$R/etc/dropbear/authorized_keys"
run CCA_YES=1 cca router sync -y >/dev/null 2>&1

fi
if sect 11b; then
# 11b. 自动更新：假的 GitHub Releases + 测试用发布密钥
G="$T/gh"; mkdir -p "$G"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$T/relkey"; ssh-keygen -q -t ed25519 -N '' -f "$T/evilkey"
TESTPUB=$(cut -d' ' -f1,2 "$T/relkey.pub")
mkver() {   # mkver 版本 输出文件 [路由器脚本标记]：换成测试密钥、改版本号，可在路由器脚本里加个标记
	# 内置代理的内核换成假的 mihomo（setup.sh 打包、算好的校验值）。像发布的那样接着辅助程序（只接这台机器的那一份；$T/cca.packed
	# 是仓库里的 cca 接好的：第一次用、或者 cca、辅助程序比它新（分节续跑时快照里那份是旧的）时做一份，之后只在它上面改这几行）
	{ [ "$T/cca.packed" -nt "$CCA" ] && [ "$T/cca.packed" -nt "$HELPER_BIN" ]; } || python3 -I "$D/../cli/pack.py" "$CCA" "$T/cca.packed" --only "$HOST_TARGET=$HELPER_BIN" >/dev/null || return 1
	sed -e "s|^VERSION=\".*\"$|VERSION=\"$1\"|" -e "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" \
		-e "s|^CORE_SHA256_ARM64=\".*\"$|CORE_SHA256_ARM64=\"$(cat "$T/mihomo/sha256")\"|" -e "s|^CORE_SHA256_AMD64=\".*\"$|CORE_SHA256_AMD64=\"$(cat "$T/mihomo/sha256")\"|" "$T/cca.packed" > "$2"
	[ -n "${3:-}" ] && sed -i '' "s|^# cca-router：|# cca-router（$3）：|" "$2"
	chmod 755 "$2"
}
publish() {   # publish 版本 [路由器脚本标记] [签名密钥]；测试版（-beta.N）像 GitHub 的 pre-release，不成为 latest
	mkdir -p "$G/v$1"; mkver "$1" "$G/v$1/cca" "${2:-}"; rm -f "$G/v$1/cca.sig"
	ssh-keygen -Y sign -q -n cca-release -f "${3:-$T/relkey}" "$G/v$1/cca" 2>/dev/null
	case "$1" in *-beta.*) ;; *) echo "$1" > "$G/latest" ;; esac
}
wait_next() { local i; for i in $(seq 1 50); do [ -f "$H/.cca/update/cca.next" ] && [ ! -d "$H/.cca/update/lock" ] && return 0; sleep 0.2; done; return 1; }
wait_log() { local i; for i in $(seq 1 50); do grep -q "$1" "$H/.cca/update/log" 2>/dev/null && [ ! -d "$H/.cca/update/lock" ] && return 0; sleep 0.2; done; return 1; }   # 后台检查不下载时：等它在日志里写下原因（不用等满 wait_next 的 10 秒）
ver() { run cca version; }
if [ -e "$(dirname "$CCA")/.git" ]; then
	out=$(run bash "$CCA" update 2>&1)
	ok "开发模式（git 仓库里的 cca）：不自动更新" 'printf "%s" "$out" | grep -q "开发模式" && [ ! -e "$G/latest" ] && ! grep -q "GH " gh.log 2>/dev/null'
fi
# 换代的那一次更新：还内嵌着 Python 的旧版本（cli/PORTED_FROM 记的那个主干提交的 cca）用它自己的更新流程装上接着辅助程序的
# 新版本——下载、验签（旧版本要求新文件 bash -n 通过、有 VERSION 那一行）、换上；新版本第一次运行自己解出辅助程序。在另一个 HOME 里
# 做（不动下面用的更新状态）。旧版本的 cca 从 git 里取（CCA_OLD_CCA=文件 可以另指一份）；取不到（不在 git 仓库里）就跳过。
# 等没有还内嵌 Python 的版本在用了，这个用例就可以删掉
OLDCCA="$T/old-cca"
if [ -n "${CCA_OLD_CCA:-}" ]; then cp "$CCA_OLD_CCA" "$OLDCCA"
else git -C "$D/.." show "$(cut -d' ' -f1 "$D/../cli/PORTED_FROM" 2>/dev/null):cca" > "$OLDCCA" 2>/dev/null || rm -f "$OLDCCA"; fi
if [ -s "$OLDCCA" ] && grep -q "^PY_SRC=" "$OLDCCA"; then
	OH="$T/oldhome"; mkdir -p "$OH/.cca/bin" "$G/v9.9.1"
	oldv=$(sed -n 's/^VERSION="\(.*\)"$/\1/p' "$OLDCCA")
	sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" "$OLDCCA" > "$OH/.cca/bin/cca"; chmod 755 "$OH/.cca/bin/cca"
	mkver 9.9.1 "$G/v9.9.1/cca"; ssh-keygen -Y sign -q -n cca-release -f "$T/relkey" "$G/v9.9.1/cca" 2>/dev/null
	ro() { run HOME="$OH" CCA_HELPER= "$@"; }   # 旧版本用 PATH 里的 python3；新版本自己解辅助程序（不指定）
	v0=$(ro "$OH/.cca/bin/cca" version 2>&1)
	out=$(ro "$OH/.cca/bin/cca" update 9.9.1 2>&1); rc=$?
	out2=$(ro "$OH/.cca/bin/cca" _gui hello 2>/dev/null)
	ok "换代的更新：内嵌 Python 的旧版本（${oldv}）自己更新到接着辅助程序的新版本，新版本第一次运行解出辅助程序、照常工作，旧版本留作回退" '[ "$v0" = "$oldv" ] && [ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已更新到 9.9.1（原来是 ${oldv}）" && [ "$(ro "$OH/.cca/bin/cca" version)" = 9.9.1 ] && grep -q "^HELPER_SUMS=\"$HOST_TARGET=" "$OH/.cca/bin/cca" && ! grep -q "^PY_SRC=" "$OH/.cca/bin/cca" && printf "%s" "$out2" | jok "d[\"kind\"] == \"hello\" and d[\"cca_version\"] == \"9.9.1\" and d[\"dev_mode\"] is False" && grep -q "^PY_SRC=" "$OH/.cca/update/cca.prev"'
	out=$(ro "$OH/.cca/bin/cca" update --rollback 2>&1)
	ok "换代的更新：退得回去（cca update --rollback 换回内嵌 Python 的那一版）" '[ "$(ro "$OH/.cca/bin/cca" version)" = "$oldv" ] && printf "%s" "$out" | grep -q "已退回 ${oldv}"'
	rm -rf "$OH" "$G/v9.9.1"
else
	echo "SKIP  换代的更新（取不到内嵌 Python 的旧版本：不在 git 仓库里；CCA_OLD_CCA=文件 可以指一份）"
fi
rm -f "$OLDCCA"
mkdir -p "$T/dl"; mkver 2.0.0 "$T/dl/cca"
run CCA_TTY="$(ans)" bash "$T/dl/cca" install >/dev/null 2>&1
ok "发布版安装：复制到 ~/.cca/bin/cca 并链接，open-url 也指向它" '[ "$(readlink "$H/.local/bin/cca")" = "$H/.cca/bin/cca" ] && grep -q "$H/.cca/bin/cca _browser" "$H/.cca/bin/open-url" && [ "$(ver)" = 2.0.0 ]'
publish 9.9.9
out=$(run cca update --check 2>&1)
ok "update --check：发现新版本但不安装" 'printf "%s" "$out" | grep -q "有新版本 9.9.9（当前 2.0.0）" && [ "$(ver)" = 2.0.0 ]'
out=$(run cca update 2>&1)
ok "update：下载、验签、原子替换，旧版本留作回退" '[ "$(ver)" = 9.9.9 ] && printf "%s" "$out" | grep -q "已更新到 9.9.9（原来是 2.0.0）" && [ -f "$H/.cca/update/cca.prev" ]'
out=$(run cca update --rollback 2>&1)
ok "update --rollback：退回 2.0.0，之后不再自动装 9.9.9" '[ "$(ver)" = 2.0.0 ] && [ "$(cat "$H/.cca/update/skip")" = 9.9.9 ] && printf "%s" "$out" | grep -q "已退回 2.0.0"'
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_log "9.9.9 被回退过"
ok "后台检查：被回退的 9.9.9 不会再下载" '[ ! -f "$H/.cca/update/cca.next" ] && grep -q "9.9.9 被回退过" "$H/.cca/update/log"'
publish 9.9.10 r2
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_next
ok "后台检查：每天一次，下载并校验好放着，这次启动不替换" '[ -f "$H/.cca/update/cca.next" ] && [ "$(ver)" = 2.0.0 ] && [ -f "$H/.cca/update/checked" ]'
n0=$(reloads)
out=$(run cca @alice --version 2>&1); rc=$?
ok "下次启动：自动换上 9.9.10，并用新版本执行这次命令" 'printf "%s" "$out" | grep -q "cca 已自动更新：2.0.0 → 9.9.10" && [ "$(ver)" = 9.9.10 ] && [ $rc -eq 3 ] && lastline ARGS | grep -q -- " --version$"'
ok "新版本改了路由器脚本：启动时自动重新部署，不重载" 'grep -q "cca-router（r2）" "$R/etc/cca/cca-router.sh" && [ "$(reloads)" = "$n0" ] && printf "%s" "$out" | grep -q "路由器端脚本有变化"'
n_gh=$(grep -c "GH latest" gh.log); run cca ls >/dev/null 2>&1
ok "一天之内不重复检查" '[ "$n_gh" = "$(grep -c "GH latest" gh.log)" ] && sleep 1 && [ "$n_gh" = "$(grep -c "GH latest" gh.log)" ]'
publish 9.9.11; echo "# 篡改" >> "$G/v9.9.11/cca"
out=$(run cca update 2>&1)
ok "被篡改的发布：签名不对，拒绝安装" 'printf "%s" "$out" | grep -q "签名校验没通过" && [ "$(ver)" = 9.9.10 ]'
publish 9.9.12 "" "$T/evilkey"
out=$(run cca update 2>&1)
ok "别的密钥签名的发布：拒绝安装" 'printf "%s" "$out" | grep -q "签名校验没通过" && [ "$(ver)" = 9.9.10 ]'
publish 1.0.0
out=$(run cca update 2>&1)
ok "旧版本（防降级）：不安装" 'printf "%s" "$out" | grep -q "已是最新版本 9.9.10" && [ "$(ver)" = 9.9.10 ]'
run cca update notify >/dev/null 2>&1; publish 9.9.13
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_next
out=$(run cca ls 2>&1)
ok "notify：只提示有新版本，不自动安装" 'printf "%s" "$out" | grep -q "cca 有新版本 9.9.13（当前 9.9.10）" && [ "$(ver)" = 9.9.10 ]'
run cca update auto >/dev/null 2>&1
# 测试版：GitHub 上是 pre-release（releases/latest 不指向它），自动更新不装，只能 cca update 版本号 手动装，能退回
run cca ls >/dev/null 2>&1   # auto：换上已经下载好的 9.9.13
publish 9.9.14-beta.1
rm -f "$H/.cca/update/checked" "$H/.cca/update/log"; run cca ls >/dev/null 2>&1
for i in $(seq 1 50); do grep -q "已是最新" "$H/.cca/update/log" 2>/dev/null && break; sleep 0.2; done
ok "测试版：自动更新看不到它（releases/latest 还是正式版），不下载也不装" '[ "$(ver)" = 9.9.13 ] && grep -q "已是最新（9.9.13）" "$H/.cca/update/log" && [ ! -f "$H/.cca/update/cca.next" ]'
out=$(run cca update 9.9.14-beta.1 2>&1)
ok "cca update 版本号：手动装测试版（照样验签），说明之后怎么更新、怎么退回" '[ "$(ver)" = 9.9.14-beta.1 ] && printf "%s" "$out" | grep -q "这是测试版" && printf "%s" "$out" | grep -q "cca update --rollback"'
out=$(run cca update 9.9.12 2>&1); rc=$?
out2=$(run cca update 9.9.14-beta.2 2>&1); rc2=$?
ok "cca update 版本号：不比现在新的不装（退回用 --rollback），没有这个版本时说明" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不比现在的 9.9.14-beta.1 新" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "没有这个版本" && [ "$(ver)" = 9.9.14-beta.1 ]'
out=$(run cca update --rollback 2>&1)
ok "测试版 → 退回原来的正式版" '[ "$(ver)" = 9.9.13 ] && printf "%s" "$out" | grep -q "已退回 9.9.13"'
out=$(run cca update v9.9.14-beta.1 2>&1); rc=$?
ok "cca update 的版本号照 GitHub 上 tag 的样子写成 v9.9.14-beta.1 也认" '[ $rc -eq 0 ] && [ "$(ver)" = 9.9.14-beta.1 ]'
publish 9.9.14-beta.3; printf 'cca 9.9.14-beta.3：修好了一个问题\n\n第二行\n' > "$G/v9.9.14-beta.3/notes"
out=$(run cca update --check 2>&1); out2=$(run cca update --check --json 2>/dev/null)
ok "update --check：装着测试版时也看更新的测试版（告诉你用版本号装，不装），带着更新说明的前几行；--json 是给图形界面的" 'printf "%s" "$out" | grep -q "有新的测试版 9.9.14-beta.3（当前 9.9.14-beta.1），运行 cca update 9.9.14-beta.3 安装" && printf "%s" "$out" | grep -q "│ cca 9.9.14-beta.3：修好了一个问题" && printf "%s" "$out" | grep -q "│ 第二行" && printf "%s" "$out2" | jok "d[\"channel\"] == \"beta\" and d[\"latest\"] == \"9.9.14-beta.3\" and d[\"available\"] is True and d[\"published_epoch\"] > 0" && [ "$(ver)" = 9.9.14-beta.1 ]'
touch "$T/gh_api_down"; out=$(run cca update --check --json 2>/dev/null); rc=$?; rm -f "$T/gh_api_down"
ok "update --check：发布列表拿不到（被限流、连不上 api.github.com）时退回 releases/latest：只看得到正式版，没有更新说明" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"source\"] == \"latest\" and d[\"latest\"] == \"9.9.13\" and d[\"available\"] is False and d[\"notes\"] is None"'
publish 9.9.14
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_next
out=$(run CCA_TTY="$(ans)" cxa @dave --version 2>&1); rc=$?
ok "装了测试版之后，出了更新的正式版照常自动更新；这次是用 cxa 启动的，换上新版本重新执行后启动的照样是 Codex" 'printf "%s" "$out" | grep -q "cca 已自动更新：9.9.14-beta.1 → 9.9.14" && [ "$(ver)" = 9.9.14 ] && [ $rc -eq 3 ] && clast ARGS | grep -q -- " --version$" && clast ENV | grep -q "CODEX_HOME=$DD/codex "'
publish 9.9.15-beta.1; out=$(run cca update --check --json 2>/dev/null); out2=$(run cca update --check 2>&1); rm -rf "$G/v9.9.15-beta.1"
ok "update --check：正式版上只看正式版（更新的 9.9.15-beta.1 不算），已是最新" 'printf "%s" "$out" | jok "d[\"channel\"] == \"stable\" and d[\"latest\"] == \"9.9.14\" and d[\"available\"] is False" && printf "%s" "$out2" | grep -q "已是最新版本 9.9.14"'

fi
if sect 11b1; then
# 11b1. 图形界面 App（cca gui）：Release 附带的 cca-app.zip（命名空间 cca-app 签名），只装和 cca 同版本的；App 开着时先下载放着，
#       退出后再换；cca 更新后，下一次运行 cca 时在后台跟着更新；开发模式从源码编译（这里用假的 build.sh）
GA="$H/Applications/cca.app"
publish_app() {   # publish_app 版本 [签名的命名空间] [Info.plist 里写的版本] [--self-check 的退出码]：假的 cca.app 打包签名，放进假 GitHub
	local d="$T/appsrc/$1"
	rm -rf "$d"; mkdir -p "$d/cca.app/Contents/MacOS"
	cat > "$d/cca.app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.cca.app</string><key>CFBundleExecutable</key><string>cca</string>
<key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>${3:-$1}</string>
</dict></plist>
EOF
	printf '#!/bin/sh\n[ "$1" = --self-check ] && { echo ok; exit %s; }\nsleep 30\n' "${4:-0}" > "$d/cca.app/Contents/MacOS/cca"
	chmod 755 "$d/cca.app/Contents/MacOS/cca"
	codesign --force --sign - "$d/cca.app" 2>/dev/null
	mkdir -p "$G/v$1"; rm -f "$G/v$1/cca-app.zip" "$G/v$1/cca-app.zip.sig"
	( cd "$d" && ditto -c -k --keepParent cca.app "$G/v$1/cca-app.zip" )
	ssh-keygen -Y sign -q -n "${2:-cca-app}" -f "$T/relkey" "$G/v$1/cca-app.zip" 2>/dev/null
}
gver() { plutil -extract CFBundleShortVersionString raw "$GA/Contents/Info.plist" 2>/dev/null; }
refused() { [ $rc -ne 0 ] && printf "%s" "$out" | grep -q "$1" && [ ! -e "$GA" ]; }   # 这次 cca gui 没装：说了原因，App 也没有
: > open.log
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：这个版本的 Release 没附带 App：说明，不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没能装上界面 App" && [ ! -e "$GA" ] && grep -q "GH get v9.9.14/cca-app.zip" gh.log'
publish_app 9.9.14
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：下载和 cca 同版本（9.9.14）的 App，按 cca-app 验签、自检后装进 ~/Applications 并打开" '[ $rc -eq 0 ] && [ "$(gver)" = 9.9.14 ] && grep -qxF "OPEN [$GA] [--args] [--window]" open.log && [ -f "$H/.cca/gui/enabled" ] && [ ! -e "$H/.cca/gui/next" ] && ! ls -d "$H/.cca/gui"/dl.* >/dev/null 2>&1 && printf "%s" "$out" | grep -q "已装好界面 App"'
n_gh=$(grep -c "cca-app.zip" gh.log)
out=$(run cca gui 2>&1)
ok "cca gui：已经是同一个版本：直接打开，不再下载" '[ "$(grep -c "cca-app.zip" gh.log)" = "$n_gh" ] && [ "$(grep -cxF "OPEN [$GA] [--args] [--window]" open.log)" = 2 ]'
out=$(run cca _gui hello 2>/dev/null)
ok "_gui hello：装着的 App 的版本" 'printf "%s" "$out" | jok "d[\"gui\"][\"app_version\"] == \"9.9.14\" and d[\"gui\"][\"staged_version\"] is None and d[\"dev_mode\"] is False"'
rm -rf "$GA"; publish_app 9.9.14; echo "# 篡改" >> "$G/v9.9.14/cca-app.zip"
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：被篡改的 App：签名校验不过，不装" 'refused "签名校验没通过"'
publish_app 9.9.14 cca-release
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：用 cca 本身的命名空间（cca-release）签的 zip 不算：两种签名不能互相顶替" 'refused "签名校验没通过"'
publish_app 9.9.14 cca-app 9.9.13
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：签名对、里面却是别的版本（拿旧版本冒充）：不装" 'refused "不是 9.9.14 版的 cca App"'
publish_app 9.9.14 cca-app "" 1
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：这台 Mac 跑不了这个 App（自检不过）：不装" 'refused "运行不了这个 App"'
mkdir -p "$GA/Contents"; printf '<plist><dict><key>CFBundleIdentifier</key><string>com.example.other</string></dict></plist>' > "$GA/Contents/Info.plist"
publish_app 9.9.14
out=$(run cca gui 2>&1); rc=$?
ok "cca gui：~/Applications 里已经有一个不是 cca 装的 cca.app：不覆盖" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不是 cca 装的" && grep -q com.example.other "$GA/Contents/Info.plist"'
rm -rf "$GA"
out=$(run CCA_GUI_OS=15 cca gui 2>&1); rc=$?
ok "cca gui：macOS 26 以前的系统：说明要 macOS 26，不下载" 'refused "要 macOS 26 或更新"'
run cca gui >/dev/null 2>&1
"$GA/Contents/MacOS/cca" & GUIPID=$!   # App 开着
sleep 0.3
publish 9.9.15; publish_app 9.9.15
run cca update >/dev/null 2>&1; run cca ls >/dev/null 2>&1   # 换上 9.9.15 后的第一次运行：后台下载对应版本的 App
for i in $(seq 1 50); do [ -d "$H/.cca/gui/next/cca.app" ] && [ ! -d "$H/.cca/gui/lock" ] && break; sleep 0.2; done
ok "cca 更新后第一次运行：后台下载同版本的 App；App 开着就先放着，不换" '[ "$(ver)" = 9.9.15 ] && [ "$(gver)" = 9.9.14 ] && [ "$(plutil -extract CFBundleShortVersionString raw "$H/.cca/gui/next/cca.app/Contents/Info.plist")" = 9.9.15 ] && run cca _gui hello 2>/dev/null | grep -q "\"staged_version\":\"9.9.15\""'
( sleep 1; kill $GUIPID 2>/dev/null ) &
out=$(run cca gui update --relaunch $GUIPID 2>&1); rc=$?
wait $GUIPID 2>/dev/null
ok "cca gui update --relaunch：等 App 退出后换上新版本（不再下载）并重新打开" '[ $rc -eq 0 ] && [ "$(gver)" = 9.9.15 ] && [ ! -e "$H/.cca/gui/next" ] && [ "$(grep -c "^GH get v9.9.15/cca-app.zip$" gh.log)" = 1 ] && [ "$(tail -n 1 open.log)" = "OPEN [$GA] [--args] [--window]" ] && printf "%s" "$out" | grep -q "界面 App 已更新到 9.9.15"'
rm -rf "$GA"; publish 9.9.16; publish_app 9.9.16
run cca update >/dev/null 2>&1; run cca ls >/dev/null 2>&1; sleep 1
ok "自己删掉了 App：cca 更新后不再装回来" '[ ! -e "$GA" ] && ! grep -q "v9.9.16/cca-app.zip" gh.log'
run cca gui >/dev/null 2>&1
# App 自己留在 ~/Library 里的（内核日志、窗口状态、设置）跟着删；设置先 defaults delete（假的：真的不看 HOME，见 setup.sh）
mkdir -p "$H/Library/Caches/local.cca.app/runs" "$H/Library/Saved Application State/local.cca.app.savedState" "$H/Library/Preferences"
: > "$H/Library/Caches/local.cca.app/core.log"; : > "$H/Library/Preferences/local.cca.app.plist"; : > "$H/Library/Preferences/com.example.other.plist"; : > "$T/defaults.log"
out=$(run cca gui remove 2>&1); rc=$?
ok "cca gui remove：删掉 App 和下载目录" '[ $rc -eq 0 ] && [ ! -e "$GA" ] && [ ! -e "$H/.cca/gui" ]'
ok "cca gui remove：App 留在 ~/Library 里的缓存、窗口状态、设置也删掉，别的 App 的不动" '[ ! -e "$H/Library/Caches/local.cca.app" ] && [ ! -e "$H/Library/Saved Application State/local.cca.app.savedState" ] && [ ! -e "$H/Library/Preferences/local.cca.app.plist" ] && [ -f "$H/Library/Preferences/com.example.other.plist" ] && grep -qx "delete local.cca.app" "$T/defaults.log"'
# cca install --gui（install.sh 在 macOS 上传的）：基础安装后装好并打开图形界面 App，剩下的配置在 App 的引导向导里走；
# 老 macOS（26 以前）或 App 装不上时自动回退到终端引导。不带 --gui 的 cca install 仍是终端引导
IGV=$(ver); publish_app "$IGV"
cp "$H/.cca/config.json" "$T/cfg.ig.bak"; cp "$H/.cca/accounts.json" "$T/tbl.ig.bak"; : > open.log
out=$(run CCA_TTY="$(ans)" cca install --gui 2>&1); rc=$?
ok "cca install --gui：macOS 26+ 装好并打开 App，剩下的配置交给 App 的引导向导——不在终端里问路由器/账号" '[ $rc -eq 0 ] && grep -qxF "OPEN [$GA] [--args] [--window]" open.log && [ "$(gver)" = "$IGV" ] && printf "%s" "$out" | grep -q "基础安装完成" && printf "%s" "$out" | grep -q "在打开的 cca App 里按引导" && ! printf "%s" "$out" | grep -qE "现在配置路由器|没找到 Claude Code|新增第一个账号"'
run cca gui remove >/dev/null 2>&1; : > open.log
out=$(run CCA_GUI_OS=15 CCA_TTY="$(ans)" cca install --gui 2>&1); rc=$?
ok "cca install --gui：macOS 26 以前自动回退到终端引导（不装 App、不提图形界面）" '[ $rc -eq 0 ] && [ ! -e "$GA" ] && ! grep -q OPEN open.log && ! printf "%s" "$out" | grep -q "正在安装图形界面"'
run cca gui remove >/dev/null 2>&1
cp "$T/cfg.ig.bak" "$H/.cca/config.json"; cp "$T/tbl.ig.bak" "$H/.cca/accounts.json"
mkdir -p "$T/devgui/Sources" "$T/devgui/tools"
cat > "$T/devgui/build.sh" <<'EOF'
#!/bin/bash
# 假的 macos/build.sh：记下参数，按 --version 写出一个 App
echo "BUILD $*" >> "$CCA_T/build.log"
v="" o=""; while [ $# -gt 0 ]; do case "$1" in --version) v="$2"; shift ;; --out) o="$2"; shift ;; esac; shift; done
mkdir -p "$o/Contents/MacOS"; printf '#!/bin/sh\n' > "$o/Contents/MacOS/cca"; chmod 755 "$o/Contents/MacOS/cca"
printf '<plist><dict><key>CFBundleIdentifier</key><string>local.cca.app</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>' "$v" > "$o/Contents/Info.plist"
echo "$o"
EOF
: > "$T/build.log"; CCAP=$(cd -P "$(dirname "$CCA")" && pwd)/cca
out=$(run CCA_GUI_SRC="$T/devgui" bash "$CCA" gui 2>&1); rc=$?
out2=$(run CCA_GUI_SRC="$T/devgui" bash "$CCA" gui 2>&1)
ok "开发模式（git 仓库里的 cca）：cca gui 从源码编译（源码没变不再编），用仓库里的 cca 打开编好的 App，不装进 ~/Applications" '[ $rc -eq 0 ] && [ "$(grep -c "^BUILD .*--arch native" "$T/build.log")" = 1 ] && grep -qF "OPEN [--env] [CCA_GUI_CCA=$CCAP] [$T/devgui/build/cca.app]" open.log && [ ! -e "$GA" ]'

fi
if sect 11b2; then
# 11b2. 维护者的账号 → 节点映射：私有仓库（假 git）+ 这台机器的 deploy key + 发布密钥签名
# （这时装着的是 mkver 换成测试密钥的版本，映射用 $T/relkey 签）
PR="$T/policy_repo"; mkdir -p "$PR"; PK="$H/.cca/policy/id_ed25519"
pol() {   # pol JSON [签名密钥]：写进假的私有映射仓库并签名
	printf '%s\n' "$1" > "$PR/policy.json"; rm -f "$PR/policy.json.sig"
	ssh-keygen -Y sign -q -n cca-policy -f "${2:-$T/relkey}" "$PR/policy.json" 2>/dev/null
}
nodes_pub() {   # nodes_pub [签名密钥]：$PR/nodes.json 用发布密钥签名（参考节点和映射用不同的命名空间）；11b2b、11b6 用
	rm -f "$PR/nodes.json.sig"; ssh-keygen -Y sign -q -n cca-nodes -f "${1:-$T/relkey}" "$PR/nodes.json" 2>/dev/null
}
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}}'
# 共享默认关着：终端里启动账号不引导配置映射（以前会问）。默认仓库用 CCA_POLICY_DEFAULT 换成假的（平时的用例里它是空的）
P=CCA_POLICY_DEFAULT=example/cca-policy
kl() { printf "'%s' %s" "$(cat "$PK.pub")" "$(cut -d@ -f2 "$PK.pub")"; }   # 给维护者的那一行里 key add 后面的部分：这台机器现在的公钥（单引号）和别名（机器名）
: > git.log
out=$(run $P CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?; out2=$(run $P cca policy 2>&1)
ok "共享默认关着：终端里启动账号不引导、不生成 deploy key、不拉；cca policy 说共享关着、怎么打开" '[ $rc -eq 3 ] && ! printf "%s" "$out" | grep -q "映射" && [ ! -f "$PK" ] && [ "$(cfgv "c.get(\"policy\", \"\")")" = "" ] && [ ! -s git.log ] && printf "%s" "$out2" | grep -q "维护者的共享关着：有维护者共享给你的账号时，运行 cca policy on 打开"'
printf 2 > "$T/policy_denied_n"; rm -f "$T/clipboard"
out=$(run $P CCA_TTY="$(ans)" cca policy on 2>&1); rc=$?
ok "cca policy on（维护者的默认仓库）：生成 deploy key，显示维护者要运行的那一行（公钥和别名写在里面，复制到剪贴板）；还没加上时说明之后每天自动拉" '[ $rc -eq 0 ] && [ -f "$PK" ] && printf "%s" "$out" | grep -qF "    ./policy.sh key add $(kl)" && [ "$(cat "$T/clipboard")" = "./policy.sh key add $(kl)" ] && printf "%s" "$out" | grep -q "这一行已复制到剪贴板" && printf "%s" "$out" | grep -q "cca 每天会自动拉一次映射" && [ "$(cfgv "c[\"policy\"]")" = example/cca-policy ] && [ ! -f "$H/.cca/policy/policy.json" ]'
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享开着、还没拉到映射：照常启动，不提问" '[ $rc -eq 3 ] && ! printf "%s" "$out" | grep -q "policy.sh key add"'
# 配置向导（App）第 1 步：等维护者时这一页不用点，每隔几秒自动拉，拉到映射自己进第 2 步；没有路由器时导入映射里账号的参考节点，映射里的账号
# 自动加进来，第 3 步只问还要不要再加。这一段结束时配置、账号表、映射仓库都还原
cp "$H/.cca/config.json" "$T/cfg.ow.bak"; cp "$H/.cca/accounts.json" "$T/tbl.ow.bak"; [ -f "$H/.cca/codex.json" ] && cp "$H/.cca/codex.json" "$T/cdx.ow.bak"
cp "$PR/policy.json" "$T/pr.ow"; cp "$PR/policy.json.sig" "$T/pr.ow.sig"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["router"]={}; c["nodes"]=[]; c.pop("node_ids", None); c.pop("local", None); json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"; [ -f "$H/.cca/codex.json" ] && printf '{}' > "$H/.cca/codex.json"
pol '{"version": 1, "claude": {"sam@example.com": "US-S"}, "aliases": {"sam@example.com": "sam"}}'
python3 -c 'import json,sys; json.dump({"version": 1, "group": "OpenAi", "proxies": [{"name": n, "type": "ss", "server": "192.0.2.%d" % (91 + i), "port": 8388, "cipher": "aes-128-gcm", "password": "x"} for i, n in enumerate(["US-S", "US-X"])]}, open(sys.argv[1], "w"))' "$PR/nodes.json"; nodes_pub
printf 2 > "$T/policy_denied_n"; : > git.log
out=$(sess '{"onboard.policy":[null,null,null],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env $P CCA_POLICY_POLL=1 cca _onboard 2>&1); rc=$?
ok "配置向导第 1 步：等维护者时这一页不用点，每隔几秒自动拉，维护者加上公钥后拉到映射、自己进第 2 步（页上说拉到了）" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.policy onboard.net onboard.add onboard.hub" ] && [ "$(grep -c "^GIT clone" git.log)" = 3 ] && evq "e.get(\"q\") == \"onboard.net\" and \"拉到了维护者的映射（Claude 1 个账号\" in e[\"message\"]" && [ -f "$H/.cca/policy/policy.json" ]'
ok "没有路由器：第 2 步先跳过也导入映射里账号的参考节点（别的参考节点不导入），映射里的账号自动加进来；第 3 步只问还要不要再加（默认不加），说明加了谁" 'cfgok "c[\"nodes\"] == [\"US-S\"] and c.get(\"onboarded\") is True" && tblok "t == {\"sam(sam@example.com)\": \"US-S\"}" && evq "e.get(\"q\") == \"onboard.add\" and e[\"fields\"][0][\"default\"] == \"n\" and \"sam\" in e.get(\"message\", \"\")"'
# 映射已经配置好（比如一键安装时带了 CCA_POLICY_REPO，维护者也加好了公钥）：拉一下就拉到了，没有映射那一页，也不算一步
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["nodes"]=[]; c.pop("node_ids", None); c.pop("onboarded", None); json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"; : > git.log
out=$(sess '{"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}],"onboard.hub":[{"v":"n"}]}' -- env $P CCA_POLICY_POLL=1 cca _onboard 2>&1); rc=$?
ok "配置向导：映射已经配置好、拉一下就拉到了 → 不问映射那一步，网络出口是第 1 步（共 3 步）、页上说拉到了映射" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.net onboard.add onboard.hub" ] && [ "$(grep -c "^GIT clone" git.log)" = 1 ] && evq "e.get(\"q\") == \"onboard.net\" and e[\"title\"] == \"第 1 步（共 3 步）：网络出口\" and \"拉到了维护者的映射\" in e[\"message\"]" && tblok "t == {\"sam(sam@example.com)\": \"US-S\"}"'
# 第 1 步先跳过的：维护者加好后靠 App 的定时拉取（cca policy pull --json，终端里的 cca policy pull 一样）补上节点和账号
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["nodes"]=[]; c.pop("node_ids", None); json.dump(c,open(p,"w"))' "$H/.cca/config.json"
printf '{}' > "$H/.cca/accounts.json"
out=$(run CCA_EVENTS="$T/ev.jsonl" cca policy pull --json 2>"$T/pull.err" </dev/null); rc=$?
ok "没有路由器：App 的定时拉取（cca policy pull --json）拉到映射后导入映射里账号的参考节点，映射里的账号随即加进来；stdout 只有 JSON，不提同步路由器" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"policy_pull\"" && grep -q "已导入维护者提供的节点：US-S" "$T/pull.err" && ! grep -q "同步" "$T/pull.err" && cfgok "c[\"nodes\"] == [\"US-S\"]" && tblok "t == {\"sam(sam@example.com)\": \"US-S\"}"'
# 没有路由器的机器关掉共享（cca policy off）：映射里的账号移出账号表（记进 parked），只有维护者参考节点才有定义的可选节点：自己的账号
# 用着的留着（说清楚关掉以后用不了、要换节点），没人用的去掉；cca policy on（deploy key 留着）直接拉到，原样加回来
edit_table 'j["mine(mine@example.com)"] = "US-S"'
out=$(run CCA_TTY="$(ans n)" cca policy off 2>&1); rc=$?
ok "cca policy off：先说会动什么（移出账号表的共享账号、登录留着；自己的账号用着只有参考节点才有的节点，关掉以后要换），问过才关（默认否），答否什么都不改" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "移出账号表的共享账号（登录留着，重新打开共享时加回来，不用重新登录）：Claude 的 sam（sam@example.com）" && printf "%s" "$out" | grep -q "这些账号用着维护者的参考节点，关掉以后用不了，要换一个节点：Claude 的 mine（mine@example.com）：US-S" && printf "%s" "$out" | grep -q "关掉维护者共享的账号？ \[y/N\]" && printf "%s" "$out" | grep -q "已取消" && tblok "\"sam(sam@example.com)\" in t" && cfgok "c.get(\"policy\") == \"example/cca-policy\""'
: > git.log
out=$(run CCA_TTY="$(ans y)" cca policy off 2>&1); rc=$?
ok "答是：sam 移出账号表（记进 parked），US-S 还有 mine 在用、留着；共享关掉（仓库记进 policy_was），拉到的映射、参考节点删掉，deploy key 留着" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已关掉共享：Claude 的 sam（sam@example.com） 移出了账号表" && tblok "t == {\"mine(mine@example.com)\": \"US-S\"}" && cfgok "\"policy\" not in c and c.get(\"policy_was\") == \"example/cca-policy\" and list(c[\"parked\"].values()) == [{\"key\": \"sam(sam@example.com)\", \"node\": \"US-S\"}] and c[\"nodes\"] == [\"US-S\"]" && [ ! -e "$H/.cca/policy/policy.json" ] && [ ! -e "$H/.cca/policy/nodes.json" ] && [ -f "$PK" ] && [ ! -s git.log ]'
out=$(run CCA_TTY="$(ans)" cca policy on 2>&1); rc=$?
ok "cca policy on：关掉时留着的 deploy key 直接拉到（不再显示给维护者的那一行），sam 原样加回来，parked 清掉" '[ $rc -eq 0 ] && [ "$(grep -c "^GIT clone" git.log)" = 1 ] && ! printf "%s" "$out" | grep -q "policy.sh key add" && printf "%s" "$out" | grep -q "已更新映射" && tblok "t == {\"mine(mine@example.com)\": \"US-S\", \"sam(sam@example.com)\": \"US-S\"}" && cfgok "c.get(\"policy\") == \"example/cca-policy\" and \"policy_was\" not in c and \"parked\" not in c"'
edit_table 'j.pop("mine(mine@example.com)")'
run CCA_TTY="$(ans y)" cca policy off >/dev/null 2>&1
ok "再关一次（没有自己的账号用 US-S 了）：只有参考节点才有定义的 US-S 从可选节点里去掉" 'tblok "t == {}" && cfgok "c[\"nodes\"] == [] and \"US-S\" not in c.get(\"node_ids\", {}) and list(c[\"parked\"]) != []"'
printf '{"version": 1, "claude": {"sam@example.com": "US-S"}}\n' > "$H/.cca/policy/policy.json"
out=$(run cca ls 2>&1); out2=$(gj state)
ok "关掉时后台正在拉的那一次之后才写回来（policy.json 又在了）：共享关着就不认它，sam 不会被加回来；_gui state 说共享关着、收起了 1 个" '! grep -q sam "$H/.cca/accounts.json" && printf "%s" "$out2" | jok "d[\"config\"][\"policy\"][\"state\"] == \"off\" and d[\"config\"][\"policy\"][\"parked\"] == 1 and d[\"config\"][\"policy\"][\"configured\"] is False and d[\"config\"][\"policy\"][\"accounts\"] == 0"'
rm -f "$H/.cca/policy/policy.json"
out=$(run cca policy pull 2>&1); rc=$?
ok "共享关着时 cca policy pull 不拉，说怎么打开" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者的共享关着（有维护者共享给你的账号时，运行 cca policy on 打开）"'
out=$(runev cca policy on 2>&1); rc=$?
ok "App 里打开共享（提问会话外、deploy key 还在）：导入映射里账号的参考节点，sam 加回来；result 是 policy_on" '[ $rc -eq 0 ] && tblok "t == {\"sam(sam@example.com)\": \"US-S\"}" && cfgok "c[\"nodes\"] == [\"US-S\"] and \"parked\" not in c" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"policy_on\"" && gj state | jok "d[\"config\"][\"policy\"][\"state\"] == \"on\" and d[\"config\"][\"policy\"][\"accounts\"] == 1 and d[\"config\"][\"policy\"][\"parked\"] == 0"'
cp "$T/cfg.ow.bak" "$H/.cca/config.json"; cp "$T/tbl.ow.bak" "$H/.cca/accounts.json"; [ -f "$T/cdx.ow.bak" ] && cp "$T/cdx.ow.bak" "$H/.cca/codex.json"
cp "$T/pr.ow" "$PR/policy.json"; cp "$T/pr.ow.sig" "$PR/policy.json.sig"; rm -f "$PR/nodes.json" "$PR/nodes.json.sig"
# 还原成没配置映射（账号表改回 US-A），接着测 cca policy setup
rm -f "$T/policy_denied_n"
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
rm -rf "$H/.cca/policy"; edit_table 'j["alice@example.com"] = "US-A"'; run cca router sync >/dev/null 2>&1
touch "$T/policy_denied"; : > git.log
out=$(run $P CCA_TTY="$(ans)" cca policy setup 2>&1); rc=$?
ok "cca policy setup（不写仓库就是维护者的默认仓库）：给这台机器生成 deploy key，显示公钥和维护者要运行的命令；公钥还没加上时说明之后会自动拉" '[ $rc -eq 0 ] && [ -f "$PK" ] && printf "%s" "$out" | grep -qF "$(cat "$PK.pub")" && printf "%s" "$out" | grep -q "说一声是谁的哪台机器" && printf "%s" "$out" | grep -qF "    ./policy.sh key add $(kl)" && printf "%s" "$out" | grep -q "GitHub 不认这台机器的 deploy key" && printf "%s" "$out" | grep -q "cca 每天会自动拉一次映射" && [ ! -f "$H/.cca/policy/policy.json" ] && [ "$(cfgv "c[\"policy\"]")" = example/cca-policy ]'
ok "拉映射只用这台机器的 deploy key：不读用户的 ssh / git 配置和 ssh-agent，只认 GitHub 的主机公钥；GitHub 拒绝了就不再重试" '[ "$(grep -c "^GIT clone" git.log)" = 1 ] && grep -q "^GIT clone git@github.com:example/cca-policy.git SSH=ssh -F /dev/null -i $PK -o IdentitiesOnly=yes -o IdentityAgent=none -o UserKnownHostsFile=$H/.cca/policy/known_hosts -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes .* GLOBAL=/dev/null$" git.log && grep -qx "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl" "$H/.cca/policy/known_hosts"'
rm -f "$T/policy_denied"
out=$(run cca policy pull 2>&1); rc=$?
ok "cca policy pull：验签通过才换上（只有自己能读），马上按映射改账号表并说明" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已更新映射（Claude 1 个账号、Codex 0 个账号）" && [ "$(stat -f %Lp "$H/.cca/policy/policy.json")" = 600 ] && grep -q "\"alice@example.com\": \"US-C\"" "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "alice@example.com 的出口按维护者的映射固定为 US-C（账号表里写的是 US-A，已改回）" && printf "%s" "$out" | grep -q "Claude  alice@example.com → US-C"'
n0=$(reloads); out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "映射换了出口：启动前同步到路由器（只换节点，不重载），照常启动" '[ $rc -eq 3 ] && [ "$(sel CCA-1)" = US-C ] && [ "$(reloads)" = "$n0" ]'
out=$(run CCA_TTY="$(ans)" EDITOR="$T/edit.py" CCA_TEST_EDIT='j["alice@example.com"] = "US-B"' cca edit 2>&1)
ok "cca edit 把映射里的账号改成别的节点：改回映射的节点并说明" 'grep -q "\"alice@example.com\": \"US-C\"" "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "按维护者的映射固定为 US-C（账号表里写的是 US-B，已改回）"'
pol '{"version": 1, "claude": {"alice@example.com": "US-D"}}' "$T/evilkey"
out=$(run cca policy pull 2>&1); rc=$?
pol '{"version": 1, "claude": {"not-an-email": "US-D", "alice@example.com": "US-C", "ALICE@example.com": "US-D"}}'
out2=$(run cca policy pull 2>&1); rc2=$?
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}}'; printf '{"version": 1, "claude": {"alice@example.com": "US-D"}}\n' > "$PR/policy.json"
out3=$(run cca policy pull 2>&1); rc3=$?
ok "不是发布密钥签的、签过之后又被改过的、格式不对的映射都不用，接着用上次拉到的" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "签名不对" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "映射的格式不对：claude：「not-an-email」不像邮箱" && printf "%s" "$out2" | grep -q "claude：ALICE@example.com 写了不止一次（邮箱不分大小写）" && [ $rc3 -ne 0 ] && printf "%s" "$out3" | grep -q "签名不对" && grep -q "\"alice@example.com\": \"US-C\"" "$H/.cca/policy/policy.json"'
# App 的定时拉取（PROTOCOL 4.9）：stdout 一行 JSON；有变化时马上照映射改账号表并同步路由器，要重载的不问；拉不到时 fail 事件；state 给下次的时间
rm -f "$H/.cca/policy/checked"
ok "App 定时拉映射：还没拉过时 state 的 next_check_epoch 是 null" 'gj state | jok "d[\"config\"][\"policy\"][\"configured\"] and d[\"config\"][\"policy\"][\"next_check_epoch\"] is None"'
pol '{"version": 1, "claude": {"alice@example.com": "US-D"}}'
runev cca policy pull --json >/dev/null 2>&1
out=$(run CCA_EVENTS="$T/ev.jsonl" cca policy pull --json 2>/dev/null </dev/null); rc=$?
ok "App 定时拉映射：cca policy pull --json：stdout 只有一份 JSON，这次没变化时 changed 是 false" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"policy_pull\" and d[\"changed\"] is False and d[\"summary\"] and d[\"generated_epoch\"] > 0"'
pol '{"version": 1, "claude": {"alice@example.com": "US-A"}}'
: > ssh.log
out=$(run CCA_EVENTS="$T/ev.jsonl" cca policy pull --json 2>/dev/null </dev/null); rc=$?
ok "App 定时拉映射：有变化：changed，账号表马上改好，同步到路由器（不用等下一条命令）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"changed\"] is True" && tblok "t[\"alice@example.com\"] == \"US-A\"" && grep -q "sh -s" ssh.log && [ "$(cfgv "c.get(\"synced\") != None")" = True ]'
ok "App 定时拉映射：拉过之后 state 给下次的时间（一小时后），启动时的后台拉取不用再拉" 'gj state | jok "abs(d[\"config\"][\"policy\"][\"next_check_epoch\"] - __import__(\"time\").time() - 3600) < 120" && [ -n "$(find "$H/.cca/policy/checked" -mmin -1)" ]'
touch "$T/policy_offline"
: > "$T/ev.jsonl"; out=$(run CCA_EVENTS="$T/ev.jsonl" cca policy pull --json 2>/dev/null </dev/null); rc=$?
rm -f "$T/policy_offline"
ok "App 定时拉映射：拉不到：退出码 1，没有 JSON，fail 事件说原因" '[ $rc -eq 1 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"policy_unavailable\" and \"连不上 GitHub\" in e[\"message\"]"'
out=$(run cca policy pull --what 2>&1); rc=$?
ok "cca policy pull：不认识的参数说用法" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法: cca policy pull"'
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}}'; run cca policy pull >/dev/null 2>&1
# 账号表表单里，维护者固定的账号不能改（只有一个选项），答案不算数
edit_table 'j["自己(self1@example.com)"] = "US-D"'; rm -f "$T/ev.jsonl"
out=$(sess '{}' --eof-at edit.form -- cca edit 2>&1)
ok "账号表表单：维护者固定的账号：只有一个选项，说明是维护者固定的" 'evq "e.get(\"q\") == \"edit.form\" and any(f[\"label\"] == \"alice@example.com\" and len(f[\"options\"]) == 1 and f[\"options\"][0][\"value\"] == \"US-C\" and \"维护者固定\" in f[\"hint\"] for f in e[\"fields\"])" && evq "e.get(\"q\") == \"edit.form\" and any(f[\"label\"] == \"自己（self1@example.com）\" and len(f[\"options\"]) > 1 for f in e[\"fields\"])"'
FP=$(python3 -I -c 'import json,sys; print([x["id"] for e in map(json.loads, open(sys.argv[1])) if e.get("q") == "edit.form" for x in e["fields"] if x["label"] == "alice@example.com"][0])' "$T/ev.jsonl")
sess '{"edit.form":[{"'$FP'":"US-B"}]}' --keep -- cca edit >/dev/null 2>&1
ok "账号表表单：维护者固定的账号：答案不算数，账号表不变" 'tblok "t[\"alice@example.com\"] == \"US-C\""'
edit_table 'j.pop("自己(self1@example.com)")'
touch "$T/policy_offline"; : > git.log
out=$(run cca policy pull 2>&1); rc=$?
rm -f "$T/policy_offline"
ok "连不上 GitHub：22 端口不通就改走 ssh.github.com:443，一共试三轮；说出真正的原因（不是 git 的套话），原来的映射照用" '[ $rc -ne 0 ] && [ "$(grep -c "^GIT clone" git.log)" = 6 ] && grep -q "^GIT clone ssh://git@ssh.github.com:443/example/cca-policy.git " git.log && printf "%s" "$out" | grep -q "连不上 GitHub：ssh: connect to host github.com port 22: Operation timed out（接着用上次拉到的）" && grep -q "\"alice@example.com\": \"US-C\"" "$H/.cca/policy/policy.json"'
# 映射和这台机器的账号表合并：映射里有、这台机器上还没有的官方账号自动加进来（别名用映射里 aliases 写的），
# 这台机器自己的账号不动；删掉的不再自动加回来（cca add 加回来）
edit_table 'j["自己(own@example.com)"] = "US-A"'
pol '{"version": 1, "claude": {"alice@example.com": "US-C", "erin@example.com": "US-D", "frank@example.com": "US-B", "gina@example.com": "HK-A"}, "aliases": {"erin@example.com": "Erin", "frank@example.com": "kimi"}}'
out=$(run cca policy pull 2>&1); rc=$?
ok "合并：映射里有、这台机器上还没有的账号自动加进账号表并分到端口（别名用映射里写的，和这台机器上的别名重名时只写邮箱）；这台机器自己的账号不动" '[ $rc -eq 0 ] && tblok "t.get(\"Erin(erin@example.com)\") == \"US-D\" and t.get(\"frank@example.com\") == \"US-B\" and t.get(\"自己(own@example.com)\") == \"US-A\" and \"kimi\" in t" && cfgok "\"Erin(erin@example.com)\" in c[\"ports\"] and \"frank@example.com\" in c[\"ports\"]" && printf "%s" "$out" | grep -q "按维护者的映射添加了 Claude 账号 Erin（erin@example.com），出口 US-D" && printf "%s" "$out" | grep -q "Claude  Erin（erin@example.com） → US-D"'
ok "映射要的节点不在这台机器的可选节点里：先不加（加进来整张表就校验不过），cca policy 里注明怎么办" '! grep -q gina "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "映射里还没加到这台机器上的账号：" && printf "%s" "$out" | grep -q "Claude  gina@example.com → HK-A：节点 HK-A 不在这台机器的可选节点里（运行 cca router setup 选上它）"'
cp "$H/.cca/policy/policy.json" "$T/policy.gui"
python3 -c 'import json,sys; p=sys.argv[1]; j=json.load(open(p)); j["claude"]["hank@example.com"]="US-B"; json.dump(j, open(p, "w"))' "$H/.cca/policy/policy.json"
out=$(gj state)
ok "_gui state（只读）：映射里新加的账号算进快照，notices 里说本来会加（policy_added），账号表和 config.json 都不改" 'printf "%s" "$out" | jok "any(n[\"code\"] == \"policy_added\" and n[\"key\"] == \"hank@example.com\" and n[\"node\"] == \"US-B\" for n in d[\"notices\"]) and \"hank@example.com\" in [a[\"key\"] for a in d[\"tools\"][0][\"accounts\"]]" && ! grep -q hank "$H/.cca/accounts.json" "$H/.cca/config.json"'
cp "$T/policy.gui" "$H/.cca/policy/policy.json"
p=$(cfgv 'c["ports"]["Erin(erin@example.com)"]')
out=$(run CCA_TTY="$(ans)" cca @erin --version 2>&1); rc=$?
ok "加进来的账号照常启动：先用它的邮箱登录，路由器同步到映射的节点" '[ $rc -eq 3 ] && grep -q "ARGS: auth login --claudeai --email erin@example.com" claude.log && [ "$(sel "CCA-$((p - 7900))")" = US-D ]'
out=$(run CCA_TTY="$(ans y)" cca rm frank 2>&1); rc=$?
out2=$(run cca ls 2>&1); out3=$(run cca policy 2>&1)
ok "删掉映射里的账号：记下来，以后不再自动加回来；cca policy 里注明怎么加回来" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "frank@example.com 在维护者的映射里：这台机器上不会再自动加回来（要加回来：cca add）" && ! grep -q frank "$H/.cca/accounts.json" && ! printf "%s" "$out2" | grep -q "按维护者的映射添加了" && cfgok "c.get(\"policy_skip\") == [\"frank@example.com\"]" && printf "%s" "$out3" | grep -q "Claude  kimi（frank@example.com） → US-B：这台机器上删掉过（要加回来：cca add）"'
out=$(run CCA_TTY="$(ans frank@example.com '')" cca add 2>&1); rc=$?
ok "用 cca add 加回来：出口直接用映射的节点，不让选；以后照常跟着映射" '[ $rc -eq 0 ] && tblok "t.get(\"frank@example.com\") == \"US-B\"" && printf "%s" "$out" | grep -q "frank@example.com 的出口按维护者的映射固定为 US-B" && ! printf "%s" "$out" | grep -q "选择节点编号" && cfgok "\"policy_skip\" not in c"'
run CCA_TTY="$(ans "$R1")" cca key frank >/dev/null 2>&1; FAID=$(cd "$T/keychain" && ls -d cca-ref-frank-example-com-* 2>/dev/null); FAID=${FAID#cca-ref-}
out=$(run CCA_TTY="$(ans)" EDITOR="$T/edit.py" CCA_TEST_EDIT='j.pop("frank@example.com")' cca edit 2>&1)
ok "cca edit 里删掉官方账号：它记下备查的 API key 一起删（钥匙串的条目和 config.json 里的记录）；删了找不回来，说一声" '[ -n "$FAID" ] && [ ! -e "$T/keychain/cca-ref-$FAID" ] && cfgok "\"$FAID\" not in c.get(\"ref_keys\", []) and \"$FAID\" not in c.get(\"key_hints\", {})" && printf "%s" "$out" | grep -qF "已从钥匙串删掉 $FAID 记下的 API key（sk-ant-…WXYZ）：账号表里已经没有这个账号了"'
ok "cca edit 里删掉映射里的账号：同样记下来，不会马上又按映射加回来" '! grep -q frank "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "frank@example.com 在维护者的映射里：删掉了就不再自动加回来" && ! printf "%s" "$out" | grep -q "按维护者的映射添加了" && cfgok "c.get(\"policy_skip\") == [\"frank@example.com\"]"'
# 先按没写别名的映射加进来、只有邮箱的账号（这里直接写进账号表），维护者后来在 aliases 里补了别名
edit_table 'j["hank@example.com"] = "US-B"'; run cca ls >/dev/null 2>&1; p0=$(cfgv 'c["ports"].get("hank@example.com")')
pol '{"version": 1, "claude": {"alice@example.com": "US-C", "erin@example.com": "US-D", "hank@example.com": "US-B"}, "aliases": {"erin@example.com": "E2", "hank@example.com": "Hank"}}'
out=$(run cca policy pull 2>&1); rc=$?; out2=$(run cca ls 2>&1)
ok "映射里给账号起了别名：这台机器上还没有别名的补上（端口不变，只说一次），已经起过别名的不改" '[ $rc -eq 0 ] && [ "$p0" != None ] && tblok "\"Hank(hank@example.com)\" in t and \"hank@example.com\" not in t and \"Erin(erin@example.com)\" in t" && [ "$(cfgv "c[\"ports\"].get(\"Hank(hank@example.com)\")")" = "$p0" ] && printf "%s" "$out" | grep -q "按维护者的映射给 Claude 账号 hank@example.com 起了别名 Hank" && ! printf "%s" "$out" | grep -q "起了别名 E2" && ! printf "%s" "$out2" | grep -q "起了别名"'
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}}'
out=$(run cca policy pull 2>&1); rc=$?
ok "维护者从映射里删掉了账号：这台机器上的账号留着（登录还在），只是不再固定出口" '[ $rc -eq 0 ] && tblok "t.get(\"Erin(erin@example.com)\") == \"US-D\"" && ! printf "%s" "$out" | grep -q "Erin（erin@example.com） →"'
run CCA_TTY="$(ans y)" cca rm erin >/dev/null 2>&1; run CCA_TTY="$(ans y)" cca rm 自己 >/dev/null 2>&1; run CCA_TTY="$(ans y)" cca rm Hank >/dev/null 2>&1
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy_skip'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
# 映射里的 Codex 账号要到第一次用 cca codex 时才加（模拟还没用过 Codex 的机器：先把 Codex 的账号表挪开，测完还原）
cp "$H/.cca/config.json" "$T/config.pre"; mv "$H/.cca/codex.json" "$T/codex.pre"
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}, "codex": {"gina@example.com": "US-A"}, "aliases": {"gina@example.com": "Gina"}}'
out=$(run cca policy pull 2>&1); rc=$?; [ -f "$H/.cca/codex.json" ] && cx1=yes || cx1=no
out2=$(run cca codex ls 2>&1)
ok "映射里的 Codex 账号：这台机器还没用过 cca codex 时不加（cca policy 里注明），第一次用 cca codex 时加进来" '[ $rc -eq 0 ] && [ $cx1 = no ] && printf "%s" "$out" | grep -q "Codex  1 个账号：第一次用 cca codex 时加进来" && printf "%s" "$out2" | grep -q "按维护者的映射添加了 Codex 账号 Gina（gina@example.com），出口 US-A" && tblok "t == {\"Gina(gina@example.com)\": \"US-A\"}" "$H/.cca/codex.json"'
printf '{"version": 1, "claude": {"alice@example.com": "US-C"}}\n' > "$H/.cca/policy/policy.json"
mv "$T/codex.pre" "$H/.cca/codex.json"; cp "$T/config.pre" "$H/.cca/config.json"
# 后台拉到了新映射（这里直接写进去，模拟后台拉取的结果）：账号表在下一条命令里才改，cca web / check 要先让路由器跟上
printf '{"version": 1, "claude": {"alice@example.com": "US-D"}}\n' > "$H/.cca/policy/policy.json"
run CCA_TTY="$(ans)" cca web alice >/dev/null 2>&1; s1=$(sel CCA-1)
printf '{"version": 1, "claude": {"alice@example.com": "US-B"}}\n' > "$H/.cca/policy/policy.json"
run cca check alice >/dev/null 2>&1; s2=$(sel CCA-1)
ok "映射在后台换了之后，cca web、cca check 先把路由器同步到映射的节点，不经旧的节点出去" '[ "$s1" = US-D ] && [ "$s2" = US-B ]'
pol '{"version": 1, "claude": {"alice@example.com": "US-A"}, "codex": {"dave@example.com": "US-D"}}'
run cca update off >/dev/null 2>&1; rm -f "$H/.cca/policy/checked"; run cca ls >/dev/null 2>&1
for i in $(seq 1 50); do grep -q US-A "$H/.cca/policy/policy.json" 2>/dev/null && break; sleep 0.2; done
run cca update auto >/dev/null 2>&1
ok "每天在后台自动拉一次映射（关了自动更新也拉）" 'grep -q "\"alice@example.com\": \"US-A\"" "$H/.cca/policy/policy.json" && [ -f "$H/.cca/policy/checked" ] && tail -n 1 "$H/.cca/policy/log" | grep -q "拉取成功"'
out=$(run cca policy 2>&1)
ok "cca policy：映射仓库、deploy key、映射摘要、最近一次拉取，以及这台机器上按映射固定出口的账号（Claude、Codex 都列）" 'printf "%s" "$out" | grep -q "映射仓库：example/cca-policy" && printf "%s" "$out" | grep -qF "$(cat "$PK.pub")" && printf "%s" "$out" | grep -q "映射：Claude 1 个账号、Codex 1 个账号" && printf "%s" "$out" | grep -q "最近一次拉取：.*拉取成功" && printf "%s" "$out" | grep -q "Claude  alice@example.com → US-A" && printf "%s" "$out" | grep -q "Codex  dave@example.com → US-D" && grep -q "\"dave@example.com\": \"US-D\"" "$H/.cca/codex.json"'
# 关掉共享（配置过路由器的机器）：共享账号正在用（Claude App 开着）时不关；App 里关先要同意；关掉以后 alice、dave 移出账号表（登录目录
# 还在），路由器同步、启动器删掉，不再拉映射（deploy key 留着）；cca policy on 原样加回来（同一个账号目录，不用重新登录）
mkdir -p "$T/fakeapp2/Contents/MacOS"; printf '#!/bin/bash\nsleep 30\n' > "$T/fakeapp2/Contents/MacOS/Claude"; chmod 755 "$T/fakeapp2/Contents/MacOS/Claude"
"$T/fakeapp2/Contents/MacOS/Claude" --user-data-dir="$AD/desktop" & FAKEPID=$!
sleep 0.3
out=$(run CCA_TTY="$(ans y)" cca policy off 2>&1); rc=$?
runev cca policy off >/dev/null 2>&1; rc2=$?
kill $FAKEPID 2>/dev/null; wait $FAKEPID 2>/dev/null
ok "cca policy off：共享账号的 Claude App 还开着：不关，说是哪个；App 里是 fail 事件 policy_off_busy（不先问同意）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "这些共享账号正在用：Claude 的 alice@example.com（Claude App）" && [ $rc2 -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"policy_off_busy\" and \"Claude App\" in e[\"message\"]" && ! evq "e[\"ev\"] == \"consent\"" && tblok "\"alice@example.com\" in t" && cfgok "c.get(\"policy\") == \"example/cca-policy\""'
out=$(runev cca policy off 2>&1); rc=$?
ok "App 里关（不在终端）：先要同意（consent policy_off，范围 all，默认否），什么都不改" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "要先同意：在终端里运行 cca policy off" && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"policy_off\" and e[\"scope\"] == \"all\" and e[\"default\"] == \"no\"" && tblok "\"alice@example.com\" in t"'
: > ssh.log; : > git.log
out=$(runev CCA_GRANT=policy_off:all cca policy off 2>&1); rc=$?
ok "同意以后：alice、dave 移出账号表（登录目录还在，记进各自工具的 parked），路由器同步，启动器删掉，deploy key 留着，result 是 policy_off" '[ $rc -eq 0 ] && ! grep -q alice "$H/.cca/accounts.json" && ! grep -q dave "$H/.cca/codex.json" && [ -d "$AD" ] && cfgok "\"policy\" not in c and c[\"parked\"][\"$(basename "$AD")\"] == {\"key\": \"alice@example.com\", \"node\": \"US-A\"} and [v[\"key\"] for v in c[\"codex\"][\"parked\"].values()] == [\"dave@example.com\"]" && grep -q "sh -s" ssh.log && printf "%s" "$out" | grep -q "已删除启动器「" && [ -f "$PK" ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"policy_off\"" && [ ! -s git.log ]'
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "关掉以后 alice 不在账号表里：启动不了它（登录还留着，打开共享时加回来）" '[ $rc -ne 0 ] && ! grep -q alice "$H/.cca/accounts.json"'
out=$(run CCA_TTY="$(ans)" cca policy on 2>&1); rc=$?
ok "cca policy on：deploy key 还在，直接拉到；alice、dave 原样加回来（同一个账号目录），parked 清掉，启动器加回来" '[ $rc -eq 0 ] && [ "$(grep -c "^GIT clone" git.log)" = 1 ] && ! printf "%s" "$out" | grep -q "policy.sh key add" && tblok "t.get(\"alice@example.com\") == \"US-A\"" && grep -q "\"dave@example.com\": \"US-D\"" "$H/.cca/codex.json" && cfgok "\"parked\" not in c and \"parked\" not in c.get(\"codex\", {})" && [ "$(ls -d "$H"/.cca/accounts/alice-example-com-*)" = "$AD" ] && printf "%s" "$out" | grep -q "已生成启动器「"'
out=$(run CCA_TTY="$(ans y)" cca policy off --forget-key 2>&1); rc=$?; out2=$(run cca policy 2>&1)
ok "cca policy off --forget-key：deploy key 一起删，不再记着仓库；cca policy 说共享关着、怎么打开" '[ $rc -eq 0 ] && [ ! -e "$PK" ] && [ ! -e "$PK.pub" ] && cfgok "\"policy\" not in c and \"policy_was\" not in c" && printf "%s" "$out2" | grep -q "维护者的共享关着：有维护者共享给你的账号时，运行 cca policy on 打开"'
out=$(run CCA_TTY="$(ans)" cca policy on 2>&1); rc=$?
ok "不记着仓库、也没有维护者的默认仓库：cca policy on 问映射仓库，回车取消（什么都不生成）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者的映射仓库（像 someone/cca-policy；回车取消）" && printf "%s" "$out" | grep -q "已取消：知道维护者的映射仓库以后，运行 cca policy setup 仓库" && [ ! -e "$PK" ] && cfgok "not c.get(\"policy\")"'
out=$(sess '{"policy.repo":[{"v":"bad name"}]}' -- cca policy on 2>&1); rc=$?
ok "App 里打开共享、不知道仓库：问映射仓库（policy.repo）；填得不对：报错，什么都不生成" '[ $rc -ne 0 ] && [ "$(asks)" = "policy.repo" ] && evq "e.get(\"q\") == \"policy.repo\" and e[\"fields\"][0][\"kind\"] == \"text\"" && [ ! -e "$PK" ]'
touch "$T/policy_denied"
out=$(sess '{"policy.repo":[{"v":"example/cca-policy"}],"onboard.policy":[{"next":"skip"}]}' -- env CCA_POLICY_POLL=1 cca policy on 2>&1); rc=$?
ok "App 里打开共享、维护者还没加上：给维护者那一行的那一页（标题是打开维护者的共享，选项说「先跳过」，页上说加好后会自动打开共享）；先跳过：共享开着、在等维护者，result 是 policy_on" '[ $rc -eq 0 ] && [ "$(asks)" = "policy.repo onboard.policy" ] && evq "e.get(\"q\") == \"onboard.policy\" and e[\"title\"] == \"打开维护者的共享\" and \"加好后这里会自动打开共享\" in e[\"message\"] and e[\"fields\"][1][\"options\"][0][\"label\"] == \"先跳过\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"policy_on\"" && [ -f "$PK" ] && cfgok "c.get(\"policy\") == \"example/cca-policy\"" && gj state | jok "d[\"config\"][\"policy\"][\"state\"] == \"pending\""'
out=$(sess '{"onboard.policy":[{"next":"decline"}]}' -- env CCA_POLICY_POLL=1 cca policy on 2>&1); rc=$?
ok "再打开一次（仓库就是开着的那个，维护者还没加上）：直接是那一页；选「不打开共享」：去掉仓库、删掉 deploy key，result 是 policy_off" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.policy" ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"policy_off\"" && [ ! -e "$PK" ] && cfgok "not c.get(\"policy\") and \"policy_was\" not in c"'
rm -f "$T/policy_denied"
out=$(run CCA_TTY="$(ans)" cca policy setup example/cca-policy 2>&1); rc=$?
ok "cca policy setup 仓库：新生成一把 deploy key（给维护者的那一行），拉到以后 alice、dave 照原样加回来" '[ $rc -eq 0 ] && [ -f "$PK" ] && printf "%s" "$out" | grep -q "policy.sh key add" && tblok "t.get(\"alice@example.com\") == \"US-A\"" && grep -q "\"dave@example.com\": \"US-D\"" "$H/.cca/codex.json"'
# 映射要的节点不在这台机器的可选节点里（路由器上有 HK-A，装的时候只选了 US-A…US-D）
pol '{"version": 1, "claude": {"alice@example.com": "HK-A"}}'; run cca policy pull >/dev/null 2>&1
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "映射要的节点不在可选节点里：不启动，说明 cca edit 改不了、要用 cca router setup 重新选可选节点" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者的映射要求它走「HK-A」，可这个节点不在可选列表" && printf "%s" "$out" | grep -q "运行 cca router setup 重新选可选节点"'
n0=$(reloads)
out=$(run CCA_TTY="$(ans '' '' '' '1 2 3 4 5' y)" cca router setup 2>&1); rc=$?
out2=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc2=$?
ok "这时 cca router setup 不会因为账号表有错误就退出：直接让重新选可选节点，选上 HK-A 后同步（端口池变了，问过再重载），照常启动" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "先重新选可选节点" && [ "$(cfgv "c[\"nodes\"] == [\"US-A\", \"US-B\", \"US-C\", \"US-D\", \"HK-A\"]")" = True ] && [ "$(sel CCA-1)" = HK-A ] && [ "$(reloads)" -gt "$n0" ] && [ $rc2 -eq 3 ]'
# 路由器上 HK-A 改名叫 HK-A2（地址不变），维护者的映射还写着 HK-A：账号表跟着改名，映射里的旧名字按记下的改名换算，不会被改回去
node_rn HK-A 192.0.2.5 HK-A2 192.0.2.5; oc_restart
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
out2=$(run cca policy 2>&1)
ok "映射里的节点在路由器上改了名、映射还没更新：账号表跟着改成 HK-A2，不会被映射改回旧名字，照常启动" '[ $rc -eq 3 ] && grep -q "\"alice@example.com\": \"HK-A2\"" "$H/.cca/accounts.json" && [ "$(sel CCA-1)" = HK-A2 ] && ! printf "%s" "$out" | grep -q "按维护者的映射固定为" && printf "%s" "$out2" | grep -q "Claude  alice@example.com → HK-A2（映射里写的是 HK-A，路由器上已经改名）"'
pol '{"version": 1, "claude": {"alice@example.com": "HK-A2"}}'; out=$(run cca policy pull 2>&1)
ok "维护者把映射更新成新名字之后：照常，不再注明改名" 'grep -q "\"alice@example.com\": \"HK-A2\"" "$H/.cca/accounts.json" && printf "%s" "$out" | grep -q "Claude  alice@example.com → HK-A2$"'

fi
if sect 11b2b; then
# 11b2b. 维护者参考配置里的节点写进路由器：./policy.sh 从维护者的 custom.yaml 抽出策略组 OpenAi 和它引用的节点（nodes.json，命名空间
#        cca-nodes 签名），各台机器拉映射时一起拉；缺的直接写进路由器的配置文件（OpenClash 选用的配置、ShellCrash 的订阅配置，行尾带
#        # cca），名字补进同名策略组。3.3.0-beta.8 删 Clash Verge 时这一节整个删掉过，这里恢复的是路由器这一半。
#        这一节结束时路由器、config.json、账号表和映射仓库都还原
rm -rf "$T/router.snap"; cp -R "$R" "$T/router.snap"; cp "$H/.cca/config.json" "$T/config.snap"; cp "$H/.cca/accounts.json" "$T/accounts.snap"
rget() {   # rget 配置 'Ruby 表达式'：c 是读出来的配置，g 是它的 OpenAi 组成员，px 是节点名
	ruby -ryaml -e 'c = YAML.load_file(ARGV[0]) || {}; g = ((c["proxy-groups"] || []).find { |x| x["name"] == "OpenAi" } || {})["proxies"]; px = (c["proxies"] || []).map { |p| p["name"] }; exit(eval(ARGV[1]) ? 0 : 1)' "$1" "$2"
}
added() {   # added 原来的 现在的：现在的只是在原来的基础上加了行（原来的一行没删、没改）；输出加的行里带 # cca 的有几行
	          # （块写法的节点只有 name 那一行带，组成员每行都带）
	diff "$1" "$2" | awk '/^</ || /^---/ { bad = 1 } /^>/ && / # cca$/ { n++ } END { if (bad) print "x"; else print n + 0 }'
}
one_line() {   # one_line 配置文件 节点 一行写法 [缩进]：把 cca 写成块写法的节点换回 2.8.0-beta.5～7 写的一行（模拟旧版本写的）
	python3 -c "import re,sys; p, n, f, i = sys.argv[1:5]; s = open(p).read(); t = re.sub(r\"(?m)^%s- name: %s # cca\\n(?:%s  .*\\n)*\" % (i, re.escape(n), i), lambda m: i + \"- \" + f + \" # cca\\n\", s, count=1); assert t != s; open(p, \"w\").write(t)" "$1" "$2" "$3" "${4:-}"
}
fill_tmp() { ls "$R"/tmp/cca.src.* "$R"/tmp/cca.fill.* 2>/dev/null; }   # 参考节点用过的临时文件（应该都删了）
cat > "$T/custom.yaml" <<'Y'
proxies:
  - {name: AI-A, type: ss, server: 192.0.2.21, port: 8388, cipher: aes-128-gcm, password: x}
  - name: AI-B
    type: vless
    server: 192.0.2.22
    port: 443
    uuid: 00000000-0000-4000-8000-000000000001
    tls: true
    servername: www.example.com
    flow: xtls-rprx-vision
    reality-opts:
      public-key: -test-public-key
      short-id: '0a1b2c3d4e5f6789'
  - {name: US-A, type: ss, server: 192.0.2.1, port: 8388, cipher: aes-128-gcm, password: other}
  - {name: AI-B2, type: ss, server: 192.0.2.2, port: 8388, cipher: aes-128-gcm, password: x}
  - {name: LAN-ONLY, type: ss, server: 192.0.2.99, port: 8388, cipher: aes-128-gcm, password: x}
proxy-groups:
  - {name: Proxy, type: select, proxies: [LAN-ONLY, AI-A]}
  - name: OpenAi
    type: select
    proxies:
      - Proxy
      - AI-A
      - AI-B
      - US-A
      - AI-B2
rules:
  - SRC-IP-CIDR,192.0.2.100/32,Proxy
  - MATCH,Proxy
Y
out=$(run bash "$D/../policy.sh" nodes "$T/custom.yaml" 2>"$T/pn.err"); rc=$?
printf '%s\n' "$out" > "$PR/nodes.json"
ok "policy.sh nodes：从维护者的配置里只抽出策略组 OpenAi 引用的节点（组成员里的策略组不算，规则、别的节点都不带），格式检查通过" '[ $rc -eq 0 ] && grep -q "参考节点：策略组「OpenAi」里的 4 个节点（Proxy 不是节点，不算）" "$T/pn.err" && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if j[\"version\"] == 1 and j[\"group\"] == \"OpenAi\" and [p[\"name\"] for p in j[\"proxies\"]] == [\"AI-A\", \"AI-B\", \"US-A\", \"AI-B2\"] and j[\"proxies\"][1][\"reality-opts\"][\"short-id\"] == \"0a1b2c3d4e5f6789\" else 1)" "$PR/nodes.json" && ! grep -q "LAN-ONLY\|SRC-IP" "$PR/nodes.json" && run bash "$CCA" _fill-check "$PR/nodes.json" >/dev/null'
nodes_pub
# 用户路由器上的配置：有 OpenAi 组（块写法，只有 US-A），没有 AI-A、AI-B；AI-B2 和 US-B 是同一个节点（地址一样）
awk '{ print } /^proxy-groups:$/ { print "  - name: OpenAi"; print "    type: select"; print "    proxies:"; print "      - US-A" }' "$SRC" > "$T/src.tmp" && cat "$T/src.tmp" > "$SRC"; oc_restart
cp "$SRC" "$T/src.fill"; n0=$(reloads); r0=$(restarts)
out=$(run CCA_TTY="$(ans y)" cca policy pull 2>&1); rc=$?
ok "拉映射时一起拉参考节点（验签通过才换上，只有自己能读），马上同步：缺的直接写进 OpenClash 选用的配置文件，说明要重启一次才用得上，问过后重启（不热加载）" '[ $rc -eq 0 ] && [ "$(stat -f %Lp "$H/.cca/policy/nodes.json")" = 600 ] && printf "%s" "$out" | grep -q "已更新映射（.*；参考节点：策略组 OpenAi 的 4 个节点）" && printf "%s" "$out" | grep -qF "已把维护者参考配置里的节点写进 OpenClash 的配置文件 ${SRC}：补上节点 AI-A、AI-B；策略组 OpenAi 里加上 AI-A、AI-B、US-B（行尾带 # cca；改之前的在 $R/etc/cca/source.bak.yaml）" && printf "%s" "$out" | grep -q "OpenClash 的配置文件里新写了维护者参考配置里的节点，要用上只能重启一次 OpenClash" && printf "%s" "$out" | grep -q "现在重启 OpenClash 吗" && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(reloads)" = "$n0" ]'
ok "OpenClash 的配置文件：AI-A、AI-B 一个一行插在 proxies 开头，名字接在 OpenAi 组后面，行尾都带 # cca，别的一个字节不动；US-A 是用户自己的（定义不动、不重复）；AI-B2 地址和 US-B 一样，组里加的是 US-B；原来的留了备份" '[ "$(added "$T/src.fill" "$SRC")" = 5 ] && rget "$SRC" "px.count(\"AI-A\") == 1 && px.include?(\"AI-B\") && !px.include?(\"AI-B2\") && px.count(\"US-A\") == 1 && c[\"proxies\"].find { |p| p[\"name\"] == \"US-A\" }[\"password\"] == \"x\" && c[\"proxies\"].find { |p| p[\"name\"] == \"AI-B\" }[\"reality-opts\"][\"short-id\"] == \"0a1b2c3d4e5f6789\" && g == %w[US-A AI-A AI-B US-B]" && cmp -s "$R/etc/cca/source.bak.yaml" "$T/src.fill"'
ok "写之前用内核 -t 校验过（拿正在用的运行配置加上这些节点），记下了这一版的 id；重启后按写好的配置文件生成运行配置，里面就有；临时文件都删了" 'grep -q "^CORE .* -t -d $R/etc/openclash -f $R/tmp/cca.fill\." router.log && [ -s "$R/etc/cca/fill.ok" ] && grep -qF "\"id\": \"$(cat "$R/etc/cca/fill.ok")\"" "$R/etc/cca/accounts.yaml" && rget "$RC" "px.include?(\"AI-A\") && px.include?(\"AI-B\") && g == %w[US-A AI-A AI-B US-B]" && [ ! -e "$R/etc/cca/fill.pending" ] && [ -z "$(fill_tmp)" ]'
out=$(run cca router nodes 2>&1); out2=$(run cca router status 2>&1); out3=$(run cca policy 2>&1)
ok "cca router nodes 列得出写进去的节点；cca router status、cca policy 说明参考节点" 'printf "%s" "$out" | grep -qx AI-A && printf "%s" "$out" | grep -qx AI-B && printf "%s\n" "$out2" | grep -qx "参考节点：维护者参考配置里策略组 OpenAi 的 4 个节点，OpenClash 的配置文件里都有" && printf "%s" "$out3" | grep -q "参考节点：维护者参考配置里策略组 OpenAi 的 4 个节点（路由器配置文件里缺的直接写进去；内置代理直接进内核的节点定义）"'
cp "$SRC" "$T/src.filled"; c0=$(grep -c '^CORE' router.log); r0=$(restarts); n0=$(reloads)
out=$(run cca router sync 2>&1); rc=$?
ok "再同步一次：配置文件里都有了，不再写、不重启也不重载，也不再拿内核校验" '[ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.filled" && [ "$(restarts)" = "$r0" ] && [ "$(reloads)" = "$n0" ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && ! printf "%s" "$out" | grep -q "写进 OpenClash 的配置文件"'
one_line "$SRC" AI-A '{name: "AI-A", type: "ss", server: "192.0.2.21", port: 8388, cipher: "aes-128-gcm", password: "x"}' '  '
out=$(run cca router sync 2>&1); rc=$?
ok "2.8.0-beta.5～7 写成一行的参考节点：同步时改成和别的节点一样的块写法（内容没变：不重启、不重载、不再拿内核校验）" '[ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.filled" && [ "$(restarts)" = "$r0" ] && [ "$(reloads)" = "$n0" ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && [ ! -e "$R/etc/cca/fill.pending" ]'
edit_table 'j["nodes"].append("AI-A")' "$H/.cca/config.json"; edit_table 'j["ai@example.com"] = "AI-A"'
out=$(run cca router sync -y 2>&1); rc=$?; fs=$(( $(cfgv 'c["ports"]["ai@example.com"]') - 7900 ))
ok "写进去的节点可以当账号的出口：选进可选节点、账号用它，端口切到 AI-A" '[ $rc -eq 0 ] && [ "$(sel "CCA-$fs")" = AI-A ]'
c0=$(grep -c '^CORE' router.log); r0=$(restarts); oc_restart
ok "OpenClash 重启：配置文件里本来就有，钩子不用再写（也不拿内核试），账号的端口照常走 AI-A" '[ "$(restarts)" = $((r0 + 1)) ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && rget "$RC" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$fs")" = AI-A ] && cmp -s "$SRC" "$T/src.filled"'
cp "$T/src.fill" "$SRC"; r0=$(restarts); oc_restart
out=$(run cca router sync 2>&1); rc=$?
ok "订阅更新把配置文件整个换掉了：OpenClash 启动时钩子写回去，这次生成的运行配置里也照样带上，不用为这个再重启一次" '[ "$(added "$T/src.fill" "$SRC")" = 5 ] && rget "$RC" "px.include?(\"AI-A\") && px.include?(\"AI-B\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$fs")" = AI-A ] && [ ! -e "$R/etc/cca/fill.pending" ] && grep -q "【cca】已把维护者参考配置里的节点写进配置文件 $SRC" "$R/tmp/openclash.log" && [ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ]'
echo 192.0.2.22 > "$R/core_reject"; rm -f "$R/etc/cca/fill.ok"; cp "$T/src.fill" "$SRC"; oc_restart
ok "这台路由器的内核不认参考配置里的节点（还没校验过的这一版）：启动时钩子先拿内核试，不通过就不写（OpenClash 照常起来，端口池照常），用它的账号宁可断开" 'cmp -s "$SRC" "$T/src.fill" && ! rget "$RC" "px.include?(\"AI-A\")" && [ "$(nl_)" -gt 0 ] && [ "$(sel "CCA-$fs")" = REJECT ] && grep -q "维护者参考配置里的节点没通过 OpenClash 内核的校验，没有写进配置文件" "$R/tmp/cca-router.log" && [ ! -e "$R/etc/cca/fill.ok" ] && [ -z "$(fill_tmp)" ]'
c0=$(grep -c '^CORE' router.log); out=$(run cca router status 2>&1); oc_restart
ok "没通过校验的记下了：再启动不再拿内核试；cca router status 说还缺什么、为什么" '[ "$(grep -c "^CORE" router.log)" = "$c0" ] && printf "%s" "$out" | grep -q "OpenClash 的配置文件里还要补上节点 AI-A、AI-B；策略组 OpenAi 里加上 AI-A、AI-B、US-B（这一版还没在这台路由器的内核上校验过）" && printf "%s" "$out" | grep -q "上次没写成：维护者参考配置里的节点没通过 OpenClash 内核的校验"'
out=$(run cca router sync -y 2>&1); rc=$?
ok "同步时照样再校验一次，内核还是不认：说明没写，配置文件不动，用它的账号断开" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者参考配置里的节点没通过 OpenClash 内核的校验，没有写进配置文件" && cmp -s "$SRC" "$T/src.fill" && [ "$(sel "CCA-$fs")" = REJECT ] && [ ! -e "$R/etc/cca/fill.ok" ]'
rm -f "$R/core_reject"; r0=$(restarts); n0=$(reloads)
out=$(run cca router sync -y 2>&1); rc=$?
ok "内核认了（比如升级了）：同步时写进配置文件，重启一次用上，账号的端口回到 AI-A" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(reloads)" = "$n0" ] && [ "$(added "$T/src.fill" "$SRC")" = 5 ] && rget "$RC" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$fs")" = AI-A ] && [ -s "$R/etc/cca/fill.ok" ]'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [x.update(port=8443) for x in j['proxies'] if x['name'] == 'AI-B']; json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
cp "$SRC" "$T/src.prev"; r0=$(restarts); out=$(run CCA_TTY="$(ans y)" cca policy pull 2>&1); rc=$?
ok "维护者改了参考配置里的节点：cca 写进去的那一行跟着改（不会多出一个），问过后重启" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "按参考配置更新了节点 AI-B" && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(diff "$T/src.prev" "$SRC" | grep -c "^[<>]")" = 2 ] && rget "$SRC" "px.count(\"AI-B\") == 1 && c[\"proxies\"].find { |p| p[\"name\"] == \"AI-B\" }[\"port\"] == 8443" && rget "$RC" "c[\"proxies\"].find { |p| p[\"name\"] == \"AI-B\" }[\"port\"] == 8443"'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [x.update(password='changed') for x in j['proxies'] if x['name'] == 'US-A']; json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
cp "$SRC" "$T/src.prev"; r0=$(restarts); out=$(run CCA_TTY="$(ans y)" cca policy pull 2>&1); rc=$?
ok "参考配置里和用户自己的节点同名的改了：用户的那个不动（不是 cca 写的），也不用重启" '[ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.prev" && [ "$(restarts)" = "$r0" ] && rget "$SRC" "c[\"proxies\"].find { |p| p[\"name\"] == \"US-A\" }[\"password\"] == \"x\""'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [x.update(password='y2') for x in j['proxies'] if x['name'] == 'AI-A']; json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
r0=$(restarts); out=$(run cca policy pull 2>&1); rc=$?
out2=$(run cca router status 2>&1)
ok "非交互时要重启也不重启：配置文件照样写好，说清楚这次的改动要等 OpenClash 下次重启；cca router status 也这么说" '[ "$(restarts)" = "$r0" ] && printf "%s" "$out" | grep -q "没有重启：这次的改动要等 OpenClash 下次重启" && [ -e "$R/etc/cca/fill.pending" ] && rget "$SRC" "c[\"proxies\"].find { |p| p[\"name\"] == \"AI-A\" }[\"password\"] == \"y2\"" && rget "$RC" "c[\"proxies\"].find { |p| p[\"name\"] == \"AI-A\" }[\"password\"] == \"x\"" && printf "%s\n" "$out2" | grep -qx "参考节点：维护者参考配置里策略组 OpenAi 的 4 个节点，OpenClash 的配置文件里都有（新写进去的要等 OpenClash 重启后才用上）"'
r0=$(restarts); n_cli=$(grep -c '^ARGS:' claude.log); out=$(run CCA_TTY="$(ans n)" cca @ai --version 2>&1); rc=$?
ok "在家启动账号：必要重启只问一次；拒绝后即使节点同名也阻断，不调用客户端" '[ $rc -eq 1 ] && [ "$(printf "%s" "$out" | grep -c "现在重启 OpenClash 吗")" = 1 ] && [ "$(restarts)" = "$r0" ] && [ "$(sel "CCA-$fs")" = AI-A ] && printf "%s" "$out" | grep -q "路由器账号节点未能核实" && [ "$(grep -c "^ARGS:" claude.log)" = "$n_cli" ]'
out=$(run CCA_TTY="$(ans y)" cca router sync 2>&1); rc=$?
ok "再同步时同意重启：用上配置文件里新写的" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && rget "$RC" "c[\"proxies\"].find { |p| p[\"name\"] == \"AI-A\" }[\"password\"] == \"y2\"" && [ ! -e "$R/etc/cca/fill.pending" ] && [ "$(sel "CCA-$fs")" = AI-A ]'
cp "$H/.cca/policy/nodes.json" "$T/nodes.good"; nodes_pub "$T/evilkey"
out=$(run cca policy pull 2>&1); rc=$?
nodes_pub; python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j['proxies'][0].pop('server'); json.dump(j, open(p, 'w'))" "$PR/nodes.json"; nodes_pub
out2=$(run cca policy pull 2>&1); rc2=$?
ok "参考节点不是发布密钥签的、格式不对：映射和参考节点都不换，接着用上次拉到的" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "参考节点（nodes.json）的签名不对" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "参考节点（nodes.json）的格式不对：第 1 个节点「AI-A」：server 不对" && cmp -s "$H/.cca/policy/nodes.json" "$T/nodes.good"'
cp "$T/nodes.good" "$PR/nodes.json"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j['proxies'].append({'name': 'SC-X', 'type': 'ss', 'server': '192.0.2.29', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'x'}); json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
# ShellCrash：订阅配置里有 OpenAi 组（块写法）；参考节点里的 SC-X 和它的自定义节点重名，不写
rm -rf "$R"; cp -R "$T/router.snap" "$R"; cp -R "$T/shellcrash/." "$R/"; echo CrashCore > "$R/core"
awk '{ sub(/^  - name: HK-A$/, "  - name: HK-A2") } /^rules:$/ { print "  - name: OpenAi"; print "    type: select"; print "    proxies:"; print "      - US-A" } { print }' "$T/shellcrash/etc/ShellCrash/yamls/config.yaml" > "$SCD/yamls/config.yaml"
cp "$SCD/yamls/config.yaml" "$T/sc_base.fill"; sc_restart; n0=$(reloads); r0=$(restarts)
out=$(run CCA_TTY="$(ans y)" cca policy pull 2>&1); rc=$?
ok "ShellCrash：缺的写进订阅配置、名字补进 OpenAi 组，问过后重启一次（不热加载）；和它的自定义节点重名的 SC-X 不写（说明原因，只有一个 SC-X）" '[ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 5 ] && rget "$SCR" "px.include?(\"AI-A\") && px.include?(\"AI-B\") && px.count(\"SC-X\") == 1 && g == %w[US-A AI-A AI-B US-B]" && printf "%s" "$out" | grep -q "参考节点「SC-X」和这里的策略组（或内置出站、ShellCrash 的自定义节点）重名，没有写" && printf "%s" "$out" | grep -q "现在重启 ShellCrash 吗" && [ "$(sel "CCA-$fs")" = AI-A ] && ! grep -q SCBAD router.log'
out=$(run cca router sync 2>&1); rc=$?
out2=$(run cca router nodes 2>&1)
ok "ShellCrash 再同步：配置文件里都有了，不用重启也不用重载；cca router nodes 列得出参考节点，不列自定义节点" '[ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = $((r0 + 1)) ] && printf "%s" "$out2" | grep -qx AI-A && ! printf "%s" "$out2" | grep -q SC-X'
cp "$T/sc_base.fill" "$SCD/yamls/config.yaml"; r0=$(restarts); sc_restart
ok "ShellCrash 的订阅更新把订阅配置换掉了：启动时钩子在它拼运行配置之前写回去，这次启动就用上" '[ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 5 ] && rget "$SCR" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$fs")" = AI-A ] && [ ! -e "$R/etc/cca/fill.pending" ] && ! grep -q SCBAD router.log'
python3 -c "import sys; p=sys.argv[1]; s=open(p).read(); b='  - name: OpenAi\n    type: select\n    proxies:\n      - US-A\n'; assert s.count(b) == 1; open(p, 'w').write(s.replace(b, '  - {name: OpenAi, type: select, proxies: [US-A]}\n'))" "$T/sc_base.fill"
cp "$T/sc_base.fill" "$SCD/yamls/config.yaml"; sc_restart; n0=$(reloads); r0=$(restarts)
out=$(run cca router sync 2>&1); rc=$?
ok "订阅配置里 OpenAi 组写成一行的：只写节点、组不动（说明为什么）；同步时不用再重载、重启" 'grep -q "策略组 OpenAi 不是一行一项的写法" "$R/tmp/cca-router.log" && [ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 2 ] && rget "$SCR" "px.include?(\"AI-A\") && g == %w[US-A]" && ! grep -q SCBAD router.log && [ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = "$r0" ]'
# 维护者的映射要某个账号走参考节点，这台机器的可选节点里还没有它：自动加进可选节点（记下摘要），账号随后照常加进来（新端口问过后生效）
pol '{"version": 1, "claude": {"alice@example.com": "HK-A2", "bea@example.com": "AI-B"}}'
out=$(run CCA_TTY="$(ans y y)" cca policy pull 2>&1); rc=$?
ok "映射要的节点是维护者的参考节点、还不在可选节点里：自动加进可选节点（说一声），映射里的账号照常加进来" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "维护者的映射要 bea@example.com 走节点 AI-B：它在维护者的参考配置里（同步时写进路由器的配置），已加进可选节点" && printf "%s" "$out" | grep -q "按维护者的映射添加了 Claude 账号 bea@example.com，出口 AI-B" && cfgok "\"AI-B\" in c[\"nodes\"] and c[\"node_ids\"].get(\"AI-B\")" && tblok "t.get(\"bea@example.com\") == \"AI-B\""'
pol '{"version": 1, "claude": {"alice@example.com": "HK-A2"}}'
# 还原：路由器、config.json、账号表回到这一节开头。维护者不再发布参考节点（映射仓库里删掉 nodes.json），拉下来后这台机器上的也删掉
rm -rf "$R"; cp -R "$T/router.snap" "$R"; cp "$T/config.snap" "$H/.cca/config.json"; cp "$T/accounts.snap" "$H/.cca/accounts.json"
rm -f "$PR/nodes.json" "$PR/nodes.json.sig"; out=$(run cca policy pull 2>&1)
ok "映射仓库里没有 nodes.json 了：这台机器上的参考节点也删掉" '[ ! -e "$H/.cca/policy/nodes.json" ] && ! printf "%s" "$out" | grep -q "参考节点："'
cp "$T/config.snap" "$H/.cca/config.json"; cp "$T/accounts.snap" "$H/.cca/accounts.json"

fi
if sect 11b2c; then
# 11b2c. 自定义节点写进路由器（cca node add / node use，App 账号页的「自定义代理…」）：和参考节点同一套写法（行尾带 # cca、先拿内核
#        校验、写了要重启才用上，问过才重启），只是不进任何策略组；分享链接以外也认 socks5:// 和 http(s)://。维护者的映射固定了节点的
#        账号不能换。这一节结束时路由器、config.json、账号表、自定义节点都还原
rm -rf "$T/router.snap2" "$T/core.snap2"; cp -R "$R" "$T/router.snap2"; cp "$H/.cca/config.json" "$T/config.snap2"; cp "$H/.cca/accounts.json" "$T/accounts.snap2"
[ ! -d "$H/.cca/core" ] || cp -Rp "$H/.cca/core" "$T/core.snap2"   # 校验节点要下载内置代理的内核：这一节结束时还原（11b4 测第一次下载）
CNF="$H/.cca/core/custom-nodes.json"
ckof() {   # ckof '节点 JSON'：自定义节点的身份摘要 ck（辅助程序的 custom_ck、路由器端 apply.rb 的 fnv(ckey(…)) 是同一个算法，这里另写一份核对）
	python3 -c '
import json, sys
p = json.loads(sys.argv[1])
s = lambda v: "" if v is None else ("true" if v is True else "false" if v is False else str(v))
h = lambda v: s(v).strip().lower().rstrip(".")
sni = p.get("servername")
if sni is None or sni is False: sni = p.get("sni")
k = "|".join([s(p.get("type")).strip().lower(), h(p.get("server")), s(p.get("port")).strip(), h(sni)] + [s(p.get(f)) for f in ("username", "password", "uuid")])
x = 0xcbf29ce484222325
for b in k.encode(): x = ((x ^ b) * 0x100000001b3) & 0xffffffffffffffff
print("%016x" % x)' "$1"
}
out=$(run CCA_TTY="$(ans 'socks5://u:p@198.51.100.7:1080' .)" cca node use alice 2>&1); rc=$?
ok "cca node use：维护者的映射固定了节点的账号不能换（不问链接，什么都不存）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者的映射固定了 alice@example.com 的出口节点" && ! printf "%s" "$out" | grep -q "粘贴" && [ ! -e "$CNF" ]'
edit_table 'j["cu@example.com"] = "US-B"'; run cca router sync -y >/dev/null 2>&1
cs=$(( $(cfgv 'c["ports"]["cu@example.com"]') - 7900 )); cp "$SRC" "$T/src.cu"; n0=$(reloads); r0=$(restarts); : > ssh.log
out=$(run CCA_TTY="$(ans y)" cca node use cu 'socks5://u%40x:p%3Aw@198.51.100.7:1080#Home%20IP' 2>&1); rc=$?
ok "cca node use 账号 socks5 链接（配置了路由器、在家）：解析、本机内核校验、存成自定义节点（600），账号改走它；同步时写进 OpenClash 选用的配置文件（行尾带 # cca，不进任何策略组），问过后重启一次（不热加载），账号的端口切到它" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "cu@example.com 改走自定义代理「Home IP」" && printf "%s" "$out" | grep -qF "已把自定义节点写进 OpenClash 的配置文件 ${SRC}：补上自定义节点 Home IP（行尾带 # cca" && printf "%s" "$out" | grep -q "OpenClash 的配置文件里新写了自定义节点，要用上只能重启一次 OpenClash" && printf "%s" "$out" | grep -q "现在重启 OpenClash 吗" && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(reloads)" = "$n0" ] && [ "$(stat -f %Lp "$CNF")" = 600 ] && [ "$(added "$T/src.cu" "$SRC")" = 1 ] && rget "$SRC" "c[\"proxies\"].find { |p| p[\"name\"] == \"Home IP\" } == {\"name\" => \"Home IP\", \"type\" => \"socks5\", \"server\" => \"198.51.100.7\", \"port\" => 1080, \"username\" => \"u@x\", \"password\" => \"p:w\"} && c[\"proxy-groups\"].none? { |x| x[\"proxies\"].to_a.include?(\"Home IP\") }" && tblok "t[\"cu@example.com\"] == \"Home IP\"" && [ "$(sel "CCA-$cs")" = "Home IP" ] && rget "$RC" "px.include?(\"Home IP\")"'
ok "节点的用户名、密码只经 ssh 的 stdin 交给路由器（不进命令行）；它进了可选节点，记下了节点指纹" '! grep -q "p:w\|p%3Aw\|u@x" ssh.log && grep -q "\"custom\": {\"id\": " "$R/etc/cca/accounts.yaml" && cfgok "\"Home IP\" in c[\"nodes\"] and c[\"node_ids\"].get(\"Home IP\")"'
ckh=$(python3 -c 'import json,sys; print(next((a.get("ck", "") for a in json.load(open(sys.argv[1]))["accounts"] if a["slot"] == int(sys.argv[2])), ""))' "$R/etc/cca/accounts.yaml" "$cs")
ckw=$(ckof '{"type": "socks5", "server": "198.51.100.7", "port": 1080, "username": "u@x", "password": "p:w"}')   # 期望值在断言外面算：bash 3.2 解析双引号里的 $(… "{\"…\"}") 会把参数拆碎
ok "用自定义节点的槽位带着节点的身份摘要 ck（连用户名、密码一起算），别的槽位不带" '[ -n "$ckh" ] && [ "$ckh" = "$ckw" ] && python3 -c "import json,sys; sys.exit(0 if all(\"ck\" not in a for a in json.load(open(sys.argv[1]))[\"accounts\"] if a[\"slot\"] != int(sys.argv[2])) else 1)" "$R/etc/cca/accounts.yaml" "$cs"'
out=$(run cca router status 2>&1); out2=$(run cca router nodes 2>&1); c0=$(grep -c '^CORE' router.log); r0=$(restarts); n0=$(reloads); cp "$SRC" "$T/src.cu2"
out3=$(run cca router sync 2>&1); rc=$?
ok "cca router status 说明自定义节点，cca router nodes 列得出它；再同步：配置文件里有了，不再写、不重启也不重载、不再拿内核校验" 'printf "%s\n" "$out" | grep -qx "自定义节点：1 个，OpenClash 的配置文件里都有" && printf "%s" "$out2" | grep -qx "Home IP" && [ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.cu2" && [ "$(restarts)" = "$r0" ] && [ "$(reloads)" = "$n0" ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && [ "$(sel "CCA-$cs")" = "Home IP" ]'
cp "$T/src.cu" "$SRC"; r0=$(restarts); oc_restart
ok "订阅更新把配置文件整个换掉了：OpenClash 启动时钩子写回去，这次的运行配置里也照样带上（不用再重启），账号的端口照常走它" '[ "$(added "$T/src.cu" "$SRC")" = 1 ] && rget "$RC" "px.include?(\"Home IP\")" && [ "$(sel "CCA-$cs")" = "Home IP" ] && [ "$(restarts)" = $((r0 + 1)) ] && [ ! -e "$R/etc/cca/fill.pending" ] && grep -q "【cca】已把自定义节点写进配置文件 $SRC" "$R/tmp/openclash.log"'
r0=$(restarts)
out=$(sess '{"node.custom":[{"link":"这不是链接"},{"link":"http://a:b@198.51.100.10:3128#Office"}],"consent.router_restart":[{"v":"y"}]}' -- cca node use cu 2>&1); rc=$?
ok "App 的「自定义代理…」（提问会话 cca node use）：粘贴代理链接，认不出的带着原因重问；同步时要重启 OpenClash 在会话里问（consent.router_restart），同意后重启；做完 result 是 saved、带账号" '[ $rc -eq 0 ] && [ "$(asks)" = "node.custom node.custom consent.router_restart" ] && evq "e.get(\"q\") == \"node.custom\" and \"认不出\" in (e[\"fields\"][0].get(\"error\") or \"\")" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"saved\" and e[\"key\"] == \"cu@example.com\" and \"Office\" in e[\"message\"]" && [ "$(restarts)" = $((r0 + 1)) ] && tblok "t[\"cu@example.com\"] == \"Office\"" && [ "$(sel "CCA-$cs")" = Office ] && rget "$SRC" "c[\"proxies\"].find { |p| p[\"name\"] == \"Office\" } == {\"name\" => \"Office\", \"type\" => \"http\", \"server\" => \"198.51.100.10\", \"port\" => 3128, \"username\" => \"a\", \"password\" => \"b\"}"'
out=$(run cca _gui state 2>/dev/null)
ok "_gui state：can.custom_proxy——维护者固定了节点的账号是 false，别的是 true" 'printf "%s" "$out" | jok "[a[\"can\"][\"custom_proxy\"] for a in d[\"tools\"][0][\"accounts\"] if a[\"key\"] in (\"alice@example.com\", \"cu@example.com\")] == [False, True]"'
# 多设备共用一个路由器：扮演另一台设备（owner bbbb…，直接写路由器的 accounts.yaml + owners，见 11a2），它的账号 dave-dev2
# 用它那台的节点。路由器按槽位带着的 ck 核对自定义节点，同名却不是同一个的宁可断开。这几个用例结束时还原到这里
rm -rf "$T/router.md"; cp -R "$R" "$T/router.md"; cp "$H/.cca/config.json" "$T/config.md"; cp "$H/.cca/accounts.json" "$T/acct.md"; cp -p "$CNF" "$T/cnf.md"
mdslot() {   # mdslot 节点 ck [custom]：另一台设备的账号占第一个空槽位（输出槽位号）；ck 空着就不带。custom 是节点 JSON 时
	          # 账号表的 custom 换成只有它（像那台最后同步过），是 - 时去掉（像旧版 cca 最后同步过）
	python3 - "$R/etc/cca/accounts.yaml" "$R/etc/cca/owners" "$@" <<'PY'
import json, sys
acc = json.load(open(sys.argv[1])); own = json.load(open(sys.argv[2]))
acc['accounts'] = [a for a in acc['accounts'] if a['name'] != 'dave-dev2']
used = {a['slot'] for a in acc['accounts']}
s = next(i for i in range(1, 99) if i not in used)
a = {'slot': s, 'name': 'dave-dev2', 'node': sys.argv[3]}
if sys.argv[4]: a['ck'] = sys.argv[4]
acc['accounts'].append(a); acc['slots'] = max(acc.get('slots', 0), s)
if len(sys.argv) > 5:
    if sys.argv[5] == '-': acc.pop('custom', None)
    else: acc['custom'] = {'id': 'other', 'proxies': [json.loads(sys.argv[5])]}
own = {k: v for k, v in own.items() if v != 'bbbb1111bbbb1111'}; own[str(s)] = 'bbbb1111bbbb1111'
json.dump(acc, open(sys.argv[1], 'w'), ensure_ascii=False); json.dump(own, open(sys.argv[2], 'w'))
print(s)
PY
}
s2=$(mdslot 'Home IP' 0123456789abcdef)
out=$(run cca router sync -y 2>&1)
ok "多设备：另一台的账号用它自己的「Home IP」（同名，ck 不一样）：它的槽位宁可断开（REJECT），不走这台写进去的；这台的账号照常；它的槽位和 ck 原样保留" '[ "$(sel "CCA-$s2")" = REJECT ] && [ "$(sel "CCA-$cs")" = Office ] && printf "%s" "$out" | grep -q "槽位 $s2 的「Home IP」和账号要的自定义节点不是同一个" && grep -q "\"name\": \"dave-dev2\", \"node\": \"Home IP\", \"ck\": \"0123456789abcdef\"" "$R/etc/cca/accounts.yaml"'
s2=$(mdslot 'Home IP' "$ckh")
out=$(run cca router sync -y 2>&1)
ok "多设备：另一台的「Home IP」和这台写进去的是同一个（ck 一样）：它的槽位照常走它" '[ "$(sel "CCA-$s2")" = "Home IP" ] && [ "$(sel "CCA-$cs")" = Office ] && grep -q "\"name\": \"dave-dev2\", \"node\": \"Home IP\", \"ck\": \"$ckh\"" "$R/etc/cca/accounts.yaml"'
SB='{"name": "Spare B", "type": "http", "server": "198.51.100.20", "port": 8080, "username": "dave", "password": "pw"}'
s2=$(mdslot 'Spare B' "$(ckof "$SB")" "$SB")
out=$(run cca router sync -y 2>&1)
ok "多设备：另一台在用的自定义节点（账号表的 custom 里有、它的槽位的 ck 是它）这台同步时照样带着：写进配置文件，它的槽位走它" '[ "$(sel "CCA-$s2")" = "Spare B" ] && [ "$(sel "CCA-$cs")" = Office ] && rget "$SRC" "c[\"proxies\"].any? { |p| p[\"name\"] == \"Spare B\" && p[\"username\"] == \"dave\" }" && python3 -c "import json,sys; sys.exit(0 if [p[\"name\"] for p in json.load(open(sys.argv[1]))[\"custom\"][\"proxies\"]] == [\"Home IP\", \"Office\", \"Spare B\"] else 1)" "$R/etc/cca/accounts.yaml"'
cp "$T/src.cu" "$SRC"; oc_restart
ok "订阅更新把配置文件整个换掉了：钩子照账号表把另一台在用的自定义节点也写回去，它的槽位照常（不用等那台再同步）" '[ "$(added "$T/src.cu" "$SRC")" = 3 ] && [ "$(sel "CCA-$s2")" = "Spare B" ] && [ "$(sel "CCA-$cs")" = Office ]'
# 这台在外面改了「Office」（密码换了，名字没变；直接改 custom-nodes.json）
ofc_b2() { python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [q.update(password='b2') for q in j['proxies'] if q['name'] == 'Office']; json.dump(j, open(p, 'w'), ensure_ascii=False)" "$CNF"; }
ofc_b2; cp "$SRC" "$T/src.md"
OFC='{"name": "Office", "type": "http", "server": "198.51.100.10", "port": 3128, "username": "a", "password": "b"}'
s2=$(mdslot Office '' -)
out=$(run cca router sync -y 2>&1)
ok "多设备：另一台的槽位没带 ck（旧版 cca 同步过，账号表里也没有 custom）：当它用着路由器上的「Office」，路由器端不换成这台改过的、说为什么；这台用它的槽位断开，另一台的照常" 'cmp -s "$SRC" "$T/src.md" && printf "%s" "$out" | grep -q "自定义节点「Office」和这里的同名节点不是同一个、那个还有账号在用（可能是别的设备的），没有写" && [ "$(sel "CCA-$cs")" = REJECT ] && [ "$(sel "CCA-$s2")" = Office ]'
s2=$(mdslot Office "$(ckof "$OFC")" "$OFC"); r0=$(restarts)
out=$(run cca router sync -y 2>&1)
ok "多设备：路由器上的「Office」另一台正用着（账号表的 custom 里有、它的槽位的 ck 是原来的那个）：这台改过的那个改名为「Office-2」（出口不变）再同步，两台都照常" 'printf "%s" "$out" | grep -q "自定义节点「Office」和别的设备在路由器上正用着的同名节点不是同一个：这台的改名为「Office-2」" && tblok "t[\"cu@example.com\"] == \"Office-2\"" && cfgok "\"Office-2\" in c[\"nodes\"] and \"Office\" not in c[\"nodes\"] and c[\"node_ids\"].get(\"Office-2\")" && [ "$(sel "CCA-$cs")" = Office-2 ] && [ "$(sel "CCA-$s2")" = Office ] && [ "$(restarts)" = $((r0 + 1)) ] && rget "$SRC" "c[\"proxies\"].any? { |p| p[\"name\"] == \"Office-2\" && p[\"password\"] == \"b2\" } && c[\"proxies\"].any? { |p| p[\"name\"] == \"Office\" && p[\"password\"] == \"b\" }"'
# 只有这台：路由器、config.json、账号表回到上面那几个用例之前，这台的「Office」照样是改过的
rm -rf "$R"; cp -R "$T/router.md" "$R"; cp "$T/config.md" "$H/.cca/config.json"; cp "$T/acct.md" "$H/.cca/accounts.json"; cp -p "$T/cnf.md" "$CNF"; ofc_b2; r0=$(restarts)
out=$(run cca router sync -y 2>&1)
ok "只有这台在用：改过的「Office」照新的更新进配置文件（问过后重启一次），账号的端口走它" 'rget "$SRC" "c[\"proxies\"].any? { |p| p[\"name\"] == \"Office\" && p[\"password\"] == \"b2\" }" && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(sel "CCA-$cs")" = Office ] && printf "%s" "$out" | grep -q "更新了自定义节点 Office"'
rm -rf "$R"; cp -R "$T/router.md" "$R"; cp "$T/config.md" "$H/.cca/config.json"; cp "$T/acct.md" "$H/.cca/accounts.json"; cp -p "$T/cnf.md" "$CNF"
# 用户在 OpenClash 的配置里把 cca 写的「Office」换成了自己的（去掉记号、换了服务器）：同名不同的节点不写，用它的端口断开（不换出口）
python3 -c "import sys; p=sys.argv[1]; s=open(p).read(); a='  - name: Office # cca\n'; assert s.count(a) == 1; s=s.replace(a, '  - name: Office\n').replace('server: 198.51.100.10', 'server: 192.0.2.77'); open(p, 'w').write(s)" "$SRC"
oc_restart; cp "$SRC" "$T/src.cu3"
out=$(run cca router sync -y 2>&1); rc=$?; out2=$(run cca router status 2>&1)
ok "配置文件里有同名、却不是同一个节点（用户自己的）：不写、不改用户的，用它的端口宁可断开（REJECT）；cca router status 说为什么" 'cmp -s "$SRC" "$T/src.cu3" && [ "$(sel "CCA-$cs")" = REJECT ] && grep -q "槽位 $cs 的「Office」和账号要的自定义节点不是同一个" "$R/tmp/cca-router.log" && printf "%s" "$out2" | grep -q "自定义节点「Office」和这里的同名节点不是同一个，没有写"'
out=$(run cca node rm Office 2>&1); rc=$?
ok "cca node rm：还有账号在用时不删" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "还有账号在用「Office」"'
# 还原：路由器、config.json、账号表、内置代理的目录（自定义节点、下载的内核）回到这一节开头
rm -rf "$R" "$H/.cca/core"; cp -R "$T/router.snap2" "$R"; cp "$T/config.snap2" "$H/.cca/config.json"; cp "$T/accounts.snap2" "$H/.cca/accounts.json"
[ ! -d "$T/core.snap2" ] || cp -Rp "$T/core.snap2" "$H/.cca/core"

fi
if sect 11b3; then
# 11b3. 维护者管理各台机器的 deploy key（policy.sh key）：假 gh 模拟 GitHub 的 deploy key 接口（存在 $T/deploy_keys.json），
# 有 $T/policy_keys 时假 git 按它认 deploy key（没加上就拒绝，认出来了记下最近使用）
PS="$D/../policy.sh"; touch "$T/policy_keys"; : > ghcli.log
NOPD="CCA_POLICY_DIR=$T/no-policy-dir"   # 仓库里的 policy.sh 默认看仓库旁边的 cca-policy（维护者真的映射）：测试里指到沙箱
pks() { local a="$1"; shift; run $P $NOPD CCA_TTY="$a" bash "$PS" key "$@"; }   # pks 答案文件 子命令…
dkt() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(\"|\".join(\"%s:%s\" % (k[\"title\"], \"ro\" if k[\"read_only\"] else \"rw\") for k in d[\"repos\"].get(\"example/cca-policy\", [])))" "$T/deploy_keys.json" 2>/dev/null; }   # 假 GitHub 上的 key：别名:ro|rw
dkk() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(\"\".join(k[\"key\"] for k in d[\"repos\"][\"example/cca-policy\"] if k[\"title\"] == sys.argv[2]))" "$T/deploy_keys.json" "$1"; }   # dkk 别名：那把 key 的公钥
posts() { grep -c '^GH api POST' ghcli.log; }
out=$(pks "$(ans)" 2>&1); rc=$?
ok "policy.sh key：仓库上还没有 deploy key 时说明怎么加" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "example/cca-policy 上还没有 deploy key：有人发来公钥后复制它，运行 ./policy.sh key add"'
rm -f "$T/clipboard"
out=$(run $P CCA_TTY="$(ans)" cca policy setup 2>&1)
ok "这台机器的公钥还没加到仓库上：拉不到映射；给维护者的那一行（带着公钥）复制到了剪贴板" '[ "$(cat "$T/clipboard")" = "./policy.sh key add $(kl)" ] && printf "%s" "$out" | grep -q "GitHub 不认这台机器的 deploy key"'
out=$(pks "$(ans alice-mbp)" add 2>&1); rc=$?
ok "policy.sh key add：读剪贴板里的公钥（认出是这台机器的），提示起别名，加成只读 deploy key" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "要加的公钥：SHA256:.*（cca-policy@.*），是这台机器的" && printf "%s" "$out" | grep -q "给它起个别名（谁的哪台机器，以后按别名查、删、改） \[" && printf "%s" "$out" | grep -q "已把「alice-mbp」加成 example/cca-policy 的只读 deploy key" && [ "$(dkt)" = alice-mbp:ro ] && [ "$(dkk alice-mbp)" = "$(cut -d" " -f1,2 "$PK.pub")" ]'
out=$(run cca policy pull 2>&1); rc=$?
ok "维护者加上之后：这台机器拉得到映射" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "映射没有变化"'
out=$(pks "$(ans)" add 2>&1); rc=$?
ok "同一把公钥再加一次：说已经加过了、别名是什么，不重复加" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "这把公钥已经加过了，别名「alice-mbp」" && [ "$(posts)" = 1 ]'
ssh-keygen -q -t ed25519 -N "" -C "cca-policy@bob-mini" -f "$T/bob"; ssh-keygen -q -t ed25519 -N "" -C "cca-policy@carol-air" -f "$T/carol"
out=$(pks "$(ans " alice-mbp " '')" add "$T/bob.pub" 2>&1); rc=$?
ok "从文件加：别名重复了（不计首尾空格）要换一个；直接回车用公钥备注里的机器名" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "别名「alice-mbp」已经有了，换一个" && [ "$(dkt)" = "alice-mbp:ro|bob-mini:ro" ]'
cp "$T/bob" "$T/clipboard"
out=$(pks "$(ans x)" add 2>&1); rc=$?
ok "剪贴板里是私钥：不加，提醒别再外传" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "剪贴板里是私钥，不是公钥" && [ "$(posts)" = 2 ]'
out=$(printf '这是我的公钥：%s 谢谢\n' "$(cat "$T/carol.pub")" | run $P $NOPD bash "$PS" key add - "Carol 的 Air" 2>&1); rc=$?
ok "从标准输入加、别名写在命令后面（不提问）；夹在聊天消息里的公钥也认得出" '[ $rc -eq 0 ] && [ "$(dkt)" = "alice-mbp:ro|bob-mini:ro|Carol 的 Air:ro" ] && [ "$(dkk "Carol 的 Air")" = "$(cut -d" " -f1,2 "$T/carol.pub")" ]'
out=$(pks "$(ans)" ls 2>&1); rc=$?; today=$(date +%F)
ok "policy.sh key：列出各台机器的 key（添加时间、最近一次拉取、别名），标出这台机器的" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "example/cca-policy 上有 3 把 deploy key" && printf "%s\n" "$out" | grep -qx "  $today  $today  alice-mbp（这台机器）" && printf "%s\n" "$out" | grep -qx "  $today  还没用过    bob-mini" && printf "%s\n" "$out" | grep -qx "  $today  还没用过    Carol 的 Air"'
out=$(pks "$(ans)" mv bob-mini "Bob 的 mini" 2>&1); rc=$?
ok "policy.sh key mv：改别名（GitHub 的 deploy key 不能改：删掉后用同一把公钥重新加上，还是只读）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "「bob-mini」改叫「Bob 的 mini」" && [ "$(dkt)" = "alice-mbp:ro|Carol 的 Air:ro|Bob 的 mini:ro" ] && [ "$(dkk "Bob 的 mini")" = "$(cut -d" " -f1,2 "$T/bob.pub")" ]'
out=$(pks "$(ans "Carol 的 Air" '' carol)" mv alice-mbp 2>&1); rc=$?
ok "改别名不写新别名就提示；新别名已经有了、是空的都要重输" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "「alice-mbp」的新别名：" && printf "%s" "$out" | grep -q "别名「Carol 的 Air」已经有了，换一个" && printf "%s" "$out" | grep -q "别名不能是空的" && [ "$(dkt)" = "Carol 的 Air:ro|Bob 的 mini:ro|carol:ro" ]'
out=$(pks "$(ans)" mv carol "Bob 的 mini" 2>&1); rc=$?
ok "命令里写的新别名已经有了：不改" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "别名「Bob 的 mini」已经有了" && [ "$(dkt)" = "Carol 的 Air:ro|Bob 的 mini:ro|carol:ro" ]'
pks "$(ans)" mv carol alice-mbp >/dev/null 2>&1
out=$(pks "$(ans)" rm "Carol 的 Air" 2>&1); rc=$?
out2=$(pks "$(ans y)" rm "Carol 的 Air" 2>&1); rc2=$?
ok "policy.sh key rm：先确认（默认不删）；确认后删掉，并给出它的公钥（要加回来时用）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "删掉「Carol 的 Air」吗？" && printf "%s" "$out" | grep -q "没删" && [ $rc2 -eq 0 ] && [ "$(dkt)" = "Bob 的 mini:ro|alice-mbp:ro" ] && printf "%s" "$out2" | grep -qF "$(cut -d" " -f1,2 "$T/carol.pub")"'
out=$(pks "$(ans)" rm nobody 2>&1); rc=$?
ok "没有这个别名：说明现有哪些" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "example/cca-policy 上没有叫「nobody」的 key（现有：Bob 的 mini、alice-mbp）"'
out=$(pks "$(ans)" rm -y alice-mbp 2>&1); rc=$?
out2=$(run cca policy pull 2>&1); rc2=$?
ok "删掉这台机器的 key（-y 不问）：这台机器拉不到新映射了，已经拉到的照用" '[ $rc -eq 0 ] && [ "$(dkt)" = "Bob 的 mini:ro" ] && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "GitHub 不认这台机器的 deploy key" && [ -f "$H/.cca/policy/policy.json" ]'
touch "$T/gh_post_fail"; out=$(pks "$(ans)" mv "Bob 的 mini" bob 2>&1); rc=$?; rm -f "$T/gh_post_fail"
ok "改别名时旧的删掉了、新的没加上（GitHub 出错，试了三次）：说清楚，并给出加回来的那一行（公钥写在命令里）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "旧的「Bob 的 mini」已经删了，可新的没加上：Server Error" && printf "%s" "$out" | grep -qxF "  ./policy.sh key add -R example/cca-policy '\''$(cut -d" " -f1,2 "$T/bob.pub")'\'' '\''bob'\''" && [ "$(dkt)" = "" ] && [ "$(grep -c "^GH api POST .* title=bob$" ghcli.log)" = 3 ]'
# cca 给出的那一行：公钥和别名都写在命令里（服务器上没有剪贴板，用户复制那一行发给维护者）
ssh-keygen -q -t ed25519 -N "" -C "cca-policy@srv-a" -f "$T/srva"; ssh-keygen -q -t ed25519 -N "" -C "cca-policy@srv-a" -f "$T/srvb"
: > "$T/clipboard"
out=$(pks "$(ans)" add "$(cat "$T/srva.pub")" srv-a 2>&1); rc=$?
ok "policy.sh key add '公钥' 别名（cca 给出的那一行）：认命令里写的公钥，不读剪贴板、不提问" '[ $rc -eq 0 ] && [ "$(dkt)" = "srv-a:ro" ] && [ "$(dkk srv-a)" = "$(cut -d" " -f1,2 "$T/srva.pub")" ] && ! printf "%s" "$out" | grep -q "起个别名"'
out=$(pks "$(ans srv-b)" add "$(cat "$T/srvb.pub")" srv-a 2>&1); rc=$?
ok "两台机器同名（命令里写的别名已经有了）：有终端时另问一个（不拿重名的当默认值）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "别名「srv-a」已经有了，换一个" && printf "%s" "$out" | grep -q "给它起个别名（谁的哪台机器，以后按别名查、删、改）：" && [ "$(dkt)" = "srv-a:ro|srv-b:ro" ]'
pks "$(ans)" rm -y srv-a >/dev/null 2>&1; pks "$(ans)" rm -y srv-b >/dev/null 2>&1
touch "$T/gh_down"; out=$(pks "$(ans)" 2>&1); rc=$?; rm -f "$T/gh_down"
ok "连不上 GitHub：说明原因" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "读不到 example/cca-policy 上的 deploy key：连不上 GitHub（error connecting to api.github.com）"'
defrepo=$(sed -n 's/^POLICY_REPO_DEFAULT="\${CCA_POLICY_DEFAULT-\([^}]*\)}"$/\1/p' "$CCA")
out=$(run $NOPD CCA_TTY="$(ans)" bash "$PS" key 2>&1); rc=$?
out2=$(run $NOPD CCA_TTY="$(ans)" bash "$PS" key -R example/other 2>&1); rc2=$?
: > ghcli.log; env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" "$NOPD" CCA_TTY="$(ans)" PATH="$T/bin:/usr/bin:/bin" bash "$PS" key >/dev/null 2>&1
ok "仓库：CCA_POLICY_DEFAULT 设成空、又没写 -R 时不猜；-R 指定别的仓库；都没设时用 cca 里写的默认仓库" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不知道是哪个映射仓库：加上 -R 维护者/仓库" && [ $rc2 -eq 0 ] && printf "%s" "$out2" | grep -q "example/other 上还没有 deploy key" && [ -n "$defrepo" ] && grep -q "^GH api GET repos/$defrepo/keys" ghcli.log'
rm -f "$T/policy_keys"

fi
if sect 11b3b; then
# 11b3b. 把账号锁进映射：这台机器生成一条命令（cca policy request），维护者在 cca 的仓库里直接运行（policy.sh lock），确认后写进映射、
#        签名推送，再拉回来这个账号就被固定了。policy.sh 读旁边那份 cca 的发布公钥：放在假的维护者目录里，用测试发布密钥的 cca；
#        映射仓库就放在它旁边（默认的位置）。这一节用自己的映射，结束时账号表、配置、映射、假的映射仓库都还原
PD="$H/.cca/policy/policy.json"
for f in "$H/.cca/accounts.json" "$H/.cca/codex.json" "$H/.cca/config.json" "$PD" "$PR/policy.json" "$PR/policy.json.sig"; do cp -p "$f" "$T/lock.$(basename "$f")"; done
MT="$T/maint"; MR="$T/cca-policy"; mkdir -p "$MT" "$MR"; cp -p "$PS" "$MT/policy.sh"; cp "$H/.cca/bin/cca" "$MT/cca"
printf '%s\n' '{"version": 1, "claude": {"alice@example.com": "US-C"}}' | tee "$PD" > "$MR/policy.json"
git -C "$MR" init -q; git -C "$MR" config user.name t; git -C "$MR" config user.email t@example.com
git -C "$MR" remote add origin git@github.com:example/cca-policy.git; git -C "$MR" add -A; git -C "$MR" commit -qm init
edit_table 'j["工作(bob@example.com)"] = "US-B"'   # 这时账号表里只有 alice（映射固定）和第三方 API：加一个映射里没有的
LK="工作(bob@example.com)"; LE=bob@example.com; LN=US-B
AK=$(python3 -c 'import json,sys; print(next(k for k, v in json.load(open(sys.argv[1])).items() if isinstance(v, dict)))' "$H/.cca/accounts.json")
: > "$T/clipboard"
out=$(run cca policy request "$LK" 2>"$T/lock.err"); rc=$?
ok "cca policy request：写一条维护者直接运行的命令（账号和它现在的节点，不带别名），复制到剪贴板" '[ $rc -eq 0 ] && [ "$out" = "./policy.sh lock claude $LE $LN" ] && [ "$(cat "$T/clipboard")" = "$out" ] && grep -q "在 cca 的仓库里运行这条命令" "$T/lock.err"'
out=$(run cca policy request alice@example.com 2>&1); rc=$?
ok "已经被映射固定的账号不用请求：说清楚固定在哪个节点上" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "已经按维护者的映射固定在 US-C 上了" && ! printf "%s" "$out" | grep -q policy.sh'
out=$(run cca policy request "$AK" 2>&1); rc=$?
ok "第三方 API 账号不能请求（映射只管官方账号）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "映射只管官方账号"'
out=$(run cca policy request 没这个账号 2>&1); rc=$?
ok "没有这个账号：说清楚" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没找到账号"'
: > "$T/clipboard"
out=$(runev cca policy request "$LK" 2>/dev/null); rc=$?
ok "图形界面（有事件文件）：stdout 还是那条命令，由 App 自己复制，cca 不碰剪贴板" '[ $rc -eq 0 ] && [ "$out" = "./policy.sh lock claude $LE $LN" ] && [ ! -s "$T/clipboard" ]'
out=$(runev cca policy request alice@example.com 2>/dev/null); rc=$?
ok "图形界面：不能请求时 fail 事件里写着原因，stdout 没有内容" '[ $rc -ne 0 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and \"已经按维护者的映射固定\" in e[\"message\"]"'
out=$(run cca _gui state 2>/dev/null </dev/null)
ok "_gui state：can.policy_lock：没被映射固定的官方账号能请求，被固定的、第三方 API 账号不能" 'printf "%s" "$out" | LK="$LK" AK="$AK" jok "(lambda a: a[__import__(\"os\").environ[\"LK\"]][\"can\"][\"policy_lock\"] and not a[__import__(\"os\").environ[\"AK\"]][\"can\"][\"policy_lock\"] and not next(v for k, v in a.items() if \"alice@example.com\" in k)[\"can\"][\"policy_lock\"])({x[\"key\"]: x for x in d[\"tools\"][0][\"accounts\"]})"'
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d.pop("policy", None); json.dump(d, open(sys.argv[1], "w"))' "$H/.cca/config.json"
out=$(run cca policy request "$LK" 2>&1); rc=$?
out2=$(run cca _gui state 2>/dev/null </dev/null)
ok "这台机器没打开共享：不能请求（先 cca policy on），App 里也没有这个入口" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没打开维护者的共享：先运行 cca policy on" && printf "%s" "$out2" | jok "not any(x[\"can\"][\"policy_lock\"] for t in d[\"tools\"] for x in t[\"accounts\"])"'
cp -p "$T/lock.config.json" "$H/.cca/config.json"
out=$(run cca codex policy request dave@example.com 2>/dev/null); rc=$?
ok "cca codex policy request：命令里写着是 Codex 的账号" '[ $rc -eq 0 ] && [ "$out" = "./policy.sh lock codex dave@example.com US-D" ]'
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["nodes"].append("HK 香港 (1)"); json.dump(d, open(sys.argv[1], "w"), ensure_ascii=False)' "$H/.cca/config.json"
edit_table 'j["weird@example.com"] = "HK 香港 (1)"'
out=$(run cca policy request weird@example.com 2>/dev/null); rc=$?
args=$(eval "$(printf '%s' "$out" | sed 's#^\./policy\.sh#printf "[%s]"#')")
ok "节点名里有空格、括号、中文：命令里加了引号，粘贴运行时还是一个参数" '[ $rc -eq 0 ] && [ "$args" = "[lock][claude][weird@example.com][HK 香港 (1)]" ]'
edit_table 'j.pop("weird@example.com")'; cp -p "$T/lock.config.json" "$H/.cca/config.json"
# 维护者这边：拿到的就是这条命令，在 cca 的仓库里直接运行，看过、确认才写
runcmd() { out=$(run CCA_RELEASE_KEY="$T/relkey" CCA_TTY="$1" bash -c "cd '$MT' && $2" 2>&1); rc=$?; }   # runcmd 答案文件 命令
CMD=$(run cca policy request "$LK" 2>/dev/null)
head0=$(git -C "$MR" rev-parse HEAD); sum0=$(cat "$MR/policy.json")
runcmd "$(ans n)" "$CMD"
ok "policy.sh lock：发来的命令直接运行（映射仓库默认在 cca 仓库旁边），列出会改什么、问过才写；回答 n 什么都不改" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "${LE} → ${LN}（新加）" && printf "%s" "$out" | grep -q "写进 policy.json、签名并推送吗" && printf "%s" "$out" | grep -q "没有改" && [ "$(cat "$MR/policy.json")" = "$sum0" ] && [ "$(git -C "$MR" rev-parse HEAD)" = "$head0" ]'
: > git.log
runcmd "$(ans y)" "$CMD"
printf 'cca-release %s\n' "$(cut -d' ' -f1,2 "$T/relkey.pub")" > "$T/lock.allowed"
ok "policy.sh lock：确认后写进 policy.json（不带别名）、用发布密钥签名（cca-policy）、提交并推送" '[ $rc -eq 0 ] && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"claude\"].get(sys.argv[2]) == sys.argv[3] and d[\"claude\"][\"alice@example.com\"] == \"US-C\" and \"aliases\" not in d else 1)" "$MR/policy.json" "$LE" "$LN" && ssh-keygen -Y verify -f "$T/lock.allowed" -I cca-release -n cca-policy -s "$MR/policy.json.sig" < "$MR/policy.json" >/dev/null 2>&1 && git -C "$MR" log -1 --format=%s | grep -q "按请求锁定账号的节点：$LE → $LN" && grep -q "GIT push $MR" git.log && printf "%s" "$out" | grep -q "已发布到 example/cca-policy"'
cp "$MR/policy.json" "$MR/policy.json.sig" "$PR/"
out=$(run cca policy pull 2>&1); rc=$?
out2=$(run cca policy request "$LK" 2>&1); rc2=$?
out3=$(run cca _gui state 2>/dev/null </dev/null)
ok "拉到新映射后这个账号被固定了：不用再请求，App 里也没有这个入口" '[ $rc -eq 0 ] && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "已经按维护者的映射固定在 $LN 上了" && printf "%s" "$out3" | LE="$LE" jok "(lambda a: a[\"pinned\"] and not a[\"can\"][\"policy_lock\"])(next(x for x in d[\"tools\"][0][\"accounts\"] if __import__(\"os\").environ[\"LE\"] in x[\"key\"]))"'
head1=$(git -C "$MR" rev-parse HEAD)
runcmd "$(ans)" "$CMD"
ok "同一条命令再运行一次：说已经是这样了，不用发布，不提交" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已经是这样" && printf "%s" "$out" | grep -q "不用发布" && [ "$(git -C "$MR" rev-parse HEAD)" = "$head1" ]'
runcmd "$(ans)" "./policy.sh lock claude ALICE@example.com US-A -y"
ok "已有的账号换节点（邮箱不分大小写，改原来那一项）；-y 不问" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "alice@example.com：US-C → US-A（换节点）" && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"claude\"][\"alice@example.com\"] == \"US-A\" and len(d[\"claude\"]) == 2 else 1)" "$MR/policy.json"'
runcmd "$(ans)" "./policy.sh lock codex Mixed@Example.com US-D -y"
ok "Codex 账号写进 codex 那一节，邮箱存成小写" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "mixed@example.com → US-D（新加）" && python3 -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"codex\"] == {\"mixed@example.com\": \"US-D\"} else 1)" "$MR/policy.json"'
printf '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-A", "type": "vless", "server": "192.0.2.1", "port": 443}]}\n' > "$MR/nodes.json"
runcmd "$(ans n)" "./policy.sh lock claude new@example.com US-B"
rm -f "$MR/nodes.json" "$MR/nodes.json.sig"
ok "节点不在参考节点里：提醒路由器上没有这个节点的机器会用不了" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "节点 US-B 不在参考节点（nodes.json）里"'
mkdir -p "$T/other"; cp "$MR/policy.json" "$T/other/"
runcmd "$(ans n)" "./policy.sh lock claude x@example.com US-A -C '$T/other'"
out2="$out"; out=$(run CCA_RELEASE_KEY="$T/relkey" CCA_POLICY_DIR="$T/other" CCA_TTY="$(ans n)" bash -c "cd '$MT' && ./policy.sh lock claude x@example.com US-A" 2>&1)
ok "映射仓库在别的地方：环境变量 CCA_POLICY_DIR 或者 -C 指定" 'printf "%s" "$out" | grep -qF "要写进映射（$T/other）" && printf "%s" "$out2" | grep -qF "要写进映射（$T/other）"'
runcmd "$(ans)" "./policy.sh lock claude x@example.com US-A -C '$T/nope'"
ok "找不到映射仓库：说明怎么指定" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用 -C 指定映射仓库的目录"'
headx=$(git -C "$MR" rev-parse HEAD); sumx=$(cat "$MR/policy.json")
bad() { out=$(cd "$MT" && run CCA_RELEASE_KEY="$T/relkey" ./policy.sh lock "$@" -y 2>&1); rc=$?; }
unchanged() { [ "$(git -C "$MR" rev-parse HEAD)" = "$headx" ] && [ "$(cat "$MR/policy.json")" = "$sumx" ]; }
noesc() { ! printf "%s" "$out" | grep -q "$(printf "\033")"; }
bad claude
ok "少写了参数：说用法，什么都不改" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法：./policy.sh lock claude|codex|agy 邮箱 节点" && unchanged'
bad router e@example.com US-A
ok "不认识的工具：不收" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "工具应该是 claude、codex 或 agy" && unchanged'
bad claude nonsense US-A
ok "不像邮箱：不收" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "邮箱不对" && unchanged'
bad claude "e$(printf '\033')[2J@example.com" US-A
ok "邮箱里有控制字符（可能搅乱终端）：不收，也不原样显示" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "邮箱不对" && noesc && unchanged'
bad claude e@example.com "US$(printf '\033')[2J"
ok "节点名里有控制字符：不收，也不原样显示" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "节点名不对" && noesc && unchanged'
touch "$T/gh_public"; : > git.log
runcmd "$(ans)" "./policy.sh lock claude pub@example.com US-A -y"; rm -f "$T/gh_public"
ok "映射仓库不是私有的：不签名、不推送" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不是私有仓库" && ! grep -q "GIT push" git.log'
runcmd "$(ans)" "./policy.sh lock claude pub@example.com US-A -y"
ok "上次写进去了、没发布成：再运行一次时提醒还没发布，告诉维护者怎么发布" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "还有没发布的改动" && printf "%s" "$out" | grep -qF "./policy.sh $MR"'
for f in accounts.json codex.json config.json; do cp -p "$T/lock.$f" "$H/.cca/$f"; done; cp -p "$T/lock.policy.json" "$PD"; cp -p "$T/lock.policy.json" "$PR/policy.json"; cp -p "$T/lock.policy.json.sig" "$PR/policy.json.sig"

fi
if sect 11b3c; then
# 11b3c. 设备统计（presence）：维护者在映射里配置了设备统计的服务器（真的 server/cca-presence.py：127.0.0.1 上随机端口、测试证书、
#        测试发布密钥，ttl 是 STTL 秒）之后，映射里的账号经 cca 用的时候这台 Mac 在后台起一个上报进程，把开着的会话报给服务器；
#        cca devices 问服务器这个账号连着哪些设备。设备清单由维护者 ./policy.sh presence sync（假 gh 的 deploy key）推给服务器。
#        测试里的会话没有终端：在跑就算在用。这一节结束时停掉服务器，映射、配置、假的映射仓库、假 GitHub 上的 key 都还原
SV="$T/presence-srv"; mkdir -p "$SV"
STTL=10   # 服务器这么多秒没收到上报就算离线。上报进程 60 秒才一次心跳，所以「在用」只在每次上报后的这几秒里看得到：
          # 以前是 3 秒，机器慢时（xprotectd 忙着扫新文件，cca 每次运行慢好几倍）看到在用以后再问一两次就过了
openssl ecparam -name prime256v1 -genkey -noout -out "$SV/key.pem" 2>/dev/null
openssl req -new -x509 -key "$SV/key.pem" -subj /CN=cca-presence -days 2 -out "$SV/cert.pem" 2>/dev/null
cp "$T/relkey.pub" "$SV/release.pub"
srv_start() {   # srv_start [端口]：起服务器（状态文件留着，重启后接着用）
	rm -f "$SV/port"
	python3 -I "$D/../server/cca-presence.py" --listen "127.0.0.1:${1:-0}" --port-file "$SV/port" --cert "$SV/cert.pem" --key "$SV/key.pem" \
		--release-key "$SV/release.pub" --state "$SV/state.json" --ttl "$STTL" --relay-listen 127.0.0.1:0 >>"$SV/log" 2>&1 &
	SVP=$!
	local i; for i in $(seq 1 50); do [ -s "$SV/port" ] && return 0; sleep 0.1; done; return 1
}
srv_start; SVPORT=$(cat "$SV/port")
PIN="sha256//$(openssl x509 -in "$SV/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)"   # 和部署时一样用 openssl 算
PRD="$H/.cca/presence"
cp -p "$H/.cca/config.json" "$T/pres.config.json"; cp -p "$H/.cca/accounts.json" "$T/pres.accounts.json"; cp -p "$T/deploy_keys.json" "$T/pres.keys.json"
cp -p "$PR/policy.json" "$T/pres.pr.json"; cp -p "$PR/policy.json.sig" "$T/pres.pr.sig"; cp -p "$H/.cca/policy/policy.json" "$T/pres.local.json"; cp -p "$MR/policy.json" "$T/pres.mr.json"
cp -p "$AJ" "$T/pres.aj.json"   # 下面 API key 的同步按登录身份（accountUuid）认，用完还原
withp() { python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j["presence"]={"url":"https://127.0.0.1:%s" % sys.argv[2],"pins":[sys.argv[3]],"salt":"ab"*16}; print(json.dumps(j))' "$T/pres.pr.json" "$SVPORT" "${1:-$PIN}"; }   # withp [指纹]：现在发布的映射加上 presence 一节
dv() { run cca devices alice@example.com --json 2>/dev/null; }   # 问服务器：alice 连着哪些设备
dstate() { dv > "$T/dv.json"; python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(",".join("%s:%s:%d" % (x["name"], x["state"], x["cli"]) for x in d["accounts"][0]["devices"]))' "$T/dv.json" 2>/dev/null; }   # 问到的留在 dv.json
# 等服务器上 alice 的设备变成这样。一秒左右问一次：服务器给每台设备限了流（每秒 0.5 个请求、最多攒 30 个，上报进程的上报和这里的查询
# 算在一起）。cca 还内嵌着 Python 的时候问一次就要大半秒，后面只歇 0.2 秒正好；换成辅助程序以后问一次不到 0.2 秒，还歇 0.2 秒的话
# 十几秒就把配额问光了，上报进程报不上去（请求太频繁），状态一直不变
dwait() { local i; for i in $(seq 1 60); do [ "$(dstate)" = "$1" ] && return 0; sleep 0.8; done; return 1; }
apid() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$PRD/agent.json" 2>/dev/null; }   # 上报进程的进程号
alive() { [ -n "$1" ] && kill -0 "$1" 2>/dev/null; }
pol "$(withp)"
out=$(run cca policy pull 2>&1); rc=$?
out2=$(run cca policy 2>&1)
ok "映射里加了设备统计的服务器（presence 一节）：照常验签拉到；cca policy 说明映射里的账号经 cca 用时会报告给它、怎么关掉" '[ $rc -eq 0 ] && grep -q "\"presence\"" "$H/.cca/policy/policy.json" && printf "%s" "$out2" | grep -q "设备统计：映射里的账号在这台 Mac 上用的时候.*https://127.0.0.1:$SVPORT" && printf "%s" "$out2" | grep -qF "\"presence\": false"'
printf '%s\n' "$(withp)" > "$T/pres.ok.json"; python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j["presence"]["url"]="http://x"; j["presence"]["pins"]=[]; json.dump(j, open(sys.argv[2], "w"))' "$T/pres.ok.json" "$T/pres.bad.json"
out=$(run cca _policy-check "$T/pres.bad.json" 2>&1); rc=$?
ok "维护者发布前的严格检查认识 presence 这一节，也查它的格式" 'run cca _policy-check "$T/pres.ok.json" >/dev/null 2>&1 && [ $rc -ne 0 ] && printf "%s" "$out" | grep -q "presence.url 应该是 https://地址:端口" && printf "%s" "$out" | grep -q "presence.pins"'
out=$(gj state)
ok "_gui state：映射里的官方账号 can.devices 为真（账号页有「设备」），第三方 API 账号没有" 'printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"can\"][\"devices\"] is True and not any(a[\"can\"].get(\"devices\") for t in d[\"tools\"] for a in t[\"accounts\"] if a[\"kind\"] == \"api\")"'
out=$(runev cca devices alice@example.com --json); rc=$?
ok "服务器还不认这台 Mac（维护者还没同步设备清单）：退出码 1，fail 事件说清楚要维护者做什么" '[ $rc -ne 0 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"devices_unavailable\" and \"presence sync\" in e[\"message\"]"'
# 维护者：仓库上各台机器的 deploy key（这台机器的要在上面）推成服务器的设备清单
printf '%s\n' "$(withp)" > "$MR/policy.json"
python3 - "$T/deploy_keys.json" "$PK.pub" <<'PY'
import json, sys
p, pub = sys.argv[1], " ".join(open(sys.argv[2]).read().split()[:2])
d = json.load(open(p))
ks = d["repos"].setdefault("example/cca-policy", [])
if not any(k["key"] == pub for k in ks):
    ks.append({"id": d["next"], "key": pub, "url": "", "title": "alice-mbp", "verified": True, "created_at": "2026-11-01T00:00:00Z",
               "read_only": True, "added_by": "maintainer", "last_used": None, "enabled": True})
    d["next"] += 1
json.dump(d, open(p, "w"), indent=1, ensure_ascii=False)
PY
ME=$(python3 -c 'import json,sys; pub=" ".join(open(sys.argv[2]).read().split()[:2]); print(next(k["title"] for k in json.load(open(sys.argv[1]))["repos"]["example/cca-policy"] if k["key"] == pub))' "$T/deploy_keys.json" "$PK.pub")
NK=$(python3 -c 'import json,sys; print(sum(1 for k in json.load(open(sys.argv[1]))["repos"]["example/cca-policy"] if k["read_only"] and k["enabled"] and k["key"].startswith("ssh-ed25519 ")))' "$T/deploy_keys.json")
runcmd "$(ans)" "$P ./policy.sh presence sync"
out2=$(run $P CCA_RELEASE_KEY="$T/relkey" bash -c "cd '$MT' && ./policy.sh presence status" 2>&1)
ok "./policy.sh presence sync：仓库上的只读 deploy key 用发布密钥签名（cca-devices）推给服务器；presence status 说服务器认得几台、验签自检过了" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "设备清单已推给服务器（$NK 台）" && printf "%s" "$out2" | grep -q "认得 $NK 台设备.*签名自检通过"'
out=$(run cca devices alice@example.com --json 2>&1); rc=$?
ok "cca devices：还没有设备经 cca 用过 alice（这台 Mac 已经被认出来）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"devices\" and d[\"me\"] == \"$ME\" and d[\"accounts\"][0][\"key\"] == \"alice@example.com\" and d[\"accounts\"][0][\"devices\"] == []"'
# 会话开着：上报进程在后台报（假 claude 在 hold 存在时不退出）
touch "$T/hold"; run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS1=$!
waitmark "$AD"; m1=$(marks "$AD")
dwait "$ME:in_use:1"; rc=$?; a1=$(apid)
mkdir -p "$T/guifx3"; cp "$T/dv.json" "$T/guifx3/devices.json"   # 契约测试解码的是 dwait 看到在用的那一份
out=$(run cca devices 2>&1)   # 看到在用就马上问：过了 STTL 秒（下一次心跳之前）服务器就算离线了
ok "映射里的账号开着会话：后台的上报进程马上报给服务器（在用、1 个会话、这台 Mac 排第一）；上报进程只有一个，后台优先级" '[ $rc -eq 0 ] && alive "$a1" && ps -o command= -p "$a1" | grep -q presence-agent && [ "$(ps -o pri= -p "$a1" | tr -d " ")" -le 20 ] && dv | jok "(lambda x: x[\"me\"] is True and x[\"since_epoch\"] and x[\"last_used_epoch\"] and x[\"last_active_epoch\"])(d[\"accounts\"][0][\"devices\"][0])"'
ok "cca devices（给人看的）：每个映射里的账号连着哪些设备，这台 Mac 标出来" 'printf "%s" "$out" | grep -q "Claude 账号连着的设备" && printf "%s" "$out" | grep -qF "${ME}（这台 Mac）  在用 · 命令行里 1 个会话"'
touch "$T/hold"; run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS2=$!
waitmark "$AD" 2
dwait "$ME:in_use:2"; rc=$?
ok "又开一个会话：同一个上报进程马上报 2 个（账号目录里的登记一变就醒，不用等心跳）" '[ $rc -eq 0 ] && [ "$(apid)" = "$a1" ]'
m2=$(marks "$AD" | grep -vx "$m1"); t2=$(pgrep -P "$m2")
kill -KILL "$m2"
dwait "$ME:in_use:1"; rc=$?
ok "cca 被 kill -9（来不及删登记）：上报进程看到它退出，马上改报 1 个" '[ $rc -eq 0 ]'
kill -KILL "$a1"; sleep 4   # dwait 接着等到过了 STTL 秒；服务器回的 seen_age 是取整的秒数，所以下面比 STTL - 1
dwait "$ME:offline:1"; rc=$?
ok "上报进程被 kill -9（就像这台 Mac 睡着了、断网了）：服务器 ttl 秒收不到心跳，算离线（最后看到时开着 1 个会话）" '[ $rc -eq 0 ] && dv | jok "d[\"accounts\"][0][\"devices\"][0][\"seen_epoch\"] < d[\"generated_epoch\"] - $((STTL - 1))"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS3=$!
dwait "$ME:in_use:2"; rc=$?; a2=$(apid)
ok "再启动一次：上报进程重新起来，报全部开着的（2 个）" '[ $rc -eq 0 ] && alive "$a2" && [ "$a2" != "$a1" ]'
rm -f "$T/hold"; wait "$SS1" "$SS3" 2>/dev/null; for i in $(seq 1 30); do alive "$t2" || break; sleep 0.1; done
dwait "$ME:none:0"; rc=$?
for i in $(seq 1 30); do alive "$a2" || break; sleep 0.1; done
ok "会话都关了：马上报“没在用”（留着上次使用），上报进程自己退出，这台 Mac 上不再有它" '[ $rc -eq 0 ] && ! alive "$a2" && dv | jok "d[\"accounts\"][0][\"devices\"][0][\"last_used_epoch\"] is not None"'
ok "上报不经账号的出口：直连（钉住证书公钥），不经路由器和本机 Clash" '! grep -q "127.0.0.1:$SVPORT" curl.log 2>/dev/null'
AK=$(python3 -c 'import json,sys; print(next(k for k, v in json.load(open(sys.argv[1])).items() if isinstance(v, dict)))' "$H/.cca/accounts.json")
out=$(runev cca devices "$AK" --json); rc=$?
ok "第三方 API 账号：不统计（说只统计官方账号），fail 事件 devices_off" '[ $rc -ne 0 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"devices_off\" and \"是第三方 API 账号：只统计官方账号\" in e[\"message\"]"'
python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["presence"]=False; json.dump(c, open(p, "w"), indent=2, ensure_ascii=False)' "$H/.cca/config.json"
out=$(runev cca devices alice@example.com --json); rc=$?
touch "$T/hold"; run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS4=$!
waitmark "$AD"; sleep 1; a3=$(apid)
out2=$(gj state)
ok "config.json 里写着 \"presence\": false：这台 Mac 不报告（没起上报进程）、也不查，账号页没有「设备」" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"devices_off\" and \"关掉了设备统计\" in e[\"message\"]" && ! alive "$a3" && printf "%s" "$out2" | jok "acct(0, \"alice@example.com\")[\"can\"][\"devices\"] is False"'
rm -f "$T/hold"; wait "$SS4" 2>/dev/null; cp -p "$T/pres.config.json" "$H/.cca/config.json"
kill "$SVP" 2>/dev/null; wait "$SVP" 2>/dev/null
out=$(runev cca devices alice@example.com --json); rc=$?
ok "服务器连不上：退出码 1，fail 事件 devices_unavailable 说连不上" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"devices_unavailable\" and \"连不上设备统计的服务器\" in e[\"message\"]"'
out2=$(run $P CCA_RELEASE_KEY="$T/relkey" bash -c "cd '$MT' && ./policy.sh presence status" 2>&1); rc2=$?
ok "./policy.sh presence status：服务器连不上时说清楚原因" '[ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "连不上 https://127.0.0.1:$SVPORT"'
srv_start "$SVPORT"
ok "服务器重启后照样认得设备清单、记得上次使用" '[ "$(dstate)" = "$ME:none:0" ]'
pol "$(withp "sha256//$(printf 'A%.0s' $(seq 1 43))=")"; run cca policy pull >/dev/null 2>&1
out=$(runev cca devices alice@example.com --json); rc=$?
ok "服务器的证书不是映射里钉住的那张（被冒充）：不发请求，说清楚" '[ $rc -ne 0 ] && evq "e[\"code\"] == \"devices_unavailable\" and \"钉住的指纹对不上\" in e[\"message\"]"'
pol "$(withp)"; run cca policy pull >/dev/null 2>&1
ssh-keygen -q -t ed25519 -N "" -C "cca-policy@dan-air" -f "$T/dan"
runcmd "$(ans dan-air)" "$P ./policy.sh key add '$T/dan.pub'"
ok "./policy.sh key add：映射里配置了设备统计的服务器时，顺带把新的设备清单推过去" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "设备统计的服务器认得的设备跟着更新了（$((NK + 1)) 台）"'
ssh-keygen -q -t ed25519 -N "" -C "cca-policy@eve-air" -f "$T/eve"
runcmd "$(ans eve-air)" "$P ./policy.sh key add -R example/other '$T/eve.pub'"
out2=$(run $P CCA_RELEASE_KEY="$T/relkey" bash -c "cd '$MT' && ./policy.sh presence status" 2>&1)
ok "./policy.sh key add -R 别的仓库：不拿那个仓库的 key 当设备清单推给服务器（服务器照样认得原来那些机器）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "推送到的是 example/cca-policy，不是 example/other" && printf "%s" "$out2" | grep -q "认得 $((NK + 1)) 台设备"'
# 记下的 API key 同步给登录了 alice 的其他设备：dan-air 也登录着 alice（身份 u-alice-0001：真的 accountUuid 是 UUID，cca 只认 8 个字符以上的），照 server/README.md 写的格式解开、封好，用自己的
# deploy key 和服务器交换。这台 Mac 记下、删掉后马上在后台同步；之后的 cca _key-sync force 是同一件事，测试里直接跑
dkey() {   # dkey get | junk | put KEY|- 时间：dan-air 取 alice 现在的（打印 JSON；none：没有；bad：认证不过）/ 传一份伪造的 / 传一份
	python3 -B - "$D" "$T/dan" "$SVPORT" "$@" <<'PY'
import base64, json, secrets, sys
sys.path.insert(0, sys.argv[1])
import spec_wire as sw   # 照 server/README.md 的格式独立实现的（不用 cca 的代码）
key, port, op = sys.argv[2], int(sys.argv[3]), sys.argv[4]
aid = sw.aid_of('ab' * 16, 'claude', 'alice@example.com')
keys = sw.derive(sw.KEYSHARE, 'ab' * 16, 'claude', 'alice@example.com', 'u-alice-0001')
b64 = lambda x: base64.b64encode(x).decode()
if op == 'get':
    extra = {'get': [aid]}
elif op == 'junk':
    extra = {'put': {aid: {'n': b64(secrets.token_bytes(16)), 'c': b64(secrets.token_bytes(256)), 't': b64(secrets.token_bytes(32))}}}
else:
    extra = {'put': {aid: sw.keyshare_seal(keys, aid, {'v': 1, 'key': None if sys.argv[5] == '-' else sys.argv[5], 'at': float(sys.argv[6])})}}
status, j = sw.post(key, port, 'keys', extra)
if status != 200:
    sys.exit(1)
if op == 'get':
    rec = j['keys'].get(aid)
    d = rec and sw.keyshare_open(keys, aid, rec['blob'])
    print('none' if not rec else 'bad' if d is None else json.dumps({'key': d['key'], 'at': d['at'], 'from': rec['name'], 'len': len(rec['blob']['c'])}))
PY
}
dkval() { dkey get | python3 -c 'import json,sys; d=sys.stdin.read().strip(); print(json.loads(d)["key"] if d.startswith("{") else d)'; }   # 服务器上 alice 现在的 key（None：删掉了）
dkwait() { local i; for i in $(seq 1 40); do [ "$(dkval)" = "$1" ] && return 0; sleep 0.5; done; return 1; }   # 后台的同步是 background 优先级：最多等 20 秒
setoa "$AJ" accountUuid=u-alice-0001
KS1="sk-ant-api03-REGRESSshare$(printf 'a%.0s' $(seq 1 20))"; KS2="sk-ant-api03-REGRESSdan$(printf 'b%.0s' $(seq 1 24))"
out=$(run CCA_TTY="$(ans "$KS1")" cca key alice 2>&1); rc=$?
dkwait "$KS1"; rc2=$?; L1=$(dkey get | python3 -c 'import json,sys; print(json.load(sys.stdin)["len"])' 2>/dev/null)
ok "记下 alice 的 API key（映射里的账号、开着设备统计）：说登录了这个账号的其他设备也能看到，马上在后台加密传给服务器；dan-air 照文档的格式解得开；服务器上没有 key、邮箱、登录身份（600）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "登录了这个账号的其他设备也能看到" && [ $rc2 -eq 0 ] && [ "$(stat -f %Lp "$SV/keys.json")" = 600 ] && ! grep -q "REGRESSshare\|example.com\|u-alice" "$SV/keys.json" && [ "$(stat -f %Lp "$PRD/keys.json")" = 600 ] && ! grep -q "REGRESSshare" "$PRD/keys.json"'
out=$(gj state)
ok "_gui state：alice 记下的 key 会同步（shared），这台 Mac 自己记下的没有来源、有记下的时间；第三方 API 账号不同步" 'printf "%s" "$out" | jok "(lambda k: k[\"shared\"] is True and k[\"saved\"] and k[\"from_device\"] is None and k[\"saved_epoch\"] > d[\"generated_at\"] - 300 and k[\"sync_error\"] is None)(acct(0, \"alice@example.com\")[\"api_key\"]) and not any(a[\"api_key\"][\"shared\"] for t in d[\"tools\"] for a in t[\"accounts\"] if a[\"kind\"] == \"api\")"'
dkey put "$KS2" "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["key_hints"][sys.argv[2]]["at"] + 0.5)' "$H/.cca/config.json" "$AAID")"; rc=$?   # 比这台 Mac 记下的晚半秒（不用将来的时间：之后在这台 Mac 上删掉，删掉的那一下才是新的）
run cca _key-sync force >/dev/null 2>&1
v=$(run cca key alice --show 2>/dev/null); out=$(gj state); printf "%s" "$out" > "$T/guifx3/state_keys.json"
ok "dan-air 换了一把（比这台 Mac 记下的新）：同步后这台 Mac 的钥匙串里换成它（--show 给新的），_gui state 说是从 dan-air 同步来的" '[ $rc -eq 0 ] && [ "$v" = "$KS2" ] && [ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$KS2" ] && printf "%s" "$out" | jok "(lambda k: k[\"from_device\"] == \"dan-air\" and k[\"saved\"] and k[\"hint\"] == \"sk-ant-…bbbb\")(acct(0, \"alice@example.com\")[\"api_key\"])"'
dkey junk; run cca _key-sync force >/dev/null 2>&1
ok "服务器上的被换成认证不过的（伪造的、坏了）：这台 Mac 不认，把自己的传回去" '[ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$KS2" ] && [ "$(dkval)" = "$KS2" ]'
out=$(run CCA_TTY="$(ans y)" cca key alice --forget 2>&1); rc=$?
dkwait None; rc2=$?; L2=$(dkey get | python3 -c 'import json,sys; print(json.load(sys.stdin)["len"])' 2>/dev/null)
ok "cca key alice --forget：说登录了这个账号的其他设备上的也会删掉；删掉也同步（dan-air 解开是删除，密文和 key 一样长）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "其他设备上的也会删掉" && [ $rc2 -eq 0 ] && [ ! -e "$T/keychain/cca-ref-$AAID" ] && [ -n "$L1" ] && [ "$L1" = "$L2" ]'
dkey put "$KS2" "$(python3 -c 'import time; print(time.time())')"
v=$(run cca key alice --show 2>/dev/null); rc=$?
ok "这台 Mac 上没有、dan-air 又记下了：cca key alice --show 先同步一次再给（App 的「显示」「复制」同样）" '[ $rc -eq 0 ] && [ "$v" = "$KS2" ]'
# 折合金额经服务器交换：各台设备只写自己的那一份，密文（密钥和 API key 一样由盐、邮箱、登录身份派生，标签是 cca-cost-v1；认证里带着设备指纹）。
# 10g 里 alice 在这台 Mac 上 4 次请求、折合 0.026 美元。dan-air 照 server/README.md 写的格式封好、解开
dcost() {   # dcost get | put 时间 美元：dan-air 解开这台 Mac 传的 alice 的那一份（打印 JSON；none：没有；bad：认证不过；明文没补齐到 1024 字节的整数倍时出错）/ 传自己的那一份
	python3 -B - "$D" "$T/dan" "$SVPORT" "$PK.pub" "$@" <<'PY'
import json, sys, time
sys.path.insert(0, sys.argv[1])
import spec_wire as sw   # 照 server/README.md 的格式独立实现的（不用 cca 的代码）
key, port, me, op = sys.argv[2], int(sys.argv[3]), sw.fp_of(sys.argv[4]), sys.argv[5]
aid = sw.aid_of('ab' * 16, 'claude', 'alice@example.com')
keys = sw.derive(sw.COST, 'ab' * 16, 'claude', 'alice@example.com', 'u-alice-0001')
if op == 'get':
    extra = {'get': [aid]}
else:
    hour = int(float(sys.argv[6]) // 3600 * 3600)
    doc = {'v': 1, 'at': time.time(), 'h': {str(hour): [int(float(sys.argv[7]) * 1e6), 1, 100, 10, 0, 0, 0]}, 'um': []}
    extra = {'put': {aid: sw.cost_seal(keys, aid, sw.fp_of(key + '.pub'), doc)}}
status, j = sw.post(key, port, 'ledger', extra)
if status != 200:
    sys.exit(1)
if op == 'get':
    row = next((x for x in j['cost'].get(aid, []) if x['fp'] == me and 'blob' in x), None)
    d = row and sw.cost_open(keys, aid, me, row['blob'])
    print('none' if not row else 'bad' if d is None else json.dumps({'usd': sum(x[0] for x in d['h'].values()) / 1e6,
                                                                      'requests': sum(x[1] for x in d['h'].values()), 'name': row['name']}))
PY
}
out=$(run cca cost sync --now --json 2>/dev/null </dev/null); rc=$?
ok "cost sync --now --json：开了设备统计、alice 在映射里、登录着：加密传给服务器（服务器上没有邮箱、登录身份、金额，600）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"sync\"][\"state\"] == \"ok\" and isinstance(d[\"sync\"][\"checked_epoch\"], float)" && [ "$(stat -f %Lp "$SV/cost-slots.json")" = 600 ] && ! grep -q "example.com\|u-alice\|26000" "$SV/cost-slots.json"'
v=$(dcost get)
ok "dan-air（也登录着 alice）照文档的格式解得开这台 Mac 传的：4 次请求、折合 0.026 美元，明文补齐到 1024 字节的整数倍" 'printf "%s" "$v" | jok "d == {\"usd\": 0.026, \"requests\": 4, \"name\": \"$ME\"}"'
dcost put "$(python3 -c 'import time; print(time.time() - 86400)')" 2.5; rc=$?
run cca cost sync --now --json >/dev/null 2>&1 </dev/null; out=$(gj state); printf '%s' "$out" > "$T/guifx3/state_cost_sync.json"
ok "dan-air 传了自己的那一份（2.5 美元）：同步后 alice 有两台设备（这台在前），合计 2.526 美元（全部账号 2.579：还有 dave 的 Codex），同步好了" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "(lambda a: [(x[\"name\"], x[\"me\"], x[\"revoked\"]) for x in a[\"devices\"]] == [(\"$ME\", True, False), (\"dan-air\", False, False)] and a[\"summary\"][\"days_usd\"] == 2.526 and a[\"sync\"][\"state\"] == \"ok\" and isinstance(a[\"sync\"][\"checked_epoch\"], float))(acct(0, \"alice@example.com\")[\"cost\"]) and d[\"cost_total\"][\"by_tool\"][\"claude\"][\"days_usd\"] == 2.526 and d[\"cost_total\"][\"days_usd\"] == 2.579"'
ok "cost_total.me 只算这台设备的那一份：全部账号 2.579 减去 dan-air 的 2.5 是 0.079（alice 0.026，dave 0.053）" 'printf "%s" "$out" | jok "(lambda m: m[\"days_usd\"] == 0.079 and m[\"by_tool\"][\"claude\"][\"days_usd\"] == 0.026 and m[\"by_tool\"][\"codex\"][\"days_usd\"] == 0.053 and len(m[\"days\"]) == 30)(d[\"cost_total\"][\"me\"])"'
ok "cost_total.devices（设备菜单）：这台设备在前（和 me 一样），dan-air 只用过 alice（2.5 美元，id 和账号里的一样）；各台加起来是顶层的" 'printf "%s" "$out" | jok "(lambda t, a: [(x[\"name\"], x[\"me\"], x[\"days_usd\"]) for x in t[\"devices\"]] == [(\"$ME\", True, 0.079), (\"dan-air\", False, 2.5)] and [x[\"id\"] for x in t[\"devices\"]] == [x[\"id\"] for x in a[\"devices\"]] and t[\"devices\"][1][\"by_tool\"][\"codex\"][\"days_usd\"] == 0 and abs(sum(x[\"days_usd\"] for x in t[\"devices\"]) - t[\"days_usd\"]) < 1e-9)(d[\"cost_total\"], acct(0, \"alice@example.com\")[\"cost\"])"'
# 远程会话的配对（docs/dev/remote.md；服务器开着中转）：这里只测 cca remote pair / join / peers / allow / unpair 这几条命令怎么问、怎么说，
# 配对、列表、接入、加密本身在 tests/remote_relay.py（unit 一节）。另一台机器还是 dan-air，照 server/README.md 的格式自己说话
dpair() {   # dpair join 配对码 | show 文件：dan-air 输入这台 Mac 显示的配对码 / 自己显示一个（写进文件），等对方输入。配好打印“配对编号 密钥”
	python3 -B - "$D" "$T/dan" "$SVPORT" "$@" <<'PY'
import secrets, sys
sys.path.insert(0, sys.argv[1])
import spec_wire as sw   # 照 server/README.md 的格式独立实现的（不用 cca 的代码）
key, port, op = sys.argv[2], int(sys.argv[3]), sys.argv[4]
me = sw.fp_of(key + '.pub')


def call(op, **extra):
    status, j = sw.post(key, port, op, extra)
    if status != 200:
        sys.exit('%s：%s %s' % (op, status, j))
    return j


n = secrets.token_bytes(32)
if op == 'join':
    kc, keys, rid = sw.pair_keys(sw.code_bytes(sys.argv[5]))
    call('pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'b', me, n, 'dan'))
    theirs = sw.pair_open(keys, rid, 'a', call('pair_get', rid=rid)['m'])
    p, pid = sw.pair_secret(kc, bytes.fromhex(theirs['n']), n, theirs['fp'], me)
else:
    c = secrets.token_bytes(10)
    kc, keys, rid = sw.pair_keys(c)
    call('pair_open', rid=rid)
    call('pair_put', rid=rid, m=sw.pair_msg(keys, rid, 'a', me, n, 'dan'))
    with open(sys.argv[5] + '.tmp', 'w') as f:
        f.write(sw.code_text(c) + '\n')
    __import__('os').replace(sys.argv[5] + '.tmp', sys.argv[5])
    for _ in range(30):
        j = call('pair_get', rid=rid, wait=2)
        if j['m']:
            break
    else:
        sys.exit('没等到对方输入')
    theirs = sw.pair_open(keys, rid, 'b', j['m'])
    p, pid = sw.pair_secret(kc, n, bytes.fromhex(theirs['n']), me, theirs['fp'])
call('pair_commit', rid=rid, pid=pid)
print(pid, p.hex())
PY
}
out=$(runev cca remote pair 2>&1); rc=$?
out2=$(run CCA_TTY="$(ans n)" cca remote pair 2>&1); rc2=$?
ok "cca remote pair：要在终端里；先问（说清楚输入配对码的那台机器能做什么），不同意就不显示配对码" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "需要在终端里交互运行" && [ $rc2 -eq 1 ] && printf "%s" "$out2" | grep -q "那台机器可以看和操作这台机器上开着远程会话的会话" && printf "%s" "$out2" | grep -q "已取消" && ! printf "%s%s" "$out" "$out2" | grep -q "配对码：" && [ ! -e "$H/.cca/remote/peers.json" ]'
run CCA_TTY="$(ans y)" cca remote pair > "$T/rp.out" 2>&1 & RPP=$!
for i in $(seq 1 100); do grep -q "配对码：" "$T/rp.out" 2>/dev/null && break; sleep 0.1; done
RCODE=$(sed -n 's/.*配对码：\([0-9A-Z-]*\).*/\1/p' "$T/rp.out" | head -n 1)
dj=$(dpair join "$RCODE" 2>&1); rc2=$?; wait "$RPP"; rc=$?
RPID=${dj%% *}; RKEY=${dj#* }
out=$(run cca remote peers 2>&1); out2=$(run cca remote 2>&1)
ok "cca remote pair：显示配对码，dan-air 输入后配好：两边算出同一把密钥，存进钥匙串（经 security -i 的十六进制，不进命令行）；配对表里是维护者设备清单里它的名字，让它看和操作" '[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && grep -q "已经和「dan-air」配对" "$T/rp.out" && grep -q "cca remote allow dan-air view" "$T/rp.out" && [ "$(cat "$T/keychain/cca-remote-$RPID/pair")" = "$RKEY" ] && ! grep -qF "$RKEY" security.log "$T/env.log" && [ "$(stat -f %Lp "$H/.cca/remote/peers.json")" = 600 ] && ! grep -qF "$RKEY" "$H/.cca/remote/peers.json" && printf "%s" "$out" | grep -q "^${RPID:0:8}  dan-air  对方可以看和操作这台机器的会话" && printf "%s" "$out2" | grep -q "配对过 1 台机器"'
ok "配对码、密钥都没有发给服务器（它只记着是哪两台设备配了对，600）" '[ "$(stat -f %Lp "$SV/remote.json")" = 600 ] && grep -q "\"$RPID\"" "$SV/remote.json" && ! grep -qF "$RKEY" "$SV"/*.json && ! grep -qF "${RCODE//-/}" "$SV"/*.json'
out=$(run cca remote list --json 2>/dev/null </dev/null); rc=$?; printf '%s' "$out" > "$T/guifx3/remote_list.json"
ok "cca remote list --json：中转用得了，配对过的机器（配对的编号、名字、它能做什么、配对码是这台显示的、服务器上还在）；它上面没有会话" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"relay\"] == {\"available\": True, \"message\": None} and d[\"errors\"] == [] and d[\"sessions\"] == [] and len(d[\"peers\"]) == 1 and (lambda p: p[\"id\"] == \"$RPID\" and p[\"short\"] == \"${RPID:0:8}\" and p[\"name\"] == \"dan-air\" and p[\"incoming\"] == \"control\" and p[\"shown\"] is True and p[\"state\"] == \"ok\" and isinstance(p[\"paired_epoch\"], (int, float)))(d[\"peers\"][0])"'
ok "cca remote list --local --json：不问服务器，配对的状态是 unknown" 'run cca remote list --local --json 2>/dev/null </dev/null | jok "d[\"peers\"][0][\"state\"] == \"unknown\" and d[\"errors\"] == []"'
out=$(runev cca remote allow "$RPID" none 2>&1); rc=$?
ok "界面 App 改对方能做什么（按配对的编号，不问）：result 事件 remote_updated 带着那句话" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"remote_updated\" and \"不能接入这台机器了\" in e[\"message\"]" && run cca remote list --local --json 2>/dev/null </dev/null | jok "d[\"peers\"][0][\"incoming\"] == \"none\""'
out=$(runev cca remote allow "${RPID%????}0000" view 2>&1); rc=$?
ok "没有这个配对：fail 事件说原因，配对表不动" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and \"没有\" in e[\"message\"]" && run cca remote list --local --json 2>/dev/null </dev/null | jok "d[\"peers\"][0][\"incoming\"] == \"none\""'
out=$(run cca remote allow dan-air view 2>&1); rc=$?
out2=$(run cca remote allow dan-air sideways 2>&1); rc2=$?
ok "cca remote allow：改成只让看（说清楚正操作着的会被断开）；写错了给用法" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "「dan-air」只能看这台机器上的会话" && run cca remote peers 2>/dev/null | grep -q "dan-air  对方只能看这台机器的会话" && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "用法：cca remote allow 名字 control|view|none"'
rm -f "$T/dan.code"; dpair show "$T/dan.code" > "$T/dan.pair" 2>&1 & DPP=$!
for i in $(seq 1 100); do [ -s "$T/dan.code" ] && break; sleep 0.1; done
out=$(run CCA_TTY="$(ans "$(tr 'A-Z' 'a-z' < "$T/dan.code")")" cca remote join 2>&1); rc=$?; wait "$DPP"; rc2=$?
RPID2=$(cut -d' ' -f1 "$T/dan.pair")
ok "cca remote join：输入 dan-air 显示的配对码（小写也行；从 stdin 交给辅助程序）；同两台机器再配一次换成新的一把密钥，以前让它做的照旧" '[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && printf "%s" "$out" | grep -q "已经和「dan-air」配对：这台机器可以接入它上面的会话" && [ -n "$RPID2" ] && [ "$RPID2" != "$RPID" ] && [ ! -e "$T/keychain/cca-remote-$RPID" ] && [ "$(cat "$T/keychain/cca-remote-$RPID2/pair")" = "$(cut -d" " -f2 "$T/dan.pair")" ] && run cca remote peers 2>/dev/null | grep -q "^${RPID2:0:8}  dan-air  对方只能看这台机器的会话" && ! grep -q "\"$RPID\"" "$SV/remote.json" && grep -q "\"$RPID2\"" "$SV/remote.json"'
out=$(run cca remote list 2>&1); rc=$?
ok "cca remote list：配对过的机器上没有会话时什么都不列（问过服务器了，没报错）" '[ $rc -eq 0 ] && [ -z "$out" ]'
out=$(run CCA_TTY="$(ans 0000-0000-0000-0000)" cca remote join 2>&1); rc=$?
ok "cca remote join：配对码不对时说清楚，配对表不动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有这个配对码：输错了，或者过期了" && [ -e "$T/keychain/cca-remote-$RPID2" ]'
out=$(run cca remote unpair dan-air 2>&1); rc=$?
out2=$(run cca remote peers 2>&1)
ok "cca remote unpair：配对表、钥匙串里的密钥、服务器上的记录都删掉" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已经取消和「dan-air」的配对" && [ ! -e "$T/keychain/cca-remote-$RPID2" ] && ! grep -q "\"$RPID2\"" "$SV/remote.json" && printf "%s" "$out2" | grep -q "还没有配对过的机器"'
# 界面 App 里配对（提问会话，PROTOCOL 9.2）：先问，再一页只显示配对码（不用回答），dan-air 输入后 cca 自己收场
sess '{"consent.remote_pair":[{"v":"y"}],"remote.pair":[null]}' -- cca remote pair > "$T/rp.out" 2>&1 & RPP=$!
RCODE=""; for i in $(seq 1 150); do RCODE=$(python3 -I -c 'import json,sys
for l in open(sys.argv[1]):
    e = json.loads(l) if l.strip() else {}
    if e.get("q") == "remote.pair":
        print(e["fields"][0]["value"])' "$T/ev.jsonl" 2>/dev/null); [ -n "$RCODE" ] && break; sleep 0.1; done
dj=$(dpair join "$RCODE" 2>&1); rc2=$?; wait "$RPP"; rc=$?
cp "$T/ev.jsonl" "$T/guifx3/prompts_remote_pair.jsonl"
ok "界面 App 里显示配对码：同意当场问，配对码在一页只显示的字段里（不用回答），对方输入后 result 事件 remote_paired" '[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && [ "$(asks)" = "consent.remote_pair remote.pair" ] && printf "%s" "$RCODE" | grep -Eq "^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$" && evq "e.get(\"q\") == \"remote.pair\" and [(f[\"id\"], f[\"kind\"]) for f in e[\"fields\"]] == [(\"code\", \"code\")]" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"remote_paired\" and \"「dan-air」\" in e[\"message\"]" && ! grep -qF "$RCODE" "$T/rp.out" && [ -e "$T/keychain/cca-remote-${dj%% *}" ]'
out=$(runev cca remote unpair "${dj%% *}" 2>&1); rc=$?
ok "界面 App 取消配对（按配对的编号）：result 事件 remote_updated，钥匙串里的密钥删掉" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"remote_updated\" and \"已经取消和「dan-air」的配对\" in e[\"message\"]" && [ ! -e "$T/keychain/cca-remote-${dj%% *}" ]'
out=$(sess '{"remote.code":[{"v":"0000-0000-0000-0000"}]}' -- cca remote join 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_remote_join.jsonl"
ok "界面 App 里输入配对码：一个输入框；配对码不对是 fail 事件，说原因" '[ $rc -ne 0 ] && [ "$(asks)" = "remote.code" ] && evq "e[\"ev\"] == \"fail\" and \"没有这个配对码\" in e[\"message\"]"'
rm -f "$H/.cca/remote/peers.json"; rmdir "$H/.cca/remote" 2>/dev/null
runcmd "$(ans y)" "$P ./policy.sh key rm dan-air"
run cca cost sync --now --json >/dev/null 2>&1 </dev/null; out=$(gj state)
ok "维护者撤销了 dan-air：它传过的照样算（是 alice 的用量），标上已撤销" 'printf "%s" "$out" | jok "(lambda a: [(x[\"name\"], x[\"revoked\"]) for x in a[\"devices\"]] == [(\"$ME\", False), (\"dan-air\", True)] and a[\"summary\"][\"days_usd\"] == 2.526)(acct(0, \"alice@example.com\")[\"cost\"])"'
# 自己的同步服务器（docs/dev/hub.md）：另起一台真的 server/cca-presence.py --mode own（另一张证书，不用发布密钥），这台 Mac 经 cca hub join
# 加入：维护者共享以外的官方账号（这里加一个 own@example.com）经它同步，alice 照旧只走维护者的。dan-air 照 server/README.md 的格式自己加入
# 维护者的服务器按设备限流（每秒 0.5 个、最多攒 30 个）：上面配对的那些把这台 Mac 的配额用得差不多了，下面还要经它同步一轮，
# 先重启它（状态文件留着），配额从头算
kill "$SVP" 2>/dev/null; wait "$SVP" 2>/dev/null; srv_start "$SVPORT"
HV="$T/hub-srv"; mkdir -p "$HV"
openssl ecparam -name prime256v1 -genkey -noout -out "$HV/key.pem" 2>/dev/null
openssl req -new -x509 -key "$HV/key.pem" -subj /CN=cca-hub -days 2 -out "$HV/cert.pem" 2>/dev/null
HADMIT=$(printf '5a%.0s' $(seq 1 16)); HSALT=$(printf 'c3%.0s' $(seq 1 16))
python3 -I "$D/../server/cca-presence.py" --mode own --admit-hash "$(printf '%s' "$HADMIT" | shasum -a 256 | cut -d' ' -f1)" --listen 127.0.0.1:0 \
	--port-file "$HV/port" --cert "$HV/cert.pem" --key "$HV/key.pem" --state "$HV/state.json" --ttl "$STTL" >>"$HV/log" 2>&1 & HVP=$!
for i in $(seq 1 50); do [ -s "$HV/port" ] && break; sleep 0.1; done; HVPORT=$(cat "$HV/port")
HPIN="sha256//$(openssl x509 -in "$HV/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)"
hlink() { python3 -I -c 'import sys; sys.path.insert(0, sys.argv[1]); import spec_wire as sw; print(sw.link_encode("https://127.0.0.1:%s" % sys.argv[2], [sys.argv[3]], sys.argv[4], sys.argv[5]))' "$D" "$HVPORT" "$HPIN" "${1:-$HADMIT}" "$HSALT"; }   # hlink [入伙口令]：照文档拼的加入链接
HLINK=$(hlink); HCHUNK=${HLINK:$((${#HLINK} - 34)):24}   # 链接末尾盐的那一截（base64 的）：查链接有没有漏出去时查它和口令本身
hdec() { python3 -I -c 'import json,sys; sys.path.insert(0, sys.argv[1]); import spec_wire as sw; print(json.dumps(sw.link_decode(sys.argv[2]), sort_keys=True))' "$D" "$1" 2>/dev/null; }   # 照文档解开的加入链接
dhub() {   # dhub join | members：dan-air 照 server/README.md 的格式加入 / 列设备（打印 状态码 和 error）
	python3 -B - "$D" "$T/dan" "$HVPORT" "$HADMIT" "$HSALT" "$1" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
import spec_wire as sw   # 照 server/README.md 的格式独立实现的（不用 cca 的代码）
key, port, admit, salt, op = sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5], sys.argv[6]
extra = {}
if op == 'join':
    with open(key + '.pub') as f:
        extra = {'pub': ' '.join(f.read().split()[:2]), 'name': 'dan-air', 'admit': admit, 'check': sw.hub_check(salt)}
status, j = sw.post(key, port, op, extra)
print(status, j.get('error') or '')
PY
}
edit_table 'any(k == "own@example.com" or k.endswith("(own@example.com)") for k in j) or j.__setitem__("own@example.com", "US-A")'   # 11b2 的合并那里加过就不再加
OWNQ='next(a for a in d["tools"][0]["accounts"] if a["key"] == "own@example.com" or a["key"].endswith("(own@example.com)"))'   # jok 里：这个账号
out=$(run cca hub 2>&1); rc=$?
ok "cca hub：还没加入时说怎么加入" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "还没加入自己的同步服务器（有加入链接时运行 cca hub join）"'
out=$(runev cca devices own@example.com --json 2>&1); rc=$?
ok "还没加入：不在维护者映射里的官方账号，cca devices 说要经自己的同步服务器（fail 事件 devices_off）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"devices_off\" and \"不在维护者的映射里，要经自己的同步服务器统计：还没加入自己的同步服务器\" in e[\"message\"]"'
out=$(runev cca hub leave 2>&1); rc=$?
ok "还没加入：cca hub leave 说没加入（fail 事件 hub_not_joined）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_not_joined\""'
out=$(runev CCA_TTY="$(ans '')" cca hub join 2>&1); rc=$?
ok "cca hub join：没输入加入链接：fail 事件 hub_link" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_link\""'
out=$(runev CCA_TTY="$(ans "${HLINK%???}")" cca hub join 2>&1); rc=$?
ok "cca hub join：链接少了一截（校验对不上）：fail 事件 hub_link，不联网" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_link\"" && [ ! -e "$H/.cca/hub/hub.json" ]'
out=$(runev CCA_TTY="$(ans "$(hlink "$(printf '9%.0s' $(seq 1 32))")")" cca hub join 2>&1); rc=$?
ok "cca hub join：入伙口令不对：服务器不认（fail 事件 hub_denied，说向已经加入的设备要），什么都不记下" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_denied\" and \"向已经加入的设备要\" in e[\"message\"]" && [ ! -e "$H/.cca/hub/hub.json" ]'
: > "$T/curl.argv"; : > "$T/env.log"
out=$(run CCA_TTY="$(ans "$HLINK")" cca hub join --name 书房的Mac 2>&1); rc=$?
ok "cca hub join：问加入链接（输入不显示），加入了：说服务器、这台设备叫什么；加入信息、密钥只给自己读，有了设备标识；链接、口令不在输出里，也不在子进程的命令行（curl、env、ssh）里" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已加入 127.0.0.1:$HVPORT 上自己的同步服务器：这台设备叫 书房的Mac，服务器上现在有 1 台设备" && [ "$(stat -f %Lp "$H/.cca/hub/hub.json")" = 600 ] && [ "$(stat -f %Lp "$H/.cca/hub/id_ed25519")" = 600 ] && [ "$(stat -f %Lp "$H/.cca/hub")" = 700 ] && cfgok "len(c[\"device_id\"]) == 16" && ! printf "%s" "$out" | grep -qF -e "$HCHUNK" -e "$HADMIT" && grep -q "127.0.0.1:$HVPORT" "$T/curl.argv" && ! grep -qsF -e "$HCHUNK" -e "$HADMIT" "$T/curl.argv" "$T/env.log" "$T/ssh.log"'
out=$(gj state); printf '%s' "$out" > "$T/guifx3/state_hub.json"
ok "_gui state：config.hubs 是维护者的和自己的两台；alice 走维护者的，own@example.com 走自己的（有「设备」一节）；第三方 API 账号哪都不走" 'printf "%s" "$out" | jok "[h[\"kind\"] for h in d[\"config\"][\"hubs\"]] == [\"maintainer\", \"own\"] and (lambda h: h[\"host\"] == \"127.0.0.1:$HVPORT\" and h[\"name\"] == \"书房的Mac\" and h[\"members\"] == 1 and h[\"accounts\"] == sum(a[\"hub\"] == \"own\" for t in d[\"tools\"] for a in t[\"accounts\"]) and h[\"can_invite\"] is True and h[\"pinned\"] is True and h[\"state\"] in (\"pending\", \"ok\"))(d[\"config\"][\"hubs\"][1]) and acct(0, \"alice@example.com\")[\"hub\"] == \"maintainer\" and $OWNQ[\"hub\"] == \"own\" and $OWNQ[\"can\"][\"devices\"] is True and all(a[\"hub\"] is None for t in d[\"tools\"] for a in t[\"accounts\"] if a[\"kind\"] == \"api\")"'
HACC=$(printf '%s' "$out" | python3 -c 'import json,sys; print(sum(a["hub"] == "own" for t in json.load(sys.stdin)["tools"] for a in t["accounts"]))')
out=$(run cca hub status --json 2>/dev/null); printf '%s' "$out" > "$T/guifx3/hub_status.json"
out2=$(run cca hub 2>&1)
ok "cca hub status --json（不联网）：加入了、用得了，经它同步的账号数和 state 里的一样；cca hub 说服务器、这台设备叫什么、几个账号" '[ "$HACC" -ge 1 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"hub_status\" and d[\"joined\"] is True and d[\"reason\"] is None and d[\"hub\"][\"kind\"] == \"own\" and d[\"hub\"][\"accounts\"] == $HACC" && printf "%s" "$out2" | grep -q "自己的同步服务器：127.0.0.1:${HVPORT}（这台设备叫 书房的Mac，服务器上 1 台设备）" && printf "%s" "$out2" | grep -q "经它同步的账号：${HACC} 个"'
v=$(dhub join)
out=$(run cca hub devices --json 2>/dev/null); rc=$?; printf '%s' "$out" > "$T/guifx3/hub_devices.json"
DHFP=$(printf '%s' "$out" | python3 -c 'import json,sys; print(next(x["id"] for x in json.load(sys.stdin)["devices"] if x["name"] == "dan-air"))' 2>/dev/null)
ok "dan-air 照文档加入了；cca hub devices --json：这台在前，两台设备（id 是设备指纹）" '[ "$v" = "200 " ] && [ $rc -eq 0 ] && printf "%s" "$out" | jok "[(x[\"name\"], x[\"me\"]) for x in d[\"devices\"]] == [(\"书房的Mac\", True), (\"dan-air\", False)]" && [ -n "$DHFP" ]'
HINV=$(run cca hub invite 2>/dev/null)
ok "cca hub invite：给出加入时的那一份（照文档解开一样）；在 Claude Code 里不给（会进对话记录）" '[ -n "$(hdec "$HLINK")" ] && [ "$(hdec "$HINV")" = "$(hdec "$HLINK")" ] && ! run CLAUDECODE=1 cca hub invite >/dev/null 2>&1 && run CLAUDECODE=1 cca hub invite 2>&1 | grep -q "在 Claude Code 里不显示加入链接"'
out=$(runev cca hub remove dan-air 2>&1); rc=$?
ok "界面 App 移除一台设备：先不移除，发一条同意事件（范围是它的设备指纹，默认不同意，说明里说它的密钥作废）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"hub_remove\" and e[\"scope\"] == \"$DHFP\" and e[\"default\"] == \"no\" and \"现在的密钥作废\" in e[\"detail\"] and \"加入链接跟着换一个\" in e[\"detail\"]" && [ "$(dhub members)" = "200 " ]'
out=$(runev CCA_GRANT="hub_remove:$DHFP" cca hub remove "$DHFP" 2>&1); rc=$?
HINV2=$(run cca hub invite 2>/dev/null)
HUPS=$(hdec "$HLINK" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["u"], d["p"], d["s"])' 2>/dev/null)   # 加入链接里的地址、指纹、盐
HUPS2=$(hdec "$HINV2" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["u"], d["p"], d["s"])' 2>/dev/null)
ok "同意后（按设备指纹）移除：result 事件 hub_removed，没有提醒；dan-air 再来服务器不认它（unknown_device）；加入链接换了一个（地址、指纹、盐不变），dan-air 拿原来的加入不了" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_removed\" and \"dan-air\" in e[\"message\"]" && ! evq "e[\"ev\"] == \"fail\"" && [ "$(dhub members)" = "403 unknown_device" ] && run cca hub devices --json 2>/dev/null | jok "[x[\"name\"] for x in d[\"devices\"]] == [\"书房的Mac\"]" && [ -n "$HINV2" ] && [ "$(hdec "$HINV2")" != "$(hdec "$HLINK")" ] && [ -n "$HUPS" ] && [ "$HUPS2" = "$HUPS" ] && [ "$(dhub join)" = "403 denied" ]'
out=$(runev cca hub invite --new 2>&1); rc=$?
ok "界面 App 换加入链接（cca hub invite --new）：先不换，发一条同意事件（hub_relink，范围 all，默认不同意，说明里说旧的作废）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"hub_relink\" and e[\"scope\"] == \"all\" and e[\"default\"] == \"no\" and \"不能再用来加入\" in e[\"detail\"]" && [ "$(run cca hub invite 2>/dev/null)" = "$HINV2" ]'
out=$(runev CCA_GRANT=hub_relink:all cca hub invite --new 2>/dev/null); rc=$?
ok "同意后换了：stdout 一行是新的加入链接（事件里没有它）；cca hub invite 给的也是它" '[ $rc -eq 0 ] && [ -n "$out" ] && [ "$out" != "$HINV2" ] && [ "$(run cca hub invite 2>/dev/null)" = "$out" ] && [ "$(hdec "$out")" != "" ] && ! grep -qF "$out" "$T/ev.jsonl"'
HLINK=$out; HCHUNK=${HLINK:$((${#HLINK} - 34)):24}
HADMIT=$(hdec "$HLINK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["a"])')   # 下面加入、查有没有漏出去都照现在的这一份
out=$(runev cca hub remove nobody 2>&1); rc=$?
ok "没有这台设备：fail 事件 hub_no_device" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_no_device\""'
out=$(run cca devices own@example.com --json 2>/dev/null); rc=$?; printf '%s' "$out" > "$T/guifx3/devices_hub.json"
ok "cca devices own@example.com --json：问的是自己的同步服务器（账号的 hub 是 own，hubs 里有它）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"accounts\"][0][\"hub\"] == \"own\" and \"own\" in [h[\"kind\"] for h in d[\"hubs\"]] and d[\"me\"] == \"书房的Mac\""'
out=$(run cca cost sync --now --json 2>/dev/null </dev/null); rc=$?; printf '%s' "$out" > "$T/guifx3/cost_sync_hubs.json"
ok "cost sync --now --json：两台服务器各同步一轮（sync.hubs 是维护者的、自己的）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "[h[\"kind\"] for h in d[\"sync\"][\"hubs\"]] == [\"maintainer\", \"own\"] and d[\"sync\"][\"hubs\"][0][\"state\"] == \"ok\""'
out=$(runev cca hub leave 2>&1); rc=$?
ok "界面 App 退出：先发同意事件（范围 all，默认不同意，说明里说同步来的折合金额会删、记下的 API key 留着），不退出" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"hub_leave\" and e[\"scope\"] == \"all\" and e[\"default\"] == \"no\" and \"记下的 API key 留在这台设备上\" in e[\"detail\"]" && [ -e "$H/.cca/hub/hub.json" ]'
printf '{"v": 1, "acc": {}}' > "$H/.cca/cost/own.remote.json"   # 经它同步来的（这里没有经它同步的会话，放一份看删不删）
out=$(runev CCA_GRANT=hub_leave:all cca hub leave 2>&1); rc=$?
ok "同意后退出：result 事件 hub_left；服务器上移除了这台设备，加入信息、密钥、经它同步来的都删掉；state 里没有它了" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_left\"" && [ ! -e "$H/.cca/hub" ] && [ ! -e "$H/.cca/cost/own.remote.json" ] && python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"machines\"] == {} else 1)" "$HV/state.json" && gj state | jok "[h[\"kind\"] for h in d[\"config\"][\"hubs\"]] == [\"maintainer\"] and $OWNQ[\"hub\"] is None"'
# 配置向导第 4 步（自己的同步服务器）：答有就接着贴加入链接（hub.join），加入了，最后的结果还是 onboarded；已经加入了的不问。
# 这一节开着维护者的共享、拉得到映射（第 1 步不问：拉一次就拉到了），路由器配好了、映射里没有新账号（第 2、3 步也不问）：只有这一步。
# 向导记下 onboarded：跑完 config.json 原样放回去
cp -p "$H/.cca/config.json" "$T/cfg.onbhub.bak"
ONBA='"onboard.policy":[{"next":"skip"}],"onboard.net":[{"v":"skip"}],"onboard.add":[{"v":"n"}]'   # 别的几步万一问到了：先跳过、答没有（照样做完，题号对不上）
out=$(sess "{$ONBA,\"onboard.hub\":[{\"v\":\"y\"}],\"hub.join\":[{\"v\":\"$HLINK\"}]}" -- cca _onboard 2>&1); rc=$?
ok "配置向导第 4 步答有加入链接：接着是贴加入链接那一页（hub.join），加入了；最后的结果还是 onboarded（加入的 hub_joined 被它盖掉）；事件里没有加入链接" '[ $rc -eq 0 ] && [ "$(asks)" = "onboard.hub hub.join" ] && evq "e.get(\"q\") == \"onboard.hub\" and e[\"title\"] == \"自己的同步服务器\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_joined\"" && [ "$(lastres)" = onboarded ] && [ -e "$H/.cca/hub/hub.json" ] && ! grep -qF -e "$HCHUNK" -e "$HADMIT" "$T/ev.jsonl"'
out=$(sess "{$ONBA}" -- cca _onboard 2>&1); rc=$?
ok "已经加入了自己的同步服务器：向导不问第 4 步" '[ $rc -eq 0 ] && ! asks | grep -q onboard.hub && [ "$(lastres)" = onboarded ]'
cp -p "$T/cfg.onbhub.bak" "$H/.cca/config.json"
run CCA_GRANT=hub_leave:all cca hub leave >/dev/null 2>&1 </dev/null
out=$(sess "{\"hub.join\":[{\"v\":\" \"},{\"v\":\"${HLINK%???}\"},{\"v\":\"$HLINK\"}]}" -- cca hub join 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_hub_join.jsonl"
ok "界面 App 里加入（提问会话）：一个密文输入框，贴加入链接；没输入、链接不对（少了一截）时带着原因重问，不停下；result 事件 hub_joined；事件文件里没有加入链接" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.join hub.join hub.join" ] && evq "e.get(\"seq\") == 2 and e[\"fields\"][0][\"error\"] == \"没有输入加入链接\"" && evq "e.get(\"seq\") == 3 and e[\"fields\"][0][\"error\"] and not e[\"fields\"][0][\"value\"]" && ! evq "e[\"ev\"] == \"fail\"" && evq "e.get(\"q\") == \"hub.join\" and [f[\"kind\"] for f in e[\"fields\"]] == [\"secret\"]" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_joined\"" && ! grep -qF -e "$HCHUNK" -e "$HADMIT" "$T/ev.jsonl" && [ -e "$H/.cca/hub/hub.json" ] && ! grep -qsF -e "$HCHUNK" -e "$HADMIT" "$T/curl.argv" "$T/env.log" "$T/ssh.log"'
cp -p "$H/.cca/config.json" "$T/cfg.hoff.bak"
python3 -I -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["presence"]=False; json.dump(c,open(p,"w"))' "$H/.cca/config.json"
out=$(sess "{\"hub.join\":[{\"v\":\"$HLINK\"}]}" -- cca hub join 2>&1); rc=$?
cp -p "$T/cfg.hoff.bak" "$H/.cca/config.json"
ok "这台设备关掉了设备统计（\"presence\": false）时重新加入：照样加入，结果之前有提醒 hub_off（现在不经它同步，说怎么打开），退出码 0" '[ $rc -eq 0 ] && evbefore fail:hub_off result:hub_joined && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_off\" and \"现在不经它同步\" in e[\"message\"]"'
rm -rf "$H/.cca/hub" "$H/.cca/presence"/own.* "$H/.cca/cost"/own.*
kill "$HVP" 2>/dev/null; wait "$HVP" 2>/dev/null; HVP=""
# 部署到托管平台（docs/dev/hub.md「托管平台」）：cca hub hosted 生成部署文件、记下还没用上的口令；平台那一头是另起的一台
# server/cca-presence.py，证书是测试用的证书机构签的（设备的 curl 经 CURL_CA_BUNDLE 认它：照系统的证书机构验，链接里没有指纹）
out=$(sess '{"hub.platform":[{"v":"sealos"}],"hub.hosted":[{"url":"https://127.0.0.1:1"}]}' -- cca hub hosted 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_hub_hosted_sealos.jsonl"
HHP="$H/.cca/hub-hosted.json"
HSRVSUM=$(shasum -a 256 < "$D/../server/cca-presence.py" | cut -d' ' -f1)
ok "界面 App 里部署到托管平台：先选平台（hub.platform），Sealos 那一页有表单里填的、配置文件的内容（就是服务端程序）、公网地址；还连不上时带着原因重问，取消了口令照样记着（只给自己读）" '[ $rc -ne 0 ] && [ "$(asks)" = "hub.platform hub.hosted hub.hosted" ] && evq "e.get(\"q\") == \"hub.hosted\" and [f[\"id\"] for f in e[\"fields\"]] == [\"steps\", \"server\", \"url\"] and [f[\"kind\"] for f in e[\"fields\"]] == [\"code\", \"code\", \"text\"]" && evq "e.get(\"q\") == \"hub.hosted\" and \"def main():\" in e[\"fields\"][1][\"value\"] and len(e[\"fields\"][1][\"value\"]) > 50000 and \"等一两分钟\" in e[\"fields\"][2][\"error\"]" && [ "$(stat -f %Lp "$HHP")" = 600 ] && [ "$(shasum -a 256 < "$H/cca-hub-sealos/cca-presence.py" | cut -d" " -f1)" = "$HSRVSUM" ] && [ ! -e "$H/.cca/hub" ]'
HHADMIT=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["admit"])' "$HHP")
HHSALT=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["salt"])' "$HHP")
out=$(run CCA_TTY="$(ans '')" cca hub hosted railway 2>&1); rc=$?
HHASH=$(sed -n 's/^ENV CCA_HUB_HOSTED=1 CCA_HUB_ADMIT_HASH=\([0-9a-f]*\)$/\1/p' "$H/cca-hub-railway/Dockerfile")
ok "终端里 cca hub hosted railway：部署文件在 ~/cca-hub-railway（Dockerfile 里只有口令的哈希，还是那一份口令），打出要运行的命令；不填地址是以后再加入" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "部署文件在 $H/cca-hub-railway" && printf "%s" "$out" | grep -q "railway domain --port 8080" && printf "%s" "$out" | grep -q "以后再加入" && [ "$HHASH" = "$(printf "%s" "$HHADMIT" | shasum -a 256 | cut -d" " -f1)" ] && ! grep -rqsF -e "$HHADMIT" -e "$HHSALT" "$H/cca-hub-railway" "$H/cca-hub-sealos"'
HC="$T/hosted-srv"; mkdir -p "$HC"
openssl ecparam -name prime256v1 -genkey -noout -out "$HC/ca.key" 2>/dev/null
openssl req -new -x509 -key "$HC/ca.key" -subj /CN=cca-test-ca -days 2 -out "$HC/ca.pem" 2>/dev/null
openssl ecparam -name prime256v1 -genkey -noout -out "$HC/key.pem" 2>/dev/null
openssl req -new -key "$HC/key.pem" -subj /CN=127.0.0.1 -out "$HC/csr.pem" 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' > "$HC/ext.cnf"
openssl x509 -req -in "$HC/csr.pem" -CA "$HC/ca.pem" -CAkey "$HC/ca.key" -CAcreateserial -days 2 -extfile "$HC/ext.cnf" -out "$HC/cert.pem" 2>/dev/null
python3 -I "$D/../server/cca-presence.py" --mode own --admit-hash "$HHASH" --listen 127.0.0.1:0 --port-file "$HC/port" --cert "$HC/cert.pem" \
	--key "$HC/key.pem" --state "$HC/state.json" --ttl "$STTL" >>"$HC/log" 2>&1 & HCP=$!
for i in $(seq 1 50); do [ -s "$HC/port" ] && break; sleep 0.1; done; HCPORT=$(cat "$HC/port")
: > "$T/curl.argv"; : > "$T/env.log"
out=$(sess "{\"hub.hosted\":[{\"url\":\"cca-hub1.eyJ1IjoiaHR0cHM6Ly9leGFtcGxlLmNvbSJ9.abcd\"},{\"url\":\"http://127.0.0.1:$HCPORT\"},{\"url\":\"https://127.0.0.1:$HCPORT\"}]}" -- env CURL_CA_BUNDLE="$HC/ca.pem" cca hub hosted 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_hub_hosted.jsonl"
ok "App 里再部署一次（平台照记着的 railway，不再问）：Railway 那一页是要运行的命令和公网地址；贴成了加入链接（凭证：bash 先挡住，不交给辅助程序、不回显）、http 的地址带着原因重问；加入了（result 事件 hub_joined），链接里没有指纹、curl 不钉证书；记着的口令删掉；口令、盐不在事件、curl 的命令行和环境里" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.hosted hub.hosted hub.hosted" ] && evq "e.get(\"q\") == \"hub.hosted\" and [f[\"id\"] for f in e[\"fields\"]] == [\"steps\", \"url\"] and e[\"fields\"][0][\"value\"].split(\"\\n\")[-1] == \"railway domain --port 8080\"" && evq "e.get(\"seq\") == 2 and \"这是加入链接\" in e[\"fields\"][1][\"error\"] and e[\"fields\"][1][\"value\"] == \"\"" && ! grep -q eyJ1IjoiaHR0cHM6Ly9leGFtcGxlLmNvbSJ9 "$T/ev.jsonl" && evq "e.get(\"seq\") == 3 and \"https\" in e[\"fields\"][1][\"error\"] and e[\"fields\"][1][\"value\"] == \"http://127.0.0.1:$HCPORT\"" && [ "$(lastres)" = hub_joined ] && [ ! -e "$HHP" ] && python3 -I -c "import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"pins\"] == [] and d[\"hosted\"] == \"railway\" and d[\"url\"] == \"https://127.0.0.1:$HCPORT\" else 1)" "$H/.cca/hub/hub.json" && grep -q "127.0.0.1:$HCPORT/v1/join" "$T/curl.argv" && ! grep "127.0.0.1:$HCPORT" "$T/curl.argv" | grep -q -e pinnedpubkey -e " -k " && ! grep -qsF -e "$HHADMIT" -e "$HHSALT" "$T/ev.jsonl" "$T/curl.argv" "$T/env.log"'
out=$(gj state)
ok "_gui state：自己的那一台不钉指纹（pinned 是 false：更新用 cca hub hosted）" 'printf "%s" "$out" | jok "d[\"config\"][\"hubs\"][-1][\"kind\"] == \"own\" and d[\"config\"][\"hubs\"][-1][\"pinned\"] is False"'
out=$(sess '{"hub.hosted":[{"next":"done"}]}' -- cca hub hosted 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_hub_hosted_update.jsonl"
ok "加入着托管平台上的那一台时 cca hub hosted 是更新：一页说要运行的（railway up），点更新好了收场（result 事件 hub_hosted）；不记口令" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.hosted" ] && evq "e.get(\"q\") == \"hub.hosted\" and [f[\"id\"] for f in e[\"fields\"]] == [\"steps\", \"next\"] and e[\"fields\"][0][\"value\"].split(\"\\n\")[-1] == \"railway up --detach\"" && [ "$(lastres)" = hub_hosted ] && [ ! -e "$HHP" ]'
out=$(runev cca hub deploy root@hubbox 2>&1); rc=$?
ok "加入的是托管平台上的：cca hub deploy 不经 SSH 碰它（fail 事件 hub_platform，说更新用 cca hub hosted）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_platform\" and \"cca hub hosted\" in e[\"message\"]" && ! grep -q hubbox "$T/ssh.log"'
run CCA_GRANT=hub_leave:all cca hub leave >/dev/null 2>&1 </dev/null
kill "$HCP" 2>/dev/null; wait "$HCP" 2>/dev/null
rm -rf "$H/.cca/hub" "$H/.cca/presence"/own.* "$H/.cca/cost"/own.* "$H/cca-hub-railway" "$H/cca-hub-sealos" "$HHP"
# 一键部署（docs/dev/hub.md「部署」）：cca hub deploy 经假 ssh 装到 hubbox（tests/hub_box.py 建的假服务器：目录当根，假 systemctl
# 起真的 server/cca-presence.py），这台 Mac 跟着加入。Docker、拉不到镜像、有设备加入过、连不上、证书换了这些细的在 tests/hub_deploy.py
python3 "$D/hub_box.py" "$T/hubbox"
HB="$T/hubbox/root/opt/cca-hub"; HBPORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
hbj() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$H/.cca/hub/hub.json" "$1" 2>/dev/null; }   # 加入信息里的一项
hbsum() { printf '%s' "$1" | shasum -a 256 | cut -d' ' -f1; }
out=$(run cca hub deploy --script 2>/dev/null); rc=$?
HBV=$(printf '%s\n' "$out" | sed -n "s/^CCA_HUB_VERSION='\(.*\)'\$/\1/p"); HB1=$(printf '%s\n' "$out" | head -n 1)
HBSRV=$(printf '%s\n' "$out" | sed -n "/^	cat <<'CCA_HUB_SERVER_PY_EOF'\$/,/^CCA_HUB_SERVER_PY_EOF\$/p" | sed '1d;$d' | shasum -a 256 | cut -d' ' -f1)
ok "cca hub deploy --script：打出安装脚本（带着这一版的版本号和原样的服务端程序，sh -n 过得去；不带 hub-deploy 给的那几项）" '[ $rc -eq 0 ] && [ "$HB1" = "#!/bin/sh" ] && [ "$HBV" = "$(ver)" ] && printf "%s\n" "$out" | /bin/dash -n && [ "$HBSRV" = "$(shasum -a 256 < "$D/../server/cca-presence.py" | cut -d" " -f1)" ] && ! printf "%s" "$out" | grep -q "CCA_HUB_MANAGED=1"'
out=$(runev cca hub deploy 2>&1); rc=$?
ok "cca hub deploy 没写服务器：说用法" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法: cca hub deploy \[用户@\]主机"'
: > "$T/ssh.log"
out=$(runev cca hub deploy root@hubbox --reset 2>&1); rc=$?
ok "界面 App 里从头部署：先发同意事件（范围是 SSH 的目标，默认不同意，说明里说每台设备都要重新加入），不连服务器" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"hub_reset\" and e[\"scope\"] == \"root@hubbox\" and e[\"default\"] == \"no\" and \"每台设备都要重新加入\" in e[\"detail\"]" && [ ! -e "$HB" ] && ! grep -q hubbox "$T/ssh.log"'
: > "$T/curl.argv"; : > "$T/env.log"
out=$(runev cca hub deploy root@hubbox --port "$HBPORT" --name 机房的服务器 2>&1); rc=$?
HBADMIT=$(hbj admit); HBSALT=$(hbj salt)
ok "cca hub deploy：经 SSH 装好（systemd + python3）、这台 Mac 跟着加入（result 事件 hub_deployed）；服务器上只有入伙口令的哈希；口令、盐不在 ssh、curl、子进程的命令行和环境里，也不在事件里" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已部署自己的同步服务器到 root@hubbox（/opt/cca-hub，systemd + python3，端口 ${HBPORT}），这台设备加入了：它叫 机房的服务器，服务器上现在有 1 台设备" && printf "%s" "$out" | grep -q "127.0.0.1 是内网地址" && evbefore fail:hub_lan result:hub_deployed && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\" and \"127.0.0.1:$HBPORT\" in e[\"message\"]" && [ ${#HBADMIT} -eq 32 ] && [ ${#HBSALT} -eq 32 ] && grep -qx "CCA_HUB_ADMIT_HASH=$(hbsum "$HBADMIT")" "$HB/.env" && ! grep -rqsF -e "$HBADMIT" -e "$HBSALT" "$T/hubbox/root" "$T/ssh.log" "$T/curl.argv" "$T/env.log" "$T/ev.jsonl" && [ "$(stat -f %Lp "$H/.cca/hub/hub.json")" = 600 ]'
out=$(runev cca hub deploy root@hubbox 2>&1); rc=$?
ok "再部署（这台已经加入了）：只更新，加入链接、数据不变，照旧加入着（result 事件说已更新）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已更新 root@hubbox 上自己的同步服务器（/opt/cca-hub，systemd + python3，端口 ${HBPORT}）" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\" and e[\"message\"].startswith(\"已更新\")" && [ "$(hbj admit)" = "$HBADMIT" ] && run cca hub devices --json 2>/dev/null | jok "[x[\"name\"] for x in d[\"devices\"]] == [\"机房的服务器\"]"'
: > "$T/ssh.log"
out=$(runev cca hub deploy root@hubbox --port abc 2>&1); rc=$?
evq 'e["ev"] == "fail" and e["code"] == "hub_usage"'; e1=$?
out2=$(runev cca hub deploy root@hubbox --bogus 2>&1); rc2=$?
ok "参数不对（端口写错了辅助程序先查，不认得的选项 bash 自己查）：不连服务器，fail 事件 hub_usage" '[ $rc -ne 0 ] && [ $e1 -eq 0 ] && [ $rc2 -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_usage\"" && printf "%s" "$out" | grep -q -- "--port 要是 1024～65535" && printf "%s" "$out2" | grep -q "用法: cca hub deploy" && ! grep -q hubbox "$T/ssh.log"'
touch "$T/hubbox/ssh_denied"; out=$(runev cca hub deploy root@hubbox 2>&1); rc=$?; rm -f "$T/hubbox/ssh_denied"
ok "SSH 登录不了：fail 事件 hub_ssh" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_ssh\" and \"SSH 连不上 root@hubbox\" in e[\"message\"]"'
out=$(runev CCA_GRANT=hub_reset:root@hubbox cca hub deploy root@hubbox --reset 2>&1); rc=$?
ok "同意后从头部署：服务器换了入伙口令、忘掉了以前的设备，这台 Mac 用新的重新加入（服务器上 1 台设备）" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\"" && [ "$(hbj admit)" != "$HBADMIT" ] && grep -qx "CCA_HUB_ADMIT_HASH=$(hbsum "$(hbj admit)")" "$HB/.env" && printf "%s" "$out" | grep -q "服务器上现在有 1 台设备"'
# 界面 App 里部署（不写服务器：提问会话 hub.deploy 问地址、SSH 用户、端口；先查登录得上不，密钥不行问密码 hub.password，经 SSH_ASKPASS
# 给 ssh；服务器上别的设备加入过、这台没加入时问要不要从头部署）
HBA=$(hbj admit); HBPINS=$(hbj pins); : > "$T/ssh.log"
rm -f "$HB/certs/cert.pem" "$HB/certs/key.pem"   # 服务器上的证书没了：更新时重新生成，这台 Mac 换上新的指纹，提醒别的设备要新的加入链接
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}]}' -- cca hub deploy 2>&1); rc=$?
ok "界面 App 里部署（不写服务器，提问会话）：一页填地址、SSH 用户、端口（加入着的预填它的地址，说是更新）；先查登录得上不，再部署：只更新，result 事件 hub_deployed" '[ $rc -eq 0 ] && [ "$(asks)" = hub.deploy ] && evq "e.get(\"q\") == \"hub.deploy\" and [f[\"id\"] for f in e[\"fields\"]] == [\"host\", \"user\", \"port\"] and [f[\"value\"] for f in e[\"fields\"]] == [\"127.0.0.1\", \"root\", \"22\"] and \"加入链接和数据都不变\" in e[\"message\"]" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\" and e[\"message\"].startswith(\"已更新\")" && grep -q "^SSH root@hubbox :: true$" "$T/ssh.log" && [ "$(hbj admit)" = "$HBA" ]'
ok "更新时服务器的证书是新生成的：这台 Mac 换上新的指纹，结果之前有提醒 hub_newpin（别的设备要新的加入链接）、hub_lan（地址只在内网），退出码照样是 0" '[ $rc -eq 0 ] && [ -n "$HBPINS" ] && [ "$(hbj pins)" != "$HBPINS" ] && evbefore fail:hub_newpin result:hub_deployed && evbefore fail:hub_lan result:hub_deployed && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_newpin\" and \"新的加入链接\" in e[\"message\"]"'
HBPW="pw-$RANDOM$RANDOM$RANDOM"; printf '%s' "$HBPW" > "$T/hubbox/password"; : > "$T/ssh.log"; : > "$T/env.log"; : > "$T/ssh.env"; mkdir -p "$T/askpass.tmp"
a='{"hub.deploy":[{"host":"nohub","user":"root","port":"22"},{"host":"hubbox","user":"root","port":"22"}],"hub.password":[{"v":"wrong-pw"},{"v":"'$HBPW'"}]}'
out=$(sess "$a" -- env TMPDIR="$T/askpass.tmp" cca hub deploy 2>&1); rc=$?; cp "$T/ev.jsonl" "$T/guifx3/prompts_hub_deploy.jsonl"
ok "App 里部署：连不上（主机名解析不了）带着 ssh 说的原因重问；这台 Mac 的密钥登录不了时问密码（hub.password），错了重问；对了经 SSH_ASKPASS 给 ssh 部署（每次连接取一次）；密码不在事件、输出、ssh 和子进程的命令行里，临时文件都删了" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.deploy hub.deploy hub.password hub.password" ] && evq "e.get(\"seq\") == 2 and \"Could not resolve hostname nohub\" in e[\"fields\"][0][\"error\"] and e[\"fields\"][0][\"value\"] == \"nohub\"" && evq "e.get(\"q\") == \"hub.password\" and [f[\"kind\"] for f in e[\"fields\"]] == [\"secret\"] and e[\"fields\"][0][\"error\"].startswith(\"密码不对，再试一次（密码没错也这样的话：服务器多半不让 root 用密码登录\")" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\"" && [ "$(grep -c "^ASKPASS rc=0 dir=$T/askpass.tmp/cca-askpass\." "$T/ssh.log")" = 3 ] && grep -q "^SSH_ASKPASS=$T/askpass.tmp/cca-askpass\." "$T/ssh.env" && ! grep -qF "$HBPW" "$T/ev.jsonl" "$T/ssh.log" "$T/env.log" "$T/ssh.env" && ! printf "%s" "$out" | grep -qF "$HBPW" && ! ls "$T/askpass.tmp" | grep -q "^cca-askpass"'
HBA=$(hbj admit)
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}],"hub.password":[{"v":"x1"},{"v":"x2"},{"v":"x3"}]}' -- cca hub deploy 2>&1); rc=$?
rm -f "$T/hubbox/password"
ok "密码错了三次：不再问（错得太多服务器会封这个地址），fail 事件 hub_ssh，说可能是不让 root 用密码登录；服务器上什么都没动" '[ $rc -ne 0 ] && [ "$(asks)" = "hub.deploy hub.password hub.password hub.password" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"hub_ssh\" and \"试了 3 次\" in e[\"message\"] and \"不让 root 用密码登录\" in e[\"message\"]" && ! evq "e[\"ev\"] == \"result\"" && [ "$(hbj admit)" = "$HBA" ]'
# 服务器只认密钥、不认这台 Mac 的（ssh_keyonly）：一页（hub.pubkey）给出在服务器上运行的那一行，加好了再查；没有密钥的先生成一把；
# 私钥有密码的选在终端里部署（terminal 事件带上服务器）
[ ! -e "$H/.ssh" ] || mv "$H/.ssh" "$T/home.ssh"
echo always > "$T/hubbox/ssh_denied"
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}],"hub.pubkey":[{"next":"keygen"},{"next":"retry"},{"next":"terminal"}]}' -- cca hub deploy 2>&1); rc=$?
HKP=$(cat "$H/.ssh/id_ed25519.pub" 2>/dev/null)
ok "App 里部署、服务器只认密钥、这台 Mac 还没有密钥：先问要不要生成一把（默认生成），生成了 ~/.ssh/id_ed25519（600）；下一页给出把公钥加进 authorized_keys 的那一行；加好了再试还不认：带着原因重问；选在终端里部署：terminal 事件带上服务器和端口，退出码非 0；服务器上什么都没动" '[ $rc -ne 0 ] && [ "$(asks)" = "hub.deploy hub.pubkey hub.pubkey hub.pubkey" ] && evq "e.get(\"seq\") == 2 and [f[\"id\"] for f in e[\"fields\"]] == [\"next\"] and e[\"fields\"][0][\"default\"] == \"keygen\"" && [ -n "$HKP" ] && [ "$(stat -f %Lp "$H/.ssh/id_ed25519")" = 600 ] && evq "e.get(\"seq\") == 3 and [f[\"id\"] for f in e[\"fields\"]] == [\"cmd\", \"next\"] and e[\"fields\"][0][\"value\"] == \"mkdir -p ~/.ssh && chmod 700 ~/.ssh && echo \x27$HKP\x27 >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys\" and e[\"fields\"][0][\"error\"] == \"\"" && evq "e.get(\"seq\") == 4 and e[\"fields\"][0][\"error\"].startswith(\"还是登录不了\")" && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"hub_deploy\" and e[\"argv\"] == [\"hub\", \"deploy\", \"root@hubbox\", \"-p\", \"22\"] and e[\"display\"] == \"cca hub deploy root@hubbox -p 22\"" && ! evq "e[\"ev\"] == \"result\"" && [ "$(hbj admit)" = "$HBA" ]'
echo once > "$T/hubbox/ssh_denied"
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}],"hub.pubkey":[{"next":"retry"}]}' -- cca hub deploy 2>&1); rc=$?
ok "加好了公钥再试：登录上了，接着部署（只更新，result 事件 hub_deployed）；已经有密钥的不再问要不要生成" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.deploy hub.pubkey" ] && evq "e.get(\"q\") == \"hub.pubkey\" and [f[\"id\"] for f in e[\"fields\"]] == [\"cmd\", \"next\"]" && [ "$(lastres)" = hub_deployed ] && [ ! -e "$T/hubbox/ssh_denied" ] && [ "$(hbj admit)" = "$HBA" ]'
rm -rf "$H/.ssh"; [ ! -e "$T/home.ssh" ] || mv "$T/home.ssh" "$H/.ssh"
out=$(run CCA_TTY="$(ans hubbox '' '')" cca hub deploy 2>&1); rc=$?
ok "终端里不写服务器（App 里「改在终端里完成」）：问地址（加入着的预填它的）、SSH 用户（root）、端口（22），再部署" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已更新 root@hubbox 上自己的同步服务器" && [ "$(hbj admit)" = "$HBA" ]'
run CCA_GRANT=hub_leave:all cca hub leave >/dev/null 2>&1 </dev/null
HBH=$(sed -n 's/^CCA_HUB_ADMIT_HASH=//p' "$HB/.env")
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}],"consent.hub_reset":[{"v":"n"}]}' -- cca hub deploy 2>&1); rc=$?
ok "App 里部署到别的设备加入过的服务器（这台没加入）：问要不要从头部署（说可以在加入了的设备上复制加入链接）；不同意：取消，服务器上什么都没动" '[ $rc -ne 0 ] && [ "$(asks)" = "hub.deploy consent.hub_reset" ] && evq "e.get(\"q\") == \"consent.hub_reset\" and e[\"fields\"][0][\"default\"] == \"n\" and \"复制加入链接\" in e[\"fields\"][0][\"detail\"]" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\"" && [ "$(sed -n "s/^CCA_HUB_ADMIT_HASH=//p" "$HB/.env")" = "$HBH" ] && [ ! -e "$H/.cca/hub/hub.json" ]'
out=$(sess '{"hub.deploy":[{"host":"hubbox","user":"root","port":"22"}],"consent.hub_reset":[{"v":"y"}]}' -- cca hub deploy 2>&1); rc=$?
ok "同意：从头部署（换了入伙口令），这台 Mac 加入了" '[ $rc -eq 0 ] && [ "$(asks)" = "hub.deploy consent.hub_reset" ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"hub_deployed\"" && [ "$(sed -n "s/^CCA_HUB_ADMIT_HASH=//p" "$HB/.env")" != "$HBH" ] && [ -e "$H/.cca/hub/hub.json" ]'
run CCA_GRANT=hub_leave:all cca hub leave >/dev/null 2>&1 </dev/null
out=$(run cca hub deploy --script 2>/dev/null | run ssh root@hubbox sh -s -- --uninstall 2>&1); rc=$?
ok "照文档卸载（cca hub deploy --script | ssh 服务器 sh -s -- --uninstall）：停掉服务、删掉程序和数据" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "卸载了自己的同步服务器" && [ ! -e "$HB" ] && [ ! -e "$T/hubbox/server.pid" ] && [ ! -e "$H/.cca/hub" ]'
rm -rf "$H/.cca/hub" "$H/.cca/presence"/own.* "$H/.cca/cost"/own.*
if command -v swiftc >/dev/null 2>&1 && [ -f "$D/../macos/test.sh" ]; then
	ok "App 的契约测试：Swift 模型解码真实的 cca devices --json、同步来的 API key、remote list --json、自己的同步服务器和配对的提问（macos/test.sh）" 'bash "$D/../macos/test.sh" "$T/guifx3" > "$T/contract3.log" 2>&1'
fi
kill "$SVP" 2>/dev/null; wait "$SVP" 2>/dev/null; SVP=""
cp -p "$T/pres.config.json" "$H/.cca/config.json"; cp -p "$T/pres.accounts.json" "$H/.cca/accounts.json"; cp -p "$T/pres.keys.json" "$T/deploy_keys.json"
cp -p "$T/pres.pr.json" "$PR/policy.json"; cp -p "$T/pres.pr.sig" "$PR/policy.json.sig"; cp -p "$T/pres.local.json" "$H/.cca/policy/policy.json"
cp -p "$T/pres.mr.json" "$MR/policy.json"; cp -p "$T/pres.aj.json" "$AJ"; rm -rf "$PRD" "$T/keychain/cca-ref-$AAID"

fi
if sect 11b4; then
# 11b4. 内置代理：在外面、这台 Mac 上又没有 Clash 客户端时，用界面 App 托管的 mihomo 内核（假的：状态在 $T/core；假 open 打开
#       界面 App 时像 App 那样在后台运行 cca _core run）。内核文件按写死的 SHA256 校验（mkver 换成了假内核的），节点定义在家时
#       从路由器拷（core/nodes.json），再补维护者的参考节点。这一节不用映射（config.json 里的 policy、~/.cca/policy 先挪开），
#       结束时都还原。这时路由器上的节点是 US-A～D 和 HK-A2（10a 里改过名），可选节点也是这几个
CD="$T/core"; CN="$H/.cca/core"
creloads() { grep -c '^RELOAD$' "$CD/core.log" 2>/dev/null || echo 0; }
csel() { cat "$CD/sel/$1" 2>/dev/null || echo -; }
cnames() { python3 -c 'import json,sys; print(",".join(p["name"] for p in json.load(open(sys.argv[1]))["proxies"]))' "$1"; }
ccfg() { python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)' "$CN/config.yaml" "$1"; }   # 内置代理的配置（c）满足条件
core_stop() { [ -f "$CD/up" ] && kill "$(cat "$CD/pid")" 2>/dev/null; local i; for i in $(seq 1 30); do [ -f "$CD/up" ] || return 0; sleep 0.1; done; }
nodes_wait() { local i; for i in $(seq 1 80); do [ -f "$CN/nodes.json" ] && [ ! -d "$H/.cca/core-nodes.lock" ] && return 0; sleep 0.1; done; return 1; }
cp "$H/.cca/config.json" "$T/config.core"; cp "$H/.cca/accounts.json" "$T/accounts.core"
run cca local off >/dev/null 2>&1
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy', None); c.pop('local', None); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"   # 连 local 一起清掉（上一句 cca local off 记的 core_off）：模拟全新机器，在家照样拷节点
rm -rf "$T/policy.core"; mv "$H/.cca/policy" "$T/policy.core"; mkdir -p "$H/.cca/policy"
edit_table 'j["alice@example.com"] = "US-A"'
mkdir -p "$GA/Contents/MacOS"
printf '<plist><dict><key>CFBundleIdentifier</key><string>local.cca.app</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>' "$(ver)" > "$GA/Contents/Info.plist"
: > ssh.log; : > open.log
run cca @alice --version >/dev/null 2>&1; nodes_wait
ok "在家启动账号：没有 Clash 客户端的机器在后台从路由器拷一份节点定义（只留可选节点；600，目录 700，临时文件不留）" '[ "$(cnames "$CN/nodes.json")" = US-A,US-B,US-C,US-D,HK-A2 ] && [ "$(stat -f %Lp "$CN/nodes.json")" = 600 ] && [ "$(stat -f %Lp "$CN")" = 700 ] && grep -q "sh -s defs" ssh.log && grep -q "\"password\"" "$CN/nodes.json" && [ -z "$(ls "$H/.cca" | grep "^defs\.")" ]'
: > ssh.log; run cca @alice --version >/dev/null 2>&1; sleep 1
ok "节点定义还新（12 小时内、可选节点没变）：不再拷" '! grep -q "sh -s defs" ssh.log'
out=$(run cca local off 2>&1)
ok "还没配置本机客户端时 cca local off：关掉内置代理的准备（记下 core_off），拷来的节点删掉" 'printf "%s" "$out" | grep -q "已关掉内置代理的准备" && cfgok "c[\"local\"] == {\"core_off\": \"1\"}" && [ ! -e "$CN/nodes.json" ]'
: > ssh.log; run cca @alice --version >/dev/null 2>&1; sleep 1
ok "关掉了：在家不再拷节点定义" '! grep -q "sh -s defs" ssh.log && [ ! -e "$CN/nodes.json" ]'
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['local'] = {}; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"; run cca _core-nodes >/dev/null 2>&1
# 经 dialer-proxy 连的节点：它用到的节点（不是可选节点的 HOP-A）也拷；指向策略组的不要（内置代理里没有那个组，带上它整份配置都加载不了）
cp "$RC" "$T/rc.core"; cp "$H/.cca/config.json" "$T/config.core2"
ruby -e 'f = ARGV[0]; s = File.read(f); s.sub!(/^proxies:\n( *)- /) { i = $1; "proxies:\n" + ["{name: US-E, type: ss, server: 192.0.2.6, port: 8388, cipher: aes-128-gcm, password: x, dialer-proxy: HOP-A}", "{name: US-F, type: ss, server: 192.0.2.7, port: 8388, cipher: aes-128-gcm, password: x, dialer-proxy: \"🚀 节点选择\"}", "{name: HOP-A, type: ss, server: 192.0.2.8, port: 8388, cipher: aes-128-gcm, password: x}"].map { |x| "#{i}- #{x}\n" }.join + "#{i}- " } or abort; File.write(f, s)' "$RC"   # 照它第一项的缩进插在最前面
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['nodes'] += ['US-E', 'US-F']; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
run cca _core-nodes >/dev/null 2>&1
ok "拷节点定义：可选节点经 dialer-proxy 用到的 HOP-A 也拷；dialer-proxy 指向策略组的 US-F 不要" '[ "$(cnames "$CN/nodes.json")" = US-E,HOP-A,US-A,US-B,US-C,US-D,HK-A2 ]'
cp "$T/rc.core" "$RC"; cp "$T/config.core2" "$H/.cca/config.json"; run cca _core-nodes >/dev/null 2>&1
# 路由器端 defs 自己写 JSON（拷的那台机器上不用 ruby）：和 ruby 把配置读出来再 JSON.generate 的逐项一样（引号、反斜杠、控制字符、
# emoji、整数和大整数、小数、true/false/null、空的 {} []、不是字符串的键、锚点和合并）
ruby -e 'f = ARGV[0]; s = File.read(f); s.sub!(/^proxies:\n( *)- /) { i = $1; "proxies:\n" + ["{name: \"🇺🇸 美国 \\\"A\\\" \\\\ 反斜杠\", type: ss, server: 192.0.2.31, port: 8388, cipher: aes-128-gcm, password: \"p\\\"a\\\\ss\\tx\\u0001y\\nz\", udp: true, tfo: false, plugin: ~, ratio: 1.5, big: 12345678901234567890, alpn: [h2, \"http/1.1\"], ws-opts: {path: /ws, headers: {Host: example.com}}, e1: {}, e2: [], 1: one}", "&A1 {name: N-A1, type: ss, server: 192.0.2.32, port: 8388, cipher: aes-128-gcm, password: x, smux: {enabled: true, protocol: h2mux}}", "{<<: *A1, name: N-A2, server: 192.0.2.33}"].map { |x| "#{i}- #{x}\n" }.join + "#{i}- " } or abort; File.write(f, s)' "$RC"
rr defs > "$T/defs.out" 2>&1
ruby -E UTF-8 -ryaml -rjson -e 'c = begin YAML.load(File.read(ARGV[0]), aliases: true) rescue ArgumentError; YAML.load_file(ARGV[0]) end; print JSON.generate({"proxies" => c["proxies"].select { |p| p.is_a?(Hash) }})' "$RC" > "$T/defs.want"
ok "路由器端 defs 自己写 JSON（最后一行是结束标记）：和 ruby 读出来再 JSON.generate 的一样（引号、反斜杠、控制字符、emoji、大整数、小数、null、空的 {} []、不是字符串的键、锚点和合并）" 'python3 -c "import json,sys; g=open(sys.argv[1],encoding=\"utf-8\").read(); e=\"\n# cca-defs-end\n\"; w=json.load(open(sys.argv[2],encoding=\"utf-8\")); p=json.loads(g[:-len(e)])[\"proxies\"] if g.endswith(e) else None; sys.exit(0 if p == w[\"proxies\"] and p[0][\"password\"] == \"p\\\"a\\\\ss\\tx\\x01y\\nz\" and p[0][\"big\"] == 12345678901234567890 and p[2][\"smux\"] == {\"enabled\": True, \"protocol\": \"h2mux\"} and p[2][\"server\"] == \"192.0.2.33\" else 1)" "$T/defs.out" "$T/defs.want"'
cp "$T/rc.core" "$RC"
# 这台机器上没有 ruby（Linux 服务器默认没装）：照样拷（本机的 ruby 换成一调用就记一笔、失败的假的；路由器那边照常用真的）
mkdir -p "$T/noruby"; printf '#!/bin/sh\necho "LOCAL RUBY $*" >> "$CCA_T/noruby.log"\nexit 127\n' > "$T/noruby/ruby"; chmod +x "$T/noruby/ruby"
printf '#!/bin/sh\nexec %s "$@"\n' "$(PATH=/opt/homebrew/bin:/usr/bin:/bin command -v ruby)" > "$T/rbin/ruby"; chmod +x "$T/rbin/ruby"
NORUBY="$T/noruby:$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
rm -f "$CN/nodes.json"; : > "$T/noruby.log"; : > ssh.log
run PATH="$NORUBY" cca _core-nodes >/dev/null 2>&1
ok "拷节点定义：这台机器上不用 ruby（JSON 由路由器端写好，这边只用 Python 读）" '[ ! -s "$T/noruby.log" ] && grep -q "sh -s defs" ssh.log && [ "$(cnames "$CN/nodes.json")" = US-A,US-B,US-C,US-D,HK-A2 ] && [ "$(stat -f %Lp "$CN/nodes.json")" = 600 ]'
printf '#!/bin/sh\n%s "$@" | head -c 300\n' "$(PATH=/opt/homebrew/bin:/usr/bin:/bin command -v ruby)" > "$T/rbin/ruby"   # 路由器那边只输出了前 300 个字节（连接断了）
rm -f "$CN/nodes.json"
run PATH="$NORUBY" cca _core-nodes >/dev/null 2>&1
ok "拷节点定义：没读到结束标记（没拷完整）就不写" '[ ! -e "$CN/nodes.json" ] && [ -z "$(ls "$CN" | grep "^nodes\.json\.")" ]'
rm -f "$T/rbin/ruby"; run cca _core-nodes >/dev/null 2>&1
touch "$T/away"; : > ssh.log; : > open.log; : > gh.log; : > curl.log
out=$(runev cca @alice --version 2>&1); rc=$?
ok "在外面、没有 Clash 客户端、非交互（界面 App）：记一条 consent（builtin_core），不下载、不启动" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"builtin_core\" and e[\"grant\"] == \"builtin_core\"" && ! grep -q "GH mihomo" gh.log && ! cfgok "c[\"local\"].get(\"client\")"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面、还没配置内置代理、非交互（Dock 上的启动器、脚本）：说明可以用内置代理，交给终端（cca local setup），不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "在终端里运行 cca local setup 配置一次" && ! cfgok "c[\"local\"].get(\"client\")" && ! grep -q "GH mihomo" gh.log'
out=$(runev CCA_GRANT=builtin_core cca @alice --version 2>&1); rc=$?
ok "界面 App 里同意之后（带着授权重跑）：改用内置代理：下载内核（按 SHA256 校验），在后台打开界面 App（--core），App 起内核，账号照常启动" '[ $rc -eq 3 ] && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ -x "$CN/mihomo-v1.19.33" ] && grep -q "^GH mihomo mihomo-darwin-.*-v1.19.33.gz$" gh.log && grep -qF "OPEN [-g] [$GA] [--args] [--core]" open.log && grep -qx START "$CD/core.log" && [ "$(cat "$CD/home")" = "$CN" ] && [ "$(cat "$CN/mihomo.pid")" = "$(cat "$CD/pid")" ] && [ -z "$(ls "$CN" | grep "^dl\.")" ]'
ok "在外面：经内置代理的 7901 出去（CCA-1 选中 US-A），Claude Code 拿到的代理也是它；不连路由器" '[ "$(csel CCA-1)" = US-A ] && grep -q "CURL via local:7901 https://claude.ai/cdn-cgi/trace" curl.log && lastenv | grep -q "https_proxy=http://127.0.0.1:7901" && [ ! -s ssh.log ]'
run cca usage alice >/dev/null 2>&1
ok "在外面 cca usage：经内置代理的端口查" 'grep "^USAGE: " claude.log | tail -n 1 | grep -q "email=alice@example.com https_proxy=http://127.0.0.1:7901"'
ok "内置代理的配置（600）：只有 127.0.0.1 上的入站（各自直通 CCA-n）、可选节点、一条 MATCH,REJECT；不开通用的代理端口、TUN、DNS；控制接口只开 Unix socket，不记选中的节点" '[ "$(stat -f %Lp "$CN/config.yaml")" = 600 ] && ccfg "all(l[\"listen\"] == \"127.0.0.1\" and l[\"proxy\"] == \"CCA-%d\" % (l[\"port\"] - 7900) for l in c[\"listeners\"]) and c[\"rules\"] == [\"MATCH,REJECT\"] and not any(k in c for k in (\"port\", \"mixed-port\", \"socks-port\", \"tun\", \"dns\", \"external-controller\")) and c[\"external-controller-unix\"].endswith(\".sock\") and c[\"profile\"][\"store-selected\"] is False and [p[\"name\"] for p in c[\"proxies\"]] == [\"US-A\", \"US-B\", \"US-C\", \"US-D\", \"HK-A2\"] and c[\"proxy-groups\"][0][\"proxies\"][0] == \"US-A\""'
# 别的程序开着 TUN（Clash 客户端的虚拟网卡模式、VPN）：去节点的流量走虚拟网卡时，内核的出站绑定到物理网卡（macOS 的 interface-name）
cp "$CN/config.yaml" "$T/core.notun"; n0=$(creloads); touch "$T/tun"
out=$(run cca @alice --version 2>&1); rc=$?
ok "开着 TUN（去公网的路由走 utun9）：内置代理的出站绑定物理网卡 en0、重新加载一次，照常启动；系统的 DNS 经物理网卡到得了就不另配 DNS" '[ $rc -eq 3 ] && [ "$(creloads)" = $((n0 + 1)) ] && ccfg "c[\"interface-name\"] == \"en0\" and \"dns\" not in c" && printf "%s" "$out" | grep -q "别的程序开着虚拟网卡 utun9（Clash 客户端的 TUN、VPN 这类）：内置代理连节点时绑定物理网卡 en0" && grep -q "CURL via local:7901 https://claude.ai/cdn-cgi/trace" curl.log'
run cca @alice --version >/dev/null 2>&1
out=$(run cca local 2>&1)
ok "开着 TUN 再启动：不再重新加载；cca local 说出站绑定了哪块网卡" '[ "$(creloads)" = $((n0 + 1)) ] && printf "%s" "$out" | grep -q "绑定物理网卡 en0，绕开它"'
out=$(gj state)
ok "开着 TUN：_gui state 的本机端口池是同步好的（摘要里带着绑定的网卡）" 'printf "%s" "$out" | jok "d[\"routing\"][\"local_sync\"] == \"ok\""'
printf 'nameserver 198.18.0.2\nnameserver 127.0.0.1\n' > "$T/resolv.conf"
out=$(run cca @alice --version 2>&1); rc=$?
ok "系统的 DNS 是 TUN 自己的地址（走虚拟网卡）、本机的：改用物理网卡从 DHCP 拿到的 DNS 解析节点的域名，重新加载一次" '[ $rc -eq 3 ] && [ "$(creloads)" = $((n0 + 2)) ] && ccfg "c[\"interface-name\"] == \"en0\" and c[\"dns\"][\"nameserver\"] == [\"192.0.2.53\"] and c[\"dns\"][\"default-nameserver\"] == [\"192.0.2.53\"]" && printf "%s" "$out" | grep -q "节点的域名经 192.0.2.53 解析"'
printf 'nameserver 192.0.2.53\n' > "$T/resolv.conf"
cfgset() { python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); exec(sys.argv[2]); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json" "$1"; }
cfgset 'c["core_bind"] = False'
run cca @alice --version >/dev/null 2>&1
ok "config.json 的 core_bind: false：开着 TUN 也不绑定" 'ccfg "\"interface-name\" not in c and \"dns\" not in c"'
cfgset 'c.pop("core_bind")'; rm -f "$T/tun"
run cca @alice --version >/dev/null 2>&1
ok "关掉 TUN：配置和开 TUN 以前逐字节一样（不绑定、不配 DNS）" 'cmp -s "$T/core.notun" "$CN/config.yaml"'
# 在外面经 ZeroTier、VPN 也连得到家里的路由器（去路由器的路由走虚拟网卡）：不算在家，照样走内置代理；绑定物理网卡探路由器
mkdir -p "$T/route_if"; echo feth9 > "$T/route_if/192.0.2.10"; touch "$T/vpn"; : > ssh.log; : > curl.log; : > nc.log
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面经 ZeroTier 连得到路由器（路由走 feth9）：绑定物理网卡探不通，不算在家，经内置代理出去，不连路由器" '[ $rc -eq 3 ] && grep -q "CURL via local:7901 https://claude.ai/cdn-cgi/trace" curl.log && ! grep -q "CURL via :" curl.log && [ ! -s ssh.log ] && grep -q "^NC -z -G 1 -b en0 192.0.2.10 " nc.log'
out=$(run cca local 2>&1)
ok "cca local：说路由器只经虚拟网卡连得到、不算在家" 'printf "%s" "$out" | grep -q "路由器 192.0.2.10 只经虚拟网卡 feth9（VPN、ZeroTier 这类）连得到，不算在家：账号走内置代理的端口"'
out=$(gj live)
ok "_gui live：经 ZeroTier 连得到路由器时看内置代理（不算在家），路由器算连得上" 'printf "%s" "$out" | jok "d[\"route\"] == \"local\" and d[\"at_home\"] is False and d[\"router\"][\"reachable\"] is True and d[\"router\"][\"error\"] is None"'
: > curl.log
out=$(run CCA_ROUTE=router cca @alice --version 2>&1); rc=$?
ok "设成只走路由器（route: router）：经 ZeroTier 照样走路由器的端口" '[ $rc -eq 3 ] && grep -q "CURL via :7901 https://claude.ai/cdn-cgi/trace" curl.log && ! grep -q "CURL via local:" curl.log'
rm -f "$T/vpn"; : > curl.log
out=$(run cca local 2>&1)
ok "ZeroTier 也连不上了：照旧说连不上路由器" 'printf "%s" "$out" | grep -q "现在连不上路由器 192.0.2.10：账号走内置代理的端口"'
rm -rf "$T/route_if"
n0=$(creloads); : > open.log; : > gh.log
run cca @alice --version >/dev/null 2>&1
ok "再启动：内核在跑，不再下载、不打开界面 App、不重新加载" '[ "$(creloads)" = "$n0" ] && [ ! -s open.log ] && ! grep -q "GH mihomo" gh.log'
printf US-B > "$CD/sel/CCA-1"
run cca @alice --version >/dev/null 2>&1
ok "选中被改：启动前经控制接口纠正回 US-A，不重新加载" '[ "$(csel CCA-1)" = US-A ] && [ "$(creloads)" = "$n0" ]'
edit_table 'j["alice@example.com"] = "US-C"'
run cca @alice --version >/dev/null 2>&1
ok "换节点：经控制接口切到 US-C，不重新加载；配置里 CCA-1 的第一个成员也换成 US-C（内核重启后照样选它）" '[ "$(csel CCA-1)" = US-C ] && [ "$(creloads)" = "$n0" ] && ccfg "c[\"proxy-groups\"][0][\"proxies\"][0] == \"US-C\""'
cp "$CN/nodes.json" "$T/nodes.core.complete"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j['proxies']=[x for x in j['proxies'] if x['name']!='US-D']; json.dump(j, open(p,'w'))" "$CN/nodes.json"
edit_table 'j["alice@example.com"] = "US-D"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "账号的节点内置代理里没有定义：重新加载（组里 REJECT 排第一），不启动，说明怎么补" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "内置代理里没有节点「US-D」的定义" && [ "$(csel CCA-1)" = REJECT ] && [ "$(creloads)" = $((n0 + 1)) ]'
out=$(run cca local 2>&1)
ok "cca local：内置代理的端口和选中的节点；节点定义从哪来、缺哪个" 'printf "%s" "$out" | grep -q "内置代理（控制接口 unix:" && printf "%s" "$out" | grep -q "CCA-1  127.0.0.1:7901  监听中  → REJECT" && printf "%s" "$out" | grep -q "节点定义：从路由器拷的 4 个" && printf "%s" "$out" | grep -q "没有定义的节点：US-D"'
# 维护者的参考节点补上路由器那份里没有的（和往路由器里补的同一套规则：同名的以路由器的为准）；共享开着才认参考节点
cfgset 'c["policy"] = "example/cca-policy"'
printf '%s\n' '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-D", "type": "ss", "server": "192.0.2.4", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}, {"name": "US-A", "type": "ss", "server": "192.0.2.99", "port": 8388, "cipher": "aes-128-gcm", "password": "y"}, {"name": "AI-Z", "type": "ss", "server": "192.0.2.29", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}]}' > "$H/.cca/policy/nodes.json"
out=$(run cca @alice --version 2>&1); rc=$?
ok "维护者的参考节点补上缺的 US-D：重新加载一次，照常启动；同名的 US-A 以路由器的为准，不是可选节点的 AI-Z 不放" '[ $rc -eq 3 ] && [ "$(csel CCA-1)" = US-D ] && [ "$(creloads)" = $((n0 + 2)) ] && ccfg "[p[\"name\"] for p in c[\"proxies\"]] == [\"US-A\", \"US-B\", \"US-C\", \"HK-A2\", \"US-D\"] and c[\"proxies\"][0][\"server\"] == \"192.0.2.1\""'
rm -f "$H/.cca/policy/nodes.json"; cfgset 'c.pop("policy")'; edit_table 'j["alice@example.com"] = "US-A"'
cp "$T/nodes.core.complete" "$CN/nodes.json"   # 缺节点负例结束：恢复 dave 所需的 US-D，后续功能测试仍要求全池核验通过
run cca @alice --version >/dev/null 2>&1
edit_table 'j.update({"u%d@example.com" % i: "US-B" for i in range(3, 11)})'
n0=$(creloads)
run cca @alice --version >/dev/null 2>&1
ok "端口池要扩容（第 9 个账号起）：内置代理重新加载一次配置，变成 16 个端口" '[ "$(creloads)" = $((n0 + 1)) ] && grep -qx 7916 "$CD/listening" && [ "$(csel CCA-1)" = US-A ]'
edit_table '[j.pop("u%d@example.com" % i) for i in range(3, 11)]'
echo 192.0.2.2 > "$CD/reject_text"
out=$(run cca @alice --version 2>&1); rc=$?
ok "内置代理的内核不收新配置：说明原因，不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "内置代理的内核没接受新配置（HTTP 400）"'
rm -f "$CD/reject_text"
run cca @alice --version >/dev/null 2>&1
ok "内核收下了：端口池缩回 8 个" '! grep -qx 7909 "$CD/listening" && grep -qx 7908 "$CD/listening"'
out=$(gj state)
ok "_gui state：本机客户端是内置代理（名字由 cca 给），已经同步" 'printf "%s" "$out" | jok "d[\"config\"][\"local\"] == {\"configured\": True, \"client\": \"builtin\", \"name\": \"内置代理\", \"udp\": {\"node\": None, \"state\": \"off\", \"installed\": False, \"running\": False, \"other_vpn\": None, \"paused\": False}} and d[\"routing\"][\"local_sync\"] == \"ok\""'
out=$(gj live)
ok "_gui live：在外面看内置代理：在运行，alice 的槽位正常" 'printf "%s" "$out" | jok "d[\"route\"] == \"local\" and d[\"local\"][\"name\"] == \"内置代理\" and d[\"local\"][\"running\"] is True and d[\"slots\"][0][\"state\"] == \"ok\""'
# 内核文件校验不对、界面 App 没装
core_stop; rm -f "$CN"/mihomo-v*
for f in "$T"/mihomo/*.gz; do cp "$f" "$f.orig"; printf x >> "$f"; done
out=$(run cca @alice --version 2>&1); rc=$?
ok "下载的内核校验不对：不用它（临时文件删掉），不启动，说明原因" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "下载的文件校验不对" && [ ! -e "$CN/mihomo-v1.19.33" ] && [ -z "$(ls "$CN" | grep "^dl\.")" ] && [ ! -f "$CD/up" ]'
for f in "$T"/mihomo/*.gz; do mv "$f.orig" "$f"; done
mv "$GA" "$GA.off"
out=$(runev cca @alice --version 2>&1); rc=$?
ok "没装界面 App、非交互（启动器、脚本）：说明，交给终端（cca local setup）" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"terminal\" and e[\"code\"] == \"local_setup\"" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"local_unavailable\"" && [ ! -f "$CD/up" ]'
mv "$GA.off" "$GA"
run cca @alice --version >/dev/null 2>&1
ok "装回界面 App：再下载内核、起内核，照常启动" '[ -x "$CN/mihomo-v1.19.33" ] && [ -f "$CD/up" ] && [ "$(csel CCA-1)" = US-A ]'
# 终端里（交互）在外面、还没配置本机客户端：照 cca local setup 的流程问（用内置代理吗）
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['local'] = {}; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "终端里在外面、还没配置本机客户端：照 cca local setup 问（用内置代理吗），答 y 照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "用内置代理吗" && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ "$(csel CCA-1)" = US-A ]'
# 关掉、在家重新配置
run cca local off >/dev/null 2>&1
ok "cca local off：关掉内置代理（记下关掉过），节点定义和配置删掉，停掉内核（界面 App 没开着时）；内核文件留着" 'cfgok "c[\"local\"] == {\"core_off\": \"1\"}" && [ ! -e "$CN/nodes.json" ] && [ ! -e "$CN/config.yaml" ] && [ ! -f "$CD/up" ] && [ -x "$CN/mihomo-v1.19.33" ]'
out=$(run CCA_GUI_OS=15 cca @alice --version 2>&1); rc=$?
ok "macOS 26 以下也能用内置代理（内核由 cca 在后台无头运行，不要界面 App）：还没配置时交给 cca local setup，不再说要 macOS 26" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "cca local setup" && ! printf "%s" "$out" | grep -q "要 macOS 26"'
rm -f "$T/away"; : > ssh.log
run cca @alice --version >/dev/null 2>&1; sleep 1
ok "关掉过内置代理：在家不再拷节点定义" '! grep -q "sh -s defs" ssh.log && [ ! -e "$CN/nodes.json" ]'
mkdir -p "$T/route_if"; echo utun9 > "$T/route_if/192.0.2.10"; : > curl.log; : > nc.log
out=$(run cca @alice --version 2>&1); rc=$?
ok "在家开着 TUN、去路由器的路由走 utun9：绑定物理网卡探得通，照样算在家、走路由器的端口" '[ $rc -eq 3 ] && grep -q "CURL via :7901 https://claude.ai/cdn-cgi/trace" curl.log && grep -q "^NC -z -G 1 -b en0 192.0.2.10 " nc.log'
rm -rf "$T/route_if"
out=$(run CCA_TTY="$(ans y)" cca local setup 2>&1)
ok "cca local setup：用内置代理（问一句），在家先从路由器拷节点定义，然后让内核就绪" 'printf "%s" "$out" | grep -q "用内置代理吗" && printf "%s" "$out" | grep -q "本机端口池已就绪：内置代理 127.0.0.1:7901" && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ "$(cnames "$CN/nodes.json")" = US-A,US-B,US-C,US-D,HK-A2 ] && [ -f "$CD/up" ]'
# 自定义节点：分享链接 / Clash 节点块 → 内核 -t 校验 → 存进 core/custom-nodes.json（600），名字加进可选节点（配置了路由器的同步时也写进路由器，见 11b2c）
cp "$H/.cca/config.json" "$T/cfg.cn.bak"
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('router',None); json.dump(c,open(p,'w'),indent=2,ensure_ascii=False)" "$H/.cca/config.json"
out=$(sess '{"add.basic":[{"email":"cn@example.com","alias":""}]}' --eof-at add.node -- cca add 2>&1); rc=$?
ok "加账号选节点：没有路由器、能用内置代理时，末尾多一项「自定义代理…」（选它可粘贴分享链接 / Clash 节点）" '[ "$(asks)" = "add.basic add.node" ] && evq "e.get(\"q\") == \"add.node\" and any(\"自定义代理\" in o[\"label\"] for o in e[\"fields\"][0][\"options\"])"'
VMLINK=$(python3 -c "import base64,json;print('vmess://'+base64.b64encode(json.dumps({'ps':'My Node','add':'198.51.100.9','port':'443','id':'uu','aid':'0','net':'ws','host':'h.example.com','path':'/p','tls':'tls','scy':'auto'}).encode()).decode())")
out=$(run cca node add "$VMLINK" 2>&1); rc=$?
ok "cca node add 分享链接：解析 vmess、内核 -t 校验、存进 custom-nodes.json（600），名字加进可选节点" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "加好了自定义节点「My Node」" && [ "$(stat -f %Lp "$CN/custom-nodes.json")" = 600 ] && python3 -c "import json,sys; p=json.load(open(sys.argv[1]))[\"proxies\"][0]; sys.exit(0 if p[\"name\"]==\"My Node\" and p[\"server\"]==\"198.51.100.9\" and p[\"type\"]==\"vmess\" and p[\"network\"]==\"ws\" else 1)" "$CN/custom-nodes.json" && cfgok "\"My Node\" in c[\"nodes\"]"'
ok "cca node ls：列出自定义节点（名字、类型、服务器:端口）" 'run cca node ls 2>&1 | grep -q "My Node（vmess 198.51.100.9:443）"'
out=$(run cca node add "这不是节点配置" 2>&1); rc=$?
ok "cca node add 认不出的内容：报错、不存（custom-nodes.json 还是只有一个）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "认不出" && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if len(j[\"proxies\"])==1 else 1)" "$CN/custom-nodes.json"'
echo "203.0.113.77" > "$T/core/reject_text"
out=$(run cca node add "vless://u@203.0.113.77:443?security=tls#Bad" 2>&1); rc=$?
rm -f "$T/core/reject_text"
ok "cca node add 内核 -t 校验没过：报错、不存" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "校验没通过" && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if len(j[\"proxies\"])==1 else 1)" "$CN/custom-nodes.json"'
python3 -c "import json,sys; p=sys.argv[1]; t=json.load(open(p)); t[\"甲(z@example.com)\"]=\"My Node\"; json.dump(t,open(p,'w'),ensure_ascii=False)" "$H/.cca/accounts.json"
out=$(run cca node rm "My Node" 2>&1); rc=$?
ok "cca node rm：还有账号在用时不删（说哪个账号在用）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "还有账号在用" && cfgok "\"My Node\" in c[\"nodes\"]"'
python3 -c "import json,sys; p=sys.argv[1]; t=json.load(open(p)); t.pop(\"甲(z@example.com)\",None); json.dump(t,open(p,'w'),ensure_ascii=False)" "$H/.cca/accounts.json"
out=$(run cca node rm "My Node" 2>&1); rc=$?
ok "cca node rm：从 custom-nodes.json 和可选节点里去掉" '[ $rc -eq 0 ] && ! cfgok "\"My Node\" in c[\"nodes\"]" && python3 -c "import json,sys; j=json.load(open(sys.argv[1])); sys.exit(0 if not j[\"proxies\"] else 1)" "$CN/custom-nodes.json"'
cp "$T/cfg.cn.bak" "$H/.cca/config.json"; touch "$T/away"; : > ssh.log
out=$(run cca node add "$VMLINK" 2>&1); rc=$?
ok "配置了路由器的机器也能加自定义节点（同步时写进路由器的配置文件，见 11b2c）；在外面时不碰路由器，说回家后下次启动时同步" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "加好了自定义节点「My Node」" && printf "%s" "$out" | grep -q "回到家里的网络后，下次启动 cca 时会自动同步" && ! grep -q "^SSH " ssh.log'
cp "$T/cfg.cn.bak" "$H/.cca/config.json"; rm -f "$CN/custom-nodes.json" "$T/away"
rm -f "$CN"/mihomo-v*; : > gh.log
run cca @alice --version >/dev/null 2>&1
for i in $(seq 1 50); do [ -x "$CN/mihomo-v1.19.33" ] && [ ! -d "$H/.cca/core-nodes.lock" ] && break; sleep 0.1; done
ok "已经用内置代理、内核文件不在（比如 cca 升级换了内核版本）：在家启动后在后台先下好（在外面直连 GitHub 可能很慢）" '[ -x "$CN/mihomo-v1.19.33" ] && grep -q "^GH mihomo" gh.log'
# 配置本机出口的表单（local.builtin）：GUI 里问一句用不用内置代理
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); (c.get('local') or {}).pop('client', None); json.dump(c, open(p, 'w'), ensure_ascii=False)" "$H/.cca/config.json"   # 前面已把 local.client 设成 builtin；清掉它，cmd_local_setup 才会问 local.builtin
: > env.log
out=$(sess '{"local.builtin":[{"v":"y"}]}' -- cca local setup 2>&1); rc=$?
ok "提问会话：配置本机出口：GUI 问用不用内置代理，答是就配好（有结果事件）" '[ $rc -eq 0 ] && [ "$(asks)" = local.builtin ] && evq "e.get(\"q\") == \"local.builtin\" and e[\"fields\"][0][\"kind\"] == \"confirm\"" && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"local_ready\"" && [ -f "$CD/up" ]'
# 还原：配置、账号表、映射还原
core_stop; rm -rf "$GA" "$CN" "$H/.cca/policy"; mv "$T/policy.core" "$H/.cca/policy"
cp "$T/config.core" "$H/.cca/config.json"; cp "$T/accounts.core" "$H/.cca/accounts.json"

fi
if sect 11b4u; then
# 11b4u. 本机 UDP 走节点（cca local udp，只在 macOS 上、只给内置代理）：root 那边换进沙箱（HOME 换过时 udp_paths 在 $H/udp-root），假的
#        osascript 用当前用户跑管理员脚本（$T/osa_cancel：在授权框里点了取消），假的 launchctl 照 PathState 起停 cca-udp（$T/launchd），
#        假内核看到 TUN 的配置时状态放在 $T/core_udp（$T/core_udp/reject_text 里的文字出现在配置里就校验不过）。节点定义直接写
#        core/nodes.json，STUN 用假的服务器。这一节结束时卸掉系统服务，配置、账号表、内核目录都还原
mkver "$(ver)" "$H/.cca/bin/cca"   # 分节续跑时快照里装着的 cca 可能比仓库里的旧
CD="$T/core"; CN="$H/.cca/core"; CU="$T/core_udp"; UR="$H/udp-root"; UF="$H/.cca/udp/on"
ru() { run CCA_UDP_SETTLE=0 "$@"; }
uup() { local i; for i in $(seq 1 60); do [ -f "$CU/up" ] && return 0; sleep 0.1; done; return 1; }
udown() { local i; for i in $(seq 1 60); do [ -f "$CU/up" ] || return 0; sleep 0.1; done; return 1; }
ucfg() { python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)' "$UR/cca-udp/config.yaml" "$1"; }   # 装着的 TUN 配置（c）满足条件
admins() { local n; n=$(grep -c '^OSA-ADMIN' "$T/osascript.log" 2>/dev/null); echo "${n:-0}"; }   # 弹过几次授权框
utmp() { sed -n 's/^S=//p' "$T/osascript.log" | tail -n 1 | tr -d "'"; }   # 最近一次管理员脚本拷文件的临时目录
ustate() { gj state | jok "d[\"config\"][\"local\"][\"udp\"] == $1"; }
core_stop() { [ -f "$CD/up" ] && kill "$(cat "$CD/pid")" 2>/dev/null; local i; for i in $(seq 1 30); do [ -f "$CD/up" ] || return 0; sleep 0.1; done; }
cp "$H/.cca/config.json" "$T/config.udp"; cp "$H/.cca/accounts.json" "$T/accounts.udp"
rm -rf "$T/policy.udp"; mv "$H/.cca/policy" "$T/policy.udp"; mkdir -p "$H/.cca/policy"   # 不用映射（它固定的节点会盖掉下面给 alice 设的）
edit_table 'j["alice@example.com"] = "US-A"'
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['local'] = {}; c.pop('policy', None); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
mkdir -p "$CN"; chmod 700 "$CN"
python3 - "$CN/nodes.json" "$H/.cca/config.json" <<'P'
import json, sys, time
nodes = json.load(open(sys.argv[2]))['nodes']
px = {'US-A': {'type': 'vless', 'server': '192.0.2.21', 'port': 443, 'uuid': 'udp-secret-a', 'udp': True, 'tls': True},
      'US-B': {'type': 'trojan', 'server': '192.0.2.22', 'port': 443, 'password': 'udp-secret-b'},
      'US-C': {'type': 'ss', 'server': '192.0.2.23', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'udp-secret-c', 'udp': True}}
out = [dict(px.get(n) or {'type': 'ss', 'server': '192.0.2.29', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'x', 'udp': True}, name=n) for n in nodes]
json.dump({'version': 1, 'saved_at': time.time(), 'nodes': nodes, 'proxies': out}, open(sys.argv[1], 'w'))
P
chmod 600 "$CN/nodes.json"
: > "$T/osascript.log"; : > "$T/launchctl.log"; : > env.log
out=$(ru cca local udp on US-A 2>&1 </dev/null); rc=$?
ok "cca local udp on：还没用内置代理时不装（只给内置代理用），不弹授权框" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "只给内置代理用" && [ "$(admins)" = 0 ] && [ ! -e "$UR" ]'
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['local'] = {'client': 'builtin'}; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
out=$(ru cca local udp 2>&1 </dev/null)
ok "cca local udp：没开时说这台 Mac 的 UDP 直连、怎么打开；STUN 没测到也照实说" 'printf "%s" "$out" | grep -q "没开：这台 Mac 的 UDP" && printf "%s" "$out" | grep -q "cca local udp on \[节点\]" && printf "%s" "$out" | grep -q "STUN 没测到"'
ok "_gui state：config.local.udp 是没开、没装" 'ustate "{\"node\": None, \"state\": \"off\", \"installed\": False, \"running\": False, \"other_vpn\": None, \"paused\": False}"'
out=$(ru cca local udp on US-B 2>&1 </dev/null); rc=$?
ok "cca local udp on 不转发 UDP 的节点：说清楚；非交互时不现选、不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "节点「US-B」不转发 UDP" && printf "%s" "$out" | grep -q "运行 cca local udp on 节点" && [ "$(admins)" = 0 ] && cfgok "not c[\"local\"].get(\"udp\")"'
out=$(ru cca local udp on NOPE 2>&1 </dev/null); rc=$?
ok "cca local udp on 认不出的节点：说内置代理里没有它的定义；非交互时不现选、不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "内置代理里没有节点「NOPE」的定义" && [ "$(admins)" = 0 ] && [ ! -e "$UR" ]'
rm -f "$T/stun.port"; python3 -I "$T/fake_stun.py" "$T" & STUN_PID=$!
for i in $(seq 1 50); do [ -s "$T/stun.port" ] && break; sleep 0.1; done; STUN="127.0.0.1:$(cat "$T/stun.port")"
echo 198.51.100.21 > "$T/stun_ip"
: > gh.log
out=$(ru CCA_TTY="$(ans 1)" cca local udp on 2>&1); rc=$?
ok "cca local udp on（终端里现选，只列转发 UDP 的节点）：弹一次授权框装上系统服务，打开开关，root 的内核起来，说 STUN 看到的出口" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "1. US-A" && ! printf "%s" "$out" | grep -q "US-B" && printf "%s" "$out" | grep -q "现在经节点 US-A 出去" && printf "%s" "$out" | grep -q "从 198.51.100.21" && [ "$(admins)" = 1 ] && uup && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && [ -f "$UF" ] && [ "$(stat -f %Lp "$H/.cca/udp")" = 700 ] && grep -q "^GH mihomo" gh.log'
ok "装到 root 那边的：内核（和内置代理的同一份）、配置（600，只有这个节点，只把 UDP 交给它）、启动脚本（755）、info.json（644，没有密钥）、日志；launchd 配置照开关文件起停" '[ -x "$UR/cca-udp/mihomo" ] && cmp -s "$UR/cca-udp/mihomo" "$CN/mihomo-v1.19.33" && [ "$(stat -f %Lp "$UR/cca-udp")" = 755 ] && [ "$(stat -f %Lp "$UR/cca-udp/config.yaml")" = 600 ] && [ "$(stat -f %Lp "$UR/cca-udp/cca-udp")" = 755 ] && [ "$(stat -f %Lp "$UR/cca-udp/info.json")" = 644 ] && [ -f "$UR/cca-udp.log" ] && ucfg "[p[\"name\"] for p in c[\"proxies\"]] == [\"US-A\"] and c[\"rules\"][0] == \"NETWORK,TCP,DIRECT\" and c[\"rules\"][-2:] == [\"NETWORK,UDP,US-A\", \"MATCH,DIRECT\"] and c[\"tun\"][\"enable\"] and \"external-controller\" not in c" && ! grep -q udp-secret "$UR/cca-udp/info.json" "$UR/local.cca.udp.plist" && python3 -c "import plistlib,sys; p=plistlib.load(open(sys.argv[1],\"rb\")); sys.exit(0 if p[\"KeepAlive\"] == {\"PathState\": {sys.argv[2]: True}} and p[\"ProgramArguments\"] == [sys.argv[3] + \"/cca-udp\"] else 1)" "$UR/local.cca.udp.plist" "$UF" "$UR/cca-udp"'
ok "节点密钥不进命令行和日志（管理员脚本是 osascript 的参数，launchctl、env 的命令行）；临时目录用完删掉" '! grep -q udp-secret "$T/osascript.log" "$T/launchctl.log" env.log && tmpd=$(utmp) && [ -n "$tmpd" ] && [ ! -e "$tmpd" ] && grep -q "^SUMS=.*mihomo:[0-9a-f]\{64\} config.yaml:[0-9a-f]\{64\}" "$T/osascript.log"'
ok "launchd：bootstrap 装上，开关文件在就起 cca-udp" 'grep -q "^LAUNCHCTL bootstrap system $UR/local.cca.udp.plist$" "$T/launchctl.log" && grep -q "^START local.cca.udp$" "$T/launchctl.log"'
ok "_gui state：config.local.udp 是开着、装好了、在运行" 'ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": True, \"other_vpn\": None, \"paused\": False}"'
out=$(ru cca local udp 2>&1 </dev/null)
ok "cca local udp：说经哪个节点出去、系统服务在运行，和 STUN 看到的出口" 'printf "%s" "$out" | grep -q "经节点 US-A 出去（系统服务 local.cca.udp 在运行" && printf "%s" "$out" | grep -q "从 198.51.100.21"'
# 别的 VPN 的 TUN 接管了流量（像 Shadowrocket：经 utun5 的默认路由、几大段、198.18/15）：root 那边的启动脚本停下内核让开，网卡名写进
# other-vpn，它的路由没了再起。排在前面的几行不算：经自己的 TUN（utun233，劈细的段）的、限定网卡的（I）、ZeroTier 的虚拟网卡（feth）
printf '%s\n' 'Routing tables' '' 'Internet:' 'Destination        Gateway            Flags               Netif Expire' \
	'1.0/9              198.19.233.233     UGSc              utun233       ' \
	'default            link#31            UCSIg               utun7       ' \
	'0/1                192.0.2.40         UGSc                feth3       ' \
	'default            192.0.2.1          UGScg                 en0       ' \
	'default            link#27            UCSg                utun5       ' \
	'1                  link#27            UCS                 utun5       ' \
	'128.0/1            link#27            UCS                 utun5       ' \
	'198.18/15          link#27            UCS                 utun5       ' > "$T/netstat.txt"
uaway() { local i; for i in $(seq 1 60); do [ -s "$UR/cca-udp/other-vpn" ] && return 0; sleep 0.1; done; return 1; }
ok "别的 VPN 接管了流量（经 utun5 的默认路由）：root 那边停下内核让开、记下网卡（自己的 TUN、限定网卡的、ZeroTier 的路由不算），日志里说了" 'uaway && udown && [ "$(cat "$UR/cca-udp/other-vpn")" = utun5 ] && grep -q "另一个 VPN（utun5）接管了这台 Mac 的流量" "$UR/cca-udp.log"'
ok "_gui state：让开时 running 是 false、other_vpn 是那块网卡" 'ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": False, \"other_vpn\": \"utun5\", \"paused\": False}"'
out=$(ru cca local udp 2>&1 </dev/null)
ok "cca local udp：说让给了另一个 VPN、它关掉后自动接回（不说系统服务没在运行）" 'printf "%s" "$out" | grep -q "虚拟网卡（utun5；" && printf "%s" "$out" | grep -q "它关掉后自动接回节点 US-A" && ! printf "%s" "$out" | grep -q "却没在运行"'
out=$(ru cca local udp on US-A 2>&1 </dev/null); rc=$?
ok "让开时再 cca local udp on：不弹授权框、不算失败，说先让给了另一个 VPN" '[ $rc -eq 0 ] && [ "$(admins)" = 1 ] && printf "%s" "$out" | grep -q "先让给它" && ! printf "%s" "$out" | grep -q "却没起来"'
rm -f "$T/netstat.txt"
ok "别的 VPN 关掉了（路由没了）：root 那边接着起内核、删掉 other-vpn，_gui state 回到在运行" 'uup && [ ! -e "$UR/cca-udp/other-vpn" ] && grep -q "另一个 VPN 关掉了" "$UR/cca-udp.log" && ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": True, \"other_vpn\": None, \"paused\": False}"'
out=$(ru cca local udp on US-A 2>&1 </dev/null); rc=$?
ok "再开同一个节点（装着的就是这一份）：不再弹授权框" '[ $rc -eq 0 ] && [ "$(admins)" = 1 ] && [ -f "$CU/up" ]'
out=$(ru cca local udp off 2>&1 </dev/null); rc=$?
ok "cca local udp off：删掉开关文件，root 的内核自己停下（等到它停下，不白等、不说没停下）；系统服务留着（不用密码）" '[ $rc -eq 0 ] && udown && [ ! -e "$UF" ] && cfgok "\"udp\" not in c[\"local\"]" && [ "$(admins)" = 1 ] && [ -f "$UR/local.cca.udp.plist" ] && printf "%s" "$out" | grep -q "系统服务还装着" && ! printf "%s" "$out" | grep -q "还没停下"'
ok "_gui state：关掉以后是 off（装着、没在运行）" 'ustate "{\"node\": None, \"state\": \"off\", \"installed\": True, \"running\": False, \"other_vpn\": None, \"paused\": False}"'
out=$(ru cca local udp on 2>&1 </dev/null); rc=$?
ok "cca local udp on 不写节点：用装着的那个（不弹授权框），又起来了" '[ $rc -eq 0 ] && uup && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && [ "$(admins)" = 1 ]'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [x.update(server='192.0.2.31') for x in j['proxies'] if x['name'] == 'US-A']; json.dump(j, open(p, 'w'))" "$CN/nodes.json"
ok "节点定义变了：_gui state 说装着的配置旧了（还在运行）" 'ustate "{\"node\": \"US-A\", \"state\": \"stale\", \"installed\": True, \"running\": True, \"other_vpn\": None, \"paused\": False}"'
mkdir -p "$GA/Contents/MacOS"
printf '<plist><dict><key>CFBundleIdentifier</key><string>local.cca.app</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>' "$(ver)" > "$GA/Contents/Info.plist"
touch "$T/away"
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面启动账号：装着的旧了只提醒（启动时不弹授权框），照常经内置代理启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "本机 UDP 走节点（US-A）要重新装一次" && [ "$(admins)" = 1 ] && [ "$(cat "$CD/sel/CCA-1" 2>/dev/null)" = US-A ]'
out=$(ru cca local udp on 2>&1 </dev/null); rc=$?
ok "cca local udp on：照新的节点定义重新装（再弹一次授权框），root 的内核换上新的配置" '[ $rc -eq 0 ] && [ "$(admins)" = 2 ] && ucfg "c[\"proxies\"][0][\"server\"] == \"192.0.2.31\"" && uup && ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": True, \"other_vpn\": None, \"paused\": False}"'
touch "$T/osa_cancel"
out=$(ru cca local udp on US-C 2>&1 </dev/null); rc=$?
rm -f "$T/osa_cancel"
ok "授权框里点了取消：什么都不改（还是经 US-A，装着的也还是它，照跑）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "已取消" && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && ucfg "c[\"rules\"][-2] == \"NETWORK,UDP,US-A\"" && [ -f "$CU/up" ]'
echo 192.0.2.23 > "$CU/reject_text"
out=$(ru cca local udp on US-C 2>&1 </dev/null); rc=$?
rm -f "$CU/reject_text"
ok "内核校验 root 那边的新配置没通过：不换上，原来的照跑，说原因" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "内核校验配置没通过" && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && ucfg "c[\"rules\"][-2] == \"NETWORK,UDP,US-A\"" && [ ! -e "$UR/cca-udp.new" ] && [ -f "$CU/up" ] && run launchctl print system/local.cca.udp >/dev/null 2>&1'
out=$(ru cca local udp on US-C 2>&1 </dev/null); rc=$?
ok "换成 US-C：重新装，root 的内核换上只有 US-C 的配置" '[ $rc -eq 0 ] && cfgok "c[\"local\"][\"udp\"] == \"US-C\"" && ucfg "[p[\"name\"] for p in c[\"proxies\"]] == [\"US-C\"]" && uup'
rm -f "$UF"; udown
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面启动账号：设了本机 UDP 走节点、开关文件却不在：补上，root 的内核又起来" '[ $rc -eq 3 ] && [ -f "$UF" ] && uup'
out=$(sess '{"local.udp":[{"v":"1"}]}' -- cca local udp pick 2>&1); rc=$?
ok "提问会话：换节点（cca local udp pick 总是问；App 里现选，不列不转发 UDP 的），选好照常装上、打开，有结果事件" '[ $rc -eq 0 ] && [ "$(asks)" = local.udp ] && evq "e.get(\"q\") == \"local.udp\" and e[\"fields\"][0][\"options\"][0][\"label\"] == \"US-A\" and not any(o[\"label\"] == \"US-B\" for o in e[\"fields\"][0][\"options\"])" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"local_udp\"" && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && uup'
# 在家自动暂停：假 netstat 说物理网卡是 lo0（绑定它也问得到本机回环上的假 STUN），STUN 看到的地址归属地默认是美国
printf 'Routing tables\n\nInternet:\nDestination        Gateway            Flags               Netif Expire\ndefault            127.0.0.1          UGScg                 lo0       \n' > "$T/netstat.txt"
rm -f "$T/away"
out=$(run cca @alice --version 2>&1); rc=$?
ok "在家（经路由器）启动账号：开着本机 UDP 走节点时绕开它的 TUN、绑定物理网卡问 STUN，路由器已经让 UDP 出了大陆：删掉开关文件（root 的内核停下），设的节点留着，说一声" '[ $rc -eq 3 ] && udown && [ ! -e "$UF" ] && [ -e "$H/.cca/udp/home" ] && cfgok "c[\"local\"][\"udp\"] == \"US-A\"" && printf "%s" "$out" | grep -q "在家：路由器已经让这台 Mac 的 UDP 从 198.51.100.21（United States" && printf "%s" "$out" | grep -q "本机 UDP 走节点先停着（出门在外时自动再打开）"'
ok "_gui state：在家暂停着（ok、没在运行、paused）" 'ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": False, \"other_vpn\": None, \"paused\": True}"'
out=$(ru cca local udp 2>&1 </dev/null)
ok "cca local udp：在家暂停着照实说（出门在外自动再打开），不说系统服务没在运行" 'printf "%s" "$out" | grep -q "在家暂停着：路由器已经让这台 Mac 的 UDP 经节点出去，本机 UDP 走节点（US-A）先停着" && ! printf "%s" "$out" | grep -q "却没在运行"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "在家暂停着再启动（WebRTC 正常）：不打开、不再说" '[ $rc -eq 3 ] && [ ! -e "$UF" ] && [ -e "$H/.cca/udp/home" ] && ! printf "%s" "$out" | grep -q "本机 UDP"'
mkdir -p "$T/geo"
printf '{"country": "China", "region": "", "city": "", "isp": "Example CN", "country_code": "cn"}' > "$T/geo/198.51.100.21.json"
rm -f "$H/.cca/leak.json"   # 归属地按网段缓存 7 天（leak.json）：换了假的归属地要清掉，不然还是上次查到的美国
out=$(run cca @alice --version 2>&1); rc=$?
ok "在家暂停着、WebRTC 检测却看到中国大陆的地址（路由器不再让 UDP 出大陆）：本机 UDP 走节点重新打开" '[ $rc -eq 3 ] && [ -f "$UF" ] && [ ! -e "$H/.cca/udp/home" ] && uup && printf "%s" "$out" | grep -q "本机 UDP 走节点（US-A）重新打开"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "在家、绑定物理网卡看到的是中国大陆的地址（路由器不管 UDP）：开着的不关" '[ $rc -eq 3 ] && [ -f "$UF" ] && [ ! -e "$H/.cca/udp/home" ] && ! printf "%s" "$out" | grep -q "先停着"'
rm -f "$T/geo/198.51.100.21.json" "$H/.cca/leak.json"
out=$(run cca check alice 2>&1)
ok "在家 cca check（App 定时检测出口）同样暂停" '[ ! -e "$UF" ] && [ -e "$H/.cca/udp/home" ] && udown && printf "%s" "$out" | grep -q "本机 UDP 走节点先停着"'
touch "$T/away"
out=$(run cca check alice 2>&1)
ok "出门在外检测出口（cca check）：在家暂停过的自动再打开，说一声" '[ -f "$UF" ] && [ ! -e "$H/.cca/udp/home" ] && uup && printf "%s" "$out" | grep -q "出门在外：本机 UDP 重新经节点 US-A 出去"'
ok "_gui state：又开着、在运行（paused 是 false）" 'ustate "{\"node\": \"US-A\", \"state\": \"ok\", \"installed\": True, \"running\": True, \"other_vpn\": None, \"paused\": False}"'
rm -f "$T/netstat.txt"; rmdir "$T/geo" 2>/dev/null
out=$(ru cca local off 2>&1 </dev/null); rc=$?
ok "cca local off（不再用内置代理）：本机 UDP 走节点一起关掉（开关文件删掉，root 的内核停下），系统服务留着" '[ $rc -eq 0 ] && udown && [ ! -e "$UF" ] && printf "%s" "$out" | grep -q "本机 UDP 走节点也关掉了" && cfgok "\"udp\" not in c.get(\"local\", {})" && [ -f "$UR/local.cca.udp.plist" ]'
n=$(admins)
out=$(ru cca local udp uninstall 2>&1 </dev/null); rc=$?
ok "cca local udp uninstall：弹一次授权框，bootout、删掉 root 那边的文件、launchd 配置和日志" '[ $rc -eq 0 ] && [ "$(admins)" = $((n + 1)) ] && [ ! -e "$UR/cca-udp" ] && [ ! -e "$UR/local.cca.udp.plist" ] && [ ! -e "$UR/cca-udp.log" ] && grep -q "^LAUNCHCTL bootout system/local.cca.udp$" "$T/launchctl.log" && [ ! -e "$T/launchd/local.cca.udp.plist" ]'
out=$(ru cca local udp uninstall 2>&1 </dev/null); rc=$?
ok "没装时 uninstall：说没装，不弹授权框" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "系统服务没装" && [ "$(admins)" = $((n + 1)) ]'
kill "$STUN_PID" 2>/dev/null; wait "$STUN_PID" 2>/dev/null; STUN="" STUN_PID=""
core_stop; rm -f "$T/away" "$T/stun_ip"; rm -rf "$GA" "$CN" "$UR" "$H/.cca/udp" "$T/launchd" "$CU"
rm -rf "$H/.cca/policy"; mv "$T/policy.udp" "$H/.cca/policy"
cp "$T/config.udp" "$H/.cca/config.json"; cp "$T/accounts.udp" "$H/.cca/accounts.json"

fi
if sect 11b5; then
# 11b5. 辅助安装（cca [codex|agy] install-cli；终端里用到没装的命令时先问）：官方安装脚本（假 curl 给 $T/fake_installer.sh）先直连，
#       不行时问过再经内置代理的一个节点装：另起的临时内核（假 mihomo，状态在 $T/core_install），节点由维护者映射的 install_node
#       指定，没指定时现选。节点定义是内置代理的那些（这里直接写 core/nodes.json）。这一节结束时映射、配置、命令都还原
IL="$T/install.log"; ID="$T/core_install"; TI="$T/tmpi"; mkdir -p "$TI"; : > "$IL"
NP="$T/bin:$H/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"   # 不带 /opt/homebrew/bin（那里可能装着真的 codex）
ri() { run PATH="$NP" TMPDIR="$TI" "$@"; }
toff() { mv "$T/bin/$1" "$T/$1.off"; }   # 模拟没装这个命令（假的安装脚本装上时链接到 $T/<命令>.off）
tback() { rm -f "$H/.local/bin/$1"; mv "$T/$1.off" "$T/bin/$1"; }
ileft() { ls "$TI" | grep -c '^cca-cli\.'; }   # 留下的临时目录（应该都删了）
icfg() { python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)' "$ID/last_config.json" "$1"; }   # 临时内核收到的配置（c）
istopped() { [ ! -f "$ID/up" ] && ! kill -0 "$(cat "$ID/pid")" 2>/dev/null && [ ! -e "$(cat "$ID/home")" ]; }   # 临时内核停了，它的目录（有节点密钥）删了
ipol() {   # ipol 节点：映射照原来的，加上 install_node（空的就不加），签名后拉下来
	pol "$(python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j.update({"install_node": sys.argv[2]} if sys.argv[2] else {}); print(json.dumps(j, ensure_ascii=False))' "$T/pr.inst" "$1")"
	run cca policy pull >/dev/null 2>&1
}
cp "$PR/policy.json" "$T/pr.inst"; rm -f "$T/pr.inst.sig"; [ ! -f "$PR/policy.json.sig" ] || cp "$PR/policy.json.sig" "$T/pr.inst.sig"
rm -rf "$T/policy.inst"; cp -Rp "$H/.cca/policy" "$T/policy.inst"; cp "$H/.cca/config.json" "$T/config.inst"; cp "$H/.cca/accounts.json" "$T/accounts.inst"
ok "_gui hello：说明有辅助安装（App 据此在“没装 X 命令”的地方给「安装」，交给终端运行 cca install-cli）" 'gj hello | jok "d[\"install_cli\"] == 1"'
out=$(run cca help 2>&1)
ok "cca help 里有 cca install-cli" 'printf "%s" "$out" | grep -q "cca install-cli  *用官方安装脚本装 Claude Code"'
out=$(ri cca install-cli 2>&1); rc=$?
ok "cca install-cli：已经装了就只说一声，不下载安装脚本" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qF "已经装了 claude：$T/bin/claude" && [ ! -s "$IL" ]'
# 直连：在空环境里运行（不带账号、代理设置和 API key；claude 用一次性的配置目录），PATH 里有 ~/.local/bin（安装脚本不去改 shell 的配置文件）
toff claude
out=$(ri ANTHROPIC_API_KEY=sk-INSTALL-LEAK-1 CLAUDE_CODE_USE_BEDROCK=1 HTTPS_PROXY=http://127.0.0.1:9 cca install-cli 2>&1); rc=$?
ok "cca install-cli（没装 claude）：直连下载官方安装脚本、运行，装进 ~/.local/bin 后说一声" '[ $rc -eq 0 ] && grep -qx "INSTALL-GET claude direct" "$IL" && grep -qx "INSTALL-DONE claude via=direct" "$IL" && printf "%s" "$out" | grep -qF "装好了 claude：$H/.local/bin/claude" && [ "$(readlink "$H/.local/bin/claude")" = "$T/claude.off" ]'
ok "安装脚本在空环境里运行：bash，不带代理设置、API key 和 cca 自己的变量；临时目录在 TMPDIR 里，claude 用里面一次性的配置目录；用完删掉，不起临时内核" 'grep -q "^INSTALL-RUN claude shell=/bin/bash proxy= tmp=$TI/cca-cli\.[^ /]* cfg=$TI/cca-cli\.[^ /]*/claude nonint= path0=$H/.local/bin leak=\[\]$" "$IL" && [ "$(ileft)" = 0 ] && [ ! -e "$ID/pid" ] && ! grep -q sk-INSTALL-LEAK "$IL" env.log'
tback claude
toff codex; : > "$IL"
if [ -x /opt/homebrew/bin/codex ] || [ -x /usr/local/bin/codex ]; then echo "SKIP  cca codex install-cli（这台机器的 /opt/homebrew/bin、/usr/local/bin 里装着 codex）"
else
	out=$(ri cca codex install-cli 2>&1); rc=$?
	ok "cca codex install-cli：codex 的安装脚本用 sh 运行、不问问题（CODEX_NON_INTERACTIVE）；只装命令不建 Codex 的账号表" '[ $rc -eq 0 ] && grep -q "^INSTALL-RUN codex shell=/bin/sh proxy= tmp=$TI/cca-cli\.[^ /]* cfg= nonint=1 path0=$H/.local/bin leak=\[\]$" "$IL" && printf "%s" "$out" | grep -qF "装好了 codex：$H/.local/bin/codex"'
fi
tback codex
# 直连拿不到安装脚本：经内置代理的节点（节点定义：路由器上拷来的 US-A～D、HK-A2）
mkdir -p "$CN"; chmod 700 "$CN"
python3 -c 'import json,sys,time; json.dump({"version": 1, "saved_at": time.time(), "nodes": [], "proxies": [{"name": n, "type": "ss", "server": "192.0.2.%d" % (51 + i), "port": 8388, "cipher": "aes-128-gcm", "password": "x"} for i, n in enumerate(["US-A", "US-B", "US-C", "US-D", "HK-A2"])]}, open(sys.argv[1], "w"))' "$CN/nodes.json"
chmod 600 "$CN/nodes.json"
toff agy; touch "$T/install_direct_down"; : > "$IL"; : > gh.log
out=$(ri cca agy install-cli 2>&1); rc=$?
ok "直连拿不到安装脚本、映射没指定安装节点、非交互：说明要在终端里选一个，不起临时内核、不装；不建 agy 的账号表" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "直连拿不到安装脚本" && printf "%s" "$out" | grep -q "维护者映射没指定安装用的节点，要在终端里选一个：运行 cca agy install-cli" && [ ! -e "$ID/pid" ] && [ ! -e "$H/.local/bin/agy" ] && [ ! -e "$H/.cca/agy.json" ]'
out=$(ri CCA_TTY="$(ans 2)" cca agy install-cli 2>&1); rc=$?
ok "终端里现选一个节点（选了就是同意，不记下）：经它的临时代理拿到安装脚本、运行（安装脚本自己下载也经它）；内核文件第一次用时下载" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "经内置代理的哪个节点安装 agy？" && grep -qx "INSTALL-GET agy via US-B" "$IL" && grep -qx "INSTALL-DONE agy via=US-B" "$IL" && grep -q "^INSTALL-RUN agy shell=/bin/bash proxy=http://127.0.0.1:[0-9]* .* leak=\[\]$" "$IL" && printf "%s" "$out" | grep -qF "装好了 agy：$H/.local/bin/agy" && grep -q "^GH mihomo" gh.log && [ ! -e "$H/.cca/agy.json" ]'
ok "临时内核：只在 127.0.0.1 上开一个入站（直通只有这个节点的组），规则只有 MATCH,REJECT，不开控制接口、通用的代理端口、TUN、DNS；装完就停，它的目录（有节点密钥）和临时目录都删掉" 'icfg "[l[\"listen\"] for l in c[\"listeners\"]] == [\"127.0.0.1\"] and c[\"listeners\"][0][\"proxy\"] == \"CCA-INSTALL\" and c[\"proxy-groups\"] == [{\"name\": \"CCA-INSTALL\", \"type\": \"select\", \"proxies\": [\"US-B\"]}] and [p[\"name\"] for p in c[\"proxies\"]] == [\"US-B\"] and c[\"rules\"] == [\"MATCH,REJECT\"] and not any(k in c for k in (\"port\", \"mixed-port\", \"socks-port\", \"tun\", \"dns\", \"external-controller\", \"external-controller-unix\"))" && istopped && [ "$(ileft)" = 0 ]'
rm -f "$H/.local/bin/agy"; touch "$T/tun"; : > "$IL"
out=$(ri CCA_TTY="$(ans 2)" cca agy install-cli 2>&1); rc=$?
ok "开着 TUN（去公网的路由走 utun9）：临时内核和内置代理一样把出站绑定到物理网卡 en0，照常装好" '[ $rc -eq 0 ] && grep -qx "INSTALL-DONE agy via=US-B" "$IL" && icfg "c[\"interface-name\"] == \"en0\" and \"dns\" not in c" && istopped'
rm -f "$T/tun" "$H/.local/bin/agy"
# 维护者在映射里指定安装用的节点（install_node）：路由器上改过名的（cca 记下的 node_renames）按现在的名字
ipol US-OLD
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['node_renames'] = {'US-OLD': 'US-B'}; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
out=$(run cca policy 2>&1)
ok "映射里写了安装用的节点（install_node，旧版本的 cca 不认识它、照样用这份映射）：cca policy 说明；路由器上改过名的按现在的名字" 'grep -q "\"install_node\": \"US-OLD\"" "$H/.cca/policy/policy.json" && printf "%s" "$out" | grep -q "安装用的节点：US-B（直连装不上 claude / codex / agy 命令时，问过再经内置代理走它）"'
: > "$IL"
out=$(ri CCA_TTY="$(ans y)" cca agy install-cli 2>&1); rc=$?
ok "映射指定了安装节点：直连拿不到时问过（默认是）再经它的临时代理装上" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "直连拿不到安装脚本。经内置代理的节点 US-B 安装 agy 吗？" && grep -qx "INSTALL-GET agy via US-B" "$IL" && grep -qx "INSTALL-DONE agy via=US-B" "$IL" && istopped && [ "$(ileft)" = 0 ]'
rm -f "$H/.local/bin/agy" "$ID/pid"; : > "$IL"
out=$(ri CCA_TTY="$(ans n)" cca agy install-cli 2>&1); rc=$?
ok "不同意：不起临时内核，什么都不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有安装 agy（以后可以运行 cca agy install-cli）" && [ ! -e "$ID/pid" ] && ! grep -q " via" "$IL" && [ ! -e "$H/.local/bin/agy" ]'
out=$(runev PATH="$NP" TMPDIR="$TI" cca agy install-cli 2>&1); rc=$?
ok "非交互（界面 App）：记一条 consent（install_proxy，范围是节点），不装" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"install_proxy\" and e[\"grant\"] == \"install_proxy:US-B\" and e[\"default\"] == \"yes\"" && [ ! -e "$ID/pid" ] && [ ! -e "$H/.local/bin/agy" ]'
out=$(runev PATH="$NP" TMPDIR="$TI" CCA_GRANT=install_proxy:US-B cca agy install-cli 2>&1); rc=$?
ok "同意之后带着授权重跑：经 US-B 装上" '[ $rc -eq 0 ] && [ -L "$H/.local/bin/agy" ] && istopped'
rm -f "$H/.local/bin/agy"
ipol US-Z; : > "$IL"
out=$(ri CCA_TTY="$(ans 3)" cca agy install-cli 2>&1); rc=$?
out2=$(run cca policy 2>&1)
ok "映射指定的安装节点这台机器上没有定义：说一声，在终端里现选一个" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "维护者映射指定的安装节点 US-Z 在这台机器上没有定义" && grep -qx "INSTALL-GET agy via US-C" "$IL" && printf "%s" "$out2" | grep -q "安装用的节点：US-Z（这台机器的内置代理里没有它的定义：要装的时候现选一个）"'
rm -f "$H/.local/bin/agy"; ipol US-B
# 拿得到安装脚本、它自己直连下载不了（实际最常见）：直连没装成，问过经 US-B 再装一次
rm -f "$T/install_direct_down"; touch "$T/install_dl_down"; : > "$IL"
out=$(ri CCA_TTY="$(ans y)" cca agy install-cli 2>&1); rc=$?
rm -f "$T/install_dl_down"
ok "直连拿到了安装脚本、它自己下载不了：直连没装成，问过经 US-B 再装一次，装上" '[ $rc -eq 0 ] && grep -qx "INSTALL-GET agy direct" "$IL" && printf "%s" "$out" | grep -q "直连没装成。经内置代理的节点 US-B 安装 agy 吗？" && grep -qx "INSTALL-DONE agy via=US-B" "$IL" && [ -L "$H/.local/bin/agy" ] && istopped && [ "$(ileft)" = 0 ]'
rm -f "$H/.local/bin/agy"; touch "$T/install_fail"; : > "$IL"
out=$(ri CCA_TTY="$(ans y)" cca agy install-cli 2>&1); rc=$?
rm -f "$T/install_fail"
ok "安装脚本失败：经 US-B 再试也失败就照实说（安装脚本的输出照样给用户看）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "经 US-B 也没装成" && printf "%s" "$out" | grep -q "Installation failed (fake)" && [ "$(grep -c "^INSTALL-RUN agy" "$IL")" = 2 ] && istopped && [ "$(ileft)" = 0 ] && [ ! -e "$H/.local/bin/agy" ]'
touch "$T/install_html"; : > "$IL"
out=$(ri CCA_TTY="$(ans n)" cca agy install-cli 2>&1); rc=$?
rm -f "$T/install_html"
ok "拿到的不是脚本（比如出错的网页）：算没拿到，不运行" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "直连拿不到安装脚本" && ! grep -q "^INSTALL-RUN" "$IL"'
touch "$T/install_noop"; : > "$IL"
out=$(ri CCA_TTY="$(ans y)" cca agy install-cli 2>&1); rc=$?
rm -f "$T/install_noop"
ok "安装脚本跑完了（退出码 0）却没找到命令：照实说，不再经内置代理试" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "安装脚本跑完了，却没找到 agy 命令" && [ "$(grep -c "^INSTALL-RUN agy" "$IL")" = 1 ] && ! printf "%s" "$out" | grep -q "经内置代理的节点"'
touch "$T/install_direct_down" "$ID/start_fail"; : > "$IL"
out=$(ri CCA_TTY="$(ans y)" cca agy install-cli 2>&1); rc=$?
rm -f "$ID/start_fail"
ok "临时内核起不来：照实说（带内核最后一行输出），不装，临时目录删掉" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "经 US-B 的临时代理没起来：.*Parse config error: fake" && ! grep -q " via" "$IL" && [ "$(ileft)" = 0 ] && [ ! -e "$H/.local/bin/agy" ]'
tback agy; rm -f "$T/install_direct_down"
# 用到没装的命令：终端里先问要不要装，装上后接着做；非交互时说明怎么装
toff claude; : > "$IL"; : > claude.log
out=$(ri CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "终端里启动账号、没装 claude：先问要不要装，装上后照常启动（用的是刚装上的）" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "没找到 claude 命令。现在用官方安装脚本装上吗？" && grep -qx "INSTALL-DONE claude via=direct" "$IL" && [ -s claude.log ]'
rm -f "$H/.local/bin/claude"; : > "$IL"
out=$(runev PATH="$NP" cca @alice --version 2>&1); rc=$?
ok "非交互（启动器、界面 App）没装 claude：不问不装，说明用 cca install-cli 装（原因码 tool_missing）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "找不到 claude 命令：运行 cca install-cli 用官方安装脚本装上" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"tool_missing\"" && [ ! -s "$IL" ]'
tback claude
# 维护者发布前的严格检查认识 install_node
printf '{"version": 1, "claude": {"alice@example.com": "US-C"}, "install_node": "US-B"}\n' > "$T/inst.ok.json"
printf '{"version": 1, "claude": {"alice@example.com": "US-C"}, "install_node": ["US-B"]}\n' > "$T/inst.bad.json"
out=$(run cca _policy-check "$T/inst.bad.json" 2>&1); rc=$?
ok "维护者发布前的严格检查认识 install_node，也查它的格式" 'run cca _policy-check "$T/inst.ok.json" >/dev/null 2>&1 && [ $rc -ne 0 ] && printf "%s" "$out" | grep -q "install_node 应该是节点的名字"'
# 还原：映射、配置、账号表，内置代理的目录（11b4 结束时就删了）
cp "$T/pr.inst" "$PR/policy.json"; rm -f "$PR/policy.json.sig"; [ ! -f "$T/pr.inst.sig" ] || cp "$T/pr.inst.sig" "$PR/policy.json.sig"
rm -rf "$H/.cca/policy" "$CN" "$ID"; mv "$T/policy.inst" "$H/.cca/policy"
cp "$T/config.inst" "$H/.cca/config.json"; cp "$T/accounts.inst" "$H/.cca/accounts.json"
ok "辅助安装用完不留东西：没建 agy 的账号表，~/.local/bin 里只剩原来的" '[ ! -e "$H/.cca/agy.json" ] && [ ! -e "$H/.local/bin/agy" ] && [ ! -e "$H/.local/bin/claude" ] && [ ! -e "$H/.local/bin/codex" ]'

fi
if sect 11b6; then
# 11b6. 服务器上安装（CCA_REMOTE_LOGIN=1：Mac 上走服务器的流程）：从没配置过路由器、直连不通（假 curl：direct_down 连不上，direct_loc 是
#       直连的出口地区）时，cca install、cca add 不问路由器，先问有没有维护者共享给你的账号（默认没有）。有：给出维护者运行的一行命令
#       （公钥和别名写在里面），之后每隔几秒自动拉映射（回车马上拉，s 先跳过）；维护者加上以后导入映射指定的安装节点和映射里账号的节点
#       （参考节点），配置内置代理（假内核、假的界面 App），经它装 Claude Code（不再先直连、不另外问），映射里的账号自动加进来。没有：
#       粘贴一条自己的代理当可选节点，配置内置代理。直连通的照旧问路由器。这一节结束时映射仓库、假命令都还原
SH="$T/homes"; mkdir -p "$SH/Applications/cca.app/Contents/MacOS"
printf '<plist><dict><key>CFBundleIdentifier</key><string>local.cca.app</string><key>CFBundleShortVersionString</key><string>%s</string></dict></plist>' "$(ver)" > "$SH/Applications/cca.app/Contents/Info.plist"
rs() { env -i LANG=en_US.UTF-8 HOME="$SH" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_HELPER="$HELPERWRAP" CCA_REMOTE_LOGIN=1 CCA_POLICY_DEFAULT=example/cca-policy CCA_RETRY_WAIT=0 CCA_STUN=127.0.0.1:9 CCA_DOCK_PREFS="$T/dock/dock.plist" CCA_DOCK_SETTLE=0 TMPDIR="$TI" PATH="$T/bin:$SH/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
scfg() { python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)' "$SH/.cca/config.json" "$1"; }   # 服务器那边的 config.json（c）
spub() { cat "$SH/.cca/policy/id_ed25519.pub"; }
cp "$PR/policy.json" "$T/pr.srv"; rm -f "$T/pr.srv.sig"; [ ! -f "$PR/policy.json.sig" ] || cp "$PR/policy.json.sig" "$T/pr.srv.sig"
pol '{"version": 1, "claude": {"sam@example.com": "US-S"}, "aliases": {"sam@example.com": "sam"}, "install_node": "US-I"}'
python3 -c 'import json,sys; json.dump({"version": 1, "group": "OpenAi", "proxies": [{"name": n, "type": "ss", "server": "192.0.2.%d" % (81 + i), "port": 8388, "cipher": "aes-128-gcm", "password": "x"} for i, n in enumerate(["US-I", "US-S", "US-X"])]}, open(sys.argv[1], "w"))' "$PR/nodes.json"; nodes_pub
toff claude; touch "$T/policy_denied"; echo HK > "$T/direct_loc"; : > "$IL"; : > git.log; : > curl.log
out=$(rs CCA_TTY="$(ans y s)" "$H/.cca/bin/cca" install 2>&1); rc=$?
ok "服务器上直连的出口在受限地区：不问路由器，先问有没有维护者共享的账号（默认没有）；答有：给出维护者运行的那一行（公钥和别名写在里面）、等着拉映射；输入 s 先跳过，不装 Claude Code、不加账号，说明之后再运行 cca install" '[ $rc -eq 0 ] && grep -q "^CURL direct https://claude.ai/cdn-cgi/trace" curl.log && printf "%s" "$out" | grep -q "正在检测这台机器能不能直连 claude.ai" && printf "%s" "$out" | grep -q "直连 claude.ai 的出口在香港，不能用：这台机器不用配置路由器，改用 cca 内置的代理" && printf "%s" "$out" | grep -q "有没有维护者共享给你的账号？ \[y/N\]" && printf "%s" "$out" | grep -qF "    ./policy.sh key add '\''$(spub)'\'' $(cut -d@ -f2 "$SH/.cca/policy/id_ed25519.pub")" && printf "%s" "$out" | grep -q "等维护者运行上面那一行" && printf "%s" "$out" | grep -q "先跳过：维护者运行上面那一行之后，再运行 cca install 接着配置" && ! printf "%s" "$out" | grep -qE "现在配置路由器|没找到 Claude Code|新增第一个账号" && scfg "c.get(\"policy\") == \"example/cca-policy\" and not c.get(\"nodes\") and not c.get(\"router\")" && [ ! -f "$SH/.cca/policy/policy.json" ]'
rm -f "$T/direct_loc"; touch "$T/direct_down"; k0=$(spub)
out=$(rs CCA_TTY="$(ans s)" cca add 2>&1); rc=$?
ok "cca add（还没有可选节点）：直连不通时同样不先要求配置路由器，接着等维护者（还是这把公钥）；先跳过时说明再运行 cca add" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "直连 claude.ai 不通：这台机器不用配置路由器" && printf "%s" "$out" | grep -qF "$k0" && [ "$(spub)" = "$k0" ] && printf "%s" "$out" | grep -q "再运行 cca add 接着配置" && ! printf "%s" "$out" | grep -q "先配置路由器"'
# 维护者在 cca 的仓库里运行那一行（假 gh 存 deploy key；有 policy_keys 时假 git 照它认）
line=$(printf '%s\n' "$out" | sed -n 's/^    \(\.\/policy\.sh key add .*\)$/\1/p' | tail -n 1)
rm -f "$T/policy_denied"; touch "$T/policy_keys"; salias=$(cut -d@ -f2 "$SH/.cca/policy/id_ed25519.pub")
out=$(cd "$D/.." && run $P $NOPD CCA_TTY="$(ans)" bash -c "$line" 2>&1); rc=$?
ok "维护者原样运行那一行：公钥加成只读 deploy key，别名是那台机器的名字，不提问" '[ -n "$line" ] && [ $rc -eq 0 ] && [ "$(dkk "$salias")" = "$(cut -d" " -f1,2 "$SH/.cca/policy/id_ed25519.pub")" ] && ! printf "%s" "$out" | grep -q "起个别名"'
: > "$IL"; : > git.log
out=$(rs CCA_TTY="$(ans y)" cca install 2>&1); rc=$?
ok "维护者加上之后再运行 cca install：拉到映射，导入映射指定的安装节点和映射里账号的节点（参考节点里别的不导入），映射里的账号自动加进来" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "拉到了维护者的映射（Claude 1 个账号" && printf "%s" "$out" | grep -q "已导入维护者提供的节点：US-I、US-S（这台机器的可选节点）" && scfg "c[\"nodes\"] == [\"US-I\", \"US-S\"] and sorted(c[\"node_ids\"]) == [\"US-I\", \"US-S\"]" && grep -q "\"sam(sam@example.com)\": \"US-S\"" "$SH/.cca/accounts.json" && printf "%s" "$out" | grep -q "按维护者的映射添加了 Claude 账号 sam"'
ok "接着配置好内置代理（假的界面 App 托管内核），sam 的端口切到 US-S" 'scfg "c[\"local\"][\"client\"] == \"builtin\"" && printf "%s" "$out" | grep -q "本机端口池已就绪：内置代理 127.0.0.1:7901" && [ -f "$CD/up" ] && [ "$(csel CCA-1)" = US-S ]'
ok "经内置代理的安装节点装 Claude Code：问一句（默认是），不再先直连、不另外征求同意；装完临时内核停掉、临时目录删掉；有账号了不再问新增账号" 'printf "%s" "$out" | grep -q "没找到 Claude Code（claude 命令）。现在经内置代理用官方安装脚本装上吗？" && grep -qx "INSTALL-GET claude via US-I" "$IL" && grep -qx "INSTALL-DONE claude via=US-I" "$IL" && ! grep -q "INSTALL-GET claude direct" "$IL" && ! printf "%s" "$out" | grep -q "安装 claude 吗" && printf "%s" "$out" | grep -qF "装好了 claude：$SH/.local/bin/claude" && istopped && [ "$(ileft)" = 0 ] && ! printf "%s" "$out" | grep -qE "新增第一个账号|现在配置路由器"'
core_stop; rm -f "$SH/.local/bin/claude"; tback claude
# 不按回车也行：每隔几秒自动拉（CCA_POLICY_POLL=1 秒；答案是一直开着、没有内容的管道）
SH="$T/homes2"; mkdir -p "$SH/Applications"; cp -R "$T/homes/Applications/cca.app" "$SH/Applications/"
rm -f "$T/policy_keys"; printf 1 > "$T/policy_denied_n"; : > git.log
mkfifo "$T/tty.fifo"; exec 8<>"$T/tty.fifo"; printf 'y\n' >&8   # 有维护者共享的账号；之后等维护者时管道里没有内容
out=$(rs CCA_TTY="$T/tty.fifo" CCA_POLICY_POLL=1 "$H/.cca/bin/cca" install 2>&1); rc=$?
exec 8<&-; rm -f "$T/tty.fifo"
ok "等维护者时不用按回车：每隔几秒自动重试，拉到了就接着导入节点、配置内置代理（装好了 Claude Code、有账号时不再问）" '[ $rc -eq 0 ] && [ "$(grep -c "^GIT clone" git.log)" = 2 ] && printf "%s" "$out" | grep -q "等维护者运行上面那一行" && printf "%s" "$out" | grep -q "拉到了维护者的映射" && scfg "c[\"nodes\"] == [\"US-I\", \"US-S\"] and c[\"local\"][\"client\"] == \"builtin\"" && [ -f "$CD/up" ] && ! printf "%s" "$out" | grep -qE "没找到 Claude Code|新增第一个账号"'
core_stop
SH="$T/homes3"; rm -f "$T/direct_down" "$T/policy_denied_n"; : > curl.log
out=$(rs CCA_TTY="$(ans n n)" "$H/.cca/bin/cca" install 2>&1); rc=$?
ok "服务器上能直连：照旧问要不要配置路由器，不等维护者、不导入节点" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "这台机器能直连 claude.ai" && printf "%s" "$out" | grep -q "现在配置路由器" && ! printf "%s" "$out" | grep -q "policy.sh key add" && [ ! -e "$SH/.cca/policy/id_ed25519" ]'
# 没有维护者共享的账号（默认）：粘贴一条自己的代理当可选节点，配置好内置代理；不生成 deploy key、不拉映射
SH="$T/homes4"; mkdir -p "$SH/Applications"; cp -R "$T/homes/Applications/cca.app" "$SH/Applications/"
touch "$T/direct_down"; : > git.log
out=$(rs CCA_TTY="$(ans n 'socks5://u:p@198.51.100.7:1080' . n n)" "$H/.cca/bin/cca" install 2>&1); rc=$?
ok "服务器上直连不通、没有维护者共享的账号：粘贴一条自己的代理当可选节点（内核校验过），配置好内置代理；不生成 deploy key、不拉映射" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "有没有维护者共享给你的账号？ \[y/N\]" && printf "%s" "$out" | grep -q "节点用你自己的代理" && printf "%s" "$out" | grep -q "已加上你的代理：" && scfg "len(c[\"nodes\"]) == 1 and c[\"local\"][\"client\"] == \"builtin\" and not c.get(\"policy\")" && [ ! -e "$SH/.cca/policy/id_ed25519" ] && ! grep -q "^GIT clone" git.log && ! printf "%s" "$out" | grep -q "policy.sh key add"'
out=$(rs CCA_TTY="$(ans n n)" "$H/.cca/bin/cca" install 2>&1); rc=$?
ok "再运行 cca install：已经有可选节点了，不再问有没有维护者、不再要代理" '[ $rc -eq 0 ] && ! printf "%s" "$out" | grep -qE "有没有维护者共享给你的账号|节点用你自己的代理"'
core_stop; rm -f "$T/direct_down"
# 还原：映射仓库、假 GitHub 上的 deploy key
cp "$T/pr.srv" "$PR/policy.json"; rm -f "$PR/policy.json.sig" "$PR/nodes.json" "$PR/nodes.json.sig"; [ ! -f "$T/pr.srv.sig" ] || cp "$T/pr.srv.sig" "$PR/policy.json.sig"
pks "$(ans)" rm -y "$salias" >/dev/null 2>&1

fi
if sect 11c; then
# 11c. 一键安装（install.sh）：全新的 HOME，下载最新版、验签、安装
H3="$T/home3"; mkdir -p "$H3" "$T/tmp3"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" "$D/../install.sh" > "$T/install.sh"
r3() { env -i LANG=en_US.UTF-8 HOME="$H3" TMPDIR="$T/tmp3" CCA_T="$T" CCA_COST=off CCA_GATEWAY= CCA_POLICY_DEFAULT= CCA_HELPER="$HELPERWRAP" SHELL=/bin/zsh PATH="$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
publish 9.9.20
out=$(r3 CCA_TTY="$(ans y n n)" CCA_POLICY_REPO=example/cca-policy bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装时带 CCA_POLICY_REPO：配好映射仓库、生成这台机器的 deploy key，并拉到映射；仓库不是默认的那个时，给维护者的命令带上 -R" '[ -f "$H3/.cca/policy/id_ed25519" ] && [ -f "$H3/.cca/policy/policy.json" ] && printf "%s" "$out" | grep -q "映射仓库：example/cca-policy" && printf "%s" "$out" | grep -q "已更新映射" && printf "%s" "$out" | grep -q "    ./policy.sh key add -R example/cca-policy '\''ssh-ed25519 AAAA[^ ]* cca-policy@[^ ]*'\'' [^ ]*$"'
ok "一键安装：下载最新版、验签、装到 ~/.cca/bin 并链接" '[ $rc -eq 0 ] && [ "$("$H3/.cca/bin/cca" version)" = 9.9.20 ] && [ "$(readlink "$H3/.local/bin/cca")" = "$H3/.cca/bin/cca" ] && [ "$(readlink "$H3/.local/bin/cxa")" = cca ] && [ "$(readlink "$H3/.local/bin/aga")" = cca ] && printf "%s" "$out" | grep -q "已下载 cca 9.9.20，签名校验通过"'
ok "一键安装：~/.local/bin 不在 PATH 里时，问过之后写进 ~/.zshrc" 'grep -qF "export PATH=\"\$HOME/.local/bin:\$PATH\"" "$H3/.zshrc" && printf "%s" "$out" | grep -q "打开一个新的终端标签，输入 cca 就能用"'
out=$(r3 CCA_TTY="$(ans n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "再运行一次：修复/重装，不重复写 PATH" '[ $rc -eq 0 ] && [ "$(grep -c "\.local/bin" "$H3/.zshrc")" = 1 ] && printf "%s" "$out" | grep -q "已经写在"'
mv "$T/bin/claude" "$T/claude.off"; : > "$T/install.log"   # 新机器上还没装 Claude Code（一键安装不再要求先装好它）
out=$(r3 CCA_TTY="$(ans n y n)" "$H3/.cca/bin/cca" install 2>&1); rc=$?
rm -f "$H3/.local/bin/claude"; mv "$T/claude.off" "$T/bin/claude"
ok "cca install：没装 Claude Code 时问过之后用官方安装脚本装上（~/.local/bin 不在 PATH 里也找得到）" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "没找到 Claude Code（claude 命令）。现在用官方安装脚本装上吗？" && grep -qx "INSTALL-DONE claude via=direct" "$T/install.log" && printf "%s" "$out" | grep -qF "装好了 claude：$H3/.local/bin/claude" && printf "%s" "$out" | grep -q "~/.local/bin 不在 PATH 里"'
publish 9.9.21; echo "# 篡改" >> "$G/v9.9.21/cca"; rm -rf "$H3"; mkdir -p "$H3"
out=$(r3 CCA_TTY="$(ans y n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装：签名不对就不装" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "签名校验没通过" && [ ! -e "$H3/.cca" ]'
n_gh=$(grep -c "GH latest-get" gh.log)
r3 bash -c "head -c 1500 '$T/install.sh' | bash" >/dev/null 2>&1
ok "install.sh 只下载了一半：什么都不执行" '[ "$(grep -c "GH latest-get" gh.log)" = "$n_gh" ] && [ ! -e "$H3/.cca" ]'
publish 9.9.22; publish 9.9.23-beta.1
r3 CCA_TTY="$(ans y n n)" bash "$T/install.sh" >/dev/null 2>&1
out=$(r3 CCA_TTY="$(ans n n)" bash "$T/install.sh" v9.9.23-beta.1 2>&1); rc=$?
ok "一键安装默认装最新的正式版；指定版本号时（写成 tag 的样子 v9.9.23-beta.1 也认）装那个测试版，换下来的正式版留着" '[ $rc -eq 0 ] && [ "$("$H3/.cca/bin/cca" version)" = 9.9.23-beta.1 ] && printf "%s" "$out" | grep -q "正在下载 cca 9.9.23-beta.1" && [ "$(cat "$H3/.cca/update/cca.prev.version")" = 9.9.22 ]'
out=$(r3 "$H3/.cca/bin/cca" update --rollback 2>&1)
ok "一键安装装上的测试版也能退回" '[ "$("$H3/.cca/bin/cca" version)" = 9.9.22 ] && printf "%s" "$out" | grep -q "已退回 9.9.22"'
# 下载用的临时目录在 TMPDIR 里：装好了、签名不对没装都要删掉（以前 tmp 是 main 的 local，装完 main 返回以后 EXIT trap 才跑，看不到它，每装一次留下一份）
ok "一键安装用完删掉下载用的临时目录（装好了、签名不对没装都删）" '! ls -d "$T/tmp3"/cca-install.* >/dev/null 2>&1'
# 依赖太旧（RHEL / Rocky 8 的 OpenSSH 8.0 这类）：下载之前就说清楚；cca install 自己也查（手动下载的）
mkdir -p "$T/oldssh"
printf '#!/bin/sh\n[ "$1" = -V ] || exit 1\necho "OpenSSH_8.0p1, OpenSSL 1.1.1k  FIPS 25 Mar 2021" >&2\n' > "$T/oldssh/ssh"
chmod +x "$T/oldssh/ssh"
n_gh=$(grep -c "GH latest-get" gh.log)
out=$(r3 PATH="$T/oldssh:$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" CCA_TTY="$(ans y n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装：OpenSSH 太旧（8.0；校验发布签名的 ssh-keygen -Y 要 8.1）时说清楚，不下载" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "OpenSSH 太旧（8.0）" && [ "$(grep -c "GH latest-get" gh.log)" = "$n_gh" ]'
# 辅助程序只有四种机器的（Intel / Apple 芯片的 Mac，x86_64 / arm64 的 Linux）：别的机器下载之前就说清楚（假的 uname 说自己是 riscv64）
mkdir -p "$T/oddcpu"; printf '#!/bin/sh\n[ "$1" = -m ] && { echo riscv64; exit 0; }\nexec /usr/bin/uname "$@"\n' > "$T/oddcpu/uname"; chmod +x "$T/oddcpu/uname"
out=$(r3 PATH="$T/oddcpu:$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" CCA_TTY="$(ans y n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装：这种机器（riscv64）用不了时说清楚，不下载" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "cca 只支持 Intel / Apple 芯片的 Mac，x86_64 / arm64 的 Linux（这台是 riscv64）" && [ "$(grep -c "GH latest-get" gh.log)" = "$n_gh" ]'
out2=$(r3 PATH="$T/oldssh:$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" "$H3/.cca/bin/cca" install 2>&1); rc2=$?
ok "cca install 也查 OpenSSH 的版本（手动下载的）" '[ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "OpenSSH 太旧（8.0）"'

fi
if sect 11d; then
# 11d. 程序坞：cca 打开的 App 实例（账号的 Chrome、桌面 App）、终端交接用完退出的系统终端，退出以后去掉程序坞「最近使用的 App」里
# 留下的图标（cca _dock-tidy）。测试里程序坞的设置是 $T/dock/dock.plist（run 带着 CCA_DOCK_PREFS；CCA_DOCK_SETTLE=0：不等），
# 假 killall 只记一笔，假 lsappinfo 回 $T/lsappinfo.txt，假 open 像 LaunchServices 那样在后台跑小 App（跑完在 open.log 记 DOCK-APP-DONE）
DK="$T/dock"
dkapp() {   # dkapp 名字 bundle-id [真程序]：沙箱里的假 App
	mkdir -p "$DK/$1.app/Contents/MacOS"
	printf '<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>%s</string><key>CFBundleExecutable</key><string>%s</string></dict></plist>\n' "$2" "$1" > "$DK/$1.app/Contents/Info.plist"
	[ -z "${3:-}" ] || { cp "$3" "$DK/$1.app/Contents/MacOS/$1"; codesign -s - -f "$DK/$1.app" >/dev/null 2>&1; }
}
dk() { python3 -I "$T/dockpl.py" "$DK/dock.plist" "$@"; }   # set 最近使用的 [固定的] [show-recents] | get | add App
kills() { local n; n=$(grep -c '^KILLALL Dock$' "$T/killall.log" 2>/dev/null); echo "${n:-0}"; }   # 重启了几次程序坞
dkwait() { local i; for i in $(seq 1 100); do [ "$(grep -cxF -- "$3" "$1" 2>/dev/null)" -ge "$2" ] 2>/dev/null && return 0; sleep 0.1; done; return 1; }   # dkwait 文件 行数 那一行：等到有这么多行是这样的（最多 10 秒）
hires() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
mkdir -p "$DK"; dkapp A local.cca.test.a; dkapp B local.cca.test.b; dkapp S local.cca.test.s /bin/sleep
ok "_gui hello：说明能整理程序坞（App 据此让终端交接的脚本退出系统终端以后调 cca _dock-tidy）" 'gj hello | jok "d[\"dock\"] == 1"'
: > "$T/killall.log"; dk set '["A","Preview:nobid","A:nobid"]'
out=$(run cca _dock-tidy "$DK/A.app" 2>&1); rc=$?
ok "程序坞：App 退出后它留在「最近使用的 App」里的图标都去掉（按 bundle id 认，图标里没有 bundle id 的按位置认），别的不动；改完重启程序坞一次，什么都不说" '[ $rc -eq 0 ] && [ -z "$out" ] && [ "$(dk get)" = "Preview:nobid" ] && [ "$(kills)" = 1 ]'
: > "$T/killall.log"; dk set '["B","Preview:nobid"]'
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1
ok "程序坞：没有它留下的图标时不改设置、不重启程序坞" '[ "$(dk get)" = "B Preview:nobid" ] && [ "$(kills)" = 0 ]'
: > "$T/killall.log"; dk set '["A","B","Preview:nobid"]'
run CCA_DOCK_SETTLE=2 cca _dock-tidy "$DK/A.app" >/dev/null 2>&1 & TA=$!
sleep 0.5; run CCA_DOCK_SETTLE=2 cca _dock-tidy "$DK/B.app" >/dev/null 2>&1 & TB=$!
wait $TA $TB
ok "程序坞：几个先后退出的（最近一次退出以后等一会儿）一起整理，只重启一次；排队的记录不留" '[ "$(dk get)" = "Preview:nobid" ] && [ "$(kills)" = 1 ] && [ -z "$(ls "$DK/cca-dock" | grep exited)" ]'
: > "$T/killall.log"; dk set '["A","Preview:nobid"]'; echo A > "$T/dock_readd"
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1
ok "程序坞：重启以后又被加回来了（LaunchServices 还留着记录时会这样）：再等一轮、再重启一次" '[ "$(dk get)" = "Preview:nobid" ] && [ "$(kills)" = 2 ] && [ ! -e "$T/dock_readd" ]'
: > "$T/killall.log"; dk set '["A","Preview:nobid"]'; printf 'A\nkeep\n' > "$T/dock_readd"
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1; rm -f "$T/dock_readd"
ok "程序坞：每次都被加回来：最多重启两次就算了" '[ "$(kills)" = 2 ] && [ "$(dk get)" = "Preview:nobid A-back" ]'
"$DK/S.app/Contents/MacOS/S" 30 & SP=$!
: > "$T/killall.log"; dk set '["S","S","Preview:nobid"]'; run cca _dock-tidy "$DK/S.app" >/dev/null 2>&1; n1=$(kills); r1=$(dk get)
: > "$T/killall.log"; dk set '["S","Preview:nobid"]'; run cca _dock-tidy "$DK/S.app" >/dev/null 2>&1; n2=$(kills); r2=$(dk get)
: > "$T/killall.log"; dk set '["S"]' '["S"]'; run cca _dock-tidy "$DK/S.app" >/dev/null 2>&1; n3=$(kills)
: > "$T/killall.log"; dk set '["A","S","Preview:nobid","B"]'; run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1; n4=$(kills); r4=$(dk get)
kill $SP 2>/dev/null; wait $SP 2>/dev/null
ok "程序坞：同一个 App 还开着一个实例：两个图标里多出一个，要整理；只有它自己的那一个时不动" '[ "$n1" = 1 ] && [ "$r1" = "Preview:nobid" ] && [ "$n2" = 0 ] && [ "$r2" = "S Preview:nobid" ]'
ok "程序坞：固定在程序坞上的 App 开着时用固定的那个图标，「最近使用的 App」里它的图标就是多出来的" '[ "$n3" = 1 ]'
ok "程序坞：要重启时开着的 App 的图标也先拿掉（新的程序坞给在跑的实例重新加一个，不认原来的，不拿掉会多出一个），没开着的照留" '[ "$n4" = 1 ] && [ "$r4" = "Preview:nobid B" ]'
: > "$T/killall.log"; dk set '["A"]' '[]' false; run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1; n1=$(kills)
dk set '["A"]' '[]' true; cp "$H/.cca/config.json" "$T/config.dock"; edit_table 'j["dock_tidy"] = False' "$H/.cca/config.json"
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1; n2=$(kills)
sleep 30 & P=$!; n_open=$(grep -c 'cca-dock.app' open.log)
run cca _dock-tidy "$DK/A.app" $P >/dev/null 2>&1; n_open2=$(grep -c 'cca-dock.app' open.log)
kill $P; wait $P 2>/dev/null; cp "$T/config.dock" "$H/.cca/config.json"
ok "程序坞：没开「显示最近使用的 App」、config.json 的 dock_tidy 设成 false 时都不整理；关掉了连等待的小 App 都不起" '[ "$n1" = 0 ] && [ "$n2" = 0 ] && [ "$(dk get)" = A ] && [ "$n_open2" = "$n_open" ]'
dkapp G "local.cca.test.guard.$$"; : > "$T/killall.log"; : > "$T/lsappinfo.log"
t0=$(hires); out=$(run CCA_DOCK_PREFS= cca _dock-tidy "$DK/G.app" 2>&1); rc=$?; t1=$(hires)
ok "程序坞：HOME 换过（像这个测试的沙箱）又没指定设置文件时，这个用户真正的程序坞碰都不碰（不查、不改、不重启）" '[ $rc -eq 0 ] && [ -z "$out" ] && [ "$(kills)" = 0 ] && [ ! -s "$T/lsappinfo.log" ] && perl -e "exit($t1 - $t0 < 2 ? 0 : 1)"'
: > "$T/killall.log"; dk set '["A","Preview:nobid"]'
sleep 30 & P=$!; n_open=$(grep -c 'cca-dock.app\] \[--args\]' open.log)
t0=$(hires); out=$(run cca _dock-tidy "$DK/A.app" $P 2>&1); rc=$?; out2=$(run cca _dock-tidy "$DK/A.app" $P 2>&1); rc2=$?; t1=$(hires)
dkwait open.log 1 "DOCK-APP-DONE $P"   # 第二个小 App：已经有一个在等这个进程号，马上退出
waiters=$(ps -axww -o args= | grep -c "[ ]$P $DK/cca-dock ")
ok "程序坞：cca _dock-tidy App 进程号：交给后台小 App（open -g -j -n，LSBackgroundOnly）等它退出，马上返回；同一个进程号只留一个在等" '[ $rc -eq 0 ] && [ $rc2 -eq 0 ] && [ -z "$out$out2" ] && perl -e "exit($t1 - $t0 < 3 ? 0 : 1)" && [ "$(grep -c "cca-dock.app\] \[--args\]" open.log)" = $((n_open + 2)) ] && grep -qF "[-g] [-j] [-n] [-a] [$H/.cca/dock/cca-dock.app] [--args]" open.log && [ "$(plutil -extract LSBackgroundOnly raw "$H/.cca/dock/cca-dock.app/Contents/Info.plist")" = true ] && [ -x "$H/.cca/dock/cca-dock.app/Contents/MacOS/cca-dock" ] && [ "$waiters" = 1 ] && [ -f "$DK/cca-dock/$P.wait" ] && [ "$(kills)" = 0 ]'
kill $P; wait $P 2>/dev/null; dkwait open.log 2 "DOCK-APP-DONE $P"
ok "程序坞：等的那个实例退出了才整理，等待的锁文件删掉" '[ "$(kills)" = 1 ] && [ "$(dk get)" = "Preview:nobid" ] && [ ! -e "$DK/cca-dock/$P.wait" ]'
: > "$T/killall.log"; dk set '["A"]'; sleep 0 & P=$!; wait $P
run cca _dock-tidy "$DK/A.app" $P >/dev/null 2>&1; dkwait open.log 1 "DOCK-APP-DONE $P"
out=$(run cca _dock-tidy "$DK/A.app" 12x 2>&1); rc=$?
ok "程序坞：交过去时那个实例已经退出了：马上整理；进程号不是数字的不收" '[ "$(kills)" = 1 ] && [ -z "$(dk get)" ] && [ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法: cca _dock-tidy"'
: > "$T/killall.log"; : > "$T/lsappinfo.log"; dk set '["A","Preview:nobid"]'
printf '%s\n' '12) "A" ASN:0x0-0x1234:' '    bundleID="local.cca.test.a"' "    bundle path=\"$DK/A.app\"" '    pid = 123 type="Foreground" flavor=3 (exited-with-subordinates) ' > "$T/lsappinfo.txt"
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1 & TP=$!
dkwait "$T/lsappinfo.log" 2 "LSAPPINFO list"; held=$(kills)
rm -f "$T/lsappinfo.txt"; wait $TP
ok "程序坞：App 退出了、LaunchServices 还留着它的记录（exited-with-subordinates：它起的进程还在）时先不重启（重启的程序坞会照着记录把它加回去），记录没了再整理" '[ "$held" = 0 ] && [ "$(kills)" = 1 ] && [ "$(dk get)" = "Preview:nobid" ]'
printf '%s\n' '12) "B" ASN:0x0-0x1235:' '    bundleID="local.cca.test.b"' '    pid = 124 type="Foreground" (exited-with-subordinates)' '13) "A" ASN:0x0-0x1236:' '    bundleID="local.cca.test.a"' "    bundle path=\"$DK/A.app\"" '    pid = 125 type="Foreground"' > "$T/lsappinfo.txt"
: > "$T/killall.log"; dk set '["A"]'; run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1; rm -f "$T/lsappinfo.txt"
ok "程序坞：别的 App 留着的记录、这个 App 正常开着的实例的记录不算" '[ "$(kills)" = 1 ]'
: > "$T/killall.log"; : > "$T/lsappinfo.log"; dk set '["A","B","Preview:nobid"]'; echo A > "$T/dock_readd"
printf '%s\n' '12) "A" ASN:0x0-0x1234:' '    bundleID="local.cca.test.a"' '    pid = 123 type="Foreground" (exited-with-subordinates)' > "$T/lsappinfo.txt"
run cca _dock-tidy "$DK/A.app" >/dev/null 2>&1 & TH=$!
dkwait "$T/lsappinfo.log" 1 "LSAPPINFO list"   # 拿着锁，在等 A 的记录
t0=$(hires); run cca _dock-tidy "$DK/B.app" >/dev/null 2>&1; t1=$(hires)
dkwait "$T/killall.log" 1 "KILLALL Dock"; n1=$(kills); r1=$(dk get)
rm -f "$T/lsappinfo.txt"; wait $TH
ok "程序坞：已经有一个在整理（拿着锁）时，新来的记一笔就走，由那一个一起整理" 'perl -e "exit($t1 - $t0 < 2 ? 0 : 1)" && [ "$n1" = 1 ]'
ok "程序坞：有的 App 的记录一直不清（它起的进程还在）时不拖累别的：别的先整理（重启前它的图标也拿掉，新的程序坞照着记录加回一个，不会多），记录没了再整理它" '[ "$r1" = "Preview:nobid A-back" ] && [ "$(kills)" = 2 ] && [ "$(dk get)" = "Preview:nobid" ] && [ -z "$(ls "$DK/cca-dock" | grep exited)" ]'

fi
if sect 12; then
# 12. 一键还原：移除端口池需要重载（restore 已确认）
cp -R "$T/appsrc/9.9.16/cca.app" "$GA"; "$GA/Contents/MacOS/cca" & GUIPID=$!; sleep 0.3
out=$(run CCA_TTY="$(ans restore)" cca restore 2>&1); rc=$?
kill $GUIPID 2>/dev/null; wait $GUIPID 2>/dev/null
ok "restore：图形界面 App 开着时不还原（先退出它）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "cca 的界面 App" && [ -d "$GA" ] && [ -d "$H/.cca" ]'
# 会话宿主里还开着一个会话（直接起辅助程序的 remote-host，客户端是 sleep）：删掉 ~/.cca 以后它就再也接不上了，先不还原
run script -q /dev/null "$HELPER_BIN" remote-host claude k 测试 -- /bin/sleep 60 </dev/null >/dev/null 2>&1 &
rs_host=$!
for rs_i in $(seq 1 100); do [ -n "$(ls "$H/.cca/remote" 2>/dev/null)" ] && break; sleep 0.1; done
out=$(run CCA_TTY="$(ans no)" cca restore 2>&1); rc=$?
ok "restore：还有开着的远程会话时不还原（先结束它们）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "还有开着的远程会话，先把它们结束再还原" && [ -d "$H/.cca" ] && [ "$(ls "$H/.cca/remote" | wc -l | tr -d " ")" = 2 ]'
pkill -f "remote-host claude k 测试 -- /bin/sleep 60" 2>/dev/null; kill "$rs_host" 2>/dev/null; wait "$rs_host" 2>/dev/null
for rs_i in $(seq 1 50); do pgrep -f "remote-host claude k 测试" >/dev/null 2>&1 || break; sleep 0.1; done
ok "（前提）会话结束了：会话目录里什么都不剩" '[ -z "$(ls "$H/.cca/remote" 2>/dev/null)" ]'
# cca uninstall 和 cca restore 是同一条命令的两个名字：确认时输入的是用的那个名字
out=$(run CCA_TTY="$(ans restore)" cca uninstall 2>&1); rc=$?
ok "uninstall：确认时要输入 uninstall（输入 restore 是取消）；清单里说了放在别处的也清掉" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "已取消" && printf "%s" "$out" | grep -q "卸载 cca" && printf "%s" "$out" | grep -q "cca 放在别处的也清掉：解出来的辅助程序和缓存、界面 App 的缓存和设置、shell 启动文件里 cca 加的那两行" && printf "%s" "$out" | grep -q "先打包备份配置" && [ -d "$H/.cca" ]'
out=$(run CCA_TTY="$(ans no)" cca uninstall --no-backup 2>&1); rc=$?
ok "uninstall --no-backup：清单里说不留备份；不认识的参数说用法，什么都不动" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "不留备份（--no-backup" && ! printf "%s" "$out" | grep -q "先打包备份配置" && ! printf "%s" "$out" | grep -q "安装前的备份在打包里" && ! run CCA_TTY="$(ans uninstall)" cca uninstall --purge >/dev/null 2>&1 && [ -d "$H/.cca" ]'
# 账号专属的 Chrome 还开着（假的 pgrep 认 $T/app-open 里的记录）：它的资料目录在 ~/.cca 里，删了它还接着往里写
un_ad=$(ls -d "$H"/.cca/accounts/alice-example-com-* 2>/dev/null | head -n 1)
mkdir -p "$T/app-open"; printf '%s\n' "$$" "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --user-data-dir=$un_ad/chrome --no-first-run" "$un_ad/chrome" 0 > "$T/app-open/900000777"
out=$(run CCA_TTY="$(ans uninstall)" cca uninstall 2>&1); rc=$?
rm -f "$T/app-open/900000777"
ok "uninstall：账号专属的浏览器窗口还开着时不卸载（先关掉）" '[ -n "$un_ad" ] && [ $rc -ne 0 ] && printf "%s" "$out" | grep -q "（浏览器窗口）。先退出它们再还原" && [ -d "$H/.cca" ]'
run CCA_TTY="$(ans "$R1")" cca key alice >/dev/null 2>&1; run CCA_TTY="$(ans "$R2")" cca codex key dave >/dev/null 2>&1
refs=$(ls "$T/keychain" | grep -cE "^cca-(codex-)?ref-")
K=$(cat "$H/.cca/router_ed25519.pub"); echo "$K" >> "$R/root/.ssh/authorized_keys"
# Linux 上的登录凭证和密钥文件：账号表里没有的账号目录（没退出登录）、没记上账的密钥文件，也不能进备份
mkdir -p "$H/.cca/secrets" "$H/.cca/accounts/orphan-example-com/claude"
echo '{"v": 1, "host": "api.example.com", "key": "sk-LEFTOVER-SECRET"}' > "$H/.cca/secrets/cca-api-orphan"
echo '{"claudeAiOauth": {"accessToken": "cred-LEFTOVER-SECRET"}}' > "$H/.cca/accounts/orphan-example-com/claude/.credentials.json"
# 维护者的参考节点里有节点密钥（连同拉取映射时留下的临时文件）：也不进备份；映射本身照样打包
mkdir -p "$H/.cca/policy/tmp.Ab12cd/repo"; [ -f "$H/.cca/policy/policy.json" ] || echo '{}' > "$H/.cca/policy/policy.json"
NODEJ='{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-Z", "type": "ss", "server": "192.0.2.99", "port": 8388, "cipher": "aes-128-gcm", "password": "node-LEFTOVER-SECRET"}]}'
for f in nodes.json nodes.json.4242 tmp.Ab12cd/repo/nodes.json; do printf '%s\n' "$NODEJ" > "$H/.cca/policy/$f"; done
# Antigravity 账号（10f2 收尾时删掉了 agy.json）：表里登录着的 gina 也退出登录；账号表里没有的账号目录（没退出登录）里的 token 同样不进备份
echo '{"gina@example.com": "US-A"}' > "$H/.cca/agy.json"
for a in "gina-example-com-$(printf %s gina@example.com | shasum | cut -c1-6)" orphan-example-com; do
	mkdir -p "$H/.cca/agy/accounts/$a/gemini/antigravity-cli"
	echo '{"access_token": "ya29.agy-LEFTOVER-SECRET", "id_token_email": "gina@example.com"}' > "$H/.cca/agy/accounts/$a/gemini/antigravity-cli/antigravity-oauth-token"
done
ag_pre=$(readlink "$H/.local/bin/aga" 2>/dev/null)
# 邮箱助手配置过：应用专用密码在钥匙串里
python3 -c 'import json,sys; p=sys.argv[1]; c=json.load(open(p)); c["mail"]={"user": "inbox@example.com"}; json.dump(c, open(p, "w"))' "$H/.cca/config.json"
mkdir -p "$T/keychain/cca-mail"; printf abcdefghijklmnop > "$T/keychain/cca-mail/inbox@example.com"
# 远程会话配对过一台机器（docs/dev/remote.md）：配对表在 ~/.cca/remote，密钥在钥匙串里
RSP=$(printf 'ab%.0s' $(seq 1 16)); mkdir -p "$H/.cca/remote" "$T/keychain/cca-remote-$RSP"; chmod 700 "$H/.cca/remote"
printf '%064d' 7 > "$T/keychain/cca-remote-$RSP/pair"
printf '{"v":1,"peers":{"%s":{"fp":"SHA256:%s","name":"dan-air","incoming":"control","shown":true,"at":1794000000}}}\n' "$RSP" "$(printf 'A%.0s' $(seq 1 43))" > "$H/.cca/remote/peers.json"
# 加入过自己的同步服务器（docs/dev/hub.md），服务器这时连不上（127.0.0.1:9）：restore 先在清单里说要退出，照样退出、说怎么在别的设备上
# 移除这台；加入信息（链接里的入伙口令、盐）、这台设备在它上面的私钥、经它同步来的折合金额都删掉，不进备份
RHA=$(printf 'e7%.0s' $(seq 1 16)); RHS=$(printf '9d%.0s' $(seq 1 16))
mkdir -p "$H/.cca/hub" "$H/.cca/cost"; chmod 700 "$H/.cca/hub"; ssh-keygen -q -t ed25519 -N '' -C hub -f "$H/.cca/hub/id_ed25519"
printf '{"v":1,"url":"https://127.0.0.1:9","pins":["sha256//%s="],"admit":"%s","salt":"%s","name":"studio","joined_at":1794000000}\n' \
	"$(printf 'A%.0s' $(seq 1 43))" "$RHA" "$RHS" > "$H/.cca/hub/hub.json"
printf '{"acc":{"x":"HUB-REMOTE-SECRET"}}\n' > "$H/.cca/cost/own.remote.json"
# macOS 26 以下由 cca 在后台托管内核时，配置内置代理时问过后在用户的 crontab 里加了开机起内核的那一行（假的 crontab：$T/crontab）。
# 这次在 CCA_GUI_OS=15 下还原：要删掉它（以前只在 Linux 上删），别的行留着
printf '%s\n' '0 3 * * * echo user-job' "@reboot '$H/.local/bin/cca' _core-keep >/dev/null 2>&1 # cca-core" > "$T/crontab"
# 装着的是接着辅助程序的那一份（11b 的 mkver）：它解出来的辅助程序在缓存目录的 cca-helper 里，卸载时整个目录删掉（别的版本解出来的
# 也在里面）。run 不带 TMPDIR 时缓存目录是这个用户真的临时目录：这一次换成它下面单独的一个（TMPDIR 在 /var/folders 下时 cca 就用它；
# $T 是 /private/var/… 开头的，不认），不然删的是这台 Mac 上真的 cca、同时在跑的别的回归解出来的那些
hs=$(sed -n 's/^HELPER_SUMS="[^=]*=\([0-9a-f]*\)"$/\1/p' "$H/.cca/bin/cca" 2>/dev/null)
un_cache="$(getconf DARWIN_USER_TEMP_DIR)cca-regress-cache.$$"
mkdir -p "$un_cache/cca-helper" "$un_cache/cca-py" "$T/dock/cca-dock"; chmod 700 "$un_cache/cca-helper"
: > "$un_cache/cca-helper/0000-another-version"; : > "$un_cache/cca-py/old.pyc"; : > "$T/dock/cca-dock/queued"
# 界面 App 留在 ~/Library 里的（上面 cca gui remove 那一条删过一次，这里再放上）
mkdir -p "$H/Library/Caches/local.cca.app/runs" "$H/Library/Saved Application State/local.cca.app.savedState" "$H/Library/HTTPStorages/local.cca.app" "$H/Library/Preferences"
: > "$H/Library/Caches/local.cca.app/core.log"; : > "$H/Library/Preferences/local.cca.app.plist"; : > "$H/Library/Preferences/com.example.other.plist"; : > "$T/defaults.log"
# shell 启动文件里 cca 加的那两行：~/.local/bin 里还有别的命令（keepme）时留着（去掉的各种情况在 tests/path_unhint.sh）
: > "$H/.local/bin/keepme"; printf 'export FOO=1\n\n# cca（Claude Code 多账号）\nexport PATH="$HOME/.local/bin:$PATH"\n' > "$H/.zshrc"; cp "$H/.zshrc" "$T/zshrc.before"
out=$(run TMPDIR="$un_cache" CCA_GUI_OS=15 CCA_TTY="$(ans uninstall)" cca uninstall 2>&1); rc=$?
ok "restore：端口池、钩子、cron、文件、公钥全部清掉，覆写脚本逐字节一致" '[ $rc -eq 0 ] && [ "$(nl_)" = 0 ] && ! ls "$R"/sel/CCA-* >/dev/null 2>&1 && cmp -s pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ ! -e "$R/etc/cca" ] && ! grep -q cca "$R/etc/crontabs/root" && [ "$(cat "$R/etc/sysupgrade.conf")" = "/root/.ssh/" ] && ! grep -qF "$K" "$R/root/.ssh/authorized_keys"'
ok "restore：启动器和图形界面 App 全部删掉（确认清单里说了会删 App）" '[ -z "$(ls "$H/Applications" 2>/dev/null)" ] && printf "%s" "$out" | grep -q "还有图形界面 $GA"'
ok "uninstall：缓存目录里解出来的辅助程序（cca-helper 整个目录，别的版本的也在里面）、3.5.0 以前留下的字节码缓存、程序坞整理的排队记录都删掉" '[ -n "$hs" ] && [ ! -e "$un_cache/cca-helper" ] && [ ! -e "$un_cache/cca-py" ] && [ ! -e "$T/dock/cca-dock" ]'
ok "uninstall：界面 App 留在 ~/Library 里的缓存、窗口状态、设置也删掉（设置先 defaults delete），别的 App 的不动" '[ ! -e "$H/Library/Caches/local.cca.app" ] && [ ! -e "$H/Library/Saved Application State/local.cca.app.savedState" ] && [ ! -e "$H/Library/HTTPStorages/local.cca.app" ] && [ ! -e "$H/Library/Preferences/local.cca.app.plist" ] && [ -f "$H/Library/Preferences/com.example.other.plist" ] && grep -qx "delete local.cca.app" "$T/defaults.log"'
ok "uninstall：~/.local/bin 里还有别的命令时，shell 启动文件里 cca 加的那两行留着、说一声" 'cmp -s "$T/zshrc.before" "$H/.zshrc" && [ -f "$H/.local/bin/keepme" ] && printf "%s" "$out" | grep -q "/.zshrc 里把 ~/.local/bin 加进 PATH 的那两行留着"'
ok "restore：本机还原（cca、cxa、aga 都删掉，确认清单里说了），共享主目录内容保留" '[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ ! -L "$H/.local/bin/cxa" ] && [ "$ag_pre" = cca ] && [ ! -L "$H/.local/bin/aga" ] && printf "%s" "$out" | grep -q "删除 ~/.local/bin 里的 cca、cxa、aga，" && [ -f "$H/.claude/projects/-p1/memory/alice.md" ]'
ok "restore：第三方 API 账号的 key、官方账号记下备查的 key（Claude、Codex 各一把）都从钥匙串删掉；配置备份只有自己能读（账号表里可能写着 key）" '[ "$refs" = 2 ] && [ -z "$(ls "$T/keychain" | grep -E "^cca-(codex-)?(api|ref)-")" ] && [ "$(stat -f %Lp "$(ls "$H"/cca-backup-*.tar.gz | head -n 1)")" = 600 ]'
ok "restore：macOS 26 以下（内核由 cca 在后台托管）也删掉 crontab 里开机起内核的那一行，别的行留着" '! grep -qF "# cca-core" "$T/crontab" && grep -qx "0 3 \* \* \* echo user-job" "$T/crontab"'
BK=$(ls "$H"/cca-backup-*.tar.gz | head -n 1)
ok "restore：先说明 Codex 账号的 git 工作树会一起删；退出 Codex 账号的登录；备份里没有登录凭证（含 Linux 上的 .credentials.json、密钥文件）和工作树" 'printf "%s" "$out" | grep -qE "git 工作树（worktrees）会一起删掉：(.*、)?dave@example.com" && grep -q "LOGOUT $DD/codex" codex.log && ! tar -tzf "$BK" | grep -qE "auth\.json|/worktrees|/id_ed25519$|\.credentials\.json|secrets" && ! tar -xzOf "$BK" 2>/dev/null | grep -qE "at-SECRET|LEFTOVER-SECRET" && tar -tzf "$BK" | grep -q "orphan-example-com/claude"'
ok "restore：备份里没有维护者参考节点的节点密钥（policy/nodes.json、拉取映射的临时文件），映射本身在" '! tar -tzf "$BK" | grep -qE "policy/(nodes\.json|tmp\.)" && tar -tzf "$BK" | grep -q "policy/policy\.json"'
ok "restore：备份里没有折合金额的缓存、别的设备同步来的（cost/scan.json、remote.json 这些），这台设备统计好的（cost/local.json）在" '! tar -tzf "$BK" | grep -qE "cost/(scan|remote|worker)\.json|cost/(last|scanned)$" && tar -tzf "$BK" | grep -q "cost/local\.json"'
ok "restore：清单里说要退出自己的同步服务器；服务器连不上照样退出，说怎么在别的设备上移除这台；加入信息、私钥、经它同步来的折合金额都不进备份" 'printf "%s" "$out" | grep -q "自己的同步服务器（127.0.0.1:9）：退出" && printf "%s" "$out" | grep -q "服务器上没能移除这台设备.*cca hub remove" && ! tar -tzf "$BK" | grep -qE "(^|/)hub/|cost/own\." && ! tar -xzOf "$BK" 2>/dev/null | grep -qE "$RHA|$RHS|HUB-REMOTE-SECRET"'
ok "restore：Codex 账号也一起还原：第三方 API 的 key、ChatGPT 启动器删掉，~/.codex 原来的登录和共享的内容不动" '[ -z "$(ls "$T/keychain" | grep "^cca-codex-api-")" ] && grep -q master "$X/auth.json" && [ -d "$X/skills/from-alice" ] && [ ! -e "$H/Applications/ChatGPT dave.app" ]'
ok "restore：Antigravity 账号也一起还原：退出登录；备份里有 agy.json、没有登录 token（账号表里没有的账号目录里的也不带）；~/.gemini 原来的登录和共享的会话不动" 'printf "%s" "$out" | grep -q "已退出账号 gina@example.com" && tar -tzf "$BK" | grep -qx "\./agy\.json" && tar -tzf "$BK" | grep -q "agy/accounts/orphan-example-com/gemini" && ! tar -tzf "$BK" | grep -q antigravity-oauth-token && grep -q master "$H/.gemini/antigravity-cli/antigravity-oauth-token" && [ -f "$H/.gemini/antigravity-cli/conversations/from-alice.pb" ]'
ok "restore：邮箱助手的应用专用密码也从钥匙串删掉（确认清单里说了）" '[ ! -e "$T/keychain/cca-mail" ] && printf "%s" "$out" | grep -q "邮箱助手：删掉钥匙串里 inbox@example.com 的应用专用密码"'
ok "restore：远程会话的配对也取消，配对的密钥从钥匙串删掉（确认清单里说了）" '[ ! -e "$T/keychain/cca-remote-$RSP" ] && printf "%s" "$out" | grep -q "远程会话：取消和别的机器的配对"'
ok "所有重载前都用 OpenClash 的 SAFE_PATHS 做了 -t 校验" 'grep -q "CORE SAFE_PATHS=/usr/share/openclash:/etc/ssl -t" router.log'
# --no-backup：不打包、直接删。上面已经卸载干净了：放一份只有空配置和空账号表的 ~/.cca，用仓库里的 cca 再卸载一次；
# ~/.local/bin 这回是空的（前面各节装进去的假命令、keepme 都拿走），启动文件里那两行跟着去掉
mkdir -p "$H/.cca" "$un_cache"; echo '{}' > "$H/.cca/config.json"; echo '{}' > "$H/.cca/accounts.json"; rm -rf "$H/.local/bin"; mkdir -p "$H/.local/bin"
un_bk0=$(ls "$H"/cca-backup-*.tar.gz 2>/dev/null | wc -l | tr -d ' ')
out=$(run TMPDIR="$un_cache" CCA_TTY="$(ans uninstall)" "$CCA" uninstall --no-backup 2>&1); rc=$?
ok "uninstall --no-backup：不留备份，照样删掉 ~/.cca；~/.local/bin 空了：目录删掉，启动文件里 cca 加的那两行（连同前面的空行）去掉" '[ $rc -eq 0 ] && [ ! -e "$H/.cca" ] && [ "$(ls "$H"/cca-backup-*.tar.gz 2>/dev/null | wc -l | tr -d " ")" = "$un_bk0" ] && printf "%s" "$out" | grep -q "没有留配置备份（--no-backup）" && [ ! -e "$H/.local/bin" ] && [ "$(cat "$H/.zshrc")" = "export FOO=1" ]'
[ -z "$un_cache" ] || rm -rf "$un_cache"
# 11b 起各节（run 不带 TMPDIR）把装着的那一份的辅助程序解在了这个用户真的临时目录里：以前由上面那次还原删掉，现在缓存目录换成了
# 单独的一个，这里照旧删掉这一份（只这个版本的，名字是它的 SHA-256）
[ -z "$hs" ] || rm -f "$(getconf DARWIN_USER_TEMP_DIR)cca-helper/$hs"
fi
echo "---- $pass passed, $fail failed"
