#!/bin/bash
# 全流程回归：端口池（槽位 + 策略组，API 切换不重载）、重载需同意、账号表、出口检测、登录校验、第三方 API 账号、
# 订阅用量、Codex 账号、自动更新、还原
# 用法：bash tests/regress.sh（只在 macOS 上跑；全部用假命令，不联网、不碰真实的 ~/.cca 和路由器）
D=$(cd "$(dirname "$0")" && pwd)
CCA="$D/../cca"; CCA=$(cd "$(dirname "$CCA")" && pwd)/cca
T=$(mktemp -d "${TMPDIR:-/tmp}/cca-test.XXXXXX") && T=$(cd "$T" && pwd -P); H=$T/home; R=$T/router   # 规整成真实路径（TMPDIR 结尾有 / 时会出现 //）
trap '[ -n "${KEEP:-}" ] && echo "沙箱保留在 $T" || rm -rf "$T"' EXIT
bash "$D/setup.sh" "$T" >/dev/null || exit 1
cd "$T" || exit 1
run() { env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_POLICY_DEFAULT= CCA_CLAUDE_APP="$T/Claude.app" CCA_CODEX_APP="$T/ChatGPT.app" PATH="$T/bin:$H/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" TERM=dumb "$@"; }
rr() { env CCA_ROOT="$R" CCA_T="$T" CCA_WAIT=4 HOME="$R/root" PATH="$T/rbin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" /bin/dash "$R/etc/cca/cca-router.sh" "$@"; }
ans() { f="$T/ans.$RANDOM$RANDOM"; printf '%s\n' "$@" > "$f"; echo "$f"; }
pass=0; fail=0
ok() {   # ok 说明 '断言'：断言写丢了（比如说明和断言之间少了空格，连成了一个参数）也算失败，不会空过
	if [ $# -eq 2 ] && [ -n "$2" ] && eval "$2"; then echo "PASS  $1"; pass=$((pass + 1)); else echo "FAIL  $1   [${2:-没有断言}]"; fail=$((fail + 1)); fi
}
edit_table() { python3 -c "import json,sys; p=sys.argv[1]; j=json.load(open(p)); $1; json.dump(j,open(p,'w'),indent=2,ensure_ascii=False)" "${2:-$H/.cca/accounts.json}"; }   # [账号表]
RC="$R/etc/openclash/custom.yaml"
nl_() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); print((c["listeners"]||[]).count { |l| l["name"].start_with?("cca-") })' "$RC"; }
first() { ruby -ryaml -e 'c=YAML.load_file(ARGV[0]); g=(c["proxy-groups"]||[]).find { |x| x["name"]==ARGV[1] }; print(g ? g["proxies"][0] : "-")' "$RC" "$1"; }
sel() { cat "$R/sel/$1" 2>/dev/null || echo -; }
reloads() { grep -c '^RELOAD' router.log 2>/dev/null || echo 0; }
restarts() { grep -c '^RESTART' router.log 2>/dev/null || echo 0; }   # OpenClash 重启（假的 init 脚本记的）
cfgv() { python3 -c "import json,sys; c=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))" "$H/.cca/config.json" "$1"; }
cfgok() { python3 -c "import json,sys; c=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)" "$H/.cca/config.json" "$1"; }   # config.json（c）满足条件
tblok() { python3 -c "import json,sys; t=json.load(open(sys.argv[1])); sys.exit(0 if eval(sys.argv[2]) else 1)" "${2:-$H/.cca/accounts.json}" "$1"; }   # 账号表（t）满足条件
claude_device() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["userID"] + ":" + d["machineID"])' "$1/claude/.claude.json"; }
ok "设备标识：旧配置迁移、共享链接、客户端重置、改别名和并发初始化" 'python3 "$D/device_identity.py"'
ok "共享资料：历史记忆配置跨账号可见，未知状态隔离，配置合并和冲突副本保留" 'python3 "$D/shared_state.py"'

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
m0=$(stat -f %m "$L/Claude 副号.app/Contents/Resources/AppIcon.icns"); sleep 1; run cca app --launchers >/dev/null 2>&1
ok "启动器图标：账号标识没变时不重画" '[ "$(stat -f %m "$L/Claude 副号.app/Contents/Resources/AppIcon.icns")" = "$m0" ]'
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1); rc=$?
ok "cca app：账号自己的资料目录、端口和 CLAUDE_CONFIG_DIR，启动前做出口检测" '[ $rc -eq 0 ] && grep -qF -- "[-n] [-a] [$T/Claude.app] [--env] [CLAUDE_CONFIG_DIR=$BD/claude]" open.log && grep -qF -- "[--args] [--user-data-dir=$BD/desktop] [--proxy-server=http://192.0.2.10:7902] [--proxy-bypass-list=localhost;127.0.0.1;[::1];*.local;10.0.0.0/8;172.16.0.0/12;192.168.0.0/16]" open.log && grep -q "CURL via :7902 https://ip.net.coffee/api/iprisk/203.0.113.2" curl.log && printf "%s" "$out" | grep -q "已打开 副号（bob@example.com） 的 Claude App，出口 US-B（端口 7902）" && printf "%s" "$out" | grep -q "第一次打开要登录 bob@example.com"'
mkdir -p "$T/fakerun/Contents/MacOS"; printf '#!/bin/bash\nsleep 30\n' > "$T/fakerun/Contents/MacOS/Claude"; chmod 755 "$T/fakerun/Contents/MacOS/Claude"
"$T/fakerun/Contents/MacOS/Claude" --user-data-dir="$BD/desktop" --proxy-server=x & FAKEPID=$!
sleep 0.5; n_open=$(grep -c "user-data-dir=$BD/desktop" open.log)
out=$(run CCA_TTY="$(ans)" cca app 副号 2>&1)
ok "cca app：这个账号的已经开着，只切到它，不开第二份" 'printf "%s" "$out" | grep -q "副号 的 Claude App 已经开着，已切到前面" && [ "$(grep -c "user-data-dir=$BD/desktop" open.log)" = "$n_open" ]'
out=$(run CCA_TTY="$(ans y)" cca rm 副号 2>&1); rc=$?
ok "cca rm：这个账号的 Claude App 开着时不删" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "Claude App 还开着" && grep -q "副号(bob@example.com)" "$H/.cca/accounts.json"'
kill $FAKEPID 2>/dev/null; wait $FAKEPID 2>/dev/null
touch "$T/open_noprofile"
out=$(run CCA_TTY="$(ans)" cca app alice 2>&1); rc=$?
ok "cca app：Claude App 不认独立资料目录时关掉并说明" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "没有用 alice@example.com 自己的资料目录启动"'
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
run CCA_TTY="$(ans y)" cca router sync >/dev/null 2>&1
ok "端口池扩容：同意后重载一次，变成 16 个槽位" '[ "$(nl_)" = 16 ] && [ "$(reloads)" = 2 ] && [ "$(sel CCA-9)" = US-D ]'

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
P1="--add-dir=$H/.cca --disallowedTools=Read(/$H/.cca/policy/id_ed25519),Edit(/$H/.cca/policy/id_ed25519),Read(/$H/.cca/codex/accounts/*/codex/auth.json),Edit(/$H/.cca/codex/accounts/*/codex/auth.json),Read(/$H/.cca/accounts/*/chrome/**),Edit(/$H/.cca/accounts/*/chrome/**),Read(/$H/.cca/accounts/*/desktop/**),Edit(/$H/.cca/accounts/*/desktop/**),Read(/$H/.cca/codex/accounts/*/chrome/**),Edit(/$H/.cca/codex/accounts/*/chrome/**),Read(/$H/.cca/codex/accounts/*/desktop/**),Edit(/$H/.cca/codex/accounts/*/desktop/**)"
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
ok "cca usage：两个官方账号一起查，表里有套餐（原来的 Max 5x 现在就叫 Max）、5 小时和本周用量、分模型额度；注明第三方 API 账号不查" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qE "alice@example.com +Max +13%  2 小时 1[0-9] 分后重置 +41%  周. [0-9:]+ 重置" && printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -q "Sonnet 本周 60%" && printf "%s" "$out" | grep -q "第三方 API 账号没有订阅用量，不查：kimi"'
ok "cca usage：每个账号经自己的端口、用自己的配置目录无界面查询（--safe-mode），查之前先看出口地区" 'grep -q "^ARGS: -p --safe-mode --no-session-persistence --strict-mcp-config --input-format stream-json --output-format stream-json --verbose$" claude.log && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=alice@example.com https_proxy=http://192.0.2.10:7901" && grep "^USAGE: " claude.log | tail -n 2 | grep -q "email=bob@example.com https_proxy=http://192.0.2.10:7902" && grep -q "CURL via :7902 https://claude.ai/cdn-cgi/trace" curl.log'
out=$(run cca usage --json 2>/dev/null)
ok "cca usage --json：stdout 只有 JSON（套餐、各窗口、数据来源、跳过的第三方 API 账号）" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); a={x[\"account\"]: x for x in d[\"accounts\"]}; sys.exit(0 if a[\"alice@example.com\"][\"five_hour\"][\"utilization\"]==13 and a[\"alice@example.com\"][\"plan\"]==\"Max\" and a[\"bob@example.com\"][\"source\"]==\"live\" and a[\"bob@example.com\"][\"seven_day_sonnet\"][\"utilization\"]==60 and d[\"skipped_api_accounts\"]==[\"kimi\"] and \"reset_credits\" not in a[\"alice@example.com\"] else 1)"'
ok "cca usage：查到的存进账号目录的 usage.json（菜单和查不到时用）" '[ -f "$AD/usage.json" ]'
edit_table 'j["副号(bob@example.com)"] = j.pop("bob@example.com")'
out=$(run cca usage 2>&1)
edit_table 'j["bob@example.com"] = j.pop("副号(bob@example.com)")'
ok "cca usage：账号一列和菜单一样写别名和邮箱，两小列对齐（没起别名的别名那格留空）" 'printf "%s" "$out" | grep -qE "^  副号  bob@example.com +Pro +98%" && printf "%s" "$out" | grep -qE "^ {8}alice@example.com +Max +13%"'
m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "菜单：官方账号附上最近一次的用量（读缓存，不联网）" 'printf "%s" "$m" | grep -q "5h 13% · 周 41%" && printf "%s" "$m" | grep -q "5h 98% · 周 72%"'
cp "$R/listening" "$T/listening.bak"; grep -vx 7902 "$T/listening.bak" > "$R/listening"; canned bob@example.com 50 50 pro
out=$(run cca usage 2>&1)
ok "出口端口不通：显示上次的缓存（注明多久前）和原因，不拿账号走别的路" 'printf "%s" "$out" | grep -qE "bob@example.com +Pro +98%" && printf "%s" "$out" | grep -qE "缓存（[0-9]+ 分钟前）：出口端口 7902 不通"'
cp "$T/listening.bak" "$R/listening"
touch "$T/usage/unsupported"
out=$(run cca usage alice 2>&1)
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
run CCA_TTY="$(ans y)" cca rm bob >/dev/null 2>&1
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
ok "这个版本的 codex 不支持经 app-server 登录：退回 codex login（说明登录页走系统默认浏览器）" '[ $rc -eq 3 ] && grep -q "LOGIN-CLI $CH" codex.log && printf "%s" "$out" | grep -q "改用 codex login"'
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
out=$(run cca codex usage 2>&1); rc=$?
ok "cca codex usage：经账号的端口让 codex 的 app-server 查（不发模型请求），表里有套餐（叫法和 codex 的 /status 一样，pro 是 Pro 200）、5 小时、本周，备注里先写剩余的重置次数，再是别的额度和 credits；第三方 API 账号不查" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -qE "alice@example.com +Pro 200 +13%  2 小时 1[0-9] 分后重置 +41%  周. [0-9:]+ 重置" && printf "%s" "$out" | grep -q "剩余重置 2 次；GPT-5-Pro 本周 30%；credits 余额 12.5" && printf "%s" "$out" | grep -q "第三方 API 账号没有订阅用量，不查：kimi" && grep -q "RPC rateLimits email=alice@example.com https_proxy=http://192.0.2.10:7902" codex.log && [ "$(grep -c "^USAGE: " claude.log)" = "$n_usage" ]'
out=$(run cca codex usage --json 2>/dev/null)
ok "cca codex usage --json：注明 tool=codex，窗口和 Claude 的一样写法；credits、reset_credits（剩余重置次数和明细）是 codex 原样给的" 'printf "%s" "$out" | python3 -c "import json,sys; d=json.load(sys.stdin); a=d[\"accounts\"][0]; sys.exit(0 if d[\"tool\"]==\"codex\" and a[\"plan\"]==\"Pro 200\" and a[\"five_hour\"][\"utilization\"]==13 and a[\"seven_day\"][\"utilization\"]==41 and a[\"credits\"][\"balance\"]==\"12.5\" and a[\"reset_credits\"][\"availableCount\"]==2 and len(a[\"reset_credits\"][\"credits\"])==2 and a[\"source\"]==\"live\" else 1)"'
out=$(run cca usage 2>&1)
ok "cca usage 只查 Claude 账号，不带 Codex 的" '! printf "%s" "$out" | grep -q "Pro" && printf "%s" "$out" | grep -q "alice@example.com"'
m=$(run CCA_TTY="$(ans q)" cca codex 2>&1)
ok "cca codex 菜单：附上最近一次的用量" 'printf "%s" "$m" | grep -q "5h 13% · 周 41%"'
touch "$T/codex_usage/unsupported"
out=$(run COLUMNS=200 cca codex usage alice 2>&1)
rm -f "$T/codex_usage/unsupported"
ok "codex 不支持查用量：提示升级，用缓存顶上（套餐、剩余重置次数也取缓存里的）" 'printf "%s" "$out" | grep -q "这个版本的 Codex 查不了用量" && printf "%s" "$out" | grep -qE "alice@example.com +Pro 200 +13%" && printf "%s" "$out" | grep -q "升级后再试）；剩余重置 2 次；GPT-5-Pro"'
touch "$T/codex_usage/hang"; t0=$(date +%s)
out=$(run CCA_USAGE_TIMEOUT=2 cca codex usage alice 2>&1); t1=$(date +%s)
rm -f "$T/codex_usage/hang"
ok "codex 查用量卡住：到时间就结束（不会一直等），显示缓存" 'printf "%s" "$out" | grep -q "查询超时" && [ $((t1 - t0)) -lt 15 ]'
ccanned alice@example.com 13 41 promax 0; out=$(run cca codex usage alice 2>&1)
ccanned alice@example.com 13 41 prolite; out2=$(run cca codex usage alice 2>&1)
ok "套餐叫法和 codex 的一样：promax 是 Pro 500、prolite 是 Pro 100；剩余 0 次也写上，codex 没给重置次数（旧版本）就不写" 'printf "%s" "$out" | grep -qE "alice@example.com +Pro 500 +13%" && printf "%s" "$out" | grep -q "剩余重置 0 次；GPT-5-Pro" && printf "%s" "$out2" | grep -qE "alice@example.com +Pro 100 +13%" && printf "%s" "$out2" | grep -q "GPT-5-Pro 本周 30%" && ! printf "%s" "$out2" | grep -q "剩余重置"'
ok "Codex 的 access / refresh token 从没出现在 cca 写的文件、日志和命令行里（cca 只从 id_token 读邮箱和套餐）" '! grep -rqF -e at-SECRET -e rt-SECRET --exclude auth.json "$H/.cca" && ! grep -qF -e at-SECRET -e rt-SECRET "$T/env.log" codex.log curl.log security.log open.log'
# 网页版、桌面 App 和启动器
: > open.log
run CCA_TTY="$(ans)" cca codex web alice >/dev/null 2>&1
ok "cca codex web：用账号专属的浏览器打开 chatgpt.com" 'grep -F "https://chatgpt.com/" open.log | grep -qF -- "--user-data-dir=$CD/chrome"'
L="$H/Applications"
ok "启动器：Codex 账号是「ChatGPT 别名」，和 Claude 账号的「Claude 别名」各管各的" '[ -x "$L/ChatGPT alice.app/Contents/MacOS/cca-launcher" ] && grep -q "codex app" "$L/ChatGPT alice.app/Contents/MacOS/cca-launcher" && [ -d "$L/Claude alice.app" ] && plutil -extract CFBundleIdentifier raw "$L/ChatGPT alice.app/Contents/Info.plist" | grep -q "^local.cca.launcher.codex.alice-example-com-"'
run cca app --launchers >/dev/null 2>&1; run cca codex app --launchers >/dev/null 2>&1
ok "启动器：两边各自重新生成时不删对方的" '[ -d "$L/ChatGPT alice.app" ] && [ -d "$L/Claude alice.app" ]'
out=$(run cca codex app alice 2>&1 </dev/null); rc=$?
ok "ChatGPT App：共享的 config.toml 里还没关 system_proxy_fallback、又不能问时：不打开，说明要在终端里运行一次" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "system_proxy_fallback"'
: > open.log
out=$(run CCA_TTY="$(ans y)" cca codex app alice 2>&1); rc=$?
ok "ChatGPT App：问过之后用 codex 自己的命令关掉 system_proxy_fallback（写进共享的 ~/.codex/config.toml）" '[ $rc -eq 0 ] && grep -q "FEATURES disable system_proxy_fallback @ $X" codex.log && grep -q "system_proxy_fallback = false" "$X/config.toml" && [ ! -L "$X/config.toml" ]'
ok "ChatGPT App：账号自己的 CODEX_HOME、资料目录（CODEX_ELECTRON_USER_DATA_PATH + --user-data-dir）和端口" 'grep -qF -- "[-n] [-a] [$T/ChatGPT.app] [--env] [CODEX_HOME=$CH] [--env] [CODEX_SQLITE_HOME=$X] [--env] [CODEX_ELECTRON_USER_DATA_PATH=$CD/desktop]" open.log && grep -qF -- "[--args] [--user-data-dir=$CD/desktop] [--proxy-server=http://192.0.2.10:7902]" open.log && printf "%s" "$out" | grep -q "已打开 alice@example.com 的 ChatGPT App，出口 US-B（端口 7902）"'
ok "ChatGPT App 第一次打开：说明 App 里要求登录时别在 App 里登，改用 cca codex login" 'printf "%s" "$out" | grep -q "运行 cca codex login alice@example.com，再打开 App"'
cp "$T/fakerun/Contents/MacOS/Claude" "$T/fakerun/Contents/MacOS/ChatGPT"   # 和上面假的 Claude App 进程一样，换个名字
"$T/fakerun/Contents/MacOS/ChatGPT" --user-data-dir="$CD/desktop" --x & FAKEPID=$!
sleep 0.5; n_open=$(grep -c "user-data-dir=$CD/desktop" open.log)
out=$(run CCA_TTY="$(ans)" cca codex app alice 2>&1)
out2=$(run CCA_TTY="$(ans y)" cca codex rm alice 2>&1); rc2=$?
kill $FAKEPID 2>/dev/null; wait $FAKEPID 2>/dev/null
ok "ChatGPT App：这个账号的已经开着就切过去；开着时不能删账号" 'printf "%s" "$out" | grep -q "alice@example.com 的 ChatGPT App 已经开着" && [ "$(grep -c "user-data-dir=$CD/desktop" open.log)" = "$n_open" ] && [ $rc2 -ne 0 ] && printf "%s" "$out2" | grep -q "ChatGPT App 还开着"'
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

# 11. 文字和编码
out=$(run cca router sync 2>&1); m=$(run CCA_TTY="$(ans q)" cca 2>&1)
ok "UTF-8 下中文提示完整" 'printf "%s" "$out" | grep -q "正在同步账号到路由器 192.0.2.10" && printf "%s" "$m" | grep -q "继续使用 alice@example.com" && printf "%s%s" "$out" "$m" | python3 -I -c "import sys; sys.stdin.buffer.read().decode(\"utf-8\")"'
ok "lint：cca、install.sh、release.sh 和 policy.sh 里没有 \$变量 紧跟非 ASCII 字符" '[ "$(python3 -I -c "import re,sys; print(sum(len(re.findall(r\"\\\$[A-Za-z_][A-Za-z0-9_]*(?=[^\\x00-\\x7f])\", open(f, encoding=\"utf-8\").read())) for f in sys.argv[1:]))" "$CCA" "$D/../install.sh" "$D/../release.sh" "$D/../policy.sh")" = 0 ]'
out=$(run cca router status 2>&1)
ok "router status：列出槽位、端口和选中节点" 'printf "%s" "$out" | grep -q "槽位 CCA-1 :7901 监听中 → US-A" && printf "%s" "$out" | grep -q "槽位 CCA-2 :7902 监听中 → REJECT"'

# 11b. 自动更新：假的 GitHub Releases + 测试用发布密钥
G="$T/gh"; mkdir -p "$G"
ssh-keygen -q -t ed25519 -N '' -C test-release -f "$T/relkey"; ssh-keygen -q -t ed25519 -N '' -f "$T/evilkey"
TESTPUB=$(cut -d' ' -f1,2 "$T/relkey.pub")
mkver() {   # mkver 版本 输出文件 [路由器脚本标记]：换成测试密钥、改版本号，可在路由器脚本里加个标记
	sed -e "s|^VERSION=\".*\"$|VERSION=\"$1\"|" -e "s|^RELEASE_KEY=\".*\"$|RELEASE_KEY=\"$TESTPUB test-release\"|" "$CCA" > "$2"
	[ -n "${3:-}" ] && sed -i '' "s|^# cca-router：|# cca-router（$3）：|" "$2"
	chmod 755 "$2"
}
publish() {   # publish 版本 [路由器脚本标记] [签名密钥]；测试版（-beta.N）像 GitHub 的 pre-release，不成为 latest
	mkdir -p "$G/v$1"; mkver "$1" "$G/v$1/cca" "${2:-}"; rm -f "$G/v$1/cca.sig"
	ssh-keygen -Y sign -q -n cca-release -f "${3:-$T/relkey}" "$G/v$1/cca" 2>/dev/null
	case "$1" in *-beta.*) ;; *) echo "$1" > "$G/latest" ;; esac
}
wait_next() { local i; for i in $(seq 1 50); do [ -f "$H/.cca/update/cca.next" ] && [ ! -d "$H/.cca/update/lock" ] && return 0; sleep 0.2; done; return 1; }
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
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_next
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
run cca update 9.9.14-beta.1 >/dev/null 2>&1; publish 9.9.14
rm -f "$H/.cca/update/checked"; run cca ls >/dev/null 2>&1; wait_next
out=$(run CCA_TTY="$(ans)" cxa @dave --version 2>&1); rc=$?
ok "装了测试版之后，出了更新的正式版照常自动更新；这次是用 cxa 启动的，换上新版本重新执行后启动的照样是 Codex" 'printf "%s" "$out" | grep -q "cca 已自动更新：9.9.14-beta.1 → 9.9.14" && [ "$(ver)" = 9.9.14 ] && [ $rc -eq 3 ] && clast ARGS | grep -q -- " --version$" && clast ENV | grep -q "CODEX_HOME=$DD/codex "'

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
p=$(cfgv 'c["ports"]["Erin(erin@example.com)"]')
out=$(run CCA_TTY="$(ans)" cca @erin --version 2>&1); rc=$?
ok "加进来的账号照常启动：先用它的邮箱登录，路由器同步到映射的节点" '[ $rc -eq 3 ] && grep -q "ARGS: auth login --claudeai --email erin@example.com" claude.log && [ "$(sel "CCA-$((p - 7900))")" = US-D ]'
out=$(run CCA_TTY="$(ans y)" cca rm frank 2>&1); rc=$?
out2=$(run cca ls 2>&1); out3=$(run cca policy 2>&1)
ok "删掉映射里的账号：记下来，以后不再自动加回来；cca policy 里注明怎么加回来" '[ $rc -eq 0 ] && printf "%s" "$out" | grep -q "frank@example.com 在维护者的映射里：这台机器上不会再自动加回来（要加回来：cca add）" && ! grep -q frank "$H/.cca/accounts.json" && ! printf "%s" "$out2" | grep -q "按维护者的映射添加了" && cfgok "c.get(\"policy_skip\") == [\"frank@example.com\"]" && printf "%s" "$out3" | grep -q "Claude  kimi（frank@example.com） → US-B：这台机器上删掉过（要加回来：cca add）"'
out=$(run CCA_TTY="$(ans frank@example.com '')" cca add 2>&1); rc=$?
ok "用 cca add 加回来：出口直接用映射的节点，不让选；以后照常跟着映射" '[ $rc -eq 0 ] && tblok "t.get(\"frank@example.com\") == \"US-B\"" && printf "%s" "$out" | grep -q "frank@example.com 的出口按维护者的映射固定为 US-B" && ! printf "%s" "$out" | grep -q "选择节点编号" && cfgok "\"policy_skip\" not in c"'
out=$(run CCA_TTY="$(ans)" EDITOR="$T/edit.py" CCA_TEST_EDIT='j.pop("frank@example.com")' cca edit 2>&1)
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

# 11b3. 维护者管理各台机器的 deploy key（policy.sh key）：假 gh 模拟 GitHub 的 deploy key 接口（存在 $T/deploy_keys.json），
# 有 $T/policy_keys 时假 git 按它认 deploy key（没加上就拒绝，认出来了记下最近使用）
PS="$D/../policy.sh"; touch "$T/policy_keys"; : > ghcli.log
pks() { local a="$1"; shift; run $P CCA_TTY="$a" bash "$PS" key "$@"; }   # pks 答案文件 子命令…
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
out=$(printf '这是我的公钥：%s 谢谢\n' "$(cat "$T/carol.pub")" | run $P bash "$PS" key add - "Carol 的 Air" 2>&1); rc=$?
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
out=$(run CCA_TTY="$(ans)" bash "$PS" key 2>&1); rc=$?
out2=$(run CCA_TTY="$(ans)" bash "$PS" key -R example/other 2>&1); rc2=$?
: > ghcli.log; env -i LANG=en_US.UTF-8 HOME="$H" CCA_T="$T" CCA_TTY="$(ans)" PATH="$T/bin:/usr/bin:/bin" bash "$PS" key >/dev/null 2>&1
ok "仓库：CCA_POLICY_DEFAULT 设成空、又没写 -R 时不猜；-R 指定别的仓库；都没设时用 cca 里写的默认仓库" '[ $rc -ne 0 ] && printf "%s" "$out" | grep -q "不知道是哪个映射仓库：加上 -R 维护者/仓库" && [ $rc2 -eq 0 ] && printf "%s" "$out2" | grep -q "example/other 上还没有 deploy key" && [ -n "$defrepo" ] && grep -q "^GH api GET repos/$defrepo/keys" ghcli.log'
rm -f "$T/policy_keys"

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
out=$(r3 CCA_TTY="$(ans n n)" bash "$T/install.sh" 9.9.23-beta.1 2>&1); rc=$?
ok "一键安装默认装最新的正式版；指定版本号时装那个测试版，换下来的正式版留着" '[ $rc -eq 0 ] && [ "$("$H3/.cca/bin/cca" version)" = 9.9.23-beta.1 ] && printf "%s" "$out" | grep -q "正在下载 cca 9.9.23-beta.1" && [ "$(cat "$H3/.cca/update/cca.prev.version")" = 9.9.22 ]'
out=$(r3 "$H3/.cca/bin/cca" update --rollback 2>&1)
ok "一键安装装上的测试版也能退回" '[ "$("$H3/.cca/bin/cca" version)" = 9.9.22 ] && printf "%s" "$out" | grep -q "已退回 9.9.22"'

# 12. 一键还原：移除端口池需要重载（restore 已确认）
K=$(cat "$H/.cca/router_ed25519.pub"); echo "$K" >> "$R/root/.ssh/authorized_keys"
out=$(run CCA_TTY="$(ans restore)" cca restore 2>&1); rc=$?
ok "restore：端口池、钩子、cron、文件、公钥全部清掉，覆写脚本逐字节一致" '[ $rc -eq 0 ] && [ "$(nl_)" = 0 ] && ! ls "$R"/sel/CCA-* >/dev/null 2>&1 && cmp -s pristine_overwrite.sh "$R/etc/openclash/custom/openclash_custom_overwrite.sh" && [ ! -e "$R/etc/cca" ] && ! grep -q cca "$R/etc/crontabs/root" && [ "$(cat "$R/etc/sysupgrade.conf")" = "/root/.ssh/" ] && ! grep -qF "$K" "$R/root/.ssh/authorized_keys"'
ok "restore：启动器全部删掉" '[ -z "$(ls "$H/Applications" 2>/dev/null)" ]'
ok "restore：Clash Verge 全局扩展脚本里 cca 的那一段删掉（逐字节还原），本机端口池移除" 'cmp -s "$VS" "$T/pristine_script.js" && ! grep -qx 7901 "$T/verge/listening"'
ok "restore：本机还原（cca、cxa 都删掉），共享主目录内容保留" '[ ! -e "$H/.cca" ] && [ ! -L "$H/.local/bin/cca" ] && [ ! -L "$H/.local/bin/cxa" ] && [ -f "$H/.claude/projects/-p1/memory/alice.md" ]'
ok "restore：第三方 API 账号的 key 从钥匙串删掉；配置备份只有自己能读（账号表里可能写着 key）" '[ -z "$(ls "$T/keychain" | grep "^cca-api-")" ] && [ "$(stat -f %Lp "$(ls "$H"/cca-backup-*.tar.gz | head -n 1)")" = 600 ]'
BK=$(ls "$H"/cca-backup-*.tar.gz | head -n 1)
ok "restore：先说明 Codex 账号的 git 工作树会一起删；退出 Codex 账号的登录；备份里没有登录凭证和工作树" 'printf "%s" "$out" | grep -qE "git 工作树（worktrees）会一起删掉：(.*、)?dave@example.com" && grep -q "LOGOUT $DD/codex" codex.log && ! tar -tzf "$BK" | grep -qE "auth\.json|/worktrees|/id_ed25519$" && ! tar -xzOf "$BK" 2>/dev/null | grep -qF at-SECRET'
ok "restore：Codex 账号也一起还原：第三方 API 的 key、ChatGPT 启动器删掉，~/.codex 原来的登录和共享的内容不动" '[ -z "$(ls "$T/keychain" | grep "^cca-codex-api-")" ] && grep -q master "$X/auth.json" && [ -d "$X/skills/from-alice" ] && [ ! -e "$H/Applications/ChatGPT dave.app" ]'
ok "所有重载前都用 OpenClash 的 SAFE_PATHS 做了 -t 校验" 'grep -q "CORE SAFE_PATHS=/usr/share/openclash:/etc/ssl -t" router.log'
echo "---- $pass passed, $fail failed"
