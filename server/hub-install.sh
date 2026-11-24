#!/bin/sh
# cca 的 Team 的同步服务器（cca team）的一键安装：在 Linux 服务器上用 root 运行，Docker compose（compose 插件）或者 systemd + 系统的 python3（3.6 起）。
#
#   sudo sh cca-hub-install.sh --team-root 'ssh-ed25519 AAAA…' [--host 公网地址] [--port 端口] [--relay-port 端口 | --no-relay]
#                              [--web-port 端口 | --no-web] [--docker | --systemd] [--dir 目录] [--name Team的名字]
#       根公钥是 cca team new 给的那一行（公开的）。每个 Team 一个服务：服务名、系统用户、容器名是 cca-team-<Team 编号的前 8 位>，
#       装在 /opt/<服务名>，端口没写时从 18445 起找一个空着的——所以一台服务器上能装几个 Team，互不相干。装好了打出管理员接上它的那一行
#       （cca team attach …）。再运行（带 --team-root，或者 --dir 那个目录）是更新：数据、证书、端口都不变
#   sudo sh cca-hub-install.sh --uninstall --dir 目录    停掉、删掉那一个 Team 的（连同数据）
#
# 同时给这个 Team 的远程会话（cca remote，docs/dev/remote.md）做中转：另开一个数据端口（普通的 TCP，上面是两台设备之间端到端加密的内容），
# 没写时从 HTTPS 那个端口的下一个起找一个空着的；--relay-port 端口 指定，--no-relay 不开。
# 还有手机的网页端（在手机的浏览器里接入远程会话）：再开一个端口，没写时从数据端口的下一个起找一个空着的（--web-port 指定，--no-web 不开），
# 用浏览器信得过的证书：证书助手（cca-acme.py）向 Let's Encrypt 要（给 IP 地址的是约 6 天的短期证书），到期前自己续。证书机构验证时
# 从公网连这台服务器的 80 端口，80 端口只有一个：这台服务器上的几个 Team 共用一个证书助手（服务 cca-team-acme，装在 /opt/cca-team-acme：
# 第一个开网页端的 Team 装上它，别的 Team 在它那里登记自己的地址；同一个地址只要一张证书；最后一个 Team 卸载时连同它一起删掉）。
# 80 端口被别的程序占着、地址是内网的，这一次就不开（说一声，再运行时再看）。--name：网页的标题、手机主屏幕上的名字带上 Team 的名字。
# 选的都记在 .env 里，再运行照旧。
#
# 也可以在管理员的电脑上运行 cca team deploy Team root@服务器：经 SSH 装上这一份，装好了自动接上。
# 目录里：app/ 服务端程序、证书助手（都只用 Python 标准库）、网页端的文件和 Team 的名字（name），certs/ 自签的证书（私钥不出这台服务器），
# data/ 设备和密文，.env 运行方式、端口、根公钥，.cca-hub 记着这个目录是它装的。证书助手的目录里：teams/ 用它的 Team 登记的地址，
# store/ 每个地址的证书，webcert/<服务名>/ 放给那个 Team 的（只有它读得了私钥），acme/ 证书机构那里的账户的私钥。
# 不改 Docker 的全局配置，不动防火墙（要放行哪些端口最后会说）。
# 3.5.3 以前这一份还装「自己的同步服务器」（cca hub，/opt/cca-hub）：没有了，改成建一个 Team。以前装的用 --uninstall --dir /opt/cca-hub 卸掉。
#
# 这一份是 cca 仓库里的模板（server/hub-install.sh）：辅助程序在下面标着 CCA-HUB-EMBED 的那一行放进版本、服务端程序、证书助手和网页端的
# 文件，才是能运行的安装脚本（cca team deploy --script 打出来的、发布页上的 cca-hub-install.sh）。格式见 server/README.md「Team 的同步服务器」
# @CCA-HUB-EMBED@

set -u
umask 022

ROOT=${CCA_HUB_ROOT:-}     # 只给测试用（tests/kit/hubbox.rs）：所有文件都放在这个目录下面
ANYIP=${CCA_HUB_ANY_IP:-}  # 只给测试用：地址是内网的也开网页端（测试里的证书助手是假的）
CERT_WAIT=${CCA_HUB_CERT_WAIT:-120}   # 等网页端的证书最多几秒（测试里调短）
D=""                       # 装在哪：--dir，没写时 /opt/<服务名>（team_setup）
SVC="" UNIT=""             # 服务名（systemd 的服务名和系统用户、Docker 的容器名：cca-team-<Team 编号的前 8 位>）和它的 unit 文件（team_setup）
WHAT="Team 的同步服务器"   # 说话时怎么叫它
# Team 的根公钥（cca team deploy 放进的，或者 --team-root；这里只留 ssh-ed25519 后面那一段，没有空格好写进 .env）
TEAMROOT=${CCA_HUB_TEAM_ROOT:-}
# cca team deploy 没写 --host 时放进的 ssh -G 的主机名：服务器上没记着地址（第一次装、搬过来的）时才用它，更新时不拿它盖掉记着的
HOSTHINT=${CCA_HUB_HOST_HINT:-}
DIRSET=""                  # 命令里写了 --dir
NAME=""                    # Team 的名字（--name；cca team deploy 放进的）：写进 app/name，网页的标题、手机主屏幕上的名字用它
CUID=10001                 # 容器里运行服务端、证书助手的用户（compose.yaml 的 user）
DEF_PORT=18445
# 这台服务器上几个 Team 共用的证书助手：服务名（systemd 的服务名和系统用户、Docker 的容器名和 compose 的项目名）、目录（固定的：
# 每个 Team 的安装脚本都要找得到它）。3.5.3-beta.4 以前每个 Team 自己一个（<服务名>-acme，占着 80 端口：只有一个 Team 开得了网页端）
ASVC=cca-team-acme
AD=/opt/$ASVC
AMODE="" AIMG=""           # 证书助手怎么运行、用哪个镜像（acme_mode）
# 官方的 python 镜像：上次用的那个，再依次试官方镜像的几个同步源（国内常常连不上 Docker Hub），最后是 Docker Hub 上的
IMAGES="public.ecr.aws/docker/library/python:3.13-slim mirror.ccs.tencentyun.com/library/python:3.13-slim docker.m.daocloud.io/library/python:3.13-slim python:3.13-slim"
# cca team deploy 经 SSH 运行时（上面那一行放进了 CCA_HUB_MANAGED=1、根公钥和参数）：进度写 stderr，结果一行一项（CCA-HUB 键 值）写 stdout
MANAGED=${CCA_HUB_MANAGED:-}
VERSION=${CCA_HUB_VERSION:-}
ROLLBACK=""                # 这一次写过 .env 了：没装完时换回原来的（write_env、rollback）

say() { if [ -n "$MANAGED" ]; then printf '  %s\n' "$*" >&2; else printf '%s\n' "$*"; fi; }
note() { [ -z "$MANAGED" ] || printf 'CCA-HUB %s\n' "$*"; }
die() {   # die 原因码 说明（原因码见 cli/src/cmd/hub/deploy.rs 的 fail_code）：.env 先换回原来的
	rollback
	code=$1
	shift
	if [ -n "$MANAGED" ]; then printf 'CCA-HUB fail %s %s\n' "$code" "$*"; else printf '✗ %s\n' "$*" >&2; fi
	exit 1
}
interrupted() {   # Ctrl-C、SSH 断了（之后再往断了的管道里写是 SIGPIPE）：服务还没起来的话 .env 换回原来的，就退出，不再写什么
	trap '' HUP INT TERM PIPE
	rollback
	exit 1
}
usage() {
	cat <<'EOF'
用法: sudo sh cca-hub-install.sh --team-root 'ssh-ed25519 AAAA…' [--host 公网地址] [--port 端口] [--relay-port 端口 | --no-relay]
                                 [--web-port 端口 | --no-web] [--docker | --systemd] [--dir 目录] [--name Team的名字]
      sudo sh cca-hub-install.sh --dir 目录 [选项…]      更新装在这个目录里的那一个
      sudo sh cca-hub-install.sh --uninstall --dir 目录
EOF
}
tmo() { if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi; }   # busybox、coreutils 都有 timeout；没有就不限时
env_get() { [ -f "$ROOT$D/.env" ] && sed -n "s/^$1=//p" "$ROOT$D/.env" | tail -n 1; }
hexof() { od -An -tx1 | tr -d ' \n'; }
pin_of() {   # 证书公钥的指纹（curl --pinnedpubkey 的写法去掉开头的 sha256//）：从证书文件读，或者从 stdin（s_client 的输出）读
	openssl x509 ${1:+-in "$1"} -pubkey -noout 2>/dev/null | openssl pkey -pubin -outform der 2>/dev/null |
		openssl dgst -sha256 -binary 2>/dev/null | openssl base64 -A 2>/dev/null
}
served_pin() { tmo 8 openssl s_client -connect "127.0.0.1:${1:-$PORT}" </dev/null 2>/dev/null | pin_of; }   # 这个端口（没写是 PORT）上给的证书
install_cmd() {   # 这个发行版上装这些包的那一行
	for pm in apt-get dnf yum zypper apk; do command -v "$pm" >/dev/null 2>&1 && break; pm=""; done
	case "$pm" in
		apt-get) printf 'apt-get install -y %s' "$*" ;;
		dnf|yum|zypper) printf '%s install -y %s' "$pm" "$*" ;;
		apk) printf 'apk add %s' "$*" ;;
		*) printf '用系统的包管理器装 %s' "$*" ;;
	esac
}
# Docker：只用 compose 插件（docker compose）。一些发行版的 apt 里的 docker-compose 1.x 认不得 compose.yaml，重建容器时还会出错
DCP=""   # compose 的项目名照服务名（-p）。不写的话是目录名：两个 Team 装在同名的目录里（/a/hub、/b/hub）会互相当成孤儿删掉
dc() { (cd "$ROOT$D" && docker compose $DCP -f compose.yaml "$@") </dev/null; }
have_docker() { command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 && docker info >/dev/null 2>&1; }
PY=""
have_systemd() {   # systemd 在管这台机器（/run/systemd/system 在），有 python3 3.6 起、带 ssl
	[ -d "$ROOT/run/systemd/system" ] && command -v systemctl >/dev/null 2>&1 || return 1
	PY=$(command -v python3 2>/dev/null) || return 1
	"$PY" -c 'import sys, ssl, selectors, socketserver, http.server; sys.exit(0 if sys.version_info >= (3, 6) else 1)' >/dev/null 2>&1
}
running_as() {   # running_as docker|systemd 名字：这个容器、这个 systemd 服务在运行
	case "$1" in
		docker) [ "$(docker inspect -f '{{.State.Running}}' "$2" 2>/dev/null)" = true ] ;;
		systemd) systemctl is-active --quiet "$2" 2>/dev/null ;;
		*) return 1 ;;
	esac
}
svc_running() { running_as "$OLD" "${1:-$SVC}"; }   # 上次装的那个服务（${1}，没写是 ${SVC}；以前它自己的证书助手是 ${SVC}-acme）在运行
aenv_get() { [ -f "$ROOT$AD/.env" ] && sed -n "s/^$1=//p" "$ROOT$AD/.env" | tail -n 1; }   # 共用的证书助手的设置
acme_running() { running_as "$(aenv_get CCA_HUB_MODE)" "$ASVC"; }
old_acme() {   # 80 端口上是另一个 Team 用以前的安装脚本装的、它自己的证书助手（<服务名>-acme）：打出它的名字
	for e in "$ROOT"/opt/cca-team-*/.env; do
		[ -f "$e" ] && [ "${e%/.env}" != "$ROOT$D" ] || continue
		s=$(sed -n 's/^# svc=\(cca-team-[0-9a-f]*\)$/\1/p' "${e%/.env}/.cca-hub" 2>/dev/null | head -n 1)
		[ -n "$s" ] && running_as "$(sed -n 's/^CCA_HUB_MODE=//p' "$e" | tail -n 1)" "$s-acme" && { printf '%s' "$s-acme"; return 0; }
	done
	return 1
}
port_busy() {   # 有程序在这个 TCP 端口上监听（ss、netstat 都没有时当作没有）
	if command -v ss >/dev/null 2>&1; then ss -ltn 2>/dev/null
	elif command -v netstat >/dev/null 2>&1; then netstat -ltn 2>/dev/null
	fi | awk '{ print $4 }' | grep -Eq "[:.]$1\$"
}
port_taken() {   # 自动挑端口时：有程序在听，或者这台服务器上别的同步服务器（默认目录里的）记着用它（没有 ss、netstat 时只看得到这个）
	port_busy "$1" && return 0
	# 先接起来再找：有的文件不在时 grep 的退出码是 2（找到了也是），接起来就只看找没找到
	cat "$ROOT"/opt/cca-team-*/.env "$ROOT/opt/cca-hub/.env" 2>/dev/null | grep -Eqx "CCA_HUB_(PORT|RELAY|WEB)=$1"
}
lan_host() {   # 只在内网里连得上的地址：私有地址、本机、链路本地、运营商级 NAT（100.64.0.0/10），IPv6 的 ::1、fc00::/7、fe80::/10
	case "$1" in
		localhost|0.*|10.*|127.*|169.254.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*) return 0 ;;
		100.6[4-9].*|100.[7-9][0-9].*|100.1[01][0-9].*|100.12[0-7].*) return 0 ;;
		::1|[Ff][CcDd]*:*|[Ff][Ee][89AaBb]*:*) return 0 ;;
	esac
	return 1
}
public_ip() {   # 默认路由出去的源地址，是公网地址才认（只查路由表，不发请求；云服务器的公网 IP 常常是 NAT 进来的，机器上看不到）
	ip=$(ip -4 route get 192.0.2.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
	[ -z "$ip" ] || ! lan_host "$ip" || ip=""
	printf '%s' "$ip"
}
put() {   # put 文件 权限 命令…：命令的输出先写到旁边、再换上（正在运行的服务读不到半截）
	f=$1 m=$2
	shift 2
	if "$@" > "$f.new.$$" && chmod "$m" "$f.new.$$" && mv -f "$f.new.$$" "$f"; then return 0; fi
	rm -f "$f.new.$$"
	die install "写不了 ${f}"
}
nologin() { for s in /usr/sbin/nologin /sbin/nologin; do [ -x "$s" ] && { printf '%s' "$s"; return; }; done; printf /bin/false; }
relay_on() { case "$RELAY" in ''|off) return 1 ;; esac; return 0; }   # 这一次开着中转（RELAY 是数据端口）
fw_ports() {   # 这一次要从外面连得上的端口
	printf '%s' "$PORT"
	! relay_on || printf ' %s' "$RELAY"
	[ -z "$WEBON" ] || printf ' %s 80' "$WEB"
}
ports_say() {   # 要放行的端口（说给人看的）
	printf 'TCP %s' "$PORT"
	! relay_on || printf '、%s（远程会话的中转）' "$RELAY"
	[ -z "$WEBON" ] || printf '、%s 和 80（手机的网页端；80 端口给证书机构验证）' "$WEB"
}
fw_hint() {   # 这台服务器自己的防火墙没放行这几个端口：说怎么放行（只看，不改）。Docker 发布的端口不经 ufw、firewalld 的规则，systemd 的要
	[ "$MODE" = systemd ] || return 0
	for fwp in $(fw_ports); do
		if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
			ufw status 2>/dev/null | grep -Eq "^${fwp}(/tcp)? .*ALLOW" || say "! 这台服务器开着 ufw：运行 ufw allow ${fwp}/tcp 放行"
		elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
			firewall-cmd --query-port="${fwp}/tcp" >/dev/null 2>&1 ||
				say "! 这台服务器开着 firewalld：运行 firewall-cmd --permanent --add-port=${fwp}/tcp && firewall-cmd --reload 放行"
		fi
	done
}

# --- 装在哪 ----------------------------------------------------------------------------------------------------------------
dir_check() {   # --dir：/ 下面的一个目录（不是 /，没有空的、.、.. 这样的一节），只用字母、数字和 . _ - /；末尾的 / 去掉。hub-deploy 的 dir_of 一样。
	                # 不能在 systemd 方式的服务看不到、写不了，或者重启就清掉的地方（unit_file 的 ProtectSystem=full、ProtectHome、PrivateTmp、PrivateDevices）
	case "$D" in /*) ;; *) die usage "--dir 要是绝对路径" ;; esac
	case "${D%/}/" in
		/|*//*|*/./*|*/../*|*[!A-Za-z0-9._/-]*) die usage "--dir 要是 / 下面的一个目录（只用字母、数字和 . _ - /）：${D}" ;;
		/usr/*|/boot/*|/efi/*|/etc/*|/home/*|/root/*|/run/*|/var/run/*|/tmp/*|/var/tmp/*|/dev/*|/proc/*|/sys/*|/bin/*|/sbin/*|/lib/*|/lib64/*)
			die usage "--dir 不能在 /usr、/etc、/home、/root、/run、/tmp 这些下面（systemd 方式的服务看不到、写不了那里）：${D}" ;;
	esac
	D=${D%/}
}
team_setup() {   # 每个 Team 一个服务（名字里带 Team 编号的前 8 位：根公钥的 SHA-256），目录默认 /opt/<服务名>
	TEAMROOT=${TEAMROOT#ssh-ed25519 }
	TEAMROOT=${TEAMROOT%% *}
	case "$TEAMROOT" in *[!A-Za-z0-9+/]*) die usage "Team 的根公钥不对（要 cca team new 给的那一行：ssh-ed25519 AAAA…）" ;; esac
	[ ${#TEAMROOT} -eq 68 ] || die usage "Team 的根公钥不对（要 cca team new 给的那一行：ssh-ed25519 AAAA…）"
	TID=$(printf '%s' "$TEAMROOT" | openssl base64 -d -A 2>/dev/null | openssl dgst -sha256 -binary 2>/dev/null | hexof | cut -c1-8)
	[ ${#TID} -eq 8 ] || die install "算不出 Team 的编号（openssl）"
	SVC=cca-team-$TID
	UNIT=/etc/systemd/system/$SVC.service
	DCP="-p $SVC"
	[ -n "$DIRSET" ] || D=/opt/$SVC
}
team_here() {   # 没写 --team-root：照这个目录里装的（上次写的 .env）——再运行是更新。.env 不见了（或者坏了）时 .cca-hub 里记着服务名：
	            # 只够卸载（停服务、删用户）。3.5.3 以前的自己的同步服务器（服务名 cca-hub）也只能卸载
	[ -z "$TEAMROOT" ] || return 0
	TEAMROOT=$(env_get CCA_TEAM_ROOT)
	case "$TEAMROOT" in *[!A-Za-z0-9+/]*) TEAMROOT="" ;; esac
	[ ${#TEAMROOT} -eq 68 ] || TEAMROOT=""
	if [ -n "$TEAMROOT" ]; then
		team_setup   # DIRSET 是 1：目录就是读到它的这一个
		return 0
	fi
	s=$(sed -n 's/^# svc=\(cca-[a-z0-9-]*\)$/\1/p' "$ROOT$D/.cca-hub" 2>/dev/null | head -n 1)
	[ -n "$s" ] || [ -z "$(env_get CCA_HUB_ADMIT_HASH)" ] || s=cca-hub   # 更早的安装脚本没在 .cca-hub 里记服务名
	case "$s" in
		cca-team-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f])
			[ -n "$UNINSTALL" ] || die usage "${D} 里装的是 Team 的同步服务器，它的 .env 不见了：带上 --team-root '根公钥' 再运行"
			DCP="-p $s" ;;
		cca-hub)
			[ -n "$UNINSTALL" ] ||
				die usage "${D} 里装的是以前的自己的同步服务器（cca hub，3.5.3 起没有了，改成建一个 Team）：先卸掉它（--uninstall --dir ${D}），Team 的装在别的目录"
			WHAT="以前的自己的同步服务器" ;;   # compose 的项目名那时是目录名（不带 -p）
		*) die usage "${D} 里没有装过 Team 的同步服务器：要给 --team-root（cca team new 给的根公钥那一行：ssh-ed25519 AAAA…）" ;;
	esac
	SVC=$s UNIT=/etc/systemd/system/$s.service
}
dir_ours() {   # 能装在这个目录：还没有、空的，或者是以前装的（有 .cca-hub 或者 .env）。别的目录不碰：里面的 data、certs 会被 chown，卸载时会删掉
	[ -d "$ROOT$D" ] || return 0
	[ -f "$ROOT$D/.cca-hub" ] || [ -f "$ROOT$D/.env" ] || [ -z "$(ls -A "$ROOT$D" 2>/dev/null)" ]
}
other_dir() {   # 这台服务器上别的目录里装着的这一个（systemd 的 unit 里写着）
	[ -f "$ROOT$UNIT" ] && sed -n 's/^EnvironmentFile=\(.*\)\/\.env$/\1/p' "$ROOT$UNIT" | grep -vxF "$D" | head -n 1
}
other_kind() {   # 写了 --team-root，这个目录里装着的却是另一个 Team 的、或者以前的自己的同步服务器：说是什么；不是返回 1。
	              # .env 不见了时照 .cca-hub 里记的服务名认
	t=$(env_get CCA_TEAM_ROOT)
	s=$(sed -n 's/^# svc=\(cca-[a-z0-9-]*\)$/\1/p' "$ROOT$D/.cca-hub" 2>/dev/null | head -n 1)
	if [ -n "$t" ] && [ "$t" != "$TEAMROOT" ]; then printf '另一个 Team 的同步服务器'
	elif [ -z "$t" ] && { [ -n "$(env_get CCA_HUB_ADMIT_HASH)" ] || [ "$s" = cca-hub ]; }; then printf '以前的自己的同步服务器（cca hub）'
	elif [ -z "$t" ] && [ -n "$s" ] && [ "$s" != "$SVC" ]; then printf '另一个 Team 的同步服务器'
	else return 1; fi
}

# --- 这一次怎么装 ------------------------------------------------------------------------------------------------------------
pick_mode() {   # MODE：--docker、--systemd 照说的；没说时上次是 systemd、还能用就照旧，再是 Docker（AUTO：拉不到镜像、起不来时改用 systemd），再是 systemd
	OLD=$(env_get CCA_HUB_MODE)
	case "$MODE" in
		docker) have_docker || die deps "这台服务器上没有 Docker（带 compose 插件：docker compose），或者 Docker 没在运行：先装好，或者换 --systemd" ;;
		systemd) have_systemd || die deps "要 systemd 和 python3（3.6 起）：$(install_cmd python3)；或者换 --docker" ;;
		*)
			if [ "$OLD" = systemd ] && have_systemd; then MODE=systemd
			elif have_docker; then MODE=docker AUTO=1
			elif have_systemd; then MODE=systemd
			else die deps "要 Docker（带 compose 插件：docker compose），或者 systemd + python3（3.6 起，$(install_cmd python3)）：这台服务器上两样都没有"
			fi ;;
	esac
}
pick_state() {   # STATE：new 第一次装、update 装过这个 Team 的（只更新：数据、证书、端口都不变）
	if [ -n "$(env_get CCA_TEAM_ROOT)" ]; then STATE=update; else STATE=new; fi
}
pick_port() {   # PORT：--port，没写时照上次的，第一次装从 18445 起找一个空着的（一台服务器上几个 Team 各用各的端口）。换端口可以：管理员重新
	            # 接上（cca team attach 新地址），各台设备照配置里的新地址换过来
	OLDPORT=$(env_get CCA_HUB_PORT)
	if [ -z "$PORT" ] && [ -n "$OLDPORT" ]; then PORT=$OLDPORT
	elif [ -z "$PORT" ]; then
		PORT=$DEF_PORT
		i=0
		while port_taken "$PORT" && [ $i -lt 50 ]; do PORT=$((PORT + 1)); i=$((i + 1)); done
	fi
}
pick_relay() {   # RELAY：给这个 Team 的远程会话做中转的数据端口；off 不开。--relay-port / --no-relay 照说的，没说时照上次的，
	             # 第一次从 HTTPS 那个端口的下一个起找一个空着的
	OLDRELAY=$(env_get CCA_HUB_RELAY)
	[ -n "$RELAY" ] || RELAY=$OLDRELAY
	if [ -z "$RELAY" ]; then
		RELAY=$((PORT + 1))
		i=0
		while { [ "$RELAY" = "$PORT" ] || port_taken "$RELAY"; } && [ $i -lt 50 ]; do RELAY=$((RELAY + 1)); i=$((i + 1)); done
	fi
	relay_on || return 0
	[ "$RELAY" != "$PORT" ] || die usage "--relay-port 不能和 --port 一样"
	if port_busy "$RELAY" && ! { [ "$RELAY" = "$OLDRELAY" ] && svc_running; }; then
		die port "中转的数据端口 ${RELAY} 被别的程序占着：换一个（--relay-port），或者不开中转（--no-relay）"
	fi
}
pick_host() {   # HOST：各台设备、手机经它连过来的地址（--host；没写时照上次记下的，再是 cca team deploy 放进的 ssh 的主机名、这台机器上
	            # 看得到的公网 IP，都没有时在终端里问）。HP 是网址里的写法。$1 是 1：一定要有（要打出管理员接上它的那一行），问不到就停下
	[ -n "$HOST" ] || HOST=$(env_get CCA_HUB_HOST)
	[ -n "$HOST" ] || HOST=$HOSTHINT
	[ -n "$HOST" ] || HOST=$(public_ip)
	if [ -z "$HOST" ] && [ -z "$MANAGED" ] && (: </dev/tty) 2>/dev/null; then
		printf '这台服务器的公网 IP 或者域名（各台设备经它连过来）: ' >/dev/tty
		read -r HOST </dev/tty || HOST=""
	fi
	HOST=${HOST#[}
	HOST=${HOST%]}
	if [ -z "$HOST" ]; then
		[ "$1" != 1 ] || die host "认不出这台服务器的公网地址（云服务器的公网 IP 常常不在机器上）：加上 --host 公网IP或域名"
		return 0
	fi
	case "$HOST" in
		*:*) case "$HOST" in *[!0-9A-Fa-f:]*) die usage "--host 写错了：${HOST}" ;; esac; HP="[$HOST]" ;;
		*[!A-Za-z0-9.-]*|-*|.*) die usage "--host 写错了：${HOST}" ;;
		*) HP=$HOST ;;
	esac
}
pick_web() {   # WEB：手机的网页端的端口；off 不开。--web-port / --no-web 照说的，没说时照上次的，第一次从数据端口的下一个起找一个空着的。
	           # WEBON：这一次开着——还要中转开着、有公网地址、80 端口给得了证书助手；不开时 WEB_WHY 是为什么（照样记着 WEB，再运行时再看）
	OLDWEB=$(env_get CCA_HUB_WEB) OLDWEBURL=$(env_get CCA_HUB_WEB_URL)
	[ -n "$WEB" ] || WEB=$OLDWEB
	if ! relay_on; then
		[ -z "$WEBSET" ] || [ "$WEB" = off ] || die usage "网页端是经中转接入会话的：--web-port 不能和 --no-relay 一起"
		return 0
	fi
	[ "$WEB" != off ] || return 0
	if [ -z "$WEB" ]; then
		WEB=$((RELAY + 1))
		i=0
		while { [ "$WEB" = "$PORT" ] || [ "$WEB" = "$RELAY" ] || port_taken "$WEB"; } && [ $i -lt 50 ]; do WEB=$((WEB + 1)); i=$((i + 1)); done
	fi
	[ "$WEB" != "$PORT" ] && [ "$WEB" != "$RELAY" ] || die usage "--web-port 不能和 --port、中转的数据端口一样"
	if port_busy "$WEB" && ! { [ "$WEB" = "$OLDWEB" ] && svc_running; }; then
		[ -z "$WEBSET" ] || die port "网页端的端口 ${WEB} 被别的程序占着：换一个（--web-port），或者不开网页端（--no-web）"
		WEB_WHY=web_port
		return 0
	fi
	if [ -z "$HOST" ]; then WEB_WHY=host; return 0; fi
	if [ -z "$ANYIP" ] && lan_host "$HOST"; then WEB_WHY=lan; return 0; fi
	# 80 端口上听着的是共用的证书助手，或者这个 Team 以前它自己的（这一次换成共用的）：不算被占着
	if port_busy 80 && ! acme_running && ! svc_running "$SVC-acme"; then
		if OLDACME=$(old_acme); then
			[ -z "$WEBSET" ] || die port "80 端口被 ${OLDACME} 占着：那是另一个 Team 用以前的安装脚本装的、它自己的证书助手。先用这一版重新装一次那个 Team（它会改用这台服务器上几个 Team 共用的证书助手 ${ASVC}），再运行这一个；或者不开网页端（--no-web）"
			WEB_WHY=acme_old
			[ -z "$MANAGED" ] || say "! 80 端口被 ${OLDACME} 占着（另一个 Team 用以前的安装脚本装的、它自己的证书助手）"
		else
			[ -z "$WEBSET" ] || die port "80 端口被别的程序占着：证书助手要用它（证书机构从公网连它，验证这台服务器的地址）。腾出来再运行，或者不开网页端（--no-web）"
			WEB_WHY=port80
		fi
		return 0
	fi
	WEBON=1 WEBURL="https://${HP}:${WEB}/"
}
acme_mode() {   # AMODE：共用的证书助手怎么运行——装着的照旧（还能用的话），没装过照这个 Team 的；AIMG：Docker 的用哪个镜像（这个 Team 刚拉的优先）
	AMODE=$(aenv_get CCA_HUB_MODE)
	case "$AMODE" in
		docker) have_docker || AMODE="" ;;
		systemd) have_systemd || AMODE="" ;;
		*) AMODE="" ;;
	esac
	[ -n "$AMODE" ] || AMODE=$MODE
	AIMG=$IMG
	[ -n "$AIMG" ] || AIMG=$(aenv_get CCA_HUB_IMAGE)
}
auid() { if [ "$AMODE" = docker ]; then printf '%s' "$CUID"; else printf '%s' "$ASVC"; fi; }   # 证书助手以哪个用户运行
tgid() { if [ "$MODE" = docker ]; then printf '%s' "$CUID"; else id -g "$SVC"; fi; }       # 这个 Team 的服务端的组
pull_image() {   # IMG：上次用的那个，再依次试几个同步源，第一个拉得到（或者本机已经有）的；都不行时 AUTO 的改用 systemd
	for i in $(env_get CCA_HUB_IMAGE) $IMAGES; do
		say "拉取镜像 ${i}…"
		if tmo 300 docker pull -q "$i" </dev/null >/dev/null 2>&1 || docker image inspect "$i" >/dev/null 2>&1; then IMG=$i; return 0; fi
	done
	[ -n "$AUTO" ] && have_systemd ||
		die image "拉不到 python:3.13-slim 镜像（Docker Hub 和几个同步源都连不上）：给 Docker 配上镜像加速再运行，或者换 --systemd"
	say "拉不到 python 镜像（Docker Hub 和几个同步源都连不上）：改用 systemd + 系统的 python3"
	MODE=systemd
}

# --- 装 ----------------------------------------------------------------------------------------------------------------------
web_file() {   # web_file 目录 名字 SHA-256：stdin（base64）解开写成 目录/名字，核对 SHA-256（hub_web_files 一个文件调一次）
	openssl base64 -d > "$1/$2" 2>/dev/null && [ "$(openssl dgst -sha256 -binary < "$1/$2" 2>/dev/null | hexof)" = "$3" ]
}
web_put() {   # 网页端的文件（安装脚本里带着的）换上：先都写进 app/web.new、核对过，再换掉 app/web（正在运行的服务端启动时已经读进内存了）
	w=$ROOT$D/app/web
	rm -rf "$w.new" "$w.old" && mkdir "$w.new" || die install "写不了 ${D}/app/web.new"
	hub_web_files "$w.new" || { rm -rf "$w.new"; die install "写不了网页端的文件（openssl base64 -d）"; }
	chmod 755 "$w.new" && chmod 644 "$w.new"/* || die install "改不了网页端的文件的权限"
	if [ -d "$w" ]; then mv "$w" "$w.old" || die install "换不了 ${D}/app/web"; fi
	mv "$w.new" "$w" || die install "换不了 ${D}/app/web"
	rm -rf "$w.old"
}
prepare() {   # 目录（.cca-hub 记着是这里装的）、服务端程序、网页端的文件、Team 的名字（--name；没写时留着上次的）、证书（没有才生成：
	           # NEWCERT）；PIN 是证书公钥的指纹。证书助手不在这里了（共用的那一个：acme_dirs）；以前的 webcert/、acme/ 留到 acme_start 搬走
	mkdir -p "$ROOT$D/app" "$ROOT$D/certs" "$ROOT$D/data" || die install "建不了 ${D}"
	printf '# cca 的同步服务器（%s）装在这个目录：安装脚本认这个文件（再运行是更新，--uninstall 删掉它装的那些）\n# svc=%s\n' "$WHAT" "$SVC" > "$ROOT$D/.cca-hub" ||
		die install "写不了 ${D}/.cca-hub"
	chmod 755 "$ROOT$D" "$ROOT$D/app" && chmod 700 "$ROOT$D/certs" "$ROOT$D/data" || die install "改不了 ${D} 的权限"
	put "$ROOT$D/app/cca-presence.py" 644 hub_server_py
	[ -z "$NAME" ] || put "$ROOT$D/app/name" 644 printf '%s\n' "$NAME"
	web_put
	if [ ! -s "$ROOT$D/certs/key.pem" ] || [ ! -s "$ROOT$D/certs/cert.pem" ]; then
		# 自签的 P-256 证书（macOS 的 curl 只给 RSA 和 P-256/P-384 做公钥钉住），私钥不出这台服务器
		rm -f "$ROOT$D/certs/key.pem" "$ROOT$D/certs/cert.pem"
		openssl ecparam -name prime256v1 -genkey -noout -out "$ROOT$D/certs/key.pem" 2>/dev/null &&
			openssl req -new -x509 -key "$ROOT$D/certs/key.pem" -subj /CN=cca-hub -days 3650 -out "$ROOT$D/certs/cert.pem" </dev/null 2>/dev/null ||
			die install "生成证书失败（openssl）"
		NEWCERT=1
		say "生成了新的证书（自签 P-256，10 年；私钥不出这台服务器）"
	fi
	chmod 600 "$ROOT$D/certs/key.pem" && chmod 644 "$ROOT$D/certs/cert.pem" || die install "改不了证书的权限"
	PIN=$(pin_of "$ROOT$D/certs/cert.pem")
	case "$PIN" in *[!A-Za-z0-9+/=]*|'') die install "算不出证书公钥的指纹（openssl）" ;; esac
	[ ${#PIN} -eq 44 ] || die install "算不出证书公钥的指纹（openssl）"
}
env_file() {
	printf '# %s的设置：安装脚本写的，改了以后再运行一次安装脚本\n' "$WHAT"
	printf 'CCA_HUB_MODE=%s\nCCA_HUB_PORT=%s\nCCA_TEAM_ROOT=%s\nCCA_HUB_RELAY=%s\nCCA_HUB_WEB=%s\n' "$MODE" "$PORT" "$TEAMROOT" "$RELAY" "$WEB"
	[ -z "$HOST" ] || printf 'CCA_HUB_HOST=%s\n' "$HOST"
	[ -z "$WEBON" ] || printf 'CCA_HUB_WEB_URL=%s\n' "$WEBURL"
	[ -z "$IMG" ] || printf 'CCA_HUB_IMAGE=%s\n' "$IMG"
	[ -z "$VERSION" ] || printf 'CCA_HUB_VERSION=%s\n' "$VERSION"
}
write_env() {   # .env 换成这一次的（起服务之前）。没装完（die、被打断）时 rollback 换回原来的（第一次装时删掉）：下一次照原来的判断是哪种情况
	if [ -z "$ROLLBACK" ]; then
		rm -f "$ROOT$D/.env.prev"
		[ ! -f "$ROOT$D/.env" ] || cp -p "$ROOT$D/.env" "$ROOT$D/.env.prev" || die install "写不了 ${D}/.env.prev"
		ROLLBACK=1
	fi
	put "$ROOT$D/.env" 600 env_file
}
rollback() {
	[ -n "$ROLLBACK" ] || return 0
	ROLLBACK=""
	if [ -f "$ROOT$D/.env.prev" ]; then mv -f "$ROOT$D/.env.prev" "$ROOT$D/.env"; else rm -f "$ROOT$D/.env"; fi
}
tasks() { if [ -n "$WEBON" ]; then printf 512; else printf 64; fi; }   # 服务端最多几个线程：网页端开着时每条 WebSocket 一个，两个端口各能同时处理 64 个请求
named() { [ -s "$ROOT$D/app/name" ]; }   # 记着 Team 的名字（网页的标题用它）
compose_yaml() {   # @SVC@ 换成服务名（容器名）、@AD@ 换成证书助手的目录。中转、网页端开着：带着 @RELAY@、@WEB@ 记号的那几处留下（去掉记号）；
	               # 没开：整个去掉（不占端口）。网页端的证书是共用的证书助手放进 @AD@/webcert/<服务名>/ 的（只读挂进来）
	if relay_on; then relay='s/@RELAY@\([^@]*\)@@/\1/g'; else relay='/^@RELAY@[^@]*@@$/d;s/@RELAY@[^@]*@@//g'; fi
	if [ -n "$WEBON" ]; then web='s/@WEB@\([^@]*\)@@/\1/g'; else web='/^@WEB@[^@]*@@$/d;s/@WEB@[^@]*@@//g'; fi
	if [ -n "$WEBON" ] && named; then name='s/@NAME@\([^@]*\)@@/\1/g'; else name='s/@NAME@[^@]*@@//g'; fi
	# 容器里：HTTPS 听 18445，中转的数据端口 18446，网页端 18447；对外是 .env 里的 CCA_HUB_PORT、CCA_HUB_RELAY、CCA_HUB_WEB
	sed -e "s/@SVC@/$SVC/g" -e "s|@AD@|$AD|g" -e "s/@PIDS@/$(tasks)/" -e "$relay" -e "$name" -e "$web" <<'EOF'
# cca 的 Team 的同步服务器（@SVC@）：安装脚本写的，再运行安装脚本时会换掉。设置在 .env 里
services:
  @SVC@:
    image: ${CCA_HUB_IMAGE:-python:3.13-slim}
    container_name: @SVC@
    restart: unless-stopped
    user: "10001:10001"
    command: ["python3", "-I", "/app/cca-presence.py", "--mode", "team", "--listen", "0.0.0.0:18445", "--cert", "/certs/cert.pem", "--key", "/certs/key.pem", "--state", "/data/state.json"@RELAY@, "--relay-listen", "0.0.0.0:18446", "--relay-port", "${CCA_HUB_RELAY:?}"@@@WEB@, "--web-url", "${CCA_HUB_WEB_URL:?}", "--web-listen", "0.0.0.0:18447", "--web-cert", "/webcert/fullchain.pem", "--web-key", "/webcert/privkey.pem"@NAME@, "--web-name-file", "/app/name"@@@@]
    ports:
      - "${CCA_HUB_PORT:-18445}:18445"
@RELAY@      - "${CCA_HUB_RELAY:?}:18446"@@
@WEB@      - "${CCA_HUB_WEB:?}:18447"@@
    environment:
      CCA_TEAM_ROOT: ${CCA_TEAM_ROOT:?}
    volumes:
      - ./app:/app:ro,z
      - ./certs:/certs:ro,z
      - ./data:/data:z
@WEB@      - @AD@/webcert/@SVC@:/webcert:ro,z@@
    read_only: true
    tmpfs:
      - /tmp:size=8m
    cap_drop: [ALL]
    security_opt:
      - no-new-privileges:true
    mem_limit: 256m
    pids_limit: @PIDS@
    logging:
      driver: json-file
      options:
        max-size: "1m"
        max-file: "3"
EOF
}
unit_file() {
	extra=""
	! relay_on || extra=" --relay-listen 0.0.0.0:${RELAY} --relay-port ${RELAY}"
	[ -z "$WEBON" ] || extra="${extra} --web-url ${WEBURL} --web-listen 0.0.0.0:${WEB} --web-cert $AD/webcert/$SVC/fullchain.pem --web-key $AD/webcert/$SVC/privkey.pem"
	[ -z "$WEBON" ] || ! named || extra="${extra} --web-name-file $D/app/name"
	cat <<EOF
# cca 的 Team 的同步服务器（${SVC}）：安装脚本写的，再运行安装脚本时会换掉。设置在 $D/.env 里
[Unit]
Description=cca ${WHAT}（${SVC}）
After=network-online.target
Wants=network-online.target

[Service]
User=$SVC
EnvironmentFile=$D/.env
ExecStart=$PY -I $D/app/cca-presence.py --mode team --listen 0.0.0.0:\${CCA_HUB_PORT} --cert $D/certs/cert.pem --key $D/certs/key.pem --state $D/data/state.json${extra}
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=full
ProtectHome=yes
CapabilityBoundingSet=
TasksMax=$(tasks)
MemoryMax=256M
LimitNOFILE=4096

[Install]
WantedBy=multi-user.target
EOF
}
acme_env() {
	printf '# 这台服务器上几个 Team 共用的证书助手的设置：安装脚本写的（用它的 Team 登记在 teams/ 里）\n'
	printf 'CCA_HUB_MODE=%s\n' "$AMODE"
	[ "$AMODE" != docker ] || printf 'CCA_HUB_IMAGE=%s\n' "$AIMG"
	[ -z "$VERSION" ] || printf 'CCA_HUB_VERSION=%s\n' "$VERSION"
}
acme_compose() {   # 共用的证书助手（Docker）：证书机构从公网连 80 端口验证（容器里听 18080）
	cat <<'EOF'
# cca 的 Team 的同步服务器共用的证书助手（cca-team-acme）：给这台服务器上各个 Team 的手机网页端向 Let's Encrypt 要证书、到期前续上。
# 安装脚本写的，再运行安装脚本时会换掉。设置在 .env 里
services:
  cca-team-acme:
    image: ${CCA_HUB_IMAGE:-python:3.13-slim}
    container_name: cca-team-acme
    restart: unless-stopped
    user: "10001:10001"
    command: ["python3", "-I", "/app/cca-acme.py", "--teams", "/teams", "--store", "/store", "--out", "/webcert", "--account", "/acme", "--listen", "0.0.0.0:18080"]
    ports:
      - "80:18080"
    volumes:
      - ./app:/app:ro,z
      - ./teams:/teams:ro,z
      - ./store:/store:z
      - ./webcert:/webcert:z
      - ./acme:/acme:z
    read_only: true
    tmpfs:
      - /tmp:size=8m
    cap_drop: [ALL]
    security_opt:
      - no-new-privileges:true
    mem_limit: 128m
    pids_limit: 32
    logging:
      driver: json-file
      options:
        max-size: "1m"
        max-file: "3"
EOF
}
acme_unit_file() {   # 共用的证书助手（systemd）：直接听 80 端口（只给它绑低端口的权限）
	cat <<EOF
# cca 的 Team 的同步服务器共用的证书助手（${ASVC}）：给这台服务器上各个 Team 的手机网页端向 Let's Encrypt 要证书、到期前续上。
# 安装脚本写的，再运行安装脚本时会换掉
[Unit]
Description=cca Team 的同步服务器共用的证书助手（${ASVC}）
After=network-online.target
Wants=network-online.target

[Service]
User=$ASVC
EnvironmentFile=$AD/.env
ExecStart=$PY -I $AD/app/cca-acme.py --teams $AD/teams --store $AD/store --out $AD/webcert --account $AD/acme --listen 0.0.0.0:80
Restart=on-failure
RestartSec=60
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=full
ProtectHome=yes
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
TasksMax=16
MemoryMax=128M

[Install]
WantedBy=multi-user.target
EOF
}
logs() {   # 起不来时：最近的日志（缩进，写 stderr）
	case "$MODE" in
		docker) dc logs --tail 20 2>&1 ;;
		systemd) journalctl -u "$SVC" -n 20 --no-pager </dev/null 2>/dev/null ;;
	esac | sed 's/^/    /' >&2
}
acme_logs() { if [ "$AMODE" = docker ]; then printf 'docker logs %s' "$ASVC"; else printf 'journalctl -u %s' "$ASVC"; fi; }
adc() { (cd "$ROOT$AD" && docker compose -p "$ASVC" -f compose.yaml "$@") </dev/null; }   # 共用的证书助手的 compose 项目
stop_docker() {   # 停掉、删掉容器（这个目录里有 compose.yaml 时；以前它自己的证书助手是同一个 compose 项目里的）
	[ -f "$ROOT$D/compose.yaml" ] || return 0
	if command -v docker >/dev/null 2>&1; then
		dc down >/dev/null 2>&1 || die install "停不掉原来的 Docker 容器（docker compose down 没成）：Docker 在运行吗？"
	fi
	rm -f "$ROOT$D/compose.yaml"
}
drop_unit() {   # drop_unit 服务名：停掉、删掉这个 systemd 服务（有的话）
	[ -f "$ROOT/etc/systemd/system/$1.service" ] || return 1
	systemctl disable --now "$1" </dev/null >/dev/null 2>&1
	rm -f "$ROOT/etc/systemd/system/$1.service"
}
stop_systemd() {   # 这个 Team 的 systemd 服务（连同以前它自己的证书助手的）
	dropped=""
	drop_unit "$SVC" && dropped=1
	drop_unit "$SVC-acme" && dropped=1
	[ -z "$dropped" ] || systemctl daemon-reload </dev/null >/dev/null 2>&1
}
acme_dirs() {   # 网页端开着：共用的证书助手的目录（第一次用时建；systemd 的还要它的系统用户）和这个 Team 的那一份（webcert/<服务名>：
	            # 证书助手写，这个 Team 的服务端读——目录属于证书助手的用户和这个 Team 的组，带 setgid，放进去的文件跟着它的组，私钥 640，
	            # 别的 Team 读不了）。起这个 Team 的服务之前：Docker 要挂载它，systemd 的服务端要读它
	acme_mode
	a=$ROOT$AD
	mkdir -p "$a/app" "$a/teams" "$a/store" "$a/acme" "$a/webcert/$SVC" || die install "建不了 ${AD}"
	chmod 755 "$a" "$a/app" "$a/teams" && chmod 700 "$a/store" "$a/acme" && chmod 711 "$a/webcert" || die install "改不了 ${AD} 的权限"
	if [ "$AMODE" = systemd ] && ! id "$ASVC" >/dev/null 2>&1; then
		useradd -r -M -d "$AD" -s "$(nologin)" "$ASVC" </dev/null >/dev/null 2>&1 || die install "建不了系统用户 ${ASVC}（useradd）"
	fi
	chown -R "$(auid)" "$a/store" "$a/acme" "$a/webcert" && chown -R "$(auid):$(tgid)" "$a/webcert/$SVC" && chmod 2750 "$a/webcert/$SVC" ||
		die install "chown 失败（${AD}）"
}
start_docker() {   # Docker compose 起服务。起不来时把 compose 说的打出来（缩进，写 stderr），返回 1
	put "$ROOT$D/compose.yaml" 644 compose_yaml
	chown -R "$CUID:$CUID" "$ROOT$D/certs" "$ROOT$D/data" || die install "chown 失败"
	[ -z "$WEBON" ] || acme_dirs
	stop_systemd
	out=$(dc up -d --force-recreate --remove-orphans 2>&1) && return 0
	printf '%s\n' "$out" | tail -n 20 | sed 's/^/    /' >&2
	return 1
}
start_systemd() {
	id "$SVC" >/dev/null 2>&1 || useradd -r -M -d "$D" -s "$(nologin)" "$SVC" </dev/null >/dev/null 2>&1 || die install "建不了系统用户 ${SVC}（useradd）"
	chown -R "$SVC" "$ROOT$D/certs" "$ROOT$D/data" || die install "chown 失败"
	[ -z "$WEBON" ] || acme_dirs
	stop_docker
	put "$ROOT$UNIT" 644 unit_file
	drop_unit "$SVC-acme"   # 以前它自己的证书助手：换成共用的（acme_start）
	systemctl daemon-reload </dev/null >/dev/null 2>&1 && systemctl enable "$SVC" </dev/null >/dev/null 2>&1 &&
		systemctl restart "$SVC" </dev/null >/dev/null 2>&1 || { logs; die start "systemd 起不来 ${SVC}（上面是它的日志）"; }
}
acme_fail() { say "! 证书助手（${ASVC}）起不来：手机的网页端拿不到证书（$(acme_logs) 看它说的）"; CERT_WAIT=0; }
acme_start() {   # 网页端开着、这个 Team 的服务已经用这一次的设置起来了：以前它自己的证书助手（<服务名>-acme，刚才已经停掉）的证书、
	             # 账户搬过来（证书放进它的那一份：共用的证书助手看到还用得了就接着用，不再要一张——同样的证书 7 天最多发 5 张），
	             # 登记这个 Team 的地址（teams/<服务名>），换上这一版的证书助手、重新起它（登记的变了它过几秒自己也会照新的）
	a=$ROOT$AD od=$ROOT$D
	if [ -s "$od/webcert/fullchain.pem" ] && [ -s "$od/webcert/privkey.pem" ] && [ ! -s "$a/webcert/$SVC/fullchain.pem" ]; then
		mv -f "$od/webcert/privkey.pem" "$a/webcert/$SVC/privkey.pem" && mv -f "$od/webcert/fullchain.pem" "$a/webcert/$SVC/fullchain.pem" ||
			die install "搬不了 ${D}/webcert 里原来的证书"
	fi
	if [ -s "$od/acme/account.key" ] && [ ! -s "$a/acme/account.key" ]; then
		mv -f "$od/acme/account.key" "$a/acme/account.key" || die install "搬不了 ${D}/acme 里原来的账户"
	fi
	rm -rf "$od/webcert" "$od/acme" "$od/app/cca-acme.py"
	chown -R "$(auid)" "$a/acme" && chown -R "$(auid):$(tgid)" "$a/webcert/$SVC" || die install "chown 失败（${AD}）"
	for f in privkey.pem:640 fullchain.pem:644; do
		[ ! -f "$a/webcert/$SVC/${f%:*}" ] || chmod "${f#*:}" "$a/webcert/$SVC/${f%:*}" || die install "改不了 ${AD}/webcert/${SVC} 的权限"
	done
	put "$a/teams/$SVC" 644 printf '%s\n' "$HOST"
	put "$a/app/cca-acme.py" 644 hub_acme_py
	put "$a/.env" 600 acme_env
	if [ "$AMODE" = docker ]; then
		! drop_unit "$ASVC" || systemctl daemon-reload </dev/null >/dev/null 2>&1   # 以前是 systemd 的
		put "$a/compose.yaml" 644 acme_compose
		out=$(adc up -d --force-recreate --remove-orphans 2>&1) || { printf '%s\n' "$out" | tail -n 20 | sed 's/^/    /' >&2; acme_fail; }
	else
		if [ -f "$a/compose.yaml" ]; then   # 以前是 Docker 的
			! command -v docker >/dev/null 2>&1 || adc down >/dev/null 2>&1
			rm -f "$a/compose.yaml"
		fi
		put "$ROOT/etc/systemd/system/$ASVC.service" 644 acme_unit_file
		systemctl daemon-reload </dev/null >/dev/null 2>&1 && systemctl enable "$ASVC" </dev/null >/dev/null 2>&1 &&
			systemctl restart "$ASVC" </dev/null >/dev/null 2>&1 || acme_fail
	fi
}
acme_leave() {   # 这个 Team 不再用网页端（这一次没开、卸载）：去掉它的登记和那一份证书。别的 Team 还用着就到此为止（证书助手过几秒自己照新的）；
	             # 没有了就停掉证书助手（证书、账户留着，再开网页端时接着用），$1 是 all（卸载）时连同目录、系统用户一起删掉：ACMEGONE 是 1
	a=$ROOT$AD
	rm -f "$ROOT$D/app/cca-acme.py"
	[ -d "$a" ] || return 0
	case "$SVC" in cca-team-????????) ;; *) return 0 ;; esac   # 以前的自己的同步服务器（cca-hub）没用过它
	rm -rf "$a/teams/$SVC" "$a/webcert/$SVC"
	[ -z "$(ls -A "$a/teams" 2>/dev/null)" ] || return 0
	if [ -f "$a/compose.yaml" ] && command -v docker >/dev/null 2>&1; then adc down >/dev/null 2>&1; fi
	rm -f "$a/compose.yaml"
	! drop_unit "$ASVC" || systemctl daemon-reload </dev/null >/dev/null 2>&1
	[ "${1:-}" = all ] || return 0
	rm -rf "$a"
	! id "$ASVC" >/dev/null 2>&1 || userdel "$ASVC" >/dev/null 2>&1
	ACMEGONE=1
}
wait_served() {   # 等它在 127.0.0.1:PORT 上用这张证书回话（最多 30 秒）
	i=0
	while [ "$(served_pin)" != "$PIN" ]; do
		i=$((i + 1))
		[ $i -lt 30 ] || { logs; die start "服务起来了，30 秒里却没在 127.0.0.1:${PORT} 上用这张证书回话（上面是它的日志）"; }
		sleep 1
	done
}
cert_ok() {   # 网页端的证书（共用的证书助手放给这个 Team 的那一份）在、还没过期
	[ -s "$ROOT$AD/webcert/$SVC/fullchain.pem" ] && [ -s "$ROOT$AD/webcert/$SVC/privkey.pem" ] &&
		openssl x509 -in "$ROOT$AD/webcert/$SVC/fullchain.pem" -noout -checkend 0 >/dev/null 2>&1
}
wait_cert() {   # 等证书助手拿到网页端的证书（最多 CERT_WAIT 秒；第一次要时证书机构要来验证，多半十几秒）：拿到了 CERTOK 是 1
	i=0
	while ! cert_ok; do
		[ "$i" -lt "$CERT_WAIT" ] 2>/dev/null || return 0
		[ $i -gt 0 ] || say "等证书助手向 Let's Encrypt 要网页端的证书（最多 ${CERT_WAIT} 秒）…"
		sleep 2
		i=$((i + 2))
	done
	CERTOK=1
	# 服务端还没有证书时隔一秒看一次文件：等它在网页端的端口上用上（最多 10 秒），说好了的时候手机马上就连得上
	want=$(pin_of "$ROOT$AD/webcert/$SVC/fullchain.pem")
	i=0
	while [ "$(served_pin "$WEB")" != "$want" ] && [ $i -lt 10 ]; do sleep 1; i=$((i + 1)); done
}

# --- 说结果 ------------------------------------------------------------------------------------------------------------------
how() {
	if [ "$MODE" = docker ]; then printf 'Docker compose，镜像 %s' "$IMG"
	else printf 'systemd + %s' "$("$PY" -c 'import platform; print("python " + platform.python_version())' 2>/dev/null)"
	fi
}
report_managed() {   # 给 cca team deploy：一行一项（deploy.rs 的 report 读）。host 是这次定下的地址（pick_host；认不出时不说）
	say "${WHAT}在运行（${D}，$(how)，端口 ${PORT}）"
	fw_hint
	note "mode $MODE"
	note "port $PORT"
	[ -z "$HOST" ] || note "host $HOST"
	! relay_on || note "relay $RELAY"
	if [ -n "$WEBON" ]; then
		note "web $WEBURL"
		if [ -n "$CERTOK" ]; then note "web_cert ok"; else note "web_cert wait"; fi
	elif [ -n "$WEB_WHY" ]; then
		note "web_off $WEB_WHY"
	fi
	note "pin sha256//$PIN"
	note "state $STATE"
	note ok
}
web_say() {   # 直接在服务器上运行的：手机的网页端怎么样了
	if [ -n "$WEBON" ] && [ -n "$CERTOK" ]; then
		say "手机的网页端：${WEBURL}（证书是 Let's Encrypt 签的，证书助手 ${ASVC} 到期前自己续：这台服务器上的几个 Team 共用它）"
	elif [ -n "$WEBON" ]; then
		say "! 手机的网页端（${WEBURL}）的证书还没拿到：证书机构要从公网连这台服务器的 80 端口（安全组里放行 TCP 80）。证书助手 ${ASVC} 过一会儿自己再试（$(acme_logs) 看它说的）"
	else
		case "$WEB_WHY" in
			port80) say "! 手机的网页端没开：80 端口被别的程序占着（证书助手要用它向 Let's Encrypt 要证书）。腾出来以后再运行一次" ;;
			acme_old) say "! 手机的网页端没开：80 端口被 ${OLDACME} 占着——那是另一个 Team 用以前的安装脚本装的、它自己的证书助手。先用这一版重新装一次那个 Team（它会改用这台服务器上几个 Team 共用的证书助手），再运行这一个" ;;
			lan) say "! 手机的网页端没开：${HOST} 是内网地址，证书机构连不过来（有公网地址的话加上 --host 公网地址 再运行一次）" ;;
			host) say "! 手机的网页端没开：认不出这台服务器的公网地址（加上 --host 公网IP或域名 再运行一次）" ;;
			web_port) say "! 手机的网页端没开：端口 ${WEB} 被别的程序占着（换一个：--web-port 端口）" ;;
		esac
	fi
}
report() {   # 直接在服务器上运行的：打出管理员接上它的那一行（地址和指纹都是公开的）
	say "✓ Team 的同步服务器在运行（${D}，$(how)，端口 ${PORT}，服务名 ${SVC}）"
	if [ "$STATE" = update ]; then
		say "  更新了服务端程序，数据都没变"
		[ -z "$NEWCERT" ] || say "! 证书是新生成的（原来的不见了）：管理员要照下面这一行重新接上，各台设备同步到新的配置以后才连得上"
		if [ -z "$NEWCERT" ] && [ "$PORT" = "$OLDPORT" ]; then
			web_say
			fw_hint
			[ -z "$WEBON" ] || [ -n "$OLDWEBURL" ] || say "云服务商的安全组（或轻量应用服务器的防火墙）里要放行 $(ports_say)。"
			return 0
		fi
		[ -n "$HP" ] || HP=这台服务器的地址
	fi
	say ""
	say "在管理员的设备上接上它（Team 写名字或者编号）："
	say ""
	say "  cca team attach Team https://${HP}:${PORT} sha256//${PIN}"
	say ""
	web_say
	fw_hint
	say "云服务商的安全组（或轻量应用服务器的防火墙）里要放行 $(ports_say)。"
}

uninstall() {   # 停掉、删掉这个目录里装的（只认装过的目录：有 .cca-hub 或者 .env）和系统用户
	[ -f "$ROOT$D/.cca-hub" ] || [ -f "$ROOT$D/.env" ] || die none "${D} 里没有装${WHAT}"
	o=$(other_dir)
	stop_docker
	[ -n "$o" ] || stop_systemd
	[ -n "$o" ] || acme_leave all   # 共用的证书助手里它的登记、那一份证书；最后一个用它的 Team 连同证书助手一起删掉
	rm -rf "$ROOT$D/app" "$ROOT$D/certs" "$ROOT$D/data" "$ROOT$D/webcert" "$ROOT$D/acme"
	rm -f "$ROOT$D/.env" "$ROOT$D/.env.prev" "$ROOT$D/.cca-hub"
	rmdir "$ROOT$D" 2>/dev/null
	[ -n "$o" ] || ! id "$SVC" >/dev/null 2>&1 || userdel "$SVC" >/dev/null 2>&1
	if [ "$SVC" = cca-hub ]; then say "✓ 卸载了以前的自己的同步服务器（${D} 里的程序、证书、数据都删了）"
	elif [ -n "$MANAGED" ]; then say "✓ 卸载了 Team 的同步服务器（${D} 里的程序、证书、数据都删了）"   # 删除 Team（cca team leave）时：接着删的就是这个 Team
	else say "✓ 卸载了 Team 的同步服务器（${D} 里的程序、证书、数据都删了）。管理员要换一台接上（cca team attach），或者各台设备退出（cca team leave）"; fi
	[ -z "$ACMEGONE" ] || say "  这台服务器上没有别的 Team 用网页端了：共用的证书助手（${AD}，连同证书、账户）也删掉了"
	note ok
}

port_arg() {   # port_arg 选项 值：1024～65535 的端口
	case "$2" in 0*|*[!0-9]*|'') die usage "$1 要是 1024～65535 的端口" ;; esac
	[ "$2" -ge 1024 ] && [ "$2" -le 65535 ] || die usage "$1 要是 1024～65535 的端口"
}

main() {
	HOST="" PORT="" MODE="" UNINSTALL="" AUTO="" IMG="" NEWCERT="" HP="" RELAY="" WEB="" WEBSET="" WEBON="" WEB_WHY="" WEBURL="" CERTOK=""
	OLD="" OLDPORT="" OLDWEBURL="" OLDACME="" ACMEGONE=""
	trap interrupted HUP INT TERM PIPE
	while [ $# -gt 0 ]; do
		case "$1" in
			--host|--port|--dir|--team-root|--relay-port|--web-port|--name)
				[ $# -ge 2 ] || { usage >&2; exit 2; }
				case "$1" in
					--host) HOST=$2 ;; --port) PORT=$2 ;; --team-root) TEAMROOT=$2 ;; --relay-port) RELAY=$2 ;;
					--web-port) WEB=$2 WEBSET=1 ;; --name) NAME=$2 ;; *) D=$2 DIRSET=1 ;;
				esac
				shift ;;
			--no-relay) RELAY=off ;;
			--no-web) WEB=off WEBSET=1 ;;
			--docker) MODE=docker ;;
			--systemd) MODE=systemd ;;
			--reset) die usage "Team 的同步服务器没有 --reset：设备、邀请都由管理员的设备管（cca team）" ;;
			--uninstall) UNINSTALL=1 ;;
			-h|--help) usage; exit 0 ;;
			*) usage >&2; exit 2 ;;
		esac
		shift
	done
	command -v hub_server_py >/dev/null 2>&1 || die usage "这是 cca 仓库里的模板：要运行的是 cca team deploy --script 生成的那一份（发布页上的 cca-hub-install.sh）"
	if [ -n "$TEAMROOT" ]; then
		command -v openssl >/dev/null 2>&1 || die deps "要 openssl（生成证书、算指纹）：$(install_cmd openssl)"
		team_setup
	elif [ -z "$DIRSET" ]; then
		die usage "要给 --team-root（cca team new 给的根公钥那一行：ssh-ed25519 AAAA…），或者 --dir 装过的目录"
	fi
	dir_check
	team_here   # 没写 --team-root：照这个目录里装的那个 Team（更新、卸载）
	case "$NAME" in *'
'*) die usage "--name 不能有换行" ;; esac
	[ ${#NAME} -le 256 ] || die usage "--name 太长了"
	[ -z "$PORT" ] || port_arg --port "$PORT"
	! relay_on || port_arg --relay-port "$RELAY"
	[ -z "$WEB" ] || [ "$WEB" = off ] || port_arg --web-port "$WEB"
	if [ "$(id -u 2>/dev/null)" != 0 ]; then
		[ -n "$MANAGED" ] && die root "要用 root 登录服务器（或者能免密码 sudo 的用户）"
		die root "要用 root 运行：sudo sh $0 …"
	fi
	if [ -n "$UNINSTALL" ]; then
		# 写了 --team-root（或者照 .env 认出了是哪个 Team）：目录里装着的要是它（不然停的是这个 Team 的服务，删的却是别的数据）
		if [ -n "$TEAMROOT" ]; then
			o=$(other_kind) && die taken "${D} 里装着的是${o}，不是要卸载的这一个：看清楚 --dir、--team-root 再运行"
		fi
		uninstall
		return
	fi
	command -v openssl >/dev/null 2>&1 || die deps "要 openssl（生成证书、算指纹）：$(install_cmd openssl)"
	dir_ours || die usage "${D} 里已经有别的东西（不是装过的${WHAT}）：换一个新的目录（--dir）"
	o=$(other_kind) && die taken "${D} 里装着的是${o}：换一个目录（--dir），或者先卸载它（--uninstall --dir ${D}）"
	o=$(other_dir)
	[ -z "$o" ] || die usage "这台服务器上已经在 ${o} 装了这个 Team 的同步服务器：照旧用 --dir ${o}，或者先卸载它（--uninstall --dir ${o}）"
	pick_mode
	pick_state
	pick_port
	pick_relay
	# 直接在服务器上运行的，要打出管理员接上它的那一行时一定要有地址：第一次装、换了端口（证书重新生成时没有地址也行：说「这台服务器的地址」）
	need=""
	[ -n "$MANAGED" ] || { [ "$STATE" = update ] && [ "$PORT" = "$OLDPORT" ]; } || need=1
	pick_host "$need"
	pick_web
	[ "$MODE" != docker ] || pull_image
	if port_busy "$PORT" && ! { [ "$PORT" = "$OLDPORT" ] && svc_running; }; then
		die port "端口 ${PORT} 被别的程序占着：换一个（--port）"
	fi

	prepare
	if [ "$MODE" = docker ]; then
		write_env
		if ! start_docker; then
			[ -n "$AUTO" ] && have_systemd || die start "docker compose up 没成（上面是它说的）"
			say "Docker compose 起不来（上面是它说的）：改用 systemd + 系统的 python3"
			MODE=systemd IMG=""
		fi
	fi
	if [ "$MODE" = systemd ]; then
		write_env
		start_systemd
	fi
	wait_served
	# 服务用这一份 .env 起来了：之后再断（打结果时 SSH 断了是 SIGPIPE）、被打断也不换回，不然跑着的服务和 .env 对不上
	ROLLBACK=""
	rm -f "$ROOT$D/.env.prev"
	# 共用的证书助手：网页端开着就登记（以前它自己的证书助手的证书、账户搬过去）、起它；没开就去掉登记（没有 Team 用了就停掉它）
	if [ -n "$WEBON" ]; then acme_start; wait_cert; else acme_leave; fi
	if [ -n "$MANAGED" ]; then report_managed; else report; fi
}

main "$@"
