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
#     API key 存成 600 的文件 → 维护者的映射（假 git：验签、照映射加账号、签名不对不换）→ 设备统计（真的 server/cca-presence.py：
#     会话开着报在用、关了报没在用；会话的登记、进程的启动时间从 /proc 读）→ Codex（假的 codex 和 app-server：添加、服务器上登录、
#     启动、查用量、cxa）→ 真实的出口和泄露检测（测试节点直连出去；服务器上不测 WebRTC）→ 只在 macOS 上的命令说清楚 → cca local off
#   Linux 桌面上的 Clash Verge（数据目录 ~/.local/share/…，内核是真的 mihomo、经数据目录里的 verge-mihomo.sock 控制）：没有图形界面时
#     说清楚，有时打开它，配置端口池、经它出去，cca local off 去掉
#   服务器上不用路由器（全新的 HOME，直连通不通由假 curl 定）：直连不通时 cca install 不问路由器，给出维护者运行的那一行，先跳过；
#     维护者加上公钥后再运行：拉到映射、导入映射指定的节点、照映射加上账号，内置代理由 cca _core-keep 托管，经账号的端口出去
#   在家走路由器（只在容器里：假路由器上跑真的 cca-router.sh / apply.rb，内核是真的 mihomo，端口池监听 0.0.0.0）：cca install 里配置
#     路由器、选节点、加账号、问过后重载，经路由器的端口出去，启动账号，在家时给内置代理拷节点（路由器端自己写 JSON）
# 测试节点是本机另一个 mihomo（socks5 → DIRECT）。端口池固定从 7901 起，被占用时不跑
set -u
R=$(cd "$(dirname "$0")/.." && pwd)
IMAGES="ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:12 debian:13 rockylinux/rockylinux:9 almalinux:10 fedora:43 amazonlinux:2023 alpine:3.22 archlinux:latest opensuse/leap:16.0"

if [ "${1:-}" = --docker ]; then
	shift
	[ $# -gt 0 ] || set -- $IMAGES
	command -v docker >/dev/null 2>&1 || { echo "没有 docker" >&2; exit 2; }
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
			apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq --no-install-recommends bash python3 curl ca-certificates gzip perl netcat-openbsd openssh-client openssl >/dev/null 2>&1
		elif command -v dnf >/dev/null 2>&1; then
			dnf install -y -q python3 gzip perl nmap-ncat openssh-clients procps-ng findutils openssl >/dev/null 2>&1
		elif command -v apk >/dev/null 2>&1; then
			apk add -q bash python3 curl perl openssh-client-default openssh-keygen openssl >/dev/null 2>&1
		elif command -v pacman >/dev/null 2>&1; then
			pacman -Sy --noconfirm --needed python curl perl openbsd-netcat openssh procps-ng openssl >/dev/null 2>&1
		elif command -v zypper >/dev/null 2>&1; then
			zypper -q -n install python3 curl perl netcat-openbsd openssh-clients procps gzip findutils openssl >/dev/null 2>&1
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
	command -v gzip >/dev/null 2>&1 && command -v python3 >/dev/null 2>&1 || { echo "  ✗ 依赖没装齐（gzip、python3）"; echo "  结果：0 通过，1 失败"; exit 1; }
	export CCA_LINUX_CONTAINER=1   # 在家走路由器只在容器里测
	exec bash /cca/tests/linux.sh --here
fi

[ "$(uname -s)" = Linux ] || { echo "只在 Linux 上跑（macOS 上用 tests/regress.sh）" >&2; exit 2; }
pass=0; fail=0
ok() { echo "  ✓ $*"; pass=$((pass + 1)); }
no() { echo "  ✗ $*"; fail=$((fail + 1)); }
for p in $(seq 7901 7916) 18080 18081; do
	if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then echo "端口 $p 已被占用（这台机器上有在用的 cca？）：不跑" >&2; exit 2; fi
done
D=$(mktemp -d "${TMPDIR:-/tmp}/cca-linux-test.XXXXXX")
H="$D/home"; mkdir -p "$H/.local/bin" "$H/.cache" "$D/bin" "$D/up" "$D/www" "$D/policy_repo"
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
# 别的当作会话：记下拿到的代理（$D/hold 在时一直开着）；--settings 里有 apiKeyHelper 时像 Claude Code 一样执行它，取到的 key 写进 $D/claude.key
cat > "$D/bin/claude" <<S
#!/usr/bin/env python3
D = '$D'
S
cat >> "$D/bin/claude" <<'S'
import http.server, json, os, socketserver, subprocess, sys, threading, time, urllib.parse as up
args = sys.argv[1:]
P = os.path.join(os.environ.get('CLAUDE_CONFIG_DIR', '/nonexistent'), '.claude.json')
def log(s):
    open(os.path.join(D, 'claude.log'), 'a').write(s + '\n')
def load():
    try:
        return json.load(open(P))
    except Exception:
        return {}
if args[:1] == ['--version']:
    print('9.9.9 (Claude Code)'); sys.exit(0)
if args[:2] == ['auth', 'status']:
    e = (load().get('oauthAccount') or {}).get('emailAddress')
    print(json.dumps({'loggedIn': bool(e), 'email': e})); sys.exit(0)
if args[:2] == ['auth', 'logout']:
    j = load(); j.pop('oauthAccount', None); json.dump(j, open(P, 'w')); log('LOGOUT'); sys.exit(0)
if args[:2] == ['auth', 'login']:
    email = args[args.index('--email') + 1] if '--email' in args else 'someone@example.com'
    state = open(os.path.join(D, 'oauth_state')).read().strip()
    got, done = {}, threading.Event()
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass
        def do_GET(self):
            u = up.urlsplit(self.path); q = dict(up.parse_qsl(u.query))
            if u.path != '/callback' or q.get('state') != state:
                self.send_response(400); self.end_headers(); got['err'] = 'Invalid state parameter'; done.set(); return
            self.send_response(302); self.send_header('Location', 'https://platform.claude.com/oauth/code/success'); self.end_headers()
            got['code'] = q.get('code', ''); done.set()
    srv = socketserver.TCPServer(('127.0.0.1', 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    url = 'https://claude.com/cai/oauth/authorize?' + up.urlencode({'code': 'true', 'client_id': 'c', 'response_type': 'code',
        'redirect_uri': 'http://localhost:%d/callback' % srv.server_address[1], 'code_challenge': 'CC', 'code_challenge_method': 'S256',
        'state': state, 'login_hint': email})
    print('Opening browser to sign in…'); sys.stdout.write('Paste code here if prompted > '); sys.stdout.flush()
    subprocess.run([os.environ['BROWSER'], url], stdin=subprocess.DEVNULL)
    if not done.wait(60) or 'err' in got:
        print('Login failed: ' + got.get('err', 'timed out')); sys.exit(1)
    log('EXCHANGE code=%s https_proxy=%s' % (got['code'], os.environ.get('https_proxy', 'unset')))
    if not got['code'].startswith('good'):
        print('Login failed: Request failed with status code 400'); sys.exit(1)
    j = load(); j['oauthAccount'] = {'emailAddress': email}; json.dump(j, open(P, 'w'))
    c = os.path.join(os.path.dirname(P), '.credentials.json')
    fd = os.open(c, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600); os.write(fd, b'{"claudeAiOauth": {}}'); os.close(fd)
    print('Login successful.'); sys.exit(0)
if '--settings' in args:
    s = json.loads(args[args.index('--settings') + 1])
    if s.get('apiKeyHelper'):
        r = subprocess.run(['/bin/sh', '-c', s['apiKeyHelper']], stdin=subprocess.DEVNULL, capture_output=True, text=True)
        open(os.path.join(D, 'claude.key'), 'w').write(r.stdout)
log('SESSION https_proxy=%s args=%s' % (os.environ.get('https_proxy', 'unset'), ' '.join(a for a in args if not a.startswith('--settings') and not a.startswith('{'))))
while os.path.exists(os.path.join(D, 'hold')):
    time.sleep(0.1)
print('fake claude session')
S
# 假的 codex：--version / --help / features list 给 cca 探测参数；logout 删 auth.json；app-server（一行一个 JSON）照真的 codex 0.167
# 登录：本机 127.0.0.1 开回调（redirect_uri 写 localhost）、回登录网址，state 不对回 400 接着等，对的“换 token”（code 以 good 开头算
# 成功，写 auth.json，邮箱取 CCA_ACCOUNT 里的），再发 account/login/completed；account/rateLimits/read 回固定的用量。
# 别的当作会话：记下参数、代理和 CODEX_HOME（$D/hold 在时一直开着）
cat > "$D/bin/codex" <<S
#!/usr/bin/env python3
D = '$D'
S
cat >> "$D/bin/codex" <<'S'
import base64, http.server, json, os, re, socketserver, sys, threading, time, urllib.parse as up
args, home = sys.argv[1:], os.environ.get('CODEX_HOME', '')
def log(s):
    with open(os.path.join(D, 'codex.log'), 'a') as f:
        f.write(s + '\n')
i = 0
while i < len(args):   # cca 加在前面的全局参数
    if args[i] == '--no-daemon':
        i += 1
    elif args[i] in ('--disable', '--enable', '-c'):
        i += 2
    else:
        break
sub = args[i] if i < len(args) else ''
if i == 0 and sub == '--version':
    print('codex-cli 0.167.0'); sys.exit(0)
if i == 0 and sub == '--help':
    print('      --no-daemon'); sys.exit(0)
if sub == 'features':
    print('respect_system_proxy                     under development  false')
    print('system_proxy_fallback                    stable             true')
    sys.exit(0)
if sub == 'logout':
    try:
        os.remove(os.path.join(home, 'auth.json'))
    except OSError:
        pass
    sys.exit(0)
if sub != 'app-server':
    log('SESSION https_proxy=%s home=%s args=%s' % (os.environ.get('https_proxy', 'unset'), home, ' '.join(args)))
    while os.path.exists(os.path.join(D, 'hold')):
        time.sleep(0.1)
    print('fake codex session'); sys.exit(0)
def b64(d):
    return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip('=')
def email_now():
    try:
        tok = json.load(open(os.path.join(home, 'auth.json')))['tokens']['id_token'].split('.')[1]
        return json.loads(base64.urlsafe_b64decode(tok + '=' * (-len(tok) % 4)))['email']
    except Exception:
        return ''
def write_auth():
    m = re.search(r'\(([^()]+@[^()]+)\)$', os.environ.get('CCA_ACCOUNT', ''))
    email = m.group(1) if m else 'someone@example.com'
    claims = {'email': email, 'https://api.openai.com/auth': {'chatgpt_plan_type': 'plus', 'chatgpt_account_id': 'ws-x'}}
    json.dump({'auth_mode': 'chatgpt', 'OPENAI_API_KEY': None, 'tokens': {'id_token': '%s.%s.sig' % (b64({'alg': 'none'}), b64(claims)),
               'access_token': 'at', 'refresh_token': 'rt', 'account_id': 'ws-x'}}, open(os.path.join(home, 'auth.json'), 'w'))
LOCK = threading.Lock()
def send(m):
    with LOCK:
        print(json.dumps(m), flush=True)
def start_login(rid):
    state = open(os.path.join(D, 'oauth_state')).read().strip()
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass
        def do_GET(self):
            u = up.urlsplit(self.path); q = dict(up.parse_qsl(u.query))
            if u.path != '/auth/callback':
                self.send_response(404); self.end_headers(); return
            if q.get('state') != state:
                self.send_response(400); self.end_headers(); self.wfile.write(b'State mismatch'); return
            log('EXCHANGE code=%s https_proxy=%s' % (q.get('code', ''), os.environ.get('https_proxy', 'unset')))
            good = q.get('code', '').startswith('good')
            if good:
                write_auth()
            self.send_response(200); self.end_headers(); self.wfile.write(b'ok' if good else b'failed')
            send({'method': 'account/login/completed', 'params': {'loginId': 'L1', 'success': good, 'error': None if good else 'Token exchange failed'}})
    srv = socketserver.TCPServer(('127.0.0.1', 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    q = up.urlencode({'response_type': 'code', 'client_id': 'app_x', 'redirect_uri': 'http://localhost:%d/auth/callback' % srv.server_address[1],
                      'scope': 'openid profile email offline_access', 'code_challenge': 'CC', 'code_challenge_method': 'S256',
                      'state': state, 'originator': 'codex_cli_rs'})
    send({'id': rid, 'result': {'type': 'chatgpt', 'loginId': 'L1', 'authUrl': 'https://auth.openai.com/oauth/authorize?' + q}})
for line in sys.stdin:
    try:
        m = json.loads(line)
    except ValueError:
        continue
    meth, rid = m.get('method'), m.get('id')
    if meth == 'initialize':
        send({'id': rid, 'result': {'userAgent': 'x'}})
    elif meth == 'account/login/start':
        start_login(rid)
    elif meth == 'account/login/cancel':
        send({'id': rid, 'result': {}})
    elif meth == 'account/rateLimits/read':
        log('USAGE email=%s https_proxy=%s' % (email_now() or '-', os.environ.get('https_proxy', 'unset')))
        if os.path.exists(os.path.join(D, 'usage_hang')):   # 卡住不回（照样读 stdin：和真的一样，stdin 一断就退出）
            continue
        if not email_now():
            send({'id': rid, 'error': {'code': -32600, 'message': 'codex account authentication required'}}); continue
        now = int(time.time())
        rl = {'limitId': 'codex', 'primary': {'usedPercent': 13, 'windowDurationMins': 300, 'resetsAt': now + 7200},
              'secondary': {'usedPercent': 41, 'windowDurationMins': 10080, 'resetsAt': now + 3 * 86400}, 'planType': 'plus'}
        send({'id': rid, 'result': {'rateLimits': rl, 'rateLimitsByLimitId': {'codex': rl}}})
    elif rid is not None:
        send({'id': rid, 'error': {'code': -32601, 'message': 'Method not found: %s' % meth}})
S
# 假的 crontab：这次测试的用户 crontab 存在 $D/crontab 里（绝不碰这台机器上真的）
cat > "$D/bin/crontab" <<S
#!/bin/sh
case "\$1" in
	-l) [ -f "$D/crontab" ] || { echo "no crontab for \$(id -un)" >&2; exit 1; }; cat "$D/crontab" ;;
	-) cat > "$D/crontab" ;;
	*) echo "crontab: 测试里只支持 -l 和 -" >&2; exit 2 ;;
esac
S
# 假的 git：只管 cca 拉映射的 git clone（把 $D/policy_repo 里的文件拷过去），记下用的 ssh 命令；$D/policy_denied 时像 GitHub 不认这台
# 机器的 deploy key（维护者还没加上）那样拒绝
cat > "$D/bin/git" <<S
#!/bin/sh
echo "GIT \$* SSH=\${GIT_SSH_COMMAND:-}" >> "$D/git.log"
[ "\$1" = clone ] || exit 1
if [ -f "$D/policy_denied" ]; then echo 'git@github.com: Permission denied (publickey).' >&2; echo 'fatal: Could not read from remote repository.' >&2; exit 128; fi
for a; do dst=\$a; done
rm -rf "\$dst" && mkdir -p "\$dst" && cp -R "$D/policy_repo/." "\$dst/"
S
chmod 755 "$D/bin/claude" "$D/bin/codex" "$D/bin/crontab" "$D/bin/git"
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
export HOME="$H" PATH="$D/bin:$H/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" CCA_POLICY_DEFAULT= LANG=C.UTF-8 CCA_COST=off
unset CCA_HOME XDG_RUNTIME_DIR XDG_DATA_HOME DISPLAY WAYLAND_DISPLAY
echo "  bash ${BASH_VERSION%%(*}，$(python3 --version 2>&1)，$(curl --version 2>/dev/null | head -n 1 | cut -d' ' -f1-2)，nc：$(readlink -f "$(command -v nc)" 2>/dev/null || echo 无)，ps：$(readlink -f "$(command -v ps)" 2>/dev/null || echo 无)，ruby：$(ruby -e 'print RUBY_VERSION' 2>/dev/null || echo 无)"
ans() { local f; f=$(mktemp "$D/ans.XXXXXX"); printf '%s\n' "$@" > "$f"; printf '%s' "$f"; }
fp() { printf '%s' "$1" | sha256sum | cut -c1-16; }   # 节点摘要（node_fp：类型|服务器|端口|TLS 域名 的 sha256 前 16 位）
FPA=$(fp 'socks5|127.0.0.1|18080|')

# 一键安装：install.sh 从 GitHub 下载 cca 和签名、验签、运行 cca install。GitHub 换成本机的文件（假 curl 只认 GitHub 上 cca 的发布），
# 发布密钥换成这次生成的（install.sh 和 cca 里的 RELEASE_KEY 一起换）；单独的 HOME，用 bash（PATH 该写进 ~/.bashrc）
I="$D/i"; mkdir -p "$I/gh" "$I/bin" "$I/home" "$I/tmp"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$I/relkey" </dev/null
pub=$(cut -d' ' -f1-2 "$I/relkey.pub")
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$R/cca" > "$I/gh/cca"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$R/install.sh" > "$I/install.sh"
ssh-keygen -q -Y sign -f "$I/relkey" -n cca-release "$I/gh/cca" </dev/null
# 下面用的 cca：同一份（认这次的发布密钥：映射、设备清单都用它签），旁边有 .git（开发模式：不自动更新，不去连 GitHub）
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
python3 - "$H/.cca" <<'PY'
import json, os, sys, time
d = sys.argv[1]
c = json.load(open(d + '/config.json'))
c['nodes'] = ['US-A']
json.dump(c, open(d + '/config.json', 'w'), ensure_ascii=False, indent=1)
json.dump({'alice(alice@example.com)': 'US-A'}, open(d + '/accounts.json', 'w'), indent=1)
os.makedirs(d + '/core', mode=0o700, exist_ok=True)
json.dump({'version': 1, 'saved_at': time.time(), 'nodes': ['US-A'],
           'proxies': [{'name': 'US-A', 'type': 'socks5', 'server': '127.0.0.1', 'port': 18080}]}, open(d + '/core/nodes.json', 'w'))
os.chmod(d + '/core/nodes.json', 0o600)
PY

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
[ -x "$core" ] && cp "$core" "$D/mihomo"   # 后面 Clash Verge、假路由器的内核也用它
echo '{"mixed-port": 18080, "bind-address": "127.0.0.1", "allow-lan": false, "mode": "direct", "log-level": "warning"}' > "$D/up/config.yaml"
[ -x "$D/mihomo" ] && { "$D/mihomo" -d "$D/up" -f "$D/up/config.yaml" > "$D/up.log" 2>&1 & echo $! >> "$D/pids"; }
echo cca-linux-ok > "$D/www/ok.txt"
( cd "$D/www" && exec python3 -m http.server 18081 --bind 127.0.0.1 > /dev/null 2>&1 ) & echo $! >> "$D/pids"
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
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice -p hi 2>&1); rc=$?
if [ $rc -eq 0 ] && grep -q 'SESSION https_proxy=http://127.0.0.1:7901 args=.*-p hi' "$D/claude.log" 2>/dev/null; then ok "cca @alice：启动账号，claude 拿到的代理是账号的端口"
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
out2=$(CCA_TTY=$(ans) "$C" @kf -p hi 2>&1); rc=$?
if [ $rc -eq 0 ] && [ "$(cat "$D/claude.key" 2>/dev/null)" = "$K2" ]; then ok "第三方 API 账号：key 在 600 的文件里，启动时 apiKeyHelper 回调 cca 取到"
else no "第三方 API 账号（退出码 $rc）：$(printf '%s\n%s' "$out" "$out2" | tail -n 4)"; fi
CCA_TTY=$(ans y) "$C" rm kf >/dev/null 2>&1
if [ -z "$(ls "$H"/.cca/secrets/cca-api-kf-* 2>/dev/null)" ]; then ok "cca rm：第三方 API 账号的 key 文件一起删掉"; else no "cca rm 之后 key 文件还在"; fi

# 维护者的映射：假 git 把 $D/policy_repo 当作私有仓库（这次的发布密钥签名）。cca policy setup 生成这台机器的 deploy key、拉取、验签，
# 照映射加上这台机器还没有的账号（bob，别名取映射里的）
PR="$D/policy_repo"
pol() { printf '%s\n' "$1" > "$PR/policy.json"; rm -f "$PR/policy.json.sig"; ssh-keygen -q -Y sign -n cca-policy -f "$I/relkey" "$PR/policy.json" </dev/null; }
pol '{"version": 1, "claude": {"alice@example.com": "US-A", "bob@example.com": "US-A"}, "aliases": {"bob@example.com": "bob"}}'
out=$("$C" policy setup example/cca-policy 2>&1); rc=$?
PK="$H/.cca/policy/id_ed25519"
if [ $rc -eq 0 ] && [ "$(sha256sum < "$PR/policy.json")" = "$(sha256sum < "$H/.cca/policy/policy.json" 2>/dev/null)" ] && [ "$(stat -c %a "$PK" 2>/dev/null)" = 600 ] && ! printf '%s' "$out" | grep -q '路由器' &&
	printf '%s' "$out" | grep -qF "$(cut -d' ' -f1-2 "$PK.pub")" && grep -q 'IdentitiesOnly=yes' "$D/git.log" && grep -q 'IdentityAgent=none' "$D/git.log" &&
	grep -q '"bob(bob@example.com)": "US-A"' "$H/.cca/accounts.json"; then
	ok "cca policy setup：生成 deploy key（600）、只用它拉取、验签，照映射加上 bob（别名取映射里的；没有路由器，不提同步路由器）"
else no "cca policy setup（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)；账号表：$(tr -d '\n' < "$H/.cca/accounts.json")"; fi
cp "$PR/policy.json.sig" "$D/sig.keep"
printf '%s\n' '{"version": 1, "claude": {"alice@example.com": "US-B"}}' > "$PR/policy.json"; cp "$D/sig.keep" "$PR/policy.json.sig"
out=$("$C" policy pull 2>&1); rc=$?
out2=$("$C" policy 2>&1)
if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q '签名' && grep -q 'bob@example.com' "$H/.cca/policy/policy.json" &&
	printf '%s' "$out2" | grep -qE '映射：.*（20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9:]+ 拉到的）'; then
	ok "签名对不上的映射不换（接着用上次的）；cca policy 说上次什么时候拉到的"
else no "签名对不上的映射（退出码 $rc）：$(printf '%s' "$out" | tail -n 2)；$(printf '%s' "$out2" | grep '映射' | head -n 1)"; fi
pol '{"version": 1, "claude": {"alice@example.com": "US-A", "bob@example.com": "US-A"}, "aliases": {"bob@example.com": "bob"}}'
"$C" policy pull >/dev/null 2>&1

# 设备统计（presence）：映射里配置了维护者的服务器（真的 server/cca-presence.py：127.0.0.1 上随机端口、openssl 生成的 P-256 证书、
# 这次的发布密钥，ttl 3 秒），设备清单是这台机器的 deploy key（发布密钥签名）。映射里的账号开着会话时 cca 在后台起上报进程报给服务器
# （Linux 上没有 kqueue：每 5 秒看一次）；会话的登记（cli-<进程号>.json）、进程的启动时间从 /proc 读（busybox 的 ps 不认 -p、-x、lstart）
if command -v openssl >/dev/null 2>&1; then
	SV="$D/presence"; mkdir -p "$SV"
	openssl ecparam -name prime256v1 -genkey -noout -out "$SV/key.pem" 2>/dev/null
	openssl req -new -x509 -key "$SV/key.pem" -subj /CN=cca-presence -days 2 -out "$SV/cert.pem" 2>/dev/null
	python3 -I "$R/server/cca-presence.py" --listen 127.0.0.1:0 --port-file "$SV/port" --cert "$SV/cert.pem" --key "$SV/key.pem" \
		--release-key "$I/relkey.pub" --state "$SV/state.json" --ttl 3 >> "$SV/log" 2>&1 & echo $! >> "$D/pids"
	for i in $(seq 1 50); do [ -s "$SV/port" ] && break; sleep 0.1; done
	SVPORT=$(cat "$SV/port" 2>/dev/null)
	PIN="sha256//$(openssl x509 -in "$SV/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)"
	pol "$(python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j["presence"]={"url":"https://127.0.0.1:%s" % sys.argv[2],"pins":[sys.argv[3]],"salt":"ab"*16}; print(json.dumps(j))' "$PR/policy.json" "$SVPORT" "$PIN")"
	"$C" policy pull >/dev/null 2>&1
	printf '{"version":1,"seq":%s,"machines":[{"key":"%s","name":"linux-test"}]}' "$(date +%s)" "$(cut -d' ' -f1-2 "$PK.pub")" > "$SV/machines.json"
	ssh-keygen -q -Y sign -n cca-devices -f "$I/relkey" "$SV/machines.json" </dev/null
	code=$(curl -q -s -k --noproxy '*' --pinnedpubkey "$PIN" -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
		-H "X-Cca-Sig: $(base64 < "$SV/machines.json.sig" | tr -d '\n')" --data-binary "@$SV/machines.json" "https://127.0.0.1:$SVPORT/v1/machines")
	dstate() { "$C" devices alice@example.com --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(",".join("%s:%s:%d" % (x["name"], x["state"], x["cli"]) for x in d["accounts"][0]["devices"]))' 2>/dev/null; }
	AD=$(ls -d "$H"/.cca/accounts/alice-example-com-* 2>/dev/null | head -n 1)
	touch "$D/hold"
	CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice -p hi >/dev/null 2>&1 & S1=$!
	for i in $(seq 1 40); do [ -f "$AD/cli-$S1.json" ] && break; sleep 0.25; done
	mk=$(python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); print("%s:%s" % (j["pid"], j["tool"]))' "$AD/cli-$S1.json" 2>/dev/null)
	for i in $(seq 1 60); do [ "$(dstate)" = linux-test:in_use:1 ] && break; sleep 0.25; done
	st1=$(dstate)
	ap=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$H/.cca/presence/agent.json" 2>/dev/null)
	if [ "$code" = 200 ] && [ "$mk" = "$S1:claude" ] && [ "$st1" = linux-test:in_use:1 ] && [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; then
		ok "设备统计：会话开着时登记 cli-<进程号>.json（启动时间从 /proc 读），后台的上报进程报给服务器：在用、1 个会话"
	else no "设备统计：设备清单 HTTP ${code:-无}，登记 ${mk:-没有}，服务器上是「${st1:-空}」，上报进程 ${ap:-没有}；$(tail -n 2 "$SV/log" 2>/dev/null | tr '\n' ' ')$(cat "$H/.cca/presence/status.json" 2>/dev/null)"; fi
	rm -f "$D/hold"; wait "$S1" 2>/dev/null
	for i in $(seq 1 80); do [ "$(dstate)" = linux-test:none:0 ] && break; sleep 0.25; done
	st2=$(dstate)
	for i in $(seq 1 40); do { [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; } || break; sleep 0.25; done
	if [ "$st2" = linux-test:none:0 ] && [ ! -e "$AD/cli-$S1.json" ] && ! { [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; }; then
		ok "会话关了：登记删掉，上报进程几秒内报“没在用”后自己退出"
	else no "会话关了之后：服务器上是「${st2:-空}」，登记$([ -e "$AD/cli-$S1.json" ] && echo 还在 || echo 删了)，上报进程$({ [ -n "$ap" ] && kill -0 "$ap" 2>/dev/null; } && echo 还在 || echo 退出了)"; fi
else
	echo "  （跳过设备统计：没有 openssl，生成不了服务器的证书）"
fi

# 折合金额：共享的会话记录里的用量按官方 API 的标价折算，按登录的身份认是谁的（Linux 上没有 st_birthtime：按修改时间排；
# 进程的启动时间从 /proc 读）。会话结束后在后台统计一次（CCA_COST 在这一步清掉：别处是 off）；cca cost sync --json 是 App 用的那一条
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-* 2>/dev/null | head -n 1)
python3 - "$AD/claude/.claude.json" "$H" <<'PY'
import json, os, sys, time
p, home = sys.argv[1], sys.argv[2]
j = json.load(open(p))
j.setdefault('oauthAccount', {})['accountUuid'] = 'u-linux-alice-01'
json.dump(j, open(p, 'w'))
d = os.path.join(home, '.claude', 'projects', '-linux')
os.makedirs(d, exist_ok=True)
t = time.strftime('%Y-%m-%dT%H:%M:%S.000Z', time.gmtime(time.time() - 3600))
line = lambda x: json.dumps(x, separators=(',', ':')) + '\n'
open(os.path.join(d, 's-linux.jsonl'), 'w').write(
    line({'type': 'session-owner', 'accountUuid': 'u-linux-alice-01', 'organizationUuid': 'o', 'sessionId': 's-linux'})
    + line({'type': 'assistant', 'timestamp': t, 'requestId': 'r1', 'sessionId': 's-linux',
            'message': {'id': 'm1', 'model': 'claude-opus-5-5', 'usage': {'input_tokens': 1000, 'output_tokens': 100}}}))
PY
CCA_COST= CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" @alice -p hi >/dev/null 2>&1
for i in $(seq 1 40); do [ -f "$H/.cca/cost/local.json" ] && break; sleep 0.25; done
kicked=$([ -f "$H/.cca/cost/last" ] && [ -f "$H/.cca/cost/local.json" ] && echo yes)
out=$("$C" cost sync --json 2>/dev/null); rc=$?
got=$(python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); print("%s|%s" % (j["who"].get("claude:u-linux-alice-01"), sum(r[0] for r in j["acc"]["claude:u-linux-alice-01"].values())))' "$H/.cca/cost/local.json" 2>/dev/null)
if [ $rc -eq 0 ] && [ "$kicked" = yes ] && [ "$got" = "alice@example.com|6000" ] && [ "$(stat -c %a "$H/.cca/cost")" = 700 ] && [ "$(stat -c %a "$H/.cca/cost/local.json")" = 600 ] &&
	printf '%s' "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if d["kind"] == "cost_sync" and d["scan"]["state"] == "ready" else 1)'; then
	ok "折合金额：会话结束后在后台统计（cca _cost-sync），按登录的身份认出是 alice 的（折合 0.006 美元）；cca cost sync --json 照样能跑；只给自己读"
else no "折合金额（退出码 $rc，后台统计${kicked:-没起}）：${got:-没统计出来}；$(printf '%s' "$out" | tail -c 300)"; fi

# Codex：添加账号（服务器上没有路由器、只配置了内置代理：节点从可选节点里选，不用先配置路由器）、服务器上登录（login-page 那一行
# 带别名、节点和节点摘要；换 token 经账号的端口）、启动（关掉会换出口的开关，经账号的端口）、查用量（经账号的端口）、cxa 是 cca codex 的简写
out=$(CCA_TTY=$(ans carol@example.com carol 1) "$C" codex add 2>&1); rc=$?
CP=$(python3 -c 'import json,sys; print(list(json.load(open(sys.argv[1]))["codex"]["ports"].values())[0])' "$H/.cca/config.json" 2>/dev/null)
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -q '已添加 carol' && ! printf '%s' "$out" | grep -qE '先配置路由器|还没配置路由器|路由器同步没完成' &&
	[ "$(readlink "$H/.local/bin/cxa")" = cca ] && [ -n "$CP" ]; then
	ok "cca codex add：没有路由器也能加（节点从可选节点里选，不提路由器），端口 ${CP}，建好 cxa"
else no "cca codex add（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans 'good-codex#st-other' 'good-codex#st-linux') "$C" codex login carol 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -qF "cca codex login-page carol@example.com --alias carol --node US-A --node-id $FPA 'https://auth.openai.com/oauth/authorize?" &&
	grep -q "EXCHANGE code=good-codex https_proxy=http://127.0.0.1:$CP" "$D/codex.log" 2>/dev/null && printf '%s' "$out" | grep -q '已重新登录'; then
	ok "cca codex login：给出 cca codex login-page 那一行，验证码核对 state 后转给 app-server 的回调，经账号的端口换 token"
else no "cca codex login（退出码 $rc）：$(printf '%s' "$out" | grep -F 'login-page' | head -n 1)$(printf '%s' "$out" | tail -n 3)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$C" codex @carol exec hi 2>&1); rc=$?
l=$(grep '^SESSION ' "$D/codex.log" 2>/dev/null | tail -n 1)
if [ $rc -eq 0 ] && printf '%s' "$l" | grep -qF "https_proxy=http://127.0.0.1:$CP home=$H/.cca/codex/accounts/carol-example-com-" &&
	printf '%s' "$l" | grep -qF -- '--no-daemon' && printf '%s' "$l" | grep -qF -- '--disable system_proxy_fallback' &&
	printf '%s' "$l" | grep -qF -- '--disable respect_system_proxy' && printf '%s' "$l" | grep -q 'exec hi$'; then
	ok "cca codex @carol：经账号的端口启动，CODEX_HOME 是账号自己的，关掉 system_proxy_fallback、respect_system_proxy，不起后台服务"
else no "cca codex @carol（退出码 $rc）：${l:-没启动}；$(printf '%s' "$out" | tail -n 2)"; fi
out=$(CCA_SKIP_CHECK=1 COLUMNS=160 "$C" codex usage 2>&1); rc=$?
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -qE 'carol.* 13% .* 41% ' && grep -q "USAGE email=carol@example.com https_proxy=http://127.0.0.1:$CP" "$D/codex.log"; then
	ok "cca codex usage：经账号的端口让 app-server 查，表里有 5 小时、本周"
else no "cca codex usage（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi
# 查用量途中按 Ctrl-C、关掉终端（TERM）：后台的查询忽略 SIGINT，cca 自己停掉它们和它们起的进程，不等查询超时。
# Debian 的容器没有 procps（没有 pkill）：走读 /proc 的那条路
queries() {   # 这次测试里还在跑的用量查询：python 的 usage-query 和它起的 app-server（按 HOME 认）
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
hv=$(python3 - "$H"/.cca/accounts/alice-example-com-*/health.json <<'PY' 2>/dev/null
import json, sys
h = json.load(open(sys.argv[1]))
lk = h.get('leak')
good = (h.get('source') == 'check' and h.get('route') == 'local' and h.get('node') == 'US-A' and h.get('port') == 7901
        and h.get('result') in ('ok', 'low_purity', 'restricted', 'no_exit', 'no_score')
        and (lk is None or (lk.get('webrtc') is None and lk.get('dns') in ('ok', 'leak', 'failed') and 'WebRTC' not in (lk.get('line') or ''))))
print('%s %s：%s · %s；DNS %s' % ('good' if good else 'bad', h.get('result'), h.get('ip') or '没拿到 IP', h.get('country') or '?',
                               (lk or {}).get('dns') or '没测（出口不对时不等）'))
PY
)
if [ $rc -eq 0 ] && [ "${hv%% *}" = good ] && ! printf '%s' "$out" | grep -q 'WebRTC' && printf '%s' "$out" | grep -q '^  alice  US-A  :7901  '; then
	ok "cca check（真的联网）：经账号的端口测出口和 DNS 泄露，记进 health.json，不说 WebRTC（${hv#* }）"
else no "cca check（退出码 $rc）：${hv:-没有 health.json}；$(printf '%s' "$out" | tail -n 3)"; fi

# 只在 macOS 上的命令：说清楚（不说"没装 Chrome""要 macOS 26"这些），cca help 不列它们
bad=""
for c in web billing limit-resets app gui; do
	out=$("$C" "$c" 2>&1) && bad="$bad $c（退出码 0）"
	printf '%s' "$out" | grep -q '只在 macOS 上；这台 Linux 上用命令行' || bad="$bad $c：$(printf '%s' "$out" | tail -n 1)"
done
if [ -z "$bad" ]; then ok "cca web、billing、limit-resets、app、gui：说只在 macOS 上"; else no "只在 macOS 上的命令：$bad"; fi
out=$("$C" help 2>&1)
if ! printf '%s' "$out" | grep -qE '^  cca (web|billing|limit-resets|app|gui|login-page) |^  cca codex app ' &&
	printf '%s' "$out" | grep -q '只在 macOS 上' && printf '%s' "$out" | grep -q '由 cca 在后台运行' && printf '%s' "$out" | grep -q '^  cca usage ' &&
	! printf '%s' "$out" | grep -q 'WebRTC'; then
	ok "cca help：不列只在 macOS 上的命令，内置代理说的是由 cca 在后台运行，泄露检测不提 WebRTC"
else no "cca help：$(printf '%s' "$out" | grep -E '^  cca (web|billing|limit-resets|app|gui|login-page) |^  cca codex app |WebRTC' | head -n 2)"; fi

# 内嵌 Python 的编译缓存：每个版本一份，写新的一份时顺手删掉 7 天前写的旧缓存和没写完的临时文件（没有 XDG_RUNTIME_DIR 时缓存在
# ~/.cache 里，系统不会清，不删就越积越多）；7 天内的、python3 指针、名字不像缓存的都留着，读缓存时不动。用 XDG_RUNTIME_DIR 指到沙箱
PC="$D/xdg/cca-py"; mkdir -p -m 700 "$D/xdg" "$PC"
for n in 0123456789abcdef1-30906f0 0123456789abcdef2-30906f0.4242 0123456789abcdef3-30e00f0 python3 notes-0123456789abcdef5 0123456789abcdefk-1; do echo x > "$PC/$n"; done
python3 - "$PC" <<'P'
import os, sys, time
for n, days in (('0123456789abcdef1-30906f0', 8), ('0123456789abcdef2-30906f0.4242', 30), ('0123456789abcdef3-30e00f0', 6),
                ('python3', 30), ('notes-0123456789abcdef5', 30), ('0123456789abcdefk-1', 30)):
    t = time.time() - days * 86400
    os.utime(os.path.join(sys.argv[1], n), (t, t))
P
XDG_RUNTIME_DIR="$D/xdg" "$C" ls >/dev/null 2>&1; l1=$(ls "$PC" | LC_ALL=C sort | tr '\n' ' ')
XDG_RUNTIME_DIR="$D/xdg" "$C" ls >/dev/null 2>&1; l2=$(ls "$PC" | LC_ALL=C sort | tr '\n' ' ')
v=$(python3 - "$l1" <<'P'
import re, sys
s = set(sys.argv[1].split())
new = [n for n in s if re.fullmatch(r'[0-9a-f]{17,}-[0-9a-f]+', n) and not n.startswith('0123456789abcdef')]
keep = {'0123456789abcdef3-30e00f0', '0123456789abcdefk-1', 'notes-0123456789abcdef5', 'python3'}
gone = {'0123456789abcdef1-30906f0', '0123456789abcdef2-30906f0.4242'}
print('ok' if keep <= s and not s & gone and len(new) == 1 else 'bad')
P
)
if [ "$v" = ok ] && [ "$l1" = "$l2" ]; then ok "内嵌 Python 的编译缓存：写新的一份时删掉 7 天前写的旧缓存和没写完的临时文件，别的都留着；读缓存时不动"
else no "内嵌 Python 的编译缓存：第一次后 ${l1:-空的}；第二次后 ${l2:-空的}"; fi

out=$("$C" local off 2>&1)
for i in $(seq 1 20); do [ -z "$(keeper)" ] && ! listen 7901 && break; sleep 0.25; done
if [ -z "$(keeper)" ] && ! listen 7901 && ! grep -qF '# cca-core' "$D/crontab" && grep -qx '0 3 \* \* \* echo user-job' "$D/crontab"; then
	ok "cca local off：托管进程退出、内核停了，crontab 里开机起内核的那一行删掉（别的行留着）"
else no "cca local off：托管进程${k:+/内核}$([ -n "$(keeper)" ] && echo 还在)；crontab 是「$(tr '\n' '|' < "$D/crontab" 2>/dev/null)」"; fi
grep -iE 'level=(error|fatal)|panic' "$H/.cca/core/core.log" 2>/dev/null | head -n 2 | sed 's/^/  内核日志：/'

# Linux 桌面上的 Clash Verge（Rev）：数据目录 ~/.local/share/io.github.clash-verge-rev.clash-verge-rev（XDG_DATA_HOME 没设时），
# 内核（真的 mihomo）由它带着 -ext-ctl-unix <数据目录>/verge-mihomo.sock 起，运行配置里不写这个 socket。没在运行时：没有图形界面
# 就说清楚、不去开；有（DISPLAY）就问过后打开 clash-verge（这里是个假的，像真的一样起内核）
VD="$H/.local/share/io.github.clash-verge-rev.clash-verge-rev"; mkdir -p "$VD/profiles" "$D/vbin"
printf '%s\n' '# Profiles Config for Clash Verge' '' 'current: Lprofile1' 'items:' '- uid: Merge' '  type: merge' '  file: Merge.yaml' \
	'- uid: Script' '  type: script' '  file: Script.js' > "$VD/profiles.yaml"
printf '// Define main function (script entry)\n\nfunction main(config, profileName) {\n  return config;\n}\n' > "$VD/profiles/Script.js"
cat > "$VD/clash-verge.yaml" <<'Y'
# Clash Verge

mode: rule
log-level: warning
external-controller: ''
secret: set-your-secret
profile:
  store-selected: true
proxies:
- name: US-A
  type: socks5
  server: 127.0.0.1
  port: 18080
proxy-groups:
- name: Proxy
  type: select
  proxies:
  - US-A
  - DIRECT
rules:
- MATCH,Proxy
Y
cat > "$D/vbin/clash-verge" <<S
#!/bin/sh
echo \$\$ > "$D/verge.pid"
exec "$D/mihomo" -d "$VD" -f "$VD/clash-verge.yaml" -ext-ctl-unix "$VD/verge-mihomo.sock" > "$D/verge.log" 2>&1
S
chmod 755 "$D/vbin/clash-verge"
out=$(CCA_TTY=$(ans y) "$C" local setup 2>&1); rc=$?
if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q '检测到 Clash Verge' && printf '%s' "$out" | grep -q '这里没有图形界面，打不开 Clash Verge：先在桌面上打开它，再试'; then
	ok "Clash Verge 认出来了（~/.local/share 里的数据目录）；没在运行、又没有图形界面时说清楚，不去开它"
else no "Clash Verge 没在运行、没有图形界面（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)"; fi
out=$(DISPLAY=:99 PATH="$D/vbin:$PATH" CCA_TTY=$(ans y y) "$C" local setup 2>&1); rc=$?
st=$("$C" local status 2>&1)
if [ $rc -eq 0 ] && printf '%s' "$out" | grep -q '本机端口池已就绪：Clash Verge 127.0.0.1:7901' && [ -S "$VD/verge-mihomo.sock" ] &&
	grep -qF '// >>> cca >>>' "$VD/profiles/Script.js" && printf '%s' "$st" | grep -q 'CCA-1  127.0.0.1:7901  监听中  → US-A' && [ "$(get)" = cca-linux-ok ]; then
	ok "有图形界面时问过后打开 Clash Verge，等它的内核起来（verge-mihomo.sock），全局扩展脚本加上端口池、热加载，经它出去"
else no "Clash Verge（退出码 $rc）：$(printf '%s' "$out" | tail -n 3)；$(printf '%s' "$st" | head -n 3)"; fi
out=$("$C" local off 2>&1)
for i in $(seq 1 20); do listen 7901 || break; sleep 0.25; done
if ! grep -qF '>>> cca >>>' "$VD/profiles/Script.js" && ! listen 7901 && [ -S "$VD/verge-mihomo.sock" ]; then ok "cca local off：全局扩展脚本里 cca 的那一段删掉，端口池撤掉（Clash Verge 照跑）"
else no "Clash Verge 的 cca local off：$(printf '%s' "$out" | tail -n 2)"; fi
kill "$(cat "$D/verge.pid" 2>/dev/null)" 2>/dev/null

# cca restore：先打包备份配置（GNU / busybox 的 tar；登录凭证、密钥文件、参考节点不进包里），再删掉 ~/.cca、~/.local/bin 里的 cca、cxa 和 aga、
# crontab 里开机起内核的那一行（别的行留着）。没有 tar 的（Amazon Linux 的最小安装）：先停下，什么都不动
mkdir -p "$H/.cca/secrets" "$H/.cca/accounts/orphan-example-com/claude"
echo '{"v": 1, "host": "api.example.com", "key": "sk-LEFTOVER-SECRET"}' > "$H/.cca/secrets/cca-api-orphan"
echo '{"claudeAiOauth": {"accessToken": "cred-LEFTOVER-SECRET"}}' > "$H/.cca/accounts/orphan-example-com/claude/.credentials.json"
printf '%s\n' "@reboot '$C' _core-keep >/dev/null 2>&1 # cca-core" >> "$D/crontab"
# 维护者的参考节点里有节点密钥（连同拉取映射时留下的临时文件）：也不进备份；映射本身照样打包
mkdir -p "$H/.cca/policy/tmp.Ab12cd/repo"; [ -f "$H/.cca/policy/policy.json" ] || echo '{}' > "$H/.cca/policy/policy.json"
for f in nodes.json nodes.json.4242 tmp.Ab12cd/repo/nodes.json; do
	printf '%s\n' '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-Z", "type": "ss", "server": "192.0.2.99", "port": 8388, "cipher": "aes-128-gcm", "password": "node-LEFTOVER-SECRET"}]}' > "$H/.cca/policy/$f"
done
out=$(CCA_TTY=$(ans restore) "$C" restore 2>&1); rc=$?
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

# 服务器上不用路由器：全新的 HOME，从来没配置过路由器、直连不通（假 curl：$D/direct_down）时，cca install 不问路由器，改用内置代理，
# 节点由维护者提供。维护者还没加上这台机器的公钥（假 git 拒绝）时给出维护者运行的那一行，回答 s 先跳过；加上之后再运行 cca install：
# 拉到映射，导入映射指定的节点（install_node、映射里账号的节点，只要维护者参考节点里有），照映射加上账号，内置代理由 cca _core-keep
# 托管（问过后在 crontab 里加开机那一行），经账号的端口出去。内核文件用前面下好的那份（不再下载一次）
SH="$D/shome"; mkdir -p "$SH/.local/bin"
sr() { HOME="$SH" PATH="$SH/.local/bin:$PATH" CCA_POLICY_DEFAULT=example/cca-policy "$@"; }   # ~/.local/bin 在 PATH 里：不问要不要写进 ~/.bashrc
if [ -x "$D/mihomo" ]; then mkdir -p -m 700 "$SH/.cca" "$SH/.cca/core" && cp "$D/mihomo" "$SH/.cca/core/mihomo-v$(sed -n 's/^CORE_VERSION="\(.*\)"$/\1/p' "$C")"; fi
pol '{"version": 1, "claude": {"sam@example.com": "US-S"}, "aliases": {"sam@example.com": "sam"}, "install_node": "US-S"}'
printf '%s\n' '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-S", "type": "socks5", "server": "127.0.0.1", "port": 18080}, {"name": "US-X", "type": "ss", "server": "192.0.2.83", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}]}' > "$PR/nodes.json"
rm -f "$PR/nodes.json.sig"; ssh-keygen -q -Y sign -n cca-nodes -f "$I/relkey" "$PR/nodes.json" </dev/null
touch "$D/direct_down" "$D/policy_denied"; : > "$D/curl.log"
out=$(sr env CCA_TTY="$(ans s)" "$C" install 2>&1); rc=$?
pk="$SH/.cca/policy/id_ed25519.pub"
line=$(printf '%s\n' "$out" | sed -n 's/^    \(\.\/policy\.sh key add .*\)$/\1/p' | head -n 1)
if [ $rc -eq 0 ] && grep -q '^CURL direct https://claude.ai/cdn-cgi/trace' "$D/curl.log" && [ -s "$pk" ] && [ "$(stat -c %a "${pk%.pub}")" = 600 ] &&
	printf '%s' "$out" | grep -q '直连 claude.ai 不通：这台机器不用配置路由器，改用 cca 内置的代理' &&
	[ "$line" = "./policy.sh key add '$(cat "$pk")' $(cut -d@ -f2 "$pk")" ] && printf '%s' "$out" | grep -q '等维护者运行上面那一行' &&
	printf '%s' "$out" | grep -q '先跳过：维护者运行上面那一行之后，再运行 cca install 接着配置' &&
	! printf '%s' "$out" | grep -qE '现在配置路由器|新增第一个账号|GitHub 不认这台机器的 deploy key' && [ -z "$(keeper "$SH")" ]; then
	ok "服务器上直连不通：不问路由器，给出维护者运行的那一行（这台机器的公钥、机器名）；维护者还没加上时先跳过，说清楚之后再运行什么"
else no "服务器上直连不通（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)"; fi
# 再运行 cca install，这次一直等着（不按回车：每隔几秒自动拉，CCA_POLICY_POLL=1 秒；答案是一直开着的管道）：维护者中途运行了那一行
# （假 git 不再拒绝），cca 自己拉到、接着配置。开机起内核那一问的答案等拉到映射以后才写进管道（那之前读到的会当作“马上重试”）
touch "$SH/.cca/policy/checked"   # 今天拉过了：启动时不再在后台拉（git.log 里只有这次 cca install 拉的）
mkfifo "$D/tty.fifo"; exec 8<>"$D/tty.fifo"; : > "$D/git.log"
sr env CCA_TTY="$D/tty.fifo" CCA_POLICY_POLL=1 "$C" install > "$D/onboard.out" 2>&1 & job=$!
for i in $(seq 1 80); do [ "$(grep -c '^GIT clone' "$D/git.log")" -ge 3 ] && break; sleep 0.25; done   # 第三次是等了一轮之后自动拉的
n_wait=$(grep -c '^GIT clone' "$D/git.log")
rm -f "$D/policy_denied"   # 维护者运行了那一行
for i in $(seq 1 80); do [ -f "$SH/.cca/policy/policy.json" ] && break; sleep 0.25; done
printf 'y\n' >&8
for i in $(seq 1 240); do kill -0 "$job" 2>/dev/null || break; sleep 0.25; done
if kill -0 "$job" 2>/dev/null; then   # 卡住了：停掉这个 HOME 的进程（失败时才扫 /proc）
	for p in /proc/[0-9]*; do { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -qxF "HOME=$SH" && kill "${p#/proc/}" 2>/dev/null; done
fi
wait "$job"; rc=$?
exec 8<&-; rm -f "$D/tty.fifo"
out=$(cat "$D/onboard.out")
for i in $(seq 1 40); do [ "$(get)" = cca-linux-ok ] && break; sleep 0.25; done
boot=$(grep -F '# cca-core' "$D/crontab" 2>/dev/null)
if [ $rc -eq 0 ] && [ "$n_wait" -ge 3 ] && [ "$(printf '%s' "$out" | grep -c '等维护者运行上面那一行')" = 1 ] &&
	printf '%s' "$out" | grep -q '拉到了维护者的映射（Claude 1 个账号' && printf '%s' "$out" | grep -qF '已导入维护者提供的节点：US-S（这台机器的可选节点）' &&
	python3 -c 'import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if c["nodes"] == ["US-S"] and c["node_ids"].get("US-S") == sys.argv[2] and c["local"]["client"] == "builtin" else 1)' "$SH/.cca/config.json" "$FPA" 2>/dev/null &&
	grep -q '"sam(sam@example.com)": "US-S"' "$SH/.cca/accounts.json" && printf '%s' "$out" | grep -q '本机端口池已就绪：内置代理 127.0.0.1:7901' &&
	[ "$boot" = "@reboot $C _core-keep >/dev/null 2>&1 # cca-core" ] && [ -n "$(keeper "$SH")" ] && [ "$(get)" = cca-linux-ok ] &&
	! printf '%s' "$out" | grep -qE '现在配置路由器|新增第一个账号|没找到 Claude Code'; then
	ok "再运行 cca install 等着：每隔几秒自动拉（拉了 ${n_wait} 次还没加上），维护者加上后拉到映射，导入映射指定的节点（参考节点里别的不导入），照映射加上 sam，内置代理由 cca _core-keep 托管（问过后加开机那一行），经账号的端口出去"
else no "等维护者确认的 cca install（退出码 $rc，加上之前拉了 ${n_wait} 次）：$(printf '%s' "$out" | tail -n 4)；crontab 是「$(tr '\n' '|' < "$D/crontab" 2>/dev/null)」"; fi
sr "$C" local off >/dev/null 2>&1
for i in $(seq 1 40); do [ -z "$(keeper "$SH")" ] && ! listen 7901 && break; sleep 0.25; done
rm -f "$D/direct_down" "$PR/nodes.json" "$PR/nodes.json.sig"

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
	cat > "$RB/netstat" <<'S'
#!/usr/bin/env python3
for f in ('/proc/net/tcp', '/proc/net/tcp6'):
    try:
        rows = open(f).read().splitlines()[1:]
    except OSError:
        continue
    for r in rows:
        p = r.split()
        if len(p) > 3 and p[3] == '0A':
            print('tcp 0 0 0.0.0.0:%d 0.0.0.0:* LISTEN' % int(p[1].rsplit(':', 1)[1], 16))
S
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
	python3 -c 'import socket
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.2", 12222)); s.listen(64)
while True:
    s.accept()[0].close()' & echo $! >> "$D/pids"
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
	out=$(rr env CCA_SKIP_CHECK=1 CCA_TTY="$(ans 'good-home#st-linux')" "$C" @alice -p hi 2>&1); rc=$?
	l=$(grep '^SESSION ' "$D/claude.log" | tail -n 1)
	for i in $(seq 1 60); do [ -s "$RH/.cca/core/nodes.json" ] && break; sleep 0.25; done
	if [ $rc -eq 0 ] && grep -q 'EXCHANGE code=good-home https_proxy=http://127.0.0.2:7901$' "$D/claude.log" && printf '%s' "$l" | grep -q 'https_proxy=http://127.0.0.2:7901 '; then
		ok "在家启动账号：认出在家（nc 连得上路由器的 SSH 端口），登录时换 token、启动的 claude 都经路由器的端口"
	else no "在家启动账号（退出码 $rc）：${l:-没有启动 claude}；$(grep '^EXCHANGE' "$D/claude.log" | tail -n 1)；$(printf '%s' "$out" | tail -n 3)"; fi
	if python3 -c 'import json,sys; n={p["name"]: p for p in json.load(open(sys.argv[1]))["proxies"]}; sys.exit(0 if n["US-A"]["port"] == 18080 and n["US-B"]["port"] == 18082 else 1)' "$RH/.cca/core/nodes.json" 2>/dev/null &&
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
echo "  结果：$pass 通过，$fail 失败"
[ $fail -eq 0 ]
