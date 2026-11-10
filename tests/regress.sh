#!/bin/bash
# 全流程回归：端口池（槽位 + 策略组，API 切换不重载）、重载需同意、账号表、出口检测、登录校验、第三方 API 账号、
# 订阅用量、Codex 账号、自动更新、还原
# 用法：bash tests/regress.sh（只在 macOS 上跑；全部用假命令，不联网、不碰真实的 ~/.cca 和路由器）
D=$(cd "$(dirname "$0")" && pwd)
CCA="$D/../cca"; CCA=$(cd "$(dirname "$CCA")" && pwd)/cca
T=$(mktemp -d "${TMPDIR:-/tmp}/cca-test.XXXXXX") && T=$(cd "$T" && pwd -P); H=$T/home; R=$T/router   # 规整成真实路径（TMPDIR 结尾有 / 时会出现 //）
STUN="" STUN_PID=""   # 假的 STUN 服务器（泄露检测那一节才开）；平时 CCA_STUN 指向不通的端口：UDP 出不去，马上就知道
SVP=""   # 设备统计的测试服务器（11b3c 那一节才开）
trap '[ -z "$SVP" ] || kill "$SVP" 2>/dev/null; [ -z "$STUN_PID" ] || kill "$STUN_PID" 2>/dev/null; [ ! -f "$T/core/up" ] || kill "$(cat "$T/core/pid")" 2>/dev/null; [ -n "${KEEP:-}" ] && echo "沙箱保留在 $T" || rm -rf "$T"' EXIT
bash "$D/setup.sh" "$T" >/dev/null || exit 1
cd "$T" || exit 1
# TIMING=文件：找慢在哪。每个 ok 记一行“距上一个 ok 几秒<TAB>调用 ok 的行号<TAB>这段时间 cca 起了几次 python<TAB>sleep 了几秒<TAB>说明”；
# python 的次数靠 CCA_PYTHON 指向的一个记数的小脚本（只在 TIMING 时用，平时 cca 照常找 python3），sleep 靠 $T/bin 里的 sleep
#（cca、假命令、经假 ssh 跑的路由器端都从 PATH 找它；后台进程的也算在内）
PYWRAP=""
if [ -n "${TIMING:-}" ]; then
	: > "$TIMING"; : > "$T/py.count"; TIMING_PY=0
	PYWRAP="$T/pycount"
	# 每次记一行：参数里短的、不带换行的那些（长的是内嵌的 Python 源码），第一个不是开关、不是目录的就是 py 的子命令。
	# 用 dash 写：/bin/sh（bash）带着几百 KB 的参数起一次要多花二三十毫秒，比要量的还多；python 用它转发到的真正的那个（和 cca 一样）
	printf '#!/bin/dash\nfor a in "$@"; do case $a in *"\n"*) ;; *) [ ${#a} -lt 120 ] && printf "%%s " "$a" ;; esac; done >> "%s/py.count"; echo >> "%s/py.count"\nexec %s "$@"\n' \
		"$T" "$T" "$(PATH=/opt/homebrew/bin:/usr/bin:/bin python3 -I -c 'import sys; print(sys.executable)')" > "$PYWRAP"
	chmod 755 "$PYWRAP"
	printf '#!/bin/dash\necho "$1" >> "%s/sleep.log"\nexec /bin/sleep "$@"\n' "$T" > "$T/bin/sleep"; chmod 755 "$T/bin/sleep"; : > "$T/sleep.log"
	TIMING_T=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
fi
run() { env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_POLICY_DEFAULT= CCA_STUN="${STUN:-127.0.0.1:9}" CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" CCA_PYTHON="$PYWRAP" CCA_RETRY_WAIT=0 PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
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
ok "设备标识：旧配置迁移、共享链接、客户端重置、改别名和并发初始化" 'python3 "$D/device_identity.py"'
ok "共享资料：历史记忆配置跨账号可见，未知状态隔离，配置合并和冲突副本保留" 'python3 "$D/shared_state.py"'
ok "浏览器隔离：不降级默认浏览器，预检和启动失败传递" 'python3 "$D/isolation_browser.py"'
ok "泄露检测：失败结果不误报通过，区分宿主探针与浏览器" 'python3 "$D/isolation_leaks.py"'
ok "配置隔离：完整 TOML、项目配置与启动参数检查" 'python3 "$D/isolation_settings.py"'
ok "凭证隔离：固定钥匙串目录，屏蔽继承凭证描述符" 'python3 "$D/isolation_credentials.py"'
ok "网络隔离：节点回读、失败阻断、探针不继承代理覆盖" 'python3 "$D/isolation_network.py"'
ok "运行记录：进程复用、代理漂移与目录链接检查" 'python3 "$D/isolation_runtime.py"'
ok "App 隔离：环境清理、启动锁、真实资料目录证据" 'python3 "$D/isolation_apps.py"'
ok "客户端更新：签名、安装确认、目标漂移与中断恢复" 'python3 "$D/client_updates.py"'
ok "设备统计：服务端验签（和 ssh-keygen 交叉核对）、设备清单、状态，客户端的扫描、闲置判定、经 curl 钉住证书公钥" 'python3 "$D/presence.py" 2>/dev/null'
ok "cca usage 的表格（一个账号一行、不超过终端的宽度）、周结算提醒（use_first）、API key 的掩码" 'python3 "$D/usage_view.py" 2>/dev/null'

# 1. 安装 + 第一个账号：建端口池需要一次重载，先征求同意
out=$(run CCA_TTY="$(ans y 192.0.2.10 '' '' '1 2 3 4' y alice@example.com '' 1 y)" bash "$CCA" install 2>&1)
ok "install：从路由器的节点里选可选节点" 'python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))[\"nodes\"]==[\"US-A\",\"US-B\",\"US-C\",\"US-D\"] else 1)" "$H/.cca/config.json" && printf "%s" "$out" | grep -q "账号可选的出口节点：US-A、US-B、US-C、US-D"'
ok "install：建端口池前先问，提示重载约 20 秒" 'printf "%s" "$out" | grep -q "约 20 秒，期间局域网的新连接会中断"'
ok "install：8 个槽位 7901-7908，CCA-1 默认 US-A，空槽位默认 REJECT" '[ "$(nl_)" = 8 ] && [ "$(first CCA-1)" = US-A ] && [ "$(first CCA-8)" = REJECT ] && [ "$(sel CCA-1)" = US-A ]'
ok "install：只重载了一次" '[ "$(reloads)" = 1 ]'
ok "install：同时建好 cxa（等于 cca codex），是指向同目录 cca 的相对链接" '[ "$(readlink "$H/.local/bin/cxa")" = cca ] && printf "%s" "$out" | grep -q "以后可以直接用 cxa 代替 cca codex：cxa @别名 直接启动"'
python3 - > "$T/accounts.golden" <<'PY'
import hashlib, json
n = 'alice-example-com-' + hashlib.sha1(b'alice@example.com').hexdigest()[:6]
print(json.dumps({'base': 7900, 'slots': 8, 'nodes': ['US-A', 'US-B', 'US-C', 'US-D'], 'accounts': [{'slot': 1, 'name': n, 'node': 'US-A'}]}))
PY
ok "install：路由器上的账号表和以前的版本逐字节一样（只有 Claude 账号时它变了，已安装的机器都会重新部署路由器端）" 'cmp -s "$T/accounts.golden" "$R/etc/cca/accounts.yaml"'
ok "install：钩子 / cron / sysupgrade" 'grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && grep -q ensure "$R/etc/crontabs/root" && grep -qx /etc/cca/ "$R/etc/sysupgrade.conf"'
AD=$(ls -d "$H"/.cca/accounts/alice-example-com-*); alice_device=$(claude_device "$AD")
ok "install：未登录时已生成账号自己的设备标识" 'printf "%s\n" "$alice_device" | grep -Eq "^[0-9a-f]{64}:[0-9a-f]{64}$" && [ -f "$AD/device.json" ]'

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

# 3. 登录 + 启动 + 出口检测
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "alice：预填邮箱登录 → 启动，退出码透传" '[ $rc -eq 3 ] && grep -q "ARGS: auth login --claudeai --email alice@example.com" claude.log'
ok "Claude 登录时已读到固定设备标识，退出同步不改变" 'grep -qF "LOGIN-DEVICE $alice_device @ $AD/claude" claude.log && [ "$(claude_device "$AD")" = "$alice_device" ]'
ok "alice：出口检测经 :7901（trace + iprisk）" 'grep -q "CURL via :7901 https://ip.net.coffee/api/iprisk/203.0.113.1" curl.log'

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

# 4. 选中节点被改动（仪表盘手动切 / 缓存恢复错）→ 启动前自动纠正，不重载
printf US-D > "$R/sel/CCA-1"
run cca @alice --version >/dev/null 2>&1
ok "选中被改：启动前经 API 纠正回 US-A，不重载" '[ "$(sel CCA-1)" = US-A ] && [ "$(reloads)" = 1 ]'
printf HK-A > "$R/sel/CCA-1"; rr ensure
ok "选中被改：cron 自愈也会纠正" '[ "$(sel CCA-1)" = US-A ] && [ "$(reloads)" = 1 ]'

# 5. 手改账号表换节点：只走 API
edit_table 'j["副号(bob@example.com)"]="US-C"'
run cca @bob >/dev/null 2>&1
ok "手改账号表换节点：启动时同步，CCA-2 → US-C，不重载" '[ "$(sel CCA-2)" = US-C ] && [ "$(reloads)" = 1 ]'

# 6. 删除账号：槽位回到 REJECT，不重载
run CCA_TTY="$(ans y)" cca rm bob >/dev/null 2>&1
ok "rm bob：退出登录、删钥匙串；CCA-2 → REJECT；不重载" 'grep -q "delete-generic-password -s Claude Code-credentials-" security.log && [ "$(sel CCA-2)" = REJECT ] && [ "$(reloads)" = 1 ] && ! grep -q bob "$H/.cca/accounts.json"'
ok "rm：账号的启动器一起删掉，别的账号的还在" '[ ! -e "$H/Applications/Claude 副号.app" ] && [ -d "$H/Applications/Claude alice.app" ]'

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

# 8b. 旧入站占着端口（真实路由器上发生过）：第一次重载 7901 绑定失败 → 自动再重载一次
edit_table 'j.update({"u%d@example.com" % i: "US-D" for i in range(2, 10)})'
touch "$R/reload_conflict_once"; n0=$(reloads)
out=$(run cca router sync -y 2>&1); rc=$?
ok "旧入站占端口：第一次没绑上 7901，自动再重载一次后成功" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "再重载一次" && [ "$(reloads)" = $((n0 + 2)) ] && grep -qx 7901 "$R/listening" && [ "$(nl_)" = 16 ]'
edit_table '[j.pop("u%d@example.com" % i) for i in range(2, 10)]'
run cca router sync -y >/dev/null 2>&1

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

# 9. OpenClash 升级/重装：钩子丢了、重启后没有端口池
cp pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh"
cp "$R/etc/openclash/config/custom.yaml" "$RC"; printf '7890\n7891\n7892\n7893\n7895\n9090\n' > "$R/listening"; rm -f "$R"/sel/CCA-*
n0=$(reloads); rr ensure
ok "cron 自愈：补回钩子，但不自动重载（只记录）" 'grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ "$(reloads)" = "$n0" ] && grep -q "端口池不在运行中的 OpenClash 里" "$R/tmp/cca-router.log"'
cp pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh"
run CCA_TTY="$(ans y)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "启动时发现端口池不在：征得同意后重载，补回钩子，正常启动" '[ $rc -eq 3 ] && [ "$(nl_)" = 8 ] && [ "$(sel CCA-1)" = US-A ] && grep -q ">>> cca >>>" "$R/etc/openclash/custom/openclash_custom_overwrite.sh"'

# 10. OpenClash 启动时由钩子注入端口池（不额外重载）
cp "$R/etc/openclash/config/custom.yaml" "$R/tmp/custom.yaml"
grep -v '^\. /usr/share\|LOG_OUT' "$R/etc/openclash/custom/openclash_custom_overwrite.sh" > ow_run.sh
env CCA_ROOT="$R" CCA_T="$T" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin" /bin/dash ow_run.sh "$R/tmp/custom.yaml"
ok "钩子：启动时把 8 个槽位注入配置，CCA-1 默认 US-A" '[ "$(ruby -ryaml -e "c=YAML.load_file(ARGV[0]); print c[\"listeners\"].size" "$R/tmp/custom.yaml")" = 8 ] && ruby -ryaml -e "c=YAML.load_file(ARGV[0]); exit(c[\"proxy-groups\"].find { |g| g[\"name\"]==\"CCA-1\" }[\"proxies\"][0]==\"US-A\" ? 0 : 1)" "$R/tmp/custom.yaml"'

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
	script -q "$T/pty.log" env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_POLICY_DEFAULT= CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" \
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

# 10b. 出门在外：连不上路由器时改走本机 Clash Verge（全局扩展脚本 + 热加载，不重启它），回家后自动走回路由器
V="$H/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev"; VS="$V/profiles/Script.js"
vreloads() { grep -c '^RELOAD$' "$T/verge/verge.log" 2>/dev/null || echo 0; }
vsel() { cat "$T/verge/sel/$1" 2>/dev/null || echo -; }
lastenv() { grep "^ENV: " claude.log | tail -n 1; }
touch "$T/away"; : > ssh.log
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面、还没配置本机 Clash、非交互：说明原因，不启动，也不去连路由器" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "连不上路由器 192.0.2.10" && printf "%s" "$out" | grep -q "cca local setup" && [ ! -s ssh.log ] && [ "$(vreloads)" = 0 ]'
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "在外面：问过之后配置 Clash Verge（全局扩展脚本末尾加一段，热加载一次，不重启它）" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "现在配置 Clash Verge 吗" && grep -q ">>> cca >>>" "$VS" && [ "$(vreloads)" = 1 ] && grep -qx 7908 "$T/verge/listening" && ! grep -q clash-verge-rev open.log'
ok "在外面：经本机 7901 出去（CCA-1 切到 US-A），Claude Code 拿到的代理也是它" '[ "$(vsel CCA-1)" = US-A ] && grep -q "CURL via local:7901 https://claude.ai/cdn-cgi/trace" curl.log && lastenv | grep -q "https_proxy=http://127.0.0.1:7901" && lastenv | grep -q "CCA_PROXY=http://127.0.0.1:7901" && [ ! -s ssh.log ]'
ok "热加载的配置：原来的节点、组和规则都在，CCA 组接在后面；Clash Verge 里没有的 US-D 不放进组" 'ruby -ryaml -rjson -e "c = YAML.safe_load(JSON.parse(File.read(ARGV[0]))[\"payload\"]); g = c[\"proxy-groups\"]; exit(c[\"proxies\"].size == 4 && g[0][\"name\"] == \"Proxy\" && g.size == 9 && c[\"rules\"] == [\"MATCH,Proxy\"] && c[\"listeners\"].size == 8 && g[1][\"proxies\"] == %w[US-A US-B US-C REJECT] && g[2][\"proxies\"] == %w[REJECT US-A US-B US-C] ? 0 : 1)" "$T/verge/last_put.json"'
"$T/verge_regen"
ok "全局扩展脚本：用真的 JavaScript 引擎跑一遍（像 Clash Verge 重新生成配置），结果和热加载的一样，选中的节点保留" 'ruby -rjson -e "c = JSON.parse(File.read(ARGV[0])); g = c[\"proxy-groups\"]; exit(g[0][\"name\"] == \"Proxy\" && g.size == 9 && c[\"rules\"] == [\"MATCH,Proxy\"] && c[\"listeners\"].map { |l| l[\"port\"] } == (7901..7908).to_a && c[\"listeners\"].all? { |l| l[\"listen\"] == \"127.0.0.1\" && l[\"users\"] == [] } && g[1] == {\"name\" => \"CCA-1\", \"type\" => \"select\", \"proxies\" => %w[US-A US-B US-C REJECT]} ? 0 : 1)" "$T/verge/regen.json" && [ "$(vsel CCA-1)" = US-A ]'
ok "全局扩展脚本：在已经带端口池的配置上再跑一次，结果不变" '/usr/bin/osascript -l JavaScript "$T/verge/run_script.js" "$VS" "$T/verge/regen.json" > "$T/verge/regen2.json" && ruby -rjson -e "exit(JSON.parse(File.read(ARGV[0])) == JSON.parse(File.read(ARGV[1])) ? 0 : 1)" "$T/verge/regen.json" "$T/verge/regen2.json"'
sed -i '' '4s|.*|  config.rules.unshift("DOMAIN,user.example,DIRECT"); return config;|' "$VS"; "$T/verge_regen"
ok "全局扩展脚本：用户自己的 main 照常执行，端口池加在它的结果上" 'ruby -rjson -e "c = JSON.parse(File.read(ARGV[0])); exit(c[\"rules\"] == [\"DOMAIN,user.example,DIRECT\", \"MATCH,Proxy\"] && c[\"listeners\"].size == 8 ? 0 : 1)" "$T/verge/regen.json"'
sed -i '' '4s|.*|  return config;|' "$VS"
n0=$(vreloads)
run cca @alice --version >/dev/null 2>&1
ok "在外面再启动：不再重载，只确认选中节点" '[ "$(vreloads)" = "$n0" ]'
printf US-B > "$T/verge/sel/CCA-1"
run cca @alice --version >/dev/null 2>&1
ok "选中被改：启动前经控制接口纠正回 US-A，不重载" '[ "$(vsel CCA-1)" = US-A ] && [ "$(vreloads)" = "$n0" ]'
edit_table 'j["alice@example.com"] = "US-C"'
out=$(run cca @alice --version 2>&1)
ok "在外面换节点：经控制接口切到 US-C，不重载；脚本里 CCA-1 默认的节点跟着改；路由器等回家再同步" '[ "$(vsel CCA-1)" = US-C ] && [ "$(vreloads)" = "$n0" ] && grep -qF "[\"CCA-1\", 7901, [\"US-C\"" "$VS" && printf "%s" "$out" | grep -q "回到家里的网络后再同步" && [ ! -s ssh.log ]'
edit_table 'j["alice@example.com"] = "US-D"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面：Clash Verge 里没有账号的节点时不启动（宁可断开也不换出口）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有节点「US-D」" && [ "$(vsel CCA-1)" = REJECT ]'
edit_table 'j["alice@example.com"] = "US-A"'
out=$(run CCA_TTY="$(ans carol@example.com 小号 2)" cca add 2>&1)
ok "在外面加账号：不连路由器，回家后下次启动再同步" 'printf "%s" "$out" | grep -q "回到家里的网络后" && grep -q "小号(carol@example.com)" "$H/.cca/accounts.json" && [ ! -s ssh.log ]'
: > open.log
run CCA_TTY="$(ans)" cca web 小号 >/dev/null 2>&1
ok "在外面：网页版和第一次登录也走本机端口（CCA-2 切到 US-B）" '[ "$(vsel CCA-2)" = US-B ] && grep -q "LOGIN carol@example.com" claude.log && tail -n 1 open.log | grep -qF -- "--proxy-server=http://127.0.0.1:7902"'
: > open.log
run CCA_ACCOUNT='小号(carol@example.com)' CCA_PROXY=http://127.0.0.1:7902 "$H/.cca/bin/open-url" 'https://claude.com/cai/oauth/authorize?code=true' >/dev/null 2>&1
ok "登录时打开的浏览器用启动它的 cca 选好的出口（在外面是本机端口）" 'grep -qF -- "--proxy-server=http://127.0.0.1:7902" open.log'
out=$(run cca check 2>&1)
ok "在外面 cca check：经本机端口检测各账号的出口" 'printf "%s" "$out" | grep -q "198.51.100.1" && printf "%s" "$out" | grep -q "198.51.100.2" && [ ! -s ssh.log ]'
out=$(run cca ls 2>&1)
ok "在外面 cca ls：看本机端口" 'printf "%s" "$out" | grep -q "alice@example.com 的本机端口 7901 已就绪"'
out=$(run cca local 2>&1)
ok "cca local：现在走哪条路、各端口的监听状态和选中的节点" 'printf "%s" "$out" | grep -q "现在连不上路由器 192.0.2.10" && printf "%s" "$out" | grep -q "CCA-1  127.0.0.1:7901  监听中  → US-A   alice@example.com"'
touch "$T/verge/down"
out=$(run cca @alice --version 2>&1); rc=$?
ok "Clash Verge 没在运行、非交互：说明后停止" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "Clash Verge 没在运行"'
n0=$(vreloads)
run CCA_TTY="$(ans y)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "Clash Verge 没在运行、交互：问过之后打开它（它按全局扩展脚本生成端口池），不用再热加载" '[ $rc -eq 3 ] && grep -q "io.github.clash-verge-rev.clash-verge-rev" open.log && [ "$(vreloads)" = $((n0 + 1)) ] && [ "$(vsel CCA-1)" = US-A ]'
edit_table 'j.update({"u%d@example.com" % i: "US-B" for i in range(3, 10)})'
n0=$(vreloads)
run cca @alice --version >/dev/null 2>&1
ok "在外面端口池要扩容（第 9 个账号）：自动热加载一次 Clash Verge，变成 16 个端口" '[ "$(vreloads)" = $((n0 + 1)) ] && grep -qx 7916 "$T/verge/listening" && [ "$(vsel CCA-9)" = US-B ]'
edit_table '[j.pop("u%d@example.com" % i) for i in range(3, 10)]'
touch "$T/verge/reload_reject"
out=$(run cca @alice --version 2>&1); rc=$?
ok "Clash Verge 的内核拒绝新配置：说明原因，不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没接受新配置（HTTP 400）"'
rm -f "$T/verge/reload_reject"
# Clash Verge 里同一个节点叫别的名字（US-B 改叫 Verge-B，地址不变）：按服务器地址换算成 Verge 里的叫法，账号表不动
cp "$T/verge/base.yaml" "$T/verge/base.orig"
sed -i '' 's/^- name: US-B$/- name: Verge-B/' "$T/verge/base.yaml"; "$T/verge_regen"
out=$(run cca @小号 --version 2>&1); rc=$?
ok "在外面：Clash Verge 里 US-B 叫 Verge-B（地址一样）：换算成 Verge-B 照常启动，账号表不动，端口池里也用 Verge 的叫法" '[ $rc -eq 3 ] && [ "$(vsel CCA-2)" = Verge-B ] && grep -q "\"小号(carol@example.com)\": \"US-B\"" "$H/.cca/accounts.json" && ! printf "%s" "$out" | grep -q "没有节点" && grep -qF "[\"CCA-2\", 7902, [\"Verge-B\"" "$VS"'
out=$(run cca local 2>&1)
ok "cca local：选中的是 Verge 里叫 Verge-B 的节点" 'printf "%s" "$out" | grep -q "CCA-2  127.0.0.1:7902  监听中  → Verge-B   小号"'
cp "$T/verge/base.orig" "$T/verge/base.yaml"; "$T/verge_regen"
run cca @小号 --version >/dev/null 2>&1
ok "Verge 里改回 US-B：跟着换算回来" '[ "$(vsel CCA-2)" = US-B ] && grep -qF "[\"CCA-2\", 7902, [\"US-B\"" "$VS"'
rm -f "$T/away"; n0=$(vreloads); : > ssh.log
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "回到家：自动走回路由器，补同步在外面改过的账号表（CCA-2 → US-B），不碰 Clash Verge" '[ $rc -eq 3 ] && grep -q "sh -s" ssh.log && [ "$(sel CCA-2)" = US-B ] && [ "$(vreloads)" = "$n0" ] && lastenv | grep -q "https_proxy=http://192.0.2.10:7901"'
run cca local off >/dev/null 2>&1
ok "cca local off：全局扩展脚本逐字节还原，端口池从内核里移除" 'cmp -s "$VS" "$T/pristine_script.js" && ! grep -qx 7901 "$T/verge/listening" && [ "$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[\"local\"])" "$H/.cca/config.json")" = "{}" ]'
run CCA_TTY="$(ans y)" cca local setup >/dev/null 2>&1
ok "cca local setup：在家也能先配置好" 'grep -q ">>> cca >>>" "$VS" && grep -qx 7901 "$T/verge/listening"'
run CCA_TTY="$(ans y)" cca rm 小号 >/dev/null 2>&1

# 10c. 第三方 API 账号：直连 / 走节点；key 存钥匙串，经 apiKeyHelper 交给 Claude Code（不进命令行，也不进环境变量）
kcd() { ls -d "$T/keychain/${2:-cca-api-}$1-"* 2>/dev/null | head -n 1; }   # 钥匙串里这个账号的条目（目录里一个服务商一个文件）[服务名前缀]
kc() { local d; d=$(kcd "$1" "${2:-}"); [ -n "$d" ] && cat "$d"/* 2>/dev/null; }
kch() { local d; d=$(kcd "$1"); [ -n "$d" ] && ls "$d"; }   # 存它的服务商（接口的域名）
lastline() { grep "^$1: " "${2:-claude.log}" | tail -n 1; }   # [日志]
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
ok "官方账号：shell 里继承的 ANTHROPIC_BASE_URL 不带进去（否则登录 token 会发到别的地址），也不加 --settings" '[ $rc -eq 3 ] && lastenv | grep -q "ANTHROPIC_BASE_URL=unset" && lastenv | grep -q "BROWSER=$H/.cca/bin/open-url" && ! lastline ARGS | grep -q -- --settings && printf "%s" "$out" | grep -q "屏蔽环境里继承的 ANTHROPIC_BASE_URL"'
# Claude 默认对 ~/.cca 放开权限（加成工作目录），登录凭证和浏览器资料除外
P1="--add-dir=$H/.cca --disallowedTools=Read(/$H/.cca/policy/id_ed25519),Edit(/$H/.cca/policy/id_ed25519),Read(/$H/.cca/policy/nodes.json),Edit(/$H/.cca/policy/nodes.json),Read(/$H/.cca/backup/clash-verge-*.yaml),Edit(/$H/.cca/backup/clash-verge-*.yaml),Read(/$H/.cca/codex/accounts/*/codex/auth.json),Edit(/$H/.cca/codex/accounts/*/codex/auth.json),Read(/$H/.cca/accounts/*/chrome/**),Edit(/$H/.cca/accounts/*/chrome/**),Read(/$H/.cca/accounts/*/desktop/**),Edit(/$H/.cca/accounts/*/desktop/**),Read(/$H/.cca/codex/accounts/*/chrome/**),Edit(/$H/.cca/codex/accounts/*/chrome/**),Read(/$H/.cca/codex/accounts/*/desktop/**),Edit(/$H/.cca/codex/accounts/*/desktop/**),Read(/$H/.cca/core/nodes.json),Edit(/$H/.cca/core/nodes.json),Read(/$H/.cca/core/config.yaml),Edit(/$H/.cca/core/config.yaml)"
out=$(run CCA_TTY="$(ans)" cca @alice "看看 ~/.cca 里的配置" 2>&1); rc=$?
ok "Claude 默认对 ~/.cca 放开权限：--add-dir 加成工作目录，登录凭证和浏览器资料用 --disallowedTools 挡住；都写成 --x=值，后面的提示词不会被当成目录" '[ $rc -eq 3 ] && [ "$(lastline ARGS)" = "ARGS: $P1 看看 ~/.cca 里的配置" ]'
run CCA_TTY="$(ans)" cca @kimi --version >/dev/null 2>&1; a1=$(lastline ARGS)
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['claude_add_dir'] = False; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; a2=$(lastline ARGS)
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('claude_add_dir'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
ok "第三方 API 账号同样放开（在 --settings 前面）；config.json 的 claude_add_dir 设成 false 就不加" 'printf "%s" "$a1" | grep -qF "ARGS: $P1 --settings {" && [ "$a2" = "ARGS: --version" ]'
cp "$H/.claude/settings.json" "$T/settings.bak"; n_args=$(grep -c "^ARGS: " claude.log)
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.setdefault(\"env\", {})[\"ANTHROPIC_BASE_URL\"]=\"http://evil.invalid\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享 settings.json 的 env 里有 ANTHROPIC_BASE_URL：拒绝启动并指出来" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "settings.json:ANTHROPIC_BASE_URL" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
cp "$T/settings.bak" "$H/.claude/settings.json"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j[\"apiKeyHelper\"]=\"echo x\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @kimi --version 2>&1); rc=$?
ok "共享 settings.json 里有 apiKeyHelper：第三方 API 账号也拒绝启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "settings.json:apiKeyHelper"'
cp "$T/settings.bak" "$H/.claude/settings.json"
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j.setdefault(\"env\", {})[\"NO_PROXY\"]=\"*\"; json.dump(j, open(p, \"w\"))" "$H/.claude/settings.json"
out=$(run CCA_TTY="$(ans)" cca @alice --version 2>&1); rc=$?
ok "共享 settings.json 的 env 里有 NO_PROXY：拒绝启动（会让请求绕过账号的端口）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "settings.json:NO_PROXY"'
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
ok "账号原子保存的 settings.json 合并后含 ANTHROPIC_BASE_URL：再次检查并拒绝启动客户端" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "settings.json:ANTHROPIC_BASE_URL" && [ "$(grep -c "^ARGS: " claude.log)" = "$n_args" ]'
cp "$T/settings.atomic.bak" "$H/.claude/settings.json"
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
ok "恢复共享配置后账号正常启动，settings.json 重新保持共享链接" '[ $rc -eq 3 ] && [ -L "$shared_client/settings.json" ] && cmp -s "$shared_client/settings.json" "$H/.claude/settings.json"'
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
ok "cca key 官方账号：记一把 API key 备查，存进钥匙串单独的条目（cca-ref-，账户名 reference），config.json 记下掩码；key 不进命令行和日志" '[ $rc -eq 0 ] && [ "$(cat "$T/keychain/cca-ref-$AAID/reference")" = "$R1" ] && [ -z "$(kcd alice)" ] && cfgok "c[\"ref_keys\"] == [\"$AAID\"] and c[\"key_hints\"][\"$AAID\"] == {\"host\": \"reference\", \"hint\": \"sk-ant-…WXYZ\"}" && ! grep -qF "$R1" security.log "$T/security.stdin" "$T/env.log" && printf "%s" "$out" | grep -q "cca 不会拿它登录或启动"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1; rc=$?
out=$(run cca _api-key "$AAID" 2>/dev/null); rc2=$?
ok "官方账号记的 key 绝不拿来启动：不加 --settings，apiKeyHelper 的回调也不给它" '[ $rc -eq 3 ] && ! lastline ARGS | grep -q -- --settings && [ ! -e "$T/claude.key" ] && [ $rc2 -ne 0 ] && [ -z "$out" ]'
v=$(run cca key alice --show 2>/dev/null); rc=$?
ok "cca key 账号 --show：stdout 原样是 key，不换行" '[ $rc -eq 0 ] && [ "$v" = "$R1" ] && [ "$(run cca key alice --show 2>/dev/null | wc -c | tr -d " ")" = "${#R1}" ]'
out=$(run CLAUDECODE=1 cca key alice --show 2>&1); rc=$?
ok "在 Claude Code 的会话里运行 --show：不打出来（会进对话记录）" '[ $rc -ne 0 ] && ! printf "%s" "$out" | grep -qF "$R1" && printf "%s" "$out" | grep -q "会进对话记录"'
run cca rename alice@example.com 主号 >/dev/null 2>&1
v=$(run cca key 主号 --show 2>/dev/null); st=$(gj state)
run cca rename 主号 - >/dev/null 2>&1
ok "改了别名记下的 key 还在（按账号目录名认）；_gui state 的 api_key 照 config.json 说（掩码），不读钥匙串" '[ "$v" = "$R1" ] && printf "%s" "$st" | jok "acct(0, \"主号(alice@example.com)\")[\"api_key\"] == {\"saved\": True, \"hint\": \"sk-ant-…WXYZ\", \"source\": \"keychain\"} and acct(0, \"主号(alice@example.com)\")[\"can\"][\"api_key\"]" && ! printf "%s" "$st" | grep -qF "$R1"'
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
touch "$T/away"
run cca usage alice >/dev/null 2>&1
ok "在外面 cca usage：经本机 Clash 的端口查" 'grep "^USAGE: " claude.log | tail -n 1 | grep -q "email=alice@example.com https_proxy=http://127.0.0.1:7901"'
rm -f "$T/away"
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

# 10e. 不用路由器、只用直连的第三方 API 账号
H4="$T/home4"; mkdir -p "$H4/.claude"
r4() { env -i LANG=en_US.UTF-8 HOME="$H4" CCA_T="$T" CCA_POLICY_DEFAULT= PATH="$T/bin:$H4/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
r4 CCA_TTY="$(ans n n)" bash "$CCA" install >/dev/null 2>&1
out=$(r4 CCA_TTY="$(ans solo https://api.example.com/anthropic "$S1" '')" cca add api 2>&1); rc=$?
out2=$(r4 CCA_TTY="$(ans)" cca @solo --version 2>&1); rc2=$?
out3=$(r4 cca ls 2>&1)
ok "不配置路由器、只有直连的第三方 API 账号：照样能添加、启动和列出，不提路由器" '[ $rc -eq 0 ] && [ $rc2 -eq 3 ] && ! printf "%s%s%s" "$out" "$out2" "$out3" | grep -q "路由器" && printf "%s" "$out3" | grep -qE "solo +api.example.com +API +直连"'
r4 CCA_TTY="$(ans y)" cca rm solo >/dev/null 2>&1

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
ok "_gui state：不联网、不写文件（~/.cca 里一个文件都没变），也不经过自动更新" '[ "$(snap)" = "$s0" ] && ! grep -q "GH " gh.log 2>/dev/null'
edit_table 'j["ports"].pop("alice@example.com")' "$H/.cca/config.json"
out=$(gj state)
ok "_gui state 只在内存里对账：config.json 里少了的端口照样算出来（7901），文件不改；下一次真正运行 cca 时才写回去" 'printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"port\"] == 7901" && cfgok "\"alice@example.com\" not in c[\"ports\"]" && run cca ls >/dev/null 2>&1 && cfgok "c[\"ports\"][\"alice@example.com\"] == 7901"'
mv "$H/.cca/codex.json" "$T/codex.gui"
out=$(gj state); printf '%s' "$out" > "$GF/state_nocodex.json"; out2=$(gj hello)
ok "_gui state / hello：没有 codex.json 时 Codex 算没启用，也不会把它建出来" 'printf "%s" "$out" | jok "d[\"tools\"][1][\"enabled\"] is False and d[\"tools\"][1][\"accounts\"] == []" && printf "%s" "$out2" | jok "d[\"tools\"][\"codex\"][\"enabled\"] is False" && [ ! -e "$H/.cca/codex.json" ]'
mv "$T/codex.gui" "$H/.cca/codex.json"
edit_table 'j["kimi2"] = {"base_url": "https://api.example.com/anthropic", "model": "m1", "node": "", "key": "sk-GUI-SECRET-1234", "id": "kimi2"}'
out=$(gj state)
ok "_gui state：第三方 API 账号只给接口的域名和模型，绝不带 key；没有 App、网页版、用量" '! printf "%s" "$out" | grep -q sk-GUI-SECRET && printf "%s" "$out" | jok "(lambda a: a[\"kind\"] == \"api\" and a[\"login\"] == \"api\" and a[\"api\"] == {\"host\": \"api.example.com\", \"model\": \"m1\"} and a[\"port\"] is None and a[\"usage\"] is None and a[\"can\"] == {\"app\": False, \"web\": False, \"billing\": False, \"limit_resets\": False, \"usage\": False, \"check\": True, \"cli\": True, \"policy_lock\": False, \"devices\": False, \"api_key\": True} and a[\"can_why\"][\"app\"] == \"api_account\" and a[\"api_key\"] == {\"saved\": True, \"hint\": \"\", \"source\": \"table\"})(acct(0, \"kimi2\"))"'
ok "_gui state：能不能打开账单页，条件和网页版一样（入口的显示由 cca 说了算）" 'printf "%s" "$out" | jok "all(a[\"can\"][\"billing\"] == a[\"can\"][\"web\"] for t in d[\"tools\"] for a in t[\"accounts\"])"'
ok "_gui state：用量页入口只给 Claude 账号，其余条件和网页版一样" 'printf "%s" "$out" | jok "all(a[\"can\"][\"limit_resets\"] == (i == 0 and a[\"can\"][\"web\"]) for i, t in enumerate(d[\"tools\"]) for a in t[\"accounts\"])"'
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
ok "_gui state 的 api_key：官方账号记的 key 只给掩码（sk-proj-…EFGH），账号表里写的 key 也只给掩码（sk-GUI-…wxyz），没存的是空的；不读钥匙串" '! printf "%s" "$out" | grep -qF "$K1" && ! printf "%s" "$out" | grep -q sk-GUI-SECRET && [ "$(grep -c "find-generic-password" security.log)" = "$n_find" ] && printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"api_key\"] == {\"saved\": True, \"hint\": \"sk-proj-…EFGH\", \"source\": \"keychain\"} and acct(0, \"kimi3\")[\"api_key\"] == {\"saved\": True, \"hint\": \"sk-GUI-…wxyz\", \"source\": \"table\"} and acct(1, \"dave@example.com\")[\"api_key\"] == {\"saved\": False, \"hint\": \"\", \"source\": \"\"} and all(a[\"can\"][\"api_key\"] for t in d[\"tools\"] for a in t[\"accounts\"])"'
edit_table 'j.pop("kimi3")'; run CCA_GRANT="key_forget:${AD##*/}" cca key alice --forget >/dev/null 2>&1; run cca ls >/dev/null 2>&1
# 短的 key（kimi 的 sk-test-CCCC3333）没有掩码，也记着存的时候的域名：换了服务商同样算没存
edit_table 'j["kimi"]["base_url"] = j["kimi"]["base_url"].replace("//api.example.com", "//api9.example.com")'; out=$(gj state)
edit_table 'j["kimi"]["base_url"] = j["kimi"]["base_url"].replace("//api9.example.com", "//api.example.com")'; out2=$(gj state)
ok "_gui state 的 api_key：第三方 API 账号换了服务商（域名和存的时候不一样）算没存，短的 key（没有掩码）也一样；换回来又算存了" 'printf "%s" "$out" | jok "acct(0, \"kimi\")[\"api_key\"] == {\"saved\": False, \"hint\": \"\", \"source\": \"\"}" && printf "%s" "$out2" | jok "acct(0, \"kimi\")[\"api_key\"] == {\"saved\": True, \"hint\": \"\", \"source\": \"keychain\"}"'
cp "$AD/claude/.claude.json" "$T/alice.claude.json"; printf '{"oauthAccount": ' > "$AD/claude/.claude.json"
out=$(gj state); rc=$?; printf '%s' "$out" > "$GF/state_account_unreadable.json"
cp "$T/alice.claude.json" "$AD/claude/.claude.json"
ok "_gui state：某个账号的 .claude.json 坏了也照样输出：这个账号当作没登录，errors 里说是哪个文件" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "acct(0, \"alice@example.com\")[\"login\"] == \"logged_out\" and any(e[\"code\"] == \"account_unreadable\" and e[\"tool\"] == \"claude\" and \".claude.json\" in e[\"message\"] for e in d[\"errors\"])"'
# 实况：在家看路由器，在外面看本机 Clash Verge
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
ok "_gui live（在外面）：指定看路由器时说连不上（槽位 closed）；默认看本机 Clash Verge 的端口池" 'printf "%s" "$out" | jok "d[\"route\"] == \"router\" and d[\"at_home\"] is False and d[\"router\"][\"error\"][\"code\"] == \"unreachable\" and all(s[\"state\"] in (\"closed\", \"empty\") for s in d[\"slots\"])" && printf "%s" "$out2" | jok "d[\"route\"] == \"local\" and d[\"local\"][\"configured\"] is True and d[\"local\"][\"name\"] == \"Clash Verge\" and d[\"local\"][\"running\"] is True and d[\"slots\"][0][\"key\"] == \"alice@example.com\""'
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
# 在外面、Clash Verge 没开：问要不要打开它
touch "$T/away" "$T/verge/down"; : > open.log
out=$(runev cca @alice --version 2>&1); rc=$?
ok "图形界面：在外面、Clash Verge 没在运行：记一条 consent（open_clash_verge），不打开它、不启动" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"open_clash_verge\" and e[\"grant\"] == \"open_clash_verge\"" && [ -f "$T/verge/down" ] && ! grep -q "io.github.clash-verge-rev.clash-verge-rev" open.log'
out=$(runev CCA_GRANT=open_clash_verge cca @alice --version 2>&1); rc=$?
ok "图形界面：同意之后打开 Clash Verge，照常启动" '[ $rc -eq 3 ] && grep -q "io.github.clash-verge-rev.clash-verge-rev" open.log && [ ! -f "$T/verge/down" ]'
rm -f "$T/away"
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
out=$(runev CCA_GRANT="reset_credit:${DD##*/}" cca codex reset dave --json 2>/dev/null); rc=$?; printf '%s' "$out" > "$GF/reset.json"
ok "图形界面：同意之后重置，stdout 是 kind=reset 的文档（结果、说明、重置后的用量）" '[ $rc -eq 0 ] && printf "%s" "$out" | jok "d[\"kind\"] == \"reset\" and d[\"tool\"] == \"codex\" and d[\"key\"] == \"dave@example.com\" and d[\"outcome\"] == \"reset\" and \"已重置\" in d[\"message\"] and d[\"usage\"][\"five_hour\"][\"utilization\"] == 0 and d[\"usage\"][\"reset_credits\"][\"availableCount\"] == 0" && [ "$(grep -c "RPC resetConsume" codex.log)" = $((nc + 1)) ]'
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
# 10h. 提问会话（macos/PROTOCOL.md 4.6）：App 开的封闭流程，提问发成 ask 事件，答案从 fd 3 的管道读（tests/gui_session.py 扮演 App）
sess() {   # sess 答案JSON [驱动的参数…] -- 命令…：带答案管道运行，stdin 接 /dev/null（App 就是这样开流程的）
	local a="$1"; shift
	: > "$T/ev.jsonl"
	printf '%s' "$a" > "$T/sess.answers"
	run CCA_EVENTS="$T/ev.jsonl" CCA_PROMPT=1 python3 -I "$D/gui_session.py" "$T/ev.jsonl" "$T/sess.answers" "$@" </dev/null
}
asks() { python3 -I -c 'import json,sys; print(" ".join(e["q"] for e in map(json.loads, open(sys.argv[1])) if e.get("ev") == "ask"))' "$T/ev.jsonl"; }   # 问过的题号，按顺序
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
unlisten 7901; n=$(reloads)
out=$(sess '{}' --eof-at consent.router_reload -- cca router sync 2>&1); rc=$?
ok "提问会话：默认“是”的同意，问的时候 App 关掉管道：按取消，不重载" '[ $rc -ne 0 ] && [ "$(reloads)" = "$n" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
out=$(sess '{"consent.router_reload":[{"v":"n"}]}' -- cca router sync 2>&1); rc=$?
ok "提问会话：同意答否：不重载" '[ "$(reloads)" = "$n" ] && [ "$(evn consent)" = 0 ]'
out=$(sess '{"consent.router_reload":[{"v":"y"}]}' -- cca router sync 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_consent.jsonl"
ok "提问会话：同意当场问（不用重跑），带标题和完整的说明（路由器的说明有好几行，最后一行也在），答是就重载" '[ $rc -eq 0 ] && [ "$(reloads)" = "$((n + 1))" ] && evq "e.get(\"q\") == \"consent.router_reload\" and e[\"title\"] and e[\"fields\"][0][\"kind\"] == \"confirm\" and \"约 20 秒\" in e[\"fields\"][0][\"detail\"] and e[\"fields\"][0][\"default\"] == \"y\""'
mv "$H/.cca/router_ed25519" "$T/rk.bak"
out=$(sess '{}' -- cca add 2>&1); rc=$?
mv "$T/rk.bak" "$H/.cca/router_ed25519"
ok "提问会话：添加账号时还没配置路由器：接着在表单里配置（说明为什么要配置），取消时什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = router.basic ] && evq "e.get(\"q\") == \"router.basic\" and \"官方账号要经路由器\" in e[\"message\"]" && [ "$(evn terminal)" = 0 ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\""'
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
# 配置本机 Clash（local.verge；没装 Clash Verge 时的 local.client / local.manual 在 11b4）
out=$(sess '{"local.verge":[{"v":"n"}]}' -- cca local setup 2>&1); rc=$?
ok "提问会话：配置本机 Clash：说明要做什么，答否按取消收场，什么都不改" '[ $rc -ne 0 ] && [ "$(asks)" = local.verge ] && evq "e.get(\"q\") == \"local.verge\" and \"全局扩展脚本\" in e[\"fields\"][0][\"detail\"] and e[\"fields\"][0][\"kind\"] == \"confirm\" and e[\"fields\"][0][\"default\"] == \"y\"" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"cancelled\"" && cmp -s "$H/.cca/config.json" "$T/cfg.bak"'
out=$(sess '{"local.verge":[{"v":"y"}]}' -- cca local setup 2>&1); rc=$?; cp "$T/ev.jsonl" "$GF/prompts_local_verge.jsonl"
ok "提问会话：配置本机 Clash：答是就配好（有结果事件），和终端里一样" '[ $rc -eq 0 ] && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"local_ready\"" && grep -q ">>> cca >>>" "$VS" && grep -qx 7901 "$T/verge/listening"'
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
allowed = {'ask', 'ask_secret', 'arrow_menu', 'gui_ask', 'gui_answer', 'policy_guide', 'cmd_edit'}
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
lint_tiers() {   # CODEX_TIERS 和 CODEX_PLANS 的键一一对应（OpenAI 加了新套餐，叫法和档位要一起补）
	python3 -I - "$CCA" <<'PY'
import ast, re, sys
s = open(sys.argv[1], encoding='utf-8').read()
g = lambda n: set(ast.literal_eval(re.search(r'^%s = (\{.*?\})\n' % n, s, re.S | re.M).group(1)))
print('ok' if g('CODEX_PLANS') == g('CODEX_TIERS') else '：对不上 %s' % sorted(g('CODEX_PLANS') ^ g('CODEX_TIERS')))
PY
}
lt=$(lint_tiers)
ok "套餐档位：CODEX_TIERS 和 CODEX_PLANS 的套餐一一对应${lt#ok}" '[ "$lt" = ok ]'
lint_imports() {   # PY_SRC 顶上只 import 每次都要用的模块（每次 py 都是新进程，少 import 一个就省一点启动时间），别的在用到的函数里 import：
	# 函数里用到的标准库模块，顶上、这个函数和外层函数里都没 import（也没有同名的变量），运行到那里就是 NameError
	python3 -I - "$CCA" <<'PY'
import ast, sys
s = open(sys.argv[1], encoding='utf-8').read()
head = "PY_SRC=$(cat <<'PY'\n"
a = s.index(head) + len(head)
tree = ast.parse(s[a:s.index('\nPY\n)', a)])
mods = getattr(sys, 'stdlib_module_names', None) or {'base64', 'filecmp', 'glob', 'plistlib', 'shutil', 'socket', 'sqlite3', 'struct', 'subprocess', 'tempfile', 'threading', 'types', 'unicodedata', 'uuid', 'zlib'}
def bound(node):
    out = set()
    for n in ast.walk(node):
        if isinstance(n, (ast.Import, ast.ImportFrom)):
            out |= {(x.asname or x.name).split('.')[0] for x in n.names}
        elif isinstance(n, ast.Name) and not isinstance(n.ctx, ast.Load):
            out.add(n.id)
        elif isinstance(n, ast.arg):
            out.add(n.arg)
        elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            out.add(n.name)
        elif isinstance(n, ast.ExceptHandler) and n.name:
            out.add(n.name)
    return out
top = set()
for n in tree.body:
    top |= {n.name} if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) else bound(n)
bad = set()
def walk(node, outer):
    for child in ast.iter_child_nodes(node):
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            names = outer | bound(child) | top
            bad.update('%s 用了 %s' % (getattr(child, 'name', 'lambda'), n.id) for n in ast.walk(child)
                       if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Load) and n.id in mods and n.id not in names)
            walk(child, names)
        else:
            walk(child, outer)
walk(tree, set())
print('ok' if not bad else '：' + '、'.join(sorted(bad)))
PY
}
li=$(lint_imports)
ok "lint：PY_SRC 的函数里用到的标准库模块都 import 过（顶上只 import 每次都用的几个）${li#ok}" '[ "$li" = ok ]'
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
else
	echo "SKIP  App 的契约测试（没有 swiftc 或 macos/test.sh）"
fi

# 11. 文字和编码
out=$(run cca router sync 2>&1); m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "UTF-8 下中文提示完整" 'printf "%s" "$out" | grep -q "正在同步账号到路由器 192.0.2.10" && printf "%s" "$m" | grep -q "继续使用 alice@example.com" && printf "%s%s" "$out" "$m" | python3 -I -c "import sys; sys.stdin.buffer.read().decode(\"utf-8\")"'
lint_dollar() {   # $变量 紧跟非 ASCII 字符：macOS 的 bash 3.2 在 UTF-8 下会把多字节字符的首字节吞进变量名，要写 ${变量}。
	# 查发出去的 bash：cca、install.sh、release.sh、policy.sh、macos 下的脚本，还有 App 里拼出来的 bash（Swift 源码，比如终端交接脚本）
	python3 -I - "$D/.." "$CCA" install.sh release.sh policy.sh 'macos/*.sh' macos/Sources macos/tools <<'PY'
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
ok "lint：cca、install.sh、release.sh、policy.sh、macos 的脚本和 App 里拼的 bash 都没有 \$变量 紧跟非 ASCII 字符${ld#ok}" '[ "$ld" = ok ]'
out=$(run cca router status 2>&1)
ok "router status：列出槽位、端口和选中节点" 'printf "%s" "$out" | grep -q "槽位 CCA-1 :7901 监听中 → US-A" && printf "%s" "$out" | grep -q "槽位 CCA-2 :7902 监听中 → REJECT"'

# 11b. 自动更新：假的 GitHub Releases + 测试用发布密钥
G="$T/gh"; mkdir -p "$G"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$T/relkey"; ssh-keygen -q -t ed25519 -N '' -f "$T/evilkey"
TESTPUB=$(cut -d' ' -f1,2 "$T/relkey.pub")
mkver() {   # mkver 版本 输出文件 [路由器脚本标记]：换成测试密钥、改版本号，可在路由器脚本里加个标记
	# 内置代理的内核换成假的 mihomo（setup.sh 打包、算好的校验值）
	sed -e "s|^VERSION=\".*\"$|VERSION=\"$1\"|" -e "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" \
		-e "s|^CORE_SHA256_ARM64=\".*\"$|CORE_SHA256_ARM64=\"$(cat "$T/mihomo/sha256")\"|" -e "s|^CORE_SHA256_AMD64=\".*\"$|CORE_SHA256_AMD64=\"$(cat "$T/mihomo/sha256")\"|" "$CCA" > "$2"
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
out=$(run cca gui remove 2>&1); rc=$?
ok "cca gui remove：删掉 App 和下载目录" '[ $rc -eq 0 ] && [ ! -e "$GA" ] && [ ! -e "$H/.cca/gui" ]'
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

# 11b2. 维护者的账号 → 节点映射：私有仓库（假 git）+ 这台机器的 deploy key + 发布密钥签名
# （这时装着的是 mkver 换成测试密钥的版本，映射用 $T/relkey 签）
PR="$T/policy_repo"; mkdir -p "$PR"; PK="$H/.cca/policy/id_ed25519"
pol() {   # pol JSON [签名密钥]：写进假的私有映射仓库并签名
	printf '%s\n' "$1" > "$PR/policy.json"; rm -f "$PR/policy.json.sig"
	ssh-keygen -Y sign -q -n cca-policy -f "${2:-$T/relkey}" "$PR/policy.json" 2>/dev/null
}
pol '{"version": 1, "claude": {"alice@example.com": "US-C"}}'
# 终端里启动账号时，还没配置映射就引导（默认仓库用 CCA_POLICY_DEFAULT 换成假的；平时的用例里它是空的，不引导）
P=CCA_POLICY_DEFAULT=example/cca-policy
out=$(run $P cca @alice --version 2>&1); rc=$?
ok "还没配置映射、非交互启动（启动器、脚本）：不引导，照常启动" '[ $rc -eq 3 ] && ! printf "%s" "$out" | grep -q "还没配置维护者的账号"'
out=$(run $P CCA_TTY="$(ans 2)" cca @alice --version 2>&1); rc=$?
ok "还没配置映射、终端里启动：先引导（选「这次先跳过」），照常启动，什么都不改" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "这台机器还没配置维护者的账号 → 节点映射（example/cca-policy）" && printf "%s" "$out" | grep -q "这次先跳过，下次启动再问" && [ ! -f "$PK" ] && [ "$(cfgv "c.get(\"policy\", \"\")")" = "" ]'
out=$(run $P CCA_TTY="$(ans 3)" cca @alice --version 2>&1); rc=$?
out2=$(run $P CCA_TTY="$(ans)" cca @alice --version 2>&1); out3=$(run $P cca policy 2>&1)
ok "选「不用映射」：记下来，以后启动不再问；cca policy 里说明" '[ $rc -eq 3 ] && [ "$(cfgv "c.get(\"policy_declined\")")" = True ] && ! printf "%s" "$out2" | grep -q "还没配置维护者的账号" && printf "%s" "$out3" | grep -q "选过不用，启动时不再问"'
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy_declined'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
printf 2 > "$T/policy_denied_n"; : > git.log; rm -f "$T/clipboard"
out=$(run $P CCA_TTY="$(ans 1 '' '')" cca @alice --version 2>&1); rc=$?
ok "选「现在配置」：生成 deploy key，显示公钥（复制到剪贴板）和维护者要运行的命令；等维护者加好按回车重新拉取，拉到后照常启动" '[ $rc -eq 3 ] && [ -f "$PK" ] && printf "%s" "$out" | grep -qF "$(cat "$PK.pub")" && [ "$(cat "$T/clipboard")" = "$(cat "$PK.pub")" ] && printf "%s" "$out" | grep -q "公钥已复制到剪贴板" && [ "$(printf "%s" "$out" | grep -o "维护者加好之后按回车重新拉取" | wc -l | tr -d " ")" = 2 ] && printf "%s" "$out" | grep -q "已更新映射（Claude 1 个账号" && [ "$(grep -c "^GIT clone" git.log)" = 3 ] && grep -q "\"alice@example.com\": \"US-C\"" "$H/.cca/accounts.json" && [ "$(cfgv "c[\"policy\"]")" = example/cca-policy ]'
printf 1 > "$T/policy_denied_n"
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
rm -rf "$H/.cca/policy"
out=$(run $P CCA_TTY="$(ans 1 s)" cca @alice --version 2>&1); rc=$?
ok "引导里输入 s 先跳过：照常启动，说明之后每天自动拉" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "先跳过：维护者加上公钥之后，cca 每天会自动拉一次映射" && [ "$(cfgv "c[\"policy\"]")" = example/cca-policy ] && [ ! -f "$H/.cca/policy/policy.json" ]'
# 还原成没配置映射（账号表改回 US-A），接着测 cca policy setup
rm -f "$T/policy_denied_n"
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy'); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
rm -rf "$H/.cca/policy"; edit_table 'j["alice@example.com"] = "US-A"'; run cca router sync >/dev/null 2>&1
touch "$T/policy_denied"; : > git.log
out=$(run $P CCA_TTY="$(ans)" cca policy setup 2>&1); rc=$?
ok "cca policy setup（不写仓库就是维护者的默认仓库）：给这台机器生成 deploy key，显示公钥和维护者要运行的命令；公钥还没加上时说明之后会自动拉" '[ $rc -eq 0 ] && [ -f "$PK" ] && printf "%s" "$out" | grep -qF "$(cat "$PK.pub")" && printf "%s" "$out" | grep -q "说一声是谁的哪台机器" && printf "%s" "$out" | grep -q "维护者：复制这一行，在 cca 的仓库里运行 ./policy.sh key add（读剪贴板里的公钥，提示起个别名）" &&printf "%s" "$out" | grep -q "GitHub 不认这台机器的 deploy key" && printf "%s" "$out" | grep -q "cca 每天会自动拉一次映射" && [ ! -f "$H/.cca/policy/policy.json" ] && [ "$(cfgv "c[\"policy\"]")" = example/cca-policy ]'
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

# 11b2b. 维护者参考配置里的节点：./policy.sh 从维护者的 custom.yaml 抽出策略组 OpenAi 和它引用的节点（nodes.json，命名空间
#        cca-nodes 签名），各台机器拉映射时一起拉；缺的直接写进路由器（OpenClash 选用的配置、ShellCrash 的订阅配置）和本机
#        Clash Verge 当前选用的配置文件（行尾带 # cca），名字补进同名策略组。这一节结束时路由器、账号表、Clash Verge 都还原
rm -rf "$T/router.snap"; cp -R "$R" "$T/router.snap"; cp "$H/.cca/config.json" "$T/config.snap"; cp "$H/.cca/accounts.json" "$T/accounts.snap"
cp "$T/verge/base.yaml" "$T/verge/base.snap"; cp "$V/profiles.yaml" "$T/verge/profiles.snap"
nodes_pub() {   # nodes_pub [签名密钥]：$PR/nodes.json 用发布密钥签名（参考节点和映射用不同的命名空间）
	rm -f "$PR/nodes.json.sig"; ssh-keygen -Y sign -q -n cca-nodes -f "${1:-$T/relkey}" "$PR/nodes.json" 2>/dev/null
}
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
ok "cca router nodes 列得出写进去的节点；cca router status、cca policy 说明参考节点" 'printf "%s" "$out" | grep -qx AI-A && printf "%s" "$out" | grep -qx AI-B && printf "%s\n" "$out2" | grep -qx "参考节点：维护者参考配置里策略组 OpenAi 的 4 个节点，OpenClash 的配置文件里都有" && printf "%s" "$out3" | grep -q "参考节点：维护者参考配置里策略组 OpenAi 的 4 个节点（路由器和本机 Clash Verge 的配置文件里缺的直接写进去）"'
cp "$SRC" "$T/src.filled"; c0=$(grep -c '^CORE' router.log); r0=$(restarts); n0=$(reloads)
out=$(run cca router sync 2>&1); rc=$?
ok "再同步一次：配置文件里都有了，不再写、不重启也不重载，也不再拿内核校验" '[ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.filled" && [ "$(restarts)" = "$r0" ] && [ "$(reloads)" = "$n0" ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && ! printf "%s" "$out" | grep -q "写进 OpenClash 的配置文件"'
one_line "$SRC" AI-A '{name: "AI-A", type: "ss", server: "192.0.2.21", port: 8388, cipher: "aes-128-gcm", password: "x"}' '  '
out=$(run cca router sync 2>&1); rc=$?
ok "2.8.0-beta.5～7 写成一行的参考节点：同步时改成和别的节点一样的块写法（内容没变：不重启、不重载、不再拿内核校验）" '[ $rc -eq 0 ] && cmp -s "$SRC" "$T/src.filled" && [ "$(restarts)" = "$r0" ] && [ "$(reloads)" = "$n0" ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && [ ! -e "$R/etc/cca/fill.pending" ]'
edit_table 'j["nodes"].append("AI-A")' "$H/.cca/config.json"; edit_table 'j["ai@example.com"] = "AI-A"'
out=$(run cca router sync -y 2>&1); rc=$?; s=$(( $(cfgv 'c["ports"]["ai@example.com"]') - 7900 ))
ok "写进去的节点可以当账号的出口：选进可选节点、账号用它，端口切到 AI-A" '[ $rc -eq 0 ] && [ "$(sel "CCA-$s")" = AI-A ]'
c0=$(grep -c '^CORE' router.log); r0=$(restarts); oc_restart
ok "OpenClash 重启：配置文件里本来就有，钩子不用再写（也不拿内核试），账号的端口照常走 AI-A" '[ "$(restarts)" = $((r0 + 1)) ] && [ "$(grep -c "^CORE" router.log)" = "$c0" ] && rget "$RC" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$s")" = AI-A ] && cmp -s "$SRC" "$T/src.filled"'
cp "$T/src.fill" "$SRC"; r0=$(restarts); oc_restart
out=$(run cca router sync 2>&1); rc=$?
ok "订阅更新把配置文件整个换掉了：OpenClash 启动时钩子写回去，这次生成的运行配置里也照样带上，不用为这个再重启一次" '[ "$(added "$T/src.fill" "$SRC")" = 5 ] && rget "$RC" "px.include?(\"AI-A\") && px.include?(\"AI-B\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$s")" = AI-A ] && [ ! -e "$R/etc/cca/fill.pending" ] && grep -q "【cca】已把维护者参考配置里的节点写进配置文件 $SRC" "$R/tmp/openclash.log" && [ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ]'
echo 192.0.2.22 > "$R/core_reject"; rm -f "$R/etc/cca/fill.ok"; cp "$T/src.fill" "$SRC"; oc_restart
ok "这台路由器的内核不认参考配置里的节点（还没校验过的这一版）：启动时钩子先拿内核试，不通过就不写（OpenClash 照常起来，端口池照常），用它的账号宁可断开" 'cmp -s "$SRC" "$T/src.fill" && ! rget "$RC" "px.include?(\"AI-A\")" && [ "$(nl_)" -gt 0 ] && [ "$(sel "CCA-$s")" = REJECT ] && grep -q "维护者参考配置里的节点没通过 OpenClash 内核的校验，没有写进配置文件" "$R/tmp/cca-router.log" && [ ! -e "$R/etc/cca/fill.ok" ] && [ -z "$(fill_tmp)" ]'
c0=$(grep -c '^CORE' router.log); out=$(run cca router status 2>&1); oc_restart
ok "没通过校验的记下了：再启动不再拿内核试；cca router status 说还缺什么、为什么" '[ "$(grep -c "^CORE" router.log)" = "$c0" ] && printf "%s" "$out" | grep -q "OpenClash 的配置文件里还要补上节点 AI-A、AI-B；策略组 OpenAi 里加上 AI-A、AI-B、US-B（这一版还没在这台路由器的内核上校验过）" && printf "%s" "$out" | grep -q "上次没写成：维护者参考配置里的节点没通过 OpenClash 内核的校验"'
out=$(run cca router sync -y 2>&1); rc=$?
ok "同步时照样再校验一次，内核还是不认：说明没写，配置文件不动，用它的账号断开" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "维护者参考配置里的节点没通过 OpenClash 内核的校验，没有写进配置文件" && cmp -s "$SRC" "$T/src.fill" && [ "$(sel "CCA-$s")" = REJECT ] && [ ! -e "$R/etc/cca/fill.ok" ]'
rm -f "$R/core_reject"; r0=$(restarts); n0=$(reloads)
out=$(run cca router sync -y 2>&1); rc=$?
ok "内核认了（比如升级了）：同步时写进配置文件，重启一次用上，账号的端口回到 AI-A" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(reloads)" = "$n0" ] && [ "$(added "$T/src.fill" "$SRC")" = 5 ] && rget "$RC" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$s")" = AI-A ] && [ -s "$R/etc/cca/fill.ok" ]'
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
ok "在家启动账号：必要重启只问一次；拒绝后即使节点同名也阻断，不调用客户端" '[ $rc -eq 1 ] && [ "$(printf "%s" "$out" | grep -c "现在重启 OpenClash 吗")" = 1 ] && [ "$(restarts)" = "$r0" ] && [ "$(sel "CCA-$s")" = AI-A ] && printf "%s" "$out" | grep -q "路由器账号节点未能核实" && [ "$(grep -c "^ARGS:" claude.log)" = "$n_cli" ]'
out=$(run CCA_TTY="$(ans y)" cca router sync 2>&1); rc=$?
ok "再同步时同意重启：用上配置文件里新写的" '[ $rc -eq 0 ] && [ "$(restarts)" = $((r0 + 1)) ] && rget "$RC" "c[\"proxies\"].find { |p| p[\"name\"] == \"AI-A\" }[\"password\"] == \"y2\"" && [ ! -e "$R/etc/cca/fill.pending" ] && [ "$(sel "CCA-$s")" = AI-A ]'
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
ok "ShellCrash：缺的写进订阅配置、名字补进 OpenAi 组，问过后重启一次（不热加载）；和它的自定义节点重名的 SC-X 不写（说明原因，只有一个 SC-X）" '[ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = $((r0 + 1)) ] && [ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 5 ] && rget "$SCR" "px.include?(\"AI-A\") && px.include?(\"AI-B\") && px.count(\"SC-X\") == 1 && g == %w[US-A AI-A AI-B US-B]" && printf "%s" "$out" | grep -q "参考节点「SC-X」和这里的策略组（或内置出站、ShellCrash 的自定义节点）重名，没有写" && printf "%s" "$out" | grep -q "现在重启 ShellCrash 吗" && [ "$(sel "CCA-$s")" = AI-A ] && ! grep -q SCBAD router.log'
out=$(run cca router sync 2>&1); rc=$?
out2=$(run cca router nodes 2>&1)
ok "ShellCrash 再同步：配置文件里都有了，不用重启也不用重载；cca router nodes 列得出参考节点，不列自定义节点" '[ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = $((r0 + 1)) ] && printf "%s" "$out2" | grep -qx AI-A && ! printf "%s" "$out2" | grep -q SC-X'
cp "$T/sc_base.fill" "$SCD/yamls/config.yaml"; r0=$(restarts); sc_restart
ok "ShellCrash 的订阅更新把订阅配置换掉了：启动时钩子在它拼运行配置之前写回去，这次启动就用上" '[ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 5 ] && rget "$SCR" "px.include?(\"AI-A\") && g == %w[US-A AI-A AI-B US-B]" && [ "$(sel "CCA-$s")" = AI-A ] && [ ! -e "$R/etc/cca/fill.pending" ] && ! grep -q SCBAD router.log'
python3 -c "import sys; p=sys.argv[1]; s=open(p).read(); b='  - name: OpenAi\n    type: select\n    proxies:\n      - US-A\n'; assert s.count(b) == 1; open(p, 'w').write(s.replace(b, '  - {name: OpenAi, type: select, proxies: [US-A]}\n'))" "$T/sc_base.fill"
cp "$T/sc_base.fill" "$SCD/yamls/config.yaml"; sc_restart; n0=$(reloads); r0=$(restarts)
out=$(run cca router sync 2>&1); rc=$?
ok "订阅配置里 OpenAi 组写成一行的：只写节点、组不动（说明为什么）；同步时不用再重载、重启" 'grep -q "策略组 OpenAi 不是一行一项的写法" "$R/tmp/cca-router.log" && [ "$(added "$T/sc_base.fill" "$SCD/yamls/config.yaml")" = 2 ] && rget "$SCR" "px.include?(\"AI-A\") && g == %w[US-A]" && ! grep -q SCBAD router.log && [ $rc -eq 0 ] && [ "$(reloads)" = "$n0" ] && [ "$(restarts)" = "$r0" ]'
rm -rf "$R"; cp -R "$T/router.snap" "$R"
# 本机 Clash Verge：当前选用的是本地配置（Verge 里也有 OpenAi 组）；在外面启动账号时写进这份配置，热加载一次。
# Codex 的 dave 同时需要 US-D：功能用例先备齐全池节点，不能靠忽略另一个账号的切换失败来通过。
awk '/^proxy-groups:$/ { print "- {name: US-D, type: ss, server: 192.0.2.4, port: 8388, cipher: aes-128-gcm, password: x}" } /^rules:$/ { print "- name: OpenAi"; print "  type: select"; print "  proxies:"; print "  - US-A" } { print }' "$T/verge/base.snap" > "$V/profiles/Lprofile1.yaml"
cp "$V/profiles/Lprofile1.yaml" "$T/verge/prof.orig"
printf '%s\n' '- uid: Lprofile1' '  type: local' '  name: 测试配置' '  file: Lprofile1.yaml' >> "$V/profiles.yaml"
"$T/verge_regen"; touch "$T/away"; n0=$(vreloads)
out=$(run CCA_TTY="$(ans)" cca @ai --version 2>&1); rc=$?
ok "在外面：参考节点写进 Clash Verge 当前选用的配置（行尾带 # cca，原来的留一份），热加载一次，走 AI-A 的账号照常启动" '[ $rc -eq 3 ] && [ "$(vreloads)" = $((n0 + 1)) ] && [ "$(vsel "CCA-$s")" = AI-A ] && printf "%s" "$out" | grep -q "已把维护者参考配置里的节点写进 Clash Verge 当前的配置「测试配置」：补上节点 AI-A、AI-B、SC-X；策略组 OpenAi 里加上 AI-A、AI-B、US-B、SC-X（行尾带 # cca；改之前的在 ~/.cca/backup/clash-verge-Lprofile1.yaml）" && [ "$(added "$T/verge/prof.orig" "$V/profiles/Lprofile1.yaml")" = 7 ] && cmp -s "$H/.cca/backup/clash-verge-Lprofile1.yaml" "$T/verge/prof.orig" && rget "$V/profiles/Lprofile1.yaml" "px.include?(\"AI-A\") && !px.include?(\"AI-B2\") && px.count(\"US-A\") == 1 && g == %w[US-A AI-A AI-B US-B SC-X]"'
ok "热加载的运行配置照写好的配置补上：节点一个不多、名字接在 OpenAi 组后面；全局扩展脚本里不带参考节点" 'ruby -ryaml -rjson -e "c = YAML.safe_load(JSON.parse(File.read(ARGV[0]))[\"payload\"]); px = c[\"proxies\"].map { |p| p[\"name\"] }; g = c[\"proxy-groups\"].find { |x| x[\"name\"] == \"OpenAi\" }[\"proxies\"]; exit(px.count(\"AI-A\") == 1 && px.include?(\"AI-B\") && !px.include?(\"AI-B2\") && px.count(\"US-A\") == 1 && px.count(\"SC-X\") == 1 && g == %w[US-A AI-A AI-B US-B SC-X] ? 0 : 1)" "$T/verge/last_put.json" && grep -q ">>> cca >>>" "$VS" && ! grep -q "ccaFill\|192\.0\.2\.2[12]" "$VS"'
"$T/verge_regen"
ok "Clash Verge 重新生成配置（真的 JavaScript 引擎跑全局扩展脚本）：参考节点来自它的配置文件，和热加载的一样" 'ruby -rjson -e "c = JSON.parse(File.read(ARGV[0])); px = c[\"proxies\"].map { |p| p[\"name\"] }; g = c[\"proxy-groups\"].find { |x| x[\"name\"] == \"OpenAi\" }[\"proxies\"]; exit(px.count(\"AI-A\") == 1 && px.include?(\"AI-B\") && px.count(\"SC-X\") == 1 && g == %w[US-A AI-A AI-B US-B SC-X] && c[\"proxy-groups\"].find { |x| x[\"name\"] == \"CCA-$s\" }[\"proxies\"][0] == \"AI-A\" ? 0 : 1)" "$T/verge/regen.json" && [ "$(vsel "CCA-$s")" = AI-A ]'
n0=$(vreloads); out=$(run CCA_TTY="$(ans)" cca @ai --version 2>&1); rc=$?
ok "再在外面启动：配置里都有了，不再写、也不再热加载" '[ $rc -eq 3 ] && [ "$(vreloads)" = "$n0" ] && ! printf "%s" "$out" | grep -q "写进 Clash Verge" && [ "$(vsel "CCA-$s")" = AI-A ]'
# 模拟运行中的内核丢了另一账号组的目标成员；当前 ai 仍正确也不能吞掉全池失败。
dave_group="CCA-$(( $(cfgv 'c["codex"]["ports"]["dave@example.com"]') - 7900 ))"
cp "$T/verge/groups/$dave_group" "$T/verge/dave.group"
grep -vx US-D "$T/verge/dave.group" > "$T/verge/groups/$dave_group"; printf US-C > "$T/verge/sel/$dave_group"
n_cli=$(grep -c '^ARGS:' claude.log); out=$(runev cca @ai --version 2>&1); rc=$?
ok "本机全池门禁：其他账号的节点不可选时置 REJECT；当前账号即使节点正确也拒绝启动" '[ $rc -eq 1 ] && [ "$(vsel "CCA-$s")" = AI-A ] && [ "$(vsel "$dave_group")" = REJECT ] && printf "%s" "$out" | grep -q "${dave_group} 切换到 US-D 后未能核实" && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"local_unavailable\"" && [ "$(grep -c "^ARGS:" claude.log)" = "$n_cli" ] && [ "$(vreloads)" = "$n0" ]'
cp "$T/verge/dave.group" "$T/verge/groups/$dave_group"
out=$(run cca @ai --version 2>&1); rc=$?
ok "其他账号的节点恢复后：全池重新核验通过，当前账号才恢复启动，无需热加载" '[ $rc -eq 3 ] && [ "$(vsel "$dave_group")" = US-D ] && [ "$(vsel "CCA-$s")" = AI-A ] && [ "$(vreloads)" = "$n0" ]'
cp "$V/profiles/Lprofile1.yaml" "$T/verge/prof.filled"
python3 - "$H/.cca/policy/nodes.json" > "$T/ai_a.flow" <<'PY'
import json, sys
p = [x for x in json.load(open(sys.argv[1]))["proxies"] if x["name"] == "AI-A"][0]
print("{" + ", ".join("%s: %s" % (k, json.dumps(v)) for k, v in p.items()) + "}")
PY
one_line "$V/profiles/Lprofile1.yaml" AI-A "$(cat "$T/ai_a.flow")"
n0=$(vreloads); out=$(run CCA_TTY="$(ans)" cca @ai --version 2>&1); rc=$?
ok "Clash Verge 配置里以前写成一行的参考节点：改成块写法（内容没变，不用热加载）" '[ $rc -eq 3 ] && cmp -s "$V/profiles/Lprofile1.yaml" "$T/verge/prof.filled" && [ "$(vreloads)" = "$n0" ]'
out=$(run CCA_TTY="$(ans y)" cca local setup 2>&1); out2=$(run cca local 2>&1)
ok "cca local setup：参考节点不算 Clash Verge 里没有的；cca local 说参考节点已经写进配置" '! printf "%s" "$out" | grep "Clash Verge 里没有这些节点" | grep -q AI-A && printf "%s" "$out2" | grep -q "参考节点：已写进 Clash Verge 当前的配置「测试配置」"'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); [x.update(port=9443) for x in j['proxies'] if x['name'] == 'AI-B']; json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
run cca policy pull >/dev/null 2>&1; cp "$V/profiles/Lprofile1.yaml" "$T/verge/prof.prev"; n0=$(vreloads)
out=$(run CCA_TTY="$(ans)" cca @ai --version 2>&1); rc=$?
ok "维护者改了节点：Clash Verge 配置里 cca 写的那一行跟着改，热加载的运行配置里也换成新的（不会多出一个）" '[ $rc -eq 3 ] && [ "$(vreloads)" = $((n0 + 1)) ] && printf "%s" "$out" | grep -q "按参考配置更新了节点 AI-B" && [ "$(diff "$T/verge/prof.prev" "$V/profiles/Lprofile1.yaml" | grep -c "^[<>]")" = 2 ] && ruby -ryaml -rjson -e "c = YAML.safe_load(JSON.parse(File.read(ARGV[0]))[\"payload\"]); b = c[\"proxies\"].select { |p| p[\"name\"] == \"AI-B\" }; exit(b.size == 1 && b[0][\"port\"] == 9443 ? 0 : 1)" "$T/verge/last_put.json"'
python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); j['proxies'].append({'name': 'AI-C', 'type': 'ss', 'server': '192.0.2.23', 'port': 8388, 'cipher': 'aes-128-gcm', 'password': 'x'}); json.dump(j, open(p, 'w'), indent=2)" "$PR/nodes.json"; nodes_pub
run cca policy pull >/dev/null 2>&1; echo 192.0.2.23 > "$T/verge/reject_text"; cp "$V/profiles/Lprofile1.yaml" "$T/verge/prof.prev"; n0=$(vreloads)
out=$(run CCA_TTY="$(ans)" cca @ai --version 2>&1); rc=$?
ok "Clash Verge 的内核不认新加的参考节点：配置文件不写，照配置里原有的再热加载一次，账号照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "Clash Verge 的内核不认维护者参考配置里的节点，没有写进它的配置" && grep -q "RELOAD rejected text" "$T/verge/verge.log" && [ "$(vreloads)" = $((n0 + 1)) ] && cmp -s "$T/verge/prof.prev" "$V/profiles/Lprofile1.yaml" && [ "$(vsel "CCA-$s")" = AI-A ] && [ -z "$(ls "$H/.cca" | grep "^local\.fill\.\|^fill\.")" ]'
# 维护者的映射要某个账号走参考节点，这台机器的可选节点里还没有它：自动加进可选节点（记下摘要），账号随后照常加进来
pol '{"version": 1, "claude": {"alice@example.com": "HK-A2", "bea@example.com": "AI-B"}}'
out=$(run cca policy pull 2>&1); rc=$?
ok "映射要的节点是维护者的参考节点、还不在可选节点里：自动加进可选节点（说一声），映射里的账号照常加进来" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "维护者的映射要 bea@example.com 走节点 AI-B：它在维护者的参考配置里（同步时写进路由器的配置），已加进可选节点" && printf "%s" "$out" | grep -q "按维护者的映射添加了 Claude 账号 bea@example.com，出口 AI-B" && cfgok "\"AI-B\" in c[\"nodes\"] and c[\"node_ids\"].get(\"AI-B\")" && tblok "t.get(\"bea@example.com\") == \"AI-B\""'
pol '{"version": 1, "claude": {"alice@example.com": "HK-A2"}}'
# 还原：维护者不再发布参考节点（映射仓库里删掉 nodes.json），拉下来后这台机器上的也删掉
rm -f "$PR/nodes.json" "$PR/nodes.json.sig"; out=$(run cca policy pull 2>&1)
ok "映射仓库里没有 nodes.json 了：这台机器上的参考节点也删掉" '[ ! -e "$H/.cca/policy/nodes.json" ] && ! printf "%s" "$out" | grep -q "参考节点："'
rm -f "$T/away" "$T/verge/reject_text" "$V/profiles/Lprofile1.yaml" "$H/.cca/backup/clash-verge-Lprofile1.yaml"
cp "$T/verge/profiles.snap" "$V/profiles.yaml"; cp "$T/verge/base.snap" "$T/verge/base.yaml"; cp "$T/config.snap" "$H/.cca/config.json"; cp "$T/accounts.snap" "$H/.cca/accounts.json"
run CCA_TTY="$(ans y)" cca local setup >/dev/null 2>&1; "$T/verge_regen"

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
ok "这台机器的公钥还没加到仓库上：拉不到映射；公钥复制到了剪贴板" '[ "$(cat "$T/clipboard")" = "$(cat "$PK.pub")" ] && printf "%s" "$out" | grep -q "GitHub 不认这台机器的 deploy key"'
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
ok "改别名时旧的删掉了、新的没加上（GitHub 出错，试了三次）：说清楚，并给出公钥好加回来" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "旧的「Bob 的 mini」已经删了，可新的没加上：Server Error" && printf "%s" "$out" | grep -qF "$(cut -d" " -f1,2 "$T/bob.pub")" && [ "$(dkt)" = "" ] && [ "$(grep -c "^GH api POST .* title=bob$" ghcli.log)" = 3 ]'
touch "$T/gh_down"; out=$(pks "$(ans)" 2>&1); rc=$?; rm -f "$T/gh_down"
ok "连不上 GitHub：说明原因" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "读不到 example/cca-policy 上的 deploy key：连不上 GitHub（error connecting to api.github.com）"'
defrepo=$(sed -n 's/^POLICY_REPO_DEFAULT="\${CCA_POLICY_DEFAULT-\([^}]*\)}"$/\1/p' "$CCA")
out=$(run $NOPD CCA_TTY="$(ans)" bash "$PS" key 2>&1); rc=$?
out2=$(run $NOPD CCA_TTY="$(ans)" bash "$PS" key -R example/other 2>&1); rc2=$?
: > ghcli.log; env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" "$NOPD" CCA_TTY="$(ans)" PATH="$T/bin:/usr/bin:/bin" bash "$PS" key >/dev/null 2>&1
ok "仓库：CCA_POLICY_DEFAULT 设成空、又没写 -R 时不猜；-R 指定别的仓库；都没设时用 cca 里写的默认仓库" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不知道是哪个映射仓库：加上 -R 维护者/仓库" && [ $rc2 -eq 0 ] && printf "%s" "$out2" | grep -q "example/other 上还没有 deploy key" && [ -n "$defrepo" ] && grep -q "^GH api GET repos/$defrepo/keys" ghcli.log'
rm -f "$T/policy_keys"

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
ok "这台机器没配置映射：不能请求（先 cca policy setup），App 里也没有这个入口" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "还没配置维护者的映射" && printf "%s" "$out2" | jok "not any(x[\"can\"][\"policy_lock\"] for t in d[\"tools\"] for x in t[\"accounts\"])"'
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
ok "少写了参数：说用法，什么都不改" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "用法：./policy.sh lock claude|codex 邮箱 节点" && unchanged'
bad router e@example.com US-A
ok "不认识的工具：不收" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "工具应该是 claude 或 codex" && unchanged'
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

# 11b3c. 设备统计（presence）：维护者在映射里配置了设备统计的服务器（真的 server/cca-presence.py：127.0.0.1 上随机端口、测试证书、
#        测试发布密钥，ttl 3 秒）之后，映射里的账号经 cca 用的时候这台 Mac 在后台起一个上报进程，把开着的会话报给服务器；
#        cca devices 问服务器这个账号连着哪些设备。设备清单由维护者 ./policy.sh presence sync（假 gh 的 deploy key）推给服务器。
#        测试里的会话没有终端：在跑就算在用。这一节结束时停掉服务器，映射、配置、假的映射仓库、假 GitHub 上的 key 都还原
SV="$T/presence-srv"; mkdir -p "$SV"
openssl ecparam -name prime256v1 -genkey -noout -out "$SV/key.pem" 2>/dev/null
openssl req -new -x509 -key "$SV/key.pem" -subj /CN=cca-presence -days 2 -out "$SV/cert.pem" 2>/dev/null
cp "$T/relkey.pub" "$SV/release.pub"
srv_start() {   # srv_start [端口]：起服务器（状态文件留着，重启后接着用）
	rm -f "$SV/port"
	python3 -I "$D/../server/cca-presence.py" --listen "127.0.0.1:${1:-0}" --port-file "$SV/port" --cert "$SV/cert.pem" --key "$SV/key.pem" \
		--release-key "$SV/release.pub" --state "$SV/state.json" --ttl 3 >>"$SV/log" 2>&1 &
	SVP=$!
	local i; for i in $(seq 1 50); do [ -s "$SV/port" ] && return 0; sleep 0.1; done; return 1
}
srv_start; SVPORT=$(cat "$SV/port")
PIN="sha256//$(openssl x509 -in "$SV/cert.pem" -pubkey -noout | openssl pkey -pubin -outform der | openssl dgst -sha256 -binary | base64)"   # 和部署时一样用 openssl 算
PRD="$H/.cca/presence"
cp -p "$H/.cca/config.json" "$T/pres.config.json"; cp -p "$H/.cca/accounts.json" "$T/pres.accounts.json"; cp -p "$T/deploy_keys.json" "$T/pres.keys.json"
cp -p "$PR/policy.json" "$T/pres.pr.json"; cp -p "$PR/policy.json.sig" "$T/pres.pr.sig"; cp -p "$H/.cca/policy/policy.json" "$T/pres.local.json"; cp -p "$MR/policy.json" "$T/pres.mr.json"
withp() { python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); j["presence"]={"url":"https://127.0.0.1:%s" % sys.argv[2],"pins":[sys.argv[3]],"salt":"ab"*16}; print(json.dumps(j))' "$T/pres.pr.json" "$SVPORT" "${1:-$PIN}"; }   # withp [指纹]：现在发布的映射加上 presence 一节
dv() { run cca devices alice@example.com --json 2>/dev/null; }   # 问服务器：alice 连着哪些设备
dstate() { dv | python3 -c 'import json,sys; d=json.load(sys.stdin); print(",".join("%s:%s:%d" % (x["name"], x["state"], x["cli"]) for x in d["accounts"][0]["devices"]))' 2>/dev/null; }
dwait() { local i; for i in $(seq 1 60); do [ "$(dstate)" = "$1" ] && return 0; sleep 0.2; done; return 1; }   # 等服务器上 alice 的设备变成这样
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
ok "映射里的账号开着会话：后台的上报进程马上报给服务器（在用、1 个会话、这台 Mac 排第一）；上报进程只有一个，后台优先级" '[ $rc -eq 0 ] && alive "$a1" && ps -o command= -p "$a1" | grep -q presence-agent && [ "$(ps -o pri= -p "$a1" | tr -d " ")" -le 20 ] && dv | jok "(lambda x: x[\"me\"] is True and x[\"since_epoch\"] and x[\"last_used_epoch\"] and x[\"last_active_epoch\"])(d[\"accounts\"][0][\"devices\"][0])"'
mkdir -p "$T/guifx3"; dv > "$T/guifx3/devices.json"
out=$(run cca devices 2>&1)
ok "cca devices（给人看的）：每个映射里的账号连着哪些设备，这台 Mac 标出来" 'printf "%s" "$out" | grep -q "Claude 账号连着的设备" && printf "%s" "$out" | grep -qF "${ME}（这台 Mac）  在用 · 命令行里 1 个会话"'
touch "$T/hold"; run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS2=$!
waitmark "$AD" 2
dwait "$ME:in_use:2"; rc=$?
ok "又开一个会话：同一个上报进程马上报 2 个（账号目录里的登记一变就醒，不用等心跳）" '[ $rc -eq 0 ] && [ "$(apid)" = "$a1" ]'
m2=$(marks "$AD" | grep -vx "$m1"); t2=$(pgrep -P "$m2")
kill -KILL "$m2"
dwait "$ME:in_use:1"; rc=$?
ok "cca 被 kill -9（来不及删登记）：上报进程看到它退出，马上改报 1 个" '[ $rc -eq 0 ]'
kill -KILL "$a1"; sleep 4
dwait "$ME:offline:1"; rc=$?
ok "上报进程被 kill -9（就像这台 Mac 睡着了、断网了）：服务器 ttl 秒收不到心跳，算离线（最后看到时开着 1 个会话）" '[ $rc -eq 0 ] && dv | jok "d[\"accounts\"][0][\"devices\"][0][\"seen_epoch\"] < d[\"generated_epoch\"] - 3"'
run CCA_TTY="$(ans)" cca @alice --version >/dev/null 2>&1 & SS3=$!
dwait "$ME:in_use:2"; rc=$?; a2=$(apid)
ok "再启动一次：上报进程重新起来，报全部开着的（2 个）" '[ $rc -eq 0 ] && alive "$a2" && [ "$a2" != "$a1" ]'
rm -f "$T/hold"; wait "$SS1" "$SS3" 2>/dev/null; for i in $(seq 1 30); do alive "$t2" || break; sleep 0.1; done
dwait "$ME:none:0"; rc=$?
for i in $(seq 1 30); do alive "$a2" || break; sleep 0.1; done
ok "会话都关了：马上报“没在用”（留着上次使用），上报进程自己退出，这台 Mac 上不再有它" '[ $rc -eq 0 ] && ! alive "$a2" && dv | jok "d[\"accounts\"][0][\"devices\"][0][\"last_used_epoch\"] is not None"'
ok "上报不经账号的出口：直连（钉住证书公钥），不经路由器和本机 Clash" '! grep -q "127.0.0.1:$SVPORT" curl.log 2>/dev/null'
if command -v swiftc >/dev/null 2>&1 && [ -f "$D/../macos/test.sh" ]; then
	ok "App 的契约测试：Swift 模型解码真实的 cca devices --json（macos/test.sh）" 'bash "$D/../macos/test.sh" "$T/guifx3" > "$T/contract3.log" 2>&1'
fi
AK=$(python3 -c 'import json,sys; print(next(k for k, v in json.load(open(sys.argv[1])).items() if isinstance(v, dict)))' "$H/.cca/accounts.json")
out=$(runev cca devices "$AK" --json); rc=$?
ok "不在映射里的账号（第三方 API 账号）：不统计，fail 事件 devices_off" '[ $rc -ne 0 ] && [ -z "$out" ] && evq "e[\"ev\"] == \"fail\" and e[\"code\"] == \"devices_off\" and \"不在维护者的映射里\" in e[\"message\"]"'
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
kill "$SVP" 2>/dev/null; wait "$SVP" 2>/dev/null; SVP=""
cp -p "$T/pres.config.json" "$H/.cca/config.json"; cp -p "$T/pres.accounts.json" "$H/.cca/accounts.json"; cp -p "$T/pres.keys.json" "$T/deploy_keys.json"
cp -p "$T/pres.pr.json" "$PR/policy.json"; cp -p "$T/pres.pr.sig" "$PR/policy.json.sig"; cp -p "$T/pres.local.json" "$H/.cca/policy/policy.json"
cp -p "$T/pres.mr.json" "$MR/policy.json"; rm -rf "$PRD"

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
mv "$V" "$V.off"; touch "$T/verge/down"   # 这台 Mac 上没有 Clash 客户端了
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c.pop('policy', None); json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
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
touch "$T/away"; : > ssh.log; : > open.log; : > gh.log; : > curl.log
out=$(runev cca @alice --version 2>&1); rc=$?
ok "在外面、没有 Clash 客户端、非交互（界面 App）：记一条 consent（builtin_core），不下载、不启动" '[ $rc -ne 0 ] && evq "e[\"ev\"] == \"consent\" and e[\"item\"] == \"builtin_core\" and e[\"grant\"] == \"builtin_core\"" && ! grep -q "GH mihomo" gh.log && ! cfgok "c[\"local\"].get(\"client\")"'
out=$(run cca @alice --version 2>&1); rc=$?
ok "在外面、没有 Clash 客户端、非交互（Dock 上的启动器、脚本）：说明可以用内置代理，交给终端（cca local setup），不启动" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没装的话可以用 cca 内置的代理）：在终端里运行 cca local setup 配置一次" && ! cfgok "c[\"local\"].get(\"client\")" && ! grep -q "GH mihomo" gh.log'
out=$(runev CCA_GRANT=builtin_core cca @alice --version 2>&1); rc=$?
ok "界面 App 里同意之后（带着授权重跑）：改用内置代理：下载内核（按 SHA256 校验），在后台打开界面 App（--core），App 起内核，账号照常启动" '[ $rc -eq 3 ] && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ -x "$CN/mihomo-v1.19.33" ] && grep -q "^GH mihomo mihomo-darwin-.*-v1.19.33.gz$" gh.log && grep -qF "OPEN [-g] [$GA] [--args] [--core]" open.log && grep -qx START "$CD/core.log" && [ "$(cat "$CD/home")" = "$CN" ] && [ "$(cat "$CN/mihomo.pid")" = "$(cat "$CD/pid")" ] && [ -z "$(ls "$CN" | grep "^dl\.")" ]'
ok "在外面：经内置代理的 7901 出去（CCA-1 选中 US-A），Claude Code 拿到的代理也是它；不连路由器" '[ "$(csel CCA-1)" = US-A ] && grep -q "CURL via local:7901 https://claude.ai/cdn-cgi/trace" curl.log && lastenv | grep -q "https_proxy=http://127.0.0.1:7901" && [ ! -s ssh.log ]'
ok "内置代理的配置（600）：只有 127.0.0.1 上的入站（各自直通 CCA-n）、可选节点、一条 MATCH,REJECT；不开通用的代理端口、TUN、DNS；控制接口只开 Unix socket，不记选中的节点" '[ "$(stat -f %Lp "$CN/config.yaml")" = 600 ] && ccfg "all(l[\"listen\"] == \"127.0.0.1\" and l[\"proxy\"] == \"CCA-%d\" % (l[\"port\"] - 7900) for l in c[\"listeners\"]) and c[\"rules\"] == [\"MATCH,REJECT\"] and not any(k in c for k in (\"port\", \"mixed-port\", \"socks-port\", \"tun\", \"dns\", \"external-controller\")) and c[\"external-controller-unix\"].endswith(\".sock\") and c[\"profile\"][\"store-selected\"] is False and [p[\"name\"] for p in c[\"proxies\"]] == [\"US-A\", \"US-B\", \"US-C\", \"US-D\", \"HK-A2\"] and c[\"proxy-groups\"][0][\"proxies\"][0] == \"US-A\""'
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
# 维护者的参考节点补上路由器那份里没有的（和往路由器、Clash Verge 里补的同一套规则：同名的以路由器的为准）
printf '%s\n' '{"version": 1, "group": "OpenAi", "proxies": [{"name": "US-D", "type": "ss", "server": "192.0.2.4", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}, {"name": "US-A", "type": "ss", "server": "192.0.2.99", "port": 8388, "cipher": "aes-128-gcm", "password": "y"}, {"name": "AI-Z", "type": "ss", "server": "192.0.2.29", "port": 8388, "cipher": "aes-128-gcm", "password": "x"}]}' > "$H/.cca/policy/nodes.json"
out=$(run cca @alice --version 2>&1); rc=$?
ok "维护者的参考节点补上缺的 US-D：重新加载一次，照常启动；同名的 US-A 以路由器的为准，不是可选节点的 AI-Z 不放" '[ $rc -eq 3 ] && [ "$(csel CCA-1)" = US-D ] && [ "$(creloads)" = $((n0 + 2)) ] && ccfg "[p[\"name\"] for p in c[\"proxies\"]] == [\"US-A\", \"US-B\", \"US-C\", \"HK-A2\", \"US-D\"] and c[\"proxies\"][0][\"server\"] == \"192.0.2.1\""'
rm -f "$H/.cca/policy/nodes.json"; edit_table 'j["alice@example.com"] = "US-A"'
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
ok "_gui state：本机客户端是内置代理（名字由 cca 给），已经同步" 'printf "%s" "$out" | jok "d[\"config\"][\"local\"] == {\"configured\": True, \"client\": \"builtin\", \"name\": \"内置代理\"} and d[\"routing\"][\"local_sync\"] == \"ok\""'
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
# 终端里（交互）在外面、还没配置本机客户端：照 cca local setup 的流程问（没装 Clash Verge 时默认内置代理，选 n 手动配置别的客户端）
python3 -c "import json,sys; p=sys.argv[1]; c=json.load(open(p)); c['local'] = {}; json.dump(c, open(p, 'w'), indent=2, ensure_ascii=False)" "$H/.cca/config.json"
out=$(run CCA_TTY="$(ans y)" cca @alice --version 2>&1); rc=$?
ok "终端里在外面、还没配置本机客户端：照 cca local setup 问（没装 Clash Verge 时默认用内置代理），答 y 照常启动" '[ $rc -eq 3 ] && printf "%s" "$out" | grep -q "用内置代理吗" && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ "$(csel CCA-1)" = US-A ]'
# 关掉、在家重新配置
run cca local off >/dev/null 2>&1
ok "cca local off：关掉内置代理（记下关掉过），节点定义和配置删掉，停掉内核（界面 App 没开着时）；内核文件留着" 'cfgok "c[\"local\"] == {\"core_off\": \"1\"}" && [ ! -e "$CN/nodes.json" ] && [ ! -e "$CN/config.yaml" ] && [ ! -f "$CD/up" ] && [ -x "$CN/mihomo-v1.19.33" ]'
out=$(run CCA_GUI_OS=15 cca @alice --version 2>&1); rc=$?
ok "macOS 太旧（内置代理在界面 App 里运行，要 macOS 26）：说明要装 Clash 客户端" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "内置代理要 macOS 26 或更新"'
rm -f "$T/away"; : > ssh.log
run cca @alice --version >/dev/null 2>&1; sleep 1
ok "关掉过内置代理：在家不再拷节点定义" '! grep -q "sh -s defs" ssh.log && [ ! -e "$CN/nodes.json" ]'
out=$(run CCA_TTY="$(ans y)" cca local setup 2>&1)
ok "cca local setup：没装 Clash Verge 时默认用内置代理；在家先从路由器拷节点定义，然后让内核就绪" 'printf "%s" "$out" | grep -q "用内置代理吗" && printf "%s" "$out" | grep -q "本机端口池已就绪：内置代理 127.0.0.1:7901" && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && [ "$(cnames "$CN/nodes.json")" = US-A,US-B,US-C,US-D,HK-A2 ] && [ -f "$CD/up" ]'
rm -f "$CN"/mihomo-v*; : > gh.log
run cca @alice --version >/dev/null 2>&1
for i in $(seq 1 50); do [ -x "$CN/mihomo-v1.19.33" ] && [ ! -d "$H/.cca/core-nodes.lock" ] && break; sleep 0.1; done
ok "已经用内置代理、内核文件不在（比如 cca 升级换了内核版本）：在家启动后在后台先下好（在外面直连 GitHub 可能很慢）" '[ -x "$CN/mihomo-v1.19.33" ] && grep -q "^GH mihomo" gh.log'
# 配置本机 Clash 的表单（没装 Clash Verge）：内置代理 / 手动配置别的客户端
run cca local off >/dev/null 2>&1; core_stop
: > env.log
out=$(sess '{"local.client":[{"v":"2"}],"local.manual":[{"ctl":"不是地址","secret":""},{"ctl":"unix:/tmp/cca-none.sock","secret":"CLASH-SECRET-SENTINEL"}]}' -- cca local setup 2>&1); rc=$?
ok "提问会话：配置本机 Clash：没装 Clash Verge 时选内置代理或手动配置；手动配置的一页有要加的配置（只显示，不用答）、外部控制地址、密钥" '[ "$(asks)" = "local.client local.manual local.manual local.manual" ] && evq "e.get(\"q\") == \"local.client\" and e[\"fields\"][0][\"kind\"] == \"choice\" and e[\"fields\"][0][\"default\"] == \"1\" and len(e[\"fields\"][0][\"options\"]) == 2" && evq "e.get(\"seq\") == 2 and e[\"fields\"][0][\"kind\"] == \"code\" and \"listeners:\" in e[\"fields\"][0][\"value\"] and \"proxy-groups:\" in e[\"fields\"][0][\"value\"] and e[\"fields\"][2][\"kind\"] == \"secret\" and e[\"fields\"][2][\"optional\"]"'
ok "提问会话：配置本机 Clash：地址写错、连不上都带着原因重问，连不上时不留配置；密钥不进事件文件和命令行" 'evq "e.get(\"seq\") == 3 and e[\"fields\"][1][\"error\"]" && evq "e.get(\"seq\") == 4 and \"连不上 unix:/tmp/cca-none.sock\" in e[\"fields\"][1][\"error\"] and e[\"fields\"][1][\"value\"] == \"unix:/tmp/cca-none.sock\"" && ! cfgok "c[\"local\"].get(\"client\")" && ! grep -qF CLASH-SECRET-SENTINEL "$T/ev.jsonl" env.log py.count'
out=$(sess '{"local.client":[{"v":"1"}]}' -- cca local setup 2>&1); rc=$?
ok "提问会话：配置本机 Clash：选内置代理：配好（有结果事件）" '[ $rc -eq 0 ] && cfgok "c[\"local\"][\"client\"] == \"builtin\"" && evq "e[\"ev\"] == \"result\" and e[\"code\"] == \"local_ready\"" && [ -f "$CD/up" ]'
# 换回 Clash Verge（cca local setup）：先停掉内置代理的内核（它还占着 127.0.0.1 的端口），删掉它的配置和拷来的节点
mv "$V.off" "$V"; rm -f "$T/verge/down"
out=$(run CCA_TTY="$(ans y)" cca local setup 2>&1)
ok "从内置代理换回 Clash Verge：先停掉内置代理的内核、删掉它的配置和拷来的节点，再配置 Clash Verge" '[ ! -f "$CD/up" ] && [ ! -e "$CN/config.yaml" ] && [ ! -e "$CN/nodes.json" ] && cfgok "c[\"local\"][\"client\"] == \"clash-verge\"" && printf "%s" "$out" | grep -q "现在配置 Clash Verge 吗"'
# 还原：配置、账号表、映射还原，重新配置 Clash Verge
core_stop; rm -rf "$GA" "$CN" "$H/.cca/policy"; mv "$T/policy.core" "$H/.cca/policy"
cp "$T/config.core" "$H/.cca/config.json"; cp "$T/accounts.core" "$H/.cca/accounts.json"
run CCA_TTY="$(ans y)" cca local setup >/dev/null 2>&1; "$T/verge_regen"

# 11c. 一键安装（install.sh）：全新的 HOME，下载最新版、验签、安装
H3="$T/home3"; mkdir -p "$H3"
sed "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" "$D/../install.sh" > "$T/install.sh"
r3() { env -i LANG=en_US.UTF-8 HOME="$H3" CCA_T="$T" CCA_POLICY_DEFAULT= SHELL=/bin/zsh PATH="$T/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
publish 9.9.20
out=$(r3 CCA_TTY="$(ans y n n)" CCA_POLICY_REPO=example/cca-policy bash "$T/install.sh" 2>&1); rc=$?
ok "一键安装时带 CCA_POLICY_REPO：配好映射仓库、生成这台机器的 deploy key，并拉到映射；仓库不是默认的那个时，给维护者的命令带上 -R" '[ -f "$H3/.cca/policy/id_ed25519" ] && [ -f "$H3/.cca/policy/policy.json" ] && printf "%s" "$out" | grep -q "映射仓库：example/cca-policy" && printf "%s" "$out" | grep -q "已更新映射" && printf "%s" "$out" | grep -q "./policy.sh key add -R example/cca-policy（"'
ok "一键安装：下载最新版、验签、装到 ~/.cca/bin 并链接" '[ $rc -eq 0 ] && [ "$("$H3/.cca/bin/cca" version)" = 9.9.20 ] && [ "$(readlink "$H3/.local/bin/cca")" = "$H3/.cca/bin/cca" ] && [ "$(readlink "$H3/.local/bin/cxa")" = cca ] && printf "%s" "$out" | grep -q "已下载 cca 9.9.20，签名校验通过"'
ok "一键安装：~/.local/bin 不在 PATH 里时，问过之后写进 ~/.zshrc" 'grep -qF "export PATH=\"\$HOME/.local/bin:\$PATH\"" "$H3/.zshrc" && printf "%s" "$out" | grep -q "打开一个新的终端标签，输入 cca 就能用"'
out=$(r3 CCA_TTY="$(ans n n)" bash "$T/install.sh" 2>&1); rc=$?
ok "再运行一次：修复/重装，不重复写 PATH" '[ $rc -eq 0 ] && [ "$(grep -c "\.local/bin" "$H3/.zshrc")" = 1 ] && printf "%s" "$out" | grep -q "已经写在"'
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

# 12. 一键还原：移除端口池需要重载（restore 已确认）
cp -R "$T/appsrc/9.9.16/cca.app" "$GA"; "$GA/Contents/MacOS/cca" & GUIPID=$!; sleep 0.3
out=$(run CCA_TTY="$(ans restore)" cca restore 2>&1); rc=$?
kill $GUIPID 2>/dev/null; wait $GUIPID 2>/dev/null
ok "restore：图形界面 App 开着时不还原（先退出它）" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "cca 的界面 App" && [ -d "$GA" ] && [ -d "$H/.cca" ]'
run CCA_TTY="$(ans "$R1")" cca key alice >/dev/null 2>&1; run CCA_TTY="$(ans "$R2")" cca codex key dave >/dev/null 2>&1
refs=$(ls "$T/keychain" | grep -cE "^cca-(codex-)?ref-")
K=$(cat "$H/.cca/router_ed25519.pub"); echo "$K" >> "$R/root/.ssh/authorized_keys"
out=$(run CCA_TTY="$(ans restore)" cca restore 2>&1); rc=$?
ok "restore：端口池、钩子、cron、文件、公钥全部清掉，覆写脚本逐字节一致" '[ $rc -eq 0 ] && [ "$(nl_)" = 0 ] && ! ls "$R"/sel/CCA-* >/dev/null 2>&1 && cmp -s pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ ! -e "$R/etc/cca" ] && ! grep -q cca "$R/etc/crontabs/root" && [ "$(cat "$R/etc/sysupgrade.conf")" = "/root/.ssh/" ] && ! grep -qF "$K" "$R/root/.ssh/authorized_keys"'
ok "restore：启动器和图形界面 App 全部删掉（确认清单里说了会删 App）" '[ -z "$(ls "$H/Applications" 2>/dev/null)" ] && printf "%s" "$out" | grep -q "还有图形界面 $GA"'
ok "restore：Clash Verge 全局扩展脚本里 cca 的那一段删掉（逐字节还原），本机端口池移除" 'cmp -s "$VS" "$T/pristine_script.js" && ! grep -qx 7901 "$T/verge/listening"'
ok "restore：本机还原（cca、cxa 都删掉），共享主目录内容保留" '[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ ! -L "$H/.local/bin/cxa" ] && [ -f "$H/.claude/projects/-p1/memory/alice.md" ]'
ok "restore：第三方 API 账号的 key、官方账号记下备查的 key（Claude、Codex 各一把）都从钥匙串删掉；配置备份只有自己能读（账号表里可能写着 key）" '[ "$refs" = 2 ] && [ -z "$(ls "$T/keychain" | grep -E "^cca-(codex-)?(api|ref)-")" ] && [ "$(stat -f %Lp "$(ls "$H"/cca-backup-*.tar.gz | head -n 1)")" = 600 ]'
BK=$(ls "$H"/cca-backup-*.tar.gz | head -n 1)
ok "restore：先说明 Codex 账号的 git 工作树会一起删；退出 Codex 账号的登录；备份里没有登录凭证和工作树" 'printf "%s" "$out" | grep -qE "git 工作树（worktrees）会一起删掉：(.*、)?dave@example.com" && grep -q "LOGOUT $DD/codex" codex.log && ! tar -tzf "$BK" | grep -qE "auth\.json|/worktrees|/id_ed25519$" && ! tar -xzOf "$BK" 2>/dev/null | grep -qF at-SECRET'
ok "restore：Codex 账号也一起还原：第三方 API 的 key、ChatGPT 启动器删掉，~/.codex 原来的登录和共享的内容不动" '[ -z "$(ls "$T/keychain" | grep "^cca-codex-api-")" ] && grep -q master "$X/auth.json" && [ -d "$X/skills/from-alice" ] && [ ! -e "$H/Applications/ChatGPT dave.app" ]'
ok "所有重载前都用 OpenClash 的 SAFE_PATHS 做了 -t 校验" 'grep -q "CORE SAFE_PATHS=/usr/share/openclash:/etc/ssl -t" router.log'
echo "---- $pass passed, $fail failed"
