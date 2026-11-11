#!/bin/bash
# Linux 上的冒烟测试（tests/regress.sh 只能在 macOS 上跑）：cca 的内置代理在这台机器上能不能用。
#   bash tests/linux.sh                    在这台 Linux 上跑一遍（临时 HOME，不碰真的 ~/.cca；要联网下载内核）
#   bash tests/linux.sh --docker [镜像…]   每个发行版一个容器各跑一遍（要 Docker；不写镜像就跑下面 IMAGES 里的主要发行版）
# 流程：一键安装（install.sh：GitHub 换成本机的文件、发布密钥换成临时生成的；验签、cca install、PATH 写进 ~/.bashrc；缺命令时给出
# 这个发行版的安装命令）→ 装 cca（假的 claude）→ 写一个账号和一个测试节点（本机另一个 mihomo：socks5 → DIRECT，充当路由器上拷来的节点）→
# cca local setup 选内置代理（下载内核、校验、后台托管、端口池、切到账号的节点）→ 经账号的端口访问本机的 HTTP 服务 →
# 登录（给出在 Mac 上运行的 cca login-page 那一行，贴回验证码，转给假 claude 本机的回调）→ 启动账号 → API key 存成 600 的文件 →
# 杀掉内核看托管进程拉起来 → 只在 macOS 上的命令（web、app、gui…）说清楚、cca help 不列它们 → cca local off 看内核停、托管进程退出。
# 端口池固定从 7901 起，被占用时不跑
set -u
R=$(cd "$(dirname "$0")/.." && pwd)
IMAGES="ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:12 debian:13 rockylinux/rockylinux:9 almalinux:10 fedora:43 amazonlinux:2023 alpine:3.22 archlinux:latest opensuse/leap:16.0"

if [ "${1:-}" = --docker ]; then
	shift
	[ $# -gt 0 ] || set -- $IMAGES
	command -v docker >/dev/null 2>&1 || { echo "没有 docker" >&2; exit 2; }
	L=$(mktemp -d "${TMPDIR:-/tmp}/cca-linux.XXXXXX")
	for img in "$@"; do   # 各用各的网络：端口池互不冲突，并行跑
		n=$(printf '%s' "$img" | tr '/:.' '---')
		docker run --rm --name "cca-linux-$n-$$" -v "$R":/cca:ro "$img" sh /cca/tests/linux.sh --in-container > "$L/$n.log" 2>&1 &
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
			apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq --no-install-recommends bash python3 curl ca-certificates gzip perl netcat-openbsd openssh-client procps >/dev/null 2>&1
		elif command -v dnf >/dev/null 2>&1; then
			dnf install -y -q python3 gzip perl nmap-ncat openssh-clients procps-ng findutils >/dev/null 2>&1
		elif command -v apk >/dev/null 2>&1; then
			apk add -q bash python3 curl perl openssh-client-default openssh-keygen >/dev/null 2>&1
		elif command -v pacman >/dev/null 2>&1; then
			pacman -Sy --noconfirm --needed python curl perl openbsd-netcat openssh procps-ng >/dev/null 2>&1
		elif command -v zypper >/dev/null 2>&1; then
			zypper -q -n install python3 curl perl netcat-openbsd openssh-clients procps gzip >/dev/null 2>&1
		fi
	}
	for i in 1 2 3; do deps && break; sleep 5; done   # 十几个容器同时装，软件源偶尔失败一次（Amazon Linux 遇到过）
	command -v bash >/dev/null 2>&1 || { echo "  ✗ 装不上 bash"; echo "  结果：0 通过，1 失败"; exit 1; }
	command -v gzip >/dev/null 2>&1 && command -v python3 >/dev/null 2>&1 || { echo "  ✗ 依赖没装齐（gzip、python3）"; echo "  结果：0 通过，1 失败"; exit 1; }
	exec bash /cca/tests/linux.sh --here
fi

[ "$(uname -s)" = Linux ] || { echo "只在 Linux 上跑（macOS 上用 tests/regress.sh）" >&2; exit 2; }
pass=0; fail=0
ok() { echo "  ✓ $*"; pass=$((pass + 1)); }
no() { echo "  ✗ $*"; fail=$((fail + 1)); }
for p in $(seq 7901 7916); do
	if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then echo "端口 $p 已被占用（这台机器上有在用的 cca？）：不跑" >&2; exit 2; fi
done
D=$(mktemp -d "${TMPDIR:-/tmp}/cca-linux-test.XXXXXX")
H="$D/home"; mkdir -p "$H/.local/bin" "$H/.cache" "$D/bin" "$D/up" "$D/www"
trap 'kill $(cat "$D/pids" 2>/dev/null) 2>/dev/null; [ -f "$H/.cca/config.json" ] && "$R/cca" local off >/dev/null 2>&1; rm -rf "$D"' EXIT
# 假的 claude：auth login 照真的 Claude Code 走（本机 127.0.0.1 开回调、经 BROWSER 交出登录网址，state 是 $D/oauth_state，
# code 以 good 开头算换到了 token，凭证写成 600 的 .credentials.json），auth status / logout（凭证文件留给 cca 删），
# 别的当作会话：记下拿到的代理；--settings 里有 apiKeyHelper 时像 Claude Code 一样执行它，取到的 key 写进 $D/claude.key
cat > "$D/bin/claude" <<S
#!/usr/bin/env python3
D = '$D'
S
cat >> "$D/bin/claude" <<'S'
import http.server, json, os, socketserver, subprocess, sys, threading, urllib.parse as up
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
print('fake claude session')
S
chmod 755 "$D/bin/claude"
export HOME="$H" PATH="$D/bin:$H/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" CCA_POLICY_DEFAULT= LANG=C.UTF-8
unset CCA_HOME XDG_RUNTIME_DIR
echo "  bash ${BASH_VERSION%%(*}，$(python3 --version 2>&1)，$(curl --version 2>/dev/null | head -n 1 | cut -d' ' -f1-2)，nc：$(readlink -f "$(command -v nc)" 2>/dev/null || echo 无)，ps：$(readlink -f "$(command -v ps)" 2>/dev/null || echo 无)"
ans() { local f; f=$(mktemp "$D/ans.XXXXXX"); printf '%s\n' "$@" > "$f"; printf '%s' "$f"; }

# 一键安装：install.sh 从 GitHub 下载 cca 和签名、验签、运行 cca install。GitHub 换成本机的文件（假 curl 只认这两个地址），
# 发布密钥换成这次生成的（install.sh 和 cca 里的 RELEASE_KEY 一起换）；单独的 HOME，用 bash（PATH 该写进 ~/.bashrc）
I="$D/i"; mkdir -p "$I/gh" "$I/bin" "$I/home" "$I/tmp"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$I/relkey" </dev/null
pub=$(cut -d' ' -f1-2 "$I/relkey.pub")
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$R/cca" > "$I/gh/cca"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$pub test-release\"|" "$R/install.sh" > "$I/install.sh"
ssh-keygen -q -Y sign -f "$I/relkey" -n cca-release "$I/gh/cca" </dev/null
cat > "$I/bin/curl" <<S
#!/bin/sh
out=""; url=""
while [ \$# -gt 0 ]; do case "\$1" in -o) out=\$2; shift ;; https://*) url=\$1 ;; esac; shift; done
case "\$url" in https://github.com/rivia7/claude-accounts/releases/latest/download/cca|https://github.com/rivia7/claude-accounts/releases/latest/download/cca.sig) cp "$I/gh/\${url##*/}" "\$out" ;; *) exit 22 ;; esac
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

out=$(CCA_TTY=$(ans n n) "$R/cca" install 2>&1)
if [ -f "$H/.cca/config.json" ]; then ok "cca install"; else no "cca install：$(printf '%s' "$out" | tail -n 3)"; echo "  结果：$pass 通过，$fail 失败"; exit 1; fi
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

t0=$SECONDS
out=$(CCA_TTY=$(ans y) "$R/cca" local setup 2>&1)
if printf '%s' "$out" | grep -q '本机端口池已就绪'; then ok "cca local setup：下载、校验内核，后台托管，端口池就绪（$((SECONDS - t0)) 秒）"
else no "cca local setup：$(printf '%s' "$out" | tail -n 4)"; fi
st=$("$R/cca" local status 2>&1)
if printf '%s' "$st" | grep -q 'CCA-1  127.0.0.1:7901  监听中  → US-A'; then ok "cca local status：CCA-1 在监听、切到了账号的节点"
else no "cca local status：$(printf '%s' "$st" | head -n 5)"; fi

# 测试节点（本机另一个 mihomo，用 cca 刚下载的内核）和本机的 HTTP 服务：经账号的端口 → CCA-1 → US-A → DIRECT → 它
core=$(ls "$H"/.cca/core/mihomo-v* 2>/dev/null | head -n 1)
echo '{"mixed-port": 18080, "bind-address": "127.0.0.1", "allow-lan": false, "mode": "direct", "log-level": "warning"}' > "$D/up/config.yaml"
[ -x "$core" ] && { "$core" -d "$D/up" -f "$D/up/config.yaml" > "$D/up.log" 2>&1 & echo $! >> "$D/pids"; }
echo cca-linux-ok > "$D/www/ok.txt"
( cd "$D/www" && exec python3 -m http.server 18081 --bind 127.0.0.1 > /dev/null 2>&1 ) & echo $! >> "$D/pids"
get() { curl -q -s -m 5 --noproxy '' -x "http://127.0.0.1:${1:-7901}" http://127.0.0.1:18081/ok.txt 2>/dev/null; }   # --noproxy ''：环境里的 no_proxy 不算数
for i in $(seq 1 40); do [ "$(get)" = cca-linux-ok ] && break; sleep 0.25; done
if [ "$(get)" = cca-linux-ok ]; then ok "经账号的端口出去（CCA-1 → US-A）"; else no "经账号的端口出不去"; fi
if [ -z "$(get 7902)" ]; then ok "空槽位（CCA-2 → REJECT）拒绝连接"; else no "空槽位的端口居然出得去"; fi

keeper() {   # 这次测试的托管进程（按 HOME 认：机器上可能还有别人的 cca）
	local p
	for p in /proc/[0-9]*; do
		{ tr '\0' ' ' < "$p/cmdline"; } 2>/dev/null | grep -q ' core-keep $' && { tr '\0' '\n' < "$p/environ"; } 2>/dev/null | grep -qxF "HOME=$H" && echo "${p#/proc/}"
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

# 登录：这台机器上没有账号专属的 Chrome。claude 照常在这里登录、开本机的回调，cca 给出在 Mac 上运行的一行（cca login-page …），
# 贴回 Mac 上拿到的验证码（code#state），cca 核对过才转给本机的回调；换 token 经账号的端口（假 claude 记下它拿到的代理）。
# 出口检测（CCA_SKIP_CHECK=1）跳过：测试节点是直连，这台机器的出口不一定在能用的地区
echo st-linux > "$D/oauth_state"
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans 'good-linux#st-other' 'good-linux#st-linux') "$R/cca" login alice 2>&1)
if printf '%s' "$out" | grep -qF "cca login-page alice@example.com --node US-A 'https://claude.com/cai/oauth/authorize?" &&
	printf '%s' "$out" | grep -q 'state 对不上' && grep -q 'EXCHANGE code=good-linux https_proxy=http://127.0.0.1:7901' "$D/claude.log" 2>/dev/null &&
	printf '%s' "$out" | grep -q '已重新登录'; then ok "cca login：给出在 Mac 上运行的一行；state 对不上的不转，对的转给本机的回调，经账号的端口换 token"
else no "cca login：$(printf '%s' "$out" | tail -n 4)"; fi
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans) "$R/cca" @alice -p hi 2>&1); rc=$?
if [ $rc -eq 0 ] && grep -q 'SESSION https_proxy=http://127.0.0.1:7901 args=.*-p hi' "$D/claude.log" 2>/dev/null; then ok "cca @alice：启动账号，claude 拿到的代理是账号的端口"
else no "cca @alice（退出码 $rc）：$(printf '%s' "$out" | tail -n 4)"; fi
cred=$(ls "$H"/.cca/accounts/alice-example-com-*/claude/.credentials.json 2>/dev/null)
out=$(CCA_SKIP_CHECK=1 CCA_TTY=$(ans '') "$R/cca" login alice 2>&1)
sleep 0.5
if [ -n "$cred" ] && [ ! -e "$cred" ] && printf '%s' "$out" | grep -q '已取消登录' && ! pgrep -f "$D/bin/claude auth login" >/dev/null 2>&1; then
	ok "cca login 先退出：Claude Code 的凭证文件（.credentials.json）删掉；回车取消时本机等着回调的 claude 跟着停掉"
else no "退出登录 / 取消登录：凭证文件 ${cred:-没找到} $([ -e "$cred" ] && echo 还在)；$(printf '%s' "$out" | tail -n 2)"; fi

# API key：没有钥匙串，存成 ~/.cca/secrets 下权限 600 的文件；第三方 API 账号启动时 apiKeyHelper 回调 cca 从文件里取
K1=sk-ant-api03-LINUXtest0123456789abcdWXYZ; K2=sk-test-LINUX2222
CCA_TTY=$(ans "$K1") "$R/cca" key alice >/dev/null 2>&1
f=$(ls "$H"/.cca/secrets/cca-ref-alice-example-com-* 2>/dev/null)
if [ -n "$f" ] && [ "$(stat -c %a "$H/.cca/secrets")" = 700 ] && [ "$(stat -c %a "$f")" = 600 ] && [ "$("$R/cca" key alice --show 2>/dev/null)" = "$K1" ]; then
	ok "cca key：存成 ~/.cca/secrets 下 600 的文件（目录 700），--show 读得出来"
else no "cca key：${f:-没有文件} $(stat -c %a "$f" 2>/dev/null)"; fi
out=$(CCA_TTY=$(ans kf http://127.0.0.1:18081/v1 "$K2" '' '') "$R/cca" add api 2>&1)
rm -f "$D/claude.key"
out2=$(CCA_TTY=$(ans) "$R/cca" @kf -p hi 2>&1); rc=$?
if [ $rc -eq 0 ] && [ "$(cat "$D/claude.key" 2>/dev/null)" = "$K2" ]; then ok "第三方 API 账号：key 在 600 的文件里，启动时 apiKeyHelper 回调 cca 取到"
else no "第三方 API 账号（退出码 $rc）：$(printf '%s\n%s' "$out" "$out2" | tail -n 4)"; fi
CCA_TTY=$(ans y) "$R/cca" rm kf >/dev/null 2>&1
if [ -z "$(ls "$H"/.cca/secrets/cca-api-kf-* 2>/dev/null)" ]; then ok "cca rm：第三方 API 账号的 key 文件一起删掉"; else no "cca rm 之后 key 文件还在"; fi

# 只在 macOS 上的命令：说清楚（不说"没装 Chrome""要 macOS 26"这些），cca help 不列它们
bad=""
for c in web billing limit-resets app gui; do
	out=$("$R/cca" "$c" 2>&1) && bad="$bad $c（退出码 0）"
	printf '%s' "$out" | grep -q '只在 macOS 上；这台 Linux 上用命令行' || bad="$bad $c：$(printf '%s' "$out" | tail -n 1)"
done
if [ -z "$bad" ]; then ok "cca web、billing、limit-resets、app、gui：说只在 macOS 上"; else no "只在 macOS 上的命令：$bad"; fi
out=$("$R/cca" help 2>&1)
if ! printf '%s' "$out" | grep -qE '^  cca (web|billing|limit-resets|app|gui|login-page) |^  cca codex app ' &&
	printf '%s' "$out" | grep -q '只在 macOS 上' && printf '%s' "$out" | grep -q '由 cca 在后台运行' && printf '%s' "$out" | grep -q '^  cca usage '; then
	ok "cca help：不列只在 macOS 上的命令，内置代理说的是由 cca 在后台运行"
else no "cca help：$(printf '%s' "$out" | grep -E '^  cca (web|billing|limit-resets|app|gui|login-page) |^  cca codex app ' | head -n 2)"; fi

out=$("$R/cca" local off 2>&1)
for i in $(seq 1 20); do [ -z "$(keeper)" ] && ! get >/dev/null && break; sleep 0.25; done
if [ -z "$(keeper)" ] && ! (exec 3<>/dev/tcp/127.0.0.1/7901) 2>/dev/null; then ok "cca local off：托管进程退出、内核停了"; else no "cca local off 后内核或托管进程还在"; fi
grep -iE 'level=(error|fatal)|panic' "$H/.cca/core/core.log" 2>/dev/null | head -n 2 | sed 's/^/  内核日志：/'
echo "  结果：$pass 通过，$fail 失败"
[ $fail -eq 0 ]
