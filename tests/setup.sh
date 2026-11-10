#!/bin/bash
# 测试沙箱：假 HOME、假路由器根目录、假命令、假 GitHub Releases（不联网，不运行任何下载来的二进制）
set -e
T="$1"
rm -rf "$T"; mkdir -p "$T"/{bin,rbin,home/.local/bin,risk}
R="$T/router"; mkdir -p "$R"/etc/{openclash/config,openclash/custom,openclash/core,init.d,crontabs,dropbear} "$R"/root/.ssh "$R"/tmp

# --- 假 ~/.claude 与 ~/.claude.json
H="$T/home"; C="$H/.claude"
mkdir -p "$C"/projects/-p1/memory "$C"/skills/s1 "$C"/plugins "$C"/sessions "$C"/statsig "$C"/state
echo "# global instructions" > "$C/CLAUDE.md"
echo '{"model":"opus","theme":"dark"}' > "$C/settings.json"
printf '%s\n' '{"display":"old 1"}' '{"display":"old 2"}' > "$C/history.jsonl"
echo "master memory" > "$C/projects/-p1/memory/MEMORY.md"
echo "skill" > "$C/skills/s1/SKILL.md"
echo '{"pid":1}' > "$C/sessions/1.json"
cat > "$H/.claude.json" <<'J'
{"numStartups": 5, "hasCompletedOnboarding": true,
 "oauthAccount": {"emailAddress": "default@example.com"},
 "projects": {"/p1": {"hasTrustDialogAccepted": true, "allowedTools": ["Bash(ls)"]}},
 "mcpServers": {"m1": {"command": "x"}}, "cachedGrowthBookFeatures": {"f": 1}}
J

# --- 假路由器（节点与真实路由器同名）
cat > "$R/etc/openclash/config/custom.yaml" <<'Y'
port: 7890
socks-port: 7891
redir-port: 7892
mixed-port: 7893
tproxy-port: 7895
allow-lan: true
external-controller: 0.0.0.0:9090
secret: 'test-secret'
authentication:
  - "user:pass"
dns:
  enable: true
  listen: 0.0.0.0:7874
proxies:
  - {name: US-A, type: ss, server: 192.0.2.1, port: 8388, cipher: aes-128-gcm, password: x}
  - {name: US-B, type: ss, server: 192.0.2.2, port: 8388, cipher: aes-128-gcm, password: x}
  - {name: US-C, type: ss, server: 192.0.2.3, port: 8388, cipher: aes-128-gcm, password: x}
  - {name: US-D, type: ss, server: 192.0.2.4, port: 8388, cipher: aes-128-gcm, password: x}
  - {name: HK-A, type: ss, server: 192.0.2.5, port: 8388, cipher: aes-128-gcm, password: x}
proxy-groups:
  - {name: "🚀 节点选择", type: select, proxies: [US-A, US-B]}
rules:
  - MATCH,🚀 节点选择
Y
cp "$R/etc/openclash/config/custom.yaml" "$R/etc/openclash/custom.yaml"
cat > "$R/etc/openclash/custom/openclash_custom_overwrite.sh" <<'O'
#!/bin/sh
. /usr/share/openclash/ruby.sh
. /usr/share/openclash/log.sh

LOG_OUT "Tip: Start Running Custom Overwrite Scripts..."
LOG_FILE="/tmp/openclash.log"
CONFIG_FILE="$1"

exit 0
O
printf '%s' "$(cat "$R/etc/openclash/custom/openclash_custom_overwrite.sh")" > "$R/etc/openclash/custom/ow.tmp" && mv "$R/etc/openclash/custom/ow.tmp" "$R/etc/openclash/custom/openclash_custom_overwrite.sh"
cp "$R/etc/openclash/custom/openclash_custom_overwrite.sh" "$T/pristine_overwrite.sh"
printf '%s\n' '/root/.ssh/' > "$R/etc/sysupgrade.conf"
printf '7890\n7891\n7892\n7893\n7895\n9090\n' > "$R/listening"
printf '#!/bin/sh\necho "cron $1" >> "%s/router.log"\n' "$T" > "$R/etc/init.d/cron"
# 假的 mihomo 内核（只管 -t 校验）：有 router/core_fail 时一律不通过；router/core_reject 里写着一段文字时，
# 要校验的配置里有它就不通过（模拟这台路由器的内核不认参考配置里的某个节点）
fake_core() {
	cat <<C
#!/bin/sh
echo "$1 \$*" >> "$T/router.log"
[ -f "$T/router/core_fail" ] && { echo "level=error msg=parse config error"; exit 1; }
f=""; p=""; for a in "\$@"; do [ "\$p" = -f ] && f="\$a"; p="\$a"; done
[ -f "$T/router/core_reject" ] && [ -n "\$f" ] && grep -qF "\$(cat "$T/router/core_reject")" "\$f" && { echo "level=error msg=proxy 3: unsupport proxy type"; exit 1; }
exit 0
C
}
fake_core 'CORE SAFE_PATHS=$SAFE_PATHS' > "$R/etc/openclash/clash"
chmod +x "$R/etc/init.d/cron" "$R/etc/openclash/clash"

# --- 路由器侧假命令（rbin）
printf '#!/bin/sh\nexec /bin/dash "$@"\n' > "$T/rbin/sh"
# OpenWrt 默认的 busybox 没有 cksum（真实路由器上遇到过）：假路由器上也没有，路由器端脚本用了就测得出来
printf '#!/bin/sh\necho "cksum: not found" >&2\nexit 127\n' > "$T/rbin/cksum"
cat > "$T/rbin/uci" <<'S'
#!/bin/sh
case "$*" in
	*"get openclash.config.config_path"*) echo /etc/openclash/config/custom.yaml ;;
	*"get openclash.config.cn_port"*) echo 9090 ;;
	*"get openclash.config.dashboard_password"*) echo test-secret ;;
esac
S
# 假的 OpenClash 重启：按 OpenClash 自己的配置重新生成运行配置，跑 cca 的钩子（真实路由器上由覆写脚本调用），再让内核加载
cat > "$R/etc/init.d/openclash" <<'S'
#!/bin/sh
[ "$1" = restart ] || exit 0
R="$CCA_T/router"; RCF="$R/etc/openclash/custom.yaml"
cp "$R/etc/openclash/config/custom.yaml" "$RCF"
CCA_ROOT="$R" /bin/dash "$R/etc/cca/cca-router.sh" hook "$RCF"
CCA_FAKE_RESTART=1 "$CCA_T/rbin/curl" -s -X PUT --data "{\"path\":\"$RCF\"}" http://127.0.0.1:9090/configs >/dev/null
S
chmod +x "$R/etc/init.d/openclash"

# --- 假的 ShellCrash（juewuy/ShellCrash 1.9）：模板放在 $T/shellcrash，ShellCrash 那一节测试才拷进路由器根目录
S="$T/shellcrash"; mkdir -p "$S"/etc/ShellCrash/{configs,yamls,task} "$S"/etc/rc.d "$S"/tmp/ShellCrash
printf '%s\n' 'alias crash="ash /etc/ShellCrash/menu.sh"' 'export CRASHDIR="/etc/ShellCrash"' > "$S/etc/profile"
: > "$S/etc/rc.d/S99shellcrash"
printf '%s\n' '#ShellCrash配置文件，不明勿动！' 'crashcore=meta' 'secret=test-secret' 'redir_mod=混合模式' > "$S/etc/ShellCrash/configs/ShellCrash.cfg"
printf '%s\n' 'TMPDIR=/tmp/ShellCrash' 'BINDIR=/etc/ShellCrash' 'COMMAND="$TMPDIR/CrashCore -d $BINDIR -f $TMPDIR/config.yaml"' > "$S/etc/ShellCrash/configs/command.env"
{
	printf '%s\n' 'port: 7890' 'socks-port: 7891' 'allow-lan: true' 'mode: Rule' 'external-controller: :9090' 'proxies:'
	for n in US-A:1 US-B:2 US-C:3 US-D:4 HK-A:5; do
		printf '  - name: %s\n    type: ss\n    server: 192.0.2.%s\n    port: 8388\n    cipher: aes-128-gcm\n    password: x\n' "${n%%:*}" "${n##*:}"
	done
	printf '%s\n' 'proxy-groups:' '  - name: Proxy' '    type: select' '    proxies:' '      - US-A' '      - US-B' 'rules:' '  - MATCH,Proxy'
} > "$S/etc/ShellCrash/yamls/config.yaml"
# ShellCrash 的自定义节点：启动时才接到节点后面（# 后面是要加进的策略组），cca 用不上
echo '- {name: SC-X, type: ss, server: 192.0.2.9, port: 8388, cipher: aes-128-gcm, password: x} #Proxy' > "$S/etc/ShellCrash/yamls/proxies.yaml"
printf '#!/bin/sh\necho "TASK $*" >> "%s/router.log"\n' "$T" > "$S/etc/ShellCrash/task/task.sh"
echo '$CRASHDIR/task/task.sh 201 启动前测试任务' > "$S/etc/ShellCrash/task/bfstart"   # 用户自己的启动前任务
fake_core SCCORE > "$S/tmp/ShellCrash/CrashCore"
# start.sh restart|start 像真的一样：先 source task/bfstart（cca 的钩子在这里把 $core_config 换成插好端口池的副本），
# 再按 ShellCrash 的做法把订阅配置按顶格的键拆成几段、和它自己的设置拼成运行配置：自定义节点接在节点那段后面；
# 有 router/sc_vmess 时像它的 meta_listeners 一样追加一个两格缩进的入站。拼好的读不出来记 SCBAD。最后让（假的）内核加载
cat > "$S/etc/ShellCrash/start.sh" <<'S'
#!/bin/sh
case "$1" in start|restart) ;; *) exit 0 ;; esac
R="$CCA_T/router"; CRASHDIR="$R/etc/ShellCrash"; TMPDIR="$R/tmp/ShellCrash"; BINDIR="$CRASHDIR"
mkdir -p "$TMPDIR"
core_config="$CRASHDIR/yamls/config.yaml"
[ -s "$CRASHDIR/task/bfstart" ] && . "$CRASHDIR/task/bfstart"
echo "$core_config" > "$CCA_T/sc_core_config"
{
	printf 'mixed-port: 7890\nredir-port: 7892\ntproxy-port: 7893\nallow-lan: true\nexternal-controller: :9999\nsecret: test-secret\n'
	for s in proxies proxy-groups rules listeners; do
		sed -n "/^$s:/,/^[a-z]/{ /^[a-z]/d; p; }" "$core_config" > "$TMPDIR/$s.part"
		if [ "$s" = proxies ] && [ -s "$CRASHDIR/yamls/proxies.yaml" ]; then
			sp=$(grep -E '^ *- ' "$TMPDIR/$s.part" | head -n 1 | sed 's/-.*//')
			sed -e '/^#/d' -e "s/^ *- /$sp- /" -e 's/#.*/ #自定义代理/' "$CRASHDIR/yamls/proxies.yaml" >> "$TMPDIR/$s.part"
		fi
		[ "$s" = listeners ] && [ -f "$R/sc_vmess" ] && printf '  - name: "vmess-in"\n    type: vmess\n    port: 7999\n    listen:\n    users:\n      - uuid: 00000000-0000-4000-8000-000000000000\n        alterId: 0\n' >> "$TMPDIR/$s.part"
		[ -s "$TMPDIR/$s.part" ] && { echo "$s:"; cat "$TMPDIR/$s.part"; }
		rm -f "$TMPDIR/$s.part"
	done
} > "$TMPDIR/config.yaml"
ruby -ryaml -e 'c = YAML.load_file(ARGV[0]); exit(c.is_a?(Hash) && (c["listeners"] || []).all? { |l| l.is_a?(Hash) } ? 0 : 1)' "$TMPDIR/config.yaml" 2>/dev/null || echo SCBAD >> "$CCA_T/router.log"
ln -sf "$TMPDIR/config.yaml" "$BINDIR/config.yaml"
CCA_FAKE_RESTART=1 "$CCA_T/rbin/curl" -s -X PUT --data "{\"path\":\"$BINDIR/config.yaml\"}" http://127.0.0.1:9999/configs >/dev/null
S
# 下一版（GitHub 上的 dev 分支）的启动脚本：启动前的任务文件在 $TASKCFGDIR（configs/task/）里，别的一样。测试里"升级"时拿它换掉 start.sh
awk '$0 == "[ -s \"$CRASHDIR/task/bfstart\" ] && . \"$CRASHDIR/task/bfstart\"" { print "TASKCFGDIR=\"$CRASHDIR\"/configs/task"; print "[ -s \"$TASKCFGDIR\"/bfstart ] && . \"$TASKCFGDIR\"/bfstart"; next } { print }' \
	"$S/etc/ShellCrash/start.sh" > "$S/etc/ShellCrash/start.next.sh"
grep -q 'TASKCFGDIR"/bfstart' "$S/etc/ShellCrash/start.next.sh"   # set -e：没换成功就让 setup 失败
chmod +x "$S/etc/ShellCrash/start.sh" "$S/etc/ShellCrash/start.next.sh" "$S/etc/ShellCrash/task/task.sh" "$S/tmp/ShellCrash/CrashCore"
# 在跑的内核进程名写在 router/core 里（默认 OpenClash 的 clash；ShellCrash 的叫 CrashCore）；router/stopped 时都不在跑
cat > "$T/rbin/pidof" <<'S'
#!/bin/sh
[ -f "$CCA_T/router/stopped" ] && exit 1
[ "$1" = "$(cat "$CCA_T/router/core" 2>/dev/null || echo clash)" ] || exit 1
echo 4242
S
cat > "$T/rbin/netstat" <<'S'
#!/bin/sh
echo "Proto Recv-Q Send-Q Local Address Foreign Address State"
while read -r p; do echo "tcp 0 0 0.0.0.0:$p 0.0.0.0:* LISTEN"; done < "$CCA_T/router/listening"
S
cat > "$T/rbin/curl" <<'S'
#!/bin/bash
# 假的 mihomo 控制接口：PUT /configs（重载）、GET/PUT /proxies/<组>（读/切换选中节点），选中状态存在 router/sel/<组>。
# 和真的一样要密钥（运行配置里的 secret，也是 uci 的 dashboard_password）：密钥不对回 401
method=GET; data=""; url=""; out=""; prev=""; auth=""
for a in "$@"; do
	case "$prev" in -X) method="$a" ;; --data) data="$a" ;; -o) out="$a" ;; -H) case "$a" in "Authorization: Bearer "*) auth="${a#Authorization: Bearer }" ;; esac ;; esac
	case "$a" in http*) url="$a" ;; esac
	prev="$a"
done
case "$url" in
	*/configs*|*/proxies/*)
		if [ -z "${CCA_FAKE_RESTART:-}" ] && [ "$auth" != test-secret ]; then
			[ -n "$out" ] && printf '{"message":"Unauthorized"}' > "$out"; printf 401; exit 0
		fi ;;
esac
RCF="$CCA_T/router/etc/openclash/custom.yaml"; SELD="$CCA_T/router/sel"; mkdir -p "$SELD"
# 在跑的是 ShellCrash 的内核时：运行配置在 /tmp/ShellCrash；和真的 mihomo 一样只肯重载主目录（-d /etc/ShellCrash）里的文件
SCK=""; [ "$(cat "$CCA_T/router/core" 2>/dev/null)" = CrashCore ] && { SCK=1; RCF="$CCA_T/router/tmp/ShellCrash/config.yaml"; }
CORE="$CCA_T/router/core.yaml"   # 内核加载的那份配置（运行配置文件被删了，内核里的组照样在）
emit() { [ -n "$out" ] && printf '%s' "$2" > "$out"; printf '%s' "$1"; }
case "$url" in
  *"/configs"|*"/configs?force=true")
    # reload_reject：明确拒绝（400，不生效）；reload_noop：断开且不生效；reload_drop：断开但生效（像真实路由器超时）
    # CCA_FAKE_RESTART：假的 OpenClash 重启让内核加载新生成的运行配置（记成 RESTART，不算重载）
    if [ -z "${CCA_FAKE_RESTART:-}" ] && [ -f "$CCA_T/router/reload_reject" ]; then echo "RELOAD rejected" >> "$CCA_T/router.log"; emit 400 '{"message":"bad"}'; exit 22; fi
    if [ -z "${CCA_FAKE_RESTART:-}" ] && [ -f "$CCA_T/router/reload_noop" ]; then echo "RELOAD dropped-noop" >> "$CCA_T/router.log"; printf 000; exit 28; fi
    path=$(printf '%s' "$data" | sed -n 's/^{"path":"\(.*\)"}$/\1/p')
    if [ -n "$SCK" ]; then case "$path" in "$CCA_T/router/etc/ShellCrash/"*) ;; *)
      echo "UNSAFE_PATH $path" >> "$CCA_T/router.log"; emit 400 "{\"message\":\"path is not subpath of home directory or SAFE_PATHS: $path\"}"; exit 0 ;; esac; fi
    cp "$path" "$CORE"
    ruby -ryaml -e '
      c = YAML.load_file(ARGV[0]) || {}
      File.write(ARGV[1], ([c["port"], c["socks-port"], c["redir-port"], c["mixed-port"], c["tproxy-port"], 9090] + (c["listeners"] || []).map { |l| l["port"] }).compact.map { |p| "#{p}\n" }.join)
      Dir.glob(File.join(ARGV[2], "CCA-*")).each { |f| File.delete(f) }
      (c["proxy-groups"] || []).each { |g| File.write(File.join(ARGV[2], g["name"]), g["proxies"][0]) if g["name"].to_s.start_with?("CCA-") }' "$path" "$CCA_T/router/listening" "$SELD"
    if [ -n "${CCA_FAKE_RESTART:-}" ]; then echo "RESTART" >> "$CCA_T/router.log"; emit 204 ""; exit 0; fi
    # reload_conflict_once：模拟真实路由器上旧入站占着 7901——第一次重载时 7901 绑定失败，再重载才绑上
    if [ -f "$CCA_T/router/reload_conflict_once" ]; then rm -f "$CCA_T/router/reload_conflict_once"; grep -vx 7901 "$CCA_T/router/listening" > "$CCA_T/router/l.tmp"; mv "$CCA_T/router/l.tmp" "$CCA_T/router/listening"; echo "RELOAD conflict-7901" >> "$CCA_T/router.log"; emit 204 ""; exit 0; fi
    if [ -f "$CCA_T/router/reload_drop" ]; then echo "RELOAD dropped-but-applied" >> "$CCA_T/router.log"; printf 000; exit 28; fi
    echo "RELOAD" >> "$CCA_T/router.log"
    emit 204 "" ;;
  */proxies/*)
    g="${url##*/proxies/}"; f="$SELD/$g"
    [ -f "$f" ] || { emit 404 '{"message":"resource not found"}'; exit 0; }
    if [ "$method" = PUT ]; then
      name=$(printf '%s' "$data" | sed -n 's/^{"name":"\(.*\)"}$/\1/p')
      # 组成员按运行配置文件判断（文件被删了时按内核加载的那份）。kernel_stale：内核还是按旧配置在跑（文件重新生成过、
      # 内核没重新加载，真实路由器上遇到过），按内核加载的那份判断
      mf="$RCF"; { [ -f "$RCF" ] && [ ! -f "$CCA_T/router/kernel_stale" ]; } || mf="$CORE"
      if ruby -ryaml -e 'c = YAML.load_file(ARGV[0]); g = (c["proxy-groups"] || []).find { |x| x["name"] == ARGV[1] }; exit((g && g["proxies"].include?(ARGV[2])) ? 0 : 1)' "$mf" "$g" "$name"; then
        printf '%s' "$name" > "$f"; echo "SELECT $g $name" >> "$CCA_T/router.log"; emit 204 ""
      else emit 400 '{"message":"Selector update error: proxy not exist"}'; fi
    else
      emit 200 "{\"name\":\"$g\",\"type\":\"Selector\",\"now\":\"$(cat "$f")\"}"
    fi ;;
  *) exec /usr/bin/curl "$@" ;;
esac
S

# --- Mac 侧假命令（bin）
cat > "$T/bin/ssh" <<'S'
#!/bin/bash
args=("$@"); i=0; n=${#args[@]}
while [ $i -lt $n ]; do
	case "${args[$i]}" in
		-O) echo "ssh -O ${args[$((i+1))]}" >> "$CCA_T/ssh.log"; exit 0 ;;
		-i|-p|-o|-l|-F|-c|-m|-b|-e|-L|-R|-D|-W|-S|-J) i=$((i+2)) ;;
		-*) i=$((i+1)) ;;
		*) break ;;
	esac
done
dest="${args[$i]}"; i=$((i+1)); cmd="${args[*]:$i}"
echo "SSH $dest :: $cmd" >> "$CCA_T/ssh.log"
# 出门在外（$CCA_T/away）：连不上路由器
[ -f "$CCA_T/away" ] && { echo "ssh: connect to host $dest port 22: Operation timed out" >&2; exit 255; }
# 路由器上还没装公钥（$CCA_T/router_needs_password 里是密码）：密钥登录被拒；提问会话的密码登录像真的 ssh 一样经 SSH_ASKPASS 取密码，
# 对了才算装上公钥（删掉这个文件）。askpass 第二次读不到东西，不会卡住（$CCA_T/askpass_twice 时读两次，模拟 ssh 问了两遍）
if [ -f "$CCA_T/router_needs_password" ]; then
	case " ${args[*]} " in
		*PubkeyAuthentication=no*)
			if [ "${SSH_ASKPASS_REQUIRE:-}" != force ] || [ -z "${SSH_ASKPASS:-}" ]; then echo "$dest: Permission denied (no askpass)." >&2; exit 255; fi
			pw=$("$SSH_ASKPASS"); rc=$?
			echo "ASKPASS rc=$rc" >> "$CCA_T/ssh.log"
			[ ! -f "$CCA_T/askpass_twice" ] || { "$SSH_ASKPASS" >/dev/null; echo "ASKPASS2 rc=$?" >> "$CCA_T/ssh.log"; }
			[ "$pw" = "$(cat "$CCA_T/router_needs_password")" ] || { echo "$dest: Permission denied (publickey,password)." >&2; exit 255; }
			rm -f "$CCA_T/router_needs_password" ;;
		*BatchMode=yes*) echo "$dest: Permission denied (publickey)." >&2; exit 255 ;;
	esac
fi
[ -n "$cmd" ] || exit 0
if [ -f "$CCA_T/ssh_fail_uninstall" ]; then case "$cmd" in *uninstall*) echo "ssh: Connection refused" >&2; exit 255 ;; esac; fi
# 像真的 ssh 一样把 stdin 读走（cca 在 while … <<< 循环里调用 ssh 又没加 </dev/null 时，会吞掉循环后面的输入）。
# 只读普通文件（bash 3.2 的 here-string 就是）：终端、管道不碰，免得测试卡住
case "$cmd" in "sh -s"*|*uninstall*|*authorized_keys*) ;; *) [ -f /dev/stdin ] && cat > /dev/null ;; esac
exec env CCA_ROOT="$CCA_T/router" CCA_WAIT=2 HOME="$CCA_T/router/root" PATH="$CCA_T/rbin:$PATH" /bin/dash -c "$cmd"
S
cat > "$T/bin/nc" <<'S'
#!/bin/bash
# 127.0.0.1：假 Clash Verge 内核、假内置代理内核（core/up 时）的监听端口；路由器：出门在外（$CCA_T/away）时一律连不上，平时 SSH 端口 22 连得上
port="${@: -1}"; host="${@: -2:1}"
case "$host" in
	127.0.0.1) { [ ! -f "$CCA_T/verge/down" ] && grep -qx "$port" "$CCA_T/verge/listening"; } ||
		{ [ -f "$CCA_T/core/up" ] && grep -qx "$port" "$CCA_T/core/listening"; } ;;
	*) [ -f "$CCA_T/away" ] && exit 1; [ "$port" = 22 ] && exit 0; grep -qx "$port" "$CCA_T/router/listening" ;;
esac
S
# claude.ai trace：出口 IP = 203.0.113.(端口-7900)，7903 故意与 7901 同出口；iprisk：默认干净，可按 IP 放 risk/<ip>.json 覆盖
cat > "$T/bin/curl" <<'S'
#!/bin/bash
proxy=""; prev=""; url=""; out=""; sock=""; method=GET; data=""
for a in "$@"; do
	case "$prev" in -x) proxy="$a" ;; -o) out="$a" ;; --unix-socket) sock="$a" ;; -X) method="$a" ;; --data|--data-binary) data="$a" ;; esac
	case "$a" in http*) url="$a";; esac; prev="$a"
done
# 假的内置代理内核的控制接口（socket 名是 cca-core-*.sock）：状态在 $CCA_T/core（假 mihomo 起来时建 up；core_load 读配置，
# 写 listening、groups/、sel/，控制接口的 socket 记在 sock）；PUT /configs 只认主目录（home）里的配置文件
if [ -n "$sock" ]; then
	case "$sock" in */cca-core-*.sock)
		CD="$CCA_T/core"; reply() { printf '%s\n%s' "$2" "$1"; }
		[ -f "$CD/up" ] && [ "$sock" = "$(cat "$CD/sock" 2>/dev/null)" ] || { printf '\n000'; exit 7; }
		path="${url#http://localhost}"
		case "$method $path" in
			"GET /version") reply 200 '{"meta":true,"version":"v1.19.33"}' ;;
			"PUT /configs?force=true")
				p=$(printf '%s' "$data" | sed -n 's/^{"path":"\(.*\)"}$/\1/p')
				case "$p" in "$(cat "$CD/home")"/*) ;; *) reply 400 '{"message":"path is not subpath of home directory"}'; exit 0 ;; esac
				if err=$("$CCA_T/core_load" "$p" 2>&1); then echo RELOAD >> "$CD/core.log"; reply 204 ""
				else echo "RELOAD failed: $err" >> "$CD/core.log"; reply 400 "{\"message\":\"$err\"}"; fi ;;
			"GET /proxies/"*)
				g="${path#/proxies/}"
				[ -f "$CD/groups/$g" ] || { reply 404 '{"message":"resource not found"}'; exit 0; }
				reply 200 "{\"name\":\"$g\",\"type\":\"Selector\",\"now\":\"$(cat "$CD/sel/$g")\"}" ;;
			"PUT /proxies/"*)
				g="${path#/proxies/}"; name=$(printf '%s' "$data" | sed -n 's/^{"name":"\(.*\)"}$/\1/p')
				if [ -f "$CD/groups/$g" ] && grep -qx "$name" "$CD/groups/$g"; then
					printf '%s' "$name" > "$CD/sel/$g"; echo "SELECT $g $name" >> "$CD/core.log"; reply 204 ""
				else reply 400 '{"message":"Selector update error: proxy not exist"}'; fi ;;
			*) reply 404 '{"message":"not found"}' ;;
		esac
		exit 0 ;;
	esac
fi
# 假的 Clash Verge 内核控制接口（--unix-socket）：状态在 $CCA_T/verge（down=没在运行，listening=监听端口，
# groups/CCA-n=组成员，sel/CCA-n=选中的节点）；输出"响应体\n状态码"（cca 用 -w '\n%{http_code}'）
if [ -n "$sock" ]; then
	[ -d "${CCA_T:-}/verge" ] || { echo "假 curl：没有测试沙箱（CCA_T）" >&2; exit 2; }
	VD="$CCA_T/verge"; mkdir -p "$VD/groups" "$VD/sel"
	reply() { printf '%s\n%s' "$2" "$1"; }
	[ -f "$VD/down" ] && { printf '\n000'; exit 7; }
	path="${url#http://localhost}"
	case "$method $path" in
		"GET /version") reply 200 '{"meta":true,"version":"v1.19.32"}' ;;
		"GET /configs") reply 200 "{\"mode\":\"$(cat "$VD/mode")\",\"mixed-port\":7897}" ;;
		"PATCH /configs") echo "PATCH $data" >> "$VD/verge.log"; printf '%s' "$data" | sed -n 's/.*"mode":"\([a-z]*\)".*/\1/p' > "$VD/mode"; reply 204 "" ;;
		"PUT /configs?force=true")
			[ -f "$VD/reload_reject" ] && { echo "RELOAD rejected" >> "$VD/verge.log"; reply 400 '{"message":"bad"}'; exit 0; }
			# reject_text 里写着一段文字时，新配置里有它就不收（模拟内核不认参考配置里的某个节点）
			[ -s "$VD/reject_text" ] && grep -qF "$(cat "$VD/reject_text")" "${data#@}" && { echo "RELOAD rejected text" >> "$VD/verge.log"; reply 400 '{"message":"proxy 3: unsupport proxy type"}'; exit 0; }
			cp "${data#@}" "$VD/last_put.json"
			if err=$(ruby -ryaml -rjson -e '
				c = YAML.safe_load(JSON.parse(File.read(ARGV[0]))["payload"]) || {}
				known = { "DIRECT" => 1, "REJECT" => 1 }
				(c["proxies"] || []).each { |p| known[p["name"]] = 1 }
				gs = c["proxy-groups"] || []
				gs.each { |g| known[g["name"]] = 1 }
				seen = {}
				gs.each do |g|
				  abort "proxy group #{g["name"]}: duplicate" if seen[g["name"]]
				  seen[g["name"]] = 1
				  (g["proxies"] || []).each { |m| abort "proxy group #{g["name"]}: #{m} not found" unless known[m] }
				end
				File.write(ARGV[1], ([c["mixed-port"]] + (c["listeners"] || []).map { |l| l["port"] }).compact.map { |p| "#{p}\n" }.join)
				Dir.glob(File.join(ARGV[2], "CCA-*")).each { |f| File.delete(f) }
				gs.select { |g| g["name"].to_s.start_with?("CCA-") }.each do |g|
				  File.write(File.join(ARGV[2], g["name"]), g["proxies"].map { |m| "#{m}\n" }.join)
				  s = File.join(ARGV[3], g["name"]); old = File.exist?(s) ? File.read(s) : nil
				  File.write(s, g["proxies"].include?(old) ? old : g["proxies"][0])   # 像 store-selected：还在组里就保留原来选中的
				end
				Dir.glob(File.join(ARGV[3], "CCA-*")).each { |f| File.delete(f) unless File.exist?(File.join(ARGV[2], File.basename(f))) }' \
				"${data#@}" "$VD/listening" "$VD/groups" "$VD/sel" 2>&1); then
				echo RELOAD >> "$VD/verge.log"; reply 204 ""
			else
				echo "RELOAD failed: $err" >> "$VD/verge.log"; reply 400 "{\"message\":\"$err\"}"
			fi ;;
		"GET /proxies/"*)
			g="${path#/proxies/}"
			[ -f "$VD/groups/$g" ] || { reply 404 '{"message":"resource not found"}'; exit 0; }
			reply 200 "{\"name\":\"$g\",\"type\":\"Selector\",\"now\":\"$(cat "$VD/sel/$g")\"}" ;;
		"PUT /proxies/"*)
			g="${path#/proxies/}"; name=$(printf '%s' "$data" | sed -n 's/^{"name":"\(.*\)"}$/\1/p')
			if [ -f "$VD/groups/$g" ] && grep -qx "$name" "$VD/groups/$g"; then
				printf '%s' "$name" > "$VD/sel/$g"; echo "SELECT $g $name" >> "$VD/verge.log"; reply 204 ""
			else reply 400 '{"message":"Selector update error: proxy not exist"}'; fi ;;
		*) reply 404 '{"message":"not found"}' ;;
	esac
	exit 0
fi
# 假的 GitHub Releases：$CCA_T/gh/latest 是最新版本号，$CCA_T/gh/v<版本>/ 下是发布文件；其它 GitHub 地址一律失败（不联网）
case "$url" in
	https://github.com/rivia7/claude-accounts/releases/latest/download/*)
		asset="${url##*/}"; v=$(cat "$CCA_T/gh/latest" 2>/dev/null)
		echo "GH latest-get $asset" >> "$CCA_T/gh.log"
		[ -f "$CCA_T/gh/v$v/$asset" ] || exit 22
		cp "$CCA_T/gh/v$v/$asset" "$out"
		exit 0 ;;
	https://github.com/rivia7/claude-accounts/releases/latest)
		echo "GH latest" >> "$CCA_T/gh.log"
		[ -f "$CCA_T/gh/latest" ] || exit 22
		printf 'https://github.com/rivia7/claude-accounts/releases/tag/v%s' "$(cat "$CCA_T/gh/latest")"
		exit 0 ;;
	https://github.com/rivia7/claude-accounts/releases/download/*)
		rel="${url#*/releases/download/}"
		echo "GH get $rel" >> "$CCA_T/gh.log"
		[ -f "$CCA_T/gh/$rel" ] || exit 22
		cp "$CCA_T/gh/$rel" "$out"
		exit 0 ;;
	https://github.com/MetaCubeX/mihomo/releases/download/*)   # 内置代理的内核：假的 mihomo（$CCA_T/mihomo/ 下，setup.sh 打包）
		f="${url##*/}"; echo "GH mihomo $f" >> "$CCA_T/gh.log"
		[ -f "$CCA_T/mihomo/$f" ] || exit 22
		cp "$CCA_T/mihomo/$f" "$out"
		exit 0 ;;
	https://api.github.com/repos/rivia7/claude-accounts/releases*)   # 最近的发布（cca update --check）：按 $CCA_T/gh/v<版本>/ 生成，
		# 测试版是 pre-release；更新说明取 v<版本>/notes（没有就是“cca 版本”）；gh_api_down 模拟连不上、被限流
		echo "GH releases" >> "$CCA_T/gh.log"
		[ ! -f "$CCA_T/gh_api_down" ] || exit 22
		python3 - "$CCA_T/gh" > "${out:-/dev/stdout}" <<'PY'
import json, os, sys
g = sys.argv[1]
rel = []
for d in sorted(os.listdir(g)) if os.path.isdir(g) else []:
    if d.startswith('v') and os.path.isfile(os.path.join(g, d, 'cca')):
        n = os.path.join(g, d, 'notes')
        rel.append({'tag_name': d, 'name': d, 'draft': False, 'prerelease': '-beta.' in d,
                    'body': open(n, encoding='utf-8').read() if os.path.exists(n) else 'cca %s' % d[1:],
                    'html_url': 'https://github.com/rivia7/claude-accounts/releases/tag/' + d, 'published_at': '2026-11-01T08:00:00Z'})
json.dump(rel, sys.stdout, ensure_ascii=False)
PY
		exit 0 ;;
	https://github.com/*|https://*.githubusercontent.com/*|https://api.github.com/*) echo "GH unexpected $url" >> "$CCA_T/gh.log"; exit 22 ;;
esac
if [ -n "$proxy" ]; then
	port="${proxy##*:}"; phost="${proxy#*://}"; phost="${phost%:*}"
	if [ "$phost" = 127.0.0.1 ]; then   # 本机的端口（Clash Verge 或内置代理）：出口 IP 换成 198.51.100.x，选中 REJECT 时连接被拒
		if [ ! -f "$CCA_T/verge/down" ] && grep -qx "$port" "$CCA_T/verge/listening"; then LD="$CCA_T/verge"
		elif [ -f "$CCA_T/core/up" ] && grep -qx "$port" "$CCA_T/core/listening"; then LD="$CCA_T/core"
		else exit 7; fi
		s=$(cat "$LD/sel/CCA-$((port - 7900))" 2>/dev/null); [ -n "$s" ] && [ "$s" != REJECT ] || exit 56
		echo "CURL via local:$port $url" >> "$CCA_T/curl.log"; net=198.51.100
	else
		[ -f "$CCA_T/away" ] && exit 7
		grep -qx "$port" "$CCA_T/router/listening" || exit 7
		echo "CURL via :$port $url" >> "$CCA_T/curl.log"; net=203.0.113
	fi
	case "$url" in
		https://claude.ai/cdn-cgi/trace|https://chatgpt.com/cdn-cgi/trace)   # 出口地区默认 US，$CCA_T/loc/<端口> 可以改（模拟节点跑到受限地区）
			last=$((port - 7900)); [ "$port" = 7903 ] && last=1
			printf 'fl=1\nip=%s.%s\nloc=%s\n' "$net" "$last" "$(cat "$CCA_T/loc/$port" 2>/dev/null || echo US)" ;;
		https://api*.example.com/*)   # 第三方 API 的接口：不带 key 访问回 401；$CCA_T/api_down 时连不上
			[ -f "$CCA_T/api_down" ] && { printf 000; exit 7; }
			printf 401 ;;
		https://ip.net.coffee/api/iprisk/*)
			ip="${url##*/}"
			if [ -f "$CCA_T/risk/$ip.json" ]; then cat "$CCA_T/risk/$ip.json"
			else printf '{"is_datacenter":false,"isResidential":true,"is_vpn":false,"is_proxy":false,"is_tor":false,"is_abuser":false,"asOrganization":"Example Networks","countryCode":"us","trust_score":97}'; fi ;;
		https://*.d.ip.net.coffee/pixel.gif)   # DNS 泄露检测：一次性的子域名经哪个端口解析，记在 $CCA_T/dnsq/<token>
			tok="${url#https://}"; tok="${tok%%.d.ip.net.coffee/*}"; tok="${tok%-*}"
			mkdir -p "$CCA_T/dnsq"; echo "$port" > "$CCA_T/dnsq/$tok" ;;
		https://ip.net.coffee/api/dns/result/*)   # 来查这个 token 的解析器：$CCA_T/dns/<端口>（一行一个），默认是出口那边的 <网段>.53；dns_down 时连不上
			[ -f "$CCA_T/dns_down" ] && exit 7
			tok="${url##*/}"; p=$(cat "$CCA_T/dnsq/$tok" 2>/dev/null)
			if [ -z "$p" ]; then list=""; elif [ -f "$CCA_T/dns/$p" ]; then list=$(cat "$CCA_T/dns/$p"); else list="$net.53"; fi
			printf '{"token": "%s", "dns_servers": [%s]}' "$tok" "$(printf '%s\n' $list | sed '/^$/d; s/.*/"&"/' | paste -sd, -)" ;;
		https://ip.net.coffee/api/geoip/*)   # 归属地：$CCA_T/geo/<IP>.json，默认美国
			ip="${url##*/}"
			if [ -f "$CCA_T/geo/$ip.json" ]; then cat "$CCA_T/geo/$ip.json"
			else printf '{"country": "United States", "region": "", "city": "", "isp": "Example ISP", "country_code": "us"}'; fi ;;
		*) exit 6 ;;
	esac
	exit 0
fi
case "$url" in   # 直连的第三方 API 接口（不能落到下面真的 curl 上：测试不联网）
	https://api*.example.com/*)
		echo "CURL api direct $url" >> "$CCA_T/curl.log"
		[ -f "$CCA_T/api_down" ] && { printf 000; exit 7; }
		printf 401; exit 0 ;;
	https://ip.net.coffee/api/geoip/*)   # WebRTC 暴露的 IP 直连查归属地
		echo "CURL direct $url" >> "$CCA_T/curl.log"
		ip="${url##*/}"
		if [ -f "$CCA_T/geo/$ip.json" ]; then cat "$CCA_T/geo/$ip.json"
		else printf '{"country": "United States", "region": "", "city": "", "isp": "Example ISP", "country_code": "us"}'; fi
		exit 0 ;;
	https://*ip.net.coffee/*) echo "CURL direct $url" >> "$CCA_T/curl.log"; exit 7 ;;   # 别的 ip.net.coffee 接口不该直连
esac
exec /usr/bin/curl "$@"
S
# 记下每次 env 的命令行参数（检查 key 从没出现在任何进程的命令行里），再交给真的 env
cat > "$T/bin/env" <<'S'
#!/bin/bash
{ printf 'ENV'; printf ' [%s]' "$@"; echo; } >> "$CCA_T/env.log"
exec /usr/bin/env "$@"
S
# 假 git：只管 clone（cca 拉维护者的映射用）：记下网址、GIT_SSH_COMMAND 和 GIT_CONFIG_GLOBAL；policy_denied 时像
# deploy key 还没加上那样拒绝，policy_offline 时像连不上 GitHub（带着 git 自己的套话），否则把 $CCA_T/policy_repo
# 里的文件拷过去。别的子命令交给真的 git
cat > "$T/bin/git" <<'S'
#!/bin/bash
if [ "$1" = clone ]; then
	dst="${@: -1}"; url="${@: -2:1}"
	echo "GIT clone $url SSH=${GIT_SSH_COMMAND:-unset} GLOBAL=${GIT_CONFIG_GLOBAL:-unset}" >> "$CCA_T/git.log"
	n=$(cat "$CCA_T/policy_denied_n" 2>/dev/null)   # policy_denied_n：先拒绝这么多次（模拟维护者中途才把公钥加上）
	denied=""
	if [ "${n:-0}" -gt 0 ] || [ -e "$CCA_T/policy_denied" ]; then
		[ "${n:-0}" -gt 0 ] && echo $((n - 1)) > "$CCA_T/policy_denied_n"
		denied=1
	elif [ -e "$CCA_T/policy_keys" ]; then   # 按假 GitHub 上的 deploy key 认（policy.sh key 加、删的那些）：认出来了记下最近使用
		idf=$(printf '%s\n' "$GIT_SSH_COMMAND" | sed -n 's/.* -i \([^ ]*\) .*/\1/p')
		python3 "$CCA_T/fake_gh.py" _use "$url" "$idf.pub" || denied=1
	fi
	if [ -n "$denied" ]; then
		echo "git@github.com: Permission denied (publickey)." >&2; echo "fatal: Could not read from remote repository." >&2; exit 128
	fi
	if [ -e "$CCA_T/policy_offline" ]; then
		printf '%s\n' "ssh: connect to host github.com port 22: Operation timed out" "fatal: Could not read from remote repository." "" \
			"Please make sure you have the correct access rights" "and the repository exists." >&2
		exit 128
	fi
	mkdir -p "$dst" && cp "$CCA_T"/policy_repo/* "$dst"/ && exit 0
	exit 128
fi
# 发布映射（policy.sh）的推送：记下来，不碰网络
if [ "$1" = -C ] && [ "$3" = push ]; then echo "GIT push $2" >> "$CCA_T/git.log"; exit 0; fi
exec /usr/bin/git "$@"
S
# 假的剪贴板（不碰真的）：复制的内容写进 $CCA_T/clipboard，粘贴时从那里读
printf '#!/bin/bash\ncat > "$CCA_T/clipboard"\n' > "$T/bin/pbcopy"
printf '#!/bin/bash\ncat "$CCA_T/clipboard" 2>/dev/null\n' > "$T/bin/pbpaste"
# 假 gh：只模拟 gh api 里 deploy key 的几个接口（列出、添加、删除，policy.sh key 用），key 存在 $CCA_T/deploy_keys.json，
# 调用记在 ghcli.log（不记公钥）。gh_down 时像连不上 GitHub，gh_post_fail 时添加一律 502。
# 假 git 用 _use 问"这把公钥加在这个仓库上了吗"（加上了就记下最近使用）
printf '#!/bin/bash\nexec python3 "$CCA_T/fake_gh.py" "$@"\n' > "$T/bin/gh"
cat > "$T/fake_gh.py" <<'S'
import json, os, re, sys, time
T = os.environ["CCA_T"]
DB = os.path.join(T, "deploy_keys.json")
def flag(n):
    return os.path.exists(os.path.join(T, n))
def now():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
def load():
    try:
        return json.load(open(DB))
    except Exception:
        return {"next": 1, "repos": {}}
def save(d):
    json.dump(d, open(DB, "w"), indent=1, ensure_ascii=False)
def http(status, msg, errors=None):   # 和真的 gh api 一样：回应的 JSON 写 stdout，"gh: 说明 (HTTP 状态码)" 写 stderr
    body = {"message": msg, "documentation_url": "https://docs.github.com/rest/deploy-keys/deploy-keys", "status": str(status)}
    if errors:
        body["errors"] = errors
    print(json.dumps(body))
    sys.stderr.write("gh: %s (HTTP %d)\n" % (msg, status))
    sys.exit(1)
a = sys.argv[1:]
if a[:1] == ["_use"]:
    m = re.search(r"(?:git@github\.com:|ssh://git@ssh\.github\.com:443/)([^/]+/[^/]+?)(?:\.git)?$", a[1])
    try:
        pub = " ".join(open(a[2]).read().split()[:2])
    except Exception:
        sys.exit(1)
    d = load()
    for k in d["repos"].get(m.group(1) if m else "", []):
        if k["key"] == pub:
            k["last_used"] = now()
            save(d)
            sys.exit(0)
    sys.exit(1)
if a[:2] == ["repo", "view"]:   # policy.sh 发布前确认仓库是私有的；gh_public 模拟公开仓库
    print("PUBLIC" if flag("gh_public") else "PRIVATE")
    sys.exit(0)
if a[:1] != ["api"]:
    sys.stderr.write("fake gh: only gh api is simulated: %s\n" % " ".join(a))
    sys.exit(1)
method, path, fields, i = "GET", "", {}, 1
while i < len(a):
    x = a[i]
    if x == "-X":
        method = a[i + 1]; i += 2
    elif x in ("-f", "-F"):
        k, _, v = a[i + 1].partition("=")
        fields[k] = (v == "true") if x == "-F" and v in ("true", "false") else v
        i += 2
    elif x == "--paginate":
        i += 1
    else:
        path = x; i += 1
with open(os.path.join(T, "ghcli.log"), "a") as f:
    f.write("GH api %s %s%s\n" % (method, path, "".join(" %s=%s" % (k, fields[k]) for k in sorted(fields) if k != "key")))
if flag("gh_down"):
    sys.stderr.write("error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com\n")
    sys.exit(1)
m = re.match(r"^repos/([^/]+/[^/?]+)/keys(?:/(\d+))?(?:\?.*)?$", path)
if not m:
    http(404, "Not Found")
repo, kid = m.group(1), m.group(2)
d = load()
keys = d["repos"].setdefault(repo, [])
if method == "GET" and not kid:
    print(json.dumps(keys))
    sys.exit(0)
if method == "POST" and not kid:
    if flag("gh_post_fail"):
        http(502, "Server Error")
    k = " ".join(str(fields.get("key", "")).split()[:2])   # GitHub 不存公钥的备注
    if not re.match(r"^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp\d+) AAAA[A-Za-z0-9+/]+=*$", k):
        http(422, "Validation Failed", [{"resource": "PublicKey", "code": "custom", "field": "key", "message": "key is invalid. You must supply a key in OpenSSH public key format"}])
    if any(x["key"] == k for r in d["repos"].values() for x in r):
        http(422, "Validation Failed", [{"resource": "PublicKey", "code": "custom", "field": "key", "message": "key is already in use"}])
    new = {"id": d["next"], "key": k, "url": "https://api.github.com/repos/%s/keys/%d" % (repo, d["next"]),
           "title": fields.get("title", ""), "verified": True, "created_at": now(),
           "read_only": fields.get("read_only", False) is True,   # 不写 read_only 时 GitHub 给的是能写的
           "added_by": "maintainer", "last_used": None, "enabled": True}
    d["next"] += 1
    keys.append(new)
    save(d)
    print(json.dumps(new))
    sys.exit(0)
if method == "DELETE" and kid:
    if not any(x["id"] == int(kid) for x in keys):
        http(404, "Not Found")
    d["repos"][repo] = [x for x in keys if x["id"] != int(kid)]
    save(d)
    sys.exit(0)
http(404, "Not Found")
S
cat > "$T/bin/open" <<'S'
#!/bin/bash
{ printf 'OPEN'; printf ' [%s]' "$@"; echo; } >> "$CCA_T/open.log"
# 打开 Clash Verge：内核起来，并像它启动时那样按全局扩展脚本重新生成配置
case " $* " in *" io.github.clash-verge-rev.clash-verge-rev "*) rm -f "$CCA_T/verge/down"; "$CCA_T/verge_regen"; exit 0 ;; esac
# 在后台打开界面 App（--args --core：内置代理要内核）：像 App 那样在后台运行 cca _core run（App 托管内核）；core/app_dead 时 App 起不来内核
case " $* " in *" --core "*) [ -f "$CCA_T/core/app_dead" ] || ( cca _core run >> "$CCA_T/core/app.log" 2>&1 </dev/null & ); exit 0 ;; esac
# 像 Chromium/Electron 一样在 --user-data-dir 里建资料（open_noprofile：模拟 Claude App 不认这个参数）
app="" profile="" previous="" arguments="" args_started=""
for a in "$@"; do
	[ -z "$args_started" ] || arguments="${arguments:+$arguments }$a"
	[ "$a" != --args ] || args_started=1
	[ "$previous" != -a ] || app="$a"
	case "$a" in --user-data-dir=*) profile="${a#--user-data-dir=}"; [ -f "$CCA_T/open_noprofile" ] || { mkdir -p "$profile"; : > "$profile/Local State"; } ;; esac
	previous="$a"
done
# 浏览器和桌面 App 的运行证据在启动它的 shell 存活期间保留；lsof 不消费记录。
# app-owner 可在单个用例中延长寿命，验证后显式删除；真实 fakerun 交给系统 pgrep/lsof/ps。
case "$app" in
	"$CCA_T/Claude.app") name=Claude ;;
	"$CCA_T/ChatGPT.app") name=ChatGPT ;;
	"$CCA_T/Codex.app") name=Codex ;;
	*/Google\ Chrome.app|Google\ Chrome) name='Google Chrome' ;;
	*) exit 0 ;;
esac
if [ -n "$profile" ] && [ ! -f "$CCA_T/open_noprofile" ]; then
	profile=$(cd "$profile" && pwd -P)
	mkdir -p "$CCA_T/app-open"
	owner="$PPID"
	[ ! -f "$CCA_T/app-owner" ] || owner=$(cat "$CCA_T/app-owner")
	pid=$((900000000 + owner))
	started=$(LC_ALL=C /bin/ps -o lstart= -p "$owner")
	printf '%s\n' "$owner" "$app/Contents/MacOS/$name $arguments" "$profile" "$started" > "$CCA_T/app-open/$pid"
fi
S
cat > "$T/bin/pgrep" <<'S'
#!/bin/bash
if [ "$#" -eq 2 ] && [ "$1" = -f ]; then
	for record in "$CCA_T"/app-open/*; do
		[ -f "$record" ] || continue
		{ IFS= read -r owner; IFS= read -r command; } < "$record"
		if ! kill -0 "$owner" 2>/dev/null; then rm -f "$record"; continue; fi
		if printf '%s\n' "$command" | grep -Eq -- "$2"; then
			printf '%s\n' "${record##*/}"
			exit 0
		fi
	done
fi
exec /usr/bin/pgrep "$@"
S
cat > "$T/bin/lsof" <<'S'
#!/bin/bash
pids="" previous=""
for a in "$@"; do
	[ "$previous" != -p ] || pids="$a"
	previous="$a"
done
for record in "$CCA_T"/app-open/*; do
	[ -f "$record" ] || continue
	pid="${record##*/}"
	case ",$pids," in
		*",$pid,"*)
			{ IFS= read -r owner; IFS= read -r command; IFS= read -r profile; } < "$record"
				kill -0 "$owner" 2>/dev/null || { rm -f "$record"; exit 1; }
			printf 'p%s\nn%s/Local State\n' "$pid" "$profile"
			exit 0 ;;
	esac
done
exec /usr/sbin/lsof "$@"
S
cat > "$T/bin/ps" <<'S'
#!/bin/bash
pids="" previous="" matched=""
for a in "$@"; do
	[ "$previous" != -p ] || pids="$a"
	previous="$a"
done
/bin/ps "$@" 2>/dev/null
rc=$?
for record in "$CCA_T"/app-open/*; do
	[ -f "$record" ] || continue
	{ IFS= read -r owner; IFS= read -r command; IFS= read -r profile; IFS= read -r started; } < "$record"
	if ! kill -0 "$owner" 2>/dev/null; then rm -f "$record"; continue; fi
	pid="${record##*/}"
	if [ -n "$pids" ]; then case ",$pids," in *",$pid,"*) ;; *) continue ;; esac; fi
	case "$*" in
		'-axo pid=') printf '%s\n' "$pid" ;;
		'-axo pid=,ppid=') printf '%s %s\n' "$pid" "$owner" ;;
		'-o pid=,lstart= -p '*) printf '%s %s\n' "$pid" "$started" ;;
		'-ww -o pid=,command= -p '*|'-axww -o pid=,command=') printf '%s %s\n' "$pid" "$command" ;;
		*) continue ;;
	esac
	matched=1
done
[ -z "$matched" ] || exit 0
exit "$rc"
S
cat > "$T/bin/osascript" <<'S'
#!/bin/bash
{ printf 'OSA'; printf ' [%s]' "$@"; echo; [ "${1:-}" = - ] && cat; } >> "$CCA_T/osascript.log"
# 画启动器图标（osascript -l JavaScript badge.js 原图标 输出 标识 颜色）：有 fake_icons 时只写一行（见下面的 sips）
[ "${1:-}" = -l ] && [ -f "$CCA_T/fake_icons" ] && [ "${3##*/}" = badge.js ] && { printf 'fake-png %s %s\n' "$6" "$7" > "$5"; exit 0; }
[ "${1:-}" = -l ] && exec /usr/bin/osascript "$@"   # JavaScript（画启动器图标、切换窗口）交给真的 osascript
exit 0
S
# 启动器图标的另外两步（sips 缩成各个尺寸、iconutil 打包）。真画一个启动器的图标要将近 1 秒，回归里加一批账号时很慢：
# 检查图标的用例（regress.sh 3c）之后 touch $CCA_T/fake_icons，换成只把参数和输入拼起来的假的（标识、颜色变了图标照样变）
cat > "$T/bin/sips" <<'S'
#!/bin/dash
[ -f "$CCA_T/fake_icons" ] && [ "$1" = -z ] && [ "$5" = --out ] && { { printf '%s %s ' "$2" "$3"; cat "$4"; } > "$6"; exit 0; }
exec /usr/bin/sips "$@"
S
cat > "$T/bin/iconutil" <<'S'
#!/bin/dash
[ -f "$CCA_T/fake_icons" ] && [ "$1" = -c ] && [ "$4" = -o ] && { cat "$3"/* > "$5"; exit 0; }
exec /usr/bin/iconutil "$@"
S
chmod 755 "$T/bin/sips" "$T/bin/iconutil"
# 假的 Claude App：图标、程序身份与可执行文件供启动核验；open 本身由上面的夹具模拟。
mkdir -p "$T/Claude.app/Contents/Resources" "$T/Claude.app/Contents/MacOS"
plutil -create xml1 "$T/Claude.app/Contents/Info.plist" && plutil -insert CFBundleIconFile -string electron "$T/Claude.app/Contents/Info.plist" &&
	plutil -insert CFBundleExecutable -string Claude "$T/Claude.app/Contents/Info.plist"
printf '#!/bin/sh\nexit 0\n' > "$T/Claude.app/Contents/MacOS/Claude"
chmod 755 "$T/Claude.app/Contents/MacOS/Claude"
cp /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericApplicationIcon.icns "$T/Claude.app/Contents/Resources/electron.icns" 2>/dev/null ||
	printf 'fake-icns' > "$T/Claude.app/Contents/Resources/electron.icns"
# 假的钥匙串：条目存在 $CCA_T/keychain/<服务名>/<账户名>（文件内容就是密码；cca 的账户名是接口的域名）。
# 命令行参数记在 security.log；-i 模式从 stdin 读命令（cca 存 key 用它，key 不进命令行），读到的命令记在 security.stdin
cat > "$T/bin/security" <<'S'
#!/bin/bash
echo "SECURITY $*" >> "$CCA_T/security.log"
KC="$CCA_T/keychain"; mkdir -p "$KC"
one() {
	local cmd="$1" svc="" acct="" hex="" pw="" w=0 prev="" a f
	shift
	for a in "$@"; do
		case "$prev" in -s) svc="$a" ;; -a) acct="$a" ;; -X) hex="$a" ;; -w) pw="$a" ;; esac
		[ "$a" = -w ] && w=1
		prev="$a"
	done
	[ -n "$svc" ] || return 1
	if [ -n "$acct" ]; then f="$KC/$svc/$acct"; else f=$(ls "$KC/$svc/"* 2>/dev/null | head -n 1); fi   # 不给 -a：第一条
	case "$cmd" in
		add-generic-password)
			[ -f "$CCA_T/keychain_locked" ] && return 36
			mkdir -p "$KC/$svc"; f="$KC/$svc/${acct:-_}"
			if [ -n "$hex" ]; then printf '%s' "$hex" | xxd -r -p > "$f"; else printf '%s' "$pw" > "$f"; fi ;;
		find-generic-password)
			[ -f "$CCA_T/keychain_unreadable" ] && return 51   # 钥匙串锁着、读不了
			[ -n "$f" ] && [ -f "$f" ] || return 44
			[ $w = 1 ] && { cat "$f"; echo; } ;;
		delete-generic-password)
			[ -n "$f" ] && [ -f "$f" ] || return 44
			rm -f "$f"; rmdir "$KC/$svc" 2>/dev/null; return 0 ;;
	esac
}
if [ "${1:-}" = -i ]; then
	while IFS= read -r line; do
		echo "$line" >> "$CCA_T/security.stdin"
		read -r -a words <<< "$line"
		one "${words[@]}"
	done
	exit 0   # 和真的一样：-i 里的命令失败了也返回 0
fi
one "$@"
S
# 假的 get_usage：按账号目录里登录的邮箱回 $CCA_T/usage/<邮箱>.json（没有这个文件就当没有订阅用量）；
# usage/unsupported：回"不支持"；usage/hang：卡住不回
cat > "$T/fake_usage.py" <<'S'
import json, os, sys, time
t, cd = os.environ['CCA_T'], os.environ.get('CLAUDE_CONFIG_DIR', '')
try:
    j = json.load(open(os.path.join(cd, '.claude.json')))
except Exception:
    j = {}
email = (j.get('oauthAccount') or {}).get('emailAddress') or ''
with open(os.path.join(t, 'claude.log'), 'a') as f:
    f.write('USAGE: email=%s https_proxy=%s cwd=%s\n' % (email or '-', os.environ.get('https_proxy', 'unset'), os.getcwd()))
print(json.dumps({'type': 'system', 'subtype': 'init'}), flush=True)
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get('type') != 'control_request':
        continue
    rid, sub = m.get('request_id'), (m.get('request') or {}).get('subtype')
    if os.path.exists(os.path.join(t, 'usage', 'hang')):
        time.sleep(60)
    p = os.path.join(t, 'usage', email + '.json')
    if os.path.exists(os.path.join(t, 'usage', 'unsupported')) or sub != 'get_usage':
        r = {'subtype': 'error', 'request_id': rid, 'error': 'Unsupported control request subtype: %s' % sub}
    elif email and os.path.exists(p):
        r = {'subtype': 'success', 'request_id': rid, 'response': dict(json.load(open(p)), session={}, behaviors=None)}
    else:
        r = {'subtype': 'success', 'request_id': rid, 'response': {'session': {}, 'subscription_type': None,
             'rate_limits_available': False, 'rate_limits': None, 'behaviors': None}}
    print(json.dumps({'type': 'control_response', 'response': r}), flush=True)
S
cat > "$T/bin/claude" <<'S'
#!/bin/bash
LOG="$CCA_T/claude.log"
echo "ARGS: $*" >> "$LOG"
echo "ENV: CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR https_proxy=${https_proxy-unset} BROWSER=${BROWSER-unset} CCA_ACCOUNT=${CCA_ACCOUNT-unset} ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY-unset} ALL_PROXY=${ALL_PROXY-unset} fd3=$( { true <&3; } 2>/dev/null && echo open || echo closed) CCA_PROXY=${CCA_PROXY-unset} ANTHROPIC_BASE_URL=${ANTHROPIC_BASE_URL-unset} ANTHROPIC_AUTH_TOKEN=${ANTHROPIC_AUTH_TOKEN-unset} ANTHROPIC_MODEL=${ANTHROPIC_MODEL-unset} CLAUDE_CODE_USE_BEDROCK=${CLAUDE_CODE_USE_BEDROCK-unset} HTTPS_PROXY=${HTTPS_PROXY-unset} CLAUDECODE=${CLAUDECODE-unset} CLAUDE_CODE_USE_MANTLE=${CLAUDE_CODE_USE_MANTLE-unset} CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=${CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST-unset}" >> "$LOG"
# 第三方 API 账号：cca 经 --settings 传地址、模型和 apiKeyHelper。记下 env；像 Claude Code 一样执行 apiKeyHelper 取 key
# （取到的 key 写进 claude.key，不进日志）
prev=""; settings=""
for a in "$@"; do [ "$prev" = --settings ] && settings="$a"; prev="$a"; done
if [ -n "$settings" ]; then
	python3 - "$settings" "$CCA_T" >> "$LOG" <<'PY'
import json, subprocess, sys
s, t = json.loads(sys.argv[1]), sys.argv[2]
print('SETTINGS: env=%s' % json.dumps(s.get('env'), sort_keys=True))
if s.get('apiKeyHelper'):
    r = subprocess.run(['/bin/sh', '-c', s['apiKeyHelper']], stdin=subprocess.DEVNULL, capture_output=True, text=True)
    open(t + '/claude.key', 'w').write(r.stdout)
    print('HELPER: rc=%d' % r.returncode)
PY
fi
case " $* " in *" --input-format stream-json "*) exec python3 "$CCA_T/fake_usage.py" ;; esac
P="$CLAUDE_CONFIG_DIR/.claude.json"
if [ "$1" = auth ]; then
	case "$2" in
		login)
			python3 - "$P" >> "$LOG" <<'PY'
import json, os, sys
p = sys.argv[1]
j = json.load(open(p)) if os.path.exists(p) else {}
print('LOGIN-DEVICE %s:%s @ %s' % (j.get('userID', ''), j.get('machineID', ''), os.path.dirname(p)))
PY
			email=""; prev=""; for a in "$@"; do [ "$prev" = --email ] && email="$a"; prev="$a"; done
			if [ -f "$CCA_T/login_as" ]; then email=$(cat "$CCA_T/login_as"); rm -f "$CCA_T/login_as"; fi   # 一次性：模拟登错号
			[ -n "$email" ] || email="someone@example.com"
			python3 -c 'import json,os,sys; p=sys.argv[1]; j=json.load(open(p)) if os.path.exists(p) else {}; j["oauthAccount"]={"emailAddress":sys.argv[2]}; json.dump(j,open(p,"w"),indent=2)' "$P" "$email"
			echo "LOGIN $email @ $CLAUDE_CONFIG_DIR" >> "$LOG" ;;
		logout)
			[ -f "$P" ] && python3 -c 'import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.pop("oauthAccount",None); json.dump(j,open(p,"w"),indent=2)' "$P"
			echo "LOGOUT $CLAUDE_CONFIG_DIR" >> "$LOG" ;;
		status)
			python3 -c 'import json,os,sys; p=sys.argv[1]; j=json.load(open(p)) if os.path.exists(p) else {}; e=(j.get("oauthAccount") or {}).get("emailAddress"); print(json.dumps({"loggedIn": bool(e), "email": e}))' "$P" ;;
	esac
	exit 0
fi
while [ -e "$CCA_T/hold" ]; do sleep 0.1; done   # 会话一直开着：回归里 touch $CCA_T/hold，删掉它会话才往下走、退出
python3 - "$CLAUDE_CONFIG_DIR" "$CCA_ACCOUNT" <<'PY'
import json, os, sys, time
cd, acct = sys.argv[1], sys.argv[2]
tag = acct.split('@')[0]
p = os.path.join(cd, '.claude.json')
j = json.load(open(p)) if os.path.exists(p) else {}
j.setdefault('projects', {})['/p-from-' + tag] = {'hasTrustDialogAccepted': True}
j.setdefault('mcpServers', {})['mcp-' + tag] = {'command': 'echo'}
json.dump(j, open(p, 'w'), indent=2)
s = os.path.join(cd, 'settings.json')
cur = json.load(open(s)) if os.path.exists(s) else {}
cur['lastWriter'] = tag
time.sleep(1.1)
json.dump(cur, open(s + '.tmp', 'w')); os.replace(s + '.tmp', s)
os.makedirs(os.path.join(cd, 'output-styles'), exist_ok=True)
open(os.path.join(cd, 'output-styles', tag + '.md'), 'w').write('style ' + tag)
open(os.path.join(cd, 'history.jsonl'), 'a').write(json.dumps({'display': 'hi from ' + tag}) + '\n')
open(os.path.join(cd, 'projects', '-p1', 'memory', tag + '.md'), 'w').write('memory from ' + tag)
PY
exit 3
S
# 假的 codex：记下参数和环境；features list / --help / --version 给 cca 探测参数；app-server 交给 fake_codex_server.py
# （登录、查用量）；logout 删掉 auth.json；login 是 cca 退回 codex login 时的登录（直接写 auth.json）；其余当作一次会话：
# 像真的 codex 那样写 CODEX_HOME 里的文件和会话库，退出码 3。codex_noflags：这个版本不认识 cca 要关的两个开关
cat > "$T/bin/codex" <<'S'
#!/bin/bash
LOG="$CCA_T/codex.log"
echo "ARGS: $*" >> "$LOG"
echo "ENV: CODEX_HOME=${CODEX_HOME-unset} CODEX_SQLITE_HOME=${CODEX_SQLITE_HOME-unset} https_proxy=${https_proxy-unset} HTTPS_PROXY=${HTTPS_PROXY-unset} NO_PROXY=${NO_PROXY-unset} CCA_ACCOUNT=${CCA_ACCOUNT-unset} OPENAI_API_KEY=${OPENAI_API_KEY-unset} OPENAI_BASE_URL=${OPENAI_BASE_URL-unset} CODEX_ACCESS_TOKEN=${CODEX_ACCESS_TOKEN-unset} CODEX_API_KEY=${CODEX_API_KEY-unset} CODEX_CA_CERTIFICATE=${CODEX_CA_CERTIFICATE-unset} ALL_PROXY=${ALL_PROXY-unset} BROWSER=${BROWSER-unset} fd3=$( { true <&3; } 2>/dev/null && echo open || echo closed)" >> "$LOG"
args=("$@"); i=0
while [ $i -lt ${#args[@]} ]; do   # 跳过 cca 加在前面的全局参数
	case "${args[$i]}" in
		--no-daemon) i=$((i + 1)) ;;
		--disable|--enable|-c)
			if [ "${args[$i]}" = --disable ] && [ -f "$CCA_T/codex_noflags" ]; then echo "Error: Unknown feature flag: ${args[$((i + 1))]}" >&2; exit 1; fi
			i=$((i + 2)) ;;
		*) break ;;
	esac
done
sub="${args[$i]:-}"
[ $i -gt 0 ] && case "$sub" in --version|--help) sub=session ;; esac   # 前面带着 cca 的参数：是启动会话（参数原样交给 codex），不是 cca 在探测
case "$sub" in
	--version) echo "codex-cli 0.161.0"; exit 0 ;;
	--help) echo "      --no-daemon"; echo "          Run without the shared background server"; exit 0 ;;
	features)
		echo "FEATURES ${args[*]:$((i + 1))} @ $CODEX_HOME" >> "$LOG"
		case "${args[$((i + 1))]:-}" in
			list)
				echo "daemon_auto_start                        stable             true"
				[ -f "$CCA_T/codex_noflags" ] || printf '%s\n' "respect_system_proxy                     under development  false" "system_proxy_fallback                    stable             true" ;;
			disable)   # 像真的 codex：顺着链接写回原文件（临时文件 + 改名，放在原文件旁边）
				python3 - "$CODEX_HOME/config.toml" "${args[$((i + 2))]}" <<'PY'
import os, re, sys
p = os.path.realpath(sys.argv[1]); s = open(p).read() if os.path.exists(p) else ''
key = sys.argv[2]
lines, result, inside, found = s.splitlines(), [], False, False
for line in lines:
    if line.strip().startswith('['):
        inside = line.strip() == '[features]'
        result.append(line)
        if inside:
            found = True
            result.append('%s = false' % key)
    elif not (inside and re.match(r'^\s*' + re.escape(key) + r'\s*=', line)):
        result.append(line)
if not found:
    result.extend(['', '[features]', '%s = false' % key])
s = '\n'.join(result) + '\n'
open(p + '.tmp', 'w').write(s); os.replace(p + '.tmp', p)
PY
				;;
		esac
		exit 0 ;;
	app-server) exec python3 "$CCA_T/fake_codex_server.py" ;;
	logout) echo "LOGOUT $CODEX_HOME" >> "$LOG"; rm -f "$CODEX_HOME/auth.json"; exit 0 ;;
	login) echo "LOGIN-CLI $CODEX_HOME" >> "$LOG"; python3 "$CCA_T/fake_codex_server.py" --write-auth; exit 0 ;;
esac
# 第三方 API 账号：像 codex 一样执行 -c model_providers.cca={…} 里 auth 的 command + args 取 key（写进 codex.key，不进日志）
prov=""; prev=""
for a in "$@"; do [ "$prev" = -c ] && case "$a" in model_providers.cca=*) prov="${a#model_providers.cca=}" ;; esac; prev="$a"; done
if [ -n "$prov" ]; then
	python3 - "$prov" "$CCA_T" >> "$LOG" <<'PY'
import json, re, subprocess, sys
s, t = sys.argv[1], sys.argv[2]
cmd = json.loads(re.search(r'command = ("(?:[^"\\]|\\.)*")', s).group(1))
args = json.loads('[' + re.search(r'args = \[(.*?)\]', s).group(1) + ']')
print('PROVIDER: base_url=%s wire_api=%s' % (json.loads(re.search(r'base_url = ("(?:[^"\\]|\\.)*")', s).group(1)), re.search(r'wire_api = "(\w+)"', s).group(1)))
r = subprocess.run([cmd] + args, stdin=subprocess.DEVNULL, capture_output=True, text=True)
open(t + '/codex.key', 'w').write(r.stdout)
print('AUTHCMD: rc=%d' % r.returncode)
PY
fi
# 当作一次会话
while [ -e "$CCA_T/hold" ]; do sleep 0.1; done   # 会话一直开着：回归里 touch $CCA_T/hold，删掉它会话才往下走、退出
python3 - "$CODEX_HOME" "${CODEX_SQLITE_HOME:-$CODEX_HOME}" "${CCA_ACCOUNT:-x}" <<'PY'
import json, os, sqlite3, sys, time, uuid
home, dbhome, acct = sys.argv[1:4]
tag = acct.split('(')[-1].split('@')[0].strip(')')
def atomic(p, text):   # 像 codex 改 config.toml：顺着链接，在原文件旁边写临时文件再改名
    p = os.path.realpath(p); open(p + '.tmp', 'w').write(text); os.replace(p + '.tmp', p)
cfg = os.path.join(home, 'config.toml')
atomic(cfg, (open(cfg).read() if os.path.exists(cfg) else '') + '# touched by %s\n' % tag)
open(os.path.join(home, 'history.jsonl'), 'a').write(json.dumps({'text': 'hi from ' + tag}) + '\n')
idx = os.path.join(home, 'session_index.jsonl')   # 像 codex delete：整个重写再改名（不顺着链接，会把链接换成普通文件）
lines = open(idx).read().splitlines() if os.path.exists(idx) else []
open(idx + '.tmp', 'w').write('\n'.join(lines + [json.dumps({'id': tag, 'thread_name': 'from ' + tag})]) + '\n'); os.replace(idx + '.tmp', idx)
tid = str(uuid.uuid4())
d = os.path.join(home, 'sessions', '2026', '11', '03'); os.makedirs(d, exist_ok=True)
rollout = os.path.join(d, 'rollout-%s.jsonl' % tid); open(rollout, 'w').write('{}\n')
for n, v in (('installation_id', str(uuid.uuid4())), ('models_cache.json', '{}')):
    if not os.path.exists(os.path.join(home, n)):
        open(os.path.join(home, n), 'w').write(v)
os.makedirs(os.path.join(home, 'worktrees', tag), exist_ok=True)
os.makedirs(os.path.join(home, 'skills', 'from-' + tag), exist_ok=True)
con = sqlite3.connect(os.path.join(dbhome, 'state_5.sqlite'))
con.execute('CREATE TABLE IF NOT EXISTS threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, title TEXT)')
con.execute('INSERT INTO threads VALUES (?, ?, ?)', (tid, rollout, tag)); con.commit(); con.close()
PY
exit 3
S
# 假的 codex app-server（JSON-RPC，一行一个 JSON）：initialize；account/login/start 回登录网址，等 cca 在账号的浏览器里
# 打开它（出现在 open.log 里）就当用户登录完了，写 auth.json（假的 id_token，邮箱默认取 CCA_ACCOUNT 里的，
# 一次性的 codex_login_as 模拟登错号）；account/rateLimits/read 按登录的邮箱回 codex_usage/<邮箱>.json。
# codex_rpc_off：不支持 app-server；codex_usage/unsupported、hang：不支持查用量、卡住；rpc_error：里面的话当出错说明回
cat > "$T/fake_codex_server.py" <<'S'
import base64, json, os, re, sys, time
t, home = os.environ['CCA_T'], os.environ.get('CODEX_HOME', '')
def log(s):
    open(os.path.join(t, 'codex.log'), 'a').write(s + '\n')
def b64(d):
    return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip('=')
def email_now():
    try:
        tok = json.load(open(os.path.join(home, 'auth.json')))['tokens']['id_token'].split('.')[1]
        return json.loads(base64.urlsafe_b64decode(tok + '=' * (-len(tok) % 4)))['email']
    except Exception:
        return ''
def write_auth():
    device = os.path.join(home, 'installation_id')
    log('LOGIN-DEVICE %s @ %s' % (open(device).read().strip() if os.path.exists(device) else '', home))
    p = os.path.join(t, 'codex_login_as')
    if os.path.exists(p):
        email = open(p).read().strip(); os.remove(p)
    else:
        a = os.environ.get('CCA_ACCOUNT', '')
        m = re.search(r'\(([^()]+@[^()]+)\)$', a)
        email = m.group(1) if m else (a if '@' in a else 'someone@example.com')
    claims = {'email': email, 'https://api.openai.com/auth': {'chatgpt_plan_type': 'plus', 'chatgpt_account_id': 'ws-' + email}}
    auth = {'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None, 'last_refresh': '2026-11-03T00:00:00Z',
            'tokens': {'id_token': '%s.%s.sig' % (b64({'alg': 'none'}), b64(claims)), 'access_token': 'at-SECRET', 'refresh_token': 'rt-SECRET', 'account_id': 'ws-' + email}}
    json.dump(auth, open(os.path.join(home, 'auth.json'), 'w'))
    log('LOGIN %s @ %s' % (email, home))
if '--write-auth' in sys.argv:
    write_auth(); sys.exit(0)
log('APPSERVER: CODEX_HOME=%s https_proxy=%s NO_PROXY=%s' % (home, os.environ.get('https_proxy', 'unset'), os.environ.get('NO_PROXY', 'unset')))
if os.path.exists(os.path.join(t, 'codex_rpc_off')):
    sys.stderr.write("error: unrecognized subcommand 'app-server'\n"); sys.exit(2)
def send(m):
    print(json.dumps(m), flush=True)
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    meth, rid = m.get('method'), m.get('id')
    if meth == 'initialize':
        log('RPC initialize client=%s' % m['params']['clientInfo']['name']); send({'id': rid, 'result': {'userAgent': 'x'}})
    elif meth == 'initialized':
        pass
    elif meth == 'account/login/start':
        url = 'https://auth.openai.com/oauth/authorize?client_id=app_x&state=s%d' % os.getpid()
        send({'id': rid, 'result': {'type': 'chatgpt', 'loginId': 'L1', 'authUrl': url}})
        for _ in range(100):   # 等 cca 在账号的浏览器里打开登录页
            if os.path.exists(os.path.join(t, 'open.log')) and url in open(os.path.join(t, 'open.log')).read():
                break
            time.sleep(0.1)
        else:
            send({'method': 'account/login/completed', 'params': {'loginId': 'L1', 'success': False, 'error': 'browser never opened'}}); continue
        write_auth()
        send({'method': 'account/login/completed', 'params': {'loginId': 'L1', 'success': True, 'error': None}})
    elif meth == 'account/rateLimits/read':
        u = os.path.join(t, 'codex_usage')
        log('RPC rateLimits email=%s https_proxy=%s' % (email_now() or '-', os.environ.get('https_proxy', 'unset')))
        if os.path.exists(os.path.join(u, 'hang')):
            time.sleep(60)
        if os.path.exists(os.path.join(u, 'unsupported')):
            send({'id': rid, 'error': {'code': -32601, 'message': 'Method not found: account/rateLimits/read'}}); continue
        if os.path.exists(os.path.join(u, 'rpc_error')):   # 这句话原样当 app-server 的出错说明（登录凭证失效之类）
            send({'id': rid, 'error': {'code': -32603, 'message': open(os.path.join(u, 'rpc_error')).read()}}); continue
        p = os.path.join(u, email_now() + '.json')
        if not email_now():
            send({'id': rid, 'error': {'code': -32600, 'message': 'codex account authentication required'}})
        elif os.path.exists(p):
            send({'id': rid, 'result': json.load(open(p))})
        else:
            send({'id': rid, 'result': {'rateLimits': {'limitId': 'codex', 'primary': None, 'secondary': None, 'planType': 'free'}}})
    elif meth == 'account/rateLimitResetCredit/consume':
        # 用掉一次重置机会：按 codex_usage/<邮箱>.json 里的用量和重置机会办——没有（或指定的那一次不在了）是 noCredit，
        # 用量都是 0 是 nothingToReset，否则把 5 小时、每周清零、去掉用掉的那一次（写回文件，之后查用量就是新的）；
        # codex_usage/reset_outcome 里写着结果时照它回，reset_unsupported 模拟旧版本的 codex
        u, prm = os.path.join(t, 'codex_usage'), m.get('params') or {}
        key = prm.get('idempotencyKey') or ''
        log('RPC resetConsume email=%s https_proxy=%s credit=%s key=%s' % (email_now() or '-', os.environ.get('https_proxy', 'unset'),
            prm.get('creditId') or '-', 'uuid' if re.match(r'^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$', key) else 'bad:' + key))
        if os.path.exists(os.path.join(u, 'reset_unsupported')):
            send({'id': rid, 'error': {'code': -32601, 'message': 'Method not found: account/rateLimitResetCredit/consume'}}); continue
        if not email_now():
            send({'id': rid, 'error': {'code': -32600, 'message': 'codex account authentication required for rate limit reset credits'}}); continue
        p = os.path.join(u, email_now() + '.json')
        j = json.load(open(p)) if os.path.exists(p) else {'rateLimits': {}}
        rc = j.get('rateLimitResetCredits') or {'availableCount': 0, 'credits': []}
        avail = [c for c in rc.get('credits') or [] if c.get('status') == 'available']
        cid = prm.get('creditId') or (avail[0]['id'] if avail else None)
        wins = [w for w in (j['rateLimits'].get('primary'), j['rateLimits'].get('secondary')) if isinstance(w, dict)]
        if os.path.exists(os.path.join(u, 'reset_outcome')):
            outcome = open(os.path.join(u, 'reset_outcome')).read().strip()
        elif rc.get('availableCount', 0) <= 0 or (prm.get('creditId') and not any(c['id'] == cid for c in avail)):
            outcome = 'noCredit'
        elif not any(w.get('usedPercent') for w in wins):
            outcome = 'nothingToReset'
        else:
            outcome = 'reset'
            for w in wins:
                w['usedPercent'] = 0
            rc['availableCount'] -= 1
            rc['credits'] = [c for c in rc.get('credits') or [] if c.get('id') != cid]
            j['rateLimitResetCredits'] = rc
            j.setdefault('rateLimitsByLimitId', {})['codex'] = j['rateLimits']
            json.dump(j, open(p, 'w'))
        send({'id': rid, 'result': {'outcome': outcome}})
    elif rid is not None:
        send({'id': rid, 'error': {'code': -32601, 'message': 'Method not found'}})
S
# 假的 ChatGPT App（Codex 桌面版），同样提供可核验的程序身份和文件。
mkdir -p "$T/ChatGPT.app/Contents/Resources" "$T/ChatGPT.app/Contents/MacOS"
plutil -create xml1 "$T/ChatGPT.app/Contents/Info.plist" && plutil -insert CFBundleIconFile -string app "$T/ChatGPT.app/Contents/Info.plist" &&
	plutil -insert CFBundleIdentifier -string com.openai.codex "$T/ChatGPT.app/Contents/Info.plist" &&
	plutil -insert CFBundleExecutable -string ChatGPT "$T/ChatGPT.app/Contents/Info.plist"
printf '#!/bin/sh\nexit 0\n' > "$T/ChatGPT.app/Contents/MacOS/ChatGPT"
chmod 755 "$T/ChatGPT.app/Contents/MacOS/ChatGPT"
cp /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericApplicationIcon.icns "$T/ChatGPT.app/Contents/Resources/app.icns" 2>/dev/null ||
	printf 'fake-icns' > "$T/ChatGPT.app/Contents/Resources/app.icns"
# 假 ~/.codex：共享的配置、会话、技能；还有不该被链接进账号目录的（登录、installation_id、桌面 App 的运行状态）
X="$H/.codex"; mkdir -p "$X"/{sessions,skills/s1,rules,automations,browser}
printf 'model = "gpt-5"\nnotify = ["notify-send"]\n\n[mcp_servers.m1]\ncommand = "x"\n' > "$X/config.toml"
echo "# codex global instructions" > "$X/AGENTS.md"
printf '%s\n' '{"text":"old codex 1"}' > "$X/history.jsonl"
echo "skill" > "$X/skills/s1/SKILL.md"
echo "allow" > "$X/rules/default.rules"
echo '{"tokens":{"id_token":"master"}}' > "$X/auth.json"; echo master-install > "$X/installation_id"
echo '{}' > "$X/.codex-global-state.json"
# --- 假的 Clash Verge（Rev）：数据目录里的 profiles.yaml、全局扩展脚本、运行配置（节点同名，但没有 US-D）
V="$H/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev"
mkdir -p "$V/profiles" "$T/verge/groups" "$T/verge/sel"
printf '%s\n' '# Profiles Config for Clash Verge' '' 'current: Lprofile1' 'items:' '- uid: Merge' '  type: merge' '  file: Merge.yaml' \
	'- uid: Script' '  type: script' '  file: Script.js' > "$V/profiles.yaml"
printf '// Define main function (script entry)\n\nfunction main(config, profileName) {\n  return config;\n}\n' > "$V/profiles/Script.js"
cp "$V/profiles/Script.js" "$T/pristine_script.js"
cat > "$T/verge/base.yaml" <<Y
# Clash Verge

mode: rule
mixed-port: 7897
external-controller: ''
secret: set-your-secret
profile:
  store-selected: true
external-controller-unix: $T/verge.sock
proxies:
- name: US-A
  type: ss
  server: 192.0.2.1
  port: 8388
  cipher: aes-128-gcm
  password: x
- name: US-B
  type: ss
  server: 192.0.2.2
  port: 8388
  cipher: aes-128-gcm
  password: x
- name: US-C
  type: ss
  server: 192.0.2.3
  port: 8388
  cipher: aes-128-gcm
  password: x
- name: HK-A
  type: ss
  server: 192.0.2.5
  port: 8388
  cipher: aes-128-gcm
  password: x
proxy-groups:
- name: Proxy
  type: select
  proxies:
  - US-A
  - DIRECT
rules:
- MATCH,Proxy
Y
cp "$T/verge/base.yaml" "$V/clash-verge.yaml"
printf 'rule' > "$T/verge/mode"
printf '7897\n' > "$T/verge/listening"
# 模拟 Clash Verge 重新生成配置：用真的 JavaScript 引擎（osascript）跑全局扩展脚本，结果写成运行配置并交给假内核
cat > "$T/verge/run_script.js" <<'S'
ObjC.import('Foundation');
function read(p) { return $.NSString.stringWithContentsOfFileEncodingError(p, $.NSUTF8StringEncoding, null).js; }
function run(argv) {   // 和 Clash Verge 一样：脚本源码后面接上 main(配置, 名字) 的调用
  globalThis.__verge_config__ = read(argv[1]);
  return (0, eval)(read(argv[0]) + "\n;JSON.stringify(main(JSON.parse(globalThis.__verge_config__),'test')||'')");
}
S
cat > "$T/verge_regen" <<'S'
#!/bin/bash
# 路径全从脚本自己的位置推出来，不看 $HOME 和 $CCA_T：测试脚本直接调用时 $HOME 是真实的主目录，绝不能碰真的 Clash Verge
T=$(cd "$(dirname "$0")" && pwd -P) && [ -d "$T/verge" ] && [ -d "$T/home" ] || exit 1
V="$T/home/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev"; VD="$T/verge"
P="$V/profiles/Lprofile1.yaml"; [ -f "$P" ] || P="$VD/base.yaml"   # 当前选用的配置（参考节点那一节才有这个文件）
ruby -ryaml -rjson -e 'print JSON.generate(YAML.load_file(ARGV[0]))' "$P" > "$VD/base.json" || exit 1
/usr/bin/osascript -l JavaScript "$VD/run_script.js" "$V/profiles/Script.js" "$VD/base.json" > "$VD/regen.json" || exit 1
ruby -ryaml -rjson -e 'c = JSON.parse(File.read(ARGV[0])); File.write(ARGV[1], YAML.dump(c)); File.write(ARGV[2], JSON.generate("payload" => YAML.dump(c)))' \
	"$VD/regen.json" "$V/clash-verge.yaml" "$VD/regen.put" || exit 1
CCA_T="$T" "$T/bin/curl" -s --unix-socket "$T/verge.sock" -X PUT --data-binary "@$VD/regen.put" 'http://localhost/configs?force=true' >/dev/null
S
# 假的 STUN 服务器（只在 127.0.0.1 上，泄露检测那一节才开）：回答 Binding 请求，说对方的公网 IP 是 $T/stun_ip 里写的（IPv4），
# 端口写进 $T/stun.port；每次回答记在 $T/stun.log
cat > "$T/fake_stun.py" <<'S'
import os, socket, struct, sys
T = sys.argv[1]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(('127.0.0.1', 0))
with open(os.path.join(T, 'stun.port.tmp'), 'w') as f:
    f.write(str(s.getsockname()[1]))
os.replace(os.path.join(T, 'stun.port.tmp'), os.path.join(T, 'stun.port'))
while True:
    d, a = s.recvfrom(2048)
    if len(d) < 20 or d[:2] != b'\x00\x01':
        continue
    try:
        ip = open(os.path.join(T, 'stun_ip')).read().strip()
    except OSError:
        continue
    with open(os.path.join(T, 'stun.log'), 'a') as f:
        f.write('STUN %s\n' % ip)
    x = bytes(p ^ q for p, q in zip(socket.inet_aton(ip), d[4:8]))
    attr = struct.pack('!HHBBH', 0x0020, 8, 0, 1, a[1] ^ 0x2112) + x
    s.sendto(struct.pack('!HH', 0x0101, len(attr)) + d[4:20] + attr, a)
S
cat > "$T/edit.py" <<'S'
#!/usr/bin/env python3
# 假的编辑器（cca edit 用）：按环境变量 CCA_TEST_EDIT 里的一句 Python 改账号表（表在变量 j 里）
import json, os, sys
p = sys.argv[1]; j = json.load(open(p))
exec(os.environ['CCA_TEST_EDIT'])
json.dump(j, open(p, 'w'), indent=2, ensure_ascii=False)
S
# --- 内置代理：假的 mihomo 内核，打包成官方 Release 的文件名（假 curl 从 $T/mihomo/ 给），SHA256 写进 $T/mihomo/sha256
# （regress.sh 的 mkver 把测试版 cca 里写死的校验值换成它）。状态在 $T/core：起来后建 up、记下主目录 home 和 pid，
# core_load 照配置写 listening、groups/、sel/；收到 SIGTERM 时清掉 up 和 listening 再退出。core/start_fail 时起不来
mkdir -p "$T/core/groups" "$T/core/sel" "$T/mihomo"
cat > "$T/mihomo/mihomo" <<'S'
#!/bin/bash
case "${1:-}" in -v) echo "Mihomo Meta v1.19.33 darwin arm64 with go1.26 (fake)"; exit 0 ;; esac
d="" f=""
while [ $# -gt 0 ]; do case "$1" in -d) d="$2"; shift ;; -f) f="$2"; shift ;; esac; shift; done
CD="$CCA_T/core"
[ -f "$CD/start_fail" ] && { echo 'level=fatal msg="Parse config error: fake"' >&2; exit 1; }
"$CCA_T/core_load" "$f" || exit 1
printf '%s' "$d" > "$CD/home"; echo $$ > "$CD/pid"; echo START >> "$CD/core.log"
trap 'rm -f "$CD/up"; : > "$CD/listening"; echo STOP >> "$CD/core.log"; exit 0' TERM INT
touch "$CD/up"
while :; do sleep 1 & wait $!; done
S
chmod 755 "$T/mihomo/mihomo"
gzip -c "$T/mihomo/mihomo" > "$T/mihomo/mihomo-darwin-arm64-v1.19.33.gz"
cp "$T/mihomo/mihomo-darwin-arm64-v1.19.33.gz" "$T/mihomo/mihomo-darwin-amd64-compatible-v1.19.33.gz"
shasum -a 256 "$T/mihomo/mihomo-darwin-arm64-v1.19.33.gz" | cut -d' ' -f1 > "$T/mihomo/sha256"
cat > "$T/core_load" <<'S'
#!/usr/bin/env python3
# 假内核读配置（cca 生成的内置代理配置是 JSON 写的）：组成员、dialer-proxy、入站指向的都要有，入站只能在 127.0.0.1 上；
# core/reject_text 里的文字出现在配置里就不收（模拟内核不认某个节点）。收下的：listening、groups/、sel/（选组里第一个，
# 像 store-selected: false），控制接口的 socket 记在 sock，配置留一份 last_config.json
import glob, json, os, sys
cd = os.path.join(os.environ['CCA_T'], 'core')
text = open(sys.argv[1], encoding='utf-8').read()
rj = os.path.join(cd, 'reject_text')
if os.path.exists(rj) and open(rj).read().strip() and open(rj).read().strip() in text:
    sys.exit('proxy 1: unsupport proxy type')
c = json.loads(text)
gs = c.get('proxy-groups') or []
known = {p['name'] for p in c.get('proxies') or []} | {g['name'] for g in gs} | {'REJECT', 'DIRECT'}
for p in c.get('proxies') or []:
    if p.get('dialer-proxy') and p['dialer-proxy'] not in known:
        sys.exit('proxy %s: dialer-proxy %s not found' % (p['name'], p['dialer-proxy']))
for g in gs:
    for m in g['proxies']:
        if m not in known:
            sys.exit('proxy group %s: %s not found' % (g['name'], m))
for l in c.get('listeners') or []:
    if l.get('proxy') not in known or l.get('listen') != '127.0.0.1':
        sys.exit('listener %s: bad' % l.get('name'))
open(os.path.join(cd, 'listening'), 'w').write(''.join('%d\n' % l['port'] for l in c.get('listeners') or []))
for f in glob.glob(os.path.join(cd, 'groups', '*')) + glob.glob(os.path.join(cd, 'sel', '*')):
    os.remove(f)
for g in gs:
    open(os.path.join(cd, 'groups', g['name']), 'w').write(''.join(m + '\n' for m in g['proxies']))
    open(os.path.join(cd, 'sel', g['name']), 'w').write(g['proxies'][0])
open(os.path.join(cd, 'sock'), 'w').write(c.get('external-controller-unix', ''))
open(os.path.join(cd, 'last_config.json'), 'w').write(text)
S
chmod +x "$T"/bin/* "$T"/rbin/* "$T/verge_regen" "$T/edit.py" "$T/core_load"
echo "sandbox ready: $T"
