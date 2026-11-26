#!/bin/bash
# Linux 上的测试（tests/regress.sh 只能在 macOS 上跑）：cca 的命令行在这台 Linux 上能不能用。
#   bash tests/linux.sh                    在这台 Linux 上跑一遍（临时 HOME，不碰真的 ~/.cca 和 crontab；要联网：下载内核、测出口）
#   bash tests/linux.sh --docker [镜像…]   每个发行版一个容器各跑一遍（要 Docker；不写镜像就跑下面 IMAGES 里的主要发行版）
# 流程：
#   一键安装（install.sh：GitHub 换成本机的文件、发布密钥换成临时生成的；验签、cca install、PATH 写进 ~/.bashrc；缺命令时给出这个
#     发行版的安装命令）→ 自动更新（假 GitHub 上的新版本：后台下载验签、下一条命令换上；退回后不再自动装）
#   内置代理：cca local setup（下载、校验内核，后台托管，端口池，问过后在 crontab 里加开机起内核的一行）→ 经账号的端口访问本机的
#     HTTP 服务 → 杀掉内核看托管进程拉起来 → 模拟重启（照 crontab 那一行、在 cron 的环境里起托管进程）
#   登录（给出在 Mac 上运行的 cca login-page 那一行：带别名、节点和节点摘要；贴回验证码，转给假 claude 本机的回调）→ 启动账号 →
#     API key 存成 600 的文件 → Team（真的服务器：辅助程序的 team-server，管理员由 cca-test spec 扮演：加入、照配置加账号、
#     不是根密钥签的一版不用）→ 设备统计（报给 Team 的服务器：会话开着报在用、关了报没在用；会话的登记、进程的启动时间从 /proc 读）
#     → Codex（假的 codex 和 app-server：添加、服务器上登录、启动、查用量、cxa）→ 真实的出口和泄露检测（测试节点直连出去；服务器上
#     不测 WebRTC）→ 只在 macOS 上的命令说清楚 → cca local off
#   服务器上不用路由器（全新的 HOME，直连通不通由假 curl 定）：直连不通时 cca install 不问路由器，先问有没有 Team 的邀请链接，没有就
#     先跳过；再运行时贴上邀请链接：加入、导入 Team 的配置指定的节点、照配置加上账号，内置代理由 cca _core-keep 托管，经账号的端口出去
#   在家走路由器（只在容器里：假路由器上跑真的 cca-router.sh / apply.rb，内核是真的 mihomo，端口池监听 0.0.0.0）：cca install 里配置
#     路由器、选节点、加账号、问过后重载，经路由器的端口出去，启动账号，在家时给内置代理拷节点（路由器端自己写 JSON）
#   远程会话、Team 的同步服务器的一键安装脚本：测试程序 suite 的几个模块（tests/remote_host.rs、remote_relay.rs、hub_deploy.rs；安装脚本那一个的
#     假服务器是 tests/kit/hubbox.rs：这个发行版的 sh、openssl、gzip，安装脚本带的服务端程序就是这一份 musl 的辅助程序，
#     假的 systemctl、docker 把解出来的那一份起成服务——真的 Linux 程序经安装脚本走一遍只有这里测得到）
# 测试节点是本机另一个 mihomo（socks5 → DIRECT）。端口池固定从 7901 起，被占用时不跑
# 辅助程序（cli/ 的 Rust 程序）用静态链接的那一份：cli/target/<x86_64|aarch64>-unknown-linux-musl/release/cca-core。--docker 时在宿主机上
# 先编（Mac 上是交叉编译，见 cli/.cargo/config.toml 和 cli/musl-env.sh）；直接在 Linux 上跑时没有就自己编（要 Rust 和这个目标），或者用 CCA_HELPER 指一份
# 测试自己用的两个程序（tests/ 的 Rust 包 cca-tests，也是静态链接的，同一个目标）：小工具 cca-test（假的 claude / codex / netstat 要做的事、
# 读写 JSON、扮演 Team 的管理员、两个小服务；这个脚本里是 KT）和测试程序 suite（上面那几个模块；这个脚本里是 KS）。有 cargo 就现编
# （cd cli && cargo build --profile kit -p cca-tests --locked --target <目标> --bins --tests）。没有 cargo 的机器：在别的机器上编好拷过来，
# 用 CCA_TEST_BIN、CCA_SUITE_BIN 指给它（辅助程序用 CCA_HELPER），或者把 cli/target/<目标>/kit/ 整个照原样拷过来（cca-test 在它下面；suite 的文件名
# 带一串摘要、在哪一层随 cargo 的版本变，脚本找最新的那一个）。
# --docker 时三个程序都在宿主机上先编好（仓库是只读挂进容器的），容器里不用 cargo
set -u
R=$(cd "$(dirname "$0")/.." && pwd)
case "$(uname -m)" in x86_64|amd64) HT=x86_64-unknown-linux-musl ;; aarch64|arm64) HT=aarch64-unknown-linux-musl ;; *) HT="" ;; esac
helper_build() {   # 这台机器（容器和宿主机是同一种芯片）的辅助程序：编出来；没有 cargo 时用已经编好的
	[ -n "$HT" ] || { echo "辅助程序没有这种机器的（$(uname -m)）" >&2; return 1; }
	if command -v cargo >/dev/null 2>&1; then
		# Mac 上交叉编译（--docker）：ring 里的 C 代码要 musl-env.sh 指定编译器和归档工具；在 Linux 上它什么都不做
		( cd "$R/cli" && . ./musl-env.sh && cargo build --release --locked --target "$HT" ) >/dev/null 2>&1 ||
			{ echo "辅助程序编不出来：cd cli && . ./musl-env.sh && cargo build --release --locked --target ${HT}（没装这个目标就先 rustup target add ${HT}）" >&2; return 1; }
	fi
	[ -x "$R/cli/target/$HT/release/cca-core" ] || { echo "辅助程序还没编出来：cd cli && cargo build --release --locked --target $HT" >&2; return 1; }
}
kit_build() {   # 测试自己用的两个程序：有 cargo 就编出来，没有时用已经编好的。找到的放进 KT（cca-test）、KS（suite）
	local j out="$R/cli/target/$HT/kit"
	[ -n "$HT" ] || { echo "测试用的程序没有这种机器的（$(uname -m)）" >&2; return 1; }
	if command -v cargo >/dev/null 2>&1; then
		# suite 是测试程序，文件名后面带着一串摘要：照 cargo 的 JSON 消息找编出来的是哪一个（和 regress.sh 一样）
		j=$(cd "$R/cli" && . ./musl-env.sh && cargo build --profile kit -p cca-tests --locked --target "$HT" --bins --tests --message-format=json 2>/dev/null) ||
			{ echo "测试用的程序编不出来：cd cli && . ./musl-env.sh && cargo build --profile kit -p cca-tests --locked --target ${HT} --bins --tests（没装这个目标就先 rustup target add ${HT}）" >&2; return 1; }
		KS=$(printf '%s\n' "$j" | sed -n 's/.*"kind":\["test"\][^}]*"name":"suite".*"executable":"\([^"]*\)".*/\1/p' | tail -n 1)
	else
		# 没有 cargo：用编好的里面最新的那一个。suite 的文件名带一串摘要，放在哪一层随 cargo 的版本变（deps/，新的是 build/cca-tests/<摘要>/out/）
		KS=$(ls -t "$out"/deps/suite-* "$out"/build/cca-tests/*/out/suite-* 2>/dev/null | while IFS= read -r j; do
			[ -f "$j" ] && [ -x "$j" ] && { printf '%s\n' "$j"; break; }; done)
	fi
	KT="$out/cca-test"
	[ -x "$KT" ] && [ -n "$KS" ] && [ -x "$KS" ] ||
		{ echo "测试用的程序还没编出来：cd cli && cargo build --profile kit -p cca-tests --locked --target $HT --bins --tests（要 Rust 和这个目标）；这台机器上没有 cargo 的话，在别的机器上编好拷过来，用 CCA_TEST_BIN、CCA_SUITE_BIN 指给它（cca-test 在 cli/target/$HT/kit/ 下；suite 的文件名带一串摘要，cargo build 加 --message-format=json 时 \"executable\" 那一项写着它在哪）" >&2; return 1; }
}
IMAGES="ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:12 debian:13 rockylinux/rockylinux:9 almalinux:10 fedora:43 amazonlinux:2023 alpine:3.22 archlinux:latest opensuse/leap:16.0"

if [ "${1:-}" = --docker ]; then
	shift
	[ $# -gt 0 ] || set -- $IMAGES
	command -v docker >/dev/null 2>&1 || { echo "没有 docker" >&2; exit 2; }
	helper_build || exit 2   # 仓库是只读挂进容器的，辅助程序在这里先编好
	kit_build || exit 2      # 测试用的两个程序也是：容器里没有 cargo，照挂进去以后的路径指给它们
	case "$KS" in "$R"/*) ;; *) echo "测试程序 suite 编到仓库外面去了（${KS}）：容器里看不到它（CARGO_TARGET_DIR 指了别处？）" >&2; exit 2 ;; esac
	L=$(mktemp -d "${TMPDIR:-/tmp}/cca-linux.XXXXXX"); mkdir -p "$L/core"
	# 内置代理的内核只下载一次（GitHub 慢的时候十几个容器同时下容易超时），校验过放进 $L/core 给各个容器（CCA_LINUX_CORE_CACHE；
	# cca 照样校验）。没下成就让各个容器自己下
	cv=$(sed -n 's/^CORE_VERSION="\(.*\)"$/\1/p' "$R/cca")
	case "$(uname -m)" in x86_64|amd64) a=amd64-compatible k=AMD64 ;; aarch64|arm64) a=arm64 k=ARM64 ;; *) a="" ;; esac
	if [ -n "$a" ] && [ -n "$cv" ]; then
		f="$L/core/mihomo-linux-$a-v$cv.gz"
		curl -q -fsSL --retry 3 --connect-timeout 15 -m 900 -o "$f" "https://github.com/MetaCubeX/mihomo/releases/download/v$cv/${f##*/}" 2>/dev/null
		[ "$(sha256sum "$f" 2>/dev/null | cut -d' ' -f1)" = "$(sed -n "s/^CORE_SHA256_LINUX_$k=\"\(.*\)\"$/\1/p" "$R/cca")" ] || rm -f "$f"
	fi
	for img in "$@"; do   # 各用各的网络：端口池互不冲突，并行跑
		n=$(printf '%s' "$img" | tr '/:.' '---')
		docker run --rm --name "cca-linux-$n-$$" -v "$R":/cca:ro -v "$L/core":/cca-core:ro -e CCA_LINUX_CORE_CACHE=/cca-core \
			-e CCA_TEST_BIN="/cca${KT#"$R"}" -e CCA_SUITE_BIN="/cca${KS#"$R"}" \
			"$img" sh /cca/tests/linux.sh --in-container > "$L/$n.log" 2>&1 &
	done
	wait
	bad=0
	for img in "$@"; do
		n=$(printf '%s' "$img" | tr '/:.' '---')
		echo; cat "$L/$n.log"
		grep -q '^  结果：[0-9]* 通过，0 失败' "$L/$n.log" || bad=$((bad + 1))
	done
	rm -rf "$L"
	echo; echo "$# 个发行版，$((${#@} - bad)) 个全部通过"
	[ $bad -eq 0 ]
	exit
fi

if [ "${1:-}" = --in-container ]; then   # 容器里（sh 跑的，可能还没有 bash）：用发行版自己的包管理器装 cca 要的，busybox、curl-minimal 原样用
	. /etc/os-release
	echo "== ${PRETTY_NAME:-$ID}"
	deps() {
		if command -v apt-get >/dev/null 2>&1; then
			export DEBIAN_FRONTEND=noninteractive
			# 不装 procps（Debian 的镜像里本来就没有，Ubuntu 的自带）：cca 在 Linux 上不靠 ps、pgrep、pkill
			apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq --no-install-recommends bash curl ca-certificates gzip perl netcat-openbsd openssh-client openssl >/dev/null 2>&1
		elif command -v dnf >/dev/null 2>&1; then
			dnf install -y -q gzip perl nmap-ncat openssh-clients procps-ng findutils openssl >/dev/null 2>&1
		elif command -v apk >/dev/null 2>&1; then
			apk add -q bash curl perl openssh-client-default openssh-keygen openssl >/dev/null 2>&1
		elif command -v pacman >/dev/null 2>&1; then
			pacman -Sy --noconfirm --needed curl perl openbsd-netcat openssh procps-ng openssl >/dev/null 2>&1
		elif command -v zypper >/dev/null 2>&1; then
			zypper -q -n install curl perl netcat-openbsd openssh-clients procps gzip findutils openssl >/dev/null 2>&1
		fi
	}
	ruby_dep() {   # 假路由器上的 cca 要 ruby（真的路由器上 OpenClash 自带）：装不上只是不测在家走路由器
		for p in "ruby" "ruby ruby-psych" "ruby rubygem-psych"; do
			ruby -ryaml -e 1 >/dev/null 2>&1 && return 0
			if command -v apt-get >/dev/null 2>&1; then apt-get install -y -qq --no-install-recommends $p
			elif command -v dnf >/dev/null 2>&1; then dnf install -y -q $p
			elif command -v apk >/dev/null 2>&1; then apk add -q $p
			elif command -v pacman >/dev/null 2>&1; then pacman -S --noconfirm --needed $p
			elif command -v zypper >/dev/null 2>&1; then zypper -q -n install $p; fi
		done >/dev/null 2>&1
		ruby -ryaml -e 1 >/dev/null 2>&1
	}
	for i in 1 2 3; do deps && break; sleep 5; done   # 十几个容器同时装，软件源偶尔失败一次（Amazon Linux 遇到过）
	ruby_dep
	command -v bash >/dev/null 2>&1 || { echo "  ✗ 装不上 bash"; echo "  结果：0 通过，1 失败"; exit 1; }
	command -v gzip >/dev/null 2>&1 || { echo "  ✗ 依赖没装齐（gzip）"; echo "  结果：0 通过，1 失败"; exit 1; }
	export CCA_LINUX_CONTAINER=1   # 在家走路由器只在容器里测
	exec bash /cca/tests/linux.sh --here
fi

[ "$(uname -s)" = Linux ] || { echo "只在 Linux 上跑（macOS 上用 tests/regress.sh）" >&2; exit 2; }
HB="${CCA_HELPER:-}"; unset CCA_HELPER   # 下面的 cca 都是接着辅助程序的那一份（像发布的一样自己解出来），不靠这个变量
if [ -z "$HB" ]; then
	HB="$R/cli/target/$HT/release/cca-core"
	[ -x "$HB" ] || helper_build || exit 2
fi
[ -x "$HB" ] || { echo "辅助程序不在：$HB" >&2; exit 2; }
# 测试自己用的两个程序：CCA_TEST_BIN、CCA_SUITE_BIN 指了就用指的（--docker 时宿主机编好、指给容器的也是它们），没指的编出来 / 找编好的。
# 写成完整的路径：假命令的脚本里要写它，那些脚本在哪个目录下跑都得找得到
tb="${CCA_TEST_BIN:-}"; sb="${CCA_SUITE_BIN:-}"; KT=""; KS=""
if [ -z "$tb" ] || [ -z "$sb" ]; then kit_build || exit 2; fi
KT="${tb:-$KT}"; KS="${sb:-$KS}"
case "$KT" in /*) ;; *) KT="$PWD/$KT" ;; esac
case "$KS" in /*) ;; *) KS="$PWD/$KS" ;; esac
[ -x "$KT" ] || { echo "测试用的小工具 cca-test 不在：$KT" >&2; exit 2; }
[ -x "$KS" ] || { echo "测试程序 suite 不在：$KS" >&2; exit 2; }
pass=0; fail=0
ok() { echo "  ✓ $*"; pass=$((pass + 1)); }
no() { echo "  ✗ $*"; fail=$((fail + 1)); }
for p in $(seq 7901 7916) 18080 18081; do
	if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then echo "端口 $p 已被占用（这台机器上有在用的 cca？）：不跑" >&2; exit 2; fi
done
D=$(mktemp -d "${TMPDIR:-/tmp}/cca-linux-test.XXXXXX")
H="$D/home"; mkdir -p "$H/.local/bin" "$H/.cache" "$D/bin" "$D/up" "$D/www"
cleanup() {   # 停掉这次起的进程（测试服务、内核、托管进程、上报进程、后台下载…：命令行或环境里带着 $D 的），删掉沙箱
	local p n h
	kill $(cat "$D/pids" 2>/dev/null) 2>/dev/null
	for h in "$H" "$D/shome" "$D/rhome"; do [ -f "$h/.cca/config.json" ] && HOME="$h" "$D/cca/cca" local off >/dev/null 2>&1; done
	for p in /proc/[0-9]*; do
		n=${p#/proc/}
		[ "$n" = $$ ] && continue
		{ tr '\0' '\n' < "$p/cmdline"; tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -qF "$D/" && kill "$n" 2>/dev/null
	done
	rm -rf "$D"
}
trap cleanup EXIT
# 假的 claude：auth login 照真的 Claude Code 走（本机 127.0.0.1 开回调、经 BROWSER 交出登录网址，state 是 $D/oauth_state，
# code 以 good 开头算换到了 token，凭证写成 600 的 .credentials.json），auth status / logout（凭证文件留给 cca 删），
# 别的当作会话：记下拿到的代理（$D/hold 在时一直开着）；--settings 里有 apiKeyHelper 时像 Claude Code 一样执行它，取到的 key 写进 $D/claude.key。
# 这里只是两行 sh：要做的都交给 cca-test 的角色 linux-claude（tests/tool/fake/linux_claude.rs）。"$0" 也带过去：进程的命令行里就有
# $D/bin/claude，下面读 /proc 认得出是它
{ echo '#!/bin/sh'; echo "exec '$KT' linux-claude '$D' \"\$0\" \"\$@\""; } > "$D/bin/claude"
# 假的 codex：--version / --help / features list 给 cca 探测参数；logout 删 auth.json；app-server（一行一个 JSON）照真的 codex 0.167
# 登录：本机 127.0.0.1 开回调（redirect_uri 写 localhost）、回登录网址，state 不对回 400 接着等，对的“换 token”（code 以 good 开头算
# 成功，写 auth.json，邮箱取 CCA_ACCOUNT 里的），再发 account/login/completed；account/rateLimits/read 回固定的用量。
# 别的当作会话：记下参数、代理和 CODEX_HOME（$D/hold 在时一直开着）。同样交给 cca-test 的角色 linux-codex（tests/tool/fake/linux_codex.rs）
{ echo '#!/bin/sh'; echo "exec '$KT' linux-codex '$D' \"\$0\" \"\$@\""; } > "$D/bin/codex"
# 假的 crontab：这次测试的用户 crontab 存在 $D/crontab 里（绝不碰这台机器上真的）
cat > "$D/bin/crontab" <<S
#!/bin/sh
case "\$1" in
	-l) [ -f "$D/crontab" ] || { echo "no crontab for \$(id -un)" >&2; exit 1; }; cat "$D/crontab" ;;
	-) cat > "$D/crontab" ;;
	*) echo "crontab: 测试里只支持 -l 和 -" >&2; exit 2 ;;
esac
S
chmod 755 "$D/bin/claude" "$D/bin/codex" "$D/bin/crontab"
# 假的 curl（包着真的）：cca 直连 claude.ai 这些的 /cdn-cgi/trace（direct_probe：服务器上能不能直连）由测试定，不看这台机器的网络：
# $D/direct_down 时连不上，$D/direct_loc 是出口地区（默认 US）。CCA_LINUX_CORE_CACHE（--docker 时宿主机下好的内核）：cca 下载内核时
# 从这里拿（照样校验 SHA256）。别的照常交给真的 curl
CORE_CACHE=""
if [ -n "${CCA_LINUX_CORE_CACHE:-}" ] && ls "$CCA_LINUX_CORE_CACHE"/mihomo-linux-*.gz >/dev/null 2>&1; then CORE_CACHE=$CCA_LINUX_CORE_CACHE; fi
{ echo '#!/bin/sh'; echo "D='$D' CORE_CACHE='$CORE_CACHE' REAL='$(command -v curl)'"; cat <<'S'; } > "$D/bin/curl"
out=""; url=""; prev=""; direct=""
for a; do
	[ "$prev" = -o ] && out=$a
	[ "$prev" = --proxy ] && [ -z "$a" ] && direct=1
	case "$a" in https://*) url=$a ;; esac
	prev=$a
done
case "$direct $url" in
	"1 https://claude.ai/cdn-cgi/trace"|"1 https://chatgpt.com/cdn-cgi/trace"|"1 https://www.cloudflare.com/cdn-cgi/trace")
		echo "CURL direct $url" >> "$D/curl.log"
		[ -f "$D/direct_down" ] && exit 7
		printf 'fl=1\nip=192.0.2.200\nloc=%s\n' "$(cat "$D/direct_loc" 2>/dev/null || echo US)"
		exit 0 ;;
esac
case "$url" in
	https://github.com/MetaCubeX/mihomo/releases/download/*)
		[ -n "$CORE_CACHE" ] && [ -n "$out" ] && [ -f "$CORE_CACHE/${url##*/}" ] && exec cp "$CORE_CACHE/${url##*/}" "$out" ;;
esac
exec "$REAL" "$@"
S
chmod 755 "$D/bin/curl"
export HOME="$H" PATH="$D/bin:$H/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" LANG=C.UTF-8 CCA_COST=off
unset CCA_HOME XDG_RUNTIME_DIR XDG_DATA_HOME XDG_CACHE_HOME DISPLAY WAYLAND_DISPLAY   # XDG_CACHE_HOME：辅助程序解在它里面，不清掉就解到沙箱外面去了
echo "  bash ${BASH_VERSION%%(*}，$(curl --version 2>/dev/null | head -n 1 | cut -d' ' -f1-2)，nc：$(readlink -f "$(command -v nc)" 2>/dev/null || echo 无)，ps：$(readlink -f "$(command -v ps)" 2>/dev/null || echo 无)，ruby：$(ruby -e 'print RUBY_VERSION' 2>/dev/null || echo 无)"
ans() { local f; f=$(mktemp "$D/ans.XXXXXX"); printf '%s\n' "$@" > "$f"; printf '%s' "$f"; }
fp() { printf '%s' "$1" | sha256sum | cut -c1-16; }   # 节点摘要（node_fp：类型|服务器|端口|TLS 域名 的 sha256 前 16 位）
FPA=$(fp 'socks5|127.0.0.1|18080|')
# kt 子命令 参数…：小工具 cca-test。读 JSON、写测试数据的那几处用它的求值器（check / val / edit / run，程序是 Python 的写法）和 fx；
# 它遇到不认识的写法退出码是 2，并在 $D/trap 里记一行——最后看这个文件是空的
kt() { CCA_TEST_TRAP="$D/trap" "$KT" "$@"; }

# 一键安装：install.sh 从 GitHub 下载 cca 和签名、验签、运行 cca install。GitHub 换成本机的文件（假 curl 只认 GitHub 上 cca 的发布），
# 发布密钥换成这次生成的（install.sh 和 cca 里的 RELEASE_KEY 一起换）；单独的 HOME，用 bash（PATH 该写进 ~/.bashrc）
I="$D/i"; mkdir -p "$I/gh" "$I/bin" "$I/home" "$I/tmp"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$I/relkey" </dev/null
pub=$(cut -d' ' -f1-2 "$I/relkey.pub")
# 像发布的那样接着辅助程序（cca-core pack：就是这一份辅助程序自己；这里只接这台机器的那一份）：第一次运行时解到 ~/.cache/cca-helper 里
"$HB" pack "$R/cca" "$I/cca.packed" --only "$HT=$HB" >/dev/null || { echo "  ✗ 辅助程序没接上（cca-core pack）"; echo "  结果：0 通过，1 失败"; exit 1; }
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$I/cca.packed" > "$I/gh/cca"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$R/install.sh" > "$I/install.sh"
ssh-keygen -q -Y sign -f "$I/relkey" -n cca-release "$I/gh/cca" </dev/null
# 下面用的 cca：同一份（认这次的发布密钥），旁边有 .git（开发模式：不自动更新，不去连 GitHub）
mkdir -p "$D/cca/.git"; C="$D/cca/cca"; cp "$I/gh/cca" "$C"; chmod 755 "$C"
# 假 GitHub：releases/latest 跳到 $I/gh/latest 里写的版本，releases/download/v版本/ 下是 $I/gh/v版本/ 里的文件
{ echo '#!/bin/sh'; echo "I='$I'"; cat <<'S'; } > "$I/bin/curl"
out=""; url=""; fmt=""
while [ $# -gt 0 ]; do case "$1" in -o) out=$2; shift ;; -w) fmt=$2; shift ;; https://*) url=$1 ;; esac; shift; done
G=https://github.com/rivia7/claude-accounts/releases
case "$url" in
	"$G/latest/download/cca"|"$G/latest/download/cca.sig") cp "$I/gh/${url##*/}" "$out" ;;
	"$G/latest") [ -s "$I/gh/latest" ] || exit 22; [ "$fmt" = '%{redirect_url}' ] && printf '%s' "$G/tag/v$(cat "$I/gh/latest")"; exit 0 ;;
	"$G/download/v"*/cca|"$G/download/v"*/cca.sig) v=${url#"$G/download/"}; f="$I/gh/${v%%/*}/${url##*/}"; [ -f "$f" ] || exit 22; cp "$f" "$out" ;;
	https://claude.ai/cdn-cgi/trace) printf 'fl=1\nip=192.0.2.200\nloc=US\n' ;;   # cca install 看能不能直连：能，照旧问要不要配置路由器
	*) exit 22 ;;
esac
S
chmod 755 "$I/bin/curl"
out=$(HOME="$I/home" TMPDIR="$I/tmp" SHELL=/bin/bash PATH="$I/bin:$PATH" CCA_TTY=$(ans y n n) bash "$I/install.sh" 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -q '签名校验通过' && [ "$(readlink "$I/home/.local/bin/cca")" = "$I/home/.cca/bin/cca" ] &&
	grep -qF 'export PATH="$HOME/.local/bin:$PATH"' "$I/home/.bashrc" 2>/dev/null && [ ! -e "$I/home/.bash_profile" ] &&
	printf '%s' "$out" | grep -qF '重新登录（或者运行 . ~/.bashrc）后，输入 cca 就能用' && ! ls -d "$I/tmp"/cca-install.* >/dev/null 2>&1; then
	ok "一键安装（install.sh）：下载、验签、装好、删掉下载用的临时目录；~/.local/bin 写进 ~/.bashrc（不新建 ~/.bash_profile，免得登录 shell 不再读 ~/.profile）"
else no "一键安装（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)$(ls -d "$I/tmp"/cca-install.* >/dev/null 2>&1 && echo '；下载用的临时目录没删')"; fi
# 缺命令（这里藏起 nc）：下载之前就停，给出这个发行版装它的那一行
mkdir -p "$I/nonc"
for f in /usr/local/bin/* /usr/bin/* /bin/*; do n=${f##*/}; [ "$n" = nc ] || [ -e "$I/nonc/$n" ] || ln -s "$f" "$I/nonc/$n"; done
out=$(HOME="$I/home2" PATH="$I/bin:$D/bin:$I/nonc" CCA_TTY=$(ans y n n) bash "$I/install.sh" 2>&1); rc=$?
if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q '缺少命令 nc，先装上再执行这条安装命令' &&
	printf '%s' "$out" | grep -qE '：(sudo )?((apt|dnf|yum|zypper) install (netcat-openbsd|nmap-ncat)|pacman -S openbsd-netcat|apk add netcat-openbsd)$' && [ ! -e "$I/home2/.cca" ]; then
	ok "一键安装：缺命令时说清楚，给出这个发行版装它的那一行（$(printf '%s' "$out" | sed -n 's/.*：//p' | tail -n 1)）"
else no "一键安装缺 nc（退出码 $rc）：$(printf '%s' "$out" | tail -n 2)"; fi

# 自动更新：一键安装装的那份（不是开发模式）。假 GitHub 上有新版本 9.9.9：每天一次的检查在后台（setsid）下载、验签，下一条命令换上
# 新版本（mv 换文件，~/.local/bin/cca 照样指着它）再接着跑；退回之后这个版本不再自动装
U="$I/home/.cca/bin/cca"; UPD="$I/home/.cca/update"
oldv=$(sed -n 's/^VERSION="\(.*\)"$/\1/p' "$U" 2>/dev/null)
mkdir -p "$I/gh/v9.9.9"
sed 's/^VERSION=".*"$/VERSION="9.9.9"/' "$I/gh/cca" > "$I/gh/v9.9.9/cca"
ssh-keygen -q -Y sign -f "$I/relkey" -n cca-release "$I/gh/v9.9.9/cca" </dev/null
echo 9.9.9 > "$I/gh/latest"
upd() { HOME="$I/home" TMPDIR="$I/tmp" PATH="$I/bin:$PATH" CCA_RETRY_WAIT=0 "$U" "$@"; }
rm -f "$UPD/checked"
upd ls >/dev/null 2>&1
for i in $(seq 1 80); do [ -f "$UPD/cca.next" ] && break; sleep 0.25; done
out=$(upd ls 2>&1)
if [ -n "$oldv" ] && printf '%s' "$out" | grep -qF "cca 已自动更新：${oldv} → 9.9.9" && grep -qx 'VERSION="9.9.9"' "$U" && [ -x "$U" ] &&
	[ "$(readlink "$I/home/.local/bin/cca")" = "$U" ]; then
	ok "自动更新：后台发现 9.9.9、下载验签，下一条命令换上新版本（${oldv} → 9.9.9）"
else no "自动更新：$(printf '%s' "$out" | tail -n 2)；后台：$(tail -n 2 "$UPD/log" 2>/dev/null | tr '\n' ' ')"; fi
out=$(upd update --rollback 2>&1)
rm -f "$UPD/checked"; upd ls >/dev/null 2>&1
for i in $(seq 1 80); do grep -q '被回退过' "$UPD/log" 2>/dev/null && break; sleep 0.25; done
if grep -qx "VERSION=\"$oldv\"" "$U" && [ "$(cat "$UPD/skip" 2>/dev/null)" = 9.9.9 ] && grep -q '9.9.9 被回退过，不再自动安装' "$UPD/log" 2>/dev/null && [ ! -e "$UPD/cca.next" ]; then
	ok "cca update --rollback：退回 ${oldv}，9.9.9 不再自动安装"
else no "cca update --rollback：$(printf '%s' "$out" | tail -n 2)；后台：$(tail -n 1 "$UPD/log" 2>/dev/null)"; fi

out=$(CCA_TTY=$(ans n n) "$C" install 2>&1)
if [ -f "$H/.cca/config.json" ]; then ok "cca install"; else no "cca install：$(printf '%s' "$out" | tail -n 3)"; echo "  结果：$pass 通过，$fail 失败"; exit 1; fi
if [ "$(readlink "$H/.local/bin/cxa")" = cca ] && [ "$(readlink "$H/.local/bin/aga")" = cca ]; then ok "cca install：建好简写 cxa、aga（等于 cca codex、cca agy），指向同目录的 cca"
else no "cca install 的简写：cxa → $(readlink "$H/.local/bin/cxa" 2>/dev/null)，aga → $(readlink "$H/.local/bin/aga" 2>/dev/null)"; fi
# 可选节点 US-A（本机的测试节点）、账号 alice 走它；内置代理要的节点定义直接写进 core/nodes.json（600）
kt edit --indent 1 --unicode c="$H/.cca/config.json" 'c["nodes"] = ["US-A"]' &&
	printf '{\n "alice(alice@example.com)": "US-A"\n}' > "$H/.cca/accounts.json" &&
	mkdir -p -m 700 "$H/.cca/core" &&
	printf '%s' "$(kt val --json '{"version": 1, "saved_at": time.time(), "nodes": ["US-A"], "proxies": [{"name": "US-A", "type": "socks5", "server": "127.0.0.1", "port": 18080}]}')" > "$H/.cca/core/nodes.json" &&
	chmod 600 "$H/.cca/core/nodes.json"

# 内置代理。用户自己的 crontab 里本来就有一行（cca 只动带 # cca-core 的那一行）
echo '0 3 * * * echo user-job' > "$D/crontab"
t0=$SECONDS
out=$(CCA_TTY=$(ans y y) "$C" local setup 2>&1)
if printf '%s' "$out" | grep -q '本机端口池已就绪'; then ok "cca local setup：下载、校验内核，后台托管，端口池就绪（$((SECONDS - t0)) 秒${CORE_CACHE:+；内核文件取自宿主机下好的那份}）"
else no "cca local setup：$(printf '%s' "$out" | tail -n 4)"; fi
boot=$(grep -F '# cca-core' "$D/crontab" 2>/dev/null)
if [ "$boot" = "@reboot $C _core-keep >/dev/null 2>&1 # cca-core" ] && grep -qx '0 3 \* \* \* echo user-job' "$D/crontab" &&
	printf '%s' "$out" | grep -q '已加上：机器重启后内置代理的内核自己起来'; then
	ok "开机起内核：问过后在用户的 crontab 里加一行 @reboot（原有的行不动；测试里是假的 crontab）"
else no "开机起内核：crontab 是「$(tr '\n' '|' < "$D/crontab" 2>/dev/null)」；$(printf '%s' "$out" | tail -n 2)"; fi
st=$("$C" local status 2>&1)
if printf '%s' "$st" | grep -q 'CCA-1  127.0.0.1:7901  监听中  → US-A'; then ok "cca local status：CCA-1 在监听、切到了账号的节点"
else no "cca local status：$(printf '%s' "$st" | head -n 5)"; fi

# 测试节点（本机另一个 mihomo，用 cca 刚下载的内核）和本机的 HTTP 服务：经账号的端口 → CCA-1 → US-A → DIRECT → 它
core=$(ls "$H"/.cca/core/mihomo-v* 2>/dev/null | head -n 1)
[ -x "$core" ] && cp "$core" "$D/mihomo"   # 后面服务器上不用路由器那一节、假路由器的内核也用它
echo '{"mixed-port": 18080, "bind-address": "127.0.0.1", "allow-lan": false, "mode": "direct", "log-level": "warning"}' > "$D/up/config.yaml"
[ -x "$D/mihomo" ] && { "$D/mihomo" -d "$D/up" -f "$D/up/config.yaml" > "$D/up.log" 2>&1 & echo $! >> "$D/pids"; }
echo cca-linux-ok > "$D/www/ok.txt"
"$KT" http-serve --dir "$D/www" --listen 127.0.0.1:18081 > /dev/null 2>&1 & echo $! >> "$D/pids"
get() { curl -q -s -m 5 --noproxy '' -x "http://${2:-127.0.0.1}:${1:-7901}" http://127.0.0.1:18081/ok.txt 2>/dev/null; }   # --noproxy ''：环境里的 no_proxy 不算数
listen() { (exec 3<>"/dev/tcp/${2:-127.0.0.1}/$1") 2>/dev/null; }
for i in $(seq 1 40); do [ "$(get)" = cca-linux-ok ] && break; sleep 0.25; done
if [ "$(get)" = cca-linux-ok ]; then ok "经账号的端口出去（CCA-1 → US-A）"; else no "经账号的端口出不去"; fi
if [ -z "$(get 7902)" ]; then ok "空槽位（CCA-2 → REJECT）拒绝连接"; else no "空槽位的端口居然出得去"; fi

running() {   # running 文字：有命令行里带着它的进程（读 /proc：Debian 的容器里没有 procps，没有 pgrep）
	local p c
	for p in /proc/[0-9]*; do
		c=$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null)
		case "$c" in *"$1"*) return 0 ;; esac
	done
	return 1
}
keeper() {   # keeper [HOME]：这次测试的托管进程（按 HOME 认：机器上可能还有别人的 cca）
	local p h="${1:-$H}"
	for p in /proc/[0-9]*; do
		{ tr '\0' ' ' < "$p/cmdline"; } 2>/dev/null | grep -q ' core-keep $' && { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -qxF "HOME=$h" && echo "${p#/proc/}"
	done
}
k=$(keeper); m=$(cat "$H/.cca/core/mihomo.pid" 2>/dev/null); n=$m
if [ -n "$k" ] && [ -n "$m" ]; then
	kill -9 "$m"
	for i in $(seq 1 60); do sleep 0.25; n=$(cat "$H/.cca/core/mihomo.pid" 2>/dev/null); [ "$n" != "$m" ] && [ "$(get)" = cca-linux-ok ] && break; done
	if [ "$n" != "$m" ] && [ "$(get)" = cca-linux-ok ]; then ok "内核被杀后托管进程把它拉起来了（$((i / 4)) 秒内）"; else no "内核被杀后没有拉起来"; fi
	sid=$(sed 's/.*) //' "/proc/$k/stat" 2>/dev/null | cut -d' ' -f4)   # /proc/<pid>/stat 的第 6 项（busybox 的 ps 不认 -p）
	if [ "$sid" = "$k" ]; then ok "托管进程自己一个会话（终端的 Ctrl-C、退出登录碰不到它）"; else no "托管进程不在自己的会话里（会话 $sid）"; fi
else
	no "没找到托管进程（${k:-无}）或内核（${m:-无}）"
fi
# 模拟重启：托管进程和内核都没了，cron 照 crontab 那一行、在它自己的环境里（只有 HOME、LOGNAME、SHELL、PATH=/usr/bin:/bin）运行
k=$(keeper); [ -z "$k" ] || kill $k 2>/dev/null
for i in $(seq 1 40); do [ -z "$(keeper)" ] && break; sleep 0.25; done
m=$(cat "$H/.cca/core/mihomo.pid" 2>/dev/null); [ -z "$m" ] || kill "$m" 2>/dev/null
for i in $(seq 1 40); do listen 7901 || break; sleep 0.25; done
down=1; listen 7901 && down=""
if [ -n "$boot" ]; then
	env -i HOME="$H" LOGNAME="$(id -un)" SHELL=/bin/sh PATH=/usr/bin:/bin sh -c "${boot#@reboot }" </dev/null >/dev/null 2>&1 &
	for i in $(seq 1 80); do [ "$(get)" = cca-linux-ok ] && break; sleep 0.25; done
fi
if [ -n "$down" ] && [ -n "$boot" ] && [ "$(get)" = cca-linux-ok ] && [ -n "$(keeper)" ]; then
	ok "模拟重启：cron 运行 crontab 里那一行（cron 的环境），托管进程起来、拉起内核，账号的端口又通了（$((i / 4)) 秒内）"
else no "模拟重启：停掉后端口$([ -n "$down" ] && echo 关了 || echo 还开着)，照 crontab 那一行起来后$([ "$(get)" = cca-linux-ok ] && echo 通了 || echo 不通)"; fi

# 登录：这台机器上没有账号专属的 Chrome。claude 照常在这里登录、开本机的回调，cca 给出在 Mac 上运行的一行（cca login-page …：带别名，
# 节点带上摘要，Mac 按地址认节点、没有这个账号时用它加上），贴回 Mac 上拿到的验证码（code#state），cca 核对过才转给本机的回调；
# 换 token 经账号的端口（假 claude 记下它拿到的代理）。出口检测（CCA_SKIP_CHECK=1）跳过：后面单独测
echo st-linux > "$D/oauth_state"
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans 'good-linux#st-other' 'good-linux#st-linux') "$C" login alice 2>&1)
if printf '%s' "$out" | grep -qF "cca login-page alice@example.com --alias alice --node US-A --node-id $FPA 'https://claude.com/cai/oauth/authorize?" &&
	printf '%s' "$out" | grep -q 'state 对不上' && grep -q 'EXCHANGE code=good-linux https_proxy=http://127.0.0.1:7901' "$D/claude.log" 2>/dev/null &&
	printf '%s' "$out" | grep -q '已重新登录'; then ok "cca login：给出在 Mac 上运行的一行（带别名、节点和节点摘要）；state 对不上的不转，对的转给本机的回调，经账号的端口换 token"
else no "cca login：$(printf '%s' "$out" | grep -F 'login-page' | head -n 1)$(printf '%s' "$out" | tail -n 3)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice hi 2>&1); rc=$?
if [ $rc -eq 0 ] && grep -q 'SESSION https_proxy=http://127.0.0.1:7901 args=.* hi$' "$D/claude.log" 2>/dev/null; then ok "cca @alice：启动账号，claude 拿到的代理是账号的端口"
else no "cca @alice（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)"; fi
cred=$(ls "$H"/.cca/accounts/alice-example-com-*/claude/.credentials.json 2>/dev/null)
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans '') "$C" login alice 2>&1)
sleep 0.5
if [ -n "$cred" ] && [ ! -e "$cred" ] && printf '%s' "$out" | grep -q '已取消登录' && ! running "$D/bin/claude auth login"; then
	ok "cca login 先退出：Claude Code 的凭证文件（.credentials.json）删掉；回车取消时本机等着回调的 claude 跟着停掉"
else no "退出登录 / 取消登录：凭证文件 ${cred:-没找到} $([ -e "$cred" ] && echo 还在)；$(printf '%s' "$out" | tail -n 2)"; fi
CCA_SKIP_CHECK=1 CCA_TTY=$(ans 'good-linux#st-linux') "$C" login alice >/dev/null 2>&1   # 登录回来（后面的会话、设备统计用）

# API key：没有钥匙串，存成 ~/.cca/secrets 下权限 600 的文件；第三方 API 账号启动时 apiKeyHelper 回调 cca 从文件里取
K1=sk-ant-api03-LINUXtest0123456789abcdWXYZ; K2=sk-test-LINUX2222
CCA_TTY=$(ans "$K1") "$C" key alice >/dev/null 2>&1
f=$(ls "$H"/.cca/secrets/cca-ref-alice-example-com-* 2>/dev/null)
if [ -n "$f" ] && [ "$(stat -c %a "$H/.cca/secrets")" = 700 ] && [ "$(stat -c %a "$f")" = 600 ] && [ "$("$C" key alice --show 2>/dev/null)" = "$K1" ]; then
	ok "cca key：存成 ~/.cca/secrets 下 600 的文件（目录 700），--show 读得出来"
else no "cca key：${f:-没有文件} $(stat -c %a "$f" 2>/dev/null)"; fi
out=$(CCA_TTY=$(ans kf http://127.0.0.1:18081/v1 "$K2" '' '') "$C" add api 2>&1)
rm -f "$D/claude.key"
out2=$(CCA_TTY=$(ans) "$C" @kf hi 2>&1); rc=$?
if [ $rc -eq 0 ] && [ "$(cat "$D/claude.key" 2>/dev/null)" = "$K2" ]; then ok "第三方 API 账号：key 在 600 的文件里，启动时 apiKeyHelper 回调 cca 取到"
else no "第三方 API 账号（退出码 $rc）：$(printf '%s\n%s' "$out" "$out2" | tail -n 4)"; fi
CCA_TTY=$(ans y) "$C" rm kf >/dev/null 2>&1
if [ -z "$(ls "$H"/.cca/secrets/cca-api-kf-* 2>/dev/null)" ]; then ok "cca rm：第三方 API 账号的 key 文件一起删掉"; else no "cca rm 之后 key 文件还在"; fi

# Team「书房组」：真的服务器（辅助程序的 team-server --mode team：127.0.0.1 上随机端口、openssl 生成的 P-256 证书，ttl 10 秒），管理员由
# cca-test spec 扮演（tests/kit/spec.rs：照 server/README.md 独立实现的那一头，不用 cca 的代码；签名用 ssh-keygen -Y）。后面服务器上不用路由器那一节
# 也用它（另一台设备）。没有 openssl 时生成不了服务器的证书：这几个用例跳过
TV="$D/team"; TVPORT=""; TID=""
team_put() {   # team_put 那串随机数 序号 'JSON' [签名的密钥]：管理员给那串随机数派生出来的设备存一版配置（accounts、aliases、nodes、install_node
	         # 照 JSON），序号是 1 时先发邀请；写了签名的密钥时拿它签（不是根密钥签的：像服务器被人冒充）。打印邀请链接
	kt spec put "$TV/root" "$TVPORT" "https://127.0.0.1:$TVPORT" "$TVPIN" "$1" "$2" 书房组 "${@:3}"
}
if command -v openssl >/dev/null 2>&1; then
	mkdir -p "$TV"
	openssl ecparam -name prime256v1 -genkey -noout -out "$TV/key.pem" 2>/dev/null
	openssl req -new -x509 -key "$TV/key.pem" -subj /CN=cca-team -days 2 -out "$TV/cert.pem" 2>/dev/null
	ssh-keygen -q -t ed25519 -N '' -C root -f "$TV/root" </dev/null; ssh-keygen -q -t ed25519 -N '' -C evil -f "$TV/evil" </dev/null
	"$HB" team-server --mode team --team-root "$(cut -d' ' -f1-2 "$TV/root.pub")" --listen 127.0.0.1:0 --port-file "$TV/port" \
		--cert "$TV/cert.pem" --key "$TV/key.pem" --state "$TV/state.json" --ttl 10 >> "$TV/log" 2>&1 & echo $! >> "$D/pids"
	for i in $(seq 1 50); do [ -s "$TV/port" ] && break; sleep 0.1; done
	TVPORT=$(cat "$TV/port" 2>/dev/null)
	TVPIN="sha256//$(openssl x509 -in "$TV/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)"
fi
if [ -n "$TVPORT" ]; then
	# 这台机器拿邀请链接加入（--stdin：链接是凭证，不写在命令行上），照 Team 的配置加上这台机器还没有的账号（bob，别名取配置里的）；
	# alice 这台机器上本来就有，Team 也给了它（节点一样）
	TS1=$(printf '1a%.0s' $(seq 1 16))
	TLINK=$(team_put "$TS1" 1 '{"accounts": {"claude": {"alice@example.com": "US-A", "bob@example.com": "US-A"}}, "aliases": {"bob@example.com": "bob"}}')
	out=$(printf '%s' "$TLINK" | "$C" team join --stdin --name linux-test 2>&1); rc=$?
	TID=$(ls "$H/.cca/teams" 2>/dev/null | grep -E '^[0-9a-f]{32}$')
	[ -z "$TID" ] || touch "$H/.cca/teams/checked"   # 不让启动时的后台同步（一个小时一次）插进来：同步都是测试自己做的
	if [ $rc -eq 0 ] && [ -n "$TLINK" ] && [ -n "$TID" ] && printf '%s' "$out" | grep -q "已加入「书房组」（127.0.0.1:${TVPORT}）：这台设备叫 linux-test" &&
		grep -q '"bob(bob@example.com)": "US-A"' "$H/.cca/accounts.json" && [ "$(stat -c %a "$H/.cca/teams/$TID/doc.json" 2>/dev/null)" = 600 ] &&
		! printf '%s' "$out" | grep -q '路由器' && ! printf '%s' "$out" | grep -qF "$TLINK"; then
		ok "cca team join --stdin：加入 Team，照它的配置加上 bob（别名取配置里的；没有路由器，不提同步路由器）；本机存的配置只有自己能读（600）"
	else no "cca team join（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)；账号表：$(tr -d '\n' < "$H/.cca/accounts.json")"; fi
	team_put "$TS1" 2 '{"accounts": {"claude": {"alice@example.com": "US-A"}}}' "$TV/evil" >/dev/null
	out=$("$C" team sync 2>&1); rc=$?
	if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q '书房组：配置的签名不是这个 Team 的根密钥签的（可能有人冒充服务器）：不用它' &&
		grep -q '"bob(bob@example.com)": "US-A"' "$H/.cca/accounts.json"; then
		ok "不是 Team 的根密钥签的一版（像服务器被人冒充）：cca team sync 说出来（退出码非 0），不用它，接着用手里的那一版"
	else no "签名不对的一版（退出码 $rc）：$(printf '%s' "$out" | tail -n 2)"; fi
	team_put "$TS1" 3 '{"accounts": {"claude": {"alice@example.com": "US-A", "bob@example.com": "US-A"}}, "aliases": {"bob@example.com": "bob"}}' >/dev/null
	"$C" team sync >/dev/null 2>&1
fi

# 设备统计（presence）：Team 给的账号开着会话时 cca 在后台起上报进程，报给这个 Team 的服务器（Linux 上没有 kqueue：每 5 秒看一次）；
# 会话的登记（cli-<进程号>.json）、进程的启动时间从 /proc 读（busybox 的 ps 不认 -p、-x、lstart）
if [ -n "$TID" ]; then
	dstate() { "$C" devices alice@example.com --json 2>/dev/null | kt val -f d=- '",".join("%s:%s:%d" % (x["name"], x["state"], x["cli"]) for x in d["accounts"][0]["devices"])' 2>/dev/null; }
	AD=$(ls -d "$H"/.cca/accounts/alice-example-com-* 2>/dev/null | head -n 1)
	touch "$D/hold"
	CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice hi >/dev/null 2>&1 & S1=$!
	for i in $(seq 1 40); do [ -f "$AD/cli-$S1.json" ] && break; sleep 0.25; done
	mk=$(kt val -f j="$AD/cli-$S1.json" '"%s:%s" % (j["pid"], j["tool"])' 2>/dev/null)
	# 一秒左右问一次：服务器给每台设备限了流（每秒 0.5 个请求、最多攒 30 个），问得太勤上报进程就报不上去了
	for i in $(seq 1 60); do [ "$(dstate)" = linux-test:in_use:1 ] && break; sleep 0.8; done
	st1=$(dstate)
	ap=$(kt val -f j="$H/.cca/presence/agent.json" 'j["pid"]' 2>/dev/null)
	if [ "$mk" = "$S1:claude" ] && [ "$st1" = linux-test:in_use:1 ] && [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; then
		ok "设备统计：Team 给的账号会话开着时登记 cli-<进程号>.json（启动时间从 /proc 读），后台的上报进程报给 Team 的服务器：在用、1 个会话"
	else no "设备统计：登记 ${mk:-没有}，服务器上是「${st1:-空}」，上报进程 ${ap:-没有}；$(tail -n 2 "$TV/log" 2>/dev/null | tr '\n' ' ')$(cat "$H/.cca/presence/team.$TID.status.json" 2>/dev/null)"; fi
	rm -f "$D/hold"; wait "$S1" 2>/dev/null
	for i in $(seq 1 80); do [ "$(dstate)" = linux-test:none:0 ] && break; sleep 0.8; done
	st2=$(dstate)
	for i in $(seq 1 40); do { [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; } || break; sleep 0.25; done
	if [ "$st2" = linux-test:none:0 ] && [ ! -e "$AD/cli-$S1.json" ] && ! { [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; }; then
		ok "会话关了：登记删掉，上报进程几秒内报“没在用”后自己退出"
	else no "会话关了之后：服务器上是「${st2:-空}」，登记$([ -e "$AD/cli-$S1.json" ] && echo 还在 || echo 删了)，上报进程$({ [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; } && echo 还在 || echo 退出了)"; fi
elif [ -n "$TVPORT" ]; then
	no "设备统计：没加入成 Team，测不了"
else
	echo "  （跳过 Team 和设备统计：没有 openssl，生成不了服务器的证书）"
fi

# suite_run 模块：跑测试程序 suite 的一个模块（tests/<模块>.rs：把辅助程序当黑盒，自己的临时 HOME，不碰上面的沙箱）。服务器是辅助程序里的
# 那一份；--setsid：自己一个会话，没有控制终端（在终端里跑、经 ssh 跑、在容器里跑都一样）。跑了、用例都过了才
# 返回 0，SN 是通过的用例数（有跳过的话后面带一句）；别的都算没过，SW 说哪里不对——suite 不认识这个模块（还没有这份测试）、一个
# 用例都没跑也在里面：不能悄悄当成过了（tests/regress.sh 的 suite_ok 是同一个标准：那一行是 pass、通过数大于 0、失败数是 0）。结果和记录在 $D/suite-<模块>/
suite_run() {
	local m="$1" d="$D/suite-$1" rc name st p f s secs got=""
	SN=""; SW=""
	mkdir -p "$d"
	# 放到后台再等它：suite 自己一个会话，终端上按 Ctrl-C 的信号到不了它；bash 等前台的命令时要等它跑完才理这个信号，等后台的（wait）
	# 马上就退出，退出时的清理再把它停掉（它的命令行里带着沙箱的路径）
	CCA_HELPER="$HB" CCA_TEST_BIN="$KT" CCA_REPO="$R" "$KS" --setsid --module "$m" --results "$d/results.tsv" --logs "$d/logs" > "$d/out" 2>&1 </dev/null &
	wait $!; rc=$?
	[ ! -f "$d/results.tsv" ] || while IFS="$(printf '\t')" read -r name st p f s secs; do
		[ "$name" = "$m" ] && got="$st $p $f $s"
	done < "$d/results.tsv"
	set -- $got
	if [ $rc -eq 0 ] && [ "${1:-}" = pass ] && [ "${2:-0}" -gt 0 ] && [ "${3:-1}" -eq 0 ]; then
		SN="$2 个用例"; [ "${4:-0}" -eq 0 ] || SN="${SN}，另有 $4 个跳过"
		return 0
	fi
	SW=$(grep -E '^(FAIL|TIMEOUT|NOTRUN) ' "$d/logs/$m.log" 2>/dev/null | head -n 4 | cut -d' ' -f1-2 | tr '\n' ' ' | sed 's/ $//')
	[ -n "$SW" ] || SW=$(tail -n 3 "$d/out" | tr '\n' ' ' | sed 's/ $//')
	SW="退出码 ${rc}，${got:-没有这个模块的结果}：${SW}"
	return 1
}
suite_more() {   # suite_more 模块：没过的用例连同它们的输出（最多 80 行；没有记录时是 suite 最后说的那几行）。沙箱退出时就删了，要看的打在这里
	local d="$D/suite-$1" t
	t=$(grep -E -A 40 '^(FAIL|TIMEOUT|NOTRUN) ' "$d/logs/$1.log" 2>/dev/null | grep -E '^(FAIL |TIMEOUT |NOTRUN |    )' | head -n 80)
	[ -n "$t" ] || t=$(tail -n 15 "$d/out" 2>/dev/null)
	[ -z "$t" ] || printf '%s\n' "$t" | sed 's/^/      /'
}
# 远程会话（docs/dev/remote.md）：会话宿主和接入端、两台机器配对、经服务器中转接入。suite 的两个模块把辅助程序当黑盒跑
# （真的伪终端、真的服务器进程、每台设备一个临时 HOME 和自己在 Team 里的密钥），不碰上面的沙箱。只在宿主机上跑：容器里各发行版的
# ps、pgrep 不一样（busybox 的 ps 没有 -p），还没在那一套上试过
if [ -n "${CCA_LINUX_CONTAINER:-}" ]; then
	echo "  （容器里不跑远程会话的测试）"
elif command -v openssl >/dev/null 2>&1 && command -v ssh-keygen >/dev/null 2>&1 && command -v pgrep >/dev/null 2>&1; then
	for rt in remote_host remote_relay; do
		if suite_run "$rt"; then ok "远程会话：tests/$rt.rs（${SN}）"
		else no "远程会话：tests/$rt.rs：$SW"; suite_more "$rt"; fi
	done
else
	echo "  （跳过远程会话的测试：要 openssl、ssh-keygen、pgrep）"
fi

# 折合金额：共享的会话记录里的用量按官方 API 的标价折算，按登录的身份认是谁的（Linux 上没有 st_birthtime：按修改时间排；
# 进程的启动时间从 /proc 读）。会话结束后在后台统计一次（CCA_COST 在这一步清掉：别处是 off）；cca cost sync --json 是 App 用的那一条
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-* 2>/dev/null | head -n 1)
# alice 登录的身份（accountUuid）记进账号的 .claude.json；共享的会话记录里放一份它的会话：1 小时前 claude-opus-5-5 进 1000 出 100（fx linux-costlog）
kt edit j="$AD/claude/.claude.json" 'j.setdefault("oauthAccount", {})["accountUuid"] = "u-linux-alice-01"' && kt fx linux-costlog "$H"
CCA_COST= CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice hi >/dev/null 2>&1
for i in $(seq 1 40); do [ -f "$H/.cca/cost/local.json" ] && break; sleep 0.25; done
kicked=$([ -f "$H/.cca/cost/last" ] && [ -f "$H/.cca/cost/local.json" ] && echo yes)
out=$("$C" cost sync --json 2>/dev/null); rc=$?
got=$(kt val -f j="$H/.cca/cost/local.json" '"%s|%s" % (j["who"].get("claude:u-linux-alice-01"), sum(r[0] for r in j["acc"]["claude:u-linux-alice-01"].values()))' 2>/dev/null)
if [ $rc -eq 0 ] && [ "$kicked" = yes ] && [ "$got" = "alice@example.com|6000" ] && [ "$(stat -c %a "$H/.cca/cost")" = 700 ] && [ "$(stat -c %a "$H/.cca/cost/local.json")" = 600 ] &&
	printf '%s' "$out" | kt check -f d=- 'd["kind"] == "cost_sync" and d["scan"]["state"] == "ready"'; then
	ok "折合金额：会话结束后在后台统计（cca _cost-sync），按登录的身份认出是 alice 的（折合 0.006 美元）；cca cost sync --json 照样能跑；只给自己读"
else no "折合金额（退出码 $rc，后台统计${kicked:-没起}）：${got:-没统计出来}；$(printf '%s' "$out" | tail -c 300)"; fi

# Codex：添加账号（服务器上没有路由器、只配置了内置代理：节点从可选节点里选，不用先配置路由器）、服务器上登录（login-page 那一行
# 带别名、节点和节点摘要；换 token 经账号的端口）、启动（关掉会换出口的开关，经账号的端口）、查用量（经账号的端口）、cxa 是 cca codex 的简写
out=$(CCA_TTY=$(ans carol@example.com carol 1) "$C" codex add 2>&1); rc=$?
CP=$(kt val -f c="$H/.cca/config.json" 'list(c["codex"]["ports"].values())[0]' 2>/dev/null)
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -q '已添加 carol' && ! printf '%s' "$out" | grep -qE '先配置路由器|还没配置路由器|路由器同步没完成' &&
	[ "$(readlink "$H/.local/bin/cxa")" = cca ] && [ -n "$CP" ]; then
	ok "cca codex add：没有路由器也能加（节点从可选节点里选，不提路由器），端口 ${CP}，建好 cxa"
else no "cca codex add（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans 'good-codex#st-other' 'good-codex#st-linux') "$C" codex login carol 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -qF "cca codex login-page carol@example.com --alias carol --node US-A --node-id $FPA 'https://auth.openai.com/oauth/authorize?" &&
	grep -q "EXCHANGE code=good-codex https_proxy=http://127.0.0.1:$CP" "$D/codex.log" 2>/dev/null && printf '%s' "$out" | grep -q '已重新登录'; then
	ok "cca codex login：给出 cca codex login-page 那一行，验证码核对 state 后转给 app-server 的回调，经账号的端口换 token"
else no "cca codex login（退出码 $rc）：$(printf '%s' "$out" | grep -F 'login-page' | head -n 1)$(printf '%s' "$out" | tail -n 3)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" codex @carol hi 2>&1); rc=$?
l=$(grep '^SESSION ' "$D/codex.log" 2>/dev/null | tail -n 1)
if [ $rc -eq 0 ] && printf '%s' "$l" | grep -qF "https_proxy=http://127.0.0.1:$CP home=$H/.cca/codex/accounts/carol-example-com-" &&
	printf '%s' "$l" | grep -qF -- '--no-daemon' && printf '%s' "$l" | grep -qF -- '--disable system_proxy_fallback' &&
	printf '%s' "$l" | grep -qF -- '--disable respect_system_proxy' && printf '%s' "$l" | grep -q ' hi$'; then
	ok "cca codex @carol：经账号的端口启动，CODEX_HOME 是账号自己的，关掉 system_proxy_fallback、respect_system_proxy，不起后台服务"
else no "cca codex @carol（退出码 $rc）：${l:-没启动}；$(printf '%s' "$out" | tail -n 2)"; fi
out=$(CCA_SKIP_CHECK=1 COLUMNS=160 "$C" codex usage 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -qE 'carol.* 13% .* 41% ' && grep -q "USAGE email=carol@example.com https_proxy=http://127.0.0.1:$CP" "$D/codex.log"; then
	ok "cca codex usage：经账号的端口让 app-server 查，表里有 5 小时、本周"
else no "cca codex usage（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi
# 查用量途中按 Ctrl-C、关掉终端（TERM）：后台的查询忽略 SIGINT，cca 自己停掉它们和它们起的进程，不等查询超时。
# Debian 的容器没有 procps（没有 pkill）：走读 /proc 的那条路
queries() {   # 这次测试里还在跑的用量查询：辅助程序的 usage-query 和它起的 app-server（按 HOME 认）
	local p
	for p in /proc/[0-9]*; do
		{ tr '\0' ' ' < "$p/cmdline"; } 2>/dev/null | grep -qE 'usage-query|app-server' &&
			{ tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -qxF "HOME=$H" && echo "${p#/proc/}"
	done
}
alive() {   # alive 进程号…：还没退出的（退出了、只剩僵尸的 cmdline 是空的）。只看这几个：机器上进程多时（上千个）扫一遍 /proc 要好几秒
	local k
	for k in "$@"; do [ -n "$({ tr -d '\0' < "/proc/$k/cmdline"; } 2>/dev/null)" ] && printf '%s ' "$k"; done
}
touch "$D/usage_hang"; n=$(grep -c '^USAGE ' "$D/codex.log")
CCA_SKIP_CHECK=1 CCA_USAGE_TIMEOUT=60 "$C" codex usage >/dev/null 2>&1 & up=$!
for i in $(seq 1 100); do [ "$(grep -c '^USAGE ' "$D/codex.log")" -gt "$n" ] && break; sleep 0.1; done
q=$(queries | tr '\n' ' '); t0=$(date +%s)
kill -TERM "$up"; wait "$up"; rc=$?; t1=$(date +%s)
for i in $(seq 1 30); do [ -z "$(alive $q)" ] && break; sleep 0.1; done; left=$(alive $q)
rm -f "$D/usage_hang"
if [ "$rc" = 130 ] && [ -n "$q" ] && [ -z "$left" ] && [ $((t1 - t0)) -lt 5 ]; then
	ok "查用量途中按 Ctrl-C、关掉终端：后台的查询连同它起的 app-server 马上停掉（不等 60 秒的超时；$(command -v pkill >/dev/null 2>&1 && echo 'pkill' || echo '没有 pkill，读 /proc')）"
else no "查用量途中 Ctrl-C（退出码 $rc，$((t1 - t0)) 秒）：查询的进程 ${q:-没找到}，还在的 ${left:-没有}"; kill $left 2>/dev/null; fi
out=$("$H/.local/bin/cxa" ls 2>&1)
if printf '%s' "$out" | grep -q 'carol' && ! printf '%s' "$out" | grep -q 'alice' && [ "$(get "$CP")" = cca-linux-ok ]; then ok "cxa ls：列的是 Codex 账号；carol 的端口也通"
else no "cxa ls：$(printf '%s' "$out" | head -n 4)"; fi

# 真实的出口和泄露检测：经账号的端口（内置代理 → US-A → 测试节点直连出去）查出口、纯净度，DNS 泄露经账号的端口测；服务器上没有账号专属的
# 浏览器和桌面 App，不测 WebRTC。结果随这台机器的网络变（出口可能在不能用的地区、检测服务可能连不上），只核对测了、记下了、没说 WebRTC
out=$("$C" check alice 2>&1); rc=$?
hf=$(ls "$H"/.cca/accounts/alice-example-com-*/health.json 2>/dev/null | head -n 1)
hv=$(kt run -f h="$hf" 'lk = h.get("leak"); good = (h.get("source") == "check" and h.get("route") == "local" and h.get("node") == "US-A" and h.get("port") == 7901 and h.get("result") in ("ok", "low_purity", "restricted", "no_exit", "no_score") and (lk is None or (lk.get("webrtc") is None and lk.get("dns") in ("ok", "leak", "failed") and "WebRTC" not in (lk.get("line") or "")))); print("%s %s：%s · %s；DNS %s" % ("good" if good else "bad", h.get("result"), h.get("ip") or "没拿到 IP", h.get("country") or "?", (lk or {}).get("dns") or "没测（出口不对时不等）"))' 2>/dev/null)
if [ $rc -eq 0 ] && [ "${hv%% *}" = good ] && ! printf '%s' "$out" | grep -q 'WebRTC' && printf '%s' "$out" | grep -q '^  alice  US-A  :7901  '; then
	ok "cca check（真的联网）：经账号的端口测出口和 DNS 泄露，记进 health.json，不说 WebRTC（${hv#* }）"
else no "cca check（退出码 $rc）：${hv:-没有 health.json}；$(printf '%s' "$out" | tail -n 3)"; fi

# 只在 macOS 上的命令：说清楚（不说"没装 Chrome""要 macOS 26"这些），cca help 不列它们
bad=""
for c in web billing limit-resets app gui mail; do
	out=$("$C" "$c" 2>&1) && bad="$bad $c（退出码 0）"
	printf '%s' "$out" | grep -q '只在 macOS 上；这台 Linux 上用命令行' || bad="$bad $c：$(printf '%s' "$out" | tail -n 1)"
done
if [ -z "$bad" ]; then ok "cca web、billing、limit-resets、app、gui、mail：说只在 macOS 上"; else no "只在 macOS 上的命令：$bad"; fi
out=$("$C" help 2>&1)
if ! printf '%s' "$out" | grep -qE '^  cca (web|billing|limit-resets|app|gui|login-page|mail) |^  cca codex app ' &&
	printf '%s' "$out" | grep -q '只在 macOS 上' && printf '%s' "$out" | grep -q '由 cca 在后台运行' && printf '%s' "$out" | grep -q '^  cca usage ' &&
	! printf '%s' "$out" | grep -q 'WebRTC'; then
	ok "cca help：不列只在 macOS 上的命令，内置代理说的是由 cca 在后台运行，泄露检测不提 WebRTC"
else no "cca help：$(printf '%s' "$out" | grep -E '^  cca (web|billing|limit-resets|app|gui|login-page|mail) |^  cca codex app |WebRTC' | head -n 2)"; fi

# 辅助程序：发布的 cca 第一次运行时把这台机器的那一份解到 ~/.cache/cca-helper 里（XDG_CACHE_HOME 指了别处就在那里；不放在
# XDG_RUNTIME_DIR：那是内存盘，退出登录就清空），文件名是它的 SHA-256，只有自己能读写执行；解新的一份时顺手删掉 30 天前解出来的
# （旧版本的、没解完的临时文件），30 天内的留着；以后每次直接用，不再解。不用 python3（PATH 里藏起它照样能跑）
HS=$(sha256sum "$HB" | cut -d' ' -f1)
HCD="$D/xdgc/cca-helper"; mkdir -p -m 700 "$D/xdgc" "$HCD"
for n in aaaa1 aaaa2.4242 bbbb1; do echo x > "$HCD/$n"; done
touch -d '32 days ago' "$HCD/aaaa1" 2>/dev/null || touch -d "@$(( $(date +%s) - 32 * 86400 ))" "$HCD/aaaa1"
touch -d '90 days ago' "$HCD/aaaa2.4242" 2>/dev/null || touch -d "@$(( $(date +%s) - 90 * 86400 ))" "$HCD/aaaa2.4242"
touch -d '28 days ago' "$HCD/bbbb1" 2>/dev/null || touch -d "@$(( $(date +%s) - 28 * 86400 ))" "$HCD/bbbb1"
XDG_CACHE_HOME="$D/xdgc" XDG_RUNTIME_DIR="$D/xdgr" "$C" ls >/dev/null 2>&1; l1=$(ls "$HCD" | LC_ALL=C sort | tr '\n' ' ')
i1=$(stat -c '%i %Y' "$HCD/$HS" 2>/dev/null)
XDG_CACHE_HOME="$D/xdgc" XDG_RUNTIME_DIR="$D/xdgr" "$C" ls >/dev/null 2>&1; l2=$(ls "$HCD" | LC_ALL=C sort | tr '\n' ' ')
if [ "$l1" = "$(printf '%s\n' "$HS" bbbb1 | LC_ALL=C sort | tr '\n' ' ')" ] && [ "$l1" = "$l2" ] && [ -n "$i1" ] && [ "$(stat -c '%i %Y' "$HCD/$HS")" = "$i1" ] &&
	[ "$(stat -c %a "$HCD/$HS")" = 700 ] && [ "$(stat -c %a "$HCD")" = 700 ] && [ "$(sha256sum "$HCD/$HS" | cut -d' ' -f1)" = "$HS" ] && [ ! -e "$D/xdgr" ]; then
	ok "辅助程序：第一次运行解到缓存目录的 cca-helper 里（文件名是 SHA-256，只有自己能读写执行），删掉 30 天前的，30 天内的留着；第二次不再解"
else no "辅助程序的缓存：第一次后 ${l1:-空的}；第二次后 ${l2:-空的}；$(ls -la "$HCD" 2>&1 | tail -n 4 | tr '\n' ' ')"; fi
if [ -x "$H/.cache/cca-helper/$HS" ] && [ ! -e "$H/.cache/cca-py" ]; then ok "辅助程序：平时解在 ~/.cache/cca-helper 里"
else no "辅助程序不在 ~/.cache/cca-helper 里：$(ls -la "$H/.cache" 2>&1 | tail -n 5 | tr '\n' ' ')"; fi
mkdir -p "$D/nopy"
for f in /usr/local/bin/* /usr/bin/* /bin/* /usr/sbin/* /sbin/*; do n=${f##*/}; case "$n" in python*|pypy*) ;; *) [ -e "$D/nopy/$n" ] || ln -s "$f" "$D/nopy/$n" ;; esac; done
out=$(PATH="$D/nopy" "$C" ls 2>&1); rc=$?
if [ $rc -eq 0 ] && [ -n "$out" ] && ! PATH="$D/nopy" command -v python3 >/dev/null 2>&1; then ok "cca 不用 python3：PATH 里没有它，cca ls 照样列出账号"
else no "PATH 里没有 python3 时 cca ls（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi

out=$("$C" local off 2>&1)
for i in $(seq 1 20); do [ -z "$(keeper)" ] && ! listen 7901 && break; sleep 0.25; done
if [ -z "$(keeper)" ] && ! listen 7901 && ! grep -qF '# cca-core' "$D/crontab" && grep -qx '0 3 \* \* \* echo user-job' "$D/crontab"; then
	ok "cca local off：托管进程退出、内核停了，crontab 里开机起内核的那一行删掉（别的行留着）"
else no "cca local off：托管进程${k:+/内核}$([ -n "$(keeper)" ] && echo 还在)；crontab 是「$(tr '\n' '|' < "$D/crontab" 2>/dev/null)」"; fi
grep -iE 'level=(error|fatal)|panic' "$H/.cca/core/core.log" 2>/dev/null | head -n 2 | sed 's/^/  内核日志：/'

# cca restore：先打包备份配置（GNU / busybox 的 tar；登录凭证、密钥文件、参考节点不进包里），再删掉 ~/.cca、~/.local/bin 里的 cca、cxa 和 aga、
# crontab 里开机起内核的那一行（别的行留着）。没有 tar 的（Amazon Linux 的最小安装）：先停下，什么都不动
mkdir -p "$H/.cca/secrets" "$H/.cca/accounts/orphan-example-com/claude"
echo '{"v": 1, "host": "api.example.com", "key": "sk-LEFTOVER-SECRET"}' > "$H/.cca/secrets/cca-api-orphan"
echo '{"claudeAiOauth": {"accessToken": "cred-LEFTOVER-SECRET"}}' > "$H/.cca/accounts/orphan-example-com/claude/.credentials.json"
printf '%s\n' "@reboot '$C' _core-keep >/dev/null 2>&1 # cca-core" >> "$D/crontab"
# 3.6.0 以前拉过维护者的映射、升级以后还没运行过别的命令（第一条走默认那一支的命令就删掉 ~/.cca/policy；cca uninstall 不走那一支）：
# 参考节点里有节点密钥（连同拉取映射时留下的临时文件），也不进备份；映射本身照样打包
mkdir -p "$H/.cca/policy/tmp.Ab12cd/repo"; echo '{}' > "$H/.cca/policy/policy.json"
for f in nodes.json nodes.json.4242 tmp.Ab12cd/repo/nodes.json; do
	printf '%s\n' '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-Z", "type": "ss", "server": "192.0.2.99", "port": 8388, "cipher": "aes-128-gcm", "password": "node-LEFTOVER-SECRET"}]}' > "$H/.cca/policy/$f"
done
# 完全卸载（cca uninstall，cca restore 是同一条命令）：~/.cca 以外的也清掉——缓存目录里解出来的辅助程序（整个 cca-helper）、3.5.0 以前
# 留下的 cca-py，安装时写进 ~/.bashrc 的那两行（~/.local/bin 里还有别的命令时留着；怎么去掉的各种情况在 tests/path_unhint.sh）
mkdir -p "$H/.cache/cca-py"; : > "$H/.cache/cca-py/old.pyc"
printf 'export FOO=1\n\n# cca（Claude Code 多账号）\nexport PATH="$HOME/.local/bin:$PATH"\n' > "$H/.bashrc"; cp "$H/.bashrc" "$D/bashrc.before"
out=$(CCA_TTY=$(ans uninstall) "$C" uninstall 2>&1); rc=$?
bk=$(ls "$H"/cca-backup-*.tar.gz 2>/dev/null | head -n 1)
if ! command -v tar >/dev/null 2>&1; then
	if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q '这台机器上没有 tar，没法先打包备份配置，什么都没改' && [ -f "$H/.cca/config.json" ] && [ -z "$bk" ]; then
		ok "cca restore：没有 tar 时先停下（没法先打包备份），什么都不动"
	else no "cca restore 没有 tar（退出码 $rc）：$(printf '%s' "$out" | tail -n 2)"; fi
elif [ $rc -eq 0 ] && [ -n "$bk" ] && [ "$(stat -c %a "$bk")" = 600 ] && printf '%s' "$out" | grep -qF "配置备份：$bk" &&
	tar -tzf "$bk" | grep -q 'accounts.json' && tar -tzf "$bk" | grep -q 'orphan-example-com/claude' &&
	! tar -tzf "$bk" | grep -qE '\.credentials\.json|secrets|id_ed25519$|policy/(nodes\.json|tmp\.)' && tar -tzf "$bk" | grep -q 'policy/policy\.json' &&
	! tar -xzOf "$bk" 2>/dev/null | grep -q 'LEFTOVER-SECRET' &&
	[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ ! -L "$H/.local/bin/cxa" ] && [ ! -L "$H/.local/bin/aga" ] &&
	! grep -qF '# cca-core' "$D/crontab" && grep -qx '0 3 \* \* \* echo user-job' "$D/crontab"; then
	ok "cca restore：先打包备份（600，登录凭证、密钥文件、参考节点不在里面），再删掉 ~/.cca、cca、cxa、aga 和 crontab 里开机起内核的那一行（别的行留着）"
else no "cca restore（退出码 $rc）：$(printf '%s' "$out" | tail -n 2)；备份：${bk:-没有} $(tar -tzf "$bk" 2>/dev/null | grep -E 'credentials|secrets|id_ed25519|policy/' | head -n 3 | tr '\n' ' ')"; fi
if ! command -v tar >/dev/null 2>&1; then   # 没有 tar：不留备份就能卸载
	out=$(CCA_TTY=$(ans uninstall) "$C" uninstall --no-backup 2>&1); rc=$?
	if [ $rc -eq 0 ] && [ ! -e "$H/.cca" ] && [ -z "$(ls "$H"/cca-backup-*.tar.gz 2>/dev/null)" ] && printf '%s' "$out" | grep -qF '没有留配置备份（--no-backup）'; then
		ok "cca uninstall --no-backup：没有 tar 也能卸载，不留备份"
	else no "cca uninstall --no-backup（退出码 ${rc}）：$(printf '%s' "$out" | tail -n 2)"; fi
fi
if [ -e "$H/.cca" ]; then no "卸载以后 ~/.cca 还在：缓存目录、~/.bashrc 那两条没法看"
else
	if [ ! -e "$H/.cache/cca-helper" ] && [ ! -e "$H/.cache/cca-py" ]; then ok "cca uninstall：缓存目录里解出来的辅助程序（整个 cca-helper）、以前留下的 cca-py 都删掉"
	else no "卸载以后缓存目录里还有：$(ls -A "$H/.cache" 2>&1 | tr '\n' ' ')"; fi
	if [ -e "$H/.local/bin" ]; then   # 里面还有别的命令：那两行留着
		if [ "$(cat "$H/.bashrc")" = "$(cat "$D/bashrc.before")" ] && printf '%s' "$out" | grep -qF '那两行留着'; then
			ok "cca uninstall：~/.local/bin 里还有别的命令，~/.bashrc 里 cca 加的那两行留着、说一声"
		else no "卸载以后 ~/.bashrc（~/.local/bin 里还有：$(ls -A "$H/.local/bin" | tr '\n' ' ')）：$(tr '\n' '|' < "$H/.bashrc")"; fi
	elif [ "$(cat "$H/.bashrc")" = "export FOO=1" ]; then
		ok "cca uninstall：~/.local/bin 空了，目录删掉，~/.bashrc 里 cca 加的那两行（连同前面的空行）去掉"
	else no "卸载以后 ~/.bashrc：$(tr '\n' '|' < "$H/.bashrc")"; fi
fi

# 服务器上不用路由器：全新的 HOME，从来没配置过路由器、直连不通（假 curl：$D/direct_down）时，cca install 不问路由器，改用内置代理，
# 先问有没有 Team 的邀请链接。一键安装时带着 CCA_POLICY_REPO（3.6.0 以前打开维护者的共享）：只说一声去掉了。先回答没有、也不贴代理：
# 先跳过。再运行 cca install、贴上 Team 的邀请链接（上面「书房组」的服务器，另一台设备）：加入，导入 Team 的配置指定的节点（install_node、
# 给这台设备的账号的节点，只要配置带的节点里有），照配置加上账号，内置代理由 cca _core-keep 托管（问过后在 crontab 里加开机那一行），
# 经账号的端口出去。内核文件用前面下好的那份（不再下载一次）
SH="$D/shome"; mkdir -p "$SH/.local/bin"
sr() { HOME="$SH" PATH="$SH/.local/bin:$PATH" "$@"; }   # ~/.local/bin 在 PATH 里：不问要不要写进 ~/.bashrc
if [ -x "$D/mihomo" ]; then mkdir -p -m 700 "$SH/.cca" "$SH/.cca/core" && cp "$D/mihomo" "$SH/.cca/core/mihomo-v$(sed -n 's/^CORE_VERSION="\(.*\)"$/\1/p' "$C")"; fi
touch "$D/direct_down"; : > "$D/curl.log"
out=$(sr env CCA_TTY="$(ans n)" CCA_POLICY_REPO=example/cca-policy "$C" install 2>&1); rc=$?
if [ $rc -eq 0 ] && grep -q '^CURL direct https://claude.ai/cdn-cgi/trace' "$D/curl.log" &&
	printf '%s' "$out" | grep -q '直连 claude.ai 不通：这台机器不用配置路由器，改用 cca 内置的代理' &&
	printf '%s' "$out" | grep -q '维护者的映射 3.6.0 起去掉了（CCA_POLICY_REPO 不再起作用）' && printf '%s' "$out" | grep -q '有没有 Team 的邀请链接？' &&
	printf '%s' "$out" | grep -q '再运行 cca install 接着配置' && [ ! -e "$SH/.cca/policy" ] &&
	! printf '%s' "$out" | grep -qE '现在配置路由器|新增第一个账号' && [ -z "$(keeper "$SH")" ]; then
	ok "服务器上直连不通：不问路由器，先问有没有 Team 的邀请链接；带着 CCA_POLICY_REPO 只说一声去掉了（不生成 deploy key）；没有链接、也没贴代理时先跳过，说清楚之后再运行什么"
else no "服务器上直连不通（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)"; fi
if [ -n "$TVPORT" ]; then
	TS2=$(printf '2b%.0s' $(seq 1 16))
	SLINK=$(team_put "$TS2" 1 '{"accounts": {"claude": {"sam@example.com": "US-S"}}, "aliases": {"sam@example.com": "sam"}, "install_node": "US-S", "nodes": {"group": "OpenAi", "proxies": [{"name": "US-S", "type": "socks5", "server": "127.0.0.1", "port": 18080}, {"name": "US-X", "type": "ss", "server": "192.0.2.83", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}]}}')
	out=$(sr env CCA_TTY="$(ans y "$SLINK" y)" "$C" install 2>&1); rc=$?
	for i in $(seq 1 40); do [ "$(get)" = cca-linux-ok ] && break; sleep 0.25; done
	boot=$(grep -F '# cca-core' "$D/crontab" 2>/dev/null)
	if [ $rc -eq 0 ] && [ -n "$SLINK" ] && printf '%s' "$out" | grep -q '已加入「书房组」' && printf '%s' "$out" | grep -qF '已导入 Team 的配置带的节点：US-S（这台机器的可选节点）' &&
		kt check -f c="$SH/.cca/config.json" 'c["nodes"] == ["US-S"] and c["node_ids"].get("US-S") == argv[0] and c["local"]["client"] == "builtin"' "$FPA" 2>/dev/null &&
		grep -q '"sam(sam@example.com)": "US-S"' "$SH/.cca/accounts.json" && printf '%s' "$out" | grep -q '本机端口池已就绪：内置代理 127.0.0.1:7901' &&
		[ "$boot" = "@reboot $C _core-keep >/dev/null 2>&1 # cca-core" ] && [ -n "$(keeper "$SH")" ] && [ "$(get)" = cca-linux-ok ] &&
		! printf '%s' "$out" | grep -qE '现在配置路由器|新增第一个账号|没找到 Claude Code' && ! printf '%s' "$out" | grep -qF "$SLINK"; then
		ok "再运行 cca install、贴上 Team 的邀请链接：加入，导入 Team 的配置指定的节点（配置带的别的节点不导入），照配置加上 sam，内置代理由 cca _core-keep 托管（问过后加开机那一行），经账号的端口出去"
	else no "服务器上加入 Team 的 cca install（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)；crontab 是「$(tr '\n' '|' < "$D/crontab" 2>/dev/null)」"; fi
	sr "$C" local off >/dev/null 2>&1
	for i in $(seq 1 40); do [ -z "$(keeper "$SH")" ] && ! listen 7901 && break; sleep 0.25; done
else
	echo "  （跳过服务器上加入 Team：没有 openssl，起不了 Team 的服务器）"
fi
# 没有 Team 的邀请链接（默认）：直连不通时粘贴一条自己的代理（本机测试节点的 socks5），真的内核校验过当可选节点，配置好内置代理；没加入 Team
SH2="$D/shome2"; mkdir -p "$SH2/.local/bin"
if [ -x "$D/mihomo" ]; then mkdir -p -m 700 "$SH2/.cca" "$SH2/.cca/core" && cp "$D/mihomo" "$SH2/.cca/core/mihomo-v$(sed -n 's/^CORE_VERSION="\(.*\)"$/\1/p' "$C")"; fi
out=$(HOME="$SH2" PATH="$SH2/.local/bin:$PATH" CCA_TTY="$(ans n 'socks5://127.0.0.1:18080' . n n n)" "$C" install 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -q '有没有 Team 的邀请链接？' && printf '%s' "$out" | grep -q '已加上你的代理：' &&
	kt check -f c="$SH2/.cca/config.json" 'len(c["nodes"]) == 1 and c["local"]["client"] == "builtin"' 2>/dev/null &&
	[ -z "$(ls "$SH2/.cca/teams" 2>/dev/null)" ]; then
	ok "服务器上直连不通、没有 Team 的邀请链接：粘贴一条自己的代理（内核校验过）当可选节点，配置好内置代理；没加入 Team"
else no "服务器上直连不通、贴自己的代理（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)"; fi
HOME="$SH2" PATH="$SH2/.local/bin:$PATH" "$C" local off >/dev/null 2>&1
for i in $(seq 1 40); do [ -z "$(keeper "$SH2")" ] && ! listen 7901 && break; sleep 0.25; done
rm -f "$D/direct_down"

# 在家走路由器：假路由器（127.0.0.2）上跑真的 cca-router.sh / apply.rb（sh 是这个发行版的 /bin/sh），内核是真的 mihomo（当作 OpenClash
# 的 clash，-d /etc/openclash），uci、pidof、netstat 是假的（netstat 读 /proc/net/tcp），假 ssh 把命令交给它；SSH 端口（12222）只是
# 开着让 cca 认出在家。全新的 HOME：cca install 里配置路由器、选节点、加第一个账号、问过后重载，经路由器的端口池出去
router_test() {
	local RH="$D/rhome" RT="$D/router" RB="$D/rbin" SB="$D/sshbin" out rc i l
	mkdir -p "$RH/.local/bin" "$RT/etc/openclash/config" "$RT/etc/openclash/custom" "$RT/etc/init.d" "$RT/etc/crontabs" "$RT/etc/dropbear" \
		"$RT/root/.ssh" "$RT/tmp" "$RB" "$SB"
	cat > "$RT/etc/openclash/config/custom.yaml" <<'Y'
allow-lan: true
mode: rule
log-level: warning
external-controller: 127.0.0.1:19390
secret: test-secret
proxies:
  - {name: US-A, type: socks5, server: 127.0.0.1, port: 18080}
  - {name: US-B, type: socks5, server: 127.0.0.1, port: 18082}
proxy-groups:
  - {name: Proxy, type: select, proxies: [US-A, US-B]}
rules:
  - MATCH,Proxy
Y
	cp "$RT/etc/openclash/config/custom.yaml" "$RT/etc/openclash/custom.yaml"
	printf '%s\n' '#!/bin/sh' '. /usr/share/openclash/ruby.sh' '. /usr/share/openclash/log.sh' '' 'LOG_OUT "Tip: Start Running Custom Overwrite Scripts..."' \
		'LOG_FILE="/tmp/openclash.log"' 'CONFIG_FILE="$1"' '' 'exit 0' > "$RT/etc/openclash/custom/openclash_custom_overwrite.sh"
	printf '/root/.ssh/\n' > "$RT/etc/sysupgrade.conf"
	cat > "$RB/uci" <<'S'
#!/bin/sh
case "$*" in
	*openclash.config.config_path*) echo /etc/openclash/config/custom.yaml ;;
	*openclash.config.cn_port*) echo 19390 ;;
	*openclash.config.dashboard_password*) echo test-secret ;;
	*openclash.config.enable*) echo 1 ;;
	*) exit 1 ;;
esac
S
	printf '#!/bin/sh\n[ "$1" = clash ] || exit 1\np=$(cat "%s/clash.pid" 2>/dev/null) && [ -n "$p" ] && kill -0 "$p" 2>/dev/null && echo "$p"\n' "$RT" > "$RB/pidof"
	# 假的 netstat：每个在监听的端口一行（cca-test 的角色 netstat 读内核的表，见 tests/tool/fake/linux_netstat.rs）；路由器端给它的参数不看
	{ echo '#!/bin/sh'; echo "exec '$KT' netstat /proc/net/tcp /proc/net/tcp6 -- \"\$@\""; } > "$RB/netstat"
	cat > "$RT/etc/init.d/openclash" <<S
#!/bin/sh
[ "\$1" = restart ] || exit 0
cp "$RT/etc/openclash/config/custom.yaml" "$RT/etc/openclash/custom.yaml"
CCA_ROOT="$RT" sh "$RT/etc/cca/cca-router.sh" hook "$RT/etc/openclash/custom.yaml"
curl -q -s --noproxy '*' -X PUT -H 'Authorization: Bearer test-secret' -d '{"path": "$RT/etc/openclash/custom.yaml"}' 'http://127.0.0.1:19390/configs?force=true' >/dev/null
S
	{ echo '#!/bin/bash'; echo "D='$D' RT='$RT' RB='$RB'"; cat <<'S'; } > "$SB/ssh"
[ "${1:-}" = -V ] && { echo "OpenSSH_9.9p1, OpenSSL 3.0" >&2; exit 0; }
args=("$@"); i=0; n=${#args[@]}
while [ $i -lt $n ]; do
	case "${args[$i]}" in
		-O) exit 0 ;;
		-i|-p|-o|-l|-F|-c|-m|-b|-e|-L|-R|-D|-W|-S|-J) i=$((i + 2)) ;;
		-*) i=$((i + 1)) ;;
		*) break ;;
	esac
done
dest="${args[$i]:-}"; i=$((i + 1)); cmd="${args[*]:$i}"
echo "SSH $dest :: ${cmd:0:60}" >> "$D/ssh.log"
[ "$dest" = root@127.0.0.2 ] || { echo "ssh: connect to host ${dest#*@} port 22: Connection refused" >&2; exit 255; }
[ -n "$cmd" ] || exit 0
exec env CCA_ROOT="$RT" HOME="$RT/root" PATH="$RB:$PATH" sh -c "$cmd"
S
	chmod 755 "$RB"/* "$RT/etc/init.d/openclash" "$SB/ssh"
	cp "$D/mihomo" "$RT/etc/openclash/clash"
	( exec "$RT/etc/openclash/clash" -d "$RT/etc/openclash" -f "$RT/etc/openclash/custom.yaml" > "$D/router-core.log" 2>&1 ) &
	echo $! > "$RT/clash.pid"; echo $! >> "$D/pids"
	"$KT" accept-close 127.0.0.2:12222 & echo $! >> "$D/pids"
	for i in $(seq 1 40); do curl -q -s -m 1 --noproxy '*' -H 'Authorization: Bearer test-secret' http://127.0.0.1:19390/version | grep -q version && listen 12222 127.0.0.2 && break; sleep 0.25; done
	rr() { HOME="$RH" PATH="$SB:$RH/.local/bin:$PATH" "$@"; }   # ~/.local/bin 在 PATH 里：cca install 不问要不要写进 ~/.bashrc（一键安装那里测过）
	out=$(rr env CCA_TTY="$(ans y 127.0.0.2 '' 12222 '1 2' y alice@example.com alice 1 y)" "$C" install 2>&1); rc=$?
	for i in $(seq 1 40); do [ "$(get 7901 127.0.0.2)" = cca-linux-ok ] && break; sleep 0.25; done
	if [ $rc -eq 0 ] && grep -q '"host": "127.0.0.2"' "$RH/.cca/config.json" && grep -q '"alice(alice@example.com)": "US-A"' "$RH/.cca/accounts.json" &&
		grep -qF 'cca-router.sh ensure' "$RT/etc/crontabs/root" && grep -qF '# >>> cca >>>' "$RT/etc/openclash/custom/openclash_custom_overwrite.sh" &&
		[ "$(get 7901 127.0.0.2)" = cca-linux-ok ]; then
		ok "在家走路由器：cca install 里配置路由器（假 ssh → 真的 cca-router.sh）、选节点、加账号，问过后重载，经路由器的端口池出去"
	else no "在家走路由器（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)；路由器日志：$(tail -n 2 "$RT/tmp/cca-router.log" 2>/dev/null | tr '\n' ' ')"; fi
	out=$(rr "$C" router status 2>&1)
	if printf '%s' "$out" | grep -q '^OpenClash：运行中' && printf '%s' "$out" | grep -q '^槽位 CCA-1 :7901 监听中 → US-A$'; then ok "cca router status：OpenClash 运行中，槽位 CCA-1 在监听、选的是 US-A"
	else no "cca router status：$(printf '%s' "$out" | head -n 8 | tr '\n' '|')"; fi
	# 这个 HOME 里 alice 还没登录：启动时先登录（服务器上的那一套，验证码贴回来），再启动
	: > "$D/claude.log"
	out=$(rr env CCA_SKIP_CHECK=1 CCA_TTY="$(ans 'good-home#st-linux')" "$C" @alice hi 2>&1); rc=$?
	l=$(grep '^SESSION ' "$D/claude.log" | tail -n 1)
	for i in $(seq 1 60); do [ -s "$RH/.cca/core/nodes.json" ] && break; sleep 0.25; done
	if [ $rc -eq 0 ] && grep -q 'EXCHANGE code=good-home https_proxy=http://127.0.0.2:7901$' "$D/claude.log" && printf '%s' "$l" | grep -q 'https_proxy=http://127.0.0.2:7901 '; then
		ok "在家启动账号：认出在家（nc 连得上路由器的 SSH 端口），登录时换 token、启动的 claude 都经路由器的端口"
	else no "在家启动账号（退出码 $rc）：${l:-没有启动 claude}；$(grep '^EXCHANGE' "$D/claude.log" | tail -n 1)；$(printf '%s' "$out" | tail -n 3)"; fi
	if kt check -f j="$RH/.cca/core/nodes.json" 'n = {p["name"]: p for p in j["proxies"]}; n["US-A"]["port"] == 18080 and n["US-B"]["port"] == 18082' 2>/dev/null &&
		[ "$(stat -c %a "$RH/.cca/core/nodes.json")" = 600 ]; then
		ok "在家时在后台给内置代理拷节点：路由器端自己写 JSON（这台机器不用 ruby），存成 600 的 core/nodes.json"
	else no "给内置代理拷节点：$(head -c 200 "$RH/.cca/core/nodes.json" 2>/dev/null || echo 没有 core/nodes.json)"; fi
}
if [ -z "${CCA_LINUX_CONTAINER:-}" ]; then
	echo "  （在家走路由器只在容器里测：假路由器的端口池监听 0.0.0.0。用 bash tests/linux.sh --docker）"
elif ! ruby -ryaml -e 1 >/dev/null 2>&1; then
	echo "  （跳过在家走路由器：装不上 ruby，假路由器上的 cca 要它（真的路由器上 OpenClash 自带））"
elif [ ! -x "$D/mihomo" ]; then
	no "在家走路由器：没有内核文件（内置代理那一步没下载成？）"
else
	router_test
fi
# Team 的同步服务器的一键安装脚本（suite 的 hub_deploy 模块，假服务器是 tests/kit/hubbox.rs）：这个发行版的 sh、openssl、gzip、coreutils / busybox，
# 安装脚本带着的服务端程序（两种机器的都是这一份辅助程序，下面的 HB）解出来、校验、试运行，假的 systemctl、docker 起的就是它
# （app/cca-core team-server；设备的密钥要 ssh-keygen；gzip 和 cca 自己要的 gunzip 是同一个包）
if ! command -v openssl >/dev/null 2>&1 || ! command -v ssh-keygen >/dev/null 2>&1; then
	echo "  （跳过 Team 的同步服务器的安装脚本：要 openssl、ssh-keygen）"
elif suite_run hub_deploy; then
	ok "Team 的同步服务器的安装脚本：在这个发行版的 sh（$(readlink -f /bin/sh 2>/dev/null || echo /bin/sh)）上装好、更新、卸载，网页端的证书助手，经 SSH 部署、接上（${SN}）"
else
	no "Team 的同步服务器的安装脚本：$SW"; suite_more hub_deploy
fi
# 小工具遇到过不认识的写法（退出码 2 的那种：上面哪一条用例里的判断其实没做成）：算失败，说出是哪一句
[ ! -s "$D/trap" ] || no "cca-test 遇到了不认识的写法：$(head -n 3 "$D/trap" | tr '\n\t' '| ')"
[ ! -s "$D/print-mode.log" ] || no "cca 用了打印模式或者非交互模式（claude -p、codex exec 这些：cca 不用，也不让经 cca 用）：$(head -n 2 "$D/print-mode.log" | tr '\n' '|')"
echo "  结果：$pass 通过，$fail 失败"
[ $fail -eq 0 ]
