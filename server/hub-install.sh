#!/bin/sh
# cca 自己的同步服务器（cca hub）的一键安装：在 Linux 服务器上用 root 运行，Docker compose（compose 插件）或者 systemd + 系统的 python3（3.6 起）。
#
#   sudo sh cca-hub-install.sh [--host 公网地址] [--port 端口] [--docker | --systemd] [--dir 目录]
#       第一次：装好、起来，打出加入链接。链接只显示这一次：服务器上只存入伙口令的 SHA-256，盐不落盘。在每台设备上运行
#       cca hub join，贴这一行
#       再运行：更新服务端程序，数据、证书、加入链接都不变（还没有设备加入过时换一个新的加入链接）
#   sudo sh cca-hub-install.sh --reset       换一个新的加入链接：服务器忘掉所有设备和经它同步的数据，每台设备都要重新加入
#   sudo sh cca-hub-install.sh --uninstall   停掉、删掉（连同数据）
#
# Team 的同步服务器（cca team）也是这一份装：
#   sudo sh cca-hub-install.sh --team-root 'ssh-ed25519 AAAA…' [--host 公网地址] [--port 端口] [--docker | --systemd] [--dir 目录]
#       根公钥是 cca team new 给的那一行（公开的）。每个 Team 一个服务：服务名、系统用户、容器名是 cca-team-<Team 编号的前 8 位>，
#       装在 /opt/<服务名>，端口没写时从 18445 起找一个空着的——所以一台服务器上能装几个 Team，互不相干。装好了打出管理员接上它的那一行
#       （cca team attach …）。再运行（带 --team-root，或者 --dir 那个目录）是更新；--uninstall --dir 目录 删掉那一个
#
# 也可以在自己的电脑上运行 cca hub deploy root@服务器：经 SSH 装上这一份，加入链接在电脑上生成，那台电脑跟着加入。
# 装在 /opt/cca-hub（--dir 换：新的、空的目录，或者以前装过的；自己的同步服务器一台服务器只装一个）：app/cca-presence.py 服务端（只用 Python 标准库），
# certs/ 自签的证书（私钥不出这台服务器），data/ 设备和密文，.env 运行方式、端口、入伙口令的哈希，.cca-hub 记着这个目录是它装的。
# 不改 Docker 的全局配置，不动防火墙（要放行哪个端口最后会说）。
#
# 这一份是 cca 仓库里的模板（server/hub-install.sh）：辅助程序在下面标着 CCA-HUB-EMBED 的那一行放进版本和服务端程序，
# 才是能运行的安装脚本（cca hub deploy --script 打出来的、发布页上的 cca-hub-install.sh）。格式见 server/README.md「自己的同步服务器」
# @CCA-HUB-EMBED@

set -u
umask 022

ROOT=${CCA_HUB_ROOT:-}     # 只给测试用（tests/hub_box.py）：所有文件都放在这个目录下面
D=/opt/cca-hub
UNIT=/etc/systemd/system/cca-hub.service
SVC=cca-hub                # systemd 的服务名和系统用户、Docker 的容器名（自己的同步服务器一台服务器只装一个；Team 的各有各的：team_setup）
WHAT=自己的同步服务器      # 说话时怎么叫它（Team 的是「Team 的同步服务器」）
SMODE=own                  # 服务端的 --mode
# Team 的同步服务器：根公钥（cca team deploy 放进的，或者 --team-root；这里只留 ssh-ed25519 后面那一段，没有空格好写进 .env）
TEAMROOT=${CCA_HUB_TEAM_ROOT:-}
DIRSET=""                  # 命令里写了 --dir
CUID=10001                 # 容器里运行服务端的用户（compose.yaml 的 user）
DEF_PORT=18445
# 官方的 python 镜像：上次用的那个，再依次试官方镜像的几个同步源（国内常常连不上 Docker Hub）。和 policy.sh 部署设备统计的服务器时的一样
IMAGES="public.ecr.aws/docker/library/python:3.13-slim mirror.ccs.tencentyun.com/library/python:3.13-slim docker.m.daocloud.io/library/python:3.13-slim python:3.13-slim"
# cca hub deploy 经 SSH 运行时（上面那一行放进了这几个）：入伙口令和盐在电脑上生成，这里只拿到口令的哈希（WANT）；电脑已经加入了时
# EXPECT 是它加入的那一份的哈希（服务器上现在的不是它就停下：not_this）。进度写 stderr，结果一行一项（CCA-HUB 键 值）写 stdout
MANAGED=${CCA_HUB_MANAGED:-}
WANT=${CCA_HUB_WANT:-}
EXPECT=${CCA_HUB_EXPECT:-}
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
用法: sudo sh cca-hub-install.sh [--host 公网地址] [--port 端口] [--docker | --systemd] [--dir 目录] [--reset]
      sudo sh cca-hub-install.sh --team-root 'ssh-ed25519 AAAA…' [--host 公网地址] [--port 端口] [--docker | --systemd] [--dir 目录]
      sudo sh cca-hub-install.sh --uninstall [--dir 目录]
EOF
}
tmo() { if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi; }   # busybox、coreutils 都有 timeout；没有就不限时
env_get() { [ -f "$ROOT$D/.env" ] && sed -n "s/^$1=//p" "$ROOT$D/.env" | tail -n 1; }
hexof() { od -An -tx1 | tr -d ' \n'; }
rand_hex() { od -An -tx1 -N16 /dev/urandom | tr -d ' \n'; }   # 16 个随机字节，32 个小写十六进制字符
sha256_hex() { printf '%s' "$1" | openssl dgst -sha256 -binary | hexof; }
hash_ok() { case "$1" in ''|*[!0-9a-f]*) return 1 ;; esac; [ ${#1} -eq 64 ]; }   # 入伙口令的哈希：64 个小写十六进制字符
pin_of() {   # 证书公钥的指纹（curl --pinnedpubkey 的写法去掉开头的 sha256//）：从证书文件读，或者从 stdin（s_client 的输出）读
	openssl x509 ${1:+-in "$1"} -pubkey -noout 2>/dev/null | openssl pkey -pubin -outform der 2>/dev/null |
		openssl dgst -sha256 -binary 2>/dev/null | openssl base64 -A 2>/dev/null
}
served_pin() { tmo 8 openssl s_client -connect "127.0.0.1:${PORT}" </dev/null 2>/dev/null | pin_of; }
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
DCP=""   # Team 的：compose 的项目名照服务名（-p）。不写的话是目录名：两个 Team 装在同名的目录里（/a/hub、/b/hub）会互相当成孤儿删掉
dc() { (cd "$ROOT$D" && docker compose $DCP -f compose.yaml "$@") </dev/null; }
have_docker() { command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 && docker info >/dev/null 2>&1; }
PY=""
have_systemd() {   # systemd 在管这台机器（/run/systemd/system 在），有 python3 3.6 起、带 ssl
	[ -d "$ROOT/run/systemd/system" ] && command -v systemctl >/dev/null 2>&1 || return 1
	PY=$(command -v python3 2>/dev/null) || return 1
	"$PY" -c 'import sys, ssl, selectors, socketserver, http.server; sys.exit(0 if sys.version_info >= (3, 6) else 1)' >/dev/null 2>&1
}
svc_running() {   # 上次装的那个现在在运行
	case "$OLD" in
		docker) [ "$(docker inspect -f '{{.State.Running}}' "$SVC" 2>/dev/null)" = true ] ;;
		systemd) systemctl is-active --quiet "$SVC" 2>/dev/null ;;
		*) return 1 ;;
	esac
}
port_busy() {   # 有程序在这个 TCP 端口上监听（ss、netstat 都没有时当作没有）
	if command -v ss >/dev/null 2>&1; then ss -ltn 2>/dev/null
	elif command -v netstat >/dev/null 2>&1; then netstat -ltn 2>/dev/null
	fi | awk '{ print $4 }' | grep -Eq "[:.]$1\$"
}
port_taken() {   # 自动挑端口时：有程序在听，或者这台服务器上别的同步服务器（默认目录里的）记着用它（没有 ss、netstat 时只看得到这个）
	port_busy "$1" && return 0
	# 先接起来再找：有的文件不在时 grep 的退出码是 2（找到了也是），接起来就只看找没找到
	cat "$ROOT"/opt/cca-team-*/.env "$ROOT/opt/cca-hub/.env" 2>/dev/null | grep -qx "CCA_HUB_PORT=$1"
}
joined_ever() {   # 现在的入伙口令有设备加入过：服务器登记了盐的校验值（状态文件读不懂时也当作加入过：不能随手换掉口令）
	[ -f "$ROOT$D/data/state.json" ] || return 1
	[ "$(sed -n 's/.*"own":{[^}]*"check":\([^,}]*\).*/\1/p' "$ROOT$D/data/state.json" | head -n 1)" != null ]
}
public_ip() {   # 默认路由出去的源地址，是公网地址才认（只查路由表，不发请求；云服务器的公网 IP 常常是 NAT 进来的，机器上看不到）
	ip=$(ip -4 route get 192.0.2.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
	case "$ip" in
		''|0.*|10.*|127.*|169.254.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*) ip="" ;;
		100.6[4-9].*|100.[7-9][0-9].*|100.1[01][0-9].*|100.12[0-7].*) ip="" ;;   # 运营商级 NAT（100.64.0.0/10）
	esac
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
fw_hint() {   # 这台服务器自己的防火墙没放行这个端口：说怎么放行（只看，不改）。Docker 发布的端口不经 ufw、firewalld 的规则，systemd 的要
	[ "$MODE" = systemd ] || return 0
	if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q '^Status: active'; then
		ufw status 2>/dev/null | grep -Eq "^${PORT}(/tcp)? .*ALLOW" || say "! 这台服务器开着 ufw：运行 ufw allow ${PORT}/tcp 放行"
	elif command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
		firewall-cmd --query-port="${PORT}/tcp" >/dev/null 2>&1 ||
			say "! 这台服务器开着 firewalld：运行 firewall-cmd --permanent --add-port=${PORT}/tcp && firewall-cmd --reload 放行"
	fi
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
team_setup() {   # Team 的同步服务器：每个 Team 一个服务（名字里带 Team 编号的前 8 位：根公钥的 SHA-256），目录默认 /opt/<服务名>
	TEAMROOT=${TEAMROOT#ssh-ed25519 }
	TEAMROOT=${TEAMROOT%% *}
	case "$TEAMROOT" in *[!A-Za-z0-9+/]*) die usage "Team 的根公钥不对（要 cca team new 给的那一行：ssh-ed25519 AAAA…）" ;; esac
	[ ${#TEAMROOT} -eq 68 ] || die usage "Team 的根公钥不对（要 cca team new 给的那一行：ssh-ed25519 AAAA…）"
	TID=$(printf '%s' "$TEAMROOT" | openssl base64 -d -A 2>/dev/null | openssl dgst -sha256 -binary 2>/dev/null | hexof | cut -c1-8)
	[ ${#TID} -eq 8 ] || die install "算不出 Team 的编号（openssl）"
	SVC=cca-team-$TID
	UNIT=/etc/systemd/system/$SVC.service
	WHAT="Team 的同步服务器" SMODE=team DCP="-p $SVC"
	[ -n "$DIRSET" ] || D=/opt/$SVC
}
team_here() {   # 这个目录里装的是哪个 Team 的（上次写的 .env）：没写 --team-root 时照它（再运行是更新、卸载认得出服务名）
	[ -z "$TEAMROOT" ] || return 0
	TEAMROOT=$(env_get CCA_TEAM_ROOT)
	case "$TEAMROOT" in *[!A-Za-z0-9+/]*) TEAMROOT="" ;; esac   # .env 里的值坏了：当作没有（下面照 .cca-hub 里记的认）
	[ ${#TEAMROOT} -eq 68 ] || TEAMROOT=""
	if [ -n "$TEAMROOT" ]; then
		DIRSET=1   # 目录就是读到它的这一个，不换成默认的
		team_setup
		return 0
	fi
	# .env 不见了（或者坏了）：.cca-hub 里记着服务名。是 Team 的话，卸载照它停服务、删用户；别的要带上 --team-root（不然会当成自己的
	# 同步服务器装进这个目录）
	s=$(sed -n 's/^# svc=\(cca-team-[0-9a-f]\{8\}\)$/\1/p' "$ROOT$D/.cca-hub" 2>/dev/null | head -n 1)
	[ -n "$s" ] || return 0
	[ -n "$UNINSTALL" ] || die usage "${D} 里装的是 Team 的同步服务器，它的 .env 不见了：带上 --team-root '根公钥' 再运行"
	SVC=$s UNIT=/etc/systemd/system/$s.service WHAT="Team 的同步服务器" SMODE=team DCP="-p $s"
}
dir_ours() {   # 能装在这个目录：还没有、空的，或者是以前装的（有 .cca-hub 或者 .env）。别的目录不碰：里面的 data、certs 会被 chown，卸载时会删掉
	[ -d "$ROOT$D" ] || return 0
	[ -f "$ROOT$D/.cca-hub" ] || [ -f "$ROOT$D/.env" ] || [ -z "$(ls -A "$ROOT$D" 2>/dev/null)" ]
}
other_dir() {   # 这台服务器上别的目录里装着的那一个（systemd 的 unit 里写着）
	[ -f "$ROOT$UNIT" ] && sed -n 's/^EnvironmentFile=\(.*\)\/\.env$/\1/p' "$ROOT$UNIT" | grep -vxF "$D" | head -n 1
}
other_kind() {   # 这个目录里装着的是另一种（自己的同步服务器 / Team 的）、或者另一个 Team 的：说是什么；不是返回 1
	t=$(env_get CCA_TEAM_ROOT)
	if [ -n "$TEAMROOT" ]; then
		if [ -n "$t" ] && [ "$t" != "$TEAMROOT" ]; then printf '另一个 Team 的同步服务器'
		elif [ -z "$t" ] && [ -n "$(env_get CCA_HUB_ADMIT_HASH)" ]; then printf '自己的同步服务器（cca hub）'
		else return 1; fi
	elif [ -n "$t" ]; then printf 'Team 的同步服务器'
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
pick_state() {   # STATE：new 第一次装、update 只更新、replace 还没有设备加入过（换一个新的口令）、reset 从头来；H：这一次的入伙口令的哈希
	if [ -n "$TEAMROOT" ]; then   # Team 的：没有口令（设备拿管理员发的邀请加入）。装过这个 Team 的就是更新
		[ -z "$RESET" ] || die usage "Team 的同步服务器没有 --reset：设备、邀请都由管理员的设备管（cca team）"
		H=""
		if [ -n "$(env_get CCA_TEAM_ROOT)" ]; then STATE=update; else STATE=new; fi
		return 0
	fi
	E=$(env_get CCA_HUB_ADMIT_HASH)
	hash_ok "$E" || E=""
	if [ -n "$MANAGED" ]; then
		hash_ok "$WANT" || die usage "入伙口令的哈希不对"
		if [ -n "$EXPECT" ]; then
			hash_ok "$EXPECT" || die usage "入伙口令的哈希不对"
			[ -n "$E" ] || die none "这台服务器上（${D}）没有装自己的同步服务器"
			[ "$E" = "$EXPECT" ] || die not_this "这台服务器上的自己的同步服务器用的是别的加入链接"
		fi
		H=$WANT
		if [ -z "$E" ]; then STATE=new
		elif [ "$E" = "$WANT" ]; then STATE=update
		elif [ -n "$RESET" ]; then STATE=reset
		elif joined_ever; then die taken "这台服务器上已经有自己的同步服务器，也有设备加入过"
		else STATE=replace
		fi
		return 0
	fi
	if [ -z "$E" ]; then STATE=new
	elif [ -n "$RESET" ]; then STATE=reset
	elif joined_ever; then STATE=update
	else STATE=replace
	fi
	if [ "$STATE" = update ]; then H=$E; return 0; fi
	ADMIT=$(rand_hex) SALT=$(rand_hex)
	[ ${#ADMIT} -eq 32 ] && [ ${#SALT} -eq 32 ] || die install "读不了 /dev/urandom"
	H=$(sha256_hex "$ADMIT")
	hash_ok "$H" || die install "算不出入伙口令的哈希（openssl）"
}
pick_port() {   # PORT：--port，没写时照上次的、再是 18445。update 时不能换（已经加入的设备认的是原来的端口）
	OLDPORT=$(env_get CCA_HUB_PORT)
	if [ -n "$TEAMROOT" ]; then
		# Team 的：没写时照上次的，第一次装从 18445 起找一个空着的（一台服务器上几个 Team 各用各的端口）。换端口可以：管理员重新接上
		# （cca team attach 新地址），各台设备照配置里的新地址换过来
		if [ -z "$PORT" ] && [ -n "$OLDPORT" ]; then PORT=$OLDPORT
		elif [ -z "$PORT" ]; then
			PORT=$DEF_PORT
			i=0
			while port_taken "$PORT" && [ $i -lt 50 ]; do PORT=$((PORT + 1)); i=$((i + 1)); done
		fi
		return 0
	fi
	[ -n "$PORT" ] || PORT=${OLDPORT:-$DEF_PORT}
	if [ -n "$OLDPORT" ] && [ "$PORT" != "$OLDPORT" ] && [ "$STATE" = update ]; then
		die port "已经有设备经端口 ${OLDPORT} 加入了：换了端口它们就连不上。要换端口得 --reset（每台设备都要用新的加入链接重新加入）"
	fi
}
pick_host() {   # 加入链接里的地址（只有这里打出链接时要）：--host，或者这台机器上看得到的公网 IP，都没有时在终端里问。HP 是链接里的写法
	[ -n "$HOST" ] || HOST=$(public_ip)
	if [ -z "$HOST" ] && (: </dev/tty) 2>/dev/null; then
		printf '这台服务器的公网 IP 或者域名（各台设备经它连过来）: ' >/dev/tty
		read -r HOST </dev/tty || HOST=""
	fi
	[ -n "$HOST" ] || die host "认不出这台服务器的公网地址（云服务器的公网 IP 常常不在机器上）：加上 --host 公网IP或域名"
	case "$HOST" in
		*:*) case "$HOST" in *[!0-9A-Fa-f:]*) die usage "--host 写错了：${HOST}" ;; esac; HP="[$HOST]" ;;
		*[!A-Za-z0-9.-]*|-*|.*) die usage "--host 写错了：${HOST}" ;;
		*) HP=$HOST ;;
	esac
}
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
prepare() {   # 目录（.cca-hub 记着是这里装的）、服务端程序、证书（没有才生成：NEWCERT）；PIN 是证书公钥的指纹
	mkdir -p "$ROOT$D/app" "$ROOT$D/certs" "$ROOT$D/data" || die install "建不了 ${D}"
	printf '# cca 的同步服务器（%s）装在这个目录：安装脚本认这个文件（再运行是更新，--uninstall 删掉它装的那些）\n# svc=%s\n' "$WHAT" "$SVC" > "$ROOT$D/.cca-hub" ||
		die install "写不了 ${D}/.cca-hub"
	chmod 755 "$ROOT$D" "$ROOT$D/app" && chmod 700 "$ROOT$D/certs" "$ROOT$D/data" || die install "改不了 ${D} 的权限"
	put "$ROOT$D/app/cca-presence.py" 644 hub_server_py
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
	if [ -n "$TEAMROOT" ]; then printf 'CCA_HUB_MODE=%s\nCCA_HUB_PORT=%s\nCCA_TEAM_ROOT=%s\n' "$MODE" "$PORT" "$TEAMROOT"
	else printf 'CCA_HUB_MODE=%s\nCCA_HUB_PORT=%s\nCCA_HUB_ADMIT_HASH=%s\n' "$MODE" "$PORT" "$H"; fi
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
compose_yaml() {   # @SVC@、@SMODE@、@SECRET@ 换成这一种的：服务名（容器名）、--mode、交给服务端的那一项（口令的哈希 / Team 的根公钥）
	if [ -n "$TEAMROOT" ]; then secret=CCA_TEAM_ROOT; else secret=CCA_HUB_ADMIT_HASH; fi
	sed -e "s/@SVC@/$SVC/g" -e "s/@SMODE@/$SMODE/g" -e "s/@SECRET@/$secret/g" <<'EOF'
# cca 的同步服务器（@SVC@）：安装脚本写的，再运行安装脚本时会换掉。设置在 .env 里
services:
  @SVC@:
    image: ${CCA_HUB_IMAGE:-python:3.13-slim}
    container_name: @SVC@
    restart: unless-stopped
    user: "10001:10001"
    command: ["python3", "-I", "/app/cca-presence.py", "--mode", "@SMODE@", "--listen", "0.0.0.0:18445", "--cert", "/certs/cert.pem", "--key", "/certs/key.pem", "--state", "/data/state.json"]
    ports:
      - "${CCA_HUB_PORT:-18445}:18445"
    environment:
      @SECRET@: ${@SECRET@:?}
    volumes:
      - ./app:/app:ro,z
      - ./certs:/certs:ro,z
      - ./data:/data:z
    read_only: true
    tmpfs:
      - /tmp:size=8m
    cap_drop: [ALL]
    security_opt:
      - no-new-privileges:true
    mem_limit: 256m
    pids_limit: 64
    logging:
      driver: json-file
      options:
        max-size: "1m"
        max-file: "3"
EOF
}
unit_file() {
	cat <<EOF
# cca 的同步服务器（${SVC}）：安装脚本写的，再运行安装脚本时会换掉。设置在 $D/.env 里
[Unit]
Description=cca ${WHAT}（${SVC}）
After=network-online.target
Wants=network-online.target

[Service]
User=$SVC
EnvironmentFile=$D/.env
ExecStart=$PY -I $D/app/cca-presence.py --mode $SMODE --listen 0.0.0.0:\${CCA_HUB_PORT} --cert $D/certs/cert.pem --key $D/certs/key.pem --state $D/data/state.json
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=full
ProtectHome=yes
CapabilityBoundingSet=
TasksMax=64
MemoryMax=256M
LimitNOFILE=4096

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
stop_docker() {   # 停掉、删掉容器（这个目录里有 compose.yaml 时）
	[ -f "$ROOT$D/compose.yaml" ] || return 0
	if command -v docker >/dev/null 2>&1; then
		dc down >/dev/null 2>&1 || die install "停不掉原来的 Docker 容器（docker compose down 没成）：Docker 在运行吗？"
	fi
	rm -f "$ROOT$D/compose.yaml"
}
stop_systemd() {
	[ -f "$ROOT$UNIT" ] || return 0
	systemctl disable --now "$SVC" </dev/null >/dev/null 2>&1
	rm -f "$ROOT$UNIT"
	systemctl daemon-reload </dev/null >/dev/null 2>&1
}
start_docker() {   # Docker compose 起服务。起不来时把 compose 说的打出来（缩进，写 stderr），返回 1
	put "$ROOT$D/compose.yaml" 644 compose_yaml
	chown -R "$CUID:$CUID" "$ROOT$D/certs" "$ROOT$D/data" || die install "chown 失败"
	stop_systemd
	out=$(dc up -d --force-recreate --remove-orphans 2>&1) && return 0
	printf '%s\n' "$out" | tail -n 20 | sed 's/^/    /' >&2
	return 1
}
start_systemd() {
	id "$SVC" >/dev/null 2>&1 || useradd -r -M -d "$D" -s "$(nologin)" "$SVC" </dev/null >/dev/null 2>&1 || die install "建不了系统用户 ${SVC}（useradd）"
	chown -R "$SVC" "$ROOT$D/certs" "$ROOT$D/data" || die install "chown 失败"
	stop_docker
	put "$ROOT$UNIT" 644 unit_file
	systemctl daemon-reload </dev/null >/dev/null 2>&1 && systemctl enable "$SVC" </dev/null >/dev/null 2>&1 &&
		systemctl restart "$SVC" </dev/null >/dev/null 2>&1 || { logs; die start "systemd 起不来 ${SVC}（上面是它的日志）"; }
}
wait_served() {   # 等它在 127.0.0.1:PORT 上用这张证书回话（最多 30 秒）
	i=0
	while [ "$(served_pin)" != "$PIN" ]; do
		i=$((i + 1))
		[ $i -lt 30 ] || { logs; die start "服务起来了，30 秒里却没在 127.0.0.1:${PORT} 上用这张证书回话（上面是它的日志）"; }
		sleep 1
	done
}

# --- 说结果 ------------------------------------------------------------------------------------------------------------------
how() {
	if [ "$MODE" = docker ]; then printf 'Docker compose，镜像 %s' "$IMG"
	else printf 'systemd + %s' "$("$PY" -c 'import platform; print("python " + platform.python_version())' 2>/dev/null)"
	fi
}
report_managed() {   # 给 hub-deploy：一行一项（deploy.rs 的 report 读）
	say "${WHAT}在运行（${D}，$(how)，端口 ${PORT}）"
	fw_hint
	note "mode $MODE"
	note "port $PORT"
	note "pin sha256//$PIN"
	note "state $STATE"
	note ok
}
report_team() {   # 直接在服务器上运行的、Team 的：打出管理员接上它的那一行（地址和指纹都是公开的）
	say "✓ Team 的同步服务器在运行（${D}，$(how)，端口 ${PORT}，服务名 ${SVC}）"
	if [ "$STATE" = update ]; then
		say "  更新了服务端程序，数据都没变"
		[ -z "$NEWCERT" ] || say "! 证书是新生成的（原来的不见了）：管理员要照下面这一行重新接上，各台设备取到新的配置以后才连得上"
		[ -n "$NEWCERT" ] || [ "$PORT" != "$OLDPORT" ] || { fw_hint; return 0; }
		[ -n "$HP" ] || HP=这台服务器的地址
	fi
	say ""
	say "在管理员的设备上接上它（Team 写名字或者编号）："
	say ""
	say "  cca team attach Team https://${HP}:${PORT} sha256//${PIN}"
	say ""
	fw_hint
	say "云服务商的安全组（或轻量应用服务器的防火墙）里要放行 TCP ${PORT}。"
}
report() {   # 直接在服务器上运行的：新的加入链接打出来（只这一次）
	if [ -n "$TEAMROOT" ]; then report_team; return; fi
	say "✓ 自己的同步服务器在运行（${D}，$(how)，端口 ${PORT}）"
	if [ "$STATE" = update ]; then
		say "  更新了服务端程序，加入链接和数据都没变。加入链接在已经加入的设备上运行 cca hub invite 看"
		[ -z "$NEWCERT" ] || say "! 证书是新生成的（原来的不见了）：已经加入的设备连不上了，要 sudo sh $0 --reset 换一个加入链接、每台设备重新加入"
	else
		# 链接的格式见 server/README.md「自己的同步服务器」（cli/src/cmd/hub.rs 的 link_encode 也照它）
		json=$(printf '{"u":"https://%s:%s","p":["sha256//%s"],"a":"%s","s":"%s"}' "$HP" "$PORT" "$PIN" "$ADMIT" "$SALT")
		head="cca-hub1.$(printf '%s' "$json" | openssl base64 -A | tr '+/' '-_' | tr -d '=')"
		sum=$(printf '%s' "$head" | openssl dgst -sha256 -binary | hexof | cut -c1-4)
		say ""
		say "加入链接（只显示这一次：服务器上不存它。谁拿到它都能加入，只发给自己的设备）："
		say ""
		say "  $head.$sum"
		say ""
		say "在每台设备上运行 cca hub join，贴这一行。"
		[ "$STATE" != reset ] || say "服务器忘掉了以前加入的设备：已经加入过的设备先 cca hub leave，再 cca hub join 贴新的这一行。"
		[ -t 1 ] || say "! 输出没接在终端上：加入链接可能被记进了日志，设备都加入以后删掉那份日志（或者用 --reset 换一个）"
	fi
	fw_hint
	say "云服务商的安全组（或轻量应用服务器的防火墙）里要放行 TCP ${PORT}。"
}

uninstall() {   # 停掉、删掉这个目录里装的（只认装过的目录：有 .cca-hub 或者 .env）和系统用户
	[ -f "$ROOT$D/.cca-hub" ] || [ -f "$ROOT$D/.env" ] || die none "${D} 里没有装${WHAT}"
	o=$(other_dir)
	stop_docker
	[ -n "$o" ] || stop_systemd
	rm -rf "$ROOT$D/app" "$ROOT$D/certs" "$ROOT$D/data"
	rm -f "$ROOT$D/.env" "$ROOT$D/.env.prev" "$ROOT$D/.cca-hub"
	rmdir "$ROOT$D" 2>/dev/null
	[ -n "$o" ] || ! id "$SVC" >/dev/null 2>&1 || userdel "$SVC" >/dev/null 2>&1
	if [ "$SMODE" = team ]; then say "✓ 卸载了 Team 的同步服务器（${D} 里的程序、证书、数据都删了）。管理员要换一台接上（cca team attach），或者各台设备退出（cca team leave）"
	else say "✓ 卸载了自己的同步服务器（${D} 里的程序、证书、数据都删了）。加入过的设备上运行 cca hub leave"; fi
	note ok
}

main() {
	HOST="" PORT="" MODE="" RESET="" UNINSTALL="" AUTO="" IMG="" NEWCERT="" ADMIT="" SALT="" HP=""
	trap interrupted HUP INT TERM PIPE
	while [ $# -gt 0 ]; do
		case "$1" in
			--host|--port|--dir|--team-root)
				[ $# -ge 2 ] || { usage >&2; exit 2; }
				case "$1" in --host) HOST=$2 ;; --port) PORT=$2 ;; --team-root) TEAMROOT=$2 ;; *) D=$2 DIRSET=1 ;; esac
				shift ;;
			--docker) MODE=docker ;;
			--systemd) MODE=systemd ;;
			--reset) RESET=1 ;;
			--uninstall) UNINSTALL=1 ;;
			-h|--help) usage; exit 0 ;;
			*) usage >&2; exit 2 ;;
		esac
		shift
	done
	command -v hub_server_py >/dev/null 2>&1 || die usage "这是 cca 仓库里的模板：要运行的是 cca hub deploy --script 生成的那一份（发布页上的 cca-hub-install.sh）"
	if [ -n "$TEAMROOT" ]; then
		command -v openssl >/dev/null 2>&1 || die deps "要 openssl（生成证书、算指纹）：$(install_cmd openssl)"
		team_setup
	fi
	dir_check
	team_here   # 没写 --team-root、这个目录里装的是 Team 的：照它（更新、卸载）
	if [ -n "$PORT" ]; then
		case "$PORT" in 0*|*[!0-9]*) die usage "--port 要是 1024～65535 的端口" ;; esac
		[ "$PORT" -ge 1024 ] && [ "$PORT" -le 65535 ] || die usage "--port 要是 1024～65535 的端口"
	fi
	if [ "$(id -u 2>/dev/null)" != 0 ]; then
		[ -n "$MANAGED" ] && die root "要用 root 登录服务器（或者能免密码 sudo 的用户）"
		die root "要用 root 运行：sudo sh $0 …"
	fi
	if [ -n "$UNINSTALL" ]; then
		o=$(other_kind) && die taken "${D} 里装着的是${o}，不是要卸载的这一个：看清楚 --dir、--team-root 再运行"
		uninstall
		return
	fi
	command -v openssl >/dev/null 2>&1 || die deps "要 openssl（生成证书、算指纹）：$(install_cmd openssl)"
	dir_ours || die usage "${D} 里已经有别的东西（不是装过的${WHAT}）：换一个新的目录（--dir）"
	o=$(other_kind) && die taken "${D} 里装着的是${o}：换一个目录（--dir），或者先卸载它（--uninstall --dir ${D}）"
	o=$(other_dir)
	[ -z "$o" ] || die usage "这台服务器上已经在 ${o} 装了${WHAT}（一台只装一个；Team 的是每个 Team 一个）：照旧用 --dir ${o}，或者先卸载它（--uninstall --dir ${o}）"
	pick_mode
	pick_state
	pick_port
	# 直接在服务器上运行、要打出地址的才问：第一次装；Team 的换了端口、证书重新生成时也要（那时要重新接上）
	[ -n "$MANAGED" ] || [ "$STATE" = update ] || pick_host
	if [ -z "$MANAGED" ] && [ -n "$TEAMROOT" ] && [ "$STATE" = update ] && [ "$PORT" != "$OLDPORT" ]; then pick_host; fi
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
	# （--reset 换掉的旧链接下次重启又能用了）
	ROLLBACK=""
	rm -f "$ROOT$D/.env.prev"
	if [ -n "$MANAGED" ]; then report_managed; else report; fi
}

main "$@"
