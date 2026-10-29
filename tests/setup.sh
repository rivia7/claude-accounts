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
printf '#!/bin/sh\necho "CORE SAFE_PATHS=$SAFE_PATHS $*" >> "%s/router.log"\n[ -f "%s/router/core_fail" ] && { echo "level=error msg=parse config error"; exit 1; }\nexit 0\n' "$T" "$T" > "$R/etc/openclash/clash"
chmod +x "$R/etc/init.d/cron" "$R/etc/openclash/clash"

# --- 路由器侧假命令（rbin）
printf '#!/bin/sh\nexec /bin/dash "$@"\n' > "$T/rbin/sh"
cat > "$T/rbin/uci" <<'S'
#!/bin/sh
case "$*" in *"get openclash.config.config_path"*) echo /etc/openclash/config/custom.yaml ;; esac
S
cat > "$T/rbin/pidof" <<'S'
#!/bin/sh
[ -f "$CCA_T/router/stopped" ] && exit 1
echo 4242
S
cat > "$T/rbin/netstat" <<'S'
#!/bin/sh
echo "Proto Recv-Q Send-Q Local Address Foreign Address State"
while read -r p; do echo "tcp 0 0 0.0.0.0:$p 0.0.0.0:* LISTEN"; done < "$CCA_T/router/listening"
S
cat > "$T/rbin/curl" <<'S'
#!/bin/bash
# 假的 mihomo 控制接口：PUT /configs（重载）、GET/PUT /proxies/<组>（读/切换选中节点），选中状态存在 router/sel/<组>
method=GET; data=""; url=""; out=""; prev=""
for a in "$@"; do
	case "$prev" in -X) method="$a" ;; --data) data="$a" ;; -o) out="$a" ;; esac
	case "$a" in http*) url="$a" ;; esac
	prev="$a"
done
RCF="$CCA_T/router/etc/openclash/custom.yaml"; SELD="$CCA_T/router/sel"; mkdir -p "$SELD"
emit() { [ -n "$out" ] && printf '%s' "$2" > "$out"; printf '%s' "$1"; }
case "$url" in
  *"/configs"|*"/configs?force=true")
    # reload_reject：明确拒绝（400，不生效）；reload_noop：断开且不生效；reload_drop：断开但生效（像真实路由器超时）
    if [ -f "$CCA_T/router/reload_reject" ]; then echo "RELOAD rejected" >> "$CCA_T/router.log"; emit 400 '{"message":"bad"}'; exit 22; fi
    if [ -f "$CCA_T/router/reload_noop" ]; then echo "RELOAD dropped-noop" >> "$CCA_T/router.log"; printf 000; exit 28; fi
    path=$(printf '%s' "$data" | sed -n 's/^{"path":"\(.*\)"}$/\1/p')
    ruby -ryaml -e '
      c = YAML.load_file(ARGV[0]) || {}
      File.write(ARGV[1], ([c["port"], c["socks-port"], c["redir-port"], c["mixed-port"], c["tproxy-port"], 9090] + (c["listeners"] || []).map { |l| l["port"] }).compact.map { |p| "#{p}\n" }.join)
      Dir.glob(File.join(ARGV[2], "CCA-*")).each { |f| File.delete(f) }
      (c["proxy-groups"] || []).each { |g| File.write(File.join(ARGV[2], g["name"]), g["proxies"][0]) if g["name"].to_s.start_with?("CCA-") }' "$path" "$CCA_T/router/listening" "$SELD"
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
      if ruby -ryaml -e 'c = YAML.load_file(ARGV[0]); g = (c["proxy-groups"] || []).find { |x| x["name"] == ARGV[1] }; exit((g && g["proxies"].include?(ARGV[2])) ? 0 : 1)' "$RCF" "$g" "$name"; then
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
[ -n "$cmd" ] || exit 0
if [ -f "$CCA_T/ssh_fail_uninstall" ]; then case "$cmd" in *uninstall*) echo "ssh: Connection refused" >&2; exit 255 ;; esac; fi
exec env CCA_ROOT="$CCA_T/router" CCA_WAIT=4 HOME="$CCA_T/router/root" PATH="$CCA_T/rbin:$PATH" /bin/dash -c "$cmd"
S
cat > "$T/bin/nc" <<'S'
#!/bin/bash
# 127.0.0.1：假 Clash Verge 内核的监听端口；路由器：出门在外（$CCA_T/away）时一律连不上，平时 SSH 端口 22 连得上
port="${@: -1}"; host="${@: -2:1}"
case "$host" in
	127.0.0.1) [ ! -f "$CCA_T/verge/down" ] && grep -qx "$port" "$CCA_T/verge/listening" ;;
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
	https://github.com/*|https://*.githubusercontent.com/*) echo "GH unexpected $url" >> "$CCA_T/gh.log"; exit 22 ;;
esac
if [ -n "$proxy" ]; then
	port="${proxy##*:}"; phost="${proxy#*://}"; phost="${phost%:*}"
	if [ "$phost" = 127.0.0.1 ]; then   # 本机 Clash Verge 的端口：出口 IP 换成 198.51.100.x，选中 REJECT 时连接被拒
		[ ! -f "$CCA_T/verge/down" ] && grep -qx "$port" "$CCA_T/verge/listening" || exit 7
		s=$(cat "$CCA_T/verge/sel/CCA-$((port - 7900))" 2>/dev/null); [ -n "$s" ] && [ "$s" != REJECT ] || exit 56
		echo "CURL via local:$port $url" >> "$CCA_T/curl.log"; net=198.51.100
	else
		[ -f "$CCA_T/away" ] && exit 7
		grep -qx "$port" "$CCA_T/router/listening" || exit 7
		echo "CURL via :$port $url" >> "$CCA_T/curl.log"; net=203.0.113
	fi
	case "$url" in
		https://claude.ai/cdn-cgi/trace)
			last=$((port - 7900)); [ "$port" = 7903 ] && last=1
			printf 'fl=1\nip=%s.%s\nloc=US\n' "$net" "$last" ;;
		https://ip.net.coffee/api/iprisk/*)
			ip="${url##*/}"
			if [ -f "$CCA_T/risk/$ip.json" ]; then cat "$CCA_T/risk/$ip.json"
			else printf '{"is_datacenter":false,"isResidential":true,"is_vpn":false,"is_proxy":false,"is_tor":false,"is_abuser":false,"asOrganization":"Example Networks","countryCode":"us","trust_score":97}'; fi ;;
		*) exit 6 ;;
	esac
	exit 0
fi
exec /usr/bin/curl "$@"
S
cat > "$T/bin/open" <<'S'
#!/bin/bash
{ printf 'OPEN'; printf ' [%s]' "$@"; echo; } >> "$CCA_T/open.log"
# 打开 Clash Verge：内核起来，并像它启动时那样按全局扩展脚本重新生成配置
case " $* " in *" io.github.clash-verge-rev.clash-verge-rev "*) rm -f "$CCA_T/verge/down"; "$CCA_T/verge_regen"; exit 0 ;; esac
# 像 Chromium/Electron 一样在 --user-data-dir 里建资料（open_noprofile：模拟 Claude App 不认这个参数）
for a in "$@"; do
	case "$a" in --user-data-dir=*) [ -f "$CCA_T/open_noprofile" ] || { mkdir -p "${a#--user-data-dir=}"; : > "${a#--user-data-dir=}/Local State"; } ;; esac
done
S
cat > "$T/bin/osascript" <<'S'
#!/bin/bash
{ printf 'OSA'; printf ' [%s]' "$@"; echo; [ "${1:-}" = - ] && cat; } >> "$CCA_T/osascript.log"
[ "${1:-}" = -l ] && exec /usr/bin/osascript "$@"   # JavaScript（画启动器图标、切换窗口）交给真的 osascript
exit 0
S
# 假的 Claude App（只用到 Info.plist 里的图标名和图标文件）
mkdir -p "$T/Claude.app/Contents/Resources" "$T/Claude.app/Contents/MacOS"
plutil -create xml1 "$T/Claude.app/Contents/Info.plist" && plutil -insert CFBundleIconFile -string electron "$T/Claude.app/Contents/Info.plist"
cp /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericApplicationIcon.icns "$T/Claude.app/Contents/Resources/electron.icns" 2>/dev/null ||
	printf 'fake-icns' > "$T/Claude.app/Contents/Resources/electron.icns"
cat > "$T/bin/security" <<'S'
#!/bin/bash
echo "SECURITY $*" >> "$CCA_T/security.log"
S
cat > "$T/bin/claude" <<'S'
#!/bin/bash
LOG="$CCA_T/claude.log"
echo "ARGS: $*" >> "$LOG"
echo "ENV: CLAUDE_CONFIG_DIR=$CLAUDE_CONFIG_DIR https_proxy=${https_proxy-unset} BROWSER=${BROWSER-unset} CCA_ACCOUNT=${CCA_ACCOUNT-unset} ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY-unset} ALL_PROXY=${ALL_PROXY-unset} fd3=$( { true <&3; } 2>/dev/null && echo open || echo closed) CCA_PROXY=${CCA_PROXY-unset}" >> "$LOG"
P="$CLAUDE_CONFIG_DIR/.claude.json"
if [ "$1" = auth ]; then
	case "$2" in
		login)
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
ruby -ryaml -rjson -e 'print JSON.generate(YAML.load_file(ARGV[0]))' "$VD/base.yaml" > "$VD/base.json" || exit 1
/usr/bin/osascript -l JavaScript "$VD/run_script.js" "$V/profiles/Script.js" "$VD/base.json" > "$VD/regen.json" || exit 1
ruby -ryaml -rjson -e 'c = JSON.parse(File.read(ARGV[0])); File.write(ARGV[1], YAML.dump(c)); File.write(ARGV[2], JSON.generate("payload" => YAML.dump(c)))' \
	"$VD/regen.json" "$V/clash-verge.yaml" "$VD/regen.put" || exit 1
CCA_T="$T" "$T/bin/curl" -s --unix-socket "$T/verge.sock" -X PUT --data-binary "@$VD/regen.put" 'http://localhost/configs?force=true' >/dev/null
S
chmod +x "$T"/bin/* "$T"/rbin/* "$T/verge_regen"
echo "sandbox ready: $T"
